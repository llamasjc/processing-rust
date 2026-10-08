// Rust Mode showcase: everything added recently in one sketch.
// Move the mouse, click to stamp the layer, press any key to clear it.

let mut layer: u64 = 0;
let mut font: u64 = 0;
let mut clicks: i32 = 0;

fn setup() {
    size(720, 420);
    layer = create_graphics(720, 420);
    // any installed font; 0 means none was found and text stays in the pixel font
    font = create_font("DejaVu Sans", 22);
}

fn draw() {
    background(24, 26, 38);

    // line ends: ROUND, SQUARE, PROJECT
    stroke(120, 180, 255);
    stroke_weight(14);
    stroke_cap(ROUND);
    line(40, 50, 180, 50);
    stroke_cap(SQUARE);
    line(40, 85, 180, 85);
    stroke_cap(PROJECT);
    line(40, 120, 180, 120);

    // corners: MITER, BEVEL, ROUND
    fill(90, 200, 140);
    stroke_weight(10);
    stroke_join(MITER);
    triangle(230, 130, 270, 40, 310, 130);
    stroke_join(BEVEL);
    triangle(340, 130, 380, 40, 420, 130);
    stroke_join(ROUND);
    triangle(450, 130, 490, 40, 530, 130);
    stroke_join(MITER);

    // arcs: plain, OPEN, CHORD, PIE
    stroke(255);
    stroke_weight(3);
    fill(255, 150, 40);
    arc(80, 220, 90, 90, 0, 4.5);
    arc(190, 220, 90, 90, 0, 4.5, OPEN);
    arc(300, 220, 90, 90, 0, 4.5, CHORD);
    arc(410, 220, 90, 90, 0, 4.5, PIE);

    // rectMode, a turning sheared shape, and a blend mode that brightens what is under it
    no_stroke();
    rect_mode(CENTER);
    push_matrix();
    translate(620, 90);
    rotate(frame_count() as f32 * 0.02);
    shear_x(0.35);
    fill(200, 80, 200);
    rect(0, 0, 90, 60);
    pop_matrix();
    rect_mode(CORNER);
    blend_mode(ADD);
    fill(0, 90, 200);
    ellipse(mouse_x(), mouse_y(), 110, 110);
    blend_mode(BLEND);

    // an off-screen layer: dots collect on it and it is drawn over everything
    begin_graphics(layer);
    no_stroke();
    fill(255, 220, 60, 90);
    ellipse(mouse_x(), mouse_y(), 12, 12);
    end_graphics();
    graphics_image(layer, 0, 0);

    // text in a real font, with the local time
    text_font(font);
    fill(255);
    push_matrix();
    translate(30, 330);
    rotate(-0.03);
    text(&format!("{:02}:{:02}:{:02}   clicks: {}   fps: {:.0}",
                  hour(), minute(), second(), clicks, current_frame_rate()), 0, 0);
    pop_matrix();
    text_font(0);
    text_size(16);
    fill(160, 170, 190);
    text("pixel font: click stamps a ring, any key clears", 30, 370);

    if frame_count() == 30 {
        save_frame("showcase.png");
    }
}

fn mouse_pressed() {
    clicks += 1;
    begin_graphics(layer);
    stroke(255, 80, 80);
    stroke_weight(3);
    no_fill();
    ellipse(mouse_x(), mouse_y(), 40, 40);
    end_graphics();
}

fn key_pressed() {
    begin_graphics(layer);
    clear();
    end_graphics();
}
