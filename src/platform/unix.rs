use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Nerd Font glyphs are on unless config.toml says otherwise: ghostty and kitty have them.
pub const NERD_DEFAULT: bool = true;

/// Sets `stop` when the terminal window closes (SIGHUP) or the system shuts down (SIGTERM), so
/// the main loop ends cleanly and saves the state.
pub fn on_close(stop: Arc<AtomicBool>) {
    for sig in [signal_hook::consts::SIGHUP, signal_hook::consts::SIGTERM] {
        let _ = signal_hook::flag::register(sig, stop.clone());
    }
}

/// The program that started spotlite (the terminal, or a shell in it). When it changes, the
/// terminal is gone.
pub fn parent_id() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}

/// Only the user can read the file (mode 600): it holds a login.
pub fn private_file(options: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

/// Makes `cmd` run on its own: in its own process group, with no terminal, so it keeps
/// running after spotlite and its terminal close.
#[cfg_attr(not(builtin_player), allow(dead_code))]
pub fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
}

/// A hidden window for the media controls: only Windows needs one.
#[cfg_attr(not(builtin_player), allow(dead_code))]
pub fn hidden_window() -> Option<*mut std::ffi::c_void> {
    None
}

/// Handles the window messages of this thread: only Windows has them.
#[cfg_attr(not(builtin_player), allow(dead_code))]
pub fn pump_messages() {}

/// Puts `text` on the clipboard (Wayland, `wl-copy`). Does nothing without wl-copy.
pub fn copy_text(text: &str) {
    if let Ok(mut child) = Command::new("wl-copy")
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || child.wait());
    }
}

/// A sans-serif font file for the share card, from fontconfig.
pub fn font_file(bold: bool) -> Result<PathBuf> {
    let pattern = if bold { "Inter,Noto Sans,DejaVu Sans,sans-serif:bold" } else { "Inter,Noto Sans,DejaVu Sans,sans-serif" };
    let out = Command::new("fc-match").args(["-f", "%{file}", pattern]).output().context("running fc-match (fontconfig)")?;
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}
