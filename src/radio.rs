//! Song radio: tracks similar to a seed track, with filters.
//!
//! The Web API's /recommendations is closed to new apps, so the tracks come from Spotify's own
//! song radio (the app's "Go to song radio", `inspiredby-mix`) over the client protocol (see
//! client.rs). Spotify shapes it by your listening; no API offers a neutral radio any more, so
//! the filters (new songs only, new artists only, ...) are the way to step outside your taste.
//! One batched metadata request gives each track's artist, year, length, popularity and
//! explicit flag, which the filters use.
//!
//! Discover mode goes further: it hides everything near your Liked Songs, pulls in the radios of
//! a few songs it found, and shuffles. Each new round drops the songs of the round before.

#[derive(Debug, Clone, PartialEq)]
pub struct RadioTrack {
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    pub artist_uris: Vec<String>,
    pub album: String,
    pub year: Option<i32>,
    pub duration_ms: u64,
    /// 0-100, from Spotify's metadata.
    pub popularity: u8,
    pub explicit: bool,
    pub album_uri: Option<String>,
    /// Cover sizes (64, 300, 640 px).
    pub images: Vec<crate::api::ImageRef>,
}

impl RadioTrack {
    /// As a Web API track, for the shared track list.
    pub fn to_track(&self) -> crate::api::Track {
        use crate::api::{Album, Named, Track};
        Track {
            uri: self.uri.clone(),
            name: self.name.clone(),
            duration_ms: self.duration_ms,
            artists: self
                .artists
                .iter()
                .zip(self.artist_uris.iter().map(Some).chain(std::iter::repeat(None)))
                .map(|(name, uri)| Named { name: name.clone(), uri: uri.cloned() })
                .collect(),
            album: Some(Album {
                name: self.album.clone(),
                images: self.images.clone(),
                id: self.album_uri.as_deref().and_then(|u| u.rsplit(':').next()).map(str::to_owned),
                uri: self.album_uri.clone(),
                release_date: self.year.map(|y| y.to_string()),
                ..Default::default()
            }),
            show: None,
            images: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    Any,
    /// Within 10 years of the seed's release year.
    NearSeed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fame {
    Any,
    /// Popularity 55 and up.
    Popular,
    /// Popularity under 40.
    DeepCuts,
}

/// Which radio tracks to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filters {
    /// Leave out songs that are already in Liked Songs.
    pub new_only: bool,
    /// Leave out artists that appear in Liked Songs.
    pub new_artists: bool,
    pub no_explicit: bool,
    /// Leave out the seed's own artist.
    pub other_artists: bool,
    /// At most this many songs per artist (0 = no limit).
    pub per_artist: u8,
    pub era: Era,
    pub fame: Fame,
    /// Discover mode (see the module docs); the rounds live on the radio page.
    pub discover: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            discover: false,
            new_only: false,
            new_artists: false,
            no_explicit: false,
            other_artists: false,
            per_artist: 0,
            era: Era::Any,
            fame: Fame::Any,
        }
    }
}

/// What the filters know about your taste. Kept on disk (library.json) and brought up to date
/// with few requests (see `worker::sync_library`).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Library {
    /// Liked Songs in Spotify's order (newest first): true shuffle plays these.
    pub liked_order: Vec<String>,
    /// When the top and recent lists were last read (unix s).
    pub taste_at: u64,
    /// Liked Songs, and their artists.
    pub tracks: std::collections::HashSet<String>,
    pub artists: std::collections::HashSet<String>,
    /// Discover also leaves out these: top and recently played songs, top artists.
    /// Empty when the login has no user-top-read / user-read-recently-played.
    pub heard: std::collections::HashSet<String>,
    pub top_artists: std::collections::HashSet<String>,
}

/// Keeps the tracks that pass the filters, in radio order.
pub fn apply(tracks: &[RadioTrack], seed: &RadioTrack, filters: &Filters, library: &Library) -> Vec<RadioTrack> {
    let mut per_artist: std::collections::HashMap<&str, u8> = std::collections::HashMap::new();
    let seed_artist = seed.artist_uris.first().map(String::as_str);
    tracks
        .iter()
        .filter(|t| t.uri != seed.uri)
        .filter(|t| !(filters.new_only && library.tracks.contains(&t.uri)))
        .filter(|t| !(filters.new_artists && t.artist_uris.iter().any(|a| library.artists.contains(a))))
        .filter(|t| !(filters.no_explicit && t.explicit))
        .filter(|t| !(filters.other_artists && t.artist_uris.first().map(String::as_str) == seed_artist))
        .filter(|t| match (filters.era, t.year, seed.year) {
            (Era::NearSeed, Some(y), Some(s)) => (y - s).abs() <= 10,
            _ => true,
        })
        .filter(|t| match filters.fame {
            Fame::Any => true,
            Fame::Popular => t.popularity >= 55,
            Fame::DeepCuts => t.popularity < 40,
        })
        .filter(|t| {
            if filters.per_artist == 0 {
                return true;
            }
            let key = t.artist_uris.first().map_or("", String::as_str);
            let n = per_artist.entry(key).or_insert(0);
            *n += 1;
            *n <= filters.per_artist
        })
        .cloned()
        .collect()
}

/// Songs one discover round shows.
pub const DISCOVER_SIZE: usize = 50;

/// The discover list: songs of the radio pool that are not near your taste (not liked, top or
/// recently played; no liked or top artist; not the seed's artist) and that no earlier round
/// showed, in `rank` order.
pub fn discover(
    pool: &[RadioTrack],
    seed: &RadioTrack,
    filters: &Filters,
    library: &Library,
    seen: &std::collections::HashSet<String>,
    rank: &std::collections::HashMap<String, u32>,
) -> Vec<RadioTrack> {
    let strict = Filters { new_only: true, new_artists: true, other_artists: true, ..*filters };
    let mut kept: Vec<RadioTrack> = apply(pool, seed, &strict, library)
        .into_iter()
        .filter(|t| !seen.contains(&t.uri) && !library.heard.contains(&t.uri))
        .filter(|t| !t.artist_uris.iter().any(|a| library.top_artists.contains(a)))
        .collect();
    kept.sort_by_key(|t| rank.get(&t.uri).copied().unwrap_or(u32::MAX));
    kept.truncate(DISCOVER_SIZE);
    kept
}

/// The radio playlist of one seed, as track URIs.
#[cfg(feature = "client")]
async fn radio_uris(sp: &librespot_core::spclient::SpClient, seed_uri: &str) -> anyhow::Result<Vec<String>> {
    use anyhow::{anyhow, Context};
    use librespot_core::{SpotifyId, SpotifyUri};
    use librespot_protocol::playlist4_external::SelectedListContent;
    use protobuf::Message;

    let seed = SpotifyUri::from_uri(seed_uri).map_err(|e| anyhow!("bad seed: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&sp.get_radio_for_track(&seed).await?)?;
    let playlist = json["mediaItems"][0]["uri"]
        .as_str()
        .and_then(|u| u.strip_prefix("spotify:playlist:"))
        .context("Spotify returned no radio for this seed")?;
    let id = SpotifyId::from_base62(playlist).map_err(|e| anyhow!("bad radio id: {e}"))?;
    let list = SelectedListContent::parse_from_bytes(&sp.get_playlist(&id).await?)?;
    Ok(list
        .contents
        .items
        .iter()
        .map(|i| i.uri().to_owned())
        .filter(|u| u.starts_with("spotify:track:"))
        .collect())
}

/// Every song of a playlist, in order, over the client protocol: it works for any playlist
/// (the Web API lists only your own) and uses none of the Web API's quota. Blocks.
#[cfg(feature = "client")]
pub fn playlist_uris(playlist_uri: &str) -> anyhow::Result<Vec<String>> {
    use anyhow::anyhow;
    use librespot_core::SpotifyId;
    use librespot_protocol::playlist4_external::SelectedListContent;
    use protobuf::Message;

    let id = playlist_uri.strip_prefix("spotify:playlist:").ok_or_else(|| anyhow!("not a playlist: {playlist_uri}"))?;
    let id = SpotifyId::from_base62(id).map_err(|e| anyhow!("bad playlist id: {e}"))?;
    let (rt, session) = crate::client::connect()?;
    let bytes = rt.block_on(session.spclient().get_playlist(&id));
    session.shutdown();
    let list = SelectedListContent::parse_from_bytes(&bytes?)?;
    Ok(list
        .contents
        .items
        .iter()
        .map(|i| i.uri().to_owned())
        .filter(|u| u.starts_with("spotify:track:"))
        .collect())
}

#[cfg(not(feature = "client"))]
pub fn playlist_uris(_playlist_uri: &str) -> anyhow::Result<Vec<String>> {
    anyhow::bail!("needs the `client` feature")
}

/// Radio tracks of several seeds together (for discover mode); seeds with no radio are skipped.
/// Blocks; run it on its own thread.
#[cfg(feature = "client")]
pub fn fetch_more(seeds: &[String]) -> anyhow::Result<Vec<RadioTrack>> {
    let (rt, session) = crate::client::connect()?;
    let result = rt.block_on(async {
        let sp = session.spclient();
        let mut all: Vec<String> = Vec::new();
        for seed in seeds {
            if let Ok(uris) = radio_uris(sp, seed).await {
                all.extend(uris.into_iter().filter(|u| !all.contains(u)).collect::<Vec<_>>());
            }
        }
        anyhow::ensure!(!all.is_empty(), "Spotify returned no radio for these songs");
        let meta = metadata(&session, &all).await?;
        session.shutdown();
        anyhow::Ok(meta)
    })?;
    Ok(result)
}

#[cfg(not(feature = "client"))]
pub fn fetch_more(_seeds: &[String]) -> anyhow::Result<Vec<RadioTrack>> {
    anyhow::bail!("song radio needs the `client` feature")
}

/// Seed and radio tracks. The seed is a track (song radio) or an artist (artist radio).
/// Blocks; run it on its own thread.
#[cfg(feature = "client")]
pub fn fetch(seed_uri: &str) -> anyhow::Result<(RadioTrack, Vec<RadioTrack>)> {
    use anyhow::anyhow;

    let is_track = seed_uri.starts_with("spotify:track:");
    let (rt, session) = crate::client::connect()?;
    let result = rt.block_on(async {
        let uris = radio_uris(session.spclient(), seed_uri).await?;
        // A track seed comes first, so one request also gives its metadata (for the filters).
        let mut all = if is_track { vec![seed_uri.to_owned()] } else { Vec::new() };
        all.extend(uris);
        let meta = metadata(&session, &all).await?;
        session.shutdown();
        anyhow::Ok(meta)
    })?;
    let mut tracks = result.into_iter();
    let seed = if is_track {
        tracks.next().ok_or_else(|| anyhow!("no metadata for the seed song"))?
    } else {
        artist_seed(seed_uri)
    };
    Ok((seed, tracks.collect()))
}

/// The seed of an artist radio: only the artist, so "hide the seed's artist" works and the
/// era filter lets everything through.
pub fn artist_seed(uri: &str) -> RadioTrack {
    RadioTrack {
        uri: uri.to_owned(),
        name: String::new(),
        artists: Vec::new(),
        artist_uris: vec![uri.to_owned()],
        album: String::new(),
        year: None,
        duration_ms: 0,
        popularity: 0,
        explicit: false,
        album_uri: None,
        images: Vec::new(),
    }
}

#[cfg(not(feature = "client"))]
pub fn fetch(_seed_uri: &str) -> anyhow::Result<(RadioTrack, Vec<RadioTrack>)> {
    anyhow::bail!("song radio needs the `client` feature")
}

/// Track metadata for many URIs in one request, in the same order (unknown ones dropped).
#[cfg(feature = "client")]
async fn metadata(session: &librespot_core::Session, uris: &[String]) -> anyhow::Result<Vec<RadioTrack>> {
    use librespot_core::SpotifyId;
    use librespot_protocol::extended_metadata::{BatchedEntityRequest, EntityRequest, ExtensionQuery};
    use librespot_protocol::extension_kind::ExtensionKind;
    use librespot_protocol::metadata::Track;
    use protobuf::{EnumOrUnknown, Message};

    let req = BatchedEntityRequest {
        entity_request: uris
            .iter()
            .map(|u| EntityRequest {
                entity_uri: u.clone(),
                query: vec![ExtensionQuery {
                    extension_kind: EnumOrUnknown::new(ExtensionKind::TRACK_V4),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let res = session.spclient().get_extended_metadata(req).await?;
    let mut by_uri = std::collections::HashMap::new();
    for array in res.extended_metadata {
        for data in array.extension_data {
            let Some(any) = data.extension_data.as_ref() else { continue };
            let Ok(t) = Track::parse_from_bytes(&any.value) else { continue };
            let uri_of = |kind: &str, gid: &[u8]| {
                SpotifyId::from_raw(gid).ok().and_then(|id| id.to_base62().ok()).map(|b| format!("spotify:{kind}:{b}"))
            };
            let images = t
                .album
                .cover_group
                .image
                .iter()
                .map(|img| crate::api::ImageRef {
                    url: format!(
                        "https://i.scdn.co/image/{}",
                        img.file_id().iter().map(|b| format!("{b:02x}")).collect::<String>()
                    ),
                    width: Some(img.width().max(0) as u32).filter(|w| *w > 0),
                })
                .collect();
            by_uri.insert(
                data.entity_uri.clone(),
                RadioTrack {
                    uri: data.entity_uri.clone(),
                    name: t.name().to_owned(),
                    artists: t.artist.iter().map(|a| a.name().to_owned()).collect(),
                    artist_uris: t.artist.iter().filter_map(|a| uri_of("artist", a.gid())).collect(),
                    album_uri: uri_of("album", t.album.gid()),
                    images,
                    album: t.album.name().to_owned(),
                    year: t.album.date.as_ref().map(|d| d.year()).filter(|y| *y > 0),
                    duration_ms: t.duration().max(0) as u64,
                    popularity: t.popularity().clamp(0, 100) as u8,
                    explicit: t.explicit(),
                },
            );
        }
    }
    Ok(uris.iter().filter_map(|u| by_uri.remove(u)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, artist: &str, year: i32, pop: u8, explicit: bool) -> RadioTrack {
        RadioTrack {
            uri: format!("spotify:track:{id}"),
            name: id.into(),
            artists: vec![artist.into()],
            artist_uris: vec![format!("spotify:artist:{artist}")],
            album: String::new(),
            year: Some(year),
            duration_ms: 200_000,
            popularity: pop,
            explicit,
            album_uri: None,
            images: Vec::new(),
        }
    }

    #[test]
    fn filters_combine() {
        let seed = t("seed", "a", 1995, 70, false);
        let tracks = vec![
            seed.clone(),
            t("1", "a", 1996, 80, false),
            t("2", "b", 2020, 30, true),
            t("3", "b", 1990, 50, false),
            t("4", "b", 1999, 20, false),
            t("5", "c", 1994, 60, false),
        ];
        let library = Library {
            tracks: ["spotify:track:5".to_owned()].into(),
            artists: ["spotify:artist:b".to_owned()].into(),
            ..Default::default()
        };
        let ids = |f: Filters| apply(&tracks, &seed, &f, &library).iter().map(|t| t.name.clone()).collect::<Vec<_>>();

        assert_eq!(ids(Filters::default()), ["1", "2", "3", "4", "5"], "seed itself is dropped");
        assert_eq!(ids(Filters { new_only: true, ..Default::default() }), ["1", "2", "3", "4"]);
        assert_eq!(ids(Filters { new_artists: true, ..Default::default() }), ["1", "5"]);
        assert_eq!(ids(Filters { no_explicit: true, ..Default::default() }), ["1", "3", "4", "5"]);
        assert_eq!(ids(Filters { other_artists: true, ..Default::default() }), ["2", "3", "4", "5"]);
        assert_eq!(ids(Filters { per_artist: 1, ..Default::default() }), ["1", "2", "5"]);
        assert_eq!(ids(Filters { era: Era::NearSeed, ..Default::default() }), ["1", "3", "4", "5"]);
        assert_eq!(ids(Filters { fame: Fame::Popular, ..Default::default() }), ["1", "5"]);
        assert_eq!(ids(Filters { fame: Fame::DeepCuts, ..Default::default() }), ["2", "4"]);
    }

    #[test]
    fn discover_hides_your_taste_and_old_rounds() {
        let seed = t("seed", "a", 1995, 70, false);
        let pool = vec![
            t("1", "a", 1996, 80, false), // the seed's artist
            t("2", "b", 2020, 30, false), // a liked artist
            t("3", "c", 1990, 50, false), // a liked song
            t("4", "d", 1999, 20, false),
            t("5", "e", 1994, 60, false),
            t("6", "f", 1994, 60, true),
        ];
        let library = Library {
            tracks: ["spotify:track:3".to_owned()].into(),
            artists: ["spotify:artist:b".to_owned()].into(),
            heard: ["spotify:track:7".to_owned()].into(),
            top_artists: ["spotify:artist:g".to_owned()].into(),
            ..Default::default()
        };
        let mut pool = pool;
        pool.push(t("7", "h", 1994, 60, false)); // recently played
        pool.push(t("8", "g", 1994, 60, false)); // a top artist
        let rank: std::collections::HashMap<String, u32> =
            [("spotify:track:6", 0), ("spotify:track:5", 1), ("spotify:track:4", 2)].map(|(u, r)| (u.to_owned(), r)).into();
        let mut seen = std::collections::HashSet::new();
        let ids = |f: &Filters, seen: &std::collections::HashSet<String>| {
            discover(&pool, &seed, f, &library, seen, &rank).iter().map(|t| t.name.clone()).collect::<Vec<_>>()
        };
        let f = Filters { discover: true, ..Default::default() };
        assert_eq!(ids(&f, &seen), ["6", "5", "4"], "shuffled order, nothing you know");
        assert_eq!(ids(&Filters { no_explicit: true, ..f }, &seen), ["5", "4"], "other filters still apply");
        seen.insert("spotify:track:6".to_owned());
        assert_eq!(ids(&f, &seen), ["5", "4"], "an old round's songs are gone");
    }
}
