use crate::api::{Page, Play, Playback, Playlist, Track};
use crate::worker::{Cmd, List, Msg};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// How often to ask Spotify for the player state. Progress is interpolated between polls.
const POLL_EVERY: Duration = Duration::from_secs(5);
const VOLUME_STEP: u8 = 5;
const SEEK_STEP_MS: u64 = 10_000;
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
    tx: Sender<Cmd>,
    in_flight: usize,
    poll_pending: bool,
    next_poll: Instant,
}

impl App {
    pub fn new(tx: Sender<Cmd>, can_like: bool) -> Self {
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
            tx,
            in_flight: 0,
            poll_pending: true,
            next_poll: now + POLL_EVERY,
        };
        app.send(Cmd::Init);
        app
    }

    fn send(&mut self, cmd: Cmd) {
        if self.tx.send(cmd).is_ok() {
            self.in_flight += 1;
        }
    }

    /// True while the API thread has work, so the UI should check for replies often.
    pub fn busy(&self) -> bool {
        self.in_flight > 0
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
        self.playback.as_ref()?.device.as_ref()?.volume_percent
    }

    /// Called on every loop pass: polls the player and loads pages that are needed.
    pub fn tick(&mut self) {
        let now = Instant::now();
        if self.toast.as_ref().is_some_and(|t| now >= t.until) {
            self.toast = None;
        }
        // A track ended: ask for the new state now instead of at the next poll.
        let ended = self.playback.as_ref().is_some_and(|pb| {
            pb.is_playing && pb.item.as_ref().is_some_and(|t| self.progress_ms() >= t.duration_ms)
        });
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
            Msg::Done { poll } => {
                self.in_flight = self.in_flight.saturating_sub(1);
                if poll {
                    self.poll_pending = false;
                }
            }
            Msg::User(user) => self.user_id = Some(user.id),
            Msg::Device(name) => {
                self.device_checked = true;
                self.device = name;
            }
            Msg::Playback(pb) => {
                let new_uri = pb.as_ref().and_then(|p| p.item.as_ref()).map(|t| t.uri.clone());
                self.playback = pb;
                self.polled_at = Instant::now();
                self.poll_pending = false;
                self.next_poll = Instant::now() + POLL_EVERY;
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
            Msg::Playlists { offset, page } => self.playlists.apply(offset, page),
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
                }
            }
            View::Liked => {
                if let Some(t) = self.liked.current() {
                    let from = t.uri.clone();
                    self.send(Cmd::PlayLiked { from });
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
                }
            }
            View::Playlists => {
                let Some(pl) = self.playlists.current().cloned() else { return };
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
    }

    fn play_whole_playlist(&mut self) {
        let pl = match self.view() {
            View::Playlists => self.playlists.current().cloned(),
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
        // Show the jump at once; the next poll corrects it.
        if let Some(pb) = self.playback.as_mut() {
            pb.progress_ms = Some(target);
        }
        self.polled_at = Instant::now();
        self.send(Cmd::Seek(target));
    }

    fn change_volume(&mut self, up: bool) {
        let current = self.volume().unwrap_or(50);
        let new = if up {
            current.saturating_add(VOLUME_STEP).min(100)
        } else {
            current.saturating_sub(VOLUME_STEP)
        };
        if let Some(d) = self.playback.as_mut().and_then(|pb| pb.device.as_mut()) {
            d.volume_percent = Some(new);
        }
        self.send(Cmd::Volume(new));
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
            View::Playlists => self.playlists.reset(),
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
        if self.view() == View::Playlists {
            self.playlists.move_by(delta);
        } else if let Some(l) = self.tracks_mut() {
            l.move_by(delta);
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
            KeyCode::Char(' ') => self.send(Cmd::Toggle),
            KeyCode::Char('n') => self.send(Cmd::Next),
            KeyCode::Char('p') => self.send(Cmd::Prev),
            KeyCode::Right => self.seek(true),
            KeyCode::Left => self.seek(false),
            KeyCode::Char('+') | KeyCode::Char('=') => self.change_volume(true),
            KeyCode::Char('-') => self.change_volume(false),
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
    (App::new(tx, true), rx)
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
