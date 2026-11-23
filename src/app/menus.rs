//! Pop-ups: the action menu, the radio filters, themes, the playlist picker, the palette.

use super::*;

impl App {
    /// The playlist open in the Playlists tab, if the user may edit it.
    pub(super) fn editable_open_playlist(&self) -> Option<&Playlist> {
        let o = self.open.as_ref()?;
        (self.view() == View::Playlist && self.owns(&o.playlist)).then_some(&o.playlist)
    }

    /// Opens the action menu for the selected track. In a playlist with no song selected,
    /// the playlist's own menu opens.
    pub fn open_menu(&mut self) {
        let Some(track) = self.selected_track() else {
            match self.open.as_ref().filter(|_| self.view() == View::Playlist).map(|o| o.playlist.clone()) {
                Some(pl) => self.open_playlist_menu(pl),
                None => self.info("Select a song first."),
            }
            return;
        };
        let mut items: Vec<(&'static str, String, Action)> = if self.queue_open {
            vec![
                ("⏎", "Play now".into(), Action::Play),
                ("", "Move to the top".into(), Action::QueueTop),
                ("D", "Remove from the queue".into(), Action::QueueRemove),
            ]
        } else {
            vec![("⏎", "Play".into(), Action::Play), ("a", "Add to queue".into(), Action::Queue)]
        };
        for a in track.artists.iter().filter(|a| a.id().is_some()).take(3) {
            items.push((
                if items.iter().any(|i| matches!(i.2, Action::Artist { .. })) { "" } else { "i" },
                format!("Go to artist: {}", a.name),
                Action::Artist { id: a.id().unwrap_or_default().to_owned(), name: a.name.clone() },
            ));
        }
        if let Some(album) = track.album.clone().filter(|a| a.id.is_some()) {
            items.push(("b", format!("Go to album: {}", album.name), Action::Album(album)));
        }
        if crate::client::AVAILABLE && track.uri.starts_with("spotify:track:") {
            items.push(("o", "Start song radio".into(), Action::Radio));
        }
        if track.uri.starts_with("spotify:track:") && track.artists.iter().any(|a| a.uri.is_some()) {
            items.push(("z", "Randomize a genre…".into(), Action::PickGenre));
        }
        items.push(("e", "Add to playlist…".into(), Action::AddToPlaylist));
        if let Some(pl) = self.editable_open_playlist().filter(|_| !self.queue_open) {
            items.push(("D", format!("Remove from {}", pl.name), Action::RemoveFromPlaylist));
        }
        let label = match self.is_liked(&track.uri) {
            Some(true) => "Remove from Liked Songs",
            Some(false) => "Add to Liked Songs",
            None => "Like / unlike",
        };
        items.push(("f", label.into(), Action::Like));
        if let Some(o) = self.open.as_ref().filter(|_| self.view() == View::Playlist && !self.queue_open) {
            let only_list = |(_, _, a): &(&str, String, Action)| !matches!(a, Action::OpenPlaylist(_) | Action::PlayPlaylist(_));
            items.extend(self.playlist_items(&o.playlist).into_iter().filter(only_list));
        }
        self.menu = Some(Menu { title: track.name.clone(), items, cursor: 0, track: Some(track), at: None });
    }

    /// The playlist under the cursor: a playlist search result, or a row of the Playlists tab.
    pub(super) fn selected_playlist(&self) -> Option<Playlist> {
        match self.view() {
            View::Search if self.search_kind == SearchKind::Playlists => match self.found.current()? {
                Found::Playlist(p) => Some(p.clone()),
                _ => None,
            },
            View::Playlists => self.current_playlist(),
            _ => None,
        }
    }

    /// The playlist's entries: open, play, pin it, save it to your library (not your own), copy it.
    fn playlist_items(&self, pl: &Playlist) -> Vec<(&'static str, String, Action)> {
        let b = || Box::new(pl.clone());
        let mut items = vec![("⏎", "Open".to_owned(), Action::OpenPlaylist(b())), ("P", "Play".to_owned(), Action::PlayPlaylist(b()))];
        let pinned = self.pins.iter().any(|p| p.uri() == pl.uri);
        items.push(("*", if pinned { "Unpin from the sidebar" } else { "Pin to the sidebar" }.to_owned(), Action::PinPlaylist(b())));
        if !self.is_mine(pl) {
            let saved = self.playlists.items.iter().any(|p| p.id == pl.id);
            let label = if saved { "Remove from your library" } else { "Add to your library" };
            items.push(("+", label.to_owned(), Action::FollowPlaylist(b())));
        }
        items.push(("c", "Copy to a new playlist…".to_owned(), Action::CopyPlaylist(b())));
        if self.is_mine(pl) {
            items.push(("X", "Delete playlist…".to_owned(), Action::DeletePlaylist(b())));
        }
        items
    }

    /// `D` on a playlist: asks to delete it if you made it, else asks to remove it from your
    /// library.
    pub(super) fn ask_remove_playlist(&mut self, pl: Playlist) {
        if self.is_mine(&pl) {
            self.ask_delete_playlist(pl);
        } else if !self.playlists.items.iter().any(|p| p.id == pl.id) {
            self.info(format!("“{}” is not in your library.", pl.name));
        } else if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing your library.");
        } else {
            self.confirm = Some(Confirm {
                text: format!("Remove “{}” from your library?", pl.name),
                cmd: Cmd::FollowPlaylist { playlist: Box::new(pl), follow: false },
            });
        }
    }

    /// Asks before a playlist you made is deleted.
    pub(super) fn ask_delete_playlist(&mut self, pl: Playlist) {
        if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing playlists.");
            return;
        }
        self.confirm = Some(Confirm {
            text: format!("Delete your playlist “{}”? (Spotify can restore it for 90 days)", pl.name),
            cmd: Cmd::DeletePlaylist(Box::new(pl)),
        });
    }

    /// The menu of a playlist (`.` on a playlist, or in an open playlist with no song).
    pub(super) fn open_playlist_menu(&mut self, pl: Playlist) {
        let items = self.playlist_items(&pl);
        self.menu = Some(Menu { title: pl.name.clone(), items, cursor: 0, track: None, at: None });
    }

    /// Runs an entry of the playlist menu. False for any other action.
    fn run_playlist_action(&mut self, action: &Action) -> bool {
        match action.clone() {
            Action::OpenPlaylist(p) => {
                let p = *p;
                if self.owns(&p) || crate::client::AVAILABLE {
                    self.open_playlist(p);
                    self.switch_tab(Tab::Playlists);
                } else {
                    self.info(format!("Playing {}", p.name));
                    self.play(Cmd::Play(Play::Context { uri: p.uri, from: None }), None);
                }
            }
            Action::PlayPlaylist(p) => {
                self.info(format!("Playing {}", p.name));
                self.play(Cmd::Play(Play::Context { uri: p.uri, from: None }), None);
            }
            Action::PinPlaylist(p) => self.toggle_pin(Pin::Playlist { uri: p.uri, name: p.name, images: p.images }),
            Action::FollowPlaylist(p) => {
                if !self.can_edit {
                    self.error("Run `spotlite auth` again to allow editing your library.");
                } else {
                    let follow = !self.playlists.items.iter().any(|q| q.id == p.id);
                    self.send(Cmd::FollowPlaylist { playlist: p, follow });
                }
            }
            Action::DeletePlaylist(p) => self.ask_delete_playlist(*p),
            Action::CopyPlaylist(p) => {
                if !self.can_edit {
                    self.error("Run `spotlite auth` again to allow editing playlists.");
                } else {
                    let mut name = Input::default();
                    name.set(format!("{} (copy)", p.name));
                    self.naming = Some(Naming { name, uris: Vec::new(), copy_of: Some(*p) });
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn filter_menu_items(&self) -> Vec<(&'static str, String, Action)> {
        let Some(Page::Radio(r)) = self.page() else { return Vec::new() };
        FilterItem::ALL
            .iter()
            .map(|i| {
                let (label, value) = i.describe(&r.filters);
                let text = if value.is_empty() { label.to_owned() } else { format!("{label}: {value}") };
                ("", text, Action::Filter(*i))
            })
            .collect()
    }

    /// Discover mode, one round: new shuffle, fresh songs from more radios, old ones gone.
    pub(super) fn discover_round(&mut self) {
        let library = self.library.clone().unwrap_or_default();
        let Some(Page::Radio(r)) = self.page_mut() else { return };
        if r.radio.is_none() {
            self.info("Wait for the radio to load first.");
            return;
        }
        if r.digging {
            self.info("Still finding songs…");
            return;
        }
        let seeds = r.next_round(&library);
        let (radio, round) = (r.seed.uri.clone(), r.round);
        if seeds.is_empty() {
            self.info("Discover: no new songs left to dig from. Press F, then Reset.");
        } else {
            self.send(Cmd::RadioMore { radio, seeds });
            self.info(format!("Discover round {round}: finding new songs…"));
        }
    }

    /// The theme picker: every theme, read fresh from disk; moving previews it.
    pub(super) fn open_themes(&mut self) {
        let current = crate::theme::name();
        let items: Vec<(&'static str, String, Action)> = crate::theme::all()
            .into_iter()
            .map(|(name, _)| {
                let edited = crate::theme::file(&name).exists();
                let label = if edited { format!("{name} (your file)") } else { name.clone() };
                ("", label, Action::Theme(name))
            })
            .collect();
        let cursor = items.iter().position(|(_, _, a)| *a == Action::Theme(current.clone())).unwrap_or(0);
        self.theme_before = Some(current);
        self.menu = Some(Menu { title: "Theme".into(), items, cursor, track: None, at: None });
    }

    /// Uses the theme under the picker's cursor (as a preview, or for good with `keep`).
    pub(super) fn try_theme(&mut self, name: &str, keep: bool) {
        match crate::theme::apply(name) {
            Ok(()) if keep => {
                self.theme_before = None;
                if !cfg!(test) && crate::theme::env_override().is_none() {
                    let _ = crate::state::save(&self.saved_state());
                }
                self.theme_watch.2 = None;
                self.info(format!("Theme: {name}"));
            }
            Ok(()) => {}
            Err(e) => self.error(e),
        }
    }

    /// Closes any menu; the theme picker puts the old theme back.
    pub(super) fn close_menu(&mut self) {
        if let Some(name) = self.theme_before.take() {
            let _ = crate::theme::apply(&name);
        }
        if let Some(fx) = self.side_fx_before.take() {
            self.set_side_fx(fx);
        }
        self.menu = None;
    }

    /// spotlite itself saved the state file: the theme watcher does not read it again for that.
    pub fn state_saved(&mut self) {
        if self.theme_watch.2.is_some() {
            self.theme_watch.2 = std::fs::metadata(crate::state::path()).and_then(|m| m.modified()).ok();
        }
    }

    /// Live reload, once a second: the theme file changed (an edit), or the saved theme name
    /// changed (`spotlite theme set`, from a shell or an AI agent).
    pub(super) fn watch_theme(&mut self) {
        if cfg!(test)
            || self.theme_watch.0.elapsed() < Duration::from_secs(1)
            || self.theme_before.is_some()
            || crate::theme::env_override().is_some()
        {
            return;
        }
        self.theme_watch.0 = Instant::now();
        let mtime = |p: std::path::PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let state_time = mtime(crate::state::path());
        if state_time != self.theme_watch.2 {
            let first = self.theme_watch.2.is_none();
            self.theme_watch.2 = state_time;
            if let Some(name) = crate::state::load().theme.filter(|n| *n != crate::theme::name()) {
                match crate::theme::apply(&name) {
                    Ok(()) if !first => self.info(format!("Theme {name}")),
                    Ok(()) => {}
                    Err(e) => self.error(e),
                }
            }
        }
        let name = crate::theme::name();
        let file_time = mtime(crate::theme::file(&name));
        if file_time != self.theme_watch.1 {
            let first = self.theme_watch.1.is_none();
            self.theme_watch.1 = file_time;
            if !first || file_time.is_some() {
                // A deleted file goes back to the built-in theme of that name, if there is one.
                if let Err(e) = crate::theme::apply(&name) {
                    self.error(e);
                }
            }
        }
    }

    pub(super) fn open_filters(&mut self) {
        if self.view() != View::Radio {
            return;
        }
        let items = self.filter_menu_items();
        self.menu = Some(Menu { title: "Radio filters".into(), items, cursor: 0, track: None, at: None });
    }

    /// Runs the menu entry under the cursor.
    pub(super) fn run_menu(&mut self) {
        let Some(menu) = self.menu.as_ref() else { return };
        let Some((_, _, action)) = menu.items.get(menu.cursor).cloned() else { return };
        let track = menu.track.clone();
        if let Action::Theme(name) = action {
            self.menu = None;
            self.try_theme(&name, true);
            return;
        }
        if let Action::SideFx(fx) = action {
            self.menu = None;
            self.side_fx_before = None;
            self.set_side_fx(fx);
            self.info(match fx {
                SideFx::Off => "Sidebar effect off".to_owned(),
                SideFx::Aura => format!("Sidebar effect: aura ({})", crate::theme::c().aura.name()),
                SideFx::Spectrum => "Sidebar effect: spectrum".to_owned(),
            });
            return;
        }
        if let Action::Device { id, name } = action {
            self.menu = None;
            self.send(Cmd::Transfer { id, name });
            self.poll_soon();
            return;
        }
        if let Action::Filter(item) = action {
            // Filters stay open: change, see the list update, change again.
            if item == FilterItem::Discover {
                self.discover_round();
            } else {
                let library = self.library.clone().unwrap_or_default();
                if let Some(Page::Radio(r)) = self.page_mut() {
                    if item == FilterItem::Reset {
                        r.stop_discover();
                    }
                    item.step(&mut r.filters);
                    r.refilter(&library);
                }
            }
            let items = self.filter_menu_items();
            if let Some(m) = self.menu.as_mut() {
                m.items = items;
            }
            return;
        }
        self.menu = None;
        if self.run_playlist_action(&action) {
            return;
        }
        let Some(track) = track else { return };
        self.run_action(action, track);
    }

    pub(super) fn run_action(&mut self, action: Action, track: Track) {
        match action {
            Action::Play => self.play_selected(),
            Action::Queue => self.add_to_queue(vec![track]),
            Action::QueueTop => self.queue_to_top(),
            Action::QueueRemove => self.queue_remove(),
            Action::Artist { id, name } => self.open_artist(id, name),
            Action::Album(album) => self.open_album(album),
            Action::Radio => self.start_radio(track),
            Action::PickGenre => self.open_genre_picker(),
            Action::AddToPlaylist => self.open_picker(track),
            Action::RemoveFromPlaylist => self.ask_remove(track),
            Action::Like => self.toggle_like(Some(track)),
            Action::Filter(_)
            | Action::Theme(_)
            | Action::SideFx(_)
            | Action::Device { .. }
            | Action::OpenPlaylist(_)
            | Action::PlayPlaylist(_)
            | Action::PinPlaylist(_)
            | Action::FollowPlaylist(_)
            | Action::CopyPlaylist(_)
            | Action::DeletePlaylist(_) => {}
        }
    }

    /// Asks to save the random songs on screen as a new playlist.
    pub(super) fn ask_save_random(&mut self) {
        if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing playlists.");
            return;
        }
        let Some(Page::Random(r)) = self.page() else { return };
        let uris: Vec<String> = r.tracks.items.iter().map(|t| t.uri.clone()).collect();
        if uris.is_empty() {
            self.info("No songs to save.");
            return;
        }
        let name = if r.rolls > 1 { format!("Random · {} {}", r.genre, r.rolls) } else { format!("Random · {}", r.genre) };
        let description = format!("{} random songs of {}, picked by spotlite (not by taste).", uris.len(), r.genre);
        self.confirm = Some(Confirm {
            text: format!("Save these {} songs as a new playlist “{name}”?", uris.len()),
            cmd: Cmd::CreatePlaylist { name, description, uris },
        });
    }

    pub(super) fn open_picker(&mut self, track: Track) {
        if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing playlists.");
            return;
        }
        self.picker = Some(Picker { track, query: Input::default(), cursor: 0 });
    }

    /// Playlists the user may add to, narrowed by the picker's text.
    pub fn picker_choices(&self) -> Vec<&Playlist> {
        let q = self.picker.as_ref().map(|p| p.query.to_lowercase()).unwrap_or_default();
        let mut list: Vec<&Playlist> = self
            .playlists
            .items
            .iter()
            .filter(|p| self.owns(p))
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q))
            .collect();
        // The playlists you added to last come first.
        list.sort_by_key(|p| self.recent_playlists.iter().position(|id| *id == p.id).unwrap_or(usize::MAX));
        list
    }

    /// Chooses the picker row under the cursor. The row after the playlists makes a new
    /// playlist: with the typed text as its name, or it asks for a name.
    pub(super) fn pick_playlist(&mut self) {
        let Some(picker) = self.picker.as_ref() else { return };
        let chosen = self.picker_choices().get(picker.cursor).map(|p| (*p).clone());
        let track = picker.track.clone();
        let name = picker.query.trim().to_owned();
        self.picker = None;
        match chosen {
            Some(pl) => self.add_to(pl, track),
            None => self.new_playlist(Some(track), name),
        }
    }

    /// Makes a new playlist with the chosen songs of `track` (none without one). Without a
    /// name, it asks for one first.
    pub(super) fn new_playlist(&mut self, track: Option<Track>, name: String) {
        if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing playlists.");
            return;
        }
        let uris: Vec<String> = track.map(|t| self.chosen(t).into_iter().map(|t| t.uri).collect()).unwrap_or_default();
        if name.is_empty() {
            self.naming = Some(Naming { name: Input::default(), uris, copy_of: None });
        } else {
            self.marks.clear();
            self.send(Cmd::CreatePlaylist { name, description: String::new(), uris });
        }
    }

    /// Enter in the name pop-up: makes the playlist, if it has a name.
    pub(super) fn finish_naming(&mut self) {
        let Some(n) = self.naming.as_ref() else { return };
        let name = n.name.trim().to_owned();
        if name.is_empty() {
            self.info("Type a name for the playlist.");
            return;
        }
        let Some(n) = self.naming.take() else { return };
        if let Some(from) = n.copy_of {
            let description = format!("A copy of “{}” by {}.", from.name, from.owner_name());
            self.info(format!("Copying “{}”…", from.name));
            self.send(Cmd::CopyPlaylist { from: from.id, name, description });
            return;
        }
        self.marks.clear();
        self.send(Cmd::CreatePlaylist { name, description: String::new(), uris: n.uris });
    }

    /// Keys in the name pop-up: type the name, Enter makes the playlist, Esc closes.
    pub(super) fn naming_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.naming = None,
            KeyCode::Enter => self.finish_naming(),
            _ => {
                if let Some(n) = self.naming.as_mut() {
                    n.name.edit(&key);
                }
            }
        }
    }

    pub(super) fn ask_remove(&mut self, track: Track) {
        if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing playlists.");
            return;
        }
        let Some(pl) = self.editable_open_playlist().cloned() else {
            self.info("Open one of your playlists to remove songs from it.");
            return;
        };
        let songs = self.chosen(track);
        let o = self.open.as_ref();
        let places: Vec<(String, usize)> = songs
            .iter()
            .filter_map(|t| o.and_then(|o| o.tracks.items.iter().position(|x| x.uri == t.uri)).map(|i| (t.uri.clone(), i)))
            .collect();
        let what = describe(&songs);
        let uris: Vec<String> = songs.iter().map(|t| t.uri.clone()).collect();
        self.send(Cmd::RemoveFromPlaylist { playlist_id: pl.id.clone(), playlist: pl.name.clone(), uris, what });
        self.marks.clear();
        self.changes.push(Change::Removed { playlist_id: pl.id, playlist: pl.name, songs: places });
    }

    pub(super) fn move_popup(&mut self, delta: isize) {
        if let Some(c) = self.undo {
            let len = self.nav.rows().len();
            self.undo = Some((c as isize + delta).clamp(0, len as isize - 1) as usize);
            return;
        }
        let len = if let Some(m) = &self.menu {
            m.items.len()
        } else if self.picker.is_some() {
            // The playlists, then "new playlist".
            self.picker_choices().len() + 1
        } else {
            return;
        };
        if len == 0 {
            return;
        }
        let step = |c: usize| (c as isize + delta).rem_euclid(len as isize) as usize;
        if let Some(m) = self.menu.as_mut() {
            m.cursor = step(m.cursor);
            match m.items.get(m.cursor).cloned() {
                Some((_, _, Action::Theme(name))) => self.try_theme(&name, false),
                Some((_, _, Action::SideFx(fx))) => self.set_side_fx(fx),
                _ => {}
            }
        } else if let Some(p) = self.picker.as_mut() {
            p.cursor = step(p.cursor.min(len - 1));
        }
    }

    /// The palette's entries for its query (what the palette lists).
    pub fn palette_entries(&self) -> Vec<crate::palette::Entry> {
        let q = self.palette.as_ref().map(|p| p.query.to_string()).unwrap_or_default();
        let mut all = crate::palette::entries(self);
        // Typed text also finds playlists, earlier searches, themes and genres, after the actions.
        if q.trim().chars().count() >= 2 {
            all.extend(crate::palette::places(self));
        }
        crate::palette::filter(all, &q)
    }

    pub(super) fn move_palette(&mut self, delta: isize) {
        let len = self.palette_entries().len();
        if let Some(p) = self.palette.as_mut().filter(|_| len > 0) {
            p.cursor = (p.cursor.min(len - 1) as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    /// Runs the palette entry under the cursor: the palette closes, then its key runs.
    pub fn run_palette(&mut self) {
        let entries = self.palette_entries();
        let Some(p) = self.palette.take() else { return };
        if let Some(e) = entries.get(p.cursor.min(entries.len().saturating_sub(1))).cloned() {
            self.run_entry(e.run);
        }
    }

    /// Asks to save the radio list on screen as a new playlist.
    pub(super) fn ask_save_radio(&mut self) {
        if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing playlists.");
            return;
        }
        let Some(Page::Radio(r)) = self.page() else { return };
        let uris: Vec<String> = r.tracks.items.iter().map(|t| t.uri.clone()).collect();
        if uris.is_empty() {
            self.info("No songs to save.");
            return;
        }
        let kind = if r.seed.uri.starts_with("spotify:artist:") { "artist radio" } else { "song radio" };
        let name = if r.filters.discover {
            format!("{} · discover {}", r.seed.name, r.round)
        } else {
            format!("{} radio", r.seed.name)
        };
        let description = format!("Made with spotlite from the {kind} of {}.", r.seed.name);
        self.confirm = Some(Confirm {
            text: format!("Save these {} songs as a new playlist “{name}”?", uris.len()),
            cmd: Cmd::CreatePlaylist { name, description, uris },
        });
    }
}
