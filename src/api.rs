//! Thin blocking client for the Spotify Web API (rules as of the Feb 2026 dev-mode changes).

use crate::auth::{self, Token};
use crate::config::Config;
use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt;
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
    Other(u16, String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ApiError::RateLimited(s) => write!(f, "rate limited by Spotify; retrying in {s}s"),
            ApiError::Unauthorized => write!(f, "login expired; run `spotlite auth`"),
            ApiError::MissingScope => write!(f, "permission missing; run `spotlite auth` again"),
            ApiError::Forbidden => write!(f, "Spotify does not allow this"),
            ApiError::NoDevice => write!(f, "no active device; is spotifyd running?"),
            ApiError::Other(code, msg) => write!(f, "Spotify API {code}: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    pub name: String,
}

/// One size of a cover picture. Custom playlist covers come without a size.
#[derive(Debug, Clone, Deserialize)]
pub struct ImageRef {
    pub url: String,
    pub width: Option<u32>,
}

/// `images` can be missing or null; both mean "no picture".
fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ImageRef>, D::Error> {
    Ok(Option::<Vec<ImageRef>>::deserialize(d)?.unwrap_or_default())
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

#[derive(Debug, Clone, Deserialize)]
pub struct Album {
    pub name: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub images: Vec<ImageRef>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
pub struct Owner {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Total {
    pub total: usize,
}

#[derive(Debug, Clone, Deserialize)]
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
    pub progress_ms: Option<u64>,
    #[serde(default)]
    pub shuffle_state: bool,
    /// "off", "context" or "track".
    #[serde(default = "repeat_off")]
    pub repeat_state: String,
    pub context: Option<Context>,
    pub item: Option<Track>,
}

fn repeat_off() -> String {
    "off".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    pub uri: String,
}

/// One page of a list, plus the size of the whole list.
#[derive(Debug, Clone)]
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

#[derive(Deserialize)]
struct SearchResponse {
    tracks: RawPage<Track>,
}

#[derive(Deserialize)]
struct SavedTrack {
    track: Option<Track>,
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

pub struct Spotify {
    agent: ureq::Agent,
    cfg: Config,
    token: Token,
}

impl Spotify {
    pub fn new(cfg: Config) -> Result<Self> {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build();
        Ok(Spotify {
            agent,
            cfg,
            token: Token::load()?,
        })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// False when the saved login predates a scope that spotlite now asks for.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.token
            .scope
            .as_deref()
            .is_some_and(|s| s.split_whitespace().any(|x| x == scope))
    }

    fn bearer(&mut self) -> Result<String> {
        if self.token.expired() {
            self.token = auth::refresh(&self.cfg, &self.token)?;
        }
        Ok(format!("Bearer {}", self.token.access_token))
    }

    fn request(
        &mut self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Option<ureq::Response>> {
        let auth = self.bearer()?;
        let mut req = self
            .agent
            .request(method, &format!("{API}{path}"))
            .set("Authorization", &auth);
        for (k, v) in query {
            req = req.query(k, v);
        }
        let result = match body {
            Some(body) => req.send_json(body),
            // Spotify wants Content-Length on body-less PUT/POST.
            None if method != "GET" => req.send_bytes(&[]),
            None => req.call(),
        };
        match result {
            Ok(resp) if resp.status() == 204 => Ok(None),
            Ok(resp) => Ok(Some(resp)),
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = resp
                    .header("Retry-After")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(5);
                let body: Value = resp.into_json().unwrap_or(Value::Null);
                let msg = body["error"]["message"].as_str().unwrap_or("no details").to_owned();
                Err(classify(code, msg, retry_after).into())
            }
            Err(e) => Err(e.into()),
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

    pub fn liked_tracks(&mut self, offset: usize) -> Result<Page<Track>> {
        self.page("/me/tracks", offset, |s: SavedTrack| s.track)
    }

    /// Only works for playlists the user owns or collaborates on (else ApiError::Forbidden).
    pub fn playlist_tracks(&mut self, playlist_id: &str, offset: usize) -> Result<Page<Track>> {
        self.page(&format!("/playlists/{playlist_id}/items"), offset, |e: PlaylistEntry| {
            e.item.or(e.track)
        })
    }

    pub fn play(&mut self, device_id: Option<&str>, what: Option<&Play>) -> Result<()> {
        let body = what.map(|what| match what {
            Play::Context { uri, from: Some(track) } => {
                json!({ "context_uri": uri, "offset": { "uri": track } })
            }
            Play::Context { uri, from: None } => json!({ "context_uri": uri }),
            Play::Tracks { uris, offset } => json!({ "uris": uris, "offset": { "position": offset } }),
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

    pub fn queue(&mut self, device_id: Option<&str>, uri: &str) -> Result<()> {
        self.send("POST", "/me/player/queue", &Self::with_device(device_id, vec![("uri", uri)]), None)
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
        assert_eq!(idle.repeat_state, "off");
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
