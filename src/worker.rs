//! Background thread that owns the API client, so the UI never waits on the network.

use crate::api::{Album, ApiError, Artist, Found, Page, Play, Playback, Playlist, Spotify, Track, User};
use crate::radio::{Library, RadioTrack};
use anyhow::{anyhow, Result};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::collections::{HashMap, VecDeque};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// A list the UI shows, used to route pages back to the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum List {
    Search(String),
    /// Search results of another kind: "artist", "album" or "playlist".
    Found { query: String, kind: &'static str },
    Playlists,
    Liked,
    Recent,
    Playlist(String),
    Album(String),
}

#[derive(Debug)]
pub enum Cmd {
    /// Fetch the user, find the device, read the player state.
    Init,
    /// Look for our device again (spotifyd may have started since).
    FindDevice,
    Poll,
    Load { list: List, offset: usize },
    Play(Play),
    /// Play Liked Songs as a context, starting at a track.
    PlayLiked { from: String },
    /// Play from a position (turning true shuffle off goes back to the list there).
    PlayAt(Play, u64),
    /// True shuffle: every song of a playlist, album or Liked Songs (`…:collection`) in a
    /// random order (`from` first), played as a list with Spotify's own shuffle off.
    PlayShuffled { context: String, from: Option<String>, position_ms: u64 },
    Toggle,
    Next,
    Prev,
    Seek(u64),
    Volume(u8),
    Shuffle(bool),
    Repeat(&'static str),
    Queue { uri: String, name: String },
    CheckSaved(String),
    SetSaved { uri: String, saved: bool },
    /// What plays next (for the stage column's "Up next").
    UpNext,
    /// Starts what played at the last quit: the song at its position, shuffle, repeat, volume.
    Resume(Box<crate::state::SavedPlayback>),
    /// Likes a song, or unlikes it if it is liked (asks Spotify which first).
    ToggleSaved(String),
    /// A new private playlist with these songs.
    CreatePlaylist { name: String, description: String, uris: Vec<String> },
    /// Saves a share card picture of the song (on its own thread).
    Card(Box<Track>),
    /// Profile, popular songs and albums of an artist (by id).
    Artist(String),
    /// Album details (by id); its tracks load through `Load { list: List::Album }`.
    Album(String),
    /// Song radio for a track URI (runs on its own thread: it takes about 2 s).
    Radio(String),
    /// Discover mode: the radios of `seeds` together, for the radio of `radio`.
    RadioMore { radio: String, seeds: Vec<String> },
    /// Every Liked Songs track and artist, for the radio filters.
    Library,
    /// Adds songs to a playlist: at the end, or at `position` (undo puts a song back there).
    AddToPlaylist { playlist_id: String, playlist: String, uris: Vec<String>, what: String, position: Option<usize> },
    /// Plays a genre's `nth` most popular song (0: the top one), from 20% in.
    Sample { genre: String, nth: usize },
    /// Ends a sample: plays what played before at its place (or pauses).
    Unsample { restore: Option<Restore> },
    /// 50 random songs of a genre.
    Randomize(String),
    /// Reload: the saved copy with this name (see `kept`) is not used until a new one is saved.
    Refresh(String),
    RemoveFromPlaylist { playlist_id: String, playlist: String, uris: Vec<String>, what: String },
    /// Likes or unlikes many songs (marked ones, or an undo).
    SetSavedMany { uris: Vec<String>, saved: bool },
    /// The Spotify Connect devices, for the device menu.
    Devices,
    /// Moves playback to a device.
    Transfer { id: String, name: String },
}

#[derive(Debug)]
pub enum Msg {
    User(User),
    /// Name of the device spotlite will play on, if Spotify sees it.
    Device(Option<String>),
    Playback(Option<Box<Playback>>),
    Tracks { list: List, offset: usize, page: Page<Track> },
    Found { list: List, offset: usize, page: Page<Found> },
    Playlists { offset: usize, page: Page<Playlist> },
    LoadFailed { list: List, forbidden: bool },
    /// Spotify limits `list` for `secs`: the UI shows what it has (maybe a saved copy) and
    /// asks again then.
    RetryLater { list: List, secs: u64 },
    Saved { uri: String, saved: bool },
    Info(String),
    Error(String),
    /// Spotify asked us to wait this many seconds.
    Backoff(u64),
    /// The user's playlist folders (from the folder thread).
    Folders(Vec<crate::folders::Entry>),
    Artist { artist: Artist, top: Vec<Track>, albums: Vec<Album> },
    Album(Album),
    Radio { seed: String, result: Result<(RadioTrack, Vec<RadioTrack>), String> },
    RadioMore { radio: String, result: Result<Vec<RadioTrack>, String> },
    Library(Library),
    /// Loading a page (artist id, album id or radio seed) failed; it shows the error.
    PageFailed { id: String, error: String },
    /// A track was removed from a playlist.
    Removed { playlist_id: String, uris: Vec<String>, text: String },
    /// The Spotify Connect devices: (id, name, active).
    Devices(Vec<(String, String, bool)>),
    /// A track was added to a playlist.
    Added { playlist_id: String, text: String },
    /// The songs that play next, in order (up to 20).
    UpNext(Vec<Track>),
    /// Every Liked Song, newest first (from the saved copy, brought up to date).
    Liked(Vec<Track>),
    /// A true shuffle started: these songs play for `context`.
    Shuffled { context: String, uris: Vec<String> },
    Random { genre: String, result: Result<Vec<Track>, String> },
    /// A sample plays (or could not start).
    Sampling { genre: String, result: Result<Track, String> },
    /// A playlist was created.
    Created(Box<Playlist>),
    /// The player state could not be read: no connection (the count of failures in a row).
    Offline(u32),
    /// The connection is back.
    Online,
    /// Sent after every command.
    Done(Kind),
    /// A player command failed: what was shown for it is not true.
    PlayerFailed,
    /// spotifyd's own player changed (MPRIS): it plays or not, and the song, if one is loaded.
    Mpris { playing: bool, track: Option<Box<Track>> },
    /// A key, a click, a paste or a resize from the terminal (the input thread). Every kind of
    /// event comes through this one channel, so the UI wakes at once for any of them.
    Term(ratatui::crossterm::event::Event),
}

/// Which kind of command finished, for the UI's in-flight bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Poll,
    Volume,
    /// A player command (the UI then asks for fresh state soon).
    Player,
    Other,
}

/// What the two API threads share: the device to play on and the user.
#[derive(Default)]
struct Shared {
    device: Option<String>,
    user: Option<User>,
}

/// The thread a command runs on. Player commands and polls have their own, so play, pause and
/// skip never wait behind a long load (a randomize can take 40 requests).
fn control_lane(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::Init
            | Cmd::FindDevice
            | Cmd::Poll
            | Cmd::Play(_)
            | Cmd::PlayLiked { .. }
            | Cmd::PlayAt(..)
            | Cmd::Toggle
            | Cmd::Next
            | Cmd::Prev
            | Cmd::Seek(_)
            | Cmd::Volume(_)
            | Cmd::Shuffle(_)
            | Cmd::Repeat(_)
            | Cmd::Queue { .. }
            | Cmd::CheckSaved(_)
            | Cmd::SetSaved { .. }
            | Cmd::SetSavedMany { .. }
            | Cmd::Devices
            | Cmd::Transfer { .. }
            | Cmd::ToggleSaved(_)
            | Cmd::UpNext
            | Cmd::Resume(_)
            | Cmd::Unsample { .. }
    )
}

/// Starts the API threads. Returns their command sender, a sender other threads can use to
/// post messages to the UI, and the UI's message receiver.
pub fn spawn(api: Spotify) -> (Sender<Cmd>, Sender<Msg>, Receiver<Msg>) {
    let (cmd_tx, cmd_rx) = channel::<Cmd>();
    let (msg_tx, msg_rx) = channel();
    let shared = Arc::new(Mutex::new(Shared::default()));
    let (control_tx, control_rx) = channel();
    let (data_tx, data_rx) = channel();
    for (name, rx) in [("spotlite-api", control_rx), ("spotlite-data", data_rx)] {
        let worker = Worker::new(api.clone(), msg_tx.clone(), shared.clone());
        thread::Builder::new()
            .name(name.into())
            .spawn(move || worker.run(rx))
            .expect("cannot start an API thread");
    }
    thread::Builder::new()
        .name("spotlite-route".into())
        .spawn(move || {
            for cmd in cmd_rx {
                let lane = if control_lane(&cmd) { &control_tx } else { &data_tx };
                if lane.send(cmd).is_err() {
                    return;
                }
            }
        })
        .expect("cannot start the API thread");
    // The saved copies stay small: the oldest go first.
    crate::store::prune("lists", 24 << 20);
    crate::store::prune("albums", 16 << 20);
    crate::store::prune("artists", 8 << 20);
    (cmd_tx, msg_tx, msg_rx)
}

struct Worker {
    api: Spotify,
    tx: Sender<Msg>,
    shared: Arc<Mutex<Shared>>,
    last: Option<Playback>,
    /// Polls in a row that did not reach Spotify (the poll waits longer after each).
    offline: u32,
    song_searches: SearchCache<Track>,
    found_searches: SearchCache<Found>,
    /// When /me was last asked without an answer (it is asked again at most every 10 min).
    me_tried: Option<Instant>,
    /// Genres sampled lately, and their songs, most popular first (Tab again is instant).
    samples: Vec<(String, Vec<Track>)>,
    /// Prepicked sample songs found so far, by their search (samples.rs); kept on disk.
    picked: HashMap<String, Track>,
    /// The newest snapshot id seen for each playlist (by id).
    snapshots: HashMap<String, Option<String>>,
    /// Saved copies asked to load again (R, or after a change), and when: a copy saved before
    /// that time is not used.
    stale: HashMap<String, SystemTime>,
    /// Lists that load page by page, until they are whole and can be saved: the entries read
    /// so far (null ones included) and the songs.
    partial: HashMap<String, (usize, Vec<Track>)>,
    /// What earlier randomize rolls found.
    memo: crate::genre::Memo,
}

/// Names of the copies kept on disk (`store`), so a second look at a list costs no request.
pub mod kept {
    /// The pages of the playlists list ("lists/playlists-0.json", ...).
    pub const PLAYLISTS: &str = "lists/playlists";

    pub fn playlist(id: &str) -> String {
        format!("lists/pl-{id}.json")
    }

    pub fn album(id: &str) -> String {
        format!("albums/{id}.json")
    }

    pub fn album_songs(id: &str) -> String {
        format!("albums/{id}-songs.json")
    }

    pub fn artist(id: &str) -> String {
        format!("artists/{id}.json")
    }
}

/// A playlist's songs as saved, with the snapshot they belong to.
#[derive(serde::Serialize, serde::Deserialize)]
struct KeptList {
    snapshot: Option<String>,
    tracks: Vec<Track>,
}

/// The Recent tab shows at most this many songs (Spotify's 50 and spotlite's own, merged).
const RECENT_MAX: usize = 100;
/// The longest spotifyd takes to log in and show up as a device after its start.
const SPOTIFYD_LOGIN: Duration = Duration::from_secs(40);
/// How long a copy without a snapshot to compare (a playlist, the playlists list) is used.
const LIST_AGE: Duration = Duration::from_secs(600);
/// How long an artist page is used.
const ARTIST_AGE: Duration = Duration::from_secs(24 * 3600);

/// The part of a whole list from `offset` on, as the last page of it.
fn rest_page(tracks: Vec<Track>, offset: usize) -> Page<Track> {
    let total = tracks.len();
    let items: Vec<Track> = tracks.into_iter().skip(offset).collect();
    Page { raw: items.len(), items, total }
}

/// True when a saved playlist is the one Spotify has now: the same snapshot if both are
/// known, else young enough.
fn playlist_current(kept: Option<&str>, known: Option<&str>, age: Duration) -> bool {
    match (kept, known) {
        (Some(a), Some(b)) => a == b,
        _ => age <= LIST_AGE,
    }
}

/// Pages of recent searches. Search as you type asks again for a text you had a moment
/// ago (Backspace); this answers it without a request.
struct SearchCache<T> {
    pages: VecDeque<(String, Instant, Page<T>)>,
}

impl<T> Default for SearchCache<T> {
    fn default() -> Self {
        SearchCache { pages: VecDeque::new() }
    }
}

impl<T: Clone> SearchCache<T> {
    const KEEP: usize = 32;
    const MAX_AGE: Duration = Duration::from_secs(120);

    /// The page of `key` at `offset`, from the cache if it is recent, else from `fetch`.
    fn get(&mut self, key: &str, offset: usize, fetch: impl FnOnce() -> Result<Page<T>>) -> Result<Page<T>> {
        let key = format!("{key}\n{offset}");
        self.pages.retain(|(_, at, _)| at.elapsed() < Self::MAX_AGE);
        if let Some((_, _, page)) = self.pages.iter().find(|(k, _, _)| *k == key) {
            return Ok(page.clone());
        }
        let page = fetch()?;
        self.pages.push_back((key, Instant::now(), page.clone()));
        if self.pages.len() > Self::KEEP {
            self.pages.pop_front();
        }
        Ok(page)
    }
}

impl Worker {
    fn new(api: Spotify, tx: Sender<Msg>, shared: Arc<Mutex<Shared>>) -> Self {
        Worker {
            api,
            tx,
            shared,
            last: None,
            offline: 0,
            song_searches: SearchCache::default(),
            found_searches: SearchCache::default(),
            me_tried: None,
            samples: Vec::new(),
            picked: crate::store::read("picked.json").unwrap_or_default(),
            snapshots: HashMap::new(),
            stale: HashMap::new(),
            partial: HashMap::new(),
            memo: crate::genre::Memo { depths: crate::store::read("depths.json").unwrap_or_default(), ..Default::default() },
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn device(&self) -> Option<String> {
        self.shared().device.clone()
    }

    fn user(&self) -> Option<User> {
        self.shared().user.clone()
    }

    fn set_user(&self, user: User) {
        self.shared().user = Some(user.clone());
        self.send(Msg::User(user));
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        let mut waiting: VecDeque<Cmd> = VecDeque::new();
        loop {
            if waiting.is_empty() {
                match rx.recv() {
                    Ok(cmd) => waiting.push_back(cmd),
                    Err(_) => return,
                }
            }
            waiting.extend(rx.try_iter());
            let Some(cmd) = waiting.pop_front() else { continue };
            // A search typed further makes the load of the older text useless: skip it.
            if is_search_start(&cmd) && waiting.iter().any(is_search_start) {
                if let Cmd::Load { list, .. } = cmd {
                    self.send(Msg::LoadFailed { list, forbidden: false });
                }
                if self.tx.send(Msg::Done(Kind::Other)).is_err() {
                    return;
                }
                continue;
            }
            let kind = match cmd {
                // Init ends with the first player state, like a poll.
                Cmd::Poll | Cmd::Init => Kind::Poll,
                Cmd::Volume(_) => Kind::Volume,
                Cmd::Sample { .. } | Cmd::Unsample { .. } | Cmd::Resume(_) | Cmd::Play(_) | Cmd::PlayAt(..) | Cmd::PlayShuffled { .. } | Cmd::PlayLiked { .. } | Cmd::Toggle | Cmd::Next | Cmd::Prev | Cmd::Seek(_) => Kind::Player,
                _ => Kind::Other,
            };
            let list = match &cmd {
                Cmd::Load { list, .. } => Some(list.clone()),
                _ => None,
            };
            let quiet = matches!(cmd, Cmd::CheckSaved(_) | Cmd::UpNext);
            if let Err(err) = self.handle(cmd) {
                if kind == Kind::Poll && matches!(err.downcast_ref::<ApiError>(), Some(ApiError::Offline)) {
                    self.offline += 1;
                    self.send(Msg::Offline(self.offline));
                }
                self.report(err, list, kind, quiet);
                if kind == Kind::Player {
                    self.send(Msg::PlayerFailed);
                }
            }
            if self.tx.send(Msg::Done(kind)).is_err() {
                return; // UI has quit
            }
        }
    }

    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    fn report(&self, err: anyhow::Error, list: Option<List>, kind: Kind, quiet: bool) {
        let api_err = err.downcast_ref::<ApiError>();
        // A check the user did not ask for (the ♥ of the playing song, what plays next) waits
        // for the limit to end without a message.
        if quiet && matches!(api_err, Some(ApiError::RateLimited(_) | ApiError::Offline)) {
            return;
        }
        // No network: the poll says so once and waits longer each time; no message each time.
        if let (Some(ApiError::Offline), Kind::Poll) = (api_err, kind) {
            return;
        }
        // Spotify limits each kind of request on its own: only a limit on the player state
        // itself delays the next poll (a limit on /me must not stop the player for hours).
        if let (Some(ApiError::RateLimited(secs)), Kind::Poll) = (api_err, kind) {
            self.send(Msg::Backoff(*secs));
        }
        if let (Some(list), Some(ApiError::RateLimited(secs))) = (&list, api_err) {
            self.send(Msg::LoadFailed { list: list.clone(), forbidden: false });
            self.send(Msg::RetryLater { list: list.clone(), secs: *secs });
            return; // not an error: the UI says it once, quietly
        }
        if let Some(list) = list {
            let forbidden = matches!(api_err, Some(ApiError::Forbidden));
            self.send(Msg::LoadFailed { list, forbidden });
            if forbidden {
                return; // the UI explains this case itself
            }
        }
        self.send(Msg::Error(format!("{err:#}")));
    }

    fn handle(&mut self, cmd: Cmd) -> Result<()> {
        match cmd {
            Cmd::Init => {
                // The device and the player first: they do not need the user, and /me can be
                // refused for hours. A profile error is reported after them.
                let user = self.api.me_cached();
                match &user {
                    Ok(user) => self.set_user(user.clone()),
                    Err(_) => self.me_tried = Some(Instant::now()),
                }
                self.find_device()?;
                self.poll()?;
                match user {
                    Ok(_) => Ok(()),
                    // Not an error to fix: playback works, and /me is asked again later.
                    Err(e) => match e.downcast_ref::<ApiError>() {
                        Some(ApiError::RateLimited(secs)) => {
                            self.send(Msg::Info(format!(
                                "Spotify limits the profile request for {} min. Playback works; spotlite asks again every 10 min.",
                                secs.div_ceil(60)
                            )));
                            Ok(())
                        }
                        _ => Err(e),
                    },
                }
            }
            Cmd::Poll => self.poll(),
            Cmd::FindDevice => self.find_device().map(|_| ()),
            Cmd::Load { list, offset } => self.load(list, offset),
            Cmd::Play(play) => self.player(|api, dev| api.play(dev, Some(&play))),
            Cmd::PlayLiked { from } => {
                let user = match self.user() {
                    Some(u) => u,
                    None => self.api.me_cached()?,
                };
                let play = Play::Context {
                    uri: format!("{}:collection", user.uri),
                    from: Some(from),
                };
                self.player(|api, dev| api.play(dev, Some(&play)))
            }
            Cmd::PlayAt(play, ms) => self.player(|api, dev| api.play_at(dev, Some(&play), ms)),
            Cmd::PlayShuffled { context, from, position_ms } => self.play_shuffled(context, from, position_ms),
            Cmd::Toggle => {
                let playing = self.last.as_ref().is_some_and(|pb| pb.is_playing);
                let idle = self.last.as_ref().and_then(|pb| pb.device.as_ref()).is_none();
                if playing {
                    self.player(|api, dev| api.pause(dev))
                } else if idle {
                    // Nothing is active: move playback to our device and resume there.
                    let dev = self.require_device()?;
                    self.api.transfer(&dev, true)
                } else {
                    self.player(|api, dev| api.play(dev, None))
                }
            }
            Cmd::Next => self.player(|api, dev| api.next(dev)),
            Cmd::Prev => self.player(|api, dev| api.previous(dev)),
            Cmd::Seek(ms) => self.player(|api, dev| api.seek(dev, ms)),
            Cmd::Volume(pct) => self.player(|api, dev| api.set_volume(dev, pct)),
            Cmd::Shuffle(on) => self.player(|api, dev| api.set_shuffle(dev, on)),
            Cmd::Repeat(state) => self.player(|api, dev| api.set_repeat(dev, state)),
            Cmd::Resume(sp) if sp.shuffle && sp.context.as_deref().is_some_and(shufflable) => {
                let context = sp.context.clone().unwrap_or_default();
                self.play_shuffled(context, Some(sp.track.clone()), sp.position_ms)?;
                let _ = self.player(|api, dev| api.set_repeat(dev, &sp.repeat));
                if let Some(v) = sp.volume {
                    let _ = self.player(|api, dev| api.set_volume(dev, v));
                }
                if !sp.playing {
                    let _ = self.player(|api, dev| api.pause(dev));
                }
                self.send(Msg::Info(format!("{} where you left off: {}", if sp.playing { "Playing" } else { "Ready" }, sp.name)));
                Ok(())
            }
            Cmd::Resume(sp) => {
                let play = match &sp.context {
                    Some(uri) => Play::Context { uri: uri.clone(), from: Some(sp.track.clone()) },
                    None => Play::Tracks { uris: vec![sp.track.clone()], offset: 0 },
                };
                self.player(|api, dev| api.play_at(dev, Some(&play), sp.position_ms))?;
                // Shuffle, repeat and volume after play: they need an active device. Spotify's
                // own shuffle stays off: spotlite shuffles itself.
                let _ = self.player(|api, dev| api.set_shuffle(dev, false));
                let _ = self.player(|api, dev| api.set_repeat(dev, &sp.repeat));
                if let Some(v) = sp.volume {
                    let _ = self.player(|api, dev| api.set_volume(dev, v));
                }
                if !sp.playing {
                    let _ = self.player(|api, dev| api.pause(dev));
                }
                let how = if sp.playing { "Playing" } else { "Ready" };
                self.send(Msg::Info(format!("{how} where you left off: {}", sp.name)));
                Ok(())
            }
            Cmd::UpNext => {
                let next = self.api.up_next()?;
                self.send(Msg::UpNext(next));
                Ok(())
            }
            Cmd::Queue { uri, name } => {
                self.player(|api, dev| api.queue(dev, &uri))?;
                self.send(Msg::Info(format!("Added to queue: {name}")));
                if let Ok(next) = self.api.up_next() {
                    self.send(Msg::UpNext(next));
                }
                Ok(())
            }
            Cmd::CheckSaved(uri) => {
                let saved = self.api.is_saved(&uri)?;
                self.send(Msg::Saved { uri, saved });
                Ok(())
            }
            Cmd::ToggleSaved(uri) => {
                let saved = !self.api.is_saved(&uri)?;
                self.api.set_saved(&uri, saved)?;
                self.send(Msg::Saved { uri, saved });
                self.send(Msg::Info(if saved { "Added to Liked Songs" } else { "Removed from Liked Songs" }.into()));
                Ok(())
            }
            Cmd::CreatePlaylist { name, description, uris } => {
                let pl = self.api.create_playlist(&name, &description)?;
                for chunk in uris.chunks(100) {
                    self.api.add_to_playlist(&pl.id, chunk, None)?;
                }
                self.stale.insert(kept::PLAYLISTS.to_owned(), SystemTime::now());
                self.send(Msg::Created(Box::new(pl)));
                self.send(Msg::Info(format!("Saved {} songs to the new playlist “{name}”", uris.len())));
                Ok(())
            }
            Cmd::SetSavedMany { uris, saved } => {
                // Spotify takes up to 40 songs in one request.
                for chunk in uris.chunks(40) {
                    self.api.set_saved(&chunk.join(","), saved)?;
                    for uri in chunk {
                        self.send(Msg::Saved { uri: uri.clone(), saved });
                    }
                }
                let n = uris.len();
                let what = if n == 1 { "1 song".to_owned() } else { format!("{n} songs") };
                self.send(Msg::Info(if saved { format!("Liked {what}") } else { format!("Unliked {what}") }));
                Ok(())
            }
            Cmd::Devices => {
                let devices = self.api.devices()?;
                let list = devices.into_iter().filter_map(|d| Some((d.id?, d.name, d.is_active))).collect();
                self.send(Msg::Devices(list));
                Ok(())
            }
            Cmd::Transfer { id, name } => {
                let playing = self.last.as_ref().is_some_and(|pb| pb.is_playing);
                self.api.transfer(&id, playing)?;
                self.shared().device = Some(id);
                self.send(Msg::Device(Some(name.clone())));
                self.send(Msg::Info(format!("Playing on {name}")));
                Ok(())
            }
            Cmd::SetSaved { uri, saved } => {
                self.api.set_saved(&uri, saved)?;
                self.send(Msg::Saved { uri, saved });
                self.send(Msg::Info(if saved { "Added to Liked Songs" } else { "Removed from Liked Songs" }.into()));
                Ok(())
            }
            Cmd::Card(track) => {
                let tx = self.tx.clone();
                thread::Builder::new().name("spotlite-card".into()).spawn(move || {
                    let msg = match crate::card::make(&track) {
                        Ok(path) => Msg::Info(format!("Saved the card to {} (path copied)", path.display())),
                        Err(e) => Msg::Error(format!("Share card: {e:#}")),
                    };
                    let _ = tx.send(msg);
                })?;
                Ok(())
            }
            Cmd::Artist(id) => {
                let name = kept::artist(&id);
                if self.current(&name, &name, Some(ARTIST_AGE)) {
                    if let Some((artist, top, albums)) = crate::store::read(&name) {
                        self.send(Msg::Artist { artist, top, albums });
                        return Ok(());
                    }
                }
                let result = (|| -> Result<()> {
                    let artist = self.api.artist(&id)?;
                    let top = self.api.artist_top_tracks(&artist)?;
                    let albums = self.api.artist_albums(&id)?;
                    crate::store::write(&name, &(&artist, &top, &albums));
                    self.send(Msg::Artist { artist, top, albums });
                    Ok(())
                })();
                if let Err(e) = &result {
                    self.send(Msg::PageFailed { id, error: format!("{e:#}") });
                }
                result
            }
            Cmd::Album(id) => {
                // An album does not change: its copy is used until a reload.
                let name = kept::album(&id);
                let album = match self.current(&name, &name, None).then(|| crate::store::read(&name)).flatten() {
                    Some(album) => album,
                    None => {
                        let album = self.api.album(&id)?;
                        crate::store::write(&name, &album);
                        album
                    }
                };
                self.send(Msg::Album(album));
                Ok(())
            }
            Cmd::Refresh(name) => {
                self.stale.insert(name, SystemTime::now());
                Ok(())
            }
            Cmd::Radio(seed) => {
                let tx = self.tx.clone();
                thread::Builder::new().name("spotlite-radio".into()).spawn(move || {
                    let result = crate::radio::fetch(&seed).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Msg::Radio { seed, result });
                })?;
                Ok(())
            }
            Cmd::RadioMore { radio, seeds } => {
                let tx = self.tx.clone();
                thread::Builder::new().name("spotlite-radio".into()).spawn(move || {
                    let result = crate::radio::fetch_more(&seeds).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Msg::RadioMore { radio, result });
                })?;
                Ok(())
            }
            Cmd::Library => {
                let songs: Vec<Track> = crate::store::read("liked.json").unwrap_or_default();
                let (library, songs) = match sync_library(&mut self.api, crate::store::read("library.json"), songs) {
                    Ok(synced) => synced,
                    // The Liked Songs tab says so already; the ♥ marks wait for the next start.
                    Err(e) if matches!(e.downcast_ref::<ApiError>(), Some(ApiError::RateLimited(_))) => return Ok(()),
                    Err(e) => return Err(e),
                };
                crate::store::write("library.json", &library);
                if songs.len() == library.liked_order.len() {
                    crate::store::write("liked.json", &songs);
                    self.send(Msg::Liked(songs));
                }
                self.send(Msg::Library(library));
                Ok(())
            }
            Cmd::Sample { genre, nth } => {
                let result = self.sample(&genre, nth);
                let msg = match result {
                    Ok(track) => Msg::Sampling { genre: genre.clone(), result: Ok(track) },
                    Err(e) => Msg::Sampling { genre: genre.clone(), result: Err(format!("{e:#}")) },
                };
                self.send(msg);
                // Find the next prepicked song now, so Tab again plays it at once.
                let _ = self.prepicked(&genre, nth + 1);
                Ok(())
            }
            Cmd::Unsample { restore } => match restore {
                Some(r) => {
                    let play = match r.context {
                        Some(uri) => Play::Context { uri, from: Some(r.track) },
                        None => Play::Tracks { uris: vec![r.track], offset: 0 },
                    };
                    self.player(|api, dev| api.play_at(dev, Some(&play), r.position_ms))?;
                    if !r.playing {
                        self.player(|api, dev| api.pause(dev))?;
                    }
                    Ok(())
                }
                None => self.player(|api, dev| api.pause(dev)),
            },
            Cmd::Randomize(genre) => {
                let term = crate::genres::term(&genre);
                let depths = self.memo.depths.len();
                let result = crate::genre::random(&mut self.api, &term, &mut self.memo).map_err(|e| format!("{e:#}"));
                if self.memo.depths.len() != depths {
                    crate::store::write("depths.json", &self.memo.depths);
                }
                self.send(Msg::Random { genre, result });
                Ok(())
            }
            Cmd::AddToPlaylist { playlist_id, playlist, uris, what, position } => {
                for chunk in uris.chunks(100) {
                    self.api.add_to_playlist(&playlist_id, chunk, position)?;
                }
                self.changed(&playlist_id);
                self.send(Msg::Added { playlist_id, text: format!("Added {what} to {playlist}") });
                Ok(())
            }
            Cmd::RemoveFromPlaylist { playlist_id, playlist, uris, what } => {
                for chunk in uris.chunks(100) {
                    self.api.remove_from_playlist(&playlist_id, chunk)?;
                }
                self.changed(&playlist_id);
                self.send(Msg::Removed { playlist_id, uris, text: format!("Removed {what} from {playlist}") });
                Ok(())
            }
        }
    }

    /// A genre's songs, most popular first, one per artist (Tab again: the next artist). Search
    /// does not rank by popularity, so 30 results are ranked by Spotify's own popularity number.
    fn top_of_genre(&mut self, genre: &str) -> Result<Vec<Track>> {
        let term = crate::genres::term(genre).replace('"', "");
        let mut found: Vec<Track> = Vec::new();
        for offset in [0, 10, 20] {
            let page = self.api.search_tracks(&format!("genre:\"{term}\""), offset)?;
            let done = page.raw == 0;
            found.extend(page.items);
            if done {
                break;
            }
        }
        if found.len() < 10 {
            found.extend(self.api.search_tracks(&term, 0)?.items);
        }
        let uris: Vec<String> = found.iter().map(|t| t.uri.clone()).collect();
        if let Ok(pop) = crate::radio::popularity(&uris) {
            // Stable: equal popularity keeps Spotify's order.
            found.sort_by_key(|t| std::cmp::Reverse(pop.get(&t.uri).copied().unwrap_or(0)));
        }
        let mut artists = std::collections::HashSet::new();
        found.retain(|t| artists.insert(t.artists.first().map(|a| a.name.clone()).unwrap_or_default()));
        Ok(found)
    }

    /// The genre's `nth` prepicked song (samples.rs), found with one search; None past the
    /// last one, or when Spotify does not find it.
    fn prepicked(&mut self, genre: &str, nth: usize) -> Option<Track> {
        let song = crate::samples::of(genre).into_iter().nth(nth)?;
        let query = song.query();
        if let Some(t) = self.picked.get(&query) {
            return Some(t.clone());
        }
        let items = self.api.search_tracks(&query, 0).ok()?.items;
        let artist = song.artist.to_lowercase();
        let by = |t: &&Track| t.artists.iter().any(|a| a.name.to_lowercase() == artist);
        let track = items.iter().find(by).or(items.first())?.clone();
        self.picked.insert(query, track.clone());
        // These songs are fixed: the next start finds them with no search.
        crate::store::write("picked.json", &self.picked);
        Some(track)
    }

    /// Plays the genre's `nth` sample from 20% in: the prepicked songs first, then the genre's
    /// most popular songs.
    fn sample(&mut self, genre: &str, nth: usize) -> Result<Track> {
        let known = crate::samples::of(genre).len();
        let track = match self.prepicked(genre, nth) {
            Some(t) => t,
            None => self.popular_sample(genre, nth.saturating_sub(known))?,
        };
        // 20% in: past the intro, and 10 s from the end at least.
        let position_ms = (track.duration_ms / 5).min(track.duration_ms.saturating_sub(12_000));
        let play = Play::Tracks { uris: vec![track.uri.clone()], offset: 0 };
        self.player(|api, dev| api.play_at(dev, Some(&play), position_ms))?;
        Ok(track)
    }

    /// The genre's `nth` most popular song (one per artist).
    fn popular_sample(&mut self, genre: &str, nth: usize) -> Result<Track> {
        let found = match self.samples.iter().find(|(g, _)| g == genre) {
            Some((_, tracks)) => tracks.clone(),
            None => {
                let tracks = self.top_of_genre(genre)?;
                self.samples.push((genre.to_owned(), tracks.clone()));
                if self.samples.len() > 20 {
                    self.samples.remove(0);
                }
                tracks
            }
        };
        found
            .get(nth % found.len().max(1))
            .cloned()
            .ok_or_else(|| anyhow!("Spotify has no songs in the genre “{genre}”."))
    }

    /// Every song of a context, in its order.
    fn context_uris(&mut self, context: &str) -> Result<Vec<String>> {
        if context.ends_with(":collection") {
            let library: Option<Library> = crate::store::read("library.json");
            if let Some(lib) = library.filter(|l| !l.liked_order.is_empty()) {
                return Ok(lib.liked_order);
            }
            let mut out = Vec::new();
            let mut offset = 0;
            loop {
                let page = self.api.liked_tracks(offset)?;
                offset += page.raw;
                out.extend(page.items.into_iter().map(|t| t.uri));
                if page.raw == 0 || offset >= page.total {
                    return Ok(out);
                }
            }
        }
        if context.starts_with("spotify:playlist:") {
            if let Ok(uris) = crate::radio::playlist_uris(context) {
                return Ok(uris);
            }
            let id = context.trim_start_matches("spotify:playlist:");
            return self.all_pages(|api, offset| api.playlist_tracks(id, offset));
        }
        let id = context.trim_start_matches("spotify:album:").to_owned();
        self.all_pages(|api, offset| api.album_tracks(&id, offset))
    }

    fn all_pages(&mut self, mut f: impl FnMut(&mut Spotify, usize) -> Result<Page<Track>>) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let page = f(&mut self.api, offset)?;
            offset += page.raw;
            out.extend(page.items.into_iter().map(|t| t.uri));
            if page.raw == 0 || offset >= page.total {
                return Ok(out);
            }
        }
    }

    fn play_shuffled(&mut self, context: String, from: Option<String>, position_ms: u64) -> Result<()> {
        let uris = match self.context_uris(&context) {
            Ok(uris) if !uris.is_empty() => uris,
            // Cannot list it: play it with Spotify's own shuffle.
            _ => {
                let _ = self.player(|api, dev| api.set_shuffle(dev, true));
                let play = Play::Context { uri: context, from };
                return self.player(|api, dev| api.play_at(dev, Some(&play), position_ms));
            }
        };
        let uris = shuffled(uris, from.as_deref(), &mut rand::thread_rng());
        let _ = self.player(|api, dev| api.set_shuffle(dev, false));
        // Only keep the position when the song it is for comes first.
        let position_ms = if from.as_deref() == uris.first().map(String::as_str) { position_ms } else { 0 };
        let play = Play::Tracks { uris: uris.clone(), offset: 0 };
        self.player(|api, dev| api.play_at(dev, Some(&play), position_ms))?;
        self.send(Msg::Shuffled { context, uris });
        Ok(())
    }

    fn poll(&mut self) -> Result<()> {
        // No user yet (/me was refused): ask again now and then, quietly.
        if self.user().is_none() && self.me_tried.is_some_and(|at| at.elapsed() >= Duration::from_secs(600)) {
            match self.api.me_cached() {
                Ok(user) => self.set_user(user),
                Err(_) => self.me_tried = Some(Instant::now()),
            }
        }
        let pb = self.api.playback()?;
        if std::mem::take(&mut self.offline) > 0 {
            self.send(Msg::Online);
        }
        self.last = pb.clone();
        self.send(Msg::Playback(pb.map(Box::new)));
        Ok(())
    }

    fn find_device(&mut self) -> Result<Option<String>> {
        let device = self.api.find_device()?;
        let id = device.as_ref().and_then(|d| d.id.clone());
        self.shared().device = id.clone();
        self.send(Msg::Device(device.map(|d| d.name)));
        Ok(id)
    }

    /// Our device. When Spotify does not see it, spotifyd is started (it may have been stopped
    /// with Q, or not be running yet) and looked for again for a few seconds.
    fn require_device(&mut self) -> Result<String> {
        if let Some(id) = self.device() {
            return Ok(id);
        }
        if let Some(id) = self.find_device()? {
            return Ok(id);
        }
        // spotifyd needs from a few seconds to about 30 s (a slow network) to log in and show
        // up: wait while it is that young, and not when it ran for long and is lost.
        if !cfg!(test) {
            let started = crate::engine::start();
            let age = if started { Some(Duration::ZERO) } else { crate::engine::age() };
            if let Some(age) = age.filter(|a| *a < SPOTIFYD_LOGIN) {
                let name = crate::engine::name();
                self.send(Msg::Info(if started { format!("Starting {name}…") } else { format!("Waiting for {name} to log in…") }));
                let until = Instant::now() + (SPOTIFYD_LOGIN - age);
                while Instant::now() < until {
                    thread::sleep(Duration::from_secs(1));
                    if let Some(id) = self.find_device()? {
                        return Ok(id);
                    }
                }
            }
        }
        Err(anyhow!(
            "Spotify does not see the device '{}'. {}",
            self.api.config().device_name,
            crate::engine::hint_check()
        ))
    }

    /// Runs a player command on our device. If Spotify says the device is gone
    /// (e.g. spotifyd restarted), looks it up again and retries once. The UI asks for the new
    /// state itself, so the thread is free for the next command at once.
    fn player(&mut self, mut f: impl FnMut(&mut Spotify, Option<&str>) -> Result<()>) -> Result<()> {
        let dev = self.require_device()?;
        match f(&mut self.api, Some(&dev)) {
            Err(e) if matches!(e.downcast_ref::<ApiError>(), Some(ApiError::NoDevice)) => {
                self.shared().device = None;
                let dev = self.require_device()?;
                f(&mut self.api, Some(&dev))?;
            }
            other => other?,
        }
        Ok(())
    }

    /// True when the copy saved as `name` can be used: it is there, it was saved after the
    /// last reload of `key`, and it is at most `max_age` old (None: any age).
    fn current(&self, name: &str, key: &str, max_age: Option<Duration>) -> bool {
        let Some(at) = crate::store::saved_at(name) else { return false };
        if self.stale.get(key).is_some_and(|since| at <= *since) {
            return false;
        }
        max_age.is_none_or(|max| at.elapsed().is_ok_and(|age| age <= max))
    }

    /// A playlist changed here: its copy and the playlists list load again when asked.
    fn changed(&mut self, playlist_id: &str) {
        let now = SystemTime::now();
        self.stale.insert(kept::playlist(playlist_id), now);
        self.stale.insert(kept::PLAYLISTS.to_owned(), now);
        self.snapshots.remove(playlist_id);
    }

    /// The saved songs of a playlist, if they are what Spotify has now.
    fn kept_playlist(&self, id: &str) -> Option<Vec<Track>> {
        let name = kept::playlist(id);
        if !self.current(&name, &name, None) {
            return None;
        }
        let age = crate::store::saved_at(&name)?.elapsed().unwrap_or_default();
        let kept: KeptList = crate::store::read(&name)?;
        let known = self.snapshots.get(id).and_then(|s| s.as_deref());
        playlist_current(kept.snapshot.as_deref(), known, age).then_some(kept.tracks)
    }

    /// Notes a page of a list that loads page by page. Returns every song once the list is
    /// whole (to save it); a page out of order drops what was noted.
    fn gather(&mut self, name: &str, offset: usize, page: &Page<Track>) -> Option<Vec<Track>> {
        if offset == 0 {
            self.partial.insert(name.to_owned(), (0, Vec::new()));
        }
        let (raw, songs) = self.partial.get_mut(name)?;
        if *raw != offset {
            self.partial.remove(name);
            return None;
        }
        songs.extend(page.items.iter().cloned());
        *raw += page.raw;
        if page.raw == 0 || *raw >= page.total {
            return self.partial.remove(name).map(|p| p.1);
        }
        None
    }

    /// A playlist's songs: the saved copy if it is current; else over the client protocol (no
    /// Web API request, and any playlist, not only your own), 100 songs at a time; else the
    /// Web API, page by page.
    fn load_playlist(&mut self, list: List, id: &str, offset: usize) -> Result<()> {
        if let Some(tracks) = self.kept_playlist(id) {
            self.send(Msg::Tracks { list, offset, page: rest_page(tracks, offset) });
            return Ok(());
        }
        let name = kept::playlist(id);
        let snapshot = self.snapshots.get(id).cloned().flatten();
        if offset == 0 && crate::client::AVAILABLE {
            let mut all: Vec<Track> = Vec::new();
            let mut sent = 0;
            let tx = self.tx.clone();
            let result = crate::radio::playlist_tracks(&format!("spotify:playlist:{id}"), |tracks, total| {
                let raw = total.saturating_sub(sent).min(crate::radio::CHUNK);
                all.extend(tracks.iter().cloned());
                let page = Page { items: tracks, raw, total };
                let alive = tx.send(Msg::Tracks { list: list.clone(), offset: sent, page }).is_ok();
                sent += raw;
                alive
            });
            match result {
                Ok(()) => {
                    crate::store::write(&name, &KeptList { snapshot, tracks: all });
                    return Ok(());
                }
                // Part of it shows: the rest loads from the Web API as the list is scrolled.
                Err(_) if sent > 0 => return Ok(()),
                Err(_) => {}
            }
        }
        let page = self.api.playlist_tracks(id, offset)?;
        if let Some(tracks) = self.gather(&name, offset, &page) {
            crate::store::write(&name, &KeptList { snapshot, tracks });
        }
        self.send(Msg::Tracks { list, offset, page });
        Ok(())
    }

    fn load(&mut self, list: List, offset: usize) -> Result<()> {
        match &list {
            List::Search(q) => {
                let api = &mut self.api;
                let page = self.song_searches.get(q, offset, || api.search_tracks(q, offset))?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Found { query, kind } => {
                let api = &mut self.api;
                let key = format!("{kind}\n{query}");
                let page = self.found_searches.get(&key, offset, || api.search(query, kind, offset))?;
                self.send(Msg::Found { list, offset, page });
            }
            List::Recent => {
                // One page: Spotify's list pages by time, not offset. spotifyd's songs are
                // not in it; spotlite's own list adds them.
                let items = if offset > 0 { Vec::new() } else { crate::history::merge(self.api.recently_played()?, crate::history::load(), RECENT_MAX) };
                let page = Page { raw: items.len(), total: offset + items.len(), items };
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Liked => {
                let page = saved_copy(&list, offset, self.api.liked_tracks(offset), &self.tx)?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Playlist(id) => {
                let id = id.clone();
                return self.load_playlist(list, &id, offset);
            }
            List::Album(id) => {
                let name = kept::album_songs(id);
                if self.current(&name, &kept::album(id), None) {
                    if let Some(tracks) = crate::store::read::<Vec<Track>>(&name) {
                        self.send(Msg::Tracks { list, offset, page: rest_page(tracks, offset) });
                        return Ok(());
                    }
                }
                let page = self.api.album_tracks(id, offset)?;
                if let Some(tracks) = self.gather(&name, offset, &page) {
                    crate::store::write(&name, &tracks);
                }
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Playlists => {
                // The list from the last 10 minutes is used as it is; R asks Spotify.
                let name = format!("{}-{offset}.json", kept::PLAYLISTS);
                let recent = self.current(&name, kept::PLAYLISTS, Some(LIST_AGE)).then(|| crate::store::read(&name)).flatten();
                let page: Page<Playlist> = match recent {
                    Some(page) => page,
                    None => saved_copy(&list, offset, self.api.playlists(offset), &self.tx)?,
                };
                for p in &page.items {
                    self.snapshots.insert(p.id.clone(), p.snapshot_id.clone());
                }
                self.send(Msg::Playlists { offset, page });
            }
        }
        Ok(())
    }
}

/// The load of a search's first page.
fn is_search_start(cmd: &Cmd) -> bool {
    matches!(cmd, Cmd::Load { list: List::Search(_) | List::Found { .. }, offset: 0 })
}

/// What played before a sample, to go back to.
#[derive(Debug, Clone)]
pub struct Restore {
    pub context: Option<String>,
    pub track: String,
    pub position_ms: u64,
    pub playing: bool,
}

/// Songs Spotify takes in one play request (a random sample of a longer list).
const SHUFFLE_MAX: usize = 500;

/// True shuffle can list these: a playlist, an album, Liked Songs.
pub fn shufflable(context: &str) -> bool {
    context.starts_with("spotify:playlist:") || context.starts_with("spotify:album:") || context.ends_with(":collection")
}

/// `uris` in a uniformly random order (Fisher-Yates), `first` at the front if it is there, cut
/// to what one play request takes. Each song comes once.
pub fn shuffled(uris: Vec<String>, first: Option<&str>, rng: &mut impl rand::Rng) -> Vec<String> {
    use rand::seq::SliceRandom;
    let mut seen = std::collections::HashSet::new();
    let mut uris: Vec<String> = uris.into_iter().filter(|u| seen.insert(u.clone())).collect();
    uris.shuffle(rng);
    if let Some(i) = first.and_then(|f| uris.iter().position(|u| u == f)) {
        let song = uris.remove(i);
        uris.insert(0, song);
    }
    uris.truncate(SHUFFLE_MAX);
    uris
}

/// Source of Liked Songs pages, so the sync can be tested without Spotify.
trait LikedPages {
    fn liked_page(&mut self, offset: usize) -> Result<Page<Track>>;
}

impl LikedPages for Spotify {
    fn liked_page(&mut self, offset: usize) -> Result<Page<Track>> {
        self.liked_tracks(offset)
    }
}

fn add_liked(library: &mut Library, t: &Track) {
    library.tracks.insert(t.uri.clone());
    library.artists.extend(t.artists.iter().filter_map(|a| a.uri.clone()));
}

/// Brings the Liked Songs index and the songs themselves up to date with as few requests as it
/// can. Liked Songs come newest first, so: read pages until a song the copy knows; if the count
/// then matches, the copy plus the new songs is the list (often 1 request). Else (songs were
/// removed) read it all. When Spotify refuses midway and there is a copy, the copy stands.
/// `songs` is the saved list; it counts only when it matches the saved index.
fn sync_liked(api: &mut impl LikedPages, copy: Option<Library>, songs: Vec<Track>) -> Result<(Library, Vec<Track>)> {
    let known = copy.as_ref().is_some_and(|c| c.liked_order.len() == songs.len());
    let mut fresh: Vec<Track> = Vec::new();
    let mut offset = 0;
    let mut total;
    let mut met = false;
    loop {
        let page = match api.liked_page(offset) {
            Ok(page) => page,
            Err(e) => return copy.map(|c| (c, songs)).ok_or(e),
        };
        total = page.total;
        offset += page.raw;
        for t in page.items {
            if known && copy.as_ref().is_some_and(|c| c.tracks.contains(&t.uri)) {
                met = true;
                break;
            }
            fresh.push(t);
        }
        if met || page.raw == 0 || offset >= total {
            break;
        }
    }
    if met && fresh.len() + songs.len() == total {
        let mut lib = copy.unwrap_or_default();
        for t in &fresh {
            add_liked(&mut lib, t);
        }
        fresh.extend(songs);
        lib.liked_order = fresh.iter().map(|t| t.uri.clone()).collect();
        return Ok((lib, fresh));
    }
    if !met {
        // No song of the copy came up: `fresh` is the whole list.
        return Ok(rebuilt(copy, fresh));
    }
    // Something changed that the newest pages do not show: read everything again.
    let mut all: Vec<Track> = Vec::new();
    let mut offset = 0;
    loop {
        let page = match api.liked_page(offset) {
            Ok(page) => page,
            Err(e) => return copy.map(|c| (c, songs)).ok_or(e),
        };
        offset += page.raw;
        let done = page.raw == 0 || offset >= page.total;
        all.extend(page.items);
        if done {
            break;
        }
    }
    Ok(rebuilt(copy, all))
}

/// The index made new from every Liked Song (what the copy knows of your taste is kept).
fn rebuilt(copy: Option<Library>, songs: Vec<Track>) -> (Library, Vec<Track>) {
    let mut lib = copy.unwrap_or_default();
    lib.tracks.clear();
    lib.artists.clear();
    for t in &songs {
        add_liked(&mut lib, t);
    }
    lib.liked_order = songs.iter().map(|t| t.uri.clone()).collect();
    (lib, songs)
}

/// The Liked Songs index, plus top and recent songs and artists (for discover) once a day.
fn sync_library(api: &mut Spotify, copy: Option<Library>, songs: Vec<Track>) -> Result<(Library, Vec<Track>)> {
    let (mut library, songs) = sync_liked(api, copy, songs)?;
    let now = crate::api::unix_now();
    if now.saturating_sub(library.taste_at) < 24 * 3600 {
        return Ok((library, songs));
    }
    // A login from before these permissions gets an error here: discover then knows Liked
    // Songs only.
    let mut heard = std::collections::HashSet::new();
    let mut top = std::collections::HashSet::new();
    if api.has_scope("user-top-read") {
        for range in ["short_term", "medium_term", "long_term"] {
            for a in api.top_artists(range).unwrap_or_default() {
                top.insert(format!("spotify:artist:{}", a.id));
            }
        }
        for range in ["short_term", "medium_term"] {
            heard.extend(api.top_tracks(range).unwrap_or_default().into_iter().map(|t| t.uri));
        }
    }
    if api.has_scope("user-read-recently-played") {
        if let Ok(played) = api.recently_played() {
            heard.extend(played.into_iter().map(|p| p.track.uri));
        }
        heard.extend(crate::history::load().into_iter().take(50).map(|p| p.track.uri));
    }
    // Keep the old lists when Spotify refused the new ones.
    if !top.is_empty() || !heard.is_empty() {
        library.top_artists = top;
        library.heard = heard;
        library.taste_at = now;
    }
    Ok((library, songs))
}

/// Pages of Liked Songs and Playlists kept on disk: a good page is saved; when Spotify refuses
/// one (a rate limit, no network), the saved one stands in, and a rate limit asks again later.
fn saved_copy<T>(list: &List, offset: usize, fetched: Result<Page<T>>, tx: &Sender<Msg>) -> Result<Page<T>>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let name = match list {
        List::Liked => format!("lists/liked-{offset}.json"),
        _ => format!("lists/playlists-{offset}.json"),
    };
    match fetched {
        Ok(page) => {
            crate::store::write(&name, &page);
            Ok(page)
        }
        Err(e) => {
            let Some(page) = crate::store::read::<Page<T>>(&name) else { return Err(e) };
            if let Some(ApiError::RateLimited(secs)) = e.downcast_ref::<ApiError>() {
                let _ = tx.send(Msg::RetryLater { list: list.clone(), secs: *secs });
            }
            Ok(page)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(n: usize) -> Page<u32> {
        Page { items: vec![n as u32], raw: 1, total: 1 }
    }

    struct Fake {
        /// Liked Songs, newest first.
        uris: Vec<String>,
        calls: usize,
        fail: bool,
    }

    impl LikedPages for Fake {
        fn liked_page(&mut self, offset: usize) -> Result<Page<Track>> {
            self.calls += 1;
            if self.fail {
                return Err(ApiError::RateLimited(60).into());
            }
            let items: Vec<Track> = self.uris.iter().skip(offset).take(50).map(|u| track(u)).collect();
            Ok(Page { raw: items.len(), items, total: self.uris.len() })
        }
    }

    fn track(uri: &str) -> Track {
        serde_json::from_value(serde_json::json!({ "uri": uri, "name": uri, "album": null, "show": null })).unwrap()
    }

    fn liked(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("spotify:track:{i}")).collect()
    }

    #[test]
    fn liked_sync_asks_little_when_little_changed() {
        let mut api = Fake { uris: liked(120), calls: 0, fail: false };
        let uris = |songs: &[Track]| songs.iter().map(|t| t.uri.clone()).collect::<Vec<_>>();
        let (lib, songs) = sync_liked(&mut api, None, Vec::new()).unwrap();
        assert_eq!((lib.liked_order.len(), songs.len(), api.calls), (120, 120, 3), "no copy: every page");
        // Nothing new: one request.
        api.calls = 0;
        let (lib, songs) = sync_liked(&mut api, Some(lib), songs).unwrap();
        assert_eq!((lib.liked_order.len(), songs.len(), api.calls), (120, 120, 1));
        // Two new songs on top: still one request, and they come first.
        api.uris.insert(0, "spotify:track:new1".into());
        api.uris.insert(0, "spotify:track:new2".into());
        api.calls = 0;
        let (lib, songs) = sync_liked(&mut api, Some(lib), songs).unwrap();
        assert_eq!(api.calls, 1);
        assert_eq!(lib.liked_order, api.uris);
        assert_eq!(uris(&songs), api.uris, "the songs themselves too");
        // A song removed: the count is off, so it reads everything again.
        api.uris.remove(60);
        api.calls = 0;
        let (lib, songs) = sync_liked(&mut api, Some(lib), songs).unwrap();
        assert_eq!(lib.liked_order, api.uris);
        assert_eq!(uris(&songs), api.uris);
        assert!(!lib.tracks.contains("spotify:track:58"));
        // An index without its songs (saved by an older version): everything is read once.
        api.calls = 0;
        let (lib, songs) = sync_liked(&mut api, Some(lib), Vec::new()).unwrap();
        assert_eq!((songs.len(), api.calls), (121, 3));
        // Refused: the copy stands; no copy: the error.
        api.fail = true;
        assert_eq!(sync_liked(&mut api, Some(lib), songs).unwrap().1.len(), 121);
        assert!(sync_liked(&mut api, None, Vec::new()).is_err());
    }

    #[test]
    fn a_saved_playlist_is_used_while_it_is_current() {
        let (young, old) = (Duration::from_secs(60), Duration::from_secs(3600));
        assert!(playlist_current(Some("a"), Some("a"), old), "the same snapshot: any age");
        assert!(!playlist_current(Some("a"), Some("b"), young), "another snapshot: load again");
        assert!(playlist_current(None, Some("b"), young) && playlist_current(Some("a"), None, young));
        assert!(!playlist_current(Some("a"), None, old), "nothing to compare: 10 minutes");
        let songs: Vec<Track> = (0..5).map(|i| track(&i.to_string())).collect();
        let page = rest_page(songs, 3);
        assert_eq!((page.items.len(), page.raw, page.total), (2, 2, 5));
    }

    #[test]
    fn true_shuffle_keeps_every_song_once_with_the_start_first() {
        let uris: Vec<String> = (0..300).map(|i| format!("spotify:track:{i}")).chain(["spotify:track:7".into()]).collect();
        let mut rng = rand::thread_rng();
        let out = shuffled(uris.clone(), Some("spotify:track:42"), &mut rng);
        assert_eq!(out[0], "spotify:track:42");
        assert_eq!(out.len(), 300, "the double one once");
        let set: std::collections::HashSet<_> = out.iter().collect();
        assert_eq!(set.len(), 300);
        assert_ne!(out[1..20], uris[..19], "not in order");
        // Every song can come anywhere: over many shuffles, song 0 is not always early.
        let late = (0..200).filter(|_| {
            let o = shuffled(uris.clone(), None, &mut rng);
            o.iter().position(|u| u == "spotify:track:0").unwrap() > 150
        });
        assert!(late.count() > 50);
        assert_eq!(shuffled((0..900).map(|i| i.to_string()).collect(), None, &mut rng).len(), SHUFFLE_MAX);
    }

    #[test]
    fn search_cache_answers_a_recent_first_page() {
        let mut cache: SearchCache<u32> = SearchCache::default();
        let mut calls = 0;
        let mut get = |cache: &mut SearchCache<u32>, key: &str, offset: usize| {
            cache
                .get(key, offset, || {
                    calls += 1;
                    Ok(page(calls))
                })
                .unwrap()
                .items[0]
        };
        assert_eq!(get(&mut cache, "bowie", 0), 1);
        assert_eq!(get(&mut cache, "bowie", 0), 1, "from the cache");
        assert_eq!(get(&mut cache, "bowie", 50), 2, "each page on its own");
        assert_eq!(get(&mut cache, "bowie", 50), 2);
        assert_eq!(get(&mut cache, "bowi", 0), 3);
        // Old entries go: only the last 32 pages stay.
        for i in 0..32 {
            get(&mut cache, &format!("q{i}"), 0);
        }
        assert_eq!(get(&mut cache, "bowie", 0), 36);
        // A failed request is not kept.
        assert!(cache.get("err", 0, || Err(anyhow!("down"))).is_err());
        assert!(cache.pages.iter().all(|(k, _, _)| k != "err"));
    }

    #[test]
    fn player_commands_never_wait_behind_loads() {
        for cmd in [Cmd::Toggle, Cmd::Next, Cmd::Poll, Cmd::Volume(3), Cmd::Seek(1), Cmd::UpNext] {
            assert!(control_lane(&cmd), "{cmd:?}");
        }
        for cmd in [Cmd::Load { list: List::Liked, offset: 0 }, Cmd::Randomize("x".into()), Cmd::Library, Cmd::Artist("a".into())] {
            assert!(!control_lane(&cmd), "{cmd:?}");
        }
        assert!(is_search_start(&Cmd::Load { list: List::Search("a".into()), offset: 0 }));
        assert!(!is_search_start(&Cmd::Load { list: List::Search("a".into()), offset: 10 }));
    }
}
