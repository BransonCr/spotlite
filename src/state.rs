//! What spotlite remembers between runs: the open tabs and the tab shown, the search, the
//! theme, the visualizer, the sidebar and its pins, and what was playing (song, position, shuffle, repeat, volume).
//! Saved as JSON in ~/.local/state/spotlite/state.json every few seconds when something
//! changed, and when spotlite quits or its window closes.

use crate::api::{Album, Track};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub tabs: Vec<SavedTab>,
    /// The tab shown at quit: an index into `tabs`, or None for Search / Playlists / Liked.
    #[serde(default)]
    pub active: Option<usize>,
    /// Search, Playlists, Liked or Recent (shown when no page tab is).
    #[serde(default)]
    pub tab: Option<crate::app::Tab>,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub search_kind: Option<crate::app::SearchKind>,
    #[serde(default)]
    pub viz_on: bool,
    /// The effect over the sidebar's song (V).
    #[serde(default)]
    pub side_fx: crate::sidebar::SideFx,
    #[serde(default)]
    pub playback: Option<SavedPlayback>,
    #[serde(default)]
    pub sidebar: Option<SavedSidebar>,
    /// Songs, albums and playlists pinned in the sidebar's Library, newest first.
    #[serde(default)]
    pub pins: Vec<crate::sidebar::Pin>,
}

/// The sidebar: open or shut, and which view it shows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SavedSidebar {
    pub open: bool,
    pub view: crate::sidebar::SideView,
}

/// What was playing, to start it again when nothing plays at the next start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPlayback {
    pub track: String,
    pub name: String,
    /// Playlist, album or Liked Songs it played from (None: a plain list of songs).
    pub context: Option<String>,
    pub position_ms: u64,
    pub playing: bool,
    pub shuffle: bool,
    /// "off", "context" or "track".
    pub repeat: String,
    pub volume: Option<u8>,
}

/// A radio page's filters and discover rounds (the radio itself loads again from Spotify).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedRadio {
    pub filters: crate::radio::Filters,
    pub extra: Vec<crate::radio::RadioTrack>,
    pub seen: Vec<String>,
    pub rank: std::collections::HashMap<String, u32>,
    pub round: u32,
}

/// An open page, with what is needed to load it again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SavedTab {
    Artist { id: String, name: String },
    Album { album: Album },
    Radio {
        seed: Track,
        /// The filters and the discover rounds, so a restart shows the same list.
        #[serde(default)]
        tuned: Option<Box<SavedRadio>>,
    },
    Random { genre: String },
}

pub fn path() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotlite/state.json")
}

/// The saved state; empty when there is none or it cannot be read.
pub fn load() -> State {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(state: &State) -> anyhow::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write then rename, so a crash never leaves half a file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(state)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
