//! Background thread that owns the API client, so the UI never waits on the network.

use crate::api::{ApiError, Page, Play, Playback, Playlist, Spotify, Track, User};
use anyhow::{anyhow, Result};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

/// A list the UI shows, used to route pages back to the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum List {
    Search(String),
    Playlists,
    Liked,
    Playlist(String),
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
}

#[derive(Debug)]
pub enum Msg {
    User(User),
    /// Name of the device spotlite will play on, if Spotify sees it.
    Device(Option<String>),
    Playback(Option<Box<Playback>>),
    Tracks { list: List, offset: usize, page: Page<Track> },
    Playlists { offset: usize, page: Page<Playlist> },
    LoadFailed { list: List, forbidden: bool },
    Saved { uri: String, saved: bool },
    Info(String),
    Error(String),
    /// Spotify asked us to wait this many seconds.
    Backoff(u64),
    /// The user's playlist folders (from the folder thread).
    Folders(Vec<crate::folders::Entry>),
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
            Cmd::SetSaved { uri, saved } => {
                self.api.set_saved(&uri, saved)?;
                self.send(Msg::Saved { uri, saved });
                self.send(Msg::Info(if saved { "Added to Liked Songs" } else { "Removed from Liked Songs" }.into()));
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
            List::Liked => {
                let page = self.api.liked_tracks(offset)?;
                self.send(Msg::Tracks { list, offset, page });
            }
            List::Playlist(id) => {
                let page = self.api.playlist_tracks(id, offset)?;
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
