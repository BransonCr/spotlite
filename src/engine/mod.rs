//! The player engine: the program that plays the audio and is spotlite's Spotify Connect
//! device. Two kinds, chosen with `backend` in config.toml:
//!
//! - `spotifyd` (default on Linux): the spotifyd user service. Started and stopped with
//!   systemctl, driven over MPRIS (playerctl), its sound read with parec.
//! - `builtin` (default on Windows): spotlite's own player, `spotlite daemon`, a librespot
//!   process that spotlite starts in the background. Driven over a local socket.
//!
//! Both give the same things to the rest of spotlite: start, stop and age; fast commands; a
//! line for each player change (in playerctl's `--follow` format, see `control::parse_follow`);
//! and the sound as 22 050 Hz mono s16le bytes for the visualizer.

use crate::config::Config;
use crate::control::Ctl;
use std::io::Read;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Child;
use std::sync::OnceLock;
use std::time::Duration;

#[cfg(builtin_player)]
pub mod builtin;
pub mod proto;
pub mod spotifyd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Spotifyd,
    Builtin,
}

impl Backend {
    /// What config.toml says, else this system's default. `builtin` in a build without the
    /// player falls back to spotifyd.
    pub fn from_config(value: Option<&str>) -> Result<Backend, String> {
        let wanted = match value {
            None => return Ok(Backend::default()),
            Some("spotifyd") => Backend::Spotifyd,
            Some("builtin") => Backend::Builtin,
            Some(other) => return Err(format!("backend = \"{other}\": write \"spotifyd\" or \"builtin\"")),
        };
        if wanted == Backend::Builtin && !cfg!(builtin_player) {
            return Err("backend = \"builtin\" needs a build with the player (cargo build --features player)".into());
        }
        Ok(wanted)
    }
}

impl Default for Backend {
    fn default() -> Self {
        if cfg!(windows) { Backend::Builtin } else { Backend::Spotifyd }
    }
}

struct Engine {
    #[cfg_attr(not(builtin_player), allow(dead_code))]
    backend: Backend,
    /// Port of the built-in player's control socket on 127.0.0.1.
    #[cfg_attr(not(builtin_player), allow(dead_code))]
    port: u16,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

/// Chooses the engine from the config. Call once, before anything below; without it the
/// system's default is used. A bad `backend` value is returned as an error (the default is
/// used then).
pub fn init(cfg: &Config) -> Result<(), String> {
    // SPOTLITE_BACKEND tries an engine for one run (tools/windows-test.sh uses it).
    let wanted = std::env::var("SPOTLITE_BACKEND").ok().or_else(|| cfg.backend.clone());
    let (backend, problem) = match Backend::from_config(wanted.as_deref()) {
        Ok(b) => (b, None),
        Err(e) => (Backend::default(), Some(e)),
    };
    let _ = ENGINE.set(Engine { backend, port: cfg.daemon_port });
    problem.map_or(Ok(()), Err)
}

#[cfg_attr(not(builtin_player), allow(dead_code))]
fn get() -> &'static Engine {
    ENGINE.get_or_init(|| Engine { backend: Backend::default(), port: crate::config::DEFAULT_DAEMON_PORT })
}

/// Calls the built-in engine, or the spotifyd one. In a build without the player, always
/// the spotifyd one (`init` does not choose builtin there).
macro_rules! pick {
    ($builtin:expr, $spotifyd:expr) => {{
        #[cfg(builtin_player)]
        {
            if get().backend == Backend::Builtin {
                $builtin
            } else {
                $spotifyd
            }
        }
        #[cfg(not(builtin_player))]
        {
            $spotifyd
        }
    }};
}

/// Starts the engine if it does not run. Does not wait. True when it did not run and was
/// asked to start.
pub fn start() -> bool {
    pick!(builtin::start(get().port), spotifyd::start())
}

/// How long the engine runs (None: it does not run).
pub fn age() -> Option<Duration> {
    pick!(builtin::age(get().port), spotifyd::age())
}

/// Stops the engine (`Q`): nothing of spotlite keeps running or playing.
pub fn stop() {
    pick!(builtin::stop(get().port), spotifyd::stop())
}

/// True if fast commands can be sent at all. The engine may start later; until then commands
/// fail and go to the Web API.
pub fn control_available() -> bool {
    pick!(true, spotifyd::control_available())
}

/// Sends a fast command. False if it did not work (the caller then uses the Web API).
pub fn run(ctl: Ctl) -> bool {
    pick!(builtin::run(get().port, ctl), spotifyd::run(ctl))
}

/// The engine's player changes, one line each, until the engine stops. None if it cannot be
/// followed now (try again later).
pub fn follow() -> Option<Stream> {
    pick!(builtin::follow(get().port), spotifyd::follow())
}

/// The engine's sound: 22 050 Hz mono s16le. None while it plays nothing.
pub fn pcm() -> Option<Stream> {
    pick!(builtin::pcm(get().port), spotifyd::pcm())
}

/// Where the engine keeps its librespot login (`credentials.json` is in it), for the client
/// protocol (folders, radio).
#[cfg_attr(not(feature = "client"), allow(dead_code))]
pub fn credentials_dir() -> Option<PathBuf> {
    pick!(builtin::credentials_dir(), spotifyd::credentials_dir())
}

/// Logs the engine in to Spotify if it has no saved login (the built-in player; spotifyd has
/// its own `spotifyd authenticate`). `again`: log in even with a saved login. Run it before
/// the terminal UI starts: it prints to the terminal and opens the browser.
pub fn ensure_login(again: bool) -> anyhow::Result<()> {
    #[cfg(builtin_player)]
    if get().backend == Backend::Builtin && (again || !builtin::logged_in()) {
        println!("Log in to Spotify for spotlite's player (the browser opens)…");
        crate::daemon::login()?;
        println!("spotlite's player is logged in.");
    }
    let _ = again;
    Ok(())
}

/// The engine's name in messages.
pub fn name() -> &'static str {
    pick!("spotlite's player", "spotifyd")
}

/// What to do when Spotify does not see the device.
pub fn hint_no_device() -> String {
    pick!(
        format!("spotlite's player did not start. See {}", builtin::log_path().display()),
        "spotifyd not found. Start it: systemctl --user start spotifyd".to_owned()
    )
}

/// How to log the engine in to Spotify (for the client protocol).
#[cfg_attr(not(feature = "client"), allow(dead_code))]
pub fn hint_login() -> &'static str {
    pick!("start spotlite to log in", "run `spotifyd authenticate`")
}

/// What to do when the device stays missing after a start.
pub fn hint_check() -> String {
    pick!(
        format!("Check {}", builtin::log_path().display()),
        "Check spotifyd: `systemctl --user status spotifyd`".to_owned()
    )
}

/// A byte stream from the engine (player lines or sound), and how to end it.
pub struct Stream {
    pub reader: Box<dyn Read + Send>,
    pub closer: Closer,
}

/// Ends a stream: the child process that writes it, or the socket it comes over.
pub enum Closer {
    Child(Child),
    #[cfg_attr(not(builtin_player), allow(dead_code))]
    Socket(TcpStream),
}

impl Closer {
    pub fn close(&mut self) {
        match self {
            Closer::Child(c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            Closer::Socket(s) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_from_config() {
        assert_eq!(Backend::from_config(None), Ok(Backend::default()));
        assert_eq!(Backend::from_config(Some("spotifyd")), Ok(Backend::Spotifyd));
        assert!(Backend::from_config(Some("vlc")).is_err());
        assert_eq!(Backend::from_config(Some("builtin")).is_ok(), cfg!(builtin_player));
        assert_eq!(Backend::default() == Backend::Builtin, cfg!(windows));
    }
}
