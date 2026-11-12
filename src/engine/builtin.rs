//! The built-in engine, UI side: starts `spotlite daemon` in the background and talks to it
//! over its local socket (see `proto`).

use super::proto::{self, Request};
use super::{Closer, Stream};
use crate::control::Ctl;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn state_dir() -> PathBuf {
    dirs::state_dir().or_else(dirs::cache_dir).unwrap_or_else(|| PathBuf::from(".")).join("spotlite")
}

/// The daemon's log (it has no terminal).
pub fn log_path() -> PathBuf {
    state_dir().join("daemon.log")
}

/// The secret each connection to the daemon starts with, so other programs cannot drive it.
pub fn token_path() -> PathBuf {
    state_dir().join("daemon.token")
}

/// The daemon's librespot login (`credentials.json`).
pub fn credentials_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("spotlite").join("librespot"))
}

/// True when the daemon has a saved login.
pub fn logged_in() -> bool {
    credentials_dir().is_some_and(|d| d.join("credentials.json").is_file())
}

/// A connection to the daemon, after the token. None if it does not run.
fn connect(port: u16, timeout: Duration) -> Option<TcpStream> {
    let token = std::fs::read_to_string(token_path()).ok()?;
    let mut s = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(300)).ok()?;
    let _ = s.set_nodelay(true);
    s.set_read_timeout(Some(timeout)).ok()?;
    s.write_all(format!("{}\n", token.trim()).as_bytes()).ok()?;
    Some(s)
}

/// Sends one request and reads its answer line.
fn ask(port: u16, req: Request) -> Option<String> {
    let mut s = connect(port, Duration::from_secs(2))?;
    s.write_all(format!("{}\n", proto::line(req)).as_bytes()).ok()?;
    let mut answer = String::new();
    BufReader::new(s).read_line(&mut answer).ok()?;
    Some(answer.trim().to_owned())
}

pub fn age(port: u16) -> Option<Duration> {
    let answer = ask(port, Request::Age)?;
    let secs: u64 = answer.strip_prefix("ok ")?.parse().ok()?;
    Some(Duration::from_secs(secs))
}

/// Starts `spotlite daemon` in the background if it does not answer.
pub fn start(port: u16) -> bool {
    if age(port).is_some() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut cmd = Command::new(exe);
    cmd.arg("daemon");
    crate::platform::detach(&mut cmd);
    cmd.spawn().is_ok()
}

pub fn stop(port: u16) {
    let _ = ask(port, Request::Quit);
}

pub fn run(port: u16, ctl: Ctl) -> bool {
    ask(port, Request::Ctl(ctl)).as_deref() == Some("ok")
}

/// A stream that lasts until the daemon or the reader ends it.
fn open(port: u16, req: Request) -> Option<Stream> {
    let mut s = connect(port, Duration::from_secs(2))?;
    s.write_all(format!("{}\n", proto::line(req)).as_bytes()).ok()?;
    s.set_read_timeout(None).ok()?;
    let reader = s.try_clone().ok()?;
    Some(Stream { reader: Box::new(reader), closer: Closer::Socket(s) })
}

pub fn follow(port: u16) -> Option<Stream> {
    open(port, Request::Follow)
}

pub fn pcm(port: u16) -> Option<Stream> {
    open(port, Request::Pcm)
}
