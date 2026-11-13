//! Small f64 vector / quaternion kit. The camera lives in f64 on the CPU so
//! it stays exact far below what a 32-bit GPU float can resolve.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct V3(pub f64, pub f64, pub f64);

impl V3 {
    pub const X: V3 = V3(1.0, 0.0, 0.0);
    pub const Y: V3 = V3(0.0, 1.0, 0.0);
    pub const Z: V3 = V3(0.0, 0.0, 1.0);
    pub fn dot(self, o: V3) -> f64 {
        self.0 * o.0 + self.1 * o.1 + self.2 * o.2
    }
    pub fn cross(self, o: V3) -> V3 {
        V3(self.1 * o.2 - self.2 * o.1, self.2 * o.0 - self.0 * o.2, self.0 * o.1 - self.1 * o.0)
    }
    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 0.0 { self * (1.0 / l) } else { V3::Z }
    }
    pub fn arr(self) -> [f64; 3] {
        [self.0, self.1, self.2]
    }
    pub fn from_arr(a: [f64; 3]) -> V3 {
        V3(a[0], a[1], a[2])
    }
    pub fn f32(self) -> [f32; 3] {
        [self.0 as f32, self.1 as f32, self.2 as f32]
    }
}
impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3(self.0 + o.0, self.1 + o.1, self.2 + o.2)
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3(self.0 - o.0, self.1 - o.1, self.2 - o.2)
    }
}
impl Mul<f64> for V3 {
    type Output = V3;
    fn mul(self, s: f64) -> V3 {
        V3(self.0 * s, self.1 * s, self.2 * s)
    }
}
impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        V3(-self.0, -self.1, -self.2)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Quat {
    pub const IDENT: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    pub fn axis_angle(axis: V3, angle: f64) -> Quat {
        let a = axis.norm();
        let (s, c) = (angle * 0.5).sin_cos();
        Quat { x: a.0 * s, y: a.1 * s, z: a.2 * s, w: c }
    }
    pub fn mul(self, o: Quat) -> Quat {
        Quat {
            x: self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            y: self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            z: self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
            w: self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
        }
    }
    pub fn norm(self) -> Quat {
        let l = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if l > 0.0 {
            Quat { x: self.x / l, y: self.y / l, z: self.z / l, w: self.w / l }
        } else {
            Quat::IDENT
        }
    }
    pub fn rotate(self, v: V3) -> V3 {
        let q = V3(self.x, self.y, self.z);
        let t = q.cross(v) * 2.0;
        v + t * self.w + q.cross(t)
    }
    /// Camera basis: X is right, Y is up, Z is forward.
    pub fn right(self) -> V3 {
        self.rotate(V3::X)
    }
    pub fn up(self) -> V3 {
        self.rotate(V3::Y)
    }
    pub fn fwd(self) -> V3 {
        self.rotate(V3::Z)
    }
    /// Column-major 3x3 (columns: right, up, forward) ready for a GLSL mat3.
    pub fn mat3(self) -> [f32; 9] {
        let (r, u, f) = (self.right(), self.up(), self.fwd());
        [r.0 as f32, r.1 as f32, r.2 as f32, u.0 as f32, u.1 as f32, u.2 as f32, f.0 as f32, f.1 as f32, f.2 as f32]
    }
    pub fn from_basis(r: V3, u: V3, f: V3) -> Quat {
        // Rotation matrix m[row][col] with columns r, u, f.
        let (m00, m01, m02) = (r.0, u.0, f.0);
        let (m10, m11, m12) = (r.1, u.1, f.1);
        let (m20, m21, m22) = (r.2, u.2, f.2);
        let tr = m00 + m11 + m22;
        let q = if tr > 0.0 {
            let s = (tr + 1.0).sqrt() * 2.0;
            Quat { w: 0.25 * s, x: (m21 - m12) / s, y: (m02 - m20) / s, z: (m10 - m01) / s }
        } else if m00 > m11 && m00 > m22 {
            let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
            Quat { w: (m21 - m12) / s, x: 0.25 * s, y: (m01 + m10) / s, z: (m02 + m20) / s }
        } else if m11 > m22 {
            let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
            Quat { w: (m02 - m20) / s, x: (m01 + m10) / s, y: 0.25 * s, z: (m12 + m21) / s }
        } else {
            let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
            Quat { w: (m10 - m01) / s, x: (m02 + m20) / s, y: (m12 + m21) / s, z: 0.25 * s }
        };
        q.norm()
    }
    pub fn look(from: V3, to: V3) -> Quat {
        let f = (to - from).norm();
        let mut r = V3::Y.cross(f);
        if r.len() < 1e-9 {
            r = V3::X;
        }
        let r = r.norm();
        Quat::from_basis(r, f.cross(r), f)
    }
    pub fn arr(self) -> [f64; 4] {
        [self.x, self.y, self.z, self.w]
    }
    pub fn from_arr(a: [f64; 4]) -> Quat {
        Quat { x: a[0], y: a[1], z: a[2], w: a[3] }.norm()
    }
}

/// Rotation from Euler angles in degrees (applied X, then Y, then Z), column-major.
pub fn euler_mat3(deg: [f32; 3]) -> [f32; 9] {
    let q = Quat::axis_angle(V3::Z, (deg[2] as f64).to_radians())
        .mul(Quat::axis_angle(V3::Y, (deg[1] as f64).to_radians()))
        .mul(Quat::axis_angle(V3::X, (deg[0] as f64).to_radians()));
    q.mat3()
}
