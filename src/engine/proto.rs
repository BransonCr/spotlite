//! The built-in player's local protocol, shared by `spotlite daemon` and the UI. Text lines
//! over TCP on 127.0.0.1. Each connection first sends the token (from the daemon's token
//! file), then one request:
//!
//! - a command (`play`, `pause`, `next`, `prev`, `seek <ms>`, `volume <0-100>`), or `age`, or
//!   `quit`: one answer line, `ok` (`ok <seconds>` for age) or `err <text>`;
//! - `follow`: one line per player change, in playerctl's `--follow` format (see
//!   `control::parse_follow`), until the connection closes;
//! - `pcm`: the sound, 22 050 Hz mono s16le, until the connection closes.
//!
//! Kept free of librespot, so its tests run in every build.

// Only the built-in player and its UI side use it; the tests use all of it.
#![cfg_attr(not(builtin_player), allow(dead_code))]

use crate::control::Ctl;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Request {
    Ctl(Ctl),
    Age,
    Quit,
    Follow,
    Pcm,
}

/// The request line for a command.
pub fn line(req: Request) -> String {
    match req {
        Request::Ctl(Ctl::Play) => "play".into(),
        Request::Ctl(Ctl::Pause) => "pause".into(),
        Request::Ctl(Ctl::Next) => "next".into(),
        Request::Ctl(Ctl::Prev) => "prev".into(),
        Request::Ctl(Ctl::Seek(ms)) => format!("seek {ms}"),
        Request::Ctl(Ctl::Volume(pct)) => format!("volume {}", pct.min(100)),
        Request::Age => "age".into(),
        Request::Quit => "quit".into(),
        Request::Follow => "follow".into(),
        Request::Pcm => "pcm".into(),
    }
}

/// Reads a request line. None for anything else.
pub fn parse(line: &str) -> Option<Request> {
    let mut words = line.split_whitespace();
    let req = match (words.next()?, words.next()) {
        ("play", None) => Request::Ctl(Ctl::Play),
        ("pause", None) => Request::Ctl(Ctl::Pause),
        ("next", None) => Request::Ctl(Ctl::Next),
        ("prev", None) => Request::Ctl(Ctl::Prev),
        ("seek", Some(ms)) => Request::Ctl(Ctl::Seek(ms.parse().ok()?)),
        ("volume", Some(pct)) => Request::Ctl(Ctl::Volume(pct.parse::<u8>().ok()?.min(100))),
        ("age", None) => Request::Age,
        ("quit", None) => Request::Quit,
        ("follow", None) => Request::Follow,
        ("pcm", None) => Request::Pcm,
        _ => return None,
    };
    words.next().is_none().then_some(req)
}

/// A song for a `follow` line.
pub struct Song<'a> {
    /// "spotify:track:ID" or "spotify:episode:ID".
    pub uri: &'a str,
    pub duration_ms: u32,
    pub name: &'a str,
    pub artists: &'a [String],
    pub album: &'a str,
    pub cover: &'a str,
}

/// One `follow` line: what playerctl writes for spotifyd, so `control::parse_follow` reads
/// both. Without a song: a line that says nothing is loaded.
pub fn follow_line(playing: bool, song: Option<&Song>) -> String {
    let status = if playing { "Playing" } else { "Paused" };
    let Some(s) = song else {
        return format!("{status}\t\t\t\t\t\t");
    };
    // Tabs and line ends would break the line apart.
    let clean = |t: &str| t.replace(['\t', '\n', '\r'], " ");
    format!(
        "{status}\t/{}\t{}\t{}\t{}\t{}\t{}",
        s.uri.replace(':', "/"),
        u64::from(s.duration_ms) * 1000,
        clean(s.name),
        clean(&s.artists.join(", ")),
        clean(s.album),
        clean(s.cover),
    )
}

/// Turns the player's sound (44 100 Hz stereo, interleaved, -1..=1) into the `pcm` stream's
/// (22 050 Hz mono s16le): each pair of stereo frames becomes one sample. Keeps a part pair
/// for the next packet.
#[derive(Default)]
pub struct Downmix {
    sum: f64,
    count: u8,
}

impl Downmix {
    pub fn push(&mut self, samples: &[f64], out: &mut Vec<u8>) {
        for v in samples {
            self.sum += v;
            self.count += 1;
            if self.count == 4 {
                let mono = (self.sum / 4.0).clamp(-1.0, 1.0);
                out.extend_from_slice(&((mono * 32767.0) as i16).to_le_bytes());
                self.sum = 0.0;
                self.count = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::parse_follow;

    #[test]
    fn requests_go_both_ways() {
        for req in [
            Request::Ctl(Ctl::Play),
            Request::Ctl(Ctl::Pause),
            Request::Ctl(Ctl::Next),
            Request::Ctl(Ctl::Prev),
            Request::Ctl(Ctl::Seek(65_500)),
            Request::Ctl(Ctl::Volume(42)),
            Request::Age,
            Request::Quit,
            Request::Follow,
            Request::Pcm,
        ] {
            assert_eq!(parse(&line(req)), Some(req), "{}", line(req));
        }
        assert_eq!(line(Request::Ctl(Ctl::Volume(150))), "volume 100");
        assert_eq!(parse("volume 300"), None);
        assert_eq!(parse("seek"), None);
        assert_eq!(parse("play now"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn follow_lines_read_like_playerctl() {
        let artists = ["The Neighbourhood".to_owned(), "Other\tOne".to_owned()];
        let song = Song {
            uri: "spotify:track:2K7xn816oNHJZ0aVqdQsha",
            duration_ms: 206_280,
            name: "Softcore",
            artists: &artists,
            album: "Hard To Imagine",
            cover: "https://i.scdn.co/image/ab67",
        };
        let (playing, track) = parse_follow(&follow_line(true, Some(&song))).unwrap();
        let t = track.unwrap();
        assert!(playing);
        assert_eq!(t.uri, "spotify:track:2K7xn816oNHJZ0aVqdQsha");
        assert_eq!((t.name.as_str(), t.duration_ms), ("Softcore", 206_280));
        assert_eq!(t.artist_names(), "The Neighbourhood, Other One");
        assert_eq!(t.album_name(), "Hard To Imagine");
        assert_eq!(t.images().len(), 1);
        let (playing, track) = parse_follow(&follow_line(false, None)).unwrap();
        assert!(!playing);
        assert!(track.is_none());
    }

    #[test]
    fn downmix_halves_the_rate_and_joins_the_channels() {
        let mut d = Downmix::default();
        let mut out = Vec::new();
        // Two stereo frames → one sample; the third frame waits for its pair.
        d.push(&[1.0, 1.0, 0.0, 0.0, -1.0, -1.0], &mut out);
        assert_eq!(out, 16383i16.to_le_bytes());
        d.push(&[-1.0, -1.0], &mut out);
        assert_eq!(&out[2..], (-32767i16).to_le_bytes());
    }
}
