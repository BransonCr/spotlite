//! Fast player control. When spotlite's own spotifyd is the active device, play/pause, skip,
//! seek and volume go straight to it over MPRIS (D-Bus, via `playerctl`): about 8 ms instead of
//! 250+ ms for a Web API round trip. Anything else, or any MPRIS failure, goes to the API thread.

use crate::worker::{Cmd, Kind, Msg};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::thread;

/// MPRIS name prefix of spotifyd (the full name has an instance suffix).
const PLAYER: &str = "spotifyd";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ctl {
    Play,
    Pause,
    Next,
    Prev,
    Seek(u64),
    Volume(u8),
}

impl Ctl {
    /// playerctl arguments for this command.
    pub fn args(self) -> Vec<String> {
        match self {
            Ctl::Play => vec!["play".into()],
            Ctl::Pause => vec!["pause".into()],
            Ctl::Next => vec!["next".into()],
            Ctl::Prev => vec!["previous".into()],
            Ctl::Seek(ms) => vec!["position".into(), format!("{:.3}", ms as f64 / 1000.0)],
            Ctl::Volume(pct) => vec!["volume".into(), format!("{:.2}", pct.min(100) as f64 / 100.0)],
        }
    }

    /// The same command for the Web API thread.
    pub fn fallback(self) -> Cmd {
        match self {
            Ctl::Play | Ctl::Pause => Cmd::Toggle,
            Ctl::Next => Cmd::Next,
            Ctl::Prev => Cmd::Prev,
            Ctl::Seek(ms) => Cmd::Seek(ms),
            Ctl::Volume(pct) => Cmd::Volume(pct),
        }
    }

    fn kind(self) -> Kind {
        match self {
            Ctl::Volume(_) => Kind::Volume,
            _ => Kind::Player,
        }
    }
}

/// True if playerctl is installed. spotifyd may start later; until it shows up on D-Bus,
/// commands fall back to the Web API.
pub fn available() -> bool {
    Command::new("playerctl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn run(ctl: Ctl) -> bool {
    Command::new("playerctl")
        .args(["-p", PLAYER])
        .args(ctl.args())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Starts the control thread. Each command ends with one `Msg::Done`, from here or (after a
/// fallback) from the API thread.
pub fn spawn(msg_tx: Sender<Msg>, api_tx: Sender<Cmd>) -> Sender<Ctl> {
    let (tx, rx) = channel::<Ctl>();
    thread::Builder::new()
        .name("spotlite-ctl".into())
        .spawn(move || {
            for ctl in rx {
                if run(ctl) {
                    if msg_tx.send(Msg::Done(ctl.kind())).is_err() {
                        return;
                    }
                } else if api_tx.send(ctl.fallback()).is_err() {
                    return;
                }
            }
        })
        .expect("cannot start the control thread");
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playerctl_arguments() {
        assert_eq!(Ctl::Seek(65_500).args(), ["position", "65.500"]);
        assert_eq!(Ctl::Volume(42).args(), ["volume", "0.42"]);
        assert_eq!(Ctl::Volume(150).args(), ["volume", "1.00"]);
        assert_eq!(Ctl::Prev.args(), ["previous"]);
        assert!(matches!(Ctl::Seek(5).fallback(), Cmd::Seek(5)));
    }
}
