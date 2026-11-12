//! The mouse: clicks, drags, the wheel, and hover.

use super::*;

impl App {
    /// Row index under a point in the list, if there is an item there.
    pub(super) fn row_at(&self, pos: Position) -> Option<usize> {
        let h = &self.hits;
        if !h.list.contains(pos) || h.row_height == 0 {
            return None;
        }
        let i = h.list_offset + ((pos.y - h.list.y) / h.row_height) as usize;
        (i < self.list_len()).then_some(i)
    }

    pub(super) fn seek_to_fraction(&mut self, f: f64) -> Option<u64> {
        let duration = self.playback.as_ref()?.item.as_ref()?.duration_ms;
        Some((f * duration as f64) as u64)
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        if self.help {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.help = false;
            }
            return;
        }
        if self.tabs_mouse(m, pos) {
            return;
        }
        // Pop-ups take the mouse while they are open; a click outside closes them.
        if self.genre_picker.is_some() {
            let genres = std::mem::take(&mut self.genre_list);
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    match self.hits.popup_rows.iter().find(|(r, _)| r.contains(pos)).map(|(_, i)| *i) {
                        Some(i) => self.genre_choose(&genres, i),
                        None if !self.hits.popup.contains(pos) => {
                            self.genre_picker = None;
                            self.stop_sample();
                        }
                        None => {}
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(p) = self.genre_picker.as_mut() {
                        p.move_by(&genres, 3);
                    }
                }
                MouseEventKind::ScrollUp => {
                    if let Some(p) = self.genre_picker.as_mut() {
                        p.move_by(&genres, -3);
                    }
                }
                _ => {}
            }
            self.genre_list = genres;
            return;
        }
        if self.menu.is_some() || self.picker.is_some() || self.confirm.is_some() || self.palette.is_some() || self.undo.is_some() {
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let hit = self.hits.popup_rows.iter().find(|(r, _)| r.contains(pos)).map(|(_, i)| *i);
                    match hit {
                        Some(i) if self.undo.is_some() => {
                            self.undo = Some(i);
                            self.undo_go();
                        }
                        Some(i) if self.palette.is_some() => {
                            if let Some(p) = self.palette.as_mut() {
                                p.cursor = i;
                            }
                            self.run_palette();
                        }
                        Some(i) if self.menu.is_some() => {
                            if let Some(menu) = self.menu.as_mut() {
                                menu.cursor = i;
                            }
                            self.run_menu();
                        }
                        Some(i) if self.picker.is_some() => {
                            if let Some(p) = self.picker.as_mut() {
                                p.cursor = i;
                            }
                            self.pick_playlist();
                        }
                        Some(0) => {
                            if let Some(c) = self.confirm.take() {
                                self.send(c.cmd);
                            }
                        }
                        _ if !self.hits.popup.contains(pos) || self.confirm.is_some() => {
                            self.close_menu();
                            self.palette = None;
                            self.picker = None;
                            self.confirm = None;
                            self.undo = None;
                        }
                        _ => {}
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    let delta: isize = if m.kind == MouseEventKind::ScrollUp { -1 } else { 1 };
                    self.move_popup(delta);
                }
                _ => {}
            }
            return;
        }
        if !self.sidebar.open && self.hits.side_open.contains(pos) && matches!(m.kind, MouseEventKind::Down(_)) {
            self.show_side(self.sidebar.view);
            return;
        }
        // The hide handle sticks out of the sidebar into the gap: check it first.
        if self.sidebar.open && self.hits.side_hide.contains(pos) && matches!(m.kind, MouseEventKind::Down(_)) {
            self.sidebar.close();
            return;
        }
        if self.sidebar.open && self.hits.side.contains(pos) {
            self.side_mouse(m, pos);
            return;
        }
        if matches!(m.kind, MouseEventKind::Down(_)) {
            // A click in the list gives the keys back to it.
            self.sidebar.focus = false;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(i) = self.row_at(pos) {
                    self.select(i);
                    self.open_menu();
                    if let Some(m) = self.menu.as_mut() {
                        m.at = Some(pos);
                    }
                } else if self.hits.np_artist.contains(pos) || self.hits.np_album.contains(pos) {
                    if let Some(t) = self.playback.as_ref().and_then(|p| p.item.clone()) {
                        let items_track = t;
                        self.menu = None;
                        self.run_action(Action::Radio, items_track);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => self.on_click(pos),
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Some(Drag::Volume) => self.set_volume((fraction(self.hits.volume, m.column) * 100.0).round() as u8),
                Some(Drag::Progress) => {
                    self.seek_preview = self.seek_to_fraction(fraction(self.hits.progress, m.column));
                }
                Some(Drag::Scroll) => self.scroll_to(m.row),
                None => {}
            },
            MouseEventKind::Up(MouseButton::Left) => {
                if self.drag.take() == Some(Drag::Progress) {
                    if let Some(ms) = self.seek_preview.take() {
                        self.seek_absolute(ms);
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if self.hits.volume.contains(pos) {
                    self.change_volume(up);
                } else if self.hits.progress.contains(pos) {
                    self.seek(up);
                } else {
                    self.move_selection(if up { -WHEEL_ROWS } else { WHEEL_ROWS });
                }
            }
            _ => {}
        }
    }

    pub(super) fn on_click(&mut self, pos: Position) {
        if self.hits.time.contains(pos) {
            self.remaining = !self.remaining;
            return;
        }
        let h = &self.hits;
        if let Some(&(_, kind)) = h.search_kinds.iter().find(|(r, _)| r.contains(pos)) {
            self.set_search_kind(kind);
        } else if let Some((_, loc)) = h.tabs.iter().find(|(r, _)| r.contains(pos)) {
            let loc = loc.clone();
            self.go(loc);
        } else if h.search_box.contains(pos) {
            self.switch_tab(Tab::Search);
            self.typing = true;
        } else if h.back.contains(pos) {
            self.back();
        } else if h.np_artist.contains(pos) {
            if let Some(t) = self.playback.as_ref().and_then(|p| p.item.clone()) {
                self.go_to_artist(&t);
            }
        } else if h.np_album.contains(pos) {
            if let Some(t) = self.playback.as_ref().and_then(|p| p.item.clone()) {
                self.go_to_album(&t);
            }
        } else if h.play.contains(pos) {
            self.toggle_play();
        } else if h.prev.contains(pos) {
            self.skip(false);
        } else if h.next.contains(pos) {
            self.skip(true);
        } else if h.shuffle.contains(pos) {
            self.toggle_shuffle();
        } else if h.repeat.contains(pos) {
            self.cycle_repeat();
        } else if h.like.contains(pos) {
            // The heart in the player is for the playing song.
            let playing = self.playback.as_ref().and_then(|p| p.item.clone());
            if playing.is_some() {
                self.toggle_like(playing);
            }
        } else if h.volume.contains(pos) {
            self.drag = Some(Drag::Volume);
            self.set_volume((fraction(h.volume, pos.x) * 100.0).round() as u8);
        } else if h.progress.contains(pos) {
            self.drag = Some(Drag::Progress);
            self.seek_preview = self.seek_to_fraction(fraction(h.progress, pos.x));
        } else if h.scrollbar.contains(pos) {
            self.drag = Some(Drag::Scroll);
            self.scroll_to(pos.y);
        } else if let Some(i) = self.row_at(pos) {
            self.typing = false;
            let double = self
                .last_click
                .is_some_and(|(last, at)| last == i && at.elapsed() < DOUBLE_CLICK);
            self.select(i);
            if double {
                self.last_click = None;
                self.play_selected();
            } else {
                self.last_click = Some((i, Instant::now()));
            }
        } else if self.typing {
            self.typing = false;
        }
    }

    /// The pointer moved (no button). True if the screen changes: the list opens, closes, or
    /// another row is under the pointer.
    pub fn on_hover(&mut self, pos: Position) -> bool {
        // The list row under the pointer (only when no pop-up covers the list).
        let popup = self.menu.is_some() || self.picker.is_some() || self.palette.is_some() || self.genre_picker.is_some() || self.help;
        let row = if popup { None } else { self.row_at(pos) };
        let row_changed = row != self.hover;
        self.hover = row;
        row_changed | self.hover_tabs(pos)
    }

    pub(super) fn hover_tabs(&mut self, pos: Position) -> bool {
        let over_chip = self.hits.tabs_more.contains(pos);
        let over_list = self.hits.tabs_list.contains(pos);
        let row = self.hits.tabs_rows.iter().find(|(r, _)| r.contains(pos)).map(|(_, i)| *i);
        match self.tabs_list.as_mut() {
            None if over_chip => {
                self.tabs_list = Some(TabsList { pinned: false, hover: None });
                true
            }
            None => false,
            Some(l) if over_chip || over_list => {
                let changed = l.hover != row;
                l.hover = row;
                changed
            }
            Some(l) if !l.pinned => {
                self.tabs_list = None;
                true
            }
            Some(l) => {
                let changed = l.hover.is_some();
                l.hover = None;
                changed
            }
        }
    }

    /// A click on the chip or in its list. True if it was used (nothing else gets it).
    pub(super) fn tabs_mouse(&mut self, m: MouseEvent, pos: Position) -> bool {
        if !matches!(m.kind, MouseEventKind::Down(_)) {
            return self.tabs_list.is_some() && self.hits.tabs_list.contains(pos);
        }
        if self.hits.tabs_more.contains(pos) {
            // A click keeps it open (or shuts it if it was kept open).
            self.tabs_list = match self.tabs_list {
                Some(l) if l.pinned => None,
                _ => Some(TabsList { pinned: true, hover: None }),
            };
            return true;
        }
        if self.tabs_list.is_none() {
            return false;
        }
        if let Some(&(_, i)) = self.hits.tabs_close.iter().find(|(r, _)| r.contains(pos)) {
            self.close_page_at(i);
            if self.pages.is_empty() {
                self.tabs_list = None;
            }
            return true;
        }
        if let Some(&(_, i)) = self.hits.tabs_rows.iter().find(|(r, _)| r.contains(pos)) {
            self.tabs_list = None;
            if let Some(key) = self.pages.get(i).map(Page::key) {
                self.go(Loc::Page(key));
            }
            return true;
        }
        // A click anywhere else shuts the list and does what it does there.
        let inside = self.hits.tabs_list.contains(pos);
        self.tabs_list = None;
        inside
    }

    /// Closes page `i`, shown or not.
    pub(super) fn close_page_at(&mut self, i: usize) {
        if self.active == Some(i) {
            self.close_page();
            return;
        }
        if i < self.pages.len() {
            let key = self.pages.remove(i).key();
            self.unloaded.remove(&key);
            if let Some(a) = self.active.filter(|a| *a > i) {
                self.active = Some(a - 1);
            }
        }
    }
}
