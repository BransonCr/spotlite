//! The left sidebar. It shows one of two views:
//! - Playing: the song that plays now (cover, details) and its actions.
//! - Library: your pins, Liked Songs and your playlists.
//!
//! It slides open and shut with a rippled edge that settles like the sound visualizer, and the
//! two views slide sideways when you switch (Playing is on the left, Library on the right, as
//! in the switch at the top).

use crate::api::{pick_image, Album, ImageRef, Track};
use crate::app::App;
use crate::theme;
use crate::ui::{fit, format_ms};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget};
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

/// How long the player bar takes to shrink to one line, or to grow back.
const SLIM_TIME: Duration = Duration::from_millis(320);
/// How long the sidebar takes to open or shut, and to switch views.
const OPEN_TIME: Duration = Duration::from_millis(260);
const SWITCH_TIME: Duration = Duration::from_millis(280);
/// Below this screen width there is no room for the sidebar beside a list.
pub const MIN_SCREEN: u16 = 90;

/// The effect over the song in the sidebar (V).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideFx {
    #[default]
    Off,
    /// Small drops of liquid over the panel, one per band of the visualizer, each swelling and
    /// trembling with its band; each theme has its own.
    Liquid,
    /// The spectrum on its side: bars from the left edge.
    Spectrum,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideView {
    #[default]
    Playing,
    Library,
}

impl SideView {
    fn index(self) -> usize {
        match self {
            SideView::Playing => 0,
            SideView::Library => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    Open,
    Close,
    /// Switching views; the view it comes from.
    Switch(SideView),
}

/// A song, album or playlist you pinned to the top of the Library view.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Pin {
    Song { track: Track },
    Album { album: Album },
    Playlist { uri: String, name: String, #[serde(default)] images: Vec<ImageRef> },
}

impl Pin {
    pub fn uri(&self) -> String {
        match self {
            Pin::Song { track } => track.uri.clone(),
            Pin::Album { album } => album.uri.clone().unwrap_or_else(|| format!("spotify:album:{}", album.id.as_deref().unwrap_or(""))),
            Pin::Playlist { uri, .. } => uri.clone(),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Pin::Song { track } => &track.name,
            Pin::Album { album } => &album.name,
            Pin::Playlist { name, .. } => name,
        }
    }

    fn glyph(&self) -> &'static str {
        match self {
            Pin::Song { .. } => "♪",
            Pin::Album { .. } => "◎",
            Pin::Playlist { .. } => "≡",
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Pin::Song { .. } => "song",
            Pin::Album { .. } => "album",
            Pin::Playlist { .. } => "playlist",
        }
    }
}

/// What the song actions in the Playing view do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SongAct {
    Like,
    AddToPlaylist,
    Radio,
    Album,
    Artist,
    Pin,
    Card,
}

impl SongAct {
    pub fn key(self) -> &'static str {
        match self {
            SongAct::Like => "f",
            SongAct::AddToPlaylist => "e",
            SongAct::Radio => "o",
            SongAct::Album => "b",
            SongAct::Artist => "i",
            SongAct::Pin => "*",
            SongAct::Card => "C",
        }
    }
}

/// One row of a sidebar view.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A section title; the cursor skips it.
    Header(String),
    Act(SongAct),
    /// An index into `App::pins`.
    Pin(usize),
    Liked,
    /// An index into the loaded playlists.
    Playlist(usize),
    /// A note (loading, nothing pinned); the cursor skips it.
    Note(&'static str),
}

impl Item {
    pub fn selectable(&self) -> bool {
        !matches!(self, Item::Header(_) | Item::Note(_))
    }
}

#[derive(Default)]
pub struct Sidebar {
    /// Open, or opening (shutting counts as not open).
    pub open: bool,
    pub view: SideView,
    /// Keys go to the sidebar (j k ⏎ …) instead of the list.
    pub focus: bool,
    cursor: [usize; 2],
    scroll: [usize; 2],
    motion: Option<(Motion, Instant)>,
    /// The player bar is slim (one line) while the Playing view shows the song: since when it
    /// changes to slim (true) or back (false).
    slim: Option<(bool, Instant)>,
}

/// Slow at the end: the panel glides into place.
fn ease_out(p: f32) -> f32 {
    1.0 - (1.0 - p.clamp(0.0, 1.0)).powi(3)
}

impl Sidebar {
    fn start(&mut self, m: Motion) {
        self.motion = theme::motion().then(|| (m, Instant::now()));
    }

    /// Opens the sidebar on `view` (sliding open), or switches to it (sliding sideways).
    /// The sidebar takes the keys.
    pub fn show(&mut self, view: SideView) {
        if !self.open {
            self.view = view;
            self.open = true;
            self.start(Motion::Open);
        } else if self.view != view {
            let from = self.view;
            self.view = view;
            self.start(Motion::Switch(from));
        }
        self.focus = true;
    }

    /// `\`: open it (with the keys), give it the keys, or shut it.
    pub fn toggle(&mut self) {
        if !self.open {
            self.show(self.view);
        } else if !self.focus {
            self.focus = true;
        } else {
            self.close();
        }
    }

    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.focus = false;
            self.start(Motion::Close);
        }
    }

    /// The other view (h l, ← →, tab).
    pub fn switch(&mut self) {
        self.show(match self.view {
            SideView::Playing => SideView::Library,
            SideView::Library => SideView::Playing,
        });
    }

    /// The motion running now and how far it is (0 to 1, eased).
    pub fn motion(&self) -> Option<(Motion, f32)> {
        let (m, at) = self.motion?;
        let total = if matches!(m, Motion::Switch(_)) { SWITCH_TIME } else { OPEN_TIME };
        let p = at.elapsed().as_secs_f32() / total.as_secs_f32();
        (p < 1.0).then(|| (m, ease_out(p)))
    }

    pub fn animating(&self) -> bool {
        self.motion().is_some() || self.slim.is_some_and(|(_, at)| at.elapsed() < SLIM_TIME)
    }

    /// The Playing view shows the song, so the player bar can drop it.
    pub fn shows_song(&self) -> bool {
        self.open && self.view == SideView::Playing
    }

    /// How slim the player bar is: 0 full, 1 one line, between while it changes. `target`:
    /// should it be slim now. A change starts the animation.
    pub fn player_slim(&mut self, target: bool) -> f32 {
        let now = Instant::now();
        match self.slim {
            Some((t, _)) if t == target => {}
            None if !target => return 0.0,
            Some((_, at)) if at.elapsed() < SLIM_TIME => {
                // Turned around mid-way: start from where it is.
                let done = at.elapsed().as_secs_f32() / SLIM_TIME.as_secs_f32();
                let back = Duration::from_secs_f32((1.0 - done) * SLIM_TIME.as_secs_f32());
                self.slim = Some((target, now - back));
            }
            _ => self.slim = Some((target, if theme::motion() { now } else { now - SLIM_TIME })),
        }
        let Some((t, at)) = self.slim else { return 0.0 };
        let e = ease_out(at.elapsed().as_secs_f32() / SLIM_TIME.as_secs_f32());
        if t { e } else { 1.0 - e }
    }

    /// Drops a finished motion. True when one ended (the screen then changes).
    pub fn step(&mut self) -> bool {
        if self.motion.is_some() && self.motion().is_none() {
            self.motion = None;
            return true;
        }
        false
    }

    /// Drawn at all (open, or still shutting).
    pub fn visible(&self) -> bool {
        self.open || matches!(self.motion(), Some((Motion::Close, _)))
    }

    /// The sidebar's full width on a screen this wide, or None if it does not fit.
    pub fn full_width(screen: u16) -> Option<u16> {
        match screen {
            w if w < MIN_SCREEN => None,
            w if w >= 150 => Some(40),
            w if w >= 120 => Some(36),
            _ => Some(32),
        }
    }

    /// The columns it takes now: all of `full` when open, less while it slides.
    pub fn shown_width(&self, full: u16) -> u16 {
        let f = match self.motion() {
            Some((Motion::Open, e)) => e,
            Some((Motion::Close, e)) => 1.0 - e,
            _ if self.open => 1.0,
            _ => 0.0,
        };
        (full as f32 * f).round() as u16
    }

    pub fn cursor(&self, view: SideView) -> usize {
        self.cursor[view.index()]
    }

    pub fn set_cursor(&mut self, view: SideView, i: usize) {
        self.cursor[view.index()] = i;
    }

    /// Moves the cursor by `delta` rows over the selectable items.
    pub fn move_cursor(&mut self, items: &[Item], delta: isize) {
        let rows: Vec<usize> = items.iter().enumerate().filter(|(_, it)| it.selectable()).map(|(i, _)| i).collect();
        if rows.is_empty() {
            return;
        }
        let cur = self.cursor(self.view);
        let at = rows.iter().position(|&r| r >= cur).unwrap_or(rows.len() - 1) as isize;
        let next = (at + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.set_cursor(self.view, rows[next]);
    }

    /// Puts the cursor on a selectable row (after the list changed under it).
    pub fn fix_cursor(&mut self, items: &[Item]) {
        let cur = self.cursor(self.view);
        if items.get(cur).is_some_and(Item::selectable) {
            return;
        }
        let near = items.iter().enumerate().skip(cur).find(|(_, it)| it.selectable()).map(|(i, _)| i);
        let near = near.or_else(|| items.iter().enumerate().take(cur).rev().find(|(_, it)| it.selectable()).map(|(i, _)| i));
        self.set_cursor(self.view, near.unwrap_or(0));
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

/// Draws the sidebar in `area` (its full width), `shown` columns of it visible.
pub fn draw(f: &mut Frame, app: &mut App, area: Rect, shown: u16) {
    let view = app.sidebar.view;
    match app.sidebar.motion() {
        None => {
            let content = render(f.buffer_mut(), area, app, view, view.index() as f32, true);
            // The sidebar effect (V), over the song, in the cells nothing else uses.
            let (fx, reveal) = app.side_fx_shown();
            if view == SideView::Playing && reveal > 0.0 {
                let frame = app.viz.frame();
                match fx {
                    SideFx::Liquid => {
                        let kind = crate::theme::c().liquid;
                        // Frozen when the theme turns motion off.
                        if crate::theme::c().motion {
                            app.liquid.update(&frame, content.width, content.height, kind);
                        }
                        crate::ui::draw_liquid(f.buffer_mut(), content, &app.liquid, kind, reveal);
                    }
                    SideFx::Spectrum => {
                        app.side_peaks.update(&frame);
                        crate::ui::draw_side_visualizer(f.buffer_mut(), content, &frame, &app.side_peaks, reveal);
                    }
                    SideFx::Off => {}
                }
            }
            // The hide handle, on the right edge, at the height of the open handle.
            if area.height >= 7 {
                // In the gap between the sidebar and the list, so the border stays whole.
                let x = area.right() - 1;
                app.hits.side_hide = handle(f.buffer_mut(), x, area.y + area.height / 2, "‹", 1);
            }
        }
        Some((Motion::Open | Motion::Close, e)) => {
            let off_area = Rect::new(0, 0, area.width, area.height);
            let mut off = Buffer::empty(off_area);
            render(&mut off, off_area, app, view, view.index() as f32, false);
            let opening = matches!(app.sidebar.motion(), Some((Motion::Open, _)));
            let e = if opening { e } else { 1.0 - e };
            draw_slide(f.buffer_mut(), &off, area, shown, e);
        }
        Some((Motion::Switch(from), e)) => {
            let off_area = Rect::new(0, 0, area.width, area.height);
            let ind = from.index() as f32 + (view.index() as f32 - from.index() as f32) * e;
            let mut old = Buffer::empty(off_area);
            render(&mut old, off_area, app, from, ind, false);
            let mut new = Buffer::empty(off_area);
            let content = render(&mut new, off_area, app, view, ind, false);
            let dir: i32 = if view.index() > from.index() { 1 } else { -1 };
            draw_switch(&mut new, &old, content, dir, e);
            for y in 0..area.height {
                for x in 0..area.width {
                    if let Some(dst) = f.buffer_mut().cell_mut((area.x + x, area.y + y)) {
                        *dst = new[(x, y)].clone();
                    }
                }
            }
        }
    }
}

/// The tab at the left edge of the window while the sidebar is hidden: click it to open the
/// sidebar again.
pub fn draw_tab(f: &mut Frame, app: &mut App, body: Rect) {
    let x = f.area().x;
    if body.height < 7 || body.x <= x {
        return;
    }
    app.hits.side_open = handle(f.buffer_mut(), x, body.y + body.height / 2, "›", 0);
}

/// The handle that opens (›) or hides (‹) the sidebar: a thin line 5 rows tall with the chevron
/// in the middle, centred on row `mid`. Both handles sit at the same height, so the one hand
/// finds the other. The click area is 2 columns wide (`glyph_col` is the line's column in it).
fn handle(buf: &mut Buffer, x: u16, mid: u16, glyph: &str, glyph_col: u16) -> Rect {
    let t = theme::c();
    let top = mid.saturating_sub(2);
    let line = Style::new().fg(theme::blend(t.faint, t.accent, 0.35));
    let gx = x + glyph_col;
    for (dy, sym, style) in [
        (0, "╷", line),
        (1, "│", line),
        (2, glyph, theme::accent().add_modifier(Modifier::BOLD)),
        (3, "│", line),
        (4, "╵", line),
    ] {
        let pos = ratatui::layout::Position::new(gx, top + dy);
        if buf.area.contains(pos) {
            buf[(gx, top + dy)].set_symbol(sym).set_style(style);
        }
    }
    Rect::new(x, top, 2, 5).intersection(buf.area)
}

/// The ripple at the moving edge: rows stick out by a few columns, most in the middle of the
/// slide and none at its ends, like the visualizer's bars settling.
fn ripple(y: u16, e: f32) -> (u16, f32) {
    let secs = crate::ui::anim(1) as f32 / 1000.0;
    let amp = 6.0 * (std::f32::consts::PI * e).sin();
    let wave = 0.5 + 0.5 * (y as f32 * 0.85 + secs * 13.0).sin();
    let grain = 0.6 + 0.4 * (((y as u32).wrapping_mul(2_654_435_761) >> 24) as f32 / 255.0);
    let level = wave * grain;
    ((amp * level).round() as u16, level)
}

/// Open or shut: the right part of the panel shows, `shown` columns wide, with a rippled edge.
fn draw_slide(dst: &mut Buffer, off: &Buffer, area: Rect, shown: u16, e: f32) {
    let t = theme::c();
    let w = area.width;
    let right = dst.area.right();
    for y in 0..area.height {
        let (extra, level) = ripple(y, e);
        let rw = (shown + extra).min(w);
        for i in 0..rw {
            let x = area.x + i;
            if x >= right {
                break;
            }
            if let Some(c) = dst.cell_mut((x, area.y + y)) {
                *c = off[(w - rw + i, y)].clone();
            }
        }
        // The edge: a bar coloured like the visualizer, brighter where it sticks out more.
        let x = area.x + rw;
        if x < right && rw > 0 {
            if let Some(c) = dst.cell_mut((x, area.y + y)) {
                c.set_symbol("▌").set_style(Style::new().fg(theme::blend(t.viz_low, t.viz_high, level)));
            }
        }
    }
}

/// Switch: the old view slides out and the new one in (to the left when going to Library),
/// with a visualizer-coloured seam between them.
fn draw_switch(new: &mut Buffer, old: &Buffer, content: Rect, dir: i32, e: f32) {
    let t = theme::c();
    let w = content.width as i32;
    let off = (w as f32 * e).round() as i32;
    let fresh = new.clone();
    for y in content.top()..content.bottom() {
        for x in 0..w {
            let src = if dir > 0 { x + off } else { x - off };
            let cell = if (0..w).contains(&src) {
                old[((content.x as i32 + src) as u16, y)].clone()
            } else {
                fresh[((content.x as i32 + src.rem_euclid(w)) as u16, y)].clone()
            };
            new[((content.x as i32 + x) as u16, y)] = cell;
        }
        // The seam, where the two views meet.
        let seam = if dir > 0 { w - off } else { off - 1 };
        if (0..w).contains(&seam) {
            let (_, level) = ripple(y, 0.5);
            new[((content.x as i32 + seam) as u16, y)]
                .set_symbol(if dir > 0 { "▐" } else { "▌" })
                .set_style(Style::new().fg(theme::blend(t.viz_low, t.viz_high, level)));
        }
    }
}

/// Draws the whole panel for `view` into `buf` at `area`. `ind` places the switch's underline
/// (0 under Playing, 1 under Library, between while sliding). `live` draws the cover picture
/// and records where to click; a sliding copy does neither. Returns the content area.
fn render(buf: &mut Buffer, area: Rect, app: &mut App, view: SideView, ind: f32, live: bool) -> Rect {
    let t = theme::c();
    // The border keeps its focus colour while the panel slides.
    let focused = app.sidebar.focus;
    let block = match t.frame {
        theme::Frame::Full => Block::bordered(),
        theme::Frame::Rule => Block::new().borders(Borders::RIGHT),
        theme::Frame::None => Block::new(),
    }
    .border_type(t.corners)
    .border_style(theme::border(focused))
    .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    // A plain background, so the slide never shows the list through it.
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].reset();
        }
    }
    block.render(area, buf);
    if inner.height < 4 || inner.width < 12 {
        return inner;
    }
    if live {
        app.hits.side = area;
    }
    // The switch: two halves, the active one bright, an underline that slides between them.
    let half = inner.width / 2;
    let segs = [("♪", "Playing"), ("≡", "Library")];
    for (i, (glyph, label)) in segs.iter().enumerate() {
        let r = Rect::new(inner.x + half * i as u16, inner.y, if i == 0 { half } else { inner.width - half }, 1);
        let active = (ind - i as f32).abs() < 0.5;
        let style = if active { theme::accent().add_modifier(Modifier::BOLD) } else { theme::muted() };
        Paragraph::new(Line::from(vec![Span::styled(format!("{glyph} "), style), Span::styled(*label, style)]))
            .alignment(Alignment::Center)
            .render(r, buf);
        if live {
            app.hits.side_tabs[i] = r;
        }
    }
    let line_y = inner.y + 1;
    for x in inner.left()..inner.right() {
        buf[(x, line_y)].set_symbol("─").set_style(Style::new().fg(t.border));
    }
    let start = inner.x as f32 + ind * half as f32;
    let width = half as f32;
    for x in inner.left()..inner.right() {
        let cx = x as f32 + 0.5;
        if cx >= start && cx < start + width {
            buf[(x, line_y)].set_symbol("━").set_style(Style::new().fg(t.accent));
        }
    }
    let content = Rect::new(inner.x, inner.y + 3, inner.width, inner.height.saturating_sub(3));
    match view {
        SideView::Playing => render_playing(buf, content, app, live),
        SideView::Library => render_library(buf, content, app, live),
    }
    content
}

fn centered(buf: &mut Buffer, area: Rect, lines: Vec<Line<'static>>) {
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Paragraph::new(lines).alignment(Alignment::Center).render(Rect { y, height: h.min(area.height), ..area }, buf);
}

/// One action or library row: glyph, label, and a key hint or count on the right. The cursor
/// row is filled while the sidebar has the keys, and only marked while it does not.
fn row(buf: &mut Buffer, r: Rect, glyph: Span<'static>, label: &str, right: &str, cursor: bool, focused: bool) {
    let t = theme::c();
    let w = r.width as usize;
    let right_w = right.width();
    let label = fit(label, w.saturating_sub(right_w + 5));
    let pad = w.saturating_sub(2 + 2 + label.width() + right_w + 1);
    let base = if cursor && focused { Style::new().bg(t.selected) } else { Style::new() };
    let text = if cursor { theme::bright().patch(base) } else { theme::text().patch(base) };
    let mark = if cursor { theme::marker() } else { " ".into() };
    let line = Line::from(vec![
        Span::styled(format!("{mark} "), theme::accent().patch(base)),
        glyph.patch_style(base),
        Span::styled(" ", base),
        Span::styled(label, text),
        Span::styled(" ".repeat(pad), base),
        Span::styled(format!("{right} "), theme::faint().patch(base)),
    ]);
    Paragraph::new(line).render(r, buf);
}

fn render_playing(buf: &mut Buffer, area: Rect, app: &mut App, live: bool) {
    let t = theme::c();
    let Some((track, playing)) = app.playback.as_ref().and_then(|pb| pb.item.clone().map(|t| (t, pb.is_playing))) else {
        centered(
            buf,
            area,
            vec![
                Line::styled("♪", theme::faint()),
                Line::raw(""),
                Line::styled("Nothing playing", theme::muted()),
                Line::styled("space resumes · ⏎ plays a song", theme::faint()),
            ],
        );
        return;
    };
    let items = app.side_items(SideView::Playing);
    let w = area.width as usize;
    // Text under the cover: title (up to 2 lines), artist, album, details, up next.
    let mut text: Vec<Line<'static>> = Vec::new();
    let mark = theme::playing_mark(playing, crate::ui::anim(250));
    let title = format!("{mark} {}", track.name);
    if title.width() > w {
        let first = fit(&title, w);
        let cut = first.trim_end_matches('…').chars().count();
        let rest: String = title.chars().skip(cut).collect();
        text.push(Line::styled(first.trim_end_matches('…').to_owned(), theme::bright().add_modifier(Modifier::BOLD)));
        text.push(Line::styled(fit(rest.trim_start(), w), theme::bright().add_modifier(Modifier::BOLD)));
    } else {
        text.push(Line::styled(title, theme::bright().add_modifier(Modifier::BOLD)));
    }
    text.push(Line::styled(fit(&track.artist_names(), w), Style::new().fg(t.accent)));
    if let Some(album) = &track.album {
        text.push(Line::styled(fit(&album.name, w), theme::muted()));
    }
    let mut details = vec![format_ms(track.duration_ms)];
    if let Some(a) = &track.album {
        if let Some(kind) = a.album_type.as_deref().filter(|k| *k != "album") {
            details.push(kind.to_owned());
        }
        if let Some(y) = a.year() {
            details.push(y.to_string());
        }
    }
    text.push(Line::styled(fit(&details.join(&format!(" {} ", t.bullet)), w), theme::faint()));
    // Rows: cover, a gap, text, a gap, actions. The cover takes what is left, up to square.
    let fixed = text.len() as u16 + 2 + items.len() as u16;
    let mut rows = area.height.saturating_sub(fixed).min(area.width / 2);
    while rows > 2 && app.images.square_cols(rows) > area.width {
        rows -= 1;
    }
    let mut y = area.y;
    if rows >= 3 {
        let cols = app.images.square_cols(rows).min(area.width);
        let cover = Rect::new(area.x + (area.width - cols) / 2, y, cols, rows);
        draw_cover(buf, app, track.images(), cover, live);
        if live {
            app.hits.np_album = cover;
        }
        y += rows + 1;
    }
    let th = (text.len() as u16).min(area.bottom().saturating_sub(y));
    Paragraph::new(text).alignment(Alignment::Center).render(Rect::new(area.x, y, area.width, th), buf);
    y += th + 1;
    let liked = matches!(&app.saved, Some((uri, true)) if *uri == track.uri);
    let pinned = app.pins.iter().any(|p| p.uri() == track.uri);
    let cursor = app.sidebar.cursor(SideView::Playing);
    let focused = app.sidebar.focus && live;
    for (i, item) in items.iter().enumerate() {
        if y >= area.bottom() {
            break;
        }
        let Item::Act(act) = item else { continue };
        let (glyph, label) = match act {
            SongAct::Like if liked => (Span::styled("♥", Style::new().fg(t.love)), "Liked"),
            SongAct::Like => (Span::styled("♡", theme::muted()), "Like"),
            SongAct::AddToPlaylist => (Span::styled("+", theme::accent()), "Add to playlist…"),
            SongAct::Radio => (Span::styled("≋", theme::accent()), "Song radio"),
            SongAct::Album => (Span::styled("◎", theme::accent()), "Go to album"),
            SongAct::Artist => (Span::styled("◉", theme::accent()), "Go to artist"),
            SongAct::Pin if pinned => (Span::styled("✦", Style::new().fg(t.accent2)), "Unpin"),
            SongAct::Pin => (Span::styled("✧", theme::accent()), "Pin to library"),
            SongAct::Card => (Span::styled("▣", theme::accent()), "Share card"),
        };
        let r = Rect::new(area.x, y, area.width, 1);
        row(buf, r, glyph, label, act.key(), i == cursor, focused);
        if live {
            app.hits.side_rows.push((r, i));
        }
        y += 1;
    }
}

/// The cover, or a grey square with a note while it loads or while the panel slides (a picture
/// cannot move with the cells).
fn draw_cover(buf: &mut Buffer, app: &mut App, list: &[ImageRef], area: Rect, live: bool) {
    if live {
        let (fw, _) = app.images.font_size();
        let min_px = u32::from(area.width) * u32::from(fw);
        if let Some(protocol) = pick_image(list, min_px).and_then(|url| app.images.get(url, area.width, area.height)) {
            ratatui_image::Image::new(protocol).render(area, buf);
            return;
        }
    }
    let t = theme::c();
    let mut lines = vec![Line::raw(""); (area.height / 2) as usize];
    lines.push(Line::from("♪").centered());
    Paragraph::new(lines).style(Style::new().bg(t.selected).fg(t.faint)).render(area, buf);
}

fn render_library(buf: &mut Buffer, area: Rect, app: &mut App, live: bool) {
    let t = theme::c();
    let items = app.side_items(SideView::Library);
    let cursor = app.sidebar.cursor(SideView::Library);
    let focused = app.sidebar.focus && live;
    // Keep the cursor in sight.
    let h = area.height as usize;
    let mut top = app.sidebar.scroll[1].min(items.len().saturating_sub(1));
    if cursor < top {
        top = cursor.saturating_sub(1);
    } else if cursor >= top + h {
        top = cursor + 1 - h;
    }
    if live {
        app.sidebar.scroll[1] = top;
    }
    for (i, item) in items.iter().enumerate().skip(top).take(h) {
        let r = Rect::new(area.x, area.y + (i - top) as u16, area.width, 1);
        match item {
            Item::Header(title) => {
                let line = Line::from(vec![Span::styled(format!("  {}", theme::title(title)), theme::faint())]);
                Paragraph::new(line).render(r, buf);
            }
            Item::Note(note) => {
                Paragraph::new(Line::styled(format!("    {note}"), theme::faint())).render(r, buf);
            }
            Item::Pin(p) => {
                let Some(pin) = app.pins.get(*p) else { continue };
                let glyph = Span::styled(pin.glyph(), Style::new().fg(t.accent2));
                row(buf, r, glyph, pin.name(), pin.kind(), i == cursor, focused);
            }
            Item::Liked => {
                let count = app.liked.total.map(|n| n.to_string()).unwrap_or_default();
                row(buf, r, Span::styled("♥", Style::new().fg(t.love)), "Liked Songs", &count, i == cursor, focused);
            }
            Item::Playlist(p) => {
                let Some(pl) = app.playlists.items.get(*p) else { continue };
                let count = pl.items.as_ref().map(|n| n.total.to_string()).unwrap_or_default();
                row(buf, r, Span::styled("≡", theme::accent()), &pl.name, &count, i == cursor, focused);
            }
            Item::Act(_) => {}
        }
        if live && item.selectable() {
            app.hits.side_rows.push((r, i));
        }
    }
    if items.len() > h && h > 0 {
        // A scroll mark on the right edge, as long as the part of the list in sight.
        let len = (h * h / items.len()).max(1);
        let pos = top * (h - len) / items.len().saturating_sub(h).max(1);
        for y in pos..(pos + len).min(h) {
            buf[(area.right() - 1, area.y + y as u16)].set_symbol("│").set_style(Style::new().fg(t.faint));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_opens_focuses_then_shuts() {
        let mut s = Sidebar::default();
        s.toggle();
        assert!(s.open && s.focus);
        s.focus = false;
        s.toggle();
        assert!(s.open && s.focus, "the second press gives it the keys");
        s.toggle();
        assert!(!s.open && !s.focus);
    }

    #[test]
    fn show_switches_views() {
        let mut s = Sidebar::default();
        s.show(SideView::Library);
        assert_eq!((s.open, s.view), (true, SideView::Library));
        s.switch();
        assert_eq!(s.view, SideView::Playing);
    }

    #[test]
    fn width_fits_the_screen() {
        assert_eq!(Sidebar::full_width(80), None);
        assert_eq!(Sidebar::full_width(100), Some(32));
        assert_eq!(Sidebar::full_width(200), Some(40));
        let mut s = Sidebar::default();
        assert_eq!(s.shown_width(32), 0);
        s.open = true;
        assert_eq!(s.shown_width(32), 32);
    }

    #[test]
    fn cursor_skips_headers() {
        let items = vec![Item::Header("Pinned".into()), Item::Pin(0), Item::Header("x".into()), Item::Liked];
        let mut s = Sidebar { view: SideView::Library, ..Default::default() };
        s.fix_cursor(&items);
        assert_eq!(s.cursor(SideView::Library), 1);
        s.move_cursor(&items, 1);
        assert_eq!(s.cursor(SideView::Library), 3);
        s.move_cursor(&items, 5);
        assert_eq!(s.cursor(SideView::Library), 3);
        s.move_cursor(&items, -1);
        assert_eq!(s.cursor(SideView::Library), 1);
    }

    #[test]
    fn ripple_is_flat_at_the_ends() {
        for y in 0..20 {
            assert_eq!(ripple(y, 0.0).0, 0);
            assert_eq!(ripple(y, 1.0).0, 0);
        }
    }
}
