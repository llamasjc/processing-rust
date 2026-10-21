// A vertex shader bends a flat grid into waves; the fragment shader colors it
// by how high each spot went. Works on both renderers.

let wave = load_shader_vf("wave.frag", "wave.vert");

fn setup() {
    size(400, 300);
}

fn draw() {
    background(10, 10, 30);
    no_stroke();
    fill(255);
    wave.set("time", millis() as f32 / 500.0);
    shader(wave);
    translate(width() / 2.0, height() / 2.0 + 20.0, -40.0);
    rotate_x(1.0);
    let step = 20.0;
    let mut y = -160.0;
    while y < 160.0 {
        begin_shape(QUAD_STRIP);
        let mut x = -200.0;
        while x <= 200.0 {
            vertex(x, y);
            vertex(x, y + step);
            x += step;
        }
        end_shape();
        y += step;
    }
    reset_shader();
}
