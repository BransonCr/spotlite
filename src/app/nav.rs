//! Where you are: tabs, pages, the undo tree of places, folders, and the saved state.

use super::*;

impl App {
    /// True when the Playlists tab shows the folder tree.
    pub fn foldered(&self) -> bool {
        self.folders.is_some()
    }

    /// Takes a folder tree (from the cache at start, or freshly fetched).
    pub fn set_folders(&mut self, tree: Vec<Entry>) {
        if !crate::folders::has_folders(&tree) {
            self.folders = None;
            return;
        }
        self.folders = Some(tree);
        self.rebuild_rows();
    }

    /// Names of the open folders, top first.
    pub fn folder_names(&self) -> Vec<String> {
        let Some(tree) = &self.folders else { return Vec::new() };
        let mut level: &[Entry] = tree;
        let mut names = Vec::new();
        for (i, _) in &self.folder_path {
            match level.get(*i) {
                Some(Entry::Folder { name, children }) => {
                    names.push(name.clone());
                    level = children;
                }
                _ => break,
            }
        }
        names
    }

    /// Rebuilds the rows of the open folder level from the tree and the loaded playlists.
    pub(super) fn rebuild_rows(&mut self) {
        let Some(tree) = &self.folders else { return };
        let mut level: &[Entry] = tree;
        let mut valid = true;
        for (i, _) in &self.folder_path {
            match level.get(*i) {
                Some(Entry::Folder { children, .. }) => level = children,
                _ => valid = false,
            }
        }
        if !valid {
            // The tree changed under us: start again from the top.
            self.folder_path.clear();
            level = tree;
        }
        let by_uri: std::collections::HashMap<&str, &Playlist> =
            self.playlists.items.iter().map(|p| (p.uri.as_str(), p)).collect();
        let mut rows: Vec<Row> = level
            .iter()
            .enumerate()
            .filter_map(|(index, e)| match e {
                Entry::Folder { name, .. } => Some(Row::Folder { name: name.clone(), index, count: e.playlist_count() }),
                Entry::Playlist(uri) => by_uri.get(uri.as_str()).map(|p| Row::Playlist((*p).clone())),
            })
            .collect();
        if self.folder_path.is_empty() {
            // Playlists the tree does not know yet (e.g. made after the last fetch) go at the end.
            fn uris<'a>(entries: &'a [Entry], out: &mut std::collections::HashSet<&'a str>) {
                for e in entries {
                    match e {
                        Entry::Folder { children, .. } => uris(children, out),
                        Entry::Playlist(u) => {
                            out.insert(u.as_str());
                        }
                    }
                }
            }
            let mut known = std::collections::HashSet::new();
            uris(tree, &mut known);
            rows.extend(
                self.playlists
                    .items
                    .iter()
                    .filter(|p| !known.contains(p.uri.as_str()))
                    .cloned()
                    .map(Row::Playlist),
            );
        }
        let total = rows.len();
        self.rows.items = rows;
        self.rows.total = Some(total);
        self.rows.loading = self.playlists.loading;
        let sel = self.rows.state.selected().unwrap_or(0).min(total.saturating_sub(1));
        self.rows.state.select((total > 0).then_some(sel));
    }

    pub(super) fn open_folder(&mut self, index: usize) {
        let parent = std::mem::take(&mut self.rows.state);
        self.folder_path.push((index, parent));
        self.rows.state.select(Some(0));
        self.rebuild_rows();
    }

    /// Back one level; false if already at the top.
    pub(super) fn close_folder(&mut self) -> bool {
        match self.folder_path.pop() {
            Some((_, parent)) => {
                self.rows.state = parent;
                self.rebuild_rows();
                true
            }
            None => false,
        }
    }

    /// The playlist under the cursor in the Playlists tab (not a folder).
    pub(super) fn current_playlist(&self) -> Option<Playlist> {
        if self.foldered() {
            match self.rows.current()? {
                Row::Playlist(p) => Some(p.clone()),
                Row::Folder { .. } => None,
            }
        } else {
            self.playlists.current().cloned()
        }
    }

    /// The page shown, if a page tab is active.
    pub fn page(&self) -> Option<&Page> {
        self.pages.get(self.active?)
    }

    pub fn page_mut(&mut self) -> Option<&mut Page> {
        let i = self.active?;
        self.pages.get_mut(i)
    }

    pub fn loc(&self) -> Loc {
        match self.page() {
            Some(p) => Loc::Page(p.key()),
            None => Loc::Base(self.tab),
        }
    }

    /// Every tab in bar order: the fixed ones, then the open pages.
    pub fn locs(&self) -> Vec<Loc> {
        Tab::ALL.iter().map(|t| Loc::Base(*t)).chain(self.pages.iter().map(|p| Loc::Page(p.key()))).collect()
    }

    pub(super) fn exists(&self, loc: &Loc) -> bool {
        match loc {
            Loc::Base(_) => true,
            Loc::Page(k) => self.pages.iter().any(|p| p.key() == *k),
        }
    }

    /// Shows a tab without a back step.
    pub(super) fn show(&mut self, loc: Loc) {
        self.typing = false;
        self.filtering = false;
        self.marks.clear();
        self.hover = None;
        match loc {
            Loc::Base(t) => {
                self.tab = t;
                self.active = None;
            }
            Loc::Page(k) => {
                self.active = self.pages.iter().position(|p| p.key() == k);
                if self.unloaded.remove(&k) {
                    if let Some(i) = self.active {
                        self.load_page(i);
                    }
                }
            }
        }
        // Recently played changes as you listen: load it again on a visit, when the song
        // changed since or a minute went by.
        if self.view() == View::Recent && !self.recent.loading {
            let song = self.playing_uri().map(str::to_owned);
            let fresh = self.recent_at.as_ref().is_some_and(|(at, s)| at.elapsed() < RECENT_AGE && *s == song);
            if !fresh {
                self.recent.reset();
                self.recent_at = Some((Instant::now(), song));
            }
        }
    }

    /// Asks for what page `i` shows (the album's songs load in tick()).
    pub(super) fn load_page(&mut self, i: usize) {
        let cmd = match &self.pages[i] {
            Page::Artist(a) => Cmd::Artist(a.id.clone()),
            Page::Album(a) => Cmd::Album(a.id.clone()),
            Page::Radio(r) => Cmd::Radio(r.seed.uri.clone()),
            Page::Random(r) => Cmd::Randomize(r.genre.clone()),
        };
        if matches!(cmd, Cmd::Radio(_)) && !self.library_asked {
            self.library_asked = true;
            self.send(Cmd::Library);
        }
        self.send(cmd);
    }

    /// The open tabs and the theme, to save at quit.
    pub fn saved_state(&self) -> State {
        let tabs = self
            .pages
            .iter()
            .map(|p| match p {
                Page::Artist(a) => SavedTab::Artist { id: a.id.clone(), name: a.name.clone() },
                Page::Album(a) => SavedTab::Album { album: a.album.clone() },
                Page::Radio(r) => SavedTab::Radio { seed: r.seed.clone(), tuned: r.saved() },
                Page::Random(r) => SavedTab::Random { genre: r.genre.clone() },
            })
            .collect();
        // A theme tried with SPOTLITE_THEME is not saved.
        // So is the mono theme that NO_COLOR asks for.
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let theme = if crate::theme::env_override().is_some() || no_color { crate::state::load().theme } else { Some(crate::theme::name()) };
        State {
            theme,
            tabs,
            active: self.active,
            tab: Some(self.tab),
            search: self.searched.clone(),
            search_kind: Some(self.search_kind),
            viz_on: self.viz_on,
            side_fx: self.side_fx,
            sidebar: Some(crate::state::SavedSidebar { open: self.sidebar.open, view: self.sidebar.view }),
            pins: self.pins.clone(),
            searches: self.search_history.clone(),
            recent_playlists: self.recent_playlists.clone(),
            // The live state (a shuffle just pressed counts), else the last one seen.
            playback: self.snapshot().or_else(|| self.last_playback.clone()),
        }
    }

    /// Opens the tabs of the last run. Nothing loads until a tab is shown.
    pub fn restore(&mut self, state: State) {
        if let Some(tab) = state.tab {
            self.tab = tab;
        }
        if !state.search.is_empty() {
            // The last search is listed again (it loads in tick()).
            self.query = state.search.as_str().into();
            self.searched = state.search;
            self.typing = false;
        }
        if let Some(kind) = state.search_kind {
            self.search_kind = kind;
        }
        self.viz_on |= state.viz_on;
        self.side_fx = state.side_fx;
        if let Some(sb) = state.sidebar {
            self.sidebar.open = sb.open;
            self.sidebar.view = sb.view;
        }
        self.pins = state.pins;
        self.search_history = state.searches;
        self.recent_playlists = state.recent_playlists;
        // Your playlists from the saved copy, so the tab and the sidebar show them at once; the
        // first page loads again soon (tick), so a change shows too.
        let mut offset = 0;
        while let Some(page) = crate::store::read::<ApiPage<Playlist>>(&format!("{}-{offset}.json", crate::worker::kept::PLAYLISTS)) {
            let raw = page.raw;
            self.playlists.apply(offset, page);
            offset += raw;
            if raw == 0 || self.playlists.total.is_some_and(|t| offset >= t) {
                break;
            }
        }
        if offset > 0 {
            self.playlists_stale = true;
            self.rebuild_rows();
        }
        // Liked Songs from the saved copy: the tab shows at once, with no request.
        if let Some(songs) = crate::store::read::<Vec<Track>>("liked.json").filter(|s| !s.is_empty()) {
            self.liked.fill(songs);
        }
        self.last_playback = state.playback.clone();
        if let Some(sp) = &state.playback {
            self.show_saved(sp);
        }
        self.true_shuffle = state.playback.as_ref().is_some_and(|p| p.shuffle);
        self.resume = state.playback.map(|p| (p, Instant::now()));
        for tab in state.tabs.into_iter().take(MAX_PAGES) {
            let page = match tab {
                SavedTab::Artist { id, name } => Page::Artist(ArtistPage::new(id, name)),
                SavedTab::Album { album } => match album.id.clone() {
                    Some(id) => Page::Album(AlbumPage { id, album, tracks: Listing::default() }),
                    None => continue,
                },
                SavedTab::Radio { seed, tuned } => Page::Radio(Box::new(RadioPage::restored(seed, tuned))),
                SavedTab::Random { genre } => Page::Random(RandomPage::new(genre)),
            };
            let key = page.key();
            if !self.exists(&Loc::Page(key.clone())) {
                self.unloaded.insert(key);
                self.pages.push(page);
            }
        }
        if let Some(key) = state.active.and_then(|i| self.pages.get(i)).map(Page::key) {
            self.show(Loc::Page(key));
        }
        // The undo tree starts where the last run ended.
        let here = self.loc();
        let (label, reopen) = self.nav_describe(&here);
        self.nav = NavTree::new(here, label);
        self.nav.nodes[0].reopen = reopen;
    }

    /// Goes to a tab; Esc comes back here.
    pub fn go(&mut self, loc: Loc) {
        self.nav_visit(&loc);
        self.show(loc);
    }

    /// Adds a visit to `loc` to the undo tree.
    pub(super) fn nav_visit(&mut self, loc: &Loc) {
        let (label, reopen) = self.nav_describe(loc);
        self.nav.visit(loc.clone(), label, reopen);
    }

    /// The tree's label for a place, and what opens it again if its tab closes.
    pub(super) fn nav_describe(&self, loc: &Loc) -> (String, Option<SavedTab>) {
        match loc {
            Loc::Base(tab) => {
                let glyph = match tab {
                    Tab::Search => "⌕",
                    Tab::Playlists => "≡",
                    Tab::Liked => "♥",
                    Tab::Recent => "◷",
                };
                (format!("{glyph} {}", tab.title()), None)
            }
            Loc::Page(k) => match self.pages.iter().find(|p| p.key() == *k) {
                Some(p) => {
                    let (glyph, saved) = match p {
                        Page::Artist(a) => ("◉", SavedTab::Artist { id: a.id.clone(), name: a.name.clone() }),
                        Page::Album(a) => ("◎", SavedTab::Album { album: a.album.clone() }),
                        Page::Radio(r) => ("≋", SavedTab::Radio { seed: r.seed.clone(), tuned: r.saved() }),
                        Page::Random(r) => ("⚄", SavedTab::Random { genre: r.genre.clone() }),
                    };
                    (format!("{glyph} {}", p.title().1), Some(saved))
                }
                None => ("?".into(), None),
            },
        }
    }

    /// Opens a closed page again from what the undo tree kept. True if it is open now.
    pub(super) fn reopen(&mut self, saved: &SavedTab) -> bool {
        let page = match saved.clone() {
            SavedTab::Artist { id, name } => Page::Artist(ArtistPage::new(id, name)),
            SavedTab::Album { album } => match album.id.clone() {
                Some(id) => Page::Album(AlbumPage { id, album, tracks: Listing::default() }),
                None => return false,
            },
            SavedTab::Radio { seed, tuned } => Page::Radio(Box::new(RadioPage::restored(seed, tuned))),
            SavedTab::Random { genre } => Page::Random(RandomPage::new(genre)),
        };
        let key = page.key();
        if !self.exists(&Loc::Page(key.clone())) {
            self.unloaded.insert(key);
            self.pages.push(page);
            if self.pages.len() > MAX_PAGES {
                self.pages.remove(0);
                self.active = None;
            }
        }
        true
    }

    /// Goes to node `i` of the undo tree (a click in the tree): opens its page again if its
    /// tab was closed.
    pub fn nav_jump(&mut self, i: usize) {
        let Some(node) = self.nav.nodes.get(i).cloned() else { return };
        if !self.exists(&node.loc) {
            match &node.reopen {
                Some(saved) if self.reopen(saved) => {}
                _ => {
                    self.info("That page cannot open again.");
                    return;
                }
            }
        }
        self.open = None;
        self.nav.jump(i);
        self.show(node.loc);
    }

    pub(super) fn switch_tab(&mut self, tab: Tab) {
        self.go(Loc::Base(tab));
    }

    /// The next (`1`) or previous (`-1`) tab in the bar, pages included.
    pub(super) fn cycle_tab(&mut self, dir: isize) {
        let locs = self.locs();
        let cur = self.loc();
        let i = locs.iter().position(|l| *l == cur).unwrap_or(0) as isize;
        let n = locs.len() as isize;
        self.go(locs[(i + dir).rem_euclid(n) as usize].clone());
    }

    /// Back one step: close an open playlist or folder, else go to the tab you were on before.
    /// Pages stay open.
    pub(super) fn back(&mut self) {
        // Esc first clears what the list shows on top: the marks, then the filter.
        if !self.marks.is_empty() {
            self.marks.clear();
            return;
        }
        if let Some(shape) = self.shape_mut().filter(|s| s.filter().is_some()) {
            shape.set_filter(None);
            return;
        }
        if self.active.is_none() && (self.close_playlist() || self.close_folder()) {
            return;
        }
        let open: std::collections::HashSet<String> = self.pages.iter().map(Page::key).collect();
        let usable = |n: &NavNode| match &n.loc {
            Loc::Base(_) => true,
            Loc::Page(k) => open.contains(k),
        };
        if let Some(loc) = self.nav.back(usable).map(|n| n.loc.clone()) {
            self.show(loc);
        } else if self.active.is_some() {
            self.show(Loc::Base(self.tab));
        }
    }

    /// Closes the page tab shown and goes back.
    pub(super) fn close_page(&mut self) {
        let Some(i) = self.active else {
            self.info("Only artist, album and radio tabs close.");
            return;
        };
        let key = self.pages.remove(i).key();
        self.unloaded.remove(&key);
        self.active = None;
        let open: std::collections::HashSet<String> = self.pages.iter().map(Page::key).collect();
        let usable = |n: &NavNode| match &n.loc {
            Loc::Base(_) => true,
            Loc::Page(k) => open.contains(k),
        };
        match self.nav.back(usable).map(|n| n.loc.clone()) {
            Some(loc) => self.show(loc),
            None => self.show(Loc::Base(self.tab)),
        }
    }

    /// Opens a page in a new tab, or goes to its tab if it is open. True for a new tab.
    pub(super) fn open_page(&mut self, page: Page) -> bool {
        let key = page.key();
        let fresh = !self.exists(&Loc::Page(key.clone()));
        if fresh {
            self.pages.push(page);
            if self.pages.len() > MAX_PAGES {
                self.pages.remove(0);
                self.active = None;
            }
        }
        let loc = Loc::Page(key);
        self.nav_visit(&loc);
        self.show(loc);
        fresh
    }

    pub fn open_artist(&mut self, id: String, name: String) {
        if self.open_page(Page::Artist(ArtistPage::new(id.clone(), name))) {
            self.send(Cmd::Artist(id));
        }
    }

    pub fn open_album(&mut self, album: Album) {
        let Some(id) = album.id.clone() else {
            self.info("This song has no album page.");
            return;
        };
        let details = album.release_date.is_none() || album.images.is_empty();
        if self.open_page(Page::Album(AlbumPage { id: id.clone(), album, tracks: Listing::default() })) && details {
            self.send(Cmd::Album(id));
        }
    }

    pub fn start_radio(&mut self, seed: Track) {
        if !crate::client::AVAILABLE {
            self.error("Song radio needs the `client` feature.");
            return;
        }
        if !seed.uri.starts_with("spotify:track:") && !seed.uri.starts_with("spotify:artist:") {
            self.info("Radio works for songs and artists only.");
            return;
        }
        let uri = seed.uri.clone();
        if !self.open_page(Page::Radio(Box::new(RadioPage::new(seed)))) {
            return;
        }
        self.send(Cmd::Radio(uri));
        if !self.library_asked {
            self.library_asked = true;
            self.send(Cmd::Library);
        }
    }

    pub(super) fn go_to_artist(&mut self, track: &Track) {
        match track.artists.iter().find(|a| a.id().is_some()) {
            Some(a) => {
                let (id, name) = (a.id().unwrap_or_default().to_owned(), a.name.clone());
                self.open_artist(id, name);
            }
            None => self.info("This song has no artist page."),
        }
    }

    pub(super) fn go_to_album(&mut self, track: &Track) {
        match track.album.clone() {
            Some(a) => self.open_album(a),
            None => self.info("This song has no album page."),
        }
    }

    /// Ctrl+Backspace: shows the tree, the cursor on where you are.
    pub(super) fn open_undo(&mut self) {
        let rows = self.nav.rows();
        self.undo = Some(rows.iter().position(|r| r.node == self.nav.cur).unwrap_or(0));
    }

    pub(super) fn undo_key(&mut self, key: KeyEvent) {
        let len = self.nav.rows().len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => self.undo = None,
            _ if is_ctrl_backspace(&key) => self.undo = None,
            KeyCode::Down | KeyCode::Char('j') => self.move_popup(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_popup(-1),
            KeyCode::Char('g') | KeyCode::Home => self.undo = Some(0),
            KeyCode::Char('G') | KeyCode::End => self.undo = Some(len.saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char(' ') => self.undo_go(),
            _ => {}
        }
    }

    /// Goes to the node under the tree's cursor and closes the tree.
    pub(super) fn undo_go(&mut self) {
        let Some(c) = self.undo.take() else { return };
        if let Some(row) = self.nav.rows().get(c) {
            let node = row.node;
            self.nav_jump(node);
        }
    }
}
