//! Cover pictures. A background thread downloads, decodes, scales and encodes each picture for
//! the terminal's graphics protocol (kitty in ghostty, else sixel/iTerm2, else half blocks),
//! so the UI thread only places finished pictures.

use image::imageops::FilterType;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// Covers kept encoded in memory; a 4x2-cell thumbnail is a few KB.
const CACHE: usize = 300;
/// Spotify covers are well under this; it stops a bad URL from filling memory.
const MAX_BYTES: u64 = 4 << 20;

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
        thread::Builder::new()
            .name("spotlite-img".into())
            .spawn(move || {
                let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
                for key in key_rx {
                    let protocol = fetch(&agent, &key.0).and_then(|bytes| encode(&worker_picker, &bytes, key.1, key.2));
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
    fn disabled_never_loads() {
        let mut images = Images::disabled();
        assert!(images.get("https://example.com/a.jpg", 4, 2).is_none());
        assert!(!images.busy());
        assert_eq!(images.square_cols(2), 4);
    }
}
