//! Command palette (`:` or Ctrl+P): every action in one list, filtered as you type. Each
//! entry runs the same key you could press, so the palette and the keys never disagree.

use crate::app::{App, SearchKind, Tab, View};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Default)]
pub struct Palette {
    pub query: String,
    pub cursor: usize,
}

#[derive(Clone)]
pub struct Entry {
    pub label: String,
    /// The key that does the same, shown on the right.
    pub key: &'static str,
    pub event: KeyEvent,
}

fn key(c: char) -> KeyEvent {
    KeyEvent::from(KeyCode::Char(c))
}

/// The actions that make sense now (the view, the selection, the open tabs).
pub fn entries(app: &App) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut add = |label: &str, k: &'static str, event: KeyEvent| {
        out.push(Entry { label: label.to_owned(), key: k, event });
    };
    let view = app.view();
    // What this view offers comes first.
    match view {
        View::Radio => {
            add("Discover new music (another round)", "d", key('d'));
            add("Radio filters", "F", key('F'));
            add("Save this radio as a playlist", "S", key('S'));
            add("Randomize a genre", "z", key('z'));
        }
        View::Random => {
            add("Roll again (50 new random songs)", "d", key('d'));
            add("Save these songs as a playlist", "S", key('S'));
        }
        View::Artist => add("Next section", "tab", KeyEvent::from(KeyCode::Tab)),
        View::Album | View::Playlist | View::Playlists => add("Play all", "P", key('P')),
        View::Search => {
            let next = match app.search_kind {
                SearchKind::Songs => "artists",
                SearchKind::Artists => "albums",
                SearchKind::Albums => "playlists",
                SearchKind::Playlists => "songs",
            };
            add(&format!("Search results: show {next}"), "t", key('t'));
        }
        _ => {}
    }
    if app.selected_track().is_some() {
        add("Song actions…", ".", key('.'));
        add("Add the song to the queue", "a", key('a'));
        add("Go to the song's artist", "i", key('i'));
        add("Go to the song's album", "b", key('b'));
        add("Song radio", "o", key('o'));
        add("Add the song to a playlist…", "e", key('e'));
        add("Like / unlike the song", "f", key('f'));
    }
    add(if app.sidebar.open { "Close the sidebar" } else { "Open the sidebar" }, "\\", key('\\'));
    add("Sidebar: now playing", "<", key('<'));
    add("Sidebar: library", ">", key('>'));
    if app.selected_track().is_some() {
        add("Pin the song to the library", "*", key('*'));
    }
    if app.playback.as_ref().is_some_and(|p| p.item.is_some()) {
        add("Share card of the song (picture)", "C", key('C'));
    }
    add("Search", "/", key('/'));
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let k: &'static str = ["1", "2", "3", "4"][i];
        add(&format!("Go to {}", tab.title()), k, key(char::from(b'1' + i as u8)));
    }
    const DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
    for (i, p) in app.pages.iter().enumerate() {
        let n = i + Tab::ALL.len();
        if n < 9 {
            let (kind, name) = p.title();
            add(&format!("Go to {kind}: {name}"), DIGITS[n], key(char::from(b'1' + n as u8)));
        }
    }
    if app.active.is_some() {
        add("Close this tab", "x", key('x'));
    }
    add("Back", "esc", KeyEvent::from(KeyCode::Esc));
    add("Where you went (undo tree)", "^⌫", KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
    add("Next tab", "L", key('L'));
    add("Previous tab", "H", key('H'));
    add("Play / pause", "space", key(' '));
    add("Next song", "n", key('n'));
    add("Previous song", "p", key('p'));
    add("Shuffle on / off", "s", key('s'));
    add("Repeat: off, all, one", "r", key('r'));
    add("Jump to the playing song", "c", key('c'));
    add("Volume up", "+", key('+'));
    add("Volume down", "-", key('-'));
    add("Mute / unmute", "m", key('m'));
    add("Theme…", "T", key('T'));
    add("Sound visualizer on / off", "v", key('v'));
    add("Reload the list", "R", key('R'));
    add("Help: all keys", "?", key('?'));
    add("Quit (music keeps playing)", "q", key('q'));
    add("Quit and stop spotifyd", "Q", key('Q'));
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

pub fn opens(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char(':') || (key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_ranks_starts_then_contains_then_letters() {
        let e = |l: &str| Entry { label: l.into(), key: "", event: key('x') };
        let all = vec![e("Volume up"), e("Theme…"), e("Go to Liked"), e("Next tab"), e("Previous tab")];
        let names = |q: &str| filter(all.clone(), q).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(names("th"), ["Theme…"]);
        assert_eq!(names("tab"), ["Next tab", "Previous tab"]);
        assert_eq!(names("gl"), ["Go to Liked"], "letters in order");
        assert!(names("zz").is_empty());
    }
}
