//! PMatrix2D and PMatrix3D, with the same fields and methods as Processing.
//! Rows first: m01 is row 0, column 1, and x' = m00 x + m01 y + m02 (+ m03).
//!
//! get_matrix() gives the current transform; set_matrix() and
//! apply_matrix_3x() put one back.

use crate::PVector;

fn jf(v: f32) -> String { crate::vector::jfloat(v) }

/// A 2D affine transform (two rows of three).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PMatrix2D {
    pub m00: f32, pub m01: f32, pub m02: f32,
    pub m10: f32, pub m11: f32, pub m12: f32,
}

impl Default for PMatrix2D { fn default() -> Self { Self::new() } }

impl PMatrix2D {
    /// The identity.
    pub fn new() -> Self { Self::from_values(1.0, 0.0, 0.0, 0.0, 1.0, 0.0) }
    pub fn from_values(m00: f32, m01: f32, m02: f32, m10: f32, m11: f32, m12: f32) -> Self {
        Self { m00, m01, m02, m10, m11, m12 }
    }
    pub fn reset(&mut self) { *self = Self::new(); }
    /// A copy.
    pub fn get(&self) -> Self { *self }
    /// The six values, row by row.
    pub fn get_array(&self) -> [f32; 6] { [self.m00, self.m01, self.m02, self.m10, self.m11, self.m12] }
    pub fn set(&mut self, m: &PMatrix2D) { *self = *m; }
    pub fn set_values(&mut self, m00: f32, m01: f32, m02: f32, m10: f32, m11: f32, m12: f32) {
        *self = Self::from_values(m00, m01, m02, m10, m11, m12);
    }
    pub fn translate(&mut self, tx: f32, ty: f32) {
        self.m02 += tx * self.m00 + ty * self.m01;
        self.m12 += tx * self.m10 + ty * self.m11;
    }
    pub fn rotate(&mut self, angle: f32) {
        let (s, c) = angle.sin_cos();
        let (a, b) = (self.m00, self.m01);
        self.m00 = c * a + s * b; self.m01 = -s * a + c * b;
        let (a, b) = (self.m10, self.m11);
        self.m10 = c * a + s * b; self.m11 = -s * a + c * b;
    }
    pub fn rotate_z(&mut self, angle: f32) { self.rotate(angle) }
    pub fn scale(&mut self, s: f32) { self.scale_xy(s, s) }
    pub fn scale_xy(&mut self, sx: f32, sy: f32) {
        self.m00 *= sx; self.m01 *= sy; self.m10 *= sx; self.m11 *= sy;
    }
    /// x' = x + tan(angle) y
    pub fn shear_x(&mut self, angle: f32) { self.apply_values(1.0, angle.tan(), 0.0, 0.0, 1.0, 0.0) }
    /// y' = y + tan(angle) x
    pub fn shear_y(&mut self, angle: f32) { self.apply_values(1.0, 0.0, 0.0, angle.tan(), 1.0, 0.0) }
    /// this = this * m
    pub fn apply(&mut self, m: &PMatrix2D) { self.apply_values(m.m00, m.m01, m.m02, m.m10, m.m11, m.m12) }
    pub fn apply_values(&mut self, n00: f32, n01: f32, n02: f32, n10: f32, n11: f32, n12: f32) {
        let s = *self;
        self.m00 = s.m00 * n00 + s.m01 * n10;
        self.m01 = s.m00 * n01 + s.m01 * n11;
        self.m02 = s.m00 * n02 + s.m01 * n12 + s.m02;
        self.m10 = s.m10 * n00 + s.m11 * n10;
        self.m11 = s.m10 * n01 + s.m11 * n11;
        self.m12 = s.m10 * n02 + s.m11 * n12 + s.m12;
    }
    /// this = m * this
    pub fn pre_apply(&mut self, m: &PMatrix2D) {
        let mut r = *m;
        r.apply(self);
        *self = r;
    }
    /// The point after this transform (z is kept).
    pub fn mult(&self, p: &PVector) -> PVector { PVector::new3(self.mult_x(p.x, p.y), self.mult_y(p.x, p.y), p.z) }
    pub fn mult_x(&self, x: f32, y: f32) -> f32 { self.m00 * x + self.m01 * y + self.m02 }
    pub fn mult_y(&self, x: f32, y: f32) -> f32 { self.m10 * x + self.m11 * y + self.m12 }
    pub fn determinant(&self) -> f32 { self.m00 * self.m11 - self.m01 * self.m10 }
    /// Inverts in place; false (and unchanged) if it can't be inverted.
    pub fn invert(&mut self) -> bool {
        let d = self.determinant();
        if d.abs() <= f32::MIN_POSITIVE { return false; }
        let s = *self;
        self.m00 = s.m11 / d;
        self.m01 = -s.m01 / d;
        self.m10 = -s.m10 / d;
        self.m11 = s.m00 / d;
        self.m02 = (s.m01 * s.m12 - s.m11 * s.m02) / d;
        self.m12 = (s.m10 * s.m02 - s.m00 * s.m12) / d;
        true
    }
    /// Prints the two rows.
    pub fn print(&self) { print!("{self}"); }
    #[inline] pub fn preApply(&mut self, m: &PMatrix2D) { self.pre_apply(m) }
    #[inline] pub fn multX(&self, x: f32, y: f32) -> f32 { self.mult_x(x, y) }
    #[inline] pub fn multY(&self, x: f32, y: f32) -> f32 { self.mult_y(x, y) }
    #[inline] pub fn shearX(&mut self, a: f32) { self.shear_x(a) }
    #[inline] pub fn shearY(&mut self, a: f32) { self.shear_y(a) }
}

impl std::fmt::Display for PMatrix2D {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{:>10} {:>10} {:>10}", jf(self.m00), jf(self.m01), jf(self.m02))?;
        writeln!(f, "{:>10} {:>10} {:>10}", jf(self.m10), jf(self.m11), jf(self.m12))
    }
}

/// A 4 x 4 transform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PMatrix3D {
    pub m00: f32, pub m01: f32, pub m02: f32, pub m03: f32,
    pub m10: f32, pub m11: f32, pub m12: f32, pub m13: f32,
    pub m20: f32, pub m21: f32, pub m22: f32, pub m23: f32,
    pub m30: f32, pub m31: f32, pub m32: f32, pub m33: f32,
}

impl Default for PMatrix3D { fn default() -> Self { Self::new() } }

impl PMatrix3D {
    /// The identity.
    pub fn new() -> Self {
        Self::from_rows([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    pub fn from_rows(r: [[f32; 4]; 4]) -> Self {
        Self {
            m00: r[0][0], m01: r[0][1], m02: r[0][2], m03: r[0][3],
            m10: r[1][0], m11: r[1][1], m12: r[1][2], m13: r[1][3],
            m20: r[2][0], m21: r[2][1], m22: r[2][2], m23: r[2][3],
            m30: r[3][0], m31: r[3][1], m32: r[3][2], m33: r[3][3],
        }
    }
    pub fn rows(&self) -> [[f32; 4]; 4] {
        [[self.m00, self.m01, self.m02, self.m03], [self.m10, self.m11, self.m12, self.m13],
         [self.m20, self.m21, self.m22, self.m23], [self.m30, self.m31, self.m32, self.m33]]
    }
    /// A 2D matrix as a 3D one.
    pub fn from_2d(m: &PMatrix2D) -> Self {
        Self::from_rows([[m.m00, m.m01, 0.0, m.m02], [m.m10, m.m11, 0.0, m.m12], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    pub(crate) fn from_col_major(m: &[f32; 16]) -> Self {
        let mut r = [[0.0; 4]; 4];
        for (c, col) in m.chunks(4).enumerate() { for (row, v) in col.iter().enumerate() { r[row][c] = *v; } }
        Self::from_rows(r)
    }
    pub(crate) fn to_col_major(&self) -> [f32; 16] {
        let r = self.rows();
        let mut m = [0.0; 16];
        for c in 0..4 { for row in 0..4 { m[c * 4 + row] = r[row][c]; } }
        m
    }
    pub fn reset(&mut self) { *self = Self::new(); }
    /// A copy.
    pub fn get(&self) -> Self { *self }
    /// The sixteen values, row by row.
    pub fn get_array(&self) -> [f32; 16] {
        let r = self.rows();
        let mut a = [0.0; 16];
        for i in 0..16 { a[i] = r[i / 4][i % 4]; }
        a
    }
    pub fn set(&mut self, m: &PMatrix3D) { *self = *m; }
    /// Sixteen values, row by row.
    pub fn set_array(&mut self, a: &[f32; 16]) {
        let mut r = [[0.0; 4]; 4];
        for i in 0..16 { r[i / 4][i % 4] = a[i]; }
        *self = Self::from_rows(r);
    }
    /// this = this * m
    pub fn apply(&mut self, m: &PMatrix3D) {
        let (a, b) = (self.rows(), m.rows());
        let mut r = [[0.0; 4]; 4];
        for i in 0..4 { for j in 0..4 { r[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum(); } }
        *self = Self::from_rows(r);
    }
    pub fn apply_2d(&mut self, m: &PMatrix2D) { self.apply(&Self::from_2d(m)) }
    /// this = m * this
    pub fn pre_apply(&mut self, m: &PMatrix3D) {
        let mut r = *m;
        r.apply(self);
        *self = r;
    }
    pub fn translate(&mut self, tx: f32, ty: f32) { self.translate_3d(tx, ty, 0.0) }
    pub fn translate_3d(&mut self, tx: f32, ty: f32, tz: f32) {
        self.apply(&Self::from_rows([[1.0, 0.0, 0.0, tx], [0.0, 1.0, 0.0, ty], [0.0, 0.0, 1.0, tz], [0.0, 0.0, 0.0, 1.0]]))
    }
    pub fn rotate(&mut self, angle: f32) { self.rotate_z(angle) }
    pub fn rotate_x(&mut self, angle: f32) {
        let (s, c) = angle.sin_cos();
        self.apply(&Self::from_rows([[1.0, 0.0, 0.0, 0.0], [0.0, c, -s, 0.0], [0.0, s, c, 0.0], [0.0, 0.0, 0.0, 1.0]]))
    }
    pub fn rotate_y(&mut self, angle: f32) {
        let (s, c) = angle.sin_cos();
        self.apply(&Self::from_rows([[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 0.0], [-s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]]))
    }
    pub fn rotate_z(&mut self, angle: f32) {
        let (s, c) = angle.sin_cos();
        self.apply(&Self::from_rows([[c, -s, 0.0, 0.0], [s, c, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]))
    }
    /// rotate(angle, x, y, z): around an axis.
    pub fn rotate_axis(&mut self, angle: f32, x: f32, y: f32, z: f32) {
        let len = (x * x + y * y + z * z).sqrt();
        if !(len > 0.0) { return; }
        let (x, y, z) = (x / len, y / len, z / len);
        let (s, c) = angle.sin_cos();
        let t = 1.0 - c;
        self.apply(&Self::from_rows([
            [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0.0],
            [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0.0],
            [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0.0],
            [0.0, 0.0, 0.0, 1.0]]))
    }
    pub fn scale(&mut self, s: f32) { self.scale_3d(s, s, s) }
    pub fn scale_xy(&mut self, sx: f32, sy: f32) { self.scale_3d(sx, sy, 1.0) }
    pub fn scale_3d(&mut self, sx: f32, sy: f32, sz: f32) {
        self.m00 *= sx; self.m01 *= sy; self.m02 *= sz;
        self.m10 *= sx; self.m11 *= sy; self.m12 *= sz;
        self.m20 *= sx; self.m21 *= sy; self.m22 *= sz;
        self.m30 *= sx; self.m31 *= sy; self.m32 *= sz;
    }
    /// x' = x + tan(angle) y
    pub fn shear_x(&mut self, angle: f32) {
        let t = angle.tan();
        self.apply(&Self::from_rows([[1.0, t, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]))
    }
    /// y' = y + tan(angle) x
    pub fn shear_y(&mut self, angle: f32) {
        let t = angle.tan();
        self.apply(&Self::from_rows([[1.0, 0.0, 0.0, 0.0], [t, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]))
    }
    /// The point after this transform (the bottom row is not used, as in Processing).
    pub fn mult(&self, p: &PVector) -> PVector {
        PVector::new3(self.mult_x(p.x, p.y, p.z), self.mult_y(p.x, p.y, p.z), self.mult_z(p.x, p.y, p.z))
    }
    /// [x, y, z, w] through the whole matrix.
    pub fn mult_4(&self, v: [f32; 4]) -> [f32; 4] {
        let r = self.rows();
        let mut o = [0.0; 4];
        for i in 0..4 { o[i] = (0..4).map(|k| r[i][k] * v[k]).sum(); }
        o
    }
    pub fn mult_x(&self, x: f32, y: f32, z: f32) -> f32 { self.m00 * x + self.m01 * y + self.m02 * z + self.m03 }
    pub fn mult_y(&self, x: f32, y: f32, z: f32) -> f32 { self.m10 * x + self.m11 * y + self.m12 * z + self.m13 }
    pub fn mult_z(&self, x: f32, y: f32, z: f32) -> f32 { self.m20 * x + self.m21 * y + self.m22 * z + self.m23 }
    pub fn mult_w(&self, x: f32, y: f32, z: f32) -> f32 { self.m30 * x + self.m31 * y + self.m32 * z + self.m33 }
    pub fn transpose(&mut self) {
        let r = self.rows();
        let mut t = [[0.0; 4]; 4];
        for i in 0..4 { for j in 0..4 { t[i][j] = r[j][i]; } }
        *self = Self::from_rows(t);
    }
    fn minor(r: &[[f32; 4]; 4], row: usize, col: usize) -> f32 {
        let mut m = [[0.0f32; 3]; 3];
        let mut mi = 0;
        for i in 0..4 {
            if i == row { continue; }
            let mut mj = 0;
            for j in 0..4 { if j == col { continue; } m[mi][mj] = r[i][j]; mj += 1; }
            mi += 1;
        }
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }
    pub fn determinant(&self) -> f32 {
        let r = self.rows();
        (0..4).map(|j| { let s = if j % 2 == 0 { 1.0 } else { -1.0 }; s * r[0][j] * Self::minor(&r, 0, j) }).sum()
    }
    /// Inverts in place; false (and unchanged) if it can't be inverted.
    pub fn invert(&mut self) -> bool {
        let d = self.determinant();
        if d == 0.0 || !d.is_finite() { return false; }
        let r = self.rows();
        let mut inv = [[0.0; 4]; 4];
        for i in 0..4 { for j in 0..4 {
            let s = if (i + j) % 2 == 0 { 1.0 } else { -1.0 };
            // adjugate: the cofactor of (j, i)
            inv[i][j] = s * Self::minor(&r, j, i) / d;
        } }
        *self = Self::from_rows(inv);
        true
    }
    /// Prints the four rows.
    pub fn print(&self) { print!("{self}"); }
    #[inline] pub fn preApply(&mut self, m: &PMatrix3D) { self.pre_apply(m) }
    #[inline] pub fn rotateX(&mut self, a: f32) { self.rotate_x(a) }
    #[inline] pub fn rotateY(&mut self, a: f32) { self.rotate_y(a) }
    #[inline] pub fn rotateZ(&mut self, a: f32) { self.rotate_z(a) }
    #[inline] pub fn multX(&self, x: f32, y: f32, z: f32) -> f32 { self.mult_x(x, y, z) }
    #[inline] pub fn multY(&self, x: f32, y: f32, z: f32) -> f32 { self.mult_y(x, y, z) }
    #[inline] pub fn multZ(&self, x: f32, y: f32, z: f32) -> f32 { self.mult_z(x, y, z) }
    #[inline] pub fn multW(&self, x: f32, y: f32, z: f32) -> f32 { self.mult_w(x, y, z) }
    #[inline] pub fn shearX(&mut self, a: f32) { self.shear_x(a) }
    #[inline] pub fn shearY(&mut self, a: f32) { self.shear_y(a) }
}

impl std::fmt::Display for PMatrix3D {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for r in self.rows() {
            writeln!(f, "{:>10} {:>10} {:>10} {:>10}", jf(r[0]), jf(r[1]), jf(r[2]), jf(r[3]))?;
        }
        Ok(())
    }
}

/// The current 2D transform (what translate, rotate, scale, shear and
/// apply_matrix built up).
pub fn get_matrix_2d() -> PMatrix2D {
    let m = crate::render::current_matrix_pub();
    PMatrix2D::from_values(m[0], m[2], m[4], m[1], m[3], m[5])
}
/// getMatrix(): the current transform as a 4 x 4 matrix. In a 2D sketch
/// this is the 2D transform; once anything 3D is in play, the 3D one.
pub fn get_matrix() -> PMatrix3D {
    if crate::three::in_3d() { PMatrix3D::from_col_major(&crate::three::model_matrix()) }
    else { PMatrix3D::from_2d(&get_matrix_2d()) }
}
fn is_2d(m: &PMatrix3D) -> bool {
    m.m02 == 0.0 && m.m12 == 0.0 && m.m20 == 0.0 && m.m21 == 0.0 && m.m22 == 1.0 && m.m23 == 0.0
        && m.m30 == 0.0 && m.m31 == 0.0 && m.m32 == 0.0 && m.m33 == 1.0
}
/// applyMatrix(m): multiplies the current transform by m.
pub fn apply_pmatrix(m: &PMatrix3D) {
    if is_2d(m) {
        crate::apply_matrix(m.m00, m.m10, m.m01, m.m11, m.m03, m.m13);
        crate::three::on_apply(m.to_col_major());
    } else {
        let r = m.get_array();
        crate::three::apply_matrix_3d(r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7],
                                      r[8], r[9], r[10], r[11], r[12], r[13], r[14], r[15]);
    }
}
/// applyMatrix(m) with a 2D matrix.
pub fn apply_pmatrix_2d(m: &PMatrix2D) { apply_pmatrix(&PMatrix3D::from_2d(m)) }
/// setMatrix(m): the current transform becomes m.
pub fn set_matrix(m: &PMatrix3D) { crate::reset_matrix(); apply_pmatrix(m); }
pub fn set_matrix_2d(m: &PMatrix2D) { crate::reset_matrix(); apply_pmatrix_2d(m); }
#[inline] pub fn getMatrix() -> PMatrix3D { get_matrix() }
#[inline] pub fn setMatrix(m: &PMatrix3D) { set_matrix(m) }

#[cfg(test)]
mod tests {
    use super::*;
    fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-4 }
    #[test]
    fn matrix_2d() {
        let mut m = PMatrix2D::new();
        m.translate(10.0, 5.0);
        m.rotate(std::f32::consts::FRAC_PI_2);
        m.scale(2.0);
        let p = m.mult(&PVector::new(1.0, 0.0));
        assert!(close(p.x, 10.0) && close(p.y, 7.0));
        let mut inv = m;
        assert!(inv.invert());
        let q = inv.mult(&p);
        assert!(close(q.x, 1.0) && close(q.y, 0.0));
        assert!(close(m.determinant(), 4.0));
        let mut s = PMatrix2D::new();
        s.shear_x(std::f32::consts::FRAC_PI_4);
        assert!(close(s.mult_x(0.0, 1.0), 1.0));
        assert!(!PMatrix2D::from_values(0.0, 0.0, 0.0, 0.0, 0.0, 0.0).invert());
        let mut a = PMatrix2D::new(); a.translate(1.0, 0.0);
        let mut b = PMatrix2D::new(); b.scale(3.0);
        let mut ab = a; ab.apply(&b);
        let mut ba = a; ba.pre_apply(&b);
        assert!(close(ab.mult_x(1.0, 0.0), 4.0));
        assert!(close(ba.mult_x(1.0, 0.0), 6.0));
    }
    #[test]
    fn matrix_3d() {
        let mut m = PMatrix3D::new();
        m.translate_3d(1.0, 2.0, 3.0);
        m.rotate_x(0.3); m.rotate_y(-0.7); m.rotate_axis(1.1, 1.0, 2.0, 0.5); m.scale_3d(2.0, 3.0, 4.0);
        let mut inv = m;
        assert!(inv.invert());
        let mut id = m; id.apply(&inv);
        let r = id.rows();
        for i in 0..4 { for j in 0..4 { assert!(close(r[i][j], if i == j { 1.0 } else { 0.0 })); } }
        assert!(close(m.determinant(), 24.0));
        let mut z = PMatrix3D::new();
        z.rotate_z(std::f32::consts::FRAC_PI_2);
        let p = z.mult(&PVector::new3(1.0, 0.0, 0.0));
        assert!(close(p.x, 0.0) && close(p.y, 1.0));
        let mut t = m; t.transpose();
        assert_eq!(t.m01, m.m10);
        assert_eq!(PMatrix3D::from_col_major(&m.to_col_major()), m);
        assert_eq!(m.mult_w(0.0, 0.0, 0.0), 1.0);
    }
}
