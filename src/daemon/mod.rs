//! `spotlite daemon`: spotlite's own player, for systems without spotifyd (Windows) or for
//! `backend = "builtin"`. A librespot Spotify Connect device that plays the audio, like
//! spotifyd does. The UI starts it in the background (see `engine::builtin`) and talks to it
//! over a local socket (see `engine::proto`); it keeps playing after the UI quits, until `Q`.
//!
//! It mirrors contrib/spotifyd.conf: 320 kbit/s, soft volume starting at 60%, volume
//! normalisation, no audio cache.

mod media;
mod tap;

use crate::config::Config;
use crate::control::Ctl;
use crate::engine::builtin;
use crate::engine::proto::{self, Request, Song};
use anyhow::{anyhow, bail, Context, Result};
use librespot_connect::{ConnectConfig, Spirc};
use librespot_core::authentication::Credentials;
use librespot_core::cache::Cache;
use librespot_core::config::DeviceType;
use librespot_core::{Session, SessionConfig};
use librespot_metadata::audio::{AudioItem, UniqueFields};
use librespot_playback::audio_backend;
use librespot_playback::config::{AudioFormat, Bitrate, PlayerConfig};
use librespot_playback::mixer::{self, MixerConfig};
use librespot_playback::player::{Player, PlayerEvent, PlayerEventChannel};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tap::{Tap, TapSink};

/// The sample format for the audio output. Windows shares the output in 32-bit float, so
/// asking for that opens the device at once, without librespot's fallback.
const OUTPUT_FORMAT: AudioFormat = if cfg!(windows) { AudioFormat::F32 } else { AudioFormat::S16 };

/// The redirect that librespot's own login uses with Spotify's client (it allows this one).
const LOGIN_REDIRECT: &str = "http://127.0.0.1:5588/login";
/// What librespot's own login asks for.
const LOGIN_SCOPES: &[&str] = &[
    "app-remote-control",
    "playlist-modify",
    "playlist-modify-private",
    "playlist-modify-public",
    "playlist-read",
    "playlist-read-collaborative",
    "playlist-read-private",
    "streaming",
    "ugc-image-upload",
    "user-follow-modify",
    "user-follow-read",
    "user-library-modify",
    "user-library-read",
    "user-modify",
    "user-modify-playback-state",
    "user-modify-private",
    "user-personalized",
    "user-read-birthdate",
    "user-read-currently-playing",
    "user-read-email",
    "user-read-play-history",
    "user-read-playback-position",
    "user-read-playback-state",
    "user-read-private",
    "user-read-recently-played",
    "user-top-read",
];
const LOGIN_PAGE: &str = "<html><body style=\"font-family:sans-serif;text-align:center;margin-top:20vh\">\
<h2>spotlite's player is logged in.</h2><p>You can close this tab and go back to the terminal.</p></body></html>";

/// Logs the player in to Spotify in the browser, and saves the login for later runs. Run it
/// from a terminal (it prints the address too, in case the browser does not open).
pub fn login() -> Result<()> {
    let dir = builtin::credentials_dir().ok_or_else(|| anyhow!("no cache folder"))?;
    let client_id = SessionConfig::default().client_id;
    let client = librespot_oauth::OAuthClientBuilder::new(&client_id, LOGIN_REDIRECT, LOGIN_SCOPES.to_vec())
        .open_in_browser()
        .with_custom_message(LOGIN_PAGE)
        .build()?;
    let token = client.get_access_token().context("logging in spotlite's player")?;
    save_login(&dir, &token.access_token)
}

/// Connects once with the browser login's token, so librespot saves a lasting login in `dir`.
fn save_login(dir: &std::path::Path, access_token: &str) -> Result<()> {
    let cache = Cache::new(Some(dir), None, None, None)?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    // librespot's session starts tasks of its own: make it inside the runtime.
    rt.block_on(async {
        let session = Session::new(SessionConfig::default(), Some(cache));
        let connected = session.connect(Credentials::with_access_token(access_token), true).await;
        session.shutdown();
        connected
    })
    .context("connecting spotlite's player to Spotify")?;
    Ok(())
}

/// Adds a line to the daemon's log.
fn log(text: &str) {
    let path = builtin::log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Keep the log small: start again past 1 MB.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {text}", crate::api::unix_now());
    }
}

/// Sends librespot's messages to the daemon's log: its warnings and errors, and which audio
/// device it plays on.
struct Messages;

impl log::Log for Messages {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
            || (m.level() == log::Level::Info && m.target().starts_with("librespot_playback::audio_backend"))
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            log(&format!("{} {}: {}", r.level(), r.target(), r.args()));
        }
    }

    fn flush(&self) {}
}

/// Logs librespot's messages, and a panic before the process ends (a release build aborts).
fn log_messages() {
    if log::set_logger(&Messages).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log(&format!("panic: {info}"));
        previous(info);
    }));
}

/// What the connections share with the player.
struct Shared {
    started: Instant,
    token: String,
    /// The Connect device, while it is connected.
    spirc: Mutex<Option<Arc<Spirc>>>,
    /// Readers of player changes, and the last change (a new reader gets it at once).
    followers: Mutex<(Vec<TcpStream>, String)>,
    tap: Arc<Tap>,
    quit: AtomicBool,
}

impl Shared {
    fn publish(&self, line: String) {
        if let Ok(mut f) = self.followers.lock() {
            if f.1 == line {
                return;
            }
            f.0.retain_mut(|s| s.write_all(format!("{line}\n").as_bytes()).is_ok());
            f.1 = line;
        }
    }
}

/// Runs the player until `quit`. Only one runs: if another answers on the port, this one ends.
pub fn run(cfg: &Config) -> Result<()> {
    log_messages();
    let result = serve(cfg);
    if let Err(e) = &result {
        log(&format!("stopped: {e:#}"));
    }
    result
}

fn serve(cfg: &Config) -> Result<()> {
    let port = cfg.daemon_port;
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) if builtin::age(port).is_some() => return Ok(()),
        Err(e) => bail!("cannot listen on 127.0.0.1:{port} ({e}); set daemon_port in config.toml"),
    };
    if !builtin::logged_in() {
        bail!("no login: start spotlite (or run `spotlite auth`) to log the player in");
    }
    let token: String = {
        use rand::Rng;
        rand::thread_rng().sample_iter(&rand::distributions::Alphanumeric).take(32).map(char::from).collect()
    };
    write_token(&token)?;
    let shared = Arc::new(Shared {
        started: Instant::now(),
        token,
        spirc: Mutex::new(None),
        followers: Mutex::new((Vec::new(), proto::follow_line(false, None))),
        tap: Tap::start(),
        quit: AtomicBool::new(false),
    });
    log(&format!("started, device '{}', port {port}", cfg.device_name));
    let accept = shared.clone();
    std::thread::Builder::new().name("spotlite-socket".into()).spawn(move || {
        for conn in listener.incoming().flatten() {
            let shared = accept.clone();
            let _ = std::thread::Builder::new().name("spotlite-conn".into()).spawn(move || answer(conn, &shared));
        }
    })?;
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let media = media::start(shared.clone());
    rt.block_on(play(cfg.device_name.clone(), shared, media));
    Ok(())
}

fn write_token(token: &str) -> Result<()> {
    let path = builtin::token_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    crate::platform::private_file(&mut options);
    options.open(&path)?.write_all(token.as_bytes())?;
    Ok(())
}

/// One connection: the token, then one request.
fn answer(conn: TcpStream, shared: &Shared) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
    let Ok(mut out) = conn.try_clone() else { return };
    let mut lines = BufReader::new(conn).lines();
    if lines.next().and_then(|l| l.ok()).is_none_or(|t| t.trim() != shared.token) {
        return;
    }
    let Some(req) = lines.next().and_then(|l| l.ok()).and_then(|l| proto::parse(&l)) else {
        let _ = out.write_all(b"err unknown request\n");
        return;
    };
    let reply = match req {
        Request::Age => format!("ok {}", shared.started.elapsed().as_secs()),
        Request::Quit => {
            let _ = out.write_all(b"ok\n");
            shared.quit.store(true, Ordering::SeqCst);
            if let Some(spirc) = shared.spirc.lock().ok().and_then(|s| s.clone()) {
                let _ = spirc.shutdown();
            }
            log("quit");
            std::thread::sleep(Duration::from_millis(300));
            std::process::exit(0);
        }
        Request::Follow => {
            let _ = out.set_write_timeout(Some(Duration::from_secs(1)));
            if let Ok(mut f) = shared.followers.lock() {
                if out.write_all(format!("{}\n", f.1).as_bytes()).is_ok() {
                    f.0.push(out);
                }
            }
            return;
        }
        Request::Pcm => {
            shared.tap.add_reader(out);
            return;
        }
        Request::Ctl(ctl) => match shared.spirc.lock().ok().and_then(|s| s.clone()) {
            None => "err not connected".to_owned(),
            Some(spirc) => match command(&spirc, ctl) {
                Ok(()) => "ok".to_owned(),
                Err(e) => format!("err {e}"),
            },
        },
    };
    let _ = out.write_all(format!("{reply}\n").as_bytes());
}

fn command(spirc: &Spirc, ctl: Ctl) -> Result<(), librespot_core::Error> {
    match ctl {
        Ctl::Play => spirc.play(),
        Ctl::Pause => spirc.pause(),
        Ctl::Next => spirc.next(),
        Ctl::Prev => spirc.prev(),
        Ctl::Seek(ms) => spirc.set_position_ms(ms.min(u64::from(u32::MAX)) as u32),
        Ctl::Volume(pct) => spirc.set_volume(volume(pct)),
    }
}

/// A volume in percent as librespot's 0..=65535.
fn volume(pct: u8) -> u16 {
    (u32::from(pct.min(100)) * u32::from(u16::MAX) / 100) as u16
}

/// Connects the device and plays; when Spotify ends the session, connects again.
async fn play(name: String, shared: Arc<Shared>, media: Option<std::sync::mpsc::Sender<media::Update>>) {
    let Some(dir) = builtin::credentials_dir() else { return };
    let cache = match Cache::new(Some(&dir), None, None, None) {
        Ok(c) => c,
        Err(e) => return log(&format!("cache: {e}")),
    };
    while !shared.quit.load(Ordering::SeqCst) {
        let Some(credentials) = cache.credentials() else {
            return log("no saved login: start spotlite to log the player in");
        };
        let session = Session::new(SessionConfig::default(), Some(cache.clone()));
        let mixer = match mixer::find(Some(mixer::softmixer::SoftMixer::NAME)).map(|open| open(MixerConfig::default())) {
            Some(Ok(m)) => m,
            _ => return log("no soft volume mixer"),
        };
        let tap = shared.tap.clone();
        let config = PlayerConfig { bitrate: Bitrate::Bitrate320, normalisation: true, ..Default::default() };
        let player = Player::new(config, session.clone(), mixer.get_soft_volume(), move || {
            let output = audio_backend::find(None).expect("an audio output");
            Box::new(TapSink::new(output(None, OUTPUT_FORMAT), tap))
        });
        let events = tokio::spawn(follow(player.get_player_event_channel(), shared.clone(), media.clone()));
        let connect = ConnectConfig {
            name: name.clone(),
            device_type: DeviceType::Computer,
            initial_volume: volume(60),
            ..Default::default()
        };
        match Spirc::new(connect, session.clone(), credentials, player, mixer).await {
            Ok((spirc, task)) => {
                log("connected");
                if let Ok(mut s) = shared.spirc.lock() {
                    *s = Some(Arc::new(spirc));
                }
                task.await;
                log("disconnected");
            }
            Err(e) => log(&format!("cannot connect: {e}")),
        }
        if let Ok(mut s) = shared.spirc.lock() {
            *s = None;
        }
        events.abort();
        session.shutdown();
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// The song now playing, as the UI is told of it.
struct Playing {
    uri: String,
    duration_ms: u32,
    name: String,
    artists: Vec<String>,
    album: String,
    cover: String,
}

impl Playing {
    fn new(item: &AudioItem) -> Self {
        let (artists, album) = match &item.unique_fields {
            UniqueFields::Track { artists, album, .. } => (artists.iter().map(|a| a.name.clone()).collect(), album.clone()),
            UniqueFields::Episode { show_name, .. } => (vec![show_name.clone()], show_name.clone()),
            UniqueFields::Local { artists, album, .. } => {
                (artists.iter().cloned().collect(), album.clone().unwrap_or_default())
            }
        };
        let cover = item.covers.iter().max_by_key(|c| c.width).map(|c| c.url.clone()).unwrap_or_default();
        Playing { uri: item.uri.clone(), duration_ms: item.duration_ms, name: item.name.clone(), artists, album, cover }
    }

    fn song(&self) -> Song<'_> {
        Song {
            uri: &self.uri,
            duration_ms: self.duration_ms,
            name: &self.name,
            artists: &self.artists,
            album: &self.album,
            cover: &self.cover,
        }
    }
}

/// Tells the followers and the system's media controls of each song change, play and pause.
async fn follow(mut events: PlayerEventChannel, shared: Arc<Shared>, media: Option<std::sync::mpsc::Sender<media::Update>>) {
    let mut song: Option<Playing> = None;
    let mut playing = false;
    let tell = |update: media::Update| {
        if let Some(m) = &media {
            let _ = m.send(update);
        }
    };
    while let Some(event) = events.recv().await {
        match event {
            PlayerEvent::TrackChanged { audio_item } => {
                let s = Playing::new(&audio_item);
                tell(media::Update::Song {
                    title: s.name.clone(),
                    artists: s.artists.join(", "),
                    album: s.album.clone(),
                    cover: s.cover.clone(),
                    duration_ms: s.duration_ms,
                });
                song = Some(s);
            }
            PlayerEvent::Playing { .. } => playing = true,
            PlayerEvent::Paused { .. } | PlayerEvent::Stopped { .. } | PlayerEvent::SessionDisconnected { .. } => {
                playing = false
            }
            _ => continue,
        }
        tell(media::Update::Playing(playing));
        shared.publish(proto::follow_line(playing, song.as_ref().map(|s| s.song()).as_ref()));
    }
}

#[cfg(test)]
mod tests {
    /// A bad token is refused with an error (not a panic). Needs the network:
    /// `cargo test --features player -- --ignored`.
    #[test]
    #[ignore]
    fn save_login_reports_a_bad_token() {
        let dir = std::env::temp_dir().join(format!("spotlite-login-test-{}", std::process::id()));
        let result = super::save_login(&dir, "not-a-token");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_err());
    }

    #[test]
    fn volume_in_percent() {
        assert_eq!(super::volume(0), 0);
        assert_eq!(super::volume(100), u16::MAX);
        assert_eq!(super::volume(250), u16::MAX);
        assert_eq!(super::volume(50), 32767);
    }
}
