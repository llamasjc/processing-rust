//! Microphone input and analysis: mic_start, mic_level, mic_waveform,
//! mic_spectrum. The default input device is opened on its own thread; the
//! newest samples (mono) sit in a ring that the sketch reads from.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

const RING: usize = 1 << 15; // ~0.7 s at 48 kHz

struct Ring { buf: Vec<f32>, pos: usize, rate: u32 }

fn ring() -> &'static parking_lot::Mutex<Ring> {
    static R: OnceLock<parking_lot::Mutex<Ring>> = OnceLock::new();
    R.get_or_init(|| parking_lot::Mutex::new(Ring { buf: vec![0.0; RING], pos: 0, rate: 44_100 }))
}
static RUNNING: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static FAILED: AtomicBool = AtomicBool::new(false);

fn push(samples: &[f32], channels: usize) {
    if PAUSED.load(Ordering::Relaxed) { return; }
    let mut r = ring().lock();
    let ch = channels.max(1);
    for frame in samples.chunks(ch) {
        let v = frame.iter().sum::<f32>() / ch as f32;
        let p = r.pos;
        r.buf[p] = if v.is_finite() { v } else { 0.0 };
        r.pos = (p + 1) % RING;
    }
}

/// Opens the default microphone. true once it is listening; false (and a
/// message, once) when there is no input device.
#[cfg(not(feature = "wasm"))]
pub fn mic_start() -> bool {
    PAUSED.store(false, Ordering::Relaxed);
    if RUNNING.load(Ordering::Relaxed) { return true; }
    if FAILED.load(Ordering::Relaxed) { return false; }
    let (tx, rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    let spawned = std::thread::Builder::new().name("microphone".into()).spawn(move || {
        use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use rodio::cpal::{SampleFormat, StreamConfig};
        let host = rodio::cpal::default_host();
        let Some(dev) = host.default_input_device() else { let _ = tx.send(Err("no microphone found".into())); return };
        let cfg = match dev.default_input_config() { Ok(c) => c, Err(e) => { let _ = tx.send(Err(e.to_string())); return } };
        let format = cfg.sample_format();
        let config: StreamConfig = cfg.into();
        let ch = config.channels as usize;
        let rate = config.sample_rate.0;
        let err = |e: rodio::cpal::StreamError| eprintln!("microphone: {e}");
        let stream = match format {
            SampleFormat::F32 => dev.build_input_stream(&config, move |d: &[f32], _: &_| push(d, ch), err, None),
            SampleFormat::I16 => dev.build_input_stream(&config, move |d: &[i16], _: &_| {
                let v: Vec<f32> = d.iter().map(|s| *s as f32 / 32768.0).collect(); push(&v, ch)
            }, err, None),
            SampleFormat::U16 => dev.build_input_stream(&config, move |d: &[u16], _: &_| {
                let v: Vec<f32> = d.iter().map(|s| (*s as f32 - 32768.0) / 32768.0).collect(); push(&v, ch)
            }, err, None),
            SampleFormat::I32 => dev.build_input_stream(&config, move |d: &[i32], _: &_| {
                let v: Vec<f32> = d.iter().map(|s| *s as f32 / 2147483648.0).collect(); push(&v, ch)
            }, err, None),
            other => { let _ = tx.send(Err(format!("unsupported sample format {other:?}"))); return }
        };
        let stream = match stream { Ok(s) => s, Err(e) => { let _ = tx.send(Err(e.to_string())); return } };
        if let Err(e) = stream.play() { let _ = tx.send(Err(e.to_string())); return; }
        let _ = tx.send(Ok(rate));
        let _keep = stream;
        loop { std::thread::park(); }
    });
    if spawned.is_err() { FAILED.store(true, Ordering::Relaxed); return false; }
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(rate)) => { ring().lock().rate = rate; RUNNING.store(true, Ordering::Relaxed); true }
        Ok(Err(e)) => { eprintln!("mic_start: {e}"); FAILED.store(true, Ordering::Relaxed); false }
        Err(_) => { eprintln!("mic_start: the microphone did not answer"); FAILED.store(true, Ordering::Relaxed); false }
    }
}
#[cfg(feature = "wasm")]
pub fn mic_start() -> bool { false }

/// Stops reading (the device stays open, so mic_start resumes at once).
pub fn mic_stop() {
    PAUSED.store(true, Ordering::Relaxed);
    let mut r = ring().lock();
    for v in r.buf.iter_mut() { *v = 0.0; }
}
pub fn mic_sample_rate() -> u32 { ring().lock().rate }

/// The newest n samples, oldest first, each -1..1.
pub fn mic_waveform(n: usize) -> Vec<f32> {
    let n = n.clamp(1, RING);
    let r = ring().lock();
    (0..n).map(|i| r.buf[(r.pos + RING - n + i) % RING]).collect()
}

/// How loud it is right now, 0..1 (root mean square of the last ~20 ms).
pub fn mic_level() -> f32 {
    let n = (mic_sample_rate() as usize / 50).max(64);
    rms(&mic_waveform(n))
}

pub fn rms(s: &[f32]) -> f32 {
    if s.is_empty() { return 0.0; }
    (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt().min(1.0)
}

/// In-place radix-2 FFT. re and im must have the same power-of-two length.
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j |= bit;
        if i < j { re.swap(i, j); im.swap(i, j); }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr; im[b] = im[a] - ti;
                re[a] += tr; im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

/// Spectrum of some samples: `bands` values, low to high pitch, each about
/// 0..1 (a full-scale sine reads near 1). Uses a Hann window.
pub fn spectrum(samples: &[f32], bands: usize) -> Vec<f32> {
    let bands = bands.max(1);
    let n = (bands * 2).next_power_of_two().max(64).min(samples.len().next_power_of_two().max(64));
    let start = samples.len().saturating_sub(n);
    let mut re: Vec<f32> = (0..n).map(|i| {
        let s = samples.get(start + i).copied().unwrap_or(0.0);
        let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
        s * w
    }).collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    let half = n / 2;
    // magnitude, scaled so a full-scale sine (Hann window halves it) is ~1
    let mags: Vec<f32> = (0..half).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * 4.0 / n as f32).collect();
    // group bins into bands (equal width, like Processing's FFT)
    (0..bands).map(|b| {
        let lo = b * half / bands;
        let hi = ((b + 1) * half / bands).max(lo + 1).min(half);
        mags[lo..hi].iter().cloned().fold(0.0, f32::max)
    }).collect()
}

/// mic_spectrum(64): how much of each pitch the microphone hears, 64 bands
/// from low to high, each about 0..1.
pub fn mic_spectrum(bands: usize) -> Vec<f32> {
    let n = (bands.max(1) * 2).next_power_of_two().max(1024);
    spectrum(&mic_waveform(n), bands)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fft_finds_the_tone() {
        let rate = 1024.0;
        let s: Vec<f32> = (0..1024).map(|i| (2.0 * std::f32::consts::PI * 64.0 * i as f32 / rate).sin()).collect();
        let sp = spectrum(&s, 512);
        let peak = sp.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
        assert_eq!(peak.0, 64);
        assert!((peak.1 - 1.0).abs() < 0.1, "{}", peak.1);
        assert!(sp[200] < 0.01);
        assert!((rms(&s) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        assert_eq!(spectrum(&[], 8).len(), 8);
    }
    #[test]
    fn ring_keeps_newest() {
        push(&[0.5, 0.5, -1.0, -1.0], 2);
        let w = mic_waveform(2);
        assert_eq!(w, vec![0.5, -1.0]);
        assert!(mic_level() > 0.0);
    }
}
