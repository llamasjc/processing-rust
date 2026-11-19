//! The libprocessing renderer (Bevy + wgpu), picked at run time with
//! PROCESSING_RENDERER=libprocessing when the crate is built with the
//! `libprocessing` feature.
//!
//! The sketch API stays the same: the render module checks `on()` at the top
//! of each drawing call and hands the call to this module, which records it
//! as a libprocessing DrawCommand. Things libprocessing doesn't have yet are
//! filled in here: pixels and filters go through a readback, images made or
//! edited on the CPU are uploaded when drawn, and layers are offscreen
//! graphics.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::color::{Color as BColor, LinearRgba};
use bevy::math::{Affine3A, Mat3, Vec3, Vec4};
use bevy::prelude::Entity;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use processing_core::config::{Config, ConfigKey};
use processing_glfw::GlfwContext;
use processing_input as input;
use processing_render as pr;
use processing_render::render::command::{
    ArcMode, DrawCommand, ShapeKind, ShapeMode, StrokeCapMode, StrokeJoinMode, TextAlignH, TextAlignV,
};

use crate::Color;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CHECKED: AtomicBool = AtomicBool::new(false);

/// Whether drawing goes to libprocessing. Decided once, from the environment.
pub(crate) fn on() -> bool {
    if !CHECKED.load(Ordering::Relaxed) {
        crate::apply_launch_file();
        let want = std::env::var("PROCESSING_RENDERER").map(|v| v.eq_ignore_ascii_case("libprocessing")).unwrap_or(false);
        ACTIVE.store(want, Ordering::Relaxed);
        CHECKED.store(true, Ordering::Relaxed);
    }
    ACTIVE.load(Ordering::Relaxed)
}

struct Lp {
    glfw: Option<GlfwContext>,
    surface: Entity,
    /// the window's own canvas: each frame, the sketch's canvas is copied
    /// into it. The window can end up bigger than the sketch (a window
    /// manager with a minimum size), the sketch's canvas never does.
    win: Entity,
    /// the sketch's canvas (an offscreen image) and its graphics
    canvas: Entity,
    main: Entity,
    /// where drawing goes right now: the window, or a layer
    target: Entity,
    size: (u32, u32),
    /// engine image id -> (libprocessing image, version uploaded)
    images: HashMap<u64, (Entity, u64)>,
    fonts: HashMap<u64, Entity>,
    /// engine layer id -> (graphics, width, height)
    layers: HashMap<u64, (Entity, u32, u32)>,
    warned: Vec<&'static str>,
}

thread_local! {
    static LP: RefCell<Option<Lp>> = const { RefCell::new(None) };
    /// commands made before the window exists (setup() before size())
    static EARLY: RefCell<Vec<DrawCommand>> = const { RefCell::new(Vec::new()) };
}

fn fail(what: &str, e: impl std::fmt::Debug) {
    eprintln!("libprocessing: {what} failed: {e:?}");
}

/// Says once that something isn't there yet on this renderer.
pub(crate) fn missing(what: &'static str) {
    LP.with(|l| {
        if let Some(l) = l.borrow_mut().as_mut() {
            if !l.warned.contains(&what) {
                l.warned.push(what);
                eprintln!("{what} isn't available on the libprocessing renderer yet");
            }
        }
    });
}

fn init_app(asset_root: &str) -> processing_core::error::Result<()> {
    use bevy::app::App;
    use bevy::asset::{AssetApp, io::AssetSourceBuilder};
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    if processing_core::is_already_init()? { return Ok(()); }
    let mut config = Config::new();
    config.set(ConfigKey::AssetRootPath, asset_root.to_string());
    let mut app = App::new();
    app.insert_resource(config);
    app.register_asset_source("assets_directory", AssetSourceBuilder::platform_default(asset_root, None));
    let plugins = DefaultPlugins
        .build()
        // pipelines are compiled before the frame that needs them: with this
        // off, filters fail with PipelineNotReady and get() reads an empty
        // canvas on the first frames
        .set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() })
        .disable::<bevy::winit::WinitPlugin>()
        // nothing here animates models or reads game pads; both cost every update
        .disable::<bevy::animation::AnimationPlugin>()
        .disable::<bevy::gilrs::GilrsPlugin>()
        .disable::<bevy::log::LogPlugin>()
        .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
        // no sound for now: don't open the sound card
        .disable::<bevy::audio::AudioPlugin>()
        .set(WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() })
        // don't watch the sketch folder for changes: in a big folder, or one
        // with folders we can't read (like /tmp), the watcher fails to start
        .set(bevy::asset::AssetPlugin { watch_for_changes_override: Some(false), ..default() });
    app.add_plugins(plugins);
    // no sound and no MIDI: just input and drawing
    app.add_plugins(processing_input::InputPlugin);
    app.add_plugins(processing_render::ProcessingRenderPlugin);
    app.finish();
    app.cleanup();
    app.update();
    processing_core::set_app(app);
    Ok(())
}

/// size(w, h, P3D): every canvas here can do 3D, so the same as size().
pub(crate) fn size_3d(w: u32, h: u32) { size(w, h); }

thread_local! {
    /// size() asked for before the window exists: it opens after setup(),
    /// at the last size asked for, with what setup() drew replayed into it.
    static PENDING: std::cell::Cell<Option<(u32, u32)>> = const { std::cell::Cell::new(None) };
}

static IN_SETUP: AtomicBool = AtomicBool::new(false);

/// size(): before the sketch starts (a host setting a default) it is only
/// remembered; from setup() on, the first call opens the window.
pub(crate) fn size(w: u32, h: u32) {
    if LP.with(|l| l.borrow().is_none()) {
        PENDING.with(|p| p.set(Some((w, h))));
        if IN_SETUP.load(Ordering::Relaxed) { ensure_open(); }
        return;
    }
    let resized = LP.with(|l| {
        let mut l = l.borrow_mut();
        let Some(l) = l.as_mut() else { return None };
        if l.size != (w, h) {
            if let Err(e) = pr::surface_resize(l.surface, w, h) { fail("resize", e); }
            if let Some((canvas, main)) = make_canvas(w, h) {
                let (old_c, old_m) = (l.canvas, l.main);
                if l.target == old_m { l.target = main; }
                l.canvas = canvas;
                l.main = main;
                l.size = (w, h);
                let _ = pr::graphics_destroy(old_m);
                let _ = pr::surface_destroy(old_c);
                return Some(main);
            }
        }
        None
    });
    // the new canvas picks up where the old one was: begun, in Processing's
    // coordinates, with Processing's gray
    if let Some(main) = resized {
        start_canvas(main);
        corner_modes();
        let (th, tv, ts) = crate::render::text_state();
        text_align(th, tv);
        text_size(ts);
        cmd(DrawCommand::BackgroundColor(BColor::linear_rgb(0.8, 0.8, 0.8)));
    }
}

/// Opens the window now if it isn't open yet: something needs the canvas
/// (reading pixels, a layer, text sizes) before size() was called.
pub(crate) fn ensure_open() {
    if LP.with(|l| l.borrow().is_some()) { return; }
    let (w, h) = PENDING.with(|p| p.take()).unwrap_or((100, 100));
    open(w, h);
}

/// Always X11 (XWayland in Wayland sessions). See Cargo.toml for why.
fn pick_display_system() {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("libprocessing needs X11 (or XWayland in a Wayland session), and DISPLAY isn't set.");
            eprintln!("Install xorg-xwayland, or use the CPU renderer.");
            std::process::exit(1);
        }
        if !glfw::Platform::X11.is_supported() {
            eprintln!("libprocessing can't open a window: the GLFW library here was built without X11.");
            eprintln!("On Arch: sudo pacman -S glfw. Or use the CPU renderer.");
            std::process::exit(1);
        }
        glfw::init_hint(glfw::InitHint::Platform(glfw::Platform::X11));
    }
}

/// The sketch's canvas: an offscreen image of exactly the sketch's size, and
/// its graphics. Always libprocessing's 3D camera, turned to Processing's
/// coordinates (see processing_coords): 2D drawing lands on the same pixels,
/// box() and sphere() get perspective, and translucent fills work
/// (libprocessing's 2D mode drops them for now).
fn make_canvas(w: u32, h: u32) -> Option<(Entity, Entity)> {
    let canvas = match pr::surface_create_offscreen(w, h, 1.0, TextureFormat::Rgba16Float) {
        Ok(s) => s, Err(e) => { fail("making the canvas", e); return None; }
    };
    let main = match pr::graphics_create(canvas, w, h, TextureFormat::Rgba16Float) {
        Ok(g) => g, Err(e) => { fail("making the canvas", e); return None; }
    };
    if let Err(e) = pr::graphics_mode_3d(main) { fail("3D mode", e); }
    default_projection(main, w, h);
    Some((canvas, main))
}
fn start_canvas(main: Entity) {
    SHADING.with(|s| s.set(None));
    DRAWN.store(false, Ordering::Relaxed);
    let _ = pr::graphics_begin_draw(main);
    processing_coords();
}
/// Ends the frame: draws the sketch's canvas, then copies it into the
/// window at the top left.
fn finish_frame() {
    #[cfg(lp_filter_now)]
    if one_update_frame() { return; }
    let Some((main, win, canvas, (w, h))) = LP.with(|l| l.borrow().as_ref().map(|l| (l.main, l.win, l.canvas, l.size))) else { return };
    // Every engine update renders and waits on the GPU, so the sketch's
    // canvas is not finished on its own: it renders in the same update as the
    // window that shows it (the canvas camera goes first), see show_with().
    let merged = to_light_filter().is_some() && canvas_first(main, win);
    if !merged { end_draw_guarded(main); }
    let _ = pr::graphics_begin_draw(win);
    let rec = |c: DrawCommand| { if let Err(e) = pr::graphics_record_command(win, c) { fail("showing the frame", e); } };
    rec(DrawCommand::BackgroundColor(BColor::srgb(0.0, 0.0, 0.0)));
    rec(DrawCommand::ImageMode(ShapeMode::Corner));
    // The window's canvas keeps the size it was made with and is stretched
    // over the whole window, so a window bigger than the sketch would stretch
    // the picture. Shrink it by as much, to show it at its own size.
    let surface = LP.with(|l| l.borrow().as_ref().map(|l| l.surface));
    let (ww, wh) = surface.and_then(|s| processing_core::app_mut(|app| {
        Ok(app.world().get::<bevy::window::Window>(s).map(|win| (win.resolution.width(), win.resolution.height())))
    }).ok().flatten()).unwrap_or((w as f32, h as f32));
    let (fw, fh) = (w as f32, h as f32);
    let (dw, dh) = (fw * fw / ww.max(1.0), fh * fh / wh.max(1.0));
    rec(DrawCommand::Image { entity: canvas, dx: 0.0, dy: 0.0, d_width: Some(dw), d_height: Some(dh), sx: None, sy: None, s_width: None, s_height: None });
    // the canvas holds sRGB numbers; the window wants light (applying a
    // filter flushes what was recorded first, so no flush of our own: each
    // one is a whole engine update)
    if let Some(f) = to_light_filter() {
        // the filter's own flush renders the canvas too when merged
        if merged { show_with(main, true); }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pr::graphics_apply_filter(win, f)));
        if merged { show_with(main, false); }
        match r {
            Ok(Err(e)) => fail("showing the frame", e),
            Err(_) => missing("drawing one of the shapes in this frame (libprocessing couldn't tessellate it)"),
            _ => {}
        }
    }
    end_draw_guarded(win);
}

// With the lp-fork a frame costs one engine update. The update renders the
// sketch's canvas and, in the same go, shows the window's picture of the frame
// before; right after it the "show" filter paints this frame's canvas into
// the window (on the GPU, no update), to be shown by the next update or by
// present_pending() when no next frame comes. So the window is a frame behind
// the canvas, as with any double-buffered display.
#[cfg(lp_filter_now)]
thread_local! {
    static SHOW: std::cell::Cell<Option<Option<Entity>>> = const { std::cell::Cell::new(None) };
    static UNSHOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(lp_filter_now)]
fn show_filter() -> Option<Entity> {
    if let Some(f) = SHOW.with(|t| t.get()) { return f; }
    const SRC: &str = r#"import processing::filter::{sample, sample_texture, FullscreenVertexOutput};

struct ShowParams {
    scale: vec4<f32>,
}

@group(1) @binding(0) var src: texture_2d<f32>;
@group(1) @binding(1) var<uniform> show_params: ShowParams;

fn to_light(v: f32) -> f32 {
    if v <= 0.04045 { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv * show_params.scale.xy;
    if uv.x > 1.0 || uv.y > 1.0 { return vec4<f32>(0.0, 0.0, 0.0, 1.0) + sample(in.uv) * 0.0; }
    let c = sample_texture(src, uv);
    return vec4<f32>(to_light(c.r), to_light(c.g), to_light(c.b), c.a);
}
"#;
    let f = pr::shader_create(SRC).and_then(pr::filter_create);
    if let Err(e) = &f { eprintln!("libprocessing: couldn't make the show filter ({e}); frames take an extra update"); }
    let f = f.ok();
    SHOW.with(|t| t.set(Some(f)));
    f
}

/// One update for the whole frame; false when it can't (then the frame is
/// shown the older way, with nothing changed).
#[cfg(lp_filter_now)]
fn one_update_frame() -> bool {
    use pr::shader_value::ShaderValue;
    let Some((main, win, canvas, (w, h), surface)) = LP.with(|l| l.borrow().as_ref().map(|l| (l.main, l.win, l.canvas, l.size, l.surface))) else { return false };
    let Some(show) = show_filter() else { return false };
    if !canvas_first(main, win) { return false; }
    let (ww, wh) = processing_core::app_mut(|app| {
        Ok(app.world().get::<bevy::window::Window>(surface).map(|win| (win.resolution.width(), win.resolution.height())))
    }).ok().flatten().unwrap_or((w as f32, h as f32));
    let scale = [ww.max(1.0) / w as f32, wh.max(1.0) / h as f32, 0.0, 0.0];
    let first = !UNSHOWN.with(|p| p.get()) && SHOW_RAN.with(|r| !r.get());
    // the update: this frame's canvas, and last frame's picture to the window
    show_with(main, true);
    // The canvas and the window take part in every engine update from now
    // on. An update with the canvas left out (a layer's endDraw()) froze it
    // at its first frame; one with the window left out dropped its picture.
    // The window and shows
    // its picture in each: an update with the window left out (a layer's
    // endDraw(), a pixel read) would drop the picture painted into it.
    UNSHOWN.with(|p| p.set(false));
    show_with_window(win, true);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| processing_core::app_mut(|app| { app.update(); Ok(()) })));
    if r.is_err() { missing("drawing one of the shapes in this frame (libprocessing couldn't tessellate it)"); }
    // then this frame's picture into the window, on the GPU only
    let set = pr::filter_set(show, "src", ShaderValue::Texture(canvas))
        .and_then(|_| pr::filter_set(show, "scale", ShaderValue::Float4(scale)));
    // the first run needs its update: it brings the shader to the GPU
    let shown = set.and_then(|_| if first { pr::graphics_apply_filter(win, show) } else { pr::graphics_apply_filter_now(win, show) });
    // the first one's own flush left the window out of later updates
    if first { show_with_window(win, true); }
    if let Err(e) = shown { fail("showing the frame", e); return true; }
    SHOW_RAN.with(|r| r.set(true));
    UNSHOWN.with(|p| p.set(true));
    true
}

#[cfg(lp_filter_now)]
thread_local! { static SHOW_RAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// Shows the last frame's picture if no update has shown it yet (the sketch
/// stopped drawing: noLoop(), or waiting on redraw()).
pub(crate) fn present_pending() {
    #[cfg(lp_filter_now)]
    if UNSHOWN.with(|p| p.replace(false)) {
        if let Some(win) = LP.with(|l| l.borrow().as_ref().map(|l| l.win)) {
            end_draw_guarded(win);
            // end_draw takes the window out of updates; put it back
            show_with_window(win, true);
        }
    }
}

#[cfg(lp_filter_now)]
fn show_with_window(win: Entity, on: bool) { show_with(win, on); }

// The engine's flush, readback and filter take their graphics out of later
// updates when they finish. With one update a frame the canvas and the window
// must stay in every update (see one_update_frame), so these put them back.
fn flush_keep(g: Entity) -> Result<(), processing_core::error::ProcessingError> { let r = pr::graphics_flush(g); keep_in_updates(); r }
fn readback_keep(g: Entity) -> Result<Vec<LinearRgba>, processing_core::error::ProcessingError> { let r = pr::graphics_readback(g); keep_in_updates(); r }
fn filter_keep(g: Entity, f: Entity) -> Result<(), processing_core::error::ProcessingError> { let r = pr::graphics_apply_filter(g, f); keep_in_updates(); r }
fn keep_in_updates() {
    #[cfg(lp_filter_now)]
    if SHOW_RAN.with(|r| r.get()) {
        // called while LP may already be borrowed (a window resize, say): then
        // the cameras are put back by the next frame's update instead
        if let Some((main, win)) = LP.with(|l| l.try_borrow().ok().and_then(|l| l.as_ref().map(|l| (l.main, l.win)))) {
            show_with(main, true);
            show_with(win, true);
        }
    }
}

/// Makes the canvas camera render before the window's, once; false if the
/// cameras can't be found (then the canvas is finished on its own).
fn canvas_first(main: Entity, win: Entity) -> bool {
    thread_local! { static DONE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) }; }
    if let Some(d) = DONE.with(|d| d.get()) { return d; }
    let ok = processing_core::app_mut(|app| {
        let w = app.world_mut();
        let Some(win_order) = w.get::<bevy::camera::Camera>(win).map(|c| c.order) else { return Ok(false) };
        let Some(mut cam) = w.get_mut::<bevy::camera::Camera>(main) else { return Ok(false) };
        cam.order = win_order - 1;
        // the window only shows the finished canvas: multisampling it would
        // cost a resolve (and a writeback) every frame for nothing
        w.entity_mut(win).insert(bevy::render::view::Msaa::Off);
        Ok(true)
    }).unwrap_or(false);
    DONE.with(|d| d.set(Some(ok)));
    ok
}

/// While on, the canvas renders and writes out its picture in whatever
/// update comes next, the same as its own end_draw would.
fn show_with(main: Entity, on: bool) {
    use bevy::camera::{Camera, CameraOutputMode};
    let _ = processing_core::app_mut(|app| {
        let mut e = app.world_mut().entity_mut(main);
        if on { e.insert(pr::Flush); } else { e.remove::<pr::Flush>(); }
        if let Some(mut cam) = e.get_mut::<Camera>() {
            cam.output_mode = if on {
                CameraOutputMode::Write { blend_state: None, clear_color: bevy::camera::ClearColorConfig::None }
            } else { CameraOutputMode::Skip };
        }
        Ok(())
    });
}

thread_local! { static TO_LIGHT: std::cell::Cell<Option<Option<Entity>>> = const { std::cell::Cell::new(None) }; }
/// A filter that turns sRGB numbers into linear light.
fn to_light_filter() -> Option<Entity> {
    if let Some(f) = TO_LIGHT.with(|t| t.get()) { return f; }
    const SRC: &str = r#"import processing::filter::{sample, FullscreenVertexOutput};

fn to_light(v: f32) -> f32 {
    if v <= 0.04045 { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let c = sample(in.uv);
    return vec4<f32>(to_light(c.r), to_light(c.g), to_light(c.b), c.a);
}
"#;
    let f = pr::shader_create(SRC).and_then(pr::filter_create);
    if let Err(e) = &f { eprintln!("libprocessing: couldn't make the display filter ({e}); colors will look too light"); }
    let f = f.ok();
    TO_LIGHT.with(|t| t.set(Some(f)));
    f
}

/// Opens the window and the canvas.
fn open(w: u32, h: u32) {
    let root = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| ".".into());
    pick_display_system();
    let mut glfw = match GlfwContext::new(w, h, false) {
        Ok(g) => g,
        Err(e) => { fail("opening the window", e); std::process::exit(1); }
    };
    if let Err(e) = init_app(&root) { fail("starting", e); std::process::exit(1); }
    let surface = match glfw.create_surface(w, h, false) {
        Ok(s) => s,
        Err(e) => { fail("making the surface", e); std::process::exit(1); }
    };
    let win = match pr::graphics_create(surface, w, h, TextureFormat::Rgba8UnormSrgb) {
        Ok(g) => g,
        Err(e) => { fail("making the window's canvas", e); std::process::exit(1); }
    };
    let Some((canvas, main)) = make_canvas(w, h) else { std::process::exit(1) };
    let _ = pr::surface_set_title(surface, "sketch".into());
    LP.with(|l| *l.borrow_mut() = Some(Lp {
        glfw: Some(glfw), surface, win, canvas, main, target: main, size: (w, h),
        images: HashMap::new(), fonts: HashMap::new(), layers: HashMap::new(), warned: Vec::new(),
    }));
    start_canvas(main);
    // start in step with the engine: its shape modes, text alignment and size
    corner_modes();
    let (th, tv, ts) = crate::render::text_state();
    text_align(th, tv);
    text_size(ts);
    // Processing's gray, then whatever setup() drew before size()
    cmd(DrawCommand::BackgroundColor(BColor::linear_rgb(0.8, 0.8, 0.8)));
    let early = EARLY.with(|e| std::mem::take(&mut *e.borrow_mut()));
    for c in early { cmd(c); }
}

// ── filter shaders ───────────────────────────────────────────────────────
thread_local! {
    /// engine shader id -> (libprocessing filter, values version sent)
    static SHADERS: RefCell<HashMap<usize, (Entity, u64)>> = RefCell::new(HashMap::new());
}
/// filter(shader): the shader's WGSL (from glsl.rs) as a libprocessing filter.
pub(crate) fn filter_shader(id: usize, src: &str, values: &[(String, crate::glsl::Ty, Vec<f32>)], textures: &[(String, Entity)], version: u64) {
    use crate::glsl::Ty;
    use pr::shader_value::ShaderValue as V;
    ensure_open();
    let made = SHADERS.with(|m| m.borrow().get(&id).copied());
    let (filter, sent) = match made {
        Some(x) => x,
        None => {
            let shader = match pr::shader_create(src) { Ok(s) => s, Err(e) => { fail("making a shader", e); return; } };
            let f = match pr::filter_create(shader) { Ok(f) => f, Err(e) => { fail("making a shader filter", e); return; } };
            SHADERS.with(|m| m.borrow_mut().insert(id, (f, u64::MAX)));
            (f, u64::MAX)
        }
    };
    if sent != version {
        for (name, t, v) in crate::shader::by_item(values) {
            let (name, t, v) = (&name, &t, &v);
            let g = |i: usize| v.get(i).copied().unwrap_or(0.0);
            let val = match t {
                Ty::Int | Ty::Bool => V::Int(g(0) as i32),
                Ty::Vec(2) => V::Float2([g(0), g(1)]),
                Ty::Vec(3) => V::Float3([g(0), g(1), g(2)]),
                Ty::Vec(4) => V::Float4([g(0), g(1), g(2), g(3)]),
                Ty::Mat(4) => { let mut m = [0.0; 16]; for (i, x) in v.iter().take(16).enumerate() { m[i] = *x; } V::Mat4(m) }
                _ => V::Float(g(0)),
            };
            if let Err(e) = pr::filter_set(filter, name.as_str(), val) { fail("setting a shader uniform", e); }
        }
        SHADERS.with(|m| { if let Some(x) = m.borrow_mut().get_mut(&id) { x.1 = version; } });
    }
    // images are sent every time: their pixels may have changed
    for (name, e) in textures {
        if let Err(err) = pr::filter_set(filter, name.as_str(), V::Texture(*e)) { fail("setting a shader image", err); }
    }
    let Some(t) = target() else { return };
    let _ = flush_keep(t);
    if let Err(e) = filter_keep(t, filter) { fail("running a shader", e); }
    DIRTY.store(true, Ordering::Relaxed);
}

// ── shader(): shapes drawn with a shader ─────────────────────────────────
// The shader becomes a libprocessing custom material. That draws 3D shapes
// (box, sphere); libprocessing's 2D shapes have no normals or texture
// coordinates, which its custom materials need, so 2D shapes stay plain.
thread_local! {
    /// engine shader id -> (libprocessing material, values version sent)
    static MATERIALS: RefCell<HashMap<usize, (Entity, u64)>> = RefCell::new(HashMap::new());
    static SHADING: std::cell::Cell<Option<Entity>> = const { std::cell::Cell::new(None) };
}
pub(crate) fn shape_shader(id: usize, src: &str, values: &[(String, crate::glsl::Ty, Vec<f32>)], textures: &[(String, Entity)], version: u64) {
    use crate::glsl::Ty;
    use pr::shader_value::ShaderValue as V;
    ensure_open();
    // the values below change the material for every shape using it
    flush_shaded();
    let made = MATERIALS.with(|m| m.borrow().get(&id).copied());
    let (mat, sent) = match made {
        Some(x) => x,
        None => {
            let shader = match pr::shader_create(src) { Ok(s) => s, Err(e) => { fail("making a shader", e); return; } };
            let m = match pr::material_create_custom(shader) { Ok(m) => m, Err(e) => { fail("making a shader material", e); return; } };
            MATERIALS.with(|x| x.borrow_mut().insert(id, (m, u64::MAX)));
            (m, u64::MAX)
        }
    };
    if sent != version {
        for (name, t, v) in crate::shader::by_item(values) {
            let (name, t, v) = (&name, &t, &v);
            if name == "resolution" || name == "texOffset" { continue; }
            let g = |i: usize| v.get(i).copied().unwrap_or(0.0);
            let val = match t {
                Ty::Int | Ty::Bool => V::Int(g(0) as i32),
                Ty::Vec(2) => V::Float2([g(0), g(1)]),
                Ty::Vec(3) => V::Float3([g(0), g(1), g(2)]),
                Ty::Vec(4) => V::Float4([g(0), g(1), g(2), g(3)]),
                Ty::Mat(4) => { let mut m = [0.0; 16]; for (i, x) in v.iter().take(16).enumerate() { m[i] = *x; } V::Mat4(m) }
                _ => V::Float(g(0)),
            };
            if let Err(e) = pr::material_set(mat, name.as_str(), val) { fail("setting a shader uniform", e); }
        }
        MATERIALS.with(|m| { if let Some(x) = m.borrow_mut().get_mut(&id) { x.1 = version; } });
    }
    if let Some((w, h)) = target_size() {
        let _ = pr::material_set(mat, "glsl_resolution", V::Float2([w as f32, h as f32]));
    }
    for (name, e) in textures {
        let _ = pr::material_set(mat, format!("tex_{name}").as_str(), V::Texture(*e));
        let _ = pr::material_set(mat, format!("smp_{name}").as_str(), V::Texture(*e));
    }
    cmd(DrawCommand::Material(mat));
    SHADING.with(|s| s.set(Some(mat)));
}
pub(crate) fn reset_shape_shader() {
    SHADING.with(|s| s.set(None));
    cmd(DrawCommand::Unlit);
    if LIT.with(|l| l.get()) { cmd(DrawCommand::Roughness(0.9)); cmd(DrawCommand::Metallic(0.0)); send_material(); }
}
thread_local! {
    /// The fill each shader material has now, and whether shapes drawn with
    /// a shader are waiting to be rendered.
    static SHADED_FILL: RefCell<HashMap<Entity, [f32; 4]>> = RefCell::new(HashMap::new());
    static SHADED_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Before a shader material's values change: render the shapes that used
/// the old ones.
fn flush_shaded() {
    if SHADED_PENDING.with(|p| p.replace(false)) {
        if let Some(t) = target() { let _ = flush_keep(t); }
    }
}
/// The fill a shape drawn with a shader gets (its vertColor). Shapes with the
/// same fill render together, so a grid of strips has no seams between them.
fn shaded_fill(mat: Entity, c: Color) {
    let v = [c.r, c.g, c.b, c.a];
    if SHADED_FILL.with(|f| f.borrow().get(&mat) != Some(&v)) {
        flush_shaded();
        let _ = pr::material_set(mat, "glsl_fill", pr::shader_value::ShaderValue::Float4(v));
        SHADED_FILL.with(|f| { f.borrow_mut().insert(mat, v); });
    }
    SHADED_PENDING.with(|p| p.set(true));
}
/// Before a 3D shape with shader() on: its fill, as vertColor.
fn shaded_3d(draw: DrawCommand) -> bool {
    let Some(mat) = SHADING.with(|s| s.get()) else { return false };
    let c = crate::render::style_pub().0.unwrap_or(Color::from_rgb(1.0, 1.0, 1.0));
    // with a vertex stage the mesh has to carry what it reads (color,
    // attrib()), so the box or sphere is built here
    if crate::shader::lp_has_vertex() {
        let mesh = match &draw {
            DrawCommand::Box { width, height, depth } => {
                let (v, n, t) = crate::three::box_mesh_flat(*width, *height, *depth);
                Some((v, Some(n), t))
            }
            DrawCommand::Sphere { radius, sectors, stacks } => {
                let (v, t, _) = crate::three::sphere_mesh(*radius, *sectors as usize, *stacks as usize);
                let n: Vec<[f32; 3]> = v.iter().map(|p| { let l = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-6); [p[0] / l, p[1] / l, p[2] / l] }).collect();
                Some((v, Some(n), t))
            }
            _ => None,
        };
        if let Some((v, n, t)) = mesh {
            if let Some(g) = whole_mesh_full(&v, &t, c, &[], n.as_deref(), true) {
                shaded_fill(mat, c);
                cmd(DrawCommand::Geometry(g));
                return true;
            }
        }
    }
    shaded_fill(mat, c);
    cmd(draw);
    true
}
/// A filled 2D shape with shader() on: its fill as a whole mesh (normals,
/// texture coordinates and colors, built in one go like libprocessing's own
/// box), so the custom material can draw it. The outline is drawn as usual.
fn shaded_2d(pts: &[(f32, f32)], outline: DrawCommand) -> bool {
    let Some(mat) = SHADING.with(|s| s.get()) else { return false };
    let Some(fill) = crate::render::style_pub().0 else { return false };
    if pts.len() < 3 { return false; }
    let verts: Vec<[f32; 3]> = pts.iter().map(|p| [p.0, p.1, 0.0]).collect();
    let (tris, _) = crate::render::shape_triangles(0, &verts, true);
    let Some(g) = whole_mesh(&verts, &tris, fill) else { return false };
    shaded_fill(mat, fill);
    cmd(DrawCommand::Geometry(g));
    if crate::render::style_pub().1.is_some() {
        cmd(DrawCommand::PushStyle);
        cmd(DrawCommand::NoFill);
        cmd(DrawCommand::Unlit);
        cmd(outline);
        cmd(DrawCommand::PopStyle);
    }
    true
}
/// A Geometry whose mesh is complete when it's made: positions, normals,
/// colors, texture coordinates (across the shape's box) and indices.
fn whole_mesh(verts: &[[f32; 3]], tris: &[[usize; 3]], fill: Color) -> Option<Entity> { whole_mesh_with(verts, tris, fill, &[]) }
/// whole_mesh, with the attrib() values each vertex had: every attribute the
/// active shader reads is in the mesh (zeros where none was given).
fn whole_mesh_with(verts: &[[f32; 3]], tris: &[[usize; 3]], fill: Color, attrs: &[Vec<(String, Vec<f32>)>]) -> Option<Entity> {
    whole_mesh_full(verts, tris, fill, attrs, None, true)
}
/// whole_mesh_with, with normals of its own (else facing the screen) and
/// whether to add each triangle the other way round too (flat 2D shapes).
fn whole_mesh_full(verts: &[[f32; 3]], tris: &[[usize; 3]], fill: Color, attrs: &[Vec<(String, Vec<f32>)>],
                   normals: Option<&[[f32; 3]]>, both_sides: bool) -> Option<Entity> {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for v in verts { x0 = x0.min(v[0]); y0 = y0.min(v[1]); x1 = x1.max(v[0]); y1 = y1.max(v[1]); }
    let (bw, bh) = ((x1 - x0).max(1e-6), (y1 - y0).max(1e-6));
    let uvs: Vec<[f32; 2]> = verts.iter().map(|v| [(v[0] - x0) / bw, (v[1] - y0) / bh]).collect();
    let normals: Vec<[f32; 3]> = match normals { Some(n) if n.len() == verts.len() => n.to_vec(), _ => vec![[0.0f32, 0.0, 1.0]; verts.len()] };
    let colors = vec![[fill.r, fill.g, fill.b, fill.a]; verts.len()];
    // both ways round, so it shows whichever way it faces
    let idx: Vec<u32> = if both_sides {
        tris.iter().flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32, t[0] as u32, t[2] as u32, t[1] as u32]).collect()
    } else {
        tris.iter().flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32]).collect()
    };
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, verts.to_vec());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    #[cfg(lp_attribs)]
    for (name, n) in crate::shader::lp_attributes() {
        let value = |i: usize| -> Vec<f32> {
            // a vertex with no color of its own has the fill
            let missing = if name == "color" { vec![fill.r, fill.g, fill.b, fill.a] } else { Vec::new() };
            let mut v = attrs.get(i).and_then(|l| l.iter().find(|(k, _)| *k == name)).map(|(_, v)| v.clone()).unwrap_or(missing);
            v.resize(n, 0.0);
            v
        };
        let attr = pr::material::custom::custom_attribute(&format!("a_{name}"), n as u32);
        let count = verts.len();
        match n {
            1 => mesh.insert_attribute(attr, (0..count).map(|i| value(i)[0]).collect::<Vec<f32>>()),
            2 => mesh.insert_attribute(attr, (0..count).map(|i| { let v = value(i); [v[0], v[1]] }).collect::<Vec<[f32; 2]>>()),
            3 => mesh.insert_attribute(attr, (0..count).map(|i| { let v = value(i); [v[0], v[1], v[2]] }).collect::<Vec<[f32; 3]>>()),
            _ => mesh.insert_attribute(attr, (0..count).map(|i| { let v = value(i); [v[0], v[1], v[2], v[3]] }).collect::<Vec<[f32; 4]>>()),
        }
    }
    #[cfg(not(lp_attribs))]
    let _ = attrs;
    mesh.insert_indices(Indices::U32(idx));
    let attrs = vec![pr::geometry_attribute_position(), pr::geometry_attribute_normal(), pr::geometry_attribute_color(), pr::geometry_attribute_uv()];
    let g = processing_core::app_mut(|app| {
        let world = app.world_mut();
        let handle = world.resource_mut::<bevy::asset::Assets<Mesh>>().add(mesh);
        let layout = world.spawn(pr::geometry::layout::VertexLayout::with_attributes(attrs)).id();
        Ok(world.spawn(pr::geometry::Geometry::new(handle, layout)).id())
    }).ok()?;
    FRAME_GEOMETRY.with(|f| f.borrow_mut().push(g));
    Some(g)
}

pub(crate) fn window_move(x: i32, y: i32) {
    LP.with(|l| if let Some(l) = l.borrow().as_ref() { let _ = pr::surface_set_position(l.surface, x, y); });
}
pub(crate) fn set_title(t: &str) {
    LP.with(|l| if let Some(l) = l.borrow().as_ref() { let _ = pr::surface_set_title(l.surface, t.to_string()); });
}

fn target() -> Option<Entity> { LP.with(|l| l.borrow().as_ref().map(|l| l.target)) }
/// Size of what is being drawn on: the window, or a layer.
fn target_size() -> Option<(u32, u32)> {
    LP.with(|l| l.borrow().as_ref().map(|l| {
        l.layers.values().find(|v| v.0 == l.target).map(|v| (v.1, v.2)).unwrap_or(l.size)
    }))
}

thread_local! { static TARGETS: RefCell<Vec<Entity>> = const { RefCell::new(Vec::new()) }; }
static DIRTY: AtomicBool = AtomicBool::new(true);
/// Something was drawn since the canvas last showed its work. libprocessing
/// clears for background() before anything else in a batch, so a background()
/// after drawing first finishes what came before it.
static DRAWN: AtomicBool = AtomicBool::new(false);

// ── transforms libprocessing can't keep ─────────────────────────────────
// libprocessing turns the current transform into a position, a turn and a
// scale for every batch it draws, so a shear (or a scale after a turn) comes
// out as a turn. The transform is followed here too, and while it is one of
// those, shapes go out with their corners already moved.
thread_local! {
    static MAT: RefCell<HashMap<Entity, (Affine3A, Vec<Affine3A>)>> = RefCell::new(HashMap::new());
    static BAKING_SHAPE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn mat_reset(t: Entity) { MAT.with(|m| { m.borrow_mut().insert(t, (Affine3A::IDENTITY, Vec::new())); }); }
fn track(t: Entity, c: &DrawCommand) {
    MAT.with(|m| {
        let mut m = m.borrow_mut();
        let (cur, stack) = m.entry(t).or_insert((Affine3A::IDENTITY, Vec::new()));
        match c {
            DrawCommand::PushMatrix => stack.push(*cur),
            DrawCommand::PopMatrix => { if let Some(p) = stack.pop() { *cur = p; } }
            DrawCommand::ResetMatrix => *cur = Affine3A::IDENTITY,
            DrawCommand::ApplyMatrix(a) => *cur *= *a,
            DrawCommand::Translate(v) => *cur *= Affine3A::from_translation(*v),
            DrawCommand::Rotate { angle, axis } => *cur *= Affine3A::from_axis_angle(axis.normalize(), *angle),
            DrawCommand::Scale(v) => *cur *= Affine3A::from_scale(*v),
            DrawCommand::ShearX { angle } => *cur *= Affine3A::from_mat3(Mat3::from_cols(Vec3::X, Vec3::new(angle.tan(), 1.0, 0.0), Vec3::Z)),
            DrawCommand::ShearY { angle } => *cur *= Affine3A::from_mat3(Mat3::from_cols(Vec3::new(1.0, angle.tan(), 0.0), Vec3::Y, Vec3::Z)),
            _ => {}
        }
    });
}
/// The current transform, when libprocessing can't keep it.
fn skewed(t: Entity) -> Option<Affine3A> {
    let cur = MAT.with(|m| m.borrow().get(&t).map(|v| v.0))?;
    let (a, b, c) = (Vec3::from(cur.matrix3.x_axis), Vec3::from(cur.matrix3.y_axis), Vec3::from(cur.matrix3.z_axis));
    let off = |u: Vec3, v: Vec3| u.dot(v).abs() > 1e-4 * (u.length() * v.length()).max(1e-12);
    (off(a, b) || off(a, c) || off(b, c)).then_some(cur)
}
fn record(t: Entity, c: DrawCommand) {
    DIRTY.store(true, Ordering::Relaxed);
    if !matches!(c, DrawCommand::BackgroundColor(_)) { DRAWN.store(true, Ordering::Relaxed); }
    if let Err(e) = pr::graphics_record_command(t, c) { fail("drawing", e); }
}
/// Starts drawing in plain Processing coordinates (the transform undone).
fn bake_begin(t: Entity) {
    record(t, DrawCommand::PushMatrix);
    record(t, DrawCommand::ResetMatrix);
    base_coords(t);
}
fn bake_end(t: Entity) { record(t, DrawCommand::PopMatrix); }
fn ellipse_points(cx: f32, cy: f32, w: f32, h: f32, a0: f32, a1: f32, n: usize) -> Vec<(f32, f32)> {
    (0..=n).map(|i| { let a = a0 + (a1 - a0) * i as f32 / n as f32; (cx + a.cos() * w / 2.0, cy + a.sin() * h / 2.0) }).collect()
}
/// Draws c with its points moved by m. false: not a shape this knows.
fn bake(t: Entity, m: Affine3A, c: &DrawCommand) -> bool {
    let p = |x: f32, y: f32| { let q = m.transform_point3(Vec3::new(x, y, 0.0)); (q.x, q.y) };
    let poly = |pts: &[(f32, f32)], close: bool| {
        bake_begin(t);
        record(t, DrawCommand::BeginShape { kind: ShapeKind::Polygon });
        for &(x, y) in pts { let (x, y) = p(x, y); record(t, DrawCommand::ShapeVertex { x, y }); }
        record(t, DrawCommand::EndShape { close });
        bake_end(t);
    };
    match *c {
        DrawCommand::Rect { x, y, w, h, radii } => {
            if radii.iter().any(|r| *r > 0.5) {
                use std::f32::consts::{FRAC_PI_2, PI};
                let fit = |r: f32| r.min(w.abs() / 2.0).min(h.abs() / 2.0).max(0.0);
                let [tl, tr, br, bl] = radii.map(fit);
                let mut pts = Vec::new();
                pts.extend(ellipse_points(x + tl, y + tl, 2.0 * tl, 2.0 * tl, PI, PI + FRAC_PI_2, 6));
                pts.extend(ellipse_points(x + w - tr, y + tr, 2.0 * tr, 2.0 * tr, PI + FRAC_PI_2, 2.0 * PI, 6));
                pts.extend(ellipse_points(x + w - br, y + h - br, 2.0 * br, 2.0 * br, 0.0, FRAC_PI_2, 6));
                pts.extend(ellipse_points(x + bl, y + h - bl, 2.0 * bl, 2.0 * bl, FRAC_PI_2, PI, 6));
                poly(&pts, true);
            } else {
                poly(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], true);
            }
        }
        DrawCommand::Ellipse { cx, cy, w, h } => {
            let n = ((w.abs() + h.abs()) * 0.3).clamp(24.0, 160.0) as usize;
            let mut pts = ellipse_points(cx, cy, w, h, 0.0, std::f32::consts::TAU, n);
            pts.pop();
            poly(&pts, true);
        }
        DrawCommand::Arc { cx, cy, w, h, start, stop, mode } => {
            let n = ((w.abs() + h.abs()) * 0.3 * ((stop - start).abs() / std::f32::consts::TAU)).clamp(8.0, 160.0) as usize;
            let mut pts = ellipse_points(cx, cy, w, h, start, stop, n);
            match mode { ArcMode::Pie => { pts.push((cx, cy)); poly(&pts, true) } ArcMode::Chord => poly(&pts, true), _ => poly(&pts, false) }
        }
        DrawCommand::Triangle { x1, y1, x2, y2, x3, y3 } => {
            let ((x1, y1), (x2, y2), (x3, y3)) = (p(x1, y1), p(x2, y2), p(x3, y3));
            bake_begin(t); record(t, DrawCommand::Triangle { x1, y1, x2, y2, x3, y3 }); bake_end(t);
        }
        DrawCommand::Quad { x1, y1, x2, y2, x3, y3, x4, y4 } => {
            let ((x1, y1), (x2, y2), (x3, y3), (x4, y4)) = (p(x1, y1), p(x2, y2), p(x3, y3), p(x4, y4));
            bake_begin(t); record(t, DrawCommand::Quad { x1, y1, x2, y2, x3, y3, x4, y4 }); bake_end(t);
        }
        DrawCommand::Line { x1, y1, x2, y2 } => {
            let ((x1, y1), (x2, y2)) = (p(x1, y1), p(x2, y2));
            bake_begin(t); record(t, DrawCommand::Line { x1, y1, x2, y2 }); bake_end(t);
        }
        DrawCommand::Point { x, y } => {
            let (x, y) = p(x, y);
            bake_begin(t); record(t, DrawCommand::Point { x, y }); bake_end(t);
        }
        DrawCommand::Image { entity, dx, dy, d_width, d_height, sx: None, .. } => {
            let (w, h) = (d_width.unwrap_or(0.0), d_height.unwrap_or(0.0));
            let corners = [p(dx, dy), p(dx + w, dy), p(dx + w, dy + h), p(dx, dy + h)];
            let verts: Vec<[f32; 3]> = corners.iter().map(|&(x, y)| [x, y, 0.0]).collect();
            bake_begin(t);
            // drawn as a textured mesh: no lights, no outline
            record(t, DrawCommand::PushStyle);
            record(t, DrawCommand::NoStroke);
            mesh_3d(&verts, &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], &[[0, 1, 2], [0, 2, 3]], Some(entity));
            record(t, DrawCommand::PopStyle);
            bake_end(t);
        }
        DrawCommand::BeginShape { kind } => {
            bake_begin(t);
            BAKING_SHAPE.with(|b| b.set(true));
            record(t, DrawCommand::BeginShape { kind });
        }
        _ => return false,
    }
    true
}
/// A vertex of a shape begun while the transform was skewed.
fn bake_vertex(m: Affine3A, c: DrawCommand) -> DrawCommand {
    let p = |x: f32, y: f32| { let q = m.transform_point3(Vec3::new(x, y, 0.0)); (q.x, q.y) };
    match c {
        DrawCommand::ShapeVertex { x, y } => { let (x, y) = p(x, y); DrawCommand::ShapeVertex { x, y } }
        DrawCommand::ShapeCurveVertex { x, y } => { let (x, y) = p(x, y); DrawCommand::ShapeCurveVertex { x, y } }
        DrawCommand::ShapeQuadraticVertex { cx, cy, x, y } => {
            let ((cx, cy), (x, y)) = (p(cx, cy), p(x, y));
            DrawCommand::ShapeQuadraticVertex { cx, cy, x, y }
        }
        DrawCommand::ShapeBezierVertex { cx1, cy1, cx2, cy2, x, y } => {
            let ((cx1, cy1), (cx2, cy2), (x, y)) = (p(cx1, cy1), p(cx2, cy2), p(x, y));
            DrawCommand::ShapeBezierVertex { cx1, cy1, cx2, cy2, x, y }
        }
        c => c,
    }
}

/// Records a draw command on whatever is being drawn on.
pub(crate) fn cmd(c: DrawCommand) {
    if !finite(&c) { return; }
    if let Some(t) = target() {
        track(t, &c);
        if BAKING_SHAPE.with(|b| b.get()) {
            let m = MAT.with(|m| m.borrow().get(&t).map(|v| v.0)).unwrap_or(Affine3A::IDENTITY);
            let end = matches!(c, DrawCommand::EndShape { .. });
            record(t, bake_vertex(m, c));
            if end { BAKING_SHAPE.with(|b| b.set(false)); bake_end(t); }
            return;
        }
        if let Some(m) = skewed(t) {
            if bake(t, m, &c) { return; }
        }
    }
    DIRTY.store(true, Ordering::Relaxed);
    if !matches!(c, DrawCommand::BackgroundColor(_)) { DRAWN.store(true, Ordering::Relaxed); }
    if LIT.with(|l| l.get()) && matches!(c, DrawCommand::Box { .. } | DrawCommand::Sphere { .. } | DrawCommand::Geometry(_)
        | DrawCommand::EndShape { .. } | DrawCommand::Rect { .. } | DrawCommand::Ellipse { .. } | DrawCommand::Triangle { .. }
        | DrawCommand::Quad { .. } | DrawCommand::Arc { .. }) {
        LIT_DRAWN.with(|d| d.set(true));
    }
    match target() {
        Some(t) => { if let Err(e) = pr::graphics_record_command(t, c) { fail("drawing", e); } }
        None => EARLY.with(|e| e.borrow_mut().push(c)),
    }
}

/// Shapes with a NaN or an endless coordinate are skipped (the software
/// renderer skips them too); libprocessing's tessellator can't take them.
fn finite(c: &DrawCommand) -> bool {
    const BIG: f32 = 1.0e7;
    let ok = |v: &[f32]| v.iter().all(|x| x.is_finite() && x.abs() < BIG);
    match c {
        DrawCommand::Rect { x, y, w, h, radii } => ok(&[*x, *y, *w, *h]) && ok(radii),
        DrawCommand::Ellipse { cx, cy, w, h } => ok(&[*cx, *cy, *w, *h]),
        DrawCommand::Line { x1, y1, x2, y2 } => ok(&[*x1, *y1, *x2, *y2]),
        DrawCommand::Point { x, y } => ok(&[*x, *y]),
        DrawCommand::Triangle { x1, y1, x2, y2, x3, y3 } => ok(&[*x1, *y1, *x2, *y2, *x3, *y3]),
        DrawCommand::Quad { x1, y1, x2, y2, x3, y3, x4, y4 } => ok(&[*x1, *y1, *x2, *y2, *x3, *y3, *x4, *y4]),
        DrawCommand::Arc { cx, cy, w, h, start, stop, .. } => ok(&[*cx, *cy, *w, *h, *start, *stop]),
        DrawCommand::ShapeVertex { x, y } | DrawCommand::ShapeCurveVertex { x, y } => ok(&[*x, *y]),
        DrawCommand::ShapeBezierVertex { cx1, cy1, cx2, cy2, x, y } => ok(&[*cx1, *cy1, *cx2, *cy2, *x, *y]),
        DrawCommand::ShapeQuadraticVertex { cx, cy, x, y } => ok(&[*cx, *cy, *x, *y]),
        DrawCommand::Translate(v) | DrawCommand::Scale(v) => ok(&[v.x, v.y, v.z]),
        DrawCommand::Rotate { angle, .. } | DrawCommand::ShearX { angle } | DrawCommand::ShearY { angle } => ok(&[*angle]),
        DrawCommand::StrokeWeight(w) | DrawCommand::TextSize(w) => ok(&[*w]),
        DrawCommand::Text { x, y, .. } => ok(&[*x, *y]),
        DrawCommand::Image { dx, dy, d_width, d_height, .. } => ok(&[*dx, *dy, d_width.unwrap_or(0.0), d_height.unwrap_or(0.0)]),
        DrawCommand::Box { width, height, depth } => ok(&[*width, *height, *depth]),
        DrawCommand::Sphere { radius, .. } => ok(&[*radius]),
        _ => true,
    }
}

// Colors go to libprocessing as the numbers they are (0.8 is 0.8), not turned
// into linear light, and come back from the canvas the same way. So blending,
// translucency and blend modes add and mix those numbers, like Processing.
// The window turns them into light when the frame is shown (finish_frame).
/// The canvas keeps colors multiplied by their alpha; get() and layers want
/// them as drawn.
fn unpremultiply(c: LinearRgba) -> LinearRgba {
    if c.alpha <= 0.0 || c.alpha >= 1.0 { return c; }
    LinearRgba::new(c.red / c.alpha, c.green / c.alpha, c.blue / c.alpha, c.alpha)
}
fn col(c: Color) -> BColor { BColor::linear_rgba(c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0), c.a.clamp(0.0, 1.0)) }

// ── style ────────────────────────────────────────────────────────────────
pub(crate) fn background(c: Color) {
    if DRAWN.swap(false, Ordering::Relaxed) { if let Some(g) = target() { let _ = flush_keep(g); } }
    cmd(DrawCommand::BackgroundColor(col(c)));
}
pub(crate) fn fill(c: Option<Color>) { cmd(match c { Some(c) => DrawCommand::Fill(col(c)), None => DrawCommand::NoFill }); }
pub(crate) fn stroke(c: Option<Color>) { cmd(match c { Some(c) => DrawCommand::StrokeColor(col(c)), None => DrawCommand::NoStroke }); }
pub(crate) fn stroke_weight(w: f32) { cmd(DrawCommand::StrokeWeight(w)); }
/// engine: cap 0 ROUND 1 SQUARE 2 PROJECT; join 0 MITER 1 BEVEL 2 ROUND
pub(crate) fn stroke_cap(c: u8) {
    cmd(DrawCommand::StrokeCap(match c { 1 => StrokeCapMode::Square, 2 => StrokeCapMode::Project, _ => StrokeCapMode::Round }));
}
pub(crate) fn stroke_join(j: u8) {
    cmd(DrawCommand::StrokeJoin(match j { 1 => StrokeJoinMode::Bevel, 2 => StrokeJoinMode::Round, _ => StrokeJoinMode::Miter }));
}
thread_local! { static TINT: std::cell::Cell<Option<Color>> = const { std::cell::Cell::new(None) }; }
pub(crate) fn tint(c: Option<Color>) {
    TINT.with(|t| t.set(c));
    cmd(match c { Some(c) => DrawCommand::Tint(col(c)), None => DrawCommand::NoTint });
}
/// hint(DISABLE_DEPTH_TEST / ENABLE_DEPTH_TEST)
#[cfg(lp_depth_test)]
pub(crate) fn depth_test(on: bool) { cmd(DrawCommand::DepthTest(on)); }
pub(crate) fn blend_mode(m: u8) {
    let mode = pr::render::command::BlendMode::try_from(m).unwrap_or_default();
    cmd(DrawCommand::BlendMode(mode.to_blend_state()));
}
/// smooth() / noSmooth(): multisampling on the canvas, on or off. What was
/// drawn before keeps the setting it was drawn with.
pub(crate) fn smooth(on: bool) {
    let Some(t) = target() else { return };
    let want = if on { bevy::render::view::Msaa::Sample4 } else { bevy::render::view::Msaa::Off };
    let same = processing_core::app_mut(|app| Ok(app.world().get::<bevy::render::view::Msaa>(t).copied() == Some(want))).unwrap_or(true);
    if same { return; }
    if DRAWN.swap(false, Ordering::Relaxed) { let _ = flush_keep(t); }
    let _ = processing_core::app_mut(|app| {
        if let Ok(mut e) = app.world_mut().get_entity_mut(t) { e.insert(want); }
        Ok(())
    });
}
pub(crate) fn push_style() { cmd(DrawCommand::PushStyle); }
pub(crate) fn pop_style() { cmd(DrawCommand::PopStyle); }
/// The engine works out rect / ellipse / image modes itself, so libprocessing
/// always gets corners (and centers for ellipses).
fn corner_modes() {
    cmd(DrawCommand::RectMode(ShapeMode::Corner));
    cmd(DrawCommand::EllipseMode(ShapeMode::Center));
    cmd(DrawCommand::ImageMode(ShapeMode::Corner));
}

// ── shapes ───────────────────────────────────────────────────────────────
pub(crate) fn rect(x: f32, y: f32, w: f32, h: f32, radii: [f32; 4]) {
    let c = DrawCommand::Rect { x, y, w, h, radii };
    if !shaded_2d(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], c.clone()) { cmd(c); }
}
pub(crate) fn ellipse(cx: f32, cy: f32, w: f32, h: f32) {
    let c = DrawCommand::Ellipse { cx, cy, w, h };
    // the outline is only worked out when a shader needs it
    if SHADING.with(|s| s.get()).is_none() { return cmd(c); }
    if !shaded_2d(&arc_outline(cx, cy, w, h, 0.0, std::f32::consts::TAU, false), c.clone()) { cmd(c); }
}
/// Points around an ellipse (or part of it, with the center for a pie).
fn arc_outline(cx: f32, cy: f32, w: f32, h: f32, a0: f32, a1: f32, center: bool) -> Vec<(f32, f32)> {
    let n = ((w.abs() + h.abs()) * 0.25).clamp(24.0, 128.0) as usize;
    let mut v: Vec<(f32, f32)> = (0..=n).map(|i| { let t = a0 + (a1 - a0) * i as f32 / n as f32; (cx + t.cos() * w / 2.0, cy + t.sin() * h / 2.0) }).collect();
    if center { v.push((cx, cy)); }
    v
}
/// Lines and points of an odd width sit in the middle of a pixel row, like
/// the other renderer, so a 1-pixel line on whole coordinates is one crisp row.
fn pixel_center() -> f32 {
    let w = crate::render::style_pub().2.round() as i64;
    if w % 2 == 1 { 0.5 } else { 0.0 }
}
pub(crate) fn line(x1: f32, y1: f32, x2: f32, y2: f32) {
    let o = pixel_center();
    cmd(DrawCommand::Line { x1: x1 + o, y1: y1 + o, x2: x2 + o, y2: y2 + o });
}
pub(crate) fn point(x: f32, y: f32) { let o = pixel_center(); cmd(DrawCommand::Point { x: x + o, y: y + o }); }
pub(crate) fn triangle(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) {
    let c = DrawCommand::Triangle { x1, y1, x2, y2, x3, y3 };
    if !shaded_2d(&[(x1, y1), (x2, y2), (x3, y3)], c.clone()) { cmd(c); }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn quad(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32, x4: f32, y4: f32) {
    let c = DrawCommand::Quad { x1, y1, x2, y2, x3, y3, x4, y4 };
    if !shaded_2d(&[(x1, y1), (x2, y2), (x3, y3), (x4, y4)], c.clone()) { cmd(c); }
}
/// engine arc modes: 0 default (Processing draws a pie fill and an open stroke), 1 OPEN, 2 CHORD, 3 PIE
pub(crate) fn arc(cx: f32, cy: f32, w: f32, h: f32, start: f32, stop: f32, mode: u8) {
    let pie = mode != 1 && mode != 2;
    // no mode given: Processing fills a pie and outlines just the curve,
    // which libprocessing has no single mode for, so it's two arcs
    if mode == 0 && SHADING.with(|s| s.get()).is_none() {
        let (fill_on, stroke_on) = { let (f, st, _) = crate::render::style_pub(); (f.is_some(), st.is_some()) };
        if fill_on {
            cmd(DrawCommand::PushStyle); cmd(DrawCommand::NoStroke);
            cmd(DrawCommand::Arc { cx, cy, w, h, start, stop, mode: ArcMode::Pie });
            cmd(DrawCommand::PopStyle);
        }
        if stroke_on {
            cmd(DrawCommand::PushStyle); cmd(DrawCommand::NoFill);
            cmd(DrawCommand::Arc { cx, cy, w, h, start, stop, mode: ArcMode::Open });
            cmd(DrawCommand::PopStyle);
        }
        return;
    }
    let mode = match mode { 2 => ArcMode::Chord, 3 => ArcMode::Pie, 1 => ArcMode::Open, _ => ArcMode::Open };
    let c = DrawCommand::Arc { cx, cy, w, h, start, stop, mode };
    // the outline only matters with shader() on
    if SHADING.with(|s| s.get()).is_none() || !shaded_2d(&arc_outline(cx, cy, w, h, start, stop, pie), c.clone()) { cmd(c); }
}
thread_local! {
    /// A shape being built while shader() is on: its kind and vertices.
    static SHADED_SHAPE: RefCell<Option<(u8, Vec<[f32; 3]>)>> = const { RefCell::new(None) };
    /// attrib() values of each of its vertices
    static SHADED_ATTRS: RefCell<Vec<Vec<(String, Vec<f32>)>>> = const { RefCell::new(Vec::new()) };
}
pub(crate) fn begin_shape(kind: u8) {
    if SHADING.with(|s| s.get()).is_some() {
        SHADED_SHAPE.with(|s| *s.borrow_mut() = Some((kind, Vec::new())));
        SHADED_ATTRS.with(|a| a.borrow_mut().clear());
    }
    // engine kinds: 0 polygon, 1 POINTS, 2 LINES, 3 TRIANGLES, 4 TRIANGLE_FAN, 5 TRIANGLE_STRIP, 6 QUADS, 7 QUAD_STRIP
    let kind = match kind {
        1 => ShapeKind::Points, 2 => ShapeKind::Lines, 3 => ShapeKind::Triangles, 4 => ShapeKind::TriangleFan,
        5 => ShapeKind::TriangleStrip, 6 => ShapeKind::Quads, 7 => ShapeKind::QuadStrip, _ => ShapeKind::Polygon,
    };
    cmd(DrawCommand::BeginShape { kind });
}
pub(crate) fn vertex(x: f32, y: f32) {
    SHADED_SHAPE.with(|s| if let Some((_, v)) = s.borrow_mut().as_mut() {
        v.push([x, y, 0.0]);
        let mut cur = crate::render::ATTRIBS.with(|a| a.borrow().clone());
        // each vertex's own fill, for a vertex shader's `color`
        if let Some(f) = crate::render::style_pub().0 { cur.push(("color".to_string(), vec![f.r, f.g, f.b, f.a])); }
        SHADED_ATTRS.with(|a| a.borrow_mut().push(cur));
    });
    cmd(DrawCommand::ShapeVertex { x, y });
}
pub(crate) fn bezier_vertex(cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
    cmd(DrawCommand::ShapeBezierVertex { cx1, cy1, cx2, cy2, x, y });
}
pub(crate) fn quadratic_vertex(cx: f32, cy: f32, x: f32, y: f32) { cmd(DrawCommand::ShapeQuadraticVertex { cx, cy, x, y }); }
pub(crate) fn curve_vertex(x: f32, y: f32) { cmd(DrawCommand::ShapeCurveVertex { x, y }); }
pub(crate) fn end_shape(close: bool) {
    let shaded = SHADED_SHAPE.with(|s| s.borrow_mut().take());
    if let (Some((kind, verts)), Some(mat), Some(fill)) = (shaded, SHADING.with(|s| s.get()), crate::render::style_pub().0) {
        // points and lines have no fill to shade
        if verts.len() >= 3 && kind != 1 && kind != 2 {
            let (tris, _) = crate::render::shape_triangles(kind, &verts, close);
            let attrs = SHADED_ATTRS.with(|a| std::mem::take(&mut *a.borrow_mut()));
            if let Some(g) = whole_mesh_with(&verts, &tris, fill, &attrs) {
                shaded_fill(mat, fill);
                cmd(DrawCommand::Geometry(g));
                // the recorded vertices still make the outline, with no fill
                cmd(DrawCommand::PushStyle);
                cmd(DrawCommand::NoFill);
                cmd(DrawCommand::Unlit);
                cmd(DrawCommand::EndShape { close });
                cmd(DrawCommand::PopStyle);
                return;
            }
        }
    }
    cmd(DrawCommand::EndShape { close });
}
pub(crate) fn begin_contour() { cmd(DrawCommand::BeginContour); }
pub(crate) fn end_contour() { cmd(DrawCommand::EndContour); }

// ── transforms ───────────────────────────────────────────────────────────
pub(crate) fn push_matrix() { cmd(DrawCommand::PushMatrix); }
pub(crate) fn pop_matrix() { cmd(DrawCommand::PopMatrix); }
pub(crate) fn reset_matrix() { cmd(DrawCommand::ResetMatrix); processing_coords(); }
pub(crate) fn translate(x: f32, y: f32, z: f32) { cmd(DrawCommand::Translate(Vec3::new(x, y, z))); }
pub(crate) fn rotate(angle: f32, axis: Vec3) { cmd(DrawCommand::Rotate { angle, axis }); }
pub(crate) fn scale(x: f32, y: f32, z: f32) { cmd(DrawCommand::Scale(Vec3::new(x, y, z))); }
/// [a c e; b d f]
// libprocessing keeps transforms as position / rotation / scale, so a shear
// comes out as a turn for now.
pub(crate) fn shear_x(angle: f32) { cmd(DrawCommand::ShearX { angle }); }
pub(crate) fn shear_y(angle: f32) { cmd(DrawCommand::ShearY { angle }); }
pub(crate) fn apply_matrix(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) {
    let m = Affine3A::from_mat3_translation(Mat3::from_cols_array(&[a, b, 0.0, c, d, 0.0, 0.0, 0.0, 1.0]), Vec3::new(e, f, 0.0));
    cmd(DrawCommand::ApplyMatrix(m));
}

// ── 3D ───────────────────────────────────────────────────────────────────
/// libprocessing's 3D camera looks at the origin with y going up; Processing
/// puts the origin in the top left corner with y going down. This turns the
/// first into the second, at the start of every frame.
fn processing_coords() {
    let Some(t) = target() else { return };
    base_coords(t);
    mat_reset(t);
}
/// Processing's coordinates, recorded as they are (not followed by track()).
fn base_coords(t: Entity) {
    let Some((w, h)) = target_size() else { return };
    record(t, DrawCommand::Scale(Vec3::new(1.0, -1.0, 1.0)));
    record(t, DrawCommand::Translate(Vec3::new(-(w as f32) / 2.0, -(h as f32) / 2.0, 0.0)));
}

pub(crate) fn box_3d(w: f32, h: f32, d: f32) {
    let c = DrawCommand::Box { width: w, height: h, depth: d };
    if !shaded_3d(c.clone()) { cmd(c); }
}
pub(crate) fn sphere(r: f32, sectors: u32, stacks: u32) {
    let c = DrawCommand::Sphere { radius: r, sectors, stacks };
    if !shaded_3d(c.clone()) { cmd(c); }
}
/// camera(eye, center, up) in Processing's coordinates (see processing_coords).
pub(crate) fn camera(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) {
    let Some((w, h)) = LP.with(|l| l.borrow().as_ref().map(|l| l.size)) else { return };
    let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
    let to_world = |p: [f32; 3]| Vec3::new(p[0] - hw, -(p[1] - hh), p[2]);
    if let Some(g) = target() {
        // Processing's up points to the bottom of the screen (its y grows
        // downward); libprocessing's points to the top
        let up = Vec3::new(-up[0], up[1], -up[2]);
        if let Err(e) = pr::graphics_camera(g, to_world(eye), to_world(center), up) { fail("camera", e); }
    }
}
/// camera(): back to the default view.
pub(crate) fn camera_default() {
    let Some((w, h)) = LP.with(|l| l.borrow().as_ref().map(|l| l.size)) else { return };
    let z = (h as f32 / 2.0) / (std::f32::consts::PI / 6.0).tan();
    camera([w as f32 / 2.0, h as f32 / 2.0, z], [w as f32 / 2.0, h as f32 / 2.0, 0.0], [0.0, 1.0, 0.0]);
}
pub(crate) fn perspective(fov: f32, aspect: f32, near: f32, far: f32) {
    set_projection(perspective_matrix(fov, aspect, near), far);
}
/// Bevy's own perspective (y up, depth reversed and endless). RustMode sets it
/// itself rather than through libprocessing, whose projection may be y-down
/// (the lp-fork's P3D frustum): the camera here already turns y around.
fn perspective_matrix(fov: f32, aspect: f32, near: f32) -> bevy::math::Mat4 {
    bevy::math::Mat4::perspective_infinite_reverse_rh(fov, aspect, near)
}
/// The projection a new canvas or layer starts with: what libprocessing's
/// 3D mode used to give (60 degrees, near 1, far ten camera distances).
fn default_projection(g: Entity, w: u32, h: u32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let fov = std::f32::consts::PI / 3.0;
    let camera_z = (h / 2.0) / (fov / 2.0).tan();
    set_projection_on(g, perspective_matrix(fov, w / h, 1.0), camera_z * 10.0);
}
pub(crate) fn ortho(w: f32, h: f32, near: f32, far: f32) {
    ortho_edges(-w / 2.0, w / 2.0, -h / 2.0, h / 2.0, near, far);
}

/// A projection given as a matrix. libprocessing's own ortho() follows the
/// window size and is always centered, so ortho with edges and frustum set the
/// camera's projection themselves. Depth is reversed (near 1, far 0), as Bevy wants.
#[derive(Debug, Clone)]
struct MatrixProjection { m: bevy::math::Mat4, far: f32 }
impl bevy::camera::CameraProjection for MatrixProjection {
    fn get_clip_from_view(&self) -> bevy::math::Mat4 { self.m }
    fn get_clip_from_view_for_sub(&self, _: &bevy::camera::SubCameraView) -> bevy::math::Mat4 { self.m }
    fn update(&mut self, _w: f32, _h: f32) {}
    fn far(&self) -> f32 { self.far }
    fn get_frustum_corners(&self, _near: f32, _far: f32) -> [bevy::math::Vec3A; 8] {
        let inv = self.m.inverse();
        let at = |x: f32, y: f32, z: f32| { let p = inv * Vec4::new(x, y, z, 1.0); bevy::math::Vec3A::from(p.truncate() / p.w) };
        [at(1.0, -1.0, 1.0), at(1.0, 1.0, 1.0), at(-1.0, 1.0, 1.0), at(-1.0, -1.0, 1.0),
         at(1.0, -1.0, 0.0), at(1.0, 1.0, 0.0), at(-1.0, 1.0, 0.0), at(-1.0, -1.0, 0.0)]
    }
}
fn set_projection(m: bevy::math::Mat4, far: f32) {
    let Some(g) = target() else { return };
    set_projection_on(g, m, far);
}
fn set_projection_on(g: Entity, m: bevy::math::Mat4, far: f32) {
    let _ = flush_keep(g);
    let _ = processing_core::app_mut(|app| {
        if let Some(mut p) = app.world_mut().get_mut::<bevy::camera::Projection>(g) {
            *p = bevy::camera::Projection::custom(MatrixProjection { m, far });
        }
        Ok(())
    });
}
// Processing's view has y growing downward and libprocessing's upward, so
// bottom and top swap sides.
pub(crate) fn ortho_edges(l: f32, r: f32, b: f32, t: f32, near: f32, far: f32) {
    let (b, t) = (-t, -b);
    set_projection(bevy::math::Mat4::orthographic_rh(l, r, b, t, far, near), far);
}
pub(crate) fn frustum(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) {
    let (b, t) = (-t, -b);
    let m = bevy::math::Mat4::from_cols(
        Vec4::new(2.0 * n / (r - l), 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 * n / (t - b), 0.0, 0.0),
        Vec4::new((r + l) / (r - l), (t + b) / (t - b), n / (f - n), -1.0),
        Vec4::new(0.0, 0.0, n * f / (f - n), 0.0),
    );
    set_projection(m, f);
}
// ── lights ───────────────────────────────────────────────────────────────
// Processing's lights last one frame: draw() asks for them again each time.
// The lights asked for during a frame are compared with the ones in the
// scene when the frame ends, and only remade when they changed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum LightSpec { Ambient([f32; 3]), Directional([f32; 3], [f32; 3]), Point([f32; 3], [f32; 3]),
    /// color, position, direction, half-angle of the cone
    Spot([f32; 3], [f32; 3], [f32; 3], f32) }
thread_local! {
    static WANT: RefCell<Vec<LightSpec>> = const { RefCell::new(Vec::new()) };
    static HAVE: RefCell<(Vec<LightSpec>, Vec<Entity>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}
// Shapes after the first light of a frame are lit (PBR); before it, and
// after no_lights(), they are flat colors like Processing without lights.
thread_local! { static LIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
thread_local! {
    /// A shape was drawn with lights on since the scene's lights were last set.
    static LIT_DRAWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub(crate) fn add_light(spec: LightSpec) {
    // libprocessing has one set of lights for everything a render draws, so
    // a light added after lit shapes were drawn would light them too. Like
    // Processing, those keep the lights they were drawn with: render them now.
    if LIT_DRAWN.with(|d| d.replace(false)) {
        sync_lights();
        if let Some(t) = target() { let _ = flush_keep(t); }
    }
    WANT.with(|w| w.borrow_mut().push(spec));
    if !LIT.with(|l| l.replace(true)) { cmd(DrawCommand::Roughness(0.9)); cmd(DrawCommand::Metallic(0.0)); send_material(); }
    // a spot light only lights what is drawn after the canvas has rendered
    // once with it in the frame, so make it now and render what came before
    if matches!(spec, LightSpec::Spot(..)) {
        sync_lights();
        if let Some(t) = target() { let _ = flush_keep(t); }
    }
}
// The material calls, as libprocessing's PBR material: emissive is its glow,
// shininess its smoothness, specular the shine's color (with the lp-fork;
// without it a black specular only turns the shine off).
// Only sent while lights are on: they make libprocessing's material a lit
// one, which would darken flat shapes. Kept, and sent when lights come on.
thread_local! { static MATERIAL: std::cell::Cell<([f32; 3], Option<f32>)> = const { std::cell::Cell::new(([0.0; 3], None)) }; }
#[cfg(lp_specular)]
thread_local! { static SPECULAR: std::cell::Cell<Option<[f32; 3]>> = const { std::cell::Cell::new(None) }; }
fn send_material() {
    let (e, rough) = MATERIAL.with(|m| m.get());
    cmd(DrawCommand::Emissive(BColor::linear_rgb(e[0], e[1], e[2])));
    if let Some(r) = rough { cmd(DrawCommand::Roughness(r)); }
    #[cfg(lp_specular)]
    if let Some(c) = SPECULAR.with(|s| s.get()) { cmd(DrawCommand::Specular(BColor::srgb(c[0], c[1], c[2]))); }
}
pub(crate) fn material_emissive(c: [f32; 3]) {
    MATERIAL.with(|m| { let (_, r) = m.get(); m.set((c, r)); });
    if LIT.with(|l| l.get()) { send_material(); }
}
pub(crate) fn material_shininess(sh: f32) {
    MATERIAL.with(|m| { let (e, _) = m.get(); m.set((e, Some((2.0 / (sh + 2.0)).sqrt().clamp(0.02, 1.0)))); });
    if LIT.with(|l| l.get()) { send_material(); }
}
pub(crate) fn material_specular(c: [f32; 3]) {
    #[cfg(lp_specular)]
    SPECULAR.with(|s| s.set(Some(c)));
    #[cfg(not(lp_specular))]
    if c == [0.0; 3] { MATERIAL.with(|m| { let (e, _) = m.get(); m.set((e, Some(1.0))); }); }
    if LIT.with(|l| l.get()) { send_material(); }
}
pub(crate) fn lights() {
    add_light(LightSpec::Ambient([0.5; 3]));
    add_light(LightSpec::Directional([0.5; 3], [0.0, 0.0, -1.0]));
}
/// noLights(): shapes from here on are flat again, and lights() after it
/// starts a new set, like Processing.
pub(crate) fn no_lights() {
    // the shapes lit so far keep their lights; the ones after start with none
    if LIT_DRAWN.with(|d| d.replace(false)) {
        sync_lights();
        if let Some(t) = target() { let _ = flush_keep(t); }
    }
    WANT.with(|w| w.borrow_mut().clear());
    LIT.with(|l| l.set(false));
    cmd(DrawCommand::Unlit);
}
// How bright a light of full strength (255) is, tuned so lights() on a
// surface facing the camera gives about its fill color, like Processing.
const DIR_LUX: f32 = 3_500.0;
const AMBIENT_NITS: f32 = 1_500.0;
// The ambient light that shows a texture at its own colors when the sketch has
// no lights (textured shapes are drawn as rough metal: all ambient, no white sheen).
#[allow(dead_code)]
const TEX_AMBIENT: f32 = 1.47;
/// Processing's point and spot lights don't fade with distance (unless
/// lightFalloff says so); libprocessing's fade with its square. So a light is
/// made as bright as it takes to light the middle of the canvas at full
/// strength from where it is.
fn point_lumens(p: [f32; 3], w: u32, h: u32) -> f32 {
    let d = Vec3::new(p[0] - w as f32 / 2.0, p[1] - h as f32 / 2.0, p[2]).length().max(1.0);
    POINT_LUMENS_PER_UNIT2 * d * d
}
const POINT_LUMENS_PER_UNIT2: f32 = 2.2e5;
fn begin_frame_lights() {
    WANT.with(|w| w.borrow_mut().clear());
    LIT_DRAWN.with(|d| d.set(false));
    LIT.with(|l| l.set(false));
    // A spot light kept from the last frame doesn't light this one on some
    // GPUs (one made in the frame always does), so spot lights are made anew
    // each frame. Points and directionals are kept.
    let had_spot = HAVE.with(|h| h.borrow().0.iter().any(|s| matches!(s, LightSpec::Spot(..))));
    if had_spot {
        let old = HAVE.with(|h| std::mem::take(&mut *h.borrow_mut()));
        let _ = processing_core::app_mut(|app| {
            for e in &old.1 { if let Ok(em) = app.world_mut().get_entity_mut(*e) { em.despawn(); } }
            Ok(())
        });
    }
}
/// Brings the scene's lights in line with what this frame asked for.
fn sync_lights() {
    let want = WANT.with(|w| w.borrow().clone());
    let same = HAVE.with(|h| h.borrow().0 == want);
    if same { return; }
    let Some((main, (w, h))) = LP.with(|l| l.borrow().as_ref().map(|l| (l.main, l.size))) else { return };
    let old = HAVE.with(|h| std::mem::take(&mut h.borrow_mut().1));
    let _ = processing_core::app_mut(|app| {
        for e in &old { if let Ok(em) = app.world_mut().get_entity_mut(*e) { em.despawn(); } }
        Ok(())
    });
    // Processing's coordinates (y down, origin top left) to libprocessing's world
    let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut ambient = [0.0f32; 3];
    let mut made = Vec::new();
    for spec in &want {
        match *spec {
            LightSpec::Ambient(c) => for i in 0..3 { ambient[i] += c[i]; },
            LightSpec::Directional(c, d) => {
                match pr::light_create_directional(main, BColor::linear_rgb(c[0], c[1], c[2]), DIR_LUX) {
                    Ok(e) => {
                        let dir = Vec3::new(d[0], -d[1], d[2]);
                        let _ = pr::transform_set_position(e, -dir * 10.0);
                        let _ = pr::transform_look_at(e, Vec3::ZERO);
                        made.push(e);
                    }
                    Err(e) => fail("a directional light", e),
                }
            }
            LightSpec::Spot(c, p, d, angle) => {
                let range = (w.max(h) as f32) * 4.0;
                let outer = angle.clamp(0.01, std::f32::consts::FRAC_PI_2 - 0.01);
                let lumens = point_lumens(p, w, h);
                // reach just past the canvas: Bevy culls a spot light by a sphere
                // along its cone, and a far reach puts that sphere out of view
                let reach = Vec3::new(p[0] - hw, p[1] - hh, p[2]).length() + w.max(h) as f32 * 2.0;
                let _ = range;
                match pr::light_create_spot(main, BColor::linear_rgb(c[0], c[1], c[2]), lumens, reach, 0.0, outer * 0.8, outer) {
                    Ok(e) => {
                        let pos = Vec3::new(p[0] - hw, -(p[1] - hh), p[2]);
                        let _ = pr::transform_set_position(e, pos);
                        let _ = pr::transform_look_at(e, pos + Vec3::new(d[0], -d[1], d[2]));
                        made.push(e);
                    }
                    Err(e) => fail("a spot light", e),
                }
            }
            LightSpec::Point(c, p) => {
                let range = (w.max(h) as f32) * 4.0;
                match pr::light_create_point(main, BColor::linear_rgb(c[0], c[1], c[2]), point_lumens(p, w, h), range * 4.0, 0.0) {
                    Ok(e) => {
                        let _ = pr::transform_set_position(e, Vec3::new(p[0] - hw, -(p[1] - hh), p[2]));
                        made.push(e);
                    }
                    Err(e) => fail("a point light", e),
                }
            }
        }
    }
    let _ = processing_core::app_mut(|app| {
        let brightness = if want.is_empty() { 80.0 } else { AMBIENT_NITS };
        let color = if want.is_empty() { BColor::WHITE } else { BColor::linear_rgb(ambient[0], ambient[1], ambient[2]) };
        app.world_mut().insert_resource(bevy::light::GlobalAmbientLight { color, brightness, affects_lightmapped_meshes: true });
        // new lights get their place in the world in PostUpdate; without
        // this they would light the scene only from the next render on
        let _ = &made;
        Ok(())
    });
    // a new light shows from the render after the one it was made in, so
    // render once now, before anything is drawn with it
    let fresh = !made.is_empty();
    HAVE.with(|h| *h.borrow_mut() = (want, made));
    if fresh { let _ = flush_keep(main); }
}

// ── 3D lines, points and triangles: small meshes made for the frame ──────
thread_local! { static FRAME_GEOMETRY: RefCell<Vec<Entity>> = const { RefCell::new(Vec::new()) }; }
fn mesh(topology: pr::geometry::Topology, pts: &[[f32; 3]], normal: Option<[f32; 3]>, color: Option<Color>) -> Option<Entity> {
    let g = match pr::geometry_create(topology) { Ok(g) => g, Err(e) => { fail("making a mesh", e); return None; } };
    if let Some(n) = normal { let _ = pr::geometry_normal(g, Vec3::from_array(n)); }
    if let Some(c) = color { let _ = pr::geometry_color(g, Vec4::new(c.r, c.g, c.b, c.a)); }
    for p in pts { let _ = pr::geometry_vertex(g, Vec3::from_array(*p)); }
    FRAME_GEOMETRY.with(|f| f.borrow_mut().push(g));
    Some(g)
}
/// line_3d / point_3d: in the stroke color, unlit, one pixel wide.
pub(crate) fn line_3d(a: [f32; 3], b: [f32; 3], stroke: Option<Color>) {
    let Some(c) = stroke else { return };
    let topology = if a == b { pr::geometry::Topology::PointList } else { pr::geometry::Topology::LineList };
    let pts: Vec<[f32; 3]> = if a == b { vec![a] } else { vec![a, b] };
    let Some(g) = mesh(topology, &pts, None, Some(c)) else { return };
    cmd(DrawCommand::PushStyle);
    cmd(DrawCommand::Fill(col(c)));
    cmd(DrawCommand::Unlit);
    cmd(DrawCommand::Geometry(g));
    cmd(DrawCommand::PopStyle);
}
/// triangle_3d: filled with the current fill (and lit, if there are lights);
/// the outline goes on as three 3D lines.
pub(crate) fn triangle_3d(p: [[f32; 3]; 3], fill_c: Option<Color>, stroke: Option<Color>) {
    if fill_c.is_some() {
        let u = Vec3::from_array(p[1]) - Vec3::from_array(p[0]);
        let v = Vec3::from_array(p[2]) - Vec3::from_array(p[0]);
        let n = u.cross(v).normalize_or_zero();
        if let Some(g) = mesh(pr::geometry::Topology::TriangleList, &p, Some(n.to_array()), None) { cmd(DrawCommand::Geometry(g)); }
    }
    if stroke.is_some() {
        line_3d(p[0], p[1], stroke); line_3d(p[1], p[2], stroke); line_3d(p[2], p[0], stroke);
    }
}
/// A begin_shape mesh in 3D (or with a texture): triangles with their own
/// texture coordinates (0 to 1). Filled with the texture when there is one.
pub(crate) fn mesh_3d(verts: &[[f32; 3]], uvs: &[[f32; 2]], tris: &[[usize; 3]], tex: Option<Entity>) {
    if tris.is_empty() { return; }
    #[cfg(lp_no_texture)]
    let tex: Option<Entity> = { let _ = tex; None };
    // flat (unlit) shapes take their color from the vertices
    let fill_c = crate::render::style_pub().0;
    let vc = match (tex, fill_c) {
        (Some(_), _) => TINT.with(|t| t.get()).unwrap_or(Color::from_rgb(1.0, 1.0, 1.0)),
        (None, Some(c)) => c,
        (None, None) => return,
    };
    let g = match pr::geometry_create(pr::geometry::Topology::TriangleList) { Ok(g) => g, Err(e) => { fail("making a mesh", e); return; } };
    let _ = pr::geometry_color(g, Vec4::new(vc.r, vc.g, vc.b, vc.a));
    for t in tris {
        let (a, b, c) = (Vec3::from_array(verts[t[0]]), Vec3::from_array(verts[t[1]]), Vec3::from_array(verts[t[2]]));
        let _ = pr::geometry_normal(g, (b - a).cross(c - a).normalize_or_zero());
        for &i in t {
            let uv = uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            let _ = pr::geometry_uv(g, uv[0], uv[1]);
            let _ = pr::geometry_vertex(g, Vec3::from_array(verts[i]));
        }
    }
    FRAME_GEOMETRY.with(|f| f.borrow_mut().push(g));
    match tex {
        #[cfg(not(lp_no_texture))]
        Some(e) => {
            let t = TINT.with(|t| t.get()).unwrap_or(Color::from_rgb(1.0, 1.0, 1.0));
            cmd(DrawCommand::PushStyle);
            // libprocessing's textures are always lit: without lights in the
            // sketch, a full white ambient light shows them at their own colors
            if !LIT.with(|l| l.get()) {
                WANT.with(|w| { let mut w = w.borrow_mut(); if !w.contains(&LightSpec::Ambient([TEX_AMBIENT; 3])) { w.push(LightSpec::Ambient([TEX_AMBIENT; 3])); } });
                cmd(DrawCommand::Roughness(1.0));
                cmd(DrawCommand::Metallic(1.0));
            }
            cmd(DrawCommand::Fill(col(t)));
            // textureWrap and noSmooth, as the image's sampler
            let filter = if crate::render::smooth_on() { 0 } else { 1 };
            let wrap = crate::render::texture_wrap_mode();
            let _ = pr::image_set_sampler(e, filter, wrap, wrap);
            cmd(DrawCommand::Texture(e));
            cmd(DrawCommand::Geometry(g));
            cmd(DrawCommand::NoTexture);
            cmd(DrawCommand::PopStyle);
        }
        _ => cmd(DrawCommand::Geometry(g)),
    }
}
fn free_frame_geometry() {
    for g in FRAME_GEOMETRY.with(|f| std::mem::take(&mut *f.borrow_mut())) { let _ = pr::geometry_destroy(g); }
}

// ── text ─────────────────────────────────────────────────────────────────
thread_local! { static ALIGN: std::cell::Cell<(u8, u8)> = const { std::cell::Cell::new((0, 0)) }; }
/// libprocessing lines text up inside a box; a plain text(s, x, y) is placed
/// here, line by line, so CENTER and RIGHT work like in Processing.
pub(crate) fn text(s: &str, x: f32, y: f32) {
    let (h, v) = ALIGN.with(|a| a.get());
    if h == 0 {
        text_cmd(DrawCommand::Text { content: s.to_string(), x, y, z: 0.0, max_w: None, max_h: None });
        return;
    }
    send_align(0, v);
    for (i, line) in s.split('\n').enumerate() {
        let w = text_width(line);
        let lx = if h == 1 { x - w / 2.0 } else { x - w };
        let ly = y + i as f32 * line_height();
        text_cmd(DrawCommand::Text { content: line.to_string(), x: lx, y: ly, z: 0.0, max_w: None, max_h: None });
    }
    send_align(h, v);
}
/// text(s, x, y, w, h): libprocessing wraps and aligns inside the box itself.
/// Vertical CENTER / BOTTOM inside the box are worked out here.
pub(crate) fn text_box(s: &str, x: f32, y: f32, w: f32, h: f32) {
    let (ha, va) = ALIGN.with(|a| a.get());
    if va == 1 || va == 2 {
        let lines = crate::render::wrap_text(s, w).len().max(1) as f32;
        let lh = line_height();
        let block = (lines * lh).min(h);
        let top = if va == 1 { y + (h - block) / 2.0 } else { y + h - block };
        send_align(ha, 0);
        text_cmd(DrawCommand::Text { content: s.to_string(), x, y: top, z: 0.0, max_w: Some(w), max_h: Some(h - (top - y)) });
        send_align(ha, va);
        return;
    }
    text_cmd(DrawCommand::Text { content: s.to_string(), x, y, z: 0.0, max_w: Some(w), max_h: Some(h) });
}
/// Text is drawn in the fill color only, like Processing (no outline).
fn text_cmd(c: DrawCommand) {
    cmd(DrawCommand::PushStyle);
    cmd(DrawCommand::NoStroke);
    cmd(c);
    cmd(DrawCommand::PopStyle);
}
fn line_height() -> f32 {
    LEADING.with(|l| l.get()).unwrap_or_else(|| (text_ascent() + text_descent()) * 1.25)
}
thread_local! { static LEADING: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) }; }
pub(crate) fn text_size(px: f32) { cmd(DrawCommand::TextSize(px)); }
pub(crate) fn text_leading(px: f32) { LEADING.with(|l| l.set(Some(px))); cmd(DrawCommand::TextLeading(px)); }
/// engine h: 0 left 1 center 2 right; v: 0 top 1 center 2 bottom 3 baseline
pub(crate) fn text_align(h: u8, v: u8) {
    ALIGN.with(|a| a.set((h, v)));
    send_align(h, v);
}
fn send_align(h: u8, v: u8) {
    let h = match h { 1 => TextAlignH::Center, 2 => TextAlignH::Right, _ => TextAlignH::Left };
    let v = match v { 1 => TextAlignV::Center, 2 => TextAlignV::Bottom, 3 => TextAlignV::Baseline, _ => TextAlignV::Top };
    cmd(DrawCommand::TextAlign { h, v });
}
/// The canvas to ask about text, with the text style set so far applied.
fn text_target() -> Option<Entity> {
    ensure_open();
    let g = target()?;
    if DIRTY.load(Ordering::Relaxed) { let _ = flush_keep(g); }
    Some(g)
}
pub(crate) fn text_width(s: &str) -> f32 {
    text_target().and_then(|g| pr::graphics_text_width(g, s).ok()).unwrap_or(0.0)
}
pub(crate) fn text_ascent() -> f32 { text_target().and_then(|g| pr::graphics_text_ascent(g).ok()).unwrap_or(0.0) }
pub(crate) fn text_descent() -> f32 { text_target().and_then(|g| pr::graphics_text_descent(g).ok()).unwrap_or(0.0) }
/// The engine has already found the font file; load it here too and remember it.
pub(crate) fn font_loaded(id: u64, path: &std::path::Path) {
    match pr::font_load(&path.to_string_lossy()) {
        Ok(e) => LP.with(|l| { if let Some(l) = l.borrow_mut().as_mut() { l.fonts.insert(id, e); } }),
        Err(e) => fail("loading the font", e),
    }
}
pub(crate) fn text_font(id: u64, size: f32) {
    let e = LP.with(|l| l.borrow().as_ref().and_then(|l| l.fonts.get(&id).copied()));
    cmd(DrawCommand::TextFont(e));
    if size > 0.0 { text_size(size); }
}

// ── images ───────────────────────────────────────────────────────────────
/// Uploads (or re-uploads after an edit) an engine image, by its id.
pub(crate) fn image_entity(id: u64, version: u64, w: usize, h: usize, rgba: impl FnOnce() -> Vec<u8>) -> Option<Entity> {
    let have = LP.with(|l| l.borrow().as_ref().and_then(|l| l.images.get(&id).copied()));
    if let Some((e, v)) = have { if v == version { return Some(e); } }
    let size = Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 };
    let e = match pr::image_create(size, rgba(), TextureFormat::Rgba8Unorm) {
        Ok(e) => e,
        Err(e) => { fail("uploading an image", e); return None; }
    };
    LP.with(|l| {
        if let Some(l) = l.borrow_mut().as_mut() {
            if let Some((old, _)) = l.images.insert(id, (e, version)) { let _ = pr::image_destroy(old); }
        }
    });
    Some(e)
}
#[allow(dead_code)]
pub(crate) fn image(e: Entity, x: f32, y: f32, w: f32, h: f32) {
    image_alpha(e, x, y, w, h, false);
}
/// libprocessing only blends an image when the tint is see-through, so an
/// image with transparent pixels is drawn with a tint a hair under opaque.
pub(crate) fn image_alpha(e: Entity, x: f32, y: f32, w: f32, h: f32, has_alpha: bool) {
    let t = TINT.with(|t| t.get());
    let nudge = has_alpha && t.map(|c| c.a >= 0.999).unwrap_or(true);
    if nudge {
        let base = t.unwrap_or(Color::from_rgb(1.0, 1.0, 1.0));
        cmd(DrawCommand::Tint(col(Color::from_rgba(base.r, base.g, base.b, 0.9999))));
    }
    cmd(DrawCommand::Image { entity: e, dx: x, dy: y, d_width: Some(w), d_height: Some(h), sx: None, sy: None, s_width: None, s_height: None });
    if nudge {
        cmd(match t { Some(c) => DrawCommand::Tint(col(c)), None => DrawCommand::NoTint });
    }
}

// ── pixels: filled in with a readback ────────────────────────────────────
/// The whole canvas (or layer) as 0xAARRGGBB, row by row.
pub(crate) fn read_pixels() -> Option<(Vec<u32>, usize, usize)> {
    ensure_open();
    let g = target()?;
    let (w, h) = target_size()?;
    sync_lights();
    // the readback renders what's waiting itself: no flush of our own (each
    // one is a whole engine update)
    let px = match readback_keep(g) { Ok(p) => p, Err(e) => { fail("reading pixels", e); return None; } };
    let all: Vec<u32> = px.iter().map(|s: &LinearRgba| {
        let s = &unpremultiply(*s);
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        (b(s.alpha) << 24) | (b(s.red) << 16) | (b(s.green) << 8) | b(s.blue)
    }).collect();
    let (w, h) = (w as usize, h as usize);
    if all.len() == w * h { return Some((all, w, h)); }
    // The canvas came back a different size than the sketch's (a window
    // manager that won't make the window that small, say). Keep the sketch's
    // part, from the top left.
    let n = all.len();
    let (aw, ah) = if n % h == 0 && n / h >= w { (n / h, h) } else if n % w == 0 { (w, n / w) } else { (0, 0) };
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("libprocessing: the canvas is {aw}x{ah} pixels, the sketch {w}x{h}; get() and saveFrame() use the top left {w}x{h}");
    }
    if std::env::var_os("LP_DEBUG_READBACK").is_some() && aw > 0 {
        let bytes: Vec<u8> = all.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]).collect();
        if let Ok(f) = std::fs::File::create("readback-raw.png") {
            let mut enc = png::Encoder::new(std::io::BufWriter::new(f), aw as u32, ah as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            if let Ok(mut wr) = enc.write_header() { let _ = wr.write_image_data(&bytes); }
        }
    }
    if aw == 0 { return None; }
    let mut out = vec![0u32; w * h];
    for y in 0..h.min(ah) { for x in 0..w.min(aw) { out[y * w + x] = all[y * aw + x]; } }
    Some((out, w, h))
}
// Copies what libprocessing has drawn into the engine's own pixel buffer,
// so get(), loadPixels(), filter() and saveFrame() can work on it. Only
// reads back when something was drawn since the last time.
thread_local! { static PULLED: std::cell::Cell<Option<Entity>> = const { std::cell::Cell::new(None) }; }
pub(crate) fn pull() {
    // read back when something was drawn, or when it's a different canvas
    // (a layer, or the window again) than last time
    let t = target();
    let same = PULLED.with(|p| p.get()) == t && t.is_some();
    if !DIRTY.swap(false, Ordering::Relaxed) && same { return; }
    PULLED.with(|p| p.set(target()));
    let Some((px, w, h)) = read_pixels() else { return };
    let (cw, ch) = (crate::CANVAS_W.load(Ordering::Relaxed) as usize, crate::CANVAS_H.load(Ordering::Relaxed) as usize);
    if (w, h) != (cw, ch) || px.len() < w * h { return; }
    // layers keep their transparency in the top byte, like the software renderer
    let keep = if crate::render::alpha_target_pub() { 0xFFFF_FFFF } else { 0x00FF_FFFF };
    let mut buf = crate::pixel_buf().lock();
    buf.clear();
    buf.extend(px.iter().map(|p| p & keep));
}
/// set(x, y, c): a one pixel square.
pub(crate) fn set_pixel(x: i32, y: i32, c: Color) {
    cmd(DrawCommand::PushMatrix);
    cmd(DrawCommand::ResetMatrix);
    processing_coords();
    cmd(DrawCommand::PushStyle);
    cmd(DrawCommand::NoStroke);
    cmd(DrawCommand::Fill(col(c)));
    cmd(DrawCommand::BlendMode(None));
    rect(x as f32, y as f32, 1.0, 1.0, [0.0; 4]);
    cmd(DrawCommand::PopStyle);
    cmd(DrawCommand::PopMatrix);
}
/// Puts a whole canvas of 0xAARRGGBB pixels back.
pub(crate) fn write_pixels(px: &[u32], w: usize, h: usize) { write_pixels_as(px, w, h, crate::render::alpha_target_pub()) }
fn write_pixels_as(px: &[u32], w: usize, h: usize, layer: bool) {
    let rgba: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, if layer { (p >> 24) as u8 } else { 0xFF }]).collect();
    let size = Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 };
    match pr::image_create(size, rgba, TextureFormat::Rgba8Unorm) {
        Ok(e) => {
            cmd(DrawCommand::PushMatrix);
            cmd(DrawCommand::ResetMatrix);
            processing_coords();
            cmd(DrawCommand::PushStyle);
            cmd(DrawCommand::NoTint);
            // the pixels replace what's there, transparent ones included: a layer
            // is cleared to transparent first, the canvas is opaque anyway
            if layer { cmd(DrawCommand::BackgroundColor(BColor::srgba(0.0, 0.0, 0.0, 0.0))); }
            else { cmd(DrawCommand::BlendMode(pr::render::command::BlendMode::Replace.to_blend_state())); }
            corner_modes();
            image_alpha(e, 0.0, 0.0, w as f32, h as f32, layer);
            cmd(DrawCommand::PopStyle);
            cmd(DrawCommand::PopMatrix);
            // the image has to live until the frame is drawn; let it go next frame
            PENDING_FREE.with(|f| f.borrow_mut().push(e));
        }
        Err(e) => fail("writing pixels", e),
    }
}
thread_local! { static PENDING_FREE: RefCell<Vec<Entity>> = const { RefCell::new(Vec::new()) }; }

// ── layers (createGraphics): offscreen graphics ─────────────────────────
pub(crate) fn create_layer(id: u64, w: u32, h: u32) -> bool {
    ensure_open();
    let surface = match pr::surface_create_offscreen(w, h, 1.0, TextureFormat::Rgba16Float) {
        Ok(s) => s, Err(e) => { fail("making a layer", e); return false; }
    };
    let g = match pr::graphics_create(surface, w, h, TextureFormat::Rgba16Float) {
        Ok(g) => g, Err(e) => { fail("making a layer", e); return false; }
    };
    LP.with(|l| { if let Some(l) = l.borrow_mut().as_mut() { l.layers.insert(id, (g, w, h)); } });
    if let Err(e) = pr::graphics_mode_3d(g) { fail("3D mode for a layer", e); }
    default_projection(g, w, h);
    let _ = pr::graphics_record_command(g, DrawCommand::StrokeJoin(StrokeJoinMode::Miter));
    // cleared to transparent on its first beginDraw(), not now: starting and
    // ending another canvas in the middle of a frame upsets this one
    FRESH.with(|f| f.borrow_mut().push(g));
    true
}
pub(crate) fn begin_layer(id: u64) -> bool {
    let ok = LP.with(|l| {
        let mut l = l.borrow_mut();
        let Some(l) = l.as_mut() else { return false };
        let Some(&(g, ..)) = l.layers.get(&id) else { return false };
        let _ = pr::graphics_begin_draw(g);
        // a layer begun inside another goes back to that one when it ends
        TARGETS.with(|t| t.borrow_mut().push(l.target));
        l.target = g;
        true
    });
    if ok {
        let fresh = LP.with(|l| l.borrow().as_ref().map(|l| l.target))
            .map(|g| FRESH.with(|f| { let mut f = f.borrow_mut(); let had = f.contains(&g); f.retain(|x| *x != g); had }))
            .unwrap_or(false);
        if fresh { cmd(DrawCommand::BackgroundColor(BColor::srgba(0.0, 0.0, 0.0, 0.0))); }
    }
    if ok { processing_coords(); corner_modes(); }
    ok
}
thread_local! { static FRESH: RefCell<Vec<Entity>> = const { RefCell::new(Vec::new()) }; }
thread_local! {
    /// What each layer looked like when its endDraw() came: (rgba, width, height).
    static LAYER_PIXELS: RefCell<HashMap<Entity, (Vec<u8>, u32, u32)>> = RefCell::new(HashMap::new());
}
/// endDraw(): the layer's pixels are read now, while it is still the one
/// being drawn on, and kept for drawing it later.
pub(crate) fn end_layer() {
    let Some((g, w, h)) = LP.with(|l| {
        let l = l.borrow();
        let l = l.as_ref()?;
        if l.target == l.main { return None; }
        l.layers.values().find(|v| v.0 == l.target).copied()
    }) else { return };
    match readback_keep(g) {
        Ok(px) => {
            let rgba: Vec<u8> = px.iter().flat_map(|s: &LinearRgba| {
                let s = &unpremultiply(*s);
                let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                [b(s.red), b(s.green), b(s.blue), b(s.alpha)]
            }).collect();
            LAYER_PIXELS.with(|m| m.borrow_mut().insert(g, (rgba, w, h)));
        }
        Err(e) => fail("reading a layer", e),
    }
    LP.with(|l| {
        let mut l = l.borrow_mut();
        let Some(l) = l.as_mut() else { return };
        let _ = pr::graphics_end_draw(l.target);
        l.target = TARGETS.with(|t| t.borrow_mut().pop()).unwrap_or(l.main);
    });
}
/// Draws a layer: read it back and draw it as an image.
pub(crate) fn draw_layer(id: u64, x: f32, y: f32, w: f32, h: f32) {
    let Some((g, lw, lh)) = LP.with(|l| l.borrow().as_ref().and_then(|l| l.layers.get(&id).copied())) else { return };
    // a layer never drawn on is transparent
    let rgba = LAYER_PIXELS.with(|m| m.borrow().get(&g).map(|v| v.0.clone()))
        .unwrap_or_else(|| vec![0u8; (lw * lh * 4) as usize]);
    if rgba.len() != (lw * lh * 4) as usize { return; }
    let size = Extent3d { width: lw, height: lh, depth_or_array_layers: 1 };
    if let Ok(e) = pr::image_create(size, rgba, TextureFormat::Rgba8Unorm) {
        image_alpha(e, x, y, w, h, true);
        PENDING_FREE.with(|f| f.borrow_mut().push(e));
    }
}

// ── the window: resizable, cursor, full screen ──────────────────────────
thread_local! {
    static APPLIED: std::cell::Cell<Option<(bool, bool, Option<(usize, usize, isize, isize)>)>> = const { std::cell::Cell::new(None) };
    static CURSOR_APPLIED: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}
/// Brings the window in line with windowResizable(), cursor() / noCursor()
/// and fullScreen(); only touches what changed.
fn sync_window() {
    let want = crate::glfw::window_wants();
    let had = APPLIED.with(|a| a.get());
    // the cursor's look
    let kind = crate::glfw::cursor_kind();
    if CURSOR_APPLIED.with(|c| c.get()) != Some(kind) {
        if let Some(s) = LP.with(|l| l.borrow().as_ref().map(|l| l.surface)) {
            CURSOR_APPLIED.with(|c| c.set(Some(kind)));
            use bevy::window::SystemCursorIcon as I;
            let icon = match kind { crate::CROSS => I::Crosshair, crate::TEXT => I::Text, crate::WAIT => I::Wait,
                                    crate::HAND => I::Pointer, crate::MOVE => I::Move, _ => I::Default };
            if let Err(e) = input::input_set_cursor_icon(s, icon) { fail("cursor", e); }
        }
    }
    // a picture: the sketch's cursor(img), or a drawn one for WAIT and MOVE,
    // which GLFW has no cursors for
    #[cfg(lp_cursor_image)]
    {
        thread_local! { static PIC_APPLIED: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) }; }
        let (gen, pic) = crate::glfw::cursor_picture(false);
        if PIC_APPLIED.with(|c| c.get()) != Some(gen) {
            PIC_APPLIED.with(|c| c.set(Some(gen)));
            LP.with(|l| {
                if let Some(g) = l.borrow_mut().as_mut().and_then(|l| l.glfw.as_mut()) {
                    g.set_cursor_image(pic.as_ref().map(|p| (p.0 as u32, p.1 as u32, p.2.as_slice(), p.3 as u32, p.4 as u32)));
                }
            });
        }
    }
    if had == Some(want) { return; }
    let Some(s) = LP.with(|l| l.borrow().as_ref().map(|l| l.surface)) else { return };
    let (resizable, cursor, present) = want;
    if had.map(|h| h.0) != Some(resizable) { if let Err(e) = pr::surface_set_resizable(s, resizable) { fail("windowResizable", e); } }
    if had.map(|h| h.1) != Some(cursor) { if let Err(e) = input::input_set_cursor_visible(s, cursor) { fail("cursor", e); } }
    if had.map(|h| h.2) != Some(present) {
        if let Some((_, _, x, y)) = present {
            let _ = pr::surface_set_decorated(s, false);
            let _ = pr::surface_set_position(s, x as i32, y as i32);
        }
    }
    APPLIED.with(|a| a.set(Some(want)));
}

// ── the frame loop ───────────────────────────────────────────────────────
fn key_code_of(k: bevy::input::keyboard::KeyCode) -> u32 {
    use bevy::input::keyboard::KeyCode as K;
    // the engine's key codes (see the constants in lib.rs)
    match k {
        K::ArrowUp => crate::UP, K::ArrowDown => crate::DOWN, K::ArrowLeft => crate::LEFT, K::ArrowRight => crate::RIGHT,
        K::Enter | K::NumpadEnter => crate::ENTER, K::Backspace => crate::BACKSPACE, K::Delete => crate::DELETE,
        K::Escape => crate::ESCAPE, K::Tab => crate::TAB, K::Home => crate::HOME, K::End => crate::END,
        K::PageUp => crate::PAGE_UP, K::PageDown => crate::PAGE_DOWN,
        K::ShiftLeft | K::ShiftRight => crate::SHIFT, K::ControlLeft | K::ControlRight => crate::CONTROL,
        K::F1 => crate::F1, K::F2 => crate::F2, K::F3 => crate::F3, K::F4 => crate::F4, K::F5 => crate::F5,
        K::F6 => crate::F6, K::F7 => crate::F7, K::F8 => crate::F8, K::F9 => crate::F9, K::F10 => crate::F10,
        K::F11 => crate::F11, K::F12 => crate::F12,
        _ => {
            // letters and digits: 0..9 are 0..9, A..Z are 10..35, like the other renderer
            let name = format!("{k:?}");
            if let Some(d) = name.strip_prefix("Digit").and_then(|d| d.parse::<u32>().ok()) { return d; }
            if let Some(c) = name.strip_prefix("Key").and_then(|c| c.chars().next()) {
                if c.is_ascii_uppercase() { return 10 + (c as u32 - 'A' as u32); }
            }
            0
        }
    }
}

fn sync(start: std::time::Instant, frame: u64) {
    let Some(s) = LP.with(|l| l.borrow().as_ref().map(|l| l.main_surface())) else { return };
    let mut g = crate::globals().write();
    g.pmouse_x = input::input_pmouse_x(s).unwrap_or(g.mouse_x);
    g.pmouse_y = input::input_pmouse_y(s).unwrap_or(g.mouse_y);
    g.mouse_x = input::input_mouse_x(s).unwrap_or(g.mouse_x);
    g.mouse_y = input::input_mouse_y(s).unwrap_or(g.mouse_y);
    g.mouse_pressed = input::input_mouse_is_pressed().unwrap_or(false);
    g.key_pressed = input::input_key_is_pressed().unwrap_or(false);
    if let Ok(Some(c)) = input::input_key() { g.key = c; }
    if let Ok(Some(k)) = input::input_key_code() {
        let code = key_code_of(k);
        if code != 0 { g.key_code = code; }
        // special keys have no character; Processing reports CODED for those
        if matches!(k, bevy::input::keyboard::KeyCode::ArrowUp | bevy::input::keyboard::KeyCode::ArrowDown
            | bevy::input::keyboard::KeyCode::ArrowLeft | bevy::input::keyboard::KeyCode::ArrowRight) {
            g.key = '\u{FFFF}';
        }
    }
    let (w, h) = LP.with(|l| l.borrow().as_ref().map(|l| l.size)).unwrap_or((100, 100));
    g.width = w as f32;
    g.height = h as f32;
    g.frame_count = frame;
    g.millis = start.elapsed().as_millis() as u64;
    g.focused = true;
    if let Ok(b) = input::input_mouse_button() {
        use bevy::input::mouse::MouseButton as B;
        let v = match b { Some(B::Left) => crate::LEFT, Some(B::Right) => crate::RIGHT, Some(B::Middle) => crate::CENTER, _ => 0 };
        if v != 0 { crate::glfw::MOUSE_BUTTON.store(v, Ordering::Relaxed); }
    }
}

impl Lp {
    fn main_surface(&self) -> Entity { self.surface }
}

/// Runs a sketch on libprocessing: setup, then draw() every frame with the
/// event callbacks in between, until the window closes.
pub(crate) fn run<S: 'static>(mut app: crate::App<S>) {
    let start = std::time::Instant::now();
    sync(start, 0);
    crate::render::begin_draw();
    IN_SETUP.store(true, Ordering::Relaxed);
    begin_frame_lights();
    EARLY.with(|e| e.borrow_mut().push(DrawCommand::Unlit));
    // Processing's default join is MITER; libprocessing's own default is ROUND
    EARLY.with(|e| e.borrow_mut().push(DrawCommand::StrokeJoin(StrokeJoinMode::Miter)));
    (app.setup_fn)(&mut app.state);
    IN_SETUP.store(false, Ordering::Relaxed);
    { use std::io::Write; let _ = std::io::stdout().flush(); }
    // no size() in setup(): Processing's 100 x 100, with what setup() drew
    ensure_open();
    let title = app.title.clone();
    if title != "sketch" { set_title(&title); }
    sync_lights();
    finish_frame();
    free_frame_geometry();

    let mut frame: u64 = 0;
    let mut first = true;
    #[cfg(lp_window_state)]
    let mut watch = crate::WinWatch::default();
    loop {
        let frame_start = std::time::Instant::now();
        let alive = LP.with(|l| l.borrow_mut().as_mut().and_then(|l| l.glfw.as_mut().map(|g| g.poll_events()))).unwrap_or(false);
        if !alive { break; }
        sync_window();
        sync(start, frame);
        // events, like Processing: before draw()
        if input::input_mouse_any_just_pressed().unwrap_or(false) { crate::click_pressed(); if let Some(f) = app.mouse_pressed_fn { f(&mut app.state); } }
        if input::input_mouse_any_just_released().unwrap_or(false) {
            if let Some(f) = app.mouse_released_fn { f(&mut app.state); }
            if crate::click_released() { if let Some(f) = app.mouse_clicked_fn { f(&mut app.state); } }
        }
        if input::input_mouse_moved().unwrap_or(false) {
            let pressed = input::input_mouse_is_pressed().unwrap_or(false);
            if pressed { crate::click_dragged(); }
            let f = if pressed { app.mouse_dragged_fn } else { app.mouse_moved_fn };
            if let Some(f) = f { f(&mut app.state); }
        }
        if input::input_mouse_scrolled().unwrap_or(false) {
            let d = input::input_mouse_wheel().unwrap_or(0.0);
            crate::__set_wheel_delta(d);
            if let Some(f) = app.mouse_wheel_fn { f(&mut app.state, d); }
        }
        if input::input_key_any_just_pressed().unwrap_or(false) {
            if let Some(f) = app.key_pressed_fn { f(&mut app.state); }
            if let Some(f) = app.key_typed_fn { f(&mut app.state); }
        }
        if input::input_key_any_just_released().unwrap_or(false) { if let Some(f) = app.key_released_fn { f(&mut app.state); } }
        // focus, the mouse coming and going, the window moving or resizing
        #[cfg(lp_window_state)]
        {
            let st = LP.with(|l| l.borrow().as_ref().and_then(|l| l.glfw.as_ref().and_then(|g| g.window_state())));
            let size = LP.with(|l| l.borrow().as_ref().map(|l| l.size)).unwrap_or((0, 0));
            if let Some((focused, inside, pos)) = st {
                for w in watch.update(focused, inside, size, Some(pos)) { app.win_fns.call(w, &mut app.state); }
            }
        }

        let draw = first || crate::glfw::is_looping() || crate::glfw::take_redraw();
        if draw {
            first = false;
            frame += 1;
            sync(start, frame);
            let _ = pr::advance_frame_count();
            let main = LP.with(|l| l.borrow().as_ref().map(|l| l.main)).unwrap();
            start_canvas(main);
            corner_modes();
            crate::render::begin_draw();
            begin_frame_lights();
            cmd(DrawCommand::Unlit);
            // nothing is drawn yet, only settings: the first background() of
            // the frame needn't render what came before (a whole engine update)
            DRAWN.store(false, Ordering::Relaxed);
            (app.draw_fn)(&mut app.state);
            crate::render::end_draw();
            sync_lights();
            finish_frame();
            free_frame_geometry();
            { use std::io::Write; let _ = std::io::stdout().flush(); }
            // images made for this frame (pixels, layers) can go now
            for e in PENDING_FREE.with(|f| std::mem::take(&mut *f.borrow_mut())) { let _ = pr::image_destroy(e); }
        }
        else { present_pending(); }
        { use std::io::Write; let _ = std::io::stdout().flush(); }
        // frameRate(): sleep off what's left of the frame
        let want = std::time::Duration::from_secs_f32(1.0 / crate::glfw::current_frame_rate_target().max(1.0));
        let spent = frame_start.elapsed();
        if spent < want { std::thread::sleep(want - spent); }
    }
    shutdown();
}

/// Ends the frame. A shape libprocessing's tessellator chokes on makes it
/// panic; that frame is lost, the sketch goes on.
fn end_draw_guarded(g: Entity) {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pr::graphics_end_draw(g)));
    if r.is_err() { missing("drawing one of the shapes in this frame (libprocessing couldn't tessellate it)"); }
}

/// Tears libprocessing down cleanly (wgpu complains if it's just dropped).
pub(crate) fn shutdown() {
    // the app (and its surfaces) go first, the windows they draw into after
    let windows = LP.try_with(|l| l.borrow_mut().take()).ok().flatten();
    if let Some(mut app) = processing_core::take_app() {
        app.world_mut().write_message(bevy::app::AppExit::Success);
        app.update();
        drop(app);
    }
    drop(windows);
}
