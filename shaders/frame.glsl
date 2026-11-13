// Fractal Worlds raymarcher. The #version line is prepended by
// formula::fragment_source, which also injects the generated deMap.
//
// Positions are offsets from the camera measured in units of the zoom scale S,
// and deMap follows the camera's reference orbit (uRef), which gives unlimited
// zoom with 32-bit maths.
#define R float
#define V3 vec3
#define V4 vec4
#define M3 mat3
#define MAX_OPS 12
const float PI = 3.14159265;

in vec2 vUV;
out vec4 oCol;

uniform vec2 uRes;
uniform vec2 uJitter;
uniform int uMode;            // 0 = image, 1 = probe (distance at the camera + along uProbeDir)
uniform vec3 uProbeDir;
uniform mat3 uCamRot;
uniform float uFovT;
uniform int uMaxSteps;
uniform float uBail;
uniform float uDetail;
uniform float uStepScale;
uniform float uMinEps;
uniform float uFar;
uniform float uDEp;
uniform vec4 uOp[MAX_OPS];
uniform mat3 uOpM[MAX_OPS];
uniform vec4 uJulia;
uniform float uJuliaMix;

uniform vec3 uStops[5];
uniform int uSrc;
uniform int uSrc2;
uniform float uSrcMix;
uniform float uFreq;
uniform float uOff;

uniform float uRough;
uniform float uMetal;
uniform float uSpec;
uniform float uRim;
uniform float uIrid;
uniform float uEmit;
uniform float uReflect;
uniform float uAO;
uniform float uGlow;
uniform vec3 uGlowCol;

uniform vec3 uSunDir;
uniform vec3 uSunCol;
uniform float uSunDisk;
uniform vec3 uSkyTop;
uniform vec3 uSkyHor;
uniform float uAmb;
uniform float uFogK;          // fog density per marching unit
uniform vec3 uFogCol;
uniform float uShadow;

// Size of a pixel at the point being evaluated, in marching units. deMap uses
// it to weight colour toward the levels whose detail is near pixel size.
R gFoot = R(1e-3);

void rot4(inout V4 v, vec3 a) {
  float c1 = cos(a.x), s1 = sin(a.x), c2 = cos(a.y), s2 = sin(a.y), c3 = cos(a.z), s3 = sin(a.z);
  V4 q = v;
  v.x = R(c1) * q.x - R(s1) * q.w; v.w = R(s1) * q.x + R(c1) * q.w; q = v;
  v.y = R(c2) * q.y - R(s2) * q.w; v.w = R(s2) * q.y + R(c2) * q.w; q = v;
  v.z = R(c3) * q.z - R(s3) * q.w; v.w = R(s3) * q.z + R(c3) * q.w;
}

#define DRU uS
uniform sampler2D uRef;       // reference orbit: one row per level
uniform int uLevels;          // rows that can be followed in offset form
uniform int uTotal;           // iterations every point runs
uniform float uTau;           // offset size (in level coordinates) at which a point goes absolute
uniform float uS;             // zoom scale as a float; underflows to 0 when very deep
uniform vec3 uCamF;           // camera position rounded to float

// |w + e| - |w| without cancellation, per component.
vec3 pAbs(vec3 w, vec3 e) {
  vec3 s = mix(vec3(-1.0), vec3(1.0), vec3(greaterThanEqual(w, vec3(0.0))));
  vec3 same = vec3(greaterThanEqual((w + e) * s, vec3(0.0)));
  return mix(-s * (2.0 * w + e), s * e, same);
}
// Reflection of the half-space n.z < d. `a` is the reference's signed distance in zoom units.
void pPlane(inout vec4 e, float a, vec3 n) {
  float b = dot(e.xyz, n);
  float t = min(b, -a) - min(0.0, -a);
  e.xyz -= t * n * (2.0 / dot(n, n));
}
// z *= num / clamp(|z|^2, lo, hi): sphere fold and inversion. Bm, Bf are the
// reference's distances to lo and hi in zoom units; cr is its clamped |z|^2.
void pRadial(inout vec4 e, inout float d, vec3 Z, float cr, float Bm, float Bf, float uf) {
  float ds = 2.0 * dot(Z, e.xyz) + dot(e.xyz * uf, e.xyz);
  float dcs = clamp(ds, Bm, Bf) - clamp(0.0, Bm, Bf);
  float cn = cr + uf * dcs;
  float ratio = cr / cn;
  e.xyz = ratio * e.xyz - dcs * Z / cn;
  d *= ratio;
}
float sincf(float x) { return abs(x) < 1e-3 ? 1.0 - x * x / 6.0 : sin(x) / x; }
// atan(num * uf, den) / uf, stable as uf goes to zero.
float angS(float num, float den, float uf) {
  float x = num * uf;
  if (den > 0.0 && abs(x) < 1e-3 * den) { float q = num / den; return q * (1.0 - x * x / (3.0 * den * den)); }
  return atan(x, den) / max(uf, 1e-37);
}
// Bulb power. T0 = (Z, |Z|), T1 = sin/cos of the reference's output angles, rn = |Z|^n.
void pBulb(inout vec4 e, inout float d, vec4 T0, vec4 T1, float rn, float uf, float g, float n) {
  vec3 Z = T0.xyz;
  float r = max(T0.w, 1e-30);
  vec3 eu = e.xyz * uf;
  float rho = max(length(Z.xy), 1e-30);
  float rNew = length(Z + eu), rhoNew = length(Z.xy + eu.xy);
  float drS = (2.0 * dot(Z, e.xyz) + dot(eu, e.xyz)) / (rNew + r);
  float dRhoS = (2.0 * dot(Z.xy, e.xy) + dot(eu.xy, e.xy)) / (rhoNew + rho);
  float aS = n * angS(Z.z * dRhoS - rho * e.z, Z.z * (Z.z + eu.z) + rho * (rho + uf * dRhoS), uf);
  float bS = n * angS(Z.x * e.y - Z.y * e.x, rho * rho + dot(Z.xy, eu.xy), uf);
  float a = aS * uf, b = bS * uf;
  float sT = T1.x, cT = T1.y, sP = T1.z, cP = T1.w;
  float saS = aS * sincf(a), haS = 0.5 * aS * sincf(0.5 * a), ha = sin(0.5 * a);
  float sbS = bS * sincf(b), hbS = 0.5 * bS * sincf(0.5 * b), hb = sin(0.5 * b);
  float dsT = cT * saS - 2.0 * sT * ha * haS, dcT = -sT * saS - 2.0 * cT * ha * haS;
  float dsP = cP * sbS - 2.0 * sP * hb * hbS, dcP = -sP * sbS - 2.0 * cP * hb * hbS;
  float sT2 = sT + uf * dsT, cT2 = cT + uf * dcT, sP2 = sP + uf * dsP, cP2 = cP + uf * dcP;
  vec3 dir2 = vec3(sT2 * cP2, sT2 * sP2, cT2);
  vec3 ddir = vec3(dsT * cP2 + sT * dcP, dsT * sP2 + sT * dsP, dcT);
  float x = uf * drS / r;
  float G = abs(x) < 1e-3 ? n * drS / r * (1.0 + 0.5 * (n - 1.0) * x + (n - 1.0) * (n - 2.0) / 6.0 * x * x)
                          : (pow(1.0 + x, n) - 1.0) / max(uf, 1e-37);
  e.xyz = rn * (G * dir2 + ddir) / g;
  d *= pow(max(1.0 + x, 1e-30), n - 1.0);
}
// Quaternion square. T0 is the reference point.
void pQuat(inout vec4 e, inout float d, vec4 Z, float uf, float g) {
  vec4 eu = e * uf;
  vec4 lin = 2.0 * vec4(Z.x * e.x - dot(Z.yzw, e.yzw), Z.x * e.yzw + e.x * Z.yzw);
  vec4 sq = vec4(e.x * eu.x - dot(e.yzw, eu.yzw), 2.0 * e.x * eu.yzw);
  d *= length(Z + eu) / max(length(Z), 1e-30);
  e = (lin + sq) / g;
}

//@FORMULA@

R pixelCone() { return R(uDetail * 2.0 * uFovT / uRes.y); }

// Sphere-trace from ro along rd. t0 is the distance already travelled by the
// parent ray, so secondary rays keep the same level of detail.
bool march(V3 ro, vec3 rd, R t0, R tmax, int maxSteps, out R tHit, out vec4 trap, out float steps) {
  R t = R(0.0);
  R cone = pixelCone();
  steps = 0.0;
  trap = vec4(0.0);
  for (int i = 0; i < maxSteps; i++) {
    R e = max((t0 + t) * cone, R(uMinEps));
    gFoot = e;
    R d = deMap(ro + V3(rd) * t, trap);
    if (d < e) { tHit = t; return true; }
    t += d * R(uStepScale);
    steps += 1.0;
    if (t > tmax) break;
  }
  tHit = t;
  return false;
}

vec3 calcNormal(V3 p, R e) {
  vec4 tr;
  vec2 k = vec2(1.0, -1.0);
  vec3 n = k.xyy * float(deMap(p + V3(k.xyy) * e, tr) / e)
         + k.yyx * float(deMap(p + V3(k.yyx) * e, tr) / e)
         + k.yxy * float(deMap(p + V3(k.yxy) * e, tr) / e)
         + k.xxx * float(deMap(p + V3(k.xxx) * e, tr) / e);
  return normalize(n + vec3(1e-9, 0.0, 0.0));
}

float softShadow(V3 ro, vec3 rd, R e, R tmax) {
  float res = 1.0;
  R t = e * 4.0;
  vec4 tr;
  for (int i = 0; i < 80; i++) {
    R d = deMap(ro + V3(rd) * t, tr);
    if (d < e * 0.5) return 0.0;
    res = min(res, uShadow * float(d / t));
    t += d;
    if (t > tmax) break;
  }
  return clamp(res, 0.0, 1.0);
}

float calcAO(V3 p, vec3 n, R t, R e) {
  float occ = 0.0;
  float w = 1.0;
  vec4 tr;
  for (int i = 1; i <= 4; i++) {
    R h = e + t * R(0.012 * float(i * i));
    float d = float(deMap(p + V3(n) * h, tr) / h);
    occ += (1.0 - clamp(d, 0.0, 1.0)) * w;
    w *= 0.7;
  }
  return clamp(1.0 - uAO * occ * 0.5, 0.0, 1.0);
}

vec3 sky(vec3 d) {
  float up = smoothstep(-0.25, 1.0, d.y);
  vec3 c = mix(uSkyHor, uSkyTop, pow(up, 0.75)) * mix(0.35, 1.0, smoothstep(-0.6, 0.1, d.y));
  float s = max(dot(d, uSunDir), 0.0);
  c += uSunCol * uSunDisk * (pow(s, 900.0) * 3.0 + pow(s, 12.0) * 0.06);
  return c;
}

vec3 palette(float t) {
  t = fract(t) * 5.0;
  int i = int(floor(t));
  if (i > 4) i = 4;
  int j = i + 1;
  if (j > 4) j = 0;
  float f = t - float(i);
  f = f * f * (3.0 - 2.0 * f);
  return mix(uStops[i], uStops[j], f);
}

float srcVal(int s, vec4 tr, vec3 n, vec3 pf) {
  if (s == 0) return tr.x;                            // orbit: distance from the centre
  if (s == 1) return tr.y * 2.0;                      // orbit: distance from the x = 0 plane
  if (s == 2) return tr.z;                            // orbit: distance from the z axis
  if (s == 3) return tr.w * 2.0;                      // orbit: distance from the y = 0 plane
  if (s == 4) return dot(n, vec3(0.577)) * 0.5 + 0.5; // surface direction
  if (s == 5) return length(pf);                      // distance from the world centre
  return pf.y;                                        // height
}

// Lit colour of a surface point. `full` turns on shadows and sampled occlusion.
vec3 surface(V3 p, vec3 rd, R t, R e, vec4 trap, float g, bool full, out vec3 n, out vec3 Fa) {
  n = calcNormal(p, e);
  vec3 pf = uCamF + p * uS;
  float ndv = max(dot(n, -rd), 0.02);
  float ct = mix(srcVal(uSrc, trap, n, pf), srcVal(uSrc2, trap, n, pf), uSrcMix) * uFreq + uOff
           + uIrid * pow(1.0 - ndv, 2.0);
  vec3 alb = palette(ct);

  vec3 L = uSunDir;
  float ndl = max(dot(n, L), 0.0);
  vec3 H = normalize(L - rd);
  float ndh = max(dot(n, H), 0.0);
  float vdh = max(dot(-rd, H), 0.0);

  float sh = 1.0;
  if (full && uShadow > 0.0 && ndl > 0.0) sh = softShadow(p + V3(n) * e * 2.0, L, e, t * 25.0 + e * 200.0);
  float ao = 1.0 - 0.6 * uAO * g;
  if (full && uAO > 0.0) ao *= calcAO(p, n, t, e);
  ao = clamp(ao, 0.0, 1.0);

  // GGX specular + Lambert diffuse.
  float a = max(uRough * uRough, 0.003);
  float a2 = a * a;
  float dd = ndh * ndh * (a2 - 1.0) + 1.0;
  float D = a2 / (PI * dd * dd);
  float k = (uRough + 1.0) * (uRough + 1.0) / 8.0;
  float G = (ndl / (ndl * (1.0 - k) + k)) * (ndv / (ndv * (1.0 - k) + k));
  vec3 F0 = mix(vec3(0.04 * uSpec), alb, uMetal);
  vec3 F = F0 + (1.0 - F0) * pow(1.0 - vdh, 5.0);
  vec3 spec = F * min(D * G / (4.0 * ndl * ndv + 1e-4), 40.0);
  vec3 dif = alb * (1.0 - uMetal) * (1.0 - F) / PI;
  vec3 col = (dif + spec) * uSunCol * ndl * sh;

  vec3 amb = mix(uSkyHor, uSkyTop, n.y * 0.5 + 0.5) * uAmb;
  col += alb * (1.0 - uMetal) * amb * ao;
  Fa = (F0 + (1.0 - F0) * pow(1.0 - ndv, 5.0) * (1.0 - uRough)) * ao;
  if (!full || uReflect <= 0.0) col += Fa * sky(reflect(rd, n)) * uAmb * (1.0 - 0.6 * uRough);
  col += uRim * pow(1.0 - ndv, 3.0) * (uSkyTop + uSkyHor) * 0.5 * ao;
  col += alb * uEmit;
  return col;
}

void main() {
  V3 ro = V3(0.0);
  R tHit;
  vec4 trap;
  float steps;

  if (uMode == 1) {
    vec4 tr;
    gFoot = R(uMinEps);
    R d0 = deMap(ro, tr);
    bool h = march(ro, uProbeDir, R(0.0), R(uFar), uMaxSteps, tHit, trap, steps);
    oCol = vec4(float(d0), h ? float(tHit) : -1.0, 0.0, 1.0);
    return;
  }

  vec2 ndc = (vUV * 2.0 - 1.0) + uJitter * 2.0 / uRes;
  vec3 rd = normalize(uCamRot * vec3(ndc.x * uFovT * uRes.x / uRes.y, ndc.y * uFovT, 1.0));
  bool hit = march(ro, rd, R(0.0), R(uFar), uMaxSteps, tHit, trap, steps);
  float g = steps / float(uMaxSteps);
  vec3 col = sky(rd);

  if (hit) {
    R e = max(tHit * pixelCone(), R(uMinEps));
    V3 p = ro + V3(rd) * tHit;
    vec3 n;
    vec3 Fa;
    gFoot = e;
    col = surface(p, rd, tHit, e, trap, g, true, n, Fa);
    if (uReflect > 0.0) {
      vec3 rr = reflect(rd, n);
      V3 o2 = p + V3(n) * e * 3.0;
      R t2;
      vec4 tr2;
      float st2;
      vec3 rc = sky(rr);
      if (march(o2, rr, tHit, tHit * 40.0 + e * 400.0, uMaxSteps / 3 + 8, t2, tr2, st2)) {
        vec3 n2;
        vec3 F2;
        R e2 = max((tHit + t2) * pixelCone(), R(uMinEps));
        gFoot = e2;
        rc = surface(o2 + V3(rr) * t2, rr, tHit + t2, e2, tr2, 0.0, false, n2, F2);
      }
      col += Fa * rc * uReflect;
    }
    float fog = 1.0 - exp(-uFogK * float(tHit));
    col = mix(col, uFogCol, fog);
  }
  col += uGlowCol * uGlow * g * g;
  if (any(isnan(col)) || any(isinf(col))) col = vec3(0.0);
  oCol = vec4(clamp(col, 0.0, 64.0), 1.0);
}
