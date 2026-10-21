// Moves a flat grid up and down in waves and passes how high each corner went
uniform mat4 transform;
uniform mat3 normalMatrix;
uniform float time;
attribute vec4 position;
attribute vec4 color;
varying vec4 vertColor;
varying float height;

void main() {
  vec4 p = position;
  height = sin(p.x * 0.05 + time) * cos(p.y * 0.05 + time * 0.7);
  p.z += height * 30.0;
  gl_Position = transform * p;
  vertColor = color;
}
