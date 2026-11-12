//! Small JSON files in ~/.cache/spotlite: what spotlite keeps between runs so it asks Spotify
//! less (list pages, playlists, albums, artist pages, Liked Songs, rate limits, genres).

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

/// When `name` was saved, if it is there.
pub fn saved_at(name: &str) -> Option<std::time::SystemTime> {
    if cfg!(test) {
        return None;
    }
    std::fs::metadata(path(name)).ok()?.modified().ok()
}

/// Removes the oldest files of the folder `name` until it holds at most `limit` bytes.
pub fn prune(name: &str, limit: u64) {
    if !cfg!(test) {
        prune_dir(&path(name), limit);
    }
}

/// Removes the oldest files of `dir` until it holds at most `limit` bytes.
pub fn prune_dir(dir: &std::path::Path, limit: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}
