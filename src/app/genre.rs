//! Randomize: the genre picker, its samples (Tab) and the volume fades around them.

use super::*;

impl App {
    /// `z`: the genre picker (on a Randomize page, it rolls again).
    pub(super) fn randomize_key(&mut self) {
        if let Some(Page::Random(_)) = self.page() {
            self.reroll();
        } else {
            self.open_genre_picker();
        }
    }

    pub fn open_genre_picker(&mut self) {
        self.menu = None;
        self.genre_picker = Some(crate::genres::Picker::new());
    }

    /// Keys of the genre picker: type to search, ↑↓ move, → ← open and close, ⏎ choose.
    pub(super) fn genre_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let genres = std::mem::take(&mut self.genre_list);
        if self.genre_picker.is_some() {
            match key.code {
                KeyCode::Tab => {
                    let genre = self.genre_picker.as_ref().and_then(|p| p.genre_at(&genres, p.cursor));
                    if let Some(g) = genre {
                        self.start_sample(g);
                    }
                    self.genre_list = genres;
                    return;
                }
                KeyCode::Esc if self.sample.is_some() => {
                    self.stop_sample();
                    self.genre_list = genres;
                    return;
                }
                _ => {}
            }
        }
        if let Some(p) = self.genre_picker.as_mut() {
            match key.code {
                KeyCode::Esc if !p.query.is_empty() => p.set_query(String::new()),
                KeyCode::Esc => {
                    self.genre_picker = None;
                    self.stop_sample();
                }
                KeyCode::Down => p.move_by(&genres, 1),
                KeyCode::Up => p.move_by(&genres, -1),
                KeyCode::Char('n') if ctrl => p.move_by(&genres, 1),
                KeyCode::Char('p') if ctrl => p.move_by(&genres, -1),
                KeyCode::PageDown => p.move_by(&genres, 10),
                KeyCode::PageUp => p.move_by(&genres, -10),
                KeyCode::Right => p.open_close(&genres, true),
                KeyCode::Left => p.open_close(&genres, false),
                KeyCode::Enter => {
                    let at = p.cursor;
                    self.genre_choose(&genres, at);
                }
                _ => {
                    let mut q = p.query.clone();
                    if q.edit(&key) == Edit::Changed {
                        p.set_query(q);
                    } else {
                        p.query = q;
                    }
                }
            }
        }
        self.genre_list = genres;
    }

    /// Chooses picker row `i` (⏎ or a click): a genre randomizes, a folder opens or closes.
    pub fn genre_choose(&mut self, genres: &[crate::genres::Genre], i: usize) {
        let Some(p) = self.genre_picker.as_mut() else { return };
        if let crate::genres::Choice::Randomize(genre) = p.choose(genres, i) {
            self.genre_picker = None;
            self.stop_sample();
            self.randomize(genre);
        }
    }

    /// Tab in the genre picker: about 10 s of the genre's top song (Tab again: the next one).
    /// What plays now fades out, the sample fades in; when it ends, the same back.
    pub(super) fn start_sample(&mut self, genre: String) {
        let nth = match &self.sample {
            Some(s) if s.genre == genre => s.nth + 1,
            _ => 0,
        };
        // A way back still waiting in a fade (Tab again while it went back): keep it.
        let pending = self.fade.iter_mut().find_map(|step| match step {
            FadeStep::Send(Cmd::Unsample { restore }) => Some(restore.take()),
            _ => None,
        });
        if let Some(restore) = pending {
            self.sample_restore = restore;
        } else if self.sample.is_none() {
            // What played before the first sample, and its volume.
            let pb = self.playback.as_ref();
            self.sample_volume = pb.and_then(|p| p.device.as_ref()).and_then(|d| d.volume_percent);
            self.sample_restore = pb.and_then(|p| {
                Some(crate::worker::Restore {
                    context: p.context.as_ref().map(|c| c.uri.clone()),
                    track: p.item.as_ref()?.uri.clone(),
                    position_ms: self.progress_ms(),
                    playing: p.is_playing,
                })
            });
        }
        let playing = self.playback.as_ref().is_some_and(|p| p.is_playing);
        self.sample = Some(Sample { genre: genre.clone(), nth, track: None, started: None });
        self.fade.clear();
        self.fade_at = None;
        if let (Some(v), true) = (self.sample_volume, playing) {
            self.fade.push_back(FadeStep::Ramp { to: v / 10, len: FADE_OUT });
        } else if let Some(v) = self.sample_volume {
            // Nothing is heard now: start quiet, then fade in.
            self.set_volume(v / 10);
        }
        self.fade.push_back(FadeStep::Send(Cmd::Sample { genre, nth }));
    }

    /// Ends the sample (time is up, Esc, or the picker closes): fades out, goes back to what
    /// played, and fades it in.
    pub(super) fn stop_sample(&mut self) {
        if self.sample.take().is_none() {
            return;
        }
        let restore = self.sample_restore.take();
        self.fade.clear();
        self.fade_at = None;
        match self.sample_volume {
            Some(v) => {
                self.fade.push_back(FadeStep::Ramp { to: v / 10, len: FADE_OUT });
                self.fade.push_back(FadeStep::Send(Cmd::Unsample { restore }));
                self.fade.push_back(FadeStep::Wait(SWITCH_WAIT));
                self.fade.push_back(FadeStep::Ramp { to: v, len: FADE_IN });
            }
            None => self.fade.push_back(FadeStep::Send(Cmd::Unsample { restore })),
        }
        self.poll_soon();
    }

    /// Opens the Randomize page of `genre` (or goes to it) and rolls it.
    pub(super) fn randomize(&mut self, genre: String) {
        if self.open_page(Page::Random(RandomPage::new(genre.clone()))) {
            self.send(Cmd::Randomize(genre));
        }
    }

    /// Rolls the open Randomize page again: 50 new random songs.
    pub(super) fn reroll(&mut self) {
        let Some(Page::Random(r)) = self.page_mut() else { return };
        if r.rolling {
            return;
        }
        r.rolling = true;
        let genre = r.genre.clone();
        self.send(Cmd::Randomize(genre));
    }
}
