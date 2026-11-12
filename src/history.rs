//! What spotlite played, kept on disk. spotifyd (librespot) does not tell Spotify what it
//! plays, so Spotify's Recently played never shows those songs; this list fills them in. The
//! Recent tab shows both lists, newest first.

use crate::api::Track;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The list keeps this many songs.
pub const KEEP: usize = 200;
/// A song counts as played after this long (as on Spotify), or half of it if it is shorter.
pub const LISTEN_MS: u64 = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Played {
    pub track: Track,
    /// When it counted as played (unix ms).
    pub at: u64,
}

pub fn path() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotlite/history.json")
}

/// The songs played, newest first; empty when there is no list (and in tests).
pub fn load() -> Vec<Played> {
    if cfg!(test) {
        return Vec::new();
    }
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Adds a song at the top of the list on disk.
pub fn add(track: &Track, at: u64) {
    if cfg!(test) {
        return;
    }
    let mut list = load();
    list.insert(0, Played { track: track.clone(), at });
    list.truncate(KEEP);
    let path = path();
    let (Some(dir), Ok(json)) = (path.parent(), serde_json::to_vec(&list)) else { return };
    let _ = std::fs::create_dir_all(dir);
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Spotify's "played_at" ("2026-11-11T23:14:16.629Z") in unix ms.
pub fn parse_time(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let ms = match s.get(19..20) {
        Some(".") => {
            let frac: String = s[20..].chars().take_while(|c| c.is_ascii_digit()).collect();
            format!("{frac:0<3}").get(..3)?.parse::<i64>().ok()?
        }
        _ => 0,
    };
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(((days * 24 + hh) * 60 + mm) * 60 * 1000 + ss * 1000 + ms).ok()
}

/// Spotify's list and the local one as one list, newest first, at most `max` songs. A play in
/// both lists (the same song, closer in time than its length) shows once.
pub fn merge(spotify: Vec<Played>, local: Vec<Played>, max: usize) -> Vec<Track> {
    let mut all: Vec<Played> = spotify.into_iter().chain(local).collect();
    all.sort_by_key(|p| std::cmp::Reverse(p.at));
    let mut out: Vec<Played> = Vec::new();
    for p in all {
        let window = p.track.duration_ms.max(60_000);
        let twin = out.iter().rev().take_while(|q| q.at - p.at < window).any(|q| q.track.uri == p.track.uri);
        if !twin {
            out.push(p);
        }
        if out.len() >= max {
            break;
        }
    }
    out.into_iter().map(|p| p.track).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(uri: &str) -> Track {
        serde_json::from_value(serde_json::json!({"uri": uri, "name": uri, "duration_ms": 200_000, "artists": [], "album": null})).unwrap()
    }

    #[test]
    fn spotify_times_read() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-11T23:14:16.629Z"), Some(1_794_438_856_629));
        assert_eq!(parse_time("2024-02-29T12:00:00.5Z"), Some(1_709_208_000_500));
        assert_eq!(parse_time("nonsense"), None);
    }

    #[test]
    fn both_lists_show_newest_first_and_a_play_shows_once() {
        let min = 60_000;
        let spotify = vec![Played { track: song("a"), at: 10 * min }, Played { track: song("b"), at: 2 * min }];
        // "c" played on spotifyd after both; "a" is in both lists (Spotify's time is a bit later).
        let local = vec![Played { track: song("c"), at: 20 * min }, Played { track: song("a"), at: 9 * min }];
        let uris: Vec<String> = merge(spotify, local, 50).into_iter().map(|t| t.uri).collect();
        assert_eq!(uris, ["c", "a", "b"]);
        // The same song again a while later is a new play.
        let again = vec![Played { track: song("a"), at: 30 * min }, Played { track: song("a"), at: 10 * min }];
        assert_eq!(merge(again, Vec::new(), 50).len(), 2);
        let many: Vec<Played> = (0..10).map(|i| Played { track: song(&i.to_string()), at: i * 10 * min }).collect();
        assert_eq!(merge(many, Vec::new(), 4).len(), 4);
    }
}
