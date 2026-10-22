//! PVector methods, the way Processing has them.
//!
//!   let mut v = create_vector(3, 4);
//!   v.add(&create_vector(1, 0)).mult(2.0);
//!   println!("{v} {}", v.mag());

use crate::{Num, PVector};

/// createVector(x, y)
pub fn create_vector(x: impl Num, y: impl Num) -> PVector { PVector::new(x.f(), y.f()) }
/// createVector(x, y, z)
pub fn create_vector3(x: impl Num, y: impl Num, z: impl Num) -> PVector { PVector::new3(x.f(), y.f(), z.f()) }
#[inline] pub fn createVector(x: impl Num, y: impl Num) -> PVector { create_vector(x, y) }
#[inline] pub fn createVector3(x: impl Num, y: impl Num, z: impl Num) -> PVector { create_vector3(x, y, z) }

fn rnd(lo: f32, hi: f32) -> f32 { crate::core::random_f32(lo, hi) }

impl PVector {
    /// A copy of this vector.
    pub fn copy(&self) -> PVector { *self }
    /// Same as copy().
    pub fn get(&self) -> PVector { *self }
    pub fn set(&mut self, x: f32, y: f32, z: f32) -> &mut Self { self.x = x; self.y = y; self.z = z; self }
    pub fn set_vec(&mut self, v: &PVector) -> &mut Self { *self = *v; self }
    pub fn add(&mut self, v: &PVector) -> &mut Self { self.x += v.x; self.y += v.y; self.z += v.z; self }
    pub fn add_xyz(&mut self, x: f32, y: f32, z: f32) -> &mut Self { self.x += x; self.y += y; self.z += z; self }
    pub fn sub(&mut self, v: &PVector) -> &mut Self { self.x -= v.x; self.y -= v.y; self.z -= v.z; self }
    pub fn sub_xyz(&mut self, x: f32, y: f32, z: f32) -> &mut Self { self.x -= x; self.y -= y; self.z -= z; self }
    pub fn mult(&mut self, n: f32) -> &mut Self { self.x *= n; self.y *= n; self.z *= n; self }
    pub fn div(&mut self, n: f32) -> &mut Self { self.x /= n; self.y /= n; self.z /= n; self }
    pub fn mag_sq(&self) -> f32 { self.x * self.x + self.y * self.y + self.z * self.z }
    pub fn dot(&self, v: &PVector) -> f32 { self.x * v.x + self.y * v.y + self.z * v.z }
    pub fn dot_xyz(&self, x: f32, y: f32, z: f32) -> f32 { self.x * x + self.y * y + self.z * z }
    /// A new vector at right angles to both.
    pub fn cross(&self, v: &PVector) -> PVector {
        PVector::new3(self.y * v.z - v.y * self.z, self.z * v.x - v.z * self.x, self.x * v.y - v.x * self.y)
    }
    pub fn dist(&self, v: &PVector) -> f32 { (*self - *v).mag() }
    /// A unit-length copy (this one is left alone).
    pub fn normalized(&self) -> PVector { let mut v = *self; v.normalize(); v }
    /// The 2D angle, from atan2(y, x).
    pub fn heading(&self) -> f32 { self.y.atan2(self.x) }
    pub fn heading_2d(&self) -> f32 { self.heading() }
    /// Points the vector at an angle, keeping its length (2D).
    pub fn set_heading(&mut self, angle: f32) -> &mut Self {
        let m = self.mag();
        self.x = m * angle.cos(); self.y = m * angle.sin(); self
    }
    pub fn set_mag(&mut self, len: f32) -> &mut Self { self.normalize(); self.mult(len) }
    /// Shortens the vector to max if it is longer.
    pub fn limit(&mut self, max: f32) -> &mut Self {
        if self.mag_sq() > max * max { self.normalize(); self.mult(max); }
        self
    }
    /// Turns the vector by an angle (2D; z is kept).
    pub fn rotate(&mut self, angle: f32) -> &mut Self {
        let (s, c) = angle.sin_cos();
        let t = self.x;
        self.x = self.x * c - self.y * s;
        self.y = t * s + self.y * c;
        self
    }
    /// Moves toward v by amt (0..1).
    pub fn lerp(&mut self, v: &PVector, amt: f32) -> &mut Self {
        self.x += (v.x - self.x) * amt; self.y += (v.y - self.y) * amt; self.z += (v.z - self.z) * amt; self
    }
    pub fn lerp_xyz(&mut self, x: f32, y: f32, z: f32, amt: f32) -> &mut Self { self.lerp(&PVector::new3(x, y, z), amt) }
    pub fn array(&self) -> [f32; 3] { [self.x, self.y, self.z] }
    /// A unit vector at an angle.
    pub fn from_angle(angle: f32) -> PVector { PVector::new(angle.cos(), angle.sin()) }
    /// A random unit vector in 2D.
    pub fn random_2d() -> PVector { PVector::from_angle(rnd(0.0, std::f32::consts::TAU)) }
    /// A random unit vector in 3D, spread evenly over the sphere.
    pub fn random_3d() -> PVector {
        let a = rnd(0.0, std::f32::consts::TAU);
        let z = rnd(-1.0, 1.0);
        let r = (1.0 - z * z).sqrt();
        PVector::new3(r * a.cos(), r * a.sin(), z)
    }
    /// The angle between two vectors, 0..PI.
    pub fn angle_between(a: &PVector, b: &PVector) -> f32 {
        if (a.x == 0.0 && a.y == 0.0 && a.z == 0.0) || (b.x == 0.0 && b.y == 0.0 && b.z == 0.0) { return 0.0; }
        let d = (a.x as f64 * b.x as f64 + a.y as f64 * b.y as f64 + a.z as f64 * b.z as f64)
            / ((a.mag_sq() as f64).sqrt() * (b.mag_sq() as f64).sqrt());
        if d <= -1.0 { std::f32::consts::PI } else if d >= 1.0 { 0.0 } else { d.acos() as f32 }
    }

    #[inline] pub fn magSq(&self) -> f32 { self.mag_sq() }
    #[inline] pub fn setMag(&mut self, len: f32) -> &mut Self { self.set_mag(len) }
    #[inline] pub fn setHeading(&mut self, angle: f32) -> &mut Self { self.set_heading(angle) }
    #[inline] pub fn heading2D(&self) -> f32 { self.heading() }
    #[inline] pub fn fromAngle(angle: f32) -> PVector { PVector::from_angle(angle) }
    #[inline] pub fn random2D() -> PVector { PVector::random_2d() }
    #[inline] pub fn random3D() -> PVector { PVector::random_3d() }
    #[inline] pub fn angleBetween(a: &PVector, b: &PVector) -> f32 { PVector::angle_between(a, b) }
}

impl std::ops::Add for PVector { type Output = PVector; fn add(self, o: PVector) -> PVector { PVector::new3(self.x + o.x, self.y + o.y, self.z + o.z) } }
impl std::ops::Sub for PVector { type Output = PVector; fn sub(self, o: PVector) -> PVector { PVector::new3(self.x - o.x, self.y - o.y, self.z - o.z) } }
impl std::ops::Mul<f32> for PVector { type Output = PVector; fn mul(self, n: f32) -> PVector { PVector::new3(self.x * n, self.y * n, self.z * n) } }
impl std::ops::Mul<PVector> for f32 { type Output = PVector; fn mul(self, v: PVector) -> PVector { v * self } }
impl std::ops::Div<f32> for PVector { type Output = PVector; fn div(self, n: f32) -> PVector { PVector::new3(self.x / n, self.y / n, self.z / n) } }
impl std::ops::Neg for PVector { type Output = PVector; fn neg(self) -> PVector { PVector::new3(-self.x, -self.y, -self.z) } }
impl std::ops::AddAssign for PVector { fn add_assign(&mut self, o: PVector) { *self = *self + o; } }
impl std::ops::SubAssign for PVector { fn sub_assign(&mut self, o: PVector) { *self = *self - o; } }
impl std::ops::MulAssign<f32> for PVector { fn mul_assign(&mut self, n: f32) { *self = *self * n; } }
impl std::ops::DivAssign<f32> for PVector { fn div_assign(&mut self, n: f32) { *self = *self / n; } }

/// Java's way of printing a float: always with a decimal point.
pub(crate) fn jfloat(v: f32) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e7 { format!("{v:.1}") }
    else if v.is_nan() { "NaN".into() }
    else if v.is_infinite() { if v > 0.0 { "Infinity".into() } else { "-Infinity".into() } }
    else { format!("{v}") }
}

impl std::fmt::Display for PVector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[ {}, {}, {} ]", jfloat(self.x), jfloat(self.y), jfloat(self.z))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-4 }
    #[test]
    fn basics() {
        let mut v = create_vector(3, 4);
        assert_eq!(v.mag(), 5.0);
        assert_eq!(v.mag_sq(), 25.0);
        v.add(&PVector::new(1.0, 1.0)).mult(2.0);
        assert_eq!(v, PVector::new(8.0, 10.0));
        v.sub_xyz(8.0, 10.0, -1.0).div(2.0);
        assert_eq!(v.array(), [0.0, 0.0, 0.5]);
        let x = PVector::new3(1.0, 0.0, 0.0);
        let y = PVector::new3(0.0, 1.0, 0.0);
        assert_eq!(x.cross(&y), PVector::new3(0.0, 0.0, 1.0));
        assert_eq!(x.dot(&y), 0.0);
        assert!(close(PVector::angle_between(&x, &y), std::f32::consts::FRAC_PI_2));
        assert_eq!(PVector::angle_between(&x, &PVector::default()), 0.0);
        assert!(close(x.dist(&y), 2f32.sqrt()));
        assert_eq!(format!("{}", create_vector3(1, 2.5, -3)), "[ 1.0, 2.5, -3.0 ]");
    }
    #[test]
    fn heading_limit_rotate_lerp() {
        let mut v = PVector::new(10.0, 0.0);
        v.limit(5.0);
        assert!(close(v.mag(), 5.0));
        v.set_mag(2.0);
        assert!(close(v.x, 2.0));
        v.rotate(std::f32::consts::FRAC_PI_2);
        assert!(close(v.x, 0.0) && close(v.y, 2.0));
        assert!(close(v.heading(), std::f32::consts::FRAC_PI_2));
        v.set_heading(0.0);
        assert!(close(v.x, 2.0) && close(v.y, 0.0));
        v.lerp(&PVector::new(4.0, 2.0), 0.5);
        assert!(close(v.x, 3.0) && close(v.y, 1.0));
        assert!(close(PVector::random_2d().mag(), 1.0));
        assert!(close(PVector::random_3d().mag(), 1.0));
    }
    #[test]
    fn operators() {
        let a = PVector::new(1.0, 2.0);
        let b = PVector::new(3.0, 4.0);
        assert_eq!(a + b, PVector::new(4.0, 6.0));
        assert_eq!(b - a, PVector::new(2.0, 2.0));
        assert_eq!(a * 2.0, PVector::new(2.0, 4.0));
        assert_eq!(2.0 * a, PVector::new(2.0, 4.0));
        assert_eq!(b / 2.0, PVector::new(1.5, 2.0));
        assert_eq!(-a, PVector::new(-1.0, -2.0));
        let mut c = a; c += b; c -= a; c *= 2.0; c /= 4.0;
        assert_eq!(c, PVector::new(1.5, 2.0));
    }
}
