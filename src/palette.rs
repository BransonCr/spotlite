//! Command palette (`:` or Ctrl+P): every action in one list, filtered as you type. Each
//! entry runs an action of the key table, so the palette and the keys never disagree.

use crate::app::{App, Loc, SearchKind, Tab, View};
use crate::input::Input;
use crate::keys::Act;

#[derive(Default)]
pub struct Palette {
    pub query: Input,
    pub cursor: usize,
}

/// What a palette entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    Act(Act),
    Go(Loc),
}

#[derive(Clone)]
pub struct Entry {
    pub label: String,
    /// The key that does the same, shown on the right.
    pub key: String,
    pub run: Run,
}

/// The actions that make sense now (the view, the selection, the open tabs).
pub fn entries(app: &App) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut add = |label: &str, act: Act| {
        out.push(Entry { label: label.to_owned(), key: app.keys.show(act), run: Run::Act(act) });
    };
    let view = app.view();
    // What this view offers comes first.
    match view {
        View::Radio => {
            add("Discover new music (another round)", Act::Discover);
            add("Radio filters", Act::Filters);
            add("Save this radio as a playlist", Act::Save);
            add("Randomize a genre", Act::Randomize);
        }
        View::Random => {
            add("Roll again (50 new random songs)", Act::Discover);
            add("Save these songs as a playlist", Act::Save);
        }
        View::Artist => add("Next section", Act::NextSection),
        View::Album | View::Playlist | View::Playlists => add("Play all", Act::PlayAll),
        View::Search => {
            let next = match app.search_kind {
                SearchKind::Songs => "artists",
                SearchKind::Artists => "albums",
                SearchKind::Albums => "playlists",
                SearchKind::Playlists => "songs",
            };
            add(&format!("Search results: show {next}"), Act::SearchType);
        }
        _ => {}
    }
    if app.selected_track().is_some() {
        add("Song actions…", Act::Actions);
        add("Add the song to the queue", Act::Queue);
        add("Go to the song's artist", Act::Artist);
        add("Go to the song's album", Act::Album);
        add("Song radio", Act::Radio);
        add("Add the song to a playlist…", Act::AddToPlaylist);
        add("Like / unlike the song", Act::Like);
        add("Pin the song to the library", Act::Pin);
    }
    add(if app.sidebar.open { "Close the sidebar" } else { "Open the sidebar" }, Act::Sidebar);
    add("Sidebar: now playing", Act::SidePlaying);
    add("Sidebar: library", Act::SideLibrary);
    add("Sidebar effect…", Act::SideEffect);
    if app.playback.as_ref().is_some_and(|p| p.item.is_some()) {
        add("Share card of the song (picture)", Act::Card);
    }
    add("Search", Act::Search);
    const DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
    for (i, tab) in Tab::ALL.iter().enumerate() {
        out.push(Entry { label: format!("Go to {}", tab.title()), key: DIGITS[i].into(), run: Run::Go(Loc::Base(*tab)) });
    }
    for (i, p) in app.pages.iter().enumerate() {
        let n = i + Tab::ALL.len();
        let (kind, name) = p.title();
        let key = DIGITS.get(n).map_or(String::new(), |d| (*d).to_owned());
        out.push(Entry { label: format!("Go to {kind}: {name}"), key, run: Run::Go(Loc::Page(p.key())) });
    }
    let mut add = |label: &str, act: Act| {
        out.push(Entry { label: label.to_owned(), key: app.keys.show(act), run: Run::Act(act) });
    };
    if app.active.is_some() {
        add("Close this tab", Act::CloseTab);
    }
    add("Back", Act::Back);
    add("Where you went (undo tree)", Act::UndoTree);
    add("Next tab", Act::NextTab);
    add("Previous tab", Act::PrevTab);
    add("Play / pause", Act::PlayPause);
    add("Next song", Act::Next);
    add("Previous song", Act::Previous);
    add("Shuffle on / off", Act::Shuffle);
    add("Repeat: off, all, one", Act::Repeat);
    add("Jump to the playing song", Act::Jump);
    add("Volume up", Act::VolumeUp);
    add("Volume down", Act::VolumeDown);
    add("Mute / unmute", Act::Mute);
    add("Theme…", Act::Theme);
    add("Sound visualizer on / off", Act::Visualizer);
    add("Randomize a genre…", Act::Randomize);
    add("Reload the list", Act::Reload);
    add("Help: all keys", Act::Help);
    add("Quit (music keeps playing)", Act::Quit);
    add("Quit and stop spotifyd", Act::QuitStop);
    // One entry per action: the first (the one for this view) stays.
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| match &e.run {
        Run::Act(a) => seen.insert(*a),
        Run::Go(_) => true,
    });
    out
}

/// Entries that match the query: all its letters in order. Names that start with it come
/// first, then names that contain it, then the rest.
pub fn filter(entries: Vec<Entry>, query: &str) -> Vec<Entry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return entries;
    }
    let mut scored: Vec<(u8, usize, Entry)> = entries
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let label = e.label.to_lowercase();
            let score = if label.starts_with(&q) || e.key.eq_ignore_ascii_case(&q) {
                0
            } else if label.contains(&q) {
                1
            } else {
                let mut chars = label.chars();
                if !q.chars().all(|c| c == ' ' || chars.any(|l| l == c)) {
                    return None;
                }
                2
            };
            Some((score, i, e))
        })
        .collect();
    scored.sort_by_key(|(s, i, _)| (*s, *i));
    scored.into_iter().map(|(_, _, e)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_ranks_starts_then_contains_then_letters() {
        let e = |l: &str| Entry { label: l.into(), key: String::new(), run: Run::Act(Act::Help) };
        let all = vec![e("Volume up"), e("Theme…"), e("Go to Liked"), e("Next tab"), e("Previous tab")];
        let names = |q: &str| filter(all.clone(), q).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(names("th"), ["Theme…"]);
        assert_eq!(names("tab"), ["Next tab", "Previous tab"]);
        assert_eq!(names("gl"), ["Go to Liked"], "letters in order");
        assert!(names("zz").is_empty());
    }
}
