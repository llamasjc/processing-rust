//! # processing
//!
//! Processing-style API for RustMode sketches.
//!
//! Generated sketches do `use processing::*;` and get all Processing drawing,
//! math, input, and lifecycle functions in scope — both snake_case and
//! camelCase naming conventions.
//!
//! Backend: software pixel-buffer renderer via minifb (native) or stub (wasm).

// The camelCase aliases (mouseX, noFill, frameRate, …) are intentional.
#![allow(non_snake_case)]

use parking_lot::RwLock;
use std::sync::{atomic::{AtomicU32, Ordering}, OnceLock};

// ─────────────────────────────────────────────────────────────────────────────
// Canvas dimensions (shared between glfw runner and render module)
// ─────────────────────────────────────────────────────────────────────────────

static CANVAS_W: AtomicU32 = AtomicU32::new(100);
static CANVAS_H: AtomicU32 = AtomicU32::new(100);

// Pixel buffer: 0xRRGGBB packed as u32 (minifb native format)
static PIXEL_BUF: OnceLock<parking_lot::Mutex<Vec<u32>>> = OnceLock::new();
fn pixel_buf() -> &'static parking_lot::Mutex<Vec<u32>> {
    PIXEL_BUF.get_or_init(|| {
        let w = CANVAS_W.load(Ordering::Relaxed) as usize;
        let h = CANVAS_H.load(Ordering::Relaxed) as usize;
        parking_lot::Mutex::new(vec![0xFF_20_20_20u32; w * h])
    })
}

/// Largest canvas side. Keeps w * h well inside memory and stops a typo like
/// size(4000000, 3) from taking the machine down.
pub const MAX_CANVAS: u32 = 8192;

fn resize_canvas(w: u32, h: u32) {
    let (w, h) = (w.clamp(1, MAX_CANVAS), h.clamp(1, MAX_CANVAS));
    CANVAS_W.store(w, Ordering::Relaxed);
    CANVAS_H.store(h, Ordering::Relaxed);
    let mut buf = pixel_buf().lock();
    buf.resize(w as usize * h as usize, 0xFF_20_20_20u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Core types
// ─────────────────────────────────────────────────────────────────────────────

mod core {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }

    impl Color {
        pub fn from_gray(v: f32) -> Self { Self { r: v, g: v, b: v, a: 1.0 } }
        pub fn from_rgb(r: f32, g: f32, b: f32) -> Self { Self { r, g, b, a: 1.0 } }
        pub fn from_rgba(r: f32, g: f32, b: f32, a: f32) -> Self { Self { r, g, b, a } }

        pub fn to_u32(self) -> u32 {
            let r = (self.r.clamp(0.0, 1.0) * 255.0) as u32;
            let g = (self.g.clamp(0.0, 1.0) * 255.0) as u32;
            let b = (self.b.clamp(0.0, 1.0) * 255.0) as u32;
            (r << 16) | (g << 8) | b
        }
    }

    pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
        Color {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: a.a + (b.a - a.a) * t,
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Default)]
    pub struct PVector { pub x: f32, pub y: f32, pub z: f32 }

    impl PVector {
        pub fn new(x: f32, y: f32) -> Self { Self { x, y, z: 0.0 } }
        pub fn new3(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
        pub fn mag(&self) -> f32 { (self.x*self.x + self.y*self.y + self.z*self.z).sqrt() }
        pub fn normalize(&mut self) {
            let m = self.mag();
            if m > 0.0 { self.x /= m; self.y /= m; self.z /= m; }
        }
    }

    // Simple xorshift RNG
    use std::sync::{Mutex, OnceLock};
    static RNG: OnceLock<Mutex<u64>> = OnceLock::new();
    /// splitmix64: spreads any seed (even 1, 2, 3...) over the full u64 range,
    /// so xorshift's first outputs aren't all near zero.
    fn mix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)).max(1)
    }
    /// Seeded from the clock like Processing, unless random_seed() is called.
    fn rng() -> &'static Mutex<u64> {
        RNG.get_or_init(|| {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64).unwrap_or(12345);
            Mutex::new(mix(t))
        })
    }
    fn next(s: &mut u64) -> u64 { *s ^= *s<<13; *s ^= *s>>7; *s ^= *s<<17; *s }

    pub fn random_seed(seed: u64) { *rng().lock().unwrap() = mix(seed); }
    // ── Noise ─────────────────────────────────────────────────────────────
    // Value noise the way Processing does it: a table of random values,
    // cosine-eased between lattice points, summed over a few octaves. Smooth
    // (nearby inputs give nearby outputs), always in 0..1, different every
    // run unless noise_seed() is called.
    const PERLIN_YWRAPB: u32 = 4;
    const PERLIN_YWRAP: i64 = 1 << PERLIN_YWRAPB;
    const PERLIN_ZWRAPB: u32 = 8;
    const PERLIN_ZWRAP: i64 = 1 << PERLIN_ZWRAPB;
    const PERLIN_SIZE: i64 = 4095;

    struct Noise { table: Vec<f32>, octaves: u32, falloff: f32 }
    impl Noise {
        fn seeded(seed: u64) -> Vec<f32> {
            let mut s = mix(seed);
            (0..=PERLIN_SIZE).map(|_| (next(&mut s) >> 40) as f32 / (1u64 << 24) as f32).collect()
        }
    }
    fn noise_state() -> &'static Mutex<Noise> {
        static N: OnceLock<Mutex<Noise>> = OnceLock::new();
        N.get_or_init(|| {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64).unwrap_or(54321);
            Mutex::new(Noise { table: Noise::seeded(t ^ 0x5EED), octaves: 4, falloff: 0.5 })
        })
    }
    fn noise_lock() -> std::sync::MutexGuard<'static, Noise> {
        noise_state().lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn noise_seed(seed: u64) { noise_lock().table = Noise::seeded(seed); }
    pub fn noise_detail(octaves: u32, falloff: f32) {
        let mut n = noise_lock();
        if octaves > 0 { n.octaves = octaves.min(16); }
        if falloff > 0.0 { n.falloff = falloff.min(1.0); }
    }
    pub fn noise1(x: f32) -> f32 { noise3(x, 0.0, 0.0) }
    pub fn noise2(x: f32, y: f32) -> f32 { noise3(x, y, 0.0) }
    pub fn noise3(x: f32, y: f32, z: f32) -> f32 {
        if !(x.is_finite() && y.is_finite() && z.is_finite()) { return 0.0; }
        let n = noise_lock();
        let split = |v: f32| { let v = (v as f64).abs(); let i = v.floor(); (i as i64, (v - i) as f32) };
        let ((mut xi, mut xf), (mut yi, mut yf), (mut zi, mut zf)) = (split(x), split(y), split(z));
        let fsc = |t: f32| 0.5 * (1.0 - (t * std::f32::consts::PI).cos());
        let at = |i: i64| n.table[(i & PERLIN_SIZE) as usize];
        let (mut r, mut ampl) = (0.0f32, 0.5f32);
        for _ in 0..n.octaves {
            let mut of = xi.wrapping_add(yi.wrapping_shl(PERLIN_YWRAPB)).wrapping_add(zi.wrapping_shl(PERLIN_ZWRAPB));
            let (rxf, ryf) = (fsc(xf), fsc(yf));
            let mut n1 = at(of);
            n1 += rxf * (at(of.wrapping_add(1)) - n1);
            let mut n2 = at(of.wrapping_add(PERLIN_YWRAP));
            n2 += rxf * (at(of.wrapping_add(PERLIN_YWRAP + 1)) - n2);
            n1 += ryf * (n2 - n1);
            of = of.wrapping_add(PERLIN_ZWRAP);
            n2 = at(of);
            n2 += rxf * (at(of.wrapping_add(1)) - n2);
            let mut n3 = at(of.wrapping_add(PERLIN_YWRAP));
            n3 += rxf * (at(of.wrapping_add(PERLIN_YWRAP + 1)) - n3);
            n2 += ryf * (n3 - n2);
            n1 += fsc(zf) * (n2 - n1);
            r += n1 * ampl;
            ampl *= n.falloff;
            xi = xi.wrapping_shl(1); xf *= 2.0;
            yi = yi.wrapping_shl(1); yf *= 2.0;
            zi = zi.wrapping_shl(1); zf *= 2.0;
            if xf >= 1.0 { xi = xi.wrapping_add(1); xf -= 1.0; }
            if yf >= 1.0 { yi = yi.wrapping_add(1); yf -= 1.0; }
            if zf >= 1.0 { zi = zi.wrapping_add(1); zf -= 1.0; }
        }
        r.clamp(0.0, 1.0)
    }

    pub fn random_f32(low: f32, high: f32) -> f32 {
        let mut s = rng().lock().unwrap();
        // top 24 bits -> [0, 1): exact in f32 and never rounds up to 1.0
        let t = (next(&mut s) >> 40) as f32 / (1u64 << 24) as f32;
        low + (high - low) * t
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Software rasterizer
// ─────────────────────────────────────────────────────────────────────────────

mod render {
    // ── begin_shape / vertex / end_shape (inserted by shapes patch) ─────────
    // Vertices are stored untransformed; fills go through tfm + fill_tri,
    // strokes through draw_line, so both follow the current matrix and style.
    struct ShapeState { kind: u8, pts: Vec<(f32, f32)>, curve: Vec<(f32, f32)>, active: bool }
    fn shape_state() -> &'static parking_lot::Mutex<ShapeState> {
        static S: std::sync::OnceLock<parking_lot::Mutex<ShapeState>> = std::sync::OnceLock::new();
        S.get_or_init(|| parking_lot::Mutex::new(ShapeState { kind: 0, pts: Vec::new(), curve: Vec::new(), active: false }))
    }
    pub fn shape_begin(kind: u8) {
        let mut s = shape_state().lock();
        s.kind = kind; s.pts.clear(); s.curve.clear(); s.active = true;
    }
    pub fn shape_vertex(x: f32, y: f32) {
        let mut s = shape_state().lock();
        s.curve.clear();
        s.pts.push((x, y));
    }
    pub fn shape_bezier_vertex(cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
        let mut s = shape_state().lock();
        let (x0, y0) = match s.pts.last() { Some(p) => *p, None => { s.pts.push((x, y)); return; } };
        for i in 1..=24 {
            let t = i as f32 / 24.0; let u = 1.0 - t;
            let px = u*u*u*x0 + 3.0*u*u*t*cx1 + 3.0*u*t*t*cx2 + t*t*t*x;
            let py = u*u*u*y0 + 3.0*u*u*t*cy1 + 3.0*u*t*t*cy2 + t*t*t*y;
            s.pts.push((px, py));
        }
    }
    pub fn shape_quadratic_vertex(cx: f32, cy: f32, x: f32, y: f32) {
        let mut s = shape_state().lock();
        let (x0, y0) = match s.pts.last() { Some(p) => *p, None => { s.pts.push((x, y)); return; } };
        for i in 1..=16 {
            let t = i as f32 / 16.0; let u = 1.0 - t;
            s.pts.push((u*u*x0 + 2.0*u*t*cx + t*t*x, u*u*y0 + 2.0*u*t*cy + t*t*y));
        }
    }
    /// Catmull-Rom, like Processing: the first and last curve_vertex are control points only.
    pub fn shape_curve_vertex(x: f32, y: f32) {
        let mut s = shape_state().lock();
        s.curve.push((x, y));
        let n = s.curve.len();
        if n < 4 { return; }
        let (p0, p1, p2, p3) = (s.curve[n-4], s.curve[n-3], s.curve[n-2], s.curve[n-1]);
        let start = if n == 4 { 0 } else { 1 };
        for i in start..=12 {
            let t = i as f32 / 12.0; let t2 = t*t; let t3 = t2*t;
            let f = |a: f32, b: f32, c: f32, d: f32| 0.5 * ((2.0*b) + (-a + c)*t + (2.0*a - 5.0*b + 4.0*c - d)*t2 + (-a + 3.0*b - 3.0*c + d)*t3);
            let pt = (f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1));
            s.pts.push(pt);
        }
    }
    pub fn shape_end(close: bool) {
        let (kind, pts) = {
            let mut s = shape_state().lock();
            if !s.active { return; }
            s.active = false;
            s.curve.clear();
            (s.kind, std::mem::take(&mut s.pts))
        };
        let (has_fill, has_stroke) = with_style(|s| (s.fill.is_some(), s.stroke.is_some()));
        let poly = |p: &[(f32, f32)], close: bool| shape_polygon(p, close, has_fill, has_stroke);
        match kind {
            1 => for p in &pts { draw_point(p.0, p.1); },                       // POINTS
            2 => if has_stroke { for w in pts.chunks_exact(2) { draw_line(w[0].0, w[0].1, w[1].0, w[1].1); } }, // LINES
            3 => for w in pts.chunks_exact(3) { poly(w, true); },                // TRIANGLES
            4 => for i in 2..pts.len() { poly(&[pts[i-2], pts[i-1], pts[i]], true); },   // TRIANGLE_STRIP
            5 => for i in 2..pts.len() { poly(&[pts[0], pts[i-1], pts[i]], true); },     // TRIANGLE_FAN
            6 => for q in pts.chunks_exact(4) { poly(q, true); },                // QUADS
            7 => for i in (3..pts.len()).step_by(2) {                            // QUAD_STRIP
                     poly(&[pts[i-3], pts[i-2], pts[i], pts[i-1]], true);
                 },
            _ => poly(&pts, close),                                              // polygon
        }
    }
    /// Fill a polygon of any shape (see fill_poly), then stroke its outline.
    fn shape_polygon(pts: &[(f32, f32)], close: bool, has_fill: bool, has_stroke: bool) {
        let n = pts.len();
        if has_fill && n >= 3 {
            if let Some(fc) = with_style(|s| s.fill) {
                let (cw, ch) = canvas_dims();
                let screen: Vec<(f32, f32)> = pts.iter().map(|p| tfm(p.0, p.1)).collect();
                let mut buf = pixel_buf().lock();
                fill_poly(&mut buf, cw, ch, &screen, fc);
            }
        }
        if has_stroke && n >= 2 {
            for i in 1..n { draw_line(pts[i-1].0, pts[i-1].1, pts[i].0, pts[i].1); }
            if close { draw_line(pts[n-1].0, pts[n-1].1, pts[0].0, pts[0].1); }
        }
    }

    use super::{CANVAS_W, CANVAS_H, pixel_buf};
    use super::core::Color;
    use std::sync::atomic::Ordering;

    // ── Style state ────────────────────────────────────────────────────────────

    #[derive(Clone)]
    struct Style {
        fill:         Option<Color>,
        stroke:       Option<Color>,
        stroke_weight: f32,
    }
    impl Default for Style {
        fn default() -> Self {
            Self {
                fill:   Some(Color::from_gray(1.0)),
                stroke: Some(Color::from_gray(0.0)),
                stroke_weight: 1.0,
            }
        }
    }

    thread_local! {
        static STYLE_STACK: std::cell::RefCell<Vec<Style>> =
            std::cell::RefCell::new(vec![Style::default()]);
    }
    fn with_style<R>(f: impl FnOnce(&Style) -> R) -> R {
        STYLE_STACK.with(|s| { let stack = s.borrow(); f(stack.last().unwrap()) })
    }
    fn with_style_mut<R>(f: impl FnOnce(&mut Style) -> R) -> R {
        STYLE_STACK.with(|s| { let mut stack = s.borrow_mut(); f(stack.last_mut().unwrap()) })
    }

    // ── Transform matrix stack (2D affine: [a,b,c,d,tx,ty]) ──────────────────

    type Mat = [f32; 6]; // [a, b, c, d, tx, ty]

    fn identity() -> Mat { [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] }

    fn mat_mul(a: &Mat, b: &Mat) -> Mat {
        [
            a[0]*b[0] + a[2]*b[1],  a[1]*b[0] + a[3]*b[1],
            a[0]*b[2] + a[2]*b[3],  a[1]*b[2] + a[3]*b[3],
            a[0]*b[4] + a[2]*b[5] + a[4],
            a[1]*b[4] + a[3]*b[5] + a[5],
        ]
    }

    thread_local! {
        static MATRIX_STACK: std::cell::RefCell<Vec<Mat>> =
            std::cell::RefCell::new(vec![identity()]);
    }
    fn current_matrix() -> Mat {
        MATRIX_STACK.with(|s| *s.borrow().last().unwrap())
    }

    // ── Pixel operations ───────────────────────────────────────────────────────
    //
    // Everything that touches the pixel buffer goes through put, hspan, bline
    // or fill_poly, and all four clip to the canvas first. Coordinates can be
    // anything a sketch produces (off-screen by a billion pixels, infinite,
    // NaN): the cost of a call is bounded by the canvas, never by its arguments.

    /// Beyond this, positions are clamped. Far outside any canvas, and small
    /// enough that f32 arithmetic on them stays finite.
    const FAR: f32 = 1.0e9;

    #[inline]
    fn blend(dst: u32, col: Color) -> u32 {
        let a = col.a;
        let dr = ((dst >> 16) & 0xFF) as f32 / 255.0;
        let dg = ((dst >> 8) & 0xFF) as f32 / 255.0;
        let db = (dst & 0xFF) as f32 / 255.0;
        Color::from_rgb(dr + (col.r - dr) * a, dg + (col.g - dg) * a, db + (col.b - db) * a).to_u32()
    }

    fn put(buf: &mut Vec<u32>, w: usize, h: usize, x: i32, y: i32, col: Color) {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 { return; }
        if !(col.a > 0.0) { return; }             // also skips a NaN alpha
        if let Some(p) = buf.get_mut(y as usize * w + x as usize) {
            *p = if col.a >= 1.0 { col.to_u32() } else { blend(*p, col) };
        }
    }

    /// Pixels x0..=x1 of row y, clipped.
    fn hspan(buf: &mut Vec<u32>, w: usize, h: usize, y: i32, x0: i32, x1: i32, col: Color) {
        if y < 0 || y >= h as i32 || w == 0 { return; }
        if !(col.a > 0.0) { return; }
        let (x0, x1) = (x0.max(0), x1.min(w as i32 - 1));
        if x0 > x1 { return; }
        let row = y as usize * w;
        if let Some(run) = buf.get_mut(row + x0 as usize..=row + x1 as usize) {
            if col.a >= 1.0 { run.fill(col.to_u32()); }
            else { for p in run { *p = blend(*p, col); } }
        }
    }

    /// One-pixel line. Clipped to the canvas before stepping (Liang-Barsky),
    /// so a line to a point a billion pixels away costs no more than one
    /// across the window.
    fn bline(buf: &mut Vec<u32>, w: usize, h: usize,
             x0: f32, y0: f32, x1: f32, y1: f32, col: Color) {
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) { return; }
        if w == 0 || h == 0 { return; }
        let (x0, y0, x1, y1) = (x0 as f64, y0 as f64, x1 as f64, y1 as f64);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (p, q) in [(-dx, x0 + 1.0), (dx, w as f64 - x0), (-dy, y0 + 1.0), (dy, h as f64 - y0)] {
            if p == 0.0 {
                if q < 0.0 { return; }
            } else {
                let r = q / p;
                if p < 0.0 { if r > t1 { return; } if r > t0 { t0 = r; } }
                else       { if r < t0 { return; } if r < t1 { t1 = r; } }
            }
        }
        let (ax, ay) = ((x0 + t0 * dx) as i32, (y0 + t0 * dy) as i32);
        let (bx, by) = ((x0 + t1 * dx) as i32, (y0 + t1 * dy) as i32);

        // Bresenham between two points that are now on (or right next to) the canvas
        let ddx = (bx - ax).abs(); let ddy = (by - ay).abs();
        let sx = if ax < bx { 1i32 } else { -1 };
        let sy = if ay < by { 1i32 } else { -1 };
        let mut err = ddx - ddy;
        let (mut x, mut y) = (ax, ay);
        for _ in 0..=(ddx + ddy) {
            put(buf, w, h, x, y, col);
            if x == bx && y == by { break; }
            let e2 = 2 * err;
            if e2 > -ddy { err -= ddy; x += sx; }
            if e2 <  ddx { err += ddx; y += sy; }
        }
    }

    /// Fill a polygon given in screen coordinates. Any shape: convex, concave
    /// or self-crossing (non-zero winding, like Processing). A pixel is filled
    /// when its centre is inside, and is written exactly once, so shapes that
    /// share an edge don't overlap and translucent fills have no seams.
    fn fill_poly(buf: &mut Vec<u32>, w: usize, h: usize, pts: &[(f32, f32)], col: Color) {
        if pts.len() < 3 || w == 0 || h == 0 { return; }
        if !(col.a > 0.0) { return; }
        if pts.iter().any(|p| p.0.is_nan() || p.1.is_nan()) { return; }
        let pts: Vec<(f32, f32)> = pts.iter().map(|p| (p.0.clamp(-FAR, FAR), p.1.clamp(-FAR, FAR))).collect();
        let n = pts.len();

        let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
        for p in &pts { ymin = ymin.min(p.1); ymax = ymax.max(p.1); }
        // rows whose centre (row + 0.5) lies in [ymin, ymax)
        let row0 = (ymin - 0.5).ceil().max(0.0) as i64;
        let row1 = (ymax - 0.5).ceil().min(h as f32) as i64;

        let mut xs: Vec<(f32, i32)> = Vec::with_capacity(8);
        for row in row0..row1 {
            let yc = row as f32 + 0.5;
            xs.clear();
            for i in 0..n {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                let up   = a.1 <= yc && b.1 > yc;
                let down = b.1 <= yc && a.1 > yc;
                if up || down {
                    let t = (yc - a.1) / (b.1 - a.1);
                    xs.push((a.0 + t * (b.0 - a.0), if up { 1 } else { -1 }));
                }
            }
            if xs.len() < 2 { continue; }
            xs.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut winding = 0;
            for k in 0..xs.len() - 1 {
                winding += xs[k].1;
                if winding == 0 { continue; }
                // pixels whose centre (px + 0.5) lies in [xs[k], xs[k+1])
                let x0 = (xs[k].0 - 0.5).ceil().max(0.0);
                let x1 = ((xs[k + 1].0 - 0.5).ceil() - 1.0).min(w as f32 - 1.0);
                if x0 <= x1 { hspan(buf, w, h, row as i32, x0 as i32, x1 as i32, col); }
            }
        }
    }

    /// A straight stroke from a to b, `weight` pixels wide.
    fn seg(buf: &mut Vec<u32>, w: usize, h: usize,
           a: (f32, f32), b: (f32, f32), weight: f32, col: Color) {
        if !(weight > 1.5) { return bline(buf, w, h, a.0, a.1, b.0, b.1, col); }
        let weight = weight.min(4096.0);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if !len.is_finite() || len < 1.0e-6 { return; }
        let (nx, ny) = (-dy / len * weight * 0.5, dx / len * weight * 0.5);
        fill_poly(buf, w, h, &[
            (a.0 - nx, a.1 - ny), (a.0 + nx, a.1 + ny),
            (b.0 + nx, b.1 + ny), (b.0 - nx, b.1 - ny),
        ], col);
    }

    /// Stroke the outline of a few-cornered shape (rect, triangle, quad).
    /// Thick edges are run half a weight past each corner so the corners come
    /// out square instead of notched.
    fn outline(buf: &mut Vec<u32>, w: usize, h: usize, pts: &[(f32, f32)], weight: f32, col: Color) {
        let n = pts.len();
        for i in 0..n {
            let (mut a, mut b) = (pts[i], pts[(i + 1) % n]);
            if weight > 1.5 {
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len = (dx * dx + dy * dy).sqrt();
                if len.is_finite() && len > 1.0e-6 {
                    let e = weight.min(4096.0) * 0.5 / len;
                    a = (a.0 - dx * e, a.1 - dy * e);
                    b = (b.0 + dx * e, b.1 + dy * e);
                }
            }
            seg(buf, w, h, a, b, weight, col);
        }
    }

    /// How many pixels one unit along each axis covers under the current
    /// matrix (used to pick how finely to cut curves into segments).
    fn matrix_scale() -> f32 {
        let m = current_matrix();
        let sx = (m[0] * m[0] + m[1] * m[1]).sqrt();
        let sy = (m[2] * m[2] + m[3] * m[3]).sqrt();
        let s = sx.max(sy);
        if s.is_finite() { s } else { 1.0 }
    }

    /// Points along an ellipse arc from angle a0 to a1, through the current
    /// matrix, so rotate() and scale() apply to ellipses too.
    fn arc_points(cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32, include_end: bool) -> Vec<(f32, f32)> {
        let span = (a1 - a0).abs();
        let rpx = rx.abs().max(ry.abs()) * matrix_scale();
        let n = if rpx.is_finite() { ((rpx * span / 2.0).ceil() as usize).clamp(8, 1024) } else { 8 };
        let last = if include_end { n } else { n - 1 };
        (0..=last).map(|i| {
            let t = a0 + (a1 - a0) * (i as f32 / n as f32);
            tfm(cx + t.cos() * rx, cy + t.sin() * ry)
        }).collect()
    }

    // ── Public drawing API ─────────────────────────────────────────────────────

    /// Start of a frame. Processing resets the transform every frame, so a
    /// translate() in draw() doesn't pile up; style (fill, stroke, ...) carries
    /// over. Stacks left unbalanced by the last frame are dropped here so a
    /// missing pop can't grow them forever.
    pub fn begin_draw() {
        MATRIX_STACK.with(|s| { let mut st = s.borrow_mut(); st.clear(); st.push(identity()); });
        STYLE_STACK.with(|s| {
            let mut st = s.borrow_mut();
            if st.len() > 1 { let top = st.pop().unwrap(); st.clear(); st.push(top); }
        });
    }
    pub fn end_draw()   {}
    pub fn present()    {}

    pub fn background_gray(v: f32) {
        let col = Color::from_gray(v);
        let p = col.to_u32();
        let mut buf = pixel_buf().lock();
        buf.fill(p);
    }
    pub fn background_rgb(r: f32, g: f32, b: f32) {
        let col = Color::from_rgb(r, g, b);
        let p = col.to_u32();
        let mut buf = pixel_buf().lock();
        buf.fill(p);
    }
    /// A translucent background blends over what is already there (the usual
    /// way to leave trails).
    pub fn background_rgba(r: f32, g: f32, b: f32, a: f32) {
        if a >= 1.0 || a.is_nan() { return background_rgb(r, g, b); }
        if a <= 0.0 { return; }
        let col = Color::from_rgba(r, g, b, a);
        let mut buf = pixel_buf().lock();
        for p in buf.iter_mut() { *p = blend(*p, col); }
    }
    pub fn background_color(c: Color) {
        let p = c.to_u32();
        let mut buf = pixel_buf().lock();
        buf.fill(p);
    }

    pub fn set_fill_gray(v: f32)                    { with_style_mut(|s| s.fill = Some(Color::from_gray(v))); }
    pub fn set_fill_rgb(r: f32, g: f32, b: f32)     { with_style_mut(|s| s.fill = Some(Color::from_rgb(r,g,b))); }
    pub fn set_fill_rgba(r: f32, g: f32, b: f32, a: f32) { with_style_mut(|s| s.fill = Some(Color::from_rgba(r,g,b,a))); }
    pub fn set_fill_none()                           { with_style_mut(|s| s.fill = None); }
    pub fn set_fill_color(c: Color)                  { with_style_mut(|s| s.fill = Some(c)); }

    pub fn set_stroke_gray(v: f32)                   { with_style_mut(|s| s.stroke = Some(Color::from_gray(v))); }
    pub fn set_stroke_rgb(r: f32, g: f32, b: f32)    { with_style_mut(|s| s.stroke = Some(Color::from_rgb(r,g,b))); }
    pub fn set_stroke_rgba(r: f32, g: f32, b: f32, a: f32) { with_style_mut(|s| s.stroke = Some(Color::from_rgba(r,g,b,a))); }
    pub fn set_stroke_none()                          { with_style_mut(|s| s.stroke = None); }
    pub fn set_stroke_color(c: Color)                 { with_style_mut(|s| s.stroke = Some(c)); }
    pub fn set_stroke_weight(w: f32) {
        // NaN and negatives fall back to a hairline
        with_style_mut(|s| s.stroke_weight = if w > 0.0 { w.min(4096.0) } else { 1.0 });
    }

    fn canvas_dims() -> (usize, usize) {
        (CANVAS_W.load(Ordering::Relaxed) as usize, CANVAS_H.load(Ordering::Relaxed) as usize)
    }

    fn tfm(x: f32, y: f32) -> (f32, f32) {
        let m = current_matrix();
        (m[0]*x + m[2]*y + m[4], m[1]*x + m[3]*y + m[5])
    }

    pub fn draw_ellipse(cx: f32, cy: f32, w: f32, h: f32) {
        if !(cx.is_finite() && cy.is_finite() && w.is_finite() && h.is_finite()) { return; }
        let (cw, ch) = canvas_dims();
        let (fill, stroke, sw) = with_style(|s| (s.fill, s.stroke, s.stroke_weight));
        let pts = arc_points(cx, cy, w * 0.5, h * 0.5, 0.0, std::f32::consts::TAU, false);
        let mut buf = pixel_buf().lock();
        if let Some(fc) = fill { fill_poly(&mut buf, cw, ch, &pts, fc); }
        if let Some(sc) = stroke {
            for i in 0..pts.len() { seg(&mut buf, cw, ch, pts[i], pts[(i + 1) % pts.len()], sw, sc); }
        }
    }

    pub fn draw_rect(x: f32, y: f32, w: f32, h: f32) {
        let (cw, ch) = canvas_dims();
        let (fill, stroke, sw) = with_style(|s| (s.fill, s.stroke, s.stroke_weight));
        // Transform all four corners so rotation/scale in the matrix stack work correctly.
        let c = [tfm(x, y), tfm(x+w, y), tfm(x+w, y+h), tfm(x, y+h)];
        let mut buf = pixel_buf().lock();
        if let Some(fc) = fill { fill_poly(&mut buf, cw, ch, &c, fc); }
        if let Some(sc) = stroke { outline(&mut buf, cw, ch, &c, sw, sc); }
    }

    pub fn draw_rect_rounded(x: f32, y: f32, w: f32, h: f32, r: f32) {
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) { return; }
        // a negative size means the rect extends the other way
        let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
        let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
        // Clamp radius so it fits inside the rectangle.
        let r = r.min(w * 0.5).min(h * 0.5);
        if !(r >= 1.0) { return draw_rect(x, y, w, h); }

        let (cw, ch) = canvas_dims();
        let (fill, stroke, sw) = with_style(|s| (s.fill, s.stroke, s.stroke_weight));
        use std::f32::consts::{PI, FRAC_PI_2};
        // one quarter circle per corner, joined into a single outline
        let mut pts = Vec::new();
        for (ccx, ccy, a0) in [
            (x + r,     y + r,     PI),              // top-left
            (x + w - r, y + r,     PI + FRAC_PI_2),  // top-right
            (x + w - r, y + h - r, 0.0),             // bottom-right
            (x + r,     y + h - r, FRAC_PI_2),       // bottom-left
        ] {
            pts.extend(arc_points(ccx, ccy, r, r, a0, a0 + FRAC_PI_2, true));
        }
        let mut buf = pixel_buf().lock();
        if let Some(fc) = fill { fill_poly(&mut buf, cw, ch, &pts, fc); }
        if let Some(sc) = stroke {
            for i in 0..pts.len() { seg(&mut buf, cw, ch, pts[i], pts[(i + 1) % pts.len()], sw, sc); }
        }
    }

    pub fn draw_triangle(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) {
        let (cw, ch) = canvas_dims();
        let (fill, stroke, sw) = with_style(|s| (s.fill, s.stroke, s.stroke_weight));
        let c = [tfm(x1, y1), tfm(x2, y2), tfm(x3, y3)];
        let mut buf = pixel_buf().lock();
        if let Some(fc) = fill { fill_poly(&mut buf, cw, ch, &c, fc); }
        if let Some(sc) = stroke { outline(&mut buf, cw, ch, &c, sw, sc); }
    }

    pub fn draw_line(x1: f32, y1: f32, x2: f32, y2: f32) {
        let (cw, ch) = canvas_dims();
        let (sc, sw) = with_style(|s| (s.stroke, s.stroke_weight));
        if let Some(c) = sc {
            let mut buf = pixel_buf().lock();
            seg(&mut buf, cw, ch, tfm(x1, y1), tfm(x2, y2), sw, c);
        }
    }

    pub fn draw_point(x: f32, y: f32) {
        let (cw, ch) = canvas_dims();
        let (sc, sw) = with_style(|s| (s.stroke, s.stroke_weight));
        if let Some(c) = sc {
            let (tx, ty) = tfm(x, y);
            if !(tx.is_finite() && ty.is_finite()) { return; }
            let mut buf = pixel_buf().lock();
            if !(sw > 1.5) {
                put(&mut buf, cw, ch, tx.clamp(-FAR, FAR) as i32, ty.clamp(-FAR, FAR) as i32, c);
            } else {
                // a dot sw pixels across
                let r = sw * 0.5;
                let n = ((r * 3.0) as usize).clamp(8, 64);
                let pts: Vec<(f32, f32)> = (0..n).map(|i| {
                    let t = i as f32 / n as f32 * std::f32::consts::TAU;
                    (tx + t.cos() * r, ty + t.sin() * r)
                }).collect();
                fill_poly(&mut buf, cw, ch, &pts, c);
            }
        }
    }

    pub fn draw_quad(x1: f32, y1: f32, x2: f32, y2: f32,
                     x3: f32, y3: f32, x4: f32, y4: f32) {
        let (cw, ch) = canvas_dims();
        let (fill, stroke, sw) = with_style(|s| (s.fill, s.stroke, s.stroke_weight));
        let c = [tfm(x1, y1), tfm(x2, y2), tfm(x3, y3), tfm(x4, y4)];
        let mut buf = pixel_buf().lock();
        if let Some(fc) = fill { fill_poly(&mut buf, cw, ch, &c, fc); }
        if let Some(sc) = stroke { outline(&mut buf, cw, ch, &c, sw, sc); }
    }

    pub fn draw_arc(cx: f32, cy: f32, w: f32, h: f32, start: f32, stop: f32) {
        if !(cx.is_finite() && cy.is_finite() && w.is_finite() && h.is_finite()
             && start.is_finite() && stop.is_finite()) { return; }
        let (cw, ch) = canvas_dims();
        let (fill, stroke, sw) = with_style(|s| (s.fill, s.stroke, s.stroke_weight));
        // more than a full turn is a full turn
        let tau = std::f32::consts::TAU;
        let stop = start + (stop - start).clamp(-tau, tau);
        if stop == start { return; }
        let arc = arc_points(cx, cy, w * 0.5, h * 0.5, start, stop, true);
        let centre = tfm(cx, cy);
        let mut buf = pixel_buf().lock();

        // Fill: a pie slice.
        if let Some(fc) = fill {
            let mut pie = Vec::with_capacity(arc.len() + 1);
            pie.push(centre);
            pie.extend_from_slice(&arc);
            fill_poly(&mut buf, cw, ch, &pie, fc);
        }

        // Stroke: arc outline + two radial lines closing the pie.
        if let Some(sc) = stroke {
            for i in 1..arc.len() { seg(&mut buf, cw, ch, arc[i - 1], arc[i], sw, sc); }
            // Close with radial lines only when fill is also set (pie mode).
            if fill.is_some() && arc.len() >= 2 {
                seg(&mut buf, cw, ch, centre, arc[0], sw, sc);
                seg(&mut buf, cw, ch, centre, arc[arc.len() - 1], sw, sc);
            }
        }
    }

    // ── Transforms ────────────────────────────────────────────────────────────

    pub fn push_matrix() {
        MATRIX_STACK.with(|s| {
            let top = *s.borrow().last().unwrap();
            s.borrow_mut().push(top);
        });
    }
    pub fn pop_matrix() {
        MATRIX_STACK.with(|s| { let mut st = s.borrow_mut(); if st.len() > 1 { st.pop(); } });
    }
    pub fn translate(tx: f32, ty: f32, _tz: f32) {
        MATRIX_STACK.with(|s| {
            let mut st = s.borrow_mut();
            let top = st.last_mut().unwrap();
            let t: Mat = [1.0, 0.0, 0.0, 1.0, tx, ty];
            *top = mat_mul(top, &t);
        });
    }
    pub fn rotate_z(angle: f32) {
        MATRIX_STACK.with(|s| {
            let mut st = s.borrow_mut();
            let top = st.last_mut().unwrap();
            let (cos, sin) = (angle.cos(), angle.sin());
            let r: Mat = [cos, sin, -sin, cos, 0.0, 0.0];
            *top = mat_mul(top, &r);
        });
    }
    pub fn scale(sx: f32, sy: f32, _sz: f32) {
        MATRIX_STACK.with(|s| {
            let mut st = s.borrow_mut();
            let top = st.last_mut().unwrap();
            let sc: Mat = [sx, 0.0, 0.0, sy, 0.0, 0.0];
            *top = mat_mul(top, &sc);
        });
    }

    // ── Style stack ───────────────────────────────────────────────────────────

    pub fn push_style() {
        STYLE_STACK.with(|s| {
            let top = s.borrow().last().unwrap().clone();
            s.borrow_mut().push(top);
        });
    }
    pub fn pop_style() {
        STYLE_STACK.with(|s| { let mut st = s.borrow_mut(); if st.len() > 1 { st.pop(); } });
    }

    // ── Text rendering ────────────────────────────────────────────────────────

    thread_local! {
        static TEXT_SIZE: std::cell::RefCell<f32> = std::cell::RefCell::new(12.0);
    }

    // 5×8 bitmap font for printable ASCII (0x20–0x7F).
    // Each entry: [col0..col4]; each byte = 8 rows, bit 0 = top row.
    static FONT5X8: [[u8; 5]; 96] = [
        [0x00,0x00,0x00,0x00,0x00], // ' '
        [0x00,0x00,0x5F,0x00,0x00], // '!'
        [0x00,0x07,0x00,0x07,0x00], // '"'
        [0x14,0x7F,0x14,0x7F,0x14], // '#'
        [0x24,0x2A,0x7F,0x2A,0x12], // '$'
        [0x23,0x13,0x08,0x64,0x62], // '%'
        [0x36,0x49,0x55,0x22,0x50], // '&'
        [0x00,0x05,0x03,0x00,0x00], // '\''
        [0x00,0x1C,0x22,0x41,0x00], // '('
        [0x00,0x41,0x22,0x1C,0x00], // ')'
        [0x14,0x08,0x3E,0x08,0x14], // '*'
        [0x08,0x08,0x3E,0x08,0x08], // '+'
        [0x00,0x50,0x30,0x00,0x00], // ','
        [0x08,0x08,0x08,0x08,0x08], // '-'
        [0x00,0x60,0x60,0x00,0x00], // '.'
        [0x20,0x10,0x08,0x04,0x02], // '/'
        [0x3E,0x51,0x49,0x45,0x3E], // '0'
        [0x00,0x42,0x7F,0x40,0x00], // '1'
        [0x42,0x61,0x51,0x49,0x46], // '2'
        [0x21,0x41,0x45,0x4B,0x31], // '3'
        [0x18,0x14,0x12,0x7F,0x10], // '4'
        [0x27,0x45,0x45,0x45,0x39], // '5'
        [0x3C,0x4A,0x49,0x49,0x30], // '6'
        [0x01,0x71,0x09,0x05,0x03], // '7'
        [0x36,0x49,0x49,0x49,0x36], // '8'
        [0x06,0x49,0x49,0x29,0x1E], // '9'
        [0x00,0x36,0x36,0x00,0x00], // ':'
        [0x00,0x56,0x36,0x00,0x00], // ';'
        [0x08,0x14,0x22,0x41,0x00], // '<'
        [0x14,0x14,0x14,0x14,0x14], // '='
        [0x00,0x41,0x22,0x14,0x08], // '>'
        [0x02,0x01,0x51,0x09,0x06], // '?'
        [0x32,0x49,0x79,0x41,0x3E], // '@'
        [0x7E,0x11,0x11,0x11,0x7E], // 'A'
        [0x7F,0x49,0x49,0x49,0x36], // 'B'
        [0x3E,0x41,0x41,0x41,0x22], // 'C'
        [0x7F,0x41,0x41,0x22,0x1C], // 'D'
        [0x7F,0x49,0x49,0x49,0x41], // 'E'
        [0x7F,0x09,0x09,0x09,0x01], // 'F'
        [0x3E,0x41,0x49,0x49,0x7A], // 'G'
        [0x7F,0x08,0x08,0x08,0x7F], // 'H'
        [0x00,0x41,0x7F,0x41,0x00], // 'I'
        [0x20,0x40,0x41,0x3F,0x01], // 'J'
        [0x7F,0x08,0x14,0x22,0x41], // 'K'
        [0x7F,0x40,0x40,0x40,0x40], // 'L'
        [0x7F,0x02,0x0C,0x02,0x7F], // 'M'
        [0x7F,0x04,0x08,0x10,0x7F], // 'N'
        [0x3E,0x41,0x41,0x41,0x3E], // 'O'
        [0x7F,0x09,0x09,0x09,0x06], // 'P'
        [0x3E,0x41,0x51,0x21,0x5E], // 'Q'
        [0x7F,0x09,0x19,0x29,0x46], // 'R'
        [0x46,0x49,0x49,0x49,0x31], // 'S'
        [0x01,0x01,0x7F,0x01,0x01], // 'T'
        [0x3F,0x40,0x40,0x40,0x3F], // 'U'
        [0x1F,0x20,0x40,0x20,0x1F], // 'V'
        [0x3F,0x40,0x38,0x40,0x3F], // 'W'
        [0x63,0x14,0x08,0x14,0x63], // 'X'
        [0x07,0x08,0x70,0x08,0x07], // 'Y'
        [0x61,0x51,0x49,0x45,0x43], // 'Z'
        [0x00,0x7F,0x41,0x41,0x00], // '['
        [0x02,0x04,0x08,0x10,0x20], // '\'
        [0x00,0x41,0x41,0x7F,0x00], // ']'
        [0x04,0x02,0x01,0x02,0x04], // '^'
        [0x40,0x40,0x40,0x40,0x40], // '_'
        [0x00,0x01,0x02,0x04,0x00], // '`'
        [0x20,0x54,0x54,0x54,0x78], // 'a'
        [0x7F,0x48,0x44,0x44,0x38], // 'b'
        [0x38,0x44,0x44,0x44,0x20], // 'c'
        [0x38,0x44,0x44,0x48,0x7F], // 'd'
        [0x38,0x54,0x54,0x54,0x18], // 'e'
        [0x08,0x7E,0x09,0x01,0x02], // 'f'
        [0x0C,0x52,0x52,0x52,0x3E], // 'g'
        [0x7F,0x08,0x04,0x04,0x78], // 'h'
        [0x00,0x44,0x7D,0x40,0x00], // 'i'
        [0x20,0x40,0x44,0x3D,0x00], // 'j'
        [0x7F,0x10,0x28,0x44,0x00], // 'k'
        [0x00,0x41,0x7F,0x40,0x00], // 'l'
        [0x7C,0x04,0x18,0x04,0x78], // 'm'
        [0x7C,0x08,0x04,0x04,0x78], // 'n'
        [0x38,0x44,0x44,0x44,0x38], // 'o'
        [0x7C,0x14,0x14,0x14,0x08], // 'p'
        [0x08,0x14,0x14,0x18,0x7C], // 'q'
        [0x7C,0x08,0x04,0x04,0x08], // 'r'
        [0x48,0x54,0x54,0x54,0x20], // 's'
        [0x04,0x3F,0x44,0x40,0x20], // 't'
        [0x3C,0x40,0x40,0x20,0x7C], // 'u'
        [0x1C,0x20,0x40,0x20,0x1C], // 'v'
        [0x3C,0x40,0x30,0x40,0x3C], // 'w'
        [0x44,0x28,0x10,0x28,0x44], // 'x'
        [0x0C,0x50,0x50,0x50,0x3C], // 'y'
        [0x44,0x64,0x54,0x4C,0x44], // 'z'
        [0x00,0x08,0x36,0x41,0x00], // '{'
        [0x00,0x00,0x7F,0x00,0x00], // '|'
        [0x00,0x41,0x36,0x08,0x00], // '}'
        [0x10,0x08,0x08,0x10,0x08], // '~'
        [0x78,0x46,0x41,0x46,0x78], // DEL placeholder
    ];

    pub fn draw_text(s: &str, x: f32, y: f32) {
        let fc = match with_style(|s| s.fill) { Some(c) => c, None => return };
        // Each font pixel maps to `cell` output pixels.  Round to the nearest
        // integer so pixels are square and there are no spacing gaps.
        let cell = text_cell() as i32;
        let cell_f = cell as f32;
        let (cw, canvas_h) = canvas_dims();
        let mut buf = pixel_buf().lock();
        // "\n" starts a new line; each line is aligned on its own
        for (n, line) in s.split('\n').enumerate() {
            let (lx, ly) = text_origin(line, x, y + n as f32 * text_leading());
            let (tx, ty) = tfm(lx, ly);
            if !(tx.is_finite() && ty.is_finite()) { continue; }
            let (tx, ty) = (tx.clamp(-FAR, FAR), ty.clamp(-FAR, FAR));
            let mut cursor_x = tx;
            for c in line.chars() {
                // nothing more of this line can land on the canvas
                if cursor_x >= cw as f32 { break; }
                let code = c as u32;
                if (32..=127).contains(&code) && cursor_x + 6.0 * cell_f > 0.0 {
                    let glyph = &FONT5X8[(code - 32) as usize];
                    for col in 0..5usize {
                        let bits = glyph[col];
                        for row in 0..8usize {
                            if bits & (1 << row) != 0 {
                                let px = (cursor_x + col as f32 * cell_f) as i32;
                                let py = (ty + row as f32 * cell_f) as i32;
                                for dy in 0..cell {
                                    hspan(&mut buf, cw, canvas_h, py + dy, px, px + cell - 1, fc);
                                }
                            }
                        }
                    }
                }
                // the font only has ASCII; anything else leaves a gap one character wide
                cursor_x += 6.0 * cell_f; // 5 cols + 1 px gap
            }
        }
    }

    /// Width in pixels of `s` at the current text size (the widest line).
    pub fn text_width(s: &str) -> f32 {
        let cell_f = text_cell();
        s.split('\n').map(|l| l.chars().count()).max().unwrap_or(0) as f32 * 6.0 * cell_f
    }

    pub fn set_text_size(size: f32) {
        // NaN, zero and negatives would make every glyph loop misbehave
        let size = if size.is_finite() { size.clamp(1.0, 2048.0) } else { 12.0 };
        TEXT_SIZE.with(|s| *s.borrow_mut() = size);
    }

    pub fn set_smooth(_on: bool) {}

    pub fn reset_matrix() {
        MATRIX_STACK.with(|s| *s.borrow_mut().last_mut().unwrap() = identity());
    }

    // ── Text alignment ────────────────────────────────────────────────────
    // h: 0 left, 1 center, 2 right.  v: 0 top, 1 center, 2 bottom, 3 baseline.
    // Default is (left, top) so existing sketches keep drawing text from its
    // top-left corner.
    thread_local! {
        static TEXT_ALIGN: std::cell::Cell<(u8, u8)> = std::cell::Cell::new((0, 0));
    }
    pub fn set_text_align(h: u8, v: u8) { TEXT_ALIGN.with(|a| a.set((h.min(2), v.min(3)))); }
    fn text_cell() -> f32 {
        ((TEXT_SIZE.with(|ts| *ts.borrow()) / 8.0).max(1.0).round()) as f32
    }
    /// Distance from one line of text to the next.
    fn text_leading() -> f32 { 10.0 * text_cell() }
    /// Glyphs are 7 rows above the baseline and 1 below.
    pub fn text_ascent() -> f32 { 7.0 * text_cell() }
    pub fn text_descent() -> f32 { 1.0 * text_cell() }
    fn text_origin(s: &str, x: f32, y: f32) -> (f32, f32) {
        let (h, v) = TEXT_ALIGN.with(|a| a.get());
        let w = text_width(s);
        let x = match h { 1 => x - w / 2.0, 2 => x - w, _ => x };
        let y = match v {
            1 => y - (text_ascent() + text_descent()) / 2.0,
            2 => y - (text_ascent() + text_descent()),
            3 => y - text_ascent(),
            _ => y,
        };
        (x, y)
    }

    // ── Images ────────────────────────────────────────────────────────────
    struct Img { w: usize, h: usize, px: Vec<Color> }
    fn images() -> &'static parking_lot::Mutex<Vec<Img>> {
        static I: std::sync::OnceLock<parking_lot::Mutex<Vec<Img>>> = std::sync::OnceLock::new();
        I.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
    }
    thread_local! {
        static TINT: std::cell::Cell<Option<Color>> = std::cell::Cell::new(None);
    }
    pub fn set_tint(c: Option<Color>) { TINT.with(|t| t.set(c)); }

    /// Load a PNG. Returns a 1-based id, or 0 if it couldn't be read.
    pub fn load_image(path: &str) -> u64 {
        let decode = || -> Result<Img, String> {
            let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut dec = png::Decoder::new(f);
            dec.set_transformations(png::Transformations::normalize_to_color8());
            let mut rd = dec.read_info().map_err(|e| e.to_string())?;
            let mut buf = vec![0; rd.output_buffer_size()];
            let info = rd.next_frame(&mut buf).map_err(|e| e.to_string())?;
            let (w, h) = (info.width as usize, info.height as usize);
            let ch = info.color_type.samples();
            let px = buf[..w * h * ch].chunks(ch).map(|p| {
                let f = |v: u8| v as f32 / 255.0;
                match ch {
                    1 => Color::from_gray(f(p[0])),
                    2 => Color::from_rgba(f(p[0]), f(p[0]), f(p[0]), f(p[1])),
                    3 => Color::from_rgb(f(p[0]), f(p[1]), f(p[2])),
                    _ => Color::from_rgba(f(p[0]), f(p[1]), f(p[2]), f(p[3])),
                }
            }).collect();
            Ok(Img { w, h, px })
        };
        match decode() {
            Ok(img) => { let mut v = images().lock(); v.push(img); v.len() as u64 }
            Err(e)  => { eprintln!("loadImage(\"{}\"): {} (only PNG is supported)", path, e); 0 }
        }
    }
    pub fn image_size(id: u64) -> (f32, f32) {
        let v = images().lock();
        v.get((id as usize).wrapping_sub(1)).map(|i| (i.w as f32, i.h as f32)).unwrap_or((0.0, 0.0))
    }
    /// Draw an image scaled into (x, y, w, h). Follows translate/scale; ignores
    /// rotation (the box is placed at the transformed corners).
    pub fn draw_image(id: u64, x: f32, y: f32, w: f32, h: f32) {
        let imgs = images().lock();
        let img = match imgs.get((id as usize).wrapping_sub(1)) { Some(i) => i, None => return };
        if img.w == 0 || img.h == 0 { return; }
        let (x0, y0) = tfm(x, y);
        let (x1, y1) = tfm(x + w, y + h);
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) { return; }
        let (lx, hx) = (x0.min(x1), x0.max(x1));
        let (ly, hy) = (y0.min(y1), y0.max(y1));
        if hx - lx < 1.0 || hy - ly < 1.0 { return; }
        let tint = TINT.with(|t| t.get());
        let (cw, ch) = canvas_dims();
        let mut buf = pixel_buf().lock();
        // only the part of the image that is on the canvas
        let (py0, py1) = (ly.floor().max(0.0) as i32, hy.ceil().min(ch as f32) as i32);
        let (px0, px1) = (lx.floor().max(0.0) as i32, hx.ceil().min(cw as f32) as i32);
        for py in py0..py1 {
            let v = (py as f32 + 0.5 - ly) / (hy - ly);
            if !(0.0..1.0).contains(&v) { continue; }
            let sy = ((v * img.h as f32) as usize).min(img.h - 1);
            for px in px0..px1 {
                let u = (px as f32 + 0.5 - lx) / (hx - lx);
                if !(0.0..1.0).contains(&u) { continue; }
                let sx = ((u * img.w as f32) as usize).min(img.w - 1);
                let mut c = img.px[sy * img.w + sx];
                if let Some(t) = tint { c = Color::from_rgba(c.r * t.r, c.g * t.g, c.b * t.b, c.a * t.a); }
                if c.a > 0.0 { put(&mut buf, cw, ch, px, py, c); }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLFW / native event loop (minifb-backed)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "wasm"))]
mod glfw {
    use super::{CANVAS_W, CANVAS_H, pixel_buf, resize_canvas};
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    pub enum Event {
        Setup, Draw,
        MousePressed, MouseReleased, MouseMoved, MouseDragged,
        MouseWheel(f32),
        KeyPressed, KeyReleased, KeyTyped,
    }

    #[derive(Clone)]
    pub struct FrameContext {
        pub mouse_x: f32, pub mouse_y: f32,
        pub width: u32,   pub height: u32,
        pub frame_count: u64, pub mouse_pressed: bool,
        pub key: Option<char>, pub key_code: u32, pub key_pressed: bool,
        pub millis: u64,  pub focused: bool,
    }
    impl FrameContext {
        pub fn mouse_x(&self)       -> f32          { self.mouse_x }
        pub fn mouse_y(&self)       -> f32          { self.mouse_y }
        pub fn width(&self)         -> u32          { self.width }
        pub fn height(&self)        -> u32          { self.height }
        pub fn frame_count(&self)   -> u64          { self.frame_count }
        pub fn mouse_pressed(&self) -> bool         { self.mouse_pressed }
        pub fn key(&self)           -> Option<char> { self.key }
        pub fn key_code(&self)      -> u32          { self.key_code }
        pub fn key_pressed(&self)   -> bool         { self.key_pressed }
        pub fn millis(&self)        -> u64          { self.millis }
        pub fn focused(&self)       -> bool         { self.focused }
        fn default(w: u32, h: u32) -> Self {
            Self { mouse_x: 0.0, mouse_y: 0.0, width: w, height: h,
                   frame_count: 0, mouse_pressed: false, key: None,
                   key_code: 0, key_pressed: false, millis: 0, focused: true }
        }
    }

    // Global desired FPS / loop control
    use std::sync::{Mutex, OnceLock};
    static FRAME_RATE: OnceLock<Mutex<f32>>  = OnceLock::new();
    static LOOPING:    OnceLock<Mutex<bool>> = OnceLock::new();
    static WANT_W:     OnceLock<Mutex<u32>>  = OnceLock::new();
    static WANT_H:     OnceLock<Mutex<u32>>  = OnceLock::new();
    static WANT_TITLE: OnceLock<Mutex<String>> = OnceLock::new();
    // Like Processing, a sketch window has a fixed size unless the sketch
    // asks otherwise. A fixed size is also what makes tiling window managers
    // (i3, sway, Hyprland, bspwm) float the window instead of tiling it.
    static RESIZABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub fn set_resizable(on: bool) { RESIZABLE.store(on, Ordering::Relaxed); }
    // Present mode: a borderless window that covers one screen, with the
    // sketch centered on a plain background. (width, height, x, y) of the screen.
    static PRESENT: Mutex<Option<(usize, usize, isize, isize)>> = Mutex::new(None);
    pub fn set_present(w: usize, h: usize, x: isize, y: isize) {
        if w > 0 && h > 0 { *PRESENT.lock().unwrap() = Some((w.min(16384), h.min(16384), x, y)); }
    }
    const PRESENT_BACKGROUND: u32 = 0xFF_66_66_66;   // Processing's default

    fn fps() -> f32  { *FRAME_RATE.get_or_init(|| Mutex::new(60.0)).lock().unwrap() }
    fn looping() -> bool { *LOOPING.get_or_init(|| Mutex::new(true)).lock().unwrap() }

    pub fn set_size(w: u32, h: u32) {
        let (w, h) = (w.clamp(1, super::MAX_CANVAS), h.clamp(1, super::MAX_CANVAS));
        *WANT_W.get_or_init(|| Mutex::new(w)).lock().unwrap() = w;
        *WANT_H.get_or_init(|| Mutex::new(h)).lock().unwrap() = h;
        resize_canvas(w, h);
    }
    pub fn set_fullscreen(_on: bool) {} // not supported on minifb
    pub fn set_frame_rate(f: f32) {
        // NaN, zero and negatives would stall or spin the loop
        let f = if f.is_finite() { f.clamp(1.0, 1000.0) } else { 60.0 };
        *FRAME_RATE.get_or_init(|| Mutex::new(60.0)).lock().unwrap() = f;
        MEASURED_FPS.store(f.to_bits(), Ordering::Relaxed);
    }
    pub fn set_looping(v: bool)   { *LOOPING.get_or_init(|| Mutex::new(true)).lock().unwrap() = v; }
    static REDRAW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    // f32 bits; 60.0 until the first frames have been timed
    static MEASURED_FPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x42700000);
    pub fn request_redraw()       { REDRAW.store(true, Ordering::Relaxed); }
    pub fn exit_sketch()          {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::process::exit(0);
    }
    /// Frames per second actually reached (smoothed), like Processing's frameRate.
    pub fn measured_frame_rate() -> f32 { f32::from_bits(MEASURED_FPS.load(Ordering::Relaxed)) }
    pub fn set_window_title(t: &str) {
        *WANT_TITLE.get_or_init(|| Mutex::new(t.to_owned())).lock().unwrap() = t.to_owned();
    }
    pub static MOUSE_BUTTON:   std::sync::atomic::AtomicU32  = std::sync::atomic::AtomicU32::new(0);
    static CURSOR_VISIBLE:     std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    pub fn set_cursor_visible(v: bool) { CURSOR_VISIBLE.store(v, Ordering::Relaxed); }
    pub fn current_frame_rate() -> f32 { measured_frame_rate() }
    pub fn is_looping() -> bool { looping() }

    pub struct Runner { title: String }
    impl Runner {
        pub fn new() -> Self { Self { title: String::from("sketch") } }
        pub fn title(mut self, t: &str) -> Self { self.title = t.to_owned(); self }

        pub fn start<F>(self, mut cb: F) where F: FnMut(Event, &FrameContext) {
            use minifb::{Window, WindowOptions, Key, MouseMode, MouseButton};

            // Set by the IDE that started this sketch.
            // PROCESSING_WATCH_STDIN: the IDE keeps our stdin open and never
            // writes to it, so end of input means the IDE is gone (crashed or
            // killed) and nobody can press Stop any more. Close instead of
            // staying behind as an orphan window.
            if std::env::var_os("PROCESSING_WATCH_STDIN").is_some() {
                let _ = std::thread::Builder::new().name("ide-watch".into()).spawn(|| {
                    use std::io::Read;
                    let mut buf = [0u8; 256];
                    let mut stdin = std::io::stdin();
                    loop {
                        match stdin.read(&mut buf) {
                            Ok(0) => std::process::exit(0),
                            Ok(_) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => return,   // no usable stdin: nothing to watch
                        }
                    }
                });
            }
            // PROCESSING_PRESENT="width,height,x,y": present mode on that screen.
            if let Ok(v) = std::env::var("PROCESSING_PRESENT") {
                let n: Vec<i64> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                if n.len() == 4 && n[0] > 0 && n[1] > 0 {
                    set_present(n[0] as usize, n[1] as usize, n[2] as isize, n[3] as isize);
                }
            }

            let init_w = CANVAS_W.load(Ordering::Relaxed);
            let init_h = CANVAS_H.load(Ordering::Relaxed);

            // Run Setup first (may call size() to set dimensions)
            {
                let ctx = FrameContext::default(init_w, init_h);
                cb(Event::Setup, &ctx);
            }

            let w = CANVAS_W.load(Ordering::Relaxed) as usize;
            let h = CANVAS_H.load(Ordering::Relaxed) as usize;

            let title = WANT_TITLE
                .get().map(|m| m.lock().unwrap().clone())
                .unwrap_or_else(|| self.title.clone());

            let present = *PRESENT.lock().unwrap();
            let (win_w, win_h, options) = match present {
                Some((pw, ph, _, _)) => (pw, ph, WindowOptions {
                    borderless: true, topmost: true, resize: false, ..WindowOptions::default() }),
                None => (w, h, WindowOptions {
                    resize: RESIZABLE.load(Ordering::Relaxed), ..WindowOptions::default() }),
            };
            let mut window = match Window::new(&title, win_w, win_h, options) {
                Ok(win) => win,
                Err(e) => {
                    eprintln!("Could not open the sketch window ({}x{}): {}", w, h, e);
                    std::process::exit(1);
                }
            };

            if let Some((_, _, px, py)) = present { window.set_position(px, py); }
            // what is shown in present mode: the background with the canvas in the middle
            let mut screen: Vec<u32> = match present {
                Some((pw, ph, _, _)) => vec![PRESENT_BACKGROUND; pw * ph],
                None => Vec::new(),
            };
            let mut screen_canvas = (0usize, 0usize);   // canvas size the background was laid out for

            let mut target_fps = fps();
            window.set_target_fps(target_fps as usize);

            // Every key transition, in order, as the OS reports it. Polling
            // the key state once a frame instead loses a quick tap whenever
            // the press and the release land inside the same frame, which
            // at low frame rates is most taps.
            struct KeyTap(std::sync::Arc<Mutex<Vec<(Key, bool)>>>);
            impl minifb::InputCallback for KeyTap {
                fn add_char(&mut self, _c: u32) {}
                fn set_key_state(&mut self, key: Key, down: bool) {
                    if let Ok(mut q) = self.0.lock() { q.push((key, down)); }
                }
            }
            let key_events: std::sync::Arc<Mutex<Vec<(Key, bool)>>> = Default::default();
            window.set_input_callback(Box::new(KeyTap(key_events.clone())));
            let mut held: Vec<Key> = Vec::new();   // keys we have reported as pressed
            let mut quit = false;

            let start = Instant::now();
            let mut frame_count: u64 = 0;
            let mut last_frame = Instant::now();
            let mut last_mouse_pressed = false;
            let mut last_mouse_x = 0.0f32;
            let mut last_mouse_y = 0.0f32;

            // Escape closes the sketch, after the sketch's own keyPressed() has seen it
            while window.is_open() && !quit {
                // frameRate() can be called at any time, not just in setup()
                if fps() != target_fps {
                    target_fps = fps();
                    window.set_target_fps(target_fps as usize);
                }
                let cur_w = window.get_size().0;
                let cur_h = window.get_size().1;
                // a minimized window reports 0x0; keep the canvas as it is
                if present.is_none() && cur_w > 0 && cur_h > 0
                    && (cur_w != CANVAS_W.load(Ordering::Relaxed) as usize
                        || cur_h != CANVAS_H.load(Ordering::Relaxed) as usize)
                {
                    resize_canvas(cur_w as u32, cur_h as u32);
                }

                let (mut mx, mut my) = window.get_mouse_pos(MouseMode::Clamp)
                    .map(|(x,y)| (x, y))
                    .unwrap_or((0.0, 0.0));
                if let Some((pw, ph, _, _)) = present {
                    // the sketch sits in the middle of the screen
                    let cw = CANVAS_W.load(Ordering::Relaxed) as f32;
                    let ch = CANVAS_H.load(Ordering::Relaxed) as f32;
                    mx = (mx - ((pw as f32 - cw) / 2.0).floor()).clamp(0.0, (cw - 1.0).max(0.0));
                    my = (my - ((ph as f32 - ch) / 2.0).floor()).clamp(0.0, (ch - 1.0).max(0.0));
                }

                let (bl, br, bm) = (window.get_mouse_down(MouseButton::Left),
                                    window.get_mouse_down(MouseButton::Right),
                                    window.get_mouse_down(MouseButton::Middle));
                let mouse_pressed = bl || br || bm;
                if mouse_pressed {
                    // Processing codes: LEFT 37, RIGHT 39, CENTER 3
                    MOUSE_BUTTON.store(if bl { 37 } else if br { 39 } else { 3 }, Ordering::Relaxed);
                }
                window.set_cursor_visibility(CURSOR_VISIBLE.load(Ordering::Relaxed));

                let millis = start.elapsed().as_millis() as u64;
                let mut ctx = FrameContext {
                    mouse_x: mx, mouse_y: my,
                    width:  CANVAS_W.load(Ordering::Relaxed),
                    height: CANVAS_H.load(Ordering::Relaxed),
                    frame_count, mouse_pressed,
                    key: None, key_code: 0, key_pressed: false,
                    millis, focused: true,
                };

                // Mouse events
                if mouse_pressed && !last_mouse_pressed {
                    cb(Event::MousePressed, &ctx);
                } else if !mouse_pressed && last_mouse_pressed {
                    cb(Event::MouseReleased, &ctx);
                }
                if (mx - last_mouse_x).abs() > 0.5 || (my - last_mouse_y).abs() > 0.5 {
                    if mouse_pressed {
                        cb(Event::MouseDragged, &ctx);
                    } else {
                        cb(Event::MouseMoved, &ctx);
                    }
                }

                // Scroll (approximated via key)
                if let Some(scroll) = window.get_scroll_wheel() {
                    cb(Event::MouseWheel(scroll.1), &ctx);
                }

                // Key events, one per real press and release
                let batch: Vec<(Key, bool)> = key_events.lock()
                    .map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
                // Auto-repeat shows up as a release followed at once by a
                // press of the same key. That is not the key coming up.
                let mut skip = vec![false; batch.len()];
                for i in 0..batch.len() {
                    if skip[i] || batch[i].1 { continue; }
                    if let Some(j) = (i + 1..batch.len()).find(|&j| batch[j].0 == batch[i].0) {
                        if batch[j].1 { skip[i] = true; skip[j] = true; }
                    }
                }
                let shifted = |held: &Vec<Key>| held.contains(&Key::LeftShift) || held.contains(&Key::RightShift);
                for (i, &(k, down)) in batch.iter().enumerate() {
                    if skip[i] { continue; }
                    if down {
                        if held.contains(&k) { continue; }
                        held.push(k);
                        if k == Key::Escape { quit = true; }
                        ctx.key_pressed = true;
                        ctx.key_code = k as u32;
                        ctx.key = key_to_char(k, shifted(&held));
                        cb(Event::KeyPressed, &ctx);
                        if ctx.key.is_some() { cb(Event::KeyTyped, &ctx); }
                    } else if let Some(pos) = held.iter().position(|h| *h == k) {
                        let shift = shifted(&held);
                        held.remove(pos);
                        ctx.key_pressed = !held.is_empty();
                        ctx.key_code = k as u32;
                        ctx.key = key_to_char(k, shift);
                        cb(Event::KeyReleased, &ctx);
                    }
                }
                if window.is_key_down(Key::Escape) { quit = true; }
                // A key let go while the window didn't have focus never
                // reports its release; don't leave it stuck down.
                for k in held.clone() {
                    if !window.is_key_down(k) {
                        held.retain(|h| *h != k);
                        ctx.key_pressed = !held.is_empty();
                        ctx.key_code = k as u32;
                        ctx.key = key_to_char(k, false);
                        cb(Event::KeyReleased, &ctx);
                    }
                }

                // Draw frame. With noLoop() draw() still runs once, and again
                // after each redraw(), like Processing.
                let redraw = REDRAW.swap(false, Ordering::Relaxed);
                if looping() || redraw || frame_count == 0 {
                    frame_count += 1;   // the first draw() sees frameCount 1
                    ctx.frame_count = frame_count;
                    let now = Instant::now();
                    let dt = now.duration_since(last_frame).as_secs_f32();
                    last_frame = now;
                    if frame_count > 1 && dt > 0.0 && looping() {
                        let old = measured_frame_rate();
                        let fresh = (1.0 / dt).min(1000.0);
                        MEASURED_FPS.store((old * 0.9 + fresh * 0.1).to_bits(), Ordering::Relaxed);
                    }
                    // key/key_code stay None/0 so sync_globals won't clobber the
                    // persisted last-pressed values; key_pressed reflects current hold state.
                    ctx.key = None; ctx.key_code = 0;
                    ctx.key_pressed = !held.is_empty();
                    ctx.mouse_x = mx; ctx.mouse_y = my; ctx.mouse_pressed = mouse_pressed;
                    cb(Event::Draw, &ctx);
                }

                // Blit pixel buffer
                let buf = pixel_buf().lock();
                let cw = CANVAS_W.load(Ordering::Relaxed) as usize;
                let ch = CANVAS_H.load(Ordering::Relaxed) as usize;
                if let Some((pw, ph, _, _)) = present {
                    if screen_canvas != (cw, ch) {
                        screen.iter_mut().for_each(|p| *p = PRESENT_BACKGROUND);
                        screen_canvas = (cw, ch);
                    }
                    // centered; a canvas bigger than the screen shows its middle
                    let (copy_w, copy_h) = (cw.min(pw), ch.min(ph));
                    let (dst_x, dst_y) = ((pw - copy_w) / 2, (ph - copy_h) / 2);
                    let (src_x, src_y) = ((cw - copy_w) / 2, (ch - copy_h) / 2);
                    if buf.len() >= cw * ch {
                        for row in 0..copy_h {
                            let s0 = (src_y + row) * cw + src_x;
                            let d0 = (dst_y + row) * pw + dst_x;
                            screen[d0..d0 + copy_w].copy_from_slice(&buf[s0..s0 + copy_w]);
                        }
                    }
                    drop(buf);
                    let _ = window.update_with_buffer(&screen, pw, ph);
                } else {
                    let _ = window.update_with_buffer(&buf, cw, ch);
                    drop(buf);
                }

                last_mouse_pressed = mouse_pressed;
                last_mouse_x = mx;
                last_mouse_y = my;
            }
        }
    }

    fn key_to_char(k: minifb::Key, shift: bool) -> Option<char> {
        use minifb::Key::*;
        // Letters: shift → uppercase
        if shift {
            match k {
                A => return Some('A'), B => return Some('B'), C => return Some('C'),
                D => return Some('D'), E => return Some('E'), F => return Some('F'),
                G => return Some('G'), H => return Some('H'), I => return Some('I'),
                J => return Some('J'), K => return Some('K'), L => return Some('L'),
                M => return Some('M'), N => return Some('N'), O => return Some('O'),
                P => return Some('P'), Q => return Some('Q'), R => return Some('R'),
                S => return Some('S'), T => return Some('T'), U => return Some('U'),
                V => return Some('V'), W => return Some('W'), X => return Some('X'),
                Y => return Some('Y'), Z => return Some('Z'),
                // US QWERTY shifted digits/symbols
                Key0 => return Some(')'), Key1 => return Some('!'),
                Key2 => return Some('@'), Key3 => return Some('#'),
                Key4 => return Some('$'), Key5 => return Some('%'),
                Key6 => return Some('^'), Key7 => return Some('&'),
                Key8 => return Some('*'), Key9 => return Some('('),
                Minus => return Some('_'), Equal => return Some('+'),
                LeftBracket => return Some('{'), RightBracket => return Some('}'),
                Semicolon => return Some(':'), Apostrophe => return Some('"'),
                Comma => return Some('<'), Period => return Some('>'),
                Slash => return Some('?'), Backslash => return Some('|'),
                _ => {}
            }
        }
        match k {
            A => Some('a'), B => Some('b'), C => Some('c'), D => Some('d'),
            E => Some('e'), F => Some('f'), G => Some('g'), H => Some('h'),
            I => Some('i'), J => Some('j'), K => Some('k'), L => Some('l'),
            M => Some('m'), N => Some('n'), O => Some('o'), P => Some('p'),
            Q => Some('q'), R => Some('r'), S => Some('s'), T => Some('t'),
            U => Some('u'), V => Some('v'), W => Some('w'), X => Some('x'),
            Y => Some('y'), Z => Some('z'),
            Key0 => Some('0'), Key1 => Some('1'), Key2 => Some('2'),
            Key3 => Some('3'), Key4 => Some('4'), Key5 => Some('5'),
            Key6 => Some('6'), Key7 => Some('7'), Key8 => Some('8'),
            Key9 => Some('9'),
            Minus => Some('-'), Equal => Some('='),
            LeftBracket => Some('['), RightBracket => Some(']'),
            Semicolon => Some(';'), Apostrophe => Some('\''),
            Comma => Some(','), Period => Some('.'), Slash => Some('/'),
            Backslash => Some('\\'), Space => Some(' '), Enter => Some('\n'),
            Tab => Some('\t'), Backspace => Some('\x08'),
            Escape => Some('\x1b'), Delete => Some('\x7f'),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WASM stub (no-op; replace with wasm-bindgen + canvas 2D later)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "wasm")]
mod wasm {
    pub enum Event {
        Draw,
        MousePressed, MouseReleased, MouseMoved, MouseDragged,
        MouseWheel(f32),
        KeyPressed, KeyReleased, KeyTyped,
    }

    pub struct FrameContext {
        pub mouse_x: f32, pub mouse_y: f32,
        pub width: u32,   pub height: u32,
        pub frame_count: u64, pub mouse_pressed: bool,
        pub key: Option<char>, pub key_code: u32, pub key_pressed: bool,
        pub millis: u64,  pub focused: bool,
    }
    impl FrameContext {
        pub fn mouse_x(&self)       -> f32          { self.mouse_x }
        pub fn mouse_y(&self)       -> f32          { self.mouse_y }
        pub fn width(&self)         -> u32          { self.width }
        pub fn height(&self)        -> u32          { self.height }
        pub fn frame_count(&self)   -> u64          { self.frame_count }
        pub fn mouse_pressed(&self) -> bool         { self.mouse_pressed }
        pub fn key(&self)           -> Option<char> { self.key }
        pub fn key_code(&self)      -> u32          { self.key_code }
        pub fn key_pressed(&self)   -> bool         { self.key_pressed }
        pub fn millis(&self)        -> u64          { self.millis }
        pub fn focused(&self)       -> bool         { self.focused }
    }

    pub struct Runner { title: String }
    impl Runner {
        pub fn new() -> Self { Self { title: String::from("sketch") } }
        pub fn title(mut self, t: &str) -> Self { self.title = t.to_owned(); self }
        pub fn start<F>(self, _cb: F) where F: FnMut(Event, &FrameContext) + 'static {}
    }

    pub fn set_size(_w: u32, _h: u32) {}
    pub fn set_frame_rate(_fps: f32) {}
    pub fn set_looping(_on: bool) {}
    pub fn request_redraw() {}
    pub fn exit_sketch() {}
    pub fn set_cursor_visible(_v: bool) {}
}

// ─────────────────────────────────────────────────────────────────────────────
// Public re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use core::{Color, PVector};

// ──────────────────────────── Global sketch state ─────────────────────────────

#[derive(Debug)]
struct Globals {
    mouse_x: f32, mouse_y: f32, pmouse_x: f32, pmouse_y: f32,
    width: f32, height: f32, frame_count: u64,
    mouse_pressed: bool,
    // `key` persists — it holds the last pressed character, exactly like Processing.
    key: char, key_code: u32, key_pressed: bool,
    millis: u64, focused: bool,
}

impl Default for Globals {
    fn default() -> Self {
        Self {
            mouse_x: 0.0, mouse_y: 0.0, pmouse_x: 0.0, pmouse_y: 0.0,
            width: 0.0, height: 0.0, frame_count: 0,
            mouse_pressed: false,
            key: '\0', key_code: 0, key_pressed: false,
            millis: 0, focused: false,
        }
    }
}

static GLOBALS: OnceLock<RwLock<Globals>> = OnceLock::new();
fn globals() -> &'static RwLock<Globals> {
    GLOBALS.get_or_init(|| RwLock::new(Globals::default()))
}

#[cfg(not(feature = "wasm"))]
fn sync_globals(ctx: &glfw::FrameContext) {
    let mut g = globals().write();
    g.pmouse_x = g.mouse_x; g.pmouse_y = g.mouse_y;
    g.mouse_x     = ctx.mouse_x();
    g.mouse_y     = ctx.mouse_y();
    g.width       = ctx.width() as f32;
    g.height      = ctx.height() as f32;
    g.frame_count = ctx.frame_count();
    g.mouse_pressed = ctx.mouse_pressed();
    // `key` and `key_code` persist across frames (Processing semantics): only update
    // when the context carries an actual key event (not the Draw call's cleared fields).
    if let Some(k) = ctx.key() { g.key = k; }
    if ctx.key_code() != 0     { g.key_code = ctx.key_code(); }
    g.key_pressed = ctx.key_pressed();
    g.millis      = ctx.millis();
    g.focused     = ctx.focused();
}

// ─────────────────────────── Public globals API ───────────────────────────────

#[inline] pub fn mouse_x()  -> f32  { globals().read().mouse_x }
#[inline] pub fn mouse_y()  -> f32  { globals().read().mouse_y }
#[inline] pub fn pmouse_x() -> f32  { globals().read().pmouse_x }
#[inline] pub fn pmouse_y() -> f32  { globals().read().pmouse_y }
#[inline] pub fn width()    -> f32  { globals().read().width }
#[inline] pub fn height()   -> f32  { globals().read().height }
#[inline] pub fn frame_count() -> u64  { globals().read().frame_count }
#[inline] pub fn mouse_pressed_global() -> bool { globals().read().mouse_pressed }
/// Last key pressed (persists across frames, '\0' before any key event).
#[inline] pub fn key()      -> char  { globals().read().key }
#[inline] pub fn key_code() -> u32   { globals().read().key_code }
#[inline] pub fn key_pressed_global() -> bool { globals().read().key_pressed }
#[inline] pub fn millis()   -> u64   { globals().read().millis }
#[inline] pub fn focused()  -> bool  { globals().read().focused }

// camelCase aliases — these are what the emitter generates for bare identifier references.
#[inline] pub fn mouseX()      -> f32  { mouse_x() }
#[inline] pub fn mouseY()      -> f32  { mouse_y() }
#[inline] pub fn pmouseX()     -> f32  { pmouse_x() }
#[inline] pub fn pmouseY()     -> f32  { pmouse_y() }
#[inline] pub fn frameCount()  -> u64  { frame_count() }
#[inline] pub fn mousePressed() -> bool { mouse_pressed_global() }
#[inline] pub fn keyPressed()  -> bool { key_pressed_global() }
#[inline] pub fn keyCode()     -> u32  { key_code() }

// ── Mouse wheel delta ─────────────────────────────────────────────────────────

static WHEEL_DELTA: OnceLock<parking_lot::Mutex<f32>> = OnceLock::new();
fn wheel_cell() -> &'static parking_lot::Mutex<f32> {
    WHEEL_DELTA.get_or_init(|| parking_lot::Mutex::new(0.0))
}
#[doc(hidden)]
pub fn __set_wheel_delta(d: f32) { *wheel_cell().lock() = d; }
#[inline] pub fn mouse_wheel_delta() -> f32 { *wheel_cell().lock() }

// ──────────────────────────── Canvas / window ────────────────────────────────

pub fn size(w: u32, h: u32) {
    #[cfg(not(feature = "wasm"))] glfw::set_size(w, h);
    #[cfg(feature = "wasm")]      wasm::set_size(w, h);
}
pub fn full_screen() {
    #[cfg(not(feature = "wasm"))] glfw::set_fullscreen(true);
}
#[inline] pub fn fullScreen() { full_screen() }

pub fn frame_rate(fps: f32) {
    #[cfg(not(feature = "wasm"))] glfw::set_frame_rate(fps);
    #[cfg(feature = "wasm")]      wasm::set_frame_rate(fps);
}
#[inline] pub fn frameRate(fps: f32) { frame_rate(fps) }

// ──────────────────────────── Background / color ─────────────────────────────

pub fn background(gray: f32)                              { render::background_gray(gray / 255.0); }
pub fn background_rgb(r: f32, g: f32, b: f32)            { render::background_rgb(r/255.0, g/255.0, b/255.0); }
pub fn background_rgba(r: f32, g: f32, b: f32, a: f32)   { render::background_rgba(r/255.0, g/255.0, b/255.0, a/255.0); }
pub fn background_color(c: Color)                         { render::background_color(c); }

pub fn fill(gray: f32)                                    { render::set_fill_gray(gray / 255.0); }
pub fn fill_rgb(r: f32, g: f32, b: f32)                  { render::set_fill_rgb(r/255.0, g/255.0, b/255.0); }
pub fn fill_rgba(r: f32, g: f32, b: f32, a: f32)         { render::set_fill_rgba(r/255.0, g/255.0, b/255.0, a/255.0); }
pub fn fill_color(c: Color)                               { render::set_fill_color(c); }
pub fn no_fill()                                          { render::set_fill_none(); }
#[inline] pub fn noFill() { no_fill() }

pub fn stroke(gray: f32)                                  { render::set_stroke_gray(gray / 255.0); }
pub fn stroke_rgb(r: f32, g: f32, b: f32)                { render::set_stroke_rgb(r/255.0, g/255.0, b/255.0); }
pub fn stroke_rgba(r: f32, g: f32, b: f32, a: f32)       { render::set_stroke_rgba(r/255.0, g/255.0, b/255.0, a/255.0); }
pub fn stroke_color(c: Color)                             { render::set_stroke_color(c); }
pub fn no_stroke()                                        { render::set_stroke_none(); }
#[inline] pub fn noStroke() { no_stroke() }

pub fn stroke_weight(w: f32)                              { render::set_stroke_weight(w); }
#[inline] pub fn strokeWeight(w: f32) { stroke_weight(w) }

// ─────────────────────────── Primitive shapes ────────────────────────────────

pub fn ellipse(x: f32, y: f32, w: f32, h: f32)           { render::draw_ellipse(x, y, w, h); }
pub fn circle(x: f32, y: f32, d: f32)                    { render::draw_ellipse(x, y, d, d); }
pub fn rect(x: f32, y: f32, w: f32, h: f32)              { render::draw_rect(x, y, w, h); }
pub fn rect_rounded(x: f32, y: f32, w: f32, h: f32, r: f32) { render::draw_rect_rounded(x, y, w, h, r); }
#[inline] pub fn rectRounded(x: f32, y: f32, w: f32, h: f32, r: f32) { rect_rounded(x, y, w, h, r) }
pub fn square(x: f32, y: f32, s: f32)                    { render::draw_rect(x, y, s, s); }
pub fn triangle(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) { render::draw_triangle(x1, y1, x2, y2, x3, y3); }
pub fn line(x1: f32, y1: f32, x2: f32, y2: f32)          { render::draw_line(x1, y1, x2, y2); }
pub fn point(x: f32, y: f32)                              { render::draw_point(x, y); }
pub fn quad(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32, x4: f32, y4: f32) {
    render::draw_quad(x1, y1, x2, y2, x3, y3, x4, y4);
}
pub fn arc(x: f32, y: f32, w: f32, h: f32, start: f32, stop: f32) { render::draw_arc(x, y, w, h, start, stop); }

// ─────────────────────────── Transforms ──────────────────────────────────────

pub fn push_matrix()              { render::push_matrix(); }
pub fn pop_matrix()               { render::pop_matrix(); }
#[inline] pub fn pushMatrix() { push_matrix() }
#[inline] pub fn popMatrix()  { pop_matrix()  }
pub fn translate(x: f32, y: f32) { render::translate(x, y, 0.0); }
pub fn translate_3d(x: f32, y: f32, z: f32) { render::translate(x, y, z); }
pub fn rotate(angle: f32)         { render::rotate_z(angle); }
pub fn scale(s: f32)              { render::scale(s, s, 1.0); }
pub fn scale_xy(sx: f32, sy: f32) { render::scale(sx, sy, 1.0); }

// ──────────────────────────── Style state ────────────────────────────────────

pub fn push_style() { render::push_style(); }
pub fn pop_style()  { render::pop_style(); }
#[inline] pub fn pushStyle() { push_style() }
#[inline] pub fn popStyle()  { pop_style()  }

// ─────────────────────────── Text ────────────────────────────────────────────

pub fn text(s: &str, x: f32, y: f32)  { render::draw_text(s, x, y); }
pub fn text_size(size: f32)            { render::set_text_size(size); }
pub fn text_width(s: &str) -> f32      { render::text_width(s) }
#[inline] pub fn textSize(size: f32)  { text_size(size) }
#[inline] pub fn textWidth(s: &str) -> f32 { text_width(s) }

// ─────────────────────────── Math ────────────────────────────────────────────

pub use std::f32::consts::PI;
pub use std::f32::consts::TAU;
pub const TWO_PI:     f32 = std::f32::consts::TAU;
pub const HALF_PI:    f32 = std::f32::consts::FRAC_PI_2;
pub const QUARTER_PI: f32 = std::f32::consts::FRAC_PI_4;

// ─────────────────────────── Key code constants ───────────────────────────────
// Matches minifb::Key discriminant values returned by key_code().
// Usage:  if key_code() == LEFT { ... }
pub const UP:        u32 = 54;
pub const DOWN:      u32 = 51;
pub const LEFT:      u32 = 52;
pub const RIGHT:     u32 = 53;
pub const ENTER:     u32 = 69;
pub const BACKSPACE: u32 = 66;
pub const DELETE:    u32 = 67;
pub const ESCAPE:    u32 = 70;
pub const TAB:       u32 = 78;
pub const HOME:      u32 = 71;
pub const END:       u32 = 68;
pub const PAGE_UP:   u32 = 75;
pub const PAGE_DOWN: u32 = 74;
pub const SHIFT:     u32 = 82;
pub const CONTROL:   u32 = 84;
pub const F1:        u32 = 36;
pub const F2:        u32 = 37;
pub const F3:        u32 = 38;
pub const F4:        u32 = 39;
pub const F5:        u32 = 40;
pub const F6:        u32 = 41;
pub const F7:        u32 = 42;
pub const F8:        u32 = 43;
pub const F9:        u32 = 44;
pub const F10:       u32 = 45;
pub const F11:       u32 = 46;
pub const F12:       u32 = 47;

#[inline] pub fn radians(deg: f32) -> f32 { deg.to_radians() }
#[inline] pub fn degrees(rad: f32) -> f32 { rad.to_degrees() }
#[inline] pub fn constrain(val: f32, low: f32, high: f32) -> f32 { val.clamp(low, high) }
#[inline] pub fn map(val: f32, start1: f32, stop1: f32, start2: f32, stop2: f32) -> f32 {
    start2 + (stop2 - start2) * ((val - start1) / (stop1 - start1))
}
#[inline] pub fn lerp(start: f32, stop: f32, t: f32) -> f32 { start + (stop - start) * t }
#[inline] pub fn dist(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    ((x2-x1).powi(2) + (y2-y1).powi(2)).sqrt()
}
#[inline] pub fn mag(x: f32, y: f32) -> f32 { (x*x + y*y).sqrt() }
#[inline] pub fn abs(v: f32)  -> f32 { v.abs() }
#[inline] pub fn ceil(v: f32) -> f32 { v.ceil() }
#[inline] pub fn floor(v: f32) -> f32 { v.floor() }
#[inline] pub fn round(v: f32) -> f32 { v.round() }
#[inline] pub fn sqrt(v: f32) -> f32 { v.sqrt() }
#[inline] pub fn pow(base: f32, exp: f32) -> f32 { base.powf(exp) }
#[inline] pub fn log(v: f32)  -> f32 { v.ln() }
#[inline] pub fn exp(v: f32)  -> f32 { v.exp() }
#[inline] pub fn min(a: f32, b: f32) -> f32 { a.min(b) }
#[inline] pub fn max(a: f32, b: f32) -> f32 { a.max(b) }

pub fn noise(x: f32) -> f32 { core::noise1(x) }
pub fn noise2(x: f32, y: f32) -> f32 { core::noise2(x, y) }

// ─────────────────────────── Random ──────────────────────────────────────────

pub fn random(high: f32) -> f32 { core::random_f32(0.0, high) }
pub fn random_range(low: f32, high: f32) -> f32 { core::random_f32(low, high) }
pub fn random_seed(seed: u64) { core::random_seed(seed); }
#[inline] pub fn randomSeed(seed: u64) { random_seed(seed) }

// ─────────────────────────── Color constructors ───────────────────────────────

#[inline] pub fn color(gray: f32) -> Color { Color::from_gray(gray / 255.0) }
#[inline] pub fn color_rgb(r: f32, g: f32, b: f32) -> Color { Color::from_rgb(r/255.0, g/255.0, b/255.0) }
#[inline] pub fn color_rgba(r: f32, g: f32, b: f32, a: f32) -> Color { Color::from_rgba(r/255.0, g/255.0, b/255.0, a/255.0) }
pub fn lerp_color(c1: Color, c2: Color, t: f32) -> Color { core::lerp_color(c1, c2, t) }
#[inline] pub fn lerpColor(c1: Color, c2: Color, t: f32) -> Color { lerp_color(c1, c2, t) }

// ─────────────────────────── Console ─────────────────────────────────────────

/// Print a value to the console with a newline (mirrors Processing's `println()`).
pub fn println(s: impl std::fmt::Display) { ::std::println!("{}", s); }
/// Print a value to the console without a newline (mirrors Processing's `print()`).
pub fn print(s: impl std::fmt::Display)   { ::std::print!("{}", s); }

// ─────────────────────────── Sketch control ──────────────────────────────────

pub fn no_loop() {
    #[cfg(not(feature = "wasm"))] glfw::set_looping(false);
    #[cfg(feature = "wasm")]      wasm::set_looping(false);
}
#[inline] pub fn noLoop() { no_loop() }

pub fn r#loop() {
    #[cfg(not(feature = "wasm"))] glfw::set_looping(true);
    #[cfg(feature = "wasm")]      wasm::set_looping(true);
}

pub fn redraw() {
    #[cfg(not(feature = "wasm"))] glfw::request_redraw();
    #[cfg(feature = "wasm")]      wasm::request_redraw();
}

pub fn exit() {
    #[cfg(not(feature = "wasm"))] glfw::exit_sketch();
    #[cfg(feature = "wasm")]      wasm::exit_sketch();
}

/// Call in setup(): whether the sketch window can be resized (default: no).
pub fn window_resizable(on: bool) {
    #[cfg(not(feature = "wasm"))] glfw::set_resizable(on);
    #[cfg(feature = "wasm")] { let _ = on; }
}
#[inline] pub fn windowResizable(on: bool) { window_resizable(on) }
/// Present mode: cover the screen at (x, y) of size w x h, sketch centered.
/// Call before the window opens (the IDE does, when Present is pressed).
pub fn present(w: usize, h: usize, x: isize, y: isize) {
    #[cfg(not(feature = "wasm"))] glfw::set_present(w, h, x, y);
    #[cfg(feature = "wasm")] { let _ = (w, h, x, y); }
}
pub fn window_title(t: &str) {
    #[cfg(not(feature = "wasm"))] glfw::set_window_title(t);
}
#[inline] pub fn windowTitle(t: &str) { window_title(t) }

pub fn smooth()    { render::set_smooth(true); }
pub fn no_smooth() { render::set_smooth(false); }
#[inline] pub fn noSmooth() { no_smooth() }

pub fn no_cursor() {
    #[cfg(not(feature = "wasm"))] glfw::set_cursor_visible(false);
    #[cfg(feature = "wasm")]      wasm::set_cursor_visible(false);
}
#[inline] pub fn noCursor() { no_cursor() }

pub fn cursor() {
    #[cfg(not(feature = "wasm"))] glfw::set_cursor_visible(true);
    #[cfg(feature = "wasm")]      wasm::set_cursor_visible(true);
}

// ─────────────────────────── Shapes: bezier / curve ──────────────────────────
// (begin_shape / vertex / end_shape and the kind constants are further down.)

pub const POLYGON: u8 = 0;
#[inline] pub fn beginShape() { begin_shape() }
#[inline] pub fn endShape() { end_shape() }
#[inline] pub fn bezierVertex(cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) { bezier_vertex(cx1, cy1, cx2, cy2, x, y) }
#[inline] pub fn curveVertex(x: f32, y: f32) { curve_vertex(x, y) }

pub fn bezier(x1: f32, y1: f32, cx1: f32, cy1: f32, cx2: f32, cy2: f32, x2: f32, y2: f32) {
    begin_shape(); vertex(x1, y1); bezier_vertex(cx1, cy1, cx2, cy2, x2, y2); end_shape();
}
/// Catmull-Rom segment from (x1,y1) to (x2,y2); the outer points are controls.
pub fn curve(cx1: f32, cy1: f32, x1: f32, y1: f32, x2: f32, y2: f32, cx2: f32, cy2: f32) {
    begin_shape();
    curve_vertex(cx1, cy1); curve_vertex(x1, y1); curve_vertex(x2, y2); curve_vertex(cx2, cy2);
    end_shape();
}

pub fn reset_matrix() { render::reset_matrix(); }
#[inline] pub fn resetMatrix() { reset_matrix() }

// Stroke caps and joins are accepted for compatibility; the software
// rasterizer always draws round-ish caps and mitred joins.
pub fn stroke_cap(_cap: u32)  {}
pub fn stroke_join(_join: u32) {}

// ─────────────────────────── Text alignment ──────────────────────────────────

pub const ALIGN_LEFT:     u8 = 0;
pub const ALIGN_CENTER:   u8 = 1;
pub const ALIGN_RIGHT:    u8 = 2;
pub const ALIGN_TOP:      u8 = 0;
pub const ALIGN_BOTTOM:   u8 = 2;
pub const ALIGN_BASELINE: u8 = 3;

/// h: ALIGN_LEFT / ALIGN_CENTER / ALIGN_RIGHT.
/// v: ALIGN_TOP / ALIGN_CENTER / ALIGN_BOTTOM / ALIGN_BASELINE.
pub fn text_align(h: u8, v: u8) { render::set_text_align(h, v); }
pub fn text_ascent()  -> f32 { render::text_ascent() }
pub fn text_descent() -> f32 { render::text_descent() }
#[inline] pub fn textAlign(h: u8, v: u8) { text_align(h, v) }
#[inline] pub fn textAscent() -> f32 { text_ascent() }
#[inline] pub fn textDescent() -> f32 { text_descent() }

// ─────────────────────────── Images ──────────────────────────────────────────

/// PNG only. Returns an image id (0 if loading failed).
pub fn load_image(path: &str) -> u64 { render::load_image(path) }
pub fn image(id: u64, x: f32, y: f32) {
    let (w, h) = render::image_size(id);
    render::draw_image(id, x, y, w, h);
}
pub fn image_sized(id: u64, x: f32, y: f32, w: f32, h: f32) { render::draw_image(id, x, y, w, h); }
pub fn image_width(id: u64)  -> f32 { render::image_size(id).0 }
pub fn image_height(id: u64) -> f32 { render::image_size(id).1 }
pub fn tint(gray: f32)                            { render::set_tint(Some(Color::from_gray(gray / 255.0))); }
pub fn tint_rgba(r: f32, g: f32, b: f32, a: f32)  { render::set_tint(Some(Color::from_rgba(r/255.0, g/255.0, b/255.0, a/255.0))); }
pub fn no_tint()                                  { render::set_tint(None); }
#[inline] pub fn loadImage(path: &str) -> u64 { load_image(path) }
#[inline] pub fn noTint() { no_tint() }

// ─────────────────────────── More input / noise / random ─────────────────────

/// Last mouse button pressed: 37 left, 39 right, 3 center (Processing codes).
pub fn mouse_button() -> u32 {
    #[cfg(not(feature = "wasm"))] { glfw::MOUSE_BUTTON.load(Ordering::Relaxed) }
    #[cfg(feature = "wasm")]      { 37 }
}
#[inline] pub fn mouseButton() -> u32 { mouse_button() }

pub fn noise3(x: f32, y: f32, z: f32) -> f32 { core::noise3(x, y, z) }
pub fn noise_seed(seed: u64) { core::noise_seed(seed); }
/// Number of octaves summed, and how much each one counts relative to the
/// one before (Processing's defaults: 4 and 0.5).
pub fn noise_detail(octaves: u32, falloff: f32) { core::noise_detail(octaves, falloff); }
#[inline] pub fn noiseSeed(seed: u64) { noise_seed(seed) }
#[inline] pub fn randomGaussian() -> f32 { random_gaussian() }

/// Frames per second the sketch is actually reaching (smoothed); starts at
/// the target set by frame_rate().
pub fn current_frame_rate() -> f32 {
    #[cfg(not(feature = "wasm"))] { glfw::current_frame_rate() }
    #[cfg(feature = "wasm")]      { 60.0 }
}
pub fn is_looping() -> bool {
    #[cfg(not(feature = "wasm"))] { glfw::is_looping() }
    #[cfg(feature = "wasm")]      { true }
}

/// Color of one canvas pixel as (r, g, b), each 0..255. Black outside the canvas.
pub fn get_pixel(x: i32, y: i32) -> (u8, u8, u8) {
    let (w, h) = (CANVAS_W.load(Ordering::Relaxed) as i32, CANVAS_H.load(Ordering::Relaxed) as i32);
    if x < 0 || y < 0 || x >= w || y >= h { return (0, 0, 0); }
    let buf = pixel_buf().lock();
    match buf.get(y as usize * w as usize + x as usize) {
        Some(p) => ((p >> 16) as u8, (p >> 8) as u8, *p as u8),
        None    => (0, 0, 0),
    }
}

// ─────────────────────────── saveFrame ───────────────────────────────────────

/// Save the current canvas as a PNG. "#" runs in the name become the
/// zero-padded frame number, like Processing: save_frame("out-####.png").
pub fn save_frame(name: &str) {
    let path = match name.find('#') {
        Some(i) => {
            let n = name[i..].chars().take_while(|c| *c == '#').count();
            format!("{}{:0width$}{}", &name[..i], frame_count(), &name[i + n..], width = n)
        }
        None => name.to_owned(),
    };
    let (w, h) = (CANVAS_W.load(Ordering::Relaxed), CANVAS_H.load(Ordering::Relaxed));
    let rgb: Vec<u8> = pixel_buf().lock().iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
        .collect();
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        let f = std::io::BufWriter::new(std::fs::File::create(&path)?);
        let mut enc = png::Encoder::new(f, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgb)?;
        Ok(())
    };
    if let Err(e) = write() { eprintln!("saveFrame(\"{}\"): {}", path, e); }
}
#[inline] pub fn saveFrame(name: &str) { save_frame(name) }

// ─────────────────────────────── App builder ─────────────────────────────────

pub struct App<S: 'static> {
    state:             S,
    setup_fn:          fn(&mut S),
    draw_fn:           fn(&mut S),
    title:             String,
    mouse_pressed_fn:  Option<fn(&mut S)>,
    mouse_released_fn: Option<fn(&mut S)>,
    mouse_moved_fn:    Option<fn(&mut S)>,
    mouse_dragged_fn:  Option<fn(&mut S)>,
    mouse_wheel_fn:    Option<fn(&mut S, f32)>,
    key_pressed_fn:    Option<fn(&mut S)>,
    key_released_fn:   Option<fn(&mut S)>,
    key_typed_fn:      Option<fn(&mut S)>,
}

impl<S: 'static> App<S> {
    pub fn new(state: S, setup_fn: fn(&mut S), draw_fn: fn(&mut S)) -> Self {
        Self {
            state, setup_fn, draw_fn,
            title: String::from("sketch"),
            mouse_pressed_fn: None, mouse_released_fn: None,
            mouse_moved_fn: None,   mouse_dragged_fn: None,
            mouse_wheel_fn: None,   key_pressed_fn: None,
            key_released_fn: None,  key_typed_fn: None,
        }
    }

    pub fn title(mut self, t: &str) -> Self { self.title = t.to_owned(); self }

    pub fn mouse_pressed(mut self, f: fn(&mut S)) -> Self  { self.mouse_pressed_fn  = Some(f); self }
    pub fn mouse_released(mut self, f: fn(&mut S)) -> Self { self.mouse_released_fn = Some(f); self }
    pub fn mouse_moved(mut self, f: fn(&mut S)) -> Self    { self.mouse_moved_fn    = Some(f); self }
    pub fn mouse_dragged(mut self, f: fn(&mut S)) -> Self  { self.mouse_dragged_fn  = Some(f); self }
    pub fn mouse_wheel(mut self, f: fn(&mut S, f32)) -> Self { self.mouse_wheel_fn  = Some(f); self }
    pub fn key_pressed(mut self, f: fn(&mut S)) -> Self    { self.key_pressed_fn    = Some(f); self }
    pub fn key_released(mut self, f: fn(&mut S)) -> Self   { self.key_released_fn   = Some(f); self }
    pub fn key_typed(mut self, f: fn(&mut S)) -> Self      { self.key_typed_fn      = Some(f); self }

    pub fn run(mut self) {
        #[cfg(not(feature = "wasm"))]
        {
            let runner = glfw::Runner::new().title(&self.title);
            runner.start(move |event, ctx| {
                use glfw::Event;
                match event {
                    Event::Setup         => { (self.setup_fn)(&mut self.state); }
                    Event::Draw          => {
                        render::begin_draw();
                        sync_globals(ctx);
                        (self.draw_fn)(&mut self.state);
                        render::end_draw();
                        render::present();
                    }
                    // Sync globals before every event callback so mouseX()/mouseY()/key()
                    // reflect the position and key that triggered this specific event.
                    Event::MousePressed  => { sync_globals(ctx); if let Some(f) = self.mouse_pressed_fn  { f(&mut self.state); } }
                    Event::MouseReleased => { sync_globals(ctx); if let Some(f) = self.mouse_released_fn { f(&mut self.state); } }
                    Event::MouseMoved    => { sync_globals(ctx); if let Some(f) = self.mouse_moved_fn    { f(&mut self.state); } }
                    Event::MouseDragged  => { sync_globals(ctx); if let Some(f) = self.mouse_dragged_fn  { f(&mut self.state); } }
                    Event::MouseWheel(d) => {
                        sync_globals(ctx);
                        __set_wheel_delta(d);
                        if let Some(f) = self.mouse_wheel_fn { f(&mut self.state, d); }
                    }
                    Event::KeyPressed    => { sync_globals(ctx); if let Some(f) = self.key_pressed_fn    { f(&mut self.state); } }
                    Event::KeyReleased   => { sync_globals(ctx); if let Some(f) = self.key_released_fn   { f(&mut self.state); } }
                    Event::KeyTyped      => { sync_globals(ctx); if let Some(f) = self.key_typed_fn      { f(&mut self.state); } }
                }
            });
        }

        #[cfg(feature = "wasm")]
        {
            let runner = wasm::Runner::new().title(&self.title);
            (self.setup_fn)(&mut self.state);

            let mut state         = self.state;
            let draw_fn           = self.draw_fn;
            let mouse_pressed_fn  = self.mouse_pressed_fn;
            let mouse_released_fn = self.mouse_released_fn;
            let mouse_moved_fn    = self.mouse_moved_fn;
            let mouse_dragged_fn  = self.mouse_dragged_fn;
            let mouse_wheel_fn    = self.mouse_wheel_fn;
            let key_pressed_fn    = self.key_pressed_fn;
            let key_released_fn   = self.key_released_fn;
            let key_typed_fn      = self.key_typed_fn;

            runner.start(move |event, ctx| {
                use wasm::Event;
                match event {
                    Event::Draw => {
                        render::begin_draw();
                        {
                            let mut g = globals().write();
                            g.pmouse_x = g.mouse_x; g.pmouse_y = g.mouse_y;
                            g.mouse_x  = ctx.mouse_x(); g.mouse_y = ctx.mouse_y();
                            g.width    = ctx.width() as f32; g.height = ctx.height() as f32;
                            g.frame_count = ctx.frame_count();
                            g.mouse_pressed = ctx.mouse_pressed();
                            if let Some(k) = ctx.key() { g.key = k; }
                            if ctx.key_code() != 0 { g.key_code = ctx.key_code(); }
                            g.key_pressed = ctx.key_pressed();
                            g.millis   = ctx.millis(); g.focused = ctx.focused();
                        }
                        draw_fn(&mut state);
                        render::end_draw();
                        render::present();
                    }
                    Event::MousePressed  => { if let Some(f) = mouse_pressed_fn  { f(&mut state); } }
                    Event::MouseReleased => { if let Some(f) = mouse_released_fn { f(&mut state); } }
                    Event::MouseMoved    => { if let Some(f) = mouse_moved_fn    { f(&mut state); } }
                    Event::MouseDragged  => { if let Some(f) = mouse_dragged_fn  { f(&mut state); } }
                    Event::MouseWheel(d) => { if let Some(f) = mouse_wheel_fn    { f(&mut state, d); } }
                    Event::KeyPressed    => { if let Some(f) = key_pressed_fn    { f(&mut state); } }
                    Event::KeyReleased   => { if let Some(f) = key_released_fn   { f(&mut state); } }
                    Event::KeyTyped      => { if let Some(f) = key_typed_fn      { f(&mut state); } }
                }
            });
        }
    }
}

// Gray + alpha, used by OverloadRewriter for fill(gray, alpha) etc.
pub fn fill_ga(gray: f32, alpha: f32)       { fill_rgba(gray, gray, gray, alpha); }
pub fn stroke_ga(gray: f32, alpha: f32)     { stroke_rgba(gray, gray, gray, alpha); }
pub fn background_ga(gray: f32, alpha: f32) { background_rgba(gray, gray, gray, alpha); }

// ── Shapes ───────────────────────────────────────────────────────────────────
pub const POINTS: u8 = 1;
pub const LINES: u8 = 2;
pub const TRIANGLES: u8 = 3;
pub const TRIANGLE_STRIP: u8 = 4;
pub const TRIANGLE_FAN: u8 = 5;
pub const QUADS: u8 = 6;
pub const QUAD_STRIP: u8 = 7;
pub const CLOSE: u8 = 1;

pub fn begin_shape()                  { render::shape_begin(0); }
pub fn begin_shape_kind(kind: u8)     { render::shape_begin(kind); }
pub fn vertex(x: f32, y: f32)         { render::shape_vertex(x, y); }
pub fn bezier_vertex(cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
    render::shape_bezier_vertex(cx1, cy1, cx2, cy2, x, y);
}
pub fn quadratic_vertex(cx: f32, cy: f32, x: f32, y: f32) { render::shape_quadratic_vertex(cx, cy, x, y); }
pub fn curve_vertex(x: f32, y: f32)   { render::shape_curve_vertex(x, y); }
pub fn end_shape()                    { render::shape_end(false); }
pub fn end_shape_mode(mode: u8)       { render::shape_end(mode == CLOSE); }

/// Gaussian random number, mean 0 and standard deviation 1 (Box-Muller).
pub fn random_gaussian() -> f32 {
    let u1 = random_range(f32::EPSILON, 1.0);
    let u2 = random(1.0);
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}
