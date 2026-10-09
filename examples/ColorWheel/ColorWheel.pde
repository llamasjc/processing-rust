// A color wheel drawn pixel by pixel in HSB, then shown as an image.
// Move the mouse over it to read the color under the pointer.

let mut wheel = create_image(240, 240);

fn setup() {
    size(400, 300);
    color_mode(HSB, 360, 100, 100);
    for y in 0..240 {
        for x in 0..240 {
            let dx = x as f32 - 120.0;
            let dy = y as f32 - 120.0;
            let d = (dx * dx + dy * dy).sqrt();
            if d < 118.0 {
                let h = (dy.atan2(dx).to_degrees() + 360.0) % 360.0;
                image_set(wheel, x, y, color(h, d / 118.0 * 100.0, 100));
            }
        }
    }
}

fn draw() {
    background(0, 0, 12);
    image(wheel, 20, 30);
    let c = image_get(wheel, mouse_x() as i32 - 20, mouse_y() as i32 - 30);
    fill(c);
    stroke(0, 0, 100);
    rect(290, 30, 90, 90);
    fill(0, 0, 100);
    text_size(14.0);
    text(&format!("hue {}", nf(hue(c).round() as i32, 3)), 290.0, 150.0);
    text(&format!("sat {}", nf(saturation(c).round() as i32, 3)), 290.0, 170.0);
    text(&format!("bri {}", nf(brightness(c).round() as i32, 3)), 290.0, 190.0);
}
