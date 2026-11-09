//! Spotify's client protocol (librespot), for what the Web API does not offer: playlist
//! folders and song radio. It logs in with spotifyd's saved librespot login (read only), so
//! there is no second login. Each use is short: connect, ask, disconnect.

#[cfg(feature = "client")]
pub use imp::*;

#[cfg(feature = "client")]
mod imp {
    use anyhow::Result;
    use librespot_core::{Session, SessionConfig};
    use tokio::runtime::Runtime;

    /// spotifyd's saved librespot login (written by `spotifyd authenticate`).
    fn spotifyd_credentials() -> Option<librespot_core::authentication::Credentials> {
        let base = dirs::cache_dir()?.join("spotifyd");
        ["oauth", "zeroconf", ""].iter().find_map(|sub| {
            let file = base.join(sub).join("credentials.json");
            serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
        })
    }

    /// A connected session and the single-threaded runtime it runs on. Blocks while it
    /// connects; call it from a background thread.
    pub fn connect() -> Result<(Runtime, Session)> {
        let credentials = spotifyd_credentials()
            .ok_or_else(|| anyhow::anyhow!("no spotifyd login found; run `spotifyd authenticate`"))?;
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let session = rt.block_on(async {
            let session = Session::new(SessionConfig::default(), None);
            session.connect(credentials, false).await.map(|_| session)
        })?;
        Ok((rt, session))
    }

    /// The account's user id, from the client protocol login. The Web API's /me can be refused
    /// for hours; this asks nothing of it.
    pub fn username() -> Option<String> {
        let (_rt, session) = connect().ok()?;
        let name = session.username();
        session.shutdown();
        (!name.is_empty()).then_some(name)
    }
}

#[cfg(not(feature = "client"))]
pub fn username() -> Option<String> {
    None
}

/// True when this build can use the client protocol.
pub const AVAILABLE: bool = cfg!(feature = "client");
