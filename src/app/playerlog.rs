//! The player's log over everything (J, or a click on the device): spotifyd's journal or the
//! built-in player's file, so a song that does not play can be explained without a terminal.

use super::*;

/// Log lines read at most.
const MAX_LINES: usize = 500;

pub struct PlayerLog {
    /// Where the lines come from (a command or a file).
    pub source: String,
    /// The lines, oldest first; Err: why the log cannot be read.
    pub lines: Result<Vec<String>, String>,
    /// Rows up from the end (0: the newest rows show).
    pub scroll: usize,
    /// The most `scroll` can be, from the last frame (the rows that do not fit).
    pub max_scroll: usize,
    /// Rows the last frame showed, for a page up or down.
    pub page: usize,
}

impl PlayerLog {
    pub fn read() -> PlayerLog {
        let (source, lines) = crate::engine::log_tail(MAX_LINES);
        PlayerLog { source, lines, scroll: 0, max_scroll: 0, page: 10 }
    }

    fn scroll_by(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta).min(self.max_scroll);
    }
}

impl App {
    pub(super) fn open_player_log(&mut self) {
        self.player_log = Some(PlayerLog::read());
    }

    /// Keys in the log: ↑ ↓ (j k) and pgup pgdn scroll, g G go to the oldest or newest line,
    /// r reads the log again, Esc (or the key that opened it) closes it.
    pub(super) fn player_log_key(&mut self, key: KeyEvent) {
        let opener = self.keys.get(&key) == Some(Act::PlayerLog);
        let Some(log) = self.player_log.as_mut() else { return };
        let page = log.page.max(1) as isize;
        match key.code {
            _ if opener => self.player_log = None,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.player_log = None,
            KeyCode::Up | KeyCode::Char('k') => log.scroll_by(1),
            KeyCode::Down | KeyCode::Char('j') => log.scroll_by(-1),
            KeyCode::PageUp => log.scroll_by(page),
            KeyCode::PageDown => log.scroll_by(-page),
            KeyCode::Home | KeyCode::Char('g') => log.scroll = log.max_scroll,
            KeyCode::End | KeyCode::Char('G') => log.scroll = 0,
            KeyCode::Char('r') | KeyCode::Char('R') => *log = PlayerLog::read(),
            _ => {}
        }
    }

    /// The wheel scrolls the log; a click outside it closes it.
    pub(super) fn player_log_mouse(&mut self, m: MouseEvent, pos: Position) {
        let outside = !self.hits.popup.contains(pos);
        let Some(log) = self.player_log.as_mut() else { return };
        match m.kind {
            MouseEventKind::ScrollUp => log.scroll_by(3),
            MouseEventKind::ScrollDown => log.scroll_by(-3),
            MouseEventKind::Down(_) if outside => self.player_log = None,
            _ => {}
        }
    }
}
