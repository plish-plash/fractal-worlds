#version 330 core
// Turns the accumulated linear HDR image into display colour.
in vec2 vUV;
out vec4 oCol;
uniform sampler2D uTex;
uniform float uExposure;
uniform float uContrast;
uniform float uSaturation;
uniform float uVignette;
uniform float uGamma;
uniform int uTonemap;

vec3 aces(vec3 x) {
  return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

void main() {
  vec3 c = texture(uTex, vUV).rgb * uExposure;
  vec2 q = vUV - 0.5;
  c *= max(1.0 - uVignette * dot(q, q) * 2.0, 0.0);
  if (uTonemap == 0) c = aces(c);
  else if (uTonemap == 1) c = c / (1.0 + c);
  else c = clamp(c, 0.0, 1.0);
  c = pow(c, vec3(1.0 / max(uGamma, 0.1)));
  float l = dot(c, vec3(0.2126, 0.7152, 0.0722));
  c = mix(vec3(l), c, uSaturation);
  c = (c - 0.5) * uContrast + 0.5;
  oCol = vec4(clamp(c, 0.0, 1.0), 1.0);
}
