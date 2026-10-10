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

#[derive(Clone, Copy)]
enum Light { Ambient([f32; 3]), Directional([f32; 3], [f32; 3]), Point([f32; 3], [f32; 3]) }

struct State {
    stack: Vec<M4>,
    view: M4,
    proj: M4,
    eye: [f32; 3],
    custom_camera: bool,
    custom_proj: bool,
    lights: Vec<Light>,
    sphere_detail: (usize, usize),
    depth: Vec<f32>,
    depth_frame: u64,
    depth_dims: (usize, usize),
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State {
        stack: vec![IDENT], view: IDENT, proj: IDENT, eye: [0.0, 0.0, 1.0],
        custom_camera: false, custom_proj: false, lights: Vec::new(),
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
    let a = a.f(); let (c, s) = (a.cos(), a.sin());
    let mut m = IDENT; m[5] = c; m[6] = s; m[9] = -s; m[10] = c; post(m);
}
pub fn rotate_y(a: impl Num) {
    let a = a.f(); let (c, s) = (a.cos(), a.sin());
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
fn frustum(fovy: f32, aspect: f32, near: f32, far: f32) -> M4 {
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
    let v = look_at(eye, [cx.f(), cy.f(), cz.f()], [ux.f(), uy.f(), uz.f()]);
    S.with(|s| { let mut s = s.borrow_mut(); s.view = v; s.eye = eye; s.custom_camera = true; });
}
pub fn camera() { S.with(|s| s.borrow_mut().custom_camera = false); }
/// perspective(fov, aspect, near, far). fov in radians, PI/3 by default.
pub fn perspective(fov: impl Num, aspect: impl Num, near: impl Num, far: impl Num) {
    let p = frustum(fov.f(), aspect.f(), near.f().max(0.001), far.f());
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
    let mut m = IDENT;
    m[0] = 2.0 / w; m[5] = 2.0 / h; m[10] = -2.0 / (f - n); m[14] = -(f + n) / (f - n);
    S.with(|s| { let mut s = s.borrow_mut(); s.proj = m; s.custom_proj = true; });
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
        let proj = if s.custom_proj { s.proj } else { frustum(std::f32::consts::PI / 3.0, w / h, ez / 10.0, ez * 10.0) };
        (*s.stack.last().unwrap(), view, proj, eye)
    })
}

// ── lights ─────────────────────────────────────────────────────────────────
/// lights(): a gray ambient light and a light shining into the screen.
pub fn lights() {
    S.with(|s| {
        let mut s = s.borrow_mut();
        s.lights.clear();
        s.lights.push(Light::Ambient([0.5, 0.5, 0.5]));
        s.lights.push(Light::Directional([0.5, 0.5, 0.5], [0.0, 0.0, -1.0]));
    });
}
pub fn no_lights() { S.with(|s| s.borrow_mut().lights.clear()); }
fn c255(r: f32, g: f32, b: f32) -> [f32; 3] { [r / 255.0, g / 255.0, b / 255.0] }
pub fn ambient_light(r: impl Num, g: impl Num, b: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    S.with(|s| s.borrow_mut().lights.push(Light::Ambient(c)));
}
/// directional_light(r, g, b, nx, ny, nz): light traveling in direction (nx, ny, nz).
pub fn directional_light(r: impl Num, g: impl Num, b: impl Num, nx: impl Num, ny: impl Num, nz: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    let d = norm([nx.f(), ny.f(), nz.f()]);
    S.with(|s| s.borrow_mut().lights.push(Light::Directional(c, d)));
}
/// point_light(r, g, b, x, y, z): a light at a point, shining every way.
pub fn point_light(r: impl Num, g: impl Num, b: impl Num, x: impl Num, y: impl Num, z: impl Num) {
    let c = c255(r.f(), g.f(), b.f());
    let p = [x.f(), y.f(), z.f()];
    // the position goes through the current matrix, like Processing
    let (m, ..) = matrices();
    let w = apply(&m, p);
    S.with(|s| s.borrow_mut().lights.push(Light::Point(c, [w[0], w[1], w[2]])));
}

fn shade(base: Color, normal: [f32; 3], center: [f32; 3], eye: [f32; 3], lights: &[Light]) -> Color {
    if lights.is_empty() { return base; }
    // two-sided: the normal faces whoever is looking
    let to_eye = sub(eye, center);
    let n = if dot(normal, to_eye) < 0.0 { [-normal[0], -normal[1], -normal[2]] } else { normal };
    let mut l = [0.0f32; 3];
    for light in lights {
        let (c, k) = match *light {
            Light::Ambient(c) => (c, 1.0),
            Light::Directional(c, d) => (c, dot(n, [-d[0], -d[1], -d[2]]).max(0.0)),
            Light::Point(c, p) => (c, dot(n, norm(sub(p, center))).max(0.0)),
        };
        for i in 0..3 { l[i] += c[i] * k; }
    }
    Color::from_rgba((base.r * l[0]).min(1.0), (base.g * l[1]).min(1.0), (base.b * l[2]).min(1.0), base.a)
}

// ── rasterizing ────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
struct SV { x: f32, y: f32, z: f32 } // screen x, y and depth (0 near .. 1 far)

fn to_screen(clip: [f32; 4], w: f32, h: f32) -> Option<SV> {
    if clip[3] <= 1e-4 { return None; }
    let (nx, ny, nz) = (clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]);
    if !(nx.is_finite() && ny.is_finite()) || !(-1.0..=1.0).contains(&nz) { return None; }
    Some(SV { x: (nx + 1.0) / 2.0 * w, y: (ny + 1.0) / 2.0 * h, z: nz * 0.5 + 0.5 })
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

fn raster_tri(a: SV, b: SV, c: SV, col: Color) {
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
                if z < depth[i] {
                    if col.a >= 0.999 { depth[i] = z; }
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
                    if z <= depth[k] { depth[k] = depth[k].min(z); put(&mut buf, k, col, opaque); }
                }
            }
        }
    });
}

/// Fills and outlines triangles given in model space, with the current style.
/// `edges` are pairs of vertex indexes to outline (None: every triangle edge).
fn draw_mesh(verts: &[[f32; 3]], tris: &[[usize; 3]], edges: Option<&[[usize; 2]]>) {
    let (m, view, proj, eye) = matrices();
    let (w, h) = dims();
    let (wf, hf) = (w as f32, h as f32);
    let vp = mul(&proj, &view);
    let world: Vec<[f32; 3]> = verts.iter().map(|p| { let q = apply(&m, *p); [q[0], q[1], q[2]] }).collect();
    let screen: Vec<Option<SV>> = world.iter().map(|p| to_screen(apply(&vp, *p), wf, hf)).collect();
    let (fill, stroke, weight) = crate::render::style_pub();
    let lights = S.with(|s| s.borrow().lights.clone());
    if let Some(fc) = fill {
        for t in tris {
            let (Some(a), Some(b), Some(c)) = (screen[t[0]], screen[t[1]], screen[t[2]]) else { continue };
            let (pa, pb, pc) = (world[t[0]], world[t[1]], world[t[2]]);
            let n = norm(cross(sub(pb, pa), sub(pc, pa)));
            let center = [(pa[0] + pb[0] + pc[0]) / 3.0, (pa[1] + pb[1] + pc[1]) / 3.0, (pa[2] + pb[2] + pc[2]) / 3.0];
            raster_tri(a, b, c, shade(fc, n, center, eye, &lights));
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

/// box(size) or box_3d(w, h, d): centered on the current origin.
pub fn box_3d(w: impl Num, h: impl Num, d: impl Num) {
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
    draw_mesh(&[[x1.f(), y1.f(), z1.f()], [x2.f(), y2.f(), z2.f()], [x3.f(), y3.f(), z3.f()]], &[[0, 1, 2]], None);
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
                        &[Light::Ambient([0.2; 3]), Light::Directional([0.8; 3], [0.0, 0.0, -1.0])]);
        assert!((lit.r - 1.0).abs() < 1e-5);
        let side = shade(base, [1.0, 0.0, 0.0], [0.0; 3], [0.0, 0.0, 10.0],
                         &[Light::Ambient([0.2; 3]), Light::Directional([0.8; 3], [0.0, 0.0, -1.0])]);
        assert!((side.r - 0.2).abs() < 1e-5);
    }
}
