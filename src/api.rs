//! Thin blocking client for the Spotify Web API (rules as of the Feb 2026 dev-mode changes).

use crate::auth::{self, Token};
use crate::config::Config;
use anyhow::{bail, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const API: &str = "https://api.spotify.com/v1";
/// Dev-mode apps get at most 10 search results per request.
const SEARCH_LIMIT: &str = "10";

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub artists: Vec<Named>,
    pub album: Option<Named>,
}

impl Track {
    pub fn artist_names(&self) -> String {
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Owner {
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Playlist {
    pub uri: String,
    pub name: String,
    pub owner: Option<Owner>,
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
pub struct Playback {
    pub device: Option<Device>,
    #[serde(default)]
    pub is_playing: bool,
    pub progress_ms: Option<u64>,
    #[serde(default)]
    pub shuffle_state: bool,
    pub item: Option<Track>,
}

/// Spotify paging object. Items can be null (e.g. removed tracks), so they are skipped.
#[derive(Deserialize)]
struct Page<T> {
    items: Vec<Option<T>>,
}

impl<T> Page<T> {
    fn into_items(self) -> Vec<T> {
        self.items.into_iter().flatten().collect()
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    tracks: Page<Track>,
}

#[derive(Deserialize)]
struct SavedTrack {
    track: Track,
}

#[derive(Deserialize)]
struct Devices {
    devices: Vec<Device>,
}

/// What to start playing.
pub enum Play<'a> {
    /// Continue the current track.
    Resume,
    /// Play a playlist/album context.
    Context(&'a str),
    /// Play a list of track URIs, starting at `offset`.
    Tracks(&'a [String], usize),
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

    /// A client with a fake token, for UI tests that make no requests.
    #[cfg(test)]
    pub fn offline() -> Self {
        Spotify {
            agent: ureq::agent(),
            cfg: toml::from_str(r#"client_id = "test""#).unwrap(),
            token: Token {
                access_token: String::new(),
                refresh_token: String::new(),
                expires_at: u64::MAX,
            },
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
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
                let body: Value = resp.into_json().unwrap_or(Value::Null);
                let msg = body["error"]["message"].as_str().unwrap_or("no details");
                match code {
                    404 if msg.contains("device") || msg.contains("Device") => {
                        bail!("no active device; is spotifyd running?")
                    }
                    429 => bail!("rate limited by Spotify; wait a moment"),
                    _ => bail!("Spotify API {code}: {msg}"),
                }
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

    fn send(&mut self, method: &str, path: &str, query: &[(&str, &str)], body: Option<Value>) -> Result<()> {
        self.request(method, path, query, body).map(|_| ())
    }

    fn device_query(device_id: Option<&str>) -> Vec<(&str, &str)> {
        device_id.map(|id| vec![("device_id", id)]).unwrap_or_default()
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

    pub fn search_tracks(&mut self, query: &str) -> Result<Vec<Track>> {
        let resp: Option<SearchResponse> = self.get(
            "/search",
            &[("q", query), ("type", "track"), ("limit", SEARCH_LIMIT)],
        )?;
        Ok(resp.map(|r| r.tracks.into_items()).unwrap_or_default())
    }

    pub fn playlists(&mut self) -> Result<Vec<Playlist>> {
        let page: Option<Page<Playlist>> = self.get("/me/playlists", &[("limit", "50")])?;
        Ok(page.map(Page::into_items).unwrap_or_default())
    }

    pub fn liked_tracks(&mut self) -> Result<Vec<Track>> {
        let page: Option<Page<SavedTrack>> = self.get("/me/tracks", &[("limit", "50")])?;
        Ok(page
            .map(|p| p.into_items().into_iter().map(|s| s.track).collect())
            .unwrap_or_default())
    }

    pub fn play(&mut self, device_id: Option<&str>, what: Play) -> Result<()> {
        let body = match what {
            Play::Resume => None,
            Play::Context(uri) => Some(json!({ "context_uri": uri })),
            Play::Tracks(uris, offset) => Some(json!({ "uris": uris, "offset": { "position": offset } })),
        };
        self.send("PUT", "/me/player/play", &Self::device_query(device_id), body)
    }

    pub fn pause(&mut self, device_id: Option<&str>) -> Result<()> {
        self.send("PUT", "/me/player/pause", &Self::device_query(device_id), None)
    }

    pub fn next(&mut self, device_id: Option<&str>) -> Result<()> {
        self.send("POST", "/me/player/next", &Self::device_query(device_id), None)
    }

    pub fn previous(&mut self, device_id: Option<&str>) -> Result<()> {
        self.send("POST", "/me/player/previous", &Self::device_query(device_id), None)
    }

    pub fn set_volume(&mut self, device_id: Option<&str>, percent: u8) -> Result<()> {
        let pct = percent.min(100).to_string();
        let mut query = vec![("volume_percent", pct.as_str())];
        query.extend(Self::device_query(device_id));
        self.send("PUT", "/me/player/volume", &query, None)
    }

    pub fn set_shuffle(&mut self, device_id: Option<&str>, on: bool) -> Result<()> {
        let mut query = vec![("state", if on { "true" } else { "false" })];
        query.extend(Self::device_query(device_id));
        self.send("PUT", "/me/player/shuffle", &query, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_with_nulls_parses() {
        let json = r#"{
            "device": {"id": "d1", "name": "spotlite", "is_active": true, "volume_percent": 40},
            "is_playing": true, "progress_ms": 1234, "shuffle_state": false,
            "item": {"uri": "spotify:track:1", "name": "Song", "duration_ms": 200000,
                     "artists": [{"name": "A"}, {"name": "B"}], "album": {"name": "Al"}},
            "currently_playing_type": "track"
        }"#;
        let pb: Playback = serde_json::from_str(json).unwrap();
        assert_eq!(pb.item.unwrap().artist_names(), "A, B");

        let idle: Playback = serde_json::from_str(r#"{"device": null, "item": null}"#).unwrap();
        assert!(!idle.is_playing && idle.item.is_none());
    }

    #[test]
    fn page_skips_null_items() {
        let json = r#"{"items": [{"track": {"uri": "u", "name": "n"}}, null]}"#;
        let page: Page<SavedTrack> = serde_json::from_str(json).unwrap();
        assert_eq!(page.into_items().len(), 1);
    }
}
