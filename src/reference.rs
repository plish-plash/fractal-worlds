//! The camera's reference orbit.
//!
//! For unlimited zoom the GPU never sees absolute coordinates. Instead the CPU
//! iterates the camera's own position through the formula at full precision,
//! once per frame, and the shader iterates only each point's offset from it.
//! This module runs that orbit and records, for every level and operation,
//! exactly the few numbers the offset form of that operation needs: mostly
//! the reference's distance to each fold boundary, measured in units of the
//! zoom scale at that level so they fit comfortably in 32-bit floats.
//!
//! Units: `s` is the zoom scale (roughly the distance from the camera to the
//! nearest surface). `u` starts at `s` and is multiplied by every scale factor
//! the reference meets, so an offset expressed in units of `u` stays about the
//! same size from level to level. Once `u` reaches ordinary size the shader
//! switches to absolute coordinates and the orbit can stop.

use crate::big::{Big, BigV};
use crate::formula::{plane_normal, Op, OpKind, MAX_OPS, REF_TEXELS, REF_WIDTH};
use crate::math::euler_mat3;

/// Offset size, in level coordinates, at which a point leaves the offset form.
pub const TAU: f32 = 0.05;
/// The orbit stops once the level unit reaches this size.
const U_STOP: f64 = 2.0;

pub struct RefOrbit {
    /// Levels that can be followed in offset form; the texture has one more row.
    pub levels: usize,
    /// Iterations every point runs in total: the levels it takes the camera's
    /// surroundings to reach ordinary size, plus the scene's detail iterations.
    pub total: usize,
    /// REF_WIDTH x (levels + 1) RGBA texels.
    pub data: Vec<f32>,
}

type Z4 = [Big; 4];

fn put(row: &mut [f32], texel: usize, v: [f32; 4]) {
    row[texel * 4..texel * 4 + 4].copy_from_slice(&v);
}
fn f32s(z: &Z4) -> [f32; 4] {
    [z[0].to_f64() as f32, z[1].to_f64() as f32, z[2].to_f64() as f32, z[3].to_f64() as f32]
}
/// (a + ib)^n by repeated squaring.
fn cpow(a: &Big, b: &Big, mut n: u32) -> (Big, Big) {
    let f = a.frac();
    let (mut ra, mut rb) = (Big::from_f64(1.0, f), Big::zero(f));
    let (mut ba, mut bb) = (a.clone(), b.clone());
    while n > 0 {
        if n & 1 == 1 {
            let t = ra.mul(&ba).sub(&rb.mul(&bb));
            rb = ra.mul(&bb).add(&rb.mul(&ba));
            ra = t;
        }
        let t = ba.mul(&ba).sub(&bb.mul(&bb));
        bb = ba.mul(&bb).add(&ba.mul(&bb));
        ba = t;
        n >>= 1;
    }
    (ra, rb)
}
fn rpow(r: &Big, mut n: u32) -> Big {
    let mut out = Big::from_f64(1.0, r.frac());
    let mut base = r.clone();
    while n > 0 {
        if n & 1 == 1 {
            out = out.mul(&base);
        }
        base = base.mul(&base);
        n >>= 1;
    }
    out
}

/// Sphere fold and inversion: z *= num / clamp(|z|^2, lo, hi).
fn radial(z: &mut Z4, lo: f64, hi: Option<f64>, num: f64, u: f64, t: &mut [[f32; 4]; 3]) -> Option<f64> {
    let f = z[0].frac();
    let r2 = z[0].mul(&z[0]).add(&z[1].mul(&z[1])).add(&z[2].mul(&z[2]));
    let lo_b = Big::from_f64(lo, f);
    let hi_b = hi.map(|h| Big::from_f64(h, f));
    let cr = if r2.lt(&lo_b) {
        lo_b.clone()
    } else {
        match &hi_b {
            Some(h) if !r2.lt(h) => h.clone(),
            _ => r2.clone(),
        }
    };
    let zf = f32s(z);
    t[0] = [zf[0], zf[1], zf[2], cr.to_f64() as f32];
    t[1] = [lo_b.sub(&r2).scaled(u), hi_b.map(|h| h.sub(&r2).scaled(u)).unwrap_or(1e30), 0.0, 0.0];
    let k = Big::from_f64(num, f).div(&cr)?;
    for c in z.iter_mut().take(3) {
        *c = c.mul(&k);
    }
    Some(k.to_f64())
}

/// Record what the offset form of `op` needs, then apply `op` to the reference.
/// Returns the reference's scale factor, or None if the orbit cannot continue
/// (for example the reference sits exactly on a pole).
fn apply(op: &Op, z: &mut Z4, c: &Z4, u: f64, s: f64, t: &mut [[f32; 4]; 3]) -> Option<f64> {
    let f = z[0].frac();
    let b = |x: f64| Big::from_f64(x, f);
    let p = |k: usize| op.p[k] as f64;
    let mut g = 1.0f64;
    match op.kind {
        OpKind::BoxFold => {
            for k in 0..3 {
                let l = b(p(k));
                let lo = l.neg();
                t[0][k] = lo.sub(&z[k]).scaled(u);
                t[1][k] = l.sub(&z[k]).scaled(u);
                if l.lt(&z[k]) {
                    z[k] = l.add(&l).sub(&z[k]);
                } else if z[k].lt(&lo) {
                    z[k] = lo.add(&lo).sub(&z[k]);
                }
            }
        }
        OpKind::SphereFold => {
            let mr = (op.p[0] * op.p[0]).max(1e-9) as f64;
            let fr = (op.p[1] * op.p[1]).max(1e-9) as f64;
            g = radial(z, mr, Some(fr), fr, u, t)?;
        }
        OpKind::Inversion => {
            let hi = if op.p[1] > 0.0 { Some(p(0) / p(1)) } else { None };
            g = radial(z, 1e-24, hi, p(0), u, t)?;
        }
        OpKind::AbsFold => {
            for k in 0..3 {
                let o = b(p(k));
                let w = z[k].add(&o);
                t[0][k] = w.scaled(u);
                z[k] = w.abs().sub(&o);
            }
        }
        OpKind::SortFold => {
            for (n, (i, j)) in [(0usize, 1usize), (0, 2), (1, 2)].into_iter().enumerate() {
                t[0][n] = z[i].sub(&z[j]).scaled(u);
                if z[i].lt(&z[j]) {
                    z.swap(i, j);
                }
            }
        }
        OpKind::TetraFold => {
            for (n, (i, j)) in [(0usize, 1usize), (0, 2), (1, 2)].into_iter().enumerate() {
                let sum = z[i].add(&z[j]);
                t[0][n] = sum.scaled(u);
                if sum.is_neg() {
                    let (a, bb) = (z[j].neg(), z[i].neg());
                    z[i] = a;
                    z[j] = bb;
                }
            }
        }
        OpKind::PlaneFold => {
            let n = plane_normal(&op.p);
            let dist = z[0].mul_f(n[0] as f64).add(&z[1].mul_f(n[1] as f64)).add(&z[2].mul_f(n[2] as f64)).sub(&b(p(2)));
            t[0][0] = dist.scaled(u);
            if dist.is_neg() {
                for k in 0..3 {
                    z[k] = z[k].sub(&dist.mul_f(2.0 * n[k] as f64));
                }
            }
        }
        OpKind::Rotate => {
            let m = euler_mat3([op.p[0], op.p[1], op.p[2]]);
            let old = [z[0].clone(), z[1].clone(), z[2].clone()];
            for k in 0..3 {
                z[k] = old[0].mul_f(m[k] as f64).add(&old[1].mul_f(m[3 + k] as f64)).add(&old[2].mul_f(m[6 + k] as f64));
            }
        }
        OpKind::Rotate4D => {
            for k in 0..3 {
                let a = op.p[k].to_radians() as f64;
                let (sn, cs) = a.sin_cos();
                let (x, w) = (z[k].clone(), z[3].clone());
                z[k] = x.mul_f(cs).sub(&w.mul_f(sn));
                z[3] = x.mul_f(sn).add(&w.mul_f(cs));
            }
        }
        OpKind::Scale => {
            if op.p[0].abs() < 1e-6 {
                return None;
            }
            for k in 0..3 {
                z[k] = z[k].mul_f(p(0)).sub(&b((op.p[k + 1] * (op.p[0] - 1.0)) as f64));
            }
            g = p(0).abs();
        }
        OpKind::MengerScale => {
            let sc = p(0).max(1e-6);
            let o = |k: usize| (op.p[k + 1] * (op.p[0] - 1.0)) as f64;
            z[0] = z[0].mul_f(sc).sub(&b(o(0)));
            z[1] = z[1].mul_f(sc).sub(&b(o(1)));
            let h = b((0.5 * op.p[3] * (op.p[0] - 1.0)) as f64);
            let q = z[2].mul_f(sc).sub(&h);
            t[0][0] = q.scaled(u * sc);
            z[2] = h.sub(&q.abs());
            g = sc;
        }
        OpKind::Translate => {
            for k in 0..3 {
                z[k] = z[k].add(&b(p(k)));
            }
        }
        OpKind::AddC => {
            for k in 0..4 {
                z[k] = z[k].add(&c[k].mul_f(p(0)));
            }
            g = (u + p(0).abs() * s) / u;
        }
        OpKind::BulbPow => {
            let n = p(0);
            let xy2 = z[0].mul(&z[0]).add(&z[1].mul(&z[1]));
            let r = xy2.add(&z[2].mul(&z[2])).sqrt()?;
            let rf = r.to_f64();
            if rf <= 0.0 {
                return None;
            }
            let zf = f32s(z);
            let (st, ct, sp, cp, rn);
            if (n - n.round()).abs() < 1e-6 && (1.0..=64.0).contains(&n) {
                // Whole powers need no trigonometry: raise the unit complex numbers
                // (cos t, sin t) and (cos p, sin p) to the n-th power instead.
                let ni = n.round() as u32;
                let rho = xy2.sqrt()?;
                let inv_r = r.recip()?;
                let (cp0, sp0) = if rho.is_zero() {
                    (b(1.0), b(0.0))
                } else {
                    let ir = rho.recip()?;
                    (z[0].mul(&ir), z[1].mul(&ir))
                };
                let (ctn, stn) = cpow(&z[2].mul(&inv_r), &rho.mul(&inv_r), ni);
                let (cpn, spn) = cpow(&cp0, &sp0, ni);
                let (sa, ca) = p(1).sin_cos();
                let (sb, cb) = p(2).sin_cos();
                ct = ctn.mul_f(ca).sub(&stn.mul_f(sa));
                st = stn.mul_f(ca).add(&ctn.mul_f(sa));
                cp = cpn.mul_f(cb).sub(&spn.mul_f(sb));
                sp = spn.mul_f(cb).add(&cpn.mul_f(sb));
                rn = rpow(&r, ni);
            } else {
                // Fractional powers fall back to f64, which limits how deep the
                // reference stays exact (about 12 orders of magnitude).
                let (x, y, zz) = (z[0].to_f64(), z[1].to_f64(), z[2].to_f64());
                let th = (zz / rf).clamp(-1.0, 1.0).acos() * n + p(1);
                let ph = y.atan2(x) * n + p(2);
                st = b(th.sin());
                ct = b(th.cos());
                sp = b(ph.sin());
                cp = b(ph.cos());
                rn = b(rf.powf(n));
            }
            t[0] = [zf[0], zf[1], zf[2], rf as f32];
            t[1] = [st.to_f64() as f32, ct.to_f64() as f32, sp.to_f64() as f32, cp.to_f64() as f32];
            t[2][0] = rn.to_f64() as f32;
            z[0] = rn.mul(&st).mul(&cp);
            z[1] = rn.mul(&st).mul(&sp);
            z[2] = rn.mul(&ct);
            g = n * rf.powf(n - 1.0);
            if !(g > 1e-300) {
                return None;
            }
        }
        OpKind::QuatSquare => {
            t[0] = f32s(z);
            let r = z.iter().map(|v| v.to_f64().powi(2)).sum::<f64>().sqrt();
            if !(r > 0.0) {
                return None;
            }
            let x = z[0].clone();
            z[0] = x.mul(&x).sub(&z[1].mul(&z[1])).sub(&z[2].mul(&z[2])).sub(&z[3].mul(&z[3]));
            for k in 1..4 {
                z[k] = x.mul(&z[k]).mul_f(2.0);
            }
            g = 2.0 * r;
        }
        OpKind::CellFold => {
            for k in 0..3 {
                let h = op.p[k].max(1e-3) as f64;
                let (hb, two_h) = (b(h), b(2.0 * h));
                let mut cell = z[k].add(&hb).mul(&two_h.recip()?).floor_i64();
                let mut loc = z[k].sub(&two_h.mul_f(cell as f64));
                // The reciprocal is rounded; settle the cell index exactly.
                while !loc.lt(&hb) {
                    cell += 1;
                    loc = loc.sub(&two_h);
                }
                while loc.lt(&hb.neg()) {
                    cell -= 1;
                    loc = loc.add(&two_h);
                }
                let mirrored = cell.rem_euclid(2) == 1;
                t[0][k] = hb.neg().sub(&loc).scaled(u);
                t[1][k] = hb.sub(&loc).scaled(u);
                t[2][k] = if mirrored { -1.0 } else { 1.0 };
                z[k] = if mirrored { loc.neg() } else { loc };
            }
        }
    }
    Some(g)
}

/// Run the camera's orbit. `julia` is the constant c already including any
/// circling, `s` the zoom scale, `iters` the scene's detail iterations; `cam`
/// must carry enough fractional limbs for that scale (see big::frac_for_scale).
pub fn build(ops: &[Op], julia: [f32; 4], julia_mix: f32, bailout: f32, iters: i32, cam: &BigV, s: f64) -> RefOrbit {
    let f = cam.frac();
    let iters = iters.max(1) as usize;
    let mut z: Z4 = [cam.0[0].clone(), cam.0[1].clone(), cam.0[2].clone(), Big::zero(f)];
    let keep = (1.0f32 - julia_mix) as f64;
    let c: Z4 = [
        cam.0[0].mul_f(keep).add(&Big::from_f64((julia[0] * julia_mix) as f64, f)),
        cam.0[1].mul_f(keep).add(&Big::from_f64((julia[1] * julia_mix) as f64, f)),
        cam.0[2].mul_f(keep).add(&Big::from_f64((julia[2] * julia_mix) as f64, f)),
        Big::from_f64((julia[3] * julia_mix) as f64, f),
    ];
    let bail2 = (bailout as f64).min(1.0e6).powi(2);
    let cap = (48.0 + 8.0 * (-s.log10()).max(0.0)).min(4000.0) as usize + iters;
    let mut u = s;
    let mut data = Vec::new();
    let mut units: Vec<f64> = Vec::new();
    // Level at which the surroundings stopped growing (camera in open space, or
    // next to a smooth wall): the unit will not even double over eight levels.
    let mut stall: Option<usize> = None;
    #[allow(unused_assignments)]
    let mut expanded = false;
    let mut n = 0usize;
    loop {
        units.push(u);
        if stall.is_none() && n >= 8 && units[n] < 2.0 * units[n - 8] {
            stall = Some(n - 8);
        }
        let mut row = vec![0.0f32; REF_WIDTH * 4];
        put(&mut row, 0, f32s(&z));
        put(&mut row, 1, [u as f32, (s / u) as f32, z[1].scaled(u), 0.0]);
        let r2: f64 = z.iter().map(|v| v.to_f64().powi(2)).sum();
        expanded = !(u < U_STOP);
        let stop = n >= cap || expanded || r2 > bail2 || stall.map(|k| n >= k + iters).unwrap_or(false);
        let mut ok = !stop;
        if ok {
            for (j, op) in ops.iter().enumerate().take(MAX_OPS) {
                if !op.runs_on(n as i32) {
                    continue;
                }
                let mut t = [[0.0f32; 4]; 3];
                match apply(op, &mut z, &c, u, s, &mut t) {
                    Some(g) if g.is_finite() && g > 0.0 => {
                        let x = 2 + REF_TEXELS * j;
                        put(&mut row, x, t[0]);
                        put(&mut row, x + 1, t[1]);
                        put(&mut row, x + 2, t[2]);
                        put(&mut row, x + 3, [u as f32, g as f32, (s / u) as f32, 0.0]);
                        u *= g;
                    }
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
        }
        data.extend_from_slice(&row);
        if !ok {
            break;
        }
        n += 1;
    }
    // Where the surroundings grew to ordinary size the shader switches to
    // absolute coordinates there. Where they stalled it keeps following the
    // reference to the last iteration, so a smooth wall an arm's length away
    // at any depth is still resolved from offsets.
    let base = if expanded { n } else { stall.unwrap_or(n).min(n) };
    let total = base + iters;
    let levels = n.min(total);
    data.truncate((levels + 1) * REF_WIDTH * 4);
    RefOrbit { levels, total, data }
}
