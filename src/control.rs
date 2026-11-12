//! Fast player control. When spotlite's own player engine (spotifyd, or the built-in player)
//! is the active device, play/pause, skip, seek and volume go straight to it (see `engine`):
//! about 8 ms instead of 250+ ms for a Web API round trip. Anything else, or any failure, goes
//! to the API thread.

use crate::worker::{Cmd, Kind, Msg};
use crate::engine::{self, Closer};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ctl {
    Play,
    Pause,
    Next,
    Prev,
    Seek(u64),
    Volume(u8),
}

impl Ctl {
    /// playerctl arguments for this command.
    pub fn args(self) -> Vec<String> {
        match self {
            Ctl::Play => vec!["play".into()],
            Ctl::Pause => vec!["pause".into()],
            Ctl::Next => vec!["next".into()],
            Ctl::Prev => vec!["previous".into()],
            Ctl::Seek(ms) => vec!["position".into(), format!("{:.3}", ms as f64 / 1000.0)],
            Ctl::Volume(pct) => vec!["volume".into(), format!("{:.2}", pct.min(100) as f64 / 100.0)],
        }
    }

    /// The same command for the Web API thread.
    pub fn fallback(self) -> Cmd {
        match self {
            Ctl::Play | Ctl::Pause => Cmd::Toggle,
            Ctl::Next => Cmd::Next,
            Ctl::Prev => Cmd::Prev,
            Ctl::Seek(ms) => Cmd::Seek(ms),
            Ctl::Volume(pct) => Cmd::Volume(pct),
        }
    }

    fn kind(self) -> Kind {
        match self {
            Ctl::Volume(_) => Kind::Volume,
            _ => Kind::Player,
        }
    }
}

/// True if fast commands can be sent (see `engine::control_available`).
pub fn available() -> bool {
    engine::control_available()
}

/// Starts the control thread. Each command ends with one `Msg::Done`, from here or (after a
/// fallback) from the API thread.
pub fn spawn(msg_tx: Sender<Msg>, api_tx: Sender<Cmd>) -> Sender<Ctl> {
    let (tx, rx) = channel::<Ctl>();
    thread::Builder::new()
        .name("spotlite-ctl".into())
        .spawn(move || {
            for ctl in rx {
                if engine::run(ctl) {
                    if msg_tx.send(Msg::Done(ctl.kind())).is_err() {
                        return;
                    }
                } else if api_tx.send(ctl.fallback()).is_err() {
                    return;
                }
            }
        })
        .expect("cannot start the control thread");
    tx
}

/// One line of `playerctl --follow` (the built-in player writes the same): playing or not, and the song. None for a line without a
/// player state (spotifyd stopped, or nothing loaded).
pub fn parse_follow(line: &str) -> Option<(bool, Option<crate::api::Track>)> {
    use crate::api::{Album, ImageRef, Named, Track};
    let f: Vec<&str> = line.trim_end_matches(['\n', '\r']).split('\t').collect();
    let playing = match f.first()?.trim() {
        "Playing" => true,
        "Paused" => false,
        _ => return None,
    };
    let field = |i: usize| f.get(i).map(|s| s.trim()).unwrap_or("");
    // "/spotify/track/ID" (or episode) → "spotify:track:ID".
    let id = field(1).trim_matches('\'');
    let mut parts = id.trim_start_matches('/').split('/');
    let track = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("spotify"), Some(kind), Some(id), None) if !kind.is_empty() && !id.is_empty() => {
            let name = field(3);
            let artists = field(4).split(", ").filter(|a| !a.is_empty()).map(|a| Named { name: a.to_owned(), uri: None }).collect();
            let art = field(6);
            Some(Track {
                uri: format!("spotify:{kind}:{id}"),
                name: name.to_owned(),
                duration_ms: field(2).parse::<u64>().map_or(0, |us| us / 1000),
                artists,
                album: Some(Album {
                    name: field(5).to_owned(),
                    // Spotify's covers over MPRIS are the 640 px ones.
                    images: if art.is_empty() { Vec::new() } else { vec![ImageRef { url: art.to_owned(), width: Some(640) }] },
                    ..Default::default()
                }),
                show: None,
                images: Vec::new(),
            })
        }
        _ => None,
    };
    Some((playing, track))
}

/// The engine's player events (see `engine::follow`).
pub struct Follow {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Closer>>>,
}

impl Follow {
    /// Ends the child process and its thread.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let child = self.child.lock().map(|mut c| c.take()).unwrap_or(None);
        if let Some(mut c) = child {
            c.close();
        }
    }
}

/// Follows the engine: each song change and each play or pause comes to the UI at once as
/// `Msg::Mpris` (no Web API request). If the engine is not there yet, or restarts, or its
/// stream ends, it is followed again 2 s later.
pub fn follow(tx: Sender<Msg>) -> Follow {
    let stop = Arc::new(AtomicBool::new(false));
    let child: Arc<Mutex<Option<Closer>>> = Arc::new(Mutex::new(None));
    let (stop2, child2) = (stop.clone(), child.clone());
    let _ = thread::Builder::new().name("spotlite-follow".into()).spawn(move || {
        let close = |slot: &Arc<Mutex<Option<Closer>>>| {
            if let Some(mut c) = slot.lock().ok().and_then(|mut s| s.take()) {
                c.close();
            }
        };
        while !stop2.load(Ordering::Relaxed) {
            if let Some(stream) = engine::follow() {
                if let Ok(mut slot) = child2.lock() {
                    *slot = Some(stream.closer);
                }
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                for line in BufReader::new(stream.reader).lines() {
                    let Ok(line) = line else { break };
                    if let Some((playing, track)) = parse_follow(&line) {
                        if tx.send(Msg::Mpris { playing, track: track.map(Box::new) }).is_err() {
                            return;
                        }
                    }
                }
                close(&child2);
            }
            thread::sleep(std::time::Duration::from_secs(2));
        }
        // Stopped: end a stream that started meanwhile.
        close(&child2);
    });
    Follow { stop, child }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_lines_become_songs() {
        let line = "Playing\t/spotify/track/2K7xn816oNHJZ0aVqdQsha\t206280000\tSoftcore\tThe Neighbourhood\tHard To Imagine\thttps://i.scdn.co/image/ab67";
        let (playing, track) = parse_follow(line).unwrap();
        let t = track.unwrap();
        assert!(playing);
        assert_eq!(t.uri, "spotify:track:2K7xn816oNHJZ0aVqdQsha");
        assert_eq!((t.name.as_str(), t.duration_ms), ("Softcore", 206_280));
        assert_eq!(t.artist_names(), "The Neighbourhood");
        assert_eq!(t.album_name(), "Hard To Imagine");
        assert_eq!(t.images().len(), 1);
        let (playing, track) = parse_follow("Paused\t/spotify/episode/abc\t0\tEp\t\t\t").unwrap();
        assert!(!playing);
        assert_eq!(track.unwrap().uri, "spotify:episode:abc");
        assert_eq!(parse_follow("Paused\t\t\t\t\t\t").unwrap().1, None, "nothing loaded");
        assert!(parse_follow("Stopped\t\t\t\t\t\t").is_none());
        assert!(parse_follow("").is_none());
    }

    #[test]
    fn playerctl_arguments() {
        assert_eq!(Ctl::Seek(65_500).args(), ["position", "65.500"]);
        assert_eq!(Ctl::Volume(42).args(), ["volume", "0.42"]);
        assert_eq!(Ctl::Volume(150).args(), ["volume", "1.00"]);
        assert_eq!(Ctl::Prev.args(), ["previous"]);
        assert!(matches!(Ctl::Seek(5).fallback(), Cmd::Seek(5)));
    }
}
