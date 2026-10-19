//! loadShader / shader.set / filter(shader): Processing's filter shaders.
//!
//!   let blur = load_shader("blur.glsl");
//!   blur.set("radius", 3.0);
//!   filter(&blur);
//!
//! The GLSL runs on the CPU with the software renderer and as a WGSL filter
//! on the GPU with libprocessing (see glsl.rs).

use crate::glsl;
use parking_lot::Mutex;
use std::collections::HashMap;

pub(crate) struct Entry {
    pub cpu: glsl::Cpu,
    /// the vertex shader, when load_shader_vf gave one
    pub vert: Option<glsl::Cpu>,
    /// its GLSL, for libprocessing
    #[cfg_attr(not(feature = "libprocessing"), allow(dead_code))]
    pub vert_prog: Option<glsl::Program>,
    /// the fragment shader's GLSL, to remake the material with a vertex stage
    #[cfg_attr(not(feature = "libprocessing"), allow(dead_code))]
    pub frag_prog: glsl::Program,
    #[cfg_attr(not(feature = "libprocessing"), allow(dead_code))]
    pub wgsl: Result<String, String>,
    /// the same shader as a material, for shader()
    #[cfg_attr(not(feature = "libprocessing"), allow(dead_code))]
    pub material: Result<String, String>,
    pub values: HashMap<String, Vec<f32>>,
    /// images set on sampler2D uniforms
    pub textures: HashMap<String, u64>,
    pub version: u64,
    pub name: String,
}

pub(crate) fn shaders() -> &'static Mutex<Vec<Entry>> {
    static S: std::sync::OnceLock<Mutex<Vec<Entry>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

/// A shader from load_shader. Cheap to copy; set() changes it everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PShader { pub(crate) id: usize }

/// Values set() takes: numbers, booleans, 2 to 4 numbers, a color, or an
/// image (its id) for a sampler2D.
pub trait UniformValue {
    fn floats(&self) -> Vec<f32>;
    fn image(&self) -> Option<u64> { None }
}
impl UniformValue for u64 { fn floats(&self) -> Vec<f32> { Vec::new() } fn image(&self) -> Option<u64> { Some(*self) } }
impl UniformValue for &u64 { fn floats(&self) -> Vec<f32> { Vec::new() } fn image(&self) -> Option<u64> { Some(**self) } }
impl UniformValue for f32 { fn floats(&self) -> Vec<f32> { vec![*self] } }
impl UniformValue for f64 { fn floats(&self) -> Vec<f32> { vec![*self as f32] } }
impl UniformValue for i32 { fn floats(&self) -> Vec<f32> { vec![*self as f32] } }
impl UniformValue for bool { fn floats(&self) -> Vec<f32> { vec![if *self { 1.0 } else { 0.0 }] } }
impl<const N: usize> UniformValue for [f32; N] { fn floats(&self) -> Vec<f32> { self.to_vec() } }
impl UniformValue for (f32, f32) { fn floats(&self) -> Vec<f32> { vec![self.0, self.1] } }
impl UniformValue for (f32, f32, f32) { fn floats(&self) -> Vec<f32> { vec![self.0, self.1, self.2] } }
impl UniformValue for (f32, f32, f32, f32) { fn floats(&self) -> Vec<f32> { vec![self.0, self.1, self.2, self.3] } }
impl UniformValue for &[f32] { fn floats(&self) -> Vec<f32> { self.to_vec() } }
impl UniformValue for Vec<f32> { fn floats(&self) -> Vec<f32> { self.clone() } }
impl UniformValue for crate::Color { fn floats(&self) -> Vec<f32> { vec![self.r, self.g, self.b, self.a] } }

impl PShader {
    /// set("name", value): a uniform. Numbers, bools, [f32; 2..4], tuples, colors.
    pub fn set(&self, name: &str, v: impl UniformValue) {
        self.set_value(name, v);
        // the shader shapes are using now takes it from the next shape on
        #[cfg(feature = "libprocessing")]
        if crate::lp::on() && *LP_ACTIVE.lock() == Some(*self) { shader(*self); }
    }
    fn set_value(&self, name: &str, v: impl UniformValue) {
        let mut all = shaders().lock();
        let Some(e) = all.get_mut(self.id.wrapping_sub(1)) else { return };
        let in_vert = e.vert.as_ref().is_some_and(|v| v.has_uniform(name));
        if !e.cpu.has_uniform(name) && !in_vert {
            eprintln!("{}: set(\"{name}\"): the shader has no uniform called that", e.name);
            return;
        }
        // a sampler2D takes an image; from Lua that arrives as its number
        let sampler = e.cpu.uniforms.get(name).or_else(|| e.vert.as_ref().and_then(|v| v.uniforms.get(name))).map(|u| u.1) == Some(glsl::Ty::Sampler);
        let img = v.image().or_else(|| if sampler { v.floats().first().map(|x| *x as u64) } else { None });
        if let Some(img) = img {
            e.textures.insert(name.to_string(), img);
            e.version += 1;
            return;
        }
        let vals = v.floats();
        e.cpu.set_uniform(name, &vals);
        e.values.insert(name.to_string(), vals);
        e.version += 1;
    }
    /// set2 / set3 / set4: Processing's set(name, x, y[, z[, w]]).
    pub fn set2(&self, name: &str, x: impl crate::Num, y: impl crate::Num) { self.set(name, [x.f(), y.f()]) }
    pub fn set3(&self, name: &str, x: impl crate::Num, y: impl crate::Num, z: impl crate::Num) { self.set(name, [x.f(), y.f(), z.f()]) }
    pub fn set4(&self, name: &str, x: impl crate::Num, y: impl crate::Num, z: impl crate::Num, w: impl crate::Num) { self.set(name, [x.f(), y.f(), z.f(), w.f()]) }
    /// Whether it loaded; a shader that didn't does nothing when used.
    pub fn is_ok(&self) -> bool { self.id != 0 }
}

/// load_shader("frag.glsl"): a filter shader (looked for as given, then in
/// data/). On a mistake it says where and gives back a shader that does nothing.
pub fn load_shader(path: &str) -> PShader {
    let file = if std::path::Path::new(path).is_file() { path.to_string() } else { format!("data/{path}") };
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) => { eprintln!("loadShader(\"{path}\"): {e}"); return PShader::default(); }
    };
    shader_from_source_named(&text, path)
}
/// load_shader_vf(frag, vert): a fragment and a vertex shader, for shader().
/// The vertex shader gets Processing's attributes (position, color, normal,
/// texCoord) and matrices (transform, modelview, projection, normalMatrix,
/// texMatrix), and its varyings reach the fragment shader.
pub fn load_shader_vf(frag: &str, vert: &str) -> PShader {
    let s = load_shader(frag);
    if s.id == 0 { return s; }
    let file = if std::path::Path::new(vert).is_file() { vert.to_string() } else { format!("data/{vert}") };
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) => { eprintln!("loadShader(\"{vert}\"): {e}"); return s; }
    };
    let prog = match glsl::parse(&text) { Ok(p) => p, Err(e) => { eprintln!("loadShader(\"{vert}\"): {e}"); return s; } };
    let cpu = match glsl::compile(&prog) { Ok(c) => c, Err(e) => { eprintln!("loadShader(\"{vert}\"): {e}"); return s; } };
    let mut all = shaders().lock();
    let e = &mut all[s.id - 1];
    attach_vertex(e, cpu, prog);
    s
}
fn attach_vertex(e: &mut Entry, cpu: glsl::Cpu, prog: glsl::Program) {
    e.material = glsl::to_wgsl_material_vf(&e.frag_prog, &prog);
    if std::env::var_os("PROCESSING_SHADER_DEBUG").is_some() {
        eprintln!("── {} with its vertex shader ──\n{}", e.name, e.material.as_ref().map(|s| s.as_str()).unwrap_or_else(|e| e));
    }
    e.vert = Some(cpu);
    e.vert_prog = Some(prog);
    e.version += 1;
}
/// load_shader_vf with GLSL text: fragment, then vertex.
pub fn shader_from_source_vf(frag: &str, vert: &str) -> PShader {
    let s = shader_from_source(frag);
    if s.id == 0 { return s; }
    let prog = match glsl::parse(vert) { Ok(p) => p, Err(e) => { eprintln!("vertex shader: {e}"); return s; } };
    let cpu = match glsl::compile(&prog) { Ok(c) => c, Err(e) => { eprintln!("vertex shader: {e}"); return s; } };
    let mut all = shaders().lock();
    let e = &mut all[s.id - 1];
    attach_vertex(e, cpu, prog);
    s
}
/// A shader from GLSL text in the sketch.
pub fn shader_from_source(src: &str) -> PShader { shader_from_source_named(src, "shader") }

fn shader_from_source_named(src: &str, name: &str) -> PShader {
    let prog = match glsl::parse(src) {
        Ok(p) => p,
        Err(e) => { eprintln!("loadShader(\"{name}\"): {e}"); return PShader::default(); }
    };
    let cpu = match glsl::compile(&prog) {
        Ok(c) => c,
        Err(e) => { eprintln!("loadShader(\"{name}\"): {e}"); return PShader::default(); }
    };
    let wgsl = glsl::to_wgsl(&prog);
    let material = glsl::to_wgsl_for(&prog, glsl::Target::Material);
    // PROCESSING_SHADER_DEBUG=1 prints what libprocessing gets
    if std::env::var_os("PROCESSING_SHADER_DEBUG").is_some() {
        eprintln!("── {name} as a filter ──\n{}", wgsl.as_ref().map(|s| s.as_str()).unwrap_or_else(|e| e));
        eprintln!("── {name} as a material ──\n{}", material.as_ref().map(|s| s.as_str()).unwrap_or_else(|e| e));
    }
    let mut all = shaders().lock();
    all.push(Entry { cpu, vert: None, vert_prog: None, frag_prog: prog, wgsl, material, values: HashMap::new(), textures: HashMap::new(), version: 0, name: name.to_string() });
    PShader { id: all.len() }
}
#[inline] pub fn loadShader(path: &str) -> PShader { load_shader(path) }

/// What filter() takes: a kind (BLUR, INVERT...) or a shader.
pub trait FilterKind { fn apply(&self); }
impl FilterKind for u32 { fn apply(&self) { crate::render::filter(*self, f32::NAN); } }
impl FilterKind for i32 { fn apply(&self) { crate::render::filter(*self as u32, f32::NAN); } }
impl FilterKind for PShader { fn apply(&self) { apply(*self) } }
impl FilterKind for &PShader { fn apply(&self) { apply(**self) } }
impl FilterKind for &mut PShader { fn apply(&self) { apply(**self) } }

/// Runs the shader over the canvas.
pub(crate) fn apply(s: PShader) {
    if s.id == 0 { return; }
    #[cfg(feature = "libprocessing")]
    if crate::lp::on() {
        let (src, values, version, name) = {
            let all = shaders().lock();
            let Some(e) = all.get(s.id - 1) else { return };
            let vals: Vec<(String, glsl::Ty, Vec<f32>)> = e.values.iter()
                .filter_map(|(k, v)| e.cpu.uniforms.get(k).map(|(_, t)| (k.clone(), *t, v.clone())))
                .collect();
            (e.wgsl.clone(), vals, e.version, (e.name.clone(), e.textures.clone()))
        };
        let (src, values, version, name, textures) = (src, values, version, name.0, name.1);
        let textures: Vec<(String, bevy::prelude::Entity)> = textures.iter()
            .filter_map(|(n, id)| crate::render::lp_image(*id).map(|e| (format!("tex_{n}"), e))).collect();
        match src {
            Ok(src) => crate::lp::filter_shader(s.id, &src, &values, &textures, version),
            Err(why) => {
                static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("{name}: can't run on libprocessing: {why}");
                }
            }
        }
        return;
    }
    let (w, h) = (crate::CANVAS_W.load(std::sync::atomic::Ordering::Relaxed) as usize, crate::CANVAS_H.load(std::sync::atomic::Ordering::Relaxed) as usize);
    let (mut cpu, textures) = {
        let all = shaders().lock();
        let Some(e) = all.get(s.id - 1) else { return };
        (e.cpu.clone(), e.textures.clone())
    };
    // the images, numbered from 1 (0 is the canvas)
    let mut images = Vec::new();
    for (name, id) in &textures {
        let (iw, ih) = crate::render::image_size(*id);
        let px: Vec<u32> = crate::render::image_pixels(*id).iter().map(|c| {
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
            (b(c.a) << 24) | (b(c.r) << 16) | (b(c.g) << 8) | b(c.b)
        }).collect();
        images.push((iw as usize, ih as usize, px));
        cpu.set_uniform(name, &[images.len() as f32]);
    }
    // Processing fills these in for filter shaders
    if !shaders().lock()[s.id - 1].values.contains_key("texOffset") { cpu.set_uniform("texOffset", &[1.0 / w as f32, 1.0 / h as f32]); }
    if !shaders().lock()[s.id - 1].values.contains_key("resolution") { cpu.set_uniform("resolution", &[w as f32, h as f32]); }
    let src = crate::render::copy_pixels();
    if src.len() < w * h { return; }
    let mut dst = vec![0u32; w * h];
    cpu.run(w, h, &src, &mut dst, &images);
    crate::render::write_pixels(&dst);
}

// ── shader(): shapes drawn with a shader ─────────────────────────────────

static ACTIVE: Mutex<Option<PShader>> = Mutex::new(None);
/// The shader shapes use on libprocessing, so set() on it reaches the GPU.
#[cfg(feature = "libprocessing")]
static LP_ACTIVE: Mutex<Option<PShader>> = Mutex::new(None);

/// shader(s): shapes drawn after it are colored by the shader (its fragment
/// part: vertColor is the fill). reset_shader() goes back to plain fills.
pub fn shader(s: impl std::borrow::Borrow<PShader>) {
    let s = *s.borrow();
    if s.id == 0 { return; }
    #[cfg(feature = "libprocessing")]
    if crate::lp::on() {
        let (src, values, textures, version) = {
            let all = shaders().lock();
            let Some(e) = all.get(s.id - 1) else { return };
            let vals: Vec<(String, glsl::Ty, Vec<f32>)> = e.values.iter()
                .filter_map(|(k, v)| e.cpu.uniforms.get(k).or_else(|| e.vert.as_ref().and_then(|vc| vc.uniforms.get(k)))
                    .map(|(_, t)| (k.clone(), *t, v.clone()))).collect();
            (e.material.clone(), vals, e.textures.clone(), e.version)
        };
        let textures: Vec<(String, bevy::prelude::Entity)> = textures.iter()
            .filter_map(|(n, id)| crate::render::lp_image(*id).map(|e| (n.clone(), e))).collect();
        *LP_ACTIVE.lock() = Some(s);
        match src {
            Ok(src) => crate::lp::shape_shader(s.id, &src, &values, &textures, version),
            Err(why) => eprintln!("shader(): can't run on libprocessing: {why}"),
        }
        return;
    }
    *ACTIVE.lock() = Some(s);
}
/// reset_shader(): shapes are plainly filled again.
pub fn reset_shader() {
    #[cfg(feature = "libprocessing")]
    if crate::lp::on() { *LP_ACTIVE.lock() = None; crate::lp::reset_shape_shader(); return; }
    *ACTIVE.lock() = None;
}
#[inline] pub fn resetShader() { reset_shader() }

/// The active shader's vertex stage, with its uniforms set (software renderer).
pub(crate) fn active_vertex() -> Option<glsl::Cpu> {
    let s = (*ACTIVE.lock())?;
    let all = shaders().lock();
    let e = all.get(s.id - 1)?;
    let mut v = e.vert.clone()?;
    for (k, vals) in &e.values { v.set_uniform(k, vals); }
    Some(v)
}
/// Is a shader with a vertex stage active? Then every shape goes through 3D.
pub(crate) fn vertex_active() -> bool {
    let Some(s) = *ACTIVE.lock() else { return false };
    shaders().lock().get(s.id - 1).is_some_and(|e| e.vert.is_some())
}

/// The shader shapes are drawn with now, ready to run (software renderer).
pub(crate) fn active_cpu(w: usize, h: usize) -> Option<(glsl::Cpu, Vec<(usize, usize, Vec<u32>)>)> {
    let s = (*ACTIVE.lock())?;
    let all = shaders().lock();
    let e = all.get(s.id - 1)?;
    let mut cpu = e.cpu.clone();
    let mut images = Vec::new();
    for (name, id) in &e.textures {
        let (iw, ih) = crate::render::image_size(*id);
        let px: Vec<u32> = crate::render::image_pixels(*id).iter().map(|c| {
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
            (b(c.a) << 24) | (b(c.r) << 16) | (b(c.g) << 8) | b(c.b)
        }).collect();
        images.push((iw as usize, ih as usize, px));
        cpu.set_uniform(name, &[images.len() as f32]);
    }
    if !e.values.contains_key("texOffset") { cpu.set_uniform("texOffset", &[1.0 / w as f32, 1.0 / h as f32]); }
    if !e.values.contains_key("resolution") { cpu.set_uniform("resolution", &[w as f32, h as f32]); }
    Some((cpu, images))
}

/// Uniform values with arrays split into their items (`name_0`, `name_1`...),
/// the way the WGSL in glsl.rs lays them out.
#[cfg_attr(not(feature = "libprocessing"), allow(dead_code))]
pub(crate) fn by_item(values: &[(String, glsl::Ty, Vec<f32>)]) -> Vec<(String, glsl::Ty, Vec<f32>)> {
    let mut out = Vec::new();
    for (n, t, v) in values {
        if let glsl::Ty::Arr(..) = t {
            let et = t.elem();
            let k = et.size_pub().max(1);
            for i in 0..t.len() {
                let chunk: Vec<f32> = (0..k).map(|j| v.get(i * k + j).copied().unwrap_or(0.0)).collect();
                out.push((format!("{n}_{i}"), et, chunk));
            }
        } else {
            out.push((n.clone(), *t, v.clone()));
        }
    }
    out
}
