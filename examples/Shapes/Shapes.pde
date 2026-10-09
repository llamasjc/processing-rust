// Shapes built once with create_shape and drawn every frame, and text
// wrapped inside a box.

let mut star = create_shape();
let mut group = create_shape_group();


fn setup() {
    size(300, 200);
    star.begin_shape();
    star.fill(255, 200, 0);
    star.stroke(80);
    star.stroke_weight(2);
    for i in 0..10 {
        let a = i as f32 * TWO_PI / 10.0 - HALF_PI;
        let r = if i % 2 == 0 { 40.0 } else { 18.0 };
        star.vertex(a.cos() * r, a.sin() * r);
    }
    star.end_shape(CLOSE);
    let mut dot = create_shape_ellipse(0, 0, 20, 20);
    dot.fill(color(0, 120, 255));
    dot.no_stroke();
    group.add_child(dot);
    group.add_child(create_shape_rect(20, -10, 30, 20));
}

fn draw() {
    background(240);
    shape(&star, 60, 60);
    star.rotate(0.01);
    shape(&group, 150, 60);
    shape(&group, 150, 120, 50, 50);
    fill(30);
    text_align(CENTER, CENTER);
    text("This sentence is long enough that it has to wrap inside the little box.", 200, 100, 90, 90);
    text_align(LEFT, TOP);
}
