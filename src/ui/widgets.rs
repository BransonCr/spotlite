//! Shared drawing helpers: panels, tables, cut text, covers, empty and loading states.

use super::*;

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

/// A panel in the theme's frame: four sides, a top rule only, or no lines at all.
pub(super) fn panel<'a>() -> Block<'a> {
    let t = theme::c();
    let block = match t.frame {
        theme::Frame::Full => Block::bordered(),
        theme::Frame::Rule => Block::new().borders(Borders::TOP),
        theme::Frame::None => Block::new(),
    };
    block.border_type(t.corners).border_style(theme::border(false)).padding(Padding::horizontal(1))
}

/// Box-drawing characters: the cells that `frame_fx` recolours.
pub(super) fn is_frame_char(sym: &str) -> bool {
    let mut it = sym.chars();
    matches!((it.next(), it.next()), (Some(c), None) if ('\u{2500}'..='\u{257F}').contains(&c))
}

/// Frame effects over the whole screen: panel lines run from accent to accent2 across the
/// screen (gradient), or pulse slowly between the line colour and the accent (glow).
pub(super) fn frame_fx(buf: &mut Buffer, area: Rect, playing: bool) {
    let t = theme::c();
    let glow = match t.frame_fx {
        theme::FrameFx::Plain => return,
        theme::FrameFx::Glow if !(playing && theme::motion()) => return,
        theme::FrameFx::Glow => {
            // One slow breath every 3.2 s.
            let phase = (anim(100) % 32) as f32 / 32.0 * std::f32::consts::TAU;
            Some(theme::blend(t.border, t.accent, 0.35 + 0.35 * phase.sin()))
        }
        theme::FrameFx::Gradient => None,
    };
    let (w, h) = (area.width.max(2) as f32 - 1.0, area.height.max(2) as f32 - 1.0);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if cell.fg != t.border || !is_frame_char(cell.symbol()) {
                continue;
            }
            let c = glow.unwrap_or_else(|| {
                let f = ((x - area.x) as f32 / w * 0.7 + (y - area.y) as f32 / h * 0.3).clamp(0.0, 1.0);
                theme::blend(theme::blend(t.accent, t.accent2, f), t.border, 0.35)
            });
            cell.set_fg(c);
        }
    }
}

/// A `width`-cell rect at `x` on the row of `area` (clipped to it).
pub(super) fn span_rect(area: Rect, x: u16, width: u16) -> Rect {
    let x = x.min(area.right());
    Rect::new(x, area.y, width.min(area.right() - x), 1)
}

/// Draws a cover picture, or a grey square with a note while it loads (or if there is none).
pub(super) fn draw_cover(f: &mut Frame, images: &mut Images, list: &[ImageRef], area: Rect) {
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
        Paragraph::new(lines).style(Style::new().bg(theme::c().selected).fg(theme::c().faint)),
        area,
    );
}

/// Cuts a styled line to `width` columns, ending with "…" when cut.
pub(super) fn fit_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let mut left = width;
    let mut spans = Vec::new();
    for span in line.spans {
        let w = span.content.width();
        if w < left {
            left -= w;
            spans.push(span);
        } else {
            spans.push(Span::styled(fit(&span.content, left), span.style));
            break;
        }
    }
    Line::from(spans)
}

/// Rows between parts of the screen, so boxes never touch.
pub(super) fn gap() -> u16 {
    1
}

/// Where a panel's title starts: after the left line, if the frame has one.
pub(super) fn title_x(area: Rect) -> u16 {
    area.x + u16::from(theme::c().frame == theme::Frame::Full)
}

/// " 12 / 266 " at the bottom right of a list panel.
pub(super) fn draw_position(f: &mut Frame, area: Rect, selected: usize, len: usize) {
    // Without a bottom line there is no place for it (the title shows the count).
    if len == 0 || area.height < 2 || theme::c().frame != theme::Frame::Full {
        return;
    }
    let text = format!(" {} / {len} ", selected + 1);
    let w = text.width() as u16;
    let x = area.right().saturating_sub(w + 2);
    f.render_widget(Span::styled(text, theme::faint()), Rect::new(x, area.bottom() - 1, w, 1));
}

/// A centred pop-up of `w` x `h` cells.
/// A `w` x `h` pop-up with its top-left corner at the mouse, moved left or up to stay on
/// screen (it opens above the mouse when there is no room below).
pub(super) fn popup_at(screen: Rect, at: Position, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(screen.width), h.min(screen.height));
    let x = at.x.min(screen.right().saturating_sub(w));
    let y = if at.y + h <= screen.bottom() { at.y } else { (at.y + 1).saturating_sub(h).max(screen.y) };
    Rect { x, y, width: w, height: h }
}

pub(super) fn popup_area(f: &Frame, w: u16, h: u16) -> Rect {
    let area = f.area();
    let [r] = Layout::vertical([Constraint::Length(h.min(area.height))]).flex(Flex::Center).areas(area);
    let [r] = Layout::horizontal([Constraint::Length(w.min(area.width))]).flex(Flex::Center).areas(r);
    r
}

pub(super) fn popup_block<'a>(title: String) -> Block<'a> {
    Block::bordered()
        .border_type(theme::c().corners)
        .border_style(Style::new().fg(theme::c().accent))
        .title(Span::styled(format!(" {title} "), theme::accent()))
        .padding(Padding::horizontal(1))
}

pub(super) fn count_label<T: crate::app::Viewable>(l: &Listing<T>, filtering: bool) -> String {
    let mut out = match (l.loading, l.total) {
        (true, _) => " loading… ".to_owned(),
        (false, Some(t)) if l.items.len() < t => format!(" {} of {t} ", l.items.len()),
        (false, Some(t)) => format!(" {t} "),
        (false, None) => String::new(),
    };
    if let Some((by, rev)) = l.sort {
        out = format!(" {} {}{out}", by.name(), if rev { "▼" } else { "▲" });
    }
    if let Some(f) = &l.filter {
        let caret = if filtering { "▏" } else { "" };
        out = format!(" / {f}{caret}  {} match{} ·{out}", l.len(), if l.len() == 1 { "" } else { "es" });
    }
    out
}

/// The count of a song list with its length: " 45 songs · 2 h 51 min ".
pub(super) fn songs_label(l: &Listing<Track>, filtering: bool) -> String {
    let base = count_label(l, filtering);
    if l.loading || l.items.is_empty() || l.filter.is_some() {
        return base;
    }
    let ms: u64 = l.items.iter().map(|t| t.duration_ms).sum();
    let mins = ms / 60_000;
    let time = if mins >= 60 { format!("{} h {} min", mins / 60, mins % 60) } else { format!("{mins} min") };
    let more = if l.total.is_some_and(|t| l.items.len() < t) { "+" } else { "" };
    format!("{} · {time}{more} ", base.trim_end())
}

/// A song or playlist name with the part the filter matched underlined.
pub(super) fn with_match(name: String, query: Option<&str>, style: Style) -> Vec<Span<'static>> {
    let word = query.and_then(|q| q.split_whitespace().next()).map(str::to_lowercase).filter(|w| !w.is_empty());
    let lower = name.to_lowercase();
    // Only where lower case keeps the byte places (most names): else no underline.
    let at = word.as_deref().and_then(|w| lower.find(w).map(|i| (i, i + w.len()))).filter(|_| lower.len() == name.len());
    match at {
        Some((i, j)) if name.is_char_boundary(i) && name.is_char_boundary(j) => vec![
            Span::styled(name[..i].to_owned(), style),
            Span::styled(name[i..j].to_owned(), style.add_modifier(Modifier::UNDERLINED | Modifier::BOLD)),
            Span::styled(name[j..].to_owned(), style),
        ],
        _ => vec![Span::styled(name, style)],
    }
}

/// Widths the table will give each column, so cell text can be cut with "…".
/// Mirrors Table's own layout: one cell for the "▌" marker, then columns 2 apart.
pub(super) fn column_widths(width: u16, widths: &[Constraint]) -> Vec<usize> {
    Layout::horizontal(widths.to_vec())
        .spacing(2)
        .split(Rect::new(0, 0, width.saturating_sub(1), 1))
        .iter()
        .map(|r| r.width as usize)
        .collect()
}

/// Hides the album, then the artist column on narrow terminals.
pub(super) fn track_columns(width: u16) -> (bool, bool) {
    (width >= 50, width >= 76)
}

/// A grey square with a folder mark, the size of a cover.
pub(super) fn draw_folder_icon(f: &mut Frame, area: Rect) {
    let mut lines = vec![Line::raw(""); (area.height.saturating_sub(1) / 2) as usize];
    lines.push(Line::from(Span::styled("▸▸", theme::accent())).centered());
    f.render_widget(Paragraph::new(lines).style(Style::new().bg(theme::c().selected)), area);
}

/// The rows of a list that fit in `height` (after `header` rows), from the list's scroll: it
/// moves, as the table's own does, only as far as needed to show the selection. A list then
/// builds only these rows, not every row on every frame.
pub(super) fn window(state: &mut TableState, len: usize, height: u16, row_height: u16, header: u16) -> std::ops::Range<usize> {
    let visible = (height.saturating_sub(header) / row_height.max(1)).max(1) as usize;
    let mut offset = state.offset().min(len.saturating_sub(1));
    if let Some(sel) = state.selected().map(|s| s.min(len.saturating_sub(1))) {
        if sel < offset {
            offset = sel;
        } else if sel >= offset + visible {
            offset = sel + 1 - visible;
        }
    }
    *state.offset_mut() = offset;
    offset..(offset + visible).min(len)
}

/// Renders a list table with the shared row style, and a scrollbar on the panel's right border.
/// The table holds the list's rows from `start` on (see `window`), or all of them (None).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table(
    f: &mut Frame,
    table: Table,
    inner: Rect,
    outer: Rect,
    state: &mut TableState,
    len: usize,
    row_height: u16,
    header_rows: u16,
    start: Option<usize>,
) -> Rect {
    let t = theme::c();
    let row_style = match t.select {
        theme::Select::Bar => Style::new().bg(t.selected).add_modifier(Modifier::BOLD),
        theme::Select::Block => Style::new().bg(t.accent).add_modifier(Modifier::BOLD),
        theme::Select::Underline | theme::Select::Arrow => Style::new().add_modifier(Modifier::BOLD),
    };
    let table = table.column_spacing(2).row_highlight_style(row_style).highlight_spacing(HighlightSpacing::Always);
    if let Some(start) = start {
        // The rows given start at the scroll: the table draws them from its top.
        let mut shown = TableState::default().with_selected(state.selected().and_then(|s| s.checked_sub(start)));
        f.render_stateful_widget(table, inner, &mut shown);
        *state.offset_mut() = start;
    } else {
        f.render_stateful_widget(table, inner, state);
    }
    // Cell text keeps its own colour; the block and underline styles restyle the row's text.
    if let Some(sel) = state.selected().filter(|&s| s >= state.offset()) {
        let y0 = inner.y + header_rows + ((sel - state.offset()) as u16) * row_height.max(1);
        let buf = f.buffer_mut();
        for y in y0..(y0 + row_height.max(1)).min(inner.bottom()) {
            for x in inner.left()..inner.right() {
                let cell = &mut buf[(x, y)];
                match t.select {
                    theme::Select::Block => {
                        cell.set_fg(t.viz_low);
                    }
                    // Only the name (text-coloured cells), not the number, artist or time.
                    theme::Select::Underline if x > inner.x && (cell.fg == t.text || cell.fg == t.accent) => {
                        cell.set_fg(t.bright);
                        cell.modifier.insert(Modifier::UNDERLINED);
                    }
                    theme::Select::Arrow if cell.symbol() != " " => {
                        cell.set_fg(t.bright);
                    }
                    _ => {}
                }
            }
        }
    }

    let visible = (inner.height.saturating_sub(header_rows) / row_height.max(1)) as usize;
    let track = outer.inner(Margin::new(0, 1));
    let track = Rect { x: track.right().saturating_sub(1), width: 1, ..track };
    if len <= visible {
        return Rect::default();
    }
    {
        let mut sb = ScrollbarState::new(len.saturating_sub(visible)).position(state.offset());
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::new().fg(theme::c().border))
                .thumb_symbol("┃")
                .thumb_style(Style::new().fg(theme::c().muted)),
            outer.inner(Margin::new(0, 1)),
            &mut sb,
        );
    }
    track
}

/// Animation frame: `period_ms` per step, from a clock that starts with the program.
pub fn anim(period_ms: u128) -> usize {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    (START.get_or_init(std::time::Instant::now).elapsed().as_millis() / period_ms.max(1)) as usize
}

/// Placeholder rows while a list loads: a dim bar where each column's text will be, of a
/// different length on each row. A soft light moves across them while the theme has motion.
pub(super) fn draw_skeleton(buf: &mut Buffer, area: Rect) {
    let t = theme::c();
    let base = theme::blend(t.selected, t.border, 0.25);
    let light = theme::blend(t.selected, t.muted, 0.35);
    let sweep = if theme::motion() { anim(30) as i32 } else { -1000 };
    let width = area.width as i32;
    for (n, y) in (area.top()..area.bottom()).enumerate().step_by(2) {
        // Number, title, artist, album, time: lengths from the row number (no flicker).
        let seed = (n as i32 * 7919) % 97;
        let title = width * (22 + seed % 14) / 100;
        let artist = width * (12 + (seed / 3) % 10) / 100;
        let album = width * (10 + (seed / 7) % 12) / 100;
        let bars = [(1, 2), (5, title), (5 + width * 42 / 100, artist), (5 + width * 66 / 100, album), (width - 6, 4)];
        for (x0, len) in bars {
            for x in x0..(x0 + len).min(width - 1) {
                let d = (x + n as i32 * 2 - sweep % (width + 40) + 20).abs();
                let c = if d < 10 { theme::blend(light, base, d as f32 / 10.0) } else { base };
                let cell = &mut buf[(area.x + x as u16, y)];
                cell.set_symbol(" ");
                cell.set_bg(c);
            }
        }
    }
}

pub(super) const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// An empty or loading list: "Loading…" gets a spinner; any other message gets the theme's
/// logo above it. A second line after "\n" is a hint (the key to press).
pub(super) fn draw_centered(f: &mut Frame, area: Rect, msg: &str) {
    // A list that loads shows the shape of its rows, so the rows take their place without a jump.
    if msg == "Loading…" && area.height >= 4 && area.width >= 30 {
        draw_skeleton(f.buffer_mut(), area);
        return;
    }
    let mut lines = Vec::new();
    if msg == "Loading…" {
        lines.push(Line::from(vec![
            Span::styled(SPINNER[anim(80) % SPINNER.len()].to_string(), theme::accent()),
            Span::styled("  Loading…", theme::muted()),
        ]));
    } else {
        let (text, hint) = msg.split_once('\n').unwrap_or((msg, ""));
        if area.height >= 5 {
            lines.push(Line::from(Span::styled(theme::c().logo.to_string(), theme::accent())));
            lines.push(Line::raw(""));
        }
        lines.push(Line::from(Span::styled(text.to_owned(), theme::muted())));
        if !hint.is_empty() && area.height >= 2 {
            lines.push(Line::from(Span::styled(hint.to_owned(), theme::faint())));
        }
    }
    let [mid] = Layout::vertical([Constraint::Length(lines.len() as u16)]).flex(Flex::Center).areas(area);
    let lines: Vec<Line> = lines.into_iter().map(|l| fit_line(l, area.width as usize)).collect();
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), mid);
}
