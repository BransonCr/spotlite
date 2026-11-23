//! The built-in player's local protocol, shared by `spotlite daemon` and the UI. Text lines
//! over TCP on 127.0.0.1. Each connection first sends the token (from the daemon's token
//! file), then one request:
//!
//! - a command (`play`, `pause`, `next`, `prev`, `seek <ms>`, `volume <0-100>`), a load (see
//!   `Load`), `preload <uri>`, or `age`, or `quit`: one answer line, `ok` (`ok <seconds>` for
//!   age) or `err <text>`;
//! - `state`: one answer line, `ok` and the player's state (see `State`);
//! - `follow`: one line per player change, in playerctl's `--follow` format (see
//!   `control::parse_follow`), until the connection closes;
//! - `pcm`: the sound, 22 050 Hz mono s16le, until the connection closes.
//!
//! Kept free of librespot, so its tests run in every build.

// Only the built-in player and its UI side use it; the tests use all of it.
#![cfg_attr(not(builtin_player), allow(dead_code))]

use crate::control::Ctl;

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Ctl(Ctl),
    /// Plays a context or a list of songs at once, with no Web API request.
    Load(Load),
    /// Loads a song in the background, so that it starts at once when it plays next.
    Preload(String),
    Age,
    /// What the player plays now, with its position (see `State`).
    State,
    Quit,
    Follow,
    Pcm,
}

/// What a `Load` plays. Spotify URIs have no spaces, so the line is the words
/// `load <shuffle 0|1> <position_ms> <offset> <context or -> <from or -> <song>...`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Load {
    /// The context (an album, a playlist, Liked Songs); None: the songs below are the list.
    pub context: Option<String>,
    /// The song of the context to start with (None: its first song).
    pub from: Option<String>,
    /// The songs to play when there is no context.
    pub tracks: Vec<String>,
    /// The song of `tracks` to start with.
    pub offset: usize,
    pub position_ms: u64,
    /// Spotify's own shuffle, on or off.
    pub shuffle: bool,
}

/// The request line for a command.
pub fn line(req: Request) -> String {
    match req {
        Request::Load(l) => {
            let mut words = vec![
                "load".to_owned(),
                u8::from(l.shuffle).to_string(),
                l.position_ms.to_string(),
                l.offset.to_string(),
                l.context.unwrap_or_else(|| "-".into()),
                l.from.unwrap_or_else(|| "-".into()),
            ];
            words.extend(l.tracks);
            words.join(" ")
        }
        Request::Preload(uri) => format!("preload {uri}"),
        Request::Ctl(Ctl::Play) => "play".into(),
        Request::Ctl(Ctl::Pause) => "pause".into(),
        Request::Ctl(Ctl::Next) => "next".into(),
        Request::Ctl(Ctl::Prev) => "prev".into(),
        Request::Ctl(Ctl::Seek(ms)) => format!("seek {ms}"),
        Request::Ctl(Ctl::Volume(pct)) => format!("volume {}", pct.min(100)),
        Request::Age => "age".into(),
        Request::State => "state".into(),
        Request::Quit => "quit".into(),
        Request::Follow => "follow".into(),
        Request::Pcm => "pcm".into(),
    }
}

/// Reads a request line. None for anything else.
pub fn parse(line: &str) -> Option<Request> {
    let mut words = line.split_whitespace();
    if line.starts_with("load ") {
        words.next();
        let shuffle = match words.next()? {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        let position_ms = words.next()?.parse().ok()?;
        let offset = words.next()?.parse().ok()?;
        let uri = |w: &str| (w != "-").then(|| w.to_owned());
        let context = uri(words.next()?);
        let from = uri(words.next()?);
        let tracks: Vec<String> = words.map(str::to_owned).collect();
        if context.is_none() && tracks.is_empty() {
            return None;
        }
        return Some(Request::Load(Load { context, from, tracks, offset, position_ms, shuffle }));
    }
    let req = match (words.next()?, words.next()) {
        ("preload", Some(uri)) => Request::Preload(uri.to_owned()),
        ("play", None) => Request::Ctl(Ctl::Play),
        ("pause", None) => Request::Ctl(Ctl::Pause),
        ("next", None) => Request::Ctl(Ctl::Next),
        ("prev", None) => Request::Ctl(Ctl::Prev),
        ("seek", Some(ms)) => Request::Ctl(Ctl::Seek(ms.parse().ok()?)),
        ("volume", Some(pct)) => Request::Ctl(Ctl::Volume(pct.parse::<u8>().ok()?.min(100))),
        ("age", None) => Request::Age,
        ("state", None) => Request::State,
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

/// The answer to `state`: `ok <active 0|1> <position_ms> <shuffle 0|1> <repeat> <volume>`,
/// a tab, then the `follow` line. `active`: a song is loaded and the device was not left for
/// another one, so this is Spotify's player state too.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub active: bool,
    pub position_ms: u64,
    pub shuffle: bool,
    /// "off", "context" or "track".
    pub repeat: String,
    /// Percent.
    pub volume: u8,
    pub follow: String,
}

pub fn state_line(st: &State) -> String {
    format!(
        "ok {} {} {} {} {}\t{}",
        u8::from(st.active),
        st.position_ms,
        u8::from(st.shuffle),
        st.repeat,
        st.volume.min(100),
        st.follow
    )
}

pub fn parse_state(line: &str) -> Option<State> {
    let (head, follow) = line.trim_end_matches(['\n', '\r']).split_once('\t')?;
    let mut w = head.split(' ');
    if w.next()? != "ok" {
        return None;
    }
    let flag = |w: Option<&str>| match w? {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    };
    let active = flag(w.next())?;
    let position_ms = w.next()?.parse().ok()?;
    let shuffle = flag(w.next())?;
    let repeat = w.next().filter(|r| matches!(*r, "off" | "context" | "track"))?.to_owned();
    let volume = w.next()?.parse::<u8>().ok()?.min(100);
    Some(State { active, position_ms, shuffle, repeat, volume, follow: follow.to_owned() })
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
            Request::State,
            Request::Follow,
            Request::Pcm,
            Request::Preload("spotify:track:2K7xn816oNHJZ0aVqdQsha".into()),
            Request::Load(Load {
                context: Some("spotify:album:4LH4d3cOWNNsVw41Gqt2kv".into()),
                from: Some("spotify:track:2K7xn816oNHJZ0aVqdQsha".into()),
                position_ms: 61_000,
                ..Default::default()
            }),
            Request::Load(Load {
                tracks: vec!["spotify:track:a".into(), "spotify:track:b".into()],
                offset: 1,
                shuffle: true,
                ..Default::default()
            }),
        ] {
            assert_eq!(parse(&line(req.clone())), Some(req.clone()), "{}", line(req));
        }
        assert_eq!(parse("load 0 0 0 - -"), None, "nothing to play");
        let st = State {
            active: true,
            position_ms: 61_000,
            shuffle: false,
            repeat: "context".into(),
            volume: 60,
            follow: follow_line(true, None),
        };
        assert_eq!(parse_state(&state_line(&st)), Some(st));
        assert_eq!(parse_state("ok 1 0 0 sometimes 60\tPlaying"), None);
        assert_eq!(parse_state("err not connected"), None);
        assert_eq!(parse("load 2 0 0 - - spotify:track:a"), None);
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
