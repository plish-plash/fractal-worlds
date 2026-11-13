//! The formula stack. A fractal is a list of operations applied in order on
//! every iteration to the point `z`, while `dr` tracks the running derivative
//! used for the distance estimate. The stack is turned into GLSL here; every
//! numeric parameter stays a uniform, so sliders and morphs never recompile.
//!
//! Each operation has two GLSL forms. The absolute form works on `z` itself.
//! The offset form works on `e`, the point's offset from the camera's own
//! orbit measured in units of the current zoom scale; it is what lets a
//! 32-bit GPU zoom without a precision limit (see reference.rs).

pub const MAX_OPS: usize = 12;
/// Texels per operation in the reference texture, after two per-level header texels.
pub const REF_TEXELS: usize = 4;
pub const REF_WIDTH: usize = 2 + REF_TEXELS * MAX_OPS;

pub struct ParamInfo {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub def: f32,
    /// The value is periodic over min..max, so it can be driven forward forever.
    pub wraps: bool,
}
const fn p(name: &'static str, min: f32, max: f32, def: f32) -> ParamInfo {
    ParamInfo { name, min, max, def, wraps: false }
}
const fn w(name: &'static str, min: f32, max: f32, def: f32) -> ParamInfo {
    ParamInfo { name, min, max, def, wraps: true }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpKind {
    BoxFold,
    SphereFold,
    AbsFold,
    SortFold,
    TetraFold,
    PlaneFold,
    Rotate,
    Rotate4D,
    Scale,
    MengerScale,
    Translate,
    AddC,
    BulbPow,
    QuatSquare,
    Inversion,
    CellFold,
}

impl OpKind {
    pub const ALL: [OpKind; 16] = [
        OpKind::BoxFold,
        OpKind::SphereFold,
        OpKind::AbsFold,
        OpKind::SortFold,
        OpKind::TetraFold,
        OpKind::PlaneFold,
        OpKind::Rotate,
        OpKind::Rotate4D,
        OpKind::Scale,
        OpKind::MengerScale,
        OpKind::Translate,
        OpKind::AddC,
        OpKind::BulbPow,
        OpKind::QuatSquare,
        OpKind::Inversion,
        OpKind::CellFold,
    ];

    pub fn name(self) -> &'static str {
        match self {
            OpKind::BoxFold => "Box fold",
            OpKind::SphereFold => "Sphere fold",
            OpKind::AbsFold => "Mirror fold",
            OpKind::SortFold => "Sort fold",
            OpKind::TetraFold => "Tetrahedral fold",
            OpKind::PlaneFold => "Plane fold",
            OpKind::Rotate => "Rotate",
            OpKind::Rotate4D => "Rotate through 4D",
            OpKind::Scale => "Scale about point",
            OpKind::MengerScale => "Menger scale",
            OpKind::Translate => "Translate",
            OpKind::AddC => "Add c",
            OpKind::BulbPow => "Bulb power",
            OpKind::QuatSquare => "Quaternion square",
            OpKind::Inversion => "Sphere inversion",
            OpKind::CellFold => "Cell repeat",
        }
    }

    /// Stable identifier used in scene files.
    pub fn key(self) -> &'static str {
        match self {
            OpKind::BoxFold => "boxfold",
            OpKind::SphereFold => "spherefold",
            OpKind::AbsFold => "absfold",
            OpKind::SortFold => "sortfold",
            OpKind::TetraFold => "tetrafold",
            OpKind::PlaneFold => "planefold2",
            OpKind::Rotate => "rotate",
            OpKind::Rotate4D => "rotate4d",
            OpKind::Scale => "scale",
            OpKind::MengerScale => "mengerscale",
            OpKind::Translate => "translate",
            OpKind::AddC => "addc",
            OpKind::BulbPow => "bulbpow",
            OpKind::QuatSquare => "quatsquare",
            OpKind::Inversion => "inversion",
            OpKind::CellFold => "cellfold",
        }
    }
    pub fn from_key(k: &str) -> Option<OpKind> {
        OpKind::ALL.iter().copied().find(|o| o.key() == k)
    }

    pub fn equation(self) -> &'static str {
        match self {
            OpKind::BoxFold => "z = 2 clamp(z, -L, L) - z",
            OpKind::SphereFold => "z = k z, with k = R^2/r^2 inside radius R and R^2/m^2 inside m",
            OpKind::AbsFold => "z = |z + o| - o",
            OpKind::SortFold => "sort the axes so that x >= y >= z",
            OpKind::TetraFold => "reflect across the planes x+y, x+z, y+z = 0",
            OpKind::PlaneFold => "reflect whatever lies behind the plane n.z = d; n is set by two angles",
            OpKind::Rotate => "z = Rot(x, y, z) z",
            OpKind::Rotate4D => "turn z in the x-w, y-w and z-w planes; set 'then every' to 999 to turn only the starting slice",
            OpKind::Scale => "z = s z - o (s - 1)",
            OpKind::MengerScale => "z = s z - o (s - 1), with the z axis mirrored back",
            OpKind::Translate => "z = z + o",
            OpKind::AddC => "z = z + w c",
            OpKind::BulbPow => "z = r^n (sin nt cos np, sin nt sin np, cos nt), t and p the polar angles of z",
            OpKind::QuatSquare => "z = z^2 as a quaternion",
            OpKind::Inversion => "z = z max(s / r^2, k)",
            OpKind::CellFold => "tile all of space with mirrored copies of a box of half-size h",
        }
    }

    pub fn params(self) -> &'static [ParamInfo] {
        const BOXFOLD: [ParamInfo; 3] = [p("Limit x", 0.2, 2.0, 1.0), p("Limit y", 0.2, 2.0, 1.0), p("Limit z", 0.2, 2.0, 1.0)];
        const SPHERE: [ParamInfo; 2] = [p("Inner radius m", 0.02, 1.5, 0.5), p("Outer radius R", 0.2, 2.5, 1.0)];
        const ABS: [ParamInfo; 3] = [p("Offset x", -1.5, 1.5, 0.0), p("Offset y", -1.5, 1.5, 0.0), p("Offset z", -1.5, 1.5, 0.0)];
        const PLANE: [ParamInfo; 3] = [w("Azimuth (deg)", -180.0, 180.0, 0.0), w("Elevation (deg)", -180.0, 180.0, 0.0), p("Distance d", -1.5, 1.5, 0.0)];
        const ROT: [ParamInfo; 3] = [w("Angle x (deg)", -180.0, 180.0, 0.0), w("Angle y (deg)", -180.0, 180.0, 0.0), w("Angle z (deg)", -180.0, 180.0, 0.0)];
        const ROT4: [ParamInfo; 3] = [w("x-w angle (deg)", -180.0, 180.0, 0.0), w("y-w angle (deg)", -180.0, 180.0, 0.0), w("z-w angle (deg)", -180.0, 180.0, 0.0)];
        const SCALE: [ParamInfo; 4] =
            [p("Scale s", -4.0, 4.0, 2.0), p("Centre x", -2.0, 2.0, 0.0), p("Centre y", -2.0, 2.0, 0.0), p("Centre z", -2.0, 2.0, 0.0)];
        const MENGER: [ParamInfo; 4] =
            [p("Scale s", 1.5, 4.0, 3.0), p("Offset x", 0.0, 2.0, 1.0), p("Offset y", 0.0, 2.0, 1.0), p("Offset z", 0.0, 2.0, 1.0)];
        const TRANS: [ParamInfo; 3] = [p("Offset x", -4.0, 4.0, 0.0), p("Offset y", -4.0, 4.0, 0.0), p("Offset z", -4.0, 4.0, 0.0)];
        const ADDC: [ParamInfo; 1] = [p("Weight w", -2.0, 2.0, 1.0)];
        const BULB: [ParamInfo; 3] = [p("Power n", 1.0, 16.0, 8.0), w("Polar phase", -3.14159265, 3.14159265, 0.0), w("Azimuth phase", -3.14159265, 3.14159265, 0.0)];
        const INV: [ParamInfo; 2] = [p("Strength s", 0.3, 2.5, 1.2), p("Minimum k", 0.0, 1.5, 0.0)];
        const CELL: [ParamInfo; 3] = [p("Half-size x", 0.3, 8.0, 1.0), p("Half-size y", 0.3, 8.0, 1.0), p("Half-size z", 0.3, 8.0, 1.0)];
        match self {
            OpKind::BoxFold => &BOXFOLD,
            OpKind::SphereFold => &SPHERE,
            OpKind::AbsFold => &ABS,
            OpKind::SortFold | OpKind::TetraFold | OpKind::QuatSquare => &[],
            OpKind::PlaneFold => &PLANE,
            OpKind::Rotate => &ROT,
            OpKind::Rotate4D => &ROT4,
            OpKind::Scale => &SCALE,
            OpKind::MengerScale => &MENGER,
            OpKind::Translate => &TRANS,
            OpKind::AddC => &ADDC,
            OpKind::BulbPow => &BULB,
            OpKind::Inversion => &INV,
            OpKind::CellFold => &CELL,
        }
    }

    /// Absolute GLSL body, run once a point has left the reference orbit.
    /// In scope: `V4 z`, `R dr`, `V4 c`, `vec4 P`, `mat3 M`, `int i`.
    /// R / V3 / V4 / M3 are aliases for the float types.
    fn glsl(self) -> &'static str {
        match self {
            OpKind::BoxFold => "z.xyz = clamp(z.xyz, -V3(P.xyz), V3(P.xyz)) * 2.0 - z.xyz;",
            OpKind::SphereFold => {
                "R r2 = dot(z.xyz, z.xyz); R mr = R(max(P.x * P.x, 1e-9)); R fr = R(max(P.y * P.y, 1e-9));\n\
                 R k = r2 < mr ? fr / mr : (r2 < fr ? fr / r2 : R(1.0)); z.xyz *= k; dr *= k;"
            }
            OpKind::AbsFold => "z.xyz = abs(z.xyz + V3(P.xyz)) - V3(P.xyz);",
            OpKind::SortFold => "if (z.x < z.y) z.xy = z.yx; if (z.x < z.z) z.xz = z.zx; if (z.y < z.z) z.yz = z.zy;",
            OpKind::TetraFold => {
                "if (z.x + z.y < 0.0) z.xy = -z.yx; if (z.x + z.z < 0.0) z.xz = -z.zx; if (z.y + z.z < 0.0) z.zy = -z.yz;"
            }
            OpKind::PlaneFold => "V3 nf = V3(M[0]); R dd = min(dot(z.xyz, nf) - R(P.z), R(0.0)); z.xyz -= 2.0 * dd * nf;",
            OpKind::Rotate => "z.xyz = M3(M) * z.xyz;",
            OpKind::Rotate4D => "rot4(z, radians(P.xyz));",
            OpKind::Scale => "z.xyz = z.xyz * R(P.x) - V3(P.yzw) * R(P.x - 1.0); dr *= abs(R(P.x));",
            OpKind::MengerScale => {
                "R s = R(P.x); V3 o = V3(P.yzw) * R(P.x - 1.0); z.xy = z.xy * s - o.xy;\n\
                 R hh = 0.5 * o.z; z.z = hh - abs(z.z * s - hh); dr *= abs(s);"
            }
            OpKind::Translate => "z.xyz += V3(P.xyz);",
            OpKind::AddC => "z += c * R(P.x); dr += R(abs(P.x)) * DRU;",
            OpKind::BulbPow => {
                "float rf = max(float(length(z.xyz)), 1e-9);\n\
                 float th = acos(clamp(float(z.z) / rf, -1.0, 1.0)) * P.x + P.y;\n\
                 float ph = atan(float(z.y), float(z.x)) * P.x + P.z;\n\
                 dr = R(pow(rf, P.x - 1.0) * P.x) * dr;\n\
                 z.xyz = R(pow(rf, P.x)) * V3(vec3(sin(th) * cos(ph), sin(th) * sin(ph), cos(th)));"
            }
            OpKind::QuatSquare => "R rq = length(z); dr = 2.0 * rq * dr; z = V4(z.x * z.x - dot(z.yzw, z.yzw), 2.0 * z.x * z.yzw);",
            OpKind::Inversion => {
                "R r2 = max(dot(z.xyz, z.xyz), R(1e-24)); R k = max(R(P.x) / r2, R(P.y)); z.xyz *= k; dr *= k;"
            }
            OpKind::CellFold => "V3 h = V3(max(P.xyz, vec3(1e-3))); z.xyz = h - abs(mod(z.xyz + h, 4.0 * h) - 2.0 * h);",
        }
    }

    /// Offset GLSL body. In scope: `vec4 e` (offset in zoom units), `float d`
    /// (derivative in zoom units), `vec3 e0`, `vec4 P`, `mat3 M`, and the
    /// reference texels `T0..T2` plus `uf` (unit as float), `g` (the
    /// reference's own scale factor for this operation) and `A` (S / unit).
    fn glsl_offset(self) -> &'static str {
        match self {
            OpKind::BoxFold => "e.xyz = 2.0 * (clamp(e.xyz, T0.xyz, T1.xyz) - clamp(vec3(0.0), T0.xyz, T1.xyz)) - e.xyz;",
            OpKind::SphereFold | OpKind::Inversion => "pRadial(e, d, T0.xyz, T0.w, T1.x, T1.y, uf);",
            OpKind::AbsFold => "e.xyz = pAbs(T0.xyz, e.xyz);",
            OpKind::SortFold => "pPlane(e, T0.x, vec3(1.0, -1.0, 0.0)); pPlane(e, T0.y, vec3(1.0, 0.0, -1.0)); pPlane(e, T0.z, vec3(0.0, 1.0, -1.0));",
            OpKind::TetraFold => "pPlane(e, T0.x, vec3(1.0, 1.0, 0.0)); pPlane(e, T0.y, vec3(1.0, 0.0, 1.0)); pPlane(e, T0.z, vec3(0.0, 1.0, 1.0));",
            OpKind::PlaneFold => "pPlane(e, T0.x, M[0]);",
            OpKind::Rotate => "e.xyz = M * e.xyz;",
            OpKind::Rotate4D => "rot4(e, radians(P.xyz));",
            OpKind::Scale => "e.xyz *= sign(P.x);",
            OpKind::MengerScale => "e.z = -pAbs(vec3(T0.x), vec3(e.z)).x;",
            OpKind::Translate => "",
            OpKind::AddC => "e.xyz += e0 * (P.x * (1.0 - uJuliaMix) * A); d += abs(P.x) * A; e /= g; d /= g;",
            OpKind::BulbPow => "pBulb(e, d, T0, T1, T2.x, uf, g, P.x);",
            OpKind::QuatSquare => "pQuat(e, d, T0, uf, g);",
            OpKind::CellFold => "e.xyz = T2.xyz * (2.0 * clamp(e.xyz, T0.xyz, T1.xyz) - e.xyz);",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub kind: OpKind,
    pub on: bool,
    /// First iteration this operation runs on.
    pub start: i32,
    /// Run every Nth iteration from `start` (1 = always). Lets two formulas alternate.
    pub every: i32,
    pub p: [f32; 4],
    /// Drive rate per parameter in cycles per minute; only used for values that wrap.
    pub drift: [f32; 4],
}

impl Op {
    pub fn new(kind: OpKind) -> Op {
        let mut pv = [0.0; 4];
        for (i, info) in kind.params().iter().enumerate() {
            pv[i] = info.def;
        }
        Op { kind, on: true, start: 0, every: 1, p: pv, drift: [0.0; 4] }
    }
    pub fn with(kind: OpKind, vals: &[f32]) -> Op {
        let mut o = Op::new(kind);
        for (i, v) in vals.iter().enumerate().take(4) {
            o.p[i] = *v;
        }
        o
    }
    pub fn once(mut self) -> Op {
        self.every = 999;
        self
    }
    /// Does this operation run on iteration `i`? Mirrors the guard in the generated GLSL.
    pub fn runs_on(&self, i: i32) -> bool {
        self.on && i >= self.start.max(0) && (i - self.start.max(0)) % self.every.max(1) == 0
    }
}

/// The range over which parameter `j` of operation `i` repeats, if it does.
/// A Translate wraps when the next active operation is a Cell repeat: the
/// mirrored tiling has period four half-sizes along each axis.
pub fn wrap_range(ops: &[Op], i: usize, j: usize) -> Option<(f32, f32)> {
    let op = &ops[i];
    let info = op.kind.params().get(j)?;
    if info.wraps {
        return Some((info.min, info.max));
    }
    if op.kind == OpKind::Translate && j < 3 {
        if let Some(next) = ops[i + 1..].iter().find(|o| o.on) {
            if next.kind == OpKind::CellFold {
                let h = next.p[j].max(1e-3);
                return Some((-2.0 * h, 2.0 * h));
            }
        }
    }
    None
}

/// Unit normal of a Plane fold from its two angles.
pub fn plane_normal(p: &[f32; 4]) -> [f32; 3] {
    let (az, el) = (p[0].to_radians(), p[1].to_radians());
    [el.cos() * az.cos(), el.sin(), el.cos() * az.sin()]
}

/// How the final distance is read off the iterated point.
pub const DE_MODES: [(&str, &str); 4] = [
    ("Linear", "(|z| - a) / dr.  For box folds and scaling systems."),
    ("Logarithmic", "|z| ln|z| / (2 dr).  For power sets: bulbs and Julias."),
    ("Box", "Distance to a cube of half-size a, over dr.  For Menger-style sponges."),
    ("Plane", "a |z.y| / dr.  For inversion and Kleinian tilings."),
];

fn guard(op: &Op) -> String {
    if op.start > 0 || op.every > 1 {
        format!("if (i >= {} && (i - {}) % {} == 0) ", op.start.max(0), op.start.max(0), op.every.max(1))
    } else {
        String::new()
    }
}
fn indent(s: &mut String, body: &str, pad: &str) {
    for line in body.lines() {
        s.push_str(pad);
        s.push_str(line);
        s.push('\n');
    }
}

/// Emit the `deMap` function for a formula stack.
///
/// The function takes the point as an offset from the camera in zoom units
/// and follows the camera's reference orbit level by level until the offset
/// has grown to ordinary size, then carries on in absolute coordinates.
pub fn generate(ops: &[Op], de_mode: i32) -> String {
    let mut s = String::new();
    s.push_str("float deMap(vec3 q, out vec4 trap) {\n");
    s.push_str("  vec4 e = vec4(q, 0.0); vec3 e0 = q; float d = 1.0; bool pert = true;\n");
    s.push_str("  vec3 p = uCamF + q * uS;\n");
    s.push_str("  vec4 z = vec4(p, 0.0); float dr = uS;\n");
    s.push_str("  int total = uTotal; float ysc = 0.0;\n");
    s.push_str("  V4 c = V4(mix(V3(p), V3(uJulia.xyz), R(uJuliaMix)), R(uJulia.w * uJuliaMix));\n");
    s.push_str("  float ws = 0.0, t0 = 0.0, t1 = 0.0, t2 = 0.0, t3 = 0.0;\n");
    s.push_str("  for (int i = 0; i < total; i++) {\n");
    s.push_str("    if (pert) {\n");
    s.push_str("      vec4 zs = texelFetch(uRef, ivec2(0, i), 0); float us = texelFetch(uRef, ivec2(1, i), 0).x;\n");
    s.push_str("      float em = max(max(abs(e.x), abs(e.y)), max(abs(e.z), abs(e.w)));\n");
    s.push_str("      if (i >= uLevels || em * us > uTau) { z = zs + e * us; dr = d * us; pert = false; }\n");
    s.push_str("    }\n");
    for (n, op) in ops.iter().enumerate().take(MAX_OPS) {
        if !op.on {
            continue;
        }
        s.push_str(&format!("    // {}: {}\n    {}{{ vec4 P = uOp[{}]; mat3 M = uOpM[{}];\n", n, op.kind.name(), guard(op), n, n));
        let x = 2 + REF_TEXELS * n;
        s.push_str("      if (pert) {\n");
        s.push_str(&format!(
            "        vec4 T0 = texelFetch(uRef, ivec2({}, i), 0), T1 = texelFetch(uRef, ivec2({}, i), 0), T2 = texelFetch(uRef, ivec2({}, i), 0), T3 = texelFetch(uRef, ivec2({}, i), 0);\n",
            x,
            x + 1,
            x + 2,
            x + 3
        ));
        s.push_str("        float uf = T3.x, g = T3.y, A = T3.z;\n");
        indent(&mut s, op.kind.glsl_offset(), "        ");
        s.push_str("      } else {\n");
        indent(&mut s, op.kind.glsl(), "        ");
        s.push_str("      }\n");
        s.push_str("    }\n");
    }
    s.push_str("    if (pert) { float un = texelFetch(uRef, ivec2(1, i + 1), 0).x; z = texelFetch(uRef, ivec2(0, i + 1), 0) + e * un; dr = d * un; ysc = texelFetch(uRef, ivec2(1, i + 1), 0).z; }\n");
    // Shared tail: colour traps weighted by how large a pixel is at this level, so
    // colour depends on the detail you can see and not on how deep the zoom is.
    s.push_str("    R rr = dot(z, z);\n");
    s.push_str("    float foot = float(abs(dr) * gFoot);\n");
    s.push_str("    float lw = clamp((log(max(foot, 1e-30)) * 0.4343 + 4.0) * 0.25, 0.0, 1.0); lw = 4.0 * lw * (1.0 - lw);\n");
    s.push_str("    ws += lw; t0 += lw * sqrt(float(rr)); t1 += lw * abs(float(z.x)); t2 += lw * length(vec2(z.xy)); t3 += lw * abs(float(z.y));\n");
    s.push_str("    if (rr > R(uBail * uBail)) break;\n  }\n");
    s.push_str("  trap = vec4(t0, t1, t2, t3) / max(ws, 1e-6);\n");
    s.push_str("  dr = max(abs(dr), R(1e-37));\n");
    if de_mode == 3 {
        // Still on the reference at the end: the wall is too close to its plane
        // for absolute coordinates, so measure the distance from the offset.
        s.push_str("  if (pert) return uDEp * abs(ysc + e.y) / max(d, 1e-37);\n");
    }
    s.push_str("  R r = length(z);\n");
    s.push_str(match de_mode {
        1 => "  return 0.5 * R(log(max(float(r), 1e-30))) * r / dr;\n",
        2 => "  return length(max(abs(z.xyz) - V3(vec3(uDEp)), V3(vec3(0.0)))) / dr;\n",
        3 => "  return R(uDEp) * abs(z.y) / dr;\n",
        _ => "  return (r - R(uDEp)) / dr;\n",
    });
    s.push_str("}\n");
    s
}

pub const FRAME_GLSL: &str = include_str!("../shaders/frame.glsl");
pub const VERT_GLSL: &str = include_str!("../shaders/fullscreen.vert");
pub const POST_GLSL: &str = include_str!("../shaders/post.frag");

/// Full fragment shader for a formula.
pub fn fragment_source(formula: &str) -> String {
    format!("#version 330 core\n{}", FRAME_GLSL.replace("//@FORMULA@", formula))
}
