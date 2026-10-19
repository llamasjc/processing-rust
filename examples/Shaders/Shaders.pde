// Filter shaders: GLSL that runs over the whole canvas, as in Processing.
// On the default renderer they run on the CPU, on libprocessing on the GPU.

let checker = load_shader("checker.glsl");
let ripple = shader_from_source("
  uniform sampler2D texture;
  uniform vec2 center;
  uniform float time;
  varying vec4 vertTexCoord;
  void main() {
    vec2 d = vertTexCoord.st - center;
    float wave = sin(length(d) * 60.0 - time * 4.0) * 0.004;
    gl_FragColor = texture2D(texture, vertTexCoord.st + normalize(d + 0.0001) * wave);
  }
");

fn setup() {
    size(320, 240);
}

fn draw() {
    let t = millis() as f32 / 1000.0;
    checker.set("resolution", width(), height());
    checker.set("time", t * 0.5);
    filter(checker);
    ripple.set("center", mouse_x() / width(), mouse_y() / height());
    ripple.set("time", t);
    filter(ripple);
}
