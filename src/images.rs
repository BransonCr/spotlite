//! Cover pictures. A background thread downloads, decodes, scales and encodes each picture for
//! the terminal's graphics protocol (kitty in ghostty, else sixel/iTerm2, else half blocks),
//! so the UI thread only places finished pictures.

use image::imageops::FilterType;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;
use std::collections::{HashMap, VecDeque};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// Covers kept encoded in memory; a 4x2-cell thumbnail is a few KB.
const CACHE: usize = 300;
/// Spotify covers are well under this; it stops a bad URL from filling memory.
const MAX_BYTES: u64 = 4 << 20;
/// Downloaded covers kept on disk (`~/.cache/spotlite/covers`), so they show at once after a
/// restart. Spotify cover URLs name their content, so a saved file never goes stale.
const DISK_LIMIT: u64 = 64 << 20;

/// A picture at one size in cells.
type Key = (String, u16, u16);

enum Slot {
    Pending,
    Ready(Protocol),
    Failed,
}

pub struct Images {
    picker: Option<Picker>,
    tx: Option<Sender<Key>>,
    rx: Option<Receiver<(Key, Option<Protocol>)>>,
    cache: HashMap<Key, Slot>,
    order: VecDeque<Key>,
    pending: usize,
}

impl Images {
    pub fn new(picker: Picker) -> Self {
        let (key_tx, key_rx) = channel::<Key>();
        let (img_tx, img_rx) = channel();
        let worker_picker = picker.clone();
        let dir = dirs::cache_dir().map(|d| d.join("spotlite").join("covers"));
        thread::Builder::new()
            .name("spotlite-img".into())
            .spawn(move || {
                if let Some(dir) = &dir {
                    prune(dir, DISK_LIMIT);
                }
                let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
                for key in key_rx {
                    let protocol = load(dir.as_deref(), &key.0, |url| fetch(&agent, url))
                        .and_then(|bytes| encode(&worker_picker, &bytes, key.1, key.2));
                    if img_tx.send((key, protocol)).is_err() {
                        return;
                    }
                }
            })
            .expect("cannot start the image thread");
        Images {
            picker: Some(picker),
            tx: Some(key_tx),
            rx: Some(img_rx),
            cache: HashMap::new(),
            order: VecDeque::new(),
            pending: 0,
        }
    }

    /// No pictures at all (tests, or `SPOTLITE_NO_IMAGES=1`).
    pub fn disabled() -> Self {
        Images {
            picker: None,
            tx: None,
            rx: None,
            cache: HashMap::new(),
            order: VecDeque::new(),
            pending: 0,
        }
    }

    /// Terminal cell size in pixels (width, height); used to keep covers square.
    pub fn font_size(&self) -> (u16, u16) {
        self.picker.as_ref().map_or((10, 20), |p| p.font_size())
    }

    /// Columns that make a square with `rows` rows.
    pub fn square_cols(&self, rows: u16) -> u16 {
        let (w, h) = self.font_size();
        ((rows as f32 * h as f32 / w.max(1) as f32).round() as u16).max(1)
    }

    /// The encoded picture, or None while it loads (the first call starts the load).
    pub fn get(&mut self, url: &str, cols: u16, rows: u16) -> Option<&Protocol> {
        let tx = self.tx.as_ref()?;
        let key = (url.to_owned(), cols, rows);
        if !self.cache.contains_key(&key) {
            if tx.send(key.clone()).is_err() {
                return None;
            }
            self.pending += 1;
            self.cache.insert(key.clone(), Slot::Pending);
            self.order.push_back(key.clone());
            self.evict();
        }
        match self.cache.get(&key) {
            Some(Slot::Ready(p)) => Some(p),
            _ => None,
        }
    }

    /// Takes finished pictures from the thread. Returns true if any arrived.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut any = false;
        while let Ok((key, protocol)) = rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            any = true;
            // It may have been evicted while loading; then it is not wanted any more.
            if let Some(slot) = self.cache.get_mut(&key) {
                *slot = protocol.map_or(Slot::Failed, Slot::Ready);
            }
        }
        any
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    fn evict(&mut self) {
        while self.order.len() > CACHE {
            if let Some(old) = self.order.pop_front() {
                self.cache.remove(&old);
            }
        }
    }
}

/// A cover's bytes (disk cache first), for a caller outside the image thread (the share card).
pub fn cover_bytes(url: &str) -> Option<Vec<u8>> {
    let dir = dirs::cache_dir().map(|d| d.join("spotlite").join("covers"));
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    load(dir.as_deref(), url, |u| fetch(&agent, u))
}

/// The picture's bytes: from the disk cache if it is there, else downloaded and saved.
fn load(dir: Option<&Path>, url: &str, fetch: impl FnOnce(&str) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let Some(dir) = dir else { return fetch(url) };
    let name: String = Sha256::digest(url.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let path = dir.join(&name);
    if let Ok(bytes) = std::fs::read(&path) {
        return Some(bytes);
    }
    let bytes = fetch(url)?;
    // Write a temporary file, then rename: a crash never leaves half a picture.
    let tmp = dir.join(format!("{name}.tmp"));
    if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, &bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
    Some(bytes)
}

/// Removes the oldest saved covers until the folder holds at most `limit` bytes.
fn prune(dir: &Path, limit: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = entries
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

fn fetch(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let resp = agent.get(url).call().ok()?;
    let mut bytes = Vec::new();
    resp.into_reader().take(MAX_BYTES).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Decodes a JPEG/PNG and encodes it to fill `cols` x `rows` cells.
pub fn encode(picker: &Picker, bytes: &[u8], cols: u16, rows: u16) -> Option<Protocol> {
    let img = image::load_from_memory(bytes).ok()?;
    picker
        .new_protocol(img, Rect::new(0, 0, cols, rows), Resize::Scale(Some(FilterType::Triangle)))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    #[test]
    fn encodes_a_png_with_halfblocks() {
        let img = RgbImage::from_fn(64, 64, |x, _| Rgb([x as u8 * 4, 100, 50]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let protocol = encode(&Picker::halfblocks(), &png, 4, 2).expect("encodes");
        assert!(matches!(protocol, Protocol::Halfblocks(_)));
        assert!(encode(&Picker::halfblocks(), b"not an image", 4, 2).is_none());
    }

    #[test]
    fn covers_are_saved_and_read_back_from_disk() {
        let dir = std::env::temp_dir().join(format!("spotlite-covers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut fetched = 0;
        let mut get = |url: &str| {
            load(Some(&dir), url, |_| {
                fetched += 1;
                Some(b"jpeg".to_vec())
            })
        };
        assert_eq!(get("https://i.scdn.co/image/a").as_deref(), Some(&b"jpeg"[..]));
        assert_eq!(get("https://i.scdn.co/image/a").as_deref(), Some(&b"jpeg"[..]));
        assert_eq!(fetched, 1, "the second load reads the disk");
        // A failed download saves nothing.
        assert!(load(Some(&dir), "https://i.scdn.co/image/b", |_| None).is_none());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prune_removes_the_oldest_first() {
        let dir = std::env::temp_dir().join(format!("spotlite-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let now = std::time::SystemTime::now();
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            let path = dir.join(name);
            std::fs::write(&path, [0u8; 10]).unwrap();
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(now - Duration::from_secs(100 - i as u64 * 10)).unwrap();
        }
        prune(&dir, 20);
        let mut left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        left.sort();
        assert_eq!(left, ["mid", "new"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn disabled_never_loads() {
        let mut images = Images::disabled();
        assert!(images.get("https://example.com/a.jpg", 4, 2).is_none());
        assert!(!images.busy());
        assert_eq!(images.square_cols(2), 4);
    }
}
