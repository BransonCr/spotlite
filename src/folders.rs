//! Playlist folders. The Web API only lists playlists; the folder tree lives in the account's
//! "rootlist", which only Spotify's client protocol can read (see client.rs). spotlite reads
//! it once at start and keeps the tree in ~/.cache/spotlite/folders.json so it shows at once
//! next time.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    Folder { name: String, children: Vec<Entry> },
    /// A playlist URI.
    Playlist(String),
}

impl Entry {
    /// Playlists in this entry, nested folders included.
    pub fn playlist_count(&self) -> usize {
        match self {
            Entry::Playlist(_) => 1,
            Entry::Folder { children, .. } => children.iter().map(Entry::playlist_count).sum(),
        }
    }
}

/// Builds the tree from rootlist URIs, where folders are bracketed by
/// `spotify:start-group:<id>:<name>` and `spotify:end-group:<id>`.
#[cfg_attr(not(feature = "client"), allow(dead_code))]
pub fn parse_rootlist<'a>(uris: impl IntoIterator<Item = &'a str>) -> Vec<Entry> {
    let mut stack: Vec<(String, Vec<Entry>)> = vec![(String::new(), Vec::new())];
    for uri in uris {
        if let Some(rest) = uri.strip_prefix("spotify:start-group:") {
            let name = rest.split_once(':').map_or("", |(_, n)| n);
            // Names are form-encoded ("+" for spaces).
            let name = url::form_urlencoded::parse(format!("n={name}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            stack.push((name, Vec::new()));
        } else if uri.starts_with("spotify:end-group:") {
            if stack.len() > 1 {
                let (name, children) = stack.pop().expect("checked");
                stack.last_mut().expect("root").1.push(Entry::Folder { name, children });
            }
        } else if uri.starts_with("spotify:playlist:") {
            stack.last_mut().expect("root").1.push(Entry::Playlist(uri.to_owned()));
        }
    }
    // Close folders a truncated list left open.
    while stack.len() > 1 {
        let (name, children) = stack.pop().expect("checked");
        stack.last_mut().expect("root").1.push(Entry::Folder { name, children });
    }
    stack.pop().map(|(_, e)| e).unwrap_or_default()
}

fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotlite/folders.json")
}

pub fn load_cached() -> Option<Vec<Entry>> {
    serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()
}

pub fn save_cache(tree: &[Entry]) -> Result<()> {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string(tree)?)?;
    Ok(())
}

/// True if the tree has at least one folder (else the flat list is just as good).
pub fn has_folders(tree: &[Entry]) -> bool {
    tree.iter().any(|e| matches!(e, Entry::Folder { .. }))
}

/// Reads the rootlist over Spotify's client protocol. Blocks; run it on its own thread.
#[cfg(feature = "client")]
pub fn fetch() -> Result<Vec<Entry>> {
    use librespot_protocol::playlist4_external::SelectedListContent;
    use protobuf::Message;

    crate::client::with(|rt, session| {
        rt.block_on(async {
            let mut uris = Vec::new();
            loop {
                let bytes = session.spclient().get_rootlist(uris.len(), Some(500)).await?;
                let list = SelectedListContent::parse_from_bytes(&bytes)?;
                let items = &list.contents.items;
                uris.extend(items.iter().map(|i| i.uri().to_owned()));
                if items.is_empty() || !list.contents.truncated() {
                    break;
                }
            }
            Ok(parse_rootlist(uris.iter().map(String::as_str)))
        })
    })
}

#[cfg(not(feature = "client"))]
pub fn fetch() -> Result<Vec<Entry>> {
    anyhow::bail!("built without the `client` feature")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nested_folders() {
        let tree = parse_rootlist([
            "spotify:playlist:a",
            "spotify:start-group:1f2e:Gym+%26+Run",
            "spotify:playlist:b",
            "spotify:start-group:3c4d:Heavy",
            "spotify:playlist:c",
            "spotify:end-group:3c4d",
            "spotify:end-group:1f2e",
            "spotify:playlist:d",
        ]);
        assert_eq!(
            tree,
            vec![
                Entry::Playlist("spotify:playlist:a".into()),
                Entry::Folder {
                    name: "Gym & Run".into(),
                    children: vec![
                        Entry::Playlist("spotify:playlist:b".into()),
                        Entry::Folder { name: "Heavy".into(), children: vec![Entry::Playlist("spotify:playlist:c".into())] },
                    ],
                },
                Entry::Playlist("spotify:playlist:d".into()),
            ]
        );
        assert_eq!(tree[1].playlist_count(), 2);
        assert!(has_folders(&tree));
    }

    #[test]
    fn unclosed_folder_and_stray_end_are_tolerated() {
        let tree = parse_rootlist(["spotify:end-group:x", "spotify:start-group:1:Open", "spotify:playlist:a"]);
        assert_eq!(tree, vec![Entry::Folder { name: "Open".into(), children: vec![Entry::Playlist("spotify:playlist:a".into())] }]);
    }
}
