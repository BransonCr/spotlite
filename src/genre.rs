//! Randomize: 50 songs picked at random inside one genre, not by your taste.
//!
//! The Web API no longer gives an artist's genres, so they come from MusicBrainz (free, no key,
//! one request a second). The songs come from Spotify's search with a `genre:"…"` filter: it
//! reaches up to 1000 songs deep. spotlite finds how deep the genre goes, reads pages at random
//! places in it, takes a few songs from each at random, and shuffles them.

use crate::api::{Page, Spotify, Track, SEARCH_PAGE};
use anyhow::{anyhow, Context, Result};
use rand::seq::SliceRandom;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Songs in one randomize.
pub const WANT: usize = 50;
/// At most this many songs of one artist.
const PER_ARTIST: usize = 2;
/// At most this many search requests for one randomize.
const MAX_REQUESTS: usize = 30;
/// Spotify's search does not go past this offset.
const DEEPEST: usize = 990;
/// Genres shown for an artist.
const MAX_GENRES: usize = 8;
const UA: &str = "spotlite/0.1 (https://github.com/BransonCr/spotlite)";
const MB: &str = "https://musicbrainz.org/ws/2";

/// The genres of an artist, most voted first (from a copy on disk when there is one).
pub fn of_artist(artist_uri: &str, name: &str) -> Result<Vec<String>> {
    let mut cache: HashMap<String, Vec<String>> = crate::store::read("genres.json").unwrap_or_default();
    if let Some(g) = cache.get(artist_uri).filter(|g| !g.is_empty()) {
        return Ok(g.clone());
    }
    let genres = musicbrainz(name)?;
    // None now may be some later: MusicBrainz grows.
    if !genres.is_empty() {
        cache.insert(artist_uri.to_owned(), genres.clone());
        crate::store::write("genres.json", &cache);
    }
    Ok(genres)
}

/// The time of the last MusicBrainz request, for all threads: it allows one a second, and
/// answers 503 to more.
static LAST_ASK: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// A MusicBrainz GET, at most one a second; a 503 (too fast, or busy) waits and tries again.
fn ask(agent: &ureq::Agent, path: &str, query: &[(&str, &str)]) -> Result<Value> {
    let gap = Duration::from_millis(1100);
    for attempt in 0..3 {
        {
            let mut last = LAST_ASK.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(at) = *last {
                std::thread::sleep(gap.saturating_sub(at.elapsed()));
            }
            *last = Some(std::time::Instant::now());
        }
        let mut req = agent.get(&format!("{MB}{path}"));
        for (k, v) in query {
            req = req.query(k, v);
        }
        match req.call() {
            Ok(resp) => return Ok(resp.into_json()?),
            Err(ureq::Error::Status(503, _)) if attempt < 2 => std::thread::sleep(Duration::from_secs(2 << attempt)),
            Err(ureq::Error::Status(503, _)) => return Err(anyhow!("MusicBrainz is busy; try again in a minute")),
            Err(e) => return Err(e).context("asking MusicBrainz"),
        }
    }
    unreachable!()
}

fn musicbrainz(name: &str) -> Result<Vec<String>> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).user_agent(UA).build();
    let query = format!("artist:\"{}\"", name.replace('"', ""));
    let found = ask(&agent, "/artist/", &[("query", &query), ("limit", "5"), ("fmt", "json")])?;
    let artists = found["artists"].as_array().cloned().unwrap_or_default();
    // The same name first (search also finds near names), else the best score.
    let pick = artists
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .or_else(|| artists.first())
        .ok_or_else(|| anyhow!("MusicBrainz does not know {name}"))?;
    let id = pick["id"].as_str().ok_or_else(|| anyhow!("MusicBrainz gave no id for {name}"))?;
    let artist = ask(&agent, &format!("/artist/{id}"), &[("inc", "genres+tags"), ("fmt", "json")])?;
    let mut genres = ranked(&artist["genres"]);
    if genres.is_empty() {
        genres = ranked(&artist["tags"]);
    }
    if genres.is_empty() {
        genres = ranked(&pick["tags"]);
    }
    genres.truncate(MAX_GENRES);
    Ok(genres)
}

/// Names of a MusicBrainz genre or tag list, most votes first; ones nobody voted for go.
fn ranked(list: &Value) -> Vec<String> {
    let mut items: Vec<(i64, String)> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| Some((g["count"].as_i64().unwrap_or(0), g["name"].as_str()?.to_lowercase())))
        .filter(|(n, _)| *n > 0)
        .collect();
    items.sort_by_key(|g| std::cmp::Reverse(g.0));
    let mut seen = HashSet::new();
    items.into_iter().map(|(_, n)| n).filter(|n| seen.insert(n.clone())).collect()
}

/// Search pages, so the pick can be tested without Spotify.
pub trait Search {
    fn search_page(&mut self, query: &str, offset: usize) -> Result<Page<Track>>;
}

impl Search for Spotify {
    fn search_page(&mut self, query: &str, offset: usize) -> Result<Page<Track>> {
        self.search_tracks(query, offset)
    }
}

/// `WANT` songs at random from the genre (fewer if it has fewer), in random order.
pub fn random(api: &mut impl Search, genre: &str) -> Result<Vec<Track>> {
    let query = format!("genre:\"{}\"", genre.replace('"', ""));
    let mut rng = rand::thread_rng();
    let mut requests = 0;
    // How deep the genre goes: `total` is not to be trusted, so look for the last full page.
    let mut pages: HashMap<usize, Vec<Track>> = HashMap::new();
    // The deepest page with songs (`lo`) and the first one past the end (`hi`).
    let mut lo = None;
    let mut hi = DEEPEST + SEARCH_PAGE;
    for probe in [DEEPEST, 490, 240, 90, 40, 0] {
        let page = api.search_page(&query, probe)?;
        requests += 1;
        if page.items.is_empty() {
            hi = probe;
        } else {
            lo = Some((probe, page.raw));
            pages.insert(probe, page.items);
            break;
        }
    }
    let Some((mut lo, mut lo_raw)) = lo else {
        return Err(anyhow!("Spotify has no songs in the genre “{genre}”."));
    };
    // Halve the gap between them.
    while hi - lo > SEARCH_PAGE {
        let mid = (lo + hi) / 2 / SEARCH_PAGE * SEARCH_PAGE;
        let page = api.search_page(&query, mid)?;
        requests += 1;
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
    order.shuffle(&mut rng);
    // A small genre: read all of it. A big one: a few songs from each of many random pages,
    // so the songs come from all through the genre, not from a few places.
    let all = starts.len() <= MAX_REQUESTS - requests;
    let per_page = if all { SEARCH_PAGE } else { 3 };
    let mut picked: Vec<Track> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut by_artist: HashMap<String, usize> = HashMap::new();
    let mut pool: Vec<Track> = Vec::new();
    for start in order {
        if !all && picked.len() >= WANT {
            break;
        }
        let items = match pages.remove(&start) {
            Some(items) => items,
            None if requests < MAX_REQUESTS => {
                requests += 1;
                match api.search_page(&query, start) {
                    Ok(page) => page.items,
                    Err(e) if picked.is_empty() && pool.is_empty() => return Err(e),
                    Err(_) => break,
                }
            }
            None => break,
        };
        let mut items = items;
        items.shuffle(&mut rng);
        if all {
            pool.extend(items);
            continue;
        }
        let mut took = 0;
        for t in items {
            if took >= per_page {
                break;
            }
            if take(&t, &mut seen, &mut by_artist) {
                picked.push(t);
                took += 1;
            }
        }
    }
    if all {
        pool.shuffle(&mut rng);
        for t in pool {
            if picked.len() >= WANT {
                break;
            }
            if take(&t, &mut seen, &mut by_artist) {
                picked.push(t);
            }
        }
    }
    picked.truncate(WANT);
    picked.shuffle(&mut rng);
    if picked.is_empty() {
        return Err(anyhow!("Spotify has no songs in the genre “{genre}”."));
    }
    Ok(picked)
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

    /// A genre of `size` songs; each artist has 3 songs.
    struct Fake {
        size: usize,
        asked: Vec<usize>,
    }

    impl Search for Fake {
        fn search_page(&mut self, query: &str, offset: usize) -> Result<Page<Track>> {
            assert_eq!(query, "genre:\"shoegaze\"");
            self.asked.push(offset);
            let items: Vec<Track> = (offset..self.size.min(offset + SEARCH_PAGE))
                .map(|i| {
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

    fn check(size: usize) -> (Vec<Track>, Vec<usize>) {
        let mut api = Fake { size, asked: Vec::new() };
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

    #[test]
    fn a_deep_genre_gives_50_songs_from_all_through_it() {
        let (songs, asked) = check(1000);
        assert_eq!(songs.len(), WANT);
        let deep = songs.iter().filter(|t| t.uri.trim_start_matches("spotify:track:").parse::<usize>().unwrap() >= 500).count();
        assert!(deep > 10, "songs from the deep half too: {deep}");
        assert!(asked.len() >= 17);
    }

    #[test]
    fn a_small_genre_is_read_whole() {
        let (songs, asked) = check(36);
        // 36 songs by 12 artists, 2 each at most.
        assert_eq!(songs.len(), 24);
        assert!(asked.contains(&0) && asked.contains(&30));
    }

    #[test]
    fn an_empty_genre_is_an_error() {
        let mut api = Fake { size: 0, asked: Vec::new() };
        assert!(random(&mut api, "shoegaze").is_err());
    }

    #[test]
    fn genres_rank_by_votes() {
        let v = serde_json::json!([{ "name": "Dream Pop", "count": 3 }, { "name": "shoegaze", "count": 9 }, { "name": "x", "count": 0 }]);
        assert_eq!(ranked(&v), ["shoegaze", "dream pop"]);
    }
}
