//! The login screen: shown before the app when there is no saved login, or when Spotify ended
//! it. It opens Spotify's login page in the browser and waits for the browser to come back, so
//! you never need `spotlite auth` in a second shell. The token it saves is refreshed from then
//! on, so the browser is needed again only if Spotify ends the login.

use crate::auth::{self, Token};
use crate::config::Config;
use crate::theme;
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

enum Step {
    /// Nothing started: Enter opens the browser.
    Ready,
    /// The browser is open on `url`; a thread waits for Spotify's answer.
    Waiting { url: String, answer: Receiver<Result<Token>>, since: Instant },
    /// The login is done: shown for a moment, then the app starts.
    Done(Instant),
}

struct Screen {
    reason: String,
    step: Step,
    /// The last thing that went wrong, shown in red.
    error: Option<String>,
    /// A short note (e.g. "link copied"), shown until the next key.
    note: Option<String>,
}

/// Shows the login screen until the login works (true) or you quit (false).
pub fn screen(terminal: &mut DefaultTerminal, cfg: &Config, reason: &str) -> Result<bool> {
    let mut s = Screen { reason: reason.to_owned(), step: Step::Ready, error: None, note: None };
    loop {
        if let Step::Waiting { answer, .. } = &s.step {
            match answer.try_recv() {
                Ok(Ok(_)) => s.step = Step::Done(Instant::now()),
                Ok(Err(e)) => {
                    s.error = Some(format!("{e:#}"));
                    s.step = Step::Ready;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    s.error = Some("the login stopped; try again".into());
                    s.step = Step::Ready;
                }
            }
        }
        if let Step::Done(at) = s.step {
            if at.elapsed() >= Duration::from_millis(600) {
                return Ok(true);
            }
        }
        terminal.draw(|f| draw(f, &s))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        s.note = None;
        let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            _ if ctrl_c => return Ok(false),
            KeyCode::Char('q') | KeyCode::Esc => return Ok(false),
            KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('l') => match &s.step {
                Step::Ready => start(&mut s, cfg),
                // The listener is already up: open the same page again.
                Step::Waiting { url, .. } => {
                    let _ = open::that_detached(url);
                    s.note = Some("Opened the login page again.".into());
                }
                Step::Done(_) => return Ok(true),
            },
            KeyCode::Char('c') | KeyCode::Char('y') => {
                if let Step::Waiting { url, .. } = &s.step {
                    copy(url);
                    s.note = Some("Copied the link (your terminal must allow OSC 52).".into());
                }
            }
            _ => {}
        }
    }
}

/// Listens for the callback, opens the browser, and waits for the answer on a thread.
fn start(s: &mut Screen, cfg: &Config) {
    s.error = None;
    let pending = match auth::begin(cfg) {
        Ok(p) => p,
        Err(e) => {
            s.error = Some(format!("{e:#}"));
            return;
        }
    };
    let url = pending.url.clone();
    let (tx, answer) = mpsc::channel();
    let cfg = cfg.clone();
    let spawned = std::thread::Builder::new().name("spotlite-login".into()).spawn(move || {
        let _ = tx.send(pending.finish(&cfg));
    });
    if let Err(e) = spawned {
        s.error = Some(e.to_string());
        return;
    }
    let _ = open::that_detached(&url);
    s.step = Step::Waiting { url, answer, since: Instant::now() };
}

/// Puts `text` on the clipboard through the terminal (OSC 52).
fn copy(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

fn draw(f: &mut Frame, s: &Screen) {
    let t = theme::c();
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().fg(t.text)), area);

    let key = |k: &'static str, what: &'static str| {
        Line::from(vec![
            Span::styled(format!("{k:>5}  "), theme::accent().add_modifier(Modifier::BOLD)),
            Span::styled(what, theme::muted()),
        ])
    };
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{}  spotlite", t.logo),
            theme::bright().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    let mut url_shown = None;
    match &s.step {
        Step::Ready => {
            lines.push(Line::from(Span::styled(s.reason.clone(), theme::text())));
            lines.push(Line::from(Span::styled(
                "spotlite keeps the login, so you do this one time.",
                theme::muted(),
            )));
            lines.push(Line::raw(""));
            lines.push(key("⏎", "log in with Spotify in the browser"));
            lines.push(key("q", "quit"));
        }
        Step::Waiting { url, since, .. } => {
            let dots = ".".repeat(1 + (since.elapsed().as_millis() / 400 % 3) as usize);
            lines.push(Line::from(Span::styled(
                format!("Log in in the browser{dots}"),
                theme::text(),
            )));
            lines.push(Line::from(Span::styled(
                "spotlite starts when Spotify sends the browser back.",
                theme::muted(),
            )));
            lines.push(Line::raw(""));
            lines.push(key("⏎", "open the login page again"));
            lines.push(key("c", "copy the link"));
            lines.push(key("q", "quit"));
            url_shown = Some(url.as_str());
        }
        Step::Done(_) => {
            lines.push(Line::from(Span::styled(
                "Logged in.",
                Style::new().fg(t.green).add_modifier(Modifier::BOLD),
            )));
        }
    }
    if let Some(e) = &s.error {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(e.clone(), Style::new().fg(t.red))));
    }
    if let Some(n) = &s.note {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(n.clone(), theme::faint())));
    }

    let width = area.width.saturating_sub(4).min(72);
    let inner_width = width.saturating_sub(6).max(1);
    let text_rows: u16 = lines
        .iter()
        .map(|l| (l.width() as u16).div_ceil(inner_width).max(1))
        .sum();
    let height = (text_rows + 4).min(area.height);
    let [row] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(area);
    let [boxed] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(row);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(t.corners)
        .border_style(theme::border(true))
        .title(Line::from(Span::styled(format!(" {} ", theme::title("Login")), theme::accent())).alignment(Alignment::Left));
    let inner = block.inner(boxed);
    f.render_widget(block, boxed);
    let text_area = Rect::new(inner.x + 2, inner.y + 1, inner.width.saturating_sub(4), inner.height.saturating_sub(1));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), text_area);

    // The link below the box, without a frame, so the terminal can select it in one piece.
    if let Some(url) = url_shown {
        let below = Rect::new(area.x + 2, boxed.bottom() + 1, area.width.saturating_sub(4), area.bottom().saturating_sub(boxed.bottom() + 1));
        if below.height > 0 {
            let p = Paragraph::new(vec![
                Line::from(Span::styled("If the browser did not open, go to:", theme::faint())),
                Line::from(Span::styled(url.to_owned(), theme::faint())),
            ])
            .wrap(Wrap { trim: false });
            f.render_widget(p, below);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(s: &Screen) -> String {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, s)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_owned()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn ready_and_waiting_screens() {
        let mut s = Screen { reason: "You are not logged in.".into(), step: Step::Ready, error: None, note: None };
        let ready = render(&s);
        println!("{ready}");
        assert!(ready.contains("You are not logged in."));
        assert!(ready.contains("log in with Spotify in the browser"));

        let (_tx, answer) = mpsc::channel();
        s.step = Step::Waiting { url: "https://accounts.spotify.com/authorize?x=1".into(), answer, since: Instant::now() };
        s.error = Some("cannot listen".into());
        let waiting = render(&s);
        println!("{waiting}");
        assert!(waiting.contains("copy the link"));
        assert!(waiting.contains("https://accounts.spotify.com/authorize?x=1"));
        assert!(waiting.contains("cannot listen"));
    }
}
