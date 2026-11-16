//! spotlite's queue in the app: adding songs (`a`), the queue view (`A`) and its edits, and
//! the handoff that gives Spotify the next song shortly before the playing one ends.

use super::*;

/// The next song of the queue goes to Spotify when the playing one has this long left.
pub const HANDOFF_MS: u64 = 10_000;

impl App {
    /// `A`: shows the queue over the tab, or closes it.
    pub(super) fn toggle_queue(&mut self) {
        if self.queue_open {
            self.close_queue();
            return;
        }
        self.typing = false;
        self.filtering = false;
        self.marks.clear();
        self.hover = None;
        self.queue_open = true;
        // The first song that can move, else the top.
        let first = self.user_queue.locked().min(self.user_queue.len().saturating_sub(1));
        if self.user_queue.list.state.selected().is_none() || self.user_queue.list.selected() >= self.user_queue.len() {
            self.user_queue.list.state.select((!self.user_queue.is_empty()).then_some(first));
        }
    }

    pub(super) fn close_queue(&mut self) {
        self.queue_open = false;
        self.queue_add = None;
        self.marks.clear();
        self.hover = None;
        if let Some(Drag::QueueRow { .. }) = self.drag {
            self.drag = None;
        }
    }

    /// A queue row dragged with the mouse: (from, to).
    pub fn queue_drag(&self) -> Option<(usize, usize)> {
        match self.drag {
            Some(Drag::QueueRow { from, to }) if self.queue_open => Some((from, to)),
            _ => None,
        }
    }

    /// The songs Spotify plays after the queue (its own queue and the list that plays), without
    /// the song the queue gave it.
    pub fn then_from_spotify(&self) -> Vec<&Track> {
        let handed = self.user_queue.handed.as_deref();
        self.up_next.iter().chain(self.spotify_next.iter()).filter(|t| Some(t.uri.as_str()) != handed).collect()
    }

    /// The song that plays after this one: the queue's first, else what Spotify said.
    pub fn next_song(&self) -> Option<&Track> {
        self.user_queue.items().first().or(self.up_next.as_ref())
    }

    /// Adds songs at the end of the queue.
    pub(super) fn add_to_queue(&mut self, songs: Vec<Track>) {
        let n = songs.len();
        let Some(first) = songs.first().map(|t| t.name.clone()) else { return };
        self.user_queue.push(songs);
        let open = if self.queue_open { String::new() } else { format!(" · {} shows it", self.keys.show(Act::QueueView)) };
        let what = if n == 1 { first } else { format!("{n} songs") };
        self.info(format!("Added to queue: {what}{open}"));
    }

    /// The marked songs of the queue view (by index), else the selected one.
    fn queue_chosen(&self) -> Vec<usize> {
        let items = self.user_queue.items();
        if !self.marks.is_empty() {
            return (0..items.len()).filter(|&i| self.marks.contains(&items[i].uri)).collect();
        }
        self.user_queue.list.index(self.user_queue.list.selected()).into_iter().collect()
    }

    fn queue_saved(&mut self) {
        self.changes.push(Change::Queue(self.user_queue.snapshot()));
    }

    /// Tells that the first song cannot change any more.
    fn queue_locked_info(&mut self) {
        let name = self.user_queue.items().first().map(|t| t.name.clone()).unwrap_or_default();
        self.info(format!("{name} is with Spotify already: it plays next."));
    }

    /// `D` in the queue view: removes the marked songs, else the selected one.
    pub(super) fn queue_remove(&mut self) {
        let chosen = self.queue_chosen();
        if chosen.is_empty() {
            return;
        }
        let before = self.user_queue.snapshot();
        let gone = self.user_queue.remove(&chosen);
        self.marks.clear();
        if gone.is_empty() {
            self.queue_locked_info();
            return;
        }
        self.changes.push(Change::Queue(before));
        let what = if gone.len() == 1 { gone[0].name.clone() } else { format!("{} songs", gone.len()) };
        self.info(format!("Removed from the queue: {what} · {} undo", self.keys.show(Act::Undo)));
    }

    /// Moves the selected song up (`-1`) or down (`1`).
    pub(super) fn queue_move(&mut self, delta: isize) {
        let Some(i) = self.user_queue.list.index(self.user_queue.list.selected()) else { return };
        if self.user_queue.is_locked(i) {
            self.queue_locked_info();
            return;
        }
        let before = self.user_queue.snapshot();
        if self.user_queue.move_by(i, delta).is_some_and(|to| to != i) {
            self.changes.push(Change::Queue(before));
        }
    }

    /// Moves a song to another place (a mouse drag).
    pub(super) fn queue_move_to(&mut self, from: usize, to: usize) {
        if self.user_queue.is_locked(from) {
            self.queue_locked_info();
            return;
        }
        let before = self.user_queue.snapshot();
        if self.user_queue.move_to(from, to).is_some_and(|t| t != from) {
            self.changes.push(Change::Queue(before));
        }
    }

    /// The selected song goes first (after a song Spotify has already).
    pub(super) fn queue_to_top(&mut self) {
        let Some(i) = self.user_queue.list.index(self.user_queue.list.selected()) else { return };
        let top = self.user_queue.locked();
        if i > top {
            self.queue_move_to(i, top);
        }
    }

    pub(super) fn queue_clear(&mut self) {
        if self.user_queue.len() <= self.user_queue.locked() {
            return;
        }
        self.queue_saved();
        self.user_queue.clear();
        self.marks.clear();
        self.info(format!("Queue emptied · {} undo", self.keys.show(Act::Undo)));
    }

    pub(super) fn queue_shuffle(&mut self) {
        if self.user_queue.len() <= self.user_queue.locked() + 1 {
            self.info("Nothing to shuffle: add songs with a.");
            return;
        }
        self.queue_saved();
        self.user_queue.shuffle();
        let start = self.user_queue.locked();
        self.user_queue.list.state.select(Some(start));
        *self.user_queue.list.state.offset_mut() = 0;
        self.info(format!("Queue shuffled · {} undo", self.keys.show(Act::Undo)));
    }

    /// Enter in the queue view: the selected song plays now. It goes first and Spotify skips
    /// to it; a song Spotify has already plays first.
    pub(super) fn queue_play_now(&mut self) {
        let Some(i) = self.user_queue.list.index(self.user_queue.list.selected()) else { return };
        if !self.user_queue.is_locked(i) {
            let top = self.user_queue.locked();
            let before = self.user_queue.snapshot();
            if self.user_queue.move_to(i, top).is_some_and(|t| t != i) {
                self.changes.push(Change::Queue(before));
            }
            if top > 0 {
                let name = self.user_queue.items().get(top).map(|t| t.name.clone()).unwrap_or_default();
                self.info(format!("{name} plays after the song Spotify has already."));
            }
        }
        self.skip(true);
    }

    /// Gives Spotify the queue's next song when the playing one is about to end.
    pub(super) fn hand_off(&mut self) {
        if self.ghost || self.switch.is_some() || self.sample.is_some() {
            return;
        }
        let Some(pb) = &self.playback else { return };
        let Some(t) = &pb.item else { return };
        if !pb.is_playing || pb.repeat_state == "track" || t.duration_ms == 0 {
            return;
        }
        if t.duration_ms.saturating_sub(self.progress_ms()) > HANDOFF_MS {
            return;
        }
        let Some(next) = self.user_queue.to_hand().cloned() else { return };
        self.user_queue.mark_handed();
        self.send(Cmd::Queue { uri: next.uri, name: next.name, quiet: true });
    }

    /// `a` in the queue view: a search to add songs.
    pub(super) fn open_queue_add(&mut self) {
        self.queue_add = Some(QueueAdd::default());
    }

    pub(super) fn queue_add_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(p) = self.queue_add.as_mut() else { return };
        let step = |p: &mut QueueAdd, d: isize| {
            if !p.found.is_empty() {
                p.cursor = (p.cursor as isize + d).clamp(0, p.found.len() as isize - 1) as usize;
            }
        };
        match key.code {
            KeyCode::Esc => self.queue_add = None,
            KeyCode::Down => step(p, 1),
            KeyCode::Up => step(p, -1),
            KeyCode::Char('n') if ctrl => step(p, 1),
            KeyCode::Char('p') if ctrl => step(p, -1),
            KeyCode::Enter => {
                let q = p.query.trim().to_owned();
                if q.is_empty() {
                    return;
                }
                if q != p.searched {
                    p.searched = q.clone();
                    p.found.clear();
                    p.cursor = 0;
                    p.loading = true;
                    self.send(Cmd::QueueSearch(q));
                } else if let Some(t) = p.found.get(p.cursor).cloned() {
                    self.add_to_queue(vec![t]);
                }
            }
            _ => {
                p.query.edit(&key);
            }
        }
    }

    /// A click on a row of the add picker adds that song.
    pub(super) fn queue_add_pick(&mut self, i: usize) {
        let Some(p) = self.queue_add.as_mut() else { return };
        p.cursor = i;
        if let Some(t) = p.found.get(i).cloned() {
            self.add_to_queue(vec![t]);
        }
    }

    pub(super) fn on_queue_found(&mut self, query: String, found: Vec<Track>) {
        if let Some(p) = self.queue_add.as_mut().filter(|p| p.searched == query) {
            p.found = found;
            p.cursor = 0;
            p.loading = false;
        }
    }

    /// Where a dragged queue row would go: the row under `y`, kept below a locked song.
    pub(super) fn queue_drop_at(&self, y: u16) -> usize {
        let h = &self.hits;
        let len = self.user_queue.len();
        let row = if y < h.list.y {
            h.list_offset.saturating_sub(1)
        } else {
            h.list_offset + ((y - h.list.y) / h.row_height.max(1)) as usize
        };
        row.clamp(self.user_queue.locked(), len.saturating_sub(1))
    }
}
