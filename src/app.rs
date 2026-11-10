use crate::api::{Album, Found, Page as ApiPage, Play, Playback, Playlist, Track};
use crate::control::Ctl;
use crate::folders::Entry;
use crate::images::Images;
use crate::pages::{AlbumPage, ArtistPage, ArtistRow, FilterItem, Page, RadioPage, RandomPage};
use crate::radio::Library;
use crate::sidebar::{Item as SideItem, Pin, SideFx, SideView, Sidebar, SongAct};
use crate::state::{SavedPlayback, SavedTab, State};
use crate::undo::{NavTree, Node as NavNode};
use crate::viz::Visualizer;
use crate::worker::{Cmd, Kind, List, Msg};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::TableState;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// How often to ask Spotify for the player state. Progress is interpolated between polls.
/// How long the like animation runs.
pub const LIKE_BURST: Duration = Duration::from_millis(720);
const POLL_EVERY: Duration = Duration::from_secs(5);
/// Search as you type: the pause after the last key before the search starts.
const SEARCH_DELAY: Duration = Duration::from_millis(300);
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    Search,
    Playlists,
    Liked,
    Recent,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Search, Tab::Playlists, Tab::Liked, Tab::Recent];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Search => "Search",
            Tab::Playlists => "Playlists",
            Tab::Liked => "Liked",
            Tab::Recent => "Recent",
        }
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

    fn apply(&mut self, offset: usize, page: ApiPage<T>) {
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

/// A place in the tab bar: a fixed tab, or an open page (by its key).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Loc {
    Base(Tab),
    Page(String),
}

/// Open page tabs at most; opening one more closes the oldest.
pub const MAX_PAGES: usize = 8;

/// What the search lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SearchKind {
    Songs,
    Artists,
    Albums,
    Playlists,
}

impl SearchKind {
    pub const ALL: [SearchKind; 4] = [SearchKind::Songs, SearchKind::Artists, SearchKind::Albums, SearchKind::Playlists];

    pub fn title(self) -> &'static str {
        match self {
            SearchKind::Songs => "Songs",
            SearchKind::Artists => "Artists",
            SearchKind::Albums => "Albums",
            SearchKind::Playlists => "Playlists",
        }
    }

    /// The Web API's search type (None for songs, which have their own list).
    fn api(self) -> Option<&'static str> {
        match self {
            SearchKind::Songs => None,
            SearchKind::Artists => Some("artist"),
            SearchKind::Albums => Some("album"),
            SearchKind::Playlists => Some("playlist"),
        }
    }
}

/// What the main panel shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Search,
    Playlists,
    Playlist,
    Liked,
    /// Recently played (the last 50 songs).
    Recent,
    Artist,
    Album,
    Radio,
    /// Randomize: random songs of one genre.
    Random,
}

/// What a menu entry does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Play,
    Queue,
    Artist { id: String, name: String },
    Album(Album),
    Radio,
    /// Open the genre picker, to randomize a genre.
    PickGenre,
    AddToPlaylist,
    RemoveFromPlaylist,
    Like,
    Filter(FilterItem),
    /// A theme by name (the theme picker, `T`).
    Theme(String),
    /// A sidebar effect (the effect menu, `V`).
    SideFx(SideFx),
}

/// A pop-up list of actions: for a track (`.` or right-click), or the radio filters (`F`).
pub struct Menu {
    pub title: String,
    /// (key hint, label, action); the label of a filter shows its current value.
    pub items: Vec<(&'static str, String, Action)>,
    pub cursor: usize,
    /// The track the actions apply to (None for the filter and theme menus).
    pub track: Option<Track>,
    /// Where the right-click was: the menu opens there (None: in the middle).
    pub at: Option<Position>,
}

impl Menu {
    pub fn is_theme(&self) -> bool {
        matches!(self.items.first(), Some((_, _, Action::Theme(_))))
    }
}

/// A genre sample: about 10 s of a genre's top song, in the genre picker.
pub struct Sample {
    pub genre: String,
    /// Which of the genre's top songs (Tab again on the same genre: the next one).
    pub nth: usize,
    /// The song, once it plays.
    pub track: Option<Track>,
    /// When it started playing; the sample ends `SAMPLE_LEN` later.
    pub started: Option<Instant>,
}

/// How long a genre sample plays.
pub const SAMPLE_LEN: Duration = Duration::from_secs(10);
/// The volume fades out this fast before a switch,
/// A visualizer sweeps in and out over these many seconds.
const VIZ_IN: f32 = 0.6;
const VIZ_OUT: f32 = 0.4;

/// How far a visualizer is in (0..1), `on` or not since `at` (None: long ago).
fn sweep(on: bool, at: Option<Instant>) -> f32 {
    let since = at.map_or(f32::MAX, |t| t.elapsed().as_secs_f32());
    if on {
        (since / VIZ_IN).min(1.0)
    } else {
        (1.0 - since / VIZ_OUT).max(0.0)
    }
}
const FADE_OUT: Duration = Duration::from_millis(150);
/// waits this long after the switch for the new song to start (spotifyd starts it a moment
/// after Spotify answers; fading in sooner makes it cut in loud),
const SWITCH_WAIT: Duration = Duration::from_millis(450);
/// and fades back in this slowly.
const FADE_IN: Duration = Duration::from_millis(350);

/// One step of a fade.
#[derive(Debug)]
pub enum FadeStep {
    /// Volume to `to`, smoothly, over the time.
    Ramp { to: u8, len: Duration },
    Wait(Duration),
    /// A command to send now (the switch).
    Send(Cmd),
}

/// Choose a playlist to add a track to; type to narrow the list.
pub struct Picker {
    pub track: Track,
    pub query: String,
    pub cursor: usize,
}

/// A yes/no question before a change that cannot be undone from spotlite.
pub struct Confirm {
    pub text: String,
    pub cmd: Cmd,
}

/// Where clickable things were drawn in the last frame (filled by ui::draw).
#[derive(Default, Debug)]
pub struct Hits {
    pub tabs: Vec<(Rect, Loc)>,
    /// Song / artist / album / playlist switches in the search results title.
    pub search_kinds: Vec<(Rect, SearchKind)>,
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
    /// Now playing: artist name and album (title, cover) open their pages.
    pub np_artist: Rect,
    pub np_album: Rect,
    /// Rows of an open menu or picker, and the index each one stands for.
    pub popup_rows: Vec<(Rect, usize)>,
    pub popup: Rect,
    /// The sidebar, its Playing / Library switch, and its rows (with the item index).
    pub side: Rect,
    pub side_tabs: [Rect; 2],
    pub side_rows: Vec<(Rect, usize)>,
    /// The sidebar's hide button, and the tab at the left edge that opens it again.
    pub side_hide: Rect,
    pub side_open: Rect,
    /// The "+N ▾" chip for tabs that do not fit, its list, the list's rows and their "×".
    pub tabs_more: Rect,
    pub tabs_list: Rect,
    pub tabs_rows: Vec<(Rect, usize)>,
    pub tabs_close: Vec<(Rect, usize)>,
}

/// The list of open pages under the "+N ▾" chip. Hovering the chip opens it and leaving closes
/// it; a click on the chip keeps it open until the next click.
#[derive(Debug, Default, Clone, Copy)]
pub struct TabsList {
    pub pinned: bool,
    /// The page row under the pointer.
    pub hover: Option<usize>,
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
    /// When to search the typed text (search as you type), set by each key in the search box.
    search_due: Option<Instant>,
    pub search_kind: SearchKind,
    /// Artists, albums or playlists found (when search_kind is not Songs).
    pub found: Listing<Found>,
    pub playlists: Listing<Playlist>,
    pub open: Option<OpenPlaylist>,
    pub liked: Listing<Track>,
    pub recent: Listing<Track>,
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
    /// The effect over the sidebar's song (V), when it last changed, the one it replaced (it
    /// sweeps out), and the one before the effect menu opened (Esc puts it back).
    pub side_fx: SideFx,
    pub side_fx_at: Option<Instant>,
    side_fx_out: SideFx,
    side_fx_before: Option<SideFx>,
    pub liquid: crate::liquid::Liquid,
    /// When the main visualizer (v) was last turned on or off, for its sweep.
    pub viz_at: Option<Instant>,
    pub side_peaks: crate::viz::Peaks,
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
    /// Shuffle and repeat set locally, kept until a poll agrees or the hold ends.
    shuffle_hold: Option<(bool, Instant)>,
    repeat_hold: Option<(&'static str, Instant)>,
    seek_hold: Option<Instant>,
    /// A poll was asked for while one was already running.
    repoll: bool,
    /// Artist, album and radio pages, each in its own tab after Search, Playlists and Liked.
    pub pages: Vec<Page>,
    /// The page tab shown; None shows `tab`.
    pub active: Option<usize>,
    /// Where you were before, for Esc (newest last).
    /// Where you went, as an undo tree (Backspace goes back; Ctrl+Backspace shows it).
    pub nav: NavTree,
    /// The undo tree popup: the row the cursor is on.
    pub undo: Option<usize>,
    /// The list of open pages under the "+N ▾" chip.
    pub tabs_list: Option<TabsList>,
    /// Pages restored at start that have not loaded yet (by key): they load when shown.
    unloaded: std::collections::HashSet<String>,
    /// Lists Spotify limits for now: each loads again at its time.
    retries: Vec<(List, Instant)>,
    /// spotlite's own shuffle (Fisher-Yates over the whole list). Spotify's is kept off.
    pub true_shuffle: bool,
    /// Randomize: the genre picker (quick picks, search, folder tree), and its genres.
    pub genre_picker: Option<crate::genres::Picker>,
    pub genre_list: Vec<crate::genres::Genre>,
    /// A genre sample playing in the picker (Tab), and what played before it.
    pub sample: Option<Sample>,
    sample_restore: Option<crate::worker::Restore>,
    sample_volume: Option<u8>,
    /// Volume fades and the commands between them (a sample's switch), run by `tick`.
    fade: std::collections::VecDeque<FadeStep>,
    /// When the first step of `fade` began, and the volume then.
    fade_at: Option<(Instant, u8)>,
    /// The list a true shuffle plays, and its songs: the player shows that list as the source.
    shuffled: Option<(String, std::collections::HashSet<String>)>,
    /// The theme in use when the picker opened (Esc goes back to it).
    theme_before: Option<String>,
    /// Live reload: when the theme file and the saved state were last looked at, and their times.
    theme_watch: (Instant, Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    pub menu: Option<Menu>,
    pub picker: Option<Picker>,
    pub palette: Option<crate::palette::Palette>,
    /// The song after the playing one, when Spotify said.
    pub up_next: Option<Track>,
    /// What played last (kept when playback stops), saved for the next start.
    pub last_playback: Option<SavedPlayback>,
    /// What to start again if nothing plays at start, and when spotlite started.
    resume: Option<(SavedPlayback, Instant)>,
    /// The first player state has arrived.
    polled_once: bool,
    /// The heart's animation: when it started, and whether the song became liked.
    pub like_burst: Option<(Instant, bool)>,
    /// A like asked for without knowing the state: the animation waits for Spotify's answer.
    like_pending: Option<String>,
    pub confirm: Option<Confirm>,
    /// Liked Songs index for the radio filters; asked for once, when a radio opens.
    pub library: Option<Library>,
    library_asked: bool,
    /// Playlist edits allowed by the saved login (playlist-modify-*).
    pub can_edit: bool,
    /// The left sidebar: the playing song, or your library.
    pub sidebar: Sidebar,
    /// Songs, albums and playlists pinned to the top of the sidebar's Library.
    pub pins: Vec<Pin>,
    /// The screen width at the last draw (the sidebar needs room).
    pub screen_w: u16,
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
            search_due: None,
            search_kind: SearchKind::Songs,
            found: Listing::default(),
            playlists: Listing::default(),
            open: None,
            liked: Listing::default(),
            recent: Listing::default(),
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
            side_fx: SideFx::Off,
            side_fx_at: None,
            side_fx_out: SideFx::Off,
            side_fx_before: None,
            liquid: Default::default(),
            viz_at: None,
            side_peaks: Default::default(),
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
            shuffle_hold: None,
            repeat_hold: None,
            seek_hold: None,
            repoll: false,
            pages: Vec::new(),
            active: None,
            nav: NavTree::new(Loc::Base(Tab::Search), "⌕ Search".into()),
            undo: None,
            tabs_list: None,
            retries: Vec::new(),
            true_shuffle: false,
            genre_picker: None,
            genre_list: crate::genres::all(),
            sample: None,
            sample_restore: None,
            sample_volume: None,
            fade: Default::default(),
            fade_at: None,
            shuffled: None,
            unloaded: Default::default(),
            theme_before: None,
            theme_watch: (Instant::now(), None, None),
            menu: None,
            picker: None,
            palette: None,
            up_next: None,
            last_playback: None,
            resume: None,
            polled_once: false,
            like_burst: None,
            like_pending: None,
            confirm: None,
            library: None,
            library_asked: false,
            can_edit: true,
            sidebar: Sidebar::default(),
            pins: Vec::new(),
            screen_w: 0,
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
        let cmd = self.with_true_shuffle(cmd);
        if self.tx.send(cmd).is_ok() {
            self.in_flight += 1;
        }
    }

    /// With true shuffle on, a play command plays the whole list in a random order instead.
    fn with_true_shuffle(&mut self, cmd: Cmd) -> Cmd {
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

    /// True while the API thread has work, so the UI should check for replies often.
    pub fn busy(&self) -> bool {
        self.in_flight > 0
            || self.vol_target.is_some()
            || self.images.busy()
            || self.like_animating()
            || self.search_due.is_some()
            || self.sample.is_some()
            || !self.fade.is_empty()
    }

    pub fn info(&mut self, text: impl Into<String>) {
        let text = text.into();
        // Long messages stay longer: about 4 s, plus a second for each 20 characters.
        let secs = 4 + (text.chars().count() as u64 / 20).min(6);
        self.toast = Some(Toast { text, error: false, until: Instant::now() + Duration::from_secs(secs) });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: true,
            until: Instant::now() + Duration::from_secs(8),
        });
    }

    pub fn view(&self) -> View {
        match self.page() {
            Some(Page::Artist(_)) => return View::Artist,
            Some(Page::Album(_)) => return View::Album,
            Some(Page::Radio(_)) => return View::Radio,
            Some(Page::Random(_)) => return View::Random,
            None => {}
        }
        match self.tab {
            Tab::Search => View::Search,
            Tab::Playlists if self.open.is_some() => View::Playlist,
            Tab::Playlists => View::Playlists,
            Tab::Liked => View::Liked,
            Tab::Recent => View::Recent,
        }
    }

    /// Track list of the current view, if it shows tracks.
    pub fn tracks(&self) -> Option<&Listing<Track>> {
        match (self.view(), self.page()) {
            (View::Album, Some(Page::Album(a))) => Some(&a.tracks),
            (View::Radio, Some(Page::Radio(r))) => Some(&r.tracks),
            (View::Random, Some(Page::Random(r))) => Some(&r.tracks),
            (View::Search, _) if self.search_kind == SearchKind::Songs => Some(&self.search),
            (View::Liked, _) => Some(&self.liked),
            (View::Recent, _) => Some(&self.recent),
            (View::Playlist, _) => self.open.as_ref().map(|o| &o.tracks),
            _ => None,
        }
    }

    pub fn tracks_mut(&mut self) -> Option<&mut Listing<Track>> {
        match self.view() {
            View::Search if self.search_kind == SearchKind::Songs => Some(&mut self.search),
            View::Search => None,
            View::Liked => Some(&mut self.liked),
            View::Recent => Some(&mut self.recent),
            View::Playlist => self.open.as_mut().map(|o| &mut o.tracks),
            View::Album => match self.page_mut() {
                Some(Page::Album(a)) => Some(&mut a.tracks),
                _ => None,
            },
            View::Radio => match self.page_mut() {
                Some(Page::Radio(r)) => Some(&mut r.tracks),
                _ => None,
            },
            View::Random => match self.page_mut() {
                Some(Page::Random(r)) => Some(&mut r.tracks),
                _ => None,
            },
            View::Playlists | View::Artist => None,
        }
    }

    fn artist_page_mut(&mut self) -> Option<&mut ArtistPage> {
        match self.page_mut() {
            Some(Page::Artist(a)) => Some(a),
            _ => None,
        }
    }

    /// The track under the cursor, in any view that lists tracks.
    pub fn selected_track(&self) -> Option<Track> {
        if let Some(Page::Artist(a)) = self.page() {
            return match a.rows.current()? {
                ArtistRow::Track(t) => Some(t.clone()),
                _ => None,
            };
        }
        self.tracks()?.current().cloned()
    }

    /// The selected track, else the playing one (for keys like `i` and `o`).
    fn target_track(&self) -> Option<Track> {
        self.selected_track().or_else(|| self.playback.as_ref()?.item.clone())
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

    fn set_side_fx(&mut self, fx: SideFx) {
        if fx == self.side_fx {
            return;
        }
        self.side_fx_out = self.side_fx;
        self.side_fx = fx;
        self.side_fx_at = Some(Instant::now());
    }

    /// V: the menu of sidebar effects. Moving in it shows each one; Esc puts the old one back.
    fn open_side_fx(&mut self) {
        if !self.sidebar.shows_song() {
            self.show_side(SideView::Playing);
        }
        let liquid = crate::theme::c().liquid.name();
        let items = vec![
            ("1", "Off".to_owned(), Action::SideFx(SideFx::Off)),
            ("2", format!("Liquid · {liquid}, drops that move to each band"), Action::SideFx(SideFx::Liquid)),
            ("3", "Spectrum · bars on their side".to_owned(), Action::SideFx(SideFx::Spectrum)),
        ];
        let cursor = items.iter().position(|(_, _, a)| *a == Action::SideFx(self.side_fx)).unwrap_or(0);
        self.side_fx_before = Some(self.side_fx);
        self.menu = Some(Menu { title: "Sidebar effect".into(), items, cursor, track: None, at: None });
    }

    /// True while a visualizer sweeps in or out.
    fn viz_sweeping(&self) -> bool {
        let r = self.side_fx_shown().1;
        let main = self.viz_reveal();
        (self.sidebar.shows_song() && r > 0.0 && r < 1.0) || (main > 0.0 && main < 1.0)
    }

    /// Called on every loop pass: polls the player and loads pages that are needed.
    /// Returns true when something on screen changed (a toast went away, a search started).
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if self.animating() {
            self.viz.ensure_running();
        } else if self.viz.running() {
            self.viz.stop();
        }
        let now = Instant::now();
        changed |= self.run_fade();
        if self.sample.as_ref().and_then(|s| s.started).is_some_and(|at| at.elapsed() >= SAMPLE_LEN) {
            self.stop_sample();
            changed = true;
        }
        if self.toast.as_ref().is_some_and(|t| now >= t.until) {
            self.toast = None;
            changed = true;
        }
        changed |= self.sidebar.step();
        if self.search_due.is_some_and(|due| now >= due) {
            self.search_due = None;
            changed |= self.live_search();
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
        self.watch_theme();
        self.try_resume();
        // The Liked index (♥ marks, radio filters) loads once the first lists are in, so it
        // does not hold up what is on screen.
        if !self.library_asked && self.user_id.is_some() && self.in_flight == 0 && !cfg!(test) {
            self.library_asked = true;
            self.send(Cmd::Library);
        }
        changed
    }

    /// How long the screen may stay as it is: the visualizer's frame while it runs, a spinner
    /// step while a request is open or the heart bursts, a quarter second while music moves,
    /// else one second (the progress bar and toasts).
    pub fn frame_period(&self, viz_frame: Duration) -> Duration {
        if self.sidebar.animating() || self.viz_sweeping() {
            Duration::from_millis(16)
        } else if !self.fade.is_empty() {
            // A volume fade: small steps, so it sounds smooth.
            Duration::from_millis(25)
        } else if self.animating() {
            viz_frame
        } else if self.in_flight > 0 || self.like_animating() {
            Duration::from_millis(120)
        } else if self.moving() {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        }
    }

    /// What plays now, as it would be saved.
    fn snapshot(&self) -> Option<SavedPlayback> {
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
        })
    }

    /// Starts again what played at the last quit, if nothing plays now. It waits for the first
    /// player state and for spotifyd to show up as a device (up to 20 s after start).
    fn try_resume(&mut self) {
        let Some((_, since)) = &self.resume else { return };
        if since.elapsed() > Duration::from_secs(20) {
            self.resume = None;
            return;
        }
        if !self.polled_once {
            return;
        }
        if self.playback.as_ref().is_some_and(|pb| pb.item.is_some()) {
            // Something plays already (spotifyd kept going): leave it alone.
            self.resume = None;
            return;
        }
        if self.device.is_none() {
            return;
        }
        if let Some((sp, _)) = self.resume.take() {
            self.send(Cmd::Resume(sp));
            self.poll_soon();
        }
    }

    /// Music is playing and the theme moves: the loop wakes 4 times a second.
    pub fn moving(&self) -> bool {
        crate::theme::motion() && self.playback.as_ref().is_some_and(|pb| pb.is_playing && pb.item.is_some())
    }

    fn load_more(&mut self) {
        let request = match self.view() {
            View::Search if self.searched.is_empty() => None,
            View::Search => match self.search_kind.api() {
                None => self.search.wants_page().map(|o| (List::Search(self.searched.clone()), o)),
                Some(kind) => {
                    self.found.wants_page().map(|o| (List::Found { query: self.searched.clone(), kind }, o))
                }
            },
            // The folder tree needs every playlist's name and cover, so load all pages at once.
            View::Playlists if self.foldered() => {
                let l = &self.playlists;
                (!l.loading && l.total.is_none_or(|t| l.fetched < t)).then_some((List::Playlists, l.fetched))
            }
            View::Playlists => self.playlists.wants_page().map(|o| (List::Playlists, o)),
            View::Liked => self.liked.wants_page().map(|o| (List::Liked, o)),
            View::Recent => self.recent.wants_page().map(|o| (List::Recent, o)),
            View::Playlist => self.open.as_ref().and_then(|o| {
                o.tracks
                    .wants_page()
                    .map(|off| (List::Playlist(o.playlist.id.clone()), off))
            }),
            View::Album => match self.page() {
                Some(Page::Album(a)) => a.tracks.wants_page().map(|off| (List::Album(a.id.clone()), off)),
                _ => None,
            },
            View::Artist | View::Radio | View::Random => None,
        };
        // The playlist picker lists every playlist, so it needs them all.
        let request = request.or_else(|| {
            let l = &self.playlists;
            let wanted = self.picker.is_some() || (self.sidebar.open && self.sidebar.view == SideView::Library);
            (wanted && !l.loading && l.total.is_none_or(|t| l.fetched < t))
                .then_some((List::Playlists, l.fetched))
        });
        // A list Spotify limited loads again when the limit ends.
        let now = Instant::now();
        let request = request.or_else(|| {
            let i = self.retries.iter().position(|(_, at)| *at <= now)?;
            Some((self.retries.remove(i).0, 0))
        });
        if let Some((list, offset)) = request {
            if offset == 0 {
                self.retries.retain(|(l, _)| *l != list);
            }
            if list == List::Playlists {
                self.playlists.loading = true;
            } else if matches!(list, List::Found { .. }) {
                self.found.loading = true;
            } else if let Some(l) = self.listing_for(&list) {
                l.loading = true;
            }
            self.send(Cmd::Load { list, offset });
        }
    }

    /// Spotify limits `list` for `secs`: say so once, and load it again then.
    fn retry_later(&mut self, list: List, secs: u64) {
        let at = Instant::now() + Duration::from_secs(secs.clamp(5, 6 * 3600));
        if let Some(r) = self.retries.iter_mut().find(|(l, _)| *l == list) {
            r.1 = at;
            return;
        }
        let what = match &list {
            List::Liked => "Liked Songs",
            List::Playlists => "your playlists",
            List::Recent => "recently played songs",
            _ => "this list",
        };
        let wait = if secs >= 120 { format!("{} min", secs.div_ceil(60)) } else { format!("{secs} s") };
        let copy = match &list {
            List::Liked if !self.liked.items.is_empty() => " It shows the saved copy until then.",
            List::Playlists if !self.playlists.items.is_empty() => " It shows the saved copy until then.",
            _ => "",
        };
        self.info(format!("Spotify limits {what} for {wait}; spotlite asks again then.{copy}"));
        self.retries.push((list, at));
    }

    fn listing_for(&mut self, list: &List) -> Option<&mut Listing<Track>> {
        match list {
            List::Search(q) if *q == self.searched => Some(&mut self.search),
            List::Liked => Some(&mut self.liked),
            List::Recent => Some(&mut self.recent),
            List::Playlist(id) => self
                .open
                .as_mut()
                .filter(|o| &o.playlist.id == id)
                .map(|o| &mut o.tracks),
            List::Album(id) => self.pages.iter_mut().rev().find_map(|p| match p {
                Page::Album(a) if &a.id == id => Some(&mut a.tracks),
                _ => None,
            }),
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
                if let Some(p) = pb.as_mut() {
                    self.own_shuffle(p);
                }
                if let (Some((held, at)), Some(p)) = (self.repeat_hold, pb.as_mut()) {
                    if p.repeat_state == held || at.elapsed() >= PLAYER_HOLD {
                        self.repeat_hold = None;
                    } else {
                        p.repeat_state = held.into();
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
                self.polled_once = true;
                if let Some(snap) = self.snapshot() {
                    self.last_playback = Some(snap);
                }
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
                        // A new song: ask what comes after it.
                        self.up_next = None;
                        self.send(Cmd::UpNext);
                    }
                    Some(_) => {}
                    None => self.saved = None,
                }
            }
            Msg::Tracks { list: List::Album(id), offset, mut page } => {
                if let Some(a) = self.pages.iter().rev().find_map(|p| match p {
                    Page::Album(a) if a.id == id => Some(a),
                    _ => None,
                }) {
                    page.items = page.items.into_iter().map(|t| a.adopt(t)).collect();
                }
                if let Some(listing) = self.listing_for(&List::Album(id)) {
                    listing.apply(offset, page);
                }
            }
            Msg::Artist { artist, top, albums } => {
                for p in self.pages.iter_mut() {
                    if let Page::Artist(a) = p {
                        if a.id == artist.id {
                            a.fill(artist.clone(), top.clone(), albums.clone());
                        }
                    }
                }
            }
            Msg::PageFailed { id, error } => {
                for p in self.pages.iter_mut() {
                    if let Page::Artist(a) = p {
                        if a.id == id {
                            a.rows.loading = false;
                            a.error = Some(error.clone());
                        }
                    }
                }
            }
            Msg::Album(album) => {
                for p in self.pages.iter_mut() {
                    if let Page::Album(a) = p {
                        if Some(&a.id) == album.id.as_ref() {
                            a.album = album.clone();
                            for t in a.tracks.items.iter_mut() {
                                if let Some(ta) = t.album.as_mut() {
                                    ta.images = album.images.clone();
                                }
                            }
                        }
                    }
                }
            }
            Msg::Shuffled { context, uris } => {
                self.shuffled = Some((context, uris.into_iter().collect()));
            }
            Msg::Sampling { genre, result } => match result {
                Ok(track) => {
                    if let Some(s) = self.sample.as_mut().filter(|s| s.genre == genre) {
                        s.track = Some(track);
                        // The 10 s count from when it can be heard.
                        s.started = Some(Instant::now() + SWITCH_WAIT);
                        if let Some(v) = self.sample_volume {
                            self.fade.push_back(FadeStep::Wait(SWITCH_WAIT));
                            self.fade.push_back(FadeStep::Ramp { to: v, len: FADE_IN });
                        }
                    }
                    self.poll_soon();
                }
                Err(e) => {
                    self.error(e);
                    if self.sample.as_ref().is_some_and(|s| s.genre == genre) {
                        self.sample = None;
                        // Nothing changed: what played goes on, back at its volume.
                        self.sample_restore = None;
                        self.fade.clear();
                        self.fade_at = None;
                        if let Some(v) = self.sample_volume {
                            self.fade.push_back(FadeStep::Ramp { to: v, len: FADE_IN / 2 });
                        }
                    }
                }
            },
            Msg::Random { genre, result } => {
                if let Err(e) = &result {
                    self.error(e.clone());
                } else if let Ok(t) = &result {
                    if t.len() < crate::genre::WANT {
                        self.info(format!("Only {} songs found in {genre}.", t.len()));
                    }
                }
                for p in self.pages.iter_mut() {
                    if let Page::Random(r) = p {
                        if r.genre == genre {
                            r.fill(result.clone());
                        }
                    }
                }
            }
            Msg::Radio { seed, result } => {
                let library = self.library.clone().unwrap_or_default();
                for p in self.pages.iter_mut() {
                    if let Page::Radio(r) = p {
                        if r.seed.uri == seed {
                            match &result {
                                Ok(radio) => {
                                    r.radio = Some(radio.clone());
                                    r.refilter(&library);
                                }
                                Err(e) => {
                                    r.error = Some(e.clone());
                                    r.tracks.loading = false;
                                }
                            }
                        }
                    }
                }
                if let Err(e) = result {
                    self.error(e);
                }
            }
            Msg::RadioMore { radio, result } => {
                let library = self.library.clone().unwrap_or_default();
                let mut found = 0;
                for p in self.pages.iter_mut() {
                    if let Page::Radio(r) = p {
                        if r.seed.uri == radio {
                            match &result {
                                Ok(tracks) => r.add_found(tracks.clone(), &library),
                                Err(_) => r.digging = false,
                            }
                            found = r.tracks.items.len();
                        }
                    }
                }
                match result {
                    Ok(_) => self.info(format!("Discover: {found} songs you have not liked")),
                    Err(e) => self.error(e),
                }
            }
            Msg::Library(library) => {
                for p in self.pages.iter_mut() {
                    if let Page::Radio(r) = p {
                        r.refilter(&library);
                    }
                }
                self.library = Some(library);
            }
            Msg::Removed { playlist_id, uri } => {
                if let Some(o) = self.open.as_mut().filter(|o| o.playlist.id == playlist_id) {
                    let before = o.tracks.items.len();
                    o.tracks.items.retain(|t| t.uri != uri);
                    let removed = before - o.tracks.items.len();
                    o.tracks.total = o.tracks.total.map(|t| t.saturating_sub(removed));
                    let len = o.tracks.items.len();
                    let sel = o.tracks.selected().min(len.saturating_sub(1));
                    o.tracks.state.select((len > 0).then_some(sel));
                }
            }
            Msg::UpNext(next) => self.up_next = next,
            Msg::Created => {
                self.playlists.reset();
                self.rebuild_rows();
            }
            Msg::Added { playlist_id } => {
                // Reload an open copy of that playlist so the new song shows.
                if let Some(o) = self.open.as_mut().filter(|o| o.playlist.id == playlist_id) {
                    o.tracks.reset();
                }
            }
            Msg::Found { list, offset, page } => {
                if self.is_current_found(&list) {
                    self.found.apply(offset, page);
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
            Msg::RetryLater { list, secs } => self.retry_later(list, secs),
            Msg::LoadFailed { list, forbidden } => {
                if list == List::Playlists {
                    self.playlists.loading = false;
                    self.playlists.total = Some(self.playlists.fetched);
                } else if self.is_current_found(&list) {
                    self.found.loading = false;
                    self.found.total = Some(self.found.fetched);
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
            Msg::Saved { uri, saved } => {
                if self.playing_uri() == Some(uri.as_str()) {
                    self.saved = Some((uri.clone(), saved));
                    if self.like_pending.as_deref() == Some(uri.as_str()) {
                        self.like_burst = Some((Instant::now(), saved));
                    }
                }
                if self.like_pending.as_deref() == Some(uri.as_str()) {
                    self.like_pending = None;
                }
                if let Some(lib) = self.library.as_mut() {
                    if saved {
                        lib.tracks.insert(uri.clone());
                    } else {
                        lib.tracks.remove(&uri);
                    }
                }
                if saved {
                    // Liked Songs loads again, with the new song at the top.
                    if self.view() != View::Liked {
                        self.liked.reset();
                    }
                } else {
                    let before = self.liked.items.len();
                    self.liked.items.retain(|t| t.uri != uri);
                    if self.liked.items.len() < before {
                        self.liked.total = self.liked.total.map(|t| t.saturating_sub(1));
                        let len = self.liked.items.len();
                        let sel = self.liked.selected().min(len.saturating_sub(1));
                        self.liked.state.select((len > 0).then_some(sel));
                    }
                }
            }
            Msg::Info(text) => self.info(text),
            Msg::Error(text) => self.error(text),
            Msg::Backoff(secs) => self.next_poll = Instant::now() + Duration::from_secs(secs.max(5)),
        }
    }

    /// The page shown, if a page tab is active.
    pub fn page(&self) -> Option<&Page> {
        self.pages.get(self.active?)
    }

    pub fn page_mut(&mut self) -> Option<&mut Page> {
        let i = self.active?;
        self.pages.get_mut(i)
    }

    pub fn loc(&self) -> Loc {
        match self.page() {
            Some(p) => Loc::Page(p.key()),
            None => Loc::Base(self.tab),
        }
    }

    /// Every tab in bar order: the fixed ones, then the open pages.
    pub fn locs(&self) -> Vec<Loc> {
        Tab::ALL.iter().map(|t| Loc::Base(*t)).chain(self.pages.iter().map(|p| Loc::Page(p.key()))).collect()
    }

    fn exists(&self, loc: &Loc) -> bool {
        match loc {
            Loc::Base(_) => true,
            Loc::Page(k) => self.pages.iter().any(|p| p.key() == *k),
        }
    }

    /// Shows a tab without a back step.
    fn show(&mut self, loc: Loc) {
        self.typing = false;
        match loc {
            Loc::Base(t) => {
                self.tab = t;
                self.active = None;
            }
            Loc::Page(k) => {
                self.active = self.pages.iter().position(|p| p.key() == k);
                if self.unloaded.remove(&k) {
                    if let Some(i) = self.active {
                        self.load_page(i);
                    }
                }
            }
        }
        // Recently played changes as you listen: load it again on each visit.
        if self.view() == View::Recent && !self.recent.loading {
            self.recent.reset();
        }
    }

    /// Asks for what page `i` shows (the album's songs load in tick()).
    fn load_page(&mut self, i: usize) {
        let cmd = match &self.pages[i] {
            Page::Artist(a) => Cmd::Artist(a.id.clone()),
            Page::Album(a) => Cmd::Album(a.id.clone()),
            Page::Radio(r) => Cmd::Radio(r.seed.uri.clone()),
            Page::Random(r) => Cmd::Randomize(r.genre.clone()),
        };
        if matches!(cmd, Cmd::Radio(_)) && !self.library_asked {
            self.library_asked = true;
            self.send(Cmd::Library);
        }
        self.send(cmd);
    }

    /// The open tabs and the theme, to save at quit.
    pub fn saved_state(&self) -> State {
        let tabs = self
            .pages
            .iter()
            .map(|p| match p {
                Page::Artist(a) => SavedTab::Artist { id: a.id.clone(), name: a.name.clone() },
                Page::Album(a) => SavedTab::Album { album: a.album.clone() },
                Page::Radio(r) => SavedTab::Radio { seed: r.seed.clone(), tuned: r.saved() },
                Page::Random(r) => SavedTab::Random { genre: r.genre.clone() },
            })
            .collect();
        // A theme tried with SPOTLITE_THEME is not saved.
        let theme = if crate::theme::env_override().is_some() { crate::state::load().theme } else { Some(crate::theme::name()) };
        State {
            theme,
            tabs,
            active: self.active,
            tab: Some(self.tab),
            search: self.searched.clone(),
            search_kind: Some(self.search_kind),
            viz_on: self.viz_on,
            side_fx: self.side_fx,
            sidebar: Some(crate::state::SavedSidebar { open: self.sidebar.open, view: self.sidebar.view }),
            pins: self.pins.clone(),
            // The live state (a shuffle just pressed counts), else the last one seen.
            playback: self.snapshot().or_else(|| self.last_playback.clone()),
        }
    }

    /// Opens the tabs of the last run. Nothing loads until a tab is shown.
    pub fn restore(&mut self, state: State) {
        if let Some(tab) = state.tab {
            self.tab = tab;
        }
        if !state.search.is_empty() {
            // The last search is listed again (it loads in tick()).
            self.query = state.search.clone();
            self.searched = state.search;
            self.typing = false;
        }
        if let Some(kind) = state.search_kind {
            self.search_kind = kind;
        }
        self.viz_on |= state.viz_on;
        self.side_fx = state.side_fx;
        if let Some(sb) = state.sidebar {
            self.sidebar.open = sb.open;
            self.sidebar.view = sb.view;
        }
        self.pins = state.pins;
        self.last_playback = state.playback.clone();
        self.true_shuffle = state.playback.as_ref().is_some_and(|p| p.shuffle);
        self.resume = state.playback.map(|p| (p, Instant::now()));
        for tab in state.tabs.into_iter().take(MAX_PAGES) {
            let page = match tab {
                SavedTab::Artist { id, name } => Page::Artist(ArtistPage::new(id, name)),
                SavedTab::Album { album } => match album.id.clone() {
                    Some(id) => Page::Album(AlbumPage { id, album, tracks: Listing::default() }),
                    None => continue,
                },
                SavedTab::Radio { seed, tuned } => Page::Radio(Box::new(RadioPage::restored(seed, tuned))),
                SavedTab::Random { genre } => Page::Random(RandomPage::new(genre)),
            };
            let key = page.key();
            if !self.exists(&Loc::Page(key.clone())) {
                self.unloaded.insert(key);
                self.pages.push(page);
            }
        }
        if let Some(key) = state.active.and_then(|i| self.pages.get(i)).map(Page::key) {
            self.show(Loc::Page(key));
        }
        // The undo tree starts where the last run ended.
        let here = self.loc();
        let (label, reopen) = self.nav_describe(&here);
        self.nav = NavTree::new(here, label);
        self.nav.nodes[0].reopen = reopen;
    }

    /// Goes to a tab; Esc comes back here.
    pub fn go(&mut self, loc: Loc) {
        self.nav_visit(&loc);
        self.show(loc);
    }

    /// Adds a visit to `loc` to the undo tree.
    fn nav_visit(&mut self, loc: &Loc) {
        let (label, reopen) = self.nav_describe(loc);
        self.nav.visit(loc.clone(), label, reopen);
    }

    /// The tree's label for a place, and what opens it again if its tab closes.
    fn nav_describe(&self, loc: &Loc) -> (String, Option<SavedTab>) {
        match loc {
            Loc::Base(tab) => {
                let glyph = match tab {
                    Tab::Search => "⌕",
                    Tab::Playlists => "≡",
                    Tab::Liked => "♥",
                    Tab::Recent => "◷",
                };
                (format!("{glyph} {}", tab.title()), None)
            }
            Loc::Page(k) => match self.pages.iter().find(|p| p.key() == *k) {
                Some(p) => {
                    let (glyph, saved) = match p {
                        Page::Artist(a) => ("◉", SavedTab::Artist { id: a.id.clone(), name: a.name.clone() }),
                        Page::Album(a) => ("◎", SavedTab::Album { album: a.album.clone() }),
                        Page::Radio(r) => ("≋", SavedTab::Radio { seed: r.seed.clone(), tuned: r.saved() }),
                        Page::Random(r) => ("⚄", SavedTab::Random { genre: r.genre.clone() }),
                    };
                    (format!("{glyph} {}", p.title().1), Some(saved))
                }
                None => ("?".into(), None),
            },
        }
    }

    /// Opens a closed page again from what the undo tree kept. True if it is open now.
    fn reopen(&mut self, saved: &SavedTab) -> bool {
        let page = match saved.clone() {
            SavedTab::Artist { id, name } => Page::Artist(ArtistPage::new(id, name)),
            SavedTab::Album { album } => match album.id.clone() {
                Some(id) => Page::Album(AlbumPage { id, album, tracks: Listing::default() }),
                None => return false,
            },
            SavedTab::Radio { seed, tuned } => Page::Radio(Box::new(RadioPage::restored(seed, tuned))),
            SavedTab::Random { genre } => Page::Random(RandomPage::new(genre)),
        };
        let key = page.key();
        if !self.exists(&Loc::Page(key.clone())) {
            self.unloaded.insert(key);
            self.pages.push(page);
            if self.pages.len() > MAX_PAGES {
                self.pages.remove(0);
                self.active = None;
            }
        }
        true
    }

    /// Goes to node `i` of the undo tree (a click in the tree): opens its page again if its
    /// tab was closed.
    pub fn nav_jump(&mut self, i: usize) {
        let Some(node) = self.nav.nodes.get(i).cloned() else { return };
        if !self.exists(&node.loc) {
            match &node.reopen {
                Some(saved) if self.reopen(saved) => {}
                _ => {
                    self.info("That page cannot open again.");
                    return;
                }
            }
        }
        self.open = None;
        self.nav.jump(i);
        self.show(node.loc);
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.go(Loc::Base(tab));
    }

    /// The next (`1`) or previous (`-1`) tab in the bar, pages included.
    fn cycle_tab(&mut self, dir: isize) {
        let locs = self.locs();
        let cur = self.loc();
        let i = locs.iter().position(|l| *l == cur).unwrap_or(0) as isize;
        let n = locs.len() as isize;
        self.go(locs[(i + dir).rem_euclid(n) as usize].clone());
    }

    /// Back one step: close an open playlist or folder, else go to the tab you were on before.
    /// Pages stay open.
    fn back(&mut self) {
        if self.active.is_none() && (self.open.take().is_some() || self.close_folder()) {
            return;
        }
        let open: std::collections::HashSet<String> = self.pages.iter().map(Page::key).collect();
        let usable = |n: &NavNode| match &n.loc {
            Loc::Base(_) => true,
            Loc::Page(k) => open.contains(k),
        };
        if let Some(loc) = self.nav.back(usable).map(|n| n.loc.clone()) {
            self.show(loc);
        } else if self.active.is_some() {
            self.show(Loc::Base(self.tab));
        }
    }

    /// Closes the page tab shown and goes back.
    fn close_page(&mut self) {
        let Some(i) = self.active else {
            self.info("Only artist, album and radio tabs close.");
            return;
        };
        let key = self.pages.remove(i).key();
        self.unloaded.remove(&key);
        self.active = None;
        let open: std::collections::HashSet<String> = self.pages.iter().map(Page::key).collect();
        let usable = |n: &NavNode| match &n.loc {
            Loc::Base(_) => true,
            Loc::Page(k) => open.contains(k),
        };
        match self.nav.back(usable).map(|n| n.loc.clone()) {
            Some(loc) => self.show(loc),
            None => self.show(Loc::Base(self.tab)),
        }
    }

    /// Opens a page in a new tab, or goes to its tab if it is open. True for a new tab.
    fn open_page(&mut self, page: Page) -> bool {
        let key = page.key();
        let fresh = !self.exists(&Loc::Page(key.clone()));
        if fresh {
            self.pages.push(page);
            if self.pages.len() > MAX_PAGES {
                self.pages.remove(0);
                self.active = None;
            }
        }
        let loc = Loc::Page(key);
        self.nav_visit(&loc);
        self.show(loc);
        fresh
    }

    pub fn open_artist(&mut self, id: String, name: String) {
        if self.open_page(Page::Artist(ArtistPage::new(id.clone(), name))) {
            self.send(Cmd::Artist(id));
        }
    }

    pub fn open_album(&mut self, album: Album) {
        let Some(id) = album.id.clone() else {
            self.info("This song has no album page.");
            return;
        };
        let details = album.release_date.is_none() || album.images.is_empty();
        if self.open_page(Page::Album(AlbumPage { id: id.clone(), album, tracks: Listing::default() })) && details {
            self.send(Cmd::Album(id));
        }
    }

    pub fn start_radio(&mut self, seed: Track) {
        if !crate::client::AVAILABLE {
            self.error("Song radio needs the `client` feature.");
            return;
        }
        if !seed.uri.starts_with("spotify:track:") && !seed.uri.starts_with("spotify:artist:") {
            self.info("Radio works for songs and artists only.");
            return;
        }
        let uri = seed.uri.clone();
        if !self.open_page(Page::Radio(Box::new(RadioPage::new(seed)))) {
            return;
        }
        self.send(Cmd::Radio(uri));
        if !self.library_asked {
            self.library_asked = true;
            self.send(Cmd::Library);
        }
    }

    fn go_to_artist(&mut self, track: &Track) {
        match track.artists.iter().find(|a| a.id().is_some()) {
            Some(a) => {
                let (id, name) = (a.id().unwrap_or_default().to_owned(), a.name.clone());
                self.open_artist(id, name);
            }
            None => self.info("This song has no artist page."),
        }
    }

    fn go_to_album(&mut self, track: &Track) {
        match track.album.clone() {
            Some(a) => self.open_album(a),
            None => self.info("This song has no album page."),
        }
    }

    /// The playlist open in the Playlists tab, if the user may edit it.
    fn editable_open_playlist(&self) -> Option<&Playlist> {
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

    fn filter_menu_items(&self) -> Vec<(&'static str, String, Action)> {
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
    fn discover_round(&mut self) {
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
    fn open_themes(&mut self) {
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
    fn try_theme(&mut self, name: &str, keep: bool) {
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
    fn close_menu(&mut self) {
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
    fn watch_theme(&mut self) {
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

    fn open_filters(&mut self) {
        if self.view() != View::Radio {
            return;
        }
        let items = self.filter_menu_items();
        self.menu = Some(Menu { title: "Radio filters".into(), items, cursor: 0, track: None, at: None });
    }

    /// Runs the menu entry under the cursor.
    fn run_menu(&mut self) {
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
                SideFx::Liquid => format!("Sidebar effect: {}", crate::theme::c().liquid.name()),
                SideFx::Spectrum => "Sidebar effect: spectrum".to_owned(),
            });
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

    fn run_action(&mut self, action: Action, track: Track) {
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
            Action::Filter(_) | Action::Theme(_) | Action::SideFx(_) => {}
        }
    }

    /// `z`: the genre picker (on a Randomize page, it rolls again).
    fn randomize_key(&mut self) {
        if let Some(Page::Random(_)) = self.page() {
            self.reroll();
        } else {
            self.open_genre_picker();
        }
    }

    pub fn open_genre_picker(&mut self) {
        self.menu = None;
        self.genre_picker = Some(crate::genres::Picker::new());
    }

    /// Keys of the genre picker: type to search, ↑↓ move, → ← open and close, ⏎ choose.
    fn genre_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let genres = std::mem::take(&mut self.genre_list);
        if self.genre_picker.is_some() {
            match key.code {
                KeyCode::Tab => {
                    let genre = self.genre_picker.as_ref().and_then(|p| p.genre_at(&genres, p.cursor));
                    if let Some(g) = genre {
                        self.start_sample(g);
                    }
                    self.genre_list = genres;
                    return;
                }
                KeyCode::Esc if self.sample.is_some() => {
                    self.stop_sample();
                    self.genre_list = genres;
                    return;
                }
                _ => {}
            }
        }
        if let Some(p) = self.genre_picker.as_mut() {
            match key.code {
                KeyCode::Esc if !p.query.is_empty() => p.set_query(String::new()),
                KeyCode::Esc => {
                    self.genre_picker = None;
                    self.stop_sample();
                }
                KeyCode::Down => p.move_by(&genres, 1),
                KeyCode::Up => p.move_by(&genres, -1),
                KeyCode::Char('n') if ctrl => p.move_by(&genres, 1),
                KeyCode::Char('p') if ctrl => p.move_by(&genres, -1),
                KeyCode::PageDown => p.move_by(&genres, 10),
                KeyCode::PageUp => p.move_by(&genres, -10),
                KeyCode::Right => p.open_close(&genres, true),
                KeyCode::Left => p.open_close(&genres, false),
                KeyCode::Enter => {
                    let at = p.cursor;
                    self.genre_choose(&genres, at);
                }
                _ if is_ctrl_backspace(&key) => {
                    let mut q = p.query.clone();
                    delete_word(&mut q);
                    p.set_query(q);
                }
                KeyCode::Backspace => {
                    let mut q = p.query.clone();
                    q.pop();
                    p.set_query(q);
                }
                KeyCode::Char(c) if !ctrl => {
                    let q = format!("{}{c}", p.query);
                    p.set_query(q);
                }
                _ => {}
            }
        }
        self.genre_list = genres;
    }

    /// Chooses picker row `i` (⏎ or a click): a genre randomizes, a folder opens or closes.
    pub fn genre_choose(&mut self, genres: &[crate::genres::Genre], i: usize) {
        let Some(p) = self.genre_picker.as_mut() else { return };
        if let crate::genres::Choice::Randomize(genre) = p.choose(genres, i) {
            self.genre_picker = None;
            self.stop_sample();
            self.randomize(genre);
        }
    }

    /// Tab in the genre picker: about 10 s of the genre's top song (Tab again: the next one).
    /// What plays now fades out, the sample fades in; when it ends, the same back.
    fn start_sample(&mut self, genre: String) {
        let nth = match &self.sample {
            Some(s) if s.genre == genre => s.nth + 1,
            _ => 0,
        };
        // A way back still waiting in a fade (Tab again while it went back): keep it.
        let pending = self.fade.iter_mut().find_map(|step| match step {
            FadeStep::Send(Cmd::Unsample { restore }) => Some(restore.take()),
            _ => None,
        });
        if let Some(restore) = pending {
            self.sample_restore = restore;
        } else if self.sample.is_none() {
            // What played before the first sample, and its volume.
            let pb = self.playback.as_ref();
            self.sample_volume = pb.and_then(|p| p.device.as_ref()).and_then(|d| d.volume_percent);
            self.sample_restore = pb.and_then(|p| {
                Some(crate::worker::Restore {
                    context: p.context.as_ref().map(|c| c.uri.clone()),
                    track: p.item.as_ref()?.uri.clone(),
                    position_ms: self.progress_ms(),
                    playing: p.is_playing,
                })
            });
        }
        let playing = self.playback.as_ref().is_some_and(|p| p.is_playing);
        self.sample = Some(Sample { genre: genre.clone(), nth, track: None, started: None });
        self.fade.clear();
        self.fade_at = None;
        if let (Some(v), true) = (self.sample_volume, playing) {
            self.fade.push_back(FadeStep::Ramp { to: v / 10, len: FADE_OUT });
        } else if let Some(v) = self.sample_volume {
            // Nothing is heard now: start quiet, then fade in.
            self.set_volume(v / 10);
        }
        self.fade.push_back(FadeStep::Send(Cmd::Sample { genre, nth }));
    }

    /// Ends the sample (time is up, Esc, or the picker closes): fades out, goes back to what
    /// played, and fades it in.
    fn stop_sample(&mut self) {
        if self.sample.take().is_none() {
            return;
        }
        let restore = self.sample_restore.take();
        self.fade.clear();
        self.fade_at = None;
        match self.sample_volume {
            Some(v) => {
                self.fade.push_back(FadeStep::Ramp { to: v / 10, len: FADE_OUT });
                self.fade.push_back(FadeStep::Send(Cmd::Unsample { restore }));
                self.fade.push_back(FadeStep::Wait(SWITCH_WAIT));
                self.fade.push_back(FadeStep::Ramp { to: v, len: FADE_IN });
            }
            None => self.fade.push_back(FadeStep::Send(Cmd::Unsample { restore })),
        }
        self.poll_soon();
    }

    /// Runs the fade steps that are due. True if the volume changed.
    fn run_fade(&mut self) -> bool {
        let mut changed = false;
        while let Some(step) = self.fade.front() {
            let volume = self.volume().unwrap_or(0);
            let (start, from) = *self.fade_at.get_or_insert((Instant::now(), volume));
            let done = match step {
                FadeStep::Send(_) => true,
                FadeStep::Wait(len) => start.elapsed() >= *len,
                FadeStep::Ramp { to, len } => {
                    let t = (start.elapsed().as_secs_f32() / len.as_secs_f32().max(0.001)).min(1.0);
                    // Ease in and out: it starts and ends gently.
                    let eased = t * t * (3.0 - 2.0 * t);
                    let v = (from as f32 + (*to as f32 - from as f32) * eased).round() as u8;
                    if Some(v) != self.volume() {
                        self.set_volume(v);
                        changed = true;
                    }
                    t >= 1.0
                }
            };
            if !done {
                break;
            }
            self.fade_at = None;
            if let Some(FadeStep::Send(cmd)) = self.fade.pop_front() {
                self.send(cmd);
            }
        }
        changed
    }

    /// Opens the Randomize page of `genre` (or goes to it) and rolls it.
    fn randomize(&mut self, genre: String) {
        if self.open_page(Page::Random(RandomPage::new(genre.clone()))) {
            self.send(Cmd::Randomize(genre));
        }
    }

    /// Rolls the open Randomize page again: 50 new random songs.
    fn reroll(&mut self) {
        let Some(Page::Random(r)) = self.page_mut() else { return };
        if r.rolling {
            return;
        }
        r.rolling = true;
        let genre = r.genre.clone();
        self.send(Cmd::Randomize(genre));
    }

    /// Asks to save the random songs on screen as a new playlist.
    fn ask_save_random(&mut self) {
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

    fn open_picker(&mut self, track: Track) {
        if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing playlists.");
            return;
        }
        self.picker = Some(Picker { track, query: String::new(), cursor: 0 });
    }

    /// Playlists the user may add to, narrowed by the picker's text.
    pub fn picker_choices(&self) -> Vec<&Playlist> {
        let q = self.picker.as_ref().map(|p| p.query.to_lowercase()).unwrap_or_default();
        self.playlists
            .items
            .iter()
            .filter(|p| self.owns(p))
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q))
            .collect()
    }

    fn pick_playlist(&mut self) {
        let Some(picker) = self.picker.as_ref() else { return };
        let Some(pl) = self.picker_choices().get(picker.cursor).map(|p| (*p).clone()) else { return };
        let track = picker.track.clone();
        self.picker = None;
        self.send(Cmd::AddToPlaylist {
            playlist_id: pl.id,
            playlist: pl.name,
            uris: vec![track.uri],
            what: track.name,
        });
    }

    fn ask_remove(&mut self, track: Track) {
        if !self.can_edit {
            self.error("Run `spotlite auth` again to allow editing playlists.");
            return;
        }
        let Some(pl) = self.editable_open_playlist().cloned() else {
            self.info("Open one of your playlists to remove songs from it.");
            return;
        };
        self.confirm = Some(Confirm {
            text: format!("Remove “{}” from {}?", track.name, pl.name),
            cmd: Cmd::RemoveFromPlaylist { playlist_id: pl.id, playlist: pl.name, uri: track.uri, what: track.name },
        });
    }

    /// Moves the cursor to the playing track, if the current list has it.
    fn jump_to_playing(&mut self) {
        let Some(uri) = self.playing_uri().map(str::to_owned) else { return };
        let found = self.tracks().and_then(|l| l.items.iter().position(|t| t.uri == uri));
        match found {
            Some(i) => self.select(i),
            None => self.info("The playing song is not in this list."),
        }
    }

    fn move_popup(&mut self, delta: isize) {
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
        let q = self.palette.as_ref().map(|p| p.query.clone()).unwrap_or_default();
        crate::palette::filter(crate::palette::entries(self), &q)
    }

    fn move_palette(&mut self, delta: isize) {
        let len = self.palette_entries().len();
        if let Some(p) = self.palette.as_mut().filter(|_| len > 0) {
            p.cursor = (p.cursor.min(len - 1) as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    /// Runs the palette entry under the cursor: the palette closes, then its key runs.
    pub fn run_palette(&mut self) {
        let entries = self.palette_entries();
        let Some(p) = self.palette.take() else { return };
        if let Some(e) = entries.get(p.cursor.min(entries.len().saturating_sub(1))) {
            self.on_key(e.event);
        }
    }

    fn play_selected(&mut self) {
        match self.view() {
            View::Album => {
                let Some(Page::Album(a)) = self.page() else { return };
                let Some(t) = a.tracks.current() else { return };
                let uri = a.album.uri.clone().unwrap_or_else(|| format!("spotify:album:{}", a.id));
                self.send(Cmd::Play(Play::Context { uri, from: Some(t.uri.clone()) }));
                self.poll_soon();
            }
            View::Recent => {
                // The same song can be in the list twice; play the list as it is.
                let uris: Vec<String> = self.recent.items.iter().map(|t| t.uri.clone()).collect();
                if !uris.is_empty() {
                    let offset = self.recent.selected();
                    self.send(Cmd::Play(Play::Tracks { uris, offset }));
                    self.poll_soon();
                }
            }
            View::Random => {
                let Some(Page::Random(r)) = self.page() else { return };
                let uris: Vec<String> = r.tracks.items.iter().map(|t| t.uri.clone()).collect();
                if !uris.is_empty() {
                    let offset = r.tracks.selected();
                    self.send(Cmd::Play(Play::Tracks { uris, offset }));
                    self.poll_soon();
                }
            }
            View::Radio => {
                let Some(Page::Radio(r)) = self.page() else { return };
                let uris: Vec<String> = r.tracks.items.iter().map(|t| t.uri.clone()).collect();
                if !uris.is_empty() {
                    let offset = r.tracks.selected();
                    self.send(Cmd::Play(Play::Tracks { uris, offset }));
                    self.poll_soon();
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
                        self.send(Cmd::Play(Play::Tracks { uris, offset }));
                        self.poll_soon();
                    }
                    Some(ArtistRow::Header(_)) | None => {}
                }
            }
            View::Search if self.search_kind != SearchKind::Songs => self.open_found(),
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
        if let Some(Page::Album(a)) = self.page() {
            let uri = a.album.uri.clone().unwrap_or_else(|| format!("spotify:album:{}", a.id));
            self.info(format!("Playing {}", a.album.name));
            self.send(Cmd::Play(Play::Context { uri, from: None }));
            self.poll_soon();
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

    fn queue_selected(&mut self) {
        if let Some(t) = self.tracks().and_then(|l| l.current()) {
            let cmd = Cmd::Queue {
                uri: t.uri.clone(),
                name: t.name.clone(),
            };
            self.send(cmd);
        }
    }

    /// Whether a song is in Liked Songs, if spotlite knows.
    pub fn is_liked(&self, uri: &str) -> Option<bool> {
        match &self.saved {
            Some((u, s)) if u == uri => Some(*s),
            _ => self.library.as_ref().map(|l| l.tracks.contains(uri)),
        }
    }

    /// Likes or unlikes a song: the selected one, else the playing one.
    fn toggle_like(&mut self, track: Option<Track>) {
        if !self.can_like {
            self.error("Run `spotlite auth` again to allow liking songs.");
            return;
        }
        let Some(track) = track.or_else(|| self.target_track()) else {
            self.info("Select a song first.");
            return;
        };
        match &self.saved {
            Some((u, saved)) if *u == track.uri => {
                // The playing song: the heart changes at once (Spotify confirms after).
                let now = !*saved;
                self.saved = Some((track.uri.clone(), now));
                self.like_burst = Some((Instant::now(), now));
                self.send(Cmd::SetSaved { uri: track.uri, saved: now })
            }
            // Spotify says which it is: the library index may be old.
            _ => {
                self.like_pending = Some(track.uri.clone());
                self.send(Cmd::ToggleSaved(track.uri))
            }
        }
    }

    /// The heart is animating (the loop wakes often meanwhile).
    pub fn like_animating(&self) -> bool {
        self.like_burst.is_some_and(|(at, _)| at.elapsed() < LIKE_BURST)
    }

    /// Asks to save the radio list on screen as a new playlist.
    fn ask_save_radio(&mut self) {
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
    fn own_shuffle(&mut self, p: &mut Playback) {
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
    fn toggle_shuffle(&mut self) {
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

    /// True when `list` is the non-song search shown now (older answers are dropped).
    fn is_current_found(&self, list: &List) -> bool {
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
    fn open_found(&mut self) {
        let Some(item) = self.found.current().cloned() else { return };
        match item {
            Found::Artist(a) => self.open_artist(a.id, a.name),
            Found::Album(a) => self.open_album(a),
            Found::Playlist(p) if self.owns(&p) => {
                // Your own playlist opens in the Playlists tab, like there.
                self.open = Some(OpenPlaylist { playlist: p, tracks: Listing::default() });
                self.switch_tab(Tab::Playlists);
            }
            Found::Playlist(p) => {
                // Spotify lists only your own playlists' songs to dev-mode apps: play it.
                self.info(format!("Playing {}", p.name));
                self.send(Cmd::Play(Play::Context { uri: p.uri, from: None }));
                self.poll_soon();
            }
        }
    }

    fn submit_search(&mut self) {
        let q = self.query.trim().to_owned();
        self.typing = false;
        self.search_due = None;
        if !q.is_empty() && q != self.searched {
            self.run_search(q);
        }
    }

    /// Search as you type: 300 ms after the last key, if the text has 2 or more characters.
    fn live_search(&mut self) -> bool {
        let q = self.query.trim().to_owned();
        if q.chars().count() < 2 || q == self.searched {
            return false;
        }
        self.run_search(q);
        true
    }

    fn run_search(&mut self, q: String) {
        self.searched = q;
        self.search.reset();
        self.found.reset();
        // tick() loads the first page.
    }

    fn reload(&mut self) {
        match self.view() {
            View::Artist | View::Album | View::Radio | View::Random => {}
            View::Search => {
                self.search.reset();
                self.found.reset();
            }
            View::Playlists => {
                self.playlists.reset();
                self.rebuild_rows();
            }
            View::Liked => self.liked.reset(),
            View::Recent => self.recent.reset(),
            View::Playlist => {
                if let Some(o) = self.open.as_mut() {
                    o.tracks.reset();
                }
            }
        }
        self.next_poll = Instant::now();
    }

    fn move_selection(&mut self, delta: isize) {
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
            View::Playlists if self.foldered() => self.rows.items.len(),
            View::Playlists => self.playlists.items.len(),
            View::Search if self.search_kind != SearchKind::Songs => self.found.items.len(),
            _ => self.tracks().map_or(0, |l| l.items.len()),
        }
    }

    fn select(&mut self, i: usize) {
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
        if self.undo.is_some() {
            self.undo_key(key);
            return;
        }
        if self.tabs_list.is_some() && key.code == KeyCode::Esc {
            self.tabs_list = None;
            return;
        }
        if self.confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    if let Some(c) = self.confirm.take() {
                        self.send(c.cmd);
                    }
                }
                _ => self.confirm = None,
            }
            return;
        }
        if self.palette.is_some() {
            match key.code {
                KeyCode::Esc => self.palette = None,
                KeyCode::Enter => self.run_palette(),
                KeyCode::Down => self.move_palette(1),
                KeyCode::Up => self.move_palette(-1),
                KeyCode::Char('n') if ctrl => self.move_palette(1),
                KeyCode::Char('p') if ctrl => self.move_palette(-1),
                KeyCode::Backspace => {
                    if let Some(p) = self.palette.as_mut() {
                        if is_ctrl_backspace(&key) {
                            delete_word(&mut p.query);
                        } else {
                            p.query.pop();
                        }
                        p.cursor = 0;
                    }
                }
                _ if is_ctrl_backspace(&key) => {
                    if let Some(p) = self.palette.as_mut() {
                        delete_word(&mut p.query);
                        p.cursor = 0;
                    }
                }
                KeyCode::Char(c) if !ctrl => {
                    if let Some(p) = self.palette.as_mut() {
                        p.query.push(c);
                        p.cursor = 0;
                    }
                }
                _ => {}
            }
            return;
        }
        if self.genre_picker.is_some() {
            self.genre_key(key);
            return;
        }
        if self.picker.is_some() {
            match key.code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Enter => self.pick_playlist(),
                KeyCode::Down => self.move_popup(1),
                KeyCode::Up => self.move_popup(-1),
                KeyCode::Char('n') if ctrl => self.move_popup(1),
                KeyCode::Char('p') if ctrl => self.move_popup(-1),
                KeyCode::Backspace => {
                    if let Some(p) = self.picker.as_mut() {
                        if is_ctrl_backspace(&key) {
                            delete_word(&mut p.query);
                        } else {
                            p.query.pop();
                        }
                        p.cursor = 0;
                    }
                }
                _ if is_ctrl_backspace(&key) => {
                    if let Some(p) = self.picker.as_mut() {
                        delete_word(&mut p.query);
                        p.cursor = 0;
                    }
                }
                KeyCode::Char(c) if !ctrl => {
                    if let Some(p) = self.picker.as_mut() {
                        p.query.push(c);
                        p.cursor = 0;
                    }
                }
                _ => {}
            }
            return;
        }
        if self.menu.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('.') => self.close_menu(),
                KeyCode::Char('F') if self.menu.as_ref().is_some_and(|m| m.track.is_none() && !m.is_theme()) => {
                    self.close_menu()
                }
                KeyCode::Char('T') if self.menu.as_ref().is_some_and(Menu::is_theme) => self.close_menu(),
                KeyCode::Char('V') if self.side_fx_before.is_some() => self.close_menu(),
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('l') => self.run_menu(),
                KeyCode::Down | KeyCode::Char('j') => self.move_popup(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_popup(-1),
                KeyCode::Char(c) => {
                    // A key hint runs its entry directly.
                    let hit = self.menu.as_ref().and_then(|m| {
                        m.items.iter().position(|(k, _, _)| k.len() == c.len_utf8() && k.starts_with(c))
                    });
                    if let Some(i) = hit {
                        if let Some(m) = self.menu.as_mut() {
                            m.cursor = i;
                        }
                        self.run_menu();
                    }
                }
                _ => {}
            }
            return;
        }
        if !self.typing && is_ctrl_backspace(&key) {
            self.open_undo();
            return;
        }
        if !self.typing && self.side_key(key) {
            return;
        }
        if self.typing {
            match key.code {
                KeyCode::Esc => {
                    self.typing = false;
                    self.search_due = None;
                }
                KeyCode::Down => self.typing = false,
                KeyCode::Enter => self.submit_search(),
                // Ctrl+Backspace (and Alt+Backspace, Ctrl+W): the last word.
                _ if is_ctrl_backspace(&key) => {
                    delete_word(&mut self.query);
                    self.search_due = Some(Instant::now() + SEARCH_DELAY);
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    self.search_due = Some(Instant::now() + SEARCH_DELAY);
                }
                KeyCode::Char('u') if ctrl => self.query.clear(),
                KeyCode::Char(c) if !ctrl => {
                    self.query.push(c);
                    self.search_due = Some(Instant::now() + SEARCH_DELAY);
                }
                _ => {}
            }
            return;
        }
        let page = 10;
        if crate::palette::opens(&key) {
            self.palette = Some(Default::default());
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('v') => {
                self.viz_on = !self.viz_on;
                self.viz_at = Some(Instant::now());
                self.info(if self.viz_on { "Visualizer on" } else { "Visualizer off" });
            }
            KeyCode::Char('Q') => {
                self.quit = true;
                self.stop_services = true;
            }
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => {
                self.switch_tab(Tab::Search);
                self.typing = true;
            }
            KeyCode::Tab if self.view() == View::Artist => {
                if let Some(a) = self.artist_page_mut() {
                    a.jump_section(1);
                }
            }
            KeyCode::BackTab if self.view() == View::Artist => {
                if let Some(a) = self.artist_page_mut() {
                    a.jump_section(-1);
                }
            }
            KeyCode::Tab | KeyCode::Char('L') => self.cycle_tab(1),
            KeyCode::BackTab | KeyCode::Char('H') => self.cycle_tab(-1),
            KeyCode::Char(c @ '1'..='9') => {
                if let Some(loc) = self.locs().get(c as usize - '1' as usize).cloned() {
                    self.go(loc);
                }
            }
            KeyCode::Char('x') => self.close_page(),
            KeyCode::Char('t') if self.view() == View::Search => {
                let i = SearchKind::ALL.iter().position(|k| *k == self.search_kind).unwrap_or(0);
                self.set_search_kind(SearchKind::ALL[(i + 1) % SearchKind::ALL.len()]);
            }
            KeyCode::Char('T') => self.open_themes(),
            KeyCode::Char('*') => self.pin_selected(),
            KeyCode::Char('C') => {
                if let Some(t) = self.target_track() {
                    self.share_card(t);
                }
            }
            KeyCode::Char('d') if !ctrl && self.view() == View::Radio => self.discover_round(),
            KeyCode::Char('d') if !ctrl && self.view() == View::Random => self.reroll(),
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') => self.back(),
            KeyCode::Char('.') => self.open_menu(),
            KeyCode::Char('i') => {
                if let Some(t) = self.target_track() {
                    self.go_to_artist(&t);
                }
            }
            KeyCode::Char('b') => {
                if let Some(t) = self.target_track() {
                    self.go_to_album(&t);
                }
            }
            KeyCode::Char('o') => {
                let radio_row = match self.page() {
                    Some(Page::Artist(a)) => match a.rows.current() {
                        Some(ArtistRow::Radio(seed)) => Some(seed.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(seed) = radio_row {
                    self.start_radio(seed);
                } else if let Some(t) = self.target_track() {
                    self.start_radio(t);
                }
            }
            KeyCode::Char('e') => {
                if let Some(t) = self.selected_track() {
                    self.open_picker(t);
                }
            }
            KeyCode::Char('D') => {
                if let Some(t) = self.selected_track() {
                    self.ask_remove(t);
                }
            }
            KeyCode::Char('F') => self.open_filters(),
            KeyCode::Char('c') => self.jump_to_playing(),
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
            KeyCode::Char('f') => self.toggle_like(None),
            KeyCode::Char('S') if self.view() == View::Radio => self.ask_save_radio(),
            KeyCode::Char('S') if self.view() == View::Random => self.ask_save_random(),
            KeyCode::Char('z') if !ctrl => self.randomize_key(),
            KeyCode::Char('R') => self.reload(),
            _ => {}
        }
    }
}

/// Ctrl+Backspace, as terminals send it: ^H (most, ghostty too), ^W (some), or Backspace with
/// Ctrl or Alt (kitty keyboard protocol, or Alt as a fallback).
/// Deletes the last word of `text`, and the spaces after it (as Ctrl+Backspace does in most
/// text boxes). Punctuation counts as part of the word.
pub fn delete_word(text: &mut String) {
    let trimmed = text.trim_end().len();
    text.truncate(trimmed);
    let start = text.rfind(char::is_whitespace).map_or(0, |i| i + text[i..].chars().next().map_or(1, char::len_utf8));
    text.truncate(start);
}

fn is_ctrl_backspace(key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Backspace => key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT),
        KeyCode::Char('h') | KeyCode::Char('w') => ctrl,
        _ => false,
    }
}

/// The tabs that do not fit in the bar: the "+N ▾" chip and its list.
impl App {
    /// The pointer moved (no button). True if the screen changes: the list opens, closes, or
    /// another row is under the pointer.
    pub fn on_hover(&mut self, pos: Position) -> bool {
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
    fn tabs_mouse(&mut self, m: MouseEvent, pos: Position) -> bool {
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
    fn close_page_at(&mut self, i: usize) {
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

/// The undo tree popup.
impl App {
    /// Ctrl+Backspace: shows the tree, the cursor on where you are.
    fn open_undo(&mut self) {
        let rows = self.nav.rows();
        self.undo = Some(rows.iter().position(|r| r.node == self.nav.cur).unwrap_or(0));
    }

    fn undo_key(&mut self, key: KeyEvent) {
        let len = self.nav.rows().len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => self.undo = None,
            _ if is_ctrl_backspace(&key) => self.undo = None,
            KeyCode::Down | KeyCode::Char('j') => self.move_popup(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_popup(-1),
            KeyCode::Char('g') | KeyCode::Home => self.undo = Some(0),
            KeyCode::Char('G') | KeyCode::End => self.undo = Some(len.saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char(' ') => self.undo_go(),
            _ => {}
        }
    }

    /// Goes to the node under the tree's cursor and closes the tree.
    fn undo_go(&mut self) {
        let Some(c) = self.undo.take() else { return };
        if let Some(row) = self.nav.rows().get(c) {
            let node = row.node;
            self.nav_jump(node);
        }
    }
}

/// The sidebar: its rows, keys, mouse and actions.
impl App {
    /// The song that plays now, if any.
    fn playing_track(&self) -> Option<Track> {
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
    fn side_fits(&mut self) -> bool {
        if self.sidebar.open || self.screen_w == 0 || Sidebar::full_width(self.screen_w).is_some() {
            return true;
        }
        self.info(format!("The window is too narrow for the sidebar ({} columns needed).", crate::sidebar::MIN_SCREEN));
        false
    }

    /// Opens or shuts the sidebar (`\`, Ctrl+B).
    fn toggle_sidebar(&mut self) {
        if self.side_fits() {
            self.sidebar.toggle();
            self.fix_side_cursor();
        }
    }

    fn show_side(&mut self, view: SideView) {
        if self.side_fits() {
            self.sidebar.show(view);
            self.fix_side_cursor();
        }
    }

    fn fix_side_cursor(&mut self) {
        let items = self.side_items(self.sidebar.view);
        self.sidebar.fix_cursor(&items);
    }

    /// Keys for the sidebar. True if the key was used.
    fn side_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('\\') => {
                self.toggle_sidebar();
                return true;
            }
            KeyCode::Char('b') if ctrl => {
                self.toggle_sidebar();
                return true;
            }
            KeyCode::Char('<') => {
                self.show_side(SideView::Playing);
                return true;
            }
            KeyCode::Char('>') => {
                self.show_side(SideView::Library);
                return true;
            }
            KeyCode::Char('V') => {
                self.open_side_fx();
                return true;
            }
            _ => {}
        }
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
    fn side_run(&mut self, i: usize) {
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
                    self.send(Cmd::Play(Play::Tracks { uris: vec![track.uri], offset: 0 }));
                    self.poll_soon();
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
    fn open_playlist_uri(&mut self, uri: &str, name: &str) {
        match self.playlists.items.iter().find(|p| p.uri == uri).cloned() {
            Some(pl) if self.owns(&pl) => {
                self.open = Some(OpenPlaylist { playlist: pl, tracks: Listing::default() });
                self.switch_tab(Tab::Playlists);
            }
            _ => {
                self.info(format!("Playing {name}"));
                self.send(Cmd::Play(Play::Context { uri: uri.to_owned(), from: None }));
                self.poll_soon();
            }
        }
    }

    fn side_mouse(&mut self, m: MouseEvent, pos: Position) {
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
    fn toggle_pin(&mut self, pin: Pin) {
        let uri = pin.uri();
        if let Some(i) = self.pins.iter().position(|p| p.uri() == uri) {
            self.unpin(i);
        } else {
            self.info(format!("Pinned {} to the library", pin.name()));
            self.pins.insert(0, pin);
        }
    }

    fn unpin(&mut self, i: usize) {
        if i < self.pins.len() {
            let pin = self.pins.remove(i);
            self.info(format!("Unpinned {}", pin.name()));
            if self.sidebar.view == SideView::Library {
                self.fix_side_cursor();
            }
        }
    }

    /// `*`: pins the selected song, album or playlist (or unpins it).
    fn pin_selected(&mut self) {
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
    fn share_card(&mut self, track: Track) {
        self.info(format!("Making a card for {}…", track.name));
        self.send(Cmd::Card(Box::new(track)));
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

    fn page(range: std::ops::Range<usize>, total: usize) -> ApiPage<Track> {
        let items: Vec<Track> = range.map(track).collect();
        ApiPage { raw: items.len(), items, total }
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

    fn searches(cmds: &[Cmd]) -> Vec<String> {
        cmds.iter()
            .filter_map(|c| match c {
                Cmd::Load { list: List::Search(q), .. } => Some(q.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn search_runs_after_a_pause_in_typing() {
        let (mut app, rx) = test_app();
        app.tab = Tab::Search;
        app.typing = true;
        for c in "bowie".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert!(app.busy(), "the loop wakes for the coming search");
        app.tick();
        assert!(searches(&drain(&rx)).is_empty(), "no search before the pause");
        app.search_due = Some(Instant::now());
        assert!(app.tick());
        assert_eq!(searches(&drain(&rx)), ["bowie"]);
        assert!(app.typing, "you can go on typing");
        // Enter on the same text does not search again.
        app.on_key(KeyEvent::from(KeyCode::Enter));
        app.tick();
        assert!(searches(&drain(&rx)).is_empty());
        // One letter is too short; Esc cancels a coming search.
        app.typing = true;
        app.query.clear();
        app.on_key(KeyEvent::from(KeyCode::Char('x')));
        app.search_due = Some(Instant::now());
        assert!(!app.tick());
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.search_due.is_none());
    }

    #[test]
    fn frame_period_follows_what_moves() {
        let (mut app, _rx) = test_app();
        let viz = Duration::from_millis(50);
        app.in_flight = 0;
        assert_eq!(app.frame_period(viz), Duration::from_secs(1));
        app.in_flight = 1;
        assert_eq!(app.frame_period(viz), Duration::from_millis(120));
        app.in_flight = 0;
        app.playback = Some(serde_json::from_value(serde_json::json!({
            "is_playing": true, "progress_ms": 0,
            "item": {"uri": "spotify:track:a", "name": "A", "duration_ms": 1000, "artists": [], "album": {"name": "x", "images": []}}
        })).unwrap());
        app.viz_on = true;
        assert_eq!(app.frame_period(viz), viz);
    }

    #[test]
    fn sidebar_keys_open_switch_and_pin() {
        use crate::sidebar::{Item as SideItem, SideView};
        let (mut app, rx) = test_app();
        app.typing = false;
        app.on_key(KeyEvent::from(KeyCode::Char('\\')));
        assert!(app.sidebar.open && app.sidebar.focus);
        app.on_key(KeyEvent::from(KeyCode::Char('l')));
        assert_eq!(app.sidebar.view, SideView::Library);
        // The cursor starts on Liked Songs; Enter opens that tab.
        assert_eq!(app.side_items(SideView::Library)[app.sidebar.cursor(SideView::Library)], SideItem::Liked);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.tab, Tab::Liked);
        // Esc gives the keys back to the list; `*` then pins the selected song.
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(!app.sidebar.focus);
        app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: page(0..3, 3) });
        app.on_key(KeyEvent::from(KeyCode::Char('*')));
        assert_eq!(app.pins.len(), 1);
        assert_eq!(app.side_items(SideView::Library)[1], SideItem::Pin(0));
        // Pins and the sidebar are saved, and come back.
        let st = app.saved_state();
        let json = serde_json::to_string(&st).unwrap();
        let (mut again, _rx2) = test_app();
        again.restore(serde_json::from_str(&json).unwrap());
        assert_eq!(again.pins.len(), 1);
        assert!(again.sidebar.open);
        // `*` again unpins; `\` twice shuts (focus first, then close).
        app.on_key(KeyEvent::from(KeyCode::Char('*')));
        assert!(app.pins.is_empty());
        app.on_key(KeyEvent::from(KeyCode::Char('\\')));
        assert!(app.sidebar.focus);
        app.on_key(KeyEvent::from(KeyCode::Char('\\')));
        assert!(!app.sidebar.open);
        drop(rx);
    }

    #[test]
    fn sidebar_actions_use_the_playing_song() {
        use crate::sidebar::SideView;
        let (mut app, rx) = test_app();
        app.typing = false;
        app.playback = Some(serde_json::from_value(serde_json::json!({
            "is_playing": true, "progress_ms": 0,
            "item": {"uri": "spotify:track:p", "name": "P", "duration_ms": 1000, "artists": [], "album": {"name": "x", "images": []}}
        })).unwrap());
        app.on_key(KeyEvent::from(KeyCode::Char('<')));
        assert_eq!(app.sidebar.view, SideView::Playing);
        drain(&rx);
        app.on_key(KeyEvent::from(KeyCode::Char('C')));
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Card(t) if t.uri == "spotify:track:p")));
        app.on_key(KeyEvent::from(KeyCode::Char('*')));
        assert_eq!(app.pins.first().map(|p| p.uri()), Some("spotify:track:p".into()));
        // Space still plays / pauses while the sidebar has the keys.
        app.on_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Toggle)));
    }

    #[test]
    fn undo_tree_keeps_branches_and_reopens_closed_pages() {
        let (mut app, _rx) = test_app();
        app.typing = false;
        app.switch_tab(Tab::Liked);
        app.open_artist("a1".into(), "Radiohead".into());
        // Backspace goes back; a new place from there starts a branch.
        app.on_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(app.loc(), Loc::Base(Tab::Liked));
        app.open_artist("a2".into(), "Portishead".into());
        let liked = app.nav.nodes.iter().position(|n| n.loc == Loc::Base(Tab::Liked)).unwrap();
        assert_eq!(app.nav.nodes[liked].children.len(), 2);
        // Close Portishead, then open the tree with Ctrl+Backspace (^H, as ghostty sends it).
        app.on_key(KeyEvent::from(KeyCode::Char('x')));
        app.on_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        let cursor = app.undo.expect("the tree is open");
        let rows = app.nav.rows();
        assert_eq!(rows[cursor].node, app.nav.cur, "the cursor starts where you are");
        // Go to Portishead: its tab opens again.
        let i = rows.iter().position(|r| app.nav.nodes[r.node].label.ends_with("Portishead")).unwrap();
        app.undo = Some(i);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.undo.is_none());
        assert!(matches!(app.page(), Some(Page::Artist(a)) if a.name == "Portishead"));
        // Ctrl+Backspace while typing a search does not open it.
        app.switch_tab(Tab::Search);
        app.typing = true;
        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert!(app.undo.is_none());
    }

    #[test]
    fn tab_samples_a_genre_then_goes_back_to_the_song() {
        let (mut app, rx) = test_app();
        app.on_msg(playing_in("spotify:playlist:p1"));
        app.on_key(KeyEvent::from(KeyCode::Char('z')));
        drain(&rx);
        // Tab on the first quick pick: pop, its top song, at the volume playing now.
        // Runs the fade to its end (each step as if its time were up), with the commands it sends.
        let fade = |app: &mut App| -> Vec<Cmd> {
            let mut out = Vec::new();
            for _ in 0..10 {
                if let Some((_, v)) = app.fade_at {
                    app.fade_at = Some((Instant::now() - Duration::from_secs(60), v));
                }
                app.tick();
                out.extend(drain(&rx).into_iter().filter(|c| !matches!(c, Cmd::Volume(_) | Cmd::Poll)));
            }
            out
        };
        app.on_key(KeyEvent::from(KeyCode::Tab));
        assert!(drain(&rx).is_empty(), "the music fades out first");
        let cmds = fade(&mut app);
        assert!(matches!(cmds.as_slice(), [Cmd::Sample { genre, nth: 0 }] if genre == "pop"), "{cmds:?}");
        assert_eq!(app.volume(), Some(5), "quiet for the switch");
        // Tab again: the next song of the genre.
        app.on_key(KeyEvent::from(KeyCode::Tab));
        assert!(matches!(fade(&mut app).as_slice(), [Cmd::Sample { nth: 1, .. }]));
        app.on_msg(Msg::Sampling { genre: "pop".into(), result: Ok(track(7)) });
        assert!(app.sample.as_ref().is_some_and(|s| s.started.is_some()));
        fade(&mut app);
        assert_eq!(app.volume(), Some(50), "faded in after the switch");
        // Ten seconds later it ends: back to the playlist, at the song and place it was.
        app.sample.as_mut().unwrap().started = Some(Instant::now() - SAMPLE_LEN);
        let cmds = fade(&mut app);
        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::Unsample { restore: Some(r) }
                if r.context.as_deref() == Some("spotify:playlist:p1") && r.track == "spotify:track:x" && r.position_ms >= 30_000 && r.playing)),
            "{cmds:?}"
        );
        assert_eq!(app.volume(), Some(50), "and back at its volume");
        assert!(app.sample.is_none() && app.genre_picker.is_some(), "the picker stays open");
        // Esc while a sample plays stops only the sample; Esc again closes the picker.
        app.on_key(KeyEvent::from(KeyCode::Tab));
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.sample.is_none() && app.genre_picker.is_some());
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.genre_picker.is_none());
    }

    #[test]
    fn ctrl_backspace_deletes_a_word_in_the_search() {
        let mut t = String::from("daft punk  one more ");
        delete_word(&mut t);
        assert_eq!(t, "daft punk  one ");
        delete_word(&mut t);
        assert_eq!(t, "daft punk  ");
        delete_word(&mut t);
        assert_eq!(t, "daft ");
        delete_word(&mut t);
        assert_eq!(t, "");
        delete_word(&mut t);
        assert_eq!(t, "");
        let mut t = String::from("Björk Jóga");
        delete_word(&mut t);
        assert_eq!(t, "Björk ");

        let (mut app, _rx) = test_app();
        app.switch_tab(Tab::Search);
        app.typing = true;
        app.query = "aphex twin xtal".into();
        // As kitty's keyboard protocol sends it, and as ghostty sends it (^H).
        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(app.query, "aphex twin ");
        app.on_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert_eq!(app.query, "aphex ");
        assert!(app.undo.is_none() && app.typing, "the undo tree stays shut");
        // A plain Backspace still deletes one letter.
        app.on_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(app.query, "aphex");
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
        let page = ApiPage { items: vec![pl("a", "A"), pl("b", "B"), pl("c", "C"), pl("new", "New one")], raw: 4, total: 120 };
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

    fn track_by(id: &str, artist: (&str, &str), album: (&str, &str)) -> Track {
        serde_json::from_value(serde_json::json!({
            "uri": format!("spotify:track:{id}"), "name": format!("Song {id}"), "duration_ms": 1000,
            "artists": [{"name": artist.1, "uri": format!("spotify:artist:{}", artist.0)}],
            "album": {"name": album.1, "id": album.0, "uri": format!("spotify:album:{}", album.0), "images": []}
        }))
        .unwrap()
    }

    #[test]
    fn menu_opens_artist_album_and_back() {
        let (mut app, rx) = test_app();
        app.switch_tab(Tab::Liked);
        let t = track_by("1", ("ar1", "Bowie"), ("al1", "Low"));
        app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: ApiPage { items: vec![t], raw: 1, total: 1 } });
        drain(&rx);

        app.on_key(KeyEvent::from(KeyCode::Char('.')));
        let labels: Vec<String> = app.menu.as_ref().unwrap().items.iter().map(|i| i.1.clone()).collect();
        assert!(labels.iter().any(|l| l == "Go to artist: Bowie"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "Go to album: Low"), "{labels:?}");
        assert!(!labels.iter().any(|l| l.starts_with("Remove from")), "not an editable playlist: {labels:?}");

        // The key hint runs its entry.
        app.on_key(KeyEvent::from(KeyCode::Char('i')));
        assert!(app.menu.is_none());
        assert_eq!(app.view(), View::Artist);
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Artist(id) if id == "ar1")));

        app.on_msg(Msg::Artist {
            artist: serde_json::from_value(serde_json::json!({"id": "ar1", "name": "David Bowie", "images": []})).unwrap(),
            top: vec![track_by("9", ("ar1", "Bowie"), ("al2", "Heroes"))],
            albums: vec![serde_json::from_value(serde_json::json!({"name": "Low", "id": "al1", "uri": "spotify:album:al1"})).unwrap()],
        });
        let Some(Page::Artist(a)) = app.page() else { panic!() };
        // Popular: 1 song; Radio: artist + 1 song radio; Albums: 1. Each section has a title row.
        assert_eq!((a.name.as_str(), a.rows.items.len()), ("David Bowie", 7));
        assert_eq!(a.rows.selected(), 1, "the cursor starts on the first song, not a title");

        // j skips the "Radio" title; Tab jumps to the next section.
        app.move_selection(1);
        let Some(Page::Artist(a)) = app.page() else { panic!() };
        assert!(matches!(a.rows.current(), Some(ArtistRow::Radio(t)) if t.uri == "spotify:artist:ar1"));
        app.move_selection(-1);
        app.on_key(KeyEvent::from(KeyCode::Tab));
        let Some(Page::Artist(a)) = app.page() else { panic!() };
        assert_eq!(a.rows.selected(), 3);

        // Enter on the artist radio opens it.
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.view(), View::Radio);
        if crate::client::AVAILABLE {
            assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Radio(u) if u == "spotify:artist:ar1")));
        }
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.view(), View::Artist);
        drain(&rx);

        // Tab again: the album section. Enter opens the album, and its tracks load.
        app.on_key(KeyEvent::from(KeyCode::Tab));
        app.on_key(KeyEvent::from(KeyCode::BackTab));
        let Some(Page::Artist(a)) = app.page() else { panic!() };
        assert_eq!(a.rows.selected(), 3, "shift+tab goes back one section");
        app.on_key(KeyEvent::from(KeyCode::Tab));
        app.on_key(KeyEvent::from(KeyCode::Tab));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.view(), View::Album);
        app.tick();
        let cmds = drain(&rx);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Load { list: List::Album(id), offset: 0 } if id == "al1")), "{cmds:?}");
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Album(id) if id == "al1")), "details asked for: {cmds:?}");

        // Album tracks get the album (for covers), and Enter plays the album from that song.
        let bare: Track = serde_json::from_value(serde_json::json!({"uri": "spotify:track:a1", "name": "Speed of Life"})).unwrap();
        app.on_msg(Msg::Tracks { list: List::Album("al1".into()), offset: 0, page: ApiPage { items: vec![bare], raw: 1, total: 1 } });
        assert_eq!(app.selected_track().unwrap().album_name(), "Low");
        app.on_key(KeyEvent::from(KeyCode::Enter));
        let cmds = drain(&rx);
        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::Play(Play::Context { uri, from: Some(f) }) if uri == "spotify:album:al1" && f == "spotify:track:a1")),
            "{cmds:?}"
        );

        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.view(), View::Artist);
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.view(), View::Liked);

        // Esc went back, but the artist, radio and album tabs stay open.
        let keys: Vec<String> = app.pages.iter().map(Page::key).collect();
        assert_eq!(keys, ["artist:ar1", "radio:spotify:artist:ar1", "album:al1"]);
        // 6 is the second page tab (after Search, Playlists, Liked, Recent).
        app.on_key(KeyEvent::from(KeyCode::Char('6')));
        assert_eq!(app.view(), View::Radio);
        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        assert_eq!(app.view(), View::Playlists);
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.view(), View::Radio, "Esc goes back to the radio tab");
        // Opening the artist again goes to its tab, without a new load.
        drain(&rx);
        app.open_artist("ar1".into(), "Bowie".into());
        assert_eq!((app.view(), app.pages.len()), (View::Artist, 3));
        assert!(drain(&rx).is_empty());
        // L and H walk the tab bar; x closes the tab and goes back.
        app.on_key(KeyEvent::from(KeyCode::Char('L')));
        assert_eq!(app.view(), View::Radio);
        app.on_key(KeyEvent::from(KeyCode::Char('x')));
        assert_eq!(app.pages.len(), 2);
        assert_eq!(app.view(), View::Artist);
        app.on_key(KeyEvent::from(KeyCode::Char('H')));
        assert_eq!(app.view(), View::Recent);
    }

    #[test]
    fn tabs_come_back_at_start_and_load_when_shown() {
        let (mut app, rx) = test_app();
        app.open_artist("ar1".into(), "Bowie".into());
        app.start_radio(track_by("s", ("ar1", "Bowie"), ("al1", "Low")));
        let saved = app.saved_state();
        let text = serde_json::to_string(&saved).unwrap();
        let back: State = serde_json::from_str(&text).unwrap();
        drain(&rx);

        let (mut app, rx) = test_app();
        app.restore(back);
        assert_eq!(app.pages.iter().map(Page::key).collect::<Vec<_>>(), ["artist:ar1", "radio:spotify:track:s"]);
        assert_eq!(app.view(), View::Radio, "the tab shown at quit");
        let cmds = drain(&rx);
        if crate::client::AVAILABLE {
            assert!(cmds.iter().any(|c| matches!(c, Cmd::Radio(u) if u == "spotify:track:s")), "{cmds:?}");
        }
        assert!(!cmds.iter().any(|c| matches!(c, Cmd::Artist(_))), "the artist waits: {cmds:?}");
        app.on_key(KeyEvent::from(KeyCode::Char('5')));
        assert_eq!(app.view(), View::Artist);
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Artist(id) if id == "ar1")));
        // Shown once, it does not load again.
        app.on_key(KeyEvent::from(KeyCode::Char('6')));
        app.on_key(KeyEvent::from(KeyCode::Char('5')));
        assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::Artist(_))));
    }

    #[test]
    fn theme_picker_previews_and_esc_puts_it_back() {
        let (mut app, _rx) = test_app();
        assert_eq!(crate::theme::name(), "amber");
        app.on_key(KeyEvent::from(KeyCode::Char('T')));
        let menu = app.menu.as_ref().unwrap();
        assert!(menu.is_theme());
        assert!(menu.items.iter().any(|i| i.2 == Action::Theme("tokyo-night".into())), "built-ins listed");
        // Moving previews; Esc goes back.
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(crate::theme::name(), "rose-pine");
        assert_eq!(crate::theme::c().logo, '✦');
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(crate::theme::name(), "amber");
        assert!(app.menu.is_none());
        // Enter keeps it, and the saved state carries it.
        app.on_key(KeyEvent::from(KeyCode::Char('T')));
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(crate::theme::name(), "gruvbox");
        assert_eq!(app.saved_state().theme.as_deref(), Some("gruvbox"));
        crate::theme::apply("amber").unwrap();
    }

    #[test]
    fn palette_finds_and_runs_actions() {
        let (mut app, _rx) = test_app();
        app.on_key(KeyEvent::from(KeyCode::Char(':')));
        assert!(app.palette.is_some());
        for c in "liked".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(app.palette_entries()[0].label, "Go to Liked");
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.palette.is_none());
        assert_eq!(app.view(), View::Liked);
        // Ctrl+P opens it too; Esc closes it and does nothing.
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert!(app.palette.is_some());
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.palette.is_none());
        assert_eq!(app.view(), View::Liked);
    }

    fn playing_in(context: &str) -> Msg {
        Msg::Playback(Some(Box::new(
            serde_json::from_value(serde_json::json!({
                "device": {"id": "d", "name": "spotlite", "is_active": true, "volume_percent": 50},
                "is_playing": true, "progress_ms": 30_000, "shuffle_state": false,
                "context": if context.is_empty() { serde_json::Value::Null } else { serde_json::json!({ "uri": context }) },
                "item": {"uri": "spotify:track:x", "name": "X", "duration_ms": 200_000}
            }))
            .unwrap(),
        )))
    }

    #[test]
    fn true_shuffle_plays_the_whole_list_in_a_random_order() {
        let (mut app, rx) = test_app();
        app.on_msg(playing_in("spotify:playlist:p1"));
        drain(&rx);
        // On: the playlist starts again, shuffled by spotlite, from the song playing.
        app.on_key(KeyEvent::from(KeyCode::Char('s')));
        let cmds = drain(&rx);
        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::PlayShuffled { context, from: Some(f), position_ms }
                if context == "spotify:playlist:p1" && f == "spotify:track:x" && *position_ms >= 30_000)),
            "{cmds:?}"
        );
        // The songs play as a list: the player still shows the playlist, and ⇄ is on.
        app.on_msg(Msg::Shuffled { context: "spotify:playlist:p1".into(), uris: vec!["spotify:track:x".into()] });
        app.on_msg(playing_in(""));
        let pb = app.playback.as_ref().unwrap();
        assert_eq!(pb.context.as_ref().map(|c| c.uri.as_str()), Some("spotify:playlist:p1"));
        assert!(pb.shuffle_state);
        // Playing an album now: shuffled too.
        drain(&rx);
        app.send(Cmd::Play(Play::Context { uri: "spotify:album:a".into(), from: None }));
        assert!(matches!(drain(&rx).as_slice(), [Cmd::PlayShuffled { context, from: None, .. }] if context == "spotify:album:a"));
        // Off: back to the playlist's order at the same song.
        app.on_key(KeyEvent::from(KeyCode::Char('s')));
        let cmds = drain(&rx);
        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::PlayAt(Play::Context { uri, from: Some(f) }, _) if uri == "spotify:playlist:p1" && f == "spotify:track:x")),
            "{cmds:?}"
        );
    }

    #[test]
    fn spotify_shuffle_turned_on_elsewhere_becomes_the_true_shuffle() {
        let (mut app, rx) = test_app();
        drain(&rx);
        let Msg::Playback(Some(mut pb)) = playing_in("spotify:playlist:p1") else { panic!() };
        pb.shuffle_state = true;
        app.on_msg(Msg::Playback(Some(pb)));
        assert!(app.true_shuffle);
        let cmds = drain(&rx);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Shuffle(false))), "{cmds:?}");
        assert!(cmds.iter().any(|c| matches!(c, Cmd::PlayShuffled { .. })), "{cmds:?}");
    }

    #[test]
    fn randomize_picks_a_genre_then_fills_a_page() {
        let (mut app, rx) = test_app();
        // z opens the picker; typing searches it.
        app.on_key(KeyEvent::from(KeyCode::Char('z')));
        assert!(app.genre_picker.is_some());
        for c in "shoeg".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        drain(&rx);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.genre_picker.is_none());
        assert_eq!(app.view(), View::Random);
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Randomize(g) if g == "shoegaze")));
        app.on_msg(Msg::Random { genre: "shoegaze".into(), result: Ok((0..50).map(track).collect()) });
        assert_eq!(app.tracks().unwrap().items.len(), 50);
        // d rolls again; S asks to save.
        app.on_key(KeyEvent::from(KeyCode::Char('d')));
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Randomize(_))));
        app.on_msg(Msg::Random { genre: "shoegaze".into(), result: Ok((50..100).map(track).collect()) });
        app.can_edit = true;
        app.on_key(KeyEvent::from(KeyCode::Char('S')));
        let confirm = app.confirm.as_ref().expect("asks first");
        assert!(matches!(&confirm.cmd, Cmd::CreatePlaylist { name, uris, .. } if name == "Random · shoegaze 2" && uris.len() == 50));
    }

    #[test]
    fn a_limited_list_keeps_what_it_shows_and_loads_again_later() {
        let (mut app, rx) = test_app();
        app.switch_tab(Tab::Liked);
        app.tick();
        app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: crate::api::Page { items: vec![track(1)], raw: 1, total: 1 } });
        app.on_msg(Msg::RetryLater { list: List::Liked, secs: 0 });
        assert_eq!(app.liked.items.len(), 1);
        assert!(app.toast.as_ref().is_some_and(|t| !t.error && t.text.contains("saved copy")));
        // The wait is at least 5 s; pretend it is over.
        app.retries[0].1 = Instant::now();
        drain(&rx);
        app.tick();
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Load { list: List::Liked, offset: 0 })));
    }

    #[test]
    fn shuffle_is_kept_until_spotify_agrees() {
        let (mut app, _rx) = test_app();
        app.on_msg(playing(40, 100_000));
        app.on_key(KeyEvent::from(KeyCode::Char('s')));
        // A poll that left before the change still says off: the screen keeps on.
        app.on_msg(playing(40, 100_000));
        assert!(app.playback.as_ref().unwrap().shuffle_state);
        assert!(app.saved_state().playback.unwrap().shuffle, "and it is what is saved");
    }

    #[test]
    fn everything_comes_back_at_start() {
        let (mut app, rx) = test_app();
        app.on_msg(Msg::Playback(Some(Box::new(
            serde_json::from_value(serde_json::json!({
                "device": {"id": "d", "name": "spotlite", "is_active": true, "volume_percent": 55},
                "is_playing": true, "progress_ms": 61_000, "shuffle_state": true, "repeat_state": "context",
                "context": {"uri": "spotify:playlist:p1"},
                "item": {"uri": "spotify:track:x", "name": "X", "duration_ms": 200_000}
            }))
            .unwrap(),
        ))));
        app.query = "bowie".into();
        app.submit_search();
        app.on_key(KeyEvent::from(KeyCode::Char('t')));
        app.on_key(KeyEvent::from(KeyCode::Char('3')));
        app.viz_on = true;
        let text = serde_json::to_string(&app.saved_state()).unwrap();
        drain(&rx);

        // Next start: tab, search, visualizer and the song come back.
        let (mut app, rx) = test_app();
        app.restore(serde_json::from_str(&text).unwrap());
        assert_eq!((app.tab, app.searched.as_str(), app.search_kind, app.viz_on), (Tab::Liked, "bowie", SearchKind::Artists, true));
        // Nothing plays yet: it waits for the player state and for spotifyd.
        app.tick();
        assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::Resume(_))));
        app.on_msg(Msg::Playback(None));
        app.on_msg(Msg::Device(Some("spotlite".into())));
        app.tick();
        let cmds = drain(&rx);
        let Some(Cmd::Resume(sp)) = cmds.iter().find(|c| matches!(c, Cmd::Resume(_))) else { panic!("{cmds:?}") };
        assert_eq!((sp.track.as_str(), sp.context.as_deref()), ("spotify:track:x", Some("spotify:playlist:p1")));
        assert!(sp.position_ms >= 61_000 && sp.playing && sp.shuffle && sp.repeat == "context");
        assert_eq!(sp.volume, Some(55));
        app.tick();
        assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::Resume(_))), "only once");

        // If music already plays at start (spotifyd kept going), it is left alone.
        let (mut app, rx) = test_app();
        app.restore(serde_json::from_str(&text).unwrap());
        app.on_msg(playing(40, 100_000));
        app.on_msg(Msg::Device(Some("spotlite".into())));
        app.tick();
        assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::Resume(_))));
    }

    #[test]
    fn recent_tab_loads_each_visit() {
        let (mut app, rx) = test_app();
        app.on_key(KeyEvent::from(KeyCode::Char('4')));
        assert_eq!(app.view(), View::Recent);
        app.tick();
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Load { list: List::Recent, offset: 0 })));
        let t = track_by("1", ("a", "A"), ("b", "B"));
        app.on_msg(Msg::Tracks { list: List::Recent, offset: 0, page: ApiPage { items: vec![t], raw: 1, total: 1 } });
        app.on_key(KeyEvent::from(KeyCode::Char('1')));
        app.on_key(KeyEvent::from(KeyCode::Char('4')));
        assert!(app.recent.items.is_empty(), "loads again");
    }

    #[test]
    fn search_lists_artists_albums_and_playlists() {
        use crate::api::{Artist, Found};
        let (mut app, rx) = test_app();
        app.query = "bowie".into();
        app.submit_search();
        app.on_key(KeyEvent::from(KeyCode::Char('t')));
        assert_eq!(app.search_kind, SearchKind::Artists);
        app.tick();
        let cmds = drain(&rx);
        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::Load { list: List::Found { query, kind: "artist" }, offset: 0 } if query == "bowie")),
            "{cmds:?}"
        );
        let artist: Artist = serde_json::from_value(serde_json::json!({"id": "ar1", "name": "David Bowie"})).unwrap();
        // An answer for another kind is dropped.
        let other = List::Found { query: "bowie".into(), kind: "album" };
        app.on_msg(Msg::Found { list: other, offset: 0, page: ApiPage { items: vec![Found::Artist(artist.clone())], raw: 1, total: 1 } });
        assert!(app.found.items.is_empty());
        let list = List::Found { query: "bowie".into(), kind: "artist" };
        app.on_msg(Msg::Found { list, offset: 0, page: ApiPage { items: vec![Found::Artist(artist)], raw: 1, total: 1 } });
        assert_eq!(app.list_len(), 1);
        // Enter opens the artist in its own tab.
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.view(), View::Artist);
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Artist(id) if id == "ar1")));
        // Back on Search, the artists are still listed.
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!((app.view(), app.found.items.len()), (View::Search, 1));
        for _ in 0..3 {
            app.on_key(KeyEvent::from(KeyCode::Char('t')));
        }
        assert_eq!(app.search_kind, SearchKind::Songs, "t goes round");
    }

    #[test]
    fn like_the_selected_song_and_save_a_radio() {
        let (mut app, rx) = test_app();
        app.switch_tab(Tab::Liked);
        let t = track_by("1", ("a", "A"), ("b", "B"));
        app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: ApiPage { items: vec![t.clone()], raw: 1, total: 1 } });
        drain(&rx);
        // f is for the selected song, not only the playing one.
        app.on_key(KeyEvent::from(KeyCode::Char('f')));
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::ToggleSaved(u) if *u == t.uri)));
        app.on_msg(Msg::Saved { uri: t.uri.clone(), saved: false });
        assert!(app.liked.items.is_empty(), "an unliked song leaves Liked Songs");
        assert_eq!(app.liked.total, Some(0));

        // S on a radio asks, then makes a playlist of the list on screen.
        app.start_radio(track_by("s", ("ar1", "A"), ("al1", "X")));
        let Some(Page::Radio(r)) = app.page_mut() else { panic!() };
        r.tracks.items = vec![t.clone(), track_by("2", ("c", "C"), ("d", "D"))];
        drain(&rx);
        app.on_key(KeyEvent::from(KeyCode::Char('S')));
        assert!(app.confirm.as_ref().unwrap().text.contains("2 songs"));
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        let cmds = drain(&rx);
        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::CreatePlaylist { name, uris, .. } if name == "Song s radio" && uris.len() == 2)),
            "{cmds:?}"
        );
    }

    #[test]
    fn page_tabs_are_capped() {
        let (mut app, _rx) = test_app();
        for i in 0..MAX_PAGES + 2 {
            app.open_artist(format!("a{i}"), format!("A{i}"));
        }
        assert_eq!(app.pages.len(), MAX_PAGES);
        assert_eq!(app.pages[0].key(), "artist:a2", "the oldest tabs closed");
        assert_eq!(app.page().map(Page::key).as_deref(), Some("artist:a9"));
        for _ in 0..20 {
            app.on_key(KeyEvent::from(KeyCode::Esc));
        }
        assert_eq!(app.view(), View::Search);
    }

    #[test]
    fn radio_filters_update_the_list() {
        use crate::radio::RadioTrack;
        let (mut app, rx) = test_app();
        app.switch_tab(Tab::Liked);
        let seed = track_by("s", ("ar1", "A"), ("al1", "X"));
        app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: ApiPage { items: vec![seed.clone()], raw: 1, total: 1 } });
        drain(&rx);
        app.on_key(KeyEvent::from(KeyCode::Char('o')));
        assert_eq!(app.view(), View::Radio);
        let cmds = drain(&rx);
        if crate::client::AVAILABLE {
            assert!(cmds.iter().any(|c| matches!(c, Cmd::Radio(u) if u == "spotify:track:s")), "{cmds:?}");
            assert!(cmds.iter().any(|c| matches!(c, Cmd::Library)), "{cmds:?}");
        }
        let rt = |id: &str, artist: &str, explicit: bool| RadioTrack {
            uri: format!("spotify:track:{id}"),
            name: id.into(),
            artists: vec![artist.into()],
            artist_uris: vec![format!("spotify:artist:{artist}")],
            album: String::new(),
            year: Some(2000),
            duration_ms: 1000,
            popularity: 50,
            explicit,
            album_uri: None,
            images: Vec::new(),
        };
        let result = Ok((rt("s", "ar1", false), vec![rt("1", "x", true), rt("2", "y", false), rt("3", "y", false)]));
        app.on_msg(Msg::Radio { seed: "spotify:track:s".into(), result });
        assert_eq!(app.tracks().unwrap().items.len(), 3);

        app.on_key(KeyEvent::from(KeyCode::Char('F')));
        // Fourth entry (after Discover): hide explicit songs; the menu stays open and the list updates.
        app.move_popup(3);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.menu.is_some());
        assert_eq!(app.tracks().unwrap().items.len(), 2);
        assert!(app.menu.as_ref().unwrap().items[3].1.ends_with(": on"));
        // Songs per artist: at most 2, then at most 1.
        app.move_popup(2);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.tracks().unwrap().items.len(), 1);
        app.on_key(KeyEvent::from(KeyCode::Esc));

        // Enter plays the filtered list.
        app.on_key(KeyEvent::from(KeyCode::Enter));
        let cmds = drain(&rx);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Play(Play::Tracks { uris, .. }) if uris == &["spotify:track:2"])), "{cmds:?}");
    }

    #[test]
    fn discover_rounds_bring_new_songs() {
        use crate::radio::{Library, RadioTrack};
        let (mut app, rx) = test_app();
        let rt = |id: &str, artist: &str| RadioTrack {
            uri: format!("spotify:track:{id}"),
            name: id.into(),
            artists: vec![artist.into()],
            artist_uris: vec![format!("spotify:artist:{artist}")],
            album: String::new(),
            year: Some(2000),
            duration_ms: 1000,
            popularity: 50,
            explicit: false,
            album_uri: None,
            images: Vec::new(),
        };
        app.start_radio(track_by("s", ("ar1", "A"), ("al1", "X")));
        drain(&rx);
        // Before the radio loads, d only says to wait.
        app.on_key(KeyEvent::from(KeyCode::Char('d')));
        assert!(drain(&rx).is_empty());
        app.on_msg(Msg::Library(Library {
            tracks: ["spotify:track:1".to_owned()].into(),
            artists: ["spotify:artist:liked".to_owned()].into(),
            ..Default::default()
        }));
        let radio = vec![rt("1", "x"), rt("2", "liked"), rt("3", "ar1"), rt("4", "y"), rt("5", "z")];
        app.on_msg(Msg::Radio { seed: "spotify:track:s".into(), result: Ok((rt("s", "ar1"), radio)) });
        assert_eq!(app.tracks().unwrap().items.len(), 5);

        // Round 1: only songs you do not know, and their radios are asked for.
        app.on_key(KeyEvent::from(KeyCode::Char('d')));
        let uris = |app: &App| {
            let mut u: Vec<String> = app.tracks().unwrap().items.iter().map(|t| t.uri.clone()).collect();
            u.sort();
            u
        };
        assert_eq!(uris(&app), ["spotify:track:4", "spotify:track:5"]);
        let cmds = drain(&rx);
        let Some(Cmd::RadioMore { radio, seeds }) = cmds.iter().find(|c| matches!(c, Cmd::RadioMore { .. })) else {
            panic!("{cmds:?}")
        };
        assert_eq!(radio, "spotify:track:s");
        let mut seeds = seeds.clone();
        seeds.sort();
        assert_eq!(seeds, ["spotify:track:4", "spotify:track:5"]);
        app.on_key(KeyEvent::from(KeyCode::Char('d')));
        assert!(drain(&rx).is_empty(), "one round at a time");
        let found = vec![rt("4", "y"), rt("6", "liked"), rt("7", "w"), rt("8", "v")];
        app.on_msg(Msg::RadioMore { radio: "spotify:track:s".into(), result: Ok(found) });
        assert_eq!(uris(&app), ["spotify:track:4", "spotify:track:5", "spotify:track:7", "spotify:track:8"]);

        // Round 2: the songs of round 1 go.
        app.on_key(KeyEvent::from(KeyCode::Char('d')));
        assert!(app.tracks().unwrap().items.is_empty());
        app.on_msg(Msg::RadioMore { radio: "spotify:track:s".into(), result: Ok(vec![rt("9", "u")]) });
        assert_eq!(uris(&app), ["spotify:track:9"]);
        let Some(Page::Radio(r)) = app.page() else { panic!() };
        assert_eq!(r.round, 2);

        // A restart keeps discover: the same round and songs once the radio loads again.
        let saved: crate::state::State = serde_json::from_str(&serde_json::to_string(&app.saved_state()).unwrap()).unwrap();
        let (mut again, _rx2) = test_app();
        again.restore(saved);
        again.on_msg(Msg::Library(Library {
            tracks: ["spotify:track:1".to_owned()].into(),
            artists: ["spotify:artist:liked".to_owned()].into(),
            ..Default::default()
        }));
        let radio = vec![rt("1", "x"), rt("2", "liked"), rt("3", "ar1"), rt("4", "y"), rt("5", "z")];
        again.on_msg(Msg::Radio { seed: "spotify:track:s".into(), result: Ok((rt("s", "ar1"), radio)) });
        let Some(Page::Radio(r)) = again.page() else { panic!("the radio page is back") };
        assert!(r.filters.discover);
        assert_eq!(r.round, 2);
        assert_eq!(uris(&again), ["spotify:track:9"]);

        // Reset goes back to the plain radio.
        app.on_key(KeyEvent::from(KeyCode::Char('F')));
        let reset = app.menu.as_ref().unwrap().items.len() - 1;
        app.move_popup(reset as isize);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.tracks().unwrap().items.len(), 5);
    }

    #[test]
    fn add_and_remove_with_picker_and_confirm() {
        let (mut app, rx) = test_app();
        app.on_msg(Msg::User(serde_json::from_value(serde_json::json!({"id": "me", "uri": "spotify:user:me"})).unwrap()));
        let theirs: Playlist = serde_json::from_value(serde_json::json!({
            "id": "t", "uri": "spotify:playlist:t", "name": "Theirs", "owner": {"id": "x"}
        }))
        .unwrap();
        app.on_msg(Msg::Playlists {
            offset: 0,
            page: ApiPage { items: vec![pl("p1", "Gym"), pl("p2", "Chill"), theirs], raw: 3, total: 3 },
        });
        app.tab = Tab::Playlists;
        app.open = Some(OpenPlaylist { playlist: pl("p1", "Gym"), tracks: Listing::default() });
        let t = track_by("1", ("a", "A"), ("b", "B"));
        app.on_msg(Msg::Tracks { list: List::Playlist("p1".into()), offset: 0, page: ApiPage { items: vec![t], raw: 1, total: 1 } });
        drain(&rx);

        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        assert_eq!(app.picker_choices().len(), 2, "only your own playlists");
        for c in "chi".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(app.picker_choices().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Chill"]);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        let cmds = drain(&rx);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::AddToPlaylist { playlist_id, uris, .. } if playlist_id == "p2" && uris == &["spotify:track:1"])), "{cmds:?}");

        app.on_key(KeyEvent::from(KeyCode::Char('D')));
        assert!(app.confirm.is_some());
        app.on_key(KeyEvent::from(KeyCode::Char('n')));
        assert!(app.confirm.is_none());
        assert!(drain(&rx).is_empty(), "cancel sends nothing");
        app.on_key(KeyEvent::from(KeyCode::Char('D')));
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        let cmds = drain(&rx);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::RemoveFromPlaylist { playlist_id, uri, .. } if playlist_id == "p1" && uri == "spotify:track:1")), "{cmds:?}");
        app.on_msg(Msg::Removed { playlist_id: "p1".into(), uri: "spotify:track:1".into() });
        assert!(app.open.as_ref().unwrap().tracks.items.is_empty());
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
