//! What spotlite remembers between runs: the open artist, album and radio tabs, and the
//! theme. Saved as JSON in ~/.local/state/spotlite/state.json when spotlite quits.

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
}

/// An open page, with what is needed to load it again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SavedTab {
    Artist { id: String, name: String },
    Album { album: Album },
    Radio { seed: Track },
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
