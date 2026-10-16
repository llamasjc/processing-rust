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

use bevy::color::{Color as BColor, LinearRgba, Srgba};
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
        let want = std::env::var("PROCESSING_RENDERER").map(|v| v.eq_ignore_ascii_case("libprocessing")).unwrap_or(false);
        ACTIVE.store(want, Ordering::Relaxed);
        CHECKED.store(true, Ordering::Relaxed);
    }
    ACTIVE.load(Ordering::Relaxed)
}

struct Lp {
    glfw: Option<GlfwContext>,
    surface: Entity,
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
        .set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() })
        .disable::<bevy::winit::WinitPlugin>()
        .disable::<bevy::log::LogPlugin>()
        .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
        // no sound for now: don't open the sound card
        .disable::<bevy::audio::AudioPlugin>()
        .set(WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() });
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
        let Some(l) = l.as_mut() else { return false };
        if l.size != (w, h) {
            if let Err(e) = pr::surface_resize(l.main, w, h) { fail("resize", e); }
            l.size = (w, h);
        }
        true
    });
    if resized { return; }
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
    let main = match pr::graphics_create(surface, w, h, TextureFormat::Rgba16Float) {
        Ok(g) => g,
        Err(e) => { fail("making the canvas", e); std::process::exit(1); }
    };
    let _ = pr::surface_set_title(surface, "sketch".into());
    LP.with(|l| *l.borrow_mut() = Some(Lp {
        glfw: Some(glfw), surface, main, target: main, size: (w, h),
        images: HashMap::new(), fonts: HashMap::new(), layers: HashMap::new(), warned: Vec::new(),
    }));
    // Always libprocessing's 3D camera, turned to Processing's coordinates
    // (see processing_coords): 2D drawing lands on the same pixels, box() and
    // sphere() get perspective, and translucent fills work (libprocessing's
    // 2D mode drops them for now).
    if let Err(e) = pr::graphics_mode_3d(main) { fail("3D mode", e); }
    let _ = pr::graphics_begin_draw(main);
    processing_coords();
    // start in step with the engine: its shape modes, text alignment and size
    corner_modes();
    let (th, tv, ts) = crate::render::text_state();
    text_align(th, tv);
    text_size(ts);
    // Processing's gray, then whatever setup() drew before size()
    cmd(DrawCommand::BackgroundColor(BColor::srgb(0.8, 0.8, 0.8)));
    let early = EARLY.with(|e| std::mem::take(&mut *e.borrow_mut()));
    for c in early { cmd(c); }
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

static DIRTY: AtomicBool = AtomicBool::new(true);

/// Records a draw command on whatever is being drawn on.
pub(crate) fn cmd(c: DrawCommand) {
    if !finite(&c) { return; }
    DIRTY.store(true, Ordering::Relaxed);
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
        DrawCommand::Rotate { angle, .. } => ok(&[*angle]),
        DrawCommand::StrokeWeight(w) | DrawCommand::TextSize(w) => ok(&[*w]),
        DrawCommand::Text { x, y, .. } => ok(&[*x, *y]),
        DrawCommand::Image { dx, dy, d_width, d_height, .. } => ok(&[*dx, *dy, d_width.unwrap_or(0.0), d_height.unwrap_or(0.0)]),
        DrawCommand::Box { width, height, depth } => ok(&[*width, *height, *depth]),
        DrawCommand::Sphere { radius, .. } => ok(&[*radius]),
        _ => true,
    }
}

fn col(c: Color) -> BColor { BColor::srgba(c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0), c.a.clamp(0.0, 1.0)) }

// ── style ────────────────────────────────────────────────────────────────
pub(crate) fn background(c: Color) { cmd(DrawCommand::BackgroundColor(col(c))); }
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
pub(crate) fn blend_mode(m: u8) {
    let mode = pr::render::command::BlendMode::try_from(m).unwrap_or_default();
    cmd(DrawCommand::BlendMode(mode.to_blend_state()));
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
pub(crate) fn rect(x: f32, y: f32, w: f32, h: f32, radii: [f32; 4]) { cmd(DrawCommand::Rect { x, y, w, h, radii }); }
pub(crate) fn ellipse(cx: f32, cy: f32, w: f32, h: f32) { cmd(DrawCommand::Ellipse { cx, cy, w, h }); }
pub(crate) fn line(x1: f32, y1: f32, x2: f32, y2: f32) { cmd(DrawCommand::Line { x1, y1, x2, y2 }); }
pub(crate) fn point(x: f32, y: f32) { cmd(DrawCommand::Point { x, y }); }
pub(crate) fn triangle(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) {
    cmd(DrawCommand::Triangle { x1, y1, x2, y2, x3, y3 });
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn quad(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32, x4: f32, y4: f32) {
    cmd(DrawCommand::Quad { x1, y1, x2, y2, x3, y3, x4, y4 });
}
/// engine arc modes: 0 default (Processing draws a pie fill and an open stroke), 1 OPEN, 2 CHORD, 3 PIE
pub(crate) fn arc(cx: f32, cy: f32, w: f32, h: f32, start: f32, stop: f32, mode: u8) {
    let mode = match mode { 2 => ArcMode::Chord, 3 => ArcMode::Pie, 1 => ArcMode::Open, _ => ArcMode::Open };
    cmd(DrawCommand::Arc { cx, cy, w, h, start, stop, mode });
}
pub(crate) fn begin_shape(kind: u8) {
    // engine kinds: 0 polygon, 1 POINTS, 2 LINES, 3 TRIANGLES, 4 TRIANGLE_FAN, 5 TRIANGLE_STRIP, 6 QUADS, 7 QUAD_STRIP
    let kind = match kind {
        1 => ShapeKind::Points, 2 => ShapeKind::Lines, 3 => ShapeKind::Triangles, 4 => ShapeKind::TriangleFan,
        5 => ShapeKind::TriangleStrip, 6 => ShapeKind::Quads, 7 => ShapeKind::QuadStrip, _ => ShapeKind::Polygon,
    };
    cmd(DrawCommand::BeginShape { kind });
}
pub(crate) fn vertex(x: f32, y: f32) { cmd(DrawCommand::ShapeVertex { x, y }); }
pub(crate) fn bezier_vertex(cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
    cmd(DrawCommand::ShapeBezierVertex { cx1, cy1, cx2, cy2, x, y });
}
pub(crate) fn quadratic_vertex(cx: f32, cy: f32, x: f32, y: f32) { cmd(DrawCommand::ShapeQuadraticVertex { cx, cy, x, y }); }
pub(crate) fn curve_vertex(x: f32, y: f32) { cmd(DrawCommand::ShapeCurveVertex { x, y }); }
pub(crate) fn end_shape(close: bool) { cmd(DrawCommand::EndShape { close }); }
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
    let Some((w, h)) = target_size() else { return };
    cmd(DrawCommand::Scale(Vec3::new(1.0, -1.0, 1.0)));
    cmd(DrawCommand::Translate(Vec3::new(-(w as f32) / 2.0, -(h as f32) / 2.0, 0.0)));
}

pub(crate) fn box_3d(w: f32, h: f32, d: f32) { cmd(DrawCommand::Box { width: w, height: h, depth: d }); }
pub(crate) fn sphere(r: f32, sectors: u32, stacks: u32) { cmd(DrawCommand::Sphere { radius: r, sectors, stacks }); }
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
    if let Some(g) = target() {
        if let Err(e) = pr::graphics_perspective(g, fov, aspect, near, far, Vec4::new(0.0, 0.0, -1.0, -near)) { fail("perspective", e); }
    }
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
    let _ = pr::graphics_flush(g);
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
pub(crate) enum LightSpec { Ambient([f32; 3]), Directional([f32; 3], [f32; 3]), Point([f32; 3], [f32; 3]) }
thread_local! {
    static WANT: RefCell<Vec<LightSpec>> = const { RefCell::new(Vec::new()) };
    static HAVE: RefCell<(Vec<LightSpec>, Vec<Entity>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}
// Shapes after the first light of a frame are lit (PBR); before it, and
// after no_lights(), they are flat colors like Processing without lights.
thread_local! { static LIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
pub(crate) fn add_light(spec: LightSpec) {
    WANT.with(|w| w.borrow_mut().push(spec));
    if !LIT.with(|l| l.replace(true)) { cmd(DrawCommand::Roughness(0.9)); cmd(DrawCommand::Metallic(0.0)); }
}
pub(crate) fn lights() {
    add_light(LightSpec::Ambient([0.5; 3]));
    add_light(LightSpec::Directional([0.5; 3], [0.0, 0.0, -1.0]));
}
/// noLights(): shapes from here on are flat again. (The lights stay in the
/// scene for the shapes drawn before: a libprocessing scene has one set.)
pub(crate) fn no_lights() {
    LIT.with(|l| l.set(false));
    cmd(DrawCommand::Unlit);
}
// How bright a light of full strength (255) is, tuned so lights() on a
// surface facing the camera gives about its fill color, like Processing.
const DIR_LUX: f32 = 3_500.0;
const AMBIENT_NITS: f32 = 1_500.0;
// The ambient light that shows a texture at its own colors when the sketch has
// no lights (textured shapes are drawn as rough metal: all ambient, no white sheen).
const TEX_AMBIENT: f32 = 1.47;
const POINT_LUMENS: f32 = 1.5e8;
fn begin_frame_lights() { WANT.with(|w| w.borrow_mut().clear()); LIT.with(|l| l.set(false)); }
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
            LightSpec::Point(c, p) => {
                let range = (w.max(h) as f32) * 4.0;
                match pr::light_create_point(main, BColor::linear_rgb(c[0], c[1], c[2]), POINT_LUMENS, range, 0.0) {
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
        Ok(())
    });
    HAVE.with(|h| *h.borrow_mut() = (want, made));
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
    if DIRTY.load(Ordering::Relaxed) { let _ = pr::graphics_flush(g); }
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
    let e = match pr::image_create(size, rgba(), TextureFormat::Rgba8UnormSrgb) {
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
    let _ = pr::graphics_flush(g);
    let px = match pr::graphics_readback(g) { Ok(p) => p, Err(e) => { fail("reading pixels", e); return None; } };
    let out = px.iter().map(|c: &LinearRgba| {
        let s: Srgba = (*c).into();
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        (b(s.alpha) << 24) | (b(s.red) << 16) | (b(s.green) << 8) | b(s.blue)
    }).collect();
    Some((out, w as usize, h as usize))
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
    match pr::image_create(size, rgba, TextureFormat::Rgba8UnormSrgb) {
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
    let _ = pr::graphics_flush(g);
    match pr::graphics_readback(g) {
        Ok(px) => {
            let rgba: Vec<u8> = px.iter().flat_map(|c: &LinearRgba| {
                let s: Srgba = (*c).into();
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
        l.target = l.main;
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
    if let Ok(e) = pr::image_create(size, rgba, TextureFormat::Rgba8UnormSrgb) {
        image_alpha(e, x, y, w, h, true);
        PENDING_FREE.with(|f| f.borrow_mut().push(e));
    }
}

// ── the window: resizable, cursor, full screen ──────────────────────────
thread_local! {
    static APPLIED: std::cell::Cell<Option<(bool, bool, Option<(usize, usize, isize, isize)>)>> = const { std::cell::Cell::new(None) };
}
/// Brings the window in line with windowResizable(), cursor() / noCursor()
/// and fullScreen(); only touches what changed.
fn sync_window() {
    let want = crate::glfw::window_wants();
    let had = APPLIED.with(|a| a.get());
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
    (app.setup_fn)(&mut app.state);
    IN_SETUP.store(false, Ordering::Relaxed);
    { use std::io::Write; let _ = std::io::stdout().flush(); }
    // no size() in setup(): Processing's 100 x 100, with what setup() drew
    ensure_open();
    let title = app.title.clone();
    if title != "sketch" { set_title(&title); }
    let main = LP.with(|l| l.borrow().as_ref().map(|l| l.main)).unwrap();
    sync_lights();
    end_draw_guarded(main);
    free_frame_geometry();

    let mut frame: u64 = 0;
    let mut first = true;
    loop {
        let frame_start = std::time::Instant::now();
        let alive = LP.with(|l| l.borrow_mut().as_mut().and_then(|l| l.glfw.as_mut().map(|g| g.poll_events()))).unwrap_or(false);
        if !alive { break; }
        sync_window();
        sync(start, frame);
        // events, like Processing: before draw()
        if input::input_mouse_any_just_pressed().unwrap_or(false) { if let Some(f) = app.mouse_pressed_fn { f(&mut app.state); } }
        if input::input_mouse_any_just_released().unwrap_or(false) { if let Some(f) = app.mouse_released_fn { f(&mut app.state); } }
        if input::input_mouse_moved().unwrap_or(false) {
            let pressed = input::input_mouse_is_pressed().unwrap_or(false);
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

        let draw = first || crate::glfw::is_looping() || crate::glfw::take_redraw();
        if draw {
            first = false;
            frame += 1;
            sync(start, frame);
            let _ = pr::advance_frame_count();
            let _ = pr::graphics_begin_draw(main);
            processing_coords();
            corner_modes();
            crate::render::begin_draw();
            begin_frame_lights();
            cmd(DrawCommand::Unlit);
            (app.draw_fn)(&mut app.state);
            crate::render::end_draw();
            sync_lights();
            end_draw_guarded(main);
            free_frame_geometry();
            { use std::io::Write; let _ = std::io::stdout().flush(); }
            // images made for this frame (pixels, layers) can go now
            for e in PENDING_FREE.with(|f| std::mem::take(&mut *f.borrow_mut())) { let _ = pr::image_destroy(e); }
        }
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
