// Click to add a score. Scores are kept in data/scores.csv and the
// settings in data/settings.json, so they are still there next run.

let mut scores = Table::new();
let mut settings = Json::Null;

fn setup() {
    size(400, 300);
    settings = loadJSONObject("data/settings.json");
    if settings.is_null() {
        settings = Json::object();
        settings.set("title", "High scores").set("max_rows", 8);
        saveJSONObject(&settings, "data/settings.json");
    }
    scores = loadTable("data/scores.csv", "header");
    if scores.column_count() == 0 {
        scores.add_column("name").add_column("score");
    }
}

fn draw() {
    background(25);
    fill(255);
    text_size(22.0);
    text(&settings.get_string("title"), 20.0, 40.0);
    text_size(16.0);
    let shown = scores.row_count().min(settings.get_int("max_rows") as usize);
    for i in 0..shown {
        let y = 80.0 + i as f32 * 26.0;
        text(&scores.get_string(i, "name"), 20.0, y);
        text(&scores.get_int(i, "score").to_string(), 300.0, y);
    }
    fill(150);
    text("click to add a score", 20.0, 285.0);
}

fn mouse_pressed() {
    let r = scores.add_row();
    scores.set(r, "name", format!("player {}", r + 1));
    scores.set_int(r, "score", random(1000.0) as i32);
    scores.sort("score");
    scores.reverse();
    save_table(&scores, "data/scores.csv");
}
