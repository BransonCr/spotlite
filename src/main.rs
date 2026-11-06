mod api;
mod app;
mod auth;
mod client;
mod config;
mod control;
mod folders;
mod images;
mod pages;
mod radio;
mod theme;
mod ui;
mod viz;
mod worker;

use anyhow::{bail, Result};
use api::Spotify;
use config::Config;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui_image::picker::Picker;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

const USAGE: &str = "\
spotlite - lightweight Spotify client (plays through spotifyd)

usage:
  spotlite            open the terminal UI
  spotlite auth       log in to Spotify (once)
  spotlite devices    list Spotify Connect devices
  spotlite folders    show your playlist folders
  spotlite radio URI  list song radio for a track (spotify:track:...)
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
        Some("folders") => {
            let tree = folders::fetch()?;
            folders::save_cache(&tree)?;
            fn show(entries: &[folders::Entry], depth: usize) {
                for e in entries {
                    match e {
                        folders::Entry::Folder { name, children } => {
                            println!("{:indent$}▸ {name} ({} playlists)", "", e.playlist_count(), indent = depth * 2);
                            show(children, depth + 1);
                        }
                        folders::Entry::Playlist(uri) => println!("{:indent$}{uri}", "", indent = depth * 2),
                    }
                }
            }
            show(&tree, 0);
            Ok(())
        }
        Some("radio") => {
            let seed = std::env::args().nth(2).ok_or_else(|| anyhow::anyhow!("usage: spotlite radio <track-uri>"))?;
            let (seed, tracks) = radio::fetch(&seed)?;
            if let Some(img) = seed.images.first() {
                println!("cover: {} ({:?} px)", img.url, img.width);
            }
            println!("{} - {} ({:?}), {} tracks:", seed.name, seed.artists.join(", "), seed.year, tracks.len());
            for t in tracks {
                println!(
                    "  {:<40} {:<28} {:>4} pop {:>3}{}",
                    t.name.chars().take(40).collect::<String>(),
                    t.artists.join(", ").chars().take(28).collect::<String>(),
                    t.year.map_or("-".into(), |y| y.to_string()),
                    t.popularity,
                    if t.explicit { " E" } else { "" }
                );
            }
            Ok(())
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
    start_spotifyd();
    let can_like = api.has_scope("user-library-modify");
    let can_edit = api.has_scope("playlist-modify-private");
    let device_name = api.config().device_name.clone();
    let (tx, msg_tx, rx) = worker::spawn(api);
    let mut terminal = ratatui::init();
    // ratatui's panic hook restores the screen; also give the mouse back to the terminal.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        hook(info);
    }));
    execute!(std::io::stdout(), EnableMouseCapture)?;
    // Ask the terminal which graphics protocol it speaks (kitty in ghostty); half blocks if none.
    let images = if std::env::var_os("SPOTLITE_NO_IMAGES").is_some() {
        images::Images::disabled()
    } else {
        images::Images::new(Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks()))
    };
    let mut app = app::App::new(tx.clone(), can_like, images);
    app.viz_on = std::env::var_os("SPOTLITE_VIZ").is_some();
    app.can_edit = can_edit;
    if let Some(tree) = folders::load_cached() {
        app.set_folders(tree);
    }
    // Refresh the folder tree in the background; the cached one shows meanwhile.
    if client::AVAILABLE {
        let folder_tx = msg_tx.clone();
        std::thread::Builder::new().name("spotlite-folders".into()).spawn(move || {
            if let Ok(tree) = folders::fetch() {
                let _ = folders::save_cache(&tree);
                let _ = folder_tx.send(worker::Msg::Folders(tree));
            }
        })?;
    }
    if control::available() {
        app = app.with_control(control::spawn(msg_tx.clone(), tx), device_name);
    }
    let fps: u64 = std::env::var("SPOTLITE_VIZ_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let viz_frame = Duration::from_millis(1000 / fps.clamp(1, 60));
    let result = (|| -> Result<()> {
        while !app.quit {
            loop {
                match rx.try_recv() {
                    Ok(msg) => app.on_msg(msg),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => anyhow::bail!("the API thread stopped"),
                }
            }
            app.images.poll();
            app.tick();
            terminal.draw(|f| ui::draw(f, &mut app))?;
            // Idle: wake once a second to move the progress bar. While a request or a picture
            // is open, wake often so its result shows at once; the visualizer runs at its fps.
            // Input always wakes us.
            let wait = if app.busy() {
                Duration::from_millis(30)
            } else if app.animating() {
                viz_frame
            } else {
                Duration::from_secs(1)
            };
            if event::poll(wait)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                    Event::Mouse(m) => app.on_mouse(m),
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    app.viz.stop();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    if app.stop_services {
        stop_spotifyd();
    }
    result
}

/// Starts the spotifyd user service if it is not running (after `Q` stopped it). Does not wait.
fn start_spotifyd() {
    let running = std::process::Command::new("systemctl")
        .args(["--user", "--quiet", "is-active", "spotifyd"])
        .status()
        .is_ok_and(|s| s.success());
    if !running {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "--no-block", "start", "spotifyd"])
            .status();
    }
}

/// `Q`: stop spotifyd too, so no part of spotlite keeps running or playing.
fn stop_spotifyd() {
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", "spotifyd"])
        .status();
}
