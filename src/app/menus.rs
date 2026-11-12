//! Pop-ups: the action menu, the radio filters, themes, the playlist picker, the palette.

use super::*;

impl App {
    /// The playlist open in the Playlists tab, if the user may edit it.
    pub(super) fn editable_open_playlist(&self) -> Option<&Playlist> {
        let o = self.open.as_ref()?;
        (self.view() == View::Playlist && self.owns(&o.playlist)).then_some(&o.playlist)
    }

    /// Opens the action menu for the selected track.
    pub fn open_menu(&mut self) {
        let Some(track) = self.selected_track() else {
            self.info("Select a song first.");
            return;
        };
        let mut items: Vec<(&'static str, String, Action)> = vec![
            ("⏎", "Play".into(), Action::Play),
            ("a", "Add to queue".into(), Action::Queue),
        ];
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
        if let Some(pl) = self.editable_open_playlist() {
            items.push(("D", format!("Remove from {}", pl.name), Action::RemoveFromPlaylist));
        }
        let label = match self.is_liked(&track.uri) {
            Some(true) => "Remove from Liked Songs",
            Some(false) => "Add to Liked Songs",
            None => "Like / unlike",
        };
        items.push(("f", label.into(), Action::Like));
        self.menu = Some(Menu { title: track.name.clone(), items, cursor: 0, track: Some(track), at: None });
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
        let Some(track) = track else { return };
        self.run_action(action, track);
    }

    pub(super) fn run_action(&mut self, action: Action, track: Track) {
        match action {
            Action::Play => self.play_selected(),
            Action::Queue => {
                let cmd = Cmd::Queue { uri: track.uri.clone(), name: track.name.clone() };
                self.send(cmd);
            }
            Action::Artist { id, name } => self.open_artist(id, name),
            Action::Album(album) => self.open_album(album),
            Action::Radio => self.start_radio(track),
            Action::PickGenre => self.open_genre_picker(),
            Action::AddToPlaylist => self.open_picker(track),
            Action::RemoveFromPlaylist => self.ask_remove(track),
            Action::Like => self.toggle_like(Some(track)),
            Action::Filter(_) | Action::Theme(_) | Action::SideFx(_) | Action::Device { .. } => {}
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

    pub(super) fn pick_playlist(&mut self) {
        let Some(picker) = self.picker.as_ref() else { return };
        let Some(pl) = self.picker_choices().get(picker.cursor).map(|p| (*p).clone()) else { return };
        let track = picker.track.clone();
        self.picker = None;
        self.add_to(pl, track);
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
            self.picker_choices().len()
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
