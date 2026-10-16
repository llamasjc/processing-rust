// A cube painted with a picture made in code, turning in space.

let tiles = create_image(64, 64);

fn setup() {
    size(400, 400, P3D);
    for y in 0..64 {
        for x in 0..64 {
            let on = (x / 8 + y / 8) % 2 == 0;
            image_set(tiles, x, y, if on { color(255, 120, 40) } else { color(40, 60, 90) });
        }
    }
    texture_mode(NORMAL);
}

fn face(img: u64, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
    begin_shape();
    texture(img);
    vertex(a[0], a[1], a[2], 0, 0);
    vertex(b[0], b[1], b[2], 1, 0);
    vertex(c[0], c[1], c[2], 1, 1);
    vertex(d[0], d[1], d[2], 0, 1);
    end_shape();
}

fn draw() {
    background(20);
    lights();
    no_stroke();
    translate(width() / 2.0, height() / 2.0, 0);
    rotate_y(frame_count() as f32 * 0.01);
    rotate_x(frame_count() as f32 * 0.007);
    let s = 90.0;
    face(tiles, [-s, -s, s], [s, -s, s], [s, s, s], [-s, s, s]);
    face(tiles, [s, -s, -s], [-s, -s, -s], [-s, s, -s], [s, s, -s]);
    face(tiles, [-s, -s, -s], [-s, -s, s], [-s, s, s], [-s, s, -s]);
    face(tiles, [s, -s, s], [s, -s, -s], [s, s, -s], [s, s, s]);
    face(tiles, [-s, -s, -s], [s, -s, -s], [s, -s, s], [-s, -s, s]);
    face(tiles, [-s, s, s], [s, s, s], [s, s, -s], [-s, s, -s]);
}
