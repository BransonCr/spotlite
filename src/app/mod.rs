//! The app state and what changes it: keys, mouse, messages from the API threads, time.

use crate::api::{Album, Found, Page as ApiPage, Play, Playback, Playlist, Track};
use crate::control::Ctl;
use crate::folders::Entry;
use crate::images::Images;
use crate::input::{is_ctrl_backspace, Edit, Input};
use crate::keys::{Act, Keys};
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

mod genre;
mod keyboard;
mod lists;
mod menus;
mod mouse;
mod nav;
mod player;
mod search;
mod side;
#[cfg(test)]
mod tests;

pub use lists::{Change, Listing, Shape, Viewable};
#[cfg(test)]
use lists::SortBy;

/// How often to ask Spotify for the player state. Progress is interpolated between polls.
/// How long the like animation runs.
pub const LIKE_BURST: Duration = Duration::from_millis(720);
/// How often to ask Spotify for the player state: often while music plays (a song can change
/// from another device), less while it is paused, rarely with nothing loaded. The end of a
/// song and spotlite's own commands ask at once anyway.
const POLL_PLAYING: Duration = Duration::from_secs(8);
const POLL_PAUSED: Duration = Duration::from_secs(20);
const POLL_IDLE: Duration = Duration::from_secs(30);
/// Recently played loads again on a visit when it is older than this (or the song changed).
const RECENT_AGE: Duration = Duration::from_secs(60);
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
    /// A Spotify Connect device to play on (the device menu).
    Device { id: String, name: String },
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
    pub query: Input,
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
    /// The scrollbar of the list shown (empty when the list fits).
    pub scrollbar: Rect,
    /// The bar part of the progress line, and the time after it.
    pub progress: Rect,
    pub time: Rect,
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
    /// The list's scrollbar.
    Scroll,
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
    pub query: Input,
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
    /// Messages that wait for an error to have been read.
    toasts: std::collections::VecDeque<Toast>,
    pub help: bool,
    /// The filter typed in help, and how far help is scrolled.
    pub help_query: Input,
    pub help_scroll: usize,
    /// The keys in use (the table in keys.rs, with `[keys]` from config.toml).
    pub keys: Keys,
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
    /// The songs after `up_next`, as Spotify listed them: a song change takes the next from
    /// here instead of asking again.
    queue: std::collections::VecDeque<Track>,
    /// Spotify cannot be reached: the header says so, and the poll waits longer.
    pub offline: bool,
    /// The playlists shown came from the saved copy: the first page is asked for again.
    playlists_stale: bool,
    /// Typing into the filter of the list shown (Ctrl+F).
    pub filtering: bool,
    /// Songs marked in the list shown (by URI): actions then apply to all of them.
    pub marks: std::collections::HashSet<String>,
    /// Changes that `u` can undo, newest last.
    changes: Vec<Change>,
    /// Playlists songs were added to, most recent first (by id): the picker lists them first.
    pub recent_playlists: Vec<String>,
    /// Earlier searches, newest first, and the one the search box shows (↑ ↓).
    pub search_history: Vec<String>,
    history_at: Option<usize>,
    /// A song to select once its list has loaded (`c` opened the list it plays from).
    pending_jump: Option<String>,
    /// Playlists opened before, with their scroll, selection, filter and sort.
    open_cache: Vec<OpenPlaylist>,
    /// The list row under the mouse pointer (it gets a faint highlight).
    pub hover: Option<usize>,
    /// The player shows the time left instead of the time played (a click on it).
    pub remaining: bool,
    /// When Recently played last loaded, and the song playing then.
    recent_at: Option<(Instant, Option<String>)>,
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
            query: Input::default(),
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
            toasts: Default::default(),
            help: false,
            help_query: Input::default(),
            help_scroll: 0,
            keys: Keys::default(),
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
            next_poll: now + POLL_PLAYING,
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
            queue: Default::default(),
            offline: false,
            playlists_stale: false,
            filtering: false,
            marks: Default::default(),
            changes: Vec::new(),
            recent_playlists: Vec::new(),
            search_history: Vec::new(),
            history_at: None,
            pending_jump: None,
            open_cache: Vec::new(),
            hover: None,
            remaining: false,
            recent_at: None,
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

    fn send(&mut self, cmd: Cmd) {
        let cmd = self.with_true_shuffle(cmd);
        if self.tx.send(cmd).is_ok() {
            self.in_flight += 1;
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
        let toast = Toast { text, error: false, until: Instant::now() + Duration::from_secs(secs) };
        // An error stays at least 3 s: a message after it waits its turn.
        let fresh_error = self.toast.as_ref().is_some_and(|t| t.error && t.until > Instant::now() + Duration::from_secs(5));
        if fresh_error {
            self.toasts.push_back(toast);
            while self.toasts.len() > 3 {
                self.toasts.pop_front();
            }
        } else {
            self.toast = Some(toast);
        }
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

    pub fn owns(&self, playlist: &Playlist) -> bool {
        playlist.collaborative || self.user_id.as_deref() == Some(playlist.owner.id.as_str())
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
            self.toast = self.toasts.pop_front().map(|mut t| {
                t.until = now + Duration::from_secs(4);
                t
            });
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
            self.next_poll = now + self.poll_every();
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
        } else if self.playback.as_ref().is_some_and(|pb| pb.is_playing) {
            // The time and the progress bar move each second.
            Duration::from_secs(1)
        } else {
            // Nothing moves: sleep until the next thing that is due (a poll, a message that
            // ends), 5 s at most (the state is saved every 5 s).
            let now = Instant::now();
            let due = [Some(self.next_poll), self.toast.as_ref().map(|t| t.until), self.search_due]
                .into_iter()
                .flatten()
                .map(|at| at.saturating_duration_since(now))
                .min()
                .unwrap_or(Duration::from_secs(5));
            due.clamp(Duration::from_millis(50), Duration::from_secs(5))
        }
    }

    fn load_more(&mut self) {
        let jumping = self.pending_jump.is_some();
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
            View::Liked => self.liked.wants_page().or_else(|| self.liked.more_for(jumping)).map(|o| (List::Liked, o)),
            View::Recent => self.recent.wants_page().map(|o| (List::Recent, o)),
            View::Playlist => self.open.as_ref().and_then(|o| {
                o.tracks
                    .wants_page()
                    .or_else(|| o.tracks.more_for(jumping))
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
        // Playlists shown from the saved copy: the first page once more (the worker answers
        // from its copy when it is recent, so this costs a request only when it is old).
        let request = request.or_else(|| {
            (self.playlists_stale && !self.playlists.loading).then(|| {
                self.playlists_stale = false;
                (List::Playlists, 0)
            })
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
                self.next_poll = Instant::now() + if std::mem::take(&mut self.repoll) { CONFIRM_AFTER } else { self.poll_every() };
                if let Some(d) = self.playback.as_ref().and_then(|p| p.device.as_ref()) {
                    self.device = Some(d.name.clone());
                }
                match new_uri {
                    Some(uri) if self.saved.as_ref().is_none_or(|(u, _)| *u != uri) => {
                        // The ♥: from the Liked index when it is there (no request).
                        self.saved = self.library.as_ref().map(|l| (uri.clone(), l.tracks.contains(&uri)));
                        if self.saved.is_none() && uri.starts_with("spotify:track:") {
                            self.send(Cmd::CheckSaved(uri.clone()));
                        }
                        self.song_changed(&uri);
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
            Msg::Removed { playlist_id, uris, text } => {
                self.info(format!("{text} · {} undo", self.keys.show(Act::Undo)));
                if let Some(o) = self.open.as_mut().filter(|o| o.playlist.id == playlist_id) {
                    let before = o.tracks.items.len();
                    o.tracks.items.retain(|t| !uris.contains(&t.uri));
                    let removed = before - o.tracks.items.len();
                    o.tracks.total = o.tracks.total.map(|t| t.saturating_sub(removed));
                    o.tracks.fetched = o.tracks.fetched.saturating_sub(removed);
                    o.tracks.changed();
                }
                self.open_cache.retain(|o| o.playlist.id != playlist_id);
            }
            Msg::UpNext(next) => {
                self.queue = next.into();
                self.up_next = self.queue.pop_front();
            }
            Msg::Liked(songs) => {
                // Shown at once, with the selection kept where it was.
                self.liked.fill(songs);
            }
            Msg::Created(pl) => {
                self.playlists.items.insert(0, *pl);
                self.playlists.fetched += 1;
                self.playlists.total = self.playlists.total.map(|t| t + 1);
                self.rebuild_rows();
            }
            Msg::Devices(list) => self.open_devices(list),
            Msg::Added { playlist_id, text } => {
                self.info(format!("{text} · {} undo", self.keys.show(Act::Undo)));
                self.open_cache.retain(|o| o.playlist.id != playlist_id);
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
                self.finish_jump();
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
                    // The song goes to the top of Liked Songs (no new load).
                    if let Some(t) = self.find_track(&uri).filter(|_| !self.liked.items.iter().any(|t| t.uri == uri)) {
                        let keep = self.liked.index(self.liked.selected()).map(|i| i + 1);
                        self.liked.items.insert(0, t);
                        self.liked.fetched += 1;
                        self.liked.total = self.liked.total.map(|n| n + 1);
                        self.liked.reshape(keep);
                    }
                    if let Some(lib) = self.library.as_mut() {
                        if !lib.liked_order.contains(&uri) {
                            lib.liked_order.insert(0, uri.clone());
                        }
                    }
                } else {
                    let before = self.liked.items.len();
                    self.liked.items.retain(|t| t.uri != uri);
                    if self.liked.items.len() < before {
                        self.liked.total = self.liked.total.map(|t| t.saturating_sub(1));
                        self.liked.fetched = self.liked.fetched.saturating_sub(1);
                        self.liked.changed();
                    }
                }
            }
            Msg::Info(text) => self.info(text),
            Msg::Error(text) => self.error(text),
            Msg::Offline(times) => {
                if !self.offline {
                    self.info("No connection to Spotify. spotlite keeps trying.");
                }
                self.offline = true;
                let wait = [5, 10, 20, 60][(times as usize).saturating_sub(1).min(3)];
                self.next_poll = Instant::now() + Duration::from_secs(wait);
            }
            Msg::Online => {
                self.offline = false;
                self.info("Connected to Spotify again.");
            }
            Msg::Backoff(secs) => self.next_poll = Instant::now() + Duration::from_secs(secs.max(5)),
        }
    }
}

/// Deletes the last word of `text`, and the spaces after it (as Ctrl+Backspace does in most
/// text boxes). Punctuation counts as part of the word.
#[cfg(test)]
pub fn delete_word(text: &mut String) {
    let mut input = Input::from(text.as_str());
    input.edit(&KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    *text = input.to_string();
}

#[cfg(test)]
pub(crate) fn test_app() -> (App, std::sync::mpsc::Receiver<Cmd>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (App::new(tx, true, Images::disabled()), rx)
}

/// The rows of a song list as shown, to play: their URIs and the selected one's place. A list
/// longer than one play request takes starts at the selected song.
fn shown_play(l: &Listing<Track>) -> (Vec<String>, usize) {
    const MAX: usize = 500;
    let uris: Vec<String> = l.rows().map(|(_, t)| t.uri.clone()).collect();
    let sel = l.selected().min(uris.len().saturating_sub(1));
    if uris.len() <= MAX {
        (uris, sel)
    } else {
        (uris.into_iter().skip(sel).take(MAX).collect(), 0)
    }
}

/// "“Teardrop”", or "3 songs".
fn describe(songs: &[Track]) -> String {
    match songs {
        [one] => format!("“{}”", one.name),
        many => format!("{} songs", many.len()),
    }
}

