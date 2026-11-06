use crate::api::{Play, Playback, Playlist, Spotify, Track};
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};

/// How often to ask Spotify for the player state. Progress is interpolated between polls.
const POLL_EVERY: Duration = Duration::from_secs(5);
const VOLUME_STEP: u8 = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Playlists,
    Liked,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Search, Tab::Playlists, Tab::Liked];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Search => "Search",
            Tab::Playlists => "Playlists",
            Tab::Liked => "Liked",
        }
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

pub struct App {
    pub api: Spotify,
    pub tab: Tab,
    pub typing: bool,
    pub query: String,
    pub results: Vec<Track>,
    pub playlists: Option<Vec<Playlist>>,
    pub liked: Option<Vec<Track>>,
    pub selected: [usize; 3],
    pub playback: Option<Playback>,
    pub polled_at: Instant,
    pub status: String,
    pub quit: bool,
    device_id: Option<String>,
    next_poll: Instant,
}

impl App {
    pub fn new(api: Spotify) -> Self {
        let now = Instant::now();
        App {
            api,
            tab: Tab::Search,
            typing: true,
            query: String::new(),
            results: Vec::new(),
            playlists: None,
            liked: None,
            selected: [0; 3],
            playback: None,
            polled_at: now,
            status: String::new(),
            quit: false,
            device_id: None,
            next_poll: now,
        }
    }

    /// Runs `f` and puts any error in the status line instead of crashing the UI.
    fn attempt(&mut self, f: impl FnOnce(&mut Self) -> Result<()>) {
        if let Err(e) = f(self) {
            self.status = format!("{e:#}");
        }
    }

    pub fn tick(&mut self) {
        if Instant::now() >= self.next_poll {
            self.next_poll = Instant::now() + POLL_EVERY;
            self.attempt(|app| {
                app.playback = app.api.playback()?;
                app.polled_at = Instant::now();
                Ok(())
            });
        }
    }

    /// Ask for fresh player state soon (Spotify needs a moment to apply a command).
    fn poll_soon(&mut self) {
        self.next_poll = Instant::now() + Duration::from_millis(400);
    }

    /// Current progress in ms, moved forward locally since the last poll.
    pub fn progress_ms(&self) -> u64 {
        let Some(pb) = &self.playback else { return 0 };
        let base = pb.progress_ms.unwrap_or(0);
        let elapsed = if pb.is_playing {
            self.polled_at.elapsed().as_millis() as u64
        } else {
            0
        };
        let duration = pb.item.as_ref().map_or(u64::MAX, |t| t.duration_ms);
        (base + elapsed).min(duration)
    }

    pub fn list_len(&self) -> usize {
        match self.tab {
            Tab::Search => self.results.len(),
            Tab::Playlists => self.playlists.as_ref().map_or(0, Vec::len),
            Tab::Liked => self.liked.as_ref().map_or(0, Vec::len),
        }
    }

    pub fn selected(&self) -> usize {
        self.selected[self.tab.index()]
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let cur = self.selected() as isize;
        self.selected[self.tab.index()] = (cur + delta).clamp(0, len as isize - 1) as usize;
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.typing = false;
        self.attempt(|app| app.load_tab(false));
    }

    fn load_tab(&mut self, force: bool) -> Result<()> {
        match self.tab {
            Tab::Search => {}
            Tab::Playlists if force || self.playlists.is_none() => {
                self.playlists = Some(self.api.playlists()?);
            }
            Tab::Liked if force || self.liked.is_none() => {
                self.liked = Some(self.api.liked_tracks()?);
            }
            _ => {}
        }
        Ok(())
    }

    fn device(&mut self) -> Result<Option<String>> {
        if self.device_id.is_none() {
            self.device_id = self.api.find_device()?.and_then(|d| d.id);
            if self.device_id.is_none() {
                self.status = format!(
                    "device '{}' not found; is spotifyd running?",
                    self.api.config().device_name
                );
            }
        }
        Ok(self.device_id.clone())
    }

    fn play_selected(&mut self) -> Result<()> {
        let device = self.device()?;
        let i = self.selected();
        match self.tab {
            Tab::Search => {
                let uris: Vec<String> = self.results.iter().map(|t| t.uri.clone()).collect();
                if i < uris.len() {
                    self.api.play(device.as_deref(), Play::Tracks(&uris, i))?;
                }
            }
            Tab::Liked => {
                let uris: Vec<String> = self.liked.iter().flatten().map(|t| t.uri.clone()).collect();
                if i < uris.len() {
                    self.api.play(device.as_deref(), Play::Tracks(&uris, i))?;
                }
            }
            Tab::Playlists => {
                if let Some(pl) = self.playlists.as_ref().and_then(|p| p.get(i)).cloned() {
                    self.api.play(device.as_deref(), Play::Context(&pl.uri))?;
                }
            }
        }
        self.status.clear();
        self.poll_soon();
        Ok(())
    }

    fn toggle(&mut self) -> Result<()> {
        let device = self.device()?;
        match &self.playback {
            Some(pb) if pb.is_playing => self.api.pause(device.as_deref())?,
            _ => self.api.play(device.as_deref(), Play::Resume)?,
        }
        self.poll_soon();
        Ok(())
    }

    fn change_volume(&mut self, up: bool) -> Result<()> {
        let device = self.device()?;
        let current = self
            .playback
            .as_ref()
            .and_then(|pb| pb.device.as_ref())
            .and_then(|d| d.volume_percent)
            .unwrap_or(50);
        let new = if up {
            current.saturating_add(VOLUME_STEP).min(100)
        } else {
            current.saturating_sub(VOLUME_STEP)
        };
        self.api.set_volume(device.as_deref(), new)?;
        if let Some(d) = self.playback.as_mut().and_then(|pb| pb.device.as_mut()) {
            d.volume_percent = Some(new);
        }
        Ok(())
    }

    fn search(&mut self) -> Result<()> {
        let q = self.query.trim().to_owned();
        if q.is_empty() {
            return Ok(());
        }
        self.results = self.api.search_tracks(&q)?;
        self.selected[Tab::Search.index()] = 0;
        self.status = format!("{} results", self.results.len());
        Ok(())
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.typing {
            match key.code {
                KeyCode::Esc => self.typing = false,
                KeyCode::Enter => {
                    self.typing = false;
                    self.attempt(Self::search);
                }
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) => self.query.push(c),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('/') => {
                self.tab = Tab::Search;
                self.typing = true;
                self.query.clear();
            }
            KeyCode::Tab => {
                let next = Tab::ALL[(self.tab.index() + 1) % Tab::ALL.len()];
                self.switch_tab(next);
            }
            KeyCode::BackTab => {
                let prev = Tab::ALL[(self.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len()];
                self.switch_tab(prev);
            }
            KeyCode::Char('1') => self.switch_tab(Tab::Search),
            KeyCode::Char('2') => self.switch_tab(Tab::Playlists),
            KeyCode::Char('3') => self.switch_tab(Tab::Liked),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Char('g') | KeyCode::Home => self.move_selection(isize::MIN / 2),
            KeyCode::Char('G') | KeyCode::End => self.move_selection(isize::MAX / 2),
            KeyCode::Enter => self.attempt(Self::play_selected),
            KeyCode::Char(' ') => self.attempt(Self::toggle),
            KeyCode::Char('n') => self.attempt(|app| {
                let device = app.device()?;
                app.api.next(device.as_deref())?;
                app.poll_soon();
                Ok(())
            }),
            KeyCode::Char('p') => self.attempt(|app| {
                let device = app.device()?;
                app.api.previous(device.as_deref())?;
                app.poll_soon();
                Ok(())
            }),
            KeyCode::Char('+') | KeyCode::Char('=') => self.attempt(|app| app.change_volume(true)),
            KeyCode::Char('-') => self.attempt(|app| app.change_volume(false)),
            KeyCode::Char('s') => self.attempt(|app| {
                let device = app.device()?;
                let on = !app.playback.as_ref().is_some_and(|pb| pb.shuffle_state);
                app.api.set_shuffle(device.as_deref(), on)?;
                app.status = format!("shuffle {}", if on { "on" } else { "off" });
                app.poll_soon();
                Ok(())
            }),
            KeyCode::Char('r') => self.attempt(|app| {
                app.device_id = None;
                app.load_tab(true)?;
                app.poll_soon();
                Ok(())
            }),
            _ => {}
        }
    }
}
