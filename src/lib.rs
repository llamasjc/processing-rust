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
            let (sc, sw, cap, join) = with_style(|s| (s.stroke, s.stroke_weight, s.cap, s.join));
            let Some(sc) = sc else { return };
            let (cw, ch) = canvas_dims();
            let screen: Vec<(f32, f32)> = pts.iter().map(|p| tfm(p.0, p.1)).collect();
            let mut buf = pixel_buf().lock();
            if close && n >= 3 {
                outline(&mut buf, cw, ch, &screen, sw, sc);
            } else if join == 2 && sc.a >= 1.0 {
                for i in 1..n { capsule(&mut buf, cw, ch, screen[i-1], screen[i], sw, sc); }
            } else if (join == 0 || (join == 2 && sc.a < 1.0)) && n > 2 && sw > 1.5 {
                // open path, pointed joins: one polygon, then the ends
                let r = sw.min(4096.0) * 0.5;
                let mut path = screen.clone();
                if cap == 2 {
                    // PROJECT: half a weight further at both ends
                    let stretch = |from: (f32, f32), to: (f32, f32)| -> (f32, f32) {
                        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                        let len = (dx * dx + dy * dy).sqrt();
                        if len.is_finite() && len > 1.0e-6 { (to.0 + dx / len * r, to.1 + dy / len * r) } else { to }
                    };
                    path[0] = stretch(screen[1], screen[0]);
                    path[n - 1] = stretch(screen[n - 2], screen[n - 1]);
                }
                stroke_path(&mut buf, cw, ch, &path, false, sw, sc);
                if cap == 0 && sc.a >= 1.0 {
                    capsule(&mut buf, cw, ch, screen[0], screen[0], sw, sc);
                    capsule(&mut buf, cw, ch, screen[n - 1], screen[n - 1], sw, sc);
                }
            } else {
                // open path: square ends where pieces meet, the chosen cap at the two ends
                for i in 1..n {
                    let piece_cap = if n == 2 { cap } else { 1 };
                    capped(&mut buf, cw, ch, screen[i-1], screen[i], sw, sc, piece_cap);
                    if (join == 1 || join == 2) && i + 1 < n && sw > 1.5 {
                        bevel(&mut buf, cw, ch, screen[i-1], screen[i], screen[i+1], sw, sc);
                    }
                }
                if n > 2 && sw > 1.5 {
                    if join == 0 {
                        for i in 1..n - 1 { miter(&mut buf, cw, ch, screen[i-1], screen[i], screen[i+1], sw, sc); }
                    }
                    if cap == 0 {
                        // round ends
                        capsule(&mut buf, cw, ch, screen[0], screen[0], sw, sc);
                        capsule(&mut buf, cw, ch, screen[n-1], screen[n-1], sw, sc);
                    }
                }
            }
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
        cap:  u8,   // 0 ROUND (default), 1 SQUARE, 2 PROJECT
        join: u8,   // 0 MITER (default), 1 BEVEL, 2 ROUND
    }
    impl Default for Style {
        fn default() -> Self {
            Self {
                fill:   Some(Color::from_gray(1.0)),
                stroke: Some(Color::from_gray(0.0)),
                stroke_weight: 1.0,
                cap: 0,
                join: 0,
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

    // smooth() / noSmooth(): anti-aliased edges, on by default as in Processing.
    static SMOOTH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    #[inline]
    fn smooth_on() -> bool { SMOOTH.load(Ordering::Relaxed) }

    // blendMode(): how a drawn color combines with what is already there.
    // 0 BLEND (default), 1 ADD, 2 SUBTRACT, 3 DARKEST, 4 LIGHTEST,
    // 5 DIFFERENCE, 6 EXCLUSION, 7 MULTIPLY, 8 SCREEN, 9 REPLACE
    static BLEND_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
    pub fn set_blend_mode(mode: u8) { BLEND_MODE.store(if mode <= 9 { mode } else { 0 }, Ordering::Relaxed); }
    #[inline]
    fn blend_mode() -> u8 { BLEND_MODE.load(Ordering::Relaxed) }

    // Drawing into a createGraphics() layer: its pixels carry alpha in the top
    // byte (0 = nothing drawn there yet), so the layer can be put over
    // anything later. The main canvas is always opaque and ignores that byte.
    static ALPHA_TARGET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    #[inline]
    fn alpha_target() -> bool { ALPHA_TARGET.load(Ordering::Relaxed) }
    /// A fully opaque pixel for the current target.
    #[inline]
    fn solid(col: Color) -> u32 { col.to_u32() | if alpha_target() { 0xFF00_0000 } else { 0 } }

    #[inline]
    fn blend(dst: u32, col: Color) -> u32 {
        if alpha_target() {
            let a = col.a.clamp(0.0, 1.0);
            let da = (dst >> 24) as f32 / 255.0;
            let oa = a + da * (1.0 - a);
            if !(oa > 0.0) { return 0; }
            let mixed = if da <= 0.0 {
                col.to_u32()                      // nothing underneath: just the color
            } else if blend_mode() != 0 {
                blend_opaque(dst, col)            // modes combine with the color underneath
            } else {
                let ch = |shift: u32, s: f32| -> f32 {
                    let d = ((dst >> shift) & 0xFF) as f32 / 255.0;
                    (s * a + d * da * (1.0 - a)) / oa
                };
                Color::from_rgb(ch(16, col.r), ch(8, col.g), ch(0, col.b)).to_u32()
            };
            return (mixed & 0x00FF_FFFF) | (((oa * 255.0).round() as u32).min(255) << 24);
        }
        blend_opaque(dst, col)
    }

    #[inline]
    fn blend_opaque(dst: u32, col: Color) -> u32 {
        let a = col.a.clamp(0.0, 1.0);
        let dr = ((dst >> 16) & 0xFF) as f32 / 255.0;
        let dg = ((dst >> 8) & 0xFF) as f32 / 255.0;
        let db = (dst & 0xFF) as f32 / 255.0;
        let mode = blend_mode();
        if mode == 0 {
            return Color::from_rgb(dr + (col.r - dr) * a, dg + (col.g - dg) * a, db + (col.b - db) * a).to_u32();
        }
        // the mode's result for a fully opaque source, then faded in by alpha
        let f = |d: f32, s: f32| -> f32 {
            let s = s.clamp(0.0, 1.0);
            let full = match mode {
                1 => d + s,
                2 => d - s,
                3 => d.min(s),
                4 => d.max(s),
                5 => (d - s).abs(),
                6 => d + s - 2.0 * d * s,
                7 => d * s,
                8 => 1.0 - (1.0 - d) * (1.0 - s),
                _ => s,
            };
            if mode == 9 { full } else { d + (full - d) * a }
        };
        Color::from_rgb(f(dr, col.r), f(dg, col.g), f(db, col.b)).to_u32()
    }

    fn put(buf: &mut Vec<u32>, w: usize, h: usize, x: i32, y: i32, col: Color) {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 { return; }
        if !(col.a > 0.0) { return; }             // also skips a NaN alpha
        if let Some(p) = buf.get_mut(y as usize * w + x as usize) {
            *p = if col.a >= 1.0 && blend_mode() == 0 { solid(col) } else { blend(*p, col) };
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
            if col.a >= 1.0 && blend_mode() == 0 { run.fill(solid(col)); }
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
        if smooth_on() { fill_poly_smooth(buf, w, h, pts, col) } else { fill_poly_hard(buf, w, h, pts, col) }
    }

    /// Anti-aliased fill. Each pixel row is sampled on SUB lines; along a
    /// line the covered length of every pixel is exact, so near-horizontal
    /// and near-vertical edges both come out smooth. Pixels that are fully
    /// inside are written in runs, exactly as the hard-edged fill does, so
    /// only the edge pixels cost extra.
    fn fill_poly_smooth(buf: &mut Vec<u32>, w: usize, h: usize, pts: &[(f32, f32)], col: Color) {
        const SUB: usize = 4;
        if pts.len() < 3 || w == 0 || h == 0 { return; }
        if !(col.a > 0.0) { return; }
        if pts.iter().any(|p| p.0.is_nan() || p.1.is_nan()) { return; }
        let pts: Vec<(f32, f32)> = pts.iter().map(|p| (p.0.clamp(-FAR, FAR), p.1.clamp(-FAR, FAR))).collect();
        let n = pts.len();

        let (mut ymin, mut ymax, mut xmin, mut xmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &pts { ymin = ymin.min(p.1); ymax = ymax.max(p.1); xmin = xmin.min(p.0); xmax = xmax.max(p.0); }
        let row0 = ymin.floor().max(0.0) as i64;
        let row1 = (ymax.ceil().min(h as f32)) as i64;
        let col0 = xmin.floor().max(0.0) as i64;
        let col1 = (xmax.ceil().min(w as f32)) as i64;        // exclusive
        if row0 >= row1 || col0 >= col1 { return; }
        let span = (col1 - col0) as usize;

        // per row: `part` takes the partly covered ends of each run, `step`
        // marks where fully covered stretches start and stop
        thread_local! {
            static SCRATCH: std::cell::RefCell<(Vec<f32>, Vec<f32>)> = std::cell::RefCell::new((Vec::new(), Vec::new()));
        }
        // both arrays are all zero between calls (every row cleans up after itself)
        let (mut part, mut step) = SCRATCH.with(|s| std::mem::take(&mut *s.borrow_mut()));
        if part.len() < span + 1 { part.resize(span + 1, 0.0); step.resize(span + 1, 0.0); }
        let mut xs: Vec<(f32, i32)> = Vec::with_capacity(8);
        let weight = 1.0 / SUB as f32;

        // Which edges cross which rows, so a row only looks at its own few
        // edges instead of all of them: `first[r]..first[r + 1]` indexes the
        // edges of row r in `edges_of`.
        let rows = (row1 - row0) as usize;
        let edge_rows = |i: usize| -> Option<(usize, usize)> {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let (lo, hi) = (a.1.min(b.1), a.1.max(b.1));
            if !(hi > lo) { return None; }                       // horizontal: never crossed
            let r0 = (lo.floor() as i64).max(row0);
            let r1 = (hi.floor() as i64).min(row1 - 1);
            if r0 > r1 { None } else { Some(((r0 - row0) as usize, (r1 - row0) as usize)) }
        };
        let mut first = vec![0u32; rows + 2];
        for i in 0..n {
            if let Some((r0, r1)) = edge_rows(i) { for r in r0..=r1 { first[r + 2] += 1; } }
        }
        for r in 2..rows + 2 { first[r] += first[r - 1]; }
        let mut edges_of = vec![0u32; first[rows + 1] as usize];
        for i in 0..n {
            if let Some((r0, r1)) = edge_rows(i) {
                for r in r0..=r1 { edges_of[first[r + 1] as usize] = i as u32; first[r + 1] += 1; }
            }
        }

        for row in row0..row1 {
            let ri = (row - row0) as usize;
            let mine = &edges_of[first[ri] as usize..first[ri + 1] as usize];
            if mine.len() < 2 { continue; }
            // the pixels this row actually reaches: [lo, hi]
            let (mut lo, mut hi) = (usize::MAX, 0usize);
            for sub in 0..SUB {
                let yc = row as f32 + (sub as f32 + 0.5) / SUB as f32;
                xs.clear();
                for &ei in mine {
                    let i = ei as usize;
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
                    // covered stretch [xa, xb) on this line, relative to col0
                    let xa = (xs[k].0 - col0 as f32).max(0.0);
                    let xb = (xs[k + 1].0 - col0 as f32).min(span as f32);
                    if !(xb > xa) { continue; }
                    let (ia, ib) = (xa.floor() as usize, (xb.floor() as usize).min(span));
                    lo = lo.min(ia);
                    hi = hi.max(ib.min(span - 1));
                    if ia == ib {
                        part[ia] += (xb - xa) * weight;
                    } else {
                        part[ia] += (ia as f32 + 1.0 - xa) * weight;
                        if ib < span { part[ib] += (xb - ib as f32) * weight; }
                        if ia + 1 < ib { step[ia + 1] += weight; step[ib] -= weight; }
                    }
                }
            }
            if lo > hi { continue; }

            // turn the two arrays into coverage and paint the row
            let mut running = 0.0f32;
            let mut run_start: Option<usize> = None;   // start of a stretch of full pixels
            for i in lo..=hi {
                running += step[i];
                let cov = (running + part[i]).min(1.0);
                step[i] = 0.0;
                part[i] = 0.0;
                if cov >= 0.999 {
                    if run_start.is_none() { run_start = Some(i); }
                    continue;
                }
                if let Some(s0) = run_start.take() {
                    hspan(buf, w, h, row as i32, (col0 as usize + s0) as i32, (col0 as usize + i - 1) as i32, col);
                }
                if cov > 0.004 {
                    put(buf, w, h, (col0 as usize + i) as i32, row as i32,
                        Color::from_rgba(col.r, col.g, col.b, col.a * cov));
                }
            }
            if let Some(s0) = run_start.take() {
                hspan(buf, w, h, row as i32, (col0 as usize + s0) as i32, (col0 as usize + hi) as i32, col);
            }
            // a run that ends exactly at the right edge leaves its end marker one past hi
            if hi + 1 <= span { step[hi + 1] = 0.0; part[hi + 1] = 0.0; }
            step[span] = 0.0;
            part[span] = 0.0;
        }
        SCRATCH.with(|s| *s.borrow_mut() = (part, step));
    }

    fn fill_poly_hard(buf: &mut Vec<u32>, w: usize, h: usize, pts: &[(f32, f32)], col: Color) {
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
        if !(weight > 1.5) {
            if !smooth_on() { return bline(buf, w, h, a.0, a.1, b.0, b.1, col); }
            return hairline(buf, w, h, a, b, col);
        }
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

    /// Strokes a whole path as ONE polygon (both edges of the stroke, joined
    /// at the ends), so a translucent or smoothed stroke is even along its
    /// length: no darker joints, no hairline gaps between pieces. Corners are
    /// pointed; one too sharp for that is cut off.
    fn stroke_path(buf: &mut Vec<u32>, w: usize, h: usize,
                   path: &[(f32, f32)], closed: bool, weight: f32, col: Color) {
        // leave out points that repeat the one before
        let mut pts: Vec<(f32, f32)> = Vec::with_capacity(path.len());
        for p in path {
            if !(p.0.is_finite() && p.1.is_finite()) { continue; }
            if let Some(q) = pts.last() {
                if (p.0 - q.0).abs() < 1.0e-4 && (p.1 - q.1).abs() < 1.0e-4 { continue; }
            }
            pts.push(*p);
        }
        if closed && pts.len() > 1 {
            let (a, b) = (pts[0], pts[pts.len() - 1]);
            if (a.0 - b.0).abs() < 1.0e-4 && (a.1 - b.1).abs() < 1.0e-4 { pts.pop(); }
        }
        let n = pts.len();
        if n < 2 { return; }
        let mut weight = weight;
        if !(weight > 1.5) {
            if !smooth_on() || n == 2 {
                let last = if closed { n } else { n - 1 };
                for i in 0..last { seg(buf, w, h, pts[i], pts[(i + 1) % n], weight, col); }
                return;
            }
            // a smooth hairline: one pixel wide, through pixel centres (see hairline)
            for p in pts.iter_mut() { *p = (p.0 + 0.5, p.1 + 0.5); }
            weight = 1.0;
        } else if n == 2 {
            return seg(buf, w, h, pts[0], pts[1], weight, col);
        }

        let r = weight.min(4096.0) * 0.5;
        // unit normal (to the left) of the piece from a to b
        let normal = |a: (f32, f32), b: (f32, f32)| -> (f32, f32) {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt().max(1.0e-9);
            (-dy / len, dx / len)
        };
        let (mut left, mut right) = (Vec::with_capacity(n + 4), Vec::with_capacity(n + 4));
        for i in 0..n {
            let p = pts[i];
            let before = if i > 0 { Some(normal(pts[i - 1], p)) } else if closed { Some(normal(pts[n - 1], p)) } else { None };
            let after  = if i + 1 < n { Some(normal(p, pts[i + 1])) } else if closed { Some(normal(p, pts[0])) } else { None };
            match (before, after) {
                (Some(n0), Some(n1)) => {
                    let (sx, sy) = (n0.0 + n1.0, n0.1 + n1.1);
                    let len = (sx * sx + sy * sy).sqrt();
                    let k = if len > 1.0e-4 { (sx * n0.0 + sy * n0.1) / len } else { 0.0 };
                    if k < 0.25 {
                        // too sharp for a point: square it off
                        left.push((p.0 + n0.0 * r, p.1 + n0.1 * r));
                        left.push((p.0 + n1.0 * r, p.1 + n1.1 * r));
                        right.push((p.0 - n0.0 * r, p.1 - n0.1 * r));
                        right.push((p.0 - n1.0 * r, p.1 - n1.1 * r));
                    } else {
                        let (mx, my) = (sx / len * r / k, sy / len * r / k);
                        left.push((p.0 + mx, p.1 + my));
                        right.push((p.0 - mx, p.1 - my));
                    }
                }
                (Some(m), None) | (None, Some(m)) => {
                    left.push((p.0 + m.0 * r, p.1 + m.1 * r));
                    right.push((p.0 - m.0 * r, p.1 - m.1 * r));
                }
                (None, None) => {}
            }
        }
        if left.len() < 2 { return; }
        let mut poly = left.clone();
        if closed {
            // outer loop, across to the inner loop, around it the other way,
            // and back: the two crossings cancel, the middle stays empty
            poly.push(left[0]);
            poly.push(right[0]);
            poly.extend(right.iter().skip(1).rev());
            poly.push(right[0]);
        } else {
            poly.extend(right.iter().rev());
        }
        fill_poly(buf, w, h, &poly, col);
    }

    /// A smooth line one pixel wide, through pixel centres like the stepped
    /// one it replaces (so a line from (10, 10) sits on row 10, not between
    /// rows 9 and 10).
    fn hairline(buf: &mut Vec<u32>, w: usize, h: usize, a: (f32, f32), b: (f32, f32), col: Color) {
        if !(a.0.is_finite() && a.1.is_finite() && b.0.is_finite() && b.1.is_finite()) { return; }
        let (a, b) = ((a.0 + 0.5, a.1 + 0.5), (b.0 + 0.5, b.1 + 0.5));
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0e-6 {
            return put(buf, w, h, a.0.clamp(-FAR, FAR) as i32, a.1.clamp(-FAR, FAR) as i32, col);
        }
        let (ux, uy) = (dx / len * 0.5, dy / len * 0.5);   // half a pixel along the line
        let (nx, ny) = (-uy, ux);                          // half a pixel across it
        fill_poly_smooth(buf, w, h, &[
            (a.0 - ux - nx, a.1 - uy - ny), (a.0 - ux + nx, a.1 - uy + ny),
            (b.0 + ux + nx, b.1 + uy + ny), (b.0 + ux - nx, b.1 + uy - ny),
        ], col);
    }

    /// A stroke from a to b with round ends, as one polygon so a translucent
    /// stroke is the same shade along its whole length.
    fn capsule(buf: &mut Vec<u32>, w: usize, h: usize,
               a: (f32, f32), b: (f32, f32), weight: f32, col: Color) {
        if !(weight > 1.5) { return seg(buf, w, h, a, b, weight, col); }
        let r = weight.min(4096.0) * 0.5;
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if !len.is_finite() { return; }
        let base = if len < 1.0e-6 { 0.0 } else { dy.atan2(dx) };
        let n = ((r * 1.6) as usize).clamp(4, 48);   // points per half circle
        let mut pts = Vec::with_capacity(2 * n + 2);
        let pi = std::f32::consts::PI;
        for i in 0..=n {   // around b
            let t = base - pi / 2.0 + pi * (i as f32 / n as f32);
            pts.push((b.0 + t.cos() * r, b.1 + t.sin() * r));
        }
        for i in 0..=n {   // around a
            let t = base + pi / 2.0 + pi * (i as f32 / n as f32);
            pts.push((a.0 + t.cos() * r, a.1 + t.sin() * r));
        }
        fill_poly(buf, w, h, &pts, col);
    }

    /// A line the way line() draws it: strokeCap() decides the ends.
    fn capped(buf: &mut Vec<u32>, w: usize, h: usize,
              a: (f32, f32), b: (f32, f32), weight: f32, col: Color, cap: u8) {
        if !(weight > 1.5) { return seg(buf, w, h, a, b, weight, col); }
        match cap {
            1 => seg(buf, w, h, a, b, weight, col),                    // SQUARE: stops at the ends
            2 => {                                                     // PROJECT: half a weight further
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len = (dx * dx + dy * dy).sqrt();
                if !len.is_finite() || len < 1.0e-6 { return; }
                let e = weight.min(4096.0) * 0.5 / len;
                seg(buf, w, h, (a.0 - dx * e, a.1 - dy * e), (b.0 + dx * e, b.1 + dy * e), weight, col);
            }
            _ => capsule(buf, w, h, a, b, weight, col),               // ROUND
        }
    }

    /// Outline of a closed shape with corners (rect, triangle, quad):
    /// strokeJoin() decides the corners.
    fn outline(buf: &mut Vec<u32>, w: usize, h: usize, pts: &[(f32, f32)], weight: f32, col: Color) {
        let join = with_style(|s| s.join);
        let n = pts.len();
        match join {
            1 => for i in 0..n {                                       // BEVEL: corners cut off
                seg(buf, w, h, pts[i], pts[(i + 1) % n], weight, col);
                if weight > 1.5 { bevel(buf, w, h, pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n], weight, col); }
            },
            2 => for i in 0..n { capsule(buf, w, h, pts[i], pts[(i + 1) % n], weight, col); },   // ROUND
            _ => outline_miter(buf, w, h, pts, weight, col),
        }
    }

    /// The triangle that closes the gap on the outside of a corner at p.
    fn bevel(buf: &mut Vec<u32>, w: usize, h: usize,
             prev: (f32, f32), p: (f32, f32), next: (f32, f32), weight: f32, col: Color) {
        let unit = |a: (f32, f32), b: (f32, f32)| -> Option<(f32, f32)> {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len.is_finite() && len > 1.0e-6 { Some((dx / len, dy / len)) } else { None }
        };
        let (Some(d0), Some(d1)) = (unit(prev, p), unit(p, next)) else { return };
        let r = weight.min(4096.0) * 0.5;
        let cross = d0.0 * d1.1 - d0.1 * d1.0;
        let side = if cross > 0.0 { -1.0 } else { 1.0 };   // the outside of the turn
        let a = (p.0 - d0.1 * r * side, p.1 + d0.0 * r * side);
        let b = (p.0 - d1.1 * r * side, p.1 + d1.0 * r * side);
        fill_poly(buf, w, h, &[p, a, b], col);
    }

    /// Outline with pointed (mitered) corners. A corner too sharp for a
    /// sensible point is cut off instead, as Processing does.
    fn outline_miter(buf: &mut Vec<u32>, w: usize, h: usize, pts: &[(f32, f32)], weight: f32, col: Color) {
        stroke_path(buf, w, h, pts, true, weight, col);
    }

    /// Fills the outside of the corner at p out to the point where the two
    /// edges of the stroke meet.
    fn miter(buf: &mut Vec<u32>, w: usize, h: usize,
             prev: (f32, f32), p: (f32, f32), next: (f32, f32), weight: f32, col: Color) {
        let unit = |a: (f32, f32), b: (f32, f32)| -> Option<(f32, f32)> {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len.is_finite() && len > 1.0e-6 { Some((dx / len, dy / len)) } else { None }
        };
        let (Some(d0), Some(d1)) = (unit(prev, p), unit(p, next)) else { return };
        let r = weight.min(4096.0) * 0.5;
        let cross = d0.0 * d1.1 - d0.1 * d1.0;
        if cross.abs() < 1.0e-4 { return; }                // straight on: nothing to fill
        let side = if cross > 0.0 { -1.0 } else { 1.0 };   // the outside of the turn
        let n0 = (-d0.1 * side, d0.0 * side);
        let n1 = (-d1.1 * side, d1.0 * side);
        let a = (p.0 + n0.0 * r, p.1 + n0.1 * r);
        let b = (p.0 + n1.0 * r, p.1 + n1.1 * r);
        let k = 1.0 + n0.0 * n1.0 + n0.1 * n1.1;
        // the point would be more than 4 stroke widths long: cut it off
        if k < 0.03 { return fill_poly(buf, w, h, &[p, a, b], col); }
        let tip = (p.0 + (n0.0 + n1.0) * r / k, p.1 + (n0.1 + n1.1) * r / k);
        fill_poly(buf, w, h, &[p, a, tip, b], col);
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
        end_all_graphics();
        MATRIX_STACK.with(|s| { let mut st = s.borrow_mut(); st.clear(); st.push(identity()); });
        STYLE_STACK.with(|s| {
            let mut st = s.borrow_mut();
            if st.len() > 1 { let top = st.pop().unwrap(); st.clear(); st.push(top); }
        });
    }
    /// End of setup(), draw() or an event: a layer left open by a missing
    /// endDraw() must not stay the drawing target.
    pub fn end_draw()   { end_all_graphics(); }
    pub fn present()    {}

    #[allow(dead_code)]
    pub fn background_gray(v: f32) {
        let col = Color::from_gray(v);
        let p = solid(col);
        let mut buf = pixel_buf().lock();
        buf.fill(p);
    }
    pub fn background_rgb(r: f32, g: f32, b: f32) {
        let col = Color::from_rgb(r, g, b);
        let p = solid(col);
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
    pub fn background_color(c: Color) { background_rgba(c.r, c.g, c.b, c.a); }

    pub fn set_fill_none()                           { with_style_mut(|s| s.fill = None); }
    pub fn set_fill_color(c: Color)                  { with_style_mut(|s| s.fill = Some(c)); }

    pub fn set_stroke_none()                          { with_style_mut(|s| s.stroke = None); }
    pub fn set_stroke_color(c: Color)                 { with_style_mut(|s| s.stroke = Some(c)); }
    /// 0 ROUND, 1 SQUARE, 2 PROJECT
    pub fn set_stroke_cap(cap: u8)   { with_style_mut(|s| s.cap  = if cap  <= 2 { cap }  else { 0 }); }
    /// 0 MITER, 1 BEVEL, 2 ROUND
    pub fn set_stroke_join(join: u8) { with_style_mut(|s| s.join = if join <= 2 { join } else { 0 }); }
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
        if let Some(sc) = stroke { stroke_path(&mut buf, cw, ch, &pts, true, sw, sc); }
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
        draw_rect_radii(x, y, w, h, [r, r, r, r])
    }
    /// Radii in Processing's order: top-left, top-right, bottom-right, bottom-left.
    pub fn draw_rect_radii(x: f32, y: f32, w: f32, h: f32, radii: [f32; 4]) {
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) { return; }
        // a negative size means the rect extends the other way
        let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
        let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
        // each radius has to fit inside the rectangle
        let fit = |r: f32| if r.is_finite() && r > 0.0 { r.min(w * 0.5).min(h * 0.5) } else { 0.0 };
        let [tl, tr, br, bl] = radii.map(fit);
        if !(tl >= 1.0 || tr >= 1.0 || br >= 1.0 || bl >= 1.0) { return draw_rect(x, y, w, h); }

        let (cw, ch) = canvas_dims();
        let (fill, stroke, sw) = with_style(|s| (s.fill, s.stroke, s.stroke_weight));
        use std::f32::consts::{PI, FRAC_PI_2};
        // one quarter circle per corner (or the corner itself), joined into a single outline
        let mut pts = Vec::new();
        for (r, ccx, ccy, a0, corner) in [
            (tl, x + tl,     y + tl,     PI,              (x, y)),
            (tr, x + w - tr, y + tr,     PI + FRAC_PI_2,  (x + w, y)),
            (br, x + w - br, y + h - br, 0.0,             (x + w, y + h)),
            (bl, x + bl,     y + h - bl, FRAC_PI_2,       (x, y + h)),
        ] {
            if r >= 1.0 { pts.extend(arc_points(ccx, ccy, r, r, a0, a0 + FRAC_PI_2, true)); }
            else { pts.push(tfm(corner.0, corner.1)); }
        }
        let mut buf = pixel_buf().lock();
        if let Some(fc) = fill { fill_poly(&mut buf, cw, ch, &pts, fc); }
        if let Some(sc) = stroke { stroke_path(&mut buf, cw, ch, &pts, true, sw, sc); }
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
            let cap = with_style(|s| s.cap);
            let mut buf = pixel_buf().lock();
            capped(&mut buf, cw, ch, tfm(x1, y1), tfm(x2, y2), sw, c, cap);
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
        draw_arc_mode(cx, cy, w, h, start, stop, 0)
    }
    /// mode: 0 default (filled as a pie, only the curve stroked), 1 OPEN,
    /// 2 CHORD, 3 PIE, as in Processing.
    pub fn draw_arc_mode(cx: f32, cy: f32, w: f32, h: f32, start: f32, stop: f32, mode: u8) {
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

        let pie_shape = mode == 0 || mode == 3;
        if let Some(fc) = fill {
            if pie_shape {
                let mut pie = Vec::with_capacity(arc.len() + 1);
                pie.push(centre);
                pie.extend_from_slice(&arc);
                fill_poly(&mut buf, cw, ch, &pie, fc);
            } else {
                // OPEN and CHORD: the area between the curve and the straight
                // line joining its ends
                fill_poly(&mut buf, cw, ch, &arc, fc);
            }
        }

        if let Some(sc) = stroke {
            stroke_path(&mut buf, cw, ch, &arc, false, sw, sc);
            if arc.len() >= 2 {
                match mode {
                    2 => capsule(&mut buf, cw, ch, arc[arc.len() - 1], arc[0], sw, sc),
                    3 => {
                        capsule(&mut buf, cw, ch, centre, arc[0], sw, sc);
                        capsule(&mut buf, cw, ch, centre, arc[arc.len() - 1], sw, sc);
                    }
                    _ => {}
                }
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
    /// Multiplies the current matrix by [a c e; b d f].
    pub fn apply_matrix(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) {
        if ![a, b, c, d, e, f].iter().all(|v| v.is_finite()) { return; }
        MATRIX_STACK.with(|s| {
            let mut st = s.borrow_mut();
            let top = st.last_mut().unwrap();
            *top = mat_mul(top, &[a, b, c, d, e, f]);
        });
    }
    pub fn shear_x(angle: f32) { apply_matrix(1.0, 0.0, angle.tan(), 1.0, 0.0, 0.0); }
    pub fn shear_y(angle: f32) { apply_matrix(1.0, angle.tan(), 0.0, 1.0, 0.0, 0.0); }
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

    // ── TrueType / OpenType fonts ─────────────────────────────────────────
    //
    // Font 0 is the built-in pixel font. createFont() loads a .ttf or .otf
    // (or finds an installed font by name) and textFont() switches to it.

    struct LoadedFont { font: std::sync::Arc<fontdue::Font>, size: f32 }
    fn fonts() -> &'static parking_lot::Mutex<Vec<LoadedFont>> {
        static F: std::sync::OnceLock<parking_lot::Mutex<Vec<LoadedFont>>> = std::sync::OnceLock::new();
        F.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
    }
    thread_local! {
        static CUR_FONT: std::cell::Cell<usize> = std::cell::Cell::new(0);
        static LEADING: std::cell::Cell<Option<f32>> = std::cell::Cell::new(None);
        // rasterized glyphs: (font, character, size in quarter pixels)
        static GLYPHS: std::cell::RefCell<std::collections::HashMap<(usize, char, u32), (fontdue::Metrics, Vec<u8>)>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    const MAX_FONTS: usize = 64;
    const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;

    /// The file for a font given by path or by name, if there is one.
    fn find_font(name: &str) -> Option<std::path::PathBuf> {
        let direct = std::path::Path::new(name);
        if direct.is_file() { return Some(direct.to_path_buf()); }
        let in_data = std::path::Path::new("data").join(name);
        if in_data.is_file() { return Some(in_data); }
        // Not a file: ask the system for an installed font with that name
        // (fontconfig: Linux, and macOS when it is installed).
        if name.contains('/') || name.contains('\\') { return None; }
        let out = std::process::Command::new("fc-match")
            .arg("--format=%{file}").arg(name)
            .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .output().ok()?;
        let path = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if path.is_file() && (ext == "ttf" || ext == "otf" || ext == "ttc") { Some(path) } else { None }
    }

    /// Loads a font. Returns its id (1 or more), or 0 if it couldn't be loaded;
    /// text then stays in the built-in font.
    pub fn load_font(name: &str, size: f32) -> u64 {
        let load = || -> Result<LoadedFont, String> {
            let path = find_font(name).ok_or("no such file, and no installed font with that name")?;
            let len = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
            if len > MAX_FONT_BYTES { return Err("font file is too large".into()); }
            let data = std::fs::read(&path).map_err(|e| e.to_string())?;
            let font = fontdue::Font::from_bytes(data, fontdue::FontSettings::default())
                .map_err(|e| format!("{} ({})", e, path.display()))?;
            Ok(LoadedFont { font: std::sync::Arc::new(font), size: if size.is_finite() && size > 0.0 { size } else { 0.0 } })
        };
        let mut all = fonts().lock();
        if all.len() >= MAX_FONTS {
            eprintln!("createFont(\"{}\"): too many fonts (load them once in setup(), not every frame)", name);
            return 0;
        }
        match load() {
            Ok(f)  => { all.push(f); all.len() as u64 }
            Err(e) => { eprintln!("createFont(\"{}\"): {} (.ttf and .otf are supported)", name, e); 0 }
        }
    }
    /// textFont(font): 0 is the built-in pixel font. A font made with a size
    /// brings that size along; `size` (when above 0) overrides it.
    pub fn set_text_font(id: u64, size: f32) {
        let own = fonts().lock().get((id as usize).wrapping_sub(1)).map(|f| f.size);
        match own {
            Some(font_size) => {
                CUR_FONT.with(|c| c.set(id as usize));
                if size > 0.0 { set_text_size(size) } else if font_size > 0.0 { set_text_size(font_size) }
            }
            None => {
                CUR_FONT.with(|c| c.set(0));
                if size > 0.0 { set_text_size(size) }
            }
        }
    }
    /// textLeading(pixels): distance between lines; 0 or less goes back to the font's own.
    pub fn set_text_leading(px: f32) { LEADING.with(|l| l.set(if px > 0.0 && px.is_finite() { Some(px.min(8192.0)) } else { None })); }

    fn text_px() -> f32 { TEXT_SIZE.with(|ts| *ts.borrow()) }
    /// Runs `f` with the current TrueType font, or returns None for the built-in one.
    fn with_font<R>(f: impl FnOnce(&fontdue::Font) -> R) -> Option<R> {
        let cur = CUR_FONT.with(|c| c.get());
        if cur == 0 { return None; }
        // hand out a reference of our own and let go of the list first: `f`
        // may well ask for the font again (drawing text measures it)
        let font = fonts().lock().get(cur - 1).map(|lf| lf.font.clone())?;
        Some(f(&font))
    }

    /// One line of text in the current TrueType font, top-left corner at (lx, ly)
    /// in sketch coordinates. Follows translate and scale, like images.
    fn draw_text_line_ttf(font: &fontdue::Font, line: &str, lx: f32, ly: f32, fc: Color) {
        if turned() { return draw_text_line_ttf_turned(font, line, lx, ly, fc); }
        let (tx, ty) = tfm(lx, ly);
        if !(tx.is_finite() && ty.is_finite()) { return; }
        let scale = matrix_scale().clamp(0.01, 64.0);
        let px = (text_px() * scale).clamp(1.0, 2048.0);
        let ascent = font.horizontal_line_metrics(px).map(|m| m.ascent).unwrap_or(px * 0.8);
        let baseline = ty.clamp(-FAR, FAR) + ascent;
        let font_id = CUR_FONT.with(|c| c.get());
        let key_px = (px * 4.0).round() as u32;
        let (cw, ch) = canvas_dims();
        let mut buf = pixel_buf().lock();
        let mut pen = tx.clamp(-FAR, FAR);
        for c in line.chars() {
            if pen >= cw as f32 { break; }      // nothing more of this line can land on the canvas
            let advance = GLYPHS.with(|g| {
                let mut g = g.borrow_mut();
                if g.len() > 4096 { g.clear(); }
                let (m, bitmap) = g.entry((font_id, c, key_px))
                    .or_insert_with(|| font.rasterize(c, key_px as f32 / 4.0));
                let gx = (pen + m.xmin as f32).round() as i32;
                let gy = (baseline - m.height as f32 - m.ymin as f32).round() as i32;
                if gx < cw as i32 && gx + m.width as i32 > 0 && gy < ch as i32 && gy + m.height as i32 > 0 {
                    for row in 0..m.height {
                        let y = gy + row as i32;
                        if y < 0 || y >= ch as i32 { continue; }
                        for col in 0..m.width {
                            let cov = bitmap[row * m.width + col];
                            if cov == 0 { continue; }
                            put(&mut buf, cw, ch, gx + col as i32, y,
                                Color::from_rgba(fc.r, fc.g, fc.b, fc.a * cov as f32 / 255.0));
                        }
                    }
                }
                m.advance_width
            });
            pen += advance;
        }
    }

    /// The same line under rotate() or shear: every glyph is rasterized at the
    /// size it will have on screen, then placed through the transform like a
    /// small image.
    fn draw_text_line_ttf_turned(font: &fontdue::Font, line: &str, lx: f32, ly: f32, fc: Color) {
        let scale = matrix_scale().clamp(0.01, 64.0);
        let size = text_px();
        let px = (size * scale).clamp(1.0, 2048.0);
        let k = size / px;                                  // glyph pixels -> sketch units
        let ascent = font.horizontal_line_metrics(size).map(|m| m.ascent).unwrap_or(size * 0.8);
        let baseline = ly + ascent;
        let font_id = CUR_FONT.with(|c| c.get());
        let key_px = (px * 4.0).round() as u32;
        let saved_tint = TINT.with(|t| t.replace(None));    // tint is for images, not letters
        let mut pen = lx;
        for c in line.chars() {
            let glyph = GLYPHS.with(|g| {
                let mut g = g.borrow_mut();
                if g.len() > 4096 { g.clear(); }
                g.entry((font_id, c, key_px)).or_insert_with(|| font.rasterize(c, key_px as f32 / 4.0)).clone()
            });
            let (m, bitmap) = glyph;
            if m.width > 0 && m.height > 0 {
                let gx = pen + m.xmin as f32 * k;
                let gy = baseline - (m.height as f32 + m.ymin as f32) * k;
                blit(m.width, m.height,
                     &|i| Color::from_rgba(fc.r, fc.g, fc.b, fc.a * bitmap[i] as f32 / 255.0),
                     gx, gy, m.width as f32 * k, m.height as f32 * k);
            }
            pen += m.advance_width * k;
        }
        TINT.with(|t| t.set(saved_tint));
    }

    pub fn draw_text(s: &str, x: f32, y: f32) {
        let fc = match with_style(|s| s.fill) { Some(c) => c, None => return };
        let drawn = with_font(|font| {
            for (n, line) in s.split('\n').enumerate() {
                let (lx, ly) = text_origin(line, x, y + n as f32 * text_leading());
                draw_text_line_ttf(font, line, lx, ly, fc);
            }
        });
        if drawn.is_some() { return; }
        // Each font pixel maps to `cell` output pixels.  Round to the nearest
        // integer so pixels are square and there are no spacing gaps.
        let cell = text_cell() as i32;
        let cell_f = cell as f32;
        let (cw, canvas_h) = canvas_dims();
        if turned() {
            // under rotate() or shear the cells are no longer rows of pixels:
            // draw each run of lit cells as a four-cornered shape
            let mut buf = pixel_buf().lock();
            for (n, line) in s.split('\n').enumerate() {
                let (lx, ly) = text_origin(line, x, y + n as f32 * text_leading());
                for (i, c) in line.chars().enumerate() {
                    let code = c as u32;
                    if !(32..=127).contains(&code) { continue; }
                    let glyph = &FONT5X8[(code - 32) as usize];
                    let gx = lx + i as f32 * 6.0 * cell_f;
                    for col in 0..5usize {
                        let bits = glyph[col];
                        let mut row = 0usize;
                        while row < 8 {
                            if bits & (1 << row) == 0 { row += 1; continue; }
                            let start = row;
                            while row < 8 && bits & (1 << row) != 0 { row += 1; }
                            let (x0, x1) = (gx + col as f32 * cell_f, gx + (col + 1) as f32 * cell_f);
                            let (y0, y1) = (ly + start as f32 * cell_f, ly + row as f32 * cell_f);
                            fill_poly(&mut buf, cw, canvas_h, &[tfm(x0, y0), tfm(x1, y0), tfm(x1, y1), tfm(x0, y1)], fc);
                        }
                    }
                }
            }
            return;
        }
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
        let px = text_px();
        let ttf = with_font(|font| {
            s.split('\n').map(|l| l.chars().map(|c| font.metrics(c, px).advance_width).sum::<f32>())
                .fold(0.0f32, f32::max)
        });
        if let Some(w) = ttf { return w; }
        let cell_f = text_cell();
        s.split('\n').map(|l| l.chars().count()).max().unwrap_or(0) as f32 * 6.0 * cell_f
    }

    pub fn set_text_size(size: f32) {
        // NaN, zero and negatives would make every glyph loop misbehave
        let size = if size.is_finite() { size.clamp(1.0, 2048.0) } else { 12.0 };
        TEXT_SIZE.with(|s| *s.borrow_mut() = size);
    }

    pub fn set_smooth(on: bool) { SMOOTH.store(on, Ordering::Relaxed); }

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
    pub fn text_align_v() -> u8 { TEXT_ALIGN.with(|a| a.get().1) }
    fn text_cell() -> f32 {
        ((TEXT_SIZE.with(|ts| *ts.borrow()) / 8.0).max(1.0).round()) as f32
    }
    /// Distance from one line of text to the next.
    fn text_leading() -> f32 {
        if let Some(l) = LEADING.with(|l| l.get()) { return l; }
        let px = text_px();
        with_font(|f| f.horizontal_line_metrics(px).map(|m| m.new_line_size).unwrap_or(px * 1.2))
            .unwrap_or_else(|| 10.0 * text_cell())
    }
    /// Built-in font: glyphs are 7 rows above the baseline and 1 below.
    pub fn text_ascent() -> f32 {
        let px = text_px();
        with_font(|f| f.horizontal_line_metrics(px).map(|m| m.ascent).unwrap_or(px * 0.8))
            .unwrap_or_else(|| 7.0 * text_cell())
    }
    pub fn text_descent() -> f32 {
        let px = text_px();
        with_font(|f| f.horizontal_line_metrics(px).map(|m| -m.descent).unwrap_or(px * 0.2))
            .unwrap_or_else(|| 1.0 * text_cell())
    }
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

    /// Breaks text into lines no wider than `w` at the current font and size.
    /// Words longer than a line are split between letters; "\n" always breaks.
    pub fn wrap_text(s: &str, w: f32) -> Vec<String> {
        let mut out = Vec::new();
        for para in s.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if text_width(&candidate) <= w || (line.is_empty() && word.is_empty()) {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() { out.push(std::mem::take(&mut line)); }
                // the word alone may still be too wide: split it by letters
                let mut piece = String::new();
                for c in word.chars() {
                    piece.push(c);
                    if text_width(&piece) > w && piece.chars().count() > 1 {
                        piece.pop();
                        out.push(std::mem::take(&mut piece));
                        piece.push(c);
                    }
                }
                line = piece;
            }
            out.push(line);
        }
        out
    }
    /// text(s, x, y, w, h): wrapped inside the box. textAlign picks left,
    /// center or right inside it, and top, center or bottom for the block.
    /// Lines that don't fit in the height are left out, like Processing.
    pub fn draw_text_box(s: &str, x: f32, y: f32, w: f32, h: f32) {
        if !(w > 0.0 && h > 0.0) { return; }
        let lines = wrap_text(s, w);
        let lead = text_leading();
        let line_h = text_ascent() + text_descent();
        // how many lines fit
        let mut fit = 0usize;
        while fit < lines.len() && (fit as f32) * lead + line_h <= h + 0.5 { fit += 1; }
        if fit == 0 { return; }
        let block = (fit - 1) as f32 * lead + line_h;
        let (ha, va) = TEXT_ALIGN.with(|a| a.get());
        let top = match va { 1 => y + (h - block) / 2.0, 2 => y + h - block, _ => y };
        let ax = match ha { 1 => x + w / 2.0, 2 => x + w, _ => x };
        TEXT_ALIGN.with(|a| a.set((ha, 0)));
        for (i, line) in lines.iter().take(fit).enumerate() {
            draw_text(line, ax, top + i as f32 * lead);
        }
        TEXT_ALIGN.with(|a| a.set((ha, va)));
    }

    /// The whole canvas, row by row, as 0xAARRGGBB (alpha always 255).
    pub fn copy_pixels() -> Vec<u32> {
        pixel_buf().lock().iter().map(|p| 0xFF00_0000 | (p & 0x00FF_FFFF)).collect()
    }
    /// Writes pixels back, as many as fit. Alpha is ignored: the canvas is opaque.
    pub fn write_pixels(px: &[u32]) {
        let mut buf = pixel_buf().lock();
        let keep = if alpha_target() { 0xFFFF_FFFF } else { 0x00FF_FFFF };
        for (d, s) in buf.iter_mut().zip(px.iter()) { *d = s & keep; }
    }

    /// filter(): changes every pixel of what is being drawn on.
    /// 11 BLUR (param: radius in pixels, default 1), 12 GRAY, 13 INVERT,
    /// 14 OPAQUE, 15 POSTERIZE (param: levels 2..255), 16 THRESHOLD
    /// (param: 0..1, default 0.5), 17 ERODE, 18 DILATE. Processing's numbers.
    pub fn filter(kind: u32, param: f32) {
        let (cw, ch) = canvas_dims();
        let mut buf = pixel_buf().lock();
        if cw == 0 || ch == 0 || buf.len() < cw * ch { return; }
        let luma = |p: u32| -> u32 {
            (77 * ((p >> 16) & 0xFF) + 151 * ((p >> 8) & 0xFF) + 28 * (p & 0xFF)) >> 8
        };
        match kind {
            12 => for p in buf.iter_mut() { let l = luma(*p); *p = (*p & 0xFF00_0000) | (l << 16) | (l << 8) | l; },
            13 => for p in buf.iter_mut() { *p = (*p & 0xFF00_0000) | (!*p & 0x00FF_FFFF); },
            14 => for p in buf.iter_mut() { *p |= 0xFF00_0000; },
            15 => {
                let levels = if param.is_finite() { param.round().clamp(2.0, 255.0) as u32 } else { 4 };
                let step = |v: u32| -> u32 { ((v * levels) >> 8) * 255 / (levels - 1) };
                for p in buf.iter_mut() {
                    *p = (*p & 0xFF00_0000) | (step((*p >> 16) & 0xFF) << 16) | (step((*p >> 8) & 0xFF) << 8) | step(*p & 0xFF);
                }
            }
            16 => {
                let level = if param.is_finite() { param.clamp(0.0, 1.0) } else { 0.5 };
                let cut = (level * 255.0) as u32;
                for p in buf.iter_mut() {
                    // the brightest channel decides, as in Processing
                    let m = ((*p >> 16) & 0xFF).max((*p >> 8) & 0xFF).max(*p & 0xFF);
                    *p = (*p & 0xFF00_0000) | if m < cut { 0 } else { 0x00FF_FFFF };
                }
            }
            17 | 18 => {
                // each pixel takes its darkest (ERODE) or brightest (DILATE) neighbour
                let src = buf.clone();
                let dilate = kind == 18;
                for y in 0..ch {
                    for x in 0..cw {
                        let mut best = src[y * cw + x];
                        let mut best_l = luma(best);
                        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                            if nx < 0 || ny < 0 || nx >= cw as i32 || ny >= ch as i32 { continue; }
                            let q = src[ny as usize * cw + nx as usize];
                            let l = luma(q);
                            if (dilate && l > best_l) || (!dilate && l < best_l) { best = q; best_l = l; }
                        }
                        buf[y * cw + x] = best;
                    }
                }
            }
            11 => {
                let r = if param.is_finite() && param > 0.0 { param.min(256.0) } else { 1.0 };
                // three passes of a box blur come very close to a Gaussian
                let radius = ((r * 0.6).round() as usize).max(1);
                let mut tmp = vec![0u32; buf.len()];
                for _ in 0..3 {
                    box_blur(&buf, &mut tmp, cw, ch, radius, true);
                    box_blur(&tmp, &mut buf, cw, ch, radius, false);
                }
            }
            _ => {}
        }
    }

    /// One pass of a box blur along rows (or columns), all four channels,
    /// edges repeated.
    fn box_blur(src: &[u32], dst: &mut [u32], w: usize, h: usize, radius: usize, rows: bool) {
        let (lines, len) = if rows { (h, w) } else { (w, h) };
        let at = |line: usize, i: usize| -> usize { if rows { line * w + i } else { i * w + line } };
        let window = (2 * radius + 1) as u32;
        for line in 0..lines {
            let mut sum = [0u32; 4];
            let add = |sum: &mut [u32; 4], p: u32| { for c in 0..4 { sum[c] += (p >> (c * 8)) & 0xFF; } };
            let sub = |sum: &mut [u32; 4], p: u32| { for c in 0..4 { sum[c] -= (p >> (c * 8)) & 0xFF; } };
            for k in 0..=2 * radius {
                let i = (k as isize - radius as isize).clamp(0, len as isize - 1) as usize;
                add(&mut sum, src[at(line, i)]);
            }
            for i in 0..len {
                dst[at(line, i)] = (0..4).fold(0u32, |p, c| p | (((sum[c] + window / 2) / window).min(255) << (c * 8)));
                let out = (i as isize - radius as isize).clamp(0, len as isize - 1) as usize;
                let inn = (i as isize + radius as isize + 1).clamp(0, len as isize - 1) as usize;
                sub(&mut sum, src[at(line, out)]);
                add(&mut sum, src[at(line, inn)]);
            }
        }
    }

    /// set(x, y, color): one pixel, no transform, no blending.
    pub fn set_pixel(x: i32, y: i32, col: Color) {
        let (cw, ch) = canvas_dims();
        if x < 0 || y < 0 || x >= cw as i32 || y >= ch as i32 { return; }
        let mut buf = pixel_buf().lock();
        if let Some(p) = buf.get_mut(y as usize * cw + x as usize) { *p = solid(col); }
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
            let data = std::fs::read(path).map_err(|e| e.to_string())?;
            // by what the file is, not what it is called
            if data.starts_with(&[0xFF, 0xD8]) { decode_jpeg(&data) }
            else if data.starts_with(&[0x89, b'P', b'N', b'G']) { decode_png(&data) }
            else { Err("not a PNG or JPEG file".into()) }
        };
        match decode() {
            Ok(img) => { let mut v = images().lock(); v.push(img); v.len() as u64 }
            Err(e)  => { eprintln!("loadImage(\"{}\"): {} (PNG and JPEG are supported)", path, e); 0 }
        }
    }

    const MAX_IMAGE_PIXELS: usize = 64 * 1024 * 1024;

    fn decode_png(data: &[u8]) -> Result<Img, String> {
        let mut dec = png::Decoder::new(std::io::Cursor::new(data));
        dec.set_transformations(png::Transformations::normalize_to_color8());
        let mut rd = dec.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; rd.output_buffer_size()];
        let info = rd.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let (w, h) = (info.width as usize, info.height as usize);
        let ch = info.color_type.samples();
        if w * h > MAX_IMAGE_PIXELS { return Err("image is too large".into()); }
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
    }

    fn decode_jpeg(data: &[u8]) -> Result<Img, String> {
        use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut dec = zune_jpeg::JpegDecoder::new_with_options(data, options);
        dec.decode_headers().map_err(|e| e.to_string())?;
        let info = dec.info().ok_or("no image information")?;
        let (w, h) = (info.width as usize, info.height as usize);
        if w == 0 || h == 0 { return Err("empty image".into()); }
        if w * h > MAX_IMAGE_PIXELS { return Err("image is too large".into()); }
        let rgb = dec.decode().map_err(|e| e.to_string())?;
        if rgb.len() < w * h * 3 { return Err("image data is cut short".into()); }
        let px = rgb[..w * h * 3].chunks(3)
            .map(|p| Color::from_rgb(p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0))
            .collect();
        Ok(Img { w, h, px })
    }

    pub fn image_size(id: u64) -> (f32, f32) {
        let v = images().lock();
        v.get((id as usize).wrapping_sub(1)).map(|i| (i.w as f32, i.h as f32)).unwrap_or((0.0, 0.0))
    }
    fn push_image(img: Img) -> u64 { let mut v = images().lock(); v.push(img); v.len() as u64 }
    fn with_img<R>(id: u64, f: impl FnOnce(&mut Img) -> R) -> Option<R> {
        let mut v = images().lock();
        v.get_mut((id as usize).wrapping_sub(1)).map(f)
    }
    /// A blank, fully transparent image.
    pub fn create_image(w: usize, h: usize) -> u64 {
        if w == 0 || h == 0 || w * h > MAX_IMAGE_PIXELS { eprintln!("createImage: {w} x {h} is not a usable size"); return 0; }
        push_image(Img { w, h, px: vec![Color::from_rgba(0.0, 0.0, 0.0, 0.0); w * h] })
    }
    pub fn image_get(id: u64, x: i64, y: i64) -> Color {
        with_img(id, |i| if x >= 0 && y >= 0 && (x as usize) < i.w && (y as usize) < i.h { i.px[y as usize * i.w + x as usize] }
                        else { Color::from_rgba(0.0, 0.0, 0.0, 0.0) })
            .unwrap_or(Color::from_rgba(0.0, 0.0, 0.0, 0.0))
    }
    pub fn image_set(id: u64, x: i64, y: i64, c: Color) {
        with_img(id, |i| if x >= 0 && y >= 0 && (x as usize) < i.w && (y as usize) < i.h { i.px[y as usize * i.w + x as usize] = c; });
    }
    pub fn image_pixels(id: u64) -> Vec<Color> { with_img(id, |i| i.px.clone()).unwrap_or_default() }
    pub fn image_set_pixels(id: u64, px: &[Color]) {
        with_img(id, |i| for (d, s) in i.px.iter_mut().zip(px) { *d = *s; });
    }
    /// A new image cut out of another one (parts outside are transparent).
    pub fn image_crop(id: u64, x: i64, y: i64, w: usize, h: usize) -> u64 {
        if w == 0 || h == 0 || w * h > MAX_IMAGE_PIXELS { return 0; }
        let px = with_img(id, |i| {
            let mut out = Vec::with_capacity(w * h);
            for yy in 0..h as i64 { for xx in 0..w as i64 {
                let (sx, sy) = (x + xx, y + yy);
                out.push(if sx >= 0 && sy >= 0 && (sx as usize) < i.w && (sy as usize) < i.h { i.px[sy as usize * i.w + sx as usize] }
                         else { Color::from_rgba(0.0, 0.0, 0.0, 0.0) });
            } }
            out
        });
        px.map(|px| push_image(Img { w, h, px })).unwrap_or(0)
    }
    pub fn image_copy(id: u64) -> u64 {
        let img = with_img(id, |i| Img { w: i.w, h: i.h, px: i.px.clone() });
        img.map(push_image).unwrap_or(0)
    }
    /// Resizes in place with bilinear sampling. A 0 keeps the aspect ratio.
    pub fn image_resize(id: u64, w: usize, h: usize) {
        with_img(id, |i| {
            let (w, h) = match (w, h) {
                (0, 0) => return,
                (0, h) => (((i.w * h) as f32 / i.h as f32).round().max(1.0) as usize, h),
                (w, 0) => (w, ((i.h * w) as f32 / i.w as f32).round().max(1.0) as usize),
                wh => wh,
            };
            if w * h > MAX_IMAGE_PIXELS { return; }
            let mut out = Vec::with_capacity(w * h);
            let at = |x: usize, y: usize| i.px[y.min(i.h - 1) * i.w + x.min(i.w - 1)];
            for y in 0..h { for x in 0..w {
                let fx = ((x as f32 + 0.5) * i.w as f32 / w as f32 - 0.5).max(0.0);
                let fy = ((y as f32 + 0.5) * i.h as f32 / h as f32 - 0.5).max(0.0);
                let (x0, y0) = (fx as usize, fy as usize);
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let l = |a: Color, b: Color, t: f32| super::core::lerp_color(a, b, t);
                out.push(l(l(at(x0, y0), at(x0 + 1, y0), tx), l(at(x0, y0 + 1), at(x0 + 1, y0 + 1), tx), ty));
            } }
            i.w = w; i.h = h; i.px = out;
        });
    }
    /// mask(): the mask's brightness becomes this image's alpha (sizes must match).
    pub fn image_mask(id: u64, mask: u64) -> bool {
        let m = match with_img(mask, |m| (m.w, m.h, m.px.clone())) { Some(m) => m, None => return false };
        with_img(id, |i| {
            if (i.w, i.h) != (m.0, m.1) { eprintln!("mask: the mask must be the same size as the image"); return false; }
            for (p, q) in i.px.iter_mut().zip(m.2.iter()) { p.a *= (q.r + q.g + q.b) / 3.0 * q.a; }
            true
        }).unwrap_or(false)
    }
    /// get(x, y, w, h): a copy of part of the canvas (or the layer being drawn) as a new image.
    pub fn canvas_region(x: i64, y: i64, w: usize, h: usize) -> u64 {
        if w == 0 || h == 0 || w * h > MAX_IMAGE_PIXELS { return 0; }
        let (cw, ch) = canvas_dims();
        let buf = pixel_buf().lock();
        let layer = alpha_target();
        let mut px = Vec::with_capacity(w * h);
        for yy in 0..h as i64 { for xx in 0..w as i64 {
            let (sx, sy) = (x + xx, y + yy);
            px.push(if sx >= 0 && sy >= 0 && (sx as usize) < cw && (sy as usize) < ch {
                let p = buf.get(sy as usize * cw + sx as usize).copied().unwrap_or(0);
                let a = if layer { ((p >> 24) & 0xFF) as f32 / 255.0 } else { 1.0 };
                Color::from_rgba(((p >> 16) & 0xFF) as f32 / 255.0, ((p >> 8) & 0xFF) as f32 / 255.0, (p & 0xFF) as f32 / 255.0, a)
            } else { Color::from_rgba(0.0, 0.0, 0.0, 0.0) });
        } }
        drop(buf);
        push_image(Img { w, h, px })
    }
    /// Saves as PNG (with alpha).
    pub fn save_image(id: u64, path: &str) -> bool {
        let Some((w, h, rgba)) = with_img(id, |i| (i.w, i.h, i.px.iter().flat_map(|c| {
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            [b(c.r), b(c.g), b(c.b), b(c.a)]
        }).collect::<Vec<u8>>())) else { eprintln!("saveImage: no such image"); return false };
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            let p = std::path::Path::new(path);
            if let Some(d) = p.parent() { if !d.as_os_str().is_empty() { std::fs::create_dir_all(d)?; } }
            let f = std::io::BufWriter::new(std::fs::File::create(p)?);
            let mut enc = png::Encoder::new(f, w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()?.write_image_data(&rgba)?;
            Ok(())
        };
        match write() { Ok(_) => true, Err(e) => { eprintln!("saveImage(\"{path}\"): {e}"); false } }
    }
    /// Draw an image scaled into (x, y, w, h), through the current transform.
    pub fn draw_image(id: u64, x: f32, y: f32, w: f32, h: f32) {
        let imgs = images().lock();
        let img = match imgs.get((id as usize).wrapping_sub(1)) { Some(i) => i, None => return };
        blit(img.w, img.h, &|i| img.px[i], x, y, w, h);
    }

    /// Is the current transform more than moving and scaling?
    fn turned() -> bool {
        let m = current_matrix();
        m[1].abs() > 1.0e-6 || m[2].abs() > 1.0e-6
    }

    /// Paints a source picture of iw x ih pixels into (x, y, w, h), with the
    /// current tint. `sample(i)` is source pixel i, row by row. Follows the
    /// whole transform, rotation and shear included: every canvas pixel in the
    /// picture's outline is traced back to the source pixel it shows. Scaled
    /// or turned pictures are smoothed (blended between neighbouring source
    /// pixels) unless noSmooth() is on; one drawn at its own size and upright
    /// is copied pixel for pixel.
    fn blit(iw: usize, ih: usize, sample: &dyn Fn(usize) -> Color, x: f32, y: f32, w: f32, h: f32) {
        if iw == 0 || ih == 0 { return; }
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) || w == 0.0 || h == 0.0 { return; }
        let m = current_matrix();
        let det = m[0] * m[3] - m[2] * m[1];
        if !det.is_finite() || det.abs() < 1.0e-12 { return; }      // squashed flat: nothing to see
        let corners = [tfm(x, y), tfm(x + w, y), tfm(x + w, y + h), tfm(x, y + h)];
        if corners.iter().any(|c| !(c.0.is_finite() && c.1.is_finite())) { return; }
        let (mut lx, mut hx, mut ly, mut hy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for c in &corners { lx = lx.min(c.0); hx = hx.max(c.0); ly = ly.min(c.1); hy = hy.max(c.1); }
        if hx - lx < 1.0 || hy - ly < 1.0 { return; }
        let tint = TINT.with(|t| t.get());
        let (cw, ch) = canvas_dims();
        let upright = !turned();
        let same_size = upright && ((hx - lx) - iw as f32).abs() < 0.01 && ((hy - ly) - ih as f32).abs() < 0.01;
        let smooth = smooth_on() && !same_size;
        // from a canvas point back to sketch coordinates
        let (ia, ib, ic, id) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
        // A turned picture gets soft edges: how much of each border pixel it
        // covers, from how far inside the outline the pixel's centre is.
        let side = |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let (wide, tall) = (side(corners[0], corners[1]), side(corners[0], corners[3]));
        let soft = smooth && !upright && wide > 1.0 && tall > 1.0;
        let (eu, ev) = if soft { (0.5 / wide, 0.5 / tall) } else { (0.0, 0.0) };
        let mut buf = pixel_buf().lock();
        // only the part of the picture that is on the canvas
        let (py0, py1) = (ly.floor().max(0.0) as i32, hy.ceil().min(ch as f32) as i32);
        let (px0, px1) = (lx.floor().max(0.0) as i32, hx.ceil().min(cw as f32) as i32);
        for py in py0..py1 {
            for px in px0..px1 {
                let (dx, dy) = (px as f32 + 0.5 - m[4], py as f32 + 0.5 - m[5]);
                let u = ((ia * dx + ic * dy) - x) / w;
                let v = ((ib * dx + id * dy) - y) / h;
                if !(u >= -eu && u < 1.0 + eu && v >= -ev && v < 1.0 + ev) { continue; }
                let cover = if soft {
                    (u * wide).min((1.0 - u) * wide).min(v * tall).min((1.0 - v) * tall) + 0.5
                } else { 1.0 };
                if cover <= 0.0 { continue; }
                let (u, v) = (u.clamp(0.0, 0.999_999), v.clamp(0.0, 0.999_999));
                let mut c = if smooth {
                    // between the four nearest source pixels
                    let (fx, fy) = ((u * iw as f32 - 0.5).max(0.0), (v * ih as f32 - 0.5).max(0.0));
                    let (x0, y0) = ((fx as usize).min(iw - 1), (fy as usize).min(ih - 1));
                    let (x1, y1) = ((x0 + 1).min(iw - 1), (y0 + 1).min(ih - 1));
                    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                    let (c00, c10, c01, c11) = (sample(y0 * iw + x0), sample(y0 * iw + x1), sample(y1 * iw + x0), sample(y1 * iw + x1));
                    // weigh colors by their alpha so transparent neighbours don't darken edges
                    let wsum = |f: fn(&Color) -> f32| -> f32 {
                        (f(&c00) * (1.0 - tx) + f(&c10) * tx) * (1.0 - ty) + (f(&c01) * (1.0 - tx) + f(&c11) * tx) * ty
                    };
                    let a = wsum(|c| c.a);
                    if a <= 0.0 { continue; }
                    Color::from_rgba(wsum(|c| c.r * c.a) / a, wsum(|c| c.g * c.a) / a, wsum(|c| c.b * c.a) / a, a)
                } else {
                    let sx = ((u * iw as f32) as usize).min(iw - 1);
                    let sy = ((v * ih as f32) as usize).min(ih - 1);
                    sample(sy * iw + sx)
                };
                if let Some(t) = tint { c = Color::from_rgba(c.r * t.r, c.g * t.g, c.b * t.b, c.a * t.a); }
                if cover < 1.0 { c.a *= cover; }
                if c.a > 0.0 { put(&mut buf, cw, ch, px, py, c); }
            }
        }
    }

    // ── loadShape(): SVG ──────────────────────────────────────────────────
    //
    // The everyday part of SVG: paths (all commands, arcs included), rect,
    // circle, ellipse, line, polyline, polygon, groups with transforms, and
    // fill / stroke / stroke-width / opacity given as attributes or in
    // style="...". Text, gradients, clipping, <use> and CSS classes are not
    // read; a gradient or pattern fill shows as a plain gray.
    //
    // Everything is flattened to straight pieces in the file's own
    // coordinates when it is loaded, so drawing a shape is as cheap as
    // drawing its polygons.

    struct SvgItem {
        contours: Vec<(Vec<(f32, f32)>, bool)>,   // points, closed?
        fill: Option<Color>,
        stroke: Option<Color>,
        weight: f32,
    }
    struct SvgShape { x: f32, y: f32, w: f32, h: f32, items: Vec<SvgItem>, own_style: bool }
    fn shapes() -> &'static parking_lot::Mutex<Vec<SvgShape>> {
        static S: std::sync::OnceLock<parking_lot::Mutex<Vec<SvgShape>>> = std::sync::OnceLock::new();
        S.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
    }
    const MAX_SHAPES: usize = 256;
    const MAX_SVG_BYTES: u64 = 32 * 1024 * 1024;
    const MAX_SVG_POINTS: usize = 2_000_000;

    #[derive(Clone)]
    struct SvgStyle { fill: Option<Color>, stroke: Option<Color>, weight: f32, fill_a: f32, stroke_a: f32, opacity: f32 }

    /// Loads an SVG file. Returns its id (1 or more), or 0 if it can't be read.
    pub fn load_shape(path: &str) -> u64 {
        let load = || -> Result<SvgShape, String> {
            let file = std::path::Path::new(path);
            let file = if file.is_file() { file.to_path_buf() } else { std::path::Path::new("data").join(path) };
            let len = std::fs::metadata(&file).map_err(|e| e.to_string())?.len();
            if len > MAX_SVG_BYTES { return Err("file is too large".into()); }
            let text = String::from_utf8_lossy(&std::fs::read(&file).map_err(|e| e.to_string())?).into_owned();
            parse_svg(&text)
        };
        let mut all = shapes().lock();
        if all.len() >= MAX_SHAPES {
            eprintln!("loadShape(\"{}\"): too many shapes (load them once in setup(), not every frame)", path);
            return 0;
        }
        match load() {
            Ok(s)  => { all.push(s); all.len() as u64 }
            Err(e) => { eprintln!("loadShape(\"{}\"): {} (SVG files are supported)", path, e); 0 }
        }
    }
    pub fn shape_size(id: u64) -> (f32, f32) {
        shapes().lock().get((id as usize).wrapping_sub(1)).map(|s| (s.w, s.h)).unwrap_or((0.0, 0.0))
    }
    /// disableStyle() / enableStyle(): draw with the sketch's fill and stroke
    /// instead of the colors in the file.
    pub fn shape_own_style(id: u64, on: bool) {
        if let Some(s) = shapes().lock().get_mut((id as usize).wrapping_sub(1)) { s.own_style = on; }
    }

    /// Draws a shape into (x, y, w, h), through the current transform.
    pub fn draw_shape(id: u64, x: f32, y: f32, w: f32, h: f32) {
        let all = shapes().lock();
        let Some(shape) = all.get((id as usize).wrapping_sub(1)) else { return };
        if !(shape.w > 0.0 && shape.h > 0.0) || !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) { return; }
        let (sx, sy) = (w / shape.w, h / shape.h);
        let place = |p: &(f32, f32)| tfm(x + (p.0 - shape.x) * sx, y + (p.1 - shape.y) * sy);
        let grow = matrix_scale() * (sx.abs() * sy.abs()).sqrt();
        let (cw, ch) = canvas_dims();
        let (cur_fill, cur_stroke, cur_weight) = with_style(|s| (s.fill, s.stroke, s.stroke_weight));
        let mut buf = pixel_buf().lock();
        for item in &shape.items {
            let (fill, stroke, weight) = if shape.own_style {
                (item.fill, item.stroke, (item.weight * grow).max(0.0))
            } else {
                (cur_fill, cur_stroke, cur_weight)
            };
            let screen: Vec<(Vec<(f32, f32)>, bool)> =
                item.contours.iter().map(|(pts, closed)| (pts.iter().map(place).collect(), *closed)).collect();
            if let Some(fc) = fill {
                // all contours as one polygon: each is walked and left again
                // through its first point, and those crossings cancel, so
                // holes stay holes and nothing is painted twice
                let mut poly: Vec<(f32, f32)> = Vec::new();
                let mut anchor: Option<(f32, f32)> = None;
                for (pts, _) in &screen {
                    if pts.len() < 3 { continue; }
                    poly.extend_from_slice(pts);
                    poly.push(pts[0]);
                    match anchor { None => anchor = Some(pts[0]), Some(a) => poly.push(a) }
                }
                if poly.len() >= 3 { fill_poly(&mut buf, cw, ch, &poly, fc); }
            }
            if let Some(sc) = stroke {
                if weight > 0.0 {
                    for (pts, closed) in &screen { stroke_path(&mut buf, cw, ch, pts, *closed, weight, sc); }
                }
            }
        }
    }

    // ── SVG reading ───────────────────────────────────────────────────────

    fn parse_svg(text: &str) -> Result<SvgShape, String> {
        let mut shape = SvgShape { x: 0.0, y: 0.0, w: 0.0, h: 0.0, items: Vec::new(), own_style: true };
        let base = SvgStyle { fill: Some(Color::from_gray(0.0)), stroke: None, weight: 1.0, fill_a: 1.0, stroke_a: 1.0, opacity: 1.0 };
        // one entry per open element: its transform and style, and whether its contents are drawn
        let mut stack: Vec<(Mat, SvgStyle, bool)> = vec![(identity(), base, true)];
        let mut seen_svg = false;
        let mut points = 0usize;
        let bytes = text.as_bytes();
        let mut i = 0usize;
        while let Some(off) = text[i..].find('<') {
            let start = i + off;
            let rest = &text[start..];
            if rest.starts_with("<!--") { i = start + rest.find("-->").map(|e| e + 3).unwrap_or(rest.len()); continue; }
            if rest.starts_with("<![CDATA[") { i = start + rest.find("]]>").map(|e| e + 3).unwrap_or(rest.len()); continue; }
            if rest.starts_with("<?") || rest.starts_with("<!") { i = start + rest.find('>').map(|e| e + 1).unwrap_or(rest.len()); continue; }
            // the end of the tag, not counting a '>' inside a quoted value
            let mut end = start + 1;
            let mut quote = 0u8;
            while end < bytes.len() {
                let b = bytes[end];
                if quote != 0 { if b == quote { quote = 0; } }
                else if b == b'"' || b == b'\'' { quote = b; }
                else if b == b'>' { break; }
                end += 1;
            }
            if end >= bytes.len() { break; }
            let tag = &text[start + 1..end];
            i = end + 1;
            if let Some(name) = tag.strip_prefix('/') {
                let _ = name;
                if stack.len() > 1 { stack.pop(); }
                continue;
            }
            let self_closing = tag.ends_with('/');
            let tag = tag.trim_end_matches('/');
            let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
            let name = tag[..name_end].rsplit(':').next().unwrap_or("");
            let attrs = svg_attrs(&tag[name_end..]);
            let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
            let num = |k: &str| get(k).and_then(svg_length).unwrap_or(0.0);

            let (parent_m, parent_style, parent_drawn) = stack.last().cloned().unwrap();
            let mut m = parent_m;
            if let Some(t) = get("transform") { m = mat_mul(&m, &svg_transform(t)); }
            let mut style = parent_style.clone();
            svg_apply_style(&mut style, &attrs);
            // things that are defined, not drawn
            let drawn = parent_drawn && !matches!(name, "defs" | "clipPath" | "mask" | "symbol" | "pattern" | "marker"
                | "linearGradient" | "radialGradient" | "metadata" | "title" | "desc" | "style" | "script")
                && get("display") != Some("none");

            if name == "svg" && !seen_svg {
                seen_svg = true;
                let vb: Vec<f32> = get("viewBox").map(svg_numbers).unwrap_or_default();
                if vb.len() == 4 && vb[2] > 0.0 && vb[3] > 0.0 {
                    shape.x = vb[0]; shape.y = vb[1]; shape.w = vb[2]; shape.h = vb[3];
                } else {
                    shape.w = num("width"); shape.h = num("height");
                }
            }

            let mut contours: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
            if drawn {
                match name {
                    "path" => if let Some(d) = get("d") { contours = svg_path(d); },
                    "rect" => {
                        let (x, y, w, h) = (num("x"), num("y"), num("width"), num("height"));
                        if w > 0.0 && h > 0.0 {
                            let rx = get("rx").and_then(svg_length);
                            let ry = get("ry").and_then(svg_length);
                            let (rx, ry) = (rx.or(ry).unwrap_or(0.0).min(w / 2.0), ry.or(rx).unwrap_or(0.0).min(h / 2.0));
                            if rx > 0.0 && ry > 0.0 {
                                let mut pts = Vec::new();
                                let pi = std::f32::consts::PI;
                                for (cx, cy, a0) in [(x + w - rx, y + ry, -pi / 2.0), (x + w - rx, y + h - ry, 0.0),
                                                     (x + rx, y + h - ry, pi / 2.0), (x + rx, y + ry, pi)] {
                                    for k in 0..=8 {
                                        let a = a0 + pi / 2.0 * k as f32 / 8.0;
                                        pts.push((cx + a.cos() * rx, cy + a.sin() * ry));
                                    }
                                }
                                contours.push((pts, true));
                            } else {
                                contours.push((vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)], true));
                            }
                        }
                    }
                    "circle" | "ellipse" => {
                        let (cx, cy) = (num("cx"), num("cy"));
                        let (rx, ry) = if name == "circle" { (num("r"), num("r")) } else { (num("rx"), num("ry")) };
                        if rx > 0.0 && ry > 0.0 {
                            let n = 64;
                            let pts = (0..n).map(|k| {
                                let a = std::f32::consts::TAU * k as f32 / n as f32;
                                (cx + a.cos() * rx, cy + a.sin() * ry)
                            }).collect();
                            contours.push((pts, true));
                        }
                    }
                    "line" => contours.push((vec![(num("x1"), num("y1")), (num("x2"), num("y2"))], false)),
                    "polyline" | "polygon" => {
                        let v = get("points").map(svg_numbers).unwrap_or_default();
                        let pts: Vec<(f32, f32)> = v.chunks_exact(2).map(|c| (c[0], c[1])).collect();
                        if pts.len() >= 2 { contours.push((pts, name == "polygon")); }
                    }
                    _ => {}
                }
            }
            if !contours.is_empty() {
                for (pts, _) in contours.iter_mut() {
                    for p in pts.iter_mut() { *p = (m[0] * p.0 + m[2] * p.1 + m[4], m[1] * p.0 + m[3] * p.1 + m[5]); }
                    pts.retain(|p| p.0.is_finite() && p.1.is_finite());
                    points += pts.len();
                }
                if points > MAX_SVG_POINTS { return Err("the drawing is too detailed".into()); }
                let det = (m[0] * m[3] - m[2] * m[1]).abs().sqrt();
                let with_alpha = |c: Option<Color>, a: f32| c.map(|c| Color::from_rgba(c.r, c.g, c.b, (a * style.opacity).clamp(0.0, 1.0)));
                // a line has nothing to fill
                let fill = if name == "line" { None } else { with_alpha(style.fill, style.fill_a) };
                shape.items.push(SvgItem {
                    contours, fill,
                    stroke: with_alpha(style.stroke, style.stroke_a),
                    weight: style.weight * if det.is_finite() { det } else { 1.0 },
                });
            }
            if !self_closing { stack.push((m, style, drawn)); }
            if stack.len() > 256 { return Err("nested too deeply".into()); }
        }
        if !seen_svg { return Err("not an SVG file".into()); }
        if !(shape.w > 0.0 && shape.h > 0.0) {
            // no size given: use what the drawing covers
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for item in &shape.items { for (pts, _) in &item.contours { for p in pts {
                x0 = x0.min(p.0); y0 = y0.min(p.1); x1 = x1.max(p.0); y1 = y1.max(p.1);
            } } }
            if x1 > x0 && y1 > y0 { shape.x = x0; shape.y = y0; shape.w = x1 - x0; shape.h = y1 - y0; }
            else { return Err("nothing to draw in it".into()); }
        }
        Ok(shape)
    }

    /// name="value" pairs of a tag. Values of style="a: b; c: d" are added as
    /// pairs of their own, after the attributes, so they win.
    fn svg_attrs(s: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            while i < b.len() && (b[i] as char).is_whitespace() { i += 1; }
            let n0 = i;
            while i < b.len() && b[i] != b'=' && !(b[i] as char).is_whitespace() { i += 1; }
            let name = &s[n0..i];
            while i < b.len() && (b[i] as char).is_whitespace() { i += 1; }
            if i >= b.len() || b[i] != b'=' { if name.is_empty() { i += 1; } continue; }
            i += 1;
            while i < b.len() && (b[i] as char).is_whitespace() { i += 1; }
            if i >= b.len() { break; }
            let q = b[i];
            if q != b'"' && q != b'\'' { continue; }
            let v0 = i + 1;
            let Some(len) = s[v0..].find(q as char) else { break };
            let value = &s[v0..v0 + len];
            i = v0 + len + 1;
            let name = name.rsplit(':').next().unwrap_or(name);
            if !name.is_empty() { out.push((name.to_string(), value.trim().to_string())); }
        }
        let style: Vec<(String, String)> = out.iter().filter(|(n, _)| n == "style").flat_map(|(_, v)| {
            v.split(';').filter_map(|d| {
                let (k, val) = d.split_once(':')?;
                Some((k.trim().to_string(), val.trim().to_string()))
            }).collect::<Vec<_>>()
        }).collect();
        out.extend(style);
        out
    }

    fn svg_apply_style(style: &mut SvgStyle, attrs: &[(String, String)]) {
        for (k, v) in attrs {
            match k.as_str() {
                "fill" => if let Some(c) = svg_color(v) { style.fill = c; },
                "stroke" => if let Some(c) = svg_color(v) { style.stroke = c; },
                "stroke-width" => if let Some(w) = svg_length(v) { if w >= 0.0 { style.weight = w; } },
                "fill-opacity" => if let Some(a) = svg_fraction(v) { style.fill_a = a; },
                "stroke-opacity" => if let Some(a) = svg_fraction(v) { style.stroke_a = a; },
                // opacity applies to the element and everything in it
                "opacity" => if let Some(a) = svg_fraction(v) { style.opacity *= a; },
                _ => {}
            }
        }
    }

    fn svg_fraction(v: &str) -> Option<f32> {
        let v = v.trim();
        let (n, scale) = match v.strip_suffix('%') { Some(n) => (n, 0.01), None => (v, 1.0) };
        n.trim().parse::<f32>().ok().filter(|a| a.is_finite()).map(|a| (a * scale).clamp(0.0, 1.0))
    }

    /// "12", "12px", "1.5em": the number, whatever the unit. Percentages are not lengths we can use.
    fn svg_length(v: &str) -> Option<f32> {
        let v = v.trim();
        if v.ends_with('%') { return None; }
        let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E'))
            .unwrap_or(v.len());
        // "1em": the e belongs to the unit
        let mut num = &v[..end];
        while !num.is_empty() && num.parse::<f32>().is_err() { num = &num[..num.len() - 1]; }
        num.parse::<f32>().ok().filter(|n| n.is_finite())
    }

    /// Some(None) is "none"; None means "not a color we know", so the inherited one stays.
    fn svg_color(v: &str) -> Option<Option<Color>> {
        let v = v.trim().to_ascii_lowercase();
        let rgb = |r: u32, g: u32, b: u32| Some(Some(Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)));
        if v == "none" || v == "transparent" { return Some(None); }
        if v.starts_with("url(") { return rgb(128, 128, 128); }     // gradient or pattern: a plain gray
        if let Some(hex) = v.strip_prefix('#') {
            let n = u32::from_str_radix(hex, 16).ok()?;
            return match hex.len() {
                3 => rgb(((n >> 8) & 0xF) * 17, ((n >> 4) & 0xF) * 17, (n & 0xF) * 17),
                6 => rgb((n >> 16) & 0xFF, (n >> 8) & 0xFF, n & 0xFF),
                8 => rgb((n >> 24) & 0xFF, (n >> 16) & 0xFF, (n >> 8) & 0xFF),
                _ => None,
            };
        }
        if let Some(inner) = v.strip_prefix("rgb(").or_else(|| v.strip_prefix("rgba(")) {
            let parts: Vec<&str> = inner.trim_end_matches(')').split(|c| c == ',' || c == ' ' || c == '/').filter(|p| !p.is_empty()).collect();
            if parts.len() < 3 { return None; }
            let ch = |p: &str| -> Option<u32> {
                match p.strip_suffix('%') {
                    Some(n) => n.trim().parse::<f32>().ok().map(|x| (x * 2.55).clamp(0.0, 255.0) as u32),
                    None => p.trim().parse::<f32>().ok().map(|x| x.clamp(0.0, 255.0) as u32),
                }
            };
            return rgb(ch(parts[0])?, ch(parts[1])?, ch(parts[2])?);
        }
        let (r, g, b) = match v.as_str() {
            "black" => (0, 0, 0), "white" => (255, 255, 255), "red" => (255, 0, 0), "green" => (0, 128, 0),
            "lime" => (0, 255, 0), "blue" => (0, 0, 255), "yellow" => (255, 255, 0), "cyan" | "aqua" => (0, 255, 255),
            "magenta" | "fuchsia" => (255, 0, 255), "gray" | "grey" => (128, 128, 128), "silver" => (192, 192, 192),
            "orange" => (255, 165, 0), "purple" => (128, 0, 128), "brown" => (165, 42, 42), "pink" => (255, 192, 203),
            "navy" => (0, 0, 128), "teal" => (0, 128, 128), "maroon" => (128, 0, 0), "olive" => (128, 128, 0),
            "gold" => (255, 215, 0), "indigo" => (75, 0, 130), "violet" => (238, 130, 238), "tomato" => (255, 99, 71),
            "darkgray" | "darkgrey" => (169, 169, 169), "lightgray" | "lightgrey" => (211, 211, 211),
            "currentcolor" => (0, 0, 0),
            _ => return None,
        };
        rgb(r, g, b)
    }

    /// Every number in a string, however they are separated ("10,20 30-5.5.5e1").
    fn svg_numbers(s: &str) -> Vec<f32> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
                let start = i;
                if c == b'-' || c == b'+' { i += 1; }
                let mut dot = false;
                while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !dot)) { if b[i] == b'.' { dot = true; } i += 1; }
                if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                    let mut j = i + 1;
                    if j < b.len() && (b[j] == b'-' || b[j] == b'+') { j += 1; }
                    if j < b.len() && b[j].is_ascii_digit() { i = j; while i < b.len() && b[i].is_ascii_digit() { i += 1; } }
                }
                if let Ok(n) = s[start..i].parse::<f32>() { if n.is_finite() { out.push(n); } }
                if i == start { i += 1; }
            } else { i += 1; }
        }
        out
    }

    fn svg_transform(s: &str) -> Mat {
        let mut m = identity();
        let mut rest = s;
        while let Some(open) = rest.find('(') {
            let name = rest[..open].trim().trim_start_matches(',').trim();
            let Some(close) = rest[open..].find(')') else { break };
            let v = svg_numbers(&rest[open + 1..open + close]);
            let a = |i: usize| v.get(i).copied().unwrap_or(0.0);
            let t: Mat = match name {
                "translate" => [1.0, 0.0, 0.0, 1.0, a(0), a(1)],
                "scale" => [a(0), 0.0, 0.0, if v.len() > 1 { a(1) } else { a(0) }, 0.0, 0.0],
                "rotate" => {
                    let (sin, cos) = a(0).to_radians().sin_cos();
                    let r: Mat = [cos, sin, -sin, cos, 0.0, 0.0];
                    if v.len() >= 3 {
                        mat_mul(&mat_mul(&[1.0, 0.0, 0.0, 1.0, a(1), a(2)], &r), &[1.0, 0.0, 0.0, 1.0, -a(1), -a(2)])
                    } else { r }
                }
                "skewX" => [1.0, 0.0, a(0).to_radians().tan(), 1.0, 0.0, 0.0],
                "skewY" => [1.0, a(0).to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
                "matrix" if v.len() >= 6 => [a(0), a(1), a(2), a(3), a(4), a(5)],
                _ => identity(),
            };
            m = mat_mul(&m, &t);
            rest = &rest[open + close + 1..];
        }
        if m.iter().all(|x| x.is_finite()) { m } else { identity() }
    }

    /// The d="..." of a path as straight-line contours.
    fn svg_path(d: &str) -> Vec<(Vec<(f32, f32)>, bool)> {
        // split into commands and their numbers; an arc's two flags may be
        // written without anything between them ("a5 5 0 01 10 10")
        let b = d.as_bytes();
        let mut out: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
        let mut cur: Vec<(f32, f32)> = Vec::new();
        let (mut x, mut y, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let (mut ctrl_x, mut ctrl_y) = (0.0f32, 0.0f32);     // last control point, for S and T
        let mut last = b' ';
        let mut i = 0usize;
        let mut total = 0usize;
        let flush = |cur: &mut Vec<(f32, f32)>, out: &mut Vec<(Vec<(f32, f32)>, bool)>, closed: bool| {
            if cur.len() >= 2 { out.push((std::mem::take(cur), closed)); } else { cur.clear(); }
        };
        while i < b.len() {
            let c = b[i];
            if !c.is_ascii_alphabetic() { i += 1; continue; }
            let cmd = c;
            let mut j = i + 1;
            while j < b.len() && !(b[j].is_ascii_alphabetic() && b[j] != b'e' && b[j] != b'E') { j += 1; }
            let args_text = &d[i + 1..j];
            i = j;
            let rel = cmd.is_ascii_lowercase();
            let upper = cmd.to_ascii_uppercase();
            let args: Vec<f32> = if upper == b'A' { svg_arc_numbers(args_text) } else { svg_numbers(args_text) };
            let per = match upper { b'M' | b'L' | b'T' => 2, b'H' | b'V' => 1, b'C' => 6, b'S' | b'Q' => 4, b'A' => 7, _ => 0 };
            if upper == b'Z' {
                flush(&mut cur, &mut out, true);
                x = sx; y = sy;
                last = b'Z';
                continue;
            }
            if per == 0 { continue; }
            for (k, a) in args.chunks_exact(per).enumerate() {
                total += 1;
                if total > 500_000 { return out; }
                let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
                match upper {
                    b'M' => {
                        if k == 0 {
                            flush(&mut cur, &mut out, false);
                            x = bx + a[0]; y = by + a[1]; sx = x; sy = y;
                            cur.push((x, y));
                        } else {            // more pairs after a move are lines
                            x = bx + a[0]; y = by + a[1]; cur.push((x, y));
                        }
                    }
                    b'L' => { x = bx + a[0]; y = by + a[1]; cur.push((x, y)); }
                    b'H' => { x = bx + a[0]; cur.push((x, y)); }
                    b'V' => { y = by + a[0]; cur.push((x, y)); }
                    b'C' | b'S' => {
                        let (c1x, c1y, c2x, c2y, ex, ey) = if upper == b'C' {
                            (bx + a[0], by + a[1], bx + a[2], by + a[3], bx + a[4], by + a[5])
                        } else {
                            // first control point mirrors the previous curve's second
                            let (rx, ry) = if matches!(last, b'C' | b'S') { (2.0 * x - ctrl_x, 2.0 * y - ctrl_y) } else { (x, y) };
                            (rx, ry, bx + a[0], by + a[1], bx + a[2], by + a[3])
                        };
                        let n = 16;
                        for s in 1..=n {
                            let t = s as f32 / n as f32;
                            let u = 1.0 - t;
                            cur.push((u * u * u * x + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * ex,
                                      u * u * u * y + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * ey));
                        }
                        ctrl_x = c2x; ctrl_y = c2y; x = ex; y = ey;
                    }
                    b'Q' | b'T' => {
                        let (cx, cy, ex, ey) = if upper == b'Q' {
                            (bx + a[0], by + a[1], bx + a[2], by + a[3])
                        } else {
                            let (rx, ry) = if matches!(last, b'Q' | b'T') { (2.0 * x - ctrl_x, 2.0 * y - ctrl_y) } else { (x, y) };
                            (rx, ry, bx + a[0], by + a[1])
                        };
                        let n = 12;
                        for s in 1..=n {
                            let t = s as f32 / n as f32;
                            let u = 1.0 - t;
                            cur.push((u * u * x + 2.0 * u * t * cx + t * t * ex, u * u * y + 2.0 * u * t * cy + t * t * ey));
                        }
                        ctrl_x = cx; ctrl_y = cy; x = ex; y = ey;
                    }
                    b'A' => {
                        let (ex, ey) = (bx + a[5], by + a[6]);
                        svg_arc(&mut cur, x, y, a[0], a[1], a[2], a[3] != 0.0, a[4] != 0.0, ex, ey);
                        x = ex; y = ey;
                    }
                    _ => {}
                }
                last = upper;
                if cur.is_empty() { cur.push((x, y)); }
            }
        }
        flush(&mut cur, &mut out, false);
        out
    }

    /// Numbers of an arc command: in each group of seven, the fourth and fifth
    /// are single-digit flags that may be run together with what follows.
    fn svg_arc_numbers(s: &str) -> Vec<f32> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let pos = out.len() % 7;
            while i < b.len() && (b[i] == b',' || (b[i] as char).is_whitespace()) { i += 1; }
            if i >= b.len() { break; }
            if pos == 3 || pos == 4 {
                if b[i] == b'0' || b[i] == b'1' { out.push((b[i] - b'0') as f32); i += 1; continue; }
                return out;     // not a flag: malformed, stop here
            }
            let start = i;
            if b[i] == b'-' || b[i] == b'+' { i += 1; }
            let mut dot = false;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !dot)) { if b[i] == b'.' { dot = true; } i += 1; }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'-' || b[j] == b'+') { j += 1; }
                if j < b.len() && b[j].is_ascii_digit() { i = j; while i < b.len() && b[i].is_ascii_digit() { i += 1; } }
            }
            match s[start..i].parse::<f32>() { Ok(n) if n.is_finite() => out.push(n), _ => return out }
            if i == start { return out; }
        }
        out
    }

    /// An elliptical arc from (x0, y0) to (x1, y1), as points (the SVG
    /// "endpoint to centre" conversion).
    fn svg_arc(out: &mut Vec<(f32, f32)>, x0: f32, y0: f32, rx: f32, ry: f32, turn_deg: f32,
               large: bool, sweep: bool, x1: f32, y1: f32) {
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx < 1.0e-6 || ry < 1.0e-6 || ((x0 - x1).abs() < 1.0e-6 && (y0 - y1).abs() < 1.0e-6) {
            out.push((x1, y1));
            return;
        }
        let (sin, cos) = turn_deg.to_radians().sin_cos();
        let (dx, dy) = ((x0 - x1) / 2.0, (y0 - y1) / 2.0);
        let (xp, yp) = (cos * dx + sin * dy, -sin * dx + cos * dy);
        let lambda = (xp * xp) / (rx * rx) + (yp * yp) / (ry * ry);
        if lambda > 1.0 { let s = lambda.sqrt(); rx *= s; ry *= s; }      // radii too small: grow them to fit
        let num = (rx * rx * ry * ry - rx * rx * yp * yp - ry * ry * xp * xp).max(0.0);
        let den = rx * rx * yp * yp + ry * ry * xp * xp;
        let mut k = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
        if large == sweep { k = -k; }
        let (cxp, cyp) = (k * rx * yp / ry, -k * ry * xp / rx);
        let (cx, cy) = (cos * cxp - sin * cyp + (x0 + x1) / 2.0, sin * cxp + cos * cyp + (y0 + y1) / 2.0);
        let a0 = ((yp - cyp) / ry).atan2((xp - cxp) / rx);
        let a1 = ((-yp - cyp) / ry).atan2((-xp - cxp) / rx);
        let tau = std::f32::consts::TAU;
        let mut da = a1 - a0;
        if sweep && da < 0.0 { da += tau; }
        if !sweep && da > 0.0 { da -= tau; }
        let n = ((da.abs() / (tau / 48.0)).ceil() as usize).clamp(2, 64);
        for s in 1..=n {
            let a = a0 + da * s as f32 / n as f32;
            let (ex, ey) = (rx * a.cos(), ry * a.sin());
            out.push((cos * ex - sin * ey + cx, sin * ex + cos * ey + cy));
        }
        if let Some(lastp) = out.last_mut() { *lastp = (x1, y1); }
    }

    // ── createGraphics(): off-screen layers ───────────────────────────────
    //
    // A layer has its own pixels (with alpha) and its own style. Between
    // begin_graphics() and end_graphics() it simply takes the canvas's place:
    // the pixel buffers and the size are swapped, so every drawing function
    // works on a layer without knowing about it.

    struct Layer { w: u32, h: u32, px: Vec<u32>, style: Style, active: bool }
    fn layers() -> &'static parking_lot::Mutex<Vec<Layer>> {
        static L: std::sync::OnceLock<parking_lot::Mutex<Vec<Layer>>> = std::sync::OnceLock::new();
        L.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
    }
    /// What was the target before a layer took over.
    struct Saved { layer: usize, w: u32, h: u32, styles: Vec<Style>, matrices: Vec<Mat>, alpha: bool }
    thread_local! {
        static TARGETS: std::cell::RefCell<Vec<Saved>> = std::cell::RefCell::new(Vec::new());
    }
    const MAX_LAYER_PIXELS: usize = 64 * 1024 * 1024;   // all layers together: 256 MB

    /// A new transparent layer. Returns a 1-based id, or 0 if it can't be made.
    pub fn create_graphics(w: u32, h: u32) -> u64 {
        let (w, h) = (w.clamp(1, super::MAX_CANVAS), h.clamp(1, super::MAX_CANVAS));
        let mut all = layers().lock();
        let used: usize = all.iter().map(|l| l.px.len()).sum();
        if used + (w as usize * h as usize) > MAX_LAYER_PIXELS {
            eprintln!("createGraphics: too many layers (make them once in setup(), not every frame)");
            return 0;
        }
        all.push(Layer { w, h, px: vec![0u32; w as usize * h as usize], style: Style::default(), active: false });
        all.len() as u64
    }
    pub fn graphics_size(id: u64) -> (u32, u32) {
        layers().lock().get((id as usize).wrapping_sub(1)).map(|l| (l.w, l.h)).unwrap_or((0, 0))
    }
    /// From here on, drawing goes to the layer. Returns false for a bad id or
    /// a layer that is already being drawn to.
    pub fn begin_graphics(id: u64) -> bool {
        let idx = (id as usize).wrapping_sub(1);
        let mut all = layers().lock();
        let Some(layer) = all.get_mut(idx) else { return false };
        if layer.active { return false; }
        layer.active = true;
        let (cw, ch) = (CANVAS_W.load(Ordering::Relaxed), CANVAS_H.load(Ordering::Relaxed));
        std::mem::swap(&mut *pixel_buf().lock(), &mut layer.px);
        CANVAS_W.store(layer.w, Ordering::Relaxed);
        CANVAS_H.store(layer.h, Ordering::Relaxed);
        let styles = STYLE_STACK.with(|s| std::mem::replace(&mut *s.borrow_mut(), vec![layer.style.clone()]));
        let matrices = MATRIX_STACK.with(|s| std::mem::replace(&mut *s.borrow_mut(), vec![identity()]));
        let alpha = ALPHA_TARGET.swap(true, Ordering::Relaxed);
        TARGETS.with(|t| t.borrow_mut().push(Saved { layer: idx, w: cw, h: ch, styles, matrices, alpha }));
        true
    }
    /// Back to whatever was the target before the last begin_graphics().
    pub fn end_graphics() {
        let Some(saved) = TARGETS.with(|t| t.borrow_mut().pop()) else { return };
        let mut all = layers().lock();
        if let Some(layer) = all.get_mut(saved.layer) {
            std::mem::swap(&mut *pixel_buf().lock(), &mut layer.px);
            layer.active = false;
            // the layer remembers its fill, stroke... for its next beginDraw()
            layer.style = STYLE_STACK.with(|s| s.borrow().first().cloned().unwrap_or_default());
        }
        CANVAS_W.store(saved.w, Ordering::Relaxed);
        CANVAS_H.store(saved.h, Ordering::Relaxed);
        STYLE_STACK.with(|s| *s.borrow_mut() = saved.styles);
        MATRIX_STACK.with(|s| *s.borrow_mut() = saved.matrices);
        ALPHA_TARGET.store(saved.alpha, Ordering::Relaxed);
    }
    fn end_all_graphics() {
        while TARGETS.with(|t| !t.borrow().is_empty()) { end_graphics(); }
    }

    /// clear(): a layer becomes fully transparent again; the canvas goes black.
    pub fn clear() {
        pixel_buf().lock().fill(0);
    }

    /// Draws a layer like an image. A layer can't be drawn while it is itself
    /// the target.
    pub fn draw_graphics(id: u64, x: f32, y: f32, w: f32, h: f32) {
        let all = layers().lock();
        let Some(layer) = all.get((id as usize).wrapping_sub(1)) else { return };
        if layer.active { return; }
        let px = &layer.px;
        blit(layer.w as usize, layer.h as usize, &|i| {
            let p = px[i];
            Color::from_rgba(((p >> 16) & 0xFF) as f32 / 255.0, ((p >> 8) & 0xFF) as f32 / 255.0,
                             (p & 0xFF) as f32 / 255.0, (p >> 24) as f32 / 255.0)
        }, x, y, w, h);
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
    /// The screen the IDE's window is on, as (width, height, x, y), when the
    /// IDE started this sketch (it passes PROCESSING_DISPLAY). None otherwise.
    pub fn display() -> Option<(usize, usize, isize, isize)> {
        let v = std::env::var("PROCESSING_DISPLAY").ok()?;
        let n: Vec<i64> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if n.len() == 4 && n[0] > 0 && n[1] > 0 {
            Some((n[0] as usize, n[1] as usize, n[2] as isize, n[3] as isize))
        } else { None }
    }
    /// fullScreen(): the canvas becomes the size of the screen and the window
    /// covers it, without a border. Needs to know the screen, which only the
    /// IDE tells us; run on its own, the sketch keeps its window.
    pub fn set_fullscreen(on: bool) {
        if !on { return; }
        match display() {
            Some((w, h, x, y)) => {
                set_size(w as u32, h as u32);
                set_present(w, h, x, y);
            }
            None => eprintln!("fullScreen() only works when the sketch is started from Processing; using a window instead"),
        }
    }
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
                    millis, focused: window.is_active(),
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
/// Width of the screen the sketch was started on (0 when not started from the IDE).
pub fn display_width() -> f32 {
    #[cfg(not(feature = "wasm"))] { glfw::display().map(|d| d.0 as f32).unwrap_or(0.0) }
    #[cfg(feature = "wasm")] { 0.0 }
}
pub fn display_height() -> f32 {
    #[cfg(not(feature = "wasm"))] { glfw::display().map(|d| d.1 as f32).unwrap_or(0.0) }
    #[cfg(feature = "wasm")] { 0.0 }
}
#[inline] pub fn fullScreen() { full_screen() }

pub fn frame_rate(fps: f32) {
    #[cfg(not(feature = "wasm"))] glfw::set_frame_rate(fps);
    #[cfg(feature = "wasm")]      wasm::set_frame_rate(fps);
}
#[inline] pub fn frameRate(fps: f32) { frame_rate(fps) }

// ──────────────────────────── Background / color ─────────────────────────────

pub fn background(v: impl Paint) { render::background_color(v.paint()); }
pub fn background_rgb(r: impl Num, g: impl Num, b: impl Num)            { let r = r.f(); let g = g.f(); let b = b.f(); render::background_color(cm(r, g, b, None)); }
pub fn background_rgba(r: impl Num, g: impl Num, b: impl Num, a: impl Num)   { let r = r.f(); let g = g.f(); let b = b.f(); let a = a.f(); render::background_color(cm(r, g, b, Some(a))); }
pub fn background_color(c: Color)                         { render::background_color(c); }

pub fn fill(v: impl Paint) { render::set_fill_color(v.paint()); }
pub fn fill_rgb(r: impl Num, g: impl Num, b: impl Num)                  { let r = r.f(); let g = g.f(); let b = b.f(); render::set_fill_color(cm(r, g, b, None)); }
pub fn fill_rgba(r: impl Num, g: impl Num, b: impl Num, a: impl Num)         { let r = r.f(); let g = g.f(); let b = b.f(); let a = a.f(); render::set_fill_color(cm(r, g, b, Some(a))); }
pub fn fill_color(c: Color)                               { render::set_fill_color(c); }
pub fn no_fill()                                          { render::set_fill_none(); }
#[inline] pub fn noFill() { no_fill() }

pub fn stroke(v: impl Paint) { render::set_stroke_color(v.paint()); }
pub fn stroke_rgb(r: impl Num, g: impl Num, b: impl Num)                { let r = r.f(); let g = g.f(); let b = b.f(); render::set_stroke_color(cm(r, g, b, None)); }
pub fn stroke_rgba(r: impl Num, g: impl Num, b: impl Num, a: impl Num)       { let r = r.f(); let g = g.f(); let b = b.f(); let a = a.f(); render::set_stroke_color(cm(r, g, b, Some(a))); }
pub fn stroke_color(c: Color)                             { render::set_stroke_color(c); }
pub fn no_stroke()                                        { render::set_stroke_none(); }
#[inline] pub fn noStroke() { no_stroke() }

pub fn stroke_weight(w: f32)                              { render::set_stroke_weight(w); }
#[inline] pub fn strokeWeight(w: f32) { stroke_weight(w) }

// ─────────────────────────── Primitive shapes ────────────────────────────────

pub fn ellipse(x: f32, y: f32, w: f32, h: f32) {
    let (x, y, w, h) = ellipse_box(x, y, w, h);
    render::draw_ellipse(x, y, w, h);
}
pub fn circle(x: f32, y: f32, d: f32)                    { ellipse(x, y, d, d); }
pub fn rect(x: f32, y: f32, w: f32, h: f32) {
    let (x, y, w, h) = corner_box(RECT_MODE.load(Ordering::Relaxed), x, y, w, h);
    render::draw_rect(x, y, w, h);
}
pub fn rect_rounded(x: f32, y: f32, w: f32, h: f32, r: f32) {
    let (x, y, w, h) = corner_box(RECT_MODE.load(Ordering::Relaxed), x, y, w, h);
    render::draw_rect_rounded(x, y, w, h, r);
}
#[inline] pub fn rectRounded(x: f32, y: f32, w: f32, h: f32, r: f32) { rect_rounded(x, y, w, h, r) }
pub fn square(x: f32, y: f32, s: f32)                    { rect(x, y, s, s); }
pub fn triangle(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) { render::draw_triangle(x1, y1, x2, y2, x3, y3); }
pub fn line(x1: f32, y1: f32, x2: f32, y2: f32)          { render::draw_line(x1, y1, x2, y2); }
pub fn point(x: f32, y: f32)                              { render::draw_point(x, y); }
pub fn quad(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32, x4: f32, y4: f32) {
    render::draw_quad(x1, y1, x2, y2, x3, y3, x4, y4);
}
pub fn arc(x: f32, y: f32, w: f32, h: f32, start: f32, stop: f32) {
    let (x, y, w, h) = ellipse_box(x, y, w, h);
    render::draw_arc(x, y, w, h, start, stop);
}
pub const OPEN:  u32 = 1;
pub const CHORD: u32 = 2;
pub const PIE:   u32 = 3;
/// arc() with a closing style: OPEN, CHORD or PIE.
pub fn arc_mode(x: f32, y: f32, w: f32, h: f32, start: f32, stop: f32, mode: u32) {
    let (x, y, w, h) = ellipse_box(x, y, w, h);
    render::draw_arc_mode(x, y, w, h, start, stop, if mode <= 3 { mode as u8 } else { 0 });
}
/// Centre and size of an ellipse from a call made in the current ellipseMode().
fn ellipse_box(a: f32, b: f32, c: f32, d: f32) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = corner_box(ELLIPSE_MODE.load(Ordering::Relaxed), a, b, c, d);
    (x + w / 2.0, y + h / 2.0, w, h)
}

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
pub fn shear_x(angle: f32) { render::shear_x(angle); }
pub fn shear_y(angle: f32) { render::shear_y(angle); }
/// Multiplies the current transform by the matrix [a c e; b d f].
pub fn apply_matrix(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) { render::apply_matrix(a, b, c, d, e, f); }
#[inline] pub fn shearX(angle: f32) { shear_x(angle) }
#[inline] pub fn shearY(angle: f32) { shear_y(angle) }

// ──────────────────────────── Style state ────────────────────────────────────

pub fn push_style() { render::push_style(); }
pub fn pop_style()  { render::pop_style(); }
#[inline] pub fn pushStyle() { push_style() }
#[inline] pub fn popStyle()  { pop_style()  }

// ─────────────────────────── Text ────────────────────────────────────────────

pub fn text(s: &str, x: f32, y: f32)  { render::draw_text(s, x, y); }
pub fn text_size(size: f32)            { render::set_text_size(size); }
pub fn text_width(s: &str) -> f32      { render::text_width(s) }
/// createFont("DejaVu Sans", 32) or createFont("data/MyFont.ttf", 32): a
/// .ttf / .otf file, or the name of an installed font. Returns 0 if it can't
/// be loaded (text then stays in the built-in font).
pub fn create_font(name: &str, size: f32) -> u64 { render::load_font(name, size) }
/// loadFont("MyFont.ttf"): same as create_font without a size.
pub fn load_font(name: &str) -> u64 { render::load_font(name, 0.0) }
/// textFont(font): use that font from now on. 0 is the built-in pixel font.
pub fn text_font(font: u64) { render::set_text_font(font, 0.0); }
/// textFont(font, size)
pub fn text_font_size(font: u64, size: f32) { render::set_text_font(font, size); }
/// textLeading(pixels): distance from one line of text to the next.
pub fn text_leading(pixels: f32) { render::set_text_leading(pixels); }
#[inline] pub fn createFont(name: &str, size: f32) -> u64 { create_font(name, size) }
#[inline] pub fn loadFont(name: &str) -> u64 { load_font(name) }
#[inline] pub fn textFont(font: u64) { text_font(font) }
#[inline] pub fn textLeading(pixels: f32) { text_leading(pixels) }
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

#[inline] pub fn color(gray: impl Num) -> Color { let gray = gray.f(); cm_gray(gray, None) }
#[inline] pub fn color_rgb(r: impl Num, g: impl Num, b: impl Num) -> Color { let r = r.f(); let g = g.f(); let b = b.f(); cm(r, g, b, None) }
#[inline] pub fn color_rgba(r: impl Num, g: impl Num, b: impl Num, a: impl Num) -> Color { let r = r.f(); let g = g.f(); let b = b.f(); let a = a.f(); cm(r, g, b, Some(a)) }
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
// Processing's own numbers, so sketches can also pass them around as ints.
pub const SQUARE:  u32 = 1;
pub const ROUND:   u32 = 2;
pub const PROJECT: u32 = 4;
pub const MITER:   u32 = 8;
pub const BEVEL:   u32 = 32;
/// Ends of lines: ROUND (default), SQUARE or PROJECT.
pub fn stroke_cap(cap: u32) {
    render::set_stroke_cap(match cap { SQUARE => 1, PROJECT => 2, _ => 0 });
}
/// Corners of shapes: MITER (default), BEVEL or ROUND.
pub fn stroke_join(join: u32) {
    render::set_stroke_join(match join { BEVEL => 1, ROUND => 2, _ => 0 });
}
#[inline] pub fn strokeCap(cap: u32)   { stroke_cap(cap) }
#[inline] pub fn strokeJoin(join: u32) { stroke_join(join) }

// ─────────────────────────── Blend modes ─────────────────────────────────────
pub const REPLACE:    u32 = 0;
pub const BLEND:      u32 = 1;
pub const ADD:        u32 = 2;
pub const SUBTRACT:   u32 = 4;
pub const LIGHTEST:   u32 = 8;
pub const DARKEST:    u32 = 16;
pub const DIFFERENCE: u32 = 32;
pub const EXCLUSION:  u32 = 64;
pub const MULTIPLY:   u32 = 128;
pub const SCREEN:     u32 = 256;
/// How new drawing combines with what is already on the canvas.
pub fn blend_mode(mode: u32) {
    render::set_blend_mode(match mode {
        ADD => 1, SUBTRACT => 2, DARKEST => 3, LIGHTEST => 4, DIFFERENCE => 5,
        EXCLUSION => 6, MULTIPLY => 7, SCREEN => 8, REPLACE => 9, _ => 0,
    });
}
#[inline] pub fn blendMode(mode: u32) { blend_mode(mode) }

// ─────────────────────────── Shape modes ─────────────────────────────────────
pub const CORNER:  u32 = 0;
pub const CORNERS: u32 = 1;
pub const RADIUS:  u32 = 2;
pub const CENTER:  u32 = 3;
static RECT_MODE:    std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(CORNER);
static ELLIPSE_MODE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(CENTER);
static IMAGE_MODE:   std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(CORNER);
/// How rect() reads its four numbers: CORNER (default), CORNERS, CENTER or RADIUS.
pub fn rect_mode(mode: u32)    { RECT_MODE.store(mode, Ordering::Relaxed); }
/// How ellipse() reads its four numbers: CENTER (default), RADIUS, CORNER or CORNERS.
pub fn ellipse_mode(mode: u32) { ELLIPSE_MODE.store(mode, Ordering::Relaxed); }
/// How image() reads its position: CORNER (default), CORNERS or CENTER.
pub fn image_mode(mode: u32)   { IMAGE_MODE.store(mode, Ordering::Relaxed); }
#[inline] pub fn rectMode(mode: u32)    { rect_mode(mode) }
#[inline] pub fn ellipseMode(mode: u32) { ellipse_mode(mode) }
#[inline] pub fn imageMode(mode: u32)   { image_mode(mode) }

/// (x, y, w, h) with (x, y) the top-left corner, from the four numbers of a
/// call made in the given mode.
fn corner_box(mode: u32, a: f32, b: f32, c: f32, d: f32) -> (f32, f32, f32, f32) {
    match mode {
        CORNERS => (a.min(c), b.min(d), (c - a).abs(), (d - b).abs()),
        RADIUS  => (a - c, b - d, c * 2.0, d * 2.0),
        CENTER  => (a - c / 2.0, b - d / 2.0, c, d),
        _       => (a, b, c, d),
    }
}

// ─────────────────────────── Date and time ───────────────────────────────────
//
// Local time when the IDE started the sketch (it passes the UTC offset in
// PROCESSING_UTC_OFFSET, in seconds); UTC for a sketch run on its own.

fn local_seconds() -> i64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    let offset = std::env::var("PROCESSING_UTC_OFFSET").ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|o| o.abs() <= 18 * 3600).unwrap_or(0);
    now + offset
}
/// (year, month 1..12, day 1..31) of a day count since 1970-01-01.
fn civil(days: i64) -> (i32, i32, i32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as i32, m as i32, d as i32)
}
pub fn year()   -> i32 { civil(local_seconds().div_euclid(86_400)).0 }
pub fn month()  -> i32 { civil(local_seconds().div_euclid(86_400)).1 }
pub fn day()    -> i32 { civil(local_seconds().div_euclid(86_400)).2 }
pub fn hour()   -> i32 { (local_seconds().rem_euclid(86_400) / 3600) as i32 }
pub fn minute() -> i32 { (local_seconds().rem_euclid(3600) / 60) as i32 }
pub fn second() -> i32 { local_seconds().rem_euclid(60) as i32 }

// ─────────────────────────── Text alignment ──────────────────────────────────

pub const ALIGN_LEFT:     u8 = 0;
pub const ALIGN_CENTER:   u8 = 1;
pub const ALIGN_RIGHT:    u8 = 2;
pub const ALIGN_TOP:      u8 = 0;
pub const ALIGN_BOTTOM:   u8 = 2;
pub const ALIGN_BASELINE: u8 = 3;

pub const TOP:      u32 = 101;
pub const BOTTOM:   u32 = 102;
pub const BASELINE: u32 = 0;

/// A text alignment: Processing's LEFT / CENTER / RIGHT / TOP / BOTTOM /
/// BASELINE, or the older ALIGN_* names.
pub trait Align: Copy { fn h(self) -> u8; fn v(self) -> u8; }
impl Align for u8 { fn h(self) -> u8 { self.min(2) } fn v(self) -> u8 { self.min(3) } }
impl Align for u32 {
    fn h(self) -> u8 { match self { CENTER => 1, RIGHT => 2, _ => 0 } }
    fn v(self) -> u8 { match self { CENTER => 1, BOTTOM => 2, BASELINE => 3, _ => 0 } }
}
impl Align for i32 { fn h(self) -> u8 { (self.max(0) as u32).h() } fn v(self) -> u8 { (self.max(0) as u32).v() } }

/// text_align(CENTER, CENTER), text_align(RIGHT, BOTTOM)...
/// h: LEFT / CENTER / RIGHT.  v: TOP / CENTER / BOTTOM / BASELINE.
pub fn text_align(h: impl Align, v: impl Align) { render::set_text_align(h.h(), v.v()); }
/// text_align(RIGHT): only the horizontal side, the vertical stays as it was.
pub fn text_align_h(h: impl Align) { render::set_text_align(h.h(), render::text_align_v()); }
pub fn text_ascent()  -> f32 { render::text_ascent() }
/// text(s, x, y, w, h): the text wrapped to fit inside a box.
pub fn text_box(s: &str, x: f32, y: f32, w: f32, h: f32) { render::draw_text_box(s, x, y, w, h) }
/// The lines text_box would draw, for measuring or drawing them yourself.
pub fn wrap_text(s: &str, w: f32) -> Vec<String> { render::wrap_text(s, w) }
pub fn text_descent() -> f32 { render::text_descent() }
#[inline] pub fn textAlign(h: impl Align, v: impl Align) { text_align(h, v) }
#[inline] pub fn textAscent() -> f32 { text_ascent() }
#[inline] pub fn textDescent() -> f32 { text_descent() }

// ─────────────────────────── Images ──────────────────────────────────────────

/// PNG only. Returns an image id (0 if loading failed).
pub fn load_image(path: &str) -> u64 { render::load_image(path) }
pub fn image(id: u64, x: f32, y: f32) {
    let (w, h) = render::image_size(id);
    // CORNERS needs both corners; with one point it is a corner
    let (x, y) = if IMAGE_MODE.load(Ordering::Relaxed) == CENTER { (x - w / 2.0, y - h / 2.0) } else { (x, y) };
    render::draw_image(id, x, y, w, h);
}
pub fn image_sized(id: u64, x: f32, y: f32, w: f32, h: f32) {
    let (x, y, w, h) = corner_box(IMAGE_MODE.load(Ordering::Relaxed), x, y, w, h);
    render::draw_image(id, x, y, w, h);
}
pub fn image_width(id: u64)  -> f32 { render::image_size(id).0 }
pub fn image_height(id: u64) -> f32 { render::image_size(id).1 }
// ─────────────────────────────────────────────────────────────────────────────
// Sound: sound files and simple oscillators
// ─────────────────────────────────────────────────────────────────────────────
//
// Each sound file and each oscillator is one "voice": a stream of samples
// that the sound card pulls from on its own thread. The sketch never touches
// the samples; it only flips the voice's controls (playing, volume, pan,
// speed, position), which are plain atomics, so nothing here can block draw().
//
// The sound card is opened the first time a sketch makes a sound. A machine
// without one still runs the sketch: loading and timing work, nothing is heard.

#[cfg(not(feature = "wasm"))]
mod audio {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, OnceLock};

    const MAX_VOICES: usize = 64;
    const MAX_SAMPLES: usize = 96_000 * 2 * 60 * 20;      // 20 minutes of stereo at 96 kHz

    /// What the sketch can change about a voice while it plays.
    struct Controls {
        playing: AtomicBool,
        looping: AtomicBool,
        amp:  AtomicU32,     // f32 bits, 0..1 (more is allowed, and may clip)
        pan:  AtomicU32,     // f32 bits, -1 left .. 1 right
        rate: AtomicU32,     // f32 bits: playback speed for a file, frequency in Hz for an oscillator
        pos:  AtomicU64,     // f64 bits: position in frames (files only)
    }
    impl Controls {
        fn new(rate: f32, amp: f32) -> Arc<Self> {
            Arc::new(Self {
                playing: AtomicBool::new(false), looping: AtomicBool::new(false),
                amp: AtomicU32::new(amp.to_bits()), pan: AtomicU32::new(0f32.to_bits()),
                rate: AtomicU32::new(rate.to_bits()), pos: AtomicU64::new(0f64.to_bits()),
            })
        }
        fn f(a: &AtomicU32) -> f32 { f32::from_bits(a.load(Ordering::Relaxed)) }
        /// Left and right gain for the current volume and pan.
        fn gains(&self) -> (f32, f32) {
            let (amp, pan) = (Self::f(&self.amp), Self::f(&self.pan).clamp(-1.0, 1.0));
            (amp * (1.0 - pan).min(1.0), amp * (1.0 + pan).min(1.0))
        }
    }

    enum Kind {
        File { data: Arc<Vec<f32>>, channels: usize, frames: usize, sample_rate: u32 },
        Osc(u8),      // 0 sine, 1 square, 2 saw, 3 triangle, 4 white noise
    }
    struct Voice { kind: Kind, ctl: Arc<Controls>, attached: AtomicBool }

    fn voices() -> &'static parking_lot::Mutex<Vec<Arc<Voice>>> {
        static V: OnceLock<parking_lot::Mutex<Vec<Arc<Voice>>>> = OnceLock::new();
        V.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
    }
    fn voice(id: u64) -> Option<Arc<Voice>> {
        voices().lock().get((id as usize).wrapping_sub(1)).cloned()
    }

    /// The sound card, opened on first use. None when there isn't one.
    fn output() -> Option<&'static rodio::OutputStreamHandle> {
        static OUT: OnceLock<Option<rodio::OutputStreamHandle>> = OnceLock::new();
        OUT.get_or_init(|| {
            // The stream has to stay alive, and can't move between threads on
            // every system: give it a thread of its own that never ends.
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("sound".into()).spawn(move || {
                match rodio::OutputStream::try_default() {
                    Ok((stream, handle)) => {
                        let _ = tx.send(Some(handle));
                        let _keep = stream;
                        loop { std::thread::park(); }
                    }
                    Err(_) => { let _ = tx.send(None); }
                }
            });
            if spawned.is_err() { return None; }
            let out = rx.recv_timeout(std::time::Duration::from_secs(5)).ok().flatten();
            if out.is_none() { eprintln!("No sound output found: the sketch runs, but silently."); }
            out
        }).as_ref()
    }

    /// The samples of one voice, as the sound card asks for them (stereo).
    struct Stream {
        voice: Arc<Voice>,
        right: Option<f32>,     // the right half of the frame just made
        phase: f32,             // oscillators: 0..1 through the wave
        noise: u32,
    }
    impl Stream {
        fn frame(&mut self) -> (f32, f32) {
            let ctl = &self.voice.ctl;
            if !ctl.playing.load(Ordering::Relaxed) { return (0.0, 0.0); }
            let (gl, gr) = ctl.gains();
            match &self.voice.kind {
                Kind::File { data, channels, frames, .. } => {
                    let pos = f64::from_bits(ctl.pos.load(Ordering::Relaxed));
                    if *frames == 0 || !(pos >= 0.0) || pos >= *frames as f64 {
                        ctl.pos.store(0f64.to_bits(), Ordering::Relaxed);
                        if !ctl.looping.load(Ordering::Relaxed) || *frames == 0 {
                            ctl.playing.store(false, Ordering::Relaxed);
                            return (0.0, 0.0);
                        }
                        return self.frame();
                    }
                    // between two neighbouring frames, so any speed sounds clean
                    let i = pos as usize;
                    let t = (pos - i as f64) as f32;
                    let j = if i + 1 < *frames { i + 1 } else if ctl.looping.load(Ordering::Relaxed) { 0 } else { i };
                    let at = |frame: usize, ch: usize| data[frame * channels + ch.min(channels - 1)];
                    let l = at(i, 0) + (at(j, 0) - at(i, 0)) * t;
                    let r = at(i, 1) + (at(j, 1) - at(i, 1)) * t;
                    let speed = Controls::f(&ctl.rate) as f64;
                    let speed = if speed.is_finite() { speed.clamp(0.0, 64.0) } else { 1.0 };
                    ctl.pos.store((pos + speed).to_bits(), Ordering::Relaxed);
                    (l * gl, r * gr)
                }
                Kind::Osc(shape) => {
                    let freq = Controls::f(&ctl.rate);
                    let freq = if freq.is_finite() { freq.clamp(0.0, 20_000.0) } else { 0.0 };
                    let p = self.phase;
                    let v = match shape {
                        0 => (p * std::f32::consts::TAU).sin(),
                        1 => if p < 0.5 { 1.0 } else { -1.0 },
                        2 => 2.0 * p - 1.0,
                        3 => 1.0 - 4.0 * (p - 0.5).abs(),
                        _ => {
                            self.noise ^= self.noise << 13; self.noise ^= self.noise >> 17; self.noise ^= self.noise << 5;
                            self.noise as f32 / u32::MAX as f32 * 2.0 - 1.0
                        }
                    };
                    self.phase = (p + freq / 44_100.0).fract();
                    (v * gl, v * gr)
                }
            }
        }
    }
    impl Iterator for Stream {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            if let Some(r) = self.right.take() { return Some(r); }
            let (l, r) = self.frame();
            self.right = Some(r);
            Some(l)
        }
    }
    impl rodio::Source for Stream {
        fn current_frame_len(&self) -> Option<usize> { None }
        fn channels(&self) -> u16 { 2 }
        fn sample_rate(&self) -> u32 {
            match &self.voice.kind { Kind::File { sample_rate, .. } => *sample_rate, Kind::Osc(_) => 44_100 }
        }
        fn total_duration(&self) -> Option<std::time::Duration> { None }
    }

    /// Connects a voice to the sound card the first time it is needed.
    /// False when there is no sound card.
    fn attach(v: &Arc<Voice>) -> bool {
        let Some(out) = output() else { return false };
        if v.attached.swap(true, Ordering::Relaxed) { return true; }
        match rodio::Sink::try_new(out) {
            Ok(sink) => {
                sink.append(Stream { voice: v.clone(), right: None, phase: 0.0, noise: 0x9E37_79B9 });
                sink.detach();
                true
            }
            Err(_) => { v.attached.store(false, Ordering::Relaxed); false }
        }
    }

    fn add(kind: Kind, rate: f32, amp: f32, what: &str) -> u64 {
        let mut all = voices().lock();
        if all.len() >= MAX_VOICES {
            eprintln!("{}: too many sounds (make them once in setup(), not every frame)", what);
            return 0;
        }
        all.push(Arc::new(Voice { kind, ctl: Controls::new(rate, amp), attached: AtomicBool::new(false) }));
        all.len() as u64
    }

    /// Loads a sound file (WAV, OGG Vorbis, FLAC, MP3). 0 if it can't be read.
    pub fn load(path: &str) -> u64 {
        let decode = || -> Result<Kind, String> {
            use rodio::Source;
            let file = std::path::Path::new(path);
            let file = if file.is_file() { file.to_path_buf() } else { std::path::Path::new("data").join(path) };
            let f = std::fs::File::open(&file).map_err(|e| e.to_string())?;
            let dec = rodio::Decoder::new(std::io::BufReader::new(f)).map_err(|e| e.to_string())?;
            let (channels, sample_rate) = (dec.channels() as usize, dec.sample_rate());
            if channels == 0 || sample_rate == 0 { return Err("no audio in it".into()); }
            let mut data: Vec<f32> = Vec::new();
            for s in dec.convert_samples::<f32>() {
                if data.len() >= MAX_SAMPLES { return Err("the sound is too long (20 minutes at most)".into()); }
                data.push(if s.is_finite() { s } else { 0.0 });
            }
            let frames = data.len() / channels;
            data.truncate(frames * channels);
            Ok(Kind::File { data: Arc::new(data), channels, frames, sample_rate })
        };
        match decode() {
            Ok(kind) => add(kind, 1.0, 1.0, "loadSound"),
            Err(e) => { eprintln!("loadSound(\"{}\"): {} (WAV, OGG, FLAC and MP3 are supported)", path, e); 0 }
        }
    }
    /// A new oscillator: 0 sine, 1 square, 2 saw, 3 triangle, 4 white noise.
    pub fn oscillator(shape: u8) -> u64 { add(Kind::Osc(shape.min(4)), 440.0, 0.5, "oscillator") }

    pub fn play(id: u64, looping: bool) {
        let Some(v) = voice(id) else { return };
        v.ctl.looping.store(looping, Ordering::Relaxed);
        // without a sound card nothing would ever move the position along
        v.ctl.playing.store(attach(&v), Ordering::Relaxed);
    }
    pub fn pause(id: u64) { if let Some(v) = voice(id) { v.ctl.playing.store(false, Ordering::Relaxed); } }
    pub fn stop(id: u64) {
        if let Some(v) = voice(id) {
            v.ctl.playing.store(false, Ordering::Relaxed);
            v.ctl.pos.store(0f64.to_bits(), Ordering::Relaxed);
        }
    }
    pub fn is_playing(id: u64) -> bool { voice(id).map(|v| v.ctl.playing.load(Ordering::Relaxed)).unwrap_or(false) }
    pub fn set_amp(id: u64, amp: f32) {
        if let Some(v) = voice(id) { v.ctl.amp.store((if amp.is_finite() { amp.clamp(0.0, 8.0) } else { 0.0 }).to_bits(), Ordering::Relaxed); }
    }
    pub fn set_pan(id: u64, pan: f32) {
        if let Some(v) = voice(id) { v.ctl.pan.store((if pan.is_finite() { pan.clamp(-1.0, 1.0) } else { 0.0 }).to_bits(), Ordering::Relaxed); }
    }
    /// Speed for a file (1 is normal), frequency in Hz for an oscillator.
    pub fn set_rate(id: u64, rate: f32) {
        if let Some(v) = voice(id) { v.ctl.rate.store((if rate.is_finite() { rate.max(0.0) } else { 1.0 }).to_bits(), Ordering::Relaxed); }
    }
    /// Length of a sound file in seconds (0 for an oscillator).
    pub fn duration(id: u64) -> f32 {
        match voice(id).as_deref() {
            Some(Voice { kind: Kind::File { frames, sample_rate, .. }, .. }) => *frames as f32 / *sample_rate as f32,
            _ => 0.0,
        }
    }
    /// Where a sound file is, in seconds.
    pub fn position(id: u64) -> f32 {
        match voice(id).as_deref() {
            Some(Voice { kind: Kind::File { sample_rate, .. }, ctl, .. }) =>
                (f64::from_bits(ctl.pos.load(Ordering::Relaxed)) / *sample_rate as f64) as f32,
            _ => 0.0,
        }
    }
    /// The n mono samples of a playing file just before where it is now, with
    /// its volume applied (silence when it isn't playing). For analysis.
    pub fn window(id: u64, n: usize) -> Vec<f32> {
        let Some(v) = voice(id) else { return vec![0.0; n] };
        let Kind::File { data, channels, frames, .. } = &v.kind else { return vec![0.0; n] };
        if !v.ctl.playing.load(Ordering::Relaxed) || *frames == 0 { return vec![0.0; n]; }
        let pos = f64::from_bits(v.ctl.pos.load(Ordering::Relaxed)) as usize;
        let amp = Controls::f(&v.ctl.amp);
        let ch = (*channels).max(1);
        (0..n).map(|i| {
            let f = pos as i64 - n as i64 + i as i64;
            if f < 0 || f as usize >= *frames { return 0.0; }
            let base = f as usize * ch;
            data[base..base + ch].iter().sum::<f32>() / ch as f32 * amp
        }).collect()
    }
    /// Moves a sound file to a time in seconds.
    pub fn jump(id: u64, seconds: f32) {
        if let Some(v) = voice(id) {
            if let Kind::File { frames, sample_rate, .. } = &v.kind {
                let frame = if seconds.is_finite() { (seconds.max(0.0) as f64 * *sample_rate as f64).min(*frames as f64) } else { 0.0 };
                v.ctl.pos.store(frame.to_bits(), Ordering::Relaxed);
            }
        }
    }

    // The sound card can't be listened to in a test, but what is handed to
    // it can be checked sample by sample.
    #[cfg(test)]
    mod tests {
        use super::*;

        fn file_voice(data: Vec<f32>, channels: usize) -> Arc<Voice> {
            let frames = data.len() / channels;
            Arc::new(Voice {
                kind: Kind::File { data: Arc::new(data), channels, frames, sample_rate: 8000 },
                ctl: Controls::new(1.0, 1.0), attached: AtomicBool::new(false),
            })
        }
        fn stream(v: &Arc<Voice>) -> Stream { Stream { voice: v.clone(), right: None, phase: 0.0, noise: 1 } }
        fn set(a: &AtomicU32, v: f32) { a.store(v.to_bits(), Ordering::Relaxed); }

        #[test]
        fn silent_until_played_then_plays_once_and_stops() {
            let v = file_voice(vec![0.1, 0.2, 0.3, 0.4], 1);
            let mut s = stream(&v);
            assert_eq!(s.by_ref().take(4).collect::<Vec<_>>(), vec![0.0; 4], "not playing: silence");
            v.ctl.playing.store(true, Ordering::Relaxed);
            let out: Vec<f32> = s.by_ref().take(8).collect();
            assert_eq!(out, vec![0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4], "mono goes to both sides");
            assert_eq!(s.next(), Some(0.0), "after the end: silence");
            assert!(!v.ctl.playing.load(Ordering::Relaxed), "and it has stopped by itself");
            assert_eq!(f64::from_bits(v.ctl.pos.load(Ordering::Relaxed)), 0.0, "back at the start");
        }

        #[test]
        fn looping_wraps_around() {
            let v = file_voice(vec![0.1, 0.2, 0.3], 1);
            v.ctl.playing.store(true, Ordering::Relaxed);
            v.ctl.looping.store(true, Ordering::Relaxed);
            let left: Vec<f32> = stream(&v).step_by(2).take(7).collect();
            assert_eq!(left, vec![0.1, 0.2, 0.3, 0.1, 0.2, 0.3, 0.1]);
            assert!(v.ctl.playing.load(Ordering::Relaxed));
        }

        #[test]
        fn stereo_keeps_its_sides_and_pan_and_volume_scale_them() {
            let v = file_voice(vec![0.5, -0.5, 0.25, -0.25], 2);
            v.ctl.playing.store(true, Ordering::Relaxed);
            assert_eq!(stream(&v).take(4).collect::<Vec<_>>(), vec![0.5, -0.5, 0.25, -0.25]);
            v.ctl.pos.store(0f64.to_bits(), Ordering::Relaxed);
            v.ctl.playing.store(true, Ordering::Relaxed);
            set(&v.ctl.pan, -1.0);
            assert_eq!(stream(&v).take(2).collect::<Vec<_>>(), vec![0.5, 0.0], "all the way left: nothing on the right");
            v.ctl.pos.store(0f64.to_bits(), Ordering::Relaxed);
            set(&v.ctl.pan, 0.0);
            set(&v.ctl.amp, 0.5);
            assert_eq!(stream(&v).take(2).collect::<Vec<_>>(), vec![0.25, -0.25], "half volume");
        }

        #[test]
        fn rate_changes_speed_and_blends_between_samples() {
            let v = file_voice(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0], 1);
            v.ctl.playing.store(true, Ordering::Relaxed);
            set(&v.ctl.rate, 2.0);
            assert_eq!(stream(&v).step_by(2).take(4).collect::<Vec<_>>(), vec![0.0, 2.0, 4.0, 6.0], "twice as fast");
            v.ctl.pos.store(0f64.to_bits(), Ordering::Relaxed);
            v.ctl.playing.store(true, Ordering::Relaxed);
            set(&v.ctl.rate, 0.5);
            assert_eq!(stream(&v).step_by(2).take(4).collect::<Vec<_>>(), vec![0.0, 0.5, 1.0, 1.5], "half speed, in between");
        }

        #[test]
        fn nothing_breaks_on_empty_or_odd_input() {
            let v = file_voice(vec![], 1);
            v.ctl.playing.store(true, Ordering::Relaxed);
            v.ctl.looping.store(true, Ordering::Relaxed);
            assert_eq!(stream(&v).take(4).collect::<Vec<_>>(), vec![0.0; 4], "an empty file, even looping");
            let v = file_voice(vec![0.5; 8], 1);
            v.ctl.playing.store(true, Ordering::Relaxed);
            for bad in [f32::NAN, f32::INFINITY, -1.0, 1.0e12] { set(&v.ctl.rate, bad); let _ = stream(&v).take(64).count(); }
            v.ctl.pos.store(f64::NAN.to_bits(), Ordering::Relaxed);
            let _ = stream(&v).take(64).count();
        }

        #[test]
        fn oscillators_make_their_waves() {
            for (shape, first, quarter) in [(0u8, 0.0f32, 1.0f32), (1, 1.0, 1.0), (2, -1.0, -0.5), (3, -1.0, 0.0)] {
                let v = Arc::new(Voice { kind: Kind::Osc(shape), ctl: Controls::new(441.0, 1.0), attached: AtomicBool::new(false) });
                v.ctl.playing.store(true, Ordering::Relaxed);
                // 441 Hz at 44100: one wave is exactly 100 frames
                let left: Vec<f32> = stream(&v).step_by(2).take(120).collect();
                assert!((left[0] - first).abs() < 1.0e-3, "shape {shape} starts at {}", left[0]);
                assert!((left[25] - quarter).abs() < 0.03, "shape {shape} a quarter in: {}", left[25]);
                // compared away from the jump that square and saw waves have at the start
                assert!((left[110] - left[10]).abs() < 0.03, "shape {shape} repeats after one wave");
                assert!(left.iter().all(|s| s.abs() <= 1.0001));
            }
            let v = Arc::new(Voice { kind: Kind::Osc(4), ctl: Controls::new(440.0, 0.5), attached: AtomicBool::new(false) });
            v.ctl.playing.store(true, Ordering::Relaxed);
            let noise: Vec<f32> = stream(&v).take(2000).collect();
            assert!(noise.iter().all(|s| s.abs() <= 0.5001), "noise stays within its volume");
            let mean = noise.iter().sum::<f32>() / noise.len() as f32;
            assert!(mean.abs() < 0.05 && noise.iter().any(|s| *s > 0.3) && noise.iter().any(|s| *s < -0.3), "noise is spread out");
        }
    }
}

/// loadSound("beep.wav"): a WAV, OGG, FLAC or MP3 file (also looked for in data/).
/// Returns its id, or 0 if it can't be read.
pub fn load_sound(path: &str) -> u64 {
    #[cfg(not(feature = "wasm"))] { audio::load(path) }
    #[cfg(feature = "wasm")] { let _ = path; 0 }
}
/// Play from where it is (from the start after sound_stop, or the first time).
pub fn sound_play(id: u64)  { #[cfg(not(feature = "wasm"))] audio::play(id, false); #[cfg(feature = "wasm")] { let _ = id; } }
/// Play over and over.
pub fn sound_loop(id: u64)  { #[cfg(not(feature = "wasm"))] audio::play(id, true); #[cfg(feature = "wasm")] { let _ = id; } }
/// Stop and keep the place; sound_play carries on from there.
pub fn sound_pause(id: u64) { #[cfg(not(feature = "wasm"))] audio::pause(id); #[cfg(feature = "wasm")] { let _ = id; } }
/// Stop and go back to the start.
pub fn sound_stop(id: u64)  { #[cfg(not(feature = "wasm"))] audio::stop(id); #[cfg(feature = "wasm")] { let _ = id; } }
pub fn sound_is_playing(id: u64) -> bool {
    #[cfg(not(feature = "wasm"))] { audio::is_playing(id) }
    #[cfg(feature = "wasm")] { let _ = id; false }
}
/// Volume: 0 is silent, 1 is the file as it is.
pub fn sound_amp(id: u64, amp: f32)   { #[cfg(not(feature = "wasm"))] audio::set_amp(id, amp); #[cfg(feature = "wasm")] { let _ = (id, amp); } }
/// Left and right: -1 is all left, 0 the middle, 1 all right.
pub fn sound_pan(id: u64, pan: f32)   { #[cfg(not(feature = "wasm"))] audio::set_pan(id, pan); #[cfg(feature = "wasm")] { let _ = (id, pan); } }
/// Speed: 1 is normal, 2 twice as fast and an octave higher, 0.5 half as fast.
pub fn sound_rate(id: u64, rate: f32) { #[cfg(not(feature = "wasm"))] audio::set_rate(id, rate); #[cfg(feature = "wasm")] { let _ = (id, rate); } }
/// Jump to a time in seconds.
pub fn sound_jump(id: u64, seconds: f32) { #[cfg(not(feature = "wasm"))] audio::jump(id, seconds); #[cfg(feature = "wasm")] { let _ = (id, seconds); } }
/// Length in seconds.
pub fn sound_duration(id: u64) -> f32 {
    #[cfg(not(feature = "wasm"))] { audio::duration(id) }
    #[cfg(feature = "wasm")] { let _ = id; 0.0 }
}
/// Where it is now, in seconds.
pub fn sound_position(id: u64) -> f32 {
    #[cfg(not(feature = "wasm"))] { audio::position(id) }
    #[cfg(feature = "wasm")] { let _ = id; 0.0 }
}

fn new_osc(shape: u8) -> u64 {
    #[cfg(not(feature = "wasm"))] { audio::oscillator(shape) }
    #[cfg(feature = "wasm")] { let _ = shape; 0 }
}
/// A sine wave oscillator: a pure tone. Starts at 440 Hz, half volume, silent
/// until osc_play. The other shapes sound brighter (saw, square) or softer (triangle).
pub fn sin_osc() -> u64 { new_osc(0) }
pub fn sqr_osc() -> u64 { new_osc(1) }
pub fn saw_osc() -> u64 { new_osc(2) }
pub fn tri_osc() -> u64 { new_osc(3) }
/// White noise: a steady hiss.
pub fn white_noise() -> u64 { new_osc(4) }
pub fn osc_play(id: u64) { sound_loop(id); }
pub fn osc_stop(id: u64) { sound_pause(id); }
/// Pitch in Hz: 440 is the A above middle C.
pub fn osc_freq(id: u64, hz: f32) { sound_rate(id, hz); }
pub fn osc_amp(id: u64, amp: f32) { sound_amp(id, amp); }
pub fn osc_pan(id: u64, pan: f32) { sound_pan(id, pan); }

/// loadShape("drawing.svg"): an SVG file (also looked for in data/). Returns
/// its id, or 0 if it can't be read.
pub fn load_shape(path: &str) -> u64 { render::load_shape(path) }
pub fn shape_width(id: u64)  -> f32 { render::shape_size(id).0 }
pub fn shape_height(id: u64) -> f32 { render::shape_size(id).1 }
/// shape(s, x, y): draws it at its own size, top-left corner at (x, y).
/// Works for SVGs from load_shape and shapes from create_shape (pass &shape).
pub fn shape(s: impl ShapeRef, x: impl Num, y: impl Num) { s.draw_at(x.f(), y.f(), None) }
/// shape(s, x, y, w, h)
pub fn shape_sized(s: impl ShapeRef, x: impl Num, y: impl Num, w: impl Num, h: impl Num) { s.draw_at(x.f(), y.f(), Some((w.f(), h.f()))) }
/// Draw the shape with the sketch's fill and stroke, not the file's colors.
pub fn shape_disable_style(id: u64) { render::shape_own_style(id, false); }
/// Back to the colors in the file.
pub fn shape_enable_style(id: u64) { render::shape_own_style(id, true); }
#[inline] pub fn loadShape(path: &str) -> u64 { load_shape(path) }

/// createGraphics(w, h): an off-screen layer, transparent to start with.
/// Returns its id (0 if it could not be made).
pub fn create_graphics(w: u32, h: u32) -> u64 { render::create_graphics(w, h) }
/// pg.beginDraw(): from here on, every drawing call goes to the layer.
pub fn begin_graphics(id: u64) -> bool { render::begin_graphics(id) }
/// pg.endDraw(): back to the canvas (or the layer that was the target before).
pub fn end_graphics() { render::end_graphics(); }
pub fn graphics_width(id: u64)  -> f32 { render::graphics_size(id).0 as f32 }
pub fn graphics_height(id: u64) -> f32 { render::graphics_size(id).1 as f32 }
/// image(pg, x, y): draws the layer at its own size. Follows imageMode() and tint().
pub fn graphics_image(id: u64, x: f32, y: f32) {
    let (w, h) = render::graphics_size(id);
    let (w, h) = (w as f32, h as f32);
    let (x, y) = if IMAGE_MODE.load(Ordering::Relaxed) == CENTER { (x - w / 2.0, y - h / 2.0) } else { (x, y) };
    render::draw_graphics(id, x, y, w, h);
}
pub fn graphics_image_sized(id: u64, x: f32, y: f32, w: f32, h: f32) {
    let (x, y, w, h) = corner_box(IMAGE_MODE.load(Ordering::Relaxed), x, y, w, h);
    render::draw_graphics(id, x, y, w, h);
}
/// clear(): makes a layer fully transparent again (on the canvas: black).
pub fn clear() { render::clear(); }
/// Size of what is being drawn on right now: the canvas, or the layer between
/// begin_graphics() and end_graphics().
pub fn target_size() -> (u32, u32) { (CANVAS_W.load(Ordering::Relaxed), CANVAS_H.load(Ordering::Relaxed)) }
#[inline] pub fn createGraphics(w: u32, h: u32) -> u64 { create_graphics(w, h) }

pub fn tint(gray: f32)                            { render::set_tint(Some(Color::from_gray(gray / 255.0))); }
pub fn tint_rgba(r: f32, g: f32, b: f32, a: f32)  { render::set_tint(Some(Color::from_rgba(r/255.0, g/255.0, b/255.0, a/255.0))); }
pub fn tint_rgb(r: f32, g: f32, b: f32)           { tint_rgba(r, g, b, 255.0); }
pub fn tint_ga(gray: f32, a: f32)                 { tint_rgba(gray, gray, gray, a); }
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

/// loadPixels(): every pixel of the canvas as 0xAARRGGBB, row by row, so the
/// pixel at (x, y) is at index y * width + x.
pub fn load_pixels() -> Vec<u32> { render::copy_pixels() }
/// updatePixels(): puts a (changed) copy from load_pixels() back on the canvas.
pub fn update_pixels(pixels: &[u32]) { render::write_pixels(pixels); }
#[inline] pub fn loadPixels() -> Vec<u32> { load_pixels() }
#[inline] pub fn updatePixels(pixels: &[u32]) { update_pixels(pixels) }

pub const BLUR:      u32 = 11;
pub const GRAY:      u32 = 12;
pub const INVERT:    u32 = 13;
pub const OPAQUE:    u32 = 14;
pub const POSTERIZE: u32 = 15;
pub const THRESHOLD: u32 = 16;
pub const ERODE:     u32 = 17;
pub const DILATE:    u32 = 18;
/// filter(GRAY), filter(INVERT), filter(BLUR)...: changes the whole canvas
/// (or the layer being drawn on).
pub fn filter(kind: u32) { render::filter(kind, f32::NAN); }
/// filter(BLUR, radius), filter(THRESHOLD, 0..1), filter(POSTERIZE, levels)
pub fn filter_param(kind: u32, param: f32) { render::filter(kind, param); }
/// rect with a radius per corner: top-left, top-right, bottom-right, bottom-left.
pub fn rect_radii(x: f32, y: f32, w: f32, h: f32, tl: f32, tr: f32, br: f32, bl: f32) {
    let (x, y, w, h) = corner_box(RECT_MODE.load(Ordering::Relaxed), x, y, w, h);
    render::draw_rect_radii(x, y, w, h, [tl, tr, br, bl]);
}

/// Sets one canvas pixel (r, g, b each 0..255). Not affected by transforms.
pub fn set_pixel(x: i32, y: i32, r: f32, g: f32, b: f32) {
    render::set_pixel(x, y, Color::from_rgb(r / 255.0, g / 255.0, b / 255.0));
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
                // whatever just ran, the canvas is the target again
                render::end_draw();
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
pub fn fill_ga(gray: impl Num, alpha: impl Num)       { let gray = gray.f(); let alpha = alpha.f(); render::set_fill_color(cm_gray(gray, Some(alpha))); }
pub fn stroke_ga(gray: impl Num, alpha: impl Num)     { let gray = gray.f(); let alpha = alpha.f(); render::set_stroke_color(cm_gray(gray, Some(alpha))); }
pub fn background_ga(gray: impl Num, alpha: impl Num) { let gray = gray.f(); let alpha = alpha.f(); render::background_color(cm_gray(gray, Some(alpha))); }

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

// Files, JSON and tables (see data.rs).
mod data;
pub use data::{load_strings, load_text, load_bytes, save_strings, save_text, save_bytes,
               split_tokens, split, join, trim, Json, parse_json, load_json, save_json,
               Table, Col, load_table, save_table};
#[inline] pub fn loadStrings(path: &str) -> Vec<String> { load_strings(path) }
#[inline] pub fn saveStrings<S: AsRef<str>>(path: &str, lines: &[S]) -> bool { save_strings(path, lines) }
#[inline] pub fn loadBytes(path: &str) -> Vec<u8> { load_bytes(path) }
#[inline] pub fn saveBytes(path: &str, b: &[u8]) -> bool { save_bytes(path, b) }
#[inline] pub fn splitTokens(s: &str, d: &str) -> Vec<String> { split_tokens(s, d) }
#[inline] pub fn loadJSONObject(path: &str) -> Json { load_json(path) }
#[inline] pub fn loadJSONArray(path: &str) -> Json { load_json(path) }
#[inline] pub fn saveJSONObject(v: &Json, path: &str) -> bool { save_json(path, v) }
#[inline] pub fn saveJSONArray(v: &Json, path: &str) -> bool { save_json(path, v) }
#[inline] pub fn parseJSONObject(s: &str) -> Json { parse_json(s).unwrap_or_default() }
#[inline] pub fn loadTable(path: &str, options: &str) -> Table { load_table(path, options) }
#[inline] pub fn saveTable(t: &Table, path: &str) -> bool { save_table(t, path) }

/// Any number type works where the API wants a number: 3, 3.0, x as usize...
pub trait Num: Copy { fn f(self) -> f32; }
macro_rules! num_impl { ($($t:ty),*) => { $(impl Num for $t { #[inline] fn f(self) -> f32 { self as f32 } })* } }
num_impl!(f32, f64, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
/// fill(), stroke() and background() take a gray level or a Color.
pub trait Paint { fn paint(self) -> Color; }
impl<T: Num> Paint for T { fn paint(self) -> Color { cm_gray(self.f(), None) } }
impl Paint for Color { fn paint(self) -> Color { self } }

// ──────────────────────────── Color mode ─────────────────────────────────────

pub const RGB: u32 = 1;
pub const HSB: u32 = 3;

#[derive(Clone, Copy)]
struct ColorMode { hsb: bool, max: [f32; 4] }
static COLOR_MODE: RwLock<ColorMode> = RwLock::new(ColorMode { hsb: false, max: [255.0; 4] });

/// colorMode(HSB): hue, saturation and brightness, each 0..255.
/// color_mode_max(HSB, 360, 100, 100) gives the usual ranges.
pub fn color_mode(mode: u32) { let m = COLOR_MODE.read().max[0]; color_mode_max(mode, m, m, m, m); }
/// color_mode_range(RGB, 1.0): every channel 0..1.
pub fn color_mode_range(mode: u32, max: f32) { color_mode_max(mode, max, max, max, max); }
pub fn color_mode_max(mode: u32, max1: f32, max2: f32, max3: f32, max_a: f32) {
    let fix = |v: f32| if v.is_finite() && v > 0.0 { v } else { 255.0 };
    *COLOR_MODE.write() = ColorMode { hsb: mode == HSB, max: [fix(max1), fix(max2), fix(max3), fix(max_a)] };
}
pub fn color_mode3(mode: u32, max1: f32, max2: f32, max3: f32) {
    let a = COLOR_MODE.read().max[3];
    color_mode_max(mode, max1, max2, max3, a);
}
#[inline] pub fn colorMode(mode: u32) { color_mode(mode) }

fn hsb_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    if s <= 0.0 { return (v, v, v); }
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 { 0 => (v, t, p), 1 => (q, v, p), 2 => (p, v, t), 3 => (p, q, v), 4 => (t, p, v), _ => (v, p, q) }
}
fn rgb_to_hsb(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d <= 0.0 { 0.0 }
        else if max == r { ((g - b) / d).rem_euclid(6.0) / 6.0 }
        else if max == g { ((b - r) / d + 2.0) / 6.0 }
        else { ((r - g) / d + 4.0) / 6.0 };
    (h, s, max)
}

/// Three channels (and maybe alpha) in the current color mode, as a Color.
fn cm(a: f32, b: f32, c: f32, alpha: Option<f32>) -> Color {
    let m = *COLOR_MODE.read();
    let n = |v: f32, i: usize| (v / m.max[i]).clamp(0.0, 1.0);
    let al = alpha.map(|v| n(v, 3)).unwrap_or(1.0);
    if m.hsb {
        let hue = (a / m.max[0]).rem_euclid(1.0);
        let hue = if a >= m.max[0] && a > 0.0 && hue == 0.0 { 0.0 } else { hue };
        let (r, g, bl) = hsb_to_rgb(hue, n(b, 1), n(c, 2));
        Color::from_rgba(r, g, bl, al)
    } else {
        Color::from_rgba(n(a, 0), n(b, 1), n(c, 2), al)
    }
}
/// Gray in the current range (the first channel's maximum, like Processing).
fn cm_gray(v: f32, alpha: Option<f32>) -> Color {
    let m = *COLOR_MODE.read();
    let g = (v / if m.hsb { m.max[2] } else { m.max[0] }).clamp(0.0, 1.0);
    Color::from_rgba(g, g, g, alpha.map(|a| (a / m.max[3]).clamp(0.0, 1.0)).unwrap_or(1.0))
}

/// red(c), green(c), blue(c), alpha(c): 0..255 (or the range set with color_mode).
pub fn red(c: Color) -> f32 { c.r * COLOR_MODE.read().max[0] }
pub fn green(c: Color) -> f32 { c.g * COLOR_MODE.read().max[1] }
pub fn blue(c: Color) -> f32 { c.b * COLOR_MODE.read().max[2] }
pub fn alpha(c: Color) -> f32 { c.a * COLOR_MODE.read().max[3] }
pub fn hue(c: Color) -> f32 { rgb_to_hsb(c).0 * COLOR_MODE.read().max[0] }
pub fn saturation(c: Color) -> f32 { rgb_to_hsb(c).1 * COLOR_MODE.read().max[1] }
pub fn brightness(c: Color) -> f32 { rgb_to_hsb(c).2 * COLOR_MODE.read().max[2] }
/// A color from a hex code: hex_color("#ff8800") or hex_color("ff880080").
pub fn hex_color(s: &str) -> Color {
    let t = s.trim().trim_start_matches('#').trim_start_matches("0x");
    let v = u32::from_str_radix(t, 16).unwrap_or(0);
    let byte = |sh: u32| ((v >> sh) & 0xFF) as f32 / 255.0;
    match t.len() {
        8 => Color::from_rgba(byte(24), byte(16), byte(8), byte(0)),
        3 => { let n = |sh: u32| ((v >> sh) & 0xF) as f32 / 15.0; Color::from_rgb(n(8), n(4), n(0)) }
        _ => Color::from_rgb(byte(16), byte(8), byte(0)),
    }
}

// ──────────────────────────── Number formatting and math ─────────────────────

fn group(digits: &str) -> String {
    let b = digits.as_bytes();
    let mut out = String::new();
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 { out.push(','); }
        out.push(*c as char);
    }
    out
}
/// nf(7, 3) is "007": pad the whole part with zeros to at least `digits`.
pub fn nf(n: impl Into<i64>, digits: usize) -> String {
    let n: i64 = n.into();
    let s = format!("{:0w$}", n.unsigned_abs(), w = digits);
    if n < 0 { format!("-{s}") } else { s }
}
/// nf_float(3.14159, 2, 2) is "03.14": left digits, then digits after the point.
pub fn nf_float(n: f32, left: usize, right: usize) -> String {
    let s = format!("{:.*}", right, n.abs());
    let (whole, frac) = s.split_once('.').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((s.clone(), None));
    let whole = format!("{:0>w$}", whole, w = left);
    let body = match frac { Some(f) => format!("{whole}.{f}"), None => whole };
    if n < 0.0 && body.chars().any(|c| c != '0' && c != '.') { format!("-{body}") } else { body }
}
/// nfc(1234567) is "1,234,567".
pub fn nfc(n: impl Into<i64>) -> String {
    let n: i64 = n.into();
    let g = group(&n.unsigned_abs().to_string());
    if n < 0 { format!("-{g}") } else { g }
}
/// nfc_float(1234.5, 2) is "1,234.50".
pub fn nfc_float(n: f32, right: usize) -> String {
    let s = format!("{:.*}", right, n.abs());
    let (whole, frac) = s.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((&s, None));
    let g = group(whole);
    let body = match frac { Some(f) => format!("{g}.{f}"), None => g };
    if n < 0.0 { format!("-{body}") } else { body }
}
/// nfs: like nf, with a space where a minus sign would go, so columns line up.
pub fn nfs(n: impl Into<i64>, digits: usize) -> String { let n: i64 = n.into(); if n < 0 { nf(n, digits) } else { format!(" {}", nf(n, digits)) } }
/// nfp: like nf, with a + on positive numbers.
pub fn nfp(n: impl Into<i64>, digits: usize) -> String { let n: i64 = n.into(); if n < 0 { nf(n, digits) } else { format!("+{}", nf(n, digits)) } }

/// A gray color with alpha.
pub fn color_ga(gray: impl Num, a: impl Num) -> Color { let gray = gray.f(); let a = a.f(); cm_gray(gray, Some(a)) }
pub fn sq(x: f32) -> f32 { x * x }
/// norm(v, low, high): where v sits between low and high, 0..1.
pub fn norm(v: f32, low: f32, high: f32) -> f32 { if high == low { 0.0 } else { (v - low) / (high - low) } }
/// A point along a bezier curve: bezier_point(a, b, c, d, t), once for x and once for y.
pub fn bezier_point(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    let u = 1.0 - t;
    a * u * u * u + 3.0 * b * t * u * u + 3.0 * c * t * t * u + d * t * t * t
}
pub fn bezier_tangent(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    3.0 * t * t * (-a + 3.0 * b - 3.0 * c + d) + 6.0 * t * (a - 2.0 * b + c) + 3.0 * (-a + b)
}
/// A point along a Catmull-Rom curve (what curve() draws).
pub fn curve_point(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    let (t2, t3) = (t * t, t * t * t);
    0.5 * ((2.0 * b) + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3)
}
pub fn curve_tangent(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    0.5 * ((-a + c) + 2.0 * (2.0 * a - 5.0 * b + 4.0 * c - d) * t + 3.0 * (-a + 3.0 * b - 3.0 * c + d) * t * t)
}
/// delay(ms): pauses the sketch. Mostly useful in setup; in draw it stalls the window.
pub fn delay(ms: u32) { std::thread::sleep(std::time::Duration::from_millis(ms as u64)); }

#[cfg(test)]
mod color_tests {
    use super::*;
    #[test]
    fn modes_and_formatting() {
        let c = cm(255.0, 0.0, 0.0, None);
        assert_eq!((c.r, c.g, c.b), (1.0, 0.0, 0.0));
        color_mode_max(HSB, 360.0, 100.0, 100.0, 1.0);
        let c = cm(120.0, 100.0, 100.0, Some(0.5));
        assert!((c.g - 1.0).abs() < 1e-4 && c.r.abs() < 1e-4 && (c.a - 0.5).abs() < 1e-4);
        assert!((hue(c) - 120.0).abs() < 0.01 && (saturation(c) - 100.0).abs() < 0.01);
        let c = cm(360.0, 100.0, 100.0, None); // hue wraps to red
        assert!((c.r - 1.0).abs() < 1e-4 && c.g.abs() < 1e-4);
        color_mode_max(RGB, 1.0, 1.0, 1.0, 1.0);
        assert!((cm_gray(0.5, None).r - 0.5).abs() < 1e-6);
        assert!((red(hex_color("#ff8000")) - 1.0).abs() < 1e-6);
        color_mode_max(RGB, 255.0, 255.0, 255.0, 255.0);
        assert!((green(hex_color("#ff8000")) - 128.0).abs() < 1.0);
        assert_eq!(nf(7, 3), "007");
        assert_eq!(nf(-7, 3), "-007");
        assert_eq!(nf_float(3.14159, 2, 2), "03.14");
        assert_eq!(nf_float(-0.001, 1, 2), "0.00");
        assert_eq!(nfc(1234567), "1,234,567");
        assert_eq!(nfc(-1000), "-1,000");
        assert_eq!(nfc(999), "999");
        assert_eq!(nfc_float(1234.5, 2), "1,234.50");
        assert_eq!(nfs(5, 2), " 05");
        assert_eq!(nfp(5, 1), "+5");
        assert_eq!(bezier_point(0.0, 0.0, 10.0, 10.0, 0.5), 5.0);
        assert_eq!(curve_point(0.0, 0.0, 10.0, 10.0, 0.0), 0.0);
        assert_eq!(norm(5.0, 0.0, 10.0), 0.5);
    }
}

// ──────────────────────────── Image editing ──────────────────────────────────

/// createImage(w, h): a blank transparent image to draw pixels into.
pub fn create_image(w: u32, h: u32) -> u64 { render::create_image(w as usize, h as usize) }
#[inline] pub fn createImage(w: u32, h: u32) -> u64 { create_image(w, h) }
/// img.get(x, y): the color of one pixel (transparent outside the image).
pub fn image_get(id: u64, x: i32, y: i32) -> Color { render::image_get(id, x as i64, y as i64) }
pub fn image_set(id: u64, x: i32, y: i32, c: Color) { render::image_set(id, x as i64, y as i64, c) }
/// Every pixel, row by row. Change them and hand them back with image_set_pixels.
pub fn image_pixels(id: u64) -> Vec<Color> { render::image_pixels(id) }
pub fn image_set_pixels(id: u64, px: &[Color]) { render::image_set_pixels(id, px) }
/// img.get(x, y, w, h): part of an image, as a new image.
pub fn image_crop(id: u64, x: i32, y: i32, w: u32, h: u32) -> u64 { render::image_crop(id, x as i64, y as i64, w as usize, h as usize) }
pub fn image_copy(id: u64) -> u64 { render::image_copy(id) }
/// img.resize(w, h). Use 0 for one side to keep the proportions.
pub fn image_resize(id: u64, w: u32, h: u32) { render::image_resize(id, w as usize, h as usize) }
/// img.mask(other): the other image's brightness becomes this one's transparency.
pub fn image_mask(id: u64, mask: u64) -> bool { render::image_mask(id, mask) }
pub fn save_image(id: u64, path: &str) -> bool { render::save_image(id, path) }
/// get(x, y, w, h): a piece of the canvas as a new image.
pub fn get_region(x: i32, y: i32, w: u32, h: u32) -> u64 { render::canvas_region(x as i64, y as i64, w as usize, h as usize) }

#[cfg(test)]
mod image_tests {
    use super::*;
    #[test]
    fn edit_images() {
        let a = create_image(4, 2);
        assert!(a > 0);
        image_set(a, 1, 1, Color::from_rgb(1.0, 0.0, 0.0));
        assert_eq!(image_get(a, 1, 1).r, 1.0);
        assert_eq!(image_get(a, 9, 9).a, 0.0);
        let c = image_crop(a, 1, 1, 2, 2);
        assert_eq!(image_get(c, 0, 0).r, 1.0);
        assert_eq!(image_get(c, 0, 1).a, 0.0);
        image_resize(a, 8, 0);
        assert_eq!(render::image_size(a), (8.0, 4.0));
        let m = create_image(8, 4);
        let mut px = image_pixels(m);
        for p in px.iter_mut() { *p = Color::from_rgba(0.5, 0.5, 0.5, 1.0); }
        image_set_pixels(m, &px);
        let b = image_copy(a);
        for p in image_pixels(b).iter() { assert!(p.a <= 1.0); }
        image_set(b, 0, 0, Color::from_rgb(0.0, 1.0, 0.0));
        assert!(image_mask(b, m));
        assert!((image_get(b, 0, 0).a - 0.5).abs() < 1e-4);
        assert!(!image_mask(b, c));
        let path = std::env::temp_dir().join(format!("rm-img-{}.png", std::process::id()));
        assert!(save_image(b, path.to_str().unwrap()));
        let back = load_image(path.to_str().unwrap());
        assert_eq!(render::image_size(back), (8.0, 4.0));
        assert!((image_get(back, 0, 0).g - 1.0).abs() < 0.01);
        let _ = std::fs::remove_file(path);
        assert_eq!(create_image(0, 5), 0);
        assert_eq!(image_get(9999, 0, 0).a, 0.0);
    }
}

#[cfg(test)]
mod text_tests {
    use super::*;
    #[test]
    fn wrapping() {
        // built-in font: 6 px per letter at size 8
        text_size(8.0);
        assert_eq!(wrap_text("aa bb cc", 30.0), vec!["aa bb", "cc"]);
        assert_eq!(wrap_text("abcdefghij", 30.0), vec!["abcde", "fghij"]);
        assert_eq!(wrap_text("one\ntwo", 1000.0), vec!["one", "two"]);
        assert_eq!(wrap_text("", 30.0), vec![""]);
        for l in wrap_text("the quick brown fox jumps over the lazy dog", 40.0) { assert!(text_width(&l) <= 40.0, "{l}"); }
    }
}

// createShape (see pshape.rs).
mod pshape;
pub use pshape::{PShape, ShapeRef, create_shape, create_shape_group, create_shape_rect, create_shape_ellipse,
                 create_shape_line, create_shape_triangle, create_shape_quad, createShape};

// Microphone and sound analysis (see mic.rs).
mod mic;
pub use mic::{mic_start, mic_stop, mic_level, mic_waveform, mic_spectrum, mic_sample_rate};
/// How loud a playing sound file is right now, 0..1.
pub fn sound_level(id: u64) -> f32 {
    #[cfg(not(feature = "wasm"))] { mic::rms(&audio::window(id, 1024)) }
    #[cfg(feature = "wasm")] { let _ = id; 0.0 }
}
/// The spectrum of a playing sound file: `bands` values, low to high, about 0..1.
pub fn sound_spectrum(id: u64, bands: usize) -> Vec<f32> {
    #[cfg(not(feature = "wasm"))] { let n = (bands.max(1) * 2).next_power_of_two().max(1024); mic::spectrum(&audio::window(id, n), bands) }
    #[cfg(feature = "wasm")] { let _ = id; vec![0.0; bands] }
}
/// The waveform of a playing sound file: the last n samples, -1..1.
pub fn sound_waveform(id: u64, n: usize) -> Vec<f32> {
    #[cfg(not(feature = "wasm"))] { audio::window(id, n.max(1)) }
    #[cfg(feature = "wasm")] { let _ = id; vec![0.0; n] }
}
