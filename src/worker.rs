//! Background thread that owns the API client, so the UI never waits on the network.

use crate::api::{Album, ApiError, Artist, Found, Page, Play, Playback, Playlist, Spotify, Track, User};
use crate::radio::{Library, RadioTrack};
use anyhow::{anyhow, Result};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

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
    /// Likes a song, or unlikes it if it is liked (asks Spotify which first).
    ToggleSaved(String),
    /// A new private playlist with these songs.
    CreatePlaylist { name: String, description: String, uris: Vec<String> },
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
}

impl Worker {
    fn run(mut self, rx: Receiver<Cmd>) {
        for cmd in rx {
            let kind = match cmd {
                Cmd::Poll => Kind::Poll,
                Cmd::Volume(_) => Kind::Volume,
                Cmd::Play(_) | Cmd::PlayLiked { .. } | Cmd::Toggle | Cmd::Next | Cmd::Prev | Cmd::Seek(_) => Kind::Player,
                _ => Kind::Other,
            };
            let list = match &cmd {
                Cmd::Load { list, .. } => Some(list.clone()),
                _ => None,
            };
            if let Err(err) = self.handle(cmd) {
                self.report(err, list);
            }
            if self.tx.send(Msg::Done(kind)).is_err() {
                return; // UI has quit
            }
        }
    }

    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    fn report(&self, err: anyhow::Error, list: Option<List>) {
        let api_err = err.downcast_ref::<ApiError>();
        if let Some(ApiError::RateLimited(secs)) = api_err {
            self.send(Msg::Backoff(*secs));
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
                let user = self.api.me()?;
                self.user = Some(user.clone());
                self.send(Msg::User(user));
                self.find_device()?;
                self.poll()
            }
            Cmd::Poll => self.poll(),
            Cmd::Load { list, offset } => self.load(list, offset),
            Cmd::Play(play) => self.player(|api, dev| api.play(dev, Some(&play))),
            Cmd::PlayLiked { from } => {
                let user = match &self.user {
                    Some(u) => u.clone(),
                    None => self.api.me()?,
                };
                let play = Play::Context {
                    uri: format!("{}:collection", user.uri),
                    from: Some(from),
                };
                self.player(|api, dev| api.play(dev, Some(&play)))
            }
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
            Cmd::Queue { uri, name } => {
                self.player(|api, dev| api.queue(dev, &uri))?;
                self.send(Msg::Info(format!("Added to queue: {name}")));
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
                let mut library = Library::default();
                let mut offset = 0;
                loop {
                    let page = self.api.liked_tracks(offset)?;
                    offset += page.raw;
                    for t in &page.items {
                        library.tracks.insert(t.uri.clone());
                        library.artists.extend(t.artists.iter().filter_map(|a| a.uri.clone()));
                    }
                    if page.raw == 0 || offset >= page.total {
                        break;
                    }
                }
                // Top and recent songs and artists, for discover. A login from before these
                // permissions gets an error here: discover then knows Liked Songs only.
                if self.api.has_scope("user-top-read") {
                    for range in ["short_term", "medium_term", "long_term"] {
                        for a in self.api.top_artists(range).unwrap_or_default() {
                            library.top_artists.insert(format!("spotify:artist:{}", a.id));
                        }
                    }
                    for range in ["short_term", "medium_term"] {
                        library.heard.extend(self.api.top_tracks(range).unwrap_or_default().into_iter().map(|t| t.uri));
                    }
                }
                if self.api.has_scope("user-read-recently-played") {
                    if let Ok(page) = self.api.recently_played(0) {
                        library.heard.extend(page.items.into_iter().map(|t| t.uri));
                    }
                }
                self.send(Msg::Library(library));
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

    fn poll(&mut self) -> Result<()> {
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
                let page = self.api.search_tracks(q, offset)?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Found { query, kind } => {
                let page = self.api.search(query, kind, offset)?;
                self.send(Msg::Found { list, offset, page });
            }
            List::Recent => {
                let page = self.api.recently_played(offset)?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Liked => {
                let page = self.api.liked_tracks(offset)?;
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
                let page = self.api.playlists(offset)?;
                self.send(Msg::Playlists { offset, page });
            }
        }
        Ok(())
    }
}
