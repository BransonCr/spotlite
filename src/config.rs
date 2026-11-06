use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// User settings, read from ~/.config/spotlite/config.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Client ID of your app on developer.spotify.com (PKCE needs no secret).
    pub client_id: String,
    /// Spotify Connect device to play on; must match `device_name` in spotifyd.conf.
    #[serde(default = "default_device_name")]
    pub device_name: String,
    /// Local port for the OAuth redirect (http://127.0.0.1:<port>/callback).
    #[serde(default = "default_redirect_port")]
    pub redirect_port: u16,
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

pub fn token_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotlite/token.json")
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
        assert_eq!(cfg.redirect_uri(), "http://127.0.0.1:8898/callback");
    }
}
