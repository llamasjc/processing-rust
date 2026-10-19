uniform vec2 resolution;
uniform float time;
const float PI = 3.14159265;
mat2 rot(float a) { float c = cos(a), s = sin(a); return mat2(c, -s, s, c); }
void split(in vec2 p, out float a, out float b) { a = p.x; b = p.y; }
float checker(vec2 p) { return mod(floor(p.x) + floor(p.y), 2.0); }
void main() {
  vec2 uv = (gl_FragCoord.xy - 0.5 * resolution) / resolution.y;
  uv = rot(time) * uv;
  float a, b;
  split(uv, a, b);
  vec3 col = vec3(checker(uv * 8.0));
  col.rb *= vec2(a + 0.5, b + 0.5);
  if (length(uv) > 0.45) col = vec3(0.1);
  gl_FragColor = vec4(col, 1);
}
