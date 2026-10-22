//! PImage and PFont methods on image and font ids.
//!
//!   let img = load_image("cat.png");
//!   let c = img.get(10, 10);
//!   img.filter(GRAY, 0.0);
//!   img.resize(200, 0);
//!
//! Images live in the shared image store, so this works the same with either
//! renderer; a changed image is sent to the GPU again on its next draw.

use crate::{Color, Num};

fn to_u32(c: Color) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (b(c.a) << 24) | (b(c.r) << 16) | (b(c.g) << 8) | b(c.b)
}
fn from_u32(p: u32) -> Color {
    let f = |s: u32| ((p >> s) & 0xFF) as f32 / 255.0;
    Color::from_rgba(f(16), f(8), f(0), f(24))
}

/// One channel of a blend mode: d is what is there, s what comes in.
fn blend_channel(mode: u32, d: f32, s: f32) -> f32 {
    use crate::constants::{BURN, DODGE, HARD_LIGHT, OVERLAY, SOFT_LIGHT};
    let screen = |a: f32, b: f32| 1.0 - (1.0 - a) * (1.0 - b);
    match mode {
        crate::ADD => (d + s).min(1.0),
        crate::SUBTRACT => (d - s).max(0.0),
        crate::LIGHTEST => d.max(s),
        crate::DARKEST => d.min(s),
        crate::DIFFERENCE => (d - s).abs(),
        crate::EXCLUSION => d + s - 2.0 * d * s,
        crate::MULTIPLY => d * s,
        crate::SCREEN => screen(d, s),
        OVERLAY => if d < 0.5 { 2.0 * d * s } else { 1.0 - 2.0 * (1.0 - d) * (1.0 - s) },
        HARD_LIGHT => if s < 0.5 { 2.0 * d * s } else { 1.0 - 2.0 * (1.0 - d) * (1.0 - s) },
        SOFT_LIGHT => (1.0 - 2.0 * s) * d * d + 2.0 * s * d,
        DODGE => if s >= 1.0 { 1.0 } else { (d / (1.0 - s)).min(1.0) },
        BURN => if s <= 0.0 { 0.0 } else { (1.0 - (1.0 - d) / s).max(0.0) },
        _ => s,
    }
}

/// A source pixel over a destination one, the way Processing's blend() does:
/// the mode's result, faded in by the source's alpha.
pub(crate) fn blend_px(mode: u32, d: Color, s: Color) -> Color {
    if mode == crate::REPLACE { return s; }
    let ch = |dv: f32, sv: f32| dv + (blend_channel(mode, dv, sv) - dv) * s.a;
    Color::from_rgba(ch(d.r, s.r), ch(d.g, s.g), ch(d.b, s.b), (d.a + s.a).min(1.0))
}

/// The blend modes the canvas blend_mode() has no drawing path for.
pub(crate) fn is_extra_blend(mode: u32) -> bool {
    use crate::constants::{BURN, DODGE, HARD_LIGHT, OVERLAY, SOFT_LIGHT};
    matches!(mode, OVERLAY | HARD_LIGHT | SOFT_LIGHT | DODGE | BURN)
}

/// Scales a w x h block of pixels onto a region of another (nearest pixel),
/// blending each one.
#[allow(clippy::too_many_arguments)]
fn blit(dst: &mut [Color], dw_img: usize, dh_img: usize, src: &[Color], sw_img: usize, sh_img: usize,
        sx: i64, sy: i64, sw: i64, sh: i64, dx: i64, dy: i64, dw: i64, dh: i64, mode: u32) {
    if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 { return; }
    for y in 0..dh {
        let ty = dy + y;
        if ty < 0 || ty >= dh_img as i64 { continue; }
        let py = sy + (y * sh) / dh;
        for x in 0..dw {
            let tx = dx + x;
            if tx < 0 || tx >= dw_img as i64 { continue; }
            let px = sx + (x * sw) / dw;
            let s = if px >= 0 && py >= 0 && (px as usize) < sw_img && (py as usize) < sh_img {
                src[py as usize * sw_img + px as usize]
            } else { Color::from_rgba(0.0, 0.0, 0.0, 0.0) };
            let i = ty as usize * dw_img + tx as usize;
            dst[i] = blend_px(mode, dst[i], s);
        }
    }
}

/// blend() on the canvas for OVERLAY, HARD_LIGHT, SOFT_LIGHT, DODGE and BURN,
/// done pixel by pixel. `part` is the sw x sh source taken from the canvas.
#[allow(clippy::too_many_arguments)]
pub(crate) fn blend_canvas(part: &[Color], sw: usize, sh: usize, dx: f32, dy: f32, dw: f32, dh: f32, mode: u32) {
    let (cw, ch) = (crate::CANVAS_W.load(std::sync::atomic::Ordering::Relaxed) as usize,
                    crate::CANVAS_H.load(std::sync::atomic::Ordering::Relaxed) as usize);
    let px = crate::render::copy_pixels();
    if px.len() < cw * ch { return; }
    let mut dst: Vec<Color> = px.iter().map(|p| from_u32(*p)).collect();
    blit(&mut dst, cw, ch, part, sw, sh, 0, 0, sw as i64, sh as i64,
         dx.round() as i64, dy.round() as i64, dw.round() as i64, dh.round() as i64, mode);
    let out: Vec<u32> = dst.into_iter().map(to_u32).collect();
    crate::render::write_pixels(&out);
}

fn luma(p: u32) -> u32 { (77 * ((p >> 16) & 0xFF) + 151 * ((p >> 8) & 0xFF) + 28 * (p & 0xFF)) >> 8 }

/// filter() on a block of 0xAARRGGBB pixels: Processing's kinds and numbers.
pub(crate) fn filter_pixels(buf: &mut [u32], w: usize, h: usize, kind: u32, param: f32) {
    if w == 0 || h == 0 || buf.len() < w * h { return; }
    match kind {
        crate::GRAY => for p in buf.iter_mut() { let l = luma(*p); *p = (*p & 0xFF00_0000) | (l << 16) | (l << 8) | l; },
        crate::INVERT => for p in buf.iter_mut() { *p = (*p & 0xFF00_0000) | (!*p & 0x00FF_FFFF); },
        crate::OPAQUE => for p in buf.iter_mut() { *p |= 0xFF00_0000; },
        crate::POSTERIZE => {
            let levels = if param.is_finite() && param >= 2.0 { param.round().min(255.0) as u32 } else { 4 };
            let step = |v: u32| -> u32 { ((v * levels) >> 8) * 255 / (levels - 1) };
            for p in buf.iter_mut() {
                *p = (*p & 0xFF00_0000) | (step((*p >> 16) & 0xFF) << 16) | (step((*p >> 8) & 0xFF) << 8) | step(*p & 0xFF);
            }
        }
        crate::THRESHOLD => {
            let level = if param.is_finite() && param > 0.0 { param.min(1.0) } else { 0.5 };
            let cut = (level * 255.0) as u32;
            for p in buf.iter_mut() {
                let m = ((*p >> 16) & 0xFF).max((*p >> 8) & 0xFF).max(*p & 0xFF);
                *p = (*p & 0xFF00_0000) | if m < cut { 0 } else { 0x00FF_FFFF };
            }
        }
        crate::ERODE | crate::DILATE => {
            let src = buf.to_vec();
            let dilate = kind == crate::DILATE;
            for y in 0..h {
                for x in 0..w {
                    let mut best = src[y * w + x];
                    let mut best_l = luma(best);
                    for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 { continue; }
                        let q = src[ny as usize * w + nx as usize];
                        let l = luma(q);
                        if (dilate && l > best_l) || (!dilate && l < best_l) { best = q; best_l = l; }
                    }
                    buf[y * w + x] = best;
                }
            }
        }
        crate::BLUR => {
            let r = if param.is_finite() && param > 0.0 { param.min(256.0) } else { 1.0 };
            // three box blurs come close to a Gaussian
            let radius = ((r * 0.6).round() as usize).max(1);
            let mut tmp = vec![0u32; w * h];
            for _ in 0..3 {
                box_blur(buf, &mut tmp, w, h, radius, true);
                box_blur(&tmp, buf, w, h, radius, false);
            }
        }
        _ => {}
    }
}

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

/// Processing's PImage methods, on an image id (from load_image,
/// create_image, get_region...).
pub trait PImageExt {
    fn width(&self) -> f32;
    fn height(&self) -> f32;
    /// True when the image was loaded or made (id is not 0).
    fn is_loaded(&self) -> bool;
    /// One pixel; transparent black outside.
    fn get(&self, x: impl Num, y: impl Num) -> Color;
    /// get(x, y, w, h): a new image cut out of this one.
    fn get_region(&self, x: impl Num, y: impl Num, w: impl Num, h: impl Num) -> u64;
    fn set(&self, x: impl Num, y: impl Num, c: Color);
    /// set(x, y, img): draws another image into this one at x, y.
    fn set_image(&self, x: impl Num, y: impl Num, src: u64);
    /// A new image with the same pixels.
    fn copy(&self) -> u64;
    /// copy(src, sx, sy, sw, sh, dx, dy, dw, dh): part of src, scaled into part of this.
    #[allow(clippy::too_many_arguments)]
    fn copy_from(&self, src: u64, sx: impl Num, sy: impl Num, sw: impl Num, sh: impl Num,
                 dx: impl Num, dy: impl Num, dw: impl Num, dh: impl Num);
    /// copy(sx, sy, sw, sh, dx, dy, dw, dh) within this image.
    #[allow(clippy::too_many_arguments)]
    fn copy_within(&self, sx: impl Num, sy: impl Num, sw: impl Num, sh: impl Num,
                   dx: impl Num, dy: impl Num, dw: impl Num, dh: impl Num);
    /// blend(src, sx, sy, sw, sh, dx, dy, dw, dh, mode)
    #[allow(clippy::too_many_arguments)]
    fn blend(&self, src: u64, sx: impl Num, sy: impl Num, sw: impl Num, sh: impl Num,
             dx: impl Num, dy: impl Num, dw: impl Num, dh: impl Num, mode: u32);
    /// resize(w, h); a 0 keeps the aspect ratio.
    fn resize(&self, w: impl Num, h: impl Num);
    /// mask(img): the mask's blue channel becomes this image's alpha.
    fn mask(&self, mask: u64) -> bool;
    /// The pixels as 0xAARRGGBB, row by row.
    fn load_pixels(&self) -> Vec<u32>;
    /// Writes pixels back (as many as fit).
    fn update_pixels(&self, px: &[u32]);
    /// filter(GRAY, 0), filter(BLUR, 3), filter(THRESHOLD, 0.5)...
    fn filter(&self, kind: u32, param: f32);
    /// Saves as PNG (with alpha).
    fn save(&self, path: &str) -> bool;
    #[inline] fn loadPixels(&self) -> Vec<u32> { self.load_pixels() }
    #[inline] fn updatePixels(&self, px: &[u32]) { self.update_pixels(px) }
    #[inline] fn isLoaded(&self) -> bool { self.is_loaded() }
}

fn dims(id: u64) -> (usize, usize) { let (w, h) = crate::render::image_size(id); (w as usize, h as usize) }

impl PImageExt for u64 {
    fn width(&self) -> f32 { crate::render::image_size(*self).0 }
    fn height(&self) -> f32 { crate::render::image_size(*self).1 }
    fn is_loaded(&self) -> bool { *self != 0 && dims(*self).0 > 0 }
    fn get(&self, x: impl Num, y: impl Num) -> Color { crate::render::image_get(*self, x.f().floor() as i64, y.f().floor() as i64) }
    fn get_region(&self, x: impl Num, y: impl Num, w: impl Num, h: impl Num) -> u64 {
        let (w, h) = (w.f(), h.f());
        if !(w >= 1.0 && h >= 1.0) { return 0; }
        crate::render::image_crop(*self, x.f().floor() as i64, y.f().floor() as i64, w as usize, h as usize)
    }
    fn set(&self, x: impl Num, y: impl Num, c: Color) { crate::render::image_set(*self, x.f().floor() as i64, y.f().floor() as i64, c) }
    fn set_image(&self, x: impl Num, y: impl Num, src: u64) {
        let (w, h) = dims(src);
        self.copy_from(src, 0, 0, w as f32, h as f32, x, y, w as f32, h as f32);
    }
    fn copy(&self) -> u64 { crate::render::image_copy(*self) }
    fn copy_from(&self, src: u64, sx: impl Num, sy: impl Num, sw: impl Num, sh: impl Num,
                 dx: impl Num, dy: impl Num, dw: impl Num, dh: impl Num) {
        self.blend(src, sx, sy, sw, sh, dx, dy, dw, dh, crate::REPLACE);
    }
    fn copy_within(&self, sx: impl Num, sy: impl Num, sw: impl Num, sh: impl Num,
                   dx: impl Num, dy: impl Num, dw: impl Num, dh: impl Num) {
        self.copy_from(*self, sx, sy, sw, sh, dx, dy, dw, dh);
    }
    fn blend(&self, src: u64, sx: impl Num, sy: impl Num, sw: impl Num, sh: impl Num,
             dx: impl Num, dy: impl Num, dw: impl Num, dh: impl Num, mode: u32) {
        let (w, h) = dims(*self);
        let (srcw, srch) = dims(src);
        if w == 0 || srcw == 0 { return; }
        let source = crate::render::image_pixels(src);
        let mut px = crate::render::image_pixels(*self);
        let i = |v: f32| if v.is_finite() { v.round() as i64 } else { 0 };
        blit(&mut px, w, h, &source, srcw, srch, i(sx.f()), i(sy.f()), i(sw.f()), i(sh.f()),
             i(dx.f()), i(dy.f()), i(dw.f()), i(dh.f()), mode);
        crate::render::image_set_pixels(*self, &px);
    }
    fn resize(&self, w: impl Num, h: impl Num) {
        let (w, h) = (w.f().max(0.0), h.f().max(0.0));
        crate::render::image_resize(*self, w as usize, h as usize)
    }
    fn mask(&self, mask: u64) -> bool {
        if dims(*self) != dims(mask) { eprintln!("mask: the mask must be the same size as the image"); return false; }
        let m = crate::render::image_pixels(mask);
        let mut px = crate::render::image_pixels(*self);
        for (p, q) in px.iter_mut().zip(m.iter()) { p.a = (q.b * 255.0).round() / 255.0; }
        crate::render::image_set_pixels(*self, &px);
        true
    }
    fn load_pixels(&self) -> Vec<u32> { crate::render::image_pixels(*self).into_iter().map(to_u32).collect() }
    fn update_pixels(&self, px: &[u32]) {
        let colors: Vec<Color> = px.iter().map(|p| from_u32(*p)).collect();
        crate::render::image_set_pixels(*self, &colors);
    }
    fn filter(&self, kind: u32, param: f32) {
        let (w, h) = dims(*self);
        let mut px = self.load_pixels();
        filter_pixels(&mut px, w, h, kind, param);
        self.update_pixels(&px);
    }
    fn save(&self, path: &str) -> bool { crate::render::save_image(*self, path) }
}

/// Processing's PFont methods, on a font id (from create_font / load_font).
/// Sizes are for a font 1 unit tall, as in Processing: multiply by the text size.
pub trait PFontExt {
    /// The size the font was made with (0 if none was given).
    fn get_size(&self) -> f32;
    /// The font's own name, or "" when it has none.
    fn get_name(&self) -> String;
    fn ascent(&self) -> f32;
    fn descent(&self) -> f32;
    /// How far one character moves the pen.
    fn char_width(&self, c: char) -> f32;
    #[inline] fn getSize(&self) -> f32 { self.get_size() }
    #[inline] fn getName(&self) -> String { self.get_name() }
}

fn font_px(size: f32) -> f32 { if size > 0.0 { size } else { 48.0 } }

impl PFontExt for u64 {
    fn get_size(&self) -> f32 { crate::render::font_info(*self).map(|f| f.1).unwrap_or(0.0) }
    fn get_name(&self) -> String {
        crate::render::font_info(*self).and_then(|f| f.0.name().map(str::to_string)).unwrap_or_default()
    }
    fn ascent(&self) -> f32 {
        crate::render::font_info(*self).and_then(|(f, s)| {
            let px = font_px(s);
            f.horizontal_line_metrics(px).map(|m| m.ascent / px)
        }).unwrap_or(0.0)
    }
    fn descent(&self) -> f32 {
        crate::render::font_info(*self).and_then(|(f, s)| {
            let px = font_px(s);
            f.horizontal_line_metrics(px).map(|m| -m.descent / px)
        }).unwrap_or(0.0)
    }
    fn char_width(&self, c: char) -> f32 {
        crate::render::font_info(*self).map(|(f, s)| { let px = font_px(s); f.metrics(c, px).advance_width / px }).unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color { Color::from_rgba(r, g, b, a) }
    #[test]
    fn get_set_copy_region() {
        let img = crate::create_image(4, 3);
        assert!(img.is_loaded());
        assert_eq!((img.width(), img.height()), (4.0, 3.0));
        img.set(1, 2, rgba(1.0, 0.0, 0.0, 1.0));
        assert_eq!(img.get(1, 2), rgba(1.0, 0.0, 0.0, 1.0));
        assert_eq!(img.get(9, 9).a, 0.0);
        let px = img.load_pixels();
        assert_eq!(px[2 * 4 + 1], 0xFFFF_0000);
        let c = img.copy();
        assert_eq!(c.get(1, 2), img.get(1, 2));
        let r = img.get_region(1, 1, 2, 2);
        assert_eq!((r.width(), r.height()), (2.0, 2.0));
        assert_eq!(r.get(0, 1), rgba(1.0, 0.0, 0.0, 1.0));
        let big = crate::create_image(8, 8);
        big.copy_from(img, 0, 0, 4, 3, 0, 0, 8, 6);
        assert_eq!(big.get(2, 4), rgba(1.0, 0.0, 0.0, 1.0));
        assert_eq!(big.get(3, 5), rgba(1.0, 0.0, 0.0, 1.0));
        big.resize(4, 0);
        assert_eq!((big.width(), big.height()), (4.0, 4.0));
        assert!(!0u64.is_loaded());
    }
    #[test]
    fn filters_and_mask() {
        let img = crate::create_image(5, 5);
        img.update_pixels(&vec![0xFF20_4080; 25]);
        img.filter(crate::INVERT, 0.0);
        assert_eq!(img.load_pixels()[0], 0xFFDF_BF7F);
        img.filter(crate::GRAY, 0.0);
        let p = img.load_pixels()[0];
        assert_eq!((p >> 16) & 0xFF, p & 0xFF);
        img.filter(crate::THRESHOLD, 0.5);
        assert_eq!(img.load_pixels()[0], 0xFFFF_FFFF);
        let mut px = vec![0xFF00_0000u32; 25];
        px[12] = 0xFFFF_FFFF;
        img.update_pixels(&px);
        img.filter(crate::DILATE, 0.0);
        assert_eq!(img.load_pixels()[7], 0xFFFF_FFFF);
        img.filter(crate::BLUR, 2.0);
        assert!(img.load_pixels().iter().all(|p| p >> 24 == 0xFF));
        let m = crate::create_image(5, 5);
        m.update_pixels(&vec![0xFF00_0080; 25]);
        assert!(img.mask(m));
        assert_eq!(img.load_pixels()[0] >> 24, 0x80);
        assert!(!img.mask(crate::create_image(2, 2)));
    }
    #[test]
    fn blend_modes() {
        let d = rgba(0.25, 0.75, 0.5, 1.0);
        let s = rgba(0.5, 0.5, 1.0, 1.0);
        let m = blend_px(crate::MULTIPLY, d, s);
        assert!((m.r - 0.125).abs() < 1e-6 && (m.b - 0.5).abs() < 1e-6);
        let o = blend_px(crate::constants::OVERLAY, d, s);
        assert!((o.r - 0.25).abs() < 1e-6 && (o.g - 0.75).abs() < 1e-6);
        assert_eq!(blend_px(crate::constants::DODGE, d, s).b, 1.0);
        assert_eq!(blend_px(crate::constants::BURN, d, rgba(0.0, 0.0, 0.0, 1.0)).r, 0.0);
        // half-transparent source: halfway to the result
        let h = blend_px(crate::ADD, rgba(0.2, 0.2, 0.2, 1.0), rgba(0.4, 0.4, 0.4, 0.5));
        assert!((h.r - 0.4).abs() < 1e-6);
        let a = crate::create_image(2, 2);
        a.update_pixels(&[0xFF80_8080; 4]);
        let b = crate::create_image(2, 2);
        b.update_pixels(&[0xFFFF_FFFF; 4]);
        a.blend(b, 0, 0, 2, 2, 0, 0, 2, 2, crate::MULTIPLY);
        assert_eq!(a.load_pixels()[3], 0xFF80_8080);
    }
    #[test]
    fn save_and_reload() {
        let dir = std::env::temp_dir().join(format!("rm_imgext_{}", std::process::id()));
        let path = dir.join("x.png");
        let img = crate::create_image(3, 2);
        img.set(2, 1, rgba(0.0, 1.0, 0.0, 1.0));
        assert!(img.save(path.to_str().unwrap()));
        let back = crate::load_image(path.to_str().unwrap());
        assert_eq!(back.get(2, 1), rgba(0.0, 1.0, 0.0, 1.0));
        let _ = std::fs::remove_dir_all(dir);
    }
}
