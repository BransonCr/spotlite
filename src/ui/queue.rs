//! The queue view (`A`): the selected song's cover and what plays after the queue on the
//! left, the queue on the right. Narrow screens show the list alone.

use super::*;

/// Below this width the cover panel is left out.
const SIDE_MIN_WIDTH: u16 = 90;

pub(super) fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    let list = if area.width >= SIDE_MIN_WIDTH {
        let side_w = (area.width * 2 / 5).clamp(30, 44);
        let [side, list] = Layout::horizontal([Constraint::Length(side_w), Constraint::Min(40)]).spacing(gap()).areas(area);
        draw_queue_side(f, app, side);
        list
    } else {
        area
    };
    let title = Line::from(Span::styled(format!(" {} ", theme::title("Queue")), theme::bright()));
    draw_tracks(f, app, list, title, "The queue is empty.\na adds songs here, or a on any song in a list");
}

fn minutes(ms: u64) -> String {
    let mins = ms.div_ceil(60_000);
    if mins >= 60 {
        format!("{} h {} min", mins / 60, mins % 60)
    } else {
        format!("{mins} min")
    }
}

/// The selected song large (else the playing one), the queue's length, and what Spotify plays
/// after the queue.
fn draw_queue_side(f: &mut Frame, app: &mut App, area: Rect) {
    let q = &app.user_queue;
    let selected = q.list.current().cloned();
    let (song, label) = match selected {
        Some(t) => {
            let i = q.list.index(q.list.selected()).unwrap_or(0);
            let label = if q.is_locked(i) { "Next · Spotify has it".to_owned() } else { format!("{} of {} in the queue", i + 1, q.len()) };
            (Some(t), label)
        }
        None => (app.playback.as_ref().and_then(|p| p.item.clone()), "Playing now".to_owned()),
    };
    let block = panel().title(Line::from(Span::styled(format!(" {} ", theme::title("Up next")), theme::bright())));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 4 || inner.width < 10 {
        return;
    }

    // The cover: as wide as the panel, at most half its height (a cell is about twice as
    // high as it is wide).
    let mut rows = (inner.height / 2).min(inner.width / 2).max(1);
    while rows > 1 && app.images.square_cols(rows) > inner.width {
        rows -= 1;
    }
    let cols = app.images.square_cols(rows);
    let mut y = inner.y + 1;
    if let Some(t) = &song {
        let [art] = Layout::horizontal([Constraint::Length(cols)]).flex(Flex::Center).areas(Rect { y, height: rows, ..inner });
        draw_cover(f, &mut app.images, t.images(), art);
        y += rows + 1;
    }

    let w = inner.width as usize;
    let q = &app.user_queue;
    let mut lines: Vec<Line> = Vec::new();
    if let Some(t) = &song {
        lines.push(Line::from(Span::styled(fit(&t.name, w), theme::bright().add_modifier(Modifier::BOLD))).centered());
        lines.push(Line::from(Span::styled(fit(&t.artist_names(), w), theme::muted())).centered());
        if !t.album_name().is_empty() {
            lines.push(Line::from(Span::styled(fit(t.album_name(), w), theme::faint())).centered());
        }
        lines.push(Line::from(Span::styled(fit(&label, w), theme::accent())).centered());
    }
    lines.push(Line::from(""));
    let summary = if q.is_empty() {
        "Nothing queued".to_owned()
    } else {
        format!("{} song{} · {}", q.len(), if q.len() == 1 { "" } else { "s" }, minutes(q.total_ms()))
    };
    lines.push(Line::from(Span::styled(summary, theme::muted())).centered());

    // What Spotify plays once the queue is done.
    let then = app.then_from_spotify();
    if !then.is_empty() {
        lines.push(Line::from(""));
        let from = app
            .playback
            .as_ref()
            .and_then(|p| p.context.as_ref())
            .map(|_| "Then from what plays")
            .unwrap_or("Then from Spotify");
        lines.push(Line::from(Span::styled(from, theme::faint().add_modifier(Modifier::BOLD))));
        for t in then {
            let text = format!("{} · {}", t.name, t.artist_names());
            lines.push(Line::from(vec![Span::styled("· ", theme::faint()), Span::styled(fit(&text, w.saturating_sub(2)), theme::faint())]));
        }
    }
    let text = Rect { y, height: inner.bottom().saturating_sub(y), ..inner };
    f.render_widget(Paragraph::new(lines), text);
}

/// The queue view's add picker: a search line, then the songs found.
pub(super) fn draw_queue_add(f: &mut Frame, app: &mut App) {
    let Some(p) = app.queue_add.as_ref() else { return };
    let screen = f.area();
    let area = popup_area(f, screen.width.saturating_sub(8).clamp(40, 72), 20);
    let block = popup_block("Add to the queue".into())
        .title_bottom(Line::from(Span::styled(" type · ⏎ search, then ⏎ add · ↑↓ choose · esc close ", theme::faint())).right_aligned());
    let inner = block.inner(area).inner(Margin::new(0, 1));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    app.hits.popup = area;
    let [query, _, list] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    f.render_widget(Line::from(vec![Span::styled("⌕ ", theme::accent()), Span::styled(p.query.to_string(), theme::text())]), query);
    f.set_cursor_position(Position::new(query.x + 2 + p.query.cursor_cols(), query.y));
    if p.found.is_empty() {
        let msg = if p.loading {
            "Searching…"
        } else if p.searched.is_empty() {
            "Type a song, an artist or both, then ⏎."
        } else {
            "No song found."
        };
        f.render_widget(Span::styled(msg, theme::faint()), list);
        return;
    }
    let cursor = p.cursor.min(p.found.len() - 1);
    let rows = list.height.max(1) as usize;
    let start = cursor.saturating_sub(rows - 1);
    let thumb = app.images.square_cols(1);
    let text_x = list.x + 2 + thumb + 1;
    let text_w = list.right().saturating_sub(text_x) as usize;
    let queued: std::collections::HashSet<&str> = app.user_queue.items().iter().map(|t| t.uri.as_str()).collect();
    let mut covers = Vec::new();
    for (n, (i, t)) in p.found.iter().enumerate().skip(start).take(rows).enumerate() {
        let row = Rect::new(list.x, list.y + n as u16, list.width, 1);
        let selected = i == cursor;
        let style = if selected { Style::new().bg(theme::c().selected).fg(theme::c().bright).add_modifier(Modifier::BOLD) } else { theme::text() };
        let marker = Span::styled(if selected { theme::c().marker.to_string() } else { " ".into() }, theme::accent());
        f.render_widget(Paragraph::new(Line::from(marker)).style(style), row);
        let time = format_ms(t.duration_ms);
        let mark = if queued.contains(t.uri.as_str()) { "✓ " } else { "" };
        let name_w = text_w.saturating_sub(time.len() + 1 + mark.len());
        let artist = format!(" {}", t.artist_names());
        let name = fit(&t.name, name_w);
        let rest = fit(&artist, name_w.saturating_sub(name.width()));
        let pad = text_w.saturating_sub(mark.width() + name.width() + rest.width() + time.len());
        f.render_widget(
            Line::from(vec![
                Span::styled(mark, theme::accent()),
                Span::styled(name, style),
                Span::styled(rest, theme::faint()),
                Span::raw(" ".repeat(pad)),
                Span::styled(time, theme::faint()),
            ]),
            Rect::new(text_x, row.y, text_w as u16, 1),
        );
        covers.push((t.images().to_vec(), Rect::new(list.x + 2, row.y, thumb, 1)));
        app.hits.popup_rows.push((row, i));
    }
    for (images, rect) in covers {
        draw_cover(f, &mut app.images, &images, rect);
    }
}
