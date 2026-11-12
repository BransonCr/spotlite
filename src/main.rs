mod api;
mod app;
mod auth;
mod client;
mod config;
mod control;
mod folders;
mod genre;
mod genres;
mod images;
mod input;
mod keys;
mod liquid;
mod log;
mod pages;
mod palette;
mod card;
mod radio;
mod samples;
mod sidebar;
mod state;
mod store;
mod theme;
mod ui;
mod undo;
mod viz;
mod worker;

use anyhow::{bail, Result};
use api::Spotify;
use config::Config;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyEventKind,
    MouseEventKind,
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
  spotlite radio URI  list the radio for a song or artist (spotify:track:... or spotify:artist:...)
  spotlite theme              list the themes (* = in use)
  spotlite theme set NAME     use a theme (a running spotlite changes within a second)
  spotlite theme file NAME    print the theme's file, copying a built-in one there first
  spotlite theme new NAME [FROM]  make a theme from FROM (default: the one in use)
  spotlite theme check NAME   check a theme file for mistakes
  spotlite keys       list every action, its name for [keys] in config.toml, and its keys
  spotlite toggle     play/pause
  spotlite next       next track
  spotlite prev       previous track";

fn main() -> Result<()> {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        None => {
            // Not `?`: Rust's own report panics if the terminal is gone (window closed).
            if let Err(e) = run_tui() {
                use std::io::Write;
                let _ = writeln!(std::io::stderr(), "Error: {e:#}");
                std::process::exit(1);
            }
            Ok(())
        }
        Some("auth") => {
            auth::login(&Config::load()?)?;
            // Another account may have logged in: ask Spotify who it is again.
            let _ = std::fs::remove_file(config::user_path());
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
        Some("theme") => theme_cmd(std::env::args().skip(2).collect()),
        Some("keys") => {
            let custom = Config::load().map(|c| c.keys).unwrap_or_default();
            let (keys, problems) = keys::Keys::new(&custom);
            for p in problems {
                println!("! {p}");
            }
            for section in keys::SECTIONS {
                println!("\n{section}");
                for info in keys::ALL.iter().filter(|i| i.section == section) {
                    println!("  {:<16} {:<18} {}", info.name, keys.show_all(info.act), info.help);
                }
            }
            println!("\nChange a key in {} under [keys], e.g. next = \"N\" or like = [\"f\", \"ctrl-l\"].", config::config_path().display());
            Ok(())
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
            let seed = std::env::args().nth(2).ok_or_else(|| anyhow::anyhow!("usage: spotlite radio <track-or-artist-uri>"))?;
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

/// `spotlite theme ...`: themes from a shell, for scripts and AI agents.
fn theme_cmd(args: Vec<String>) -> Result<()> {
    let mut saved = state::load();
    let current = saved.theme.clone().unwrap_or_else(|| theme::DEFAULT.to_owned());
    let arg = |i: usize| args.get(i).map(String::as_str);
    match (arg(0), arg(1)) {
        (None | Some("list"), _) => {
            for (name, text) in theme::all() {
                let mark = if name == current { "*" } else { " " };
                let state = match theme::parse(&text) {
                    Err(e) => format!("  (broken: {e})"),
                    Ok(_) if theme::file(&name).exists() => format!("  ({})", theme::file(&name).display()),
                    Ok(_) => String::new(),
                };
                println!("{mark} {name}{state}");
            }
            println!("\nThemes folder: {}", theme::dir().display());
            Ok(())
        }
        (Some("set"), Some(name)) => {
            theme::load(name).map_err(anyhow::Error::msg)?;
            saved.theme = Some(name.to_owned());
            state::save(&saved)?;
            println!("Theme set to {name}. A running spotlite changes within a second.");
            Ok(())
        }
        (Some("file"), Some(name)) => {
            let path = theme::file(name);
            if !path.exists() {
                let (_, text) = theme::all()
                    .into_iter()
                    .find(|(n, _)| n == name)
                    .ok_or_else(|| anyhow::anyhow!("no theme “{name}”; `spotlite theme new {name}` makes one"))?;
                std::fs::create_dir_all(theme::dir())?;
                std::fs::write(&path, text)?;
            }
            println!("{}", path.display());
            Ok(())
        }
        (Some("new"), Some(name)) => {
            let from = arg(2).unwrap_or(&current);
            let path = theme::file(name);
            if path.exists() {
                bail!("{} already exists", path.display());
            }
            let (_, text) = theme::all()
                .into_iter()
                .find(|(n, _)| n == from)
                .ok_or_else(|| anyhow::anyhow!("no theme “{from}”"))?;
            // Keep the comments: they say what each key does.
            let text = match text.split_once('\n') {
                Some((first, rest)) if first.starts_with('#') => format!("# spotlite theme “{name}” (made from {from}).\n{rest}"),
                _ => text,
            };
            std::fs::create_dir_all(theme::dir())?;
            std::fs::write(&path, text)?;
            println!("{}", path.display());
            Ok(())
        }
        (Some("check"), Some(name)) => {
            theme::load(name).map_err(anyhow::Error::msg)?;
            println!("{name}: OK");
            Ok(())
        }
        _ => bail!("usage: spotlite theme [list | set NAME | file NAME | new NAME [FROM] | check NAME]"),
    }
}

fn run_tui() -> Result<()> {
    let cfg = Config::load()?;
    let cfg_keys = cfg.keys.clone();
    let api = Spotify::new(cfg)?;
    start_spotifyd();
    let can_like = api.has_scope("user-library-modify");
    let can_edit = api.has_scope("playlist-modify-private");
    let can_taste = api.has_scope("user-top-read") && api.has_scope("user-read-recently-played");
    let device_name = api.config().device_name.clone();
    let (tx, msg_tx, rx) = worker::spawn(api);
    let mut terminal = ratatui::init();
    // ratatui's panic hook restores the screen; also give the mouse back to the terminal.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        // A note for a bug report, beside the saved state (the release build stops at once).
        let path = state::path().with_file_name("crash.log");
        let note = format!(
            "spotlite {} crashed at unix {}: {info}\n{}\n",
            env!("CARGO_PKG_VERSION"),
            api::unix_now(),
            std::backtrace::Backtrace::force_capture()
        );
        let _ = std::fs::write(&path, note);
        hook(info);
        eprintln!("A note about this crash is in {}", path.display());
    }));
    // Paste arrives as one piece of text (not as keys that would run actions).
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    // Ask the terminal which graphics protocol it speaks (kitty in ghostty); half blocks if none.
    let images = if std::env::var_os("SPOTLITE_NO_IMAGES").is_some() {
        images::Images::disabled()
    } else {
        images::Images::new(Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks()))
    };
    let mut app = app::App::new(tx.clone(), can_like, images);
    app.viz_on = std::env::var_os("SPOTLITE_VIZ").is_some();
    let (keys, problems) = keys::Keys::new(&cfg_keys);
    app.keys = keys;
    if let Some(p) = problems.first() {
        app.error(format!("{p} (config.toml){}", if problems.len() > 1 { format!(", and {} more", problems.len() - 1) } else { String::new() }));
    }
    app.can_edit = can_edit;
    if let Some(tree) = folders::load_cached() {
        app.set_folders(tree);
    }
    let saved = state::load();
    // SPOTLITE_THEME tries a theme for this run only (it is not saved). NO_COLOR asks for no
    // colour: the mono theme.
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let wanted = theme::env_override().or(no_color.then(|| "mono".to_owned())).or(saved.theme.clone());
    if let Some(name) = &wanted {
        if let Err(e) = theme::apply(name) {
            app.error(e);
        }
    }
    app.restore(saved);
    if !can_taste {
        app.info("Run `spotlite auth` again to turn on Recent and a stronger discover.");
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
    // Closing the terminal window sends SIGHUP (a shutdown, SIGTERM): stop the loop cleanly,
    // so the state below is saved instead of lost.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGHUP, signal_hook::consts::SIGTERM] {
        let _ = signal_hook::flag::register(sig, stop.clone());
    }
    watchdog(stop.clone());
    let trial = theme::env_override().is_some();
    let mut saved_json = String::new();
    let mut save_at = std::time::Instant::now();
    let result = (|| -> Result<()> {
        let mut last_draw = std::time::Instant::now();
        let mut shown_title = String::new();
        // Something on screen changed by input (or it is the first frame): draw.
        let mut input = true;
        // The program that started spotlite (the terminal, or a shell in it). If it goes away
        // the terminal is gone too, even when no SIGHUP came (a shell in between does not pass
        // it on): stop, instead of spinning on a dead terminal.
        let parent = std::os::unix::process::parent_id();
        while !app.quit && !stop.load(std::sync::atomic::Ordering::Relaxed) {
            if std::os::unix::process::parent_id() != parent {
                break;
            }
            // Every 5 s, save what changed: a crash or a kill loses at most a few seconds.
            if !trial && save_at.elapsed() >= Duration::from_secs(5) {
                save_at = std::time::Instant::now();
                let st = app.saved_state();
                if let Ok(json) = serde_json::to_string(&st) {
                    if json != saved_json && state::save(&st).is_ok() {
                        saved_json = json;
                    }
                }
            }
            let mut dirty = std::mem::take(&mut input);
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        app.on_msg(msg);
                        dirty = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => anyhow::bail!("the API thread stopped"),
                }
            }
            dirty |= app.images.poll();
            dirty |= app.tick();
            let now = std::time::Instant::now();
            let period = app.frame_period(viz_frame);
            dirty |= now >= last_draw + period;
            if dirty {
                draw(&mut terminal, &mut app)?;
                last_draw = std::time::Instant::now();
                // The window title follows the song (task bars and window lists show it).
                let title = app.window_title();
                if title != shown_title {
                    let _ = execute!(std::io::stdout(), ratatui::crossterm::terminal::SetTitle(&title));
                    shown_title = title;
                }
            }
            // Sleep until the next frame is due (1 s idle, faster while music or the visualizer
            // moves). While a request or a picture is open, wake often so its result shows at
            // once. Input always wakes us.
            let due = (last_draw + period).saturating_duration_since(std::time::Instant::now());
            let wait = if app.busy() { due.min(Duration::from_millis(30)) } else { due };
            if event::poll(wait)? {
                // Take every event that waits, then draw one time: a wheel burst or a held
                // key does not queue up frames.
                for _ in 0..256 {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            app.on_key(key);
                            input = true;
                        }
                        // Pointer motion draws only when it changes something (a hover list).
                        Event::Mouse(m) if m.kind == MouseEventKind::Moved => {
                            if app.on_hover(ratatui::layout::Position::new(m.column, m.row)) {
                                input = true;
                            }
                        }
                        Event::Mouse(m) => {
                            app.on_mouse(m);
                            input = true;
                        }
                        Event::Paste(text) => {
                            app.on_paste(&text);
                            input = true;
                        }
                        Event::Resize(..) => input = true,
                        _ => {}
                    }
                    if app.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();
    app.viz.stop();
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    // A trial run (SPOTLITE_THEME) saves nothing, so it never replaces your tabs or theme.
    if theme::env_override().is_none() {
        if let Err(e) = state::save(&app.saved_state()) {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "spotlite: could not save the open tabs: {e:#}");
        }
    }
    if app.stop_services {
        stop_spotifyd();
    }
    result
}

/// Ends spotlite if it does not stop by itself within 1.5 s of being told to (SIGHUP, SIGTERM)
/// or of losing the program that started it (its terminal closed). When the terminal goes away
/// during a wait for input, crossterm 0.28 keeps reading the dead terminal in a loop that never
/// returns, at full CPU; the main loop then never sees the signal. The state was saved at most
/// 5 s before.
fn watchdog(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let parent = std::os::unix::process::parent_id();
    let _ = std::thread::Builder::new().name("spotlite-watch".into()).spawn(move || {
        let mut since: Option<std::time::Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let told = stop.load(std::sync::atomic::Ordering::Relaxed) || std::os::unix::process::parent_id() != parent;
            if told {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                let at = *since.get_or_insert_with(std::time::Instant::now);
                if at.elapsed() >= Duration::from_millis(1500) {
                    std::process::exit(0);
                }
            }
        }
    });
}

/// Draws one frame inside a synchronized update (DEC mode 2026): the terminal shows the whole
/// frame at once, so covers and the visualizer do not tear. Terminals without it ignore it.
fn draw(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> Result<()> {
    use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
    let _ = execute!(std::io::stdout(), BeginSynchronizedUpdate);
    let started = std::time::Instant::now();
    let drawn = terminal.draw(|f| ui::draw(f, app));
    let took = started.elapsed();
    if took > Duration::from_millis(8) && log::on() {
        log::line(&format!("slow frame: {} ms ({:?})", took.as_millis(), app.view()));
    }
    let _ = execute!(std::io::stdout(), EndSynchronizedUpdate);
    drawn?;
    Ok(())
}

/// Starts the spotifyd user service if it is not running (after `Q` stopped it). Does not wait.
/// True when it was not running and was asked to start.
pub fn start_spotifyd() -> bool {
    let running = std::process::Command::new("systemctl")
        .args(["--user", "--quiet", "is-active", "spotifyd"])
        .status()
        .is_ok_and(|s| s.success());
    if !running {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "--no-block", "start", "spotifyd"])
            .status();
    }
    !running
}

/// `Q`: stop spotifyd too, so no part of spotlite keeps running or playing.
fn stop_spotifyd() {
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", "spotifyd"])
        .status();
}
