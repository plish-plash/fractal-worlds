//! Continuous morphing by driving values forward.
//!
//! Only values that wrap can run forward forever without a seam: rotation
//! angles, bulb phases, a plane fold's direction, a 4D rotation, a Translate
//! that feeds a Cell repeat, the phase of the Julia constant on its circle,
//! and the light's azimuth. Each has a rate in cycles per minute; while the
//! morph plays, the value itself advances and wraps, so the sliders show
//! where it is and stopping leaves the shape exactly as it looks.

use crate::formula::wrap_range;
use crate::scene::Scene;

#[derive(Clone, Debug, PartialEq)]
pub struct Morph {
    pub playing: bool,
    /// Multiplier on every rate; negative runs the morph backwards.
    pub speed: f32,
}

impl Default for Morph {
    fn default() -> Self {
        Morph { playing: false, speed: 1.0 }
    }
}

pub fn wrap(v: f32, lo: f32, hi: f32) -> f32 {
    let span = hi - lo;
    if span <= 0.0 { v } else { lo + (v - lo).rem_euclid(span) }
}

/// One entry per value that can be driven: (operation index, parameter index) or a scene-level value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Op(usize, usize),
    JuliaCircle,
    LightAzimuth,
}

/// Every drivable value in the scene with a label for the Morph panel.
pub fn targets(sc: &Scene) -> Vec<(Target, String)> {
    let mut out = Vec::new();
    for (i, op) in sc.ops.iter().enumerate() {
        if !op.on {
            continue;
        }
        for (j, info) in op.kind.params().iter().enumerate() {
            if wrap_range(&sc.ops, i, j).is_some() {
                out.push((Target::Op(i, j), format!("{}. {}: {}", i + 1, op.kind.name(), info.name)));
            }
        }
    }
    out.push((Target::JuliaCircle, "Constant c: position on its circle".to_string()));
    out.push((Target::LightAzimuth, "Light azimuth".to_string()));
    out
}

impl Morph {
    /// Is anything actually moving?
    pub fn active(&self, sc: &Scene) -> bool {
        self.playing
            && self.speed != 0.0
            && (sc.fr.circle_drift != 0.0 && sc.fr.circle_radius != 0.0
                || sc.light.spin != 0.0
                || sc.ops.iter().enumerate().any(|(i, o)| o.on && (0..4).any(|j| o.drift[j] != 0.0 && wrap_range(&sc.ops, i, j).is_some())))
    }

    /// Advance every driven value by `dt` seconds.
    pub fn advance(&self, sc: &mut Scene, dt: f64) {
        if !self.playing {
            return;
        }
        let turns = |rate: f32| (rate as f64 * self.speed as f64 * dt / 60.0) as f32;
        for i in 0..sc.ops.len() {
            for j in 0..4 {
                let rate = sc.ops[i].drift[j];
                if rate == 0.0 || !sc.ops[i].on {
                    continue;
                }
                if let Some((lo, hi)) = wrap_range(&sc.ops, i, j) {
                    sc.ops[i].p[j] = wrap(sc.ops[i].p[j] + turns(rate) * (hi - lo), lo, hi);
                }
            }
        }
        if sc.fr.circle_drift != 0.0 {
            sc.fr.circle_phase = wrap(sc.fr.circle_phase + turns(sc.fr.circle_drift) * 360.0, -180.0, 180.0);
        }
        if sc.light.spin != 0.0 {
            sc.light.azimuth = wrap(sc.light.azimuth + turns(sc.light.spin) * 360.0, -180.0, 180.0);
        }
    }
}
