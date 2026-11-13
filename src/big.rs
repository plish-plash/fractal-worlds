//! Arbitrary-precision fixed-point numbers for the camera position and its
//! reference orbit. Deep zoom needs the camera to far more digits than f64
//! holds; everything the GPU sees is an offset from these values.
//!
//! A value is sign + magnitude, `INT` whole limbs and a variable number of
//! fractional limbs (32 bits each, least significant first). All operands of
//! one operation must have the same number of fractional limbs.

use std::cmp::Ordering;

pub const INT: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub struct Big {
    neg: bool,
    m: Vec<u32>,
}

fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    Ordering::Equal
}
fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut carry = 0u64;
    for i in 0..a.len() {
        let s = a[i] as u64 + b[i] as u64 + carry;
        out.push(s as u32);
        carry = s >> 32;
    }
    out
}
/// a - b, requires |a| >= |b|.
fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0i64;
    for i in 0..a.len() {
        let mut s = a[i] as i64 - b[i] as i64 - borrow;
        borrow = 0;
        if s < 0 {
            s += 1 << 32;
            borrow = 1;
        }
        out.push(s as u32);
    }
    out
}

/// x * 2^e without intermediate overflow or underflow.
pub fn ldexp(mut x: f64, mut e: i64) -> f64 {
    while e > 900 {
        x *= 2f64.powi(900);
        e -= 900;
    }
    while e < -900 {
        x *= 2f64.powi(-900);
        e += 900;
    }
    x * 2f64.powi(e as i32)
}

impl Big {
    pub fn zero(frac: usize) -> Big {
        Big { neg: false, m: vec![0; frac + INT] }
    }
    pub fn frac(&self) -> usize {
        self.m.len() - INT
    }
    pub fn is_zero(&self) -> bool {
        self.m.iter().all(|&l| l == 0)
    }
    pub fn is_neg(&self) -> bool {
        self.neg && !self.is_zero()
    }

    /// mant * 2^exp, truncated to the available fractional limbs.
    pub fn from_parts(mant: f64, exp: i64, frac: usize) -> Big {
        let mut out = Big::zero(frac);
        if mant == 0.0 || !mant.is_finite() {
            return out;
        }
        out.neg = mant < 0.0;
        let bits = mant.abs().to_bits();
        let raw_e = ((bits >> 52) & 0x7ff) as i64;
        let (m, e) = if raw_e == 0 { (bits & ((1 << 52) - 1), -1074) } else { ((bits & ((1 << 52) - 1)) | (1 << 52), raw_e - 1075) };
        // |value| = m * 2^(e + exp); in fixed point that is m << (e + exp + 32*frac).
        let shift = e + exp + 32 * frac as i64;
        let (m, shift) = if shift < 0 {
            if shift <= -64 {
                return Big::zero(frac);
            }
            (m >> (-shift), 0)
        } else {
            (m, shift)
        };
        let (limb, bit) = ((shift / 32) as usize, (shift % 32) as u32);
        let wide = (m as u128) << bit;
        for k in 0..4 {
            if limb + k < out.m.len() {
                out.m[limb + k] = (wide >> (32 * k)) as u32;
            }
        }
        out
    }
    pub fn from_f64(x: f64, frac: usize) -> Big {
        Big::from_parts(x, 0, frac)
    }

    /// Same value with a different number of fractional limbs.
    pub fn resized(&self, frac: usize) -> Big {
        let old = self.frac();
        let mut m = vec![0u32; frac + INT];
        for i in 0..m.len() {
            let src = i as i64 + old as i64 - frac as i64;
            if src >= 0 && (src as usize) < self.m.len() {
                m[i] = self.m[src as usize];
            }
        }
        Big { neg: self.neg, m }
    }

    pub fn neg(&self) -> Big {
        Big { neg: !self.neg, m: self.m.clone() }
    }
    pub fn abs(&self) -> Big {
        Big { neg: false, m: self.m.clone() }
    }
    pub fn add(&self, o: &Big) -> Big {
        if self.neg == o.neg {
            return Big { neg: self.neg, m: add_mag(&self.m, &o.m) };
        }
        match cmp_mag(&self.m, &o.m) {
            Ordering::Less => Big { neg: o.neg, m: sub_mag(&o.m, &self.m) },
            _ => Big { neg: self.neg, m: sub_mag(&self.m, &o.m) },
        }
    }
    pub fn sub(&self, o: &Big) -> Big {
        self.add(&o.neg())
    }
    pub fn cmp(&self, o: &Big) -> Ordering {
        match (self.is_neg(), o.is_neg()) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => cmp_mag(&self.m, &o.m),
            (true, true) => cmp_mag(&o.m, &self.m),
        }
    }
    pub fn lt(&self, o: &Big) -> bool {
        self.cmp(o) == Ordering::Less
    }

    pub fn mul(&self, o: &Big) -> Big {
        let n = self.m.len();
        let f = self.frac();
        let mut prod = vec![0u32; 2 * n];
        for i in 0..n {
            let a = self.m[i] as u64;
            if a == 0 {
                continue;
            }
            let mut carry = 0u64;
            for j in 0..n {
                let t = a * o.m[j] as u64 + prod[i + j] as u64 + carry;
                prod[i + j] = t as u32;
                carry = t >> 32;
            }
            prod[i + n] = carry as u32;
        }
        Big { neg: self.neg != o.neg, m: prod[f..f + n].to_vec() }
    }
    pub fn mul_f(&self, x: f64) -> Big {
        self.mul(&Big::from_f64(x, self.frac()))
    }
    pub fn half(&self) -> Big {
        let mut m = self.m.clone();
        let mut carry = 0u32;
        for l in m.iter_mut().rev() {
            let next = *l & 1;
            *l = (*l >> 1) | (carry << 31);
            carry = next;
        }
        Big { neg: self.neg, m }
    }

    /// (mantissa, binary exponent) with value = mantissa * 2^exponent, good to f64 precision.
    pub fn mant_exp(&self) -> (f64, i64) {
        let Some(t) = (0..self.m.len()).rev().find(|&i| self.m[i] != 0) else { return (0.0, 0) };
        let get = |i: i64| if i >= 0 { self.m[i as usize] as f64 } else { 0.0 };
        let t = t as i64;
        let v = get(t) * 18446744073709551616.0 + get(t - 1) * 4294967296.0 + get(t - 2);
        (if self.neg { -v } else { v }, 32 * (t - 2) - 32 * self.frac() as i64)
    }
    pub fn to_f64(&self) -> f64 {
        let (m, e) = self.mant_exp();
        ldexp(m, e)
    }
    /// self / unit as an f32, saturated to +-1e30. `unit` may be far below f32 range.
    pub fn scaled(&self, unit: f64) -> f32 {
        let (m, e) = self.mant_exp();
        if m == 0.0 {
            return 0.0;
        }
        // Work with logarithms first so nothing overflows on the way.
        let lg = m.abs().log2() + e as f64 - unit.log2();
        if lg > 99.0 {
            return if m < 0.0 { -1e30 } else { 1e30 };
        }
        if lg < -140.0 {
            return 0.0;
        }
        let ue = unit.log2().floor() as i64;
        (ldexp(m, e - ue) / ldexp(unit, -ue)).clamp(-1e30, 1e30) as f32
    }

    /// Largest whole number not above the value (for small magnitudes only).
    pub fn floor_i64(&self) -> i64 {
        let f = self.frac();
        let whole = self.m[f] as i64 | ((self.m[f + 1] as i64 & 0x7fff_ffff) << 32);
        let has_frac = self.m[..f].iter().any(|&l| l != 0);
        if self.is_neg() {
            if has_frac { -whole - 1 } else { -whole }
        } else {
            whole
        }
    }

    /// 1 / self by Newton's method from an f64 seed. None if the result does not fit.
    pub fn recip(&self) -> Option<Big> {
        let (m, e) = self.mant_exp();
        if m == 0.0 || (-(m.abs().log2() + e as f64)) > 80.0 {
            return None;
        }
        let f = self.frac();
        let two = Big::from_f64(2.0, f);
        let mut y = Big::from_parts(1.0 / m, -e, f);
        let rounds = 3 + ((32 * (f + INT)) as f64 / 50.0).log2().ceil().max(0.0) as usize;
        for _ in 0..rounds {
            y = y.mul(&two.sub(&self.mul(&y)));
        }
        Some(y)
    }
    pub fn div(&self, o: &Big) -> Option<Big> {
        o.recip().map(|r| self.mul(&r))
    }
    /// Square root of a non-negative value. None if it is too close to zero to invert.
    pub fn sqrt(&self) -> Option<Big> {
        if self.is_zero() {
            return Some(self.clone());
        }
        if self.neg {
            return None;
        }
        let (mut m, mut e) = self.mant_exp();
        if e.rem_euclid(2) == 1 {
            m *= 2.0;
            e -= 1;
        }
        if -(m.log2() + e as f64) * 0.5 > 80.0 {
            return None;
        }
        let f = self.frac();
        let three = Big::from_f64(3.0, f);
        let mut y = Big::from_parts(1.0 / m.sqrt(), -e / 2, f);
        let rounds = 3 + ((32 * (f + INT)) as f64 / 50.0).log2().ceil().max(0.0) as usize;
        for _ in 0..rounds {
            y = y.mul(&three.sub(&self.mul(&y).mul(&y))).half();
        }
        Some(self.mul(&y))
    }

    pub fn to_text(&self) -> String {
        let limbs: Vec<String> = self.m.iter().rev().map(|l| format!("{:08x}", l)).collect();
        format!("{}{}", if self.neg { "-" } else { "+" }, limbs.join(""))
    }
    pub fn from_text(s: &str) -> Option<Big> {
        let s = s.trim();
        let (neg, hex) = (s.starts_with('-'), s.trim_start_matches(|c| c == '+' || c == '-'));
        if hex.len() % 8 != 0 || hex.len() < 8 * (INT + 1) || !hex.is_ascii() {
            return None;
        }
        let mut m = Vec::new();
        for i in (0..hex.len() / 8).rev() {
            m.push(u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).ok()?);
        }
        Some(Big { neg, m })
    }
}

/// A point in space at camera precision.
#[derive(Clone, Debug, PartialEq)]
pub struct BigV(pub [Big; 3]);

impl BigV {
    pub fn from_f64(p: [f64; 3], frac: usize) -> BigV {
        BigV([Big::from_f64(p[0], frac), Big::from_f64(p[1], frac), Big::from_f64(p[2], frac)])
    }
    pub fn frac(&self) -> usize {
        self.0[0].frac()
    }
    pub fn to_f64(&self) -> [f64; 3] {
        [self.0[0].to_f64(), self.0[1].to_f64(), self.0[2].to_f64()]
    }
    /// Make sure there are at least `frac` fractional limbs (never drops precision).
    pub fn ensure(&mut self, frac: usize) {
        if self.frac() < frac {
            for c in self.0.iter_mut() {
                *c = c.resized(frac);
            }
        }
    }
    pub fn add_f64(&mut self, d: [f64; 3]) {
        let f = self.frac();
        for i in 0..3 {
            self.0[i] = self.0[i].add(&Big::from_f64(d[i], f));
        }
    }
}

/// Fractional limbs needed to work at view scale `s` with ~96 guard bits.
pub fn frac_for_scale(s: f64) -> usize {
    ((-(s.max(1e-300)).log2()).max(0.0) / 32.0).ceil() as usize + 3
}
