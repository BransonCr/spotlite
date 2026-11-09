//! Small JSON files in ~/.cache/spotlite: what spotlite keeps between runs so it asks Spotify
//! less (saved list pages, the Liked Songs index, rate limits, genres).

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::PathBuf;

pub fn path(name: &str) -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join("spotlite").join(name)
}

/// The value saved as `name`, if there is one and it reads.
pub fn read<T: DeserializeOwned>(name: &str) -> Option<T> {
    if cfg!(test) {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path(name)).ok()?).ok()
}

/// Saves `value` as `name` (through a temporary file, so a crash never leaves half a file).
/// Errors are ignored: the copy only saves requests.
pub fn write<T: Serialize + ?Sized>(name: &str, value: &T) {
    if cfg!(test) {
        return;
    }
    let path = path(name);
    let (Some(dir), Ok(json)) = (path.parent(), serde_json::to_vec(value)) else { return };
    let _ = std::fs::create_dir_all(dir);
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}
