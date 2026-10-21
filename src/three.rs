//! 3D on the software renderer: box, sphere, line_3d, a camera, a depth
//! buffer and simple lights.
//!
//! translate / rotate / scale / push_matrix / pop_matrix move 3D shapes too,
//! and rotate_x / rotate_y turn them in depth. With the default camera a
//! point at z = 0 lands on the same pixel as in 2D, like Processing's P3D.

use crate::{Color, Num, CANVAS_H, CANVAS_W};
use std::cell::RefCell;
use std::sync::atomic::Ordering;

type M4 = [f32; 16]; // column-major

const IDENT: M4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

fn mul(a: &M4, b: &M4) -> M4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}
fn apply(m: &M4, p: [f32; 3]) -> [f32; 4] {
    let [x, y, z] = p;
    [m[0] * x + m[4] * y + m[8] * z + m[12],
     m[1] * x + m[5] * y + m[9] * z + m[13],
     m[2] * x + m[6] * y + m[10] * z + m[14],
     m[3] * x + m[7] * y + m[11] * z + m[15]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn norm(a: [f32; 3]) -> [f32; 3] { let l = dot(a, a).sqrt(); if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { a } }

/// A light, as Processing has them: its kind, color, the specular color and
/// falloff in effect when it was made, and for a spot its cone.
#[derive(Clone, Copy)]
enum Kind { Ambient, Directional([f32; 3]), Point([f32; 3]), Spot { pos: [f32; 3], dir: [f32; 3], cos_angle: f32, conc: f32 } }
#[derive(Clone, Copy)]
struct Light { kind: Kind, c: [f32; 3], spec: [f32; 3], fall: [f32; 3] }
#[allow(non_snake_case)]
fn Ambient(c: [f32; 3]) -> Light { Light { kind: Kind::Ambient, c, spec: [0.0; 3], fall: [1.0, 0.0, 0.0] } }
#[allow(non_snake_case)]
fn Directional(c: [f32; 3], d: [f32; 3]) -> Light { Light { kind: Kind::Directional(d), c, spec: [0.0; 3], fall: [1.0, 0.0, 0.0] } }

/// The material: ambient (None: the fill), specular, emissive, shininess.
#[derive(Clone, Copy)]
struct Material { ambient: Option<[f32; 3]>, specular: [f32; 3], emissive: [f32; 3], shininess: f32 }
const DEFAULT_MATERIAL: Material = Material { ambient: None, specular: [0.5; 3], emissive: [0.0; 3], shininess: 1.0 };

struct State {
    stack: Vec<M4>,
    view: M4,
    proj: M4,
    eye: [f32; 3],
    custom_camera: bool,
    custom_proj: bool,
    lights: Vec<Light>,
    material: Material,
    light_spec: [f32; 3],
    light_fall: [f32; 3],
    sphere_detail: (usize, usize),
    depth: Vec<f32>,
    depth_frame: u64,
    depth_dims: (usize, usize),
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State {
        stack: vec![IDENT], view: IDENT, proj: IDENT, eye: [0.0, 0.0, 1.0],
        custom_camera: false, custom_proj: false, lights: Vec::new(),
        material: DEFAULT_MATERIAL, light_spec: [0.0; 3], light_fall: [1.0, 0.0, 0.0],
        sphere_detail: (30, 30), depth: Vec::new(), depth_frame: u64::MAX, depth_dims: (0, 0),
    });
}

fn dims() -> (usize, usize) { (CANVAS_W.load(Ordering::Relaxed) as usize, CANVAS_H.load(Ordering::Relaxed) as usize) }

fn top_mut(f: impl FnOnce(&mut M4)) { S.with(|s| f(s.borrow_mut().stack.last_mut().unwrap())); }
fn post(m: M4) { top_mut(|t| *t = mul(t, &m)); }

// ── hooks called by the 2D transform functions ─────────────────────────────
pub(crate) fn on_push() { S.with(|s| { let mut s = s.borrow_mut(); let t = *s.stack.last().unwrap(); s.stack.push(t); }); }
pub(crate) fn on_pop() { S.with(|s| { let mut s = s.borrow_mut(); if s.stack.len() > 1 { s.stack.pop(); } }); }
pub(crate) fn on_translate(x: f32, y: f32, z: f32) {
    let mut m = IDENT; m[12] = x; m[13] = y; m[14] = z; post(m);
}
pub(crate) fn on_scale(x: f32, y: f32, z: f32) {
    let mut m = IDENT; m[0] = x; m[5] = y; m[10] = z; post(m);
}
pub(crate) fn on_rotate_z(a: f32) {
    let (c, s) = (a.cos(), a.sin());
    let mut m = IDENT; m[0] = c; m[1] = s; m[4] = -s; m[5] = c; post(m);
}
pub(crate) fn on_reset() { top_mut(|t| *t = IDENT); }
/// Start of every frame: matrices back to the start, depth cleared, lights off.
pub(crate) fn on_begin_frame() {
    S.with(|s| {
        let mut s = s.borrow_mut();
        s.stack.clear();
        s.stack.push(IDENT);
        s.lights.clear();
        s.depth_frame = u64::MAX; // cleared on first use
    });
}

pub fn rotate_x(a: impl Num) {
    let a = a.f();
    lp_route! { crate::lp::rotate(a, bevy::math::Vec3::X); } let (c, s) = (a.cos(), a.sin());
    let mut m = IDENT; m[5] = c; m[6] = s; m[9] = -s; m[10] = c; post(m);
}
pub fn rotate_y(a: impl Num) {
    let a = a.f();
    lp_route! { crate::lp::rotate(a, bevy::math::Vec3::Y); } let (c, s) = (a.cos(), a.sin());
    let mut m = IDENT; m[0] = c; m[2] = -s; m[8] = s; m[10] = c; post(m);
}

// ── camera ─────────────────────────────────────────────────────────────────
fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> M4 {
    let f = norm(sub(center, eye));
    let s = norm(cross(f, up));
    let u = cross(s, f);
    [s[0], u[0], -f[0], 0.0,
     s[1], u[1], -f[1], 0.0,
     s[2], u[2], -f[2], 0.0,
     -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0]
}
fn persp_matrix(fovy: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let f = 1.0 / (fovy / 2.0).tan();
    let mut m = [0.0; 16];
    m[0] = f / aspect; m[5] = f; m[10] = (far + near) / (near - far); m[11] = -1.0;
    m[14] = 2.0 * far * near / (near - far);
    m
}
fn default_eye_z(h: f32) -> f32 { (h / 2.0) / (std::f32::consts::PI / 6.0).tan() }

/// camera(): looks at the middle of the canvas so z = 0 matches 2D.
/// camera_look(eye_x, eye_y, eye_z, center_x, center_y, center_z, up_x, up_y, up_z)
#[allow(clippy::too_many_arguments)]
pub fn camera_look(ex: impl Num, ey: impl Num, ez: impl Num, cx: impl Num, cy: impl Num, cz: impl Num,
                   ux: impl Num, uy: impl Num, uz: impl Num) {
    let eye = [ex.f(), ey.f(), ez.f()];
    lp_route! { crate::lp::camera(eye, [cx.f(), cy.f(), cz.f()], [ux.f(), uy.f(), uz.f()]); return; }
    let v = look_at(eye, [cx.f(), cy.f(), cz.f()], [ux.f(), uy.f(), uz.f()]);
    S.with(|s| { let mut s = s.borrow_mut(); s.view = v; s.eye = eye; s.custom_camera = true; });
}
pub fn camera() {
    lp_route! { crate::lp::camera_default(); return; }
    S.with(|s| s.borrow_mut().custom_camera = false);
}
/// perspective(fov, aspect, near, far). fov in radians, PI/3 by default.
pub fn perspective(fov: impl Num, aspect: impl Num, near: impl Num, far: impl Num) {
    lp_route! { crate::lp::perspective(fov.f(), aspect.f(), near.f().max(0.001), far.f()); return; }
    let p = persp_matrix(fov.f(), aspect.f(), near.f().max(0.001), far.f());
    S.with(|s| { let mut s = s.borrow_mut(); s.proj = p; s.custom_proj = true; });
}
/// perspective(): back to the default.
pub fn perspective_default() { S.with(|s| s.borrow_mut().custom_proj = false); }
/// ortho(): no perspective, sizes don't shrink with distance.
pub fn ortho() {
    let (w, h) = dims();
    let (w, h) = (w as f32, h as f32);
    let ez = default_eye_z(h);
    let (n, f) = (ez / 10.0, ez * 10.0);
    lp_route! { crate::lp::ortho(w, h, n, f); return; }
    let mut m = IDENT;
    m[0] = 2.0 / w; m[5] = 2.0 / h; m[10] = -2.0 / (f - n); m[14] = -(f + n) / (f - n);
    S.with(|s| { let mut s = s.borrow_mut(); s.proj = m; s.custom_proj = true; });
}

/// ortho(left, right, bottom, top, near, far): no perspective, with the
/// edges of the view given in pixels from the middle of the screen.
pub fn ortho_planes(l: impl Num, r: impl Num, b: impl Num, t: impl Num, n: impl Num, f: impl Num) {
    let (l, r, b, t, n, f) = (l.f(), r.f(), b.f(), t.f(), n.f(), f.f());
    if r == l || t == b || f == n { return; }
    #[cfg(feature = "libprocessing")]
    if crate::lp::on() {
        crate::lp::ortho_edges(l, r, b, t, n, f);
        return;
    }
    let mut m = IDENT;
    m[0] = 2.0 / (r - l); m[5] = 2.0 / (t - b); m[10] = -2.0 / (f - n);
    m[12] = -(r + l) / (r - l); m[13] = -(t + b) / (t - b); m[14] = -(f + n) / (f - n);
    S.with(|s| { let mut s = s.borrow_mut(); s.proj = m; s.custom_proj = true; });
}
/// ortho(left, right, bottom, top): near and far as in ortho().
pub fn ortho_rect(l: impl Num, r: impl Num, b: impl Num, t: impl Num) {
    let (_, h) = dims();
    let ez = default_eye_z(h as f32);
    ortho_planes(l, r, b, t, ez / 10.0, ez * 10.0);
}
/// frustum(left, right, bottom, top, near, far): a perspective view given by
/// the edges of its near side.
pub fn frustum(l: impl Num, r: impl Num, b: impl Num, t: impl Num, n: impl Num, f: impl Num) {
    let (l, r, b, t, n, f) = (l.f(), r.f(), b.f(), t.f(), n.f().max(1e-4), f.f());
    if r == l || t == b || f == n { return; }
    #[cfg(feature = "libprocessing")]
    if crate::lp::on() {
        crate::lp::frustum(l, r, b, t, n, f);
        return;
    }
    let mut m = [0.0; 16];
    m[0] = 2.0 * n / (r - l); m[5] = 2.0 * n / (t - b);
    m[8] = (r + l) / (r - l); m[9] = (t + b) / (t - b);
    m[10] = -(f + n) / (f - n); m[11] = -1.0; m[14] = -2.0 * f * n / (f - n);
    S.with(|s| { let mut s = s.borrow_mut(); s.proj = m; s.custom_proj = true; });
}

fn invert(m: &M4) -> Option<M4> {
    let mut inv = [0.0f32; 16];
    inv[0] = m[5]*m[10]*m[15] - m[5]*m[11]*m[14] - m[9]*m[6]*m[15] + m[9]*m[7]*m[14] + m[13]*m[6]*m[11] - m[13]*m[7]*m[10];
    inv[4] = -m[4]*m[10]*m[15] + m[4]*m[11]*m[14] + m[8]*m[6]*m[15] - m[8]*m[7]*m[14] - m[12]*m[6]*m[11] + m[12]*m[7]*m[10];
    inv[8] = m[4]*m[9]*m[15] - m[4]*m[11]*m[13] - m[8]*m[5]*m[15] + m[8]*m[7]*m[13] + m[12]*m[5]*m[11] - m[12]*m[7]*m[9];
    inv[12] = -m[4]*m[9]*m[14] + m[4]*m[10]*m[13] + m[8]*m[5]*m[14] - m[8]*m[6]*m[13] - m[12]*m[5]*m[10] + m[12]*m[6]*m[9];
    inv[1] = -m[1]*m[10]*m[15] + m[1]*m[11]*m[14] + m[9]*m[2]*m[15] - m[9]*m[3]*m[14] - m[13]*m[2]*m[11] + m[13]*m[3]*m[10];
    inv[5] = m[0]*m[10]*m[15] - m[0]*m[11]*m[14] - m[8]*m[2]*m[15] + m[8]*m[3]*m[14] + m[12]*m[2]*m[11] - m[12]*m[3]*m[10];
    inv[9] = -m[0]*m[9]*m[15] + m[0]*m[11]*m[13] + m[8]*m[1]*m[15] - m[8]*m[3]*m[13] - m[12]*m[1]*m[11] + m[12]*m[3]*m[9];
    inv[13] = m[0]*m[9]*m[14] - m[0]*m[10]*m[13] - m[8]*m[1]*m[14] + m[8]*m[2]*m[13] + m[12]*m[1]*m[10] - m[12]*m[2]*m[9];
    inv[2] = m[1]*m[6]*m[15] - m[1]*m[7]*m[14] - m[5]*m[2]*m[15] + m[5]*m[3]*m[14] + m[13]*m[2]*m[7] - m[13]*m[3]*m[6];
    inv[6] = -m[0]*m[6]*m[15] + m[0]*m[7]*m[14] + m[4]*m[2]*m[15] - m[4]*m[3]*m[14] - m[12]*m[2]*m[7] + m[12]*m[3]*m[6];
    inv[10] = m[0]*m[5]*m[15] - m[0]*m[7]*m[13] - m[4]*m[1]*m[15] + m[4]*m[3]*m[13] + m[12]*m[1]*m[7] - m[12]*m[3]*m[5];
    inv[14] = -m[0]*m[5]*m[14] + m[0]*m[6]*m[13] + m[4]*m[1]*m[14] - m[4]*m[2]*m[13] - m[12]*m[1]*m[6] + m[12]*m[2]*m[5];
    inv[3] = -m[1]*m[6]*m[11] + m[1]*m[7]*m[10] + m[5]*m[2]*m[11] - m[5]*m[3]*m[10] - m[9]*m[2]*m[7] + m[9]*m[3]*m[6];
    inv[7] = m[0]*m[6]*m[11] - m[0]*m[7]*m[10] - m[4]*m[2]*m[11] + m[4]*m[3]*m[10] + m[8]*m[2]*m[7] - m[8]*m[3]*m[6];
    inv[11] = -m[0]*m[5]*m[11] + m[0]*m[7]*m[9] + m[4]*m[1]*m[11] - m[4]*m[3]*m[9] - m[8]*m[1]*m[7] + m[8]*m[3]*m[5];
    inv[15] = m[0]*m[5]*m[10] - m[0]*m[6]*m[9] - m[4]*m[1]*m[10] + m[4]*m[2]*m[9] + m[8]*m[1]*m[6] - m[8]*m[2]*m[5];
    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if !det.is_finite() || det.abs() < 1e-20 { return None; }
    Some(inv.map(|v| v / det))
}

thread_local! { static CAMERA_EDIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
/// begin_camera() ... end_camera(): translate, rotate and so on in between
/// move the camera instead of the shapes.
pub fn begin_camera() {
    if CAMERA_EDIT.with(|c| c.replace(true)) { eprintln!("beginCamera() called twice without endCamera()"); return; }
    crate::render::push_matrix();
    crate::render::reset_matrix();
}
pub fn end_camera() {
    if !CAMERA_EDIT.with(|c| c.replace(false)) { eprintln!("endCamera() without beginCamera()"); return; }
    let moved = S.with(|s| *s.borrow().stack.last().unwrap());
    crate::render::pop_matrix();
    let (_, view, _, _) = matrices();
    // moving the camera by `moved` moves everything else the other way
    let Some(undo) = invert(&moved) else { return };
    let v = mul(&undo, &view);
    let Some(cam) = invert(&v) else { return };
    let eye = [cam[12], cam[13], cam[14]];
    #[cfg(feature = "libprocessing")]
    if crate::lp::on() {
        let fwd = [-cam[8], -cam[9], -cam[10]];
        let up = [cam[4], cam[5], cam[6]];
        crate::lp::camera(eye, [eye[0] + fwd[0], eye[1] + fwd[1], eye[2] + fwd[2]], up);
    }
    S.with(|s| { let mut s = s.borrow_mut(); s.view = v; s.eye = eye; s.custom_camera = true; });
}

fn print_m4(m: &M4) {
    for r in 0..4 {
        println!("{:>10.4} {:>10.4} {:>10.4} {:>10.4}", m[r], m[4 + r], m[8 + r], m[12 + r]);
    }
}
/// printCamera(): the camera matrix, row by row.
pub fn print_camera() { let (_, v, _, _) = matrices(); print_m4(&v); }
/// printProjection(): the projection matrix, row by row.
pub fn print_projection() { let (_, _, p, _) = matrices(); print_m4(&p); }
/// printMatrix() in 3D: the current model matrix.
pub fn print_matrix_3d() { let (m, ..) = matrices(); print_m4(&m); }

/// rotate(angle, x, y, z): turns around the axis (x, y, z).
pub fn rotate_axis(a: impl Num, x: impl Num, y: impl Num, z: impl Num) {
    let (a, ax) = (a.f(), norm([x.f(), y.f(), z.f()]));
    if !(dot(ax, ax) > 0.0) { return; }
    #[cfg(feature = "libprocessing")]
    if crate::lp::on() { crate::lp::rotate(a, bevy::math::Vec3::from_array(ax)); }
    if ax[0].abs() < 1e-6 && ax[1].abs() < 1e-6 {
        // around z: the 2D matrix turns too
        let a = if ax[2] < 0.0 { -a } else { a };
        crate::render::rotate_z_no_lp(a);
        return;
    }
    let (c, s, t) = (a.cos(), a.sin(), 1.0 - a.cos());
    let [x, y, z] = ax;
    let m = [t*x*x + c, t*x*y + s*z, t*x*z - s*y, 0.0,
             t*x*y - s*z, t*y*y + c, t*y*z + s*x, 0.0,
             t*x*z + s*y, t*y*z - s*x, t*z*z + c, 0.0,
             0.0, 0.0, 0.0, 1.0];
    post(m);
}
/// applyMatrix with 16 numbers, row by row as in Processing.
#[allow(clippy::too_many_arguments)]
pub fn apply_matrix_3d(n00: f32, n01: f32, n02: f32, n03: f32, n10: f32, n11: f32, n12: f32, n13: f32,
                       n20: f32, n21: f32, n22: f32, n23: f32, n30: f32, n31: f32, n32: f32, n33: f32) {
    let m = [n00, n10, n20, n30, n01, n11, n21, n31, n02, n12, n22, n32, n03, n13, n23, n33];
    if !m.iter().all(|v| v.is_finite()) { return; }
    #[cfg(feature = "libprocessing")]
    if crate::lp::on() { crate::lp::missing("applyMatrix with 16 numbers"); }
    post(m);
}

/// modelX/Y/Z: where a point ends up in space after the current transforms
/// (camera not included).
pub fn model_x(x: impl Num, y: impl Num, z: impl Num) -> f32 { model_pos([x.f(), y.f(), z.f()])[0] }
pub fn model_y(x: impl Num, y: impl Num, z: impl Num) -> f32 { model_pos([x.f(), y.f(), z.f()])[1] }
pub fn model_z(x: impl Num, y: impl Num, z: impl Num) -> f32 { model_pos([x.f(), y.f(), z.f()])[2] }
fn model_pos(p: [f32; 3]) -> [f32; 3] {
    let (m, ..) = matrices();
    let q = apply(&m, p);
    let w = if q[3].abs() > 1e-12 { q[3] } else { 1.0 };
    [q[0] / w, q[1] / w, q[2] / w]
}
/// screenZ: depth on the screen, 0 near to 1 far.
pub fn screen_z(x: impl Num, y: impl Num, z: impl Num) -> f32 {
    let (m, view, proj, _) = matrices();
    let (w, h) = dims();
    let q = apply(&m, [x.f(), y.f(), z.f()]);
    to_screen(apply(&mul(&proj, &view), [q[0], q[1], q[2]]), w as f32, h as f32).map(|s| s.z).unwrap_or(f32::NAN)
}

fn matrices() -> (M4, M4, M4, [f32; 3]) {
    let (w, h) = dims();
    let (w, h) = (w as f32, h as f32);
    S.with(|s| {
        let s = s.borrow();
        let ez = default_eye_z(h);
        let (view, eye) = if s.custom_camera { (s.view, s.eye) } else {
            let eye = [w / 2.0, h / 2.0, ez];
            (look_at(eye, [w / 2.0, h / 2.0, 0.0], [0.0, 1.0, 0.0]), eye)
        };
        let proj = if s.custom_proj { s.proj } else { persp_matrix(std::f32::consts::PI / 3.0, w / h, ez / 10.0, ez * 10.0) };
        (*s.stack.last().unwrap(), view, proj, eye)
    })
}

// ── lights ─────────────────────────────────────────────────────────────────
/// lights(): a gray ambient light and a light shining into the screen.
pub fn lights() {
    lp_route! { crate::lp::lights(); return; }
    S.with(|s| {
        let mut s = s.borrow_mut();
        s.lights.clear();
        s.lights.push(Ambient([0.5, 0.5, 0.5]));
        s.lights.push(Directional([0.5, 0.5, 0.5], [0.0, 0.0, -1.0]));
    });
}
pub fn no_lights() {
    lp_route! { crate::lp::no_lights(); return; } S.with(|s| s.borrow_mut().lights.clear()); }
fn c255(r: f32, g: f32, b: f32) -> [f32; 3] { [r / 255.0, g / 255.0, b / 255.0] }
pub fn ambient_light(r: impl Num, g: impl Num, b: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    lp_route! { crate::lp::add_light(crate::lp::LightSpec::Ambient(c)); return; }
    S.with(|s| s.borrow_mut().lights.push(Ambient(c)));
}
/// directional_light(r, g, b, nx, ny, nz): light traveling in direction (nx, ny, nz).
pub fn directional_light(r: impl Num, g: impl Num, b: impl Num, nx: impl Num, ny: impl Num, nz: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    let d = norm([nx.f(), ny.f(), nz.f()]);
    lp_route! { crate::lp::add_light(crate::lp::LightSpec::Directional(c, d)); return; }
    S.with(|s| { let mut s = s.borrow_mut(); let spec = s.light_spec; s.lights.push(Light { spec, ..Directional(c, d) }); });
}
/// point_light(r, g, b, x, y, z): a light at a point, shining every way.
pub fn point_light(r: impl Num, g: impl Num, b: impl Num, x: impl Num, y: impl Num, z: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    let p = [x.f(), y.f(), z.f()];
    // the position goes through the current matrix, like Processing
    let (m, ..) = matrices();
    let w = apply(&m, p);
    lp_route! { crate::lp::add_light(crate::lp::LightSpec::Point(c, [w[0], w[1], w[2]])); return; }
    S.with(|s| { let mut s = s.borrow_mut(); let (spec, fall) = (s.light_spec, s.light_fall);
        s.lights.push(Light { kind: Kind::Point([w[0], w[1], w[2]]), c, spec, fall }); });
}
/// spot_light(r, g, b, x, y, z, nx, ny, nz, angle, concentration): a cone of
/// light from (x, y, z) pointing along (nx, ny, nz), `angle` wide on each side.
#[allow(clippy::too_many_arguments)]
pub fn spot_light(r: impl Num, g: impl Num, b: impl Num, x: impl Num, y: impl Num, z: impl Num,
                  nx: impl Num, ny: impl Num, nz: impl Num, angle: impl Num, concentration: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    let (m, ..) = matrices();
    let w = apply(&m, [x.f(), y.f(), z.f()]);
    let pos = [w[0], w[1], w[2]];
    let tip = apply(&m, [x.f() + nx.f(), y.f() + ny.f(), z.f() + nz.f()]);
    let dir = norm(sub([tip[0], tip[1], tip[2]], pos));
    let (angle, conc) = (angle.f(), concentration.f().max(0.0));
    lp_route! { crate::lp::add_light(crate::lp::LightSpec::Spot(c, pos, dir, angle)); return; }
    S.with(|s| { let mut s = s.borrow_mut(); let (spec, fall) = (s.light_spec, s.light_fall);
        s.lights.push(Light { kind: Kind::Spot { pos, dir, cos_angle: angle.cos(), conc }, c, spec, fall }); });
}
/// light_falloff(constant, linear, quadratic): how point and spot lights made
/// after it fade with distance d: 1 / (constant + linear d + quadratic d²).
pub fn light_falloff(constant: impl Num, linear: impl Num, quadratic: impl Num) {
    let f = [constant.f(), linear.f(), quadratic.f()];
    S.with(|s| s.borrow_mut().light_fall = f);
}
/// light_specular(r, g, b): the shine color of lights made after it.
pub fn light_specular(r: impl Num, g: impl Num, b: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    S.with(|s| s.borrow_mut().light_spec = c);
}
fn material_mut(f: impl FnOnce(&mut Material)) { S.with(|s| f(&mut s.borrow_mut().material)); }
/// ambient(r, g, b): how the shape takes ambient light (by default, like its fill).
pub fn ambient(r: impl Num, g: impl Num, b: impl Num) { let c = c255(r.f(), g.f(), b.f()); material_mut(|m| m.ambient = Some(c)); }
/// specular(r, g, b): the color of the shine on the shape.
pub fn specular(r: impl Num, g: impl Num, b: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    lp_route! { crate::lp::material_specular(c); }
    material_mut(|m| m.specular = c);
}
/// emissive(r, g, b): light the shape gives off itself.
pub fn emissive(r: impl Num, g: impl Num, b: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    lp_route! { crate::lp::material_emissive(c); }
    material_mut(|m| m.emissive = c);
}
/// shininess(s): how tight the shine is; bigger is a smaller, sharper spot.
pub fn shininess(sh: impl Num) {
    let sh = sh.f().max(0.0);
    lp_route! { crate::lp::material_shininess(sh); }
    material_mut(|m| m.shininess = sh);
}
#[inline] pub fn spotLight(r: impl Num, g: impl Num, b: impl Num, x: impl Num, y: impl Num, z: impl Num,
                           nx: impl Num, ny: impl Num, nz: impl Num, angle: impl Num, concentration: impl Num) {
    spot_light(r, g, b, x, y, z, nx, ny, nz, angle, concentration)
}
#[inline] pub fn lightFalloff(c: impl Num, l: impl Num, q: impl Num) { light_falloff(c, l, q) }
#[inline] pub fn lightSpecular(r: impl Num, g: impl Num, b: impl Num) { light_specular(r, g, b) }

#[cfg(test)]
fn shade(base: Color, normal: [f32; 3], center: [f32; 3], eye: [f32; 3], lights: &[Light]) -> Color {
    shade_with(base, normal, center, eye, lights, &DEFAULT_MATERIAL)
}
/// Processing's lighting: emissive, plus for each light its ambient part
/// (times the ambient material), diffuse part (times the fill) and shine
/// (times the specular material), faded with distance and by a spot's cone.
fn shade_with(base: Color, normal: [f32; 3], center: [f32; 3], eye: [f32; 3], lights: &[Light], mat: &Material) -> Color {
    if lights.is_empty() { return base; }
    // two-sided: the normal faces whoever is looking
    let to_eye = norm(sub(eye, center));
    let n = if dot(normal, to_eye) < 0.0 { [-normal[0], -normal[1], -normal[2]] } else { normal };
    let fill = [base.r, base.g, base.b];
    let amb = mat.ambient.unwrap_or(fill);
    let mut out = mat.emissive;
    for light in lights {
        let (to_light, dist) = match light.kind {
            Kind::Ambient => { for i in 0..3 { out[i] += light.c[i] * amb[i]; } continue; }
            Kind::Directional(d) => ([-d[0], -d[1], -d[2]], 0.0),
            Kind::Point(p) | Kind::Spot { pos: p, .. } => { let v = sub(p, center); let d = dot(v, v).sqrt(); (norm(v), d) }
        };
        let f = light.fall;
        let mut k = 1.0 / (f[0] + f[1] * dist + f[2] * dist * dist).max(1e-6);
        if let Kind::Spot { dir, cos_angle, conc, .. } = light.kind {
            let c = dot([-to_light[0], -to_light[1], -to_light[2]], dir);
            if c < cos_angle { continue; }
            k *= c.max(0.0).powf(conc);
        }
        let diff = dot(n, to_light).max(0.0);
        let h = norm([to_light[0] + to_eye[0], to_light[1] + to_eye[1], to_light[2] + to_eye[2]]);
        let shine = if diff > 0.0 && mat.shininess > 0.0 { dot(n, h).max(0.0).powf(mat.shininess) } else { 0.0 };
        for i in 0..3 { out[i] += k * (light.c[i] * fill[i] * diff + light.spec[i] * mat.specular[i] * shine); }
    }
    Color::from_rgba(out[0].min(1.0), out[1].min(1.0), out[2].min(1.0), base.a)
}

// ── rasterizing ────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
struct SV { x: f32, y: f32, z: f32, iw: f32 } // screen x, y, depth (0 near .. 1 far), 1 / clip w

fn to_screen(clip: [f32; 4], w: f32, h: f32) -> Option<SV> {
    if clip[3] <= 1e-4 { return None; }
    let (nx, ny, nz) = (clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]);
    if !(nx.is_finite() && ny.is_finite()) || !(-1.0..=1.0).contains(&nz) { return None; }
    Some(SV { x: (nx + 1.0) / 2.0 * w, y: (ny + 1.0) / 2.0 * h, z: nz * 0.5 + 0.5, iw: 1.0 / clip[3] })
}

fn with_depth<R>(f: impl FnOnce(&mut Vec<f32>, usize, usize) -> R) -> R {
    let (w, h) = dims();
    let frame = crate::frame_count();
    S.with(|s| {
        let mut s = s.borrow_mut();
        if s.depth_dims != (w, h) || s.depth.len() != w * h || s.depth_frame != frame {
            s.depth.clear();
            s.depth.resize(w * h, f32::INFINITY);
            s.depth_dims = (w, h);
            s.depth_frame = frame;
        }
        f(&mut s.depth, w, h)
    })
}

fn put(buf: &mut [u32], i: usize, c: Color, opaque_target: bool) {
    let a = c.a.clamp(0.0, 1.0);
    let src = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u32;
    if a >= 0.999 {
        buf[i] = (src(c.r) << 16) | (src(c.g) << 8) | src(c.b) | if opaque_target { 0xFF00_0000 } else { buf[i] & 0xFF00_0000 };
        return;
    }
    let d = buf[i];
    let mix = |s: f32, sh: u32| -> u32 { let dv = ((d >> sh) & 0xFF) as f32 / 255.0; src(s * a + dv * (1.0 - a)) << sh };
    buf[i] = mix(c.r, 16) | mix(c.g, 8) | mix(c.b, 0) | (d & 0xFF00_0000);
}

thread_local! {
    /// The pixel raster_tri_with is coloring, for shaders that need it.
    static PIXEL: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}
fn raster_tri(a: SV, b: SV, c: SV, col: Color) {
    raster_tri_with(a, b, c, col.a < 0.999, &|_, _, _| col);
}

/// Fills a triangle, asking `color` for each pixel with the perspective
/// correct weights of the three corners.
fn raster_tri_with(a: SV, b: SV, c: SV, see_through: bool, color: &dyn Fn(f32, f32, f32) -> Color) {
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area.abs() < 1e-6 { return; }
    with_depth(|depth, w, h| {
        let minx = a.x.min(b.x).min(c.x).floor().max(0.0) as i64;
        let maxx = a.x.max(b.x).max(c.x).ceil().min(w as f32 - 1.0) as i64;
        let miny = a.y.min(b.y).min(c.y).floor().max(0.0) as i64;
        let maxy = a.y.max(b.y).max(c.y).ceil().min(h as f32 - 1.0) as i64;
        if minx > maxx || miny > maxy { return; }
        let opaque = crate::render::alpha_target_pub();
        let mut buf = crate::pixel_buf().lock();
        if buf.len() < w * h { return; }
        for y in miny..=maxy {
            let py = y as f32 + 0.5;
            for x in minx..=maxx {
                let px = x as f32 + 0.5;
                let w0 = ((b.x - px) * (c.y - py) - (b.y - py) * (c.x - px)) / area;
                let w1 = ((c.x - px) * (a.y - py) - (c.y - py) * (a.x - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                let z = w0 * a.z + w1 * b.z + w2 * c.z;
                let i = y as usize * w + x as usize;
                if z < depth[i] || !DEPTH_TEST.load(Ordering::Relaxed) {
                    // weights in space, not on the screen, so textures don't swim
                    let (p0, p1, p2) = (w0 * a.iw, w1 * b.iw, w2 * c.iw);
                    let sum = p0 + p1 + p2;
                    PIXEL.with(|p| p.set((x as usize, y as usize)));
                    let col = if sum > 0.0 { color(p0 / sum, p1 / sum, p2 / sum) } else { color(w0, w1, w2) };
                    if col.a <= 0.0 { continue; }
                    if !see_through && col.a >= 0.999 { depth[i] = z; }
                    put(&mut buf, i, col, opaque);
                }
            }
        }
    });
}

fn raster_line(a: SV, b: SV, col: Color, weight: f32) {
    with_depth(|depth, w, h| {
        let opaque = crate::render::alpha_target_pub();
        let mut buf = crate::pixel_buf().lock();
        if buf.len() < w * h { return; }
        let steps = ((b.x - a.x).abs().max((b.y - a.y).abs()).ceil() as usize).clamp(1, 16384);
        let r = ((weight.max(1.0) - 1.0) / 2.0).round() as i64;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y, z) = (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t - 1e-4);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (px, py) = (x as i64 + dx, y as i64 + dy);
                    if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 { continue; }
                    let k = py as usize * w + px as usize;
                    if z <= depth[k] || !DEPTH_TEST.load(Ordering::Relaxed) { depth[k] = depth[k].min(z); put(&mut buf, k, col, opaque); }
                }
            }
        }
    });
}

/// Fills and outlines triangles given in model space, with the current style.
/// `edges` are pairs of vertex indexes to outline (None: every triangle edge).
fn draw_mesh(verts: &[[f32; 3]], tris: &[[usize; 3]], edges: Option<&[[usize; 2]]>) {
    if let Some((vs, imgs)) = crate::shader::active_vertex() { return draw_mesh_vs(&vs, &imgs, verts, None, tris, edges); }
    let (m, view, proj, eye) = matrices();
    let (w, h) = dims();
    let (wf, hf) = (w as f32, h as f32);
    let vp = mul(&proj, &view);
    let world: Vec<[f32; 3]> = verts.iter().map(|p| { let q = apply(&m, *p); [q[0], q[1], q[2]] }).collect();
    let screen: Vec<Option<SV>> = world.iter().map(|p| to_screen(apply(&vp, *p), wf, hf)).collect();
    let (fill, stroke, weight) = crate::render::style_pub();
    let (lights, mat) = S.with(|s| { let s = s.borrow(); (s.lights.clone(), s.material) });
    if let Some(fc) = fill {
        // shader(): the shader colors each pixel, with the fill as vertColor
        let shaded = crate::shader::active_cpu(w, h);
        let px = shaded.as_ref().map(|(cpu, images)| {
            let src: Vec<u32> = crate::pixel_buf().lock().iter().map(|p| 0xFF00_0000 | (p & 0x00FF_FFFF)).collect();
            cpu.one_pixel(w, h, src, images.clone())
        });
        for t in tris {
            let (Some(a), Some(b), Some(c)) = (screen[t[0]], screen[t[1]], screen[t[2]]) else { continue };
            if let Some(px) = &px {
                raster_tri_with(a, b, c, fc.a < 0.999, &|_, _, _| {
                    let (x, y) = PIXEL.with(|p| p.get());
                    match px.color(x, y, [fc.r, fc.g, fc.b, fc.a]) {
                        Some(c) => Color::from_rgba(c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), c[3].clamp(0.0, 1.0)),
                        None => Color::from_rgba(0.0, 0.0, 0.0, 0.0),
                    }
                });
                continue;
            }
            let (pa, pb, pc) = (world[t[0]], world[t[1]], world[t[2]]);
            let n = norm(cross(sub(pb, pa), sub(pc, pa)));
            let center = [(pa[0] + pb[0] + pc[0]) / 3.0, (pa[1] + pb[1] + pc[1]) / 3.0, (pa[2] + pb[2] + pc[2]) / 3.0];
            raster_tri(a, b, c, shade_with(fc, n, center, eye, &lights, &mat));
        }
    }
    if let Some(sc) = stroke {
        let draw = |i: usize, j: usize| {
            if let (Some(a), Some(b)) = (screen[i], screen[j]) { raster_line(a, b, sc, weight); }
        };
        match edges {
            Some(es) => for e in es { draw(e[0], e[1]); },
            None => for t in tris { draw(t[0], t[1]); draw(t[1], t[2]); draw(t[2], t[0]); },
        }
    }
}

/// A begin_shape / vertex / end_shape made in 3D (or textured in 3D):
/// filled with the texture or the fill, lit, and outlined along `edges`.
pub(crate) fn draw_mesh_uv(verts: &[[f32; 3]], uvs: &[(f32, f32)], tris: &[[usize; 3]], edges: &[[usize; 2]], tex: Option<u64>) {
    if let Some((vs, imgs)) = crate::shader::active_vertex() { return draw_mesh_vs(&vs, &imgs, verts, Some(uvs), tris, Some(edges)); }
    let Some(tex) = tex else { return draw_mesh(verts, tris, Some(edges)) };
    let (m, view, proj, eye) = matrices();
    let (w, h) = dims();
    let vp = mul(&proj, &view);
    let world: Vec<[f32; 3]> = verts.iter().map(|p| { let q = apply(&m, *p); [q[0], q[1], q[2]] }).collect();
    let screen: Vec<Option<SV>> = world.iter().map(|p| to_screen(apply(&vp, *p), w as f32, h as f32)).collect();
    let (lights, mat) = S.with(|s| { let s = s.borrow(); (s.lights.clone(), s.material) });
    let white = Color::from_rgb(1.0, 1.0, 1.0);
    let _ = crate::render::with_texture(tex, |sample| {
        for t in tris {
            let (Some(a), Some(b), Some(c)) = (screen[t[0]], screen[t[1]], screen[t[2]]) else { continue };
            let (pa, pb, pc) = (world[t[0]], world[t[1]], world[t[2]]);
            let n = norm(cross(sub(pb, pa), sub(pc, pa)));
            let center = [(pa[0] + pb[0] + pc[0]) / 3.0, (pa[1] + pb[1] + pc[1]) / 3.0, (pa[2] + pb[2] + pc[2]) / 3.0];
            let light = shade_with(white, n, center, eye, &lights, &mat);
            let (ua, ub, uc) = (uvs[t[0]], uvs[t[1]], uvs[t[2]]);
            raster_tri_with(a, b, c, false, &|k0, k1, k2| {
                let col = sample(k0 * ua.0 + k1 * ub.0 + k2 * uc.0, k0 * ua.1 + k1 * ub.1 + k2 * uc.1);
                Color::from_rgba(col.r * light.r, col.g * light.g, col.b * light.b, col.a)
            });
        }
    });
    let (_, stroke, weight) = crate::render::style_pub();
    if let Some(sc) = stroke {
        for e in edges { if let (Some(a), Some(b)) = (screen[e[0]], screen[e[1]]) { raster_line(a, b, sc, weight); } }
    }
}

fn mat3_of(m: &M4) -> [f32; 9] { [m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]] }
fn transpose(m: &M4) -> M4 { let mut t = [0.0; 16]; for c in 0..4 { for r in 0..4 { t[c * 4 + r] = m[r * 4 + c]; } } t }

/// Processing's light uniforms (lightCount, lightPosition[8], ...) in eye space.
fn light_uniforms(vs: &mut crate::glsl::VertexRun, view: &M4, lights: &[Light]) {
    let n = lights.len().min(8);
    let (mut pos, mut nrm, mut amb, mut dif, mut spe, mut fal, mut spot) =
        (vec![0.0; 32], vec![0.0; 24], vec![0.0; 24], vec![0.0; 24], vec![0.0; 24], vec![0.0; 24], vec![0.0; 16]);
    let dir_eye = |d: [f32; 3]| { let q = apply(view, d); let o = apply(view, [0.0; 3]); norm([q[0] - o[0], q[1] - o[1], q[2] - o[2]]) };
    for (i, l) in lights.iter().take(8).enumerate() {
        let (p, d): ([f32; 4], [f32; 3]) = match l.kind {
            Kind::Ambient => ([0.0, 0.0, 0.0, 1.0], [0.0; 3]),
            Kind::Directional(d) => ([0.0, 0.0, 0.0, 0.0], dir_eye(d)),
            Kind::Point(p) => { let q = apply(view, p); ([q[0], q[1], q[2], 1.0], [0.0; 3]) }
            Kind::Spot { pos, dir, cos_angle, conc } => {
                spot[i * 2] = cos_angle; spot[i * 2 + 1] = conc;
                let q = apply(view, pos); ([q[0], q[1], q[2], 1.0], dir_eye(dir))
            }
        };
        pos[i * 4..i * 4 + 4].copy_from_slice(&p);
        nrm[i * 3..i * 3 + 3].copy_from_slice(&d);
        if matches!(l.kind, Kind::Ambient) { amb[i * 3..i * 3 + 3].copy_from_slice(&l.c); } else { dif[i * 3..i * 3 + 3].copy_from_slice(&l.c); }
        spe[i * 3..i * 3 + 3].copy_from_slice(&l.spec);
        fal[i * 3..i * 3 + 3].copy_from_slice(&l.fall);
    }
    vs.set_uniform("lightCount", &[n as f32]);
    for (name, v) in [("lightPosition", &pos), ("lightNormal", &nrm), ("lightAmbient", &amb), ("lightDiffuse", &dif),
                      ("lightSpecular", &spe), ("lightFalloff", &fal), ("lightSpot", &spot)] {
        vs.set_uniform(name, v);
    }
}

thread_local! {
    /// attrib() values for each vertex of the shape being drawn
    static SHAPE_ATTRIBS: RefCell<Vec<Vec<(String, Vec<f32>)>>> = const { RefCell::new(Vec::new()) };
}
pub(crate) fn set_shape_attribs(a: Vec<Vec<(String, Vec<f32>)>>) { SHAPE_ATTRIBS.with(|s| *s.borrow_mut() = a); }

/// Shapes drawn with a shader that has a vertex stage: it places each corner
/// (gl_Position) and what it passes along is blended across the triangle
/// for the fragment stage.
fn draw_mesh_vs(vcpu: &crate::glsl::Cpu, vimages: &[(usize, usize, Vec<u32>)], verts: &[[f32; 3]], uvs: Option<&[(f32, f32)]>, tris: &[[usize; 3]], edges: Option<&[[usize; 2]]>) {
    let (m, view, proj, _) = matrices();
    let (w, h) = dims();
    let (fill, stroke, weight) = crate::render::style_pub();
    let lights = S.with(|s| s.borrow().lights.clone());
    let Some((fcpu, images)) = crate::shader::active_cpu(w, h) else { return };
    let mv = mul(&view, &m);
    let pmv = mul(&proj, &mv);
    let nm = mat3_of(&transpose(&invert(&mv).unwrap_or(IDENT)));
    // corner normals: the faces around each corner, averaged
    let mut normals = vec![[0.0f32; 3]; verts.len()];
    for t in tris {
        let n = cross(sub(verts[t[1]], verts[t[0]]), sub(verts[t[2]], verts[t[0]]));
        for &i in t { for k in 0..3 { normals[i][k] += n[k]; } }
    }
    // what reaches the fragment stage: built-ins plus varyings both declare
    let mut outs: Vec<String> = vec!["vertColor".into(), "vertTexCoord".into()];
    for n in &vcpu.varyings { if fcpu.global(n).is_some() && !outs.contains(n) { outs.push(n.clone()); } }
    let slots: Vec<(usize, crate::glsl::Ty)> = outs.iter().map(|n| fcpu.global(n).unwrap()).collect();
    let sizes: Vec<usize> = slots.iter().map(|(_, t)| t.size_pub().max(1)).collect();
    let stride: usize = sizes.iter().sum();
    let run_stage = |col: Color| -> Vec<Option<(SV, Vec<f32>)>> {
        let mut vs = vcpu.vertex_with(vimages.to_vec());
        for (k, v) in [("transform", &pmv), ("modelviewMatrix", &mv), ("modelview", &mv), ("projectionMatrix", &proj),
                       ("projection", &proj), ("projmodelviewMatrix", &pmv), ("texMatrix", &IDENT)] {
            if vcpu.uniforms.contains_key(k) && !vs.was_set(k) { vs.set_uniform(k, v); }
        }
        if !vs.was_set("normalMatrix") { vs.set_uniform("normalMatrix", &nm); }
        if !vs.was_set("viewport") { vs.set_uniform("viewport", &[0.0, 0.0, w as f32, h as f32]); }
        light_uniforms(&mut vs, &view, &lights);
        verts.iter().enumerate().map(|(i, p)| {
            let c = [col.r, col.g, col.b, col.a];
            let uv = uvs.and_then(|u| u.get(i)).copied().unwrap_or((0.0, 0.0));
            let n = norm(normals[i]);
            vs.set("position", &[p[0], p[1], p[2], 1.0]);
            vs.set("vertex", &[p[0], p[1], p[2], 1.0]);
            vs.set("color", &c);
            vs.set("normal", &n);
            vs.set("texCoord", &[uv.0, uv.1]);
            vs.set("vertColor", &c);
            vs.set("vertTexCoord", &[uv.0, uv.1, 0.0, 1.0]);
            vs.set("gl_Position", &[0.0, 0.0, 0.0, 1.0]);
            SHAPE_ATTRIBS.with(|a| if let Some(list) = a.borrow().get(i) { for (n, v) in list { vs.set(n, v); } });
            let (clip, o) = vs.run(&outs)?;
            Some((to_screen(clip, w as f32, h as f32)?, o))
        }).collect()
    };
    if let Some(fc) = fill {
        let corners = run_stage(fc);
        let src: Vec<u32> = crate::pixel_buf().lock().iter().map(|p| 0xFF00_0000 | (p & 0x00FF_FFFF)).collect();
        let px = fcpu.one_pixel(w, h, src, images);
        let mut mix = vec![0.0f32; stride];
        let mix = std::cell::RefCell::new(&mut mix);
        for t in tris {
            let (Some((a, oa)), Some((b, ob)), Some((c, oc))) = (&corners[t[0]], &corners[t[1]], &corners[t[2]]) else { continue };
            raster_tri_with(*a, *b, *c, fc.a < 0.999, &|k0, k1, k2| {
                let (x, y) = PIXEL.with(|p| p.get());
                let mut mx = mix.borrow_mut();
                for j in 0..stride { mx[j] = k0 * oa[j] + k1 * ob[j] + k2 * oc[j]; }
                let mut vary: [(usize, crate::glsl::Ty, &[f32]); 16] = [(0, crate::glsl::Ty::Float, &[]); 16];
                let mut at = 0;
                let nv = slots.len().min(16);
                for (k, (slot, ty)) in slots.iter().take(16).enumerate() { vary[k] = (*slot, *ty, &mx[at..at + sizes[k]]); at += sizes[k]; }
                let vary = &vary[..nv];
                let vc = [mx[0], mx[1], mx[2], mx[3]];
                match px.color_with(x, y, vc, vary) {
                    Some(c) => Color::from_rgba(c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), c[3].clamp(0.0, 1.0)),
                    None => Color::from_rgba(0.0, 0.0, 0.0, 0.0),
                }
            });
        }
    }
    if let Some(sc) = stroke {
        // lines go where the vertex stage put the corners, in the stroke color
        let corners = run_stage(sc);
        let draw = |i: usize, j: usize| {
            if let (Some((a, _)), Some((b, _))) = (&corners[i], &corners[j]) { raster_line(*a, *b, sc, weight); }
        };
        match edges {
            Some(es) => for e in es { draw(e[0], e[1]); },
            None => for t in tris { draw(t[0], t[1]); draw(t[1], t[2]); draw(t[2], t[0]); },
        }
    }
}

static DEPTH_TEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// hint(DISABLE_DEPTH_TEST / ENABLE_DEPTH_TEST)
pub(crate) fn set_depth_test(on: bool) { DEPTH_TEST.store(on, Ordering::Relaxed); }

/// background() and clear() start the depth over.
pub(crate) fn clear_depth() { S.with(|s| s.borrow_mut().depth_frame = u64::MAX); }

/// Is anything 3D in play: a camera, a projection, or a turn or move in depth?
pub(crate) fn in_3d() -> bool {
    S.with(|s| {
        let s = s.borrow();
        let m = s.stack.last().unwrap();
        s.custom_camera || s.custom_proj || !s.lights.is_empty() || crate::shader::vertex_active()
            || m[2].abs() > 1e-6 || m[6].abs() > 1e-6 || m[8].abs() > 1e-6 || m[9].abs() > 1e-6
            || m[14].abs() > 1e-6
    })
}

/// box(size) or box_3d(w, h, d): centered on the current origin.
pub fn box_3d(w: impl Num, h: impl Num, d: impl Num) {
    lp_route! { crate::lp::box_3d(w.f(), h.f(), d.f()); return; }
    let (x, y, z) = (w.f() / 2.0, h.f() / 2.0, d.f() / 2.0);
    let v = [[-x, -y, -z], [x, -y, -z], [x, y, -z], [-x, y, -z], [-x, -y, z], [x, -y, z], [x, y, z], [-x, y, z]];
    let t = [[0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6], [0, 4, 5], [0, 5, 1],
             [3, 2, 6], [3, 6, 7], [0, 3, 7], [0, 7, 4], [1, 5, 6], [1, 6, 2]];
    let e = [[0, 1], [1, 2], [2, 3], [3, 0], [4, 5], [5, 6], [6, 7], [7, 4], [0, 4], [1, 5], [2, 6], [3, 7]];
    draw_mesh(&v, &t, Some(&e));
}
pub fn box_cube(size: impl Num) { let s = size.f(); box_3d(s, s, s); }

/// sphere_detail(n) or sphere_detail_uv(around, down): how many pieces a sphere is made of.
pub fn sphere_detail(n: impl Num) { let n = (n.f() as usize).clamp(3, 200); S.with(|s| s.borrow_mut().sphere_detail = (n, n)); }
pub fn sphere_detail_uv(u: impl Num, v: impl Num) {
    let (u, v) = ((u.f() as usize).clamp(3, 200), (v.f() as usize).clamp(2, 200));
    S.with(|s| s.borrow_mut().sphere_detail = (u, v));
}
pub fn sphere(r: impl Num) {
    let r = r.f();
    let (us, vs) = S.with(|s| s.borrow().sphere_detail);
    lp_route! { crate::lp::sphere(r, us as u32, vs as u32); return; }
    let mut v = Vec::with_capacity((us + 1) * (vs + 1));
    for j in 0..=vs {
        let phi = std::f32::consts::PI * j as f32 / vs as f32;
        for i in 0..=us {
            let th = std::f32::consts::TAU * i as f32 / us as f32;
            v.push([r * phi.sin() * th.cos(), -r * phi.cos(), r * phi.sin() * th.sin()]);
        }
    }
    let idx = |i: usize, j: usize| j * (us + 1) + i;
    let mut t = Vec::new();
    let mut e = Vec::new();
    for j in 0..vs {
        for i in 0..us {
            let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1));
            if j != 0 { t.push([a, b, c]); }
            if j != vs - 1 { t.push([a, c, d]); }
            e.push([a, d]);
            if j != 0 { e.push([a, b]); }
        }
    }
    draw_mesh(&v, &t, Some(&e));
}
pub fn line_3d(x1: impl Num, y1: impl Num, z1: impl Num, x2: impl Num, y2: impl Num, z2: impl Num) {
    lp_route! { crate::lp::line_3d([x1.f(), y1.f(), z1.f()], [x2.f(), y2.f(), z2.f()], crate::render::style_pub().1); return; }
    let (m, view, proj, _) = matrices();
    let (w, h) = dims();
    let vp = mul(&proj, &view);
    let s = |p: [f32; 3]| { let q = apply(&m, p); to_screen(apply(&vp, [q[0], q[1], q[2]]), w as f32, h as f32) };
    let (_, stroke, weight) = crate::render::style_pub();
    if let (Some(c), Some(a), Some(b)) = (stroke, s([x1.f(), y1.f(), z1.f()]), s([x2.f(), y2.f(), z2.f()])) {
        raster_line(a, b, c, weight);
    }
}
pub fn point_3d(x: impl Num, y: impl Num, z: impl Num) {
    let (x, y, z) = (x.f(), y.f(), z.f());
    line_3d(x, y, z, x, y, z);
}
/// A triangle in space, filled and outlined with the current style.
#[allow(clippy::too_many_arguments)]
pub fn triangle_3d(x1: impl Num, y1: impl Num, z1: impl Num, x2: impl Num, y2: impl Num, z2: impl Num,
                   x3: impl Num, y3: impl Num, z3: impl Num) {
    let p = [[x1.f(), y1.f(), z1.f()], [x2.f(), y2.f(), z2.f()], [x3.f(), y3.f(), z3.f()]];
    lp_route! { let (f, st, _) = crate::render::style_pub(); crate::lp::triangle_3d(p, f, st); return; }
    draw_mesh(&p, &[[0, 1, 2]], None);
}

/// Where a point in space lands on the screen.
pub fn screen_x(x: impl Num, y: impl Num, z: impl Num) -> f32 { screen_pos([x.f(), y.f(), z.f()]).0 }
pub fn screen_y(x: impl Num, y: impl Num, z: impl Num) -> f32 { screen_pos([x.f(), y.f(), z.f()]).1 }
fn screen_pos(p: [f32; 3]) -> (f32, f32) {
    let (m, view, proj, _) = matrices();
    let (w, h) = dims();
    let q = apply(&m, p);
    to_screen(apply(&mul(&proj, &view), [q[0], q[1], q[2]]), w as f32, h as f32)
        .map(|s| (s.x, s.y)).unwrap_or((f32::NAN, f32::NAN))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_camera_matches_2d() {
        CANVAS_W.store(200, Ordering::Relaxed);
        CANVAS_H.store(100, Ordering::Relaxed);
        on_begin_frame();
        let (x, y) = screen_pos([30.0, 70.0, 0.0]);
        assert!((x - 30.0).abs() < 0.01 && (y - 70.0).abs() < 0.01, "{x} {y}");
        // closer to the camera looks bigger: a point right of center moves right
        let (x2, _) = screen_pos([150.0, 50.0, 40.0]);
        assert!(x2 > 150.0);
        on_translate(100.0, 50.0, 0.0);
        rotate_y(std::f32::consts::FRAC_PI_2);
        let (x3, _) = screen_pos([0.0, 0.0, 10.0]);
        // after turning a quarter about y, +z points along +x
        assert!(x3 > 100.0, "{x3}");
        on_push(); on_scale(2.0, 2.0, 2.0); on_pop();
        let (x4, _) = screen_pos([0.0, 0.0, 10.0]);
        assert!((x3 - x4).abs() < 1e-3);
    }
    #[test]
    fn lighting() {
        let base = Color::from_rgb(1.0, 1.0, 1.0);
        let lit = shade(base, [0.0, 0.0, 1.0], [0.0; 3], [0.0, 0.0, 10.0],
                        &[Ambient([0.2; 3]), Directional([0.8; 3], [0.0, 0.0, -1.0])]);
        assert!((lit.r - 1.0).abs() < 1e-5);
        let side = shade(base, [1.0, 0.0, 0.0], [0.0; 3], [0.0, 0.0, 10.0],
                         &[Ambient([0.2; 3]), Directional([0.8; 3], [0.0, 0.0, -1.0])]);
        assert!((side.r - 0.2).abs() < 1e-5);
    }
}
