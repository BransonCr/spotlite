use crate::api::{Page, Play, Playback, Playlist, Track};
use crate::control::Ctl;
use crate::folders::Entry;
use crate::images::Images;
use crate::viz::Visualizer;
use crate::worker::{Cmd, Kind, List, Msg};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::TableState;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// How often to ask Spotify for the player state. Progress is interpolated between polls.
const POLL_EVERY: Duration = Duration::from_secs(5);
const VOLUME_STEP: u8 = 5;
const SEEK_STEP_MS: u64 = 10_000;
/// After a volume change, trust the local value over polled state for this long
/// (Spotify reports the old volume for a moment).
const VOLUME_HOLD: Duration = Duration::from_millis(2500);
/// After play/pause or a seek, trust the local state over polls for this long: Spotify's
/// state lags the player by up to a second.
const PLAYER_HOLD: Duration = Duration::from_millis(2500);
/// How soon to confirm a player command with a poll.
const CONFIRM_AFTER: Duration = Duration::from_millis(500);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Rows one mouse-wheel step moves.
const WHEEL_ROWS: isize = 3;
/// Load the next page when the selection is this close to the end of what is loaded.
const PREFETCH: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Search,
    Playlists,
    Liked,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Search, Tab::Playlists, Tab::Liked];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Search => "Search",
            Tab::Playlists => "Playlists",
            Tab::Liked => "Liked",
        }
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// A paged list with its own selection and scroll position.
pub struct Listing<T> {
    pub items: Vec<T>,
    /// Entries fetched so far, null ones included (the offset of the next page).
    fetched: usize,
    /// Size of the whole list on Spotify; None until the first page arrives.
    pub total: Option<usize>,
    pub loading: bool,
    pub state: TableState,
}

impl<T> Default for Listing<T> {
    fn default() -> Self {
        Listing {
            items: Vec::new(),
            fetched: 0,
            total: None,
            loading: false,
            state: TableState::default(),
        }
    }
}

impl<T> Listing<T> {
    pub fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }

    pub fn current(&self) -> Option<&T> {
        self.items.get(self.selected())
    }

    fn has_more(&self) -> bool {
        self.total.is_none_or(|t| self.fetched < t)
    }

    /// Offset of the next page to fetch, if one is needed now.
    fn wants_page(&self) -> Option<usize> {
        let near_end = self.items.len().saturating_sub(self.selected()) <= PREFETCH;
        (!self.loading && self.has_more() && (self.items.is_empty() || near_end)).then_some(self.fetched)
    }

    fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len() as isize - 1;
        let next = (self.selected() as isize).saturating_add(delta).clamp(0, max);
        self.state.select(Some(next as usize));
    }

    fn apply(&mut self, offset: usize, page: Page<T>) {
        if offset == 0 {
            self.items.clear();
            self.fetched = 0;
            *self.state.offset_mut() = 0;
        }
        // Ignore pages that do not continue the list (e.g. a reload raced a page load).
        if offset == self.fetched {
            self.items.extend(page.items);
            self.fetched += page.raw;
            // An empty page means the list ended, whatever the total says.
            self.total = Some(if page.raw == 0 { self.fetched } else { page.total });
        }
        self.loading = false;
        if self.state.selected().is_none() || self.selected() >= self.items.len() {
            self.state.select((!self.items.is_empty()).then_some(0));
        }
    }

    fn reset(&mut self) {
        *self = Listing::default();
    }
}

/// A row of the Playlists tab when the account has folders.
#[derive(Debug, Clone)]
pub enum Row {
    /// `index` is the folder's position in its parent level of the tree.
    Folder { name: String, index: usize, count: usize },
    Playlist(Playlist),
}

/// Playlist opened from the Playlists tab.
pub struct OpenPlaylist {
    pub playlist: Playlist,
    pub tracks: Listing<Track>,
}

/// What the main panel shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Search,
    Playlists,
    Playlist,
    Liked,
}

/// Where clickable things were drawn in the last frame (filled by ui::draw).
#[derive(Default, Debug)]
pub struct Hits {
    pub tabs: Vec<(Rect, Tab)>,
    pub search_box: Rect,
    /// Rows of the current list (header excluded).
    pub list: Rect,
    pub row_height: u16,
    pub list_offset: usize,
    pub back: Rect,
    pub play: Rect,
    pub prev: Rect,
    pub next: Rect,
    pub shuffle: Rect,
    pub repeat: Rect,
    pub like: Rect,
    /// The bar part of the progress line.
    pub progress: Rect,
    /// The bar part of the volume slider.
    pub volume: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Progress,
    Volume,
}

/// 0.0 at the left edge of `r`, 1.0 at the right edge.
fn fraction(r: Rect, x: u16) -> f64 {
    if r.width <= 1 {
        return 0.0;
    }
    (x.saturating_sub(r.x) as f64 / (r.width - 1) as f64).clamp(0.0, 1.0)
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    until: Instant,
}

pub struct App {
    pub tab: Tab,
    pub typing: bool,
    pub query: String,
    pub search: Listing<Track>,
    /// Query the search results belong to.
    pub searched: String,
    pub playlists: Listing<Playlist>,
    pub open: Option<OpenPlaylist>,
    pub liked: Listing<Track>,
    pub playback: Option<Playback>,
    pub polled_at: Instant,
    /// Whether the playing track is in Liked Songs.
    pub saved: Option<(String, bool)>,
    pub user_id: Option<String>,
    pub device: Option<String>,
    pub device_checked: bool,
    pub toast: Option<Toast>,
    pub help: bool,
    pub can_like: bool,
    pub quit: bool,
    /// Sound visualizer (v); off by default.
    pub viz_on: bool,
    pub viz: Visualizer,
    /// The terminal window has focus (the visualizer pauses when it does not).
    pub focused: bool,
    /// Quit and also stop spotifyd (Q): nothing keeps running.
    pub stop_services: bool,
    pub images: Images,
    pub hits: Hits,
    tx: Sender<Cmd>,
    in_flight: usize,
    poll_pending: bool,
    next_poll: Instant,
    /// Volume to send once the previous volume command is done (coalesces drags and key repeat).
    vol_target: Option<u8>,
    vol_in_flight: bool,
    vol_set: Option<(u8, Instant)>,
    /// Volume to go back to when unmuting.
    unmute_to: Option<u8>,
    drag: Option<Drag>,
    /// Progress shown while the progress bar is dragged.
    seek_preview: Option<u64>,
    last_click: Option<(usize, Instant)>,
    /// The folder tree, when the account has folders (else the Playlists tab is a flat list).
    pub folders: Option<Vec<Entry>>,
    /// Folders opened from the top: each one's index in its parent, and the parent's selection.
    folder_path: Vec<(usize, TableState)>,
    /// Rows of the open folder level.
    pub rows: Listing<Row>,
    /// Fast control path to our spotifyd (MPRIS), and its device name.
    control: Option<(Sender<Ctl>, String)>,
    /// Play state set locally, kept until a poll agrees or the hold ends.
    play_hold: Option<(bool, Instant)>,
    seek_hold: Option<Instant>,
    /// A poll was asked for while one was already running.
    repoll: bool,
}

impl App {
    pub fn new(tx: Sender<Cmd>, can_like: bool, images: Images) -> Self {
        let now = Instant::now();
        let mut app = App {
            tab: Tab::Search,
            typing: false,
            query: String::new(),
            search: Listing::default(),
            searched: String::new(),
            playlists: Listing::default(),
            open: None,
            liked: Listing::default(),
            playback: None,
            polled_at: now,
            saved: None,
            user_id: None,
            device: None,
            device_checked: false,
            toast: None,
            help: false,
            can_like,
            quit: false,
            stop_services: false,
            viz_on: false,
            viz: Visualizer::default(),
            focused: true,
            images,
            hits: Hits::default(),
            tx,
            in_flight: 0,
            poll_pending: true,
            next_poll: now + POLL_EVERY,
            vol_target: None,
            vol_in_flight: false,
            vol_set: None,
            unmute_to: None,
            drag: None,
            seek_preview: None,
            last_click: None,
            folders: None,
            folder_path: Vec::new(),
            rows: Listing::default(),
            control: None,
            play_hold: None,
            seek_hold: None,
            repoll: false,
        };
        app.send(Cmd::Init);
        app
    }

    /// Sends player commands for `device` to spotifyd over MPRIS when it is the active device.
    pub fn with_control(mut self, tx: Sender<Ctl>, device: String) -> Self {
        self.control = Some((tx, device));
        self
    }

    /// True when the Playlists tab shows the folder tree.
    pub fn foldered(&self) -> bool {
        self.folders.is_some()
    }

    /// Takes a folder tree (from the cache at start, or freshly fetched).
    pub fn set_folders(&mut self, tree: Vec<Entry>) {
        if !crate::folders::has_folders(&tree) {
            self.folders = None;
            return;
        }
        self.folders = Some(tree);
        self.rebuild_rows();
    }

    /// Names of the open folders, top first.
    pub fn folder_names(&self) -> Vec<String> {
        let Some(tree) = &self.folders else { return Vec::new() };
        let mut level: &[Entry] = tree;
        let mut names = Vec::new();
        for (i, _) in &self.folder_path {
            match level.get(*i) {
                Some(Entry::Folder { name, children }) => {
                    names.push(name.clone());
                    level = children;
                }
                _ => break,
            }
        }
        names
    }

    /// Rebuilds the rows of the open folder level from the tree and the loaded playlists.
    fn rebuild_rows(&mut self) {
        let Some(tree) = &self.folders else { return };
        let mut level: &[Entry] = tree;
        let mut valid = true;
        for (i, _) in &self.folder_path {
            match level.get(*i) {
                Some(Entry::Folder { children, .. }) => level = children,
                _ => valid = false,
            }
        }
        if !valid {
            // The tree changed under us: start again from the top.
            self.folder_path.clear();
            level = tree;
        }
        let by_uri: std::collections::HashMap<&str, &Playlist> =
            self.playlists.items.iter().map(|p| (p.uri.as_str(), p)).collect();
        let mut rows: Vec<Row> = level
            .iter()
            .enumerate()
            .filter_map(|(index, e)| match e {
                Entry::Folder { name, .. } => Some(Row::Folder { name: name.clone(), index, count: e.playlist_count() }),
                Entry::Playlist(uri) => by_uri.get(uri.as_str()).map(|p| Row::Playlist((*p).clone())),
            })
            .collect();
        if self.folder_path.is_empty() {
            // Playlists the tree does not know yet (e.g. made after the last fetch) go at the end.
            fn uris<'a>(entries: &'a [Entry], out: &mut std::collections::HashSet<&'a str>) {
                for e in entries {
                    match e {
                        Entry::Folder { children, .. } => uris(children, out),
                        Entry::Playlist(u) => {
                            out.insert(u.as_str());
                        }
                    }
                }
            }
            let mut known = std::collections::HashSet::new();
            uris(tree, &mut known);
            rows.extend(
                self.playlists
                    .items
                    .iter()
                    .filter(|p| !known.contains(p.uri.as_str()))
                    .cloned()
                    .map(Row::Playlist),
            );
        }
        let total = rows.len();
        self.rows.items = rows;
        self.rows.total = Some(total);
        self.rows.loading = self.playlists.loading;
        let sel = self.rows.state.selected().unwrap_or(0).min(total.saturating_sub(1));
        self.rows.state.select((total > 0).then_some(sel));
    }

    fn open_folder(&mut self, index: usize) {
        let parent = std::mem::take(&mut self.rows.state);
        self.folder_path.push((index, parent));
        self.rows.state.select(Some(0));
        self.rebuild_rows();
    }

    /// Back one level; false if already at the top.
    fn close_folder(&mut self) -> bool {
        match self.folder_path.pop() {
            Some((_, parent)) => {
                self.rows.state = parent;
                self.rebuild_rows();
                true
            }
            None => false,
        }
    }

    /// The playlist under the cursor in the Playlists tab (not a folder).
    fn current_playlist(&self) -> Option<Playlist> {
        if self.foldered() {
            match self.rows.current()? {
                Row::Playlist(p) => Some(p.clone()),
                Row::Folder { .. } => None,
            }
        } else {
            self.playlists.current().cloned()
        }
    }

    /// True when our own spotifyd is the active device, so MPRIS can drive it.
    fn local(&self) -> bool {
        let Some((_, ours)) = &self.control else { return false };
        self.playback
            .as_ref()
            .and_then(|pb| pb.device.as_ref())
            .is_some_and(|d| d.is_active && d.name.eq_ignore_ascii_case(ours))
    }

    /// A player command: MPRIS when local (about 8 ms), else the Web API.
    fn player(&mut self, ctl: Ctl) {
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
    fn poll_soon(&mut self) {
        if self.poll_pending {
            self.repoll = true;
        } else {
            self.next_poll = self.next_poll.min(Instant::now() + CONFIRM_AFTER);
        }
    }

    /// Moves the progress the clock added since the last poll into the state, so a change to
    /// is_playing or polled_at does not make the bar jump.
    fn bake_progress(&mut self) {
        let now = self.progress_ms();
        if let Some(pb) = self.playback.as_mut() {
            pb.progress_ms = Some(now);
        }
        self.polled_at = Instant::now();
    }

    fn toggle_play(&mut self) {
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

    fn skip(&mut self, next: bool) {
        if let Some(pb) = self.playback.as_mut() {
            pb.progress_ms = Some(0);
        }
        self.polled_at = Instant::now();
        self.seek_hold = None;
        self.player(if next { Ctl::Next } else { Ctl::Prev });
    }

    fn send(&mut self, cmd: Cmd) {
        if self.tx.send(cmd).is_ok() {
            self.in_flight += 1;
        }
    }

    /// True while the API thread has work, so the UI should check for replies often.
    pub fn busy(&self) -> bool {
        self.in_flight > 0 || self.vol_target.is_some() || self.images.busy()
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: false,
            until: Instant::now() + Duration::from_secs(3),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: true,
            until: Instant::now() + Duration::from_secs(6),
        });
    }

    pub fn view(&self) -> View {
        match self.tab {
            Tab::Search => View::Search,
            Tab::Playlists if self.open.is_some() => View::Playlist,
            Tab::Playlists => View::Playlists,
            Tab::Liked => View::Liked,
        }
    }

    /// Track list of the current view, if it shows tracks.
    pub fn tracks(&self) -> Option<&Listing<Track>> {
        match self.view() {
            View::Search => Some(&self.search),
            View::Liked => Some(&self.liked),
            View::Playlist => self.open.as_ref().map(|o| &o.tracks),
            View::Playlists => None,
        }
    }

    fn tracks_mut(&mut self) -> Option<&mut Listing<Track>> {
        match self.view() {
            View::Search => Some(&mut self.search),
            View::Liked => Some(&mut self.liked),
            View::Playlist => self.open.as_mut().map(|o| &mut o.tracks),
            View::Playlists => None,
        }
    }

    pub fn playing_uri(&self) -> Option<&str> {
        self.playback.as_ref()?.item.as_ref().map(|t| t.uri.as_str())
    }

    pub fn owns(&self, playlist: &Playlist) -> bool {
        playlist.collaborative || self.user_id.as_deref() == Some(playlist.owner.id.as_str())
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

    /// True while the visualizer should move: on, focused, and music playing.
    pub fn animating(&self) -> bool {
        self.viz_on && self.focused && self.playback.as_ref().is_some_and(|pb| pb.is_playing)
    }

    /// Called on every loop pass: polls the player and loads pages that are needed.
    pub fn tick(&mut self) {
        if self.animating() {
            self.viz.ensure_running();
        } else if self.viz.running() {
            self.viz.stop();
        }
        let now = Instant::now();
        if self.toast.as_ref().is_some_and(|t| now >= t.until) {
            self.toast = None;
        }
        // A track ended: ask for the new state now instead of at the next poll.
        let ended = self.playback.as_ref().is_some_and(|pb| {
            pb.is_playing && pb.item.as_ref().is_some_and(|t| self.progress_ms() >= t.duration_ms)
        });
        if !self.vol_in_flight {
            if let Some(v) = self.vol_target.take() {
                self.vol_in_flight = true;
                self.player(Ctl::Volume(v));
            }
        }
        if !self.poll_pending && (now >= self.next_poll || ended) {
            self.poll_pending = true;
            self.next_poll = now + POLL_EVERY;
            self.send(Cmd::Poll);
        }
        self.load_more();
    }

    fn load_more(&mut self) {
        let request = match self.view() {
            View::Search if !self.searched.is_empty() => self
                .search
                .wants_page()
                .map(|o| (List::Search(self.searched.clone()), o)),
            View::Search => None,
            // The folder tree needs every playlist's name and cover, so load all pages at once.
            View::Playlists if self.foldered() => {
                let l = &self.playlists;
                (!l.loading && l.total.is_none_or(|t| l.fetched < t)).then_some((List::Playlists, l.fetched))
            }
            View::Playlists => self.playlists.wants_page().map(|o| (List::Playlists, o)),
            View::Liked => self.liked.wants_page().map(|o| (List::Liked, o)),
            View::Playlist => self.open.as_ref().and_then(|o| {
                o.tracks
                    .wants_page()
                    .map(|off| (List::Playlist(o.playlist.id.clone()), off))
            }),
        };
        if let Some((list, offset)) = request {
            if list == List::Playlists {
                self.playlists.loading = true;
            } else if let Some(l) = self.listing_for(&list) {
                l.loading = true;
            }
            self.send(Cmd::Load { list, offset });
        }
    }

    fn listing_for(&mut self, list: &List) -> Option<&mut Listing<Track>> {
        match list {
            List::Search(q) if *q == self.searched => Some(&mut self.search),
            List::Liked => Some(&mut self.liked),
            List::Playlist(id) => self
                .open
                .as_mut()
                .filter(|o| &o.playlist.id == id)
                .map(|o| &mut o.tracks),
            _ => None,
        }
    }

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Done(kind) => {
                self.in_flight = self.in_flight.saturating_sub(1);
                match kind {
                    Kind::Poll => self.poll_pending = false,
                    Kind::Volume => self.vol_in_flight = false,
                    Kind::Player => self.poll_soon(),
                    Kind::Other => {}
                }
            }
            Msg::User(user) => self.user_id = Some(user.id),
            Msg::Device(name) => {
                self.device_checked = true;
                self.device = name;
            }
            Msg::Playback(pb) => {
                let mut pb = pb.map(|b| *b);
                let new_uri = pb.as_ref().and_then(|p| p.item.as_ref()).map(|t| t.uri.clone());
                if let (Some((v, at)), Some(d)) = (self.vol_set, pb.as_mut().and_then(|p| p.device.as_mut())) {
                    if at.elapsed() < VOLUME_HOLD {
                        d.volume_percent = Some(v);
                    }
                }
                // Spotify's state lags a local command: keep what the user just did until the
                // poll agrees or the hold runs out.
                let local_now = self.progress_ms();
                if let (Some((held, at)), Some(p)) = (self.play_hold, pb.as_mut()) {
                    if p.is_playing == held || at.elapsed() >= PLAYER_HOLD {
                        self.play_hold = None;
                    } else {
                        p.is_playing = held;
                        p.progress_ms = Some(local_now);
                    }
                }
                if let (Some(at), Some(p)) = (self.seek_hold, pb.as_mut()) {
                    let agrees = p.progress_ms.is_some_and(|ms| ms.abs_diff(local_now) < 1500);
                    if agrees || at.elapsed() >= PLAYER_HOLD {
                        self.seek_hold = None;
                    } else {
                        p.progress_ms = Some(local_now);
                    }
                }
                self.playback = pb;
                self.polled_at = Instant::now();
                self.poll_pending = false;
                self.next_poll = Instant::now() + if std::mem::take(&mut self.repoll) { CONFIRM_AFTER } else { POLL_EVERY };
                if let Some(d) = self.playback.as_ref().and_then(|p| p.device.as_ref()) {
                    self.device = Some(d.name.clone());
                }
                match new_uri {
                    Some(uri) if self.saved.as_ref().is_none_or(|(u, _)| *u != uri) => {
                        self.saved = None;
                        if uri.starts_with("spotify:track:") {
                            self.send(Cmd::CheckSaved(uri));
                        }
                    }
                    Some(_) => {}
                    None => self.saved = None,
                }
            }
            Msg::Tracks { list, offset, page } => {
                if let Some(listing) = self.listing_for(&list) {
                    listing.apply(offset, page);
                }
            }
            Msg::Playlists { offset, page } => {
                self.playlists.apply(offset, page);
                self.rebuild_rows();
            }
            Msg::Folders(tree) => self.set_folders(tree),
            Msg::LoadFailed { list, forbidden } => {
                if list == List::Playlists {
                    self.playlists.loading = false;
                    self.playlists.total = Some(self.playlists.fetched);
                } else if let Some(listing) = self.listing_for(&list) {
                    listing.loading = false;
                    listing.total = Some(listing.fetched);
                }
                if forbidden {
                    if let List::Playlist(_) = list {
                        self.error("Spotify only lists tracks of playlists you own. Press P to play it.");
                    }
                }
            }
            Msg::Saved { uri, saved } => self.saved = Some((uri, saved)),
            Msg::Info(text) => self.info(text),
            Msg::Error(text) => self.error(text),
            Msg::Backoff(secs) => self.next_poll = Instant::now() + Duration::from_secs(secs.max(5)),
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.typing = false;
    }

    fn play_selected(&mut self) {
        match self.view() {
            View::Search => {
                let uris: Vec<String> = self.search.items.iter().map(|t| t.uri.clone()).collect();
                if !uris.is_empty() {
                    let offset = self.search.selected();
                    self.send(Cmd::Play(Play::Tracks { uris, offset }));
                    self.poll_soon();
                }
            }
            View::Liked => {
                if let Some(t) = self.liked.current() {
                    let from = t.uri.clone();
                    self.send(Cmd::PlayLiked { from });
                    self.poll_soon();
                }
            }
            View::Playlist => {
                let Some(open) = &self.open else { return };
                if let Some(t) = open.tracks.current() {
                    let play = Play::Context {
                        uri: open.playlist.uri.clone(),
                        from: Some(t.uri.clone()),
                    };
                    self.send(Cmd::Play(play));
                    self.poll_soon();
                }
            }
            View::Playlists => {
                if let Some(Row::Folder { index, .. }) = self.foldered().then(|| self.rows.current()).flatten() {
                    let index = *index;
                    self.open_folder(index);
                    return;
                }
                let Some(pl) = self.current_playlist() else { return };
                if self.owns(&pl) {
                    self.open = Some(OpenPlaylist {
                        playlist: pl,
                        tracks: Listing::default(),
                    });
                } else {
                    self.play_playlist(pl);
                }
            }
        }
    }

    fn play_playlist(&mut self, pl: Playlist) {
        self.info(format!("Playing {}", pl.name));
        self.send(Cmd::Play(Play::Context { uri: pl.uri, from: None }));
        self.poll_soon();
    }

    fn play_whole_playlist(&mut self) {
        let pl = match self.view() {
            View::Playlists => self.current_playlist(),
            View::Playlist => self.open.as_ref().map(|o| o.playlist.clone()),
            _ => None,
        };
        if let Some(pl) = pl {
            self.play_playlist(pl);
        }
    }

    fn queue_selected(&mut self) {
        if let Some(t) = self.tracks().and_then(|l| l.current()) {
            let cmd = Cmd::Queue {
                uri: t.uri.clone(),
                name: t.name.clone(),
            };
            self.send(cmd);
        }
    }

    fn toggle_like(&mut self) {
        if !self.can_like {
            self.error("Run `spotlite auth` again to allow liking songs.");
            return;
        }
        match self.saved.clone() {
            Some((uri, saved)) => self.send(Cmd::SetSaved { uri, saved: !saved }),
            None => self.info("Nothing to like yet."),
        }
    }

    fn seek(&mut self, forward: bool) {
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

    fn seek_absolute(&mut self, target: u64) {
        // Show the jump at once; the next poll corrects it.
        if let Some(pb) = self.playback.as_mut() {
            pb.progress_ms = Some(target);
        }
        self.polled_at = Instant::now();
        self.seek_hold = Some(Instant::now());
        self.player(Ctl::Seek(target));
    }

    fn change_volume(&mut self, up: bool) {
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

    fn toggle_mute(&mut self) {
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

    fn cycle_repeat(&mut self) {
        let next = match self.playback.as_ref().map(|pb| pb.repeat_state.as_str()) {
            Some("off") | None => "context",
            Some("context") => "track",
            _ => "off",
        };
        if let Some(pb) = self.playback.as_mut() {
            pb.repeat_state = next.into();
        }
        self.info(format!("Repeat {}", match next {
            "context" => "all",
            "track" => "one",
            _ => "off",
        }));
        self.send(Cmd::Repeat(next));
    }

    fn toggle_shuffle(&mut self) {
        let on = !self.playback.as_ref().is_some_and(|pb| pb.shuffle_state);
        if let Some(pb) = self.playback.as_mut() {
            pb.shuffle_state = on;
        }
        self.info(format!("Shuffle {}", if on { "on" } else { "off" }));
        self.send(Cmd::Shuffle(on));
    }

    fn submit_search(&mut self) {
        let q = self.query.trim().to_owned();
        self.typing = false;
        if q.is_empty() {
            return;
        }
        self.searched = q;
        self.search.reset();
        // tick() loads the first page.
    }

    fn reload(&mut self) {
        match self.view() {
            View::Search => self.search.reset(),
            View::Playlists => {
                self.playlists.reset();
                self.rebuild_rows();
            }
            View::Liked => self.liked.reset(),
            View::Playlist => {
                if let Some(o) = self.open.as_mut() {
                    o.tracks.reset();
                }
            }
        }
        self.next_poll = Instant::now();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.view() == View::Playlists && self.foldered() {
            self.rows.move_by(delta);
        } else if self.view() == View::Playlists {
            self.playlists.move_by(delta);
        } else if let Some(l) = self.tracks_mut() {
            l.move_by(delta);
        }
    }

    fn list_len(&self) -> usize {
        match self.view() {
            View::Playlists if self.foldered() => self.rows.items.len(),
            View::Playlists => self.playlists.items.len(),
            _ => self.tracks().map_or(0, |l| l.items.len()),
        }
    }

    fn select(&mut self, i: usize) {
        if self.view() == View::Playlists && self.foldered() {
            self.rows.state.select(Some(i));
        } else if self.view() == View::Playlists {
            self.playlists.state.select(Some(i));
        } else if let Some(l) = self.tracks_mut() {
            l.state.select(Some(i));
        }
    }

    /// Row index under a point in the list, if there is an item there.
    fn row_at(&self, pos: Position) -> Option<usize> {
        let h = &self.hits;
        if !h.list.contains(pos) || h.row_height == 0 {
            return None;
        }
        let i = h.list_offset + ((pos.y - h.list.y) / h.row_height) as usize;
        (i < self.list_len()).then_some(i)
    }

    fn seek_to_fraction(&mut self, f: f64) -> Option<u64> {
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
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => self.on_click(pos),
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Some(Drag::Volume) => self.set_volume((fraction(self.hits.volume, m.column) * 100.0).round() as u8),
                Some(Drag::Progress) => {
                    self.seek_preview = self.seek_to_fraction(fraction(self.hits.progress, m.column));
                }
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

    fn on_click(&mut self, pos: Position) {
        let h = &self.hits;
        if let Some(&(_, tab)) = h.tabs.iter().find(|(r, _)| r.contains(pos)) {
            self.switch_tab(tab);
        } else if h.search_box.contains(pos) {
            self.tab = Tab::Search;
            self.typing = true;
        } else if h.back.contains(pos) {
            if self.open.take().is_none() {
                self.close_folder();
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
            self.toggle_like();
        } else if h.volume.contains(pos) {
            self.drag = Some(Drag::Volume);
            self.set_volume((fraction(h.volume, pos.x) * 100.0).round() as u8);
        } else if h.progress.contains(pos) {
            self.drag = Some(Drag::Progress);
            self.seek_preview = self.seek_to_fraction(fraction(h.progress, pos.x));
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

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.typing {
            match key.code {
                KeyCode::Esc => self.typing = false,
                KeyCode::Down => self.typing = false,
                KeyCode::Enter => self.submit_search(),
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char('u') if ctrl => self.query.clear(),
                KeyCode::Char(c) if !ctrl => self.query.push(c),
                _ => {}
            }
            return;
        }
        let page = 10;
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('v') => {
                self.viz_on = !self.viz_on;
                self.info(if self.viz_on { "Visualizer on" } else { "Visualizer off" });
            }
            KeyCode::Char('Q') => {
                self.quit = true;
                self.stop_services = true;
            }
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => {
                self.tab = Tab::Search;
                self.typing = true;
            }
            KeyCode::Tab => self.switch_tab(Tab::ALL[(self.tab.index() + 1) % Tab::ALL.len()]),
            KeyCode::BackTab => {
                self.switch_tab(Tab::ALL[(self.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len()])
            }
            KeyCode::Char('1') => self.switch_tab(Tab::Search),
            KeyCode::Char('2') => self.switch_tab(Tab::Playlists),
            KeyCode::Char('3') => self.switch_tab(Tab::Liked),
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') if self.view() == View::Playlist => {
                self.open = None;
            }
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') if self.view() == View::Playlists => {
                self.close_folder();
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => self.change_volume(true),
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => self.change_volume(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Char('d') if ctrl => self.move_selection(page),
            KeyCode::Char('u') if ctrl => self.move_selection(-page),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::Char('g') | KeyCode::Home => self.move_selection(isize::MIN / 2),
            KeyCode::Char('G') | KeyCode::End => self.move_selection(isize::MAX / 2),
            KeyCode::Enter | KeyCode::Char('l') => self.play_selected(),
            KeyCode::Char('P') => self.play_whole_playlist(),
            KeyCode::Char('a') => self.queue_selected(),
            KeyCode::Char(' ') => self.toggle_play(),
            KeyCode::Char('n') => self.skip(true),
            KeyCode::Char('p') => self.skip(false),
            KeyCode::Right => self.seek(true),
            KeyCode::Left => self.seek(false),
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char(']') => self.change_volume(true),
            KeyCode::Char('-') | KeyCode::Char('[') => self.change_volume(false),
            KeyCode::Char('m') => self.toggle_mute(),
            KeyCode::Char('s') => self.toggle_shuffle(),
            KeyCode::Char('r') => self.cycle_repeat(),
            KeyCode::Char('f') => self.toggle_like(),
            KeyCode::Char('R') => self.reload(),
            _ => {}
        }
    }
}

#[cfg(test)]
pub(crate) fn test_app() -> (App, std::sync::mpsc::Receiver<Cmd>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (App::new(tx, true, Images::disabled()), rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{PAGE, SEARCH_PAGE};

    fn track(n: usize) -> Track {
        serde_json::from_value(serde_json::json!({"uri": format!("spotify:track:{n}"), "name": format!("T{n}")}))
            .unwrap()
    }

    fn page(range: std::ops::Range<usize>, total: usize) -> Page<Track> {
        let items: Vec<Track> = range.map(track).collect();
        Page { raw: items.len(), items, total }
    }

    fn drain(rx: &std::sync::mpsc::Receiver<Cmd>) -> Vec<Cmd> {
        rx.try_iter().collect()
    }

    #[test]
    fn liked_pages_load_as_you_scroll() {
        let (mut app, rx) = test_app();
        drain(&rx);
        app.switch_tab(Tab::Liked);
        app.tick();
        let cmds = drain(&rx);
        assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Liked, offset: 0 }]), "{cmds:?}");

        app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: page(0..PAGE, 120) });
        app.tick();
        assert!(drain(&rx).is_empty(), "no prefetch while far from the end");

        app.move_selection(45);
        app.tick();
        let cmds = drain(&rx);
        assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Liked, offset: 50 }]), "{cmds:?}");
        app.tick();
        assert!(drain(&rx).is_empty(), "no duplicate request while loading");

        app.on_msg(Msg::Tracks { list: List::Liked, offset: 50, page: page(50..100, 120) });
        assert_eq!(app.liked.items.len(), 100);
        assert_eq!(app.liked.selected(), 45);
    }

    #[test]
    fn playlists_load_once_per_page() {
        let (mut app, rx) = test_app();
        drain(&rx);
        app.switch_tab(Tab::Playlists);
        app.tick();
        app.tick();
        let cmds = drain(&rx);
        assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Playlists, offset: 0 }]), "{cmds:?}");
    }

    #[test]
    fn search_ignores_stale_results() {
        let (mut app, rx) = test_app();
        app.query = "first".into();
        app.submit_search();
        app.tick();
        app.query = "second".into();
        app.submit_search();
        app.tick();
        drain(&rx);
        app.on_msg(Msg::Tracks { list: List::Search("first".into()), offset: 0, page: page(0..10, 10) });
        assert!(app.search.items.is_empty());
        app.on_msg(Msg::Tracks { list: List::Search("second".into()), offset: 0, page: page(0..10, 30) });
        assert_eq!(app.search.items.len(), 10);
        assert_eq!(app.search.total, Some(30));
    }

    #[test]
    fn null_entries_still_advance_the_offset() {
        let mut l: Listing<Track> = Listing::default();
        let mut p = page(0..48, 120);
        p.raw = 50; // two removed tracks came back as null
        l.apply(0, p);
        assert_eq!(l.items.len(), 48);
        l.state.select(Some(45));
        assert_eq!(l.wants_page(), Some(50));
    }

    #[test]
    fn empty_page_ends_the_list() {
        let mut l: Listing<Track> = Listing::default();
        l.apply(0, page(0..3, 10));
        assert_eq!(l.wants_page(), Some(3));
        l.apply(3, page(0..0, 10));
        assert_eq!(l.total, Some(3));
        assert!(l.wants_page().is_none());
    }

    #[test]
    fn liked_plays_as_collection_and_search_as_uris() {
        let (mut app, rx) = test_app();
        app.switch_tab(Tab::Liked);
        app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: page(0..5, 5) });
        app.move_selection(2);
        drain(&rx);
        app.play_selected();
        let cmds = drain(&rx);
        assert!(matches!(cmds.as_slice(), [Cmd::PlayLiked { from }] if from == "spotify:track:2"), "{cmds:?}");

        app.switch_tab(Tab::Search);
        app.searched = "x".into();
        app.on_msg(Msg::Tracks { list: List::Search("x".into()), offset: 0, page: page(0..SEARCH_PAGE, 50) });
        app.play_selected();
        let cmds = drain(&rx);
        assert!(
            matches!(cmds.as_slice(), [Cmd::Play(Play::Tracks { uris, offset: 0 })] if uris.len() == SEARCH_PAGE),
            "{cmds:?}"
        );
    }

    #[test]
    fn forbidden_playlist_closes_loading_and_explains() {
        let (mut app, _rx) = test_app();
        let pl: Playlist = serde_json::from_value(serde_json::json!({
            "id": "p1", "uri": "spotify:playlist:p1", "name": "Theirs", "owner": {"id": "someone"}
        }))
        .unwrap();
        app.tab = Tab::Playlists;
        app.open = Some(OpenPlaylist { playlist: pl, tracks: Listing::default() });
        app.tick();
        app.on_msg(Msg::LoadFailed { list: List::Playlist("p1".into()), forbidden: true });
        let open = app.open.as_ref().unwrap();
        assert!(!open.tracks.loading);
        assert!(open.tracks.wants_page().is_none());
        assert!(app.toast.as_ref().unwrap().text.contains("Press P"));
    }

    fn playing(volume: u8, duration_ms: u64) -> Msg {
        Msg::Playback(Some(Box::new(
            serde_json::from_value(serde_json::json!({
                "device": {"id": "d", "name": "spotlite", "is_active": true, "volume_percent": volume},
                "is_playing": true, "progress_ms": 0,
                "item": {"uri": "spotify:track:x", "name": "X", "duration_ms": duration_ms}
            }))
            .unwrap(),
        )))
    }

    fn volumes(cmds: &[Cmd]) -> Vec<u8> {
        cmds.iter().filter_map(|c| if let Cmd::Volume(v) = c { Some(*v) } else { None }).collect()
    }

    #[test]
    fn volume_changes_are_coalesced() {
        let (mut app, rx) = test_app();
        app.on_msg(playing(40, 1000));
        drain(&rx);
        for _ in 0..4 {
            app.on_key(KeyEvent::from(KeyCode::Char('+')));
        }
        assert_eq!(app.volume(), Some(60), "shown at once");
        app.tick();
        assert_eq!(volumes(&drain(&rx)), vec![60], "one request for four presses");
        app.on_key(KeyEvent::from(KeyCode::Char('-')));
        app.tick();
        assert!(volumes(&drain(&rx)).is_empty(), "waits for the first to finish");
        app.on_msg(Msg::Done(Kind::Volume));
        app.tick();
        assert_eq!(volumes(&drain(&rx)), vec![55]);
        // A poll with the old volume does not move the slider back.
        app.on_msg(playing(40, 1000));
        assert_eq!(app.volume(), Some(55));
    }

    #[test]
    fn local_controls_use_mpris_and_show_at_once() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (ctl_tx, ctl_rx) = std::sync::mpsc::channel();
        let mut app = App::new(tx, true, Images::disabled()).with_control(ctl_tx, "spotlite".into());
        app.on_msg(playing(50, 200_000));
        app.poll_pending = false;
        drain(&rx);

        app.on_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(!app.playback.as_ref().unwrap().is_playing, "paused on screen at once");
        assert_eq!(ctl_rx.try_recv().unwrap(), Ctl::Pause);
        assert!(drain(&rx).iter().all(|c| !matches!(c, Cmd::Toggle)), "not sent to the Web API");

        // A poll that still says "playing" does not undo the pause.
        app.on_msg(playing(50, 200_000));
        assert!(!app.playback.as_ref().unwrap().is_playing);

        app.on_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(ctl_rx.try_recv().unwrap(), Ctl::Seek(10_000));
        app.on_key(KeyEvent::from(KeyCode::Char('n')));
        assert_eq!(ctl_rx.try_recv().unwrap(), Ctl::Next);
        assert_eq!(app.progress_ms(), 0);
        app.tick();
        assert!(app.next_poll <= Instant::now() + CONFIRM_AFTER, "confirms soon");
    }

    #[test]
    fn other_device_uses_the_web_api() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (ctl_tx, ctl_rx) = std::sync::mpsc::channel();
        let mut app = App::new(tx, true, Images::disabled()).with_control(ctl_tx, "spotlite".into());
        let mut msg = playing(50, 200_000);
        if let Msg::Playback(Some(pb)) = &mut msg {
            pb.device.as_mut().unwrap().name = "Phone".into();
        }
        app.on_msg(msg);
        drain(&rx);
        app.on_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(ctl_rx.try_recv().is_err());
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Toggle)));
    }

    fn pl(id: &str, name: &str) -> Playlist {
        serde_json::from_value(serde_json::json!({
            "id": id, "uri": format!("spotify:playlist:{id}"), "name": name, "owner": {"id": "me"}
        }))
        .unwrap()
    }

    #[test]
    fn folders_browse_and_play() {
        let (mut app, rx) = test_app();
        app.on_msg(Msg::User(serde_json::from_value(serde_json::json!({"id": "me", "uri": "spotify:user:me"})).unwrap()));
        app.set_folders(crate::folders::parse_rootlist([
            "spotify:playlist:a",
            "spotify:start-group:1:Gym",
            "spotify:playlist:b",
            "spotify:start-group:2:Heavy",
            "spotify:playlist:c",
            "spotify:end-group:2",
            "spotify:end-group:1",
        ]));
        app.switch_tab(Tab::Playlists);
        drain(&rx);
        app.tick();
        let cmds = drain(&rx);
        assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Playlists, offset: 0 }]), "{cmds:?}");
        let page = Page { items: vec![pl("a", "A"), pl("b", "B"), pl("c", "C"), pl("new", "New one")], raw: 4, total: 120 };
        app.on_msg(Msg::Playlists { offset: 0, page });
        app.tick();
        let cmds = drain(&rx);
        assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Playlists, offset: 4 }]), "loads every page: {cmds:?}");

        let names = |app: &App| -> Vec<String> {
            app.rows.items.iter().map(|r| match r {
                Row::Folder { name, count, .. } => format!("[{name} {count}]"),
                Row::Playlist(p) => p.name.clone(),
            }).collect()
        };
        assert_eq!(names(&app), ["A", "[Gym 2]", "New one"], "untracked playlists go last");

        app.move_selection(1);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(names(&app), ["B", "[Heavy 1]"]);
        assert_eq!(app.folder_names(), ["Gym"]);
        app.move_selection(1);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(names(&app), ["C"]);
        app.on_key(KeyEvent::from(KeyCode::Char('P')));
        let cmds = drain(&rx);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Play(Play::Context { uri, .. }) if uri == "spotify:playlist:c")), "{cmds:?}");

        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(names(&app), ["B", "[Heavy 1]"]);
        assert_eq!(app.rows.selected(), 1, "selection comes back");
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.rows.selected(), 1);
        assert!(app.folder_names().is_empty());
    }

    #[test]
    fn a_tree_without_folders_keeps_the_flat_list() {
        let (mut app, _rx) = test_app();
        app.set_folders(crate::folders::parse_rootlist(["spotify:playlist:a"]));
        assert!(!app.foldered());
    }

    #[test]
    fn mute_and_unmute() {
        let (mut app, rx) = test_app();
        app.on_msg(playing(35, 1000));
        app.on_key(KeyEvent::from(KeyCode::Char('m')));
        assert!(app.muted());
        app.on_key(KeyEvent::from(KeyCode::Char('m')));
        assert_eq!(app.volume(), Some(35));
        assert!(!app.muted());
        drain(&rx);
    }

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_selects_plays_and_drags() {
        let (mut app, rx) = test_app();
        app.switch_tab(Tab::Liked);
        app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: page(0..20, 20) });
        app.on_msg(playing(50, 200_000));
        app.hits.list = Rect::new(2, 5, 60, 10);
        app.hits.row_height = 1;
        app.hits.list_offset = 4;
        app.hits.volume = Rect::new(70, 30, 11, 1);
        app.hits.progress = Rect::new(10, 29, 101, 1);
        drain(&rx);

        app.on_mouse(click(10, 7));
        assert_eq!(app.liked.selected(), 6, "row 2 of the view + offset 4");
        assert!(drain(&rx).is_empty(), "one click only selects");
        app.on_mouse(click(10, 7));
        let cmds = drain(&rx);
        assert!(matches!(cmds.as_slice(), [Cmd::PlayLiked { from }] if from == "spotify:track:6"), "{cmds:?}");

        app.on_mouse(click(10, 14 + 5));
        assert_eq!(app.liked.selected(), 6, "click below the list does nothing");

        app.on_mouse(click(75, 30));
        assert_eq!(app.volume(), Some(50), "middle of the slider");
        app.on_mouse(MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), ..click(80, 30) });
        assert_eq!(app.volume(), Some(100));
        app.on_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), ..click(80, 30) });

        app.on_mouse(click(60, 29));
        assert_eq!(app.progress_ms(), 100_000, "drag preview");
        app.on_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), ..click(60, 29) });
        let cmds = drain(&rx);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Seek(100_000))), "{cmds:?}");

        app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, ..click(10, 7) });
        assert_eq!(app.liked.selected(), 9);
    }

    #[test]
    fn typing_keys_go_to_the_query() {
        let (mut app, _rx) = test_app();
        app.on_key(KeyEvent::from(KeyCode::Char('/')));
        for c in "q1 /".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert!(!app.quit);
        assert_eq!(app.query, "q1 /");
    }
}
