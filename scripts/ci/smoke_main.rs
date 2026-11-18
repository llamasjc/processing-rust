// The sketch CI builds against each precompiled cache and runs for a moment:
// the way RustMode builds a sketch (one rustc call), then a window that
// draws, loads a font's worth of text, and closes itself.
use processing::*;

struct S { x: f32 }

fn setup(_s: &mut S) {
    size(320, 240);
}

fn draw(s: &mut S) {
    background(30);
    s.x = (s.x + 3.0) % width();
    fill_rgb(255, 120, 0);
    ellipse(s.x, height() / 2.0, 40.0, 40.0);
    text("smoke", 10.0, 20.0);
    if frame_count() >= 60 {
        println!("smoke ok");
        exit();
    }
}

fn main() {
    App::new(S { x: 0.0 }, setup, draw).title("smoke").run();
}
