//! Thin blocking client for the Spotify Web API (rules as of the Feb 2026 dev-mode changes).

use crate::auth::{self, Token};
use crate::config::Config;
use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const API: &str = "https://api.spotify.com/v1";
/// Dev-mode apps get at most 10 search results per request.
pub const SEARCH_PAGE: usize = 10;
pub const PAGE: usize = 50;

/// Errors the UI reacts to differently from a plain message.
#[derive(Debug)]
pub enum ApiError {
    /// 429: wait this many seconds before the next request.
    RateLimited(u64),
    /// 401: the login is no longer valid.
    Unauthorized,
    /// 403 "Insufficient client scope": the token is missing a permission.
    MissingScope,
    /// 403 on a playlist the user does not own or collaborate on.
    Forbidden,
    /// 404 "no active device".
    NoDevice,
    /// The request did not reach Spotify (no network, a timeout).
    Offline,
    Other(u16, String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ApiError::RateLimited(s) if *s >= 120 => write!(f, "Spotify limits this request for {} min", s.div_ceil(60)),
            ApiError::RateLimited(s) => write!(f, "Spotify limits this request; try again in {s}s"),
            ApiError::Unauthorized => write!(f, "login expired; run `spotlite auth`"),
            ApiError::MissingScope => write!(f, "permission missing; run `spotlite auth` again"),
            ApiError::Forbidden => write!(f, "Spotify does not allow this"),
            ApiError::NoDevice => write!(f, "no active device; is spotifyd running?"),
            ApiError::Offline => write!(f, "no connection to Spotify"),
            ApiError::Other(code, _) if *code >= 500 => write!(f, "Spotify has a problem on its side ({code}); try again in a moment"),
            ApiError::Other(code, msg) if msg.is_empty() => write!(f, "Spotify refused this ({code})"),
            ApiError::Other(code, msg) => write!(f, "Spotify refused this ({code}: {msg})"),
        }
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Named {
    pub name: String,
    /// Artist URI (absent on some old or local items).
    pub uri: Option<String>,
}

impl Named {
    pub fn id(&self) -> Option<&str> {
        self.uri.as_deref()?.rsplit(':').next()
    }
}

/// One size of a cover picture. Custom playlist covers come without a size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub url: String,
    pub width: Option<u32>,
}

/// `images` can be missing or null; both mean "no picture".
fn null_as_empty<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// URL of the smallest picture that is at least `min_px` wide, else the largest one.
pub fn pick_image(images: &[ImageRef], min_px: u32) -> Option<&str> {
    let size = |i: &ImageRef| i.width.unwrap_or(u32::MAX);
    images
        .iter()
        .filter(|i| size(i) >= min_px)
        .min_by_key(|i| size(i))
        .or_else(|| images.iter().max_by_key(|i| size(i)))
        .map(|i| i.url.as_str())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub name: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub images: Vec<ImageRef>,
    pub id: Option<String>,
    pub uri: Option<String>,
    /// "1999", "1999-05" or "1999-05-17".
    pub release_date: Option<String>,
    /// "album", "single" or "compilation".
    pub album_type: Option<String>,
    pub total_tracks: Option<usize>,
    /// Given in search results and album details; empty elsewhere.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub artists: Vec<Named>,
}

impl Album {
    pub fn year(&self) -> Option<&str> {
        self.release_date.as_deref().map(|d| d.split('-').next().unwrap_or(d))
    }

    /// "Album · 1999 · 12 tracks"
    pub fn summary(&self) -> String {
        let kind = match self.album_type.as_deref() {
            Some("single") => "Single",
            Some("compilation") => "Compilation",
            _ => "Album",
        };
        let mut parts = vec![kind.to_owned()];
        parts.extend(self.year().map(str::to_owned));
        parts.extend(self.total_tracks.map(|n| format!("{n} tracks")));
        parts.join(" · ")
    }
}

/// An artist's profile. The Feb 2026 API keeps only name and pictures here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub images: Vec<ImageRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub artists: Vec<Named>,
    pub album: Option<Album>,
    /// Podcast episodes have a show instead of artists, and their own pictures.
    pub show: Option<Named>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub images: Vec<ImageRef>,
}

impl Track {
    pub fn artist_names(&self) -> String {
        if self.artists.is_empty() {
            return self.show.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        }
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn album_name(&self) -> &str {
        self.album.as_ref().map_or("", |a| a.name.as_str())
    }

    pub fn images(&self) -> &[ImageRef] {
        match &self.album {
            Some(a) if !a.images.is_empty() => &a.images,
            _ => &self.images,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Owner {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Total {
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub owner: Owner,
    #[serde(default)]
    pub collaborative: bool,
    /// Called `tracks` before the Feb 2026 rename.
    #[serde(alias = "tracks")]
    pub items: Option<Total>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub images: Vec<ImageRef>,
    /// Changes when the playlist's songs change: a saved copy with the same one is current.
    #[serde(default)]
    pub snapshot_id: Option<String>,
}

impl Playlist {
    pub fn track_count(&self) -> Option<usize> {
        self.items.as_ref().map(|t| t.total)
    }

    pub fn owner_name(&self) -> &str {
        self.owner.display_name.as_deref().unwrap_or(&self.owner.id)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Device {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub is_active: bool,
    pub volume_percent: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Context {
    pub uri: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Playback {
    pub device: Option<Device>,
    #[serde(default)]
    pub is_playing: bool,
    #[serde(default, deserialize_with = "position")]
    pub progress_ms: Option<u64>,
    #[serde(default)]
    pub shuffle_state: bool,
    /// "off", "context" or "track".
    #[serde(default = "repeat_off")]
    pub repeat_state: String,
    pub context: Option<Context>,
    pub item: Option<Track>,
}

/// A position in ms. Spotify sends a small negative one at times, just after a song starts:
/// that is 0.
fn position<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(|ms| ms.max(0.0) as u64))
}

fn repeat_off() -> String {
    "off".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub uri: String,
}

/// One page of a list, plus the size of the whole list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Entries Spotify returned, null ones included; the next page starts this far on.
    pub raw: usize,
    pub total: usize,
}

impl<T> Page<T> {
    fn empty() -> Self {
        Page { items: Vec::new(), raw: 0, total: 0 }
    }
}

/// Spotify paging object. Items can be null (e.g. removed tracks), so they are skipped.
#[derive(Deserialize)]
struct RawPage<T> {
    items: Vec<Option<T>>,
    #[serde(default)]
    total: usize,
}

impl<T> RawPage<T> {
    fn into_page<U>(self, f: impl FnMut(T) -> Option<U>) -> Page<U> {
        Page {
            raw: self.items.len(),
            items: self.items.into_iter().flatten().filter_map(f).collect(),
            total: self.total,
        }
    }
}

/// A search result that is not a song.
#[derive(Debug, Clone)]
pub enum Found {
    Artist(Artist),
    Album(Album),
    Playlist(Playlist),
}

impl Found {
    pub fn name(&self) -> &str {
        match self {
            Found::Artist(a) => &a.name,
            Found::Album(a) => &a.name,
            Found::Playlist(p) => &p.name,
        }
    }

    pub fn images(&self) -> &[ImageRef] {
        match self {
            Found::Artist(a) => &a.images,
            Found::Album(a) => &a.images,
            Found::Playlist(p) => &p.images,
        }
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    tracks: RawPage<Track>,
}

#[derive(Deserialize)]
struct SavedTrack {
    track: Option<Track>,
}

/// Entry of /me/player/recently-played.
#[derive(Deserialize)]
struct PlayedEntry {
    track: Option<Track>,
    #[serde(default)]
    played_at: String,
}

/// Entry of /playlists/{id}/items. The track was in `track` before the Feb 2026 rename.
#[derive(Deserialize)]
struct PlaylistEntry {
    item: Option<Track>,
    track: Option<Track>,
}

#[derive(Deserialize)]
struct Devices {
    devices: Vec<Device>,
}

/// What to start playing.
#[derive(Debug, Clone)]
pub enum Play {
    /// Play a playlist/album/collection context, optionally from one track in it.
    Context { uri: String, from: Option<String> },
    /// Play a list of track URIs, starting at `offset`.
    Tracks { uris: Vec<String>, offset: usize },
}

/// A client. Clones share the connection pool, the login and the rate limits, so two threads
/// can each have one.
#[derive(Clone)]
pub struct Spotify {
    agent: ureq::Agent,
    cfg: Config,
    token: Arc<Mutex<Token>>,
    /// Requests Spotify refused with 429, by bucket, and the time (unix s) they open again.
    /// Until then spotlite does not ask: each refused request makes the wait longer.
    limits: Arc<Mutex<std::collections::HashMap<String, u64>>>,
}

/// The bucket of a request path: its ids become `*` ("/playlists/*/items").
pub fn bucket(path: &str) -> String {
    path.split('/')
        .map(|seg| if seg.len() >= 16 && seg.chars().all(|c| c.is_ascii_alphanumeric()) { "*" } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Spotify {
    pub fn new(cfg: Config) -> Result<Self> {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build();
        Ok(Spotify {
            agent,
            cfg,
            token: Arc::new(Mutex::new(Token::load()?)),
            limits: Arc::new(Mutex::new(crate::store::read("limits.json").unwrap_or_default())),
        })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// False when the saved login predates a scope that spotlite now asks for.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.token()
            .scope
            .as_deref()
            .is_some_and(|s| s.split_whitespace().any(|x| x == scope))
    }

    fn token(&self) -> MutexGuard<'_, Token> {
        self.token.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn limits(&self) -> MutexGuard<'_, std::collections::HashMap<String, u64>> {
        self.limits.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The access token, refreshed first when it ran out (one thread refreshes; the other waits
    /// for it).
    fn bearer(&mut self) -> Result<String> {
        let mut token = self.token();
        if token.expired() {
            *token = auth::refresh(&self.cfg, &token)?;
        }
        Ok(format!("Bearer {}", token.access_token))
    }

    fn request(
        &mut self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Option<ureq::Response>> {
        let key = bucket(path);
        let now = unix_now();
        {
            let mut limits = self.limits();
            if let Some(&until) = limits.get(&key) {
                if until > now {
                    return Err(ApiError::RateLimited(until - now).into());
                }
                limits.remove(&key);
            }
        }
        // One more try when it can help and cannot do harm: a GET that Spotify's side failed
        // (5xx) or that did not get through, and any request refused as "login expired"
        // (the token can end before its time: a new one often works).
        let mut tries = 0;
        loop {
            tries += 1;
            let auth = self.bearer()?;
            let result = self.request_once(&auth, method, path, query, body.clone());
            let again = tries < 2
                && match &result {
                    Err(ureq::Error::Status(401, _)) => {
                        self.token().expires_at = 0;
                        true
                    }
                    Err(ureq::Error::Status(code, _)) => method == "GET" && *code >= 500,
                    Err(ureq::Error::Transport(_)) => method == "GET",
                    Ok(_) => false,
                };
            if crate::log::on() {
                let status = match &result {
                    Ok(resp) => resp.status().to_string(),
                    Err(ureq::Error::Status(code, _)) => code.to_string(),
                    Err(_) => "no answer".to_owned(),
                };
                crate::log::line(&format!("{method} {key} {status}"));
            }
            if again {
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
            return match result {
                Ok(resp) if resp.status() == 204 => Ok(None),
                Ok(resp) => Ok(Some(resp)),
                Err(ureq::Error::Status(code, resp)) => {
                    let retry_after = resp.header("Retry-After").and_then(|s| s.parse().ok()).unwrap_or(5);
                    let body: Value = resp.into_json().unwrap_or(Value::Null);
                    let msg = body["error"]["message"].as_str().unwrap_or("").to_owned();
                    if code == 429 {
                        let mut limits = self.limits();
                        limits.insert(key, now + retry_after);
                        crate::store::write("limits.json", &*limits);
                    }
                    Err(classify(code, msg, retry_after).into())
                }
                Err(ureq::Error::Transport(_)) => Err(ApiError::Offline.into()),
            };
        }
    }

    #[allow(clippy::result_large_err)]
    fn request_once(
        &self,
        auth: &str,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<ureq::Response, ureq::Error> {
        let mut req = self.agent.request(method, &format!("{API}{path}")).set("Authorization", auth);
        for (k, v) in query {
            req = req.query(k, v);
        }
        match body {
            Some(body) => req.send_json(body),
            // Spotify wants Content-Length on body-less PUT/POST.
            None if method != "GET" => req.send_bytes(&[]),
            None => req.call(),
        }
    }

    fn get<T: DeserializeOwned>(&mut self, path: &str, query: &[(&str, &str)]) -> Result<Option<T>> {
        match self.request("GET", path, query, None)? {
            Some(resp) => Ok(Some(resp.into_json()?)),
            None => Ok(None),
        }
    }

    /// Player commands answer 200 with a plain-text body, or 204; the body is not used.
    fn send(&mut self, method: &str, path: &str, query: &[(&str, &str)], body: Option<Value>) -> Result<()> {
        self.request(method, path, query, body).map(|_| ())
    }

    fn with_device<'a>(device_id: Option<&'a str>, mut query: Vec<(&'a str, &'a str)>) -> Vec<(&'a str, &'a str)> {
        if let Some(id) = device_id {
            query.push(("device_id", id));
        }
        query
    }

    pub fn me(&mut self) -> Result<User> {
        self.get("/me", &[])?
            .ok_or_else(|| anyhow::anyhow!("empty /me response"))
    }

    /// The user, from the copy kept on disk if there is one, else from Spotify (then kept).
    /// Spotify limits /me hard (once an app starts often, it can refuse it for hours), and the
    /// user of a login never changes; `spotlite auth` removes the copy.
    pub fn me_cached(&mut self) -> Result<User> {
        let path = crate::config::user_path();
        if let Some(user) = read_user(&path) {
            return Ok(user);
        }
        let user = match self.me() {
            Ok(user) => user,
            // Refused (often a rate limit): the client protocol login knows the same id.
            Err(e) => match crate::client::username() {
                Some(id) => User { uri: format!("spotify:user:{id}"), id },
                None => return Err(e),
            },
        };
        write_user(&path, &user);
        Ok(user)
    }

    pub fn playback(&mut self) -> Result<Option<Playback>> {
        self.get("/me/player", &[])
    }

    pub fn devices(&mut self) -> Result<Vec<Device>> {
        Ok(self
            .get::<Devices>("/me/player/devices", &[])?
            .map(|d| d.devices)
            .unwrap_or_default())
    }

    /// The configured spotifyd device if Spotify sees it, else the active device.
    pub fn find_device(&mut self) -> Result<Option<Device>> {
        let wanted = self.cfg.device_name.clone();
        let devices = self.devices()?;
        Ok(devices
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(&wanted))
            .or_else(|| devices.iter().find(|d| d.is_active))
            .cloned())
    }

    pub fn search_tracks(&mut self, query: &str, offset: usize) -> Result<Page<Track>> {
        let offset = offset.to_string();
        let limit = SEARCH_PAGE.to_string();
        let resp: Option<SearchResponse> = self.get(
            "/search",
            &[("q", query), ("type", "track"), ("limit", &limit), ("offset", &offset)],
        )?;
        Ok(resp
            .map(|r| r.tracks.into_page(Some))
            .unwrap_or_else(Page::empty))
    }

    /// Search for one kind of result: "artist", "album" or "playlist".
    pub fn search(&mut self, query: &str, kind: &str, offset: usize) -> Result<Page<Found>> {
        let off = offset.to_string();
        let limit = SEARCH_PAGE.to_string();
        let resp: Option<Value> =
            self.get("/search", &[("q", query), ("type", kind), ("limit", &limit), ("offset", &off)])?;
        let Some(mut resp) = resp else { return Ok(Page::empty()) };
        // Not `resp[..]`: indexing a non-object Value for writing panics, and a panic aborts the app.
        let v = resp.get_mut(format!("{kind}s")).map(Value::take).unwrap_or(Value::Null);
        Ok(match kind {
            "artist" => serde_json::from_value::<RawPage<Artist>>(v)?.into_page(|a| Some(Found::Artist(a))),
            "album" => serde_json::from_value::<RawPage<Album>>(v)?.into_page(|a| Some(Found::Album(a))),
            _ => serde_json::from_value::<RawPage<Playlist>>(v)?.into_page(|p| Some(Found::Playlist(p))),
        })
    }

    fn page<T: DeserializeOwned, U>(
        &mut self,
        path: &str,
        offset: usize,
        f: impl FnMut(T) -> Option<U>,
    ) -> Result<Page<U>> {
        let offset = offset.to_string();
        let limit = PAGE.to_string();
        let raw: Option<RawPage<T>> = self.get(path, &[("limit", &limit), ("offset", &offset)])?;
        Ok(raw
            .map(|r| r.into_page(f))
            .unwrap_or_else(Page::empty))
    }

    pub fn playlists(&mut self, offset: usize) -> Result<Page<Playlist>> {
        self.page("/me/playlists", offset, Some)
    }

    /// The last 50 songs played on Spotify, newest first, with when each played. Songs that
    /// spotifyd played are not there (see `history`).
    pub fn recently_played(&mut self) -> Result<Vec<crate::history::Played>> {
        let raw: Option<RawPage<PlayedEntry>> = self.get("/me/player/recently-played", &[("limit", "50")])?;
        let items = raw.map(|r| r.items).unwrap_or_default();
        Ok(items
            .into_iter()
            .flatten()
            .filter_map(|e| Some(crate::history::Played { at: crate::history::parse_time(&e.played_at)?, track: e.track? }))
            .collect())
    }

    /// Your most played artists over a time range ("short_term", "medium_term", "long_term").
    pub fn top_artists(&mut self, range: &str) -> Result<Vec<Artist>> {
        let raw: Option<RawPage<Artist>> = self.get("/me/top/artists", &[("limit", "50"), ("time_range", range)])?;
        Ok(raw.map(|r| r.into_page(Some).items).unwrap_or_default())
    }

    pub fn top_tracks(&mut self, range: &str) -> Result<Vec<Track>> {
        let raw: Option<RawPage<Track>> = self.get("/me/top/tracks", &[("limit", "50"), ("time_range", range)])?;
        Ok(raw.map(|r| r.into_page(Some).items).unwrap_or_default())
    }

    pub fn liked_tracks(&mut self, offset: usize) -> Result<Page<Track>> {
        self.page("/me/tracks", offset, |s: SavedTrack| s.track)
    }

    /// Only works for playlists the user owns or collaborates on (else ApiError::Forbidden).
    pub fn artist(&mut self, id: &str) -> Result<Artist> {
        self.get(&format!("/artists/{id}"), &[])?
            .ok_or_else(|| anyhow::anyhow!("empty artist response"))
    }

    /// Albums and singles, as Spotify orders them; the first 30 (dev-mode pages hold 10).
    pub fn artist_albums(&mut self, id: &str) -> Result<Vec<Album>> {
        let mut out = Vec::new();
        for offset in [0, 10, 20] {
            let offset = offset.to_string();
            let raw: Option<RawPage<Album>> = self.get(
                &format!("/artists/{id}/albums"),
                &[("include_groups", "album,single"), ("limit", "10"), ("offset", &offset)],
            )?;
            let Some(page) = raw.map(|r| r.into_page(Some)) else { break };
            let done = out.len() + page.raw >= page.total || page.raw == 0;
            out.extend(page.items);
            if done {
                break;
            }
        }
        Ok(out)
    }

    /// The artist's popular songs. /artists/{id}/top-tracks is closed to dev-mode apps, so this
    /// searches for the artist's tracks (Spotify ranks them by popularity) and keeps theirs.
    pub fn artist_top_tracks(&mut self, artist: &Artist) -> Result<Vec<Track>> {
        let q = format!("artist:\"{}\"", artist.name.replace('"', ""));
        let mut out = Vec::new();
        for offset in [0, SEARCH_PAGE] {
            let page = self.search_tracks(&q, offset)?;
            out.extend(page.items.into_iter().filter(|t| t.artists.iter().any(|a| a.id() == Some(artist.id.as_str()))));
        }
        Ok(out)
    }

    pub fn album(&mut self, id: &str) -> Result<Album> {
        self.get(&format!("/albums/{id}"), &[])?
            .ok_or_else(|| anyhow::anyhow!("empty album response"))
    }

    /// Tracks of an album (simplified: their `album` field is empty).
    pub fn album_tracks(&mut self, album_id: &str, offset: usize) -> Result<Page<Track>> {
        self.page(&format!("/albums/{album_id}/tracks"), offset, Some)
    }

    /// Adds tracks to a playlist, at the end or at `position` (needs playlist-modify-*).
    pub fn add_to_playlist(&mut self, playlist_id: &str, uris: &[String], position: Option<usize>) -> Result<()> {
        let mut body = json!({ "uris": uris });
        if let Some(p) = position {
            body["position"] = json!(p);
        }
        self.send("POST", &format!("/playlists/{playlist_id}/items"), &[], Some(body))
    }

    /// Removes every occurrence of the tracks from a playlist (needs playlist-modify-*).
    pub fn remove_from_playlist(&mut self, playlist_id: &str, uris: &[String]) -> Result<()> {
        let items: Vec<Value> = uris.iter().map(|u| json!({ "uri": u })).collect();
        self.send("DELETE", &format!("/playlists/{playlist_id}/items"), &[], Some(json!({ "items": items })))
    }

    pub fn playlist_tracks(&mut self, playlist_id: &str, offset: usize) -> Result<Page<Track>> {
        self.page(&format!("/playlists/{playlist_id}/items"), offset, |e: PlaylistEntry| {
            e.item.or(e.track)
        })
    }

    pub fn play(&mut self, device_id: Option<&str>, what: Option<&Play>) -> Result<()> {
        self.play_at(device_id, what, 0)
    }

    /// Like `play`, starting `position_ms` into the first song.
    pub fn play_at(&mut self, device_id: Option<&str>, what: Option<&Play>, position_ms: u64) -> Result<()> {
        let body = what.map(|what| {
            let mut body = match what {
                Play::Context { uri, from: Some(track) } => {
                    json!({ "context_uri": uri, "offset": { "uri": track } })
                }
                Play::Context { uri, from: None } => json!({ "context_uri": uri }),
                Play::Tracks { uris, offset } => json!({ "uris": uris, "offset": { "position": offset } }),
            };
            if position_ms > 0 {
                body["position_ms"] = json!(position_ms);
            }
            body
        });
        self.send("PUT", "/me/player/play", &Self::with_device(device_id, vec![]), body)
    }

    /// Moves playback to a device (used when no device is active).
    pub fn transfer(&mut self, device_id: &str, play: bool) -> Result<()> {
        self.send("PUT", "/me/player", &[], Some(json!({ "device_ids": [device_id], "play": play })))
    }

    pub fn pause(&mut self, device_id: Option<&str>) -> Result<()> {
        self.send("PUT", "/me/player/pause", &Self::with_device(device_id, vec![]), None)
    }

    pub fn next(&mut self, device_id: Option<&str>) -> Result<()> {
        self.send("POST", "/me/player/next", &Self::with_device(device_id, vec![]), None)
    }

    pub fn previous(&mut self, device_id: Option<&str>) -> Result<()> {
        self.send("POST", "/me/player/previous", &Self::with_device(device_id, vec![]), None)
    }

    pub fn seek(&mut self, device_id: Option<&str>, position_ms: u64) -> Result<()> {
        let pos = position_ms.to_string();
        self.send("PUT", "/me/player/seek", &Self::with_device(device_id, vec![("position_ms", &pos)]), None)
    }

    pub fn set_volume(&mut self, device_id: Option<&str>, percent: u8) -> Result<()> {
        let pct = percent.min(100).to_string();
        self.send("PUT", "/me/player/volume", &Self::with_device(device_id, vec![("volume_percent", &pct)]), None)
    }

    pub fn set_shuffle(&mut self, device_id: Option<&str>, on: bool) -> Result<()> {
        let state = if on { "true" } else { "false" };
        self.send("PUT", "/me/player/shuffle", &Self::with_device(device_id, vec![("state", state)]), None)
    }

    /// `state` is "off", "context" or "track".
    pub fn set_repeat(&mut self, device_id: Option<&str>, state: &str) -> Result<()> {
        self.send("PUT", "/me/player/repeat", &Self::with_device(device_id, vec![("state", state)]), None)
    }

    /// The songs that play next, in order (Spotify gives up to 20). Episodes are left out.
    pub fn up_next(&mut self) -> Result<Vec<Track>> {
        let v: Option<Value> = self.get("/me/player/queue", &[])?;
        Ok(v.map(queued).unwrap_or_default())
    }

    pub fn queue(&mut self, device_id: Option<&str>, uri: &str) -> Result<()> {
        self.send("POST", "/me/player/queue", &Self::with_device(device_id, vec![("uri", uri)]), None)
    }

    /// A new private playlist of the user's (needs playlist-modify-private).
    pub fn create_playlist(&mut self, name: &str, description: &str) -> Result<Playlist> {
        let body = json!({ "name": name, "description": description, "public": false });
        self.request("POST", "/me/playlists", &[], Some(body))?
            .ok_or_else(|| anyhow::anyhow!("empty create-playlist response"))?
            .into_json()
            .map_err(Into::into)
    }

    pub fn is_saved(&mut self, uri: &str) -> Result<bool> {
        let saved: Option<Vec<bool>> = self.get("/me/library/contains", &[("uris", uri)])?;
        Ok(saved.and_then(|v| v.first().copied()).unwrap_or(false))
    }

    /// Adds or removes a track from Liked Songs (needs user-library-modify).
    pub fn set_saved(&mut self, uri: &str, saved: bool) -> Result<()> {
        let method = if saved { "PUT" } else { "DELETE" };
        self.send(method, "/me/library", &[("uris", uri)], None)
    }
}

/// The songs of a `/me/player/queue` answer. The queue can be empty or missing, and an entry
/// can be null or an episode: those are skipped (an index into a value we change would panic,
/// and a panic aborts the app).
fn read_user(path: &std::path::Path) -> Option<User> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_user(path: &std::path::Path, user: &User) {
    if let (Some(dir), Ok(json)) = (path.parent(), serde_json::to_vec(user)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, json);
    }
}

fn queued(mut v: Value) -> Vec<Track> {
    let Some(Value::Array(items)) = v.get_mut("queue").map(Value::take) else { return Vec::new() };
    items.into_iter().filter(|i| !i.is_null()).filter_map(|i| serde_json::from_value(i).ok()).collect()
}

fn classify(code: u16, msg: String, retry_after: u64) -> ApiError {
    let lower = msg.to_lowercase();
    match code {
        401 => ApiError::Unauthorized,
        403 if lower.contains("scope") => ApiError::MissingScope,
        403 => ApiError::Forbidden,
        404 if lower.contains("device") => ApiError::NoDevice,
        429 => ApiError::RateLimited(retry_after),
        _ => ApiError::Other(code, msg),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn buckets_hide_ids() {
        assert_eq!(super::bucket("/playlists/37i9dQZF1DXcBWIGoYBM5M/items"), "/playlists/*/items");
        assert_eq!(super::bucket("/me/tracks"), "/me/tracks");
    }

    #[test]
    fn up_next_with_an_empty_or_odd_queue_does_not_panic() {
        use serde_json::json;
        assert!(super::queued(json!({"currently_playing": null, "queue": []})).is_empty());
        assert!(super::queued(json!({})).is_empty());
        assert!(super::queued(json!(null)).is_empty());
        let song = |n: &str| json!({"uri": format!("spotify:track:{n}"), "name": n, "duration_ms": 1, "artists": [], "album": {"name": "X"}});
        let t = super::queued(json!({"queue": [song("A"), null, {"odd": true}, song("B")]}));
        assert_eq!(t.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["A", "B"], "in order, odd entries skipped");
    }

    use super::*;

    #[test]
    fn playback_with_nulls_parses() {
        let json = r#"{
            "device": {"id": "d1", "name": "spotlite", "is_active": true, "volume_percent": 40},
            "is_playing": true, "progress_ms": 1234, "shuffle_state": false, "repeat_state": "context",
            "context": {"uri": "spotify:user:u:collection", "type": "collection"},
            "item": {"uri": "spotify:track:1", "name": "Song", "duration_ms": 200000,
                     "artists": [{"name": "A"}, {"name": "B"}], "album": {"name": "Al"}},
            "currently_playing_type": "track"
        }"#;
        let pb: Playback = serde_json::from_str(json).unwrap();
        assert_eq!(pb.repeat_state, "context");
        assert_eq!(pb.item.unwrap().artist_names(), "A, B");

        let idle: Playback = serde_json::from_str(r#"{"device": null, "item": null}"#).unwrap();
        assert!(!idle.is_playing && idle.item.is_none());
        assert_eq!(idle.progress_ms, None);

        let early: Playback = serde_json::from_str(r#"{"progress_ms": -285, "item": null}"#).unwrap();
        assert_eq!(early.progress_ms, Some(0));
        assert_eq!(idle.repeat_state, "off");
    }

    #[test]
    fn the_user_is_kept_on_disk() {
        let path = std::env::temp_dir().join(format!("spotlite-user-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(read_user(&path).is_none());
        write_user(&path, &User { id: "me".into(), uri: "spotify:user:me".into() });
        assert_eq!(read_user(&path).map(|u| u.id).as_deref(), Some("me"));
        std::fs::write(&path, "broken").unwrap();
        assert!(read_user(&path).is_none(), "a broken copy asks Spotify again");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn episode_uses_show_name() {
        let ep: Track = serde_json::from_str(
            r#"{"uri": "spotify:episode:1", "name": "Ep", "show": {"name": "Pod"}}"#,
        )
        .unwrap();
        assert_eq!(ep.artist_names(), "Pod");
    }

    #[test]
    fn page_skips_null_items() {
        let json = r#"{"items": [{"track": {"uri": "u", "name": "n"}}, null, {"track": null}], "total": 3}"#;
        let raw: RawPage<SavedTrack> = serde_json::from_str(json).unwrap();
        let page = raw.into_page(|s| s.track);
        assert_eq!((page.items.len(), page.raw, page.total), (1, 3, 3));
    }

    #[test]
    fn playlist_parses_new_and_old_names() {
        let new = r#"{"id": "1", "uri": "spotify:playlist:1", "name": "P",
                      "owner": {"id": "me", "display_name": null}, "items": {"total": 45}}"#;
        let old = r#"{"id": "1", "uri": "spotify:playlist:1", "name": "P",
                      "owner": {"id": "me"}, "tracks": {"total": 7}}"#;
        let p: Playlist = serde_json::from_str(new).unwrap();
        assert_eq!((p.track_count(), p.owner_name()), (Some(45), "me"));
        let p: Playlist = serde_json::from_str(old).unwrap();
        assert_eq!(p.track_count(), Some(7));

        let entry: PlaylistEntry =
            serde_json::from_str(r#"{"item": {"uri": "u", "name": "n"}, "added_at": "x"}"#).unwrap();
        assert!(entry.item.or(entry.track).is_some());
    }

    #[test]
    fn picks_the_right_image_size() {
        let imgs: Vec<ImageRef> = serde_json::from_str(
            r#"[{"url": "640", "width": 640}, {"url": "300", "width": 300}, {"url": "64", "width": 64}]"#,
        )
        .unwrap();
        assert_eq!(pick_image(&imgs, 40), Some("64"));
        assert_eq!(pick_image(&imgs, 100), Some("300"));
        assert_eq!(pick_image(&imgs, 2000), Some("640"));
        let custom: Vec<ImageRef> = serde_json::from_str(r#"[{"url": "c", "width": null}]"#).unwrap();
        assert_eq!(pick_image(&custom, 40), Some("c"));
        assert_eq!(pick_image(&[], 40), None);

        let p: Playlist = serde_json::from_str(
            r#"{"id": "1", "uri": "u", "name": "P", "owner": {"id": "me"}, "images": null}"#,
        )
        .unwrap();
        assert!(p.images.is_empty());
        let t: Track = serde_json::from_str(
            r#"{"uri": "u", "name": "n", "album": {"name": "A", "images": [{"url": "a", "width": 64}]}}"#,
        )
        .unwrap();
        assert_eq!(pick_image(t.images(), 10), Some("a"));
    }

    #[test]
    fn errors_are_classified() {
        assert!(matches!(classify(429, String::new(), 7), ApiError::RateLimited(7)));
        assert!(matches!(classify(403, "Insufficient client scope".into(), 0), ApiError::MissingScope));
        assert!(matches!(classify(403, "Forbidden".into(), 0), ApiError::Forbidden));
        assert!(matches!(classify(404, "Player command failed: No active device found".into(), 0), ApiError::NoDevice));
        assert!(matches!(classify(401, String::new(), 0), ApiError::Unauthorized));
    }
}

