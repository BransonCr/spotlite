//! Pages that open on top of the tabs: an artist, an album, a song radio. Esc goes back.

use crate::api::{Album, Artist, Track};
use crate::app::Listing;
use crate::radio::{self, Era, Fame, Filters, Library, RadioTrack};

pub enum ArtistRow {
    /// One of the artist's popular songs.
    Track(Track),
    Album(Album),
}

pub struct ArtistPage {
    pub id: String,
    pub name: String,
    pub artist: Option<Artist>,
    pub rows: Listing<ArtistRow>,
    pub error: Option<String>,
}

impl ArtistPage {
    pub fn new(id: String, name: String) -> Self {
        let mut rows = Listing::default();
        rows.loading = true;
        ArtistPage { id, name, artist: None, rows, error: None }
    }

    pub fn fill(&mut self, artist: Artist, top: Vec<Track>, albums: Vec<Album>) {
        self.name = artist.name.clone();
        self.artist = Some(artist);
        let mut rows: Vec<ArtistRow> = top.into_iter().map(ArtistRow::Track).collect();
        rows.extend(albums.into_iter().map(ArtistRow::Album));
        self.rows.total = Some(rows.len());
        self.rows.items = rows;
        self.rows.loading = false;
        self.rows.state.select((!self.rows.items.is_empty()).then_some(0));
    }

    /// The popular songs, in order (to play them as a list).
    pub fn top_tracks(&self) -> Vec<&Track> {
        self.rows
            .items
            .iter()
            .filter_map(|r| match r {
                ArtistRow::Track(t) => Some(t),
                ArtistRow::Album(_) => None,
            })
            .collect()
    }
}

pub struct AlbumPage {
    pub id: String,
    pub album: Album,
    pub tracks: Listing<Track>,
}

impl AlbumPage {
    /// Album tracks come without their album; give them this one (for covers and links).
    pub fn adopt(&self, mut t: Track) -> Track {
        if t.album.is_none() {
            t.album = Some(self.album.clone());
        }
        t
    }
}

pub struct RadioPage {
    pub seed: Track,
    /// Seed and radio tracks, once Spotify answered.
    pub radio: Option<(RadioTrack, Vec<RadioTrack>)>,
    pub error: Option<String>,
    pub filters: Filters,
    /// The radio tracks that pass the filters.
    pub tracks: Listing<Track>,
}

impl RadioPage {
    pub fn new(seed: Track) -> Self {
        let mut tracks = Listing::default();
        tracks.loading = true;
        RadioPage { seed, radio: None, error: None, filters: Filters::default(), tracks }
    }

    /// Applies the filters again (after a filter change or when Liked Songs arrive).
    pub fn refilter(&mut self, library: &Library) {
        let Some((seed, all)) = &self.radio else { return };
        let kept = radio::apply(all, seed, &self.filters, library);
        let sel = self.tracks.selected();
        self.tracks.items = kept.iter().map(RadioTrack::to_track).collect();
        self.tracks.total = Some(self.tracks.items.len());
        self.tracks.loading = false;
        let len = self.tracks.items.len();
        self.tracks.state.select((len > 0).then_some(sel.min(len.saturating_sub(1))));
    }

    pub fn total(&self) -> usize {
        self.radio.as_ref().map_or(0, |(_, all)| all.len())
    }
}

/// The radio filter menu: one entry per filter, with its current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterItem {
    NewOnly,
    NewArtists,
    NoExplicit,
    OtherArtists,
    PerArtist,
    Era,
    Fame,
    Reset,
}

impl FilterItem {
    pub const ALL: [FilterItem; 8] = [
        FilterItem::NewOnly,
        FilterItem::NewArtists,
        FilterItem::NoExplicit,
        FilterItem::OtherArtists,
        FilterItem::PerArtist,
        FilterItem::Era,
        FilterItem::Fame,
        FilterItem::Reset,
    ];

    /// Label and current value for the menu.
    pub fn describe(self, f: &Filters) -> (&'static str, String) {
        let on = |b: bool| if b { "on" } else { "off" }.to_owned();
        match self {
            FilterItem::NewOnly => ("Hide songs in Liked Songs", on(f.new_only)),
            FilterItem::NewArtists => ("Hide artists in Liked Songs", on(f.new_artists)),
            FilterItem::NoExplicit => ("Hide explicit songs", on(f.no_explicit)),
            FilterItem::OtherArtists => ("Hide the seed's artist", on(f.other_artists)),
            FilterItem::PerArtist => (
                "Songs per artist",
                match f.per_artist {
                    0 => "any".into(),
                    n => format!("at most {n}"),
                },
            ),
            FilterItem::Era => (
                "Era",
                match f.era {
                    Era::Any => "any".into(),
                    Era::NearSeed => "near the seed (±10 y)".into(),
                },
            ),
            FilterItem::Fame => (
                "Popularity",
                match f.fame {
                    Fame::Any => "any".into(),
                    Fame::Popular => "popular".into(),
                    Fame::DeepCuts => "deep cuts".into(),
                },
            ),
            FilterItem::Reset => ("Reset all filters", String::new()),
        }
    }

    /// Changes the filter to its next value.
    pub fn step(self, f: &mut Filters) {
        match self {
            FilterItem::NewOnly => f.new_only = !f.new_only,
            FilterItem::NewArtists => f.new_artists = !f.new_artists,
            FilterItem::NoExplicit => f.no_explicit = !f.no_explicit,
            FilterItem::OtherArtists => f.other_artists = !f.other_artists,
            FilterItem::PerArtist => {
                f.per_artist = match f.per_artist {
                    0 => 2,
                    2 => 1,
                    _ => 0,
                }
            }
            FilterItem::Era => {
                f.era = match f.era {
                    Era::Any => Era::NearSeed,
                    Era::NearSeed => Era::Any,
                }
            }
            FilterItem::Fame => {
                f.fame = match f.fame {
                    Fame::Any => Fame::Popular,
                    Fame::Popular => Fame::DeepCuts,
                    Fame::DeepCuts => Fame::Any,
                }
            }
            FilterItem::Reset => *f = Filters::default(),
        }
    }
}

/// Short names of the active filters, for the radio page title.
pub fn active_filters(f: &Filters) -> Vec<String> {
    FilterItem::ALL
        .iter()
        .filter(|i| **i != FilterItem::Reset)
        .filter_map(|i| {
            let (label, value) = i.describe(f);
            let (_, default) = i.describe(&Filters::default());
            (value != default).then(|| match i {
                FilterItem::NewOnly => "new songs".to_owned(),
                FilterItem::NewArtists => "new artists".to_owned(),
                FilterItem::NoExplicit => "clean".to_owned(),
                FilterItem::OtherArtists => "other artists".to_owned(),
                _ => format!("{}: {value}", label.to_lowercase()),
            })
        })
        .collect()
}

/// A page on top of the tabs.
pub enum Page {
    Artist(ArtistPage),
    Album(AlbumPage),
    Radio(Box<RadioPage>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_items_cycle_and_reset() {
        let mut f = Filters::default();
        FilterItem::PerArtist.step(&mut f);
        assert_eq!(f.per_artist, 2);
        FilterItem::PerArtist.step(&mut f);
        assert_eq!(f.per_artist, 1);
        FilterItem::Fame.step(&mut f);
        FilterItem::NewOnly.step(&mut f);
        assert_eq!(active_filters(&f), ["new songs", "songs per artist: at most 1", "popularity: popular"]);
        FilterItem::Reset.step(&mut f);
        assert_eq!(f, Filters::default());
        assert!(active_filters(&f).is_empty());
    }
}
