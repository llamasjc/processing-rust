//! createShape(): a shape built once in code and drawn many times.
//!
//!   let mut star = create_shape();
//!   star.begin_shape();
//!   star.fill(color_rgb(255, 200, 0));
//!   star.vertex(0, -50); ...
//!   star.end_shape_mode(CLOSE);
//!   shape(&star, 100, 100);
//!
//! Style set on the shape (fill, stroke, stroke_weight) belongs to it; a shape
//! without any is drawn in the sketch's current style. translate, rotate and
//! scale on the shape turn the shape itself and add up.

use crate::{Color, Num, Paint};

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Begin(u8),
    Vertex(f32, f32),
    Bezier([f32; 6]),
    Quadratic([f32; 4]),
    Curve(f32, f32),
    End(bool),
    Rect([f32; 4]),
    Ellipse([f32; 4]),
    Line([f32; 4]),
    Triangle([f32; 6]),
    Quad([f32; 8]),
    /// x, y, z, u, v; which of z and u, v were given (1 = z, 2 = u, v)
    Full([f32; 5], u8),
    BeginContour,
    EndContour,
    Box([f32; 3]),
    Sphere(f32),
    /// attrib(name, values) replayed before the next vertex.
    Attrib(String, Vec<f32>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Move { T(f32, f32), R(f32), S(f32, f32), T3([f32; 3]), RX(f32), RY(f32), S3([f32; 3]), M2([f32; 6]), M3([f32; 16]) }

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PShape {
    steps: Vec<Step>,
    fill: Option<Option<Color>>,
    stroke: Option<Option<Color>>,
    weight: Option<f32>,
    moves: Vec<Move>,
    children: Vec<PShape>,
    visible: bool,
    texture: Option<u64>,
    name: String,
    /// Processing's stroke cap / join numbers, applied when drawn.
    cap: Option<u32>,
    join: Option<u32>,
    /// Kept for get_tint, materials and normals for their getters; not drawn.
    tint: Option<Option<Color>>,
    material: Material,
    normals: std::collections::BTreeMap<usize, [f32; 3]>,
    /// set_3d overrides what is worked out from the vertices.
    three: Option<bool>,
    /// disable_style(): use the sketch's style instead of the shape's own.
    style_off: bool,
    /// bezier_detail, curve_detail and curve_tightness used while drawing.
    bezier_detail: Option<u32>,
    curve_detail: Option<u32>,
    curve_tightness: Option<f32>,
    /// texture_mode used while drawing (IMAGE 0, NORMAL 1).
    texture_mode: Option<u8>,
    /// color_mode for the shape's number color setters: mode and maxes.
    color_mode: Option<(u32, [f32; 4])>,
    /// set_kind / set_family override what is worked out from the steps.
    kind: Option<u8>,
    family: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Default)]
struct Material { ambient: Option<Color>, emissive: Option<Color>, specular: Option<Color>, shininess: Option<f32> }

/// createShape(): an empty shape; fill it with begin_shape / vertex / end_shape.
pub fn create_shape() -> PShape { PShape { visible: true, ..Default::default() } }
/// createShape(GROUP): holds other shapes, added with add_child.
pub fn create_shape_group() -> PShape { create_shape() }
pub fn create_shape_rect(x: impl Num, y: impl Num, w: impl Num, h: impl Num) -> PShape {
    let mut s = create_shape(); s.steps.push(Step::Rect([x.f(), y.f(), w.f(), h.f()])); s
}
pub fn create_shape_ellipse(x: impl Num, y: impl Num, w: impl Num, h: impl Num) -> PShape {
    let mut s = create_shape(); s.steps.push(Step::Ellipse([x.f(), y.f(), w.f(), h.f()])); s
}
pub fn create_shape_line(x1: impl Num, y1: impl Num, x2: impl Num, y2: impl Num) -> PShape {
    let mut s = create_shape(); s.steps.push(Step::Line([x1.f(), y1.f(), x2.f(), y2.f()])); s
}
pub fn create_shape_triangle(x1: impl Num, y1: impl Num, x2: impl Num, y2: impl Num, x3: impl Num, y3: impl Num) -> PShape {
    let mut s = create_shape(); s.steps.push(Step::Triangle([x1.f(), y1.f(), x2.f(), y2.f(), x3.f(), y3.f()])); s
}
#[allow(clippy::too_many_arguments)]
pub fn create_shape_quad(x1: impl Num, y1: impl Num, x2: impl Num, y2: impl Num,
                         x3: impl Num, y3: impl Num, x4: impl Num, y4: impl Num) -> PShape {
    let mut s = create_shape();
    s.steps.push(Step::Quad([x1.f(), y1.f(), x2.f(), y2.f(), x3.f(), y3.f(), x4.f(), y4.f()]));
    s
}
#[inline] pub fn createShape() -> PShape { create_shape() }
/// createShape(BOX, w, h, d)
pub fn create_shape_box(w: impl Num, h: impl Num, d: impl Num) -> PShape {
    let mut s = create_shape(); s.steps.push(Step::Box([w.f(), h.f(), d.f()])); s
}
/// createShape(SPHERE, r)
pub fn create_shape_sphere(r: impl Num) -> PShape {
    let mut s = create_shape(); s.steps.push(Step::Sphere(r.f())); s
}

impl PShape {
    /// What a primitive shape was made with: create_shape_rect(0, 0, 80, 40)
    /// gives [0, 0, 80, 40]; empty for other shapes.
    pub fn get_params(&self) -> Vec<f32> {
        match self.steps.first() {
            Some(Step::Rect(p) | Step::Ellipse(p) | Step::Line(p)) => p.to_vec(),
            Some(Step::Triangle(p)) => p.to_vec(),
            Some(Step::Quad(p)) => p.to_vec(),
            Some(Step::Box(p)) => p.to_vec(),
            Some(Step::Sphere(r)) => vec![*r],
            _ => Vec::new(),
        }
    }
    /// One of get_params(), or 0 past the end.
    pub fn get_param(&self, i: usize) -> f32 { self.get_params().get(i).copied().unwrap_or(0.0) }
    #[inline] pub fn getParams(&self) -> Vec<f32> { self.get_params() }
    #[inline] pub fn getParam(&self, i: usize) -> f32 { self.get_param(i) }

    // ── building ─────────────────────────────────────────────────────────
    pub fn begin_shape(&mut self) -> &mut Self { self.begin_shape_kind(0) }
    /// begin_shape_kind(TRIANGLES) and friends.
    pub fn begin_shape_kind(&mut self, kind: u8) -> &mut Self {
        self.steps.retain(|s| !matches!(s, Step::Begin(_) | Step::Vertex(..) | Step::Bezier(_)
            | Step::Quadratic(_) | Step::Curve(..) | Step::End(_)));
        self.steps.push(Step::Begin(kind));
        self
    }
    pub fn vertex(&mut self, x: impl Num, y: impl Num) -> &mut Self { self.steps.push(Step::Vertex(x.f(), y.f())); self }
    pub fn bezier_vertex(&mut self, cx1: impl Num, cy1: impl Num, cx2: impl Num, cy2: impl Num, x: impl Num, y: impl Num) -> &mut Self {
        self.steps.push(Step::Bezier([cx1.f(), cy1.f(), cx2.f(), cy2.f(), x.f(), y.f()])); self
    }
    pub fn quadratic_vertex(&mut self, cx: impl Num, cy: impl Num, x: impl Num, y: impl Num) -> &mut Self {
        self.steps.push(Step::Quadratic([cx.f(), cy.f(), x.f(), y.f()])); self
    }
    pub fn curve_vertex(&mut self, x: impl Num, y: impl Num) -> &mut Self { self.steps.push(Step::Curve(x.f(), y.f())); self }
    pub fn vertex_3d(&mut self, x: impl Num, y: impl Num, z: impl Num) -> &mut Self {
        self.steps.push(Step::Full([x.f(), y.f(), z.f(), 0.0, 0.0], 1)); self
    }
    pub fn vertex_uv(&mut self, x: impl Num, y: impl Num, u: impl Num, v: impl Num) -> &mut Self {
        self.steps.push(Step::Full([x.f(), y.f(), 0.0, u.f(), v.f()], 2)); self
    }
    pub fn vertex_3d_uv(&mut self, x: impl Num, y: impl Num, z: impl Num, u: impl Num, v: impl Num) -> &mut Self {
        self.steps.push(Step::Full([x.f(), y.f(), z.f(), u.f(), v.f()], 3)); self
    }
    pub fn begin_contour(&mut self) -> &mut Self { self.steps.push(Step::BeginContour); self }
    pub fn end_contour(&mut self) -> &mut Self { self.steps.push(Step::EndContour); self }
    /// texture(img): paints the shape with an image (give vertices u, v).
    pub fn texture(&mut self, img: impl crate::ImageSource) -> &mut Self { self.texture = Some(img.image_id()); self }
    pub fn set_texture(&mut self, img: impl crate::ImageSource) -> &mut Self { self.texture(img) }
    pub fn no_texture(&mut self) -> &mut Self { self.texture = None; self }
    pub fn normal(&mut self, _x: impl Num, _y: impl Num, _z: impl Num) -> &mut Self { self }
    pub fn end_shape(&mut self) -> &mut Self { self.steps.push(Step::End(false)); self }
    /// end_shape_mode(CLOSE) joins the last point back to the first.
    pub fn end_shape_mode(&mut self, mode: u8) -> &mut Self { self.steps.push(Step::End(mode == crate::CLOSE)); self }
    pub fn add_child(&mut self, child: PShape) -> &mut Self { self.children.push(child); self }
    pub fn child_count(&self) -> usize { self.children.len() }
    pub fn get_child(&mut self, i: usize) -> Option<&mut PShape> { self.children.get_mut(i) }
    /// get_child_named("name"): a child given that name with set_name.
    pub fn get_child_named(&mut self, name: &str) -> Option<&mut PShape> { self.children.iter_mut().find(|c| c.name == name) }
    pub fn remove_child(&mut self, i: usize) -> &mut Self { if i < self.children.len() { self.children.remove(i); } self }
    pub fn add_child_at(&mut self, child: PShape, i: usize) -> &mut Self { let i = i.min(self.children.len()); self.children.insert(i, child); self }
    pub fn set_name(&mut self, name: &str) -> &mut Self { self.name = name.to_string(); self }
    pub fn get_name(&self) -> &str { &self.name }
    pub fn get_child_count(&self) -> usize { self.child_count() }

    // ── style ────────────────────────────────────────────────────────────
    pub fn fill(&mut self, c: impl Paint) -> &mut Self { self.fill = Some(Some(c.paint())); self }
    pub fn fill_ga(&mut self, g: impl Num, a: impl Num) -> &mut Self { self.fill = Some(Some(self.in_color_mode(|| crate::color_ga(g, a)))); self }
    pub fn fill_rgb(&mut self, r: impl Num, g: impl Num, b: impl Num) -> &mut Self { self.fill = Some(Some(self.in_color_mode(|| crate::color_rgb(r, g, b)))); self }
    pub fn fill_rgba(&mut self, r: impl Num, g: impl Num, b: impl Num, a: impl Num) -> &mut Self {
        self.fill = Some(Some(self.in_color_mode(|| crate::color_rgba(r, g, b, a)))); self
    }
    pub fn no_fill(&mut self) -> &mut Self { self.fill = Some(None); self }
    pub fn stroke(&mut self, c: impl Paint) -> &mut Self { self.stroke = Some(Some(c.paint())); self }
    pub fn stroke_ga(&mut self, g: impl Num, a: impl Num) -> &mut Self { self.stroke = Some(Some(self.in_color_mode(|| crate::color_ga(g, a)))); self }
    pub fn stroke_rgb(&mut self, r: impl Num, g: impl Num, b: impl Num) -> &mut Self { self.stroke = Some(Some(self.in_color_mode(|| crate::color_rgb(r, g, b)))); self }
    pub fn stroke_rgba(&mut self, r: impl Num, g: impl Num, b: impl Num, a: impl Num) -> &mut Self {
        self.stroke = Some(Some(self.in_color_mode(|| crate::color_rgba(r, g, b, a)))); self
    }
    pub fn no_stroke(&mut self) -> &mut Self { self.stroke = Some(None); self }
    pub fn stroke_weight(&mut self, w: impl Num) -> &mut Self { self.weight = Some(w.f()); self }
    /// set_fill / set_stroke: the same as fill / stroke, for changing a shape later.
    pub fn set_fill(&mut self, c: impl Paint) -> &mut Self { self.fill(c) }
    pub fn set_stroke(&mut self, c: impl Paint) -> &mut Self { self.stroke(c) }
    pub fn set_stroke_weight(&mut self, w: impl Num) -> &mut Self { self.stroke_weight(w) }
    /// disable_style(): draw with the sketch's fill and stroke instead.
    /// Its own style is kept and comes back with enable_style. Children too.
    pub fn disable_style(&mut self) -> &mut Self { self.set_style(false); self }
    /// enable_style(): draw with the shape's own style again.
    pub fn enable_style(&mut self) -> &mut Self { self.set_style(true); self }
    fn set_style(&mut self, on: bool) { self.style_off = !on; for c in &mut self.children { c.set_style(on); } }
    /// Is the shape drawn with its own style?
    pub fn is_style_enabled(&self) -> bool { !self.style_off }
    pub fn set_visible(&mut self, v: bool) -> &mut Self { self.visible = v; self }
    pub fn is_visible(&self) -> bool { self.visible }

    // ── moving ───────────────────────────────────────────────────────────
    pub fn translate(&mut self, x: impl Num, y: impl Num) -> &mut Self { self.moves.push(Move::T(x.f(), y.f())); self }
    pub fn rotate(&mut self, a: impl Num) -> &mut Self { self.moves.push(Move::R(a.f())); self }
    pub fn scale(&mut self, s: impl Num) -> &mut Self { let s = s.f(); self.moves.push(Move::S(s, s)); self }
    pub fn scale_xy(&mut self, x: impl Num, y: impl Num) -> &mut Self { self.moves.push(Move::S(x.f(), y.f())); self }
    pub fn scale_3d(&mut self, x: impl Num, y: impl Num, z: impl Num) -> &mut Self { self.moves.push(Move::S3([x.f(), y.f(), z.f()])); self }
    pub fn translate_3d(&mut self, x: impl Num, y: impl Num, z: impl Num) -> &mut Self { self.moves.push(Move::T3([x.f(), y.f(), z.f()])); self }
    pub fn rotate_x(&mut self, a: impl Num) -> &mut Self { self.moves.push(Move::RX(a.f())); self }
    pub fn rotate_y(&mut self, a: impl Num) -> &mut Self { self.moves.push(Move::RY(a.f())); self }
    pub fn rotate_z(&mut self, a: impl Num) -> &mut Self { self.rotate(a) }
    /// reset_matrix(): forget translate / rotate / scale.
    pub fn reset_matrix(&mut self) -> &mut Self { self.moves.clear(); self }
    /// apply_matrix(a, b, c, d, e, f): the 2D matrix [a b c; d e f], added to the shape's moves.
    pub fn apply_matrix(&mut self, a: impl Num, b: impl Num, c: impl Num, d: impl Num, e: impl Num, f: impl Num) -> &mut Self {
        self.moves.push(Move::M2([a.f(), b.f(), c.f(), d.f(), e.f(), f.f()])); self
    }
    /// apply_matrix_3d: 16 numbers, row by row as in Processing.
    pub fn apply_matrix_3d(&mut self, m: [f32; 16]) -> &mut Self { self.moves.push(Move::M3(m)); self }

    // ── vertices ─────────────────────────────────────────────────────────
    fn points(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.steps.iter().filter_map(|s| match s {
            Step::Vertex(x, y) | Step::Curve(x, y) => Some((*x, *y)),
            Step::Bezier(v) => Some((v[4], v[5])),
            Step::Quadratic(v) => Some((v[2], v[3])),
            Step::Full(v, _) => Some((v[0], v[1])),
            _ => None,
        })
    }
    /// get_vertex_z(i): the z of a vertex given one (0 otherwise).
    pub fn get_vertex_z(&self, i: usize) -> f32 {
        self.steps.iter().filter(|s| matches!(s, Step::Vertex(..) | Step::Curve(..) | Step::Bezier(_) | Step::Quadratic(_) | Step::Full(..)))
            .nth(i).map(|s| if let Step::Full(v, _) = s { v[2] } else { 0.0 }).unwrap_or(0.0)
    }
    pub fn get_vertex_x(&self, i: usize) -> f32 { self.get_vertex(i).0 }
    pub fn get_vertex_y(&self, i: usize) -> f32 { self.get_vertex(i).1 }
    pub fn get_vertex_count(&self) -> usize { self.vertex_count() }
    /// The shape's extent from its vertices (0 for shapes made of rect, ellipse...).
    pub fn width(&self) -> f32 { let (a, b) = self.span(|p| p.0); b - a }
    pub fn height(&self) -> f32 { let (a, b) = self.span(|p| p.1); b - a }
    fn span(&self, f: impl Fn((f32, f32)) -> f32) -> (f32, f32) {
        let mut it = self.points().map(f);
        let Some(first) = it.next() else { return (0.0, 0.0) };
        it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)))
    }
    pub fn vertex_count(&self) -> usize { self.points().count() }
    /// get_vertex(i): (x, y), or (0, 0) past the end.
    pub fn get_vertex(&self, i: usize) -> (f32, f32) { self.points().nth(i).unwrap_or((0.0, 0.0)) }
    pub fn set_vertex(&mut self, i: usize, x: impl Num, y: impl Num) -> &mut Self {
        let (x, y) = (x.f(), y.f());
        let mut n = 0;
        for s in self.steps.iter_mut() {
            let p = match s {
                Step::Vertex(a, b) | Step::Curve(a, b) => Some((a, b)),
                Step::Bezier(v) => { let (l, r) = v.split_at_mut(5); Some((&mut l[4], &mut r[0])) }
                Step::Quadratic(v) => { let (l, r) = v.split_at_mut(3); Some((&mut l[2], &mut r[0])) }
                Step::Full(v, _) => { let (l, r) = v.split_at_mut(1); Some((&mut l[0], &mut r[0])) }
                _ => None,
            };
            if let Some((a, b)) = p {
                if n == i { *a = x; *b = y; break; }
                n += 1;
            }
        }
        self
    }

    // ── drawing ──────────────────────────────────────────────────────────
    pub(crate) fn draw(&self) {
        if !self.visible { return; }
        crate::push_matrix();
        crate::push_style();
        for m in &self.moves {
            match *m {
                Move::T(x, y) => crate::translate(x, y),
                Move::R(a) => crate::rotate(a),
                Move::S(x, y) => crate::scale_xy(x, y),
                Move::T3(v) => crate::translate_3d(v[0], v[1], v[2]),
                Move::RX(a) => crate::rotate_x(a),
                Move::RY(a) => crate::rotate_y(a),
                Move::S3(v) => crate::scale_3d(v[0], v[1], v[2]),
                Move::M2(m) => crate::apply_matrix(m[0], m[1], m[2], m[3], m[4], m[5]),
                Move::M3(m) => crate::apply_matrix_3d(m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7],
                                                      m[8], m[9], m[10], m[11], m[12], m[13], m[14], m[15]),
            }
        }
        // settings the shape carries, put back after it is drawn
        use std::sync::atomic::Ordering::Relaxed;
        let old_bd = self.bezier_detail.map(|n| crate::BEZIER_DETAIL.swap(n, Relaxed));
        let old_cd = self.curve_detail.map(|n| crate::CURVE_DETAIL.swap(n, Relaxed));
        let old_ct = self.curve_tightness.map(|t| crate::CURVE_TIGHTNESS.swap(t.to_bits(), Relaxed));
        let old_tm = self.texture_mode.map(|m| { let o = crate::render::texture_mode(); crate::render::set_texture_mode(m); o });
        if !self.style_off { self.apply_style(); }
        self.draw_steps();
        for c in &self.children { c.draw(); }
        if let Some(n) = old_bd { crate::BEZIER_DETAIL.store(n, Relaxed); }
        if let Some(n) = old_cd { crate::CURVE_DETAIL.store(n, Relaxed); }
        if let Some(t) = old_ct { crate::CURVE_TIGHTNESS.store(t, Relaxed); }
        if let Some(m) = old_tm { crate::render::set_texture_mode(m); }
        crate::pop_style();
        crate::pop_matrix();
    }
    fn apply_style(&self) {
        match self.fill { Some(Some(c)) => crate::fill_color(c), Some(None) => crate::no_fill(), None => {} }
        match self.stroke { Some(Some(c)) => crate::stroke_color(c), Some(None) => crate::no_stroke(), None => {} }
        if let Some(w) = self.weight { crate::stroke_weight(w); }
        if let Some(c) = self.cap { crate::stroke_cap(c); }
        if let Some(j) = self.join { crate::stroke_join(j); }
    }
    fn draw_steps(&self) {
        let mut open = false;
        for s in &self.steps {
            match *s {
                Step::Begin(k) => {
                    crate::begin_shape_kind(k); open = true;
                    if let Some(t) = self.texture { crate::texture(t); }
                }
                Step::Full(v, f) => match f {
                    1 => crate::vertex_3d(v[0], v[1], v[2]),
                    2 => crate::vertex_uv(v[0], v[1], v[3], v[4]),
                    _ => crate::vertex_3d_uv(v[0], v[1], v[2], v[3], v[4]),
                },
                Step::BeginContour => crate::begin_contour(),
                Step::EndContour => crate::end_contour(),
                Step::Box(v) => crate::box_3d(v[0], v[1], v[2]),
                Step::Sphere(r) => crate::sphere(r),
                Step::Vertex(x, y) => crate::vertex(x, y),
                Step::Bezier(v) => crate::bezier_vertex(v[0], v[1], v[2], v[3], v[4], v[5]),
                Step::Quadratic(v) => crate::quadratic_vertex(v[0], v[1], v[2], v[3]),
                Step::Curve(x, y) => crate::curve_vertex(x, y),
                Step::End(close) => { crate::end_shape_mode(if close { crate::CLOSE } else { 0 }); open = false; }
                Step::Rect(v) => crate::rect(v[0], v[1], v[2], v[3]),
                Step::Ellipse(v) => crate::ellipse(v[0], v[1], v[2], v[3]),
                Step::Line(v) => crate::line(v[0], v[1], v[2], v[3]),
                Step::Triangle(v) => crate::triangle(v[0], v[1], v[2], v[3], v[4], v[5]),
                Step::Quad(v) => crate::quad(v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]),
                Step::Attrib(ref name, ref v) => crate::render::set_attrib(name, v.clone()),
            }
        }
        // a shape left open (no end_shape) is still drawn, as an open outline
        if open { crate::end_shape(); }
    }
}

impl PShape {
    // ── more of Processing's PShape ──────────────────────────────────────
    fn is_vertex(s: &Step) -> bool { matches!(s, Step::Vertex(..) | Step::Curve(..) | Step::Bezier(_) | Step::Quadratic(_) | Step::Full(..)) }
    /// Every corner the shape reaches, primitives and children included (x, y, z).
    fn extent_points(&self, out: &mut Vec<[f32; 3]>) {
        for s in &self.steps {
            match *s {
                Step::Vertex(x, y) | Step::Curve(x, y) => out.push([x, y, 0.0]),
                Step::Bezier(v) => out.push([v[4], v[5], 0.0]),
                Step::Quadratic(v) => out.push([v[2], v[3], 0.0]),
                Step::Full(v, f) => out.push([v[0], v[1], if f & 1 != 0 { v[2] } else { 0.0 }]),
                Step::Rect(v) => { out.push([v[0], v[1], 0.0]); out.push([v[0] + v[2], v[1] + v[3], 0.0]); }
                Step::Ellipse(v) => { out.push([v[0] - v[2] / 2.0, v[1] - v[3] / 2.0, 0.0]); out.push([v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, 0.0]); }
                Step::Line(v) => { out.push([v[0], v[1], 0.0]); out.push([v[2], v[3], 0.0]); }
                Step::Triangle(v) => for p in v.chunks(2) { out.push([p[0], p[1], 0.0]); },
                Step::Quad(v) => for p in v.chunks(2) { out.push([p[0], p[1], 0.0]); },
                Step::Box(v) => { out.push([-v[0] / 2.0, -v[1] / 2.0, -v[2] / 2.0]); out.push([v[0] / 2.0, v[1] / 2.0, v[2] / 2.0]); }
                Step::Sphere(r) => { out.push([-r, -r, -r]); out.push([r, r, r]); }
                _ => {}
            }
        }
        for c in &self.children { c.extent_points(out); }
    }
    fn extent(&self, axis: usize) -> f32 {
        let mut pts = Vec::new();
        self.extent_points(&mut pts);
        let mut it = pts.iter().map(|p| p[axis]);
        let Some(first) = it.next() else { return 0.0 };
        let (lo, hi) = it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
        hi - lo
    }
    /// The size of the box around everything in the shape (children included).
    pub fn get_width(&self) -> f32 { self.extent(0) }
    pub fn get_height(&self) -> f32 { self.extent(1) }
    pub fn get_depth(&self) -> f32 { self.extent(2) }

    /// The shape's own fill (the same for every vertex here); None when it
    /// uses the sketch's.
    pub fn get_fill(&self, _i: usize) -> Option<Color> { self.fill.flatten() }
    pub fn get_stroke(&self, _i: usize) -> Option<Color> { self.stroke.flatten() }
    /// Its own stroke weight, or 1.
    pub fn get_stroke_weight(&self) -> f32 { self.weight.unwrap_or(1.0) }
    /// ROUND, SQUARE or PROJECT, used when the shape is drawn.
    pub fn set_stroke_cap(&mut self, cap: u32) -> &mut Self { self.cap = Some(cap); self }
    /// MITER, BEVEL or ROUND, used when the shape is drawn.
    pub fn set_stroke_join(&mut self, join: u32) -> &mut Self { self.join = Some(join); self }
    pub fn stroke_cap(&mut self, cap: u32) -> &mut Self { self.set_stroke_cap(cap) }
    pub fn stroke_join(&mut self, join: u32) -> &mut Self { self.set_stroke_join(join) }

    /// tint() on a shape: kept so get_tint can read it back.
    pub fn tint(&mut self, c: impl crate::Paint) -> &mut Self { self.tint = Some(Some(c.paint())); self }
    pub fn set_tint(&mut self, c: impl crate::Paint) -> &mut Self { self.tint(c) }
    pub fn no_tint(&mut self) -> &mut Self { self.tint = Some(None); self }
    pub fn get_tint(&self) -> Option<Color> { self.tint.flatten() }

    pub fn get_children(&self) -> &[PShape] { &self.children }
    /// Where a child is among the children (compared by contents).
    pub fn get_child_index(&self, who: &PShape) -> Option<usize> { self.children.iter().position(|c| c == who) }
    /// A shape with that name, searched through children and their children.
    pub fn find_child(&self, name: &str) -> Option<&PShape> {
        for c in &self.children {
            if c.name == name { return Some(c); }
            if let Some(f) = c.find_child(name) { return Some(f); }
        }
        None
    }
    /// add_name(name, shape): adds the shape as a child under that name.
    pub fn add_name(&mut self, name: &str, mut shape: PShape) -> &mut Self { shape.name = name.to_string(); self.add_child(shape) }

    /// Is the shape 3D: a vertex with z, a box or sphere, or a 3D move. set_3d overrides.
    pub fn is_3d(&self) -> bool {
        if let Some(t) = self.three { return t; }
        self.steps.iter().any(|s| matches!(s, Step::Full(_, 1 | 3) | Step::Box(_) | Step::Sphere(_)))
            || self.moves.iter().any(|m| matches!(m, Move::T3(_) | Move::RX(_) | Move::RY(_) | Move::S3(_) | Move::M3(_)))
            || self.children.iter().any(|c| c.is_3d())
    }
    pub fn is_2d(&self) -> bool { !self.is_3d() }
    pub fn set_3d(&mut self, on: bool) -> &mut Self { self.three = Some(on); self }
    /// Was it ended with end_shape_mode(CLOSE)?
    pub fn is_closed(&self) -> bool { self.steps.iter().any(|s| matches!(s, Step::End(true))) }

    /// Is the point inside the outline made by the vertices (even-odd rule)?
    pub fn contains(&self, x: impl Num, y: impl Num) -> bool {
        let (x, y) = (x.f(), y.f());
        let pts: Vec<(f32, f32)> = self.points().collect();
        if pts.len() < 3 { return false; }
        let mut inside = false;
        let mut j = pts.len() - 1;
        for i in 0..pts.len() {
            let ((xi, yi), (xj, yj)) = (pts[i], pts[j]);
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi { inside = !inside; }
            j = i;
        }
        inside
    }

    /// The kind given to begin_shape_kind, or RECT, ELLIPSE, BOX... for
    /// primitives (POLYGON for a plain shape, GROUP for an empty group).
    pub fn get_kind(&self) -> u8 {
        if let Some(k) = self.kind { return k; }
        for s in &self.steps {
            return match s {
                Step::Begin(k) => *k,
                Step::Rect(_) => crate::constants::RECT,
                Step::Ellipse(_) => crate::constants::ELLIPSE,
                Step::Line(_) => crate::constants::LINE,
                Step::Triangle(_) => crate::constants::TRIANGLE,
                Step::Quad(_) => crate::constants::QUAD,
                Step::Box(_) => crate::constants::BOX,
                Step::Sphere(_) => crate::constants::SPHERE,
                _ => continue,
            };
        }
        if self.steps.is_empty() && !self.children.is_empty() { crate::constants::GROUP } else { crate::POLYGON }
    }
    /// GROUP (children only), PRIMITIVE (rect, ellipse, box...) or GEOMETRY (vertices).
    pub fn get_family(&self) -> u8 {
        if let Some(f) = self.family { return f; }
        if self.steps.iter().any(|s| matches!(s, Step::Rect(_) | Step::Ellipse(_) | Step::Line(_) | Step::Triangle(_)
            | Step::Quad(_) | Step::Box(_) | Step::Sphere(_))) { crate::constants::PRIMITIVE }
        else if self.steps.is_empty() && !self.children.is_empty() { crate::constants::GROUP }
        else { crate::constants::GEOMETRY }
    }

    /// set_kind / set_family: stored and returned by get_kind / get_family; drawing is unchanged.
    pub fn set_kind(&mut self, k: u8) -> &mut Self { self.kind = Some(k); self }
    pub fn set_family(&mut self, f: u8) -> &mut Self { self.family = Some(f); self }

    /// Processing's code for each point of the path: VERTEX, BEZIER_VERTEX,
    /// QUADRATIC_VERTEX, CURVE_VERTEX, or BREAK for begin_contour.
    pub fn get_vertex_codes(&self) -> Vec<i32> {
        self.steps.iter().filter_map(|s| match s {
            Step::Vertex(..) | Step::Full(..) => Some(crate::constants::VERTEX),
            Step::Bezier(_) => Some(crate::constants::BEZIER_VERTEX),
            Step::Quadratic(_) => Some(crate::constants::QUADRATIC_VERTEX),
            Step::Curve(..) => Some(crate::constants::CURVE_VERTEX),
            Step::BeginContour => Some(crate::constants::BREAK),
            _ => None,
        }.map(|c| c as i32)).collect()
    }
    pub fn get_vertex_code_count(&self) -> usize { self.get_vertex_codes().len() }
    /// get_vertex_code(i), or VERTEX past the end.
    pub fn get_vertex_code(&self, i: usize) -> i32 { self.get_vertex_codes().get(i).copied().unwrap_or(0) }

    /// attrib(name, values) on the shape: replayed before the next vertex when drawn.
    pub fn attrib(&mut self, name: &str, v: impl crate::shader::UniformValue) -> &mut Self {
        self.steps.push(Step::Attrib(name.to_string(), v.floats())); self
    }
    pub fn attrib_position(&mut self, name: &str, x: impl Num, y: impl Num, z: impl Num) -> &mut Self { self.attrib(name, [x.f(), y.f(), z.f()]) }
    pub fn attrib_normal(&mut self, name: &str, x: impl Num, y: impl Num, z: impl Num) -> &mut Self { self.attrib(name, [x.f(), y.f(), z.f()]) }
    pub fn attrib_color(&mut self, name: &str, c: impl Paint) -> &mut Self { let c = c.paint(); self.attrib(name, [c.r, c.g, c.b, c.a]) }

    /// bezier_detail / curve_detail / curve_tightness used while this shape is drawn.
    pub fn bezier_detail(&mut self, n: impl Num) -> &mut Self { self.bezier_detail = Some(n.f().clamp(1.0, 1000.0) as u32); self }
    pub fn curve_detail(&mut self, n: impl Num) -> &mut Self { self.curve_detail = Some(n.f().clamp(1.0, 1000.0) as u32); self }
    pub fn curve_tightness(&mut self, t: impl Num) -> &mut Self { let t = t.f(); if t.is_finite() { self.curve_tightness = Some(t); } self }
    /// texture_mode(IMAGE or NORMAL) used for this shape's u, v.
    pub fn texture_mode(&mut self, m: u8) -> &mut Self { self.texture_mode = Some(m.min(1)); self }
    pub fn set_texture_mode(&mut self, m: u8) -> &mut Self { self.texture_mode(m) }

    /// color_mode for the number setters (fill_rgb, stroke_rgba...) on this shape.
    pub fn color_mode(&mut self, mode: u32) -> &mut Self { let m = self.color_mode.map_or(255.0, |c| c.1[0]); self.color_mode_max(mode, m, m, m, m) }
    pub fn color_mode_range(&mut self, mode: u32, max: impl Num) -> &mut Self { let m = max.f(); self.color_mode_max(mode, m, m, m, m) }
    pub fn color_mode_max(&mut self, mode: u32, m1: impl Num, m2: impl Num, m3: impl Num, ma: impl Num) -> &mut Self {
        self.color_mode = Some((mode, [m1.f(), m2.f(), m3.f(), ma.f()])); self
    }
    /// Runs f with the shape's color_mode in place, if it has one.
    fn in_color_mode(&self, f: impl FnOnce() -> Color) -> Color {
        let Some((mode, m)) = self.color_mode else { return f() };
        let old = *crate::COLOR_MODE.read();
        crate::color_mode_max(mode, m[0], m[1], m[2], m[3]);
        let c = f();
        *crate::COLOR_MODE.write() = old;
        c
    }

    fn nth_vertex_mut(&mut self, i: usize) -> Option<&mut Step> { self.steps.iter_mut().filter(|s| Self::is_vertex(s)).nth(i) }
    /// set_texture_uv(i, u, v): the spot on the texture for a vertex.
    pub fn set_texture_uv(&mut self, i: usize, u: impl Num, v: impl Num) -> &mut Self {
        let (u, v) = (u.f(), v.f());
        if let Some(s) = self.nth_vertex_mut(i) {
            match s {
                Step::Full(p, f) => { p[3] = u; p[4] = v; *f |= 2; }
                Step::Vertex(x, y) => *s = Step::Full([*x, *y, 0.0, u, v], 2),
                _ => {}
            }
        }
        self
    }
    fn vertex_uv_of(&self, i: usize) -> (f32, f32) {
        match self.steps.iter().filter(|s| Self::is_vertex(s)).nth(i) {
            Some(Step::Full(p, f)) if f & 2 != 0 => (p[3], p[4]),
            _ => (0.0, 0.0),
        }
    }
    pub fn get_texture_u(&self, i: usize) -> f32 { self.vertex_uv_of(i).0 }
    pub fn get_texture_v(&self, i: usize) -> f32 { self.vertex_uv_of(i).1 }
    /// set_vertex_3d(i, x, y, z)
    pub fn set_vertex_3d(&mut self, i: usize, x: impl Num, y: impl Num, z: impl Num) -> &mut Self {
        let (x, y, z) = (x.f(), y.f(), z.f());
        self.set_vertex(i, x, y);
        if let Some(s) = self.nth_vertex_mut(i) {
            match s {
                Step::Full(p, f) => { p[2] = z; *f |= 1; }
                Step::Vertex(x, y) => *s = Step::Full([*x, *y, z, 0.0, 0.0], 1),
                _ => {}
            }
        }
        self
    }

    /// set_normal(i, x, y, z): kept for get_normal; lighting uses each face's own.
    pub fn set_normal(&mut self, i: usize, x: impl Num, y: impl Num, z: impl Num) -> &mut Self {
        self.normals.insert(i, [x.f(), y.f(), z.f()]); self
    }
    pub fn get_normal(&self, i: usize) -> crate::PVector {
        let n = self.normals.get(&i).copied().unwrap_or([0.0; 3]);
        crate::PVector::new3(n[0], n[1], n[2])
    }
    pub fn get_normal_x(&self, i: usize) -> f32 { self.get_normal(i).x }
    pub fn get_normal_y(&self, i: usize) -> f32 { self.get_normal(i).y }
    pub fn get_normal_z(&self, i: usize) -> f32 { self.get_normal(i).z }

    /// Material colors, kept on the shape (read back with the getters).
    pub fn set_ambient(&mut self, c: impl crate::Paint) -> &mut Self { self.material.ambient = Some(c.paint()); self }
    pub fn set_emissive(&mut self, c: impl crate::Paint) -> &mut Self { self.material.emissive = Some(c.paint()); self }
    pub fn set_specular(&mut self, c: impl crate::Paint) -> &mut Self { self.material.specular = Some(c.paint()); self }
    pub fn set_shininess(&mut self, s: impl Num) -> &mut Self { self.material.shininess = Some(s.f()); self }
    pub fn ambient(&mut self, c: impl crate::Paint) -> &mut Self { self.set_ambient(c) }
    pub fn emissive(&mut self, c: impl crate::Paint) -> &mut Self { self.set_emissive(c) }
    pub fn specular(&mut self, c: impl crate::Paint) -> &mut Self { self.set_specular(c) }
    pub fn shininess(&mut self, s: impl Num) -> &mut Self { self.set_shininess(s) }
    pub fn get_ambient(&self, _i: usize) -> Option<Color> { self.material.ambient }
    pub fn get_emissive(&self, _i: usize) -> Option<Color> { self.material.emissive }
    pub fn get_specular(&self, _i: usize) -> Option<Color> { self.material.specular }
    pub fn get_shininess(&self, _i: usize) -> f32 { self.material.shininess.unwrap_or(0.0) }

    #[inline] pub fn getWidth(&self) -> f32 { self.get_width() }
    #[inline] pub fn getHeight(&self) -> f32 { self.get_height() }
    #[inline] pub fn getDepth(&self) -> f32 { self.get_depth() }
    #[inline] pub fn getKind(&self) -> u8 { self.get_kind() }
    #[inline] pub fn getFamily(&self) -> u8 { self.get_family() }
    #[inline] pub fn isClosed(&self) -> bool { self.is_closed() }
    #[inline] pub fn is2D(&self) -> bool { self.is_2d() }
    #[inline] pub fn is3D(&self) -> bool { self.is_3d() }
    #[inline] pub fn findChild(&self, name: &str) -> Option<&PShape> { self.find_child(name) }
    #[inline] pub fn getVertexX(&self, i: usize) -> f32 { self.get_vertex_x(i) }
    #[inline] pub fn getVertexY(&self, i: usize) -> f32 { self.get_vertex_y(i) }
    #[inline] pub fn getVertexZ(&self, i: usize) -> f32 { self.get_vertex_z(i) }
    #[inline] pub fn getVertexCount(&self) -> usize { self.get_vertex_count() }
    #[inline] pub fn setStrokeCap(&mut self, c: u32) -> &mut Self { self.set_stroke_cap(c) }
    #[inline] pub fn setStrokeJoin(&mut self, j: u32) -> &mut Self { self.set_stroke_join(j) }
    #[inline] pub fn setTextureUV(&mut self, i: usize, u: impl Num, v: impl Num) -> &mut Self { self.set_texture_uv(i, u, v) }
    #[inline] pub fn getTextureU(&self, i: usize) -> f32 { self.get_texture_u(i) }
    #[inline] pub fn getTextureV(&self, i: usize) -> f32 { self.get_texture_v(i) }
    #[inline] pub fn applyMatrix(&mut self, a: impl Num, b: impl Num, c: impl Num, d: impl Num, e: impl Num, f: impl Num) -> &mut Self { self.apply_matrix(a, b, c, d, e, f) }
    #[inline] pub fn applyMatrix3D(&mut self, m: [f32; 16]) -> &mut Self { self.apply_matrix_3d(m) }
    #[inline] pub fn resetMatrix(&mut self) -> &mut Self { self.reset_matrix() }
    #[inline] pub fn bezierDetail(&mut self, n: impl Num) -> &mut Self { self.bezier_detail(n) }
    #[inline] pub fn curveDetail(&mut self, n: impl Num) -> &mut Self { self.curve_detail(n) }
    #[inline] pub fn curveTightness(&mut self, t: impl Num) -> &mut Self { self.curve_tightness(t) }
    #[inline] pub fn textureMode(&mut self, m: u8) -> &mut Self { self.texture_mode(m) }
    #[inline] pub fn setTextureMode(&mut self, m: u8) -> &mut Self { self.texture_mode(m) }
    #[inline] pub fn colorMode(&mut self, mode: u32) -> &mut Self { self.color_mode(mode) }
    #[inline] pub fn attribPosition(&mut self, name: &str, x: impl Num, y: impl Num, z: impl Num) -> &mut Self { self.attrib_position(name, x, y, z) }
    #[inline] pub fn attribNormal(&mut self, name: &str, x: impl Num, y: impl Num, z: impl Num) -> &mut Self { self.attrib_normal(name, x, y, z) }
    #[inline] pub fn attribColor(&mut self, name: &str, c: impl Paint) -> &mut Self { self.attrib_color(name, c) }
    #[inline] pub fn getVertexCode(&self, i: usize) -> i32 { self.get_vertex_code(i) }
    #[inline] pub fn getVertexCodeCount(&self) -> usize { self.get_vertex_code_count() }
    #[inline] pub fn getVertexCodes(&self) -> Vec<i32> { self.get_vertex_codes() }
    #[inline] pub fn set3D(&mut self, on: bool) -> &mut Self { self.set_3d(on) }
    #[inline] pub fn enableStyle(&mut self) -> &mut Self { self.enable_style() }
    #[inline] pub fn disableStyle(&mut self) -> &mut Self { self.disable_style() }
    #[inline] pub fn setKind(&mut self, k: u8) -> &mut Self { self.set_kind(k) }
    #[inline] pub fn setFamily(&mut self, f: u8) -> &mut Self { self.set_family(f) }
}

/// What shape() can draw: an SVG from load_shape (its id) or a PShape.
pub trait ShapeRef {
    fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>);
    /// Its own width and height, for shape_mode(CENTER).
    fn natural_size(&self) -> (f32, f32);
}
impl ShapeRef for u64 {
    fn natural_size(&self) -> (f32, f32) { crate::render::shape_size(*self) }
    fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>) {
        match size {
            Some((w, h)) => crate::render::draw_shape(*self, x, y, w, h),
            None => { let (w, h) = crate::render::shape_size(*self); crate::render::draw_shape(*self, x, y, w, h) }
        }
    }
}
impl ShapeRef for PShape {
    fn natural_size(&self) -> (f32, f32) { (self.width(), self.height()) }
    fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>) {
        crate::push_matrix();
        crate::translate(x, y);
        // shape(s, x, y, w, h) on a code-made shape scales it as if it were 100 x 100,
        // which is what Processing does for shapes without a known size
        if let Some((w, h)) = size { crate::scale_xy(w / 100.0, h / 100.0); }
        self.draw();
        crate::pop_matrix();
    }
}
impl<T: ShapeRef + ?Sized> ShapeRef for &T {
    fn natural_size(&self) -> (f32, f32) { (**self).natural_size() }
    fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>) { (**self).draw_at(x, y, size) }
}
impl<T: ShapeRef + ?Sized> ShapeRef for &mut T {
    fn natural_size(&self) -> (f32, f32) { (**self).natural_size() }
    fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>) { (**self).draw_at(x, y, size) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn params() {
        assert_eq!(super::create_shape_rect(0, 0, 80, 40).get_params(), vec![0.0, 0.0, 80.0, 40.0]);
        assert_eq!(super::create_shape_sphere(5).get_param(0), 5.0);
        assert!(super::create_shape().get_params().is_empty());
    }
    use super::*;
    #[test]
    fn build_and_edit() {
        let mut s = create_shape();
        s.begin_shape().fill_rgb(255, 0, 0).vertex(0, 0).vertex(10, 0).bezier_vertex(1, 2, 3, 4, 5, 6).end_shape_mode(crate::CLOSE);
        assert_eq!(s.vertex_count(), 3);
        assert_eq!(s.get_vertex(2), (5.0, 6.0));
        s.set_vertex(2, 7, 8);
        assert_eq!(s.get_vertex(2), (7.0, 8.0));
        assert_eq!(s.get_vertex(99), (0.0, 0.0));
        // begin again starts over, keeps style
        s.begin_shape().vertex(1, 1);
        assert_eq!(s.vertex_count(), 1);
        assert!(s.fill.is_some());
        let mut g = create_shape_group();
        g.add_child(s.clone()).add_child(create_shape_rect(0, 0, 5, 5));
        assert_eq!(g.child_count(), 2);
        g.get_child(1).unwrap().translate(3, 3).rotate(0.5);
        assert_eq!(g.children[1].moves.len(), 2);
        g.disable_style();
        assert!(g.style_off && g.children[0].style_off && g.children[0].fill.is_some());
        g.enable_style();
        assert!(g.is_style_enabled() && !g.children[0].style_off);
    }
    #[test]
    fn processing_getters() {
        let mut s = create_shape();
        s.begin_shape().vertex(0, 0).vertex(10, 0).vertex(10, 20).vertex(0, 20).end_shape_mode(crate::CLOSE);
        assert_eq!((s.get_width(), s.get_height(), s.get_depth()), (10.0, 20.0, 0.0));
        assert!(s.is_closed() && s.is_2d());
        assert!(s.contains(5, 5));
        assert!(!s.contains(15, 5));
        assert_eq!(s.get_kind(), crate::POLYGON);
        assert_eq!(s.get_family(), crate::constants::GEOMETRY);
        s.set_texture_uv(1, 0.5, 0.25);
        assert_eq!((s.get_texture_u(1), s.get_texture_v(1)), (0.5, 0.25));
        assert_eq!(s.get_vertex(1), (10.0, 0.0));
        s.set_vertex_3d(2, 10, 20, 5);
        assert!(s.is_3d());
        assert_eq!(s.get_vertex_z(2), 5.0);
        assert_eq!(s.get_depth(), 5.0);
        s.set_3d(false);
        assert!(s.is_2d());
        s.set_stroke_cap(crate::SQUARE).tint(crate::color_rgb(255, 0, 0)).set_shininess(4);
        assert_eq!(s.get_tint(), Some(crate::color_rgb(255, 0, 0)));
        assert_eq!(s.get_shininess(0), 4.0);
        s.set_normal(0, 0, 0, 1);
        assert_eq!(s.get_normal_z(0), 1.0);
        assert_eq!(s.get_fill(0), None);
        s.fill_rgb(0, 255, 0);
        assert_eq!(s.get_fill(3), Some(crate::color_rgb(0, 255, 0)));

        let r = create_shape_rect(0, 0, 30, 40);
        assert_eq!(r.get_kind(), crate::constants::RECT);
        assert_eq!(r.get_family(), crate::constants::PRIMITIVE);
        assert_eq!(create_shape_box(1, 2, 3).get_depth(), 3.0);
        let mut g = create_shape_group();
        let mut inner = create_shape_group();
        inner.add_name("deep", r.clone());
        g.add_child(inner.clone());
        assert_eq!(g.get_kind(), crate::constants::GROUP);
        assert_eq!(g.get_family(), crate::constants::GROUP);
        assert_eq!(g.find_child("deep").map(|c| c.get_width()), Some(30.0));
        assert_eq!(g.get_child_index(&inner), Some(0));
        assert_eq!(g.get_children().len(), 1);
        assert_eq!(g.get_height(), 40.0);
        g.set_kind(crate::constants::RECT).set_family(crate::constants::PATH);
        assert_eq!((g.get_kind(), g.get_family()), (crate::constants::RECT, crate::constants::PATH));
    }
    #[test]
    fn vertex_codes() {
        let mut s = create_shape();
        s.begin_shape().vertex(0, 0).bezier_vertex(1, 2, 3, 4, 5, 6).quadratic_vertex(1, 1, 2, 2)
            .curve_vertex(3, 3).begin_contour().vertex_uv(1, 1, 0, 0).end_contour().end_shape();
        assert_eq!(s.get_vertex_codes(), vec![0, 1, 2, 3, 4, 0]);
        assert_eq!(s.get_vertex_code_count(), 6);
        assert_eq!(s.get_vertex_code(1), crate::constants::BEZIER_VERTEX as i32);
        assert_eq!(create_shape_rect(0, 0, 1, 1).get_vertex_code_count(), 0);
    }
    #[test]
    fn matrix_moves() {
        let mut s = create_shape();
        s.translate(1, 2).apply_matrix(1, 0, 5, 0, 1, 6);
        assert_eq!(s.moves[1], Move::M2([1.0, 0.0, 5.0, 0.0, 1.0, 6.0]));
        assert!(s.is_2d());
        let mut m = [0.0; 16]; m[0] = 1.0; m[5] = 1.0; m[10] = 1.0; m[15] = 1.0;
        s.apply_matrix_3d(m);
        assert_eq!(s.moves[2], Move::M3(m));
        assert!(s.is_3d());
        s.reset_matrix();
        assert!(s.moves.is_empty());
    }
    #[test]
    fn attribs_recorded() {
        let mut s = create_shape();
        s.begin_shape().attrib_position("p", 1, 2, 3).vertex(0, 0).attrib("w", 0.5f32).vertex(1, 1).end_shape();
        assert_eq!(s.steps[1], Step::Attrib("p".into(), vec![1.0, 2.0, 3.0]));
        assert_eq!(s.steps[2], Step::Vertex(0.0, 0.0));
        assert_eq!(s.steps[3], Step::Attrib("w".into(), vec![0.5]));
        assert_eq!(s.vertex_count(), 2);
        assert_eq!(s.get_vertex_codes(), vec![0, 0]);
        s.bezier_detail(5).curve_tightness(0.5).texture_mode(crate::NORMAL);
        assert_eq!((s.bezier_detail, s.curve_tightness, s.texture_mode), (Some(5), Some(0.5), Some(1)));
    }
}
