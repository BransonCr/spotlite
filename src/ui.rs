use crate::api::{Playlist, Track};
use crate::app::{App, Listing, Tab, View};
use crate::theme::{self, ACCENT, BRIGHT, FAINT, GREEN, LINE, MUTED, RED, SELECTED_BG};
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Margin, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, HighlightSpacing, Padding, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table,
};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub fn format_ms(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Cuts `s` to at most `width` columns, ending with "…" when it was cut.
pub fn fit(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

fn panel<'a>() -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border(false))
        .padding(Padding::horizontal(1))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, _, body, now, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(f.area().inner(Margin::new(1, 0)));

    draw_header(f, app, header);
    match app.view() {
        View::Search => draw_search(f, app, body),
        View::Playlists => draw_playlists(f, app, body),
        View::Liked => {
            let title = Line::from(Span::styled(" Liked Songs ", theme::bright()));
            draw_tracks(f, app, body, title, "No liked songs.");
        }
        View::Playlist => {
            let name = app.open.as_ref().map_or("", |o| o.playlist.name.as_str()).to_owned();
            let title = Line::from(vec![
                Span::styled(" ‹ ", theme::accent()),
                Span::styled(format!("{name} "), theme::bright()),
            ]);
            draw_tracks(f, app, body, title, "This playlist is empty.");
        }
    }
    draw_now_playing(f, app, now);
    draw_footer(f, app, footer);
    if app.help {
        draw_help(f);
    }
}

fn count_label<T>(l: &Listing<T>) -> String {
    match (l.loading, l.total) {
        (true, _) => " loading… ".into(),
        (false, Some(t)) if l.items.len() < t => format!(" {} of {t} ", l.items.len()),
        (false, Some(t)) => format!(" {t} "),
        (false, None) => String::new(),
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut left = vec![Span::styled("♪ spotlite", theme::accent().add_modifier(Modifier::BOLD)), Span::raw("   ")];
    for tab in Tab::ALL {
        let count = match tab {
            Tab::Search => None,
            Tab::Playlists => app.playlists.total,
            Tab::Liked => app.liked.total,
        };
        let style = if tab == app.tab {
            Style::new().fg(BRIGHT).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            theme::muted()
        };
        left.push(Span::styled(tab.title(), style));
        if let Some(n) = count {
            left.push(Span::styled(format!(" {n}"), theme::faint()));
        }
        left.push(Span::raw("   "));
    }

    let ours = app.device.as_deref();
    let active = app
        .playback
        .as_ref()
        .and_then(|pb| pb.device.as_ref())
        .is_some_and(|d| d.is_active);
    let (dot, label) = match ours {
        Some(name) => (if active { GREEN } else { MUTED }, name.to_owned()),
        None if app.device_checked => (RED, "no device".into()),
        None => (FAINT, "connecting".into()),
    };
    let mut right = vec![Span::styled("● ", Style::new().fg(dot)), Span::styled(label, theme::muted())];
    if let Some(v) = app.volume() {
        right.push(Span::styled(format!("   vol {v}%"), theme::muted()));
    }
    let right = Line::from(right);
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)]).areas(area);
    f.render_widget(Line::from(left), l);
    f.render_widget(right, r);
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let [input, list] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(area);
    let block = panel()
        .border_style(theme::border(app.typing))
        .title(Span::styled(" Search ", if app.typing { theme::accent() } else { theme::muted() }));
    let inner = block.inner(input);
    let content = if app.query.is_empty() && !app.typing {
        Line::from(vec![
            Span::styled("/ ", theme::accent()),
            Span::styled("search songs, artists or albums", theme::faint()),
        ])
    } else {
        Line::from(vec![Span::styled("/ ", theme::accent()), Span::styled(app.query.as_str(), theme::text())])
    };
    f.render_widget(Paragraph::new(content).block(block), input);
    if app.typing {
        let x = inner.x + 2 + app.query.width() as u16;
        f.set_cursor_position(Position::new(x.min(inner.right().saturating_sub(1)), inner.y));
    }

    let title = if app.searched.is_empty() {
        Line::from(Span::styled(" Results ", theme::bright()))
    } else {
        Line::from(vec![
            Span::styled(" Results for ", theme::muted()),
            Span::styled(format!("“{}” ", app.searched), theme::bright()),
        ])
    };
    let empty = if app.searched.is_empty() {
        "Type a song, artist or album, then press Enter."
    } else {
        "No results."
    };
    draw_tracks(f, app, list, title, empty);
}

/// Widths the table will give each column, so cell text can be cut with "…".
/// Mirrors Table's own layout: one cell for the "▌" marker, then columns 2 apart.
fn column_widths(width: u16, widths: &[Constraint]) -> Vec<usize> {
    Layout::horizontal(widths.to_vec())
        .spacing(2)
        .split(Rect::new(0, 0, width.saturating_sub(1), 1))
        .iter()
        .map(|r| r.width as usize)
        .collect()
}

/// Hides the album, then the artist column on narrow terminals.
fn track_columns(width: u16) -> (bool, bool) {
    (width >= 50, width >= 76)
}

fn draw_tracks(f: &mut Frame, app: &mut App, area: Rect, title: Line, empty: &str) {
    let playing = app.playing_uri().map(str::to_owned);
    let Some(listing) = (match app.view() {
        View::Search => Some(&mut app.search),
        View::Liked => Some(&mut app.liked),
        View::Playlist => app.open.as_mut().map(|o| &mut o.tracks),
        View::Playlists => None,
    }) else {
        return;
    };
    let block = panel()
        .title(title)
        .title(Line::from(Span::styled(count_label(listing), theme::faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);

    if listing.items.is_empty() {
        let msg = if listing.loading { "Loading…" } else { empty };
        draw_centered(f, inner, msg);
        return;
    }

    let (show_artist, show_album) = track_columns(inner.width);
    let mut widths = vec![Constraint::Length(4), Constraint::Fill(3)];
    let mut head = vec![Cell::from(Line::from("#").right_aligned()), Cell::from("Title")];
    if show_artist {
        widths.push(Constraint::Fill(2));
        head.push(Cell::from("Artist"));
    }
    if show_album {
        widths.push(Constraint::Fill(2));
        head.push(Cell::from("Album"));
    }
    widths.push(Constraint::Length(5));
    head.push(Cell::from(Line::from("Time").right_aligned()));

    let cols = column_widths(inner.width, &widths);
    let rows = listing.items.iter().enumerate().map(|(i, t): (usize, &Track)| {
        let is_playing = playing.as_deref() == Some(t.uri.as_str());
        let num = if is_playing {
            Span::styled("♪", theme::accent())
        } else {
            Span::styled((i + 1).to_string(), theme::faint())
        };
        let mut cells = vec![
            Cell::from(Line::from(num).right_aligned()),
            Cell::from(Span::styled(fit(&t.name, cols[1]), if is_playing { theme::accent() } else { theme::text() })),
        ];
        if show_artist {
            cells.push(Cell::from(Span::styled(fit(&t.artist_names(), cols[2]), theme::muted())));
        }
        if show_album {
            cells.push(Cell::from(Span::styled(fit(t.album_name(), cols[3]), theme::faint())));
        }
        cells.push(Cell::from(Line::from(Span::styled(format_ms(t.duration_ms), theme::faint())).right_aligned()));
        Row::new(cells)
    });
    draw_table(f, inner, area, rows.collect(), widths, head, &mut listing.state, listing.items.len());
}

fn draw_playlists(f: &mut Frame, app: &mut App, area: Rect) {
    let playing_ctx = app
        .playback
        .as_ref()
        .and_then(|pb| pb.context.as_ref())
        .map(|c| c.uri.clone());
    let user = app.user_id.clone();
    let listing = &mut app.playlists;
    let block = panel()
        .title(Span::styled(" Playlists ", theme::bright()))
        .title(Line::from(Span::styled(count_label(listing), theme::faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if listing.items.is_empty() {
        draw_centered(f, inner, if listing.loading { "Loading…" } else { "No playlists." });
        return;
    }

    let show_owner = inner.width >= 50;
    let mut widths = vec![Constraint::Length(4), Constraint::Fill(3)];
    let mut head = vec![Cell::from(Line::from("#").right_aligned()), Cell::from("Name")];
    if show_owner {
        widths.push(Constraint::Fill(2));
        head.push(Cell::from("Owner"));
    }
    widths.push(Constraint::Length(6));
    head.push(Cell::from(Line::from("Tracks").right_aligned()));

    let cols = column_widths(inner.width, &widths);
    let rows = listing.items.iter().enumerate().map(|(i, p): (usize, &Playlist)| {
        let is_playing = playing_ctx.as_deref() == Some(p.uri.as_str());
        let mine = p.collaborative || user.as_deref() == Some(p.owner.id.as_str());
        let num = if is_playing {
            Span::styled("♪", theme::accent())
        } else {
            Span::styled((i + 1).to_string(), theme::faint())
        };
        let mut cells = vec![
            Cell::from(Line::from(num).right_aligned()),
            Cell::from(Span::styled(fit(&p.name, cols[1]), if is_playing { theme::accent() } else { theme::text() })),
        ];
        if show_owner {
            let owner = if mine { "you" } else { p.owner_name() };
            cells.push(Cell::from(Span::styled(fit(owner, cols[2]), theme::muted())));
        }
        let count = p.track_count().map(|n| n.to_string()).unwrap_or_default();
        cells.push(Cell::from(Line::from(Span::styled(count, theme::faint())).right_aligned()));
        Row::new(cells)
    });
    let len = listing.items.len();
    draw_table(f, inner, area, rows.collect(), widths, head, &mut listing.state, len);
}

#[allow(clippy::too_many_arguments)]
fn draw_table(
    f: &mut Frame,
    inner: Rect,
    outer: Rect,
    rows: Vec<Row>,
    widths: Vec<Constraint>,
    head: Vec<Cell>,
    state: &mut ratatui::widgets::TableState,
    len: usize,
) {
    let table = Table::new(rows, widths)
        .header(Row::new(head).style(theme::faint()).bottom_margin(1))
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        .highlight_symbol(Text::from(Span::styled("▌", theme::accent())))
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, inner, state);

    let visible = inner.height.saturating_sub(2) as usize;
    if len > visible {
        let mut sb = ScrollbarState::new(len.saturating_sub(visible)).position(state.offset());
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::new().fg(LINE))
                .thumb_symbol("┃")
                .thumb_style(Style::new().fg(MUTED)),
            outer.inner(Margin::new(0, 1)),
            &mut sb,
        );
    }
}

fn draw_centered(f: &mut Frame, area: Rect, msg: &str) {
    let [mid] = Layout::vertical([Constraint::Length(1)]).flex(Flex::Center).areas(area);
    f.render_widget(Paragraph::new(msg).style(theme::faint()).alignment(Alignment::Center), mid);
}

fn draw_now_playing(f: &mut Frame, app: &App, area: Rect) {
    let block = panel();
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [l1, l2, l3] = Layout::vertical([Constraint::Length(1); 3]).areas(inner);

    let pb = app.playback.as_ref();
    let Some(track) = pb.and_then(|p| p.item.as_ref()) else {
        f.render_widget(Line::from(Span::styled("Nothing playing", theme::muted())), l1);
        let hint = if app.device_checked && app.device.is_none() {
            Span::styled(
                "spotifyd not found. Start it: systemctl --user start spotifyd",
                Style::new().fg(RED),
            )
        } else {
            Span::styled("Pick a track and press Enter, or Space to resume.", theme::faint())
        };
        f.render_widget(Line::from(hint), l2);
        return;
    };
    let pb = pb.expect("track implies playback");

    // Line 1: state, title, like; shuffle and repeat on the right.
    let mode_on = |on: bool| if on { theme::accent() } else { theme::faint() };
    let repeat = match pb.repeat_state.as_str() {
        "track" => "↻1",
        _ => "↻ ",
    };
    let modes = Line::from(vec![
        Span::styled("⇄", mode_on(pb.shuffle_state)),
        Span::raw("  "),
        Span::styled(repeat, mode_on(pb.repeat_state != "off")),
    ]);
    let [title_area, modes_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(modes.width() as u16)]).areas(l1);
    let liked = matches!(&app.saved, Some((uri, true)) if *uri == track.uri);
    let room = (title_area.width as usize).saturating_sub(3 + if liked { 3 } else { 0 } + 2);
    let mut title = vec![
        Span::styled(if pb.is_playing { "▶  " } else { "‖  " }, if pb.is_playing { theme::accent() } else { theme::muted() }),
        Span::styled(fit(&track.name, room), theme::bright()),
    ];
    if liked {
        title.push(Span::styled("  ♥", theme::accent()));
    }
    f.render_widget(Line::from(title), title_area);
    f.render_widget(modes, modes_area);

    // Line 2: artist · album, and where it plays from.
    let from = app.context_name().map(|n| format!("from {n}")).unwrap_or_default();
    let from_width = (from.width() as u16).min(l2.width / 3);
    let [who_area, from_area] = Layout::horizontal([Constraint::Min(0), Constraint::Length(from_width)])
        .spacing(if from.is_empty() { 0 } else { 3 })
        .areas(l2);
    let mut who = track.artist_names();
    if !track.album_name().is_empty() {
        who = format!("{who}  ·  {}", track.album_name());
    }
    let who = fit(&who, (who_area.width as usize).saturating_sub(3));
    f.render_widget(Line::from(vec![Span::raw("   "), Span::styled(who, theme::muted())]), who_area);
    f.render_widget(
        Line::from(Span::styled(fit(&from, from_width as usize), theme::faint())).right_aligned(),
        from_area,
    );

    // Line 3: progress bar.
    let progress = app.progress_ms();
    f.render_widget(progress_line(progress, track.duration_ms, pb.is_playing, l3.width), l3);
}

/// `1:05 ━━━━━━━━──────────── 5:30`, filling the given width.
pub fn progress_line(progress: u64, duration: u64, playing: bool, width: u16) -> Line<'static> {
    let left = format!("   {} ", format_ms(progress));
    let right = format!(" {}", format_ms(duration));
    let bar = (width as usize).saturating_sub(left.width() + right.width());
    let ratio = if duration > 0 { (progress as f64 / duration as f64).clamp(0.0, 1.0) } else { 0.0 };
    let filled = (ratio * bar as f64).round() as usize;
    let fill_style = if playing { theme::accent() } else { theme::muted() };
    Line::from(vec![
        Span::styled(left, theme::muted()),
        Span::styled("━".repeat(filled), fill_style),
        Span::styled("─".repeat(bar - filled), Style::new().fg(LINE)),
        Span::styled(right, theme::muted()),
    ])
}

fn hints(app: &App) -> &'static [(&'static str, &'static str)] {
    if app.typing {
        return &[("⏎", "search"), ("esc", "done"), ("^u", "clear")];
    }
    match app.view() {
        View::Search => &[("/", "search"), ("⏎", "play"), ("a", "queue"), ("␣", "pause"), ("←→", "seek"), ("?", "help")],
        View::Playlists => &[("⏎", "open"), ("P", "play all"), ("␣", "pause"), ("n/p", "skip"), ("?", "help")],
        View::Playlist => &[("⏎", "play"), ("P", "play all"), ("a", "queue"), ("esc", "back"), ("?", "help")],
        View::Liked => &[("⏎", "play"), ("a", "queue"), ("f", "like"), ("␣", "pause"), ("?", "help")],
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.toast {
        Some(t) if t.error => Line::from(vec![
            Span::styled(" ✕ ", Style::new().fg(RED)),
            Span::styled(t.text.as_str(), Style::new().fg(RED)),
        ]),
        Some(t) => Line::from(vec![
            Span::styled(" ✓ ", Style::new().fg(GREEN)),
            Span::styled(t.text.as_str(), theme::text()),
        ]),
        None => {
            let mut spans = vec![Span::raw(" ")];
            for (i, (key, what)) in hints(app).iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled("  ·  ", Style::new().fg(LINE)));
                }
                spans.push(Span::styled(*key, theme::muted()));
                spans.push(Span::styled(format!(" {what}"), theme::faint()));
            }
            Line::from(spans)
        }
    };
    f.render_widget(line, area);
}

const HELP: &[(&str, &str)] = &[
    ("/", "search"),
    ("1 2 3, tab", "switch tab"),
    ("j k, ↑ ↓", "move"),
    ("g G, ^d ^u", "top, bottom, page"),
    ("⏎", "play, or open a playlist"),
    ("P", "play the whole playlist"),
    ("esc", "back to playlists"),
    ("a", "add to queue"),
    ("space", "play / pause"),
    ("n p", "next / previous"),
    ("← →", "seek 10 s"),
    ("+ -", "volume"),
    ("s", "shuffle"),
    ("r", "repeat: off, all, one"),
    ("f", "like / unlike the song"),
    ("R", "reload the list"),
    ("q", "quit"),
];

fn draw_help(f: &mut Frame) {
    let height = HELP.len() as u16 + 4;
    let [area] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(f.area());
    let [area] = Layout::horizontal([Constraint::Length(46)]).flex(Flex::Center).areas(area);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, v)| Line::from(vec![Span::styled(format!("{k:>12}"), theme::accent()), Span::styled(format!("   {v}"), theme::text())]))
        .collect();
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(Span::styled(" keys ", theme::accent()))
                .title_bottom(Line::from(Span::styled(" any key to close ", theme::faint())).right_aligned())
                .padding(Padding::uniform(1)),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Device, Page, Playback};
    use crate::app::test_app;
    use crate::worker::{List, Msg};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn track(name: &str, artist: &str, album: &str) -> Track {
        serde_json::from_value(serde_json::json!({
            "uri": format!("spotify:track:{name}"), "name": name, "duration_ms": 330_000,
            "artists": [{"name": artist}], "album": {"name": album}
        }))
        .unwrap()
    }

    #[test]
    fn format_ms_works() {
        assert_eq!(format_ms(0), "0:00");
        assert_eq!(format_ms(61_500), "1:01");
        assert_eq!(format_ms(3_600_000), "60:00");
    }

    #[test]
    fn fit_cuts_with_ellipsis() {
        assert_eq!(fit("Teardrop", 20), "Teardrop");
        assert_eq!(fit("Teardrop", 5), "Tear…");
        assert_eq!(fit("日本語の歌", 6), "日本…");
        assert!(fit("日本語の歌", 6).width() <= 6);
    }

    #[test]
    fn progress_line_fills_width() {
        let line = progress_line(165_000, 330_000, true, 60);
        assert_eq!(line.width(), 60);
        let s: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.contains("2:45") && s.contains("5:30"));
        let (done, left) = (s.matches('━').count() as i32, s.matches('─').count() as i32);
        assert!((done - left).abs() <= 1, "{s}");
    }

    #[test]
    fn renders_results_and_now_playing() {
        let (mut app, _rx) = test_app();
        app.typing = false;
        app.searched = "teardrop".into();
        let t = track("Teardrop", "Massive Attack", "Mezzanine");
        app.on_msg(Msg::Tracks {
            list: List::Search("teardrop".into()),
            offset: 0,
            page: Page { items: vec![t.clone(), track("Angel", "Massive Attack", "Mezzanine")], raw: 2, total: 2 },
        });
        app.on_msg(Msg::Playback(Some(Playback {
            device: Some(Device { id: Some("d".into()), name: "spotlite".into(), is_active: true, volume_percent: Some(40) }),
            is_playing: true,
            progress_ms: Some(65_000),
            shuffle_state: true,
            repeat_state: "context".into(),
            context: None,
            item: Some(t.clone()),
        })));
        app.on_msg(Msg::Saved { uri: t.uri.clone(), saved: true });
        let out = screen(&mut app, 100, 24);
        assert!(out.contains("♪ spotlite"), "{out}");
        assert!(out.contains("● spotlite   vol 40%"), "{out}");
        assert!(out.contains("Results for “teardrop”"), "{out}");
        assert!(out.contains("Title") && out.contains("Album"), "{out}");
        assert!(out.contains("♪  Teardrop"), "playing row is marked: {out}");
        assert!(out.contains("Angel"), "{out}");
        assert!(out.contains("▶  Teardrop  ♥"), "{out}");
        assert!(out.contains("1:05 ━"), "{out}");
        assert!(out.contains("⇄"), "{out}");

        // Long cells end with "…" instead of being cut mid-word.
        let long = "A Very Long Song Title That Cannot Possibly Fit In The Column";
        app.search.items[1].name = long.into();
        let out = screen(&mut app, 100, 24);
        assert!(out.contains("A Very Long Song") && out.contains('…') && !out.contains("In The Column"), "{out}");

        // Narrow terminals drop the album column but still render.
        let narrow = screen(&mut app, 60, 20);
        assert!(!narrow.contains("Album") && narrow.contains("Teardrop"), "{narrow}");
    }

    #[test]
    fn renders_empty_states_and_help() {
        let (mut app, _rx) = test_app();
        let out = screen(&mut app, 90, 20);
        assert!(out.contains("Type a song, artist or album"), "{out}");
        assert!(out.contains("/ search songs, artists or albums"), "{out}");
        assert!(out.contains("Nothing playing"), "{out}");
        assert!(out.contains("connecting"), "{out}");

        app.on_msg(Msg::Device(None));
        let out = screen(&mut app, 90, 20);
        assert!(out.contains("no device") && out.contains("systemctl --user start spotifyd"), "{out}");

        app.help = true;
        let out = screen(&mut app, 90, 26);
        assert!(out.contains("like / unlike the song"), "{out}");
    }

    #[test]
    fn renders_playlists_with_owner() {
        let (mut app, _rx) = test_app();
        app.on_msg(Msg::User(serde_json::from_value(serde_json::json!({"id": "me", "uri": "spotify:user:me"})).unwrap()));
        app.tab = Tab::Playlists;
        let pls: Vec<Playlist> = serde_json::from_value(serde_json::json!([
            {"id": "1", "uri": "spotify:playlist:1", "name": "Jammer", "owner": {"id": "me"}, "items": {"total": 174}},
            {"id": "2", "uri": "spotify:playlist:2", "name": "Night Runners", "owner": {"id": "x", "display_name": "Xander"}, "items": {"total": 28}}
        ]))
        .unwrap();
        app.on_msg(Msg::Playlists { offset: 0, page: Page { items: pls, raw: 2, total: 2 } });
        let out = screen(&mut app, 90, 20);
        assert!(out.contains("Jammer") && out.contains("you") && out.contains("174"), "{out}");
        assert!(out.contains("Xander"), "{out}");
        assert!(out.contains("Playlists 2"), "{out}");
    }
}
