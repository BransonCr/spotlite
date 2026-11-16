//! spotlite's own queue. Spotify's Web API can add a song to its queue but cannot remove,
//! move or shuffle one, so spotlite keeps the order itself and gives Spotify only the next
//! song, a few seconds before the playing one ends. Until then every song can still change
//! place. The song given to Spotify (the head) is locked: it plays next whatever happens here.

use crate::api::Track;
use crate::app::Listing;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

/// The queue's file in the cache folder.
const FILE: &str = "queue.json";
/// Song changes after which a song given to Spotify that never played is not waited for any
/// more (the queue was cleared on another device).
const MISSED_LIMIT: u8 = 3;

#[derive(Default)]
pub struct Queue {
    /// The songs in the order they play, with the view's selection and scroll.
    pub list: Listing<Track>,
    /// The URI of the first song, once it was given to Spotify.
    pub handed: Option<String>,
    /// Song changes since the handed song was given, without it playing.
    missed: u8,
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    items: Vec<Track>,
    handed: Option<String>,
}

impl Queue {
    /// The queue saved at the last change.
    pub fn load() -> Queue {
        let saved: Saved = crate::store::read(FILE).unwrap_or_default();
        let mut q = Queue { handed: saved.handed, ..Default::default() };
        q.list.items = saved.items;
        q.fix_handed();
        q.list.total = Some(q.list.items.len());
        q.list.changed();
        q
    }

    fn save(&self) {
        crate::store::write(FILE, &Saved { items: self.list.items.clone(), handed: self.handed.clone() });
    }

    /// The songs changed: the lock, the list's rows and the file follow.
    fn changed(&mut self) {
        self.fix_handed();
        self.list.total = Some(self.list.items.len());
        self.list.changed();
        self.save();
    }

    /// The handed song is always the first one.
    fn fix_handed(&mut self) {
        if self.list.items.first().map(|t| &t.uri) != self.handed.as_ref() {
            self.handed = None;
            self.missed = 0;
        }
    }

    pub fn items(&self) -> &[Track] {
        &self.list.items
    }

    pub fn len(&self) -> usize {
        self.list.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.items.is_empty()
    }

    /// How many songs at the top cannot move: 1 when the first was given to Spotify.
    pub fn locked(&self) -> usize {
        usize::from(self.handed.is_some())
    }

    pub fn is_locked(&self, i: usize) -> bool {
        i < self.locked()
    }

    pub fn total_ms(&self) -> u64 {
        self.list.items.iter().map(|t| t.duration_ms).sum()
    }

    /// Adds songs at the end.
    pub fn push(&mut self, tracks: impl IntoIterator<Item = Track>) {
        self.list.items.extend(tracks);
        self.changed();
    }

    /// Removes the songs at these indexes (a locked one stays). Returns the songs removed.
    pub fn remove(&mut self, indexes: &[usize]) -> Vec<Track> {
        let locked = self.locked();
        let mut gone = Vec::new();
        let mut i = 0;
        self.list.items.retain(|t| {
            let drop = i >= locked && indexes.contains(&i);
            if drop {
                gone.push(t.clone());
            }
            i += 1;
            !drop
        });
        if !gone.is_empty() {
            self.changed();
        }
        gone
    }

    /// Moves the song at `from` to `to` (both are clamped below the locked head). The
    /// selection follows the song. Returns where it went.
    pub fn move_to(&mut self, from: usize, to: usize) -> Option<usize> {
        let locked = self.locked();
        let len = self.len();
        if from < locked || from >= len {
            return None;
        }
        let to = to.clamp(locked, len - 1);
        if to != from {
            let t = self.list.items.remove(from);
            self.list.items.insert(to, t);
            self.changed();
        }
        self.list.state.select(self.list.pos_of(to));
        Some(to)
    }

    /// Moves the song at `i` one place up (`-1`) or down (`1`).
    pub fn move_by(&mut self, i: usize, delta: isize) -> Option<usize> {
        let to = (i as isize + delta).max(0) as usize;
        self.move_to(i, to)
    }

    /// Puts the songs after the locked head in a random order.
    pub fn shuffle(&mut self) {
        let locked = self.locked();
        if self.len() > locked + 1 {
            self.list.items[locked..].shuffle(&mut rand::thread_rng());
            self.changed();
        }
    }

    /// The song to give Spotify next, if there is one and none was given yet.
    pub fn to_hand(&self) -> Option<&Track> {
        if self.handed.is_some() {
            return None;
        }
        self.list.items.first()
    }

    /// The first song was given to Spotify: it is locked until it plays.
    pub fn mark_handed(&mut self) {
        self.handed = self.list.items.first().map(|t| t.uri.clone());
        self.missed = 0;
        self.save();
    }

    /// A new song plays. The handed song leaves the queue when it starts; a handed song that
    /// does not come after a few changes is not waited for. Returns true when the queue
    /// changed.
    pub fn on_song(&mut self, uri: &str) -> bool {
        let Some(handed) = &self.handed else { return false };
        if handed == uri {
            self.list.items.remove(0);
            self.handed = None;
            self.missed = 0;
            self.changed();
            return true;
        }
        self.missed += 1;
        if self.missed >= MISSED_LIMIT {
            self.handed = None;
            self.missed = 0;
            self.save();
            return true;
        }
        false
    }

    /// The songs, for undo.
    pub fn snapshot(&self) -> Vec<Track> {
        self.list.items.clone()
    }

    /// Puts back songs from `snapshot` (the handed song stays first if it is still there).
    pub fn restore(&mut self, mut items: Vec<Track>) {
        if let Some(h) = &self.handed {
            items.retain(|t| &t.uri != h);
            if let Some(first) = self.list.items.first().cloned() {
                items.insert(0, first);
            }
        }
        self.list.items = items;
        self.changed();
    }

    /// Empties the queue (a handed song stays: Spotify plays it anyway).
    pub fn clear(&mut self) {
        self.list.items.truncate(self.locked());
        self.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(n: &str) -> Track {
        Track {
            uri: format!("spotify:track:{n}"),
            name: n.into(),
            duration_ms: 1000,
            artists: Vec::new(),
            album: None,
            show: None,
            images: Vec::new(),
        }
    }

    fn names(q: &Queue) -> String {
        q.items().iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join("")
    }

    fn queue(s: &str) -> Queue {
        let mut q = Queue::default();
        q.push(s.chars().map(|c| song(&c.to_string())));
        q
    }

    #[test]
    fn moves_stay_in_range_and_the_selection_follows() {
        let mut q = queue("abcd");
        assert_eq!(q.move_to(0, 2), Some(2));
        assert_eq!(names(&q), "bcad");
        assert_eq!(q.list.selected(), 2);
        assert_eq!(q.move_by(2, 5), Some(3));
        assert_eq!(names(&q), "bcda");
        assert_eq!(q.move_to(9, 0), None);
        assert_eq!(q.move_by(1, -9), Some(0));
        assert_eq!(names(&q), "cbda");
    }

    #[test]
    fn the_handed_head_is_locked() {
        let mut q = queue("abcd");
        q.mark_handed();
        assert!(q.is_locked(0));
        assert_eq!(q.to_hand(), None);
        assert_eq!(q.move_to(0, 3), None);
        assert_eq!(q.move_to(3, 0), Some(1));
        assert_eq!(names(&q), "adbc");
        assert!(q.remove(&[0, 1]).len() == 1);
        assert_eq!(names(&q), "abc");
        for _ in 0..20 {
            q.shuffle();
            assert_eq!(&names(&q)[..1], "a");
        }
        q.clear();
        assert_eq!(names(&q), "a");
    }

    #[test]
    fn the_handed_song_leaves_when_it_plays() {
        let mut q = queue("abc");
        q.mark_handed();
        assert!(!q.on_song("spotify:track:x"));
        assert!(q.on_song("spotify:track:a"));
        assert_eq!(names(&q), "bc");
        assert_eq!(q.to_hand().map(|t| t.name.as_str()), Some("b"));
    }

    #[test]
    fn a_handed_song_that_never_plays_is_let_go() {
        let mut q = queue("ab");
        q.mark_handed();
        for _ in 0..MISSED_LIMIT - 1 {
            assert!(!q.on_song("spotify:track:x"));
        }
        assert!(q.on_song("spotify:track:x"));
        assert_eq!(q.locked(), 0);
        assert_eq!(names(&q), "ab");
    }

    #[test]
    fn restore_keeps_the_handed_song_first() {
        let mut q = queue("abc");
        let before = q.snapshot();
        q.mark_handed();
        q.remove(&[1, 2]);
        q.restore(before);
        assert_eq!(names(&q), "abc");
        assert!(q.is_locked(0));
    }
}
