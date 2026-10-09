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
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Move { T(f32, f32), R(f32), S(f32, f32) }

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PShape {
    steps: Vec<Step>,
    fill: Option<Option<Color>>,
    stroke: Option<Option<Color>>,
    weight: Option<f32>,
    moves: Vec<Move>,
    children: Vec<PShape>,
    visible: bool,
}

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

impl PShape {
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
    pub fn end_shape(&mut self) -> &mut Self { self.steps.push(Step::End(false)); self }
    /// end_shape_mode(CLOSE) joins the last point back to the first.
    pub fn end_shape_mode(&mut self, mode: u8) -> &mut Self { self.steps.push(Step::End(mode == crate::CLOSE)); self }
    pub fn add_child(&mut self, child: PShape) -> &mut Self { self.children.push(child); self }
    pub fn child_count(&self) -> usize { self.children.len() }
    pub fn get_child(&mut self, i: usize) -> Option<&mut PShape> { self.children.get_mut(i) }

    // ── style ────────────────────────────────────────────────────────────
    pub fn fill(&mut self, c: impl Paint) -> &mut Self { self.fill = Some(Some(c.paint())); self }
    pub fn fill_ga(&mut self, g: impl Num, a: impl Num) -> &mut Self { self.fill = Some(Some(crate::color_ga(g, a))); self }
    pub fn fill_rgb(&mut self, r: impl Num, g: impl Num, b: impl Num) -> &mut Self { self.fill = Some(Some(crate::color_rgb(r, g, b))); self }
    pub fn fill_rgba(&mut self, r: impl Num, g: impl Num, b: impl Num, a: impl Num) -> &mut Self {
        self.fill = Some(Some(crate::color_rgba(r, g, b, a))); self
    }
    pub fn no_fill(&mut self) -> &mut Self { self.fill = Some(None); self }
    pub fn stroke(&mut self, c: impl Paint) -> &mut Self { self.stroke = Some(Some(c.paint())); self }
    pub fn stroke_ga(&mut self, g: impl Num, a: impl Num) -> &mut Self { self.stroke = Some(Some(crate::color_ga(g, a))); self }
    pub fn stroke_rgb(&mut self, r: impl Num, g: impl Num, b: impl Num) -> &mut Self { self.stroke = Some(Some(crate::color_rgb(r, g, b))); self }
    pub fn stroke_rgba(&mut self, r: impl Num, g: impl Num, b: impl Num, a: impl Num) -> &mut Self {
        self.stroke = Some(Some(crate::color_rgba(r, g, b, a))); self
    }
    pub fn no_stroke(&mut self) -> &mut Self { self.stroke = Some(None); self }
    pub fn stroke_weight(&mut self, w: impl Num) -> &mut Self { self.weight = Some(w.f()); self }
    /// set_fill / set_stroke: the same as fill / stroke, for changing a shape later.
    pub fn set_fill(&mut self, c: impl Paint) -> &mut Self { self.fill(c) }
    pub fn set_stroke(&mut self, c: impl Paint) -> &mut Self { self.stroke(c) }
    pub fn set_stroke_weight(&mut self, w: impl Num) -> &mut Self { self.stroke_weight(w) }
    /// disable_style(): draw with the sketch's fill and stroke instead.
    pub fn disable_style(&mut self) -> &mut Self { self.fill = None; self.stroke = None; self.weight = None; self }
    pub fn set_visible(&mut self, v: bool) -> &mut Self { self.visible = v; self }
    pub fn is_visible(&self) -> bool { self.visible }

    // ── moving ───────────────────────────────────────────────────────────
    pub fn translate(&mut self, x: impl Num, y: impl Num) -> &mut Self { self.moves.push(Move::T(x.f(), y.f())); self }
    pub fn rotate(&mut self, a: impl Num) -> &mut Self { self.moves.push(Move::R(a.f())); self }
    pub fn scale(&mut self, s: impl Num) -> &mut Self { let s = s.f(); self.moves.push(Move::S(s, s)); self }
    pub fn scale_xy(&mut self, x: impl Num, y: impl Num) -> &mut Self { self.moves.push(Move::S(x.f(), y.f())); self }
    /// reset_matrix(): forget translate / rotate / scale.
    pub fn reset_matrix(&mut self) -> &mut Self { self.moves.clear(); self }

    // ── vertices ─────────────────────────────────────────────────────────
    fn points(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.steps.iter().filter_map(|s| match s {
            Step::Vertex(x, y) | Step::Curve(x, y) => Some((*x, *y)),
            Step::Bezier(v) => Some((v[4], v[5])),
            Step::Quadratic(v) => Some((v[2], v[3])),
            _ => None,
        })
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
            }
        }
        match self.fill { Some(Some(c)) => crate::fill_color(c), Some(None) => crate::no_fill(), None => {} }
        match self.stroke { Some(Some(c)) => crate::stroke_color(c), Some(None) => crate::no_stroke(), None => {} }
        if let Some(w) = self.weight { crate::stroke_weight(w); }
        let mut open = false;
        for s in &self.steps {
            match *s {
                Step::Begin(k) => { crate::begin_shape_kind(k); open = true; }
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
            }
        }
        // a shape left open (no end_shape) is still drawn, as an open outline
        if open { crate::end_shape(); }
        for c in &self.children { c.draw(); }
        crate::pop_style();
        crate::pop_matrix();
    }
}

/// What shape() can draw: an SVG from load_shape (its id) or a PShape.
pub trait ShapeRef { fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>); }
impl ShapeRef for u64 {
    fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>) {
        match size {
            Some((w, h)) => crate::render::draw_shape(*self, x, y, w, h),
            None => { let (w, h) = crate::render::shape_size(*self); crate::render::draw_shape(*self, x, y, w, h) }
        }
    }
}
impl ShapeRef for PShape {
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
    fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>) { (**self).draw_at(x, y, size) }
}
impl<T: ShapeRef + ?Sized> ShapeRef for &mut T {
    fn draw_at(&self, x: f32, y: f32, size: Option<(f32, f32)>) { (**self).draw_at(x, y, size) }
}

#[cfg(test)]
mod tests {
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
        assert!(g.fill.is_none());
    }
}
