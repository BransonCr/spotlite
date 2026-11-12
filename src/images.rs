//! Cover pictures. Background threads download, decode, scale and encode each picture for
//! the terminal's graphics protocol (kitty in ghostty, else sixel/iTerm2, else half blocks),
//! so the UI thread only places finished pictures.

use image::imageops::FilterType;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;
use std::collections::HashMap;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// Covers kept encoded in memory; a 4x2-cell thumbnail is a few KB.
const CACHE: usize = 300;
/// Spotify covers are well under this; it stops a bad URL from filling memory.
const MAX_BYTES: u64 = 4 << 20;
/// Downloaded covers kept on disk (`~/.cache/spotlite/covers`), so they show at once after a
/// restart. Spotify cover URLs name their content, so a saved file never goes stale.
const DISK_LIMIT: u64 = 64 << 20;

/// A picture at one size in cells: the URL's hash, columns, rows.
type Key = (u64, u16, u16);

/// The colours of a cover: `SWATCH` × `SWATCH` points of it, row by row. The sidebar's aura
/// takes its colours from them.
pub const SWATCH: usize = 8;
pub type Swatch = [[u8; 3]; SWATCH * SWATCH];

/// A picture ready for the terminal, and its colours.
type Loaded = (Protocol, Box<Swatch>);

/// Download threads: covers of a fresh list come in parallel.
const THREADS: usize = 3;

enum Slot {
    Pending,
    Ready(Protocol, Box<Swatch>),
    Failed,
}

/// Pictures asked for and not started yet, newest last (the newest go first: they are the ones
/// on screen now).
#[derive(Default)]
struct Queue {
    jobs: Vec<(Key, String)>,
    closed: bool,
}

pub struct Images {
    picker: Option<Picker>,
    queue: Option<Arc<(Mutex<Queue>, Condvar)>>,
    rx: Option<Receiver<(Key, Option<Loaded>)>>,
    /// Each picture, and the draw it was last used in (the least recent goes first).
    cache: HashMap<Key, (Slot, u64)>,
    /// The draw count: `poll` moves it on.
    frame: u64,
    pending: usize,
}

fn hash(url: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    h.finish()
}

impl Images {
    pub fn new(picker: Picker) -> Self {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let (img_tx, img_rx) = channel();
        let dir = dirs::cache_dir().map(|d| d.join("spotlite").join("covers"));
        if let Some(dir) = &dir {
            let dir = dir.clone();
            let _ = thread::Builder::new().name("spotlite-img-prune".into()).spawn(move || crate::store::prune_dir(&dir, DISK_LIMIT));
        }
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
        for n in 0..THREADS {
            let (queue, img_tx, picker, dir, agent) = (queue.clone(), img_tx.clone(), picker.clone(), dir.clone(), agent.clone());
            thread::Builder::new()
                .name(format!("spotlite-img{n}"))
                .spawn(move || loop {
                    let job = {
                        let (lock, wake) = &*queue;
                        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if q.closed {
                                return;
                            }
                            if let Some(job) = q.jobs.pop() {
                                break job;
                            }
                            q = wake.wait(q).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    let ((_, cols, rows), url) = (job.0, &job.1);
                    let protocol = load(dir.as_deref(), url, |url| fetch(&agent, url)).and_then(|bytes| encode(&picker, &bytes, cols, rows));
                    if img_tx.send((job.0, protocol)).is_err() {
                        return;
                    }
                })
                .expect("cannot start an image thread");
        }
        Images { picker: Some(picker), queue: Some(queue), rx: Some(img_rx), cache: HashMap::new(), frame: 0, pending: 0 }
    }

    /// No pictures at all (tests, or `SPOTLITE_NO_IMAGES=1`).
    pub fn disabled() -> Self {
        Images { picker: None, queue: None, rx: None, cache: HashMap::new(), frame: 0, pending: 0 }
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
        let queue = self.queue.as_ref()?;
        let key = (hash(url), cols, rows);
        let frame = self.frame;
        if !self.cache.contains_key(&key) {
            let (lock, wake) = &**queue;
            lock.lock().unwrap_or_else(|e| e.into_inner()).jobs.push((key, url.to_owned()));
            wake.notify_one();
            self.pending += 1;
            self.cache.insert(key, (Slot::Pending, frame));
            self.evict();
        }
        match self.cache.get_mut(&key) {
            Some((slot, used)) => {
                *used = frame;
                match slot {
                    Slot::Ready(p, _) => Some(p),
                    _ => None,
                }
            }
            None => None,
        }
    }

    /// The colours of a cover that is loaded (at any size), or None.
    pub fn swatch(&self, url: &str) -> Option<Swatch> {
        let h = hash(url);
        self.cache.iter().find_map(|(key, (slot, _))| match slot {
            Slot::Ready(_, s) if key.0 == h => Some(**s),
            _ => None,
        })
    }

    /// Takes finished pictures from the threads. Returns true if any arrived.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut any = false;
        while let Ok((key, protocol)) = rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            any = true;
            // It may have been evicted while loading; then it is not wanted any more.
            if let Some((slot, _)) = self.cache.get_mut(&key) {
                *slot = protocol.map_or(Slot::Failed, |(p, s)| Slot::Ready(p, s));
            }
        }
        any
    }

    /// Starts a draw: pictures still waiting that the last draw did not use (a list scrolled
    /// past them) are not loaded.
    pub fn begin_frame(&mut self) {
        if let Some(queue) = &self.queue {
            let frame = self.frame;
            let cache = &mut self.cache;
            let mut dropped = 0;
            let mut q = queue.0.lock().unwrap_or_else(|e| e.into_inner());
            q.jobs.retain(|(key, _)| {
                let wanted = cache.get(key).is_some_and(|(_, used)| *used >= frame);
                if !wanted {
                    cache.remove(key);
                    dropped += 1;
                }
                wanted
            });
            self.pending = self.pending.saturating_sub(dropped);
        }
        self.frame += 1;
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    /// Removes the pictures used least recently, down to `CACHE`.
    fn evict(&mut self) {
        let over = self.cache.len().saturating_sub(CACHE);
        if over == 0 {
            return;
        }
        let mut uses: Vec<(u64, Key)> = self
            .cache
            .iter()
            .filter(|(_, (slot, _))| !matches!(slot, Slot::Pending))
            .map(|(k, (_, used))| (*used, *k))
            .collect();
        uses.sort_unstable();
        for (_, key) in uses.into_iter().take(over + CACHE / 10) {
            self.cache.remove(&key);
        }
    }
}

impl Drop for Images {
    fn drop(&mut self) {
        if let Some(queue) = &self.queue {
            queue.0.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
            queue.1.notify_all();
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

fn fetch(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let resp = agent.get(url).call().ok()?;
    let mut bytes = Vec::new();
    resp.into_reader().take(MAX_BYTES).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Decodes a JPEG/PNG and encodes it to fill `cols` x `rows` cells. Also gives its colours.
pub fn encode(picker: &Picker, bytes: &[u8], cols: u16, rows: u16) -> Option<Loaded> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.thumbnail_exact(SWATCH as u32, SWATCH as u32).to_rgb8();
    let mut swatch = Box::new([[0u8; 3]; SWATCH * SWATCH]);
    for (slot, px) in swatch.iter_mut().zip(small.pixels()) {
        *slot = px.0;
    }
    let protocol = picker.new_protocol(img, Rect::new(0, 0, cols, rows), Resize::Scale(Some(FilterType::Triangle))).ok()?;
    Some((protocol, swatch))
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
        let (protocol, swatch) = encode(&Picker::halfblocks(), &png, 4, 2).expect("encodes");
        assert!(matches!(protocol, Protocol::Halfblocks(_)));
        assert!(swatch[0][0] < swatch[SWATCH - 1][0], "the colours go left to right: {swatch:?}");
        assert_eq!(swatch[0][1], 100);
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
        crate::store::prune_dir(&dir, 20);
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
