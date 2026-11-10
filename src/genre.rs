//! Randomize: 50 songs picked at random inside one genre, not by your taste.
//!
//! You pick the genre (genres.rs: quick picks, a search, a folder tree). The songs come from
//! Spotify's search with a `genre:"…"` filter: it reaches up to 1000 songs deep. spotlite finds how deep the genre goes, reads pages at random
//! places in it, takes a few songs from each at random, and shuffles them.

use crate::api::{Page, Spotify, Track, SEARCH_PAGE};
use anyhow::{anyhow, Result};
use rand::seq::SliceRandom;
use std::collections::{HashMap, HashSet};

/// Songs in one randomize.
pub const WANT: usize = 50;
/// At most this many songs of one artist.
const PER_ARTIST: usize = 2;
/// At most this many search requests for one randomize.
const MAX_REQUESTS: usize = 40;
/// Spotify's search does not go past this offset.
const DEEPEST: usize = 990;

/// Search pages, so the pick can be tested without Spotify.
pub trait Search {
    fn search_page(&mut self, query: &str, offset: usize) -> Result<Page<Track>>;
}

impl Search for Spotify {
    fn search_page(&mut self, query: &str, offset: usize) -> Result<Page<Track>> {
        self.search_tracks(query, offset)
    }
}

/// `WANT` songs at random from the genre (fewer if Spotify has fewer), in random order.
///
/// First the songs Spotify tags with the genre (`genre:"…"`). Spotify tags few songs with many
/// sub-genres (synthwave, hardstyle, dream pop: under 50), so the rest come from a text search
/// for the genre's name, which reaches 1000 songs deep.
pub fn random(api: &mut impl Search, genre: &str) -> Result<Vec<Track>> {
    let name = genre.replace('"', "");
    let mut pick = Pick::default();
    let mut rng = rand::thread_rng();
    let tagged = sample(api, &format!("genre:\"{name}\""), &mut pick, &mut rng);
    if pick.picked.len() < WANT {
        let named = sample(api, &name, &mut pick, &mut rng);
        if pick.picked.is_empty() {
            tagged.and(named)?;
        }
    } else {
        tagged?;
    }
    let mut picked = pick.picked;
    picked.truncate(WANT);
    picked.shuffle(&mut rng);
    if picked.is_empty() {
        return Err(anyhow!("Spotify has no songs in the genre “{genre}”."));
    }
    Ok(picked)
}

/// The songs picked so far, and what is spent.
#[derive(Default)]
struct Pick {
    picked: Vec<Track>,
    seen: HashSet<String>,
    by_artist: HashMap<String, usize>,
    requests: usize,
}

/// Adds songs at random from one search to `pick`, until it has `WANT` or the requests run out.
fn sample(api: &mut impl Search, query: &str, pick: &mut Pick, rng: &mut impl rand::Rng) -> Result<()> {
    // How deep the search goes: `total` is not to be trusted, so look for the last full page.
    let mut pages: HashMap<usize, Vec<Track>> = HashMap::new();
    // The deepest page with songs (`lo`) and the first one past the end (`hi`).
    let mut lo = None;
    let mut hi = DEEPEST + SEARCH_PAGE;
    for probe in [DEEPEST, 490, 240, 90, 40, 0] {
        let page = api.search_page(query, probe)?;
        pick.requests += 1;
        if page.items.is_empty() {
            hi = probe;
        } else {
            lo = Some((probe, page.raw));
            pages.insert(probe, page.items);
            break;
        }
    }
    let Some((mut lo, mut lo_raw)) = lo else { return Ok(()) };
    // Halve the gap between them.
    while hi - lo > SEARCH_PAGE && pick.requests < MAX_REQUESTS {
        let mid = (lo + hi) / 2 / SEARCH_PAGE * SEARCH_PAGE;
        let page = api.search_page(query, mid)?;
        pick.requests += 1;
        if page.items.is_empty() {
            hi = mid;
        } else {
            lo = mid;
            lo_raw = page.raw;
            pages.insert(mid, page.items);
        }
    }
    let depth = lo + lo_raw;
    let starts: Vec<usize> = (0..depth).step_by(SEARCH_PAGE).collect();
    let mut order = starts.clone();
    order.shuffle(rng);
    // A small pool: read all of it. A big one: a few songs from each of many random pages,
    // so the songs come from all through it, not from a few places.
    let all = starts.len() <= MAX_REQUESTS.saturating_sub(pick.requests);
    let mut pool: Vec<Track> = Vec::new();
    for start in order {
        if !all && pick.picked.len() >= WANT {
            break;
        }
        let mut items = match pages.remove(&start) {
            Some(items) => items,
            None if pick.requests < MAX_REQUESTS => {
                pick.requests += 1;
                match api.search_page(query, start) {
                    Ok(page) => page.items,
                    Err(_) => break,
                }
            }
            None => break,
        };
        items.shuffle(rng);
        if all {
            pool.extend(items);
            continue;
        }
        let mut took = 0;
        for t in items {
            if took >= 3 {
                break;
            }
            if take(&t, &mut pick.seen, &mut pick.by_artist) {
                pick.picked.push(t);
                took += 1;
            }
        }
    }
    pool.shuffle(rng);
    for t in pool {
        if pick.picked.len() >= WANT {
            break;
        }
        if take(&t, &mut pick.seen, &mut pick.by_artist) {
            pick.picked.push(t);
        }
    }
    Ok(())
}

/// True if `t` may join: a song not taken yet, by an artist not taken too often.
fn take(t: &Track, seen: &mut HashSet<String>, by_artist: &mut HashMap<String, usize>) -> bool {
    let artist = t.artists.first().map(|a| a.uri.clone().unwrap_or_else(|| a.name.clone())).unwrap_or_default();
    // The same song can be on an album and a single: one name per artist.
    let key = format!("{}\n{}", artist, t.name.to_lowercase());
    if !seen.insert(t.uri.clone()) || !seen.insert(key) {
        return false;
    }
    let n = by_artist.entry(artist).or_default();
    if *n >= PER_ARTIST {
        return false;
    }
    *n += 1;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tagged` songs tagged with the genre and `named` more found by its name; 3 songs per artist.
    struct Fake {
        tagged: usize,
        named: usize,
        asked: Vec<(bool, usize)>,
    }

    impl Search for Fake {
        fn search_page(&mut self, query: &str, offset: usize) -> Result<Page<Track>> {
            let by_tag = query == "genre:\"shoegaze\"";
            assert!(by_tag || query == "shoegaze", "{query}");
            self.asked.push((by_tag, offset));
            let (size, base) = if by_tag { (self.tagged, 0) } else { (self.named, 100_000) };
            let items: Vec<Track> = (offset..size.min(offset + SEARCH_PAGE))
                .map(|i| {
                    let i = i + base;
                    serde_json::from_value(serde_json::json!({
                        "uri": format!("spotify:track:{i}"), "name": format!("song {i}"),
                        "artists": [{ "name": "a", "uri": format!("spotify:artist:{}", i / 3) }],
                        "album": null, "show": null
                    }))
                    .unwrap()
                })
                .collect();
            Ok(Page { raw: items.len(), items, total: 1000 })
        }
    }

    fn check(tagged: usize, named: usize) -> (Vec<Track>, Vec<(bool, usize)>) {
        let mut api = Fake { tagged, named, asked: Vec::new() };
        let songs = random(&mut api, "shoegaze").unwrap();
        let uris: HashSet<_> = songs.iter().map(|t| t.uri.clone()).collect();
        assert_eq!(uris.len(), songs.len(), "no song twice");
        let mut per: HashMap<String, usize> = HashMap::new();
        for t in &songs {
            *per.entry(t.artists[0].uri.clone().unwrap()).or_default() += 1;
        }
        assert!(per.values().all(|&n| n <= PER_ARTIST), "{per:?}");
        assert!(api.asked.len() <= MAX_REQUESTS);
        (songs, api.asked)
    }

    fn number(t: &Track) -> usize {
        t.uri.trim_start_matches("spotify:track:").parse().unwrap()
    }

    #[test]
    fn a_deep_genre_gives_50_tagged_songs_from_all_through_it() {
        let (songs, asked) = check(1000, 1000);
        assert_eq!(songs.len(), WANT);
        assert!(songs.iter().all(|t| number(t) < 100_000), "all tagged with the genre");
        let deep = songs.iter().filter(|t| number(t) >= 500).count();
        assert!(deep > 5, "songs from the deep half too: {deep}");
        assert!(asked.iter().all(|(tag, _)| *tag), "no text search needed");
    }

    #[test]
    fn a_small_genre_is_filled_up_by_name() {
        let (songs, asked) = check(36, 1000);
        assert_eq!(songs.len(), WANT);
        // 36 tagged songs by 12 artists, 2 each: all 24 are in, the rest found by name.
        assert_eq!(songs.iter().filter(|t| number(t) < 100_000).count(), 24);
        assert!(asked.contains(&(true, 0)) && asked.contains(&(true, 30)));
        assert!(asked.iter().any(|(tag, _)| !tag));
    }

    #[test]
    fn a_genre_spotify_does_not_know_is_an_error() {
        let mut api = Fake { tagged: 0, named: 0, asked: Vec::new() };
        assert!(random(&mut api, "shoegaze").is_err());
        let (songs, _) = check(0, 40);
        assert!(!songs.is_empty(), "by name only");
    }
}
