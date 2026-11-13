#version 330 core
out vec2 vUV;
void main() {
  // One oversized triangle covering the viewport; no vertex buffers needed.
  vec2 p = vec2(float((gl_VertexID & 1) << 2) - 1.0, float((gl_VertexID & 2) << 1) - 1.0);
  vUV = p * 0.5 + 0.5;
  gl_Position = vec4(p, 0.0, 1.0);
}
