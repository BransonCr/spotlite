//! Keys: every key press goes through `on_key`, then the key table (`run`).

use super::*;

impl App {
    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.help {
            self.help_key(key);
            return;
        }
        if self.undo.is_some() {
            self.undo_key(key);
            return;
        }
        if self.tabs_list.is_some() && key.code == KeyCode::Esc {
            self.tabs_list = None;
            return;
        }
        if self.confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    if let Some(c) = self.confirm.take() {
                        self.send(c.cmd);
                    }
                }
                _ => self.confirm = None,
            }
            return;
        }
        if self.palette.is_some() {
            match key.code {
                KeyCode::Esc => self.palette = None,
                KeyCode::Enter => self.run_palette(),
                KeyCode::Down => self.move_palette(1),
                KeyCode::Up => self.move_palette(-1),
                KeyCode::Char('n') if ctrl => self.move_palette(1),
                KeyCode::Char('p') if ctrl => self.move_palette(-1),
                _ => {
                    if let Some(p) = self.palette.as_mut() {
                        if p.query.edit(&key) == Edit::Changed {
                            p.cursor = 0;
                        }
                    }
                }
            }
            return;
        }
        if self.genre_picker.is_some() {
            self.genre_key(key);
            return;
        }
        if self.picker.is_some() {
            match key.code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Enter => self.pick_playlist(),
                KeyCode::Down => self.move_popup(1),
                KeyCode::Up => self.move_popup(-1),
                KeyCode::Char('n') if ctrl => self.move_popup(1),
                KeyCode::Char('p') if ctrl => self.move_popup(-1),
                _ => {
                    if let Some(p) = self.picker.as_mut() {
                        if p.query.edit(&key) == Edit::Changed {
                            p.cursor = 0;
                        }
                    }
                }
            }
            return;
        }
        if self.menu.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('.') => self.close_menu(),
                KeyCode::Char('F') if self.menu.as_ref().is_some_and(|m| m.track.is_none() && !m.is_theme()) => {
                    self.close_menu()
                }
                KeyCode::Char('T') if self.menu.as_ref().is_some_and(Menu::is_theme) => self.close_menu(),
                KeyCode::Char('V') if self.side_fx_before.is_some() => self.close_menu(),
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('l') => self.run_menu(),
                KeyCode::Down | KeyCode::Char('j') => self.move_popup(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_popup(-1),
                KeyCode::Char(c) => {
                    // A key hint runs its entry directly.
                    let hit = self.menu.as_ref().and_then(|m| {
                        m.items.iter().position(|(k, _, _)| k.len() == c.len_utf8() && k.starts_with(c))
                    });
                    if let Some(i) = hit {
                        if let Some(m) = self.menu.as_mut() {
                            m.cursor = i;
                        }
                        self.run_menu();
                    }
                }
                _ => {}
            }
            return;
        }
        if !self.typing && is_ctrl_backspace(&key) {
            self.open_undo();
            return;
        }
        if !self.typing && self.side_key(key) {
            return;
        }
        if self.filtering {
            self.filter_key(key);
            return;
        }
        if self.typing {
            match key.code {
                KeyCode::Esc => {
                    self.typing = false;
                    self.search_due = None;
                }
                // ↑ ↓ go through earlier searches; ↓ past the newest leaves the box.
                KeyCode::Up if !self.search_history.is_empty() => {
                    let next = self.history_at.map_or(0, |i| (i + 1).min(self.search_history.len() - 1));
                    self.history_at = Some(next);
                    self.query.set(self.search_history[next].clone());
                    self.search_due = Some(Instant::now() + SEARCH_DELAY);
                }
                KeyCode::Down if self.history_at.is_some_and(|i| i > 0) => {
                    let next = self.history_at.unwrap_or(1) - 1;
                    self.history_at = Some(next);
                    self.query.set(self.search_history[next].clone());
                    self.search_due = Some(Instant::now() + SEARCH_DELAY);
                }
                KeyCode::Down => {
                    self.typing = false;
                    self.history_at = None;
                }
                KeyCode::Enter => self.submit_search(),
                _ => {
                    if self.query.edit(&key) == Edit::Changed {
                        self.search_due = Some(Instant::now() + SEARCH_DELAY);
                    }
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char(c @ '1'..='9') if !ctrl => {
                if let Some(loc) = self.locs().get(c as usize - '1' as usize).cloned() {
                    self.go(loc);
                }
            }
            _ => {
                if let Some(act) = self.keys.get(&key) {
                    self.run(act);
                }
            }
        }
    }

    /// Text pasted into the terminal: it goes to the text box that is open, else to search.
    pub fn on_paste(&mut self, text: &str) {
        if let Some(p) = self.palette.as_mut() {
            p.query.insert(text);
            p.cursor = 0;
        } else if let Some(p) = self.picker.as_mut() {
            p.query.insert(text);
            p.cursor = 0;
        } else if let Some(p) = self.genre_picker.as_mut() {
            let mut q = p.query.clone();
            q.insert(text);
            p.set_query(q);
        } else if self.help {
            self.help_query.insert(text);
            self.help_scroll = 0;
        } else if self.menu.is_none() && self.confirm.is_none() && self.undo.is_none() {
            if !self.typing {
                self.switch_tab(Tab::Search);
                self.typing = true;
            }
            self.query.insert(text);
            self.search_due = Some(Instant::now() + SEARCH_DELAY);
        }
    }

    /// Keys in help: type to filter, ↑ ↓ to scroll, Esc (or ? with no filter) to close.
    pub(super) fn help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc if !self.help_query.is_empty() => self.help_query.clear(),
            KeyCode::Esc | KeyCode::Enter => self.help = false,
            KeyCode::Char('?') if self.help_query.is_empty() => self.help = false,
            KeyCode::Down => self.help_scroll += 1,
            KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
            KeyCode::PageDown => self.help_scroll += 10,
            KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
            _ => {
                if self.help_query.edit(&key) == Edit::Changed {
                    self.help_scroll = 0;
                }
            }
        }
    }

    /// Runs one action of the key table (a key, the palette, or a click).
    pub fn run(&mut self, act: Act) {
        let page = 10;
        match act {
            Act::Quit => self.quit = true,
            Act::QuitStop => {
                self.quit = true;
                self.stop_services = true;
            }
            Act::Visualizer => {
                self.viz_on = !self.viz_on;
                self.viz_at = Some(Instant::now());
                if self.viz_on && crate::theme::c().viz == crate::theme::Viz::None {
                    // Without this the visualizer is on, but nothing shows.
                    self.info(format!("The theme “{}” has no visualizer (viz = \"none\"): pick a style in its file", crate::theme::name()));
                } else {
                    self.info(if self.viz_on { "Visualizer on" } else { "Visualizer off" });
                }
            }
            Act::SideEffect => self.open_side_fx(),
            Act::Help => {
                self.help = true;
                self.help_query.clear();
                self.help_scroll = 0;
            }
            Act::Search => {
                self.switch_tab(Tab::Search);
                self.typing = true;
            }
            Act::Palette => self.palette = Some(Default::default()),
            Act::UndoTree => self.open_undo(),
            Act::NextSection | Act::PrevSection if self.view() == View::Artist => {
                let step = if act == Act::NextSection { 1 } else { -1 };
                if let Some(a) = self.artist_page_mut() {
                    a.jump_section(step);
                }
            }
            Act::NextTab | Act::NextSection => self.cycle_tab(1),
            Act::PrevTab | Act::PrevSection => self.cycle_tab(-1),
            Act::CloseTab => self.close_page(),
            Act::SearchType if self.view() == View::Search => {
                let i = SearchKind::ALL.iter().position(|k| *k == self.search_kind).unwrap_or(0);
                self.set_search_kind(SearchKind::ALL[(i + 1) % SearchKind::ALL.len()]);
            }
            Act::SearchType => {}
            Act::Theme => self.open_themes(),
            Act::Pin => self.pin_selected(),
            Act::Card => {
                if let Some(t) = self.target_track() {
                    self.share_card(t);
                }
            }
            Act::Discover if self.view() == View::Radio => self.discover_round(),
            Act::Discover if self.view() == View::Random => self.reroll(),
            Act::Discover => {}
            Act::Back => self.back(),
            Act::Actions => self.open_menu(),
            Act::Artist => {
                if let Some(t) = self.target_track() {
                    self.go_to_artist(&t);
                }
            }
            Act::Album => {
                if let Some(t) = self.target_track() {
                    self.go_to_album(&t);
                }
            }
            Act::Radio => {
                let radio_row = match self.page() {
                    Some(Page::Artist(a)) => match a.rows.current() {
                        Some(ArtistRow::Radio(seed)) => Some(seed.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(seed) = radio_row {
                    self.start_radio(seed);
                } else if let Some(t) = self.target_track() {
                    self.start_radio(t);
                }
            }
            Act::AddToPlaylist => {
                if let Some(t) = self.selected_track() {
                    self.open_picker(t);
                }
            }
            Act::Remove => {
                if let Some(t) = self.selected_track() {
                    self.ask_remove(t);
                }
            }
            Act::Filters => self.open_filters(),
            Act::Jump => self.jump_to_playing(),
            Act::VolumeUp => self.change_volume(true),
            Act::VolumeDown => self.change_volume(false),
            Act::Down => self.move_selection(1),
            Act::Up => self.move_selection(-1),
            Act::PageDown => self.move_selection(page),
            Act::PageUp => self.move_selection(-page),
            Act::Top => self.move_selection(isize::MIN / 2),
            Act::Bottom => self.move_selection(isize::MAX / 2),
            Act::Play => self.play_selected(),
            Act::PlayAll => self.play_whole_playlist(),
            Act::Queue => self.queue_selected(),
            Act::PlayPause => self.toggle_play(),
            Act::Next => self.skip(true),
            Act::Previous => self.skip(false),
            Act::Forward => self.seek(true),
            Act::Rewind => self.seek(false),
            Act::Mute => self.toggle_mute(),
            Act::Shuffle => self.toggle_shuffle(),
            Act::Repeat => self.cycle_repeat(),
            Act::Like => self.toggle_like(None),
            Act::Save if self.view() == View::Radio => self.ask_save_radio(),
            Act::Save if self.view() == View::Random => self.ask_save_random(),
            Act::Save => {}
            Act::Randomize => self.randomize_key(),
            Act::Reload => self.reload(),
            Act::Sidebar => self.toggle_sidebar(),
            Act::SidePlaying => self.show_side(SideView::Playing),
            Act::SideLibrary => self.show_side(SideView::Library),
            Act::Filter => self.start_filter(),
            Act::Sort => self.sort_list(false),
            Act::SortReverse => self.sort_list(true),
            Act::Mark => self.mark(),
            Act::MarkAll => self.mark_all(),
            Act::Undo => self.undo_change(),
            Act::AddAgain => self.add_again(),
            Act::Devices => self.send(Cmd::Devices),
        }
    }

    /// Runs a palette entry.
    pub fn run_entry(&mut self, run: crate::palette::Run) {
        use crate::palette::Run;
        match run {
            Run::Act(act) => self.run(act),
            Run::Go(loc) => self.go(loc),
            Run::Playlist(pl) => {
                self.go(Loc::Base(Tab::Playlists));
                if self.owns(&pl) || crate::client::AVAILABLE {
                    self.open_playlist(pl);
                } else {
                    self.play_playlist(pl);
                }
            }
            Run::Theme(name) => self.try_theme(&name, true),
            Run::Genre(genre) => self.randomize(genre),
            Run::Search(q) => {
                self.switch_tab(Tab::Search);
                self.query.set(q.clone());
                self.run_search(q);
            }
        }
    }
}
