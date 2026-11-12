//! Playback: play, pause, skip, seek, volume, shuffle, repeat, likes, and what plays next.

use super::*;

impl App {
    /// True when our own spotifyd is the active device, so MPRIS can drive it.
    pub(super) fn local(&self) -> bool {
        let Some((_, ours)) = &self.control else { return false };
        self.playback
            .as_ref()
            .and_then(|pb| pb.device.as_ref())
            .is_some_and(|d| d.is_active && d.name.eq_ignore_ascii_case(ours))
    }

    /// A player command: MPRIS when local (about 8 ms), else the Web API.
    pub(super) fn player(&mut self, ctl: Ctl) {
        if self.local() {
            if let Some((tx, _)) = &self.control {
                if tx.send(ctl).is_ok() {
                    self.in_flight += 1;
                }
            }
        } else {
            self.send(ctl.fallback());
        }
        if !matches!(ctl, Ctl::Volume(_)) {
            self.poll_soon();
        }
    }

    /// Ask for fresh player state shortly, to confirm a command.
    pub(super) fn poll_soon(&mut self) {
        if self.poll_pending {
            self.repoll = true;
        } else {
            self.next_poll = self.next_poll.min(Instant::now() + CONFIRM_AFTER);
        }
    }

    /// Moves the progress the clock added since the last poll into the state, so a change to
    /// is_playing or polled_at does not make the bar jump.
    pub(super) fn bake_progress(&mut self) {
        let now = self.progress_ms();
        if let Some(pb) = self.playback.as_mut() {
            pb.progress_ms = Some(now);
        }
        self.polled_at = Instant::now();
    }

    pub(super) fn toggle_play(&mut self) {
        if self.ghost {
            // The last song, shown from before: start it again where it was.
            if let Some(mut sp) = self.snapshot() {
                sp.playing = true;
                self.resume = None;
                self.resume_from(sp);
            }
            return;
        }
        let Some(playing) = self.playback.as_ref().filter(|pb| pb.item.is_some()).map(|pb| pb.is_playing) else {
            // Nothing loaded: the API thread moves playback to spotifyd and resumes there.
            self.send(Cmd::Toggle);
            self.poll_soon();
            return;
        };
        self.bake_progress();
        if let Some(pb) = self.playback.as_mut() {
            pb.is_playing = !playing;
        }
        self.play_hold = Some((!playing, Instant::now()));
        self.player(if playing { Ctl::Pause } else { Ctl::Play });
    }

    pub(super) fn skip(&mut self, next: bool) {
        if self.ghost {
            self.info(format!("Nothing plays now: {} starts the last song again.", self.keys.show(Act::PlayPause)));
            return;
        }
        // Back, after the first seconds, starts the same song again (Spotify does so).
        let restart = !next && self.progress_ms() > 3000;
        let up_next = if next { self.up_next.clone() } else { None };
        if let Some(pb) = self.playback.as_mut() {
            pb.progress_ms = Some(0);
        }
        self.polled_at = Instant::now();
        if restart {
            self.seek_hold = Some(Instant::now());
        } else {
            self.seek_hold = None;
            // The next song shows at once when spotlite knows it; either way polls come fast
            // until Spotify shows the change.
            self.expect_switch(up_next, true);
        }
        self.player(if next { Ctl::Next } else { Ctl::Prev });
    }

    /// A song change starts: show `track` at once (when known) and poll fast until Spotify
    /// agrees. `same_list`: it plays from the list that plays now (a skip), so the "from"
    /// stays.
    pub(super) fn expect_switch(&mut self, track: Option<Track>, same_list: bool) {
        let from = self.playing_uri().map(str::to_owned);
        let want = track.as_ref().map(|t| t.uri.clone());
        if let Some(t) = track {
            self.show_song(t, true, same_list);
        }
        self.ghost = false;
        self.switch = Some(Switch { from, want, at: Instant::now(), len: SWITCH_HOLD });
        self.poll_soon();
    }

    /// Puts `track` in the player and the sidebar now, from its start.
    pub(super) fn show_song(&mut self, track: Track, playing: bool, same_list: bool) {
        let uri = track.uri.clone();
        let shuffle = self.true_shuffle;
        let pb = self.playback.get_or_insert_with(|| Playback {
            device: None,
            is_playing: playing,
            progress_ms: Some(0),
            shuffle_state: shuffle,
            repeat_state: "off".into(),
            context: None,
            item: None,
        });
        pb.item = Some(track);
        pb.progress_ms = Some(0);
        pb.is_playing = playing;
        if !same_list {
            pb.context = None;
        }
        self.polled_at = Instant::now();
        self.seek_hold = None;
        self.play_hold = None;
        self.seek_preview = None;
        // The ♥ from the Liked index, else ask (quietly) now, not after Spotify agrees.
        if self.saved.as_ref().is_none_or(|(u, _)| *u != uri) {
            self.saved = self.library.as_ref().map(|l| (uri.clone(), l.tracks.contains(&uri)));
            if self.saved.is_none() && uri.starts_with("spotify:track:") {
                self.send(Cmd::CheckSaved(uri));
            }
        }
    }

    /// Sends a play command whose first song is `first` (when known), and shows the change.
    pub(super) fn play(&mut self, cmd: Cmd, first: Option<Track>) {
        self.send(cmd);
        self.expect_switch(first, false);
    }

    /// Spotify's state shows a song it did not show before: its ♥, and what plays next.
    pub(super) fn new_song(&mut self, uri: &str) {
        if self.saved.as_ref().is_none_or(|(u, _)| u != uri) {
            // The ♥: from the Liked index when it is there (no request).
            self.saved = self.library.as_ref().map(|l| (uri.to_owned(), l.tracks.contains(uri)));
            if self.saved.is_none() && uri.starts_with("spotify:track:") {
                self.send(Cmd::CheckSaved(uri.to_owned()));
            }
        }
        self.song_changed(uri);
    }

    /// spotifyd says what it plays (MPRIS): a new song or play/pause shows at once (an end of
    /// a song, a media key, a skip from a phone), and a poll confirms it.
    pub(super) fn on_mpris(&mut self, playing: bool, track: Option<Track>) {
        let Some(track) = track else { return };
        let device_known = self.playback.as_ref().is_some_and(|pb| pb.device.is_some());
        // spotifyd keeps its last state when another device plays: only ours counts. Before
        // the first poll, only a song that plays counts.
        if !(self.local() || (!device_known && playing)) {
            return;
        }
        if self.playing_uri() != Some(track.uri.as_str()) {
            let full = self
                .up_next
                .iter()
                .chain(self.queue.iter())
                .find(|t| t.uri == track.uri)
                .cloned()
                .or_else(|| self.find_track(&track.uri));
            let from = self.playing_uri().map(str::to_owned);
            let uri = track.uri.clone();
            self.show_song(full.unwrap_or(track), playing, true);
            self.ghost = false;
            self.switch = Some(Switch { from, want: Some(uri), at: Instant::now(), len: SWITCH_HOLD });
            self.poll_soon();
            return;
        }
        let shown = self.playback.as_ref().is_some_and(|pb| pb.is_playing);
        if shown != playing && self.play_hold.is_none() && self.switch.is_none() {
            self.bake_progress();
            if let Some(pb) = self.playback.as_mut() {
                pb.is_playing = playing;
            }
            self.ghost = false;
            self.poll_soon();
        }
    }

    /// The window has the focus again: a look at the player, if the last one is not fresh.
    pub fn on_focus(&mut self) {
        if self.polled_at.elapsed() > Duration::from_secs(2) && !self.poll_pending {
            self.next_poll = self.next_poll.min(Instant::now());
        }
    }

    /// With true shuffle on, a play command plays the whole list in a random order instead.
    pub(super) fn with_true_shuffle(&mut self, cmd: Cmd) -> Cmd {
        if !self.true_shuffle {
            return cmd;
        }
        match cmd {
            Cmd::Play(Play::Context { uri, from }) if crate::worker::shufflable(&uri) => {
                Cmd::PlayShuffled { context: uri, from, position_ms: 0 }
            }
            Cmd::PlayLiked { from } => {
                let context = match &self.user_id {
                    Some(id) => format!("spotify:user:{id}:collection"),
                    None => "spotify:collection".into(),
                };
                Cmd::PlayShuffled { context, from: Some(from), position_ms: 0 }
            }
            Cmd::Play(Play::Tracks { uris, offset }) if uris.len() > 1 => {
                let first = uris.get(offset).cloned();
                let uris = crate::worker::shuffled(uris, first.as_deref(), &mut rand::thread_rng());
                self.shuffled = None;
                Cmd::Play(Play::Tracks { uris, offset: 0 })
            }
            cmd => cmd,
        }
    }

    pub fn playing_uri(&self) -> Option<&str> {
        self.playback.as_ref()?.item.as_ref().map(|t| t.uri.as_str())
    }

    /// Name of the playlist (or Liked Songs) that is playing, if known.
    pub fn context_name(&self) -> Option<String> {
        let uri = &self.playback.as_ref()?.context.as_ref()?.uri;
        if uri.ends_with(":collection") {
            return Some("Liked Songs".into());
        }
        self.playlists
            .items
            .iter()
            .find(|p| &p.uri == uri)
            .map(|p| p.name.clone())
    }

    /// Current progress in ms, moved forward locally since the last poll.
    pub fn progress_ms(&self) -> u64 {
        if let Some(ms) = self.seek_preview {
            return ms;
        }
        let Some(pb) = &self.playback else { return 0 };
        let base = pb.progress_ms.unwrap_or(0);
        let elapsed = if pb.is_playing {
            self.polled_at.elapsed().as_millis() as u64
        } else {
            0
        };
        let duration = pb.item.as_ref().map_or(u64::MAX, |t| t.duration_ms);
        (base + elapsed).min(duration)
    }

    pub fn volume(&self) -> Option<u8> {
        match self.vol_set {
            Some((v, at)) if at.elapsed() < VOLUME_HOLD => Some(v),
            _ => self.playback.as_ref()?.device.as_ref()?.volume_percent,
        }
    }

    pub fn muted(&self) -> bool {
        self.unmute_to.is_some() && self.volume() == Some(0)
    }

    /// True while the visualizer should move: on, and music playing. (Not tied to window
    /// focus: with focus-follows-mouse, focus leaves whenever the pointer does.)
    pub fn animating(&self) -> bool {
        // Also while one sweeps out, so its bars have sound to leave with.
        let side = self.sidebar.shows_song() && self.side_fx_shown().1 > 0.0;
        (self.viz_reveal() > 0.0 || side) && self.playback.as_ref().is_some_and(|pb| pb.is_playing)
    }

    /// How far the main visualizer is in (0..1): it sweeps in from the left and out again.
    pub fn viz_reveal(&self) -> f32 {
        sweep(self.viz_on, self.viz_at)
    }

    /// The sidebar effect to draw, and how far it is in (0..1): the one chosen sweeps in; after
    /// Off, the one before sweeps out.
    pub fn side_fx_shown(&self) -> (SideFx, f32) {
        if self.side_fx != SideFx::Off {
            (self.side_fx, sweep(true, self.side_fx_at))
        } else {
            (self.side_fx_out, sweep(false, self.side_fx_at))
        }
    }

    pub(super) fn set_side_fx(&mut self, fx: SideFx) {
        if fx == self.side_fx {
            return;
        }
        self.side_fx_out = self.side_fx;
        self.side_fx = fx;
        self.side_fx_at = Some(Instant::now());
    }

    /// V: the menu of sidebar effects. Moving in it shows each one; Esc puts the old one back.
    pub(super) fn open_side_fx(&mut self) {
        if !self.sidebar.shows_song() {
            self.show_side(SideView::Playing);
        }
        let aura = crate::theme::c().aura;
        let items = vec![
            ("1", "Off".to_owned(), Action::SideFx(SideFx::Off)),
            ("2", format!("Aura · {}, {}", aura.name(), aura.about()), Action::SideFx(SideFx::Aura)),
            ("3", "Spectrum · bars on their side".to_owned(), Action::SideFx(SideFx::Spectrum)),
        ];
        let cursor = items.iter().position(|(_, _, a)| *a == Action::SideFx(self.side_fx)).unwrap_or(0);
        self.side_fx_before = Some(self.side_fx);
        self.menu = Some(Menu { title: "Sidebar effect".into(), items, cursor, track: None, at: None });
    }

    /// True while a visualizer sweeps in or out.
    pub(super) fn viz_sweeping(&self) -> bool {
        let r = self.side_fx_shown().1;
        let main = self.viz_reveal();
        (self.sidebar.shows_song() && r > 0.0 && r < 1.0) || (main > 0.0 && main < 1.0)
    }

    /// The time between polls, from what the player does now.
    pub(super) fn poll_every(&self) -> Duration {
        match &self.playback {
            // Our spotifyd tells of its changes itself (MPRIS); another device does not.
            Some(pb) if pb.is_playing && self.follow.is_some() && self.local() => POLL_PLAYING,
            Some(pb) if pb.is_playing => POLL_REMOTE,
            Some(pb) if pb.item.is_some() => POLL_PAUSED,
            _ => POLL_IDLE,
        }
    }

    /// What plays now, as it would be saved.
    pub(super) fn snapshot(&self) -> Option<SavedPlayback> {
        let pb = self.playback.as_ref()?;
        let t = pb.item.as_ref()?;
        Some(SavedPlayback {
            track: t.uri.clone(),
            name: t.name.clone(),
            context: pb.context.as_ref().map(|c| c.uri.clone()),
            position_ms: self.progress_ms(),
            playing: pb.is_playing,
            shuffle: pb.shuffle_state,
            repeat: pb.repeat_state.clone(),
            volume: self.volume().filter(|v| *v > 0),
            item: Some(t.clone()),
        })
    }

    /// Shows the song of the last run (paused, at its place) before Spotify answers, so the
    /// player and the sidebar are not empty at start.
    pub(super) fn show_saved(&mut self, sp: &SavedPlayback) {
        let Some(item) = sp.item.clone() else { return };
        self.playback = Some(Playback {
            device: None,
            is_playing: false,
            progress_ms: Some(sp.position_ms),
            shuffle_state: sp.shuffle,
            repeat_state: sp.repeat.clone(),
            context: sp.context.clone().map(|uri| crate::api::Context { uri }),
            item: Some(item),
        });
        self.polled_at = Instant::now();
        self.ghost = true;
    }

    /// Starts `sp` again (its song, place, shuffle, repeat, volume), shown at once.
    pub(super) fn resume_from(&mut self, sp: SavedPlayback) {
        let uri = sp.track.clone();
        let from = self.playing_uri().map(str::to_owned);
        if let Some(pb) = self.playback.as_mut() {
            pb.is_playing = sp.playing;
        }
        self.polled_at = Instant::now();
        self.play_hold = Some((sp.playing, Instant::now()));
        self.ghost = false;
        self.switch = Some(Switch { from, want: Some(uri), at: Instant::now(), len: RESUME_HOLD });
        self.send(Cmd::Resume(Box::new(sp)));
        self.poll_soon();
    }

    /// Starts again what played at the last quit, if nothing plays now. It waits for the first
    /// player state and for spotifyd to show up as a device (up to 45 s after start: a slow login takes 30 s). spotifyd
    /// may have just started (it needs about 2 s to log in): the device is looked for again as
    /// soon as each answer is in, at most every `FIND_EVERY`, so the song starts at once.
    pub(super) fn try_resume(&mut self) {
        let Some((_, since)) = &self.resume else { return };
        if since.elapsed() > Duration::from_secs(45) {
            self.resume = None;
            return;
        }
        if !self.polled_once {
            return;
        }
        if !self.ghost && self.playback.as_ref().is_some_and(|pb| pb.item.is_some()) {
            // Something plays already (spotifyd kept going): leave it alone.
            self.resume = None;
            return;
        }
        if self.device.is_none() {
            // An answer that never came (an error) does not stop the looking.
            let every = if self.finding { Duration::from_secs(3) } else { FIND_EVERY };
            if self.find_device_at.is_none_or(|at| at.elapsed() >= every) {
                if self.find_device_at.is_none() {
                    // Say why nothing plays yet (only when it has to wait).
                    self.info(format!("Starting where you left off: {}…", self.resume.as_ref().map_or("", |(sp, _)| sp.name.as_str())));
                }
                self.find_device_at = Some(Instant::now());
                self.finding = true;
                self.send(Cmd::FindDevice);
            }
            return;
        }
        if let Some((sp, _)) = self.resume.take() {
            self.resume_from(sp);
        }
    }

    /// Adds the song to the local history once it played 30 s (half of it, if it is shorter)
    /// on spotlite's device: spotifyd does not tell Spotify, so Recently played would miss it.
    /// A play of the same song from the start again counts again. True if the list on screen
    /// changed.
    pub(super) fn note_listen(&mut self) -> bool {
        let Some(pb) = self.playback.as_ref().filter(|p| p.is_playing && !self.ghost) else { return false };
        let Some(track) = pb.item.as_ref() else { return false };
        // Songs on another device (a phone) are in Spotify's own list.
        let own = match (&self.control, &pb.device) {
            (Some((_, name)), Some(d)) => d.name == *name,
            _ => true,
        };
        let ms = pb.progress_ms.unwrap_or(0) + self.polled_at.elapsed().as_millis() as u64;
        let done = match &mut self.listen {
            Some((uri, done)) if *uri == track.uri => {
                if *done && ms < 5_000 {
                    *done = false;
                }
                *done
            }
            _ => {
                self.listen = Some((track.uri.clone(), false));
                false
            }
        };
        let need = crate::history::LISTEN_MS.min(track.duration_ms / 2).max(1_000);
        if done || !own || ms < need {
            return false;
        }
        if let Some(l) = self.listen.as_mut() {
            l.1 = true;
        }
        let track = track.clone();
        crate::history::add(&track, crate::history::now_ms());
        // Recently played shows it at once; a later visit loads the merged list again.
        self.recent_at = None;
        if self.recent.total.is_some() && !self.recent.loading {
            self.recent.push_front(track);
            return self.view() == View::Recent;
        }
        false
    }

    /// Music is playing and the theme moves: the loop wakes 4 times a second.
    pub fn moving(&self) -> bool {
        crate::theme::motion() && self.playback.as_ref().is_some_and(|pb| pb.is_playing && pb.item.is_some())
    }

    pub(super) fn play_selected(&mut self) {
        // The song that starts, shown at once.
        let first = self.selected_track();
        match self.view() {
            View::Album | View::Liked | View::Playlist if self.tracks().is_some_and(|l| l.reshaped()) => {
                // Filtered or sorted: the songs play as they show.
                let Some(l) = self.tracks() else { return };
                let (uris, offset) = shown_play(l);
                if !uris.is_empty() {
                    self.play(Cmd::Play(Play::Tracks { uris, offset }), first);
                }
            }
            View::Album => {
                let Some(Page::Album(a)) = self.page() else { return };
                let Some(t) = a.tracks.current() else { return };
                let uri = a.album.uri.clone().unwrap_or_else(|| format!("spotify:album:{}", a.id));
                let cmd = Cmd::Play(Play::Context { uri, from: Some(t.uri.clone()) });
                self.play(cmd, first);
            }
            View::Recent => {
                // The same song can be in the list twice; play the list as it is.
                let (uris, offset) = shown_play(&self.recent);
                if !uris.is_empty() {
                    self.play(Cmd::Play(Play::Tracks { uris, offset }), first);
                }
            }
            View::Random => {
                let Some(Page::Random(r)) = self.page() else { return };
                let (uris, offset) = shown_play(&r.tracks);
                if !uris.is_empty() {
                    self.play(Cmd::Play(Play::Tracks { uris, offset }), first);
                }
            }
            View::Radio => {
                let Some(Page::Radio(r)) = self.page() else { return };
                let (uris, offset) = shown_play(&r.tracks);
                if !uris.is_empty() {
                    self.play(Cmd::Play(Play::Tracks { uris, offset }), first);
                }
            }
            View::Artist => {
                let Some(Page::Artist(a)) = self.page() else { return };
                match a.rows.current() {
                    Some(ArtistRow::Album(album)) => {
                        let album = album.clone();
                        self.open_album(album);
                    }
                    Some(ArtistRow::Radio(seed)) => {
                        let seed = seed.clone();
                        self.start_radio(seed);
                    }
                    Some(ArtistRow::Track(t)) => {
                        let top = a.top_tracks();
                        let offset = top.iter().position(|x| x.uri == t.uri).unwrap_or(0);
                        let uris = top.iter().map(|t| t.uri.clone()).collect();
                        self.play(Cmd::Play(Play::Tracks { uris, offset }), first);
                    }
                    Some(ArtistRow::Header(_)) | None => {}
                }
            }
            View::Search if self.search_kind != SearchKind::Songs => self.open_found(),
            View::Search => {
                let (uris, offset) = shown_play(&self.search);
                if !uris.is_empty() {
                    self.play(Cmd::Play(Play::Tracks { uris, offset }), first);
                }
            }
            View::Liked => {
                if let Some(t) = self.liked.current() {
                    let from = t.uri.clone();
                    self.play(Cmd::PlayLiked { from }, first);
                }
            }
            View::Playlist => {
                let Some(open) = &self.open else { return };
                if let Some(t) = open.tracks.current() {
                    let play = Play::Context {
                        uri: open.playlist.uri.clone(),
                        from: Some(t.uri.clone()),
                    };
                    self.play(Cmd::Play(play), first);
                }
            }
            View::Playlists => {
                if let Some(Row::Folder { index, .. }) = self.foldered().then(|| self.rows.current()).flatten() {
                    let index = *index;
                    self.open_folder(index);
                    return;
                }
                let Some(pl) = self.current_playlist() else { return };
                if self.owns(&pl) || crate::client::AVAILABLE {
                    self.open_playlist(pl);
                } else {
                    self.play_playlist(pl);
                }
            }
        }
    }

    pub(super) fn play_playlist(&mut self, pl: Playlist) {
        self.info(format!("Playing {}", pl.name));
        self.play(Cmd::Play(Play::Context { uri: pl.uri, from: None }), None);
    }

    pub(super) fn play_whole_playlist(&mut self) {
        if let Some(Page::Album(a)) = self.page() {
            let uri = a.album.uri.clone().unwrap_or_else(|| format!("spotify:album:{}", a.id));
            self.info(format!("Playing {}", a.album.name));
            self.play(Cmd::Play(Play::Context { uri, from: None }), None);
            return;
        }
        let pl = match self.view() {
            View::Playlists => self.current_playlist(),
            View::Playlist => self.open.as_ref().map(|o| o.playlist.clone()),
            _ => None,
        };
        if let Some(pl) = pl {
            self.play_playlist(pl);
        }
    }

    pub(super) fn queue_selected(&mut self) {
        let Some(t) = self.tracks().and_then(|l| l.current()).cloned() else { return };
        let songs = self.chosen(t);
        for t in &songs {
            self.send(Cmd::Queue { uri: t.uri.clone(), name: t.name.clone() });
        }
        if songs.len() > 1 {
            self.info(format!("Adding {} songs to the queue", songs.len()));
        }
        self.marks.clear();
    }

    /// Whether a song is in Liked Songs, if spotlite knows.
    pub fn is_liked(&self, uri: &str) -> Option<bool> {
        match &self.saved {
            Some((u, s)) if u == uri => Some(*s),
            _ => self.library.as_ref().map(|l| l.tracks.contains(uri)),
        }
    }

    /// Likes or unlikes a song: the selected one, else the playing one.
    pub(super) fn toggle_like(&mut self, track: Option<Track>) {
        if !self.can_like {
            self.error("Run `spotlite auth` again to allow liking songs.");
            return;
        }
        let Some(track) = track.or_else(|| self.target_track()) else {
            self.info("Select a song first.");
            return;
        };
        if self.marks.len() > 1 || (self.marks.len() == 1 && !self.marks.contains(&track.uri)) {
            // Marked songs: like them all, or unlike them all when all are liked.
            let songs = self.chosen(track);
            let saved = !songs.iter().all(|t| self.is_liked(&t.uri) == Some(true));
            let uris: Vec<String> = songs.iter().map(|t| t.uri.clone()).collect();
            self.changes.push(Change::Liked { uris: uris.clone(), saved });
            self.send(Cmd::SetSavedMany { uris, saved });
            self.marks.clear();
            return;
        }
        self.changes.push(Change::Liked { uris: vec![track.uri.clone()], saved: self.is_liked(&track.uri) != Some(true) });
        match &self.saved {
            Some((u, saved)) if *u == track.uri => {
                // The playing song: the heart changes at once (Spotify confirms after).
                let now = !*saved;
                self.saved = Some((track.uri.clone(), now));
                self.like_burst = Some((Instant::now(), now));
                self.send(Cmd::SetSaved { uri: track.uri, saved: now })
            }
            // Known from the Liked index: one request, and the heart moves at once.
            _ if self.library.is_some() => {
                let now = !self.is_liked(&track.uri).unwrap_or(false);
                if self.playing_uri() == Some(track.uri.as_str()) {
                    self.saved = Some((track.uri.clone(), now));
                    self.like_burst = Some((Instant::now(), now));
                }
                self.send(Cmd::SetSaved { uri: track.uri, saved: now })
            }
            // Spotify says which it is.
            _ => {
                self.like_pending = Some(track.uri.clone());
                self.send(Cmd::ToggleSaved(track.uri))
            }
        }
    }

    /// A new song plays: "up next" moves on through the list Spotify gave, and asks again only
    /// when the list runs out or the song is not the one it said.
    pub(super) fn song_changed(&mut self, uri: &str) {
        let was_next = self.up_next.as_ref().is_some_and(|t| t.uri == uri);
        if was_next {
            self.up_next = self.queue.pop_front();
            if self.up_next.is_some() {
                return;
            }
        }
        self.queue.clear();
        self.up_next = None;
        self.send(Cmd::UpNext);
    }

    /// A song by its URI, from what is loaded (the player, the lists, the open pages).
    pub(super) fn find_track(&self, uri: &str) -> Option<Track> {
        let playing = self.playback.as_ref().and_then(|p| p.item.as_ref());
        let lists = [Some(&self.search), Some(&self.recent), self.open.as_ref().map(|o| &o.tracks)];
        playing
            .filter(|t| t.uri == uri)
            .or_else(|| lists.into_iter().flatten().find_map(|l| l.items.iter().find(|t| t.uri == uri)))
            .or_else(|| self.tracks().and_then(|l| l.items.iter().find(|t| t.uri == uri)))
            .cloned()
    }

    /// The heart is animating (the loop wakes often meanwhile).
    pub fn like_animating(&self) -> bool {
        self.like_burst.is_some_and(|(at, _)| at.elapsed() < LIKE_BURST)
    }

    pub(super) fn seek(&mut self, forward: bool) {
        let Some(pb) = &self.playback else { return };
        let Some(track) = &pb.item else { return };
        let now = self.progress_ms();
        let target = if forward {
            (now + SEEK_STEP_MS).min(track.duration_ms.saturating_sub(1000))
        } else {
            now.saturating_sub(SEEK_STEP_MS)
        };
        self.seek_absolute(target);
    }

    pub(super) fn seek_absolute(&mut self, target: u64) {
        // Show the jump at once; the next poll corrects it.
        if let Some(pb) = self.playback.as_mut() {
            pb.progress_ms = Some(target);
        }
        self.polled_at = Instant::now();
        if self.ghost {
            // Nothing plays: the place is where Space starts the song.
            return;
        }
        self.seek_hold = Some(Instant::now());
        self.player(Ctl::Seek(target));
    }

    pub(super) fn change_volume(&mut self, up: bool) {
        let current = self.volume().unwrap_or(50);
        let new = if up {
            current.saturating_add(VOLUME_STEP).min(100)
        } else {
            current.saturating_sub(VOLUME_STEP)
        };
        self.set_volume(new);
    }

    /// Shows the new volume at once and sends it when the previous change is done.
    pub fn set_volume(&mut self, percent: u8) {
        let percent = percent.min(100);
        if percent > 0 {
            self.unmute_to = None;
        }
        self.vol_set = Some((percent, Instant::now()));
        if let Some(d) = self.playback.as_mut().and_then(|pb| pb.device.as_mut()) {
            d.volume_percent = Some(percent);
        }
        self.vol_target = Some(percent);
    }

    pub(super) fn toggle_mute(&mut self) {
        match self.unmute_to.take() {
            Some(v) if self.volume() == Some(0) => self.set_volume(v),
            _ => {
                let current = self.volume().unwrap_or(50);
                if current > 0 {
                    self.set_volume(0);
                    self.unmute_to = Some(current);
                }
            }
        }
    }

    pub(super) fn cycle_repeat(&mut self) {
        let next = match self.playback.as_ref().map(|pb| pb.repeat_state.as_str()) {
            Some("off") | None => "context",
            Some("context") => "track",
            _ => "off",
        };
        if let Some(pb) = self.playback.as_mut() {
            pb.repeat_state = next.into();
        }
        self.repeat_hold = Some((next, Instant::now()));
        self.info(format!("Repeat {}", match next {
            "context" => "all",
            "track" => "one",
            _ => "off",
        }));
        self.send(Cmd::Repeat(next));
    }

    /// Spotify's state, with spotlite's shuffle: ⇄ shows the true shuffle, and the list a true
    /// shuffle plays shows as the source. Spotify's own shuffle, turned on elsewhere (a phone),
    /// becomes the true shuffle: spotlite turns Spotify's off and shuffles the list itself.
    pub(super) fn own_shuffle(&mut self, p: &mut Playback) {
        let recent = self.shuffle_hold.is_some_and(|(_, at)| at.elapsed() < PLAYER_HOLD);
        if p.shuffle_state && !recent {
            self.shuffle_hold = Some((true, Instant::now()));
            self.send(Cmd::Shuffle(false));
            if !self.true_shuffle {
                self.true_shuffle = true;
                let ctx = p.context.as_ref().map(|c| c.uri.clone()).filter(|c| crate::worker::shufflable(c));
                if let (Some(context), Some(t)) = (ctx, p.item.as_ref()) {
                    let cmd = Cmd::PlayShuffled { context, from: Some(t.uri.clone()), position_ms: p.progress_ms.unwrap_or(0) };
                    self.send(cmd);
                }
            }
        }
        p.shuffle_state = self.true_shuffle;
        let playing = p.item.as_ref().map(|t| t.uri.as_str());
        match (&p.context, &self.shuffled) {
            (None, Some((context, uris))) if playing.is_some_and(|u| uris.contains(u)) => {
                p.context = Some(crate::api::Context { uri: context.clone() });
            }
            // Something else plays now.
            (Some(c), Some((context, _))) if c.uri != *context => self.shuffled = None,
            _ => {}
        }
    }

    /// `s`: true shuffle on or off. On, a playlist, album or Liked Songs that plays starts again
    /// in a random order from the song playing; off, it goes back to the list's own order.
    pub(super) fn toggle_shuffle(&mut self) {
        let on = !self.true_shuffle;
        self.true_shuffle = on;
        self.shuffle_hold = Some((on, Instant::now()));
        let pb = self.playback.clone();
        if let Some(pb) = self.playback.as_mut() {
            pb.shuffle_state = on;
        }
        let context = pb.as_ref().and_then(|p| p.context.as_ref()).map(|c| c.uri.clone()).filter(|c| crate::worker::shufflable(c));
        let track = pb.as_ref().and_then(|p| p.item.as_ref()).map(|t| t.uri.clone());
        let position_ms = self.progress_ms();
        match (on, context, track) {
            (true, Some(context), Some(from)) if self.shuffled.is_none() => {
                self.info("True shuffle on: the whole list, in a random order");
                self.send(Cmd::PlayShuffled { context, from: Some(from), position_ms });
                self.poll_soon();
            }
            (false, Some(uri), Some(from)) if self.shuffled.is_some() => {
                self.info("Shuffle off: back to the list's order");
                self.shuffled = None;
                self.send(Cmd::PlayAt(Play::Context { uri, from: Some(from) }, position_ms));
                self.poll_soon();
            }
            _ => self.info(if on { "True shuffle on" } else { "Shuffle off" }),
        }
    }
}
