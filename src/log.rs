//! A debug log, off unless `SPOTLITE_LOG` names a file. It shows what spotlite asks Spotify
//! (one line per request) and which frames were slow to draw, so a change can be measured.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

fn file() -> Option<&'static Mutex<std::fs::File>> {
    FILE.get_or_init(|| {
        let path = std::env::var_os("SPOTLITE_LOG")?;
        std::fs::File::options().create(true).append(true).open(path).ok().map(Mutex::new)
    })
    .as_ref()
}

/// True when the log is on (so callers can skip the work of making a line).
pub fn on() -> bool {
    file().is_some()
}

/// Adds a line: the time (unix seconds, with milliseconds), then `text`.
pub fn line(text: &str) {
    let Some(file) = file() else { return };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "{}.{:03} {text}", now.as_secs(), now.subsec_millis());
    }
}
