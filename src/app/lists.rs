//! Lists: paging, the filter and sort over them, marks and undo.

use super::*;

/// How a list can be sorted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortBy {
    Title,
    Artist,
    Album,
    Time,
}

impl SortBy {
    pub fn name(self) -> &'static str {
        match self {
            SortBy::Title => "title",
            SortBy::Artist => "artist",
            SortBy::Album => "album",
            SortBy::Time => "time",
        }
    }
}

/// What a row offers to the filter and the sort.
pub trait Viewable {
    /// The text the filter looks in (lower case).
    fn haystack(&self) -> String;
    /// The sort key for `by`; None when the row cannot be sorted that way.
    fn sort_key(&self, _by: SortBy) -> Option<String> {
        None
    }
    fn sort_ms(&self) -> u64 {
        0
    }
}

impl Viewable for Track {
    fn haystack(&self) -> String {
        format!("{}\n{}\n{}", self.name, self.artist_names(), self.album_name()).to_lowercase()
    }
    fn sort_key(&self, by: SortBy) -> Option<String> {
        Some(match by {
            SortBy::Title => self.name.to_lowercase(),
            SortBy::Artist => self.artist_names().to_lowercase(),
            SortBy::Album => self.album_name().to_lowercase(),
            SortBy::Time => String::new(),
        })
    }
    fn sort_ms(&self) -> u64 {
        self.duration_ms
    }
}

impl Viewable for Playlist {
    fn haystack(&self) -> String {
        format!("{}\n{}", self.name, self.owner_name()).to_lowercase()
    }
    fn sort_key(&self, by: SortBy) -> Option<String> {
        match by {
            SortBy::Title => Some(self.name.to_lowercase()),
            SortBy::Artist => Some(self.owner_name().to_lowercase()),
            _ => None,
        }
    }
    fn sort_ms(&self) -> u64 {
        self.track_count().unwrap_or(0) as u64
    }
}

impl Viewable for Row {
    fn haystack(&self) -> String {
        match self {
            Row::Folder { name, .. } => name.to_lowercase(),
            Row::Playlist(p) => p.haystack(),
        }
    }
    fn sort_key(&self, by: SortBy) -> Option<String> {
        match self {
            // Folders stay before playlists whatever the sort.
            Row::Folder { name, .. } => Some(format!("\0{}", name.to_lowercase())),
            Row::Playlist(p) => p.sort_key(by),
        }
    }
}

impl Viewable for Found {
    fn haystack(&self) -> String {
        self.name().to_lowercase()
    }
    fn sort_key(&self, by: SortBy) -> Option<String> {
        (by == SortBy::Title).then(|| self.name().to_lowercase())
    }
}

impl Viewable for ArtistRow {
    fn haystack(&self) -> String {
        String::new()
    }
}

/// True when every word of `query` is in `haystack`.
pub fn matches(haystack: &str, query: &str) -> bool {
    query.split_whitespace().all(|w| haystack.contains(&w.to_lowercase()))
}

/// A paged list with its own selection and scroll position, and an optional filter and sort
/// over what is loaded. The selection and the scroll count the rows shown.
pub struct Listing<T> {
    pub items: Vec<T>,
    /// Entries fetched so far, null ones included (the offset of the next page).
    pub(super) fetched: usize,
    /// Size of the whole list on Spotify; None until the first page arrives.
    pub total: Option<usize>,
    pub loading: bool,
    pub state: TableState,
    /// The rows shown, as indexes into `items`, when a filter or a sort is on.
    pub(super) shown: Option<Vec<usize>>,
    /// The filter typed (Ctrl+F), and the sort (`,`): field and reversed.
    pub filter: Option<Input>,
    pub sort: Option<(SortBy, bool)>,
}

impl<T> Default for Listing<T> {
    fn default() -> Self {
        Listing {
            items: Vec::new(),
            fetched: 0,
            total: None,
            loading: false,
            state: TableState::default(),
            shown: None,
            filter: None,
            sort: None,
        }
    }
}

impl<T: Viewable> Listing<T> {
    pub fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }

    /// Rows shown (all of them, or what the filter keeps).
    pub fn len(&self) -> usize {
        self.shown.as_ref().map_or(self.items.len(), Vec::len)
    }

    /// The index into `items` of shown row `pos`.
    pub fn index(&self, pos: usize) -> Option<usize> {
        match &self.shown {
            Some(s) => s.get(pos).copied(),
            None => (pos < self.items.len()).then_some(pos),
        }
    }

    /// The shown row of `items[i]`, if it is shown.
    pub fn pos_of(&self, i: usize) -> Option<usize> {
        match &self.shown {
            Some(s) => s.iter().position(|&j| j == i),
            None => (i < self.items.len()).then_some(i),
        }
    }

    /// The rows shown, in order: (index into `items`, row).
    pub fn rows(&self) -> impl Iterator<Item = (usize, &T)> + '_ {
        let n = self.len();
        (0..n).filter_map(move |pos| self.index(pos).map(|i| (i, &self.items[i])))
    }

    /// True when a filter or a sort changes what shows.
    pub fn reshaped(&self) -> bool {
        self.shown.is_some()
    }

    pub fn current(&self) -> Option<&T> {
        self.items.get(self.index(self.selected())?)
    }

    pub(super) fn has_more(&self) -> bool {
        self.total.is_none_or(|t| self.fetched < t)
    }

    /// Offset of the next page to fetch, if one is needed now. With a filter or a sort on, the
    /// whole list loads (the rows it needs can be anywhere).
    pub(super) fn wants_page(&self) -> Option<usize> {
        let near_end = self.len().saturating_sub(self.selected()) <= PREFETCH;
        (!self.loading && self.has_more() && (self.items.is_empty() || near_end || self.reshaped())).then_some(self.fetched)
    }

    /// The next page when the whole list is wanted now (`c` looks for the playing song).
    pub(super) fn more_for(&self, wanted: bool) -> Option<usize> {
        (wanted && !self.loading && self.has_more()).then_some(self.fetched)
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        if self.len() == 0 {
            return;
        }
        let max = self.len() as isize - 1;
        let next = (self.selected() as isize).saturating_add(delta).clamp(0, max);
        self.state.select(Some(next as usize));
    }

    pub(super) fn apply(&mut self, offset: usize, page: ApiPage<T>) {
        let keep = self.index(self.selected());
        if offset == 0 {
            self.items.clear();
            self.fetched = 0;
            *self.state.offset_mut() = 0;
        }
        // Ignore pages that do not continue the list (e.g. a reload raced a page load).
        if offset == self.fetched {
            self.items.extend(page.items);
            self.fetched += page.raw;
            // An empty page means the list ended, whatever the total says.
            self.total = Some(if page.raw == 0 { self.fetched } else { page.total });
        }
        self.loading = false;
        self.reshape(if offset == 0 { None } else { keep });
    }

    pub(super) fn reset(&mut self) {
        let (filter, sort) = (self.filter.take(), self.sort);
        *self = Listing::default();
        // A reload keeps the filter and the sort.
        self.filter = filter;
        self.sort = sort;
    }

    /// Puts an item at the top. The selected row stays on the same item; a filter or a sort
    /// is worked out again.
    pub(super) fn push_front(&mut self, item: T) {
        let keep = self.index(self.selected());
        self.items.insert(0, item);
        self.fetched += 1;
        self.total = self.total.map(|t| t + 1);
        self.reshape(keep.map(|i| i + 1));
    }

    /// The whole list at once (from a saved copy): nothing more to load.
    pub(super) fn fill(&mut self, items: Vec<T>) {
        let keep = self.state.selected();
        self.fetched = items.len();
        self.total = Some(items.len());
        self.items = items;
        self.loading = false;
        self.shown = None;
        self.state.select(keep);
        self.reshape(None);
    }

    /// The items changed (added, removed): the filter and the sort apply again, and the
    /// selection stays on the same row where it can.
    pub fn changed(&mut self) {
        let keep = self.index(self.selected());
        self.reshape(keep);
    }

    /// Works out the rows shown from the filter and the sort. The selection goes to `keep`
    /// (an index into `items`) if it shows, else it stays in range.
    pub(crate) fn reshape(&mut self, keep: Option<usize>) {
        let pos = self.state.selected();
        let query = self.filter.as_ref().map(|f| f.trim().to_lowercase()).unwrap_or_default();
        self.shown = if query.is_empty() && self.sort.is_none() {
            None
        } else {
            let mut shown: Vec<usize> =
                (0..self.items.len()).filter(|&i| query.is_empty() || matches(&self.items[i].haystack(), &query)).collect();
            if let Some((by, rev)) = self.sort {
                let items = &self.items;
                match by {
                    SortBy::Time => shown.sort_by_key(|&i| items[i].sort_ms()),
                    _ => shown.sort_by_cached_key(|&i| items[i].sort_key(by).unwrap_or_default()),
                }
                if rev {
                    shown.reverse();
                }
            }
            Some(shown)
        };
        let len = self.len();
        let sel = keep.and_then(|i| self.pos_of(i)).or(pos).map(|p| p.min(len.saturating_sub(1)));
        self.state.select(if len == 0 { None } else { Some(sel.unwrap_or(0)) });
        if self.state.offset() >= len {
            *self.state.offset_mut() = 0;
        }
    }

    /// Sets the filter text (None: no filter). While typing, the first match is selected; when
    /// the filter goes, the selection stays on its song.
    pub fn set_filter(&mut self, filter: Option<Input>) {
        let keep = if filter.is_none() { self.index(self.selected()) } else { None };
        self.filter = filter;
        if keep.is_none() {
            *self.state.offset_mut() = 0;
            self.state.select(Some(0));
        }
        self.reshape(keep);
    }

    /// The next sort: title, artist, album, time (longest last), then the list's own order.
    /// Rows that cannot sort a way skip it.
    pub fn next_sort(&mut self) {
        let order = [SortBy::Title, SortBy::Artist, SortBy::Album, SortBy::Time];
        let start = match self.sort {
            None => 0,
            Some((by, _)) => order.iter().position(|b| *b == by).map_or(order.len(), |p| p + 1),
        };
        let can = |by: SortBy| by == SortBy::Time || self.items.first().is_none_or(|t| t.sort_key(by).is_some());
        self.sort = order[start.min(order.len())..].iter().copied().find(|b| can(*b)).map(|b| (b, false));
        let keep = self.index(self.selected());
        self.reshape(keep);
    }

    /// Turns the sort round (or the list's own order, when there is no sort).
    pub fn reverse_sort(&mut self) {
        self.sort = Some(match self.sort {
            Some((by, rev)) => (by, !rev),
            None => (SortBy::Time, false),
        });
        let keep = self.index(self.selected());
        self.reshape(keep);
    }
}

/// A list whose filter and sort the keys change, whatever its rows are.
pub trait Shape {
    fn filter(&self) -> Option<&Input>;
    fn set_filter(&mut self, filter: Option<Input>);
    fn next_sort(&mut self);
    fn reverse_sort(&mut self);
    fn sort(&self) -> Option<(SortBy, bool)>;
}

impl<T: Viewable> Shape for Listing<T> {
    fn filter(&self) -> Option<&Input> {
        self.filter.as_ref()
    }
    fn set_filter(&mut self, filter: Option<Input>) {
        Listing::set_filter(self, filter);
    }
    fn next_sort(&mut self) {
        Listing::next_sort(self);
    }
    fn reverse_sort(&mut self) {
        Listing::reverse_sort(self);
    }
    fn sort(&self) -> Option<(SortBy, bool)> {
        self.sort
    }
}

/// A change that `u` undoes.
#[derive(Debug, Clone)]
pub enum Change {
    /// Songs removed from a playlist, with the place each had.
    Removed { playlist_id: String, playlist: String, songs: Vec<(String, usize)> },
    /// Songs added to the end of a playlist.
    Added { playlist_id: String, playlist: String, uris: Vec<String> },
    /// Songs liked (true) or unliked.
    Liked { uris: Vec<String>, saved: bool },
    /// A pin taken off the sidebar, and its place.
    Unpinned(Box<Pin>, usize),
}


impl App {
    /// The scrollbar is dragged to row `y`: the selection goes to that part of the list.
    pub(super) fn scroll_to(&mut self, y: u16) {
        let bar = self.hits.scrollbar;
        let len = self.list_len();
        if bar.height == 0 || len == 0 {
            return;
        }
        let f = (y.saturating_sub(bar.y) as f32 / (bar.height.max(2) - 1) as f32).clamp(0.0, 1.0);
        self.select(((len - 1) as f32 * f).round() as usize);
    }

    /// The terminal window's title: the song and its artist while one plays.
    pub fn window_title(&self) -> String {
        match self.playback.as_ref().filter(|p| p.is_playing).and_then(|p| p.item.as_ref()) {
            Some(t) => format!("{} — {} · spotlite", t.name, t.artist_names()),
            None => "spotlite".to_owned(),
        }
    }

    /// The list shown, for its filter and sort.
    pub fn shape_mut(&mut self) -> Option<&mut dyn Shape> {
        match self.view() {
            View::Artist => None,
            View::Playlists if self.foldered() => Some(&mut self.rows),
            View::Playlists => Some(&mut self.playlists),
            View::Search if self.search_kind != SearchKind::Songs => Some(&mut self.found),
            _ => self.tracks_mut().map(|l| l as &mut dyn Shape),
        }
    }

    /// Opens a playlist in the Playlists tab. One opened before in this run comes back as you
    /// left it (its songs, place, filter and sort).
    pub(super) fn open_playlist(&mut self, pl: Playlist) {
        if self.open.as_ref().is_some_and(|o| o.playlist.id == pl.id) {
            return;
        }
        self.close_playlist();
        let open = match self.open_cache.iter().position(|o| o.playlist.id == pl.id) {
            Some(i) => self.open_cache.remove(i),
            None => OpenPlaylist { playlist: pl, tracks: Listing::default() },
        };
        self.open = Some(open);
        self.marks.clear();
        self.filtering = false;
    }

    /// Closes the open playlist and keeps it for a later visit. True if one was open.
    pub(super) fn close_playlist(&mut self) -> bool {
        let Some(open) = self.open.take() else { return false };
        // A list still loading starts again next time.
        if !open.tracks.loading {
            self.open_cache.retain(|o| o.playlist.id != open.playlist.id);
            self.open_cache.insert(0, open);
            self.open_cache.truncate(8);
        }
        self.marks.clear();
        self.filtering = false;
        true
    }

    /// The songs an action is for: the marked ones (in list order) if there are any, else
    /// `track`.
    pub(super) fn chosen(&self, track: Track) -> Vec<Track> {
        if self.marks.is_empty() {
            return vec![track];
        }
        let marked: Vec<Track> = self
            .tracks()
            .map(|l| l.items.iter().filter(|t| self.marks.contains(&t.uri)).cloned().collect())
            .unwrap_or_default();
        if marked.is_empty() { vec![track] } else { marked }
    }

    /// Adds the chosen songs to a playlist, and keeps it as the last one used.
    pub(super) fn add_to(&mut self, pl: Playlist, track: Track) {
        let songs = self.chosen(track);
        let uris: Vec<String> = songs.iter().map(|t| t.uri.clone()).collect();
        // A song already in the playlist (when it is loaded here) is not added twice by mistake.
        let known: Option<std::collections::HashSet<&str>> = self
            .open
            .iter()
            .chain(self.open_cache.iter())
            .find(|o| o.playlist.id == pl.id && !o.tracks.has_more())
            .map(|o| o.tracks.items.iter().map(|t| t.uri.as_str()).collect());
        let (fresh, there): (Vec<String>, Vec<String>) = match &known {
            Some(set) => uris.iter().cloned().partition(|u| !set.contains(u.as_str())),
            None => (uris.clone(), Vec::new()),
        };
        self.marks.clear();
        self.recent_playlists.retain(|id| *id != pl.id);
        self.recent_playlists.insert(0, pl.id.clone());
        self.recent_playlists.truncate(20);
        if fresh.is_empty() {
            self.info(format!("{} is already in {}", describe(&songs), pl.name));
            return;
        }
        let what = if there.is_empty() { describe(&songs) } else { format!("{} new songs ({} were there)", fresh.len(), there.len()) };
        self.changes.push(Change::Added { playlist_id: pl.id.clone(), playlist: pl.name.clone(), uris: fresh.clone() });
        self.send(Cmd::AddToPlaylist { playlist_id: pl.id, playlist: pl.name, uris: fresh, what, position: None });
    }

    /// `E`: adds the song (or the marked ones) to the playlist used last.
    pub(super) fn add_again(&mut self) {
        let Some(t) = self.selected_track() else {
            self.info("Select a song first.");
            return;
        };
        let last = self.recent_playlists.first().and_then(|id| self.playlists.items.iter().find(|p| p.id == *id)).cloned();
        match last {
            Some(pl) if self.can_edit => self.add_to(pl, t),
            _ => self.open_picker(t),
        }
    }

    /// Ctrl+F: type to narrow the list shown.
    pub(super) fn start_filter(&mut self) {
        let Some(shape) = self.shape_mut() else {
            self.info("This page has no list to filter.");
            return;
        };
        if shape.filter().is_none() {
            shape.set_filter(Some(Input::default()));
        }
        self.filtering = true;
    }

    /// Keys while typing a filter: Enter or ↓ keeps it and goes to the list, Esc clears it.
    pub(super) fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filtering = false;
                if let Some(shape) = self.shape_mut() {
                    shape.set_filter(None);
                }
            }
            KeyCode::Enter | KeyCode::Down => {
                self.filtering = false;
                // An empty filter goes away.
                if let Some(shape) = self.shape_mut().filter(|s| s.filter().is_some_and(|f| f.trim().is_empty())) {
                    shape.set_filter(None);
                }
            }
            KeyCode::Up => self.filtering = false,
            _ => {
                let Some(shape) = self.shape_mut() else {
                    self.filtering = false;
                    return;
                };
                let mut text = shape.filter().cloned().unwrap_or_default();
                if text.edit(&key) != Edit::Ignored {
                    shape.set_filter(Some(text));
                }
            }
        }
    }

    /// `,`: the next sort; `;`: turns it round.
    pub(super) fn sort_list(&mut self, reverse: bool) {
        let Some(shape) = self.shape_mut() else {
            self.info("This page has no list to sort.");
            return;
        };
        if reverse {
            shape.reverse_sort();
        } else {
            shape.next_sort();
        }
        let text = match shape.sort() {
            Some((by, rev)) => format!("Sorted by {}{}", by.name(), if rev { ", reversed" } else { "" }),
            None => "The list's own order".to_owned(),
        };
        self.info(text);
    }

    /// `w`: marks the song (or takes the mark off) and moves down.
    pub(super) fn mark(&mut self) {
        let Some(t) = self.tracks().and_then(|l| l.current()).map(|t| t.uri.clone()) else {
            self.info("Marks are for songs: select one first.");
            return;
        };
        if !self.marks.remove(&t) {
            self.marks.insert(t);
        }
        self.move_selection(1);
    }

    /// `W`: marks every song shown, or none when all are marked.
    pub(super) fn mark_all(&mut self) {
        let Some(l) = self.tracks() else { return };
        let shown: Vec<String> = l.rows().map(|(_, t)| t.uri.clone()).collect();
        if !shown.is_empty() && shown.iter().all(|u| self.marks.contains(u)) {
            self.marks.clear();
        } else {
            self.marks.extend(shown);
            self.info(format!("{} songs marked · actions apply to all · esc clears", self.marks.len()));
        }
    }

    /// `u`: undoes the last change (a remove, an add, a like, an unpin).
    pub(super) fn undo_change(&mut self) {
        let Some(change) = self.changes.pop() else {
            self.info("Nothing to undo.");
            return;
        };
        match change {
            Change::Removed { playlist_id, playlist, mut songs } => {
                // Back at their places, the first place first, so each lands where it was.
                songs.sort_by_key(|(_, at)| *at);
                for (uri, at) in &songs {
                    self.send(Cmd::AddToPlaylist {
                        playlist_id: playlist_id.clone(),
                        playlist: playlist.clone(),
                        uris: vec![uri.clone()],
                        what: "the song".into(),
                        position: Some(*at),
                    });
                }
                self.info(format!("Putting {} back in {playlist}", if songs.len() == 1 { "the song".into() } else { format!("{} songs", songs.len()) }));
            }
            Change::Added { playlist_id, playlist, uris } => {
                let what = if uris.len() == 1 { "the song".to_owned() } else { format!("{} songs", uris.len()) };
                self.send(Cmd::RemoveFromPlaylist { playlist_id, playlist, uris, what });
            }
            Change::Liked { uris, saved } => {
                if let [uri] = uris.as_slice() {
                    if self.playing_uri() == Some(uri.as_str()) {
                        self.saved = Some((uri.clone(), !saved));
                    }
                }
                self.send(Cmd::SetSavedMany { uris, saved: !saved });
            }
            Change::Unpinned(pin, at) => {
                self.info(format!("Pinned {} again", pin.name()));
                self.pins.insert(at.min(self.pins.len()), *pin);
            }
        }
    }

    /// The device menu: every Spotify Connect device; the one chosen plays from now on.
    pub(super) fn open_devices(&mut self, list: Vec<(String, String, bool)>) {
        if list.is_empty() {
            self.info("Spotify sees no devices. Is spotifyd running?");
            return;
        }
        let cursor = list.iter().position(|d| d.2).unwrap_or(0);
        let items = list
            .into_iter()
            .map(|(id, name, active)| ("", if active { format!("{name}  · playing here") } else { name.clone() }, Action::Device { id, name }))
            .collect();
        self.menu = Some(Menu { title: "Play on".into(), items, cursor, track: None, at: None });
    }
}
