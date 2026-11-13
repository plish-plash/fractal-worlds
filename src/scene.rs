//! Everything that describes a world: formula stack, look, camera, keyframes.
//! Also the plain-text scene format and the list of uniforms sent to the GPU.

use crate::big::{Big, BigV};
use crate::formula::{plane_normal, Op, OpKind, MAX_OPS};
use crate::math::{euler_mat3, Quat, V3};
use crate::reference::TAU;

// ---------------------------------------------------------------- text codec

pub trait Enc: Sized {
    fn enc(&self) -> String;
    fn dec(s: &str) -> Option<Self>;
}
impl Enc for f32 {
    fn enc(&self) -> String {
        format!("{}", self)
    }
    fn dec(s: &str) -> Option<Self> {
        s.trim().parse().ok()
    }
}
impl Enc for i32 {
    fn enc(&self) -> String {
        format!("{}", self)
    }
    fn dec(s: &str) -> Option<Self> {
        s.trim().parse().ok()
    }
}
impl Enc for bool {
    fn enc(&self) -> String {
        (if *self { "1" } else { "0" }).to_string()
    }
    fn dec(s: &str) -> Option<Self> {
        Some(s.trim() == "1" || s.trim() == "true")
    }
}
fn floats(s: &str) -> Vec<f32> {
    s.split_whitespace().filter_map(|t| t.parse().ok()).collect()
}
fn doubles(s: &str) -> Vec<f64> {
    s.split_whitespace().filter_map(|t| t.parse().ok()).collect()
}
fn join<T: std::fmt::Display>(v: &[T]) -> String {
    v.iter().map(|x| format!("{}", x)).collect::<Vec<_>>().join(" ")
}
impl<const N: usize> Enc for [f32; N] {
    fn enc(&self) -> String {
        join(self)
    }
    fn dec(s: &str) -> Option<Self> {
        floats(s).try_into().ok()
    }
}
impl Enc for [[f32; 3]; 5] {
    fn enc(&self) -> String {
        join(&self.concat())
    }
    fn dec(s: &str) -> Option<Self> {
        let v = floats(s);
        if v.len() != 15 {
            return None;
        }
        let mut o = [[0.0; 3]; 5];
        for i in 0..15 {
            o[i / 3][i % 3] = v[i];
        }
        Some(o)
    }
}

/// Declares a settings struct with defaults plus save/load of every field.
macro_rules! settings {
    ($name:ident { $($f:ident : $t:ty = $d:expr),* $(,)? }) => {
        #[derive(Clone, PartialEq, Debug)]
        pub struct $name { $(pub $f: $t),* }
        impl Default for $name {
            fn default() -> Self { Self { $($f: $d),* } }
        }
        impl $name {
            pub fn save(&self, prefix: &str, out: &mut String) {
                $( out.push_str(&format!("{}.{} = {}\n", prefix, stringify!($f), self.$f.enc())); )*
            }
            pub fn load(&mut self, key: &str, val: &str) {
                $( if key == stringify!($f) { if let Some(v) = <$t as Enc>::dec(val) { self.$f = v; } } )*
            }
        }
    };
}

settings!(Fractal {
    iters: i32 = 12,
    bailout: f32 = 4.0,
    de_mode: i32 = 0,
    de_param: f32 = 0.0,
    julia: [f32; 4] = [0.0, 0.0, 0.0, 0.0],
    julia_mix: f32 = 0.0,
    circle_radius: f32 = 0.0,
    circle_phase: f32 = 0.0,
    circle_plane: i32 = 0,
    circle_drift: f32 = 0.0,
});

settings!(Quality {
    max_steps: i32 = 320,
    detail: f32 = 1.0,
    step_scale: f32 = 0.9,
    far: f32 = 60.0,
    fov: f32 = 70.0,
    shadows: bool = true,
    shadow_soft: f32 = 10.0,
    ao: f32 = 0.7,
    res_scale: f32 = 1.0,
    samples: i32 = 32,
});

settings!(Color {
    stops: [[f32; 3]; 5] = PALETTES[0].1,
    src: i32 = 0,
    src2: i32 = 3,
    src_mix: f32 = 0.0,
    freq: f32 = 1.0,
    offset: f32 = 0.0,
    cycle: f32 = 0.0,
});

settings!(Material {
    rough: f32 = 0.5,
    metal: f32 = 0.0,
    spec: f32 = 1.0,
    rim: f32 = 0.2,
    irid: f32 = 0.0,
    emit: f32 = 0.0,
    reflect: f32 = 0.0,
    glow: f32 = 0.25,
    glow_col: [f32; 3] = [1.0, 0.75, 0.45],
});

settings!(Light {
    headlamp: bool = false,
    azimuth: f32 = 35.0,
    elevation: f32 = 40.0,
    sun_col: [f32; 3] = [1.0, 0.95, 0.88],
    spin: f32 = 0.0,
    sun_int: f32 = 3.2,
    sky_top: [f32; 3] = [0.16, 0.27, 0.52],
    sky_hor: [f32; 3] = [0.74, 0.72, 0.70],
    ambient: f32 = 0.55,
    fog: f32 = 0.02,
    fog_col: [f32; 3] = [0.62, 0.64, 0.70],
});

settings!(Post {
    exposure: f32 = 1.0,
    tonemap: i32 = 0,
    gamma: f32 = 2.2,
    saturation: f32 = 1.0,
    contrast: f32 = 1.0,
    vignette: f32 = 0.35,
});

pub const PALETTES: [(&str, [[f32; 3]; 5]); 8] = [
    ("Ember", [[0.05, 0.02, 0.08], [0.55, 0.10, 0.12], [0.95, 0.45, 0.10], [1.0, 0.85, 0.55], [0.25, 0.20, 0.35]]),
    ("Glacier", [[0.02, 0.06, 0.15], [0.05, 0.35, 0.55], [0.55, 0.85, 0.90], [0.95, 0.98, 1.0], [0.15, 0.25, 0.45]]),
    ("Verdigris", [[0.05, 0.10, 0.08], [0.10, 0.45, 0.38], [0.70, 0.80, 0.55], [0.80, 0.50, 0.25], [0.25, 0.15, 0.10]]),
    ("Bone", [[0.85, 0.80, 0.72], [0.55, 0.50, 0.45], [0.95, 0.93, 0.88], [0.35, 0.30, 0.28], [0.70, 0.62, 0.52]]),
    ("Spectrum", [[0.90, 0.20, 0.20], [0.95, 0.80, 0.20], [0.20, 0.80, 0.40], [0.20, 0.40, 0.95], [0.80, 0.30, 0.85]]),
    ("Orchid", [[0.10, 0.03, 0.15], [0.55, 0.15, 0.55], [0.95, 0.55, 0.75], [0.98, 0.90, 0.80], [0.30, 0.15, 0.45]]),
    ("Gold and ink", [[0.02, 0.02, 0.03], [0.20, 0.15, 0.08], [0.85, 0.62, 0.20], [1.0, 0.92, 0.65], [0.10, 0.12, 0.20]]),
    ("Reef", [[0.02, 0.25, 0.30], [0.10, 0.60, 0.60], [0.98, 0.50, 0.40], [1.0, 0.80, 0.65], [0.05, 0.15, 0.25]]),
];

/// (name, roughness, metallic, specular, rim, iridescence, emission, reflection)
pub const MATERIALS: [(&str, [f32; 7]); 8] = [
    ("Matte clay", [0.85, 0.0, 0.5, 0.10, 0.0, 0.0, 0.0]),
    ("Porcelain", [0.22, 0.0, 1.0, 0.30, 0.0, 0.0, 0.0]),
    ("Brushed metal", [0.42, 1.0, 1.0, 0.10, 0.0, 0.0, 0.0]),
    ("Chrome", [0.08, 1.0, 1.0, 0.0, 0.0, 0.0, 0.8]),
    ("Pearl", [0.30, 0.0, 1.0, 0.50, 0.6, 0.0, 0.0]),
    ("Oil slick", [0.18, 0.6, 1.0, 0.30, 1.4, 0.0, 0.4]),
    ("Lava", [0.75, 0.0, 0.3, 0.20, 0.0, 0.6, 0.0]),
    ("Wet stone", [0.12, 0.0, 1.6, 0.15, 0.0, 0.0, 0.35]),
];

pub const COLOR_SOURCES: [&str; 7] =
    ["Orbit: distance from centre", "Orbit: distance from x plane", "Orbit: distance from z axis", "Orbit: distance from y plane", "Surface direction", "Distance from world centre", "Height"];

pub const CIRCLE_PLANES: [&str; 4] = ["x-y", "x-z", "y-z", "x-w"];

// ------------------------------------------------------------------- scene

#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    /// Arbitrary precision: at deep zoom the position needs far more digits than f64 has.
    pub pos: BigV,
    pub rot: Quat,
}
impl Camera {
    pub fn look(from: [f64; 3], to: [f64; 3]) -> Camera {
        let (f, t) = (V3::from_arr(from), V3::from_arr(to));
        Camera { pos: BigV::from_f64(from, 3), rot: Quat::look(f, t) }
    }
    pub fn pos_f64(&self) -> V3 {
        V3::from_arr(self.pos.to_f64())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub name: String,
    pub ops: Vec<Op>,
    pub fr: Fractal,
    pub q: Quality,
    pub col: Color,
    pub mat: Material,
    pub light: Light,
    pub post: Post,
    pub cam: Camera,
    /// Flight speed in world units per second.
    pub speed: f64,
}

impl Scene {
    /// The constant c actually used: the stored value plus its position on the circle.
    pub fn julia_now(&self) -> [f32; 4] {
        let mut j = self.fr.julia;
        let (a, b) = [(0, 1), (0, 2), (1, 2), (0, 3)][self.fr.circle_plane.clamp(0, 3) as usize];
        let ph = self.fr.circle_phase.to_radians();
        j[a] += self.fr.circle_radius * ph.cos();
        j[b] += self.fr.circle_radius * ph.sin();
        j
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# Fractal Worlds scene\n");
        s.push_str(&format!("name = {}\n", self.name));
        self.fr.save("fractal", &mut s);
        self.q.save("quality", &mut s);
        self.col.save("color", &mut s);
        self.mat.save("material", &mut s);
        self.light.save("light", &mut s);
        self.post.save("post", &mut s);
        s.push_str(&format!("camera_rot = {}\n", join(&self.cam.rot.arr())));
        for (axis, v) in ["x", "y", "z"].iter().zip(self.cam.pos.0.iter()) {
            s.push_str(&format!("camera_{} = {}\n", axis, v.to_text()));
        }
        s.push_str(&format!("speed = {}\n", self.speed));
        for o in &self.ops {
            s.push_str(&format!(
                "op = {};{};{};{};{};{}\n",
                o.kind.key(),
                o.on as i32,
                o.start,
                o.every,
                join(&o.p),
                join(&o.drift)
            ));
        }
        s
    }

    pub fn from_text(text: &str) -> Result<Scene, String> {
        let mut sc = presets().remove(0);
        sc.ops.clear();
        let mut pos: [Option<Big>; 3] = [None, None, None];
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.strip_prefix(' ').unwrap_or(v));
            match k {
                "name" => sc.name = v.trim().to_string(),
                "camera" => {
                    // Format written by the first version: position as plain numbers.
                    let d = doubles(v);
                    if d.len() == 7 {
                        sc.cam = Camera { pos: BigV::from_f64([d[0], d[1], d[2]], 3), rot: Quat::from_arr([d[3], d[4], d[5], d[6]]) };
                    }
                }
                "camera_rot" => {
                    let d = doubles(v);
                    if d.len() == 4 {
                        sc.cam.rot = Quat::from_arr([d[0], d[1], d[2], d[3]]);
                    }
                }
                "camera_x" => pos[0] = Big::from_text(v),
                "camera_y" => pos[1] = Big::from_text(v),
                "camera_z" => pos[2] = Big::from_text(v),
                "speed" => {
                    if let Ok(x) = v.trim().parse::<f64>() {
                        if x.is_finite() && x > 0.0 {
                            sc.speed = x;
                        }
                    }
                }
                "op" => {
                    let parts: Vec<&str> = v.split(';').collect();
                    if parts.len() < 6 {
                        return Err(format!("Bad operation line: {}", line));
                    }
                    let kind = OpKind::from_key(parts[0].trim()).ok_or_else(|| format!("Unknown operation '{}'", parts[0]))?;
                    let mut o = Op::new(kind);
                    o.on = parts[1].trim() == "1";
                    o.start = parts[2].trim().parse().unwrap_or(0);
                    o.every = parts[3].trim().parse().unwrap_or(1);
                    if let Some(p) = <[f32; 4]>::dec(parts[4]) {
                        o.p = p;
                    }
                    if let Some(w) = <[f32; 4]>::dec(parts[5]) {
                        o.drift = w;
                    }
                    if sc.ops.len() < MAX_OPS {
                        sc.ops.push(o);
                    }
                }
                _ => {
                    if let Some((group, field)) = k.split_once('.') {
                        match group {
                            "fractal" => sc.fr.load(field, v),
                            "quality" => sc.q.load(field, v),
                            "color" => sc.col.load(field, v),
                            "material" => sc.mat.load(field, v),
                            "light" => sc.light.load(field, v),
                            "post" => sc.post.load(field, v),
                            _ => {}
                        }
                    }
                }
            }
        }
        if sc.ops.is_empty() {
            return Err("The file has no formula operations.".to_string());
        }
        if let [Some(x), Some(y), Some(z)] = pos {
            if x.frac() == y.frac() && y.frac() == z.frac() {
                sc.cam.pos = BigV([x, y, z]);
            }
        }
        Ok(sc)
    }
}

// ----------------------------------------------------------------- presets

fn base(name: &str, ops: Vec<Op>, cam: Camera, speed: f64) -> Scene {
    Scene {
        name: name.to_string(),
        ops,
        fr: Fractal::default(),
        q: Quality::default(),
        col: Color::default(),
        mat: Material::default(),
        light: Light::default(),
        post: Post::default(),
        cam,
        speed,
    }
}

/// Lighting for worlds you fly inside: a lamp on the camera and dark, foggy distance.
fn interior(s: &mut Scene, fog: f32) {
    s.light.headlamp = true;
    s.light.azimuth = 25.0;
    s.light.elevation = 30.0;
    s.light.sun_int = 2.6;
    s.light.ambient = 0.35;
    s.light.fog = fog;
    s.light.fog_col = [0.05, 0.06, 0.09];
    s.light.sky_top = [0.03, 0.04, 0.07];
    s.light.sky_hor = [0.10, 0.11, 0.14];
    s.q.shadows = false;
}

fn drift(mut op: Op, rates: &[f32]) -> Op {
    for (i, r) in rates.iter().enumerate().take(4) {
        op.drift[i] = *r;
    }
    op
}

pub fn presets() -> Vec<Scene> {
    use OpKind::*;
    let mut v = Vec::new();
    let o = [0.0, 0.0, 0.0];

    let mut s = base("Mandelbulb", vec![drift(Op::with(BulbPow, &[8.0, 0.0, 0.0]), &[0.0, 1.5, 0.0]), Op::with(AddC, &[1.0])], Camera::look([1.5, 0.9, -2.2], o), 0.5);
    s.fr.iters = 9;
    s.fr.de_mode = 1;
    s.q.step_scale = 0.8;
    s.col.freq = 1.2;
    s.col.offset = 0.35;
    v.push(s);

    let mut s = base(
        "Mandelbox",
        vec![Op::with(BoxFold, &[1.0, 1.0, 1.0]), Op::with(SphereFold, &[0.5, 1.0]), Op::with(Scale, &[2.4, 0.0, 0.0, 0.0]), Op::with(AddC, &[1.0])],
        Camera::look([7.5, 5.0, -12.5], [0.0, -0.3, 0.0]),
        3.0,
    );
    s.fr.iters = 16;
    s.fr.bailout = 100.0;
    s.col.stops = PALETTES[1].1;
    s.col.freq = 0.6;
    s.mat.rough = 0.3;
    s.light.fog = 0.004;
    v.push(s);

    let mut s = base(
        "Menger sponge",
        vec![Op::new(AbsFold), Op::new(SortFold), drift(Op::new(Rotate), &[0.5, 0.0, 0.0]), Op::with(MengerScale, &[3.0, 1.0, 1.0, 1.0])],
        Camera::look([2.1, 1.5, -2.7], o),
        0.6,
    );
    s.fr.iters = 10;
    s.fr.bailout = 100.0;
    s.fr.de_mode = 2;
    s.fr.de_param = 1.0;
    s.col.stops = PALETTES[3].1;
    s.col.src = 1;
    s.col.freq = 0.8;
    s.mat.rough = 0.7;
    v.push(s);

    let mut s = base(
        "Sierpinski",
        vec![Op::new(TetraFold), drift(Op::new(Rotate), &[0.0, 0.0, 0.7]), Op::with(Scale, &[2.0, 1.0, 1.0, 1.0])],
        Camera::look([2.4, 1.2, -3.2], o),
        0.7,
    );
    s.fr.iters = 16;
    s.fr.bailout = 1000.0;
    s.fr.de_param = 2.0;
    s.col.stops = PALETTES[6].1;
    s.col.freq = 0.5;
    s.mat.metal = 0.8;
    s.mat.rough = 0.35;
    v.push(s);

    let mut s = base(
        "Quaternion Julia",
        vec![drift(Op::new(Rotate4D), &[1.5, 0.0, 0.0]).once(), Op::new(QuatSquare), Op::with(AddC, &[1.0])],
        Camera::look([1.7, 1.0, -2.3], o),
        0.5,
    );
    s.fr.iters = 11;
    s.fr.de_mode = 1;
    s.fr.julia = [-0.125, -0.256, 0.847, 0.09];
    s.fr.julia_mix = 1.0;
    s.q.step_scale = 0.7;
    s.col.stops = PALETTES[5].1;
    s.mat.rough = 0.25;
    s.mat.irid = 0.5;
    v.push(s);

    let mut s = base("Julia bulb", vec![Op::with(BulbPow, &[6.0, 0.0, 0.0]), Op::with(AddC, &[1.0])], Camera::look([1.6, 0.8, -2.0], o), 0.5);
    s.fr.iters = 9;
    s.fr.de_mode = 1;
    s.fr.julia = [0.35, -0.55, 0.25, 0.0];
    s.fr.julia_mix = 1.0;
    s.fr.circle_radius = 0.12;
    s.fr.circle_drift = 2.0;
    s.q.step_scale = 0.75;
    s.col.stops = PALETTES[2].1;
    v.push(s);

    let mut s = base(
        "Kaleidoscope IFS",
        vec![Op::new(AbsFold), Op::new(SortFold), drift(Op::with(Rotate, &[0.0, 9.0, 0.0]), &[0.0, 0.6, 0.0]), Op::with(Scale, &[2.0, 1.0, 0.0, 0.0])],
        Camera::look([1.5, 1.0, -1.9], o),
        0.5,
    );
    s.fr.iters = 18;
    s.fr.bailout = 1000.0;
    s.fr.de_param = 1.0;
    s.col.stops = PALETTES[4].1;
    s.col.freq = 0.35;
    s.col.src = 2;
    v.push(s);

    let mut s = base(
        "Rotated box",
        vec![
            Op::with(BoxFold, &[1.0, 1.0, 1.0]),
            drift(Op::with(Rotate, &[8.0, 0.0, 15.0]), &[0.4, 0.0, 0.0]),
            Op::with(SphereFold, &[0.45, 1.0]),
            Op::with(Scale, &[-1.8, 0.0, 0.0, 0.0]),
            Op::with(AddC, &[1.0]),
        ],
        Camera::look([3.2, 2.2, -5.0], o),
        1.2,
    );
    s.fr.iters = 18;
    s.fr.bailout = 100.0;
    s.col.stops = PALETTES[2].1;
    s.col.freq = 0.9;
    s.mat.rough = 0.3;
    s.mat.metal = 0.5;
    v.push(s);

    // ---- Endless worlds: a Cell repeat tiles all of space, so you can fly forever.

    let mut s = base(
        "Apollonian caves",
        vec![Op::with(CellFold, &[1.0, 1.0, 1.0]), Op::new(Rotate), Op::with(Inversion, &[1.2, 0.0])],
        Camera::look([0.95, 0.5, 2.0], [0.0, 0.3, 0.0]),
        0.25,
    );
    s.fr.iters = 12;
    s.fr.bailout = 1.0e9;
    s.fr.de_mode = 3;
    s.fr.de_param = 0.25;
    s.q.step_scale = 0.6;
    s.col.stops = PALETTES[7].1;
    s.col.freq = 1.6;
    interior(&mut s, 0.35);
    v.push(s);

    let mut s = base(
        "Kleinian city",
        vec![Op::with(BoxFold, &[0.92436, 0.92436, 0.90756]), Op::with(Inversion, &[1.0, 1.0]), Op::with(AddC, &[1.0])],
        Camera::look([0.35, 0.25, -1.6], [0.0, 0.1, 0.0]),
        0.2,
    );
    s.fr.iters = 14;
    s.fr.bailout = 1.0e9;
    s.fr.de_mode = 3;
    s.fr.de_param = 0.5;
    s.fr.julia_mix = 1.0;
    s.q.step_scale = 0.7;
    s.col.stops = PALETTES[6].1;
    s.col.freq = 1.2;
    interior(&mut s, 0.35);
    v.push(s);

    let mut s = base(
        "Sponge lattice",
        vec![Op::with(CellFold, &[1.0, 1.0, 1.0]).once(), Op::new(AbsFold), Op::new(SortFold), Op::new(Rotate), Op::with(MengerScale, &[3.0, 1.0, 1.0, 1.0])],
        Camera::look([0.0, 0.0, 0.0], [1.0, 0.45, 0.25]),
        0.3,
    );
    s.fr.iters = 9;
    s.fr.bailout = 100.0;
    s.fr.de_mode = 2;
    s.fr.de_param = 1.0;
    s.col.stops = PALETTES[3].1;
    s.col.src = 1;
    interior(&mut s, 0.25);
    v.push(s);

    let mut s = base(
        "Julia orchard",
        vec![Op::with(CellFold, &[1.5, 1.5, 1.5]).once(), Op::with(BulbPow, &[6.0, 0.0, 0.0]), Op::with(AddC, &[1.0])],
        Camera::look([1.5, 0.6, -1.5], o),
        0.5,
    );
    s.fr.iters = 9;
    s.fr.de_mode = 1;
    s.fr.julia = [0.35, -0.55, 0.25, 0.0];
    s.fr.julia_mix = 1.0;
    s.q.step_scale = 0.75;
    s.col.stops = PALETTES[2].1;
    s.light.fog = 0.08;
    v.push(s);

    // Three more worlds from the same two operations: only the inversion strength and cell shape change.
    for (name, hs, strength, pal) in [
        ("Stalagmite plains", [1.0f32, 1.0, 1.0], 1.05f32, 1usize),
        ("Foam grotto", [1.0, 1.0, 1.0], 1.5, 7),
        ("Open sea", [1.0, 1.6, 1.0], 1.15, 5),
    ] {
        let mut s = base(
            name,
            vec![drift(Op::new(Translate), &[0.0, 0.0, 0.0]), Op::with(CellFold, &hs), Op::new(Rotate), Op::with(Inversion, &[strength, 0.0])],
            Camera::look([0.95, 0.5, 2.0], [0.0, 0.3, 0.0]),
            0.25,
        );
        s.fr.iters = 14;
        s.fr.bailout = 1.0e9;
        s.fr.de_mode = 3;
        s.fr.de_param = 0.25;
        s.q.step_scale = 0.6;
        s.col.stops = PALETTES[pal].1;
        s.col.freq = 1.3;
        interior(&mut s, 0.35);
        v.push(s);
    }

    let mut s = base(
        "Twisted caverns",
        vec![drift(Op::new(Translate), &[1.0, 0.0, 0.0]), Op::with(CellFold, &[1.0, 1.3, 1.0]), Op::with(Rotate, &[0.0, 14.0, 0.0]), Op::with(Inversion, &[1.3, 0.0])],
        Camera::look([0.95, 0.5, 2.0], [0.0, 0.3, 0.0]),
        0.25,
    );
    s.fr.iters = 12;
    s.fr.bailout = 1.0e9;
    s.fr.de_mode = 3;
    s.fr.de_param = 0.25;
    s.q.step_scale = 0.6;
    s.col.stops = PALETTES[0].1;
    s.col.freq = 1.4;
    interior(&mut s, 0.35);
    v.push(s);

    let mut s = base(
        "Octahedral lattice",
        vec![Op::with(CellFold, &[1.0, 1.0, 1.0]).once(), Op::new(AbsFold), Op::new(SortFold), Op::with(Rotate, &[0.0, 9.0, 0.0]), Op::with(Scale, &[2.0, 1.0, 0.0, 0.0])],
        Camera::look([0.55, 0.5, 0.6], [-1.0, -0.4, -1.0]),
        0.3,
    );
    s.fr.iters = 16;
    s.fr.bailout = 1000.0;
    s.fr.de_param = 1.0;
    s.col.stops = PALETTES[4].1;
    s.col.freq = 0.35;
    s.col.src = 2;
    s.light.fog = 0.1;
    v.push(s);

    let mut s = base(
        "Quaternion reef",
        vec![Op::with(CellFold, &[1.6, 1.6, 1.6]).once(), drift(Op::new(Rotate4D), &[1.5, 0.0, 0.0]).once(), Op::new(QuatSquare), Op::with(AddC, &[1.0])],
        Camera::look([1.6, 0.9, -1.6], o),
        0.4,
    );
    s.fr.iters = 11;
    s.fr.de_mode = 1;
    s.fr.julia = [-0.125, -0.256, 0.847, 0.09];
    s.fr.julia_mix = 1.0;
    s.q.step_scale = 0.7;
    s.col.stops = PALETTES[5].1;
    s.light.fog = 0.08;
    v.push(s);

    let mut s = base(
        "Kleinian cells",
        vec![Op::with(CellFold, &[0.92436, 0.92436, 0.90756]), Op::with(Inversion, &[1.0, 1.0]), Op::with(AddC, &[1.0])],
        Camera::look([0.35, 0.25, -1.6], [0.0, 0.1, 0.0]),
        0.2,
    );
    s.fr.iters = 14;
    s.fr.bailout = 1.0e9;
    s.fr.de_mode = 3;
    s.fr.de_param = 0.5;
    s.fr.julia_mix = 1.0;
    s.q.step_scale = 0.7;
    s.col.stops = PALETTES[6].1;
    interior(&mut s, 0.35);
    v.push(s);

    v
}

// ---------------------------------------------------------------- uniforms

#[derive(Clone, Debug, PartialEq)]
pub enum UVal {
    F(f32),
    F3([f32; 3]),
    F4([f32; 4]),
    I(i32),
    M3([f32; 9]),
}

fn lin(c: [f32; 3]) -> [f32; 3] {
    [c[0].max(0.0).powf(2.2), c[1].max(0.0).powf(2.2), c[2].max(0.0).powf(2.2)]
}

/// Every content uniform of the frame shader. Resolution, jitter, mode and the
/// probe direction are added by the renderer. `s` is the zoom scale; `levels`
/// and `total` come from the reference orbit.
pub fn uniforms(sc: &Scene, time: f64, s: f64, levels: i32, total: i32) -> Vec<(String, UVal)> {
    let mut u: Vec<(String, UVal)> = Vec::with_capacity(72);
    let mut put = |n: &str, v: UVal| u.push((n.to_string(), v));
    let cam = &sc.cam;
    let pos = cam.pos_f64();

    put("uCamF", UVal::F3(pos.f32()));
    put("uS", UVal::F(s as f32));
    put("uLevels", UVal::I(levels));
    put("uTotal", UVal::I(total));
    put("uTau", UVal::F(TAU));
    put("uMinEps", UVal::F(1.0e-6));
    put("uFar", UVal::F((sc.q.far as f64 / s).min(1.0e30) as f32));
    put("uFogK", UVal::F((sc.light.fog as f64 * s) as f32));
    put("uCamRot", UVal::M3(cam.rot.mat3()));
    put("uFovT", UVal::F((sc.q.fov.to_radians() * 0.5).tan()));
    put("uMaxSteps", UVal::I(sc.q.max_steps));
    put("uBail", UVal::F(sc.fr.bailout));
    put("uDetail", UVal::F(sc.q.detail));
    put("uStepScale", UVal::F(sc.q.step_scale));
    put("uDEp", UVal::F(sc.fr.de_param));
    for (i, op) in sc.ops.iter().enumerate().take(MAX_OPS) {
        put(&format!("uOp[{}]", i), UVal::F4(op.p));
        match op.kind {
            OpKind::Rotate => put(&format!("uOpM[{}]", i), UVal::M3(euler_mat3([op.p[0], op.p[1], op.p[2]]))),
            OpKind::PlaneFold => {
                let n = plane_normal(&op.p);
                put(&format!("uOpM[{}]", i), UVal::M3([n[0], n[1], n[2], 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]));
            }
            _ => {}
        }
    }
    put("uJulia", UVal::F4(sc.julia_now()));
    put("uJuliaMix", UVal::F(sc.fr.julia_mix));

    for i in 0..5 {
        put(&format!("uStops[{}]", i), UVal::F3(lin(sc.col.stops[i])));
    }
    put("uSrc", UVal::I(sc.col.src));
    put("uSrc2", UVal::I(sc.col.src2));
    put("uSrcMix", UVal::F(sc.col.src_mix));
    put("uFreq", UVal::F(sc.col.freq));
    put("uOff", UVal::F(sc.col.offset + (sc.col.cycle as f64 * time).rem_euclid(1.0) as f32));

    put("uRough", UVal::F(sc.mat.rough));
    put("uMetal", UVal::F(sc.mat.metal));
    put("uSpec", UVal::F(sc.mat.spec));
    put("uRim", UVal::F(sc.mat.rim));
    put("uIrid", UVal::F(sc.mat.irid));
    put("uEmit", UVal::F(sc.mat.emit));
    put("uReflect", UVal::F(sc.mat.reflect));
    put("uAO", UVal::F(sc.q.ao));
    put("uGlow", UVal::F(sc.mat.glow));
    put("uGlowCol", UVal::F3(lin(sc.mat.glow_col)));

    let (az, el) = ((sc.light.azimuth as f64).to_radians(), (sc.light.elevation as f64).to_radians());
    let sun = if sc.light.headlamp {
        // Relative to the view: a lamp carried just behind the camera.
        cam.rot.rotate(V3(el.cos() * az.sin(), el.sin(), -el.cos() * az.cos()))
    } else {
        V3(el.cos() * az.sin(), el.sin(), el.cos() * az.cos())
    };
    let sc_lin = lin(sc.light.sun_col);
    put("uSunDir", UVal::F3(sun.norm().f32()));
    put("uSunCol", UVal::F3([sc_lin[0] * sc.light.sun_int, sc_lin[1] * sc.light.sun_int, sc_lin[2] * sc.light.sun_int]));
    put("uSunDisk", UVal::F(if sc.light.headlamp { 0.0 } else { 1.0 }));
    put("uSkyTop", UVal::F3(lin(sc.light.sky_top)));
    put("uSkyHor", UVal::F3(lin(sc.light.sky_hor)));
    put("uAmb", UVal::F(sc.light.ambient));
    put("uFogCol", UVal::F3(lin(sc.light.fog_col)));
    put("uShadow", UVal::F(if sc.q.shadows { sc.q.shadow_soft } else { 0.0 }));
    u
}

pub fn post_uniforms(p: &Post) -> Vec<(String, UVal)> {
    vec![
        ("uExposure".to_string(), UVal::F(p.exposure)),
        ("uContrast".to_string(), UVal::F(p.contrast)),
        ("uSaturation".to_string(), UVal::F(p.saturation)),
        ("uVignette".to_string(), UVal::F(p.vignette)),
        ("uGamma".to_string(), UVal::F(p.gamma)),
        ("uTonemap".to_string(), UVal::I(p.tonemap)),
    ]
}
