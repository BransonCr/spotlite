//! The spotifyd engine: the spotifyd user service (systemd), its MPRIS player (via
//! `playerctl`) and its sound (via `parec`).

use super::{Closer, Stream};
use crate::control::Ctl;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// MPRIS name prefix of spotifyd (the full name has an instance suffix).
const PLAYER: &str = "spotifyd";

/// Sample rate of the sound for the visualizer.
const RATE: usize = 22_050;

/// What `follow` asks playerctl for, one line per change: status, track id, length (µs),
/// title, artists, album, cover.
const FOLLOW_FORMAT: &str =
    "{{status}}\t{{mpris:trackid}}\t{{mpris:length}}\t{{xesam:title}}\t{{xesam:artist}}\t{{xesam:album}}\t{{mpris:artUrl}}";

/// Starts the spotifyd user service if it is not running (after `Q` stopped it). Does not wait.
/// True when it was not running and was asked to start.
pub fn start() -> bool {
    let running = Command::new("systemctl")
        .args(["--user", "--quiet", "is-active", "spotifyd"])
        .status()
        .is_ok_and(|s| s.success());
    if !running {
        let _ = Command::new("systemctl").args(["--user", "--no-block", "start", "spotifyd"]).status();
    }
    !running
}

/// How long spotifyd runs (None: it does not run).
pub fn age() -> Option<Duration> {
    let out = Command::new("systemctl").args(["--user", "show", "spotifyd", "-p", "MainPID", "--value"]).output().ok()?;
    let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().ok().filter(|p| *p > 0)?;
    let out = Command::new("ps").args(["-o", "etimes=", "-p", &pid.to_string()]).output().ok()?;
    let secs: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

pub fn stop() {
    let _ = Command::new("systemctl").args(["--user", "stop", "spotifyd"]).status();
}

/// True if playerctl is installed. spotifyd may start later; until it shows up on D-Bus,
/// commands fall back to the Web API.
pub fn control_available() -> bool {
    Command::new("playerctl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn run(ctl: Ctl) -> bool {
    Command::new("playerctl")
        .args(["-p", PLAYER])
        .args(ctl.args())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `playerctl --follow`: if spotifyd is not there yet, or restarts, playerctl waits for it.
pub fn follow() -> Option<Stream> {
    let mut child = Command::new("playerctl")
        .args(["-p", PLAYER, "--follow", "metadata", "--format", FOLLOW_FORMAT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    Some(Stream { reader: Box::new(out), closer: Closer::Child(child) })
}

/// `parec` records only spotifyd's own stream (its PulseAudio/PipeWire sink input), so other
/// sounds on the machine do not show.
pub fn pcm() -> Option<Stream> {
    let stream = spotifyd_stream()?;
    let mut child = Command::new("parec")
        .arg(format!("--monitor-stream={stream}"))
        .args(["--raw", "--format=s16le", "--channels=1", "--latency-msec=20"])
        .arg(format!("--rate={RATE}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    Some(Stream { reader: Box::new(out), closer: Closer::Child(child) })
}

/// Index of spotifyd's sink input, from `pactl` (works with pipewire-pulse too).
fn spotifyd_stream() -> Option<u32> {
    let out = Command::new("pactl").args(["--format=json", "list", "sink-inputs"]).stderr(Stdio::null()).output().ok()?;
    let inputs: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    inputs.as_array()?.iter().find_map(|i| {
        let props = &i["properties"];
        let ours = props["application.process.binary"] == "spotifyd" || props["application.name"] == "spotifyd";
        ours.then(|| i["index"].as_u64()).flatten().map(|n| n as u32)
    })
}

/// spotifyd's saved librespot login (written by `spotifyd authenticate`).
#[cfg_attr(not(feature = "client"), allow(dead_code))]
pub fn credentials_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("spotifyd"))
}
