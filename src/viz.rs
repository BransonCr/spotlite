//! Sound visualizer: the spectrum of what the player engine (spotifyd, or the built-in
//! player) is playing.
//!
//! The engine gives only its own sound, at 22 kHz mono (see `engine::pcm`), so other sounds
//! on the machine do not show. A thread turns each 1024-sample
//! window into BANDS log-spaced levels. Nothing runs while the visualizer is off, the music
//! is paused, or the window does not have focus.

use std::io::Read;
use crate::engine::Closer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const BANDS: usize = 48;
const RATE: usize = 22_050;
/// FFT size: about 46 ms of sound, 21.5 Hz per bin.
const N: usize = 1024;
/// New samples per update: about 12 ms.
const HOP: usize = 256;
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 10_000.0;
/// Level range shown, in dB.
const FLOOR_DB: f32 = -62.0;
const RANGE_DB: f32 = 52.0;
/// Easing per update (~12 ms): bars rise over about 70 ms and fall over about 0.35 s,
/// so they glide instead of jumping.
const RISE: f32 = 0.18;
const FALL: f32 = 0.035;
/// Blur across neighbouring bands, so the shape is a gentle wave, not jagged bars.
const BLUR: [f32; 5] = [0.1, 0.2, 0.4, 0.2, 0.1];
/// Automatic gain: the loudest band is scaled toward this level; the reference loudness
/// follows the music up at once and down slowly (about 0.2% per update).
const AGC_TARGET: f32 = 0.7;
const AGC_FALL: f32 = 0.998;
/// Levels 0..=1 per band.
#[derive(Clone, Copy)]
pub struct Frame {
    pub levels: [f32; BANDS],
}

impl Frame {
    /// The level at `pos` (0 = lowest band, 1 = highest), with smooth blending between bands.
    pub fn at(&self, pos: f32) -> f32 {
        let pos = pos.clamp(0.0, 1.0) * (BANDS - 1) as f32;
        let (i, t) = (pos.floor() as usize, pos.fract());
        let j = (i + 1).min(BANDS - 1);
        // Smoothstep between bands: rounded curves instead of straight steps.
        let t = t * t * (3.0 - 2.0 * t);
        self.levels[i] * (1.0 - t) + self.levels[j] * t
    }
}

impl Default for Frame {
    fn default() -> Self {
        Frame { levels: [0.0; BANDS] }
    }
}

/// Peak markers for the side visualizer: each jumps to its band's level, rests a moment, then
/// falls with gravity, so the caps hang over the bars and drop back.
#[derive(Clone)]
pub struct Peaks {
    pub level: [f32; BANDS],
    /// Seconds each peak still rests before it falls.
    hold: [f32; BANDS],
    /// Fall speed of each peak, in levels per second.
    speed: [f32; BANDS],
    at: Option<Instant>,
}

impl Default for Peaks {
    fn default() -> Self {
        Peaks { level: [0.0; BANDS], hold: [0.0; BANDS], speed: [0.0; BANDS], at: None }
    }
}

/// A peak rests this long at its top.
const PEAK_HOLD: f32 = 0.35;
/// Gravity on a falling peak, in levels per second per second.
const PEAK_GRAVITY: f32 = 2.4;

impl Peaks {
    /// Moves the peaks on to now, from the bands of `frame`.
    pub fn update(&mut self, frame: &Frame) {
        let now = Instant::now();
        let dt = self.at.map_or(0.0, |t| (now - t).as_secs_f32()).min(0.1);
        self.at = Some(now);
        self.step(frame, dt);
    }

    fn step(&mut self, frame: &Frame, dt: f32) {
        for b in 0..BANDS {
            let l = frame.levels[b];
            if l >= self.level[b] {
                self.level[b] = l;
                self.hold[b] = PEAK_HOLD;
                self.speed[b] = 0.0;
            } else if self.hold[b] > 0.0 {
                self.hold[b] -= dt;
            } else {
                self.speed[b] += PEAK_GRAVITY * dt;
                self.level[b] = (self.level[b] - self.speed[b] * dt).max(l);
            }
        }
    }
}

pub struct Visualizer {
    frame: Arc<Mutex<Frame>>,
    stop: Arc<AtomicBool>,
    child: Option<Closer>,
    thread: Option<JoinHandle<()>>,
    last_start: Option<Instant>,
}

impl Default for Visualizer {
    fn default() -> Self {
        Visualizer {
            frame: Arc::new(Mutex::new(Frame::default())),
            stop: Arc::new(AtomicBool::new(false)),
            child: None,
            thread: None,
            last_start: None,
        }
    }
}

impl Visualizer {
    /// True while the capture thread is reading sound.
    pub fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    pub fn frame(&self) -> Frame {
        self.frame.lock().map(|f| *f).unwrap_or_default()
    }

    /// Starts capture if it is not running. The engine opens a new stream after a pause or a
    /// device change, which ends the old capture; retries are spaced one second apart.
    pub fn ensure_running(&mut self) {
        if self.running() || self.last_start.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.stop();
        self.last_start = Some(Instant::now());
        let Some(stream) = crate::engine::pcm() else { return };
        let (mut out, child) = (stream.reader, stream.closer);
        self.child = Some(child);
        self.stop.store(false, Ordering::SeqCst);
        let (frame, stop) = (self.frame.clone(), self.stop.clone());
        self.thread = thread::Builder::new()
            .name("spotlite-viz".into())
            .spawn(move || {
                let mut analyzer = Analyzer::new();
                let mut buf = [0u8; HOP * 2];
                while !stop.load(Ordering::SeqCst) && out.read_exact(&mut buf).is_ok() {
                    let samples = buf.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0);
                    let next = analyzer.push(samples);
                    if let Ok(mut f) = frame.lock() {
                        *f = next;
                    }
                }
            })
            .ok();
    }

    /// Stops capture; the levels fall to zero.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.take() {
            child.close();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut f) = self.frame.lock() {
            *f = Frame::default();
        }
    }
}

impl Drop for Visualizer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// In-place radix-2 FFT (n a power of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

/// Turns a stream of samples into band levels.
pub struct Analyzer {
    ring: Vec<f32>,
    window: Vec<f32>,
    /// FFT bin range [start, end) of each band.
    bins: Vec<(usize, usize)>,
    frame: Frame,
    /// Loudest recent band level before gain.
    reference: f32,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Analyzer {
    pub fn new() -> Self {
        let window = (0..N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos())
            .collect();
        let hz_per_bin = RATE as f32 / N as f32;
        let bins = (0..BANDS)
            .map(|b| {
                let edge = |k: usize| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(k as f32 / BANDS as f32);
                let lo = (edge(b) / hz_per_bin).floor() as usize;
                let hi = (edge(b + 1) / hz_per_bin).ceil() as usize;
                (lo.max(1), hi.max(lo + 1).min(N / 2))
            })
            .collect();
        Analyzer {
            ring: vec![0.0; N],
            window,
            bins,
            frame: Frame::default(),
            reference: 0.0,
            re: vec![0.0; N],
            im: vec![0.0; N],
        }
    }

    pub fn push(&mut self, samples: impl IntoIterator<Item = f32>) -> Frame {
        let new: Vec<f32> = samples.into_iter().collect();
        let keep = N.saturating_sub(new.len());
        self.ring.drain(..N - keep);
        self.ring.extend(new.iter().rev().take(N - keep).rev());
        for i in 0..N {
            self.re[i] = self.ring[i] * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        let mut raw = [0.0f32; BANDS];
        for (b, &(lo, hi)) in self.bins.iter().enumerate() {
            let power = (lo..hi).map(|k| self.re[k] * self.re[k] + self.im[k] * self.im[k]).fold(0.0, f32::max);
            // Window gain (~N/4) to full-scale amplitude, then dB; tilt up highs a little.
            let amp = power.sqrt() / (N as f32 / 4.0);
            let db = 20.0 * amp.max(1e-9).log10() + 4.0 * b as f32 / BANDS as f32;
            raw[b] = ((db - FLOOR_DB) / RANGE_DB).clamp(0.0, 1.0);
        }
        let loudest = raw.iter().cloned().fold(0.0, f32::max);
        self.reference = loudest.max(self.reference * AGC_FALL);
        // Never boost more than 2x, so near-silence stays near zero.
        let gain = (AGC_TARGET / self.reference.max(0.45)).min(2.0);
        let blurred: Vec<f32> = (0..BANDS)
            .map(|b| {
                BLUR.iter()
                    .enumerate()
                    .map(|(k, w)| w * raw[(b + k).saturating_sub(2).min(BANDS - 1)])
                    .sum::<f32>()
            })
            .collect();
        for (b, r) in blurred.iter().enumerate() {
            let target = (r * gain).clamp(0.0, 1.0);
            let level = &mut self.frame.levels[b];
            *level += (target - *level) * if target > *level { RISE } else { FALL };
        }
        self.frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_finds_a_pure_tone() {
        let mut re: Vec<f32> = (0..64).map(|i| (2.0 * std::f32::consts::PI * 5.0 * i as f32 / 64.0).sin()).collect();
        let mut im = vec![0.0; 64];
        fft(&mut re, &mut im);
        let mags: Vec<f32> = (0..32).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let max = mags.iter().cloned().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        assert_eq!(max.0, 5);
        assert!((max.1 - 32.0).abs() < 0.5, "{}", max.1);
    }

    #[test]
    fn a_tone_lights_its_band_and_silence_falls() {
        let mut a = Analyzer::new();
        let tone = |i: usize| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / RATE as f32).sin();
        let mut f = Frame::default();
        for chunk in 0..40 {
            f = a.push((chunk * HOP..(chunk + 1) * HOP).map(tone));
        }
        let band = a.bins.iter().position(|&(lo, hi)| (lo..hi).contains(&(1000 * N / RATE))).unwrap();
        assert!(f.levels[band] > 0.4, "1 kHz band level {}", f.levels[band]);
        assert!(f.levels[0] < 0.3 && f.levels[BANDS - 1] < 0.3, "{:?}", f.levels);

        let before = f.levels[band];
        // Once the window holds only silence, the level eases down.
        for _ in 0..N / HOP + 2 {
            f = a.push(std::iter::repeat_n(0.0, HOP));
        }
        assert!(f.levels[band] < before, "falls on silence");
    }

    #[test]
    fn peaks_rest_then_fall_with_gravity() {
        let mut p = Peaks::default();
        let mut f = Frame::default();
        f.levels[3] = 0.8;
        p.step(&f, 0.0);
        f.levels[3] = 0.1;
        p.step(&f, 0.2);
        assert_eq!(p.level[3], 0.8, "rests");
        p.step(&f, 0.2);
        let mut before = p.level[3];
        let mut drops = Vec::new();
        for _ in 0..10 {
            p.step(&f, 0.05);
            drops.push(before - p.level[3]);
            before = p.level[3];
        }
        assert!(drops[3] > drops[1], "falls faster and faster: {drops:?}");
        for _ in 0..40 {
            p.step(&f, 0.05);
        }
        assert_eq!(p.level[3], 0.1, "never under the bar");
    }

    #[test]
    fn bands_cover_increasing_bins() {
        let a = Analyzer::new();
        assert!(a.bins.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(a.bins.iter().all(|&(lo, hi)| lo < hi && hi <= N / 2));
    }
}
