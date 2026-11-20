//! Movie and Capture: video files and cameras, through ffmpeg.
//!
//!   let mut movie = Movie::new("clip.mp4");
//!   movie.loop_();
//!   ...
//!   if movie.available() { movie.read(); }
//!   image(&movie, 0, 0);
//!
//! ffmpeg (and ffprobe, for movies) has to be installed and on the PATH.
//! Frames are decoded on their own thread, so a slow decode never holds up
//! draw(). Sound isn't played.

use crate::Num;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

static WARNED: AtomicBool = AtomicBool::new(false);
fn no_ffmpeg(what: &str, e: impl std::fmt::Display) {
    if !WARNED.swap(true, Ordering::Relaxed) {
        let how = if cfg!(target_os = "macos") { "brew install ffmpeg" }
            else if cfg!(windows) { "winget install ffmpeg" }
            else if std::path::Path::new("/usr/bin/pacman").exists() { "sudo pacman -S ffmpeg" }
            else if std::path::Path::new("/usr/bin/dnf").exists() { "sudo dnf install ffmpeg" }
            else { "sudo apt install ffmpeg" };
        eprintln!("{what}: couldn't start ffmpeg ({e}). Video needs ffmpeg: install it with `{how}`, then run the sketch again.");
    }
}

/// Something image() and texture() can draw: an image id, a Movie or a Capture.
pub trait ImageSource { fn image_id(&self) -> u64; }
impl ImageSource for u64 { fn image_id(&self) -> u64 { *self } }
impl ImageSource for Movie { fn image_id(&self) -> u64 { self.img } }
impl ImageSource for Capture { fn image_id(&self) -> u64 { self.img } }
impl<T: ImageSource + ?Sized> ImageSource for &T { fn image_id(&self) -> u64 { (**self).image_id() } }
impl<T: ImageSource + ?Sized> ImageSource for &mut T { fn image_id(&self) -> u64 { (**self).image_id() } }

/// What the decoding thread and the sketch share.
#[derive(Default)]
struct Shared {
    frame: Vec<u8>,     // the newest frame, RGBA
    fresh: bool,        // not read yet
    time: f32,          // seconds into the movie of that frame
    playing: bool,
    looping: bool,
    speed: f32,
    seek: Option<f32>,  // jump() asked for this
    ended: bool,
    quit: bool,
}
struct Link { s: Mutex<Shared>, wake: Condvar }

fn rgba_to_image(img: u64, w: usize, h: usize, rgba: &[u8]) {
    if rgba.len() < w * h * 4 { return; }
    let px: Vec<crate::Color> = rgba.chunks_exact(4).map(|p| crate::Color::from_rgba(
        p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, 1.0)).collect();
    crate::image_set_pixels(img, &px);
}

fn read_frame(out: &mut impl Read, buf: &mut [u8]) -> bool { out.read_exact(buf).is_ok() }

// ── Movie ────────────────────────────────────────────────────────────────

pub struct Movie {
    img: u64,
    w: usize,
    h: usize,
    fps: f32,
    duration: f32,
    link: Arc<Link>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn probe(path: &str) -> Option<(usize, usize, f32, f32)> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,avg_frame_rate,r_frame_rate:format=duration",
               "-of", "default=noprint_wrappers=1", path])
        .stderr(Stdio::null()).output();
    let out = match out { Ok(o) => o, Err(e) => { no_ffmpeg("Movie", e); return None; } };
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut w, mut h, mut fps, mut dur) = (0usize, 0usize, 0.0f32, 0.0f32);
    let rate = |v: &str| -> f32 {
        match v.split_once('/') {
            Some((a, b)) => { let (a, b) = (a.parse::<f32>().unwrap_or(0.0), b.parse::<f32>().unwrap_or(0.0)); if b > 0.0 { a / b } else { 0.0 } }
            None => v.parse().unwrap_or(0.0),
        }
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "width" => w = v.parse().unwrap_or(0),
            "height" => h = v.parse().unwrap_or(0),
            "avg_frame_rate" => { let r = rate(v); if r > 0.0 { fps = r; } }
            "r_frame_rate" => { if fps <= 0.0 { fps = rate(v); } }
            "duration" => dur = v.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    (w > 0 && h > 0).then_some((w, h, if fps > 0.0 && fps < 1000.0 { fps } else { 30.0 }, dur))
}

impl Movie {
    /// Opens a video file (looked for as given, then in data/). Shows its
    /// first frame once read; call play() or loop_() to start it.
    pub fn new(path: &str) -> Movie {
        let file = if std::path::Path::new(path).is_file() { path.to_string() } else { crate::paths::data_dir().join(path).to_string_lossy().into_owned() };
        let link = Arc::new(Link { s: Mutex::new(Shared { speed: 1.0, ..Default::default() }), wake: Condvar::new() });
        let Some((w, h, fps, duration)) = probe(&file) else {
            eprintln!("Movie(\"{path}\"): couldn't open it as a video");
            return Movie { img: 0, w: 0, h: 0, fps: 30.0, duration: 0.0, link, thread: None };
        };
        let img = crate::create_image(w as u32, h as u32);
        let l = link.clone();
        let thread = std::thread::spawn(move || movie_thread(file, w, h, fps, l));
        Movie { img, w, h, fps, duration, link, thread: Some(thread) }
    }
    fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let r = f(&mut self.link.s.lock().unwrap());
        self.link.wake.notify_all();
        r
    }
    pub fn play(&mut self) { self.with(|s| { if s.ended { s.seek = Some(0.0); s.ended = false; } s.playing = true; }); }
    /// loop() in Processing (loop is a Rust keyword).
    pub fn loop_(&mut self) { self.with(|s| { s.looping = true; s.playing = true; if s.ended { s.seek = Some(0.0); s.ended = false; } }); }
    #[inline] pub fn r#loop(&mut self) { self.loop_() }
    pub fn no_loop(&mut self) { self.with(|s| s.looping = false); }
    pub fn pause(&mut self) { self.with(|s| s.playing = false); }
    pub fn stop(&mut self) { self.with(|s| { s.playing = false; s.seek = Some(0.0); }); }
    /// jump(seconds): goes to that time.
    pub fn jump(&mut self, t: impl Num) { let t = t.f().max(0.0); self.with(|s| { s.seek = Some(t); s.ended = false; }); }
    /// speed(rate): 1 is normal, 2 twice as fast, 0.5 half.
    pub fn speed(&mut self, rate: impl Num) { let r = rate.f(); if r > 0.0 && r.is_finite() { self.with(|s| s.speed = r); } }
    /// volume(): accepted; sound isn't played.
    pub fn volume(&mut self, _v: impl Num) {}
    pub fn time(&self) -> f32 { self.with(|s| s.time) }
    pub fn duration(&self) -> f32 { self.duration }
    pub fn frame_rate(&self) -> f32 { self.fps }
    pub fn is_playing(&self) -> bool { self.with(|s| s.playing && !s.ended) }
    /// A new frame is waiting for read().
    pub fn available(&self) -> bool { self.with(|s| s.fresh) }
    /// Puts the newest frame into the movie's image.
    pub fn read(&mut self) {
        let frame = self.with(|s| { s.fresh = false; s.frame.clone() });
        if !frame.is_empty() { rgba_to_image(self.img, self.w, self.h, &frame); }
    }
    pub fn width(&self) -> f32 { self.w as f32 }
    pub fn height(&self) -> f32 { self.h as f32 }
    /// The image the frames go into, for get(), pixels and so on.
    pub fn image(&self) -> u64 { self.img }
    #[inline] pub fn isPlaying(&self) -> bool { self.is_playing() }
    #[inline] pub fn noLoop(&mut self) { self.no_loop() }
}
impl Drop for Movie {
    fn drop(&mut self) {
        self.with(|s| s.quit = true);
        if let Some(t) = self.thread.take() { let _ = t.join(); }
    }
}

fn spawn_decoder(file: &str, w: usize, h: usize, from: f32) -> Option<Child> {
    let mut c = Command::new("ffmpeg");
    c.args(["-v", "error", "-nostdin"]);
    if from > 0.0 { c.args(["-ss", &format!("{from:.3}")]); }
    c.args(["-i", file, "-an", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-"]);
    match c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(ch) => Some(ch),
        Err(e) => { no_ffmpeg("Movie", e); None }
    }
}

fn movie_thread(file: String, w: usize, h: usize, fps: f32, link: Arc<Link>) {
    let mut buf = vec![0u8; w * h * 4];
    let mut start_at = 0.0f32;
    'restart: loop {
        let Some(mut child) = spawn_decoder(&file, w, h, start_at) else { return };
        let Some(mut out) = child.stdout.take() else { return };
        let mut n = 0u64;
        // the first frame shows even before play()
        let mut clock = Instant::now();
        let mut base = start_at;
        loop {
            // wait while paused, and handle stop / jump / quit
            {
                let mut s = link.s.lock().unwrap();
                loop {
                    if s.quit { let _ = child.kill(); let _ = child.wait(); return; }
                    if let Some(t) = s.seek.take() { start_at = t; let _ = child.kill(); let _ = child.wait(); continue 'restart; }
                    if s.playing || n == 0 { break; }
                    s = link.wake.wait(s).unwrap();
                    clock = Instant::now();
                    base = s.time;
                }
            }
            if !read_frame(&mut out, &mut buf) {
                let _ = child.wait();
                let mut s = link.s.lock().unwrap();
                if s.looping && !s.quit { start_at = 0.0; continue 'restart; }
                s.ended = true;
                s.playing = false;
                // wait for play / jump / quit
                while !s.quit && s.seek.is_none() && !s.playing { s = link.wake.wait(s).unwrap(); }
                if s.quit { return; }
                start_at = s.seek.take().unwrap_or(0.0);
                s.ended = false;
                continue 'restart;
            }
            let t = start_at + n as f32 / fps;
            n += 1;
            let speed = {
                let mut s = link.s.lock().unwrap();
                s.frame.clear();
                s.frame.extend_from_slice(&buf);
                s.fresh = true;
                s.time = t;
                s.speed
            };
            // keep to the movie's own pace
            let due = Duration::from_secs_f32(((t - base) / speed).max(0.0));
            let now = clock.elapsed();
            if due > now {
                let s = link.s.lock().unwrap();
                let _ = link.wake.wait_timeout(s, due - now).unwrap();
            }
        }
    }
}

// ── Capture ──────────────────────────────────────────────────────────────

pub struct Capture {
    img: u64,
    w: usize,
    h: usize,
    device: String,
    fps: f32,
    link: Arc<Link>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The camera names Capture can open.
pub fn capture_list() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        let mut v: Vec<String> = std::fs::read_dir("/dev").map(|d| d.filter_map(|e| {
            let n = e.ok()?.file_name().to_string_lossy().into_owned();
            n.starts_with("video").then(|| format!("/dev/{n}"))
        }).collect()).unwrap_or_default();
        v.sort();
        v
    }
    #[cfg(not(target_os = "linux"))]
    {
        // ffmpeg prints the devices on stderr
        let (fmt, input) = if cfg!(target_os = "macos") { ("avfoundation", "") } else { ("dshow", "dummy") };
        let out = Command::new("ffmpeg").args(["-hide_banner", "-f", fmt, "-list_devices", "true", "-i", input]).output();
        let Ok(out) = out else { return Vec::new() };
        let text = String::from_utf8_lossy(&out.stderr);
        let mut names = Vec::new();
        let mut video = cfg!(target_os = "windows");
        for line in text.lines() {
            if line.contains("video devices") { video = true; continue; }
            if line.contains("audio devices") { video = false; continue; }
            if !video { continue; }
            if let (Some(a), Some(b)) = (line.find('"'), line.rfind('"')) {
                if b > a && !line.contains("Alternative name") && (cfg!(target_os = "windows") && line.contains("(video)") || cfg!(target_os = "macos")) {
                    names.push(line[a + 1..b].to_string());
                }
            } else if cfg!(target_os = "macos") {
                if let Some(i) = line.rfind("] ") { if line.contains("] [") { names.push(line[i + 2..].trim().to_string()); } }
            }
        }
        names
    }
}

impl Capture {
    /// The first camera, at w x h.
    pub fn new(w: impl Num, h: impl Num) -> Capture { Capture::with_device(w, h, "") }
    /// A camera by name (from capture_list()), at w x h.
    pub fn with_device(w: impl Num, h: impl Num, device: &str) -> Capture {
        let (w, h) = (w.f().max(1.0) as usize, h.f().max(1.0) as usize);
        let device = if device.is_empty() { capture_list().into_iter().next().unwrap_or_default() } else { device.to_string() };
        let link = Arc::new(Link { s: Mutex::new(Shared { speed: 1.0, ..Default::default() }), wake: Condvar::new() });
        let img = crate::create_image(w as u32, h as u32);
        Capture { img, w, h, device, fps: 30.0, link, thread: None }
    }
    /// with_fps(w, h, device, fps)
    pub fn with_fps(w: impl Num, h: impl Num, device: &str, fps: impl Num) -> Capture {
        let mut c = Capture::with_device(w, h, device);
        c.fps = fps.f().clamp(1.0, 240.0);
        c
    }
    pub fn start(&mut self) {
        if self.thread.is_some() { return; }
        if self.device.is_empty() && cfg!(target_os = "linux") { eprintln!("Capture: no camera found"); return; }
        { let mut s = self.link.s.lock().unwrap(); s.quit = false; s.playing = true; }
        let (dev, w, h, fps, link) = (self.device.clone(), self.w, self.h, self.fps, self.link.clone());
        self.thread = Some(std::thread::spawn(move || capture_thread(dev, w, h, fps, link)));
    }
    pub fn stop(&mut self) {
        { self.link.s.lock().unwrap().quit = true; }
        self.link.wake.notify_all();
        if let Some(t) = self.thread.take() { let _ = t.join(); }
    }
    pub fn available(&self) -> bool { self.link.s.lock().unwrap().fresh }
    pub fn read(&mut self) {
        let frame = { let mut s = self.link.s.lock().unwrap(); s.fresh = false; std::mem::take(&mut s.frame) };
        if !frame.is_empty() { rgba_to_image(self.img, self.w, self.h, &frame); }
    }
    pub fn width(&self) -> f32 { self.w as f32 }
    pub fn height(&self) -> f32 { self.h as f32 }
    pub fn image(&self) -> u64 { self.img }
}
impl Drop for Capture { fn drop(&mut self) { self.stop(); } }

fn capture_thread(device: String, w: usize, h: usize, fps: f32, link: Arc<Link>) {
    let mut c = Command::new("ffmpeg");
    c.args(["-v", "error", "-nostdin"]);
    let size = format!("{w}x{h}");
    let rate = format!("{fps}");
    if cfg!(target_os = "linux") {
        c.args(["-f", "v4l2", "-framerate", &rate, "-video_size", &size, "-i", &device]);
    } else if cfg!(target_os = "macos") {
        let dev = if device.is_empty() { "0".to_string() } else { device.clone() };
        c.args(["-f", "avfoundation", "-framerate", &rate, "-video_size", &size, "-i", &format!("{dev}:none")]);
    } else {
        c.args(["-f", "dshow", "-video_size", &size, "-i", &format!("video={device}")]);
    }
    c.args(["-an", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &size, "-"]);
    let mut child = match c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn() {
        Ok(ch) => ch,
        Err(e) => { no_ffmpeg("Capture", e); return; }
    };
    let Some(mut out) = child.stdout.take() else { return };
    let mut buf = vec![0u8; w * h * 4];
    loop {
        if link.s.lock().unwrap().quit { break; }
        if !read_frame(&mut out, &mut buf) { eprintln!("Capture: the camera stopped sending pictures"); break; }
        let mut s = link.s.lock().unwrap();
        s.frame.clear();
        s.frame.extend_from_slice(&buf);
        s.fresh = true;
    }
    let _ = child.kill();
    let _ = child.wait();
}
