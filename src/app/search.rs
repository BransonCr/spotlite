//! Search, moving in lists, and finding the playing song.

use super::*;

impl App {
    /// Moves the cursor to the playing track, if the current list has it.
    /// `c`: selects the playing song. When this list does not have it, the list it plays from
    /// opens (its playlist, album, Liked Songs or artist), and the song is selected there.
    pub(super) fn jump_to_playing(&mut self) {
        let Some(uri) = self.playing_uri().map(str::to_owned) else {
            self.info("Nothing plays.");
            return;
        };
        if let Some(pos) = self.tracks().and_then(|l| l.items.iter().position(|t| t.uri == uri).and_then(|i| l.pos_of(i))) {
            self.select(pos);
            return;
        }
        let pb = self.playback.as_ref();
        let context = pb.and_then(|p| p.context.as_ref()).map(|c| c.uri.clone());
        let track = pb.and_then(|p| p.item.clone());
        let shuffled = self.shuffled.as_ref().map(|(c, _)| c.clone());
        let context = context.or(shuffled);
        self.pending_jump = Some(uri);
        match context.as_deref() {
            Some(c) if c.starts_with("spotify:playlist:") => {
                let name = self.context_name().unwrap_or_default();
                match self.playlists.items.iter().find(|p| p.uri == c).cloned() {
                    Some(pl) => {
                        self.go(Loc::Base(Tab::Playlists));
                        self.open_playlist(pl);
                    }
                    None => {
                        self.pending_jump = None;
                        self.info(format!("It plays from {name}, which is not in your playlists."));
                        return;
                    }
                }
            }
            Some(c) if c.ends_with(":collection") => self.go(Loc::Base(Tab::Liked)),
            Some(c) if c.starts_with("spotify:artist:") => {
                let id = c.trim_start_matches("spotify:artist:").to_owned();
                let name = track.as_ref().and_then(|t| t.artists.first()).map(|a| a.name.clone()).unwrap_or_default();
                self.open_artist(id, name);
            }
            _ => match track.and_then(|t| t.album.filter(|a| a.id.is_some())) {
                Some(album) => self.open_album(album),
                None => {
                    self.pending_jump = None;
                    self.info("The playing song is not in this list.");
                    return;
                }
            },
        }
        self.finish_jump();
    }

    /// Selects the song `c` asked for, once the list that has it shows it.
    pub(super) fn finish_jump(&mut self) {
        let Some(uri) = self.pending_jump.clone() else { return };
        let Some(l) = self.tracks() else { return };
        if let Some(pos) = l.items.iter().position(|t| t.uri == uri).and_then(|i| l.pos_of(i)) {
            self.pending_jump = None;
            self.select(pos);
        } else if !l.has_more() && !l.loading && !l.items.is_empty() {
            self.pending_jump = None;
        }
    }

    /// True when `list` is the non-song search shown now (older answers are dropped).
    pub(super) fn is_current_found(&self, list: &List) -> bool {
        matches!(list, List::Found { query, kind } if *query == self.searched && Some(*kind) == self.search_kind.api())
    }

    /// Shows another kind of search result (the list loads in tick()).
    pub fn set_search_kind(&mut self, kind: SearchKind) {
        if self.tab != Tab::Search || self.active.is_some() {
            self.switch_tab(Tab::Search);
        }
        if kind != self.search_kind {
            self.search_kind = kind;
            self.found.reset();
        }
    }

    /// Enter on an artist, album or playlist found by search.
    pub(super) fn open_found(&mut self) {
        let Some(item) = self.found.current().cloned() else { return };
        match item {
            Found::Artist(a) => self.open_artist(a.id, a.name),
            Found::Album(a) => self.open_album(a),
            // Your own playlist opens in the Playlists tab, like there; so does any other when
            // the client protocol can list its songs.
            Found::Playlist(p) if self.owns(&p) || crate::client::AVAILABLE => {
                self.open_playlist(p);
                self.switch_tab(Tab::Playlists);
            }
            Found::Playlist(p) => {
                // Spotify lists only your own playlists' songs to dev-mode apps: play it.
                self.info(format!("Playing {}", p.name));
                self.play(Cmd::Play(Play::Context { uri: p.uri, from: None }), None);
            }
        }
    }

    pub(super) fn submit_search(&mut self) {
        let q = self.query.trim().to_owned();
        self.typing = false;
        self.search_due = None;
        if !q.is_empty() && q != self.searched {
            self.run_search(q);
        }
    }

    /// Search as you type: 300 ms after the last key, if the text has 2 or more characters.
    pub(super) fn live_search(&mut self) -> bool {
        let q = self.query.trim().to_owned();
        if q.chars().count() < 2 || q == self.searched {
            return false;
        }
        self.run_search(q);
        true
    }

    pub(super) fn run_search(&mut self, q: String) {
        // Typing makes searches for every few letters: only one a pause or Enter kept goes in.
        self.search_history.retain(|h| *h != q && !q.starts_with(h.as_str()));
        self.search_history.insert(0, q.clone());
        self.search_history.truncate(30);
        self.history_at = None;
        self.searched = q;
        self.search.reset();
        self.found.reset();
        // tick() loads the first page.
    }

    pub(super) fn reload(&mut self) {
        use crate::worker::kept;
        match self.view() {
            View::Artist | View::Album | View::Radio | View::Random => {
                // The page asks Spotify again (not its saved copy).
                let Some(i) = self.active else { return };
                match &mut self.pages[i] {
                    Page::Artist(a) => {
                        let id = a.id.clone();
                        a.error = None;
                        a.rows.loading = true;
                        self.send(Cmd::Refresh(kept::artist(&id)));
                    }
                    Page::Album(a) => {
                        let id = a.id.clone();
                        a.tracks.reset();
                        self.send(Cmd::Refresh(kept::album(&id)));
                    }
                    Page::Radio(r) => {
                        r.error = None;
                        r.tracks.loading = true;
                    }
                    Page::Random(_) => {}
                }
                if !matches!(self.pages[i], Page::Random(_)) {
                    self.load_page(i);
                }
            }
            View::Search => {
                self.search.reset();
                self.found.reset();
            }
            View::Playlists => {
                self.send(Cmd::Refresh(kept::PLAYLISTS.to_owned()));
                self.playlists.reset();
                self.rebuild_rows();
            }
            View::Liked => self.liked.reset(),
            View::Recent => {
                self.recent.reset();
                self.recent_at = Some((Instant::now(), self.playing_uri().map(str::to_owned)));
            }
            View::Playlist => {
                if let Some(o) = self.open.as_mut() {
                    let id = o.playlist.id.clone();
                    o.tracks.reset();
                    self.send(Cmd::Refresh(kept::playlist(&id)));
                }
            }
        }
        self.next_poll = Instant::now();
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        if let Some(a) = self.artist_page_mut() {
            a.move_by(delta);
        } else if self.view() == View::Search && self.search_kind != SearchKind::Songs {
            self.found.move_by(delta);
        } else if self.view() == View::Playlists && self.foldered() {
            self.rows.move_by(delta);
        } else if self.view() == View::Playlists {
            self.playlists.move_by(delta);
        } else if let Some(l) = self.tracks_mut() {
            l.move_by(delta);
        }
    }

    pub fn list_len(&self) -> usize {
        if let Some(Page::Artist(a)) = self.page() {
            return a.rows.items.len();
        }
        match self.view() {
            View::Playlists if self.foldered() => self.rows.len(),
            View::Playlists => self.playlists.len(),
            View::Search if self.search_kind != SearchKind::Songs => self.found.len(),
            _ => self.tracks().map_or(0, |l| l.len()),
        }
    }

    pub(super) fn select(&mut self, i: usize) {
        if let Some(a) = self.artist_page_mut() {
            a.select(i);
        } else if self.view() == View::Search && self.search_kind != SearchKind::Songs {
            self.found.state.select(Some(i));
        } else if self.view() == View::Playlists && self.foldered() {
            self.rows.state.select(Some(i));
        } else if self.view() == View::Playlists {
            self.playlists.state.select(Some(i));
        } else if let Some(l) = self.tracks_mut() {
            l.state.select(Some(i));
        }
    }
}
