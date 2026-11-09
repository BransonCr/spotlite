//! Background thread that owns the API client, so the UI never waits on the network.

use crate::api::{Album, ApiError, Artist, Found, Page, Play, Playback, Playlist, Spotify, Track, User};
use crate::radio::{Library, RadioTrack};
use anyhow::{anyhow, Result};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::collections::VecDeque;
use std::thread;
use std::time::{Duration, Instant};

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
    Resume(crate::state::SavedPlayback),
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
    AddToPlaylist { playlist_id: String, playlist: String, uris: Vec<String>, what: String },
    /// The genres of an artist (MusicBrainz, on its own thread), for Randomize.
    Genres { artists: Vec<(String, String)> },
    /// 50 random songs of a genre.
    Randomize(String),
    RemoveFromPlaylist { playlist_id: String, playlist: String, uri: String, what: String },
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
    Removed { playlist_id: String, uri: String },
    /// A track was added to a playlist.
    Added { playlist_id: String },
    UpNext(Option<Track>),
    /// A true shuffle started: these songs play for `context`.
    Shuffled { context: String, uris: Vec<String> },
    Genres { artist: String, result: Result<Vec<String>, String> },
    Random { genre: String, result: Result<Vec<Track>, String> },
    /// A playlist was created: the Playlists tab loads again.
    Created,
    /// Sent after every command.
    Done(Kind),
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

/// Starts the API thread. Returns its command sender, a sender other threads can use to
/// post messages to the UI, and the UI's message receiver.
pub fn spawn(api: Spotify) -> (Sender<Cmd>, Sender<Msg>, Receiver<Msg>) {
    let (cmd_tx, cmd_rx) = channel();
    let (msg_tx, msg_rx) = channel();
    let worker = Worker {
        api,
        tx: msg_tx.clone(),
        device: None,
        user: None,
        last: None,
        song_searches: SearchCache::default(),
        found_searches: SearchCache::default(),
        me_tried: None,
    };
    thread::Builder::new()
        .name("spotlite-api".into())
        .spawn(move || worker.run(cmd_rx))
        .expect("cannot start the API thread");
    (cmd_tx, msg_tx, msg_rx)
}

struct Worker {
    api: Spotify,
    tx: Sender<Msg>,
    device: Option<String>,
    user: Option<User>,
    last: Option<Playback>,
    song_searches: SearchCache<Track>,
    found_searches: SearchCache<Found>,
    /// When /me was last asked without an answer (it is asked again at most every 10 min).
    me_tried: Option<Instant>,
}

/// First pages of recent searches. Search as you type asks again for a text you had a moment
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
    const KEEP: usize = 16;
    const MAX_AGE: Duration = Duration::from_secs(120);

    /// The first page for `key`, from the cache if it is recent, else from `fetch`. Later pages
    /// always come from `fetch`.
    fn get(&mut self, key: &str, offset: usize, fetch: impl FnOnce() -> Result<Page<T>>) -> Result<Page<T>> {
        if offset > 0 {
            return fetch();
        }
        self.pages.retain(|(_, at, _)| at.elapsed() < Self::MAX_AGE);
        if let Some((_, _, page)) = self.pages.iter().find(|(k, _, _)| k == key) {
            return Ok(page.clone());
        }
        let page = fetch()?;
        self.pages.push_back((key.to_owned(), Instant::now(), page.clone()));
        if self.pages.len() > Self::KEEP {
            self.pages.pop_front();
        }
        Ok(page)
    }
}

impl Worker {
    fn run(mut self, rx: Receiver<Cmd>) {
        for cmd in rx {
            let kind = match cmd {
                // Init ends with the first player state, like a poll.
                Cmd::Poll | Cmd::Init => Kind::Poll,
                Cmd::Volume(_) => Kind::Volume,
                Cmd::Play(_) | Cmd::PlayAt(..) | Cmd::PlayShuffled { .. } | Cmd::PlayLiked { .. } | Cmd::Toggle | Cmd::Next | Cmd::Prev | Cmd::Seek(_) => Kind::Player,
                _ => Kind::Other,
            };
            let list = match &cmd {
                Cmd::Load { list, .. } => Some(list.clone()),
                _ => None,
            };
            let quiet = matches!(cmd, Cmd::CheckSaved(_) | Cmd::UpNext);
            if let Err(err) = self.handle(cmd) {
                self.report(err, list, kind, quiet);
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
        if quiet && matches!(api_err, Some(ApiError::RateLimited(_))) {
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
                    Ok(user) => {
                        self.user = Some(user.clone());
                        self.send(Msg::User(user.clone()));
                    }
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
            Cmd::Load { list, offset } => self.load(list, offset),
            Cmd::Play(play) => self.player(|api, dev| api.play(dev, Some(&play))),
            Cmd::PlayLiked { from } => {
                let user = match &self.user {
                    Some(u) => u.clone(),
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
                    self.api.add_to_playlist(&pl.id, chunk)?;
                }
                self.send(Msg::Created);
                self.send(Msg::Info(format!("Saved {} songs to the new playlist “{name}”", uris.len())));
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
                let result = (|| -> Result<()> {
                    let artist = self.api.artist(&id)?;
                    let top = self.api.artist_top_tracks(&artist)?;
                    let albums = self.api.artist_albums(&id)?;
                    self.send(Msg::Artist { artist, top, albums });
                    Ok(())
                })();
                if let Err(e) = &result {
                    self.send(Msg::PageFailed { id, error: format!("{e:#}") });
                }
                result
            }
            Cmd::Album(id) => {
                let album = self.api.album(&id)?;
                self.send(Msg::Album(album));
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
                let library = match sync_library(&mut self.api, crate::store::read("library.json")) {
                    Ok(library) => library,
                    // The Liked Songs tab says so already; the ♥ marks wait for the next start.
                    Err(e) if matches!(e.downcast_ref::<ApiError>(), Some(ApiError::RateLimited(_))) => return Ok(()),
                    Err(e) => return Err(e),
                };
                crate::store::write("library.json", &library);
                self.send(Msg::Library(library));
                Ok(())
            }
            Cmd::Genres { artists } => {
                let tx = self.tx.clone();
                thread::Builder::new().name("spotlite-genres".into()).spawn(move || {
                    // The first artist MusicBrainz has genres for (a song's guests count too).
                    let mut result = Ok(Vec::new());
                    let first = artists.first().map(|a| a.1.clone()).unwrap_or_default();
                    for (uri, name) in &artists {
                        result = crate::genre::of_artist(uri, name).map_err(|e| format!("{e:#}"));
                        if let Ok(g) = &result {
                            if !g.is_empty() {
                                let _ = tx.send(Msg::Genres { artist: name.clone(), result });
                                return;
                            }
                        }
                    }
                    let _ = tx.send(Msg::Genres { artist: first, result });
                })?;
                Ok(())
            }
            Cmd::Randomize(genre) => {
                let result = crate::genre::random(&mut self.api, &genre).map_err(|e| format!("{e:#}"));
                self.send(Msg::Random { genre, result });
                Ok(())
            }
            Cmd::AddToPlaylist { playlist_id, playlist, uris, what } => {
                self.api.add_to_playlist(&playlist_id, &uris)?;
                self.send(Msg::Added { playlist_id });
                self.send(Msg::Info(format!("Added {what} to {playlist}")));
                Ok(())
            }
            Cmd::RemoveFromPlaylist { playlist_id, playlist, uri, what } => {
                self.api.remove_from_playlist(&playlist_id, std::slice::from_ref(&uri))?;
                self.send(Msg::Removed { playlist_id, uri });
                self.send(Msg::Info(format!("Removed {what} from {playlist}")));
                Ok(())
            }
        }
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
        if self.user.is_none() && self.me_tried.is_some_and(|at| at.elapsed() >= Duration::from_secs(600)) {
            match self.api.me_cached() {
                Ok(user) => {
                    self.user = Some(user.clone());
                    self.send(Msg::User(user));
                }
                Err(_) => self.me_tried = Some(Instant::now()),
            }
        }
        let pb = self.api.playback()?;
        self.last = pb.clone();
        self.send(Msg::Playback(pb.map(Box::new)));
        Ok(())
    }

    fn find_device(&mut self) -> Result<Option<String>> {
        let device = self.api.find_device()?;
        self.device = device.as_ref().and_then(|d| d.id.clone());
        self.send(Msg::Device(device.map(|d| d.name)));
        Ok(self.device.clone())
    }

    fn require_device(&mut self) -> Result<String> {
        if let Some(id) = &self.device {
            return Ok(id.clone());
        }
        self.find_device()?.ok_or_else(|| {
            anyhow!(
                "device '{}' not found; start it with `systemctl --user start spotifyd`",
                self.api.config().device_name
            )
        })
    }

    /// Runs a player command on our device. If Spotify says the device is gone
    /// (e.g. spotifyd restarted), looks it up again and retries once. The UI asks for the new
    /// state itself, so the thread is free for the next command at once.
    fn player(&mut self, mut f: impl FnMut(&mut Spotify, Option<&str>) -> Result<()>) -> Result<()> {
        let dev = self.require_device()?;
        match f(&mut self.api, Some(&dev)) {
            Err(e) if matches!(e.downcast_ref::<ApiError>(), Some(ApiError::NoDevice)) => {
                self.device = None;
                let dev = self.require_device()?;
                f(&mut self.api, Some(&dev))?;
            }
            other => other?,
        }
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
                let page = self.api.recently_played(offset)?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Liked => {
                let page = saved_copy(&list, offset, self.api.liked_tracks(offset), &self.tx)?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Playlist(id) => {
                let page = self.api.playlist_tracks(id, offset)?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Album(id) => {
                let page = self.api.album_tracks(id, offset)?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Playlists => {
                let page = saved_copy(&list, offset, self.api.playlists(offset), &self.tx)?;
                self.send(Msg::Playlists { offset, page });
            }
        }
        Ok(())
    }
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

/// Brings the Liked Songs index up to date with as few requests as it can. Liked Songs come
/// newest first, so: read pages until a song the copy knows; if the count then matches, the
/// copy plus the new songs is the list (often 1 request). Else (songs were removed) read it all.
/// With no copy, or when Spotify refuses midway and there is a copy, the copy stands.
fn sync_liked(api: &mut impl LikedPages, copy: Option<Library>) -> Result<Library> {
    let mut fresh: Vec<Track> = Vec::new();
    let mut offset = 0;
    let mut total;
    let mut met = false;
    loop {
        let page = match api.liked_page(offset) {
            Ok(page) => page,
            Err(e) => return copy.ok_or(e),
        };
        total = page.total;
        offset += page.raw;
        for t in page.items {
            if copy.as_ref().is_some_and(|c| c.tracks.contains(&t.uri)) {
                met = true;
                break;
            }
            fresh.push(t);
        }
        if met || page.raw == 0 || offset >= total {
            break;
        }
    }
    if let Some(mut lib) = copy.clone().filter(|_| met) {
        let joined = fresh.len() + lib.liked_order.len();
        if joined == total {
            for t in &fresh {
                add_liked(&mut lib, t);
            }
            let mut order: Vec<String> = fresh.iter().map(|t| t.uri.clone()).collect();
            order.append(&mut lib.liked_order);
            lib.liked_order = order;
            return Ok(lib);
        }
    }
    if !met {
        // No song of the copy came up: `fresh` is the whole list.
        let mut lib = copy.unwrap_or_default();
        lib.tracks.clear();
        lib.artists.clear();
        for t in &fresh {
            add_liked(&mut lib, t);
        }
        lib.liked_order = fresh.into_iter().map(|t| t.uri).collect();
        return Ok(lib);
    }
    // Something changed that the newest pages do not show: read everything again.
    let mut lib = copy.clone().unwrap_or_default();
    lib.tracks.clear();
    lib.artists.clear();
    lib.liked_order.clear();
    let mut offset = 0;
    loop {
        let page = match api.liked_page(offset) {
            Ok(page) => page,
            Err(e) => return copy.ok_or(e),
        };
        offset += page.raw;
        for t in &page.items {
            add_liked(&mut lib, t);
            lib.liked_order.push(t.uri.clone());
        }
        if page.raw == 0 || offset >= page.total {
            break;
        }
    }
    Ok(lib)
}

/// The Liked Songs index, plus top and recent songs and artists (for discover) once a day.
fn sync_library(api: &mut Spotify, copy: Option<Library>) -> Result<Library> {
    let mut library = sync_liked(api, copy)?;
    let now = crate::api::unix_now();
    if now.saturating_sub(library.taste_at) < 24 * 3600 {
        return Ok(library);
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
        if let Ok(page) = api.recently_played(0) {
            heard.extend(page.items.into_iter().map(|t| t.uri));
        }
    }
    // Keep the old lists when Spotify refused the new ones.
    if !top.is_empty() || !heard.is_empty() {
        library.top_artists = top;
        library.heard = heard;
        library.taste_at = now;
    }
    Ok(library)
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
        let lib = sync_liked(&mut api, None).unwrap();
        assert_eq!((lib.liked_order.len(), api.calls), (120, 3), "no copy: every page");
        // Nothing new: one request.
        api.calls = 0;
        let lib = sync_liked(&mut api, Some(lib)).unwrap();
        assert_eq!((lib.liked_order.len(), api.calls), (120, 1));
        // Two new songs on top: still one request, and they come first.
        api.uris.insert(0, "spotify:track:new1".into());
        api.uris.insert(0, "spotify:track:new2".into());
        api.calls = 0;
        let lib = sync_liked(&mut api, Some(lib)).unwrap();
        assert_eq!(api.calls, 1);
        assert_eq!(lib.liked_order, api.uris);
        // A song removed: the count is off, so it reads everything again.
        api.uris.remove(60);
        api.calls = 0;
        let lib = sync_liked(&mut api, Some(lib)).unwrap();
        assert_eq!(lib.liked_order, api.uris);
        assert!(!lib.tracks.contains("spotify:track:58"));
        // Refused: the copy stands; no copy: the error.
        api.fail = true;
        assert_eq!(sync_liked(&mut api, Some(lib)).unwrap().liked_order.len(), 121);
        assert!(sync_liked(&mut api, None).is_err());
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
        assert_eq!(get(&mut cache, "bowie", 50), 2, "later pages always load");
        assert_eq!(get(&mut cache, "bowi", 0), 3);
        // Old entries go: only the last 16 texts stay.
        for i in 0..16 {
            get(&mut cache, &format!("q{i}"), 0);
        }
        assert_eq!(get(&mut cache, "bowie", 0), 20);
        // A failed request is not kept.
        assert!(cache.get("err", 0, || Err(anyhow!("down"))).is_err());
        assert!(cache.pages.iter().all(|(k, _, _)| k != "err"));
    }
}
