//! Spotify's client protocol (librespot), for what the Web API does not offer: playlist
//! folders and song radio. It logs in with spotifyd's saved librespot login (read only), so
//! there is no second login. Uses close together share one connection (see `with`).

#[cfg(feature = "client")]
pub use imp::*;

#[cfg(feature = "client")]
mod imp {
    use anyhow::Result;
    use librespot_core::{Session, SessionConfig};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tokio::runtime::Runtime;

    /// The player engine's saved librespot login: spotifyd's (written by `spotifyd
    /// authenticate`), or the built-in player's.
    fn spotifyd_credentials() -> Option<librespot_core::authentication::Credentials> {
        let base = crate::engine::credentials_dir()?;
        ["oauth", "zeroconf", ""].iter().find_map(|sub| {
            let file = base.join(sub).join("credentials.json");
            serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
        })
    }

    /// A connected session and the single-threaded runtime it runs on. Blocks while it
    /// connects.
    fn connect() -> Result<(Runtime, Session)> {
        let credentials = spotifyd_credentials()
            .ok_or_else(|| anyhow::anyhow!("no {} login found; {}", crate::engine::name(), crate::engine::hint_login()))?;
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let session = rt.block_on(async {
            let session = Session::new(SessionConfig::default(), None);
            session.connect(credentials, false).await.map(|_| session)
        })?;
        Ok((rt, session))
    }

    /// The last session, kept for a short time: a radio, its discover rounds, a playlist and a
    /// shuffle often come close together, and each connect takes 1 to 2 s. Nothing drives an
    /// idle session, so an older one is not trusted.
    static KEPT: Mutex<Option<(Runtime, Session, Instant)>> = Mutex::new(None);
    const KEEP: Duration = Duration::from_secs(45);

    /// Runs `f` with a connected session: the kept one if it is recent, else a new one. If the
    /// kept one fails, `f` runs once more with a new session. Blocks; call it from a
    /// background thread. One use at a time (others wait).
    pub fn with<T>(f: impl Fn(&Runtime, &Session) -> Result<T>) -> Result<T> {
        let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((rt, session, at)) = kept.take() {
            if at.elapsed() < KEEP && !session.is_invalid() {
                if let Ok(v) = f(&rt, &session) {
                    *kept = Some((rt, session, Instant::now()));
                    return Ok(v);
                }
            }
            session.shutdown();
        }
        let (rt, session) = connect()?;
        let result = f(&rt, &session);
        match result {
            Ok(_) => *kept = Some((rt, session, Instant::now())),
            Err(_) => session.shutdown(),
        }
        result
    }

    /// The account's user id, from the client protocol login. The Web API's /me can be refused
    /// for hours; this asks nothing of it.
    pub fn username() -> Option<String> {
        let name = with(|_, session| Ok(session.username())).ok()?;
        (!name.is_empty()).then_some(name)
    }
}

#[cfg(not(feature = "client"))]
pub fn username() -> Option<String> {
    None
}

/// True when this build can use the client protocol.
pub const AVAILABLE: bool = cfg!(feature = "client");
