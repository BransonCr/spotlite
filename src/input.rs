//! A one-line text box with a cursor, for search, the palette, the pickers and the list
//! filter: ← → Home End (Ctrl+A, Ctrl+E), Ctrl+← → by word, Backspace, Delete, Ctrl+Backspace
//! (or Ctrl+W, Alt+Backspace) for the word before, Ctrl+U for all before, Ctrl+K for all after,
//! and paste.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    text: String,
    /// The cursor, as a byte index into `text` (always on a char boundary).
    at: usize,
}

/// What a key did to the text box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// The text changed.
    Changed,
    /// Only the cursor moved.
    Moved,
    /// Not a text box key.
    Ignored,
}

impl From<&str> for Input {
    fn from(text: &str) -> Self {
        Input::from(text.to_owned())
    }
}

impl From<String> for Input {
    fn from(text: String) -> Self {
        Input { at: text.len(), text }
    }
}

impl std::ops::Deref for Input {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl PartialEq<&str> for Input {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl std::fmt::Display for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// Ctrl+Backspace, as terminals send it: ^H (most, ghostty too), ^W (some), or Backspace with
/// Ctrl or Alt (kitty keyboard protocol, or Alt as a fallback).
pub fn is_ctrl_backspace(key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Backspace => key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT),
        KeyCode::Char('h') | KeyCode::Char('w') => ctrl,
        _ => false,
    }
}

impl Input {
    /// Columns before the cursor (where the terminal cursor goes).
    pub fn cursor_cols(&self) -> u16 {
        self.text[..self.at].width() as u16
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.at = self.text.len();
    }

    pub fn clear(&mut self) {
        self.set(String::new());
    }

    /// Puts `text` in at the cursor (typing or paste); line breaks become spaces.
    pub fn insert(&mut self, text: &str) {
        let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        self.text.insert_str(self.at, &clean);
        self.at += clean.len();
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(j, _)| j)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    /// The start of the word before `i` (spaces after it are part of the jump).
    fn word_back(&self, i: usize) -> usize {
        let before = self.text[..i].trim_end();
        before.rfind(char::is_whitespace).map_or(0, |j| j + 1)
    }

    fn word_forward(&self, i: usize) -> usize {
        let rest = &self.text[i..];
        let skip = rest.len() - rest.trim_start().len();
        let word = rest[skip..].find(char::is_whitespace).unwrap_or(rest.len() - skip);
        i + skip + word
    }

    /// Runs a key on the text box.
    pub fn edit(&mut self, key: &KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let before = (self.text.len(), self.at);
        if is_ctrl_backspace(key) {
            let start = self.word_back(self.at);
            self.text.replace_range(start..self.at, "");
            self.at = start;
            return if self.text.len() != before.0 { Edit::Changed } else { Edit::Moved };
        }
        match key.code {
            KeyCode::Char('a') if ctrl => self.at = 0,
            KeyCode::Char('e') if ctrl => self.at = self.text.len(),
            KeyCode::Char('u') if ctrl => {
                self.text.replace_range(..self.at, "");
                self.at = 0;
            }
            KeyCode::Char('k') if ctrl => self.text.truncate(self.at),
            KeyCode::Char(c) if !ctrl && !alt => {
                let mut buf = [0u8; 4];
                self.insert(c.encode_utf8(&mut buf));
            }
            KeyCode::Backspace => {
                let start = self.prev(self.at);
                self.text.replace_range(start..self.at, "");
                self.at = start;
            }
            KeyCode::Delete => {
                let end = self.next(self.at);
                self.text.replace_range(self.at..end, "");
            }
            KeyCode::Left if ctrl || alt => self.at = self.word_back(self.at),
            KeyCode::Right if ctrl || alt => self.at = self.word_forward(self.at),
            KeyCode::Left => self.at = self.prev(self.at),
            KeyCode::Right => self.at = self.next(self.at),
            KeyCode::Home => self.at = 0,
            KeyCode::End => self.at = self.text.len(),
            _ => return Edit::Ignored,
        }
        let moved_only = matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End)
            || (ctrl && matches!(key.code, KeyCode::Char('a' | 'e')));
        if moved_only && self.text.len() == before.0 {
            Edit::Moved
        } else {
            Edit::Changed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut i = Input::from("aphex twin");
        assert_eq!(i.edit(&k(KeyCode::Left)), Edit::Moved);
        i.edit(&k(KeyCode::Left));
        i.edit(&k(KeyCode::Left));
        i.edit(&k(KeyCode::Char('X')));
        assert_eq!(i, "aphex tXwin");
        i.edit(&k(KeyCode::Backspace));
        i.edit(&k(KeyCode::Delete));
        assert_eq!(i, "aphex tin");
        i.edit(&ctrl('a'));
        i.insert("the\nnew ");
        assert_eq!(i, "the new aphex tin", "paste: a line break becomes a space");
        i.edit(&ctrl('e'));
        i.edit(&ctrl('w'));
        assert_eq!(i, "the new aphex ", "the word before");
        i.edit(&KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(i.cursor_cols(), 8);
        i.edit(&ctrl('k'));
        assert_eq!(i, "the new ");
        i.edit(&k(KeyCode::Home));
        i.edit(&KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(i.cursor_cols(), 3);
        i.edit(&ctrl('u'));
        assert_eq!(i, " new ");
        assert_eq!(i.edit(&k(KeyCode::Enter)), Edit::Ignored);
    }

    #[test]
    fn wide_and_accented_letters_move_as_one() {
        let mut i = Input::from("café 東京");
        i.edit(&k(KeyCode::Left));
        assert_eq!(i.cursor_cols(), 7, "東 is two columns wide");
        i.edit(&k(KeyCode::Backspace));
        assert_eq!(i, "café 京");
        i.edit(&k(KeyCode::Home));
        i.edit(&k(KeyCode::Right));
        i.edit(&k(KeyCode::Right));
        i.edit(&k(KeyCode::Right));
        i.edit(&k(KeyCode::Delete));
        assert_eq!(i, "caf 京");
    }
}
