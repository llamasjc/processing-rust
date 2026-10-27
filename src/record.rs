//! begin_record("drawing.svg") ... end_record(): what is drawn in between is
//! also written out as vector shapes, to an SVG or a PDF file (by the
//! file's extension, or begin_record(PDF, ...) / begin_record(SVG, ...)).
//!
//! Shapes are kept as outlines in canvas pixels, after the transform, with
//! their fill, stroke and stroke weight. Curves and ellipses become many short
//! lines. Text keeps its size and its font (embedded in the file). Images are
//! embedded, placed with the transform. Boxes and spheres become their
//! triangles, lit and sorted back to front.

use crate::Color;
use parking_lot::Mutex;

pub const PDF: &str = "pdf";
pub const SVG: &str = "svg";

enum Item {
    Path { pts: Vec<(f32, f32)>, closed: bool, fill: Option<Color>, stroke: Option<Color>, weight: f32 },
    Text { s: String, x: f32, y: f32, size: f32, fill: Color, font: Option<usize> },
    /// an image: RGBA rows, and where its unit square goes on the canvas
    /// (screen = a*u + c*v + e, b*u + d*v + f for u, v in 0..1)
    Image { w: usize, h: usize, rgba: Vec<u8>, m: [f32; 6] },
}

struct Rec { path: String, kind: String, items: Vec<Item>, shape: Option<(u8, Vec<(f32, f32)>, Vec<(f32, f32)>)>,
             /// fonts used by the text: the file and the font, in order of first use
             fonts: Vec<(std::path::PathBuf, std::sync::Arc<fontdue::Font>)> }

static REC: Mutex<Option<Rec>> = Mutex::new(None);

/// begin_record(kind, path): kind is PDF, SVG or "" (then the extension decides).
pub fn begin_record(kind: &str, path: &str) {
    *REC.lock() = Some(Rec { path: path.to_string(), kind: kind.to_lowercase(), items: Vec::new(), shape: None, fonts: Vec::new() });
}
/// end_record(): writes the file. False if it couldn't.
pub fn end_record() -> bool {
    let Some(r) = REC.lock().take() else { return false };
    let (w, h) = (crate::width(), crate::height());
    let pdf = r.kind.contains("pdf") || (r.kind.is_empty() && r.path.to_lowercase().ends_with(".pdf"));
    let bytes = if pdf { write_pdf(&r.items, &r.fonts, w, h) } else { write_svg(&r.items, &r.fonts, w, h).into_bytes() };
    match std::fs::write(&r.path, bytes) {
        Ok(()) => true,
        Err(e) => { eprintln!("endRecord(): can't write {}: {e}", r.path); false }
    }
}
/// begin_raw / end_raw: the same as begin_record / end_record here.
pub fn begin_raw(kind: &str, path: &str) { begin_record(kind, path) }
pub fn end_raw() -> bool { end_record() }
#[inline] pub fn beginRecord(kind: &str, path: &str) { begin_record(kind, path) }
#[inline] pub fn endRecord() -> bool { end_record() }
#[inline] pub fn beginRaw(kind: &str, path: &str) { begin_raw(kind, path) }
#[inline] pub fn endRaw() -> bool { end_raw() }

#[inline]
pub(crate) fn on() -> bool { REC.lock().is_some() }

fn to_screen(x: f32, y: f32) -> (f32, f32) { (crate::three::screen_x(x, y, 0.0), crate::three::screen_y(x, y, 0.0)) }

fn push(pts: &[(f32, f32)], closed: bool, filled: bool) {
    let (fill, stroke, weight) = crate::render::style_pub();
    let fill = if filled { fill } else { None };
    if fill.is_none() && stroke.is_none() { return; }
    let pts: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| to_screen(x, y)).collect();
    // the stroke weight grows with the transform, like the drawing
    let (a, b) = (to_screen(0.0, 0.0), to_screen(1.0, 0.0));
    let k = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    if let Some(r) = REC.lock().as_mut() { r.items.push(Item::Path { pts, closed, fill, stroke, weight: weight * k }); }
}

fn oval(cx: f32, cy: f32, w: f32, h: f32, a0: f32, a1: f32, n: usize) -> Vec<(f32, f32)> {
    (0..=n).map(|i| { let t = a0 + (a1 - a0) * i as f32 / n as f32; (cx + t.cos() * w / 2.0, cy + t.sin() * h / 2.0) }).collect()
}

pub(crate) fn rect(x: f32, y: f32, w: f32, h: f32) { if on() { push(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], true, true); } }
pub(crate) fn ellipse(cx: f32, cy: f32, w: f32, h: f32) {
    if on() { let mut p = oval(cx, cy, w, h, 0.0, std::f32::consts::TAU, 72); p.pop(); push(&p, true, true); }
}
/// mode: 0 or OPEN 1 (filled like a pie, outline open), CHORD 2, PIE 3
pub(crate) fn arc(cx: f32, cy: f32, w: f32, h: f32, start: f32, stop: f32, mode: u8) {
    if !on() { return; }
    let n = (((stop - start).abs() / std::f32::consts::TAU) * 72.0).ceil().max(2.0) as usize;
    let p = oval(cx, cy, w, h, start, stop, n);
    match mode {
        2 => push(&p, true, true),
        3 => { let mut q = vec![(cx, cy)]; q.extend(p); push(&q, true, true); }
        _ => {
            let mut q = vec![(cx, cy)]; q.extend(p.iter().copied());
            // filled like a pie, outlined only along the curve
            let (f, s, _) = crate::render::style_pub();
            if f.is_some() {
                let q: Vec<(f32, f32)> = q.iter().map(|&(x, y)| to_screen(x, y)).collect();
                if let Some(r) = REC.lock().as_mut() { r.items.push(Item::Path { pts: q, closed: true, fill: f, stroke: None, weight: 0.0 }); }
            }
            if s.is_some() { push(&p, false, false); }
        }
    }
}
pub(crate) fn poly(pts: &[(f32, f32)]) { if on() { push(pts, true, true); } }
pub(crate) fn line(x1: f32, y1: f32, x2: f32, y2: f32) { if on() { push(&[(x1, y1), (x2, y2)], false, false); } }
pub(crate) fn point(x: f32, y: f32) {
    if !on() { return; }
    let (_, stroke, weight) = crate::render::style_pub();
    let Some(c) = stroke else { return };
    let (sx, sy) = to_screen(x, y);
    let mut p = oval(sx, sy, weight.max(1.0), weight.max(1.0), 0.0, std::f32::consts::TAU, 16);
    p.pop();
    if let Some(r) = REC.lock().as_mut() { r.items.push(Item::Path { pts: p, closed: true, fill: Some(c), stroke: None, weight: 0.0 }); }
}
pub(crate) fn background(c: Color) {
    if let Some(r) = REC.lock().as_mut() {
        // a new background covers what came before
        r.items.clear();
        let (w, h) = (crate::width(), crate::height());
        r.items.push(Item::Path { pts: vec![(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)], closed: true, fill: Some(c), stroke: None, weight: 0.0 });
    }
}
pub(crate) fn text(s: &str, x: f32, y: f32) {
    if !on() { return; }
    let (fill, _, _) = crate::render::style_pub();
    let Some(fill) = fill else { return };
    let (_, _, size) = crate::render::text_state();
    let (sx, sy) = to_screen(x, y);
    let file = crate::render::current_font_file();
    if let Some(r) = REC.lock().as_mut() {
        let font = file.map(|(p, f)| match r.fonts.iter().position(|(q, _)| *q == p) {
            Some(i) => i,
            None => { r.fonts.push((p, f)); r.fonts.len() - 1 }
        });
        r.items.push(Item::Text { s: s.to_string(), x: sx, y: sy, size, fill, font });
    }
}

/// image(): the picture, placed with the transform.
pub(crate) fn image(id: u64, x: f32, y: f32, w: f32, h: f32) {
    if !on() { return; }
    let (iw, ih) = crate::render::image_size(id);
    let (iw, ih) = (iw as usize, ih as usize);
    if iw == 0 || ih == 0 { return; }
    let px = crate::render::image_pixels(id);
    if px.len() < iw * ih { return; }
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let rgba: Vec<u8> = px.iter().take(iw * ih).flat_map(|c| [b(c.r), b(c.g), b(c.b), b(c.a)]).collect();
    let p0 = to_screen(x, y);
    let p1 = to_screen(x + w, y);
    let p2 = to_screen(x, y + h);
    let m = [p1.0 - p0.0, p1.1 - p0.1, p2.0 - p0.0, p2.1 - p0.1, p0.0, p0.1];
    if let Some(r) = REC.lock().as_mut() { r.items.push(Item::Image { w: iw, h: ih, rgba, m }); }
}

/// A shape already in canvas pixels (3D triangles and edges from three.rs).
pub(crate) fn screen_path(pts: &[(f32, f32)], closed: bool, fill: Option<Color>, stroke: Option<Color>, weight: f32) {
    if let Some(r) = REC.lock().as_mut() { r.items.push(Item::Path { pts: pts.to_vec(), closed, fill, stroke, weight }); }
}

// begin_shape ... end_shape, followed alongside the renderer
pub(crate) fn shape_begin(kind: u8) { if let Some(r) = REC.lock().as_mut() { r.shape = Some((kind, Vec::new(), Vec::new())); } }
pub(crate) fn shape_vertex(x: f32, y: f32) {
    if let Some(r) = REC.lock().as_mut() { if let Some((_, pts, curve)) = r.shape.as_mut() { pts.push((x, y)); curve.clear(); } }
}
pub(crate) fn shape_bezier(cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
    if let Some(r) = REC.lock().as_mut() { if let Some((_, pts, _)) = r.shape.as_mut() {
        let Some(&(x0, y0)) = pts.last() else { pts.push((x, y)); return };
        for i in 1..=24 {
            let t = i as f32 / 24.0; let u = 1.0 - t;
            pts.push((u*u*u*x0 + 3.0*u*u*t*cx1 + 3.0*u*t*t*cx2 + t*t*t*x, u*u*u*y0 + 3.0*u*u*t*cy1 + 3.0*u*t*t*cy2 + t*t*t*y));
        }
    } }
}
pub(crate) fn shape_quadratic(cx: f32, cy: f32, x: f32, y: f32) {
    if let Some(r) = REC.lock().as_mut() { if let Some((_, pts, _)) = r.shape.as_mut() {
        let Some(&(x0, y0)) = pts.last() else { pts.push((x, y)); return };
        for i in 1..=16 { let t = i as f32 / 16.0; let u = 1.0 - t; pts.push((u*u*x0 + 2.0*u*t*cx + t*t*x, u*u*y0 + 2.0*u*t*cy + t*t*y)); }
    } }
}
pub(crate) fn shape_curve(x: f32, y: f32) {
    if let Some(r) = REC.lock().as_mut() { if let Some((_, pts, curve)) = r.shape.as_mut() {
        curve.push((x, y));
        let n = curve.len();
        if n < 4 { return; }
        let (p0, p1, p2, p3) = (curve[n-4], curve[n-3], curve[n-2], curve[n-1]);
        let start = if n == 4 { 0 } else { 1 };
        for i in start..=20 {
            let t = i as f32 / 20.0;
            pts.push((crate::curve_point(p0.0, p1.0, p2.0, p3.0, t), crate::curve_point(p0.1, p1.1, p2.1, p3.1, t)));
        }
    } }
}
pub(crate) fn shape_end(close: bool) {
    let shape = REC.lock().as_mut().and_then(|r| r.shape.take());
    let Some((kind, p, _)) = shape else { return };
    match kind {
        1 => for &(x, y) in &p { point(x, y); },
        2 => for c in p.chunks_exact(2) { push(c, false, false); },
        3 => for c in p.chunks_exact(3) { push(c, true, true); },
        4 => for i in 1..p.len().saturating_sub(1) { push(&[p[0], p[i], p[i + 1]], true, true); },
        5 => for i in 0..p.len().saturating_sub(2) { push(&[p[i], p[i + 1], p[i + 2]], true, true); },
        6 => for c in p.chunks_exact(4) { push(c, true, true); },
        7 => { let mut i = 0; while i + 3 < p.len() { push(&[p[i], p[i + 1], p[i + 3], p[i + 2]], true, true); i += 2; } }
        _ => if p.len() >= 2 { push(&p, close, true); },
    }
}

// ── writing ──────────────────────────────────────────────────────────────

fn hex(c: Color) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
}
fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }

fn write_svg(items: &[Item], fonts: &[(std::path::PathBuf, std::sync::Arc<fontdue::Font>)], w: f32, h: f32) -> String {
    let mut o = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n");
    // the fonts the text uses, inside the file so it looks the same anywhere
    if !fonts.is_empty() {
        o.push_str("<style>\n");
        for (i, (p, _)) in fonts.iter().enumerate() {
            if let Ok(data) = std::fs::read(p) {
                let kind = if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("otf")) { "opentype" } else { "truetype" };
                o.push_str(&format!("@font-face {{ font-family: \"f{i}\"; src: url(data:font/{kind};base64,{}) format(\"{kind}\"); }}\n", base64(&data)));
            }
        }
        o.push_str("</style>\n");
    }
    for it in items {
        match it {
            Item::Path { pts, closed, fill, stroke, weight } => {
                if pts.is_empty() { continue; }
                let mut d = String::new();
                for (i, (x, y)) in pts.iter().enumerate() { d.push_str(&format!("{}{x:.2} {y:.2} ", if i == 0 { "M" } else { "L" })); }
                if *closed { d.push('Z'); }
                let f = match fill { Some(c) => format!("fill=\"{}\"{}", hex(*c), if c.a < 1.0 { format!(" fill-opacity=\"{:.3}\"", c.a) } else { String::new() }), None => "fill=\"none\"".into() };
                let s = match stroke {
                    Some(c) => format!(" stroke=\"{}\" stroke-width=\"{weight:.2}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"{}", hex(*c),
                                       if c.a < 1.0 { format!(" stroke-opacity=\"{:.3}\"", c.a) } else { String::new() }),
                    None => String::new(),
                };
                o.push_str(&format!("<path d=\"{}\" {f}{s}/>\n", d.trim_end()));
            }
            Item::Text { s, x, y, size, fill, font } => {
                let family = match font { Some(i) => format!("f{i}, sans-serif"), None => "sans-serif".into() };
                o.push_str(&format!("<text x=\"{x:.2}\" y=\"{y:.2}\" font-family=\"{family}\" font-size=\"{size}\" fill=\"{}\">{}</text>\n", hex(*fill), esc(s)));
            }
            Item::Image { w, h, rgba, m } => {
                let png = png_bytes(*w, *h, rgba);
                o.push_str(&format!("<image width=\"1\" height=\"1\" preserveAspectRatio=\"none\" transform=\"matrix({:.4} {:.4} {:.4} {:.4} {:.4} {:.4})\" xlink:href=\"data:image/png;base64,{}\" href=\"data:image/png;base64,{}\"/>\n",
                    m[0], m[1], m[2], m[3], m[4], m[5], base64(&png), base64(&png)));
            }
        }
    }
    o.push_str("</svg>\n");
    o
}

fn write_pdf(items: &[Item], fonts: &[(std::path::PathBuf, std::sync::Arc<fontdue::Font>)], w: f32, h: f32) -> Vec<u8> {
    let mut images: Vec<(usize, usize, &Vec<u8>)> = Vec::new();
    // one page the size of the canvas, in points; y goes up in PDF
    let mut c = String::new();
    let rgb = |col: &Color| format!("{:.3} {:.3} {:.3}", col.r.clamp(0.0, 1.0), col.g.clamp(0.0, 1.0), col.b.clamp(0.0, 1.0));
    let mut alphas: Vec<(u32, u32)> = Vec::new(); // (fill, stroke) alpha in thousandths
    for it in items {
        match it {
            Item::Path { pts, closed, fill, stroke, weight } => {
                if pts.is_empty() { continue; }
                let fa = fill.map(|f| (f.a.clamp(0.0, 1.0) * 1000.0) as u32).unwrap_or(1000);
                let sa = stroke.map(|s| (s.a.clamp(0.0, 1.0) * 1000.0) as u32).unwrap_or(1000);
                let gs = match alphas.iter().position(|a| *a == (fa, sa)) { Some(i) => i, None => { alphas.push((fa, sa)); alphas.len() - 1 } };
                c.push_str(&format!("/GS{gs} gs\n"));
                if let Some(f) = fill { c.push_str(&format!("{} rg\n", rgb(f))); }
                if let Some(s) = stroke { c.push_str(&format!("{} RG {weight:.2} w 1 J 1 j\n", rgb(s))); }
                for (i, (x, y)) in pts.iter().enumerate() { c.push_str(&format!("{x:.2} {:.2} {}\n", h - y, if i == 0 { "m" } else { "l" })); }
                if *closed { c.push_str("h\n"); }
                c.push_str(match (fill.is_some(), stroke.is_some()) { (true, true) => "B\n", (true, false) => "f\n", (false, true) => "S\n", _ => "n\n" });
            }
            Item::Text { s, x, y, size, fill, font } => {
                // WinAnsi: Latin-1 characters, the rest as '?'
                let mut t = String::new();
                for ch in s.chars() {
                    let b = if (ch as u32) < 256 { ch as u32 as u8 } else { b'?' };
                    match b { b'\\' | b'(' | b')' => { t.push('\\'); t.push(b as char); } 32..=126 => t.push(b as char), _ => t.push_str(&format!("\\{b:03o}")) }
                }
                let f = match font { Some(i) => format!("/F{}", i + 2), None => "/F1".into() };
                c.push_str(&format!("{} rg BT {f} {size} Tf {x:.2} {:.2} Td ({t}) Tj ET\n", rgb(fill), h - y));
            }
            Item::Image { w: iw, h: ih, rgba, m } => {
                // the unit square's top row is v = 1 in PDF, and y goes up
                let (a, b, cc, d) = (m[0], -m[1], -m[2], m[3]);
                let (e, f) = (m[4] + m[2], h - (m[5] + m[3]));
                images.push((*iw, *ih, rgba));
                c.push_str(&format!("q {a:.4} {b:.4} {cc:.4} {d:.4} {e:.4} {f:.4} cm /Im{} Do Q\n", images.len() - 1));
            }
        }
    }
    let mut ext = String::new();
    for (i, (fa, sa)) in alphas.iter().enumerate() {
        ext.push_str(&format!("/GS{i} << /ca {:.3} /CA {:.3} >> ", *fa as f32 / 1000.0, *sa as f32 / 1000.0));
    }
    // objects 1-5 as before, then per font: font, descriptor, file; per image: image, alpha
    let mut extra: Vec<Vec<u8>> = Vec::new();
    let mut font_res = String::from("/F1 5 0 R ");
    for (i, (p, f)) in fonts.iter().enumerate() {
        let base = 6 + extra.len();
        let data = std::fs::read(p).unwrap_or_default();
        let name: String = f.name().unwrap_or("Font").chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let name = if name.is_empty() { format!("Font{i}") } else { name };
        let widths: Vec<String> = (32u32..=255).map(|code| {
            let ch = char::from_u32(code).unwrap_or(' ');
            format!("{:.0}", f.metrics(ch, 1000.0).advance_width)
        }).collect();
        let m = f.horizontal_line_metrics(1000.0);
        let (asc, desc) = m.map(|m| (m.ascent, m.descent)).unwrap_or((800.0, -200.0));
        extra.push(format!("<< /Type /Font /Subtype /TrueType /BaseFont /{name} /FirstChar 32 /LastChar 255 /Widths [{}] /FontDescriptor {} 0 R /Encoding /WinAnsiEncoding >>",
            widths.join(" "), base + 1).into_bytes());
        extra.push(format!("<< /Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [-500 {desc:.0} 1500 {asc:.0}] /ItalicAngle 0 /Ascent {asc:.0} /Descent {desc:.0} /CapHeight {asc:.0} /StemV 80 /FontFile2 {} 0 R >>", base + 2).into_bytes());
        let mut file = format!("<< /Length {} /Length1 {} >>\nstream\n", data.len(), data.len()).into_bytes();
        file.extend_from_slice(&data);
        file.extend_from_slice(b"\nendstream");
        extra.push(file);
        font_res.push_str(&format!("/F{} {base} 0 R ", i + 2));
    }
    let mut xobj = String::new();
    for (i, (iw, ih, rgba)) in images.iter().enumerate() {
        let base = 6 + extra.len();
        let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        let alpha: Vec<u8> = rgba.chunks_exact(4).map(|p| p[3]).collect();
        let mut img = format!("<< /Type /XObject /Subtype /Image /Width {iw} /Height {ih} /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {} 0 R /Length {} >>\nstream\n", base + 1, rgb.len()).into_bytes();
        img.extend_from_slice(&rgb);
        img.extend_from_slice(b"\nendstream");
        extra.push(img);
        let mut sm = format!("<< /Type /XObject /Subtype /Image /Width {iw} /Height {ih} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} >>\nstream\n", alpha.len()).into_bytes();
        sm.extend_from_slice(&alpha);
        sm.extend_from_slice(b"\nendstream");
        extra.push(sm);
        xobj.push_str(&format!("/Im{i} {base} 0 R "));
    }
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 4 0 R /Resources << /Font << {font_res}>> /XObject << {xobj}>> /ExtGState << {ext}>> >> >>").into_bytes(),
        format!("<< /Length {} >>\nstream\n{c}endstream", c.len()).into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    objs.extend(extra);
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets { out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes()); }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(T[(n >> 18) as usize & 63] as char);
        o.push(T[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    o
}

fn png_bytes(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut wr) = enc.write_header() { let _ = wr.write_image_data(rgba); }
    }
    out
}
