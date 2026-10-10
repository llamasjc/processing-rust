// Talk, whistle or clap: the bars show how much of each pitch the
// microphone hears, low on the left, high on the right. The circle is the
// overall loudness.

let mut smooth_bars = vec![0.0f32; 64];
let mut listening = false;

fn setup() {
    size(640, 360);
    listening = mic_start();
}

fn draw() {
    background(18);
    if !listening {
        fill(220);
        text_align(CENTER, CENTER);
        text("No microphone found", width() / 2.0, height() / 2.0);
        return;
    }
    let bands = mic_spectrum(64);
    let w = width() / 64.0;
    no_stroke();
    color_mode(HSB, 360, 100, 100);
    for i in 0..64 {
        // ease toward the new value so the bars don't flicker
        smooth_bars[i] += (bands[i] - smooth_bars[i]) * 0.3;
        let h = (smooth_bars[i] * 8.0).min(1.0) * (height() - 40.0);
        fill(i as f32 * 4.0, 70, 95);
        rect(i as f32 * w, height() - h, w - 2.0, h);
    }
    color_mode(RGB, 255);
    let level = mic_level();
    fill(255, 255, 255, 60);
    circle(width() / 2.0, 80.0, 20.0 + level * 400.0);
}
