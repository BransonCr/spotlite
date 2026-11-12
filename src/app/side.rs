//! The sidebar: the playing song and your library.

use super::*;

impl App {
    /// The song that plays now, if any.
    pub(super) fn playing_track(&self) -> Option<Track> {
        self.playback.as_ref()?.item.clone()
    }

    /// The rows of a sidebar view.
    pub fn side_items(&self, view: SideView) -> Vec<SideItem> {
        match view {
            SideView::Playing => {
                let Some(t) = self.playing_track() else { return Vec::new() };
                let mut acts = vec![SongAct::Like, SongAct::AddToPlaylist];
                if crate::client::AVAILABLE && t.uri.starts_with("spotify:track:") {
                    acts.push(SongAct::Radio);
                }
                if t.album.as_ref().is_some_and(|a| a.id.is_some()) {
                    acts.push(SongAct::Album);
                }
                if t.artists.iter().any(|a| a.id().is_some()) {
                    acts.push(SongAct::Artist);
                }
                acts.extend([SongAct::Pin, SongAct::Card]);
                acts.into_iter().map(SideItem::Act).collect()
            }
            SideView::Library => {
                let mut items = Vec::new();
                if !self.pins.is_empty() {
                    items.push(SideItem::Header("Pinned".into()));
                    items.extend((0..self.pins.len()).map(SideItem::Pin));
                }
                items.push(SideItem::Header("Library".into()));
                items.push(SideItem::Liked);
                let n = self.playlists.items.len();
                items.push(SideItem::Header(match self.playlists.total {
                    Some(t) => format!("Playlists · {t}"),
                    None => "Playlists".into(),
                }));
                if n == 0 {
                    items.push(SideItem::Note(if self.playlists.total.is_none() { "Loading…" } else { "No playlists" }));
                }
                items.extend((0..n).map(SideItem::Playlist));
                items
            }
        }
    }

    /// True if the window has room for the sidebar; else says so.
    pub(super) fn side_fits(&mut self) -> bool {
        if self.sidebar.open || self.screen_w == 0 || Sidebar::full_width(self.screen_w).is_some() {
            return true;
        }
        self.info(format!("The window is too narrow for the sidebar ({} columns needed).", crate::sidebar::MIN_SCREEN));
        false
    }

    /// Opens or shuts the sidebar (`\`, Ctrl+B).
    pub(super) fn toggle_sidebar(&mut self) {
        if self.side_fits() {
            self.sidebar.toggle();
            self.fix_side_cursor();
        }
    }

    pub(super) fn show_side(&mut self, view: SideView) {
        if self.side_fits() {
            self.sidebar.show(view);
            self.fix_side_cursor();
        }
    }

    pub(super) fn fix_side_cursor(&mut self) {
        let items = self.side_items(self.sidebar.view);
        self.sidebar.fix_cursor(&items);
    }

    /// Keys for the sidebar. True if the key was used.
    pub(super) fn side_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if !(self.sidebar.open && self.sidebar.focus) {
            return false;
        }
        let view = self.sidebar.view;
        let items = self.side_items(view);
        match key.code {
            KeyCode::Esc => self.sidebar.focus = false,
            KeyCode::Down | KeyCode::Char('j') => self.sidebar.move_cursor(&items, 1),
            KeyCode::Up | KeyCode::Char('k') => self.sidebar.move_cursor(&items, -1),
            KeyCode::Char('g') | KeyCode::Home => self.sidebar.move_cursor(&items, -10_000),
            KeyCode::Char('G') | KeyCode::End => self.sidebar.move_cursor(&items, 10_000),
            KeyCode::Char('h') | KeyCode::Left => self.show_side(SideView::Playing),
            KeyCode::Char('l') | KeyCode::Right => self.show_side(SideView::Library),
            KeyCode::Tab | KeyCode::BackTab => self.sidebar.switch(),
            KeyCode::Enter => {
                let i = self.sidebar.cursor(view);
                self.side_run(i);
            }
            KeyCode::Char('*') if view == SideView::Library => match items.get(self.sidebar.cursor(view)) {
                Some(SideItem::Pin(p)) => {
                    let p = *p;
                    self.unpin(p);
                }
                Some(SideItem::Playlist(p)) => {
                    if let Some(pl) = self.playlists.items.get(*p).cloned() {
                        self.toggle_pin(Pin::Playlist { uri: pl.uri, name: pl.name, images: pl.images });
                    }
                }
                _ => {}
            },
            KeyCode::Char(c) if view == SideView::Playing && !ctrl => {
                let hit = items
                    .iter()
                    .position(|it| matches!(it, SideItem::Act(a) if a.key().starts_with(c) && a.key().len() == c.len_utf8()));
                match hit {
                    Some(i) => self.side_run(i),
                    // Other keys (space, n, +, …) work as always.
                    None => return false,
                }
            }
            _ => return false,
        }
        true
    }

    /// Runs row `i` of the sidebar view shown: a song action, or opens a library entry.
    pub(super) fn side_run(&mut self, i: usize) {
        let view = self.sidebar.view;
        let items = self.side_items(view);
        let Some(item) = items.get(i).cloned() else { return };
        self.sidebar.set_cursor(view, i);
        match item {
            SideItem::Act(act) => {
                let Some(t) = self.playing_track() else { return };
                match act {
                    SongAct::Like => self.toggle_like(Some(t)),
                    SongAct::AddToPlaylist => self.open_picker(t),
                    SongAct::Radio => self.start_radio(t),
                    SongAct::Album => self.go_to_album(&t),
                    SongAct::Artist => self.go_to_artist(&t),
                    SongAct::Pin => self.toggle_pin(Pin::Song { track: t }),
                    SongAct::Card => self.share_card(t),
                }
            }
            SideItem::Pin(p) => match self.pins.get(p).cloned() {
                Some(Pin::Song { track }) => {
                    self.info(format!("Playing {}", track.name));
                    let uris = vec![track.uri.clone()];
                    self.play(Cmd::Play(Play::Tracks { uris, offset: 0 }), Some(track));
                }
                Some(Pin::Album { album }) => self.open_album(album),
                Some(Pin::Playlist { uri, name, .. }) => self.open_playlist_uri(&uri, &name),
                None => {}
            },
            SideItem::Liked => {
                self.open = None;
                self.switch_tab(Tab::Liked);
            }
            SideItem::Playlist(p) => {
                if let Some(pl) = self.playlists.items.get(p).cloned() {
                    self.open_playlist_uri(&pl.uri, &pl.name);
                }
            }
            SideItem::Header(_) | SideItem::Note(_) => {}
        }
    }

    /// Opens your playlist in the Playlists tab; plays one you do not own (Spotify does not
    /// list its songs to spotlite).
    pub(super) fn open_playlist_uri(&mut self, uri: &str, name: &str) {
        match self.playlists.items.iter().find(|p| p.uri == uri).cloned() {
            Some(pl) if self.owns(&pl) || crate::client::AVAILABLE => {
                self.open_playlist(pl);
                self.switch_tab(Tab::Playlists);
            }
            _ => {
                self.info(format!("Playing {name}"));
                self.play(Cmd::Play(Play::Context { uri: uri.to_owned(), from: None }), None);
            }
        }
    }

    pub(super) fn side_mouse(&mut self, m: MouseEvent, pos: Position) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.sidebar.focus = true;
                if let Some(i) = self.hits.side_tabs.iter().position(|r| r.contains(pos)) {
                    self.show_side(if i == 0 { SideView::Playing } else { SideView::Library });
                } else if let Some(&(_, i)) = self.hits.side_rows.iter().find(|(r, _)| r.contains(pos)) {
                    self.side_run(i);
                } else if self.hits.np_album.contains(pos) {
                    if let Some(t) = self.playing_track() {
                        self.go_to_album(&t);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click a library row: pin or unpin it.
                self.sidebar.focus = true;
                if let Some(&(_, i)) = self.hits.side_rows.iter().find(|(r, _)| r.contains(pos)) {
                    if self.sidebar.view == SideView::Library {
                        self.sidebar.set_cursor(SideView::Library, i);
                        self.side_key(KeyEvent::from(KeyCode::Char('*')));
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let items = self.side_items(self.sidebar.view);
                self.sidebar.move_cursor(&items, if m.kind == MouseEventKind::ScrollUp { -1 } else { 1 });
            }
            _ => {}
        }
    }

    /// Pins it, or unpins it if it is pinned.
    pub(super) fn toggle_pin(&mut self, pin: Pin) {
        let uri = pin.uri();
        if let Some(i) = self.pins.iter().position(|p| p.uri() == uri) {
            self.unpin(i);
        } else {
            self.info(format!("Pinned {} to the library", pin.name()));
            self.pins.insert(0, pin);
        }
    }

    pub(super) fn unpin(&mut self, i: usize) {
        if i < self.pins.len() {
            let pin = self.pins.remove(i);
            self.info(format!("Unpinned {} · {} undo", pin.name(), self.keys.show(Act::Undo)));
            self.changes.push(Change::Unpinned(Box::new(pin.clone()), i));
            if self.sidebar.view == SideView::Library {
                self.fix_side_cursor();
            }
        }
    }

    /// `*`: pins the selected song, album or playlist (or unpins it).
    pub(super) fn pin_selected(&mut self) {
        let pin = match self.view() {
            View::Search if self.search_kind != SearchKind::Songs => match self.found.current().cloned() {
                Some(Found::Album(album)) => Some(Pin::Album { album }),
                Some(Found::Playlist(p)) => Some(Pin::Playlist { uri: p.uri, name: p.name, images: p.images }),
                Some(Found::Artist(_)) => {
                    self.info("Artists cannot be pinned; pin a song or an album.");
                    return;
                }
                None => None,
            },
            View::Playlists => self.current_playlist().map(|p| Pin::Playlist { uri: p.uri, name: p.name, images: p.images }),
            View::Artist => match self.page() {
                Some(Page::Artist(a)) => match a.rows.current() {
                    Some(ArtistRow::Album(album)) => Some(Pin::Album { album: album.clone() }),
                    _ => self.selected_track().map(|track| Pin::Song { track }),
                },
                _ => None,
            },
            _ => self.selected_track().map(|track| Pin::Song { track }),
        };
        match pin {
            Some(pin) => self.toggle_pin(pin),
            None => self.info("Select a song, album or playlist first."),
        }
    }

    /// Saves a picture card of the song (cover, title, artist, album) to ~/Pictures/spotlite.
    pub(super) fn share_card(&mut self, track: Track) {
        self.info(format!("Making a card for {}…", track.name));
        self.send(Cmd::Card(Box::new(track)));
    }
}
