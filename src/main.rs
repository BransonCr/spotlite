mod api;
mod app;
mod auth;
mod config;
mod theme;
mod ui;
mod worker;

use anyhow::{bail, Result};
use api::Spotify;
use config::Config;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

const USAGE: &str = "\
spotlite - lightweight Spotify client (plays through spotifyd)

usage:
  spotlite            open the terminal UI
  spotlite auth       log in to Spotify (once)
  spotlite devices    list Spotify Connect devices
  spotlite toggle     play/pause
  spotlite next       next track
  spotlite prev       previous track";

fn main() -> Result<()> {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        None => run_tui(),
        Some("auth") => {
            auth::login(&Config::load()?)?;
            println!("Logged in. Token saved to {}", config::token_path().display());
            Ok(())
        }
        Some("devices") => {
            let mut api = Spotify::new(Config::load()?)?;
            let devices = api.devices()?;
            if devices.is_empty() {
                println!("No devices. Start spotifyd (systemctl --user start spotifyd).");
            }
            for d in devices {
                let active = if d.is_active { " (active)" } else { "" };
                println!("{}{active}", d.name);
            }
            Ok(())
        }
        Some(cmd @ ("toggle" | "next" | "prev")) => {
            let mut api = Spotify::new(Config::load()?)?;
            let device = api.find_device()?.and_then(|d| d.id);
            let device = device.as_deref();
            match cmd {
                "next" => api.next(device),
                "prev" => api.previous(device),
                _ => match api.playback()? {
                    Some(pb) if pb.is_playing => api.pause(device),
                    _ => api.play(device, None),
                },
            }
        }
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command '{other}'\n\n{USAGE}"),
    }
}

fn run_tui() -> Result<()> {
    let api = Spotify::new(Config::load()?)?;
    let can_like = api.has_scope("user-library-modify");
    let (tx, rx) = worker::spawn(api);
    let mut app = app::App::new(tx, can_like);
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        while !app.quit {
            loop {
                match rx.try_recv() {
                    Ok(msg) => app.on_msg(msg),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => anyhow::bail!("the API thread stopped"),
                }
            }
            app.tick();
            terminal.draw(|f| ui::draw(f, &mut app))?;
            // Idle: wake once a second to move the progress bar. While a request is
            // open, wake often so its reply shows at once. Keys always wake us.
            let wait = if app.busy() { Duration::from_millis(30) } else { Duration::from_secs(1) };
            if event::poll(wait)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}
