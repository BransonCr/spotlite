//! The sound for the visualizer. `TapSink` sits in front of the real audio output: it passes
//! every packet on, and copies a 22 050 Hz mono version to `Tap`. The output plays about half
//! a second behind what it is given, so each copy waits for the time it is heard before it
//! goes to the readers (the visualizer then moves with the music, as with parec on Linux).

use crate::engine::proto::Downmix;
use librespot_playback::audio_backend::{Sink, SinkResult};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::{NUM_CHANNELS, SAMPLE_RATE};
use std::collections::VecDeque;
use std::io::Write;
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// At most this much sound waits (more is dropped: nobody reads it in time).
const MAX_WAITING: usize = 64;

#[derive(Default)]
struct Inner {
    /// Sound and the time it is heard.
    waiting: VecDeque<(Instant, Vec<u8>)>,
    readers: Vec<TcpStream>,
}

/// Copies of the sound, sent to every reader of the `pcm` stream at the time they are heard.
#[derive(Default)]
pub struct Tap {
    inner: Mutex<Inner>,
    ready: Condvar,
}

impl Tap {
    /// The tap and its sending thread.
    pub fn start() -> Arc<Tap> {
        let tap = Arc::new(Tap::default());
        let sender = tap.clone();
        let _ = std::thread::Builder::new().name("spotlite-tap".into()).spawn(move || sender.send_loop());
        tap
    }

    pub fn add_reader(&self, s: TcpStream) {
        let _ = s.set_nodelay(true);
        let _ = s.set_write_timeout(Some(Duration::from_millis(200)));
        if let Ok(mut i) = self.inner.lock() {
            i.readers.push(s);
        }
    }

    fn push(&self, at: Instant, bytes: Vec<u8>) {
        if let Ok(mut i) = self.inner.lock() {
            if i.readers.is_empty() {
                return;
            }
            if i.waiting.len() >= MAX_WAITING {
                i.waiting.pop_front();
            }
            i.waiting.push_back((at, bytes));
            self.ready.notify_one();
        }
    }

    fn send_loop(&self) {
        let Ok(mut i) = self.inner.lock() else { return };
        loop {
            let Some(at) = i.waiting.front().map(|w| w.0) else {
                i = match self.ready.wait(i) {
                    Ok(i) => i,
                    Err(_) => return,
                };
                continue;
            };
            let now = Instant::now();
            if at > now {
                i = match self.ready.wait_timeout(i, at - now) {
                    Ok((i, _)) => i,
                    Err(_) => return,
                };
                continue;
            }
            let Some((_, bytes)) = i.waiting.pop_front() else { continue };
            // A reader that is gone, or too slow, is dropped: playback never waits for one.
            i.readers.retain_mut(|r| r.write_all(&bytes).is_ok());
            if i.readers.is_empty() {
                i.waiting.clear();
            }
        }
    }
}

/// The audio output, with a copy of the sound to `tap`.
pub struct TapSink {
    inner: Box<dyn Sink>,
    tap: Arc<Tap>,
    mix: Downmix,
    /// When the sound given to the output so far ends.
    heard_until: Instant,
}

impl TapSink {
    pub fn new(inner: Box<dyn Sink>, tap: Arc<Tap>) -> Self {
        TapSink { inner, tap, mix: Downmix::default(), heard_until: Instant::now() }
    }
}

impl Sink for TapSink {
    fn start(&mut self) -> SinkResult<()> {
        self.inner.start()
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.inner.stop()
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if let AudioPacket::Samples(samples) = &packet {
            let mut bytes = Vec::with_capacity(samples.len() / 2);
            self.mix.push(samples, &mut bytes);
            let frames = samples.len() / NUM_CHANNELS as usize;
            let length = Duration::from_secs_f64(frames as f64 / f64::from(SAMPLE_RATE));
            let at = self.heard_until.max(Instant::now());
            self.heard_until = at + length;
            if !bytes.is_empty() {
                self.tap.push(at, bytes);
            }
        }
        self.inner.write(packet, converter)
    }
}
