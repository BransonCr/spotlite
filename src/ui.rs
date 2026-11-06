use crate::api::{pick_image, ImageRef, Playlist, Track};
use crate::app::{App, Hits, Listing, Row as PlRow, Tab, View};
use crate::images::Images;
use crate::viz;
use crate::theme::{self, ACCENT, BRIGHT, FAINT, GREEN, LINE, MUTED, RED, SELECTED_BG};
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Margin, Position, Rect};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, HighlightSpacing, Padding, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, TableState,
};
use ratatui::Frame;
use ratatui_image::Image;
use unicode_width::UnicodeWidthStr;

/// Height of the player panel, borders included; the album art is this tall minus 2.
const PLAYER_HEIGHT: u16 = 6;
/// Rows per playlist (the cover thumbnail is this tall).
const PLAYLIST_ROW: u16 = 2;
/// Cells of the volume slider's bar.
const VOLUME_BAR: u16 = 12;

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

/// A `width`-cell rect at `x` on the row of `area` (clipped to it).
fn span_rect(area: Rect, x: u16, width: u16) -> Rect {
    let x = x.min(area.right());
    Rect::new(x, area.y, width.min(area.right() - x), 1)
}

/// Draws a cover picture, or a grey square with a note while it loads (or if there is none).
fn draw_cover(f: &mut Frame, images: &mut Images, list: &[ImageRef], area: Rect) {
    let (fw, _) = images.font_size();
    let min_px = u32::from(area.width) * u32::from(fw);
    if let Some(protocol) = pick_image(list, min_px).and_then(|url| images.get(url, area.width, area.height)) {
        f.render_widget(Image::new(protocol), area);
        return;
    }
    let note = if area.height >= 3 { "♪" } else { "" };
    let mut lines = vec![Line::raw(""); (area.height / 2) as usize];
    lines.push(Line::from(note).centered());
    f.render_widget(
        Paragraph::new(lines).style(Style::new().bg(SELECTED_BG).fg(FAINT)),
        area,
    );
}

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits = Hits::default();
    let [header, _, body, now, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(PLAYER_HEIGHT),
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
            app.hits.back = span_rect(body, body.x + 1, title.width() as u16);
            draw_tracks(f, app, body, title, "This playlist is empty.");
        }
    }
    draw_now_playing(f, app, now);
    draw_footer(f, app, footer);
    if app.viz_on {
        // Inside the main panel, rising from its bottom edge: that is where the empty space is.
        draw_visualizer(f.buffer_mut(), body.inner(Margin::new(1, 1)), &app.viz.frame());
    }
    if app.help {
        draw_help(f);
    }
}

/// Visualizer colours: a cool steel blue held close to black, so the bars read as a faint
/// layer behind the amber UI. Bottom to top.
const VIZ_SHADES: [Color; 5] = [
    Color::Rgb(0x0b, 0x10, 0x16),
    Color::Rgb(0x0e, 0x15, 0x1d),
    Color::Rgb(0x12, 0x1a, 0x25),
    Color::Rgb(0x16, 0x20, 0x2d),
    Color::Rgb(0x1b, 0x27, 0x36),
];
/// Bar tops in eighths of a cell, for smooth heights.
const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
/// Bars reach at most this share of the height.
const VIZ_MAX: f32 = 0.55;

/// Level for column `x` of `width`, between the two nearest bands.
fn viz_column(frame: &viz::Frame, x: u16, width: u16) -> f32 {
    let pos = x as f32 / width.saturating_sub(1).max(1) as f32 * (viz::BANDS - 1) as f32;
    let (i, t) = (pos.floor() as usize, pos.fract());
    let j = (i + 1).min(viz::BANDS - 1);
    // Smoothstep between bands: rounded curves instead of straight steps.
    let t = t * t * (3.0 - 2.0 * t);
    frame.levels[i] * (1.0 - t) + frame.levels[j] * t
}

/// Draws the spectrum behind the UI, rising from the bottom of `area`: smooth bars (eighth
/// blocks) in empty cells only. Text, the selection and pictures are left alone, and so is
/// every cell the bars do not reach, so most frames change one cell per column.
pub fn draw_visualizer(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    if area.height == 0 || frame.levels.iter().all(|&l| l < 0.01) {
        return;
    }
    let max = area.height as f32 * VIZ_MAX;
    for x in area.left()..area.right() {
        let height = viz_column(frame, x - area.x, area.width) * max;
        let full = height.floor() as u16;
        let part = ((height - full as f32) * 8.0).round() as usize;
        for k in 0..=full.min(area.height - 1) {
            let symbol = if k < full { EIGHTHS[8] } else { EIGHTHS[part] };
            if symbol == " " {
                continue;
            }
            let y = area.bottom() - 1 - k;
            let cell = &mut buf[(x, y)];
            let sym = cell.symbol();
            if cell.skip || cell.bg != Color::Reset || sym != " " {
                continue;
            }
            let shade = VIZ_SHADES[(k as usize * VIZ_SHADES.len() / area.height.max(1) as usize).min(VIZ_SHADES.len() - 1)];
            cell.set_symbol(symbol).set_fg(shade);
        }
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

fn draw_header(f: &mut Frame, app: &mut App, area: Rect) {
    let mut left = vec![Span::styled("♪ spotlite", theme::accent().add_modifier(Modifier::BOLD)), Span::raw("   ")];
    let mut x = area.x + Line::from(left.clone()).width() as u16;
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
        let mut spans = vec![Span::styled(tab.title(), style)];
        if let Some(n) = count {
            spans.push(Span::styled(format!(" {n}"), theme::faint()));
        }
        let w = Line::from(spans.clone()).width() as u16;
        app.hits.tabs.push((span_rect(area, x, w), tab));
        x += w + 3;
        left.extend(spans);
        left.push(Span::raw("   "));
    }

    let active = app
        .playback
        .as_ref()
        .and_then(|pb| pb.device.as_ref())
        .is_some_and(|d| d.is_active);
    let (dot, label) = match app.device.as_deref() {
        Some(name) => (if active { GREEN } else { MUTED }, name.to_owned()),
        None if app.device_checked => (RED, "no device".into()),
        None => (FAINT, "connecting".into()),
    };
    let left_w = Line::from(left.clone()).width() as u16;
    let mut right = vec![Span::styled("●", Style::new().fg(dot))];
    // The device name goes only where it does not cover the tabs.
    if left_w + 2 + label.width() as u16 <= area.width {
        right.push(Span::styled(format!(" {label}"), theme::muted()));
    }
    let right = Line::from(right);
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)]).areas(area);
    f.render_widget(Line::from(left), l);
    f.render_widget(right, r);
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let [input, list] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(area);
    app.hits.search_box = input;
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
    // A one-row album thumbnail after the number.
    let thumb = app.images.square_cols(1);
    let mut widths = vec![Constraint::Length(4), Constraint::Length(thumb), Constraint::Fill(3)];
    let mut head = vec![Cell::from(Line::from("#").right_aligned()), Cell::from(""), Cell::from("Title")];
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
            Cell::from(""),
            Cell::from(Span::styled(fit(&t.name, cols[2]), if is_playing { theme::accent() } else { theme::text() })),
        ];
        if show_artist {
            cells.push(Cell::from(Span::styled(fit(&t.artist_names(), cols[3]), theme::muted())));
        }
        if show_album {
            cells.push(Cell::from(Span::styled(fit(t.album_name(), cols[4]), theme::faint())));
        }
        cells.push(Cell::from(Line::from(Span::styled(format_ms(t.duration_ms), theme::faint())).right_aligned()));
        Row::new(cells)
    });
    let table = Table::new(rows, widths)
        .header(Row::new(head).style(theme::faint()).bottom_margin(1))
        .highlight_symbol(Text::from(Span::styled("▌", theme::accent())));
    let len = listing.items.len();
    draw_table(f, table, inner, area, &mut listing.state, len, 1, 2);
    let offset = listing.state.offset();
    let rows_area = Rect { y: inner.y + 2, height: inner.height.saturating_sub(2), ..inner };

    // Thumbnails over the empty second column of each visible row.
    let x = inner.x + 1 + cols[0] as u16 + 2;
    for (n, t) in listing.items.iter().skip(offset).take(rows_area.height as usize).enumerate() {
        let rect = Rect::new(x, rows_area.y + n as u16, thumb, 1);
        if rect.right() <= inner.right() {
            draw_cover(f, &mut app.images, t.images(), rect);
        }
    }
    app.hits.list = rows_area;
    app.hits.row_height = 1;
    app.hits.list_offset = offset;
}

/// One line of the Playlists tab: a folder (when the account has folders) or a playlist.
enum Entry<'a> {
    Folder { name: &'a str, count: usize },
    Playlist(&'a Playlist),
}

/// A grey square with a folder mark, the size of a cover.
fn draw_folder_icon(f: &mut Frame, area: Rect) {
    let mut lines = vec![Line::raw(""); (area.height.saturating_sub(1) / 2) as usize];
    lines.push(Line::from(Span::styled("▸▸", theme::accent())).centered());
    f.render_widget(Paragraph::new(lines).style(Style::new().bg(SELECTED_BG)), area);
}

fn draw_playlists(f: &mut Frame, app: &mut App, area: Rect) {
    let playing_ctx = app
        .playback
        .as_ref()
        .and_then(|pb| pb.context.as_ref())
        .map(|c| c.uri.clone());
    let user = app.user_id.clone();
    let thumb = app.images.square_cols(PLAYLIST_ROW);
    let path = app.folder_names();
    let foldered = app.foldered();

    // Title: "Playlists", or "‹ Playlists › Gym › Heavy" inside a folder.
    let mut title = vec![];
    if !path.is_empty() {
        title.push(Span::styled(" ‹", theme::accent()));
    }
    title.push(Span::styled(" Playlists ", if path.is_empty() { theme::bright() } else { theme::muted() }));
    for (i, name) in path.iter().enumerate() {
        let last = i + 1 == path.len();
        title.push(Span::styled("› ", theme::faint()));
        title.push(Span::styled(format!("{name} "), if last { theme::bright() } else { theme::muted() }));
    }
    let title = Line::from(title);
    if !path.is_empty() {
        app.hits.back = span_rect(area, area.x + 1, title.width() as u16);
    }

    let images = &mut app.images;
    let (entries, state, label, loading): (Vec<Entry>, &mut TableState, String, bool) = if foldered {
        let l = &mut app.rows;
        let label = if app.playlists.loading { " loading… ".into() } else { format!(" {} ", l.items.len()) };
        let entries = l
            .items
            .iter()
            .map(|r| match r {
                PlRow::Folder { name, count, .. } => Entry::Folder { name, count: *count },
                PlRow::Playlist(p) => Entry::Playlist(p),
            })
            .collect();
        (entries, &mut l.state, label, app.playlists.loading)
    } else {
        let l = &mut app.playlists;
        let label = count_label(l);
        (l.items.iter().map(Entry::Playlist).collect(), &mut l.state, label, l.loading)
    };
    let block = panel()
        .title(title)
        .title(Line::from(Span::styled(label, theme::faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if entries.is_empty() {
        draw_centered(f, inner, if loading { "Loading…" } else { "No playlists." });
        return;
    }

    let widths = [Constraint::Length(thumb), Constraint::Fill(1), Constraint::Length(12)];
    let cols = column_widths(inner.width, &widths);
    let rows = entries.iter().map(|e| {
        let (name, sub, count, is_playing) = match e {
            Entry::Folder { name, count } => (*name, "folder".to_owned(), format!("{count} playlists"), false),
            Entry::Playlist(p) => {
                let mine = p.collaborative || user.as_deref() == Some(p.owner.id.as_str());
                let owner = if mine { "by you".to_owned() } else { p.owner_name().to_owned() };
                let count = p.track_count().map(|n| format!("{n} tracks")).unwrap_or_default();
                (p.name.as_str(), owner, count, playing_ctx.as_deref() == Some(p.uri.as_str()))
            }
        };
        let name_style = match (e, is_playing) {
            (_, true) => theme::accent(),
            (Entry::Folder { .. }, _) => theme::bright(),
            _ => theme::text(),
        };
        let mut first = vec![Span::styled(fit(name, cols[1].saturating_sub(if is_playing { 2 } else { 0 })), name_style)];
        if is_playing {
            first.insert(0, Span::styled("♪ ", theme::accent()));
        }
        Row::new(vec![
            Cell::from(""),
            Cell::from(Text::from(vec![Line::from(first), Line::from(Span::styled(fit(&sub, cols[1]), theme::faint()))])),
            Cell::from(Line::from(Span::styled(count, theme::faint())).right_aligned()),
        ])
        .height(PLAYLIST_ROW)
    });
    let marker = Span::styled("▌", theme::accent());
    let table = Table::new(rows, widths).highlight_symbol(Text::from(vec![Line::from(marker.clone()), Line::from(marker)]));
    let len = entries.len();
    draw_table(f, table, inner, area, state, len, PLAYLIST_ROW, 0);
    let offset = state.offset();

    // Covers (or folder marks) go over the empty first column of each visible row.
    let visible = (inner.height / PLAYLIST_ROW) as usize;
    for (n, e) in entries.iter().skip(offset).take(visible).enumerate() {
        let rect = Rect::new(inner.x + 1, inner.y + n as u16 * PLAYLIST_ROW, thumb, PLAYLIST_ROW);
        if rect.right() <= inner.right() && rect.bottom() <= inner.bottom() {
            match e {
                Entry::Folder { .. } => draw_folder_icon(f, rect),
                Entry::Playlist(p) => draw_cover(f, images, &p.images, rect),
            }
        }
    }
    app.hits.list = inner;
    app.hits.row_height = PLAYLIST_ROW;
    app.hits.list_offset = offset;
}

/// Renders a list table with the shared row style, and a scrollbar on the panel's right border.
#[allow(clippy::too_many_arguments)]
fn draw_table(
    f: &mut Frame,
    table: Table,
    inner: Rect,
    outer: Rect,
    state: &mut TableState,
    len: usize,
    row_height: u16,
    header_rows: u16,
) {
    let table = table
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, inner, state);

    let visible = (inner.height.saturating_sub(header_rows) / row_height.max(1)) as usize;
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

fn draw_now_playing(f: &mut Frame, app: &mut App, area: Rect) {
    let block = panel();
    let inner = block.inner(area);
    f.render_widget(block, area);

    let art_rows = inner.height;
    let art_cols = app.images.square_cols(art_rows);
    let [art, _, info] =
        Layout::horizontal([Constraint::Length(art_cols), Constraint::Length(2), Constraint::Min(10)]).areas(inner);
    let [l1, l2, l3, l4] = Layout::vertical([Constraint::Length(1); 4]).areas(info);

    let pb = app.playback.clone();
    let track = pb.as_ref().and_then(|p| p.item.as_ref());
    draw_cover(f, &mut app.images, track.map_or(&[][..], |t| t.images()), art);

    // Line 4: transport controls on the left, volume slider on the right.
    let playing = pb.as_ref().is_some_and(|p| p.is_playing);
    let controls = [("◀◀", false), (if playing { "‖" } else { "▶" }, true), ("▶▶", false)];
    let mut x = l4.x;
    let mut spans = Vec::new();
    for (i, (label, main)) in controls.iter().enumerate() {
        let w = label.width() as u16;
        let rect = span_rect(l4, x, w);
        match i {
            0 => app.hits.prev = rect,
            1 => app.hits.play = rect,
            _ => app.hits.next = rect,
        }
        spans.push(Span::styled(*label, if *main { theme::accent() } else { theme::muted() }));
        spans.push(Span::raw("    "));
        x += w + 4;
    }
    f.render_widget(Line::from(spans), l4);
    // The slider gets what the controls leave; it shrinks, then drops its label, then hides.
    let room = l4.right().saturating_sub(x);
    if let (Some(volume), Some((bar_w, label))) = (app.volume(), volume_fit(room)) {
        let (line, bar) = volume_line(volume, app.muted(), bar_w, label);
        let w = line.width() as u16;
        let rect = span_rect(l4, l4.right().saturating_sub(w), w);
        app.hits.volume = Rect { x: rect.x + bar.0, width: bar.1, ..rect };
        f.render_widget(line, rect);
    }

    let Some(track) = track else {
        f.render_widget(Line::from(Span::styled("Nothing playing", theme::muted())), l1);
        let hint = if app.device_checked && app.device.is_none() {
            Span::styled("spotifyd not found. Start it: systemctl --user start spotifyd", Style::new().fg(RED))
        } else {
            Span::styled("Pick a track and press Enter, or Space to resume.", theme::faint())
        };
        f.render_widget(Line::from(hint), l2);
        return;
    };
    let pb = pb.as_ref().expect("track implies playback");

    // Line 1: title and like; shuffle and repeat on the right.
    let mode_on = |on: bool| if on { theme::accent() } else { theme::faint() };
    let repeat = if pb.repeat_state == "track" { "↻1" } else { "↻ " };
    let modes = Line::from(vec![
        Span::styled("⇄", mode_on(pb.shuffle_state)),
        Span::raw("  "),
        Span::styled(repeat, mode_on(pb.repeat_state != "off")),
    ]);
    let [title_area, modes_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(modes.width() as u16)]).areas(l1);
    app.hits.shuffle = span_rect(modes_area, modes_area.x, 1);
    app.hits.repeat = span_rect(modes_area, modes_area.x + 3, 2);
    let liked = matches!(&app.saved, Some((uri, saved)) if *uri == track.uri && *saved);
    let known = matches!(&app.saved, Some((uri, _)) if *uri == track.uri);
    let room = (title_area.width as usize).saturating_sub(4);
    let name = fit(&track.name, room);
    let mut title = vec![Span::styled(name.clone(), theme::bright())];
    if known {
        title.push(Span::raw("  "));
        title.push(Span::styled(if liked { "♥" } else { "♡" }, if liked { theme::accent() } else { theme::faint() }));
        app.hits.like = span_rect(title_area, title_area.x + name.width() as u16 + 2, 1);
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
    f.render_widget(Line::from(Span::styled(fit(&who, who_area.width as usize), theme::muted())), who_area);
    f.render_widget(
        Line::from(Span::styled(fit(&from, from_width as usize), theme::faint())).right_aligned(),
        from_area,
    );

    // Line 3: progress bar.
    let (line, bar) = progress_line(app.progress_ms(), track.duration_ms, pb.is_playing, l3.width);
    app.hits.progress = Rect { x: l3.x + bar.0, width: bar.1, ..l3 };
    f.render_widget(line, l3);
}

/// `1:05 ━━━━━━━━──────────── 5:30`, filling the given width.
/// Also returns where the bar is: (offset from the left, width).
pub fn progress_line(progress: u64, duration: u64, playing: bool, width: u16) -> (Line<'static>, (u16, u16)) {
    let left = format!("{} ", format_ms(progress));
    let right = format!(" {}", format_ms(duration));
    let bar = (width as usize).saturating_sub(left.width() + right.width());
    let ratio = if duration > 0 { (progress as f64 / duration as f64).clamp(0.0, 1.0) } else { 0.0 };
    let filled = (ratio * bar as f64).round() as usize;
    let fill_style = if playing { theme::accent() } else { theme::muted() };
    let line = Line::from(vec![
        Span::styled(left.clone(), theme::muted()),
        Span::styled("━".repeat(filled), fill_style),
        Span::styled("─".repeat(bar - filled), Style::new().fg(LINE)),
        Span::styled(right, theme::muted()),
    ]);
    (line, (left.width() as u16, bar as u16))
}

/// Bar width and whether the "vol " label fits in `room` cells (None: no room for a slider).
pub fn volume_fit(room: u16) -> Option<(u16, bool)> {
    const VALUE: u16 = 5; // " 100%"
    const MIN_BAR: u16 = 4;
    if room >= 4 + VALUE + MIN_BAR {
        Some(((room - 4 - VALUE).min(VOLUME_BAR), true))
    } else if room >= VALUE + MIN_BAR {
        Some(((room - VALUE).min(VOLUME_BAR), false))
    } else {
        None
    }
}

/// `vol ━━━━━━━───── 60%`; returns the line and where its bar is: (offset, width).
pub fn volume_line(volume: u8, muted: bool, bar: u16, with_label: bool) -> (Line<'static>, (u16, u16)) {
    let label = if with_label { "vol " } else { "" };
    let filled = ((volume.min(100) as f64 / 100.0) * bar as f64).round() as usize;
    let value = if muted { " mute".to_owned() } else { format!(" {volume:>3}%") };
    let line = Line::from(vec![
        Span::styled(label, theme::faint()),
        Span::styled("━".repeat(filled), if muted { theme::faint() } else { theme::text() }),
        Span::styled("─".repeat(bar as usize - filled), Style::new().fg(LINE)),
        Span::styled(value, if muted { theme::accent() } else { theme::muted() }),
    ]);
    (line, (label.width() as u16, bar))
}

fn hints(app: &App) -> &'static [(&'static str, &'static str)] {
    if app.typing {
        return &[("⏎", "search"), ("esc", "done"), ("^u", "clear")];
    }
    match app.view() {
        View::Search => &[("/", "search"), ("⏎", "play"), ("a", "queue"), ("␣", "pause"), ("+/-", "volume"), ("?", "help")],
        View::Playlists => &[("⏎", "open"), ("P", "play all"), ("esc", "up"), ("␣", "pause"), ("?", "help")],
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
    ("esc", "back (playlist, folder)"),
    ("a", "add to queue"),
    ("space", "play / pause"),
    ("n p", "next / previous"),
    ("← →", "seek 10 s"),
    ("+ -, ] [", "volume up / down"),
    ("shift ↑ ↓", "volume up / down"),
    ("m", "mute / unmute"),
    ("s", "shuffle"),
    ("r", "repeat: off, all, one"),
    ("f", "like / unlike the song"),
    ("R", "reload the list"),
    ("q", "quit (music keeps playing)"),
    ("Q", "quit and stop spotifyd"),
    ("v", "sound visualizer on / off"),
    ("", ""),
    ("mouse", "click: select, tabs, buttons"),
    ("", "double-click: play / open"),
    ("", "wheel: scroll; on a bar: adjust"),
    ("", "drag the progress or volume bar"),
];

fn draw_help(f: &mut Frame) {
    let height = (HELP.len() as u16 + 4).min(f.area().height);
    let [area] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(f.area());
    let [area] = Layout::horizontal([Constraint::Length(52)]).flex(Flex::Center).areas(area);
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
    fn progress_and_volume_lines() {
        let (line, (x, w)) = progress_line(165_000, 330_000, true, 60);
        assert_eq!(line.width(), 60);
        assert_eq!((x, w), (5, 50));
        let s: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.starts_with("2:45 ") && s.ends_with(" 5:30"));
        assert_eq!(s.matches('━').count(), 25);

        let (line, (x, w)) = volume_line(50, false, VOLUME_BAR, true);
        let s: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!((x, w), (4, VOLUME_BAR));
        assert!(s.ends_with("  50%"), "{s}");
        assert_eq!(s.matches('━').count(), 6);
        let (line, _) = volume_line(0, true, VOLUME_BAR, true);
        assert_eq!(volume_fit(40), Some((VOLUME_BAR, true)));
        assert_eq!(volume_fit(16), Some((7, true)));
        assert_eq!(volume_fit(11), Some((6, false)));
        assert_eq!(volume_fit(6), None);
        assert!(line.spans.iter().any(|s| s.content.contains("mute")));
    }

    fn playing(t: &Track) -> Msg {
        Msg::Playback(Some(Box::new(Playback {
            device: Some(Device { id: Some("d".into()), name: "spotlite".into(), is_active: true, volume_percent: Some(40) }),
            is_playing: true,
            progress_ms: Some(65_000),
            shuffle_state: true,
            repeat_state: "context".into(),
            context: None,
            item: Some(t.clone()),
        })))
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
        app.on_msg(playing(&t));
        app.on_msg(Msg::Saved { uri: t.uri.clone(), saved: true });
        let out = screen(&mut app, 100, 26);
        assert!(out.contains("♪ spotlite"), "{out}");
        assert!(out.contains("● spotlite"), "{out}");
        assert!(out.contains("Results for “teardrop”"), "{out}");
        assert!(out.contains("Title") && out.contains("Album"), "{out}");
        assert!(
            out.lines().any(|l| l.contains("♪      Teardrop")),
            "playing row is marked, with a 2-cell thumbnail before the title: {out}"
        );
        assert!(out.contains("Teardrop  ♥"), "{out}");
        assert!(out.contains("1:05 ━"), "{out}");
        assert!(out.contains("◀◀    ‖    ▶▶"), "{out}");
        assert!(out.contains("vol ━━━━━───────  40%"), "{out}");
        assert!(out.contains("⇄"), "{out}");

        // Hit areas line up with what was drawn.
        let rows: Vec<&str> = out.lines().collect();
        let h = &app.hits;
        let at = |r: Rect| -> String {
            rows[r.y as usize].chars().skip(r.x as usize).take(r.width as usize).collect()
        };
        assert_eq!(at(h.play), "‖");
        assert_eq!(at(h.prev), "◀◀");
        assert_eq!(at(h.like), "♥");
        assert_eq!(at(h.shuffle), "⇄");
        assert_eq!(at(h.volume).chars().count(), VOLUME_BAR as usize);
        assert!(at(h.progress).starts_with('━'));
        assert_eq!(at(h.tabs[1].0), "Playlists");
        assert!(at(Rect { height: 1, ..h.list }).contains("Teardrop"), "first list row is the first track");

        // Long cells end with "…" instead of being cut mid-word.
        let long = "A Very Long Song Title That Cannot Possibly Fit In The Column";
        app.search.items[1].name = long.into();
        let out = screen(&mut app, 100, 26);
        assert!(out.contains("A Very Long Song") && out.contains('…') && !out.contains("In The Column"), "{out}");

        // A narrow window keeps the controls clear of the slider and the tabs clear of the device.
        let narrow = screen(&mut app, 52, 24);
        assert!(narrow.contains("▶▶"), "{narrow}");
        assert!(narrow.contains("Liked"), "{narrow}");
        let r = app.hits.next;
        assert!(app.hits.volume.x >= r.x + r.width, "slider starts after ▶▶");

        // Narrow terminals drop the album column but still render.
        let narrow = screen(&mut app, 60, 22);
        assert!(!narrow.contains("Album") && narrow.contains("Teardrop"), "{narrow}");
    }

    #[test]
    fn visualizer_draws_smooth_bars_in_empty_cells_only() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        buf.set_string(0, 9, "TEXT", Style::new());
        buf[(5, 9)].set_bg(SELECTED_BG);
        buf[(6, 9)].skip = true;
        let frame = viz::Frame { levels: [0.5; viz::BANDS] };
        let area = buf.area;
        draw_visualizer(&mut buf, area, &frame);
        // 0.5 * VIZ_MAX * 10 rows = 4 full cells, then a partial top.
        let height = 0.5 * VIZ_MAX * 10.0;
        let full = height.floor() as u16;
        assert_eq!(buf[(10, 9)].symbol(), "█");
        assert_eq!(buf[(10, 10 - full)].symbol(), "█");
        let part = ((height - full as f32) * 8.0).round() as usize;
        assert_eq!(buf[(10, 9 - full)].symbol(), EIGHTHS[part]);
        assert_eq!(buf[(10, 8 - full)].symbol(), " ", "above the bar stays empty");
        assert_eq!(buf[(0, 9)].symbol(), "T", "text is kept");
        assert_eq!(buf[(0, 9)].bg, Color::Reset, "and not changed");
        assert_eq!(buf[(5, 9)].bg, SELECTED_BG, "selection untouched");
        assert_eq!(buf[(6, 9)].symbol(), " ", "skipped (picture) cell untouched");

        let mut quiet = Buffer::empty(Rect::new(0, 0, 20, 10));
        let area = quiet.area;
        draw_visualizer(&mut quiet, area, &viz::Frame::default());
        assert_eq!(quiet, Buffer::empty(Rect::new(0, 0, 20, 10)), "silence draws nothing");
    }

    #[test]
    fn renders_empty_states_and_help() {
        let (mut app, _rx) = test_app();
        let out = screen(&mut app, 90, 22);
        assert!(out.contains("Type a song, artist or album"), "{out}");
        assert!(out.contains("/ search songs, artists or albums"), "{out}");
        assert!(out.contains("Nothing playing"), "{out}");
        assert!(out.contains("connecting"), "{out}");

        app.on_msg(Msg::Device(None));
        let out = screen(&mut app, 90, 22);
        assert!(out.contains("no device") && out.contains("systemctl --user start spotifyd"), "{out}");

        app.help = true;
        let out = screen(&mut app, 90, 32);
        assert!(out.contains("like / unlike the song") && out.contains("mute / unmute"), "{out}");
    }

    #[test]
    fn renders_folders_and_breadcrumb() {
        let (mut app, _rx) = test_app();
        app.set_folders(crate::folders::parse_rootlist([
            "spotify:start-group:1:Gym+Stuff",
            "spotify:playlist:1",
            "spotify:end-group:1",
        ]));
        app.tab = Tab::Playlists;
        let pls: Vec<Playlist> = serde_json::from_value(serde_json::json!([
            {"id": "1", "uri": "spotify:playlist:1", "name": "Jammer", "owner": {"id": "me"}, "items": {"total": 174}}
        ]))
        .unwrap();
        app.on_msg(Msg::Playlists { offset: 0, page: Page { items: pls, raw: 1, total: 1 } });
        let out = screen(&mut app, 90, 22);
        assert!(out.contains("Gym Stuff") && out.contains("1 playlists") && out.contains("folder"), "{out}");
        assert!(out.contains("▸▸"), "{out}");
        assert!(!out.contains("Jammer"), "inside the folder only: {out}");
        app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Enter));
        let out = screen(&mut app, 90, 22);
        assert!(out.contains("‹ Playlists › Gym Stuff") && out.contains("Jammer"), "{out}");
        assert!(app.hits.back.width > 0);
    }

    #[test]
    fn renders_playlists_as_two_line_rows() {
        let (mut app, _rx) = test_app();
        app.on_msg(Msg::User(serde_json::from_value(serde_json::json!({"id": "me", "uri": "spotify:user:me"})).unwrap()));
        app.tab = Tab::Playlists;
        let pls: Vec<Playlist> = serde_json::from_value(serde_json::json!([
            {"id": "1", "uri": "spotify:playlist:1", "name": "Jammer", "owner": {"id": "me"}, "items": {"total": 174}},
            {"id": "2", "uri": "spotify:playlist:2", "name": "Night Runners", "owner": {"id": "x", "display_name": "Xander"}, "items": {"total": 28}}
        ]))
        .unwrap();
        app.on_msg(Msg::Playlists { offset: 0, page: Page { items: pls, raw: 2, total: 2 } });
        let out = screen(&mut app, 90, 22);
        let rows: Vec<&str> = out.lines().collect();
        let jammer = rows.iter().position(|r| r.contains("Jammer")).expect(&out);
        assert!(rows[jammer].contains("174 tracks"), "{out}");
        assert!(rows[jammer + 1].contains("by you"), "{out}");
        assert!(rows[jammer + 2].contains("Night Runners") && rows[jammer + 3].contains("Xander"), "{out}");
        assert!(out.contains("Playlists 2"), "{out}");
        assert_eq!(app.hits.row_height, PLAYLIST_ROW);
        assert_eq!(app.hits.list.y as usize, jammer);
    }
}
