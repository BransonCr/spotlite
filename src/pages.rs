//! Pages that open in their own tabs: an artist, an album, a song radio. Esc goes back to the
//! tab you came from; the page stays open until you close it (`x`).

use crate::api::{Album, Artist, Named, Track};
use crate::app::Listing;
use crate::radio::{self, Era, Fame, Filters, Library, RadioTrack};
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::{HashMap, HashSet};

pub enum ArtistRow {
    /// A section title ("Popular", "Radio", "Albums", ...). The cursor skips it.
    Header(&'static str),
    /// One of the artist's popular songs.
    Track(Track),
    /// A radio to start: the artist's own radio, or a song radio from a popular song.
    Radio(Track),
    Album(Album),
}

impl ArtistRow {
    pub fn selectable(&self) -> bool {
        !matches!(self, ArtistRow::Header(_))
    }
}

/// Song radios offered on an artist page, from its most popular songs.
const SONG_RADIOS: usize = 3;
/// Popular songs shown on an artist page (Spotify's own page shows 5, then 10).
const POPULAR: usize = 10;

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

    /// The seed of the artist radio, as a track-like row (the radio page shows its name).
    pub fn radio_seed(&self) -> Track {
        let uri = format!("spotify:artist:{}", self.id);
        Track {
            uri: uri.clone(),
            name: self.name.clone(),
            duration_ms: 0,
            artists: vec![Named { name: self.name.clone(), uri: Some(uri) }],
            album: None,
            show: None,
            images: self.artist.as_ref().map(|a| a.images.clone()).unwrap_or_default(),
        }
    }

    pub fn fill(&mut self, artist: Artist, mut top: Vec<Track>, albums: Vec<Album>) {
        top.truncate(POPULAR);
        self.name = artist.name.clone();
        self.artist = Some(artist);
        let mut rows = Vec::new();
        if !top.is_empty() {
            rows.push(ArtistRow::Header("Popular"));
        }
        let radios: Vec<Track> = top.iter().take(SONG_RADIOS).cloned().collect();
        rows.extend(top.into_iter().map(ArtistRow::Track));
        rows.push(ArtistRow::Header("Radio"));
        rows.push(ArtistRow::Radio(self.radio_seed()));
        rows.extend(radios.into_iter().map(ArtistRow::Radio));
        let kind = |a: &Album| match a.album_type.as_deref() {
            Some("single") => 1,
            Some("compilation") => 2,
            _ => 0,
        };
        for (k, title) in [(0, "Albums"), (1, "Singles and EPs"), (2, "Compilations")] {
            let group: Vec<&Album> = albums.iter().filter(|a| kind(a) == k).collect();
            if !group.is_empty() {
                rows.push(ArtistRow::Header(title));
                rows.extend(group.into_iter().cloned().map(ArtistRow::Album));
            }
        }
        self.rows.total = Some(rows.len());
        self.rows.items = rows;
        self.rows.loading = false;
        self.rows.state.select(self.rows.items.iter().position(ArtistRow::selectable));
    }

    /// Moves the cursor by `delta` rows, over the section titles.
    pub fn move_by(&mut self, delta: isize) {
        let items = &self.rows.items;
        if items.is_empty() {
            return;
        }
        let mut i = self.rows.selected() as isize;
        let step = delta.signum();
        let mut left = delta.abs();
        let mut best = i;
        while left > 0 {
            i += step;
            if i < 0 || i >= items.len() as isize {
                break;
            }
            if items[i as usize].selectable() {
                best = i;
                left -= 1;
            }
        }
        self.rows.state.select(Some(best as usize));
        // Keep a title in view when its first row is selected.
        if best > 0 && !items[best as usize - 1].selectable() && self.rows.state.offset() == best as usize {
            *self.rows.state.offset_mut() = best as usize - 1;
        }
    }

    /// Selects row `i`, or the next selectable row after a title.
    pub fn select(&mut self, i: usize) {
        if let Some(j) = self.rows.items.iter().skip(i).position(ArtistRow::selectable) {
            self.rows.state.select(Some(i + j));
        }
    }

    /// Jumps to the first row of the next (`1`) or previous (`-1`) section.
    pub fn jump_section(&mut self, dir: isize) {
        let items = &self.rows.items;
        let starts: Vec<usize> = (1..items.len())
            .filter(|&i| !items[i - 1].selectable() && items[i].selectable())
            .collect();
        let cur = self.rows.selected();
        let target = if dir > 0 {
            starts.iter().copied().find(|&s| s > cur)
        } else {
            starts.iter().copied().rev().find(|&s| s < cur).map(|s| {
                // From inside a section, go to its start first.
                let own = starts.iter().copied().rev().find(|&s| s <= cur);
                match own {
                    Some(o) if o < cur => o,
                    _ => s,
                }
            })
        };
        if let Some(t) = target {
            self.rows.state.select(Some(t));
            *self.rows.state.offset_mut() = self.rows.state.offset().min(t - 1);
        }
    }

    /// The popular songs, in order (to play them as a list).
    pub fn top_tracks(&self) -> Vec<&Track> {
        self.rows
            .items
            .iter()
            .filter_map(|r| match r {
                ArtistRow::Track(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    pub fn count(&self, f: impl Fn(&ArtistRow) -> bool) -> usize {
        self.rows.items.iter().filter(|r| f(r)).count()
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
    /// Discover mode: songs from the radios of songs it found (added to the pool each round).
    pub extra: Vec<RadioTrack>,
    /// Songs shown in earlier rounds; a new round leaves them out.
    pub seen: HashSet<String>,
    /// Shuffled place of each pool song in the discover list.
    pub rank: HashMap<String, u32>,
    pub round: u32,
    /// A round is waiting for Spotify.
    pub digging: bool,
}

/// Songs whose radios a discover round adds.
const DIG_SEEDS: usize = 4;

impl RadioPage {
    pub fn new(seed: Track) -> Self {
        let mut tracks = Listing::default();
        tracks.loading = true;
        RadioPage {
            seed,
            radio: None,
            error: None,
            filters: Filters::default(),
            tracks,
            extra: Vec::new(),
            seen: HashSet::new(),
            rank: HashMap::new(),
            round: 0,
            digging: false,
        }
    }

    /// Starts a discover round: the songs on screen now count as heard, the pool is shuffled
    /// again, and the songs whose radios to add come back (none until the radio has loaded).
    pub fn next_round(&mut self, library: &Library) -> Vec<String> {
        let Some((seed, all)) = &self.radio else { return Vec::new() };
        let mut rng = rand::thread_rng();
        let shown: Vec<String> = self.tracks.items.iter().map(|t| t.uri.clone()).collect();
        // Dig from songs you do not know; on the first round the plain radio is on screen.
        let mut seeds: Vec<String> = if self.filters.discover {
            shown.clone()
        } else {
            radio::discover(all, seed, &self.filters, library, &self.seen, &HashMap::new())
                .into_iter()
                .map(|t| t.uri)
                .collect()
        };
        if self.filters.discover {
            self.seen.extend(shown);
        }
        seeds.shuffle(&mut rng);
        seeds.truncate(DIG_SEEDS);
        self.filters.discover = true;
        self.round += 1;
        self.digging = !seeds.is_empty();
        self.reshuffle();
        self.refilter(library);
        seeds
    }

    /// Adds songs that a round's radios brought.
    pub fn add_found(&mut self, found: Vec<RadioTrack>, library: &Library) {
        self.digging = false;
        let mut have: HashSet<String> = self.extra.iter().map(|t| t.uri.clone()).collect();
        if let Some((_, all)) = &self.radio {
            have.extend(all.iter().map(|t| t.uri.clone()));
        }
        self.extra.extend(found.into_iter().filter(|t| have.insert(t.uri.clone())));
        self.reshuffle();
        self.refilter(library);
    }

    /// Ends discover mode: back to the plain radio.
    pub fn stop_discover(&mut self) {
        self.filters.discover = false;
        self.seen.clear();
        self.round = 0;
        self.digging = false;
    }

    fn reshuffle(&mut self) {
        let mut rng = rand::thread_rng();
        let pool = self.radio.iter().flat_map(|(_, all)| all.iter()).chain(self.extra.iter());
        self.rank = pool.map(|t| (t.uri.clone(), rng.gen())).collect();
    }

    /// Applies the filters again (after a filter change or when Liked Songs arrive).
    pub fn refilter(&mut self, library: &Library) {
        let Some((seed, all)) = &self.radio else { return };
        let kept = if self.filters.discover {
            let pool: Vec<RadioTrack> = all.iter().chain(self.extra.iter()).cloned().collect();
            radio::discover(&pool, seed, &self.filters, library, &self.seen, &self.rank)
        } else {
            radio::apply(all, seed, &self.filters, library)
        };
        let sel = self.tracks.selected();
        self.tracks.items = kept.iter().map(RadioTrack::to_track).collect();
        self.tracks.total = Some(self.tracks.items.len());
        self.tracks.loading = false;
        let len = self.tracks.items.len();
        self.tracks.state.select((len > 0).then_some(sel.min(len.saturating_sub(1))));
    }

    /// Songs in the pool: the radio, and in discover mode the songs its rounds found.
    pub fn total(&self) -> usize {
        let extra = if self.filters.discover { self.extra.len() } else { 0 };
        self.radio.as_ref().map_or(0, |(_, all)| all.len()) + extra
    }
}

/// The radio filter menu: one entry per filter, with its current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterItem {
    Discover,
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
    pub const ALL: [FilterItem; 9] = [
        FilterItem::Discover,
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
            FilterItem::Discover => ("Discover new music", if f.discover { "on · again" } else { "off" }.into()),
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

    /// Changes the filter to its next value. (Discover runs a round instead: see RadioPage.)
    pub fn step(self, f: &mut Filters) {
        match self {
            FilterItem::Discover => f.discover = true,
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
                FilterItem::Discover => "discover".to_owned(),
                FilterItem::NewOnly => "new songs".to_owned(),
                FilterItem::NewArtists => "new artists".to_owned(),
                FilterItem::NoExplicit => "clean".to_owned(),
                FilterItem::OtherArtists => "other artists".to_owned(),
                _ => format!("{}: {value}", label.to_lowercase()),
            })
        })
        .collect()
}

/// A page in its own tab.
pub enum Page {
    Artist(ArtistPage),
    Album(AlbumPage),
    Radio(Box<RadioPage>),
}

impl Page {
    /// One tab per artist, album and radio seed: opening it again goes to the open tab.
    pub fn key(&self) -> String {
        match self {
            Page::Artist(a) => format!("artist:{}", a.id),
            Page::Album(a) => format!("album:{}", a.id),
            Page::Radio(r) => format!("radio:{}", r.seed.uri),
        }
    }

    /// Kind and name, for the tab bar.
    pub fn title(&self) -> (&'static str, &str) {
        match self {
            Page::Artist(a) => ("Artist", &a.name),
            Page::Album(a) => ("Album", &a.album.name),
            Page::Radio(r) => ("Radio", &r.seed.name),
        }
    }
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
