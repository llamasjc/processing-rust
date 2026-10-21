varying vec4 vertColor;
varying float height;

void main() {
  vec3 low = vec3(0.1, 0.2, 0.5);
  vec3 high = vec3(0.9, 0.95, 1.0);
  gl_FragColor = vec4(mix(low, high, height * 0.5 + 0.5) * vertColor.rgb, 1.0);
}
