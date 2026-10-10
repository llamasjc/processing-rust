// A box and a sphere, lit, turning in 3D. Move the mouse to turn the light.

let mut a = 0.0f32;

fn setup() {
    size(640, 400, P3D);
}

fn draw() {
    background(24);
    ambient_light(60, 60, 70);
    let lx = (mouse_x() / width() - 0.5) * 2.0;
    let ly = (mouse_y() / height() - 0.5) * 2.0;
    directional_light(220, 210, 200, -lx, -ly, -1);

    push_matrix();
    translate(220, 200, 0);
    rotate_x(a * 0.7);
    rotate_y(a);
    fill(220, 90, 60);
    stroke(255, 255, 255, 120);
    box(140);
    pop_matrix();

    push_matrix();
    translate(470, 200, -60);
    no_stroke();
    fill(70, 150, 255);
    sphere(110);
    pop_matrix();

    a += 0.01;
}
