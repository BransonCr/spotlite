//! Every action that has a key, in one table: its name (for `[keys]` in config.toml), its
//! default keys, and what help says about it. The key handler, the help overlay, the footer
//! hints and the palette all read this table, so they never disagree.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Act {
    // Move
    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    Bottom,
    NextTab,
    PrevTab,
    NextSection,
    PrevSection,
    CloseTab,
    Back,
    UndoTree,
    Jump,
    Search,
    SearchType,
    Palette,
    Filter,
    Sort,
    // Sidebar
    Sidebar,
    SidePlaying,
    SideLibrary,
    Pin,
    Card,
    // Play
    Play,
    PlayAll,
    PlayPause,
    Next,
    Previous,
    Forward,
    Rewind,
    Shuffle,
    Repeat,
    VolumeUp,
    VolumeDown,
    Mute,
    Devices,
    // Song
    Actions,
    Queue,
    Artist,
    Album,
    Radio,
    Randomize,
    AddToPlaylist,
    AddAgain,
    Remove,
    Like,
    Mark,
    MarkAll,
    Undo,
    // Other
    Filters,
    Discover,
    Save,
    Theme,
    Visualizer,
    SideEffect,
    Reload,
    Help,
    Quit,
    QuitStop,
}

/// One action: its config name, help section, help text and default keys.
pub struct Info {
    pub act: Act,
    pub name: &'static str,
    pub section: &'static str,
    pub help: &'static str,
    pub keys: &'static [&'static str],
}

const fn i(act: Act, name: &'static str, section: &'static str, help: &'static str, keys: &'static [&'static str]) -> Info {
    Info { act, name, section, help, keys }
}

/// Help sections, in the order they show.
pub const SECTIONS: [&str; 5] = ["Move", "Play", "Song", "Sidebar", "Other"];

pub const ALL: &[Info] = &[
    i(Act::Down, "down", "Move", "move down", &["j", "down"]),
    i(Act::Up, "up", "Move", "move up", &["k", "up"]),
    i(Act::PageDown, "page_down", "Move", "a page down", &["ctrl-d", "pgdn"]),
    i(Act::PageUp, "page_up", "Move", "a page up", &["ctrl-u", "pgup"]),
    i(Act::Top, "top", "Move", "to the top", &["g", "home"]),
    i(Act::Bottom, "bottom", "Move", "to the bottom", &["G", "end"]),
    i(Act::NextTab, "next_tab", "Move", "next tab", &["L"]),
    i(Act::PrevTab, "prev_tab", "Move", "previous tab", &["H"]),
    i(Act::NextSection, "next_section", "Move", "next section (artist), else next tab", &["tab"]),
    i(Act::PrevSection, "prev_section", "Move", "previous section, else tab", &["shift-tab"]),
    i(Act::CloseTab, "close_tab", "Move", "close this artist, album, radio tab", &["x"]),
    i(Act::Back, "back", "Move", "back to the last place", &["esc", "h", "backspace"]),
    i(Act::UndoTree, "undo_tree", "Move", "every place you went (undo tree)", &["ctrl-backspace"]),
    i(Act::Jump, "jump", "Move", "jump to the playing song", &["c"]),
    i(Act::Search, "search", "Move", "search", &["/"]),
    i(Act::SearchType, "search_type", "Move", "results: songs, artists, albums…", &["t"]),
    i(Act::Palette, "palette", "Move", "do anything (palette)", &[":", "ctrl-p"]),
    i(Act::Filter, "filter", "Move", "filter the list as you type", &["ctrl-f"]),
    i(Act::Sort, "sort", "Move", "sort the list", &[","]),
    i(Act::Play, "play", "Play", "play the song, or open", &["enter", "l"]),
    i(Act::PlayAll, "play_all", "Play", "play the whole playlist or album", &["P"]),
    i(Act::PlayPause, "play_pause", "Play", "play / pause", &["space"]),
    i(Act::Next, "next", "Play", "next song", &["n"]),
    i(Act::Previous, "previous", "Play", "previous song", &["p"]),
    i(Act::Forward, "forward", "Play", "10 s forward", &["right"]),
    i(Act::Rewind, "rewind", "Play", "10 s back", &["left"]),
    i(Act::Shuffle, "shuffle", "Play", "true shuffle on / off", &["s"]),
    i(Act::Repeat, "repeat", "Play", "repeat: off, all, one", &["r"]),
    i(Act::VolumeUp, "volume_up", "Play", "volume up", &["+", "=", "]", "shift-up"]),
    i(Act::VolumeDown, "volume_down", "Play", "volume down", &["-", "[", "shift-down"]),
    i(Act::Mute, "mute", "Play", "mute / unmute", &["m"]),
    i(Act::Devices, "devices", "Play", "play on another device", &["y"]),
    i(Act::Actions, "actions", "Song", "all actions (or right-click)", &["."]),
    i(Act::Queue, "queue", "Song", "add to the queue", &["a"]),
    i(Act::Artist, "artist", "Song", "go to the artist", &["i"]),
    i(Act::Album, "album", "Song", "go to the album", &["b"]),
    i(Act::Radio, "radio", "Song", "song radio (or the artist's)", &["o"]),
    i(Act::Randomize, "randomize", "Song", "50 random songs of a genre", &["z"]),
    i(Act::AddToPlaylist, "add_to_playlist", "Song", "add to a playlist", &["e"]),
    i(Act::AddAgain, "add_again", "Song", "add to the last playlist again", &["E"]),
    i(Act::Remove, "remove", "Song", "remove from this playlist", &["D"]),
    i(Act::Like, "like", "Song", "like / unlike", &["f"]),
    i(Act::Mark, "mark", "Song", "mark (act on all marked)", &["w"]),
    i(Act::MarkAll, "mark_all", "Song", "mark all, or none", &["W"]),
    i(Act::Undo, "undo", "Song", "undo the last change", &["u"]),
    i(Act::Sidebar, "sidebar", "Sidebar", "open, focus, close the sidebar", &["\\", "ctrl-b"]),
    i(Act::SidePlaying, "side_playing", "Sidebar", "sidebar: now playing", &["<"]),
    i(Act::SideLibrary, "side_library", "Sidebar", "sidebar: library", &[">"]),
    i(Act::Pin, "pin", "Sidebar", "pin to the library, unpin", &["*"]),
    i(Act::Card, "card", "Sidebar", "share card picture of the song", &["C"]),
    i(Act::Filters, "filters", "Other", "radio filters", &["F"]),
    i(Act::Discover, "discover", "Other", "discover, or roll again", &["d"]),
    i(Act::Save, "save", "Other", "save radio / random as playlist", &["S"]),
    i(Act::Theme, "theme", "Other", "theme", &["T"]),
    i(Act::Visualizer, "visualizer", "Other", "sound visualizer", &["v"]),
    i(Act::SideEffect, "side_effect", "Other", "sidebar effect", &["V"]),
    i(Act::Reload, "reload", "Other", "load again from Spotify", &["R"]),
    i(Act::Help, "help", "Other", "these keys", &["?"]),
    i(Act::Quit, "quit", "Other", "quit (music keeps playing)", &["q"]),
    i(Act::QuitStop, "quit_stop", "Other", "quit and stop spotifyd", &["Q"]),
];

/// A key as the table knows it: Shift is part of the character ('G'), not a modifier.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Key {
    pub fn of(ev: &KeyEvent) -> Key {
        let mut mods = ev.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        if matches!(ev.code, KeyCode::Char(_) | KeyCode::BackTab) {
            mods.remove(KeyModifiers::SHIFT);
        }
        // Ctrl+Backspace comes in many forms (see `is_ctrl_backspace` in app.rs): one key here.
        if matches!(ev.code, KeyCode::Backspace) && mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return Key { code: KeyCode::Backspace, mods: KeyModifiers::CONTROL };
        }
        let code = match ev.code {
            KeyCode::Char(c) if mods.contains(KeyModifiers::CONTROL) => KeyCode::Char(c.to_ascii_lowercase()),
            code => code,
        };
        Key { code, mods }
    }

    /// As help shows it: "^f", "space", "⏎", "⇧↑".
    pub fn label(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "space".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "⏎".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "⇧tab".into(),
            KeyCode::Backspace => "⌫".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::PageUp => "pgup".into(),
            KeyCode::PageDown => "pgdn".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::Delete => "del".into(),
            KeyCode::F(n) => format!("f{n}"),
            _ => "?".into(),
        };
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push('^');
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("alt-");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push('⇧');
        }
        out + &base
    }
}

/// Reads a key as written in config.toml: "n", "G", "ctrl-f", "shift-up", "space", "enter".
pub fn parse(text: &str) -> Result<Key, String> {
    let bad = || format!("“{text}” is not a key (try “n”, “ctrl-f”, “shift-up”, “space”, “enter”)");
    let mut mods = KeyModifiers::NONE;
    let mut rest = text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let (m, len) = if lower.starts_with("ctrl-") || lower.starts_with("ctrl+") {
            (KeyModifiers::CONTROL, 5)
        } else if lower.starts_with("alt-") || lower.starts_with("alt+") {
            (KeyModifiers::ALT, 4)
        } else if lower.starts_with("shift-") || lower.starts_with("shift+") {
            (KeyModifiers::SHIFT, 6)
        } else {
            break;
        };
        if rest.len() == len {
            break; // "ctrl-" alone is not a key; "-" alone is.
        }
        mods |= m;
        rest = &rest[len..];
    }
    let code = match rest.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" if mods.contains(KeyModifiers::SHIFT) => {
            mods.remove(KeyModifiers::SHIFT);
            KeyCode::BackTab
        }
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => {
            KeyCode::F(f[1..].parse().unwrap_or(1))
        }
        _ => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => {
                    // "shift-g" is "G".
                    let c = if mods.contains(KeyModifiers::SHIFT) { c.to_ascii_uppercase() } else { c };
                    KeyCode::Char(c)
                }
                _ => return Err(bad()),
            }
        }
    };
    if matches!(code, KeyCode::Char(c) if c.is_ascii_digit()) && mods.is_empty() {
        return Err(format!("“{text}”: the digits go to the tabs"));
    }
    Ok(Key::of(&KeyEvent::new(code, mods)))
}

/// One action or several keys, as `[keys]` in config.toml takes them.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
pub enum Spec {
    One(String),
    Many(Vec<String>),
}

/// The keys in use: the defaults, with `[keys]` from config.toml on top.
pub struct Keys {
    map: HashMap<Key, Act>,
    by_act: HashMap<Act, Vec<Key>>,
}

impl Default for Keys {
    fn default() -> Self {
        Keys::new(&HashMap::new()).0
    }
}

impl Keys {
    /// The table with `custom` applied, and what in `custom` was wrong. An action named there
    /// gets only the keys named (none for ""); a key it takes leaves its default action.
    pub fn new(custom: &HashMap<String, Spec>) -> (Keys, Vec<String>) {
        let mut problems = Vec::new();
        let mut chosen: HashMap<Act, Vec<Key>> = HashMap::new();
        let mut names: Vec<_> = custom.iter().collect();
        names.sort_by_key(|(n, _)| n.as_str());
        let mut taken: HashMap<Key, &str> = HashMap::new();
        for (name, spec) in names {
            let Some(info) = ALL.iter().find(|i| i.name == name.as_str()) else {
                problems.push(format!("[keys] {name}: no such action"));
                continue;
            };
            let texts = match spec {
                Spec::One(t) => vec![t.clone()],
                Spec::Many(v) => v.clone(),
            };
            let mut keys = Vec::new();
            for t in texts.iter().filter(|t| !t.is_empty() && t.as_str() != "none") {
                match parse(t) {
                    Ok(k) => match taken.insert(k, info.name) {
                        Some(other) if other != info.name => {
                            problems.push(format!("[keys] “{t}” is set for both {other} and {name}"));
                        }
                        _ => keys.push(k),
                    },
                    Err(e) => problems.push(format!("[keys] {name}: {e}")),
                }
            }
            chosen.insert(info.act, keys);
        }
        let mut map = HashMap::new();
        let mut by_act: HashMap<Act, Vec<Key>> = HashMap::new();
        for info in ALL {
            let keys = match chosen.get(&info.act) {
                Some(keys) => keys.clone(),
                None => info.keys.iter().filter_map(|t| parse(t).ok()).filter(|k| !taken.contains_key(k)).collect(),
            };
            for k in &keys {
                map.entry(*k).or_insert(info.act);
            }
            by_act.insert(info.act, keys);
        }
        (Keys { map, by_act }, problems)
    }

    /// The action of a key press, if it has one.
    pub fn get(&self, ev: &KeyEvent) -> Option<Act> {
        self.map.get(&Key::of(ev)).copied()
    }

    pub fn keys(&self, act: Act) -> &[Key] {
        self.by_act.get(&act).map_or(&[], Vec::as_slice)
    }

    /// The first key of an action, as help shows it ("" if it has none).
    pub fn show(&self, act: Act) -> String {
        self.keys(act).first().map(|k| k.label()).unwrap_or_default()
    }

    /// Every key of an action, as help shows them: "j ↓".
    pub fn show_all(&self, act: Act) -> String {
        self.keys(act).iter().map(|k| k.label()).collect::<Vec<_>>().join(" ")
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn the_default_table_has_no_double_keys_and_every_action_has_a_key() {
        let mut seen: HashMap<Key, &str> = HashMap::new();
        for info in ALL {
            assert!(SECTIONS.contains(&info.section), "{}", info.name);
            for t in info.keys {
                let k = parse(t).unwrap_or_else(|e| panic!("{}: {e}", info.name));
                if let Some(other) = seen.insert(k, info.name) {
                    panic!("{t} is set for {other} and {}", info.name);
                }
            }
        }
        let keys = Keys::default();
        assert_eq!(keys.get(&ev(KeyCode::Char('G'), KeyModifiers::SHIFT)), Some(Act::Bottom), "shift is part of the letter");
        assert_eq!(keys.get(&ev(KeyCode::Char('f'), KeyModifiers::CONTROL)), Some(Act::Filter));
        assert_eq!(keys.get(&ev(KeyCode::Char('F'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)), Some(Act::Filter));
        assert_eq!(keys.get(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)), Some(Act::PrevSection));
        assert_eq!(keys.get(&ev(KeyCode::Up, KeyModifiers::SHIFT)), Some(Act::VolumeUp));
        assert_eq!(keys.get(&ev(KeyCode::Up, KeyModifiers::NONE)), Some(Act::Up));
        assert_eq!(keys.get(&ev(KeyCode::Backspace, KeyModifiers::ALT)), Some(Act::UndoTree));
        assert_eq!(keys.show_all(Act::Down), "j ↓");
        assert_eq!(keys.show(Act::Filter), "^f");
    }

    #[test]
    fn config_keys_replace_the_defaults_and_mistakes_are_named() {
        let mut custom = HashMap::new();
        custom.insert("next".to_owned(), Spec::Many(vec!["N".into(), "ctrl-n".into()]));
        custom.insert("like".to_owned(), Spec::One("n".into()));
        custom.insert("mute".to_owned(), Spec::One("".into()));
        let (keys, problems) = Keys::new(&custom);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(keys.get(&ev(KeyCode::Char('n'), KeyModifiers::NONE)), Some(Act::Like), "taken from next");
        assert_eq!(keys.get(&ev(KeyCode::Char('N'), KeyModifiers::SHIFT)), Some(Act::Next));
        assert_eq!(keys.get(&ev(KeyCode::Char('n'), KeyModifiers::CONTROL)), Some(Act::Next));
        assert_eq!(keys.get(&ev(KeyCode::Char('f'), KeyModifiers::NONE)), None, "like has only its new key");
        assert_eq!(keys.get(&ev(KeyCode::Char('m'), KeyModifiers::NONE)), None, "mute has none");

        let mut custom = HashMap::new();
        custom.insert("nxt".to_owned(), Spec::One("n".into()));
        custom.insert("like".to_owned(), Spec::One("ctrl-".into()));
        custom.insert("next".to_owned(), Spec::One("x".into()));
        custom.insert("queue".to_owned(), Spec::One("x".into()));
        custom.insert("mute".to_owned(), Spec::One("3".into()));
        let (_, problems) = Keys::new(&custom);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("both next and queue")), "{problems:?}");
    }
}
