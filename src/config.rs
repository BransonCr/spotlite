use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// User settings, read from ~/.config/spotlite/config.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Client ID of your app on developer.spotify.com (PKCE needs no secret).
    pub client_id: String,
    /// Spotify Connect device to play on; must match `device_name` in spotifyd.conf (the
    /// built-in player uses it as its name).
    #[serde(default = "default_device_name")]
    pub device_name: String,
    /// Local port for the OAuth redirect (http://127.0.0.1:<port>/callback).
    #[serde(default = "default_redirect_port")]
    pub redirect_port: u16,
    /// Keys you set yourself: `[keys]` with `next = "n"` or `like = ["f", "ctrl-l"]` (see
    /// the names in `spotlite keys`).
    #[serde(default)]
    pub keys: std::collections::HashMap<String, crate::keys::Spec>,
    /// The terminal's font has Nerd Font glyphs (powerline arrows, the like heart). Default:
    /// true on Linux, false on Windows (Windows Terminal's own font has none).
    #[serde(default)]
    pub nerd_font: Option<bool>,
    /// The player engine: "spotifyd" (default on Linux) or "builtin" (spotlite's own player,
    /// default on Windows). See `engine`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Local port of the built-in player's control socket (127.0.0.1).
    #[serde(default = "default_daemon_port")]
    pub daemon_port: u16,
}

pub const DEFAULT_DAEMON_PORT: u16 = 8899;

fn default_daemon_port() -> u16 {
    DEFAULT_DAEMON_PORT
}

fn default_device_name() -> String {
    "spotlite".into()
}

fn default_redirect_port() -> u16 {
    8898
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotlite/config.toml")
}

/// The logged-in user (id), kept so /me is asked once per login.
pub fn user_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotlite/user.json")
}

pub fn token_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotlite/token.json")
}

/// The first run: no config.toml yet. Asks for the app's Client ID in the terminal and writes
/// the file, so nobody has to make it by hand. Does nothing when the file is there, or when
/// nobody can answer (no terminal).
pub fn first_run() -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    let path = config_path();
    if path.exists() || !std::io::stdin().is_terminal() {
        return Ok(());
    }
    println!("Welcome to spotlite. It needs the Client ID of your own Spotify app (one time):");
    println!("  1. Open https://developer.spotify.com/dashboard and log in.");
    println!("  2. Create an app. Redirect URI: http://127.0.0.1:{}/callback  API: Web API", default_redirect_port());
    println!("  3. Copy its Client ID.");
    loop {
        print!("Client ID: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line)? == 0 {
            anyhow::bail!("no Client ID given; write {} by hand (see README)", path.display());
        }
        let id = line.trim();
        if valid_client_id(id) {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, format!("client_id = \"{id}\"\n"))?;
            println!("Saved in {}", path.display());
            return Ok(());
        }
        println!("That is not a Client ID (32 letters and digits). Try again, or press Ctrl+C.");
    }
}

fn valid_client_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "cannot read {}; create it with your Spotify client_id (see README)",
                path.display()
            )
        })?;
        toml::from_str(&text).with_context(|| format!("invalid config in {}", path.display()))
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply() {
        let cfg: Config = toml::from_str(r#"client_id = "abc""#).unwrap();
        assert_eq!(cfg.device_name, "spotlite");
        assert_eq!((cfg.backend, cfg.daemon_port), (None, DEFAULT_DAEMON_PORT));
        let cfg: Config = toml::from_str("client_id = \"abc\"\n[keys]\nnext = \"N\"\nlike = [\"f\", \"ctrl-l\"]\n").unwrap();
        assert_eq!(cfg.keys.len(), 2);
        assert_eq!(cfg.redirect_uri(), "http://127.0.0.1:8898/callback");
    }

    #[test]
    fn client_ids() {
        assert!(valid_client_id("0123456789abcdef0123456789ABCDEF"));
        assert!(!valid_client_id("0123456789abcdef"));
        assert!(!valid_client_id("0123456789abcdef0123456789abcdeg"));
    }
}
