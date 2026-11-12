use crate::api::{pick_image, Found, ImageRef, Playlist, Track};
use crate::keys::Act;
use crate::app::{Action, App, Hits, Listing, Loc, Row as PlRow, SearchKind, Tab, View, SAMPLE_LEN};
use crate::pages::{ArtistRow, Page};
use crate::images::Images;
use crate::viz;
use crate::theme;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Margin, Position, Rect};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, HighlightSpacing, Padding, Paragraph, Row, Scrollbar,
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

/// A panel in the theme's frame: four sides, a top rule only, or no lines at all.
fn panel<'a>() -> Block<'a> {
    let t = theme::c();
    let block = match t.frame {
        theme::Frame::Full => Block::bordered(),
        theme::Frame::Rule => Block::new().borders(Borders::TOP),
        theme::Frame::None => Block::new(),
    };
    block.border_type(t.corners).border_style(theme::border(false)).padding(Padding::horizontal(1))
}

/// Box-drawing characters: the cells that `frame_fx` recolours.
fn is_frame_char(sym: &str) -> bool {
    let mut it = sym.chars();
    matches!((it.next(), it.next()), (Some(c), None) if ('\u{2500}'..='\u{257F}').contains(&c))
}

/// Frame effects over the whole screen: panel lines run from accent to accent2 across the
/// screen (gradient), or pulse slowly between the line colour and the accent (glow).
fn frame_fx(buf: &mut Buffer, area: Rect, playing: bool) {
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
        Paragraph::new(lines).style(Style::new().bg(theme::c().selected).fg(theme::c().faint)),
        area,
    );
}

/// The player shows the time left (set from `App::remaining` at each draw).
static REMAINING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits = Hits::default();
    REMAINING.store(app.remaining, std::sync::atomic::Ordering::Relaxed);
    app.images.begin_frame();
    let t = theme::c();
    let full = f.area().inner(Margin::new(1, 0));
    app.screen_w = f.area().width;
    // Stage: a big cover column on wide terminals; the player then needs only two lines.
    // The sidebar shows the song itself, so no stage column beside it.
    let stage = t.layout == theme::Layout::Stage && full.width >= 140 && full.height >= 24 && !app.sidebar.visible();
    // Short terminals get a 2-line player, so the list keeps its room. On stage the big
    // column shows the song, so the player is only the controls, progress and volume.
    let player = if stage { 3 } else if f.area().height < 24 { 4 } else { PLAYER_HEIGHT };
    // While the sidebar shows the song, the player drops it and keeps one line (controls,
    // progress, shuffle, repeat, volume); it shrinks row by row.
    let slim_target = app.sidebar.shows_song() && crate::sidebar::Sidebar::full_width(full.width).is_some();
    let slim = app.sidebar.player_slim(slim_target);
    let one_line = player - panel().inner(Rect::new(0, 0, 20, player)).height.saturating_sub(1);
    let player = (player as f32 - (player - one_line.min(player)) as f32 * slim).round() as u16;
    let (header, mut body, now, footer) = if t.layout == theme::Layout::Top {
        let [header, now, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(player),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .spacing(gap())
        .areas(full);
        (header, body, now, footer)
    } else {
        let [header, body, now, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(player),
            Constraint::Length(1),
        ])
        .spacing(gap())
        .areas(full);
        (header, body, now, footer)
    };
    // The sidebar, at the left of the list: its full width, and what shows while it slides.
    let side_fits = crate::sidebar::Sidebar::full_width(full.width).is_some();
    let side_full = crate::sidebar::Sidebar::full_width(full.width).filter(|_| app.sidebar.visible());
    let mut side = None;
    if let Some(w) = side_full {
        let shown = app.sidebar.shown_width(w);
        if shown > 0 {
            side = Some((Rect { width: w, ..body }, shown));
            let cut = (shown + 1).min(body.width.saturating_sub(20));
            body = Rect { x: body.x + cut, width: body.width - cut, ..body };
        }
    }
    if stage {
        let [list, side] = Layout::horizontal([Constraint::Min(60), Constraint::Length(48)]).spacing(1).areas(body);
        body = list;
        draw_stage(f, app, side);
    }
    // The visualizer is drawn under the list: in the empty cells of the main panel.
    let viz_area = body.inner(Margin::new(1, 1));

    draw_header(f, app, header);
    match app.view() {
        View::Search => draw_search(f, app, body),
        View::Playlists => draw_playlists(f, app, body),
        View::Liked => {
            let title = Line::from(Span::styled(format!(" {} ", theme::title("Liked songs")), theme::bright()));
            draw_tracks(f, app, body, title, "No liked songs yet.\nf on any song likes it");
        }
        View::Recent => {
            let title = Line::from(Span::styled(format!(" {} ", theme::title("Recently played")), theme::bright()));
            draw_tracks(
                f,
                app,
                body,
                title,
                "Nothing played yet.\nIf you logged in before this version, run `spotlite auth` again",
            );
        }
        View::Playlist => {
            let name = app.open.as_ref().map_or("", |o| o.playlist.name.as_str()).to_owned();
            let title = Line::from(vec![
                Span::styled(" ‹ ", theme::accent()),
                Span::styled(format!("{name} "), theme::bright()),
            ]);
            app.hits.back = span_rect(body, title_x(body), title.width() as u16);
            draw_tracks(f, app, body, title, "This playlist is empty.\ne on any song adds it to a playlist");
        }
        View::Artist => draw_artist(f, app, body),
        View::Album => draw_album(f, app, body),
        View::Radio => draw_radio(f, app, body),
        View::Random => draw_random(f, app, body),
    }
    draw_now_playing(f, app, now);
    draw_footer(f, app, footer);
    if app.tabs_list.is_some() {
        draw_tabs_list(f, app, header);
    }
    if let Some((area, shown)) = side {
        crate::sidebar::draw(f, app, area, shown);
    } else if side_fits && !app.sidebar.visible() {
        crate::sidebar::draw_tab(f, app, body);
    }
    let reveal = app.viz_reveal();
    if reveal > 0.0 {
        // Under the list, in the cells that nothing else uses.
        let frame = app.viz.frame();
        draw_visualizer_fx(f.buffer_mut(), viz_area, &frame, reveal);
    }
    if app.menu.is_some() {
        draw_menu(f, app);
    } else if app.genre_picker.is_some() {
        draw_genre_picker(f, app);
    } else if app.picker.is_some() {
        draw_picker(f, app);
    } else if app.confirm.is_some() {
        draw_confirm(f, app);
    } else if app.palette.is_some() {
        draw_palette(f, app);
    } else if app.undo.is_some() {
        draw_undo(f, app);
    }
    if app.help {
        draw_help(f, app);
    }
    let playing = app.playback.as_ref().is_some_and(|pb| pb.is_playing);
    let area = f.area();
    frame_fx(f.buffer_mut(), area, playing);
}

/// Cuts a styled line to `width` columns, ending with "…" when cut.
fn fit_line(line: Line<'static>, width: usize) -> Line<'static> {
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

/// " ‹ Name " for a page title; records the back button.
/// The title of a page's list: "‹ Back · Radio". The name is in the page header above, so it
/// is not repeated here. The whole title is the back button.
fn page_title(app: &mut App, area: Rect, kind: &str) -> Line<'static> {
    let t = theme::c();
    let line = Line::from(vec![
        Span::styled(" ‹ ", theme::accent()),
        Span::styled(theme::title("Back"), theme::muted()),
        Span::styled(format!(" {} ", t.bullet), theme::faint()),
        Span::styled(format!("{} ", theme::title(kind)), theme::bright()),
    ]);
    app.hits.back = span_rect(area, title_x(area), line.width() as u16);
    line
}

/// Rows between parts of the screen, so boxes never touch.
fn gap() -> u16 {
    1
}

/// Where a panel's title starts: after the left line, if the frame has one.
fn title_x(area: Rect) -> u16 {
    area.x + u16::from(theme::c().frame == theme::Frame::Full)
}

/// The top part of a page: a cover, then up to three lines of text.
fn draw_page_header(f: &mut Frame, images: &mut Images, area: Rect, cover: &[ImageRef], lines: Vec<Line<'static>>) {
    let rows = area.height;
    let cols = images.square_cols(rows);
    let [art, _, text] =
        Layout::horizontal([Constraint::Length(cols), Constraint::Length(2), Constraint::Min(10)]).areas(area);
    draw_cover(f, images, cover, art);
    let lines: Vec<Line> = lines.into_iter().map(|l| fit_line(l, text.width as usize)).collect();
    let [mid] = Layout::vertical([Constraint::Length(lines.len() as u16)]).flex(Flex::Center).areas(text);
    f.render_widget(Paragraph::new(lines), mid);
}

fn draw_artist(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(Page::Artist(page)) = app.page() else { return };
    let name = page.name.clone();
    let title = page_title(app, area, "Artist");
    let block = panel().title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    // No gap under the header: the first section title has a blank line of its own.
    // One empty row under the top line, so the picture does not touch it.
    let [_, head, list] = Layout::vertical([Constraint::Length(1), Constraint::Length(5), Constraint::Min(2)]).areas(inner);

    let Some(Page::Artist(page)) = app.active.and_then(|i| app.pages.get_mut(i)) else { return };
    let tracks = page.count(|r| matches!(r, ArtistRow::Track(_)));
    let singles = page.count(|r| matches!(r, ArtistRow::Album(a) if a.album_type.as_deref() == Some("single")));
    let albums = page.count(|r| matches!(r, ArtistRow::Album(_))) - singles;
    let cover = page.artist.as_ref().map(|a| a.images.clone()).unwrap_or_default();
    let summary = if let Some(e) = &page.error {
        format!("Could not load: {e}")
    } else if page.rows.loading {
        "Loading…".to_owned()
    } else {
        let mut parts = vec![format!("{tracks} popular songs")];
        if albums > 0 {
            parts.push(format!("{albums} {}", if albums == 1 { "album" } else { "albums" }));
        }
        if singles > 0 {
            parts.push(format!("{singles} singles and EPs"));
        }
        parts.join(" · ")
    };
    draw_page_header(
        f,
        &mut app.images,
        head,
        &cover,
        vec![
            Line::from(Span::styled(name.clone(), theme::bright())),
            Line::from(Span::styled(summary, theme::muted())),
            Line::from(Span::styled("⏎ play or open · tab next section · o radio · . more", theme::faint())),
        ],
    );

    if page.rows.items.is_empty() {
        draw_centered(f, list, if page.rows.loading { "Loading…" } else { "Nothing found for this artist." });
        return;
    }
    let playing = app.playback.as_ref().and_then(|p| p.item.as_ref()).map(|t| t.uri.clone());
    let thumb = app.images.square_cols(PLAYLIST_ROW);
    let widths = [Constraint::Length(thumb), Constraint::Fill(1), Constraint::Length(8)];
    let cols = column_widths(list.width, &widths);
    let section_w = list.width.saturating_sub(2) as usize;
    let mut rank = 0;
    let rows = page.rows.items.iter().map(|r| {
        let (name, sub, right, is_playing) = match r {
            ArtistRow::Header(title) => {
                // A blank line above, then the title and a faint rule to the edge.
                let title = theme::title(title);
                let rule = "─".repeat(section_w.saturating_sub(title.width() + 1));
                return Row::new(vec![
                    Cell::from(""),
                    Cell::from(Text::from(vec![
                        Line::from(""),
                        Line::from(vec![
                            Span::styled(title, theme::accent().add_modifier(Modifier::BOLD)),
                            Span::styled(format!(" {rule}"), theme::faint()),
                        ]),
                    ])),
                    Cell::from(""),
                ])
                .height(PLAYLIST_ROW);
            }
            ArtistRow::Track(t) => {
                rank += 1;
                (
                    t.name.clone(),
                    format!("{rank:>2} · {}", t.album_name()),
                    format_ms(t.duration_ms),
                    playing.as_deref() == Some(t.uri.as_str()),
                )
            }
            ArtistRow::Radio(t) if t.uri.starts_with("spotify:artist:") => {
                (format!("{} Radio", t.name), "Artist radio · songs like this artist's".to_owned(), "radio".to_owned(), false)
            }
            ArtistRow::Radio(t) => {
                (format!("{} Radio", t.name), format!("Song radio · from {}", t.album_name()), "radio".to_owned(), false)
            }
            ArtistRow::Album(a) => (a.name.clone(), a.summary(), String::new(), false),
        };
        let style = if is_playing { theme::accent() } else { theme::text() };
        Row::new(vec![
            Cell::from(""),
            Cell::from(Text::from(vec![
                Line::from(Span::styled(fit(&name, cols[1]), style)),
                Line::from(Span::styled(fit(&sub, cols[1]), theme::faint())),
            ])),
            Cell::from(Line::from(Span::styled(right, theme::faint())).right_aligned()),
        ])
        .height(PLAYLIST_ROW)
    });
    let marker = Span::styled(theme::marker(), theme::accent());
    let table = Table::new(rows, widths).highlight_symbol(Text::from(vec![Line::from(marker.clone()), Line::from(marker)]));
    let len = page.rows.items.len();
    draw_table(f, table, list, area, &mut page.rows.state, len, PLAYLIST_ROW, 0, None);
    let offset = page.rows.state.offset();
    let visible = (list.height / PLAYLIST_ROW) as usize;
    for (n, r) in page.rows.items.iter().skip(offset).take(visible).enumerate() {
        let rect = Rect::new(list.x + 1, list.y + n as u16 * PLAYLIST_ROW, thumb, PLAYLIST_ROW);
        let images = match r {
            ArtistRow::Header(_) => continue,
            ArtistRow::Track(t) | ArtistRow::Radio(t) => t.images(),
            ArtistRow::Album(a) => &a.images,
        };
        if rect.right() <= list.right() && rect.bottom() <= list.bottom() {
            draw_cover(f, &mut app.images, images, rect);
        }
    }
    draw_position(f, area, page.rows.selected(), len);
    app.hits.list = list;
    app.hits.row_height = PLAYLIST_ROW;
    app.hits.list_offset = offset;
}

fn draw_album(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(Page::Album(page)) = app.page() else { return };
    let album = page.album.clone();
    let [head, list] = Layout::vertical([Constraint::Length(6), Constraint::Min(4)]).spacing(gap()).areas(area);
    let block = panel();
    let inner = block.inner(head);
    f.render_widget(block, head);
    let artists = album_artists(app);
    draw_page_header(
        f,
        &mut app.images,
        inner,
        &album.images,
        vec![
            Line::from(Span::styled(album.name.clone(), theme::bright())),
            Line::from(Span::styled(format!("{artists}{}", album.summary()), theme::muted())),
            Line::from(Span::styled("⏎ play from a song · P play the album · . more", theme::faint())),
        ],
    );
    let title = page_title(app, list, "Album songs");
    draw_tracks(f, app, list, title, "This album has no songs.");
}

/// "Artist · " for the album page (from its first track), or nothing.
fn album_artists(app: &App) -> String {
    let Some(Page::Album(page)) = app.page() else { return String::new() };
    page.tracks
        .items
        .first()
        .map(|t| format!("{} · ", t.artist_names()))
        .unwrap_or_default()
}

fn draw_radio(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(Page::Radio(page)) = app.page() else { return };
    let seed = page.seed.clone();
    let artist_seed = seed.uri.starts_with("spotify:artist:");
    let filters = crate::pages::active_filters(&page.filters);
    let (kept, total, error) = (page.tracks.items.len(), page.total(), page.error.clone());
    let [head, list] = Layout::vertical([Constraint::Length(6), Constraint::Min(4)]).spacing(gap()).areas(area);
    let block = panel();
    let inner = block.inner(head);
    f.render_widget(block, head);
    let (discover, round, digging) = (page.filters.discover, page.round, page.digging);
    let filter_line = if filters.is_empty() {
        Line::from(vec![Span::styled("No filters", theme::faint()), Span::styled("  ·  F to filter", theme::faint())])
    } else {
        Line::from(vec![
            Span::styled("Filters: ", theme::faint()),
            Span::styled(filters.join(" · "), theme::accent()),
            Span::styled(format!("  ·  {kept} of {total} songs  ·  F to change"), theme::faint()),
        ])
    };
    draw_page_header(
        f,
        &mut app.images,
        inner,
        seed.images(),
        vec![
            Line::from(vec![
                Span::styled(if artist_seed { "Artist radio  " } else { "Song radio  " }, theme::muted()),
                Span::styled(seed.name.clone(), theme::bright()),
            ]),
            if discover {
                Line::from(vec![
                    Span::styled(format!("Discover · round {round}"), theme::accent().add_modifier(Modifier::BOLD)),
                    Span::styled(
                        if digging {
                            "  ·  finding new songs…".to_owned()
                        } else {
                            "  ·  nothing from your Liked Songs, shuffled  ·  d for new songs".to_owned()
                        },
                        theme::muted(),
                    ),
                ])
            } else {
                Line::from(Span::styled(
                    if artist_seed {
                        "Songs like this artist's · picked by Spotify for you  ·  d to discover".to_owned()
                    } else {
                        format!("Similar to {} · picked by Spotify for you  ·  d to discover", seed.artist_names())
                    },
                    theme::muted(),
                ))
            },
            filter_line,
        ],
    );
    let title = page_title(app, list, if artist_seed { "Artist radio" } else { "Song radio" });
    let empty = match &error {
        Some(e) => format!("Spotify gave no radio: {e}"),
        None if discover && digging => "Finding new songs…".into(),
        None if discover => "No new songs left. Press d for another round, or F to change the filters.".into(),
        None if total > 0 => "No songs pass the filters. Press F to change them.".into(),
        None => "Asking Spotify for the radio…".into(),
    };
    draw_tracks(f, app, list, title, &empty);
}

fn draw_random(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(Page::Random(page)) = app.page() else { return };
    let (genre, rolling, error, count) = (page.genre.clone(), page.rolling, page.error.clone(), page.tracks.items.len());
    let cover = page.tracks.items.first().map(|t| t.images().to_vec()).unwrap_or_default();
    let [head, list] = Layout::vertical([Constraint::Length(6), Constraint::Min(4)]).spacing(gap()).areas(area);
    let block = panel();
    let inner = block.inner(head);
    f.render_widget(block, head);
    let state = if rolling {
        format!("{} Picking random songs…", SPINNER[anim(80) % SPINNER.len()])
    } else {
        format!("{count} songs  ·  d to roll again  ·  S to save as a playlist")
    };
    draw_page_header(
        f,
        &mut app.images,
        inner,
        &cover,
        vec![
            Line::from(vec![Span::styled("Randomize  ", theme::muted()), Span::styled(genre.clone(), theme::bright())]),
            Line::from(Span::styled(
                "Picked at random from all through the genre on Spotify, not by your taste",
                theme::muted(),
            )),
            Line::from(Span::styled(state, theme::accent())),
        ],
    );
    let title = page_title(app, list, "Randomize");
    let empty = match &error {
        Some(e) => e.clone(),
        None => format!("Picking random {genre} songs…"),
    };
    draw_tracks(f, app, list, title, &empty);
}

/// " 12 / 266 " at the bottom right of a list panel.
fn draw_position(f: &mut Frame, area: Rect, selected: usize, len: usize) {
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
fn popup_at(screen: Rect, at: Position, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(screen.width), h.min(screen.height));
    let x = at.x.min(screen.right().saturating_sub(w));
    let y = if at.y + h <= screen.bottom() { at.y } else { (at.y + 1).saturating_sub(h).max(screen.y) };
    Rect { x, y, width: w, height: h }
}

fn popup_area(f: &Frame, w: u16, h: u16) -> Rect {
    let area = f.area();
    let [r] = Layout::vertical([Constraint::Length(h.min(area.height))]).flex(Flex::Center).areas(area);
    let [r] = Layout::horizontal([Constraint::Length(w.min(area.width))]).flex(Flex::Center).areas(r);
    r
}

fn popup_block<'a>(title: String) -> Block<'a> {
    Block::bordered()
        .border_type(theme::c().corners)
        .border_style(Style::new().fg(theme::c().accent))
        .title(Span::styled(format!(" {title} "), theme::accent()))
        .padding(Padding::horizontal(1))
}

fn draw_menu(f: &mut Frame, app: &mut App) {
    let Some(menu) = app.menu.as_ref() else { return };
    let filters = menu.track.is_none() && !menu.is_theme();
    let width = menu.items.iter().map(|(_, l, _)| l.width()).max().unwrap_or(10).max(menu.title.width()) as u16 + 12;
    let (w, h) = (width.clamp(30, 64), menu.items.len() as u16 + 4);
    let area = match menu.at {
        Some(at) => popup_at(f.area(), at, w, h),
        None => popup_area(f, w, h),
    };
    let block = popup_block(fit(&menu.title, w.saturating_sub(6) as usize)).title_bottom(
        Line::from(Span::styled(
            if menu.is_theme() {
                " ⏎ use · esc cancel "
            } else if filters {
                " ⏎ change · esc done "
            } else {
                " ⏎ choose · esc close "
            },
            theme::faint(),
        ))
        .right_aligned(),
    );
    let inner = block.inner(area).inner(Margin::new(0, 1));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    app.hits.popup = area;
    for (i, (key, label, action)) in menu.items.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, y, inner.width, 1);
        let selected = i == menu.cursor;
        let style = if selected { Style::new().bg(theme::c().selected).fg(theme::c().bright).add_modifier(Modifier::BOLD) } else { theme::text() };
        let label_style = match action {
            Action::RemoveFromPlaylist => style.fg(theme::c().red),
            _ => style,
        };
        let line = Line::from(vec![
            Span::styled(if selected { theme::c().marker.to_string() } else { " ".into() }, theme::accent()),
            Span::styled(format!(" {label}"), label_style),
        ]);
        f.render_widget(Paragraph::new(line).style(style), row);
        f.render_widget(Line::from(Span::styled(format!("{key} "), theme::faint())).right_aligned(), row);
        app.hits.popup_rows.push((row, i));
    }
}

/// Randomize's genre picker: a search line, the quick picks, and the folder tree.
fn draw_genre_picker(f: &mut Frame, app: &mut App) {
    use crate::genres::Row as G;
    let Some(picker) = app.genre_picker.as_ref() else { return };
    let genres = &app.genre_list;
    let rows = picker.rows(genres);
    let t = theme::c();
    let area = popup_area(f, 58, f.area().height.saturating_sub(6).clamp(12, 34));
    let block = popup_block("Randomize · pick a genre".into())
        .title_bottom(Line::from(Span::styled(" type to search · → open · tab sample · ⏎ randomize · esc ", theme::faint())).right_aligned());
    let inner = block.inner(area).inner(Margin::new(0, 1));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    app.hits.popup = area;
    let [search, _, list] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    // The sample modal covers the bottom of the picker: the list ends above it, so the row
    // under the cursor stays in view.
    let list = if app.sample.is_some() { Rect { height: list.height.saturating_sub(SAMPLE_H - 1).max(1), ..list } } else { list };
    let line = if picker.query.is_empty() {
        Line::from(vec![Span::styled("/ ", theme::accent()), Span::styled(format!("search {} genres, or type any genre", genres.len()), theme::faint())])
    } else {
        Line::from(vec![Span::styled("/ ", theme::accent()), Span::styled(picker.query.to_string(), theme::text())])
    };
    f.render_widget(line, search);
    f.set_cursor_position(Position::new(search.x + 2 + picker.query.cursor_cols(), search.y));
    if rows.is_empty() {
        return;
    }
    let h = list.height as usize;
    let cursor = picker.cursor.min(rows.len() - 1);
    let start = cursor.saturating_sub(h.saturating_sub(1).min(h * 2 / 3)).min(rows.len().saturating_sub(h));
    let mut hits = Vec::new();
    for (n, (i, row)) in rows.iter().enumerate().skip(start).take(h).enumerate() {
        let r = Rect::new(list.x, list.y + n as u16, list.width, 1);
        let selected = i == cursor;
        let base = if selected { Style::new().bg(t.selected) } else { Style::new() };
        let name_style = if selected { theme::bright().add_modifier(Modifier::BOLD) } else { theme::text() };
        let spans: Vec<Span> = match row {
            G::Header(title) => vec![Span::styled(title.to_uppercase(), theme::faint().add_modifier(Modifier::BOLD))],
            G::Quick(g) => vec![Span::styled("  ⚄ ", theme::accent().patch(base)), Span::styled(*g, name_style.patch(base))],
            G::Typed(g) => vec![
                Span::styled("⚄ ", theme::accent().patch(base)),
                Span::styled(format!("Randomize “{g}”"), name_style.patch(base)),
                Span::styled("  any genre Spotify knows", theme::faint().patch(base)),
            ],
            G::Match(g) => {
                let path = crate::genres::path(genres, *g);
                let parent = path.rsplit_once(" › ").map(|(p, _)| format!("  in {p}")).unwrap_or_default();
                vec![
                    Span::styled("⚄ ", theme::accent().patch(base)),
                    Span::styled(genres[*g].name, name_style.patch(base)),
                    Span::styled(parent, theme::faint().patch(base)),
                ]
            }
            G::Node(g) => {
                let node = &genres[*g];
                let indent = "  ".repeat(node.depth);
                if node.children.is_empty() {
                    vec![Span::styled(format!("{indent}  · "), Style::new().fg(t.border).patch(base)), Span::styled(node.name, name_style.patch(base))]
                } else {
                    let open = picker.open.contains(g);
                    vec![
                        Span::styled(format!("{indent}{} ", if open { "▾" } else { "▸" }), theme::accent().patch(base)),
                        Span::styled(node.name, name_style.add_modifier(Modifier::BOLD).patch(base)),
                        Span::styled(format!("  {}", node.children.len()), theme::faint().patch(base)),
                    ]
                }
            }
            G::All(g) => {
                let indent = "  ".repeat(genres[*g].depth + 1);
                vec![
                    Span::styled(format!("{indent}⚄ "), theme::accent().patch(base)),
                    Span::styled(format!("all of {}", genres[*g].name), theme::accent().patch(base)),
                ]
            }
        };
        let used: usize = spans.iter().map(|s| s.width()).sum();
        let mut spans = spans;
        spans.push(Span::styled(" ".repeat((r.width as usize).saturating_sub(used)), base));
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        if row.selectable() {
            hits.push((r, i));
        }
    }
    app.hits.popup_rows.extend(hits);
    if app.sample.is_some() {
        draw_sample(f, app, area);
    }
}

/// Rows of the sample modal.
const SAMPLE_H: u16 = 8;

/// The sample playing (Tab in the genre picker): its cover, song and artist, and the time left,
/// over the bottom of the picker.
fn draw_sample(f: &mut Frame, app: &mut App, picker: Rect) {
    let Some(sample) = app.sample.as_ref() else { return };
    let t = theme::c();
    let w = picker.width.saturating_sub(4).min(56);
    let area = Rect::new(picker.x + (picker.width - w) / 2, picker.bottom().saturating_sub(SAMPLE_H + 1), w, SAMPLE_H);
    let block = Block::bordered()
        .border_type(t.corners)
        .border_style(Style::new().fg(t.accent))
        .title(Span::styled(format!(" ♪ Sample · {} ", sample.genre), theme::accent().add_modifier(Modifier::BOLD)))
        .title_bottom(Line::from(Span::styled(" tab next · ⏎ randomize · esc stop ", theme::faint())).right_aligned())
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let (lines, cover, left) = match &sample.track {
        Some(track) => {
            let left = sample.started.map_or(SAMPLE_LEN, |at| SAMPLE_LEN.saturating_sub(at.elapsed()));
            let album = track.album.as_ref().map(|a| a.name.clone()).unwrap_or_default();
            (
                vec![
                    Line::from(Span::styled(track.name.clone(), theme::bright().add_modifier(Modifier::BOLD))),
                    Line::from(Span::styled(track.artist_names(), theme::accent())),
                    Line::from(Span::styled(album, theme::faint())),
                ],
                track.images().to_vec(),
                Some(left),
            )
        }
        None => (
            vec![
                Line::from(Span::styled(format!("{} Finding the top {} song…", SPINNER[anim(80) % SPINNER.len()], sample.genre), theme::muted())),
                Line::from(Span::styled("What plays now fades out, and comes back after.", theme::faint())),
            ],
            Vec::new(),
            None,
        ),
    };
    let [head, _, bar] = Layout::vertical([Constraint::Length(4), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    draw_page_header(f, &mut app.images, head, &cover, lines);
    if let Some(left) = left {
        // Time left, as a bar that empties.
        let frac = left.as_secs_f64() / SAMPLE_LEN.as_secs_f64();
        let label = format!(" {:>2} s", left.as_secs_f64().ceil() as u64);
        let width = (bar.width as usize).saturating_sub(label.width());
        let full = (frac * width as f64).round() as usize;
        f.render_widget(
            Line::from(vec![
                Span::styled("━".repeat(full), theme::accent()),
                Span::styled("─".repeat(width - full), Style::new().fg(t.border)),
                Span::styled(label, theme::faint()),
            ]),
            bar,
        );
    }
}

fn draw_picker(f: &mut Frame, app: &mut App) {
    let choices: Vec<String> = app.picker_choices().iter().map(|p| p.name.clone()).collect();
    let loading = app.playlists.loading;
    let Some(picker) = app.picker.as_ref() else { return };
    let visible = 12usize;
    let area = popup_area(f, 56, visible as u16 + 6);
    let block = popup_block(format!("Add “{}” to…", fit(&picker.track.name, 30)))
        .title_bottom(Line::from(Span::styled(" type to filter · ⏎ add · esc close ", theme::faint())).right_aligned());
    let inner = block.inner(area).inner(Margin::new(0, 1));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    app.hits.popup = area;
    let [query, _, list] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    f.render_widget(
        Line::from(vec![Span::styled("/ ", theme::accent()), Span::styled(picker.query.to_string(), theme::text())]),
        query,
    );
    f.set_cursor_position(Position::new(query.x + 2 + picker.query.cursor_cols(), query.y));
    if choices.is_empty() {
        let msg = if loading { "Loading your playlists…" } else { "No playlist of yours matches." };
        f.render_widget(Span::styled(msg, theme::faint()), list);
        return;
    }
    let cursor = picker.cursor.min(choices.len() - 1);
    let start = cursor.saturating_sub(list.height.saturating_sub(1) as usize);
    for (n, (i, name)) in choices.iter().enumerate().skip(start).take(list.height as usize).enumerate() {
        let row = Rect::new(list.x, list.y + n as u16, list.width, 1);
        let selected = i == cursor;
        let style = if selected { Style::new().bg(theme::c().selected).fg(theme::c().bright).add_modifier(Modifier::BOLD) } else { theme::text() };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if selected { theme::c().marker.to_string() } else { " ".into() }, theme::accent()),
                Span::styled(format!(" {}", fit(name, list.width.saturating_sub(3) as usize)), style),
            ]))
            .style(style),
            row,
        );
        app.hits.popup_rows.push((row, i));
    }
}

/// The command palette: a filter line, then the matching actions with their keys.
fn draw_palette(f: &mut Frame, app: &mut App) {
    let entries = app.palette_entries();
    let Some(p) = app.palette.as_ref() else { return };
    let area = popup_area(f, 60, 20);
    let block = popup_block("Do anything".into())
        .title_bottom(Line::from(Span::styled(" type to filter · ⏎ run · esc close ", theme::faint())).right_aligned());
    let inner = block.inner(area).inner(Margin::new(0, 1));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    app.hits.popup = area;
    let [query, _, list] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    let prompt = if p.query.is_empty() {
        Line::from(vec![Span::styled(": ", theme::accent()), Span::styled("type what you want to do", theme::faint())])
    } else {
        Line::from(vec![Span::styled(": ", theme::accent()), Span::styled(p.query.to_string(), theme::text())])
    };
    f.render_widget(prompt, query);
    f.set_cursor_position(Position::new(query.x + 2 + p.query.cursor_cols(), query.y));
    if entries.is_empty() {
        f.render_widget(Span::styled("Nothing matches.", theme::faint()), list);
        return;
    }
    let cursor = p.cursor.min(entries.len() - 1);
    let start = cursor.saturating_sub(list.height.saturating_sub(1) as usize);
    let t = theme::c();
    for (n, (i, e)) in entries.iter().enumerate().skip(start).take(list.height as usize).enumerate() {
        let row = Rect::new(list.x, list.y + n as u16, list.width, 1);
        let selected = i == cursor;
        let style = if selected { Style::new().bg(t.selected).fg(t.bright).add_modifier(Modifier::BOLD) } else { theme::text() };
        let room = list.width.saturating_sub(e.key.width() as u16 + 5) as usize;
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if selected { t.marker.to_string() } else { " ".into() }, theme::accent()),
                Span::styled(format!(" {}", fit(&e.label, room)), style),
            ]))
            .style(style),
            row,
        );
        f.render_widget(Line::from(Span::styled(format!("{} ", e.key), theme::faint())).right_aligned(), row);
        app.hits.popup_rows.push((row, i));
    }
}

/// The undo tree (Ctrl+Backspace): every place you went, as branches; where you are is marked.
/// Closed pages show dim and open again when chosen.
fn draw_undo(f: &mut Frame, app: &mut App) {
    let Some(cursor) = app.undo else { return };
    let rows = app.nav.rows();
    let t = theme::c();
    let height = (rows.len() as u16 + 4).clamp(6, f.area().height.saturating_sub(4));
    let area = popup_area(f, 64, height);
    let block = popup_block("Where you went".into())
        .title_bottom(Line::from(Span::styled(" ⏎ go · click a place · esc close ", theme::faint())).right_aligned());
    let inner = block.inner(area).inner(Margin::new(1, 1));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    app.hits.popup = area;
    let h = inner.height as usize;
    let cursor = cursor.min(rows.len().saturating_sub(1));
    let start = cursor.saturating_sub(h.saturating_sub(1).min(h / 2)).min(rows.len().saturating_sub(h));
    let open: std::collections::HashSet<String> = app.pages.iter().map(|p| p.key()).collect();
    // Labels start in one column, after the widest graph (at most half the width).
    let graph_w = rows.iter().map(|r| r.before.width() + 1 + r.after.width()).max().unwrap_or(1).min(inner.width as usize / 2);
    for (n, (i, row)) in rows.iter().enumerate().skip(start).take(h).enumerate() {
        let node = &app.nav.nodes[row.node];
        let r = Rect::new(inner.x, inner.y + n as u16, inner.width, 1);
        let current = row.node == app.nav.cur;
        let closed = matches!(&node.loc, Loc::Page(k) if !open.contains(k));
        let selected = i == cursor;
        let base = if selected { Style::new().bg(t.selected) } else { Style::new() };
        let (dot, dot_style) = if current { ("●", theme::accent()) } else { ("○", theme::faint()) };
        let label_style = match (current, closed) {
            (true, _) => theme::accent().add_modifier(Modifier::BOLD),
            (_, true) => theme::faint(),
            _ if selected => theme::bright(),
            _ => theme::text(),
        };
        let when = crate::undo::ago(node.at);
        let note = if current { "  you are here" } else if closed { "  closed" } else { "" };
        let lane = Style::new().fg(t.border).patch(base);
        let after_w = graph_w.saturating_sub(row.before.width() + 1);
        let after: String = row.after.chars().take(after_w).collect();
        let pad = " ".repeat(after_w.saturating_sub(after.width()) + 2);
        let room = (r.width as usize).saturating_sub(graph_w + 2 + note.width() + when.width() + 2);
        let mut spans = vec![
            Span::styled(row.before.clone(), lane),
            Span::styled(dot, dot_style.patch(base)),
            Span::styled(after, lane),
            Span::styled(pad, base),
            Span::styled(fit(&node.label, room), label_style.patch(base)),
            Span::styled(note, theme::faint().patch(base)),
        ];
        let used: usize = spans.iter().map(|s| s.width()).sum();
        spans.push(Span::styled(" ".repeat((r.width as usize).saturating_sub(used + when.width() + 1)), base));
        spans.push(Span::styled(format!("{when} "), theme::faint().patch(base)));
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        app.hits.popup_rows.push((r, i));
    }
}

fn draw_confirm(f: &mut Frame, app: &mut App) {
    let Some(confirm) = app.confirm.as_ref() else { return };
    let width = (confirm.text.width() as u16 + 8).clamp(36, 70);
    let area = popup_area(f, width, 7);
    let block = popup_block("Are you sure?".into()).border_style(Style::new().fg(theme::c().red));
    let inner = block.inner(area).inner(Margin::new(0, 1));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    app.hits.popup = area;
    let [text, _, buttons] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Span::styled(fit(&confirm.text, text.width as usize), theme::text()), text);
    let yes = span_rect(buttons, buttons.x, 12);
    let no = span_rect(buttons, buttons.x + 15, 12);
    f.render_widget(Span::styled(" y  Remove ", Style::new().fg(theme::c().red).add_modifier(Modifier::BOLD)), yes);
    f.render_widget(Span::styled(" n  Cancel ", theme::muted()), no);
    app.hits.popup_rows.push((yes, 0));
    app.hits.popup_rows.push((no, 1));
}

/// The line floats this many quarter-cells above the bar tops.
const VIZ_LINE_GAP: i32 = 3;
/// Bar tops in eighths of a cell, for smooth heights.
const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
/// Bars reach at most this share of the height.
const VIZ_MAX: f32 = 0.55;

/// Level for column `x` of `width`, between the two nearest bands.
fn viz_column(frame: &viz::Frame, x: u16, width: u16) -> f32 {
    viz_at(frame, x as f32 / width.saturating_sub(1).max(1) as f32)
}

/// Level at `pos` (0 = left edge, 1 = right edge), with smooth blending between bands.
fn viz_at(frame: &viz::Frame, pos: f32) -> f32 {
    let pos = pos.clamp(0.0, 1.0) * (viz::BANDS - 1) as f32;
    let (i, t) = (pos.floor() as usize, pos.fract());
    let j = (i + 1).min(viz::BANDS - 1);
    // Smoothstep between bands: rounded curves instead of straight steps.
    let t = t * t * (3.0 - 2.0 * t);
    frame.levels[i] * (1.0 - t) + frame.levels[j] * t
}

/// Draws the spectrum behind the UI, rising from the bottom of `area`: smooth bars (eighth
/// blocks) in empty cells only. Text, the selection and pictures are left alone, and so is
/// every cell the bars do not reach, so most frames change one cell per column.
#[cfg(test)]
pub fn draw_visualizer(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    draw_visualizer_fx(buf, area, frame, 1.0);
}

/// The visualizer: `reveal` (0..1) sweeps it in from the left and out again, and a light
/// ripples across the bars.
pub fn draw_visualizer_fx(buf: &mut Buffer, area: Rect, frame: &viz::Frame, reveal: f32) {
    let frame = &revealed(frame, reveal);
    if area.height == 0 || frame.levels.iter().all(|&l| l < 0.01) {
        return;
    }
    match theme::c().viz {
        theme::Viz::None => {}
        theme::Viz::Bars => draw_viz_bars(buf, area, frame),
        theme::Viz::Mirror => draw_viz_mirror(buf, area, frame),
        theme::Viz::Dots => draw_viz_dots(buf, area, frame),
        theme::Viz::Wave => draw_viz_wave(buf, area, frame),
    }
}

/// `frame` as far as the sweep has come: bands come in from the left, each with a short ease.
fn revealed(frame: &viz::Frame, reveal: f32) -> viz::Frame {
    if reveal >= 1.0 {
        return *frame;
    }
    let mut out = *frame;
    for (b, l) in out.levels.iter_mut().enumerate() {
        let e = (reveal * 1.6 - b as f32 / (viz::BANDS - 1) as f32 * 0.6).clamp(0.0, 1.0);
        *l *= e * e * (3.0 - 2.0 * e);
    }
    out
}

/// A band of light that travels across the screen (0..1 at `pos`, 0 = left edge).
fn viz_ripple(pos: f32) -> f32 {
    let secs = anim(16) as f32 * 0.016;
    0.5 + 0.5 * (pos * 9.0 - secs * 4.5).sin()
}

/// The middle line of the mirror style, and how far its bars reach.
fn mirror_axis(area: Rect) -> (u16, f32) {
    (area.y + area.height * 3 / 4, (area.height / 4) as f32 * 0.9)
}

/// Smooth bars with a line over them; the light ripples across their tops.
fn draw_viz_wave(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let t = theme::c();
    let max = area.height as f32 * VIZ_MAX;
    let shades = theme::viz_shades();
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
            let shade = shades[(k as usize * shades.len() / area.height.max(1) as usize).min(shades.len() - 1)];
            let pos = (x - area.x) as f32 / area.width.max(2) as f32;
            let glow = 0.3 * viz_ripple(pos) * (k as f32 + 1.0) / (full as f32 + 1.0);
            cell.set_symbol(symbol).set_fg(theme::blend(shade, theme::blend(t.accent, t.accent2, pos), glow));
        }
    }
    draw_viz_line(buf, area, frame);
}

/// A visualizer cell may be drawn only where nothing else is.
fn viz_free(buf: &Buffer, x: u16, y: u16) -> bool {
    let cell = &buf[(x, y)];
    !cell.skip && cell.bg == Color::Reset && cell.symbol() == " "
}

/// Separate chunky bars (2 cells wide, 1 apart), shaded upward and tinted across the screen
/// from accent to accent2, with a brighter cap.
fn draw_viz_bars(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let t = theme::c();
    let shades = theme::viz_shades();
    let max = area.height as f32 * VIZ_MAX;
    let w = area.width.max(2) as f32 - 1.0;
    let mut x = area.x;
    while x + 1 < area.right() {
        let height = viz_column(frame, x + 1 - area.x, area.width) * max;
        let full = height.floor() as u16;
        let part = ((height - full as f32) * 8.0).round() as usize;
        let hue = theme::blend(t.accent, t.accent2, (x - area.x) as f32 / w);
        let top = if part > 0 { full } else { full.saturating_sub(1) };
        for k in 0..=full.min(area.height - 1) {
            let symbol = if k < full { EIGHTHS[8] } else { EIGHTHS[part] };
            if symbol == " " {
                continue;
            }
            let y = area.bottom() - 1 - k;
            let shade = shades[(k as usize * shades.len() / area.height.max(1) as usize).min(shades.len() - 1)];
            let ripple = 0.2 * viz_ripple((x - area.x) as f32 / w);
            let colour = theme::blend(shade, hue, if k == top { 0.45 + ripple } else { 0.15 + ripple * 0.6 });
            for dx in 0..2 {
                if viz_free(buf, x + dx, y) {
                    buf[(x + dx, y)].set_symbol(symbol).set_fg(colour);
                }
            }
        }
        x += 3;
    }
}

/// Bars that grow up and down from the middle line, like a sound wave in an editor; the tips
/// glow toward the accent.
fn draw_viz_mirror(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let t = theme::c();
    let shades = theme::viz_shades();
    // The horizon sits low, in the empty space under the list, and the wave stays small.
    let (mid, half) = mirror_axis(area);
    for x in area.left()..area.right() {
        let height = viz_column(frame, x - area.x, area.width) * half;
        let full = height.floor() as u16;
        let part = ((height - full as f32) * 8.0).round() as usize;
        for k in 0..=full {
            let shade = shades[(k as usize * shades.len() / (half as usize + 1)).min(shades.len() - 1)];
            let ripple = 0.25 * viz_ripple((x - area.x) as f32 / area.width.max(2) as f32);
            let colour = if k + 1 >= full { theme::blend(shade, t.accent, 0.12 + ripple) } else { theme::blend(shade, t.accent, ripple * 0.4) };
            // Up: eighth blocks from the bottom of each cell.
            let up = if k < full { EIGHTHS[8] } else { EIGHTHS[part] };
            if up != " " && mid > area.y + k {
                let y = mid - 1 - k;
                if y >= area.y && viz_free(buf, x, y) {
                    buf[(x, y)].set_symbol(up).set_fg(colour);
                }
            }
            // Down: full cells, then an upper half block for the rest.
            let down = if k < full { "█" } else if part >= 4 { "▀" } else { " " };
            let y = mid + k;
            if down != " " && y < area.bottom() && viz_free(buf, x, y) {
                buf[(x, y)].set_symbol(down).set_fg(theme::blend(colour, Color::Rgb(0, 0, 0), 0.25));
            }
        }
    }
}

/// Star dust: a braille dot at each peak, and a few twinkling dots under it.
fn draw_viz_dots(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let t = theme::c();
    let sub_w = area.width as usize * 2;
    let sub_h = area.height as i32 * 4;
    if sub_w < 2 {
        return;
    }
    let max = area.height as f32 * VIZ_MAX * 4.0;
    let tick = anim(400) as u32;
    let mut dots: std::collections::HashMap<(u16, u16), (u8, bool)> = std::collections::HashMap::new();
    for sx in 0..sub_w {
        let level = viz_at(frame, sx as f32 / (sub_w - 1) as f32);
        let peak = sub_h - 1 - (level * max).round() as i32;
        for sy in peak.max(0)..sub_h {
            // The peak always; below it, about one dot in nine, changing every 0.4 s.
            let h = (sx as u32).wrapping_mul(73_856_093) ^ (sy as u32).wrapping_mul(19_349_663) ^ tick.wrapping_mul(83_492_791);
            let is_peak = sy == peak;
            if !is_peak && !h.is_multiple_of(9) {
                continue;
            }
            let cell = (area.x + (sx / 2) as u16, area.y + (sy / 4) as u16);
            let e = dots.entry(cell).or_insert((0, false));
            e.0 |= BRAILLE[sx % 2][(sy % 4) as usize];
            e.1 |= is_peak;
        }
    }
    for ((x, y), (bits, peak)) in dots {
        if !viz_free(buf, x, y) {
            continue;
        }
        if let Some(c) = char::from_u32(0x2800 + bits as u32) {
            buf[(x, y)].set_char(c).set_fg(if peak { t.viz_line } else { theme::blend(t.viz_high, t.viz_line, 0.4) });
        }
    }
}

/// Braille dot bits for (column 0..2, row 0..4) inside one cell.
const BRAILLE: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// A thin braille line (2x4 dots per cell) that follows the bar tops, a little above them.
fn draw_viz_line(buf: &mut Buffer, area: Rect, frame: &viz::Frame) {
    let sub_w = area.width as usize * 2;
    let sub_h = area.height as i32 * 4;
    if sub_w < 2 {
        return;
    }
    let max = area.height as f32 * VIZ_MAX * 4.0;
    let ys: Vec<i32> = (0..sub_w)
        .map(|sx| sub_h - 1 - (viz_at(frame, sx as f32 / (sub_w - 1) as f32) * max).round() as i32 - VIZ_LINE_GAP)
        .collect();
    let mut dots: std::collections::HashMap<(u16, u16), u8> = std::collections::HashMap::new();
    for sx in 0..sub_w {
        // Not where the bars are not swept in yet.
        if viz_at(frame, sx as f32 / (sub_w - 1) as f32) < 0.002 {
            continue;
        }
        // Join each dot to the midpoint toward its neighbour, so steep parts have no gaps.
        let (y, next) = (ys[sx], ys.get(sx + 1).copied().unwrap_or(ys[sx]));
        let mid = (y + next) / 2;
        for dy in y.min(mid)..=y.max(mid) {
            if !(0..sub_h).contains(&dy) {
                continue;
            }
            let cell = (area.x + (sx / 2) as u16, area.y + (dy / 4) as u16);
            *dots.entry(cell).or_insert(0) |= BRAILLE[sx % 2][(dy % 4) as usize];
        }
    }
    for ((x, y), bits) in dots {
        let cell = &mut buf[(x, y)];
        if cell.skip || cell.bg != Color::Reset || cell.symbol() != " " {
            continue;
        }
        if let Some(c) = char::from_u32(0x2800 + bits as u32) {
            cell.set_char(c).set_fg(theme::c().viz_line);
        }
    }
}

/// Bar ends in eighths of a cell, from the left: for bars that grow to the right.
const LEFT_EIGHTHS: [&str; 9] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];
/// Side bars reach at most this share of the width.
const SIDE_MAX: f32 = 0.66;

/// The side visualizer, over the sidebar's song: the spectrum turned on its side. Bass at the
/// bottom, treble at the top; each band is a bar that grows from the left edge to the right,
/// in empty cells only. A light ripples up the bars, a cap hangs over each bar and falls back
/// with gravity, and sparks fly off the loud bar ends. `reveal` (0..1) sweeps it in from the
/// bottom row up, and out again.
pub fn draw_side_visualizer(buf: &mut Buffer, area: Rect, frame: &viz::Frame, peaks: &viz::Peaks, reveal: f32) {
    if area.height < 2 || area.width < 4 || reveal <= 0.0 {
        return;
    }
    let t = theme::c();
    let secs = anim(16) as f32 * 0.016;
    let h = area.height as f32;
    let maxw = area.width as f32 * SIDE_MAX;
    for k in 0..area.height {
        let y = area.bottom() - 1 - k;
        let pos = k as f32 / (h - 1.0).max(1.0);
        // Rows come in from the bottom up, each with a short ease.
        let e = (reveal * 1.6 - pos * 0.6).clamp(0.0, 1.0);
        let row_in = e * e * (3.0 - 2.0 * e);
        if row_in <= 0.0 {
            continue;
        }
        let level = viz_at(frame, pos) * row_in;
        let len = level * maxw;
        let hue = theme::blend(t.accent, t.accent2, pos);
        // A band of light that travels up the bars.
        let ripple = 0.5 + 0.5 * (pos * 9.0 - secs * 4.5).sin();
        let full = len.floor() as u16;
        let part = ((len - full as f32) * 8.0).round() as usize;
        let cells = if part > 0 { full + 1 } else { full };
        for i in 0..cells.min(area.width) {
            let x = area.x + i;
            if !viz_free(buf, x, y) {
                continue;
            }
            let along = (i as f32 + 0.5) / len.max(1.0);
            let base = theme::blend(t.viz_low, t.viz_high, (i as f32 / maxw.max(1.0)) * 1.4);
            let glow = 0.12 + 0.55 * along.powi(2) + 0.2 * ripple * along;
            let colour = theme::blend(base, hue, glow);
            let symbol = match t.viz {
                // Separate segments, like a level meter.
                theme::Viz::Bars => {
                    if i < full || part >= 4 {
                        "▊"
                    } else {
                        continue;
                    }
                }
                // Dots: braille columns, full or half.
                theme::Viz::Dots => {
                    if i < full || part >= 4 {
                        "⣿"
                    } else {
                        "⡇"
                    }
                }
                _ => {
                    if i < full {
                        "█"
                    } else {
                        LEFT_EIGHTHS[part]
                    }
                }
            };
            buf[(x, y)].set_symbol(symbol).set_fg(colour);
        }
        // The cap: a thin bright line over the bar, falling back when the bar drops.
        let peak = (peak_at(peaks, pos) * row_in * maxw).round() as u16;
        if peak > cells && peak < area.width && viz_free(buf, area.x + peak, y) {
            let cap = theme::blend(t.viz_line, hue, 0.55);
            buf[(area.x + peak, y)].set_symbol("▏").set_fg(cap);
        }
        // Sparks: loud bars throw dots off their ends, which drift right and fade.
        if level > 0.22 {
            let reach = (area.width as f32 - len).min(maxw * 0.6);
            for j in 0..3u32 {
                let seed = (k as u32).wrapping_mul(2_654_435_761) ^ j.wrapping_mul(40_503);
                let offset = (seed % 1000) as f32 / 1000.0;
                let speed = 0.55 + level * 0.9 + (seed >> 10) as f32 % 7.0 * 0.05;
                let phase = (secs * speed + offset).fract();
                // Only some sparks are lit at a time, more when the band is loud.
                if (seed >> 3) % 10 >= (level * 10.0) as u32 + 2 {
                    continue;
                }
                let x = area.x + (len + 1.0 + phase * reach).round() as u16;
                if x >= area.right() || !viz_free(buf, x, y) {
                    continue;
                }
                let symbol = if phase < 0.35 { "•" } else if phase < 0.7 { "∙" } else { "·" };
                buf[(x, y)].set_symbol(symbol).set_fg(theme::blend(hue, t.viz_low, phase));
            }
        }
    }
}

/// The colours of a liquid: deep body, body near the surface, and the surface line.
fn liquid_colours(kind: theme::Liquid) -> (Color, Color, Color) {
    use theme::{blend, Liquid as L};
    let t = theme::c();
    match kind {
        L::Honey => (blend(t.viz_low, t.accent, 0.22), blend(t.viz_high, t.accent, 0.5), blend(t.accent, t.bright, 0.15)),
        L::Soda => (blend(t.viz_low, t.accent2, 0.2), blend(t.viz_high, t.accent, 0.38), blend(t.accent, t.bright, 0.3)),
        L::Lava => (blend(t.viz_low, t.red, 0.3), blend(t.accent2, t.viz_low, 0.35), t.accent),
        L::Ink => (blend(t.viz_low, t.faint, 0.35), blend(t.viz_high, t.muted, 0.35), t.muted),
        L::Ice => (blend(t.viz_low, t.accent2, 0.25), blend(t.viz_high, t.accent, 0.3), blend(t.accent, t.bright, 0.4)),
        L::Dew => (blend(t.viz_low, t.accent2, 0.45), blend(t.viz_high, t.accent, 0.6), t.viz_line),
        L::Neon => (t.viz_low, blend(t.viz_low, t.accent2, 0.3), t.accent),
    }
}

/// A text colour that reads well on `bg`: `fg` if it does already (contrast 4.5:1, as the
/// web's rule for text), else `fg` taken toward white or black until it does.
pub fn readable(fg: Color, bg: Color) -> Color {
    fn lum(c: Color) -> f32 {
        let Color::Rgb(r, g, b) = c else { return 0.5 };
        let ch = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.039_28 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
    }
    let ratio = |a: Color, b: Color| {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    };
    if !matches!(fg, Color::Rgb(..)) || !matches!(bg, Color::Rgb(..)) || ratio(fg, bg) >= 4.5 {
        return fg;
    }
    let to = if lum(bg) < 0.18 { Color::Rgb(255, 255, 255) } else { Color::Rgb(0, 0, 0) };
    (1..=10).map(|i| theme::blend(fg, to, i as f32 / 10.0)).find(|c| ratio(*c, bg) >= 4.5).unwrap_or(to)
}

/// The liquid of the sidebar effect, in `area`: the body and the pieces flung off it, drawn as
/// one shape that flows together where they meet. It fills empty cells (half cells at the edge,
/// so it looks round) and is the background of text it covers; that text is made to stand out
/// from it. Pictures and the selected row are left alone. Then the particles. `reveal` (0..1) grows it from
/// nothing, and shrinks it away.
pub fn draw_liquid(buf: &mut Buffer, area: Rect, liquid: &crate::liquid::Liquid, kind: theme::Liquid, reveal: f32) {
    use theme::{blend, Liquid as L};
    if area.height < 2 || area.width < 4 || reveal <= 0.0 {
        return;
    }
    let t = theme::c();
    let (deep, near, line) = liquid_colours(kind);
    let scale = reveal * reveal * (3.0 - 2.0 * reveal);
    let time = liquid.time;
    // The field once per frame, at 2 × 4 points per cell (the braille dots of a cell).
    let gw = area.width as usize * 2;
    let grid = liquid.grid(area.width, area.height, 2, 4, scale);
    let at = |col: u16, row: u16, dc: usize, dr: usize| grid[(row as usize * 4 + dr) * gw + col as usize * 2 + dc];
    let half_field = |col: u16, row: u16, lower: usize| {
        (at(col, row, 0, lower * 2) + at(col, row, 1, lower * 2) + at(col, row, 0, lower * 2 + 1) + at(col, row, 1, lower * 2 + 1)) / 4.0
    };
    let (cx, cy) = (area.width as f32 / 2.0, area.height as f32);
    for row in 0..area.height {
        for col in 0..area.width {
            let (x, y) = (area.x + col, area.y + row);
            let px = col as f32 + 0.5;
            let top = half_field(col, row, 0);
            let bottom = half_field(col, row, 1);
            let f = top.max(bottom);
            if f <= 1.0 {
                continue;
            }
            let py = row as f32 * 2.0 + 1.0;
            let rim = f < 1.35;
            let inner = ((f - 1.0) / 2.5).clamp(0.0, 1.0).sqrt();
            let mut colour = blend(near, deep, inner);
            match kind {
                // Honey: a slow sheen that turns around inside.
                L::Honey => colour = blend(colour, line, 0.12 * (0.5 + 0.5 * ((py - cy).atan2(px - cx) * 2.0 + time * 0.5).sin())),
                // Ice: light stripes drift through it.
                L::Ice if ((py * 0.65 + px * 0.4 - time * 0.8).sin() > 0.85) => colour = blend(colour, line, 0.22),
                // Neon: scan lines.
                L::Neon if row % 2 == 1 => colour = blend(colour, t.viz_low, 0.4),
                _ => {}
            }
            let body = colour;
            if rim {
                colour = match kind {
                    L::Neon => blend(t.accent, t.accent2, (col as f32 / area.width as f32).clamp(0.0, 1.0)),
                    L::Lava => blend(colour, line, 0.55 + 0.15 * (time * 1.5 + px * 0.5).sin()),
                    _ => blend(colour, line, 0.3),
                };
            }
            let cell = &buf[(x, y)];
            let text = !cell.skip && cell.bg == Color::Reset && cell.symbol() != " ";
            if text {
                // The text floats in the liquid, in a colour that stands out from it.
                if f >= 1.15 {
                    let bg = blend(body, t.viz_low, 0.35);
                    let fg = readable(if cell.fg == Color::Reset { t.text } else { cell.fg }, bg);
                    buf[(x, y)].set_bg(bg).set_fg(fg);
                }
                continue;
            }
            if !viz_free(buf, x, y) {
                continue;
            }
            let half = |both: &'static str| match (top > 1.0, bottom > 1.0) {
                (true, true) => both,
                (true, false) => "▀",
                (false, true) => "▄",
                (false, false) => " ",
            };
            let symbol = match kind {
                // Lava: whole cells only, blocky.
                L::Lava => {
                    if top > 1.0 && bottom > 1.0 {
                        "█"
                    } else {
                        continue;
                    }
                }
                // Ink: shades, light at the skin, dark inside.
                L::Ink => {
                    if top <= 1.0 || bottom <= 1.0 {
                        half("░")
                    } else if f < 1.6 {
                        "░"
                    } else if f < 3.0 {
                        "▒"
                    } else {
                        "▓"
                    }
                }
                // Dew: braille, a dotted skin and a fine mesh inside.
                L::Dew => {
                    let mut bits = 0u8;
                    for (dc, column) in BRAILLE.iter().enumerate() {
                        for (dr, bit) in column.iter().enumerate() {
                            let q = at(col, row, dc, dr);
                            if q > 1.0 && (q < 1.3 || (dc + dr) % 2 == 0) {
                                bits |= bit;
                            }
                        }
                    }
                    if let Some(c) = char::from_u32(0x2800 + bits as u32).filter(|_| bits != 0) {
                        buf[(x, y)].set_char(c).set_fg(if rim { line } else { colour });
                    }
                    continue;
                }
                _ => half("█"),
            };
            if symbol != " " {
                buf[(x, y)].set_symbol(symbol).set_fg(colour);
            }
        }
    }
    for q in &liquid.particles {
        let (col, row) = (q.x.floor(), (q.y / 2.0).floor());
        if col < 0.0 || row < 0.0 || col >= area.width as f32 || row >= area.height as f32 {
            continue;
        }
        let (px, py) = (area.x + col as u16, area.y + row as u16);
        let fade = (q.age / q.life).clamp(0.0, 1.0);
        let (symbol, colour): (&str, Color) = match q.bit {
            crate::liquid::Bit::Bubble => (if q.age < 1.5 { "∘" } else { "°" }, blend(line, t.bright, 0.2 + fade * 0.3)),
            crate::liquid::Bit::Blob => ("●", blend(t.accent, line, 0.5)),
            crate::liquid::Bit::Floe => ("◇", blend(t.bright, t.accent, 0.4 + fade * 0.4)),
            crate::liquid::Bit::Twinkle => ("✧", blend(t.viz_line, t.bright, 0.5 - (fade - 0.5).abs())),
            crate::liquid::Bit::Spark => (if fade < 0.5 { "•" } else { "·" }, blend(t.accent, t.viz_low, fade)),
        };
        let cell = &buf[(px, py)];
        // Over the liquid's body: the body stays the background; never over text or pictures.
        let body = cell.bg == Color::Reset && !cell.skip && matches!(cell.symbol(), "█" | "▓" | "▒" | "░");
        if !(viz_free(buf, px, py) || body) {
            continue;
        }
        let bg = if body { buf[(px, py)].fg } else { Color::Reset };
        buf[(px, py)].set_symbol(symbol).set_fg(colour).set_bg(bg);
    }
}

/// Peak level at `pos`, like `viz_at`.
fn peak_at(peaks: &viz::Peaks, pos: f32) -> f32 {
    viz_at(&viz::Frame { levels: peaks.level }, pos)
}

fn count_label<T: crate::app::Viewable>(l: &Listing<T>, filtering: bool) -> String {
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

/// A song or playlist name with the part the filter matched underlined.
fn with_match(name: String, query: Option<&str>, style: Style) -> Vec<Span<'static>> {
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

fn draw_header(f: &mut Frame, app: &mut App, area: Rect) {
    // The wordmark runs from accent to accent2, letter by letter.
    let t = theme::c();
    let word = format!("{} spotlite", t.logo);
    let n = word.chars().count().saturating_sub(1).max(1) as f32;
    let mut left: Vec<Span> = word
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            Span::styled(ch.to_string(), Style::new().fg(theme::blend(t.accent, t.accent2, i as f32 / n)).add_modifier(Modifier::BOLD))
        })
        .collect();
    left.push(Span::raw("   "));
    let mut x = area.x + Line::from(left.clone()).width() as u16;
    let gap: u16 = match t.header {
        theme::Header::Plain => 3,
        theme::Header::Pills | theme::Header::Brackets => 1,
        theme::Header::Powerline => 0,
    };
    let gap_str = " ".repeat(gap as usize);
    for (n, tab) in Tab::ALL.into_iter().enumerate() {
        let count = match tab {
            Tab::Search => None,
            Tab::Playlists => app.playlists.total,
            Tab::Liked => app.liked.total,
            Tab::Recent => None,
        };
        let active = app.active.is_none() && tab == app.tab;
        let spans = tab_chip(Some(n + 1), None, tab.title(), count, active);
        let w = Line::from(spans.clone()).width() as u16;
        app.hits.tabs.push((span_rect(area, x, w), Loc::Base(tab)));
        x += w + gap;
        left.extend(spans);
        left.push(Span::raw(gap_str.clone()));
    }
    // Open pages, after a rule. When they do not fit, the names of the other pages get shorter
    // (18, then 10 letters), then show only their number and glyph; the page shown keeps its
    // name and always shows. Pages that still do not fit go into the "+N ▾" chip, whose list
    // (hover or click) has every page.
    if !app.pages.is_empty() {
        left.push(Span::styled(" │ ", theme::faint()));
        x += 3;
    }
    // Room for the device dot.
    let end = area.x + area.width.saturating_sub(3);
    let n = app.pages.len();
    let chip = |i: usize, limit: Option<usize>| -> Vec<Span<'static>> {
        let (kind, name) = app.pages[i].title();
        let active = app.active == Some(i);
        let name = match limit {
            _ if name.is_empty() => "…".to_owned(),
            _ if active => fit(name, 18),
            Some(l) => fit(name, l),
            None => String::new(),
        };
        let glyph = match kind {
            "Artist" => '◉',
            "Album" => '◎',
            "Random" => '⚄',
            _ => '≋',
        };
        let key = i + Tab::ALL.len() + 1;
        if name.is_empty() {
            // Glyph only: the glyph is the label.
            tab_chip((key <= 9).then_some(key), None, &glyph.to_string(), None, active)
        } else {
            tab_chip((key <= 9).then_some(key), Some(glyph), &name, None, active)
        }
    };
    let width = |spans: &Vec<Span<'static>>| Line::from(spans.clone()).width() as u16 + gap;
    const MORE_W: u16 = 7;
    let mut chosen: Vec<(usize, Vec<Span<'static>>)> = Vec::new();
    let mut hidden = 0;
    for limit in [Some(18), Some(10), None] {
        let all: Vec<_> = (0..n).map(|i| (i, chip(i, limit))).collect();
        let total: u16 = all.iter().map(|(_, c)| width(c)).sum();
        if x + total <= end || limit.is_none() {
            if x + total <= end {
                chosen = all;
            } else {
                // Still too wide: pages in order while they fit beside the chip, the shown page
                // always among them.
                let room = end.saturating_sub(x + MORE_W);
                let mut used = 0u16;
                let mut keep: Vec<usize> = Vec::new();
                for (i, c) in &all {
                    if used + width(c) <= room {
                        used += width(c);
                        keep.push(*i);
                    }
                }
                if let Some(a) = app.active.filter(|a| !keep.contains(a)) {
                    let aw = width(&all[a].1);
                    while used + aw > room {
                        match keep.pop() {
                            Some(i) => used -= width(&all[i].1),
                            None => break,
                        }
                    }
                    keep.push(a);
                    keep.sort();
                }
                hidden = n - keep.len();
                chosen = all.into_iter().filter(|(i, _)| keep.contains(i)).collect();
            }
            break;
        }
    }
    for (i, spans) in chosen {
        let w = Line::from(spans.clone()).width() as u16;
        app.hits.tabs.push((span_rect(area, x, w), Loc::Page(app.pages[i].key())));
        x += w + gap;
        left.extend(spans);
        left.push(Span::raw(gap_str.clone()));
    }
    if hidden > 0 {
        let open = app.tabs_list.is_some();
        let style = if open {
            Style::new().bg(t.accent).fg(t.viz_low).add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(t.selected).fg(t.bright)
        };
        let label = format!(" +{hidden} ▾ ");
        app.hits.tabs_more = span_rect(area, x, label.width() as u16);
        left.push(Span::styled(label, style));
    }

    let active = app
        .playback
        .as_ref()
        .and_then(|pb| pb.device.as_ref())
        .is_some_and(|d| d.is_active);
    let (dot, label) = match app.device.as_deref() {
        _ if app.offline => (theme::c().red, "offline".to_owned()),
        Some(name) => (if active { theme::c().green } else { theme::c().muted }, name.to_owned()),
        None if app.device_checked => (theme::c().red, "no device".into()),
        None => (theme::c().faint, "connecting".into()),
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

/// The list under the "+N ▾" chip: every open page, its number key, glyph and whole name.
/// A click goes there; "×" closes it. The row under the pointer is lit.
fn draw_tabs_list(f: &mut Frame, app: &mut App, header: Rect) {
    let Some(list) = app.tabs_list else { return };
    let chip = app.hits.tabs_more;
    if chip.width == 0 || app.pages.is_empty() {
        app.tabs_list = None;
        return;
    }
    let t = theme::c();
    let screen = f.area();
    let w = 46.min(screen.width.saturating_sub(2));
    let h = (app.pages.len() as u16 + 2).min(screen.height.saturating_sub(header.bottom()));
    let x = chip.x.min(screen.right().saturating_sub(w + 1));
    let area = Rect::new(x, header.bottom(), w, h);
    let block = popup_block("Open tabs".into());
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    app.hits.tabs_list = Rect { y: chip.y, height: area.bottom() - chip.y, ..area }.union(chip);
    for (n, i) in (0..app.pages.len()).take(inner.height as usize).enumerate() {
        let r = Rect::new(inner.x, inner.y + n as u16, inner.width, 1);
        let (kind, name) = app.pages[i].title();
        let glyph = match kind {
            "Artist" => '◉',
            "Album" => '◎',
            "Random" => '⚄',
            _ => '≋',
        };
        let key = i + Tab::ALL.len() + 1;
        let active = app.active == Some(i);
        let hover = list.hover == Some(i);
        let base = if hover { Style::new().bg(t.selected) } else { Style::new() };
        let mark = if active { theme::marker() } else { " ".into() };
        let num = if key <= 9 { format!("{key} ") } else { "  ".into() };
        let name_style = if active { theme::accent().add_modifier(Modifier::BOLD) } else if hover { theme::bright() } else { theme::text() };
        let room = (r.width as usize).saturating_sub(4 + num.width() + 2 + 9);
        let spans = vec![
            Span::styled(format!("{mark} "), theme::accent().patch(base)),
            Span::styled(num, theme::faint().patch(base)),
            Span::styled(format!("{glyph} "), Style::new().fg(t.accent2).patch(base)),
            Span::styled(fit(name, room), name_style.patch(base)),
        ];
        let used: usize = spans.iter().map(|s| s.width()).sum();
        let kind_label = format!("{} ", kind.to_lowercase());
        let mut spans = spans;
        spans.push(Span::styled(" ".repeat((r.width as usize).saturating_sub(used + kind_label.width() + 3)), base));
        spans.push(Span::styled(kind_label, theme::faint().patch(base)));
        spans.push(Span::styled(" × ", if hover { Style::new().fg(t.red).patch(base) } else { theme::faint() }));
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        app.hits.tabs_rows.push((Rect { width: r.width.saturating_sub(3), ..r }, i));
        app.hits.tabs_close.push((Rect { x: r.right().saturating_sub(3), width: 3, ..r }, i));
    }
}

/// One tab in the header, in the theme's header style. `key` is its number key (faint).
fn tab_chip(key: Option<usize>, glyph: Option<char>, label: &str, count: Option<usize>, active: bool) -> Vec<Span<'static>> {
    let t = theme::c();
    // spotlite's own tab names take the theme's letter case; a page's name (a song, an
    // artist) is shown as written.
    let label = match (glyph, t.titles) {
        (Some(_), _) => label.to_owned(),
        (None, theme::Titles::Spaced) => label.to_uppercase(),
        (None, _) => theme::title(label),
    };
    let count = count.map(|n| format!(" {n}")).unwrap_or_default();
    let glyph = glyph.map(|g| format!("{g} ")).unwrap_or_default();
    let key = key.map(|k| k.to_string());
    let dark = t.viz_low;
    match t.header {
        theme::Header::Plain => {
            let style = if active {
                Style::new().fg(t.bright).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                theme::muted()
            };
            let mut v = Vec::new();
            if let Some(k) = key {
                v.push(Span::styled(format!("{k} "), theme::faint()));
            }
            v.push(Span::styled(glyph, if active { theme::accent() } else { theme::faint() }));
            v.push(Span::styled(label, style));
            v.push(Span::styled(count, theme::faint()));
            v
        }
        theme::Header::Pills => {
            let (bg, fg) = if active { (t.accent, dark) } else { (t.selected, t.muted) };
            let base = Style::new().bg(bg).fg(fg);
            let mut v = vec![Span::styled(" ", base)];
            if let Some(k) = key {
                v.push(Span::styled(format!("{k} "), base.fg(if active { dark } else { t.faint })));
            }
            v.push(Span::styled(format!("{glyph}{label}"), if active { base.add_modifier(Modifier::BOLD) } else { base }));
            v.push(Span::styled(format!("{count} "), base.fg(if active { dark } else { t.faint })));
            v
        }
        theme::Header::Brackets => {
            let (open, close) = if active { ("[", "]") } else { (" ", " ") };
            let style = if active { Style::new().fg(t.bright).add_modifier(Modifier::BOLD) } else { theme::muted() };
            let mut v = vec![Span::styled(open, theme::accent())];
            if let Some(k) = key {
                v.push(Span::styled(format!("{k}:"), theme::faint()));
            }
            v.push(Span::styled(format!("{glyph}{label}"), style));
            v.push(Span::styled(count, theme::faint()));
            v.push(Span::styled(close, theme::accent()));
            v
        }
        theme::Header::Powerline => {
            let (bg, fg) = if active { (t.accent, dark) } else { (t.selected, t.muted) };
            let base = Style::new().bg(bg).fg(fg);
            let sep = if t.nerd { "\u{e0b0}" } else { "▌" };
            let mut v = vec![Span::styled(" ", base)];
            if let Some(k) = key {
                v.push(Span::styled(format!("{k} "), base.fg(if active { dark } else { t.faint })));
            }
            v.push(Span::styled(format!("{glyph}{label}{count} "), if active { base.add_modifier(Modifier::BOLD) } else { base }));
            v.push(Span::styled(sep, Style::new().fg(bg)));
            v
        }
    }
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let [input, list] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).spacing(gap()).areas(area);
    app.hits.search_box = input;
    let block = panel()
        .border_style(theme::border(app.typing))
        .title(Span::styled(format!(" {} ", theme::title("Search")), if app.typing { theme::accent() } else { theme::muted() }));
    let inner = block.inner(input);
    let content = if app.query.is_empty() && !app.typing {
        Line::from(vec![
            Span::styled("/ ", theme::accent()),
            Span::styled("search songs, artists, albums or playlists", theme::faint()),
        ])
    } else {
        Line::from(vec![Span::styled("/ ", theme::accent()), Span::styled(app.query.to_string(), theme::text())])
    };
    f.render_widget(Paragraph::new(content).block(block), input);
    if app.typing {
        let x = inner.x + 2 + app.query.cursor_cols();
        f.set_cursor_position(Position::new(x.min(inner.right().saturating_sub(1)), inner.y));
    }

    // Title: the result kinds (click one, or t / T), then the query.
    let mut spans = vec![Span::raw(" ")];
    let mut x = title_x(list) + 1;
    for kind in SearchKind::ALL {
        let on = kind == app.search_kind;
        let style = if on {
            Style::new().fg(theme::c().bright).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            theme::muted()
        };
        let label = theme::title(kind.title());
        let w = label.width() as u16;
        app.hits.search_kinds.push((span_rect(list, x, w), kind));
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(" · ", theme::faint()));
        x += w + 3;
    }
    spans.pop();
    if !app.searched.is_empty() {
        spans.push(Span::styled("  for ", theme::faint()));
        spans.push(Span::styled(format!("“{}” ", app.searched), theme::bright()));
    } else {
        spans.push(Span::raw(" "));
    }
    let title = Line::from(spans);
    let empty = if app.searched.is_empty() && !app.search_history.is_empty() {
        let recent: Vec<String> = app.search_history.iter().take(3).map(|q| format!("“{q}”")).collect();
        format!("Search Spotify\n/ to type · ↑ in the box for earlier searches: {}", recent.join(", "))
    } else if app.searched.is_empty() {
        "Search Spotify\n/ to type · Enter to search · t for songs, artists, albums or playlists".to_owned()
    } else {
        "No results.\nTry other words, or t for another kind".to_owned()
    };
    if app.search_kind == SearchKind::Songs {
        draw_tracks(f, app, list, title, &empty);
    } else {
        draw_found(f, app, list, title, &empty);
    }
}

/// Artists, albums or playlists found by search: cover, name, details.
fn draw_found(f: &mut Frame, app: &mut App, area: Rect, title: Line, empty: &str) {
    let thumb = app.images.square_cols(PLAYLIST_ROW);
    let user = app.user_id.clone();
    let playing_ctx = app.playback.as_ref().and_then(|pb| pb.context.as_ref()).map(|c| c.uri.clone());
    let l = &mut app.found;
    let block = panel()
        .title(title)
        .title(Line::from(Span::styled(count_label(l, app.filtering), theme::faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if l.items.is_empty() {
        draw_centered(f, inner, if l.loading { "Loading…" } else { empty });
        return;
    }
    if l.len() == 0 {
        draw_centered(f, inner, "Nothing matches the filter.\nesc clears it");
        return;
    }
    let widths = [Constraint::Length(thumb), Constraint::Fill(1), Constraint::Length(12)];
    let cols = column_widths(inner.width, &widths);
    let len = l.len();
    let win = window(&mut l.state, len, inner.height, PLAYLIST_ROW, 0);
    let rows = l.rows().skip(win.start).take(win.len()).map(|(_, item)| item).collect::<Vec<_>>();
    let rows = rows.into_iter().map(|item| {
        let (sub, right, uri) = match item {
            Found::Artist(a) => ("Artist  ·  ⏎ opens the page, with radios".to_owned(), String::new(), format!("spotify:artist:{}", a.id)),
            Found::Album(a) => {
                let artists = a.artists.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", ");
                let kind = match a.album_type.as_deref() {
                    Some("single") => "Single",
                    Some("compilation") => "Compilation",
                    _ => "Album",
                };
                let sub = [Some(kind.to_owned()), a.year().map(str::to_owned), (!artists.is_empty()).then_some(artists)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let right = a.total_tracks.map(|n| format!("{n} tracks")).unwrap_or_default();
                (sub, right, a.uri.clone().unwrap_or_default())
            }
            Found::Playlist(p) => {
                let mine = p.collaborative || user.as_deref() == Some(p.owner.id.as_str());
                let owner = if mine { "by you".to_owned() } else { format!("by {}", p.owner_name()) };
                let right = p.track_count().map(|n| format!("{n} tracks")).unwrap_or_default();
                (format!("Playlist · {owner}"), right, p.uri.clone())
            }
        };
        let playing = playing_ctx.as_deref() == Some(uri.as_str());
        let mut first = vec![Span::styled(
            fit(item.name(), cols[1].saturating_sub(if playing { 2 } else { 0 })),
            if playing { theme::accent() } else { theme::text() },
        )];
        if playing {
            first.insert(0, Span::styled(format!("{} ", theme::c().logo), theme::accent()));
        }
        Row::new(vec![
            Cell::from(""),
            Cell::from(Text::from(vec![Line::from(first), Line::from(Span::styled(fit(&sub, cols[1]), theme::faint()))])),
            Cell::from(Line::from(Span::styled(right, theme::faint())).right_aligned()),
        ])
        .height(PLAYLIST_ROW)
    });
    let marker = Span::styled(theme::marker(), theme::accent());
    let table = Table::new(rows, widths).highlight_symbol(Text::from(vec![Line::from(marker.clone()), Line::from(marker)]));
    let len = l.len();
    app.hits.scrollbar = draw_table(f, table, inner, area, &mut l.state, len, PLAYLIST_ROW, 0, Some(win.start));
    let offset = l.state.offset();
    let visible = (inner.height / PLAYLIST_ROW) as usize;
    let images = &mut app.images;
    let shown: Vec<usize> = app.found.rows().skip(offset).take(visible).map(|(i, _)| i).collect();
    for (n, item) in shown.into_iter().map(|i| &app.found.items[i]).enumerate() {
        let rect = Rect::new(inner.x + 1, inner.y + n as u16 * PLAYLIST_ROW, thumb, PLAYLIST_ROW);
        if rect.right() <= inner.right() && rect.bottom() <= inner.bottom() {
            draw_cover(f, images, item.images(), rect);
        }
    }
    app.hits.list = inner;
    app.hits.row_height = PLAYLIST_ROW;
    app.hits.list_offset = offset;
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
    let filtering = app.filtering;
    let marks = std::mem::take(&mut app.marks);
    let hover = app.hover;
    let is_on = app.playback.as_ref().is_some_and(|pb| pb.is_playing);
    let mark = theme::playing_mark(is_on, anim(250));
    let empty_set = std::collections::HashSet::new();
    let liked = app.library.as_ref().map_or(&empty_set, |l| &l.tracks);
    let Some(listing) = (match app.view() {
        View::Search => Some(&mut app.search),
        View::Liked => Some(&mut app.liked),
        View::Recent => Some(&mut app.recent),
        View::Playlist => app.open.as_mut().map(|o| &mut o.tracks),
        View::Album | View::Radio | View::Random => match app.active.and_then(|i| app.pages.get_mut(i)) {
            Some(Page::Album(a)) => Some(&mut a.tracks),
            Some(Page::Radio(r)) => Some(&mut r.tracks),
            Some(Page::Random(r)) => Some(&mut r.tracks),
            _ => None,
        },
        View::Playlists | View::Artist => None,
    }) else {
        return;
    };
    let block = panel()
        .title(title)
        .title(Line::from(Span::styled(count_label(listing, filtering), theme::faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);

    if listing.items.is_empty() {
        let msg = if listing.loading { "Loading…" } else { empty };
        draw_centered(f, inner, msg);
        return;
    }
    if listing.len() == 0 {
        let msg = if listing.loading { "Loading…" } else { "No song matches the filter.\nesc clears it" };
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
    widths.push(Constraint::Length(1));
    head.push(Cell::from(""));
    widths.push(Constraint::Length(5));
    head.push(Cell::from(Line::from("Time").right_aligned()));

    let cols = column_widths(inner.width, &widths);
    let len = listing.len();
    let win = window(&mut listing.state, len, inner.height, 1, 2);
    let query = listing.filter.as_ref().map(|f| f.to_string());
    let rows = listing.rows().skip(win.start).take(win.len()).map(|(i, t): (usize, &Track)| {
        let is_playing = playing.as_deref() == Some(t.uri.as_str());
        let num = if marks.contains(&t.uri) {
            Span::styled("◆", theme::accent().add_modifier(Modifier::BOLD))
        } else if is_playing {
            Span::styled(mark.to_string(), theme::accent())
        } else {
            Span::styled((i + 1).to_string(), theme::faint())
        };
        let mut cells = vec![
            Cell::from(Line::from(num).right_aligned()),
            Cell::from(""),
            Cell::from({
                let name = fit(&t.name, cols[2]);
                let width = name.width();
                let style = if is_playing { theme::accent() } else { theme::text() };
                let mut spans = with_match(name, query.as_deref(), style);
                if theme::c().leaders && cols[2] > width + 3 {
                    // A printed track list: dots lead from the title toward the next column.
                    let dots = ".".repeat(cols[2] - width - 2);
                    spans.push(Span::styled(format!(" {dots}"), Style::new().fg(theme::dim())));
                }
                Line::from(spans)
            }),
        ];
        if show_artist {
            cells.push(Cell::from(Span::styled(fit(&t.artist_names(), cols[3]), theme::muted())));
        }
        if show_album {
            cells.push(Cell::from(Span::styled(fit(t.album_name(), cols[4]), theme::faint())));
        }
        cells.push(Cell::from(if liked.contains(&t.uri) {
            Span::styled(theme::heart(true), Style::new().fg(theme::c().love))
        } else {
            Span::raw("")
        }));
        cells.push(Cell::from(Line::from(Span::styled(format_ms(t.duration_ms), theme::faint())).right_aligned()));
        Row::new(cells)
    });
    let table = Table::new(rows, widths)
        .header(Row::new(head).style(theme::faint()).bottom_margin(1))
        .highlight_symbol(Text::from(Span::styled(theme::marker(), theme::accent())));
    let len = listing.len();
    let bar = draw_table(f, table, inner, area, &mut listing.state, len, 1, 2, Some(win.start));
    let offset = listing.state.offset();
    let rows_area = Rect { y: inner.y + 2, height: inner.height.saturating_sub(2), ..inner };

    // Thumbnails over the empty second column of each visible row.
    let x = inner.x + 1 + cols[0] as u16 + 2;
    // The row under the pointer: its text a little brighter.
    if let Some(h) = hover.filter(|h| *h >= offset && Some(*h) != listing.state.selected()) {
        let y = rows_area.y + (h - offset) as u16;
        if y < rows_area.bottom() {
            let t = theme::c();
            let buf = f.buffer_mut();
            for x in rows_area.left()..rows_area.right() {
                let cell = &mut buf[(x, y)];
                if cell.fg == t.text || cell.fg == t.muted {
                    cell.set_fg(t.bright);
                } else if cell.fg == t.faint {
                    cell.set_fg(t.muted);
                }
            }
        }
    }
    let shown: Vec<&Track> = listing.rows().skip(offset).take(rows_area.height as usize).map(|(_, t)| t).collect();
    for (n, t) in shown.into_iter().enumerate() {
        let rect = Rect::new(x, rows_area.y + n as u16, thumb, 1);
        if rect.right() <= inner.right() {
            draw_cover(f, &mut app.images, t.images(), rect);
        }
    }
    draw_position(f, area, listing.selected(), listing.len());
    app.marks = marks;
    app.hits.scrollbar = bar;
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
    f.render_widget(Paragraph::new(lines).style(Style::new().bg(theme::c().selected)), area);
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
    title.push(Span::styled(format!(" {} ", theme::title("Playlists")), if path.is_empty() { theme::bright() } else { theme::muted() }));
    for (i, name) in path.iter().enumerate() {
        let last = i + 1 == path.len();
        title.push(Span::styled("› ", theme::faint()));
        title.push(Span::styled(format!("{name} "), if last { theme::bright() } else { theme::muted() }));
    }
    let title = Line::from(title);
    if !path.is_empty() {
        app.hits.back = span_rect(area, title_x(area), title.width() as u16);
    }

    let images = &mut app.images;
    let (entries, state, label, loading): (Vec<Entry>, &mut TableState, String, bool) = if foldered {
        let l = &mut app.rows;
        let label = if app.playlists.loading { " loading… ".into() } else { count_label(l, app.filtering) };
        let idx: Vec<usize> = l.rows().map(|(i, _)| i).collect();
        let entries = idx
            .iter()
            .map(|&i| match &l.items[i] {
                PlRow::Folder { name, count, .. } => Entry::Folder { name, count: *count },
                PlRow::Playlist(p) => Entry::Playlist(p),
            })
            .collect();
        (entries, &mut l.state, label, app.playlists.loading)
    } else {
        let l = &mut app.playlists;
        let label = count_label(l, app.filtering);
        let idx: Vec<usize> = l.rows().map(|(i, _)| i).collect();
        (idx.iter().map(|&i| Entry::Playlist(&l.items[i])).collect(), &mut l.state, label, l.loading)
    };
    let block = panel()
        .title(title)
        .title(Line::from(Span::styled(label, theme::faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if entries.is_empty() {
        let filtered = if foldered { app.rows.filter.is_some() } else { app.playlists.filter.is_some() };
        let msg = match (loading, filtered) {
            (true, _) => "Loading…",
            (false, true) => "No playlist matches the filter.\nesc clears it",
            (false, false) => "No playlists.\nR reloads the list",
        };
        draw_centered(f, inner, msg);
        return;
    }

    let widths = [Constraint::Length(thumb), Constraint::Fill(1), Constraint::Length(12)];
    let cols = column_widths(inner.width, &widths);
    let win = window(state, entries.len(), inner.height, PLAYLIST_ROW, 0);
    let rows = entries[win.clone()].iter().map(|e| {
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
            first.insert(0, Span::styled(format!("{} ", theme::c().logo), theme::accent()));
        }
        Row::new(vec![
            Cell::from(""),
            Cell::from(Text::from(vec![Line::from(first), Line::from(Span::styled(fit(&sub, cols[1]), theme::faint()))])),
            Cell::from(Line::from(Span::styled(count, theme::faint())).right_aligned()),
        ])
        .height(PLAYLIST_ROW)
    });
    let marker = Span::styled(theme::marker(), theme::accent());
    let table = Table::new(rows, widths).highlight_symbol(Text::from(vec![Line::from(marker.clone()), Line::from(marker)]));
    let len = entries.len();
    let bar = draw_table(f, table, inner, area, state, len, PLAYLIST_ROW, 0, Some(win.start));
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
    app.hits.scrollbar = bar;
}

/// The rows of a list that fit in `height` (after `header` rows), from the list's scroll: it
/// moves, as the table's own does, only as far as needed to show the selection. A list then
/// builds only these rows, not every row on every frame.
fn window(state: &mut TableState, len: usize, height: u16, row_height: u16, header: u16) -> std::ops::Range<usize> {
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
fn draw_table(
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

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// An empty or loading list: "Loading…" gets a spinner; any other message gets the theme's
/// logo above it. A second line after "\n" is a hint (the key to press).
fn draw_centered(f: &mut Frame, area: Rect, msg: &str) {
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

fn draw_now_playing(f: &mut Frame, app: &mut App, area: Rect) {
    let block = panel();
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 {
        draw_transport(f, app, inner);
        return;
    }
    if inner.height < 4 {
        draw_player_compact(f, app, inner);
        return;
    }

    let art_rows = inner.height;
    let art_cols = app.images.square_cols(art_rows);
    let [art, _, info] =
        Layout::horizontal([Constraint::Length(art_cols), Constraint::Length(2), Constraint::Min(10)]).areas(inner);
    let [l1, l2, l3, l4] = Layout::vertical([Constraint::Length(1); 4]).areas(info);

    let pb = app.playback.clone();
    let track = pb.as_ref().and_then(|p| p.item.as_ref());
    draw_cover(f, &mut app.images, track.map_or(&[][..], |t| t.images()), art);
    if track.is_some() {
        app.hits.np_album = art;
    }

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
            Span::styled("spotifyd not found. Start it: systemctl --user start spotifyd", Style::new().fg(theme::c().red))
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
    let room = (title_area.width as usize).saturating_sub(8);
    let name = fit(&track.name, room);
    let mark = theme::playing_mark(pb.is_playing, anim(250));
    let mut title = vec![Span::styled(format!("{mark} "), theme::accent()), Span::styled(name.clone(), theme::bright())];
    let heart_x = title_area.x + 2 + name.width() as u16 + 2;
    if known {
        title.push(Span::raw("  "));
        title.extend(heart_button(liked, app.like_burst));
        app.hits.like = span_rect(title_area, heart_x, 3);
    }
    f.render_widget(Line::from(title), title_area);
    if known {
        draw_like_burst(f, app.like_burst, Rect { x: heart_x, width: 3, ..title_area }, title_area);
    }
    f.render_widget(modes, modes_area);

    // Line 2: artist · album, and where it plays from.
    let from = app.context_name().map(|n| format!("from {n}")).unwrap_or_default();
    let from_width = (from.width() as u16).min(l2.width / 3);
    let artists_w = (track.artist_names().width() as u16).min(l2.width);
    app.hits.np_artist = span_rect(l2, l2.x, artists_w);
    let [who_area, from_area] = Layout::horizontal([Constraint::Min(0), Constraint::Length(from_width)])
        .spacing(if from.is_empty() { 0 } else { 3 })
        .areas(l2);
    let mut who = track.artist_names();
    // The album, unless it only repeats the song's name (a single).
    if !track.album_name().is_empty() && !track.album_name().eq_ignore_ascii_case(&track.name) {
        who = format!("{who}  {}  {}", theme::c().bullet, track.album_name());
    }
    f.render_widget(Line::from(Span::styled(fit(&who, who_area.width as usize), theme::muted())), who_area);
    f.render_widget(
        Line::from(Span::styled(fit(&from, from_width as usize), theme::faint())).right_aligned(),
        from_area,
    );

    // Line 3: progress bar.
    let (line, bar) = progress_line(app.progress_ms(), track.duration_ms, pb.is_playing, l3.width);
    app.hits.progress = Rect { x: l3.x + bar.0, width: bar.1, ..l3 };
    app.hits.time = Rect { x: l3.x + bar.0 + bar.1, width: l3.width.saturating_sub(bar.0 + bar.1), ..l3 };
    f.render_widget(line, l3);
}

/// The stage column: a large cover, then the song, its artist and album, and what is next.
fn draw_stage(f: &mut Frame, app: &mut App, area: Rect) {
    let block = panel().title(Span::styled(format!(" {} ", theme::title("Now playing")), theme::muted()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let pb = app.playback.clone();
    let Some((pb, track)) = pb.as_ref().and_then(|p| p.item.as_ref().map(|t| (p, t))) else {
        draw_centered(f, inner, "Nothing playing\nSpace resumes · Enter plays the selected song");
        return;
    };
    // The largest square cover that leaves room for 7 lines of text.
    let mut rows = inner.height.saturating_sub(8).min(inner.width / 2);
    while rows > 2 && app.images.square_cols(rows) > inner.width {
        rows -= 1;
    }
    let cols = app.images.square_cols(rows);
    let cover = Rect { x: inner.x + inner.width.saturating_sub(cols) / 2, y: inner.y + 1, width: cols, height: rows };
    draw_cover(f, &mut app.images, track.images(), cover);
    app.hits.np_album = cover;
    let t = theme::c();
    let w = inner.width as usize;
    let mark = theme::playing_mark(pb.is_playing, anim(250));
    let year = track.album.as_ref().and_then(|a| a.year()).map(|y| format!("  {}  {y}", t.bullet)).unwrap_or_default();
    let known = matches!(&app.saved, Some((uri, _)) if *uri == track.uri);
    let liked = matches!(&app.saved, Some((uri, true)) if *uri == track.uri);
    let name = fit(&track.name, w.saturating_sub(if known { 8 } else { 2 }));
    let mut first = vec![Span::styled(format!("{mark} "), theme::accent()), Span::styled(name.clone(), theme::bright())];
    if known {
        first.push(Span::raw("  "));
        first.extend(heart_button(liked, app.like_burst));
    }
    let first_w = Line::from(first.clone()).width() as u16;
    let mut lines = vec![
        Line::from(first),
        Line::from(Span::styled(fit(&track.artist_names(), w), Style::new().fg(t.accent))),
        Line::from(Span::styled(fit(&format!("{}{year}", track.album_name()), w), theme::muted())),
    ];
    if let Some(next) = &app.up_next {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(theme::title("Up next"), theme::faint())));
        lines.push(Line::from(Span::styled(fit(&format!("{}  {}  {}", next.name, t.bullet, next.artist_names()), w), theme::text())));
    }
    let text_y = cover.bottom() + 1;
    let text = Rect { x: inner.x, y: text_y, width: inner.width, height: inner.bottom().saturating_sub(text_y) };
    if known && text.height > 0 {
        // The heart, in the centred first line: after the mark, the name and two spaces.
        let x0 = text.x + text.width.saturating_sub(first_w) / 2;
        let heart = Rect { x: x0 + 2 + name.width() as u16 + 2, y: text.y, width: 3, height: 1 };
        app.hits.like = heart;
    }
    app.hits.np_artist = Rect { x: text.x, y: text.y + 1, width: track.artist_names().width().min(w) as u16, height: 1 };
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), text);
    if known && text.height > 0 {
        draw_like_burst(f, app.like_burst, app.hits.like, Rect { height: 1, ..text });
    }
    if app.hits.np_artist.y >= text.bottom() {
        app.hits.np_artist = Rect::default();
    } else {
        // Centred text: the artist line starts in the middle.
        let aw = app.hits.np_artist.width;
        app.hits.np_artist.x = text.x + text.width.saturating_sub(aw) / 2;
    }
}

/// The like button: a heart on a soft pill, 3 cells wide (easy to click, not squeezed).
/// Liked: a vivid heart; not liked: an outline. Right after a click the pill flashes.
fn heart_button(liked: bool, burst: Option<(std::time::Instant, bool)>) -> Vec<Span<'static>> {
    let t = theme::c();
    let flash = burst
        .map(|(at, _)| at.elapsed().as_millis() as f32 / crate::app::LIKE_BURST.as_millis() as f32)
        .filter(|f| *f < 1.0);
    let (glyph, fg, bg) = if liked {
        // While the burst runs the pill starts bright and settles.
        let glow = flash.map_or(0.0, |f| 1.0 - f);
        (format!(" {} ", theme::heart(true)), t.love, theme::blend(theme::blend(t.selected, t.love, 0.28), t.love, glow * 0.45))
    } else {
        (format!(" {} ", theme::heart(false)), t.muted, t.selected)
    };
    let fg = if liked && flash.is_some() { theme::blend(t.love, t.bright, 0.3) } else { fg };
    vec![Span::styled(glyph, Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD))]
}

/// Sparkles that fly out from the heart after a like (and a short fade after an unlike),
/// drawn only in empty cells around the button.
fn draw_like_burst(f: &mut Frame, burst: Option<(std::time::Instant, bool)>, heart: Rect, room: Rect) {
    let Some((at, liked)) = burst else { return };
    let total = crate::app::LIKE_BURST.as_millis() as f32;
    let p = at.elapsed().as_millis() as f32 / total;
    if p >= 1.0 {
        return;
    }
    let t = theme::c();
    let colour = theme::blend(if liked { t.love } else { t.muted }, t.faint, p);
    let frames: &[char] = if liked { &['✦', '✧', '⋆', '·', '·', ' '] } else { &['·', ' '] };
    let glyph = frames[((p * frames.len() as f32) as usize).min(frames.len() - 1)];
    // Sparkles move outward: 1 cell away, then 2.
    let reach = if p < 0.4 { 1 } else { 2 };
    let buf = f.buffer_mut();
    let spots = [heart.x.checked_sub(reach), Some(heart.right() + reach - 1)];
    for x in spots.into_iter().flatten() {
        if x >= room.x && x < room.right() && buf[(x, heart.y)].symbol() == " " {
            buf[(x, heart.y)].set_char(glyph).set_fg(colour);
        }
    }
    // A spark above the heart, when the row above is in the same panel.
    if liked && p < 0.6 && heart.y > room.y.saturating_sub(1) && heart.y > 0 {
        let (x, y) = (heart.x + 1, heart.y - 1);
        if buf[(x, y)].symbol() == " " {
            buf[(x, y)].set_char(if p < 0.3 { '˙' } else { '·' }).set_fg(colour);
        }
    }
}

/// One line under the stage column: ◀◀ ‖ ▶▶, the progress bar, the volume.
fn draw_transport(f: &mut Frame, app: &mut App, line: Rect) {
    let pb = app.playback.clone();
    let Some((pb, track)) = pb.as_ref().and_then(|p| p.item.as_ref().map(|t| (p, t))) else {
        f.render_widget(Line::from(Span::styled("Nothing playing · Space resumes", theme::muted())), line);
        return;
    };
    let play = if pb.is_playing { "‖" } else { "▶" };
    app.hits.prev = span_rect(line, line.x, 2);
    app.hits.play = span_rect(line, line.x + 3, 2);
    app.hits.next = span_rect(line, line.x + 5, 3);
    let controls = Line::from(vec![
        Span::styled("◀◀ ", theme::muted()),
        Span::styled(play, theme::accent()),
        Span::styled("  ▶▶", theme::muted()),
    ]);
    f.render_widget(controls, Rect { width: 9.min(line.width), ..line });
    let vol_w = if line.width >= 60 { 16 } else { 0 };
    // Shuffle and repeat, before the volume.
    let modes_w = if line.width >= 40 { 7 } else { 0 };
    if modes_w > 0 {
        let mode_on = |on: bool| if on { theme::accent() } else { theme::faint() };
        let repeat = if pb.repeat_state == "track" { "↻1" } else { "↻ " };
        let x = line.right().saturating_sub(vol_w + modes_w);
        let modes = Line::from(vec![
            Span::styled("⇄", mode_on(pb.shuffle_state)),
            Span::raw("  "),
            Span::styled(repeat, mode_on(pb.repeat_state != "off")),
        ]);
        f.render_widget(modes, Rect { x, width: 5, ..line });
        app.hits.shuffle = span_rect(line, x, 1);
        app.hits.repeat = span_rect(line, x + 3, 2);
    }
    let bar_area = Rect { x: line.x + 11, width: line.width.saturating_sub(13 + vol_w + modes_w), ..line };
    let (bar, at) = progress_line(app.progress_ms(), track.duration_ms, pb.is_playing, bar_area.width);
    app.hits.progress = Rect { x: bar_area.x + at.0, width: at.1, ..bar_area };
    app.hits.time = Rect { x: bar_area.x + at.0 + at.1, width: bar_area.width.saturating_sub(at.0 + at.1), ..bar_area };
    f.render_widget(bar, bar_area);
    if let (Some(volume), true) = (app.volume(), vol_w > 0) {
        let (vline, vbar) = volume_line(volume, app.muted(), vol_w - 7, false);
        let rect = Rect { x: line.right() - vol_w + 1, width: vol_w - 1, ..line };
        app.hits.volume = Rect { x: rect.x + vbar.0, width: vbar.1, ..rect };
        f.render_widget(vline, rect);
    }
}

/// The player in two lines: title · artist, and the controls with the progress bar.
fn draw_player_compact(f: &mut Frame, app: &mut App, inner: Rect) {
    let [l1, l2] = Layout::vertical([Constraint::Length(1); 2]).areas(inner);
    let pb = app.playback.clone();
    let Some((pb, track)) = pb.as_ref().and_then(|p| p.item.as_ref().map(|t| (p, t))) else {
        f.render_widget(Line::from(Span::styled("Nothing playing · Space resumes", theme::muted())), l1);
        if app.device_checked && app.device.is_none() {
            let hint = "spotifyd not found. Start it: systemctl --user start spotifyd";
            f.render_widget(Line::from(Span::styled(hint, Style::new().fg(theme::c().red))), l2);
        }
        return;
    };
    let mark = theme::playing_mark(pb.is_playing, anim(250));
    // The like button at the right end of the first line.
    let known = matches!(&app.saved, Some((uri, _)) if *uri == track.uri);
    let liked = matches!(&app.saved, Some((uri, true)) if *uri == track.uri);
    let l1 = if known && l1.width > 20 {
        let heart = Rect { x: l1.right() - 3, width: 3, ..l1 };
        f.render_widget(Line::from(heart_button(liked, app.like_burst)), heart);
        draw_like_burst(f, app.like_burst, heart, l1);
        app.hits.like = heart;
        Rect { width: l1.width - 6, ..l1 }
    } else {
        l1
    };
    let who = format!("  {}  {}", theme::c().bullet, track.artist_names());
    let line = Line::from(vec![
        Span::styled(format!("{mark} "), theme::accent()),
        Span::styled(track.name.clone(), theme::bright()),
        Span::styled(who, theme::muted()),
    ]);
    // In the stage layout the big column has the clickable artist already.
    if app.hits.np_artist.width == 0 {
        app.hits.np_artist = span_rect(l1, l1.x + 2 + track.name.width() as u16 + 5, track.artist_names().width() as u16);
    }
    f.render_widget(fit_line(line, l1.width as usize), l1);
    // Line 2: ◀◀ ‖ ▶▶, then the progress bar, then the volume.
    let play = if pb.is_playing { "‖" } else { "▶" };
    app.hits.prev = span_rect(l2, l2.x, 2);
    app.hits.play = span_rect(l2, l2.x + 3, 2);
    app.hits.next = span_rect(l2, l2.x + 5, 3);
    let controls = Line::from(vec![
        Span::styled("◀◀ ", theme::muted()),
        Span::styled(play, theme::accent()),
        Span::styled("  ▶▶", theme::muted()),
    ]);
    f.render_widget(controls, Rect { width: 9.min(l2.width), ..l2 });
    let vol_w = if l2.width >= 60 { 14 } else { 0 };
    let bar_area = Rect { x: l2.x + 10, width: l2.width.saturating_sub(10 + vol_w), ..l2 };
    let (line, bar) = progress_line(app.progress_ms(), track.duration_ms, pb.is_playing, bar_area.width);
    app.hits.progress = Rect { x: bar_area.x + bar.0, width: bar.1, ..bar_area };
    app.hits.time = Rect { x: bar_area.x + bar.0 + bar.1, width: bar_area.width.saturating_sub(bar.0 + bar.1), ..bar_area };
    f.render_widget(line, bar_area);
    if let (Some(volume), true) = (app.volume(), vol_w > 0) {
        let (line, vbar) = volume_line(volume, app.muted(), vol_w - 7, false);
        let rect = Rect { x: l2.right() - vol_w + 1, width: vol_w - 1, ..l2 };
        app.hits.volume = Rect { x: rect.x + vbar.0, width: vbar.1, ..rect };
        f.render_widget(line, rect);
    }
}

/// `1:05 ━━━━━━━━──────────── 5:30`, filling the given width.
/// Also returns where the bar is: (offset from the left, width).
pub fn progress_line(progress: u64, duration: u64, playing: bool, width: u16) -> (Line<'static>, (u16, u16)) {
    let left = format!("{} ", format_ms(progress));
    // A click on the time shows the time left instead (and back).
    let right = if REMAINING.load(std::sync::atomic::Ordering::Relaxed) {
        format!(" -{}", format_ms(duration.saturating_sub(progress)))
    } else {
        format!(" {}", format_ms(duration))
    };
    let bar = (width as usize).saturating_sub(left.width() + right.width());
    let ratio = if duration > 0 { (progress as f64 / duration as f64).clamp(0.0, 1.0) } else { 0.0 };
    let filled = (ratio * bar as f64).round() as usize;
    let t = theme::c();
    let mut spans = vec![Span::styled(left.clone(), theme::muted())];
    let span = bar.saturating_sub(1).max(1) as f32;
    // The wave moves along while playing (motion on): its gradient shifts each step.
    let shift = if playing && theme::motion() { anim(150) } else { 0 };
    let (fill, knob) = match t.progress {
        theme::Progress::Line => (t.bar.0, false),
        theme::Progress::Knob => (t.bar.0, filled > 0),
        theme::Progress::Blocks => (t.bar.0, false),
        theme::Progress::Wave => ('∿', false),
    };
    let body = if knob { filled - 1 } else { filled };
    for i in 0..body {
        let f = match t.progress {
            // Blocks: the gradient in 8 steps, like segments.
            theme::Progress::Blocks => ((i as f32 / span) * 8.0).floor() / 8.0,
            theme::Progress::Wave => (((i + shift) % 24) as f32 / 23.0 * std::f32::consts::PI).sin(),
            _ => i as f32 / span,
        };
        let colour = if playing { theme::blend(t.accent, t.accent2, f) } else { t.muted };
        spans.push(Span::styled(fill.to_string(), Style::new().fg(colour)));
    }
    if knob {
        let colour = if playing { t.bright } else { t.muted };
        spans.push(Span::styled("●", Style::new().fg(colour).add_modifier(Modifier::BOLD)));
    }
    spans.push(Span::styled(t.bar.1.to_string().repeat(bar - filled), Style::new().fg(theme::dim())));
    spans.push(Span::styled(right, theme::muted()));
    let line = Line::from(spans);
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
    let t = theme::c();
    let mut spans = vec![Span::styled(label, theme::faint())];
    match t.volume {
        theme::Volume::Bar => {
            spans.push(Span::styled(t.bar.0.to_string().repeat(filled), if muted { theme::faint() } else { theme::text() }));
            spans.push(Span::styled(t.bar.1.to_string().repeat(bar as usize - filled), Style::new().fg(theme::dim())));
        }
        theme::Volume::Leds => {
            // A VU meter: green, then the accent, then red at the top.
            for i in 0..bar as usize {
                let f = i as f32 / bar.max(1) as f32;
                let on = i < filled && !muted;
                let colour = if !on {
                    theme::dim()
                } else if f < 0.6 {
                    t.green
                } else if f < 0.85 {
                    t.accent
                } else {
                    t.red
                };
                spans.push(Span::styled(if on { "▮" } else { "▯" }, Style::new().fg(colour)));
            }
        }
    }
    spans.push(Span::styled(value, if muted { theme::accent() } else { theme::muted() }));
    let line = Line::from(spans);
    (line, (label.width() as u16, bar))
}

fn hints(app: &App) -> Vec<(String, &'static str)> {
    let fixed = |list: &[(&str, &'static str)]| list.iter().map(|(k, w)| ((*k).to_owned(), *w)).collect();
    if app.typing {
        return fixed(&[("⏎", "search"), ("esc", "done"), ("↑", "earlier"), ("← →", "move"), ("^⌫", "word"), ("^u", "clear")]);
    }
    if app.filtering {
        return fixed(&[("type", "filter"), ("⏎", "keep it"), ("esc", "clear it"), ("^⌫", "word")]);
    }
    if !app.marks.is_empty() {
        let k = &app.keys;
        let mut out = vec![(format!("{} marked", app.marks.len()), "·")];
        for (act, word) in [(Act::Queue, "queue"), (Act::AddToPlaylist, "add to…"), (Act::Like, "like"), (Act::Remove, "remove"), (Act::MarkAll, "all / none")] {
            out.push((k.show(act), word));
        }
        out.push(("esc".into(), "clear"));
        return out;
    }
    if app.undo.is_some() {
        return fixed(&[("↑↓", "move"), ("⏎", "go there"), ("click", "go there"), ("esc", "close")]);
    }
    if app.menu.is_some() || app.picker.is_some() || app.confirm.is_some() || app.palette.is_some() {
        return fixed(&[("↑↓", "move"), ("⏎", "choose"), ("esc", "close")]);
    }
    let k = &app.keys;
    let acts = |list: &[(Act, &'static str)]| -> Vec<(String, &'static str)> {
        list.iter().map(|(a, w)| (k.show(*a), *w)).filter(|(key, _)| !key.is_empty()).collect()
    };
    if app.sidebar.open && app.sidebar.focus {
        return match app.sidebar.view {
            crate::sidebar::SideView::Playing => {
                let mut out = fixed(&[("⏎", "run"), ("f e o", "keys"), ("l", "library")]);
                out.extend(acts(&[(Act::SideEffect, "effect")]));
                out.extend(fixed(&[("esc", "list")]));
                out.extend(acts(&[(Act::Sidebar, "close"), (Act::Help, "help")]));
                out
            }
            crate::sidebar::SideView::Library => {
                let mut out = fixed(&[("⏎", "open")]);
                out.extend(acts(&[(Act::Pin, "pin")]));
                out.extend(fixed(&[("h", "playing"), ("esc", "list")]));
                out.extend(acts(&[(Act::Sidebar, "close"), (Act::Help, "help")]));
                out
            }
        };
    }
    acts(match app.view() {
        View::Search if app.search_kind != SearchKind::Songs => &[(Act::Search, "search"), (Act::Play, "open"), (Act::SearchType, "type"), (Act::Palette, "anything"), (Act::Help, "help")],
        View::Search => &[(Act::Search, "search"), (Act::Play, "play"), (Act::SearchType, "type"), (Act::Actions, "actions"), (Act::Radio, "radio"), (Act::Palette, "anything"), (Act::Help, "help")],
        View::Playlists => &[(Act::Play, "open"), (Act::PlayAll, "play all"), (Act::Filter, "filter"), (Act::Back, "up"), (Act::Palette, "anything"), (Act::Help, "help")],
        View::Playlist => &[(Act::Play, "play"), (Act::Actions, "actions"), (Act::Filter, "filter"), (Act::Sort, "sort"), (Act::Mark, "mark"), (Act::AddToPlaylist, "add to…"), (Act::Back, "back"), (Act::Help, "help")],
        View::Liked => &[(Act::Play, "play"), (Act::Actions, "actions"), (Act::Filter, "filter"), (Act::Sort, "sort"), (Act::Mark, "mark"), (Act::Like, "unlike"), (Act::Help, "help")],
        View::Recent => &[(Act::Play, "play"), (Act::Actions, "actions"), (Act::Like, "like"), (Act::Radio, "radio"), (Act::Reload, "reload"), (Act::Help, "help")],
        View::Artist => &[(Act::Play, "play / open"), (Act::NextSection, "section"), (Act::CloseTab, "close"), (Act::Back, "back"), (Act::Help, "help")],
        View::Album => &[(Act::Play, "play"), (Act::PlayAll, "play album"), (Act::Actions, "actions"), (Act::Mark, "mark"), (Act::CloseTab, "close"), (Act::Back, "back"), (Act::Help, "help")],
        View::Radio => &[(Act::Play, "play"), (Act::Discover, "discover"), (Act::Filters, "filters"), (Act::Filter, "filter"), (Act::Randomize, "randomize"), (Act::CloseTab, "close"), (Act::Help, "help")],
        View::Random => &[(Act::Play, "play"), (Act::Discover, "roll again"), (Act::Save, "save as playlist"), (Act::Filter, "filter"), (Act::CloseTab, "close"), (Act::Help, "help")],
    })
}

/// Key hints on the left; a message (✓ or ✕) as a pill on the right. Hints that do not fit
/// beside the message are left out from the end.
fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let t = theme::c();
    let toast = app.toast.as_ref().map(|toast| {
        let (mark, fg) = if toast.error { ("✕", t.red) } else { ("✓", t.green) };
        let room = (area.width as usize).saturating_sub(6);
        Line::from(vec![
            Span::styled(format!(" {mark} "), Style::new().fg(fg).bg(t.selected).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} ", fit(&toast.text, room)), Style::new().fg(if toast.error { t.red } else { t.text }).bg(t.selected)),
        ])
    });
    let toast_w = toast.as_ref().map_or(0, |l| l.width() + 2);
    let room = (area.width as usize).saturating_sub(toast_w);
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, what)) in hints(app).iter().enumerate() {
        let mut add = Vec::new();
        if i > 0 {
            add.push(Span::styled(format!("  {}  ", t.bullet), Style::new().fg(theme::dim())));
        }
        add.push(Span::styled(key.clone(), theme::muted()));
        add.push(Span::styled(format!(" {what}"), theme::faint()));
        let w: usize = spans.iter().chain(add.iter()).map(|s| s.width()).sum();
        if w > room {
            break;
        }
        spans.extend(add);
    }
    f.render_widget(Line::from(spans), area);
    if let Some(line) = toast {
        let w = (line.width() as u16).min(area.width);
        f.render_widget(line, Rect { x: area.right() - w, width: w, ..area });
    }
}

/// Help: every action of the key table with the keys in use, in sections, in two columns
/// when there is room. Typing filters it; ↑ ↓ scroll it.
fn draw_help(f: &mut Frame, app: &App) {
    let q = app.help_query.to_lowercase();
    let shown = |info: &crate::keys::Info| {
        q.is_empty()
            || info.help.to_lowercase().contains(&q)
            || info.name.contains(&q)
            || app.keys.show_all(info.act).to_lowercase().split(' ').any(|k| k == q)
    };
    let mut sections: Vec<Vec<Line>> = Vec::new();
    for section in crate::keys::SECTIONS {
        let rows: Vec<Line> = crate::keys::ALL
            .iter()
            .filter(|i| i.section == section && shown(i))
            .map(|i| {
                let keys = app.keys.show_all(i.act);
                let keys = if keys.is_empty() { "—".to_owned() } else { keys };
                Line::from(vec![Span::styled(format!("{keys:>13}"), theme::accent()), Span::styled(format!("   {}", i.help), theme::text())])
            })
            .collect();
        if !rows.is_empty() {
            let mut lines = vec![Line::from(Span::styled(section, theme::bright()))];
            lines.extend(rows);
            lines.push(Line::raw(""));
            sections.push(lines);
        }
    }
    let total: usize = sections.iter().map(Vec::len).sum();
    let wide = f.area().width >= 100;
    // Two columns: the sections split where the halves are closest in length.
    let (left, right): (Vec<Line>, Vec<Line>) = if wide {
        let mut best = (usize::MAX, 0);
        let mut run = 0;
        for (i, sec) in sections.iter().enumerate() {
            run += sec.len();
            let diff = run.abs_diff(total - run);
            if diff < best.0 {
                best = (diff, i + 1);
            }
        }
        let right = sections.split_off(best.1.min(sections.len()));
        (sections.into_iter().flatten().collect(), right.into_iter().flatten().collect())
    } else {
        (sections.into_iter().flatten().collect(), Vec::new())
    };
    let rows = left.len().max(right.len()).max(1) as u16;
    let area = popup_area(f, if wide { 108 } else { 60 }, rows + 6);
    let title = if app.help_query.is_empty() { " type to filter · ↑↓ scroll · esc close ".to_owned() } else { format!(" filter: {} · esc clears ", app.help_query) };
    let block = popup_block("keys".into())
        .title_bottom(Line::from(Span::styled(title, theme::faint())).right_aligned())
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    if left.is_empty() {
        draw_centered(f, inner, "No key matches.\nesc clears the filter");
        return;
    }
    let max_scroll = (rows as usize).saturating_sub(inner.height as usize);
    let scroll = app.help_scroll.min(max_scroll) as u16;
    let cols: Vec<Rect> = if wide {
        Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).spacing(2).split(inner).to_vec()
    } else {
        vec![inner]
    };
    for (lines, area) in [left, right].into_iter().zip(cols) {
        f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
    }
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
    fn fit_line_keeps_styles_and_cuts() {
        let line = Line::from(vec![Span::styled("Song radio  ", theme::muted()), Span::styled("A very long title", theme::bright())]);
        let cut = fit_line(line, 20);
        assert_eq!(cut.width(), 20);
        assert_eq!(cut.spans[0].style, theme::muted());
        assert!(cut.spans[1].content.ends_with('…'));
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
    fn sidebar_shows_the_song_then_the_library() {
        use crate::sidebar::SideView;
        let (mut app, _rx) = test_app();
        app.typing = false;
        let t = track("Teardrop", "Massive Attack", "Mezzanine");
        app.on_msg(playing(&t));
        app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('<')));
        assert!(app.sidebar.open && app.sidebar.focus);
        // Mid-slide, then settled: both draw.
        let _ = screen(&mut app, 120, 40);
        while app.sidebar.animating() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _ = screen(&mut app, 120, 40);
        }
        app.sidebar.step();
        let out = screen(&mut app, 120, 40);
        for want in ["Playing", "Library", "Teardrop", "Massive Attack", "Share card", "Add to playlist"] {
            assert!(out.contains(want), "{want}: {out}");
        }
        app.sidebar.show(SideView::Library);
        let _ = screen(&mut app, 120, 40);
        while app.sidebar.animating() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _ = screen(&mut app, 120, 40);
        }
        app.sidebar.step();
        let out = screen(&mut app, 120, 40);
        assert!(out.contains("Liked Songs") && out.contains("PLAYLISTS") || out.contains("Playlists"), "{out}");
        // Too narrow: no sidebar, and no panic.
        let out = screen(&mut app, 70, 20);
        assert!(!out.contains("Liked Songs"), "{out}");
    }

    #[test]
    fn player_drops_the_song_while_the_sidebar_shows_it() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind, KeyModifiers};
        let settle = |app: &mut App| {
            while app.sidebar.animating() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                let _ = screen(app, 120, 40);
            }
            app.sidebar.step();
            screen(app, 120, 40)
        };
        let (mut app, _rx) = test_app();
        app.typing = false;
        app.on_msg(playing(&track("Teardrop", "Massive Attack", "Mezzanine")));
        // Hidden: the tab at the left edge, and the full player with the artist.
        let out = settle(&mut app);
        assert!(out.lines().any(|l| l.starts_with('›')), "{out}");
        assert_eq!(out.matches("Massive Attack").count(), 1, "{out}");
        // Click the tab: the sidebar opens on Playing; the player keeps one line.
        let tab = app.hits.side_open;
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: tab.x, row: tab.y + 1, modifiers: KeyModifiers::NONE });
        assert!(app.sidebar.open);
        let out = settle(&mut app);
        assert_eq!(out.matches("Massive Attack").count(), 1, "only the sidebar names the artist: {out}");
        assert!(out.contains('⇄') && out.contains('↻'), "{out}");
        // Library: the player has the song again.
        app.on_key(KeyEvent::from(KeyCode::Char('>')));
        let out = settle(&mut app);
        assert_eq!(out.matches("Massive Attack").count(), 1, "{out}");
        // The hide button shuts it.
        let hide = app.hits.side_hide;
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: hide.x + 1, row: hide.y, modifiers: KeyModifiers::NONE });
        assert!(!app.sidebar.open);
    }

    #[test]
    fn undo_tree_draws_and_clicks() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (mut app, _rx) = test_app();
        app.typing = false;
        app.on_key(KeyEvent::from(KeyCode::Char('3')));
        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        // Back to Liked, then Recent: a second branch from Liked.
        app.on_key(KeyEvent::from(KeyCode::Backspace));
        app.on_key(KeyEvent::from(KeyCode::Char('4')));
        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        let out = screen(&mut app, 120, 40);
        assert!(out.contains("Where you went") && out.contains("you are here"), "{out}");
        // Drawn like `git log --graph`: the Playlists branch in its own lane, joining at Liked.
        assert!(out.contains("●    ◷ Recent") && out.contains("│ ○  ≡ Playlists") && out.contains("○─╯  ♥ Liked"), "{out}");
        // Click the Liked row: go there, the tree closes.
        let liked = app.hits.popup_rows.iter().find(|(_, i)| app.nav.nodes[app.nav.rows()[*i].node].label.contains("Liked")).map(|(r, _)| *r).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: liked.x + 4, row: liked.y, modifiers: KeyModifiers::NONE });
        assert!(app.undo.is_none());
        assert_eq!(app.loc(), crate::app::Loc::Base(crate::app::Tab::Liked));
    }

    #[test]
    fn tabs_that_do_not_fit_go_in_a_hover_list() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let click = |x: u16, y: u16| MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE };
        let (mut app, _rx) = test_app();
        app.typing = false;
        let names = ["Radiohead", "Portishead", "Massive Attack", "Boards of Canada", "Aphex Twin", "Burial", "Bonobo", "Tycho"];
        for (i, n) in names.iter().enumerate() {
            app.open_artist(format!("a{i}"), n.to_string());
        }
        // The last one is shown: at 90 columns it keeps its name, and a chip holds the rest.
        let out = screen(&mut app, 90, 30);
        let top = out.lines().next().unwrap().to_owned();
        assert!(top.contains("Tycho"), "{top}");
        assert!(top.contains('▾'), "{top}");
        assert!(app.tabs_list.is_none());
        // Hover the chip: the list opens with every page.
        let chip = app.hits.tabs_more;
        assert!(app.on_hover(ratatui::layout::Position::new(chip.x + 1, chip.y)));
        let out = screen(&mut app, 90, 30);
        for n in names {
            assert!(out.contains(n), "{n}: {out}");
        }
        // Leave it: it closes. Hover again, click Radiohead: it shows.
        assert!(app.on_hover(ratatui::layout::Position::new(5, 25)));
        assert!(app.tabs_list.is_none());
        app.on_hover(ratatui::layout::Position::new(chip.x + 1, chip.y));
        let _ = screen(&mut app, 90, 30);
        let row = app.hits.tabs_rows[0].0;
        app.on_mouse(click(row.x + 3, row.y));
        assert!(matches!(app.page(), Some(crate::pages::Page::Artist(a)) if a.name == "Radiohead"));
        // Click the chip (kept open), then × on Bonobo: it closes, the list stays.
        let _ = screen(&mut app, 90, 30);
        let chip = app.hits.tabs_more;
        app.on_mouse(click(chip.x + 1, chip.y));
        let _ = screen(&mut app, 90, 30);
        let (x_rect, _) = *app.hits.tabs_close.iter().find(|(_, i)| *i == 6).unwrap();
        app.on_mouse(click(x_rect.x + 1, x_rect.y));
        assert_eq!(app.pages.len(), 7);
        assert!(app.tabs_list.is_some());
        assert!(matches!(app.page(), Some(crate::pages::Page::Artist(a)) if a.name == "Radiohead"), "the page shown stays");
    }

    #[test]
    fn every_theme_renders_at_every_size() {
        for (name, _) in crate::theme::BUILTIN {
            crate::theme::apply(name).unwrap();
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
            app.up_next = Some(track("Inertia Creeps", "Massive Attack", "Mezzanine"));
            for (w, h) in [(160, 45), (100, 30), (60, 20)] {
                let out = screen(&mut app, w, h);
                assert!(out.contains("spotlite"), "{name} {w}x{h}: {out}");
                assert!(out.contains("Angel"), "{name} {w}x{h}: the list shows: {out}");
                assert!(out.contains("Teardrop"), "{name} {w}x{h}: the player shows: {out}");
            }
            // Stage themes put the song in the big column on a wide screen.
            if crate::theme::c().layout == crate::theme::Layout::Stage {
                let out = screen(&mut app, 160, 45);
                assert!(out.to_lowercase().contains("now playing") || out.contains("N O W"), "{name}: {out}");
                assert!(out.contains("Inertia Creeps"), "{name}: up next on stage: {out}");
            }
            app.help = true;
            let out = screen(&mut app, 100, 34);
            assert!(out.contains("move down") && out.contains("all actions"), "{name}: help: {out}");
            app.help = false;
        }
        crate::theme::apply("amber").unwrap();
    }

    fn click(app: &mut App, x: u16, y: u16, button: ratatui::crossterm::event::MouseButton) {
        use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(button), column: x, row: y, modifiers: KeyModifiers::NONE });
    }

    /// Text on screen under a hit rect.
    fn under(out: &str, r: Rect) -> String {
        let line = out.lines().nth(r.y as usize).unwrap_or("");
        line.chars().skip(r.x as usize).take(r.width as usize).collect()
    }

    #[test]
    fn mouse_targets_match_what_is_drawn_in_every_theme() {
        use ratatui::crossterm::event::MouseButton;
        for (name, _) in crate::theme::BUILTIN {
            crate::theme::apply(name).unwrap();
            let (mut app, _rx) = test_app();
            app.typing = false;
            app.searched = "x".into();
            let songs: Vec<Track> = (0..12).map(|i| track(&format!("Song {i}"), "A", "B")).collect();
            app.on_msg(Msg::Tracks { list: List::Search("x".into()), offset: 0, page: Page { items: songs.clone(), raw: 12, total: 12 } });
            app.on_msg(playing(&songs[0]));
            app.on_msg(Msg::Saved { uri: songs[0].uri.clone(), saved: false });
            let out = screen(&mut app, 120, 34);
            // Result kinds in the list title: the click lands on the word.
            for (r, kind) in app.hits.search_kinds.clone() {
                let label = crate::theme::title(kind.title());
                assert_eq!(under(&out, r), label, "{name}: search kind hit");
            }
            // Tabs: the number and the name are under the hit.
            let (r, _) = app.hits.tabs[2].clone();
            assert!(under(&out, r).to_lowercase().contains("liked"), "{name}: tab hit is “{}”", under(&out, r));
            // A click on a song's row selects that song.
            let list = app.hits.list;
            click(&mut app, list.x + 10, list.y + 4, MouseButton::Left);
            assert_eq!(app.search.selected(), 4, "{name}: row click");
            let out = screen(&mut app, 120, 34);
            let row = out.lines().nth((list.y + 4) as usize).unwrap();
            assert!(row.contains("Song 4"), "{name}: the clicked row shows Song 4: {row}");
            // The heart button is 3 cells and a click on any of them likes the song.
            let like = app.hits.like;
            assert_eq!((like.width, under(&out, like).trim()), (3, crate::theme::heart(false)), "{name}: heart");
            click(&mut app, like.x + 2, like.y, MouseButton::Left);
            assert!(matches!(&app.saved, Some((_, true))), "{name}: liked at once");
            assert!(app.like_burst.is_some_and(|(_, liked)| liked), "{name}: the animation starts");
        }
        crate::theme::apply("amber").unwrap();
    }

    #[test]
    fn the_visualizer_never_changes_the_list_size() {
        let (mut app, _rx) = test_app();
        app.typing = false;
        app.searched = "x".into();
        let songs: Vec<Track> = (0..60).map(|i| track(&format!("Song {i}"), "A", "B")).collect();
        app.on_msg(Msg::Tracks { list: List::Search("x".into()), offset: 0, page: Page { items: songs.clone(), raw: 60, total: 60 } });
        app.on_msg(playing(&songs[0]));
        screen(&mut app, 120, 40);
        let off = app.hits.list;
        app.viz_on = true;
        screen(&mut app, 120, 40);
        assert_eq!(app.hits.list, off, "it is drawn under the list, not beside it");
    }

    #[test]
    fn right_click_menu_opens_at_the_mouse() {
        use ratatui::crossterm::event::MouseButton;
        let (mut app, _rx) = test_app();
        app.typing = false;
        app.searched = "x".into();
        let songs: Vec<Track> = (0..60).map(|i| track(&format!("Song {i}"), "A", "B")).collect();
        app.on_msg(Msg::Tracks { list: List::Search("x".into()), offset: 0, page: Page { items: songs, raw: 60, total: 60 } });
        screen(&mut app, 120, 40);
        let list = app.hits.list;
        click(&mut app, 30, list.y + 2, MouseButton::Right);
        assert_eq!(app.search.selected(), 2);
        assert_eq!(app.menu.as_ref().unwrap().title, "Song 2");
        screen(&mut app, 120, 40);
        assert_eq!((app.hits.popup.x, app.hits.popup.y), (30, list.y + 2), "top-left corner at the mouse");
        // Near the edges it moves left, and up above the mouse, to stay on screen.
        let screen = Rect::new(0, 0, 120, 34);
        assert_eq!(popup_at(screen, Position::new(10, 5), 30, 9), Rect::new(10, 5, 30, 9));
        assert_eq!(popup_at(screen, Position::new(110, 5), 30, 9), Rect::new(90, 5, 30, 9), "moved left");
        assert_eq!(popup_at(screen, Position::new(10, 30), 30, 9), Rect::new(10, 22, 30, 9), "above: its last row is the mouse row");
        assert_eq!(popup_at(screen, Position::new(10, 3), 30, 40), Rect::new(10, 0, 30, 34), "too tall: the whole screen");
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
        assert!(out.contains("Songs · Artists · Albums · Playlists  for “teardrop”"), "{out}");
        assert!(out.contains("Title") && out.contains("Album"), "{out}");
        assert!(
            out.lines().any(|l| ['▁', '▃', '▅', '▇'].iter().any(|m| l.contains(&format!("{m}      Teardrop")))),
            "playing row is marked, with a 2-cell thumbnail before the title: {out}"
        );
        assert!(out.contains(&format!("Teardrop   {} ", crate::theme::heart(true))), "the heart is a 3-cell button: {out}");
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
        assert_eq!(at(h.like), format!(" {} ", crate::theme::heart(true)), "the whole button is clickable");
        assert_eq!(at(h.shuffle), "⇄");
        assert_eq!(at(h.volume).chars().count(), VOLUME_BAR as usize);
        assert!(at(h.progress).starts_with('━'));
        assert_eq!(at(h.tabs[1].0), "2 Playlists");
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
        assert!(!narrow.lines().any(|l| l.contains("Title") && l.contains("Album")) && narrow.contains("Teardrop"), "{narrow}");
    }

    #[test]
    fn visualizer_draws_smooth_bars_in_empty_cells_only() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        buf.set_string(0, 9, "TEXT", Style::new());
        buf[(5, 9)].set_bg(theme::c().selected);
        buf[(6, 9)].skip = true;
        let frame = viz::Frame { levels: [0.5; viz::BANDS] };
        let area = buf.area;
        draw_visualizer(&mut buf, area, &frame);
        // Under the text: 0.5 * VIZ_MAX * 10 rows = 4 full cells, then a partial top.
        let height = 0.5 * VIZ_MAX * 10.0;
        let full = height.floor() as u16;
        assert_eq!(buf[(10, 9)].symbol(), "█", "it fills the empty cells of a row with text");
        assert_eq!(buf[(10, 10 - full)].symbol(), "█");
        let part = ((height - full as f32) * 8.0).round() as usize;
        assert_eq!(buf[(10, 9 - full)].symbol(), EIGHTHS[part]);
        assert!((0..10).any(|y| buf[(10, y)].fg == theme::c().viz_line), "the line is drawn over the bars");
        assert_eq!(buf[(0, 9)].symbol(), "T", "text is kept");
        assert_eq!(buf[(0, 9)].bg, Color::Reset, "and not changed");
        assert_eq!(buf[(5, 9)].bg, theme::c().selected, "selection untouched");
        assert_eq!(buf[(6, 9)].symbol(), " ", "skipped (picture) cell untouched");

        let mut quiet = Buffer::empty(Rect::new(0, 0, 20, 10));
        let area = quiet.area;
        draw_visualizer(&mut quiet, area, &viz::Frame::default());
        assert_eq!(quiet, Buffer::empty(Rect::new(0, 0, 20, 10)), "silence draws nothing");
    }

    #[test]
    fn main_visualizer_sweeps_in_from_the_left_with_nothing_stray() {
        let frame = viz::Frame { levels: [0.5; viz::BANDS] };
        let area = Rect::new(0, 0, 40, 20);
        let mut half = Buffer::empty(area);
        draw_visualizer_fx(&mut half, area, &frame, 0.3);
        assert_eq!(half[(2, 19)].symbol(), "█", "the left side is in");
        assert_eq!(half[(39, 19)].symbol(), " ", "the right side not yet");
        for style in [theme::Viz::Wave, theme::Viz::Bars, theme::Viz::Mirror] {
            theme::set("x", theme::Theme { viz: style, ..theme::c() });
            let mut all = Buffer::empty(area);
            draw_visualizer_fx(&mut all, area, &frame, 1.0);
            // Over the bars and their line: nothing (no caps, no sparks).
            for y in 0..6 {
                for x in 0..40 {
                    assert_eq!(all[(x, y)].symbol(), " ", "{style:?}: stray cell at {x},{y}");
                }
            }
        }
        let mut off = Buffer::empty(area);
        draw_visualizer_fx(&mut off, area, &frame, 0.0);
        assert_eq!(off, Buffer::empty(area), "swept out: nothing");
    }

    #[test]
    fn side_visualizer_grows_from_the_left_in_empty_cells_only() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 12));
        buf.set_string(0, 11, "AB", Style::new());
        buf[(3, 11)].skip = true;
        let frame = viz::Frame { levels: [0.5; viz::BANDS] };
        let mut peaks = viz::Peaks::default();
        peaks.update(&viz::Frame { levels: [0.9; viz::BANDS] });
        let area = buf.area;
        draw_side_visualizer(&mut buf, area, &frame, &peaks, 1.0);
        assert_eq!(buf[(0, 11)].symbol(), "A", "text is kept");
        assert_eq!(buf[(3, 11)].symbol(), " ", "skipped (picture) cell untouched");
        let len = 0.5 * SIDE_MAX * 30.0;
        for y in 0..12 {
            assert_ne!(buf[(4, y)].symbol(), " ", "row {y} has a bar");
            assert_ne!(buf[(len as u16 + 1, y)].symbol(), "█", "the bar stops at its level");
        }
        let cap = (0.9 * SIDE_MAX * 30.0_f32).round() as u16;
        assert!((0..12).any(|y| buf[(cap, y)].symbol() == "▏"), "the peak cap hangs over the bars");

        // Half revealed: the bottom rows are in, the top ones not yet.
        let mut half = Buffer::empty(Rect::new(0, 0, 30, 12));
        let area = half.area;
        draw_side_visualizer(&mut half, area, &frame, &peaks, 0.3);
        assert_ne!(half[(1, 11)].symbol(), " ");
        assert_eq!(half[(1, 0)].symbol(), " ");
        let mut off = Buffer::empty(Rect::new(0, 0, 30, 12));
        draw_side_visualizer(&mut off, area, &frame, &peaks, 0.0);
        assert_eq!(off, Buffer::empty(area), "off draws nothing");
    }

    #[test]
    fn capital_v_picks_the_sidebar_effect_with_a_preview() {
        use crate::sidebar::SideFx;
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let (mut app, _rx) = test_app();
        app.typing = false;
        assert_eq!(app.side_fx, SideFx::Off, "off by default");
        app.on_key(KeyEvent::from(KeyCode::Char('V')));
        assert!(app.sidebar.open, "it opens the sidebar on the song");
        assert_eq!(app.menu.as_ref().unwrap().title, "Sidebar effect");
        app.on_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(app.side_fx, SideFx::Liquid, "moving previews");
        assert!(app.side_fx_shown().1 < 0.5, "it fills in");
        let _ = screen(&mut app, 120, 40);
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.side_fx, SideFx::Off, "esc puts the old one back");
        app.on_key(KeyEvent::from(KeyCode::Char('V')));
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.side_fx, SideFx::Spectrum);
        assert!(app.menu.is_none());
        assert_eq!(app.saved_state().side_fx, SideFx::Spectrum, "it is kept");
        assert!(!app.viz_on, "the main visualizer is apart");
    }

    #[test]
    fn text_over_the_liquid_is_made_readable() {
        let dark = Color::Rgb(0x30, 0x24, 0x10);
        // Dim text on a dark liquid becomes light; text that reads well stays.
        let fixed = readable(Color::Rgb(0x5f, 0x5e, 0x59), dark);
        assert_ne!(fixed, Color::Rgb(0x5f, 0x5e, 0x59));
        assert_eq!(readable(Color::Rgb(0xec, 0xeb, 0xe6), dark), Color::Rgb(0xec, 0xeb, 0xe6));
        // On a light liquid, text goes dark.
        let Color::Rgb(r, _, _) = readable(Color::Rgb(0xd0, 0xd0, 0xd0), Color::Rgb(0xe0, 0xc0, 0x80)) else { panic!() };
        assert!(r < 0x80);
    }

    #[test]
    fn every_liquid_draws_its_drops_and_keeps_text_and_pictures() {
        for (name, kind) in theme::Liquid::ALL {
            let mut liquid = crate::liquid::Liquid::default();
            // Loud, so the drops are big.
            liquid.advance(&viz::Frame { levels: [0.9; viz::BANDS] }, 30, 20, kind, 0.5);
            let area = Rect::new(0, 0, 30, 20);
            let d = liquid.drops[20];
            let (mx, my) = (d.x as u16, (d.y / 2.0) as u16);
            let mut buf = Buffer::empty(area);
            buf.set_string(mx, my, "S", Style::new());
            let e = liquid.drops[26];
            let (px, py) = (e.x as u16, (e.y / 2.0) as u16);
            buf[(px, py)].skip = true;
            draw_liquid(&mut buf, area, &liquid, kind, 1.0);
            assert_eq!(buf[(mx, my)].symbol(), "S", "{name}: text is kept");
            assert_ne!(buf[(mx, my)].bg, Color::Reset, "{name}: and floats in the liquid");
            assert_eq!(buf[(px, py)].symbol(), " ", "{name}: the picture is left alone");
            let drawn = (0..20).filter(|&y| (0..30).any(|x| buf[(x, y)].symbol() != " ")).count();
            assert!(drawn >= 12, "{name}: drops all over, top to bottom: {drawn} rows");
            let mut empty = Buffer::empty(area);
            draw_liquid(&mut empty, area, &liquid, kind, 0.0);
            assert_eq!(empty, Buffer::empty(area), "{name}: gone, nothing");
        }
    }

    #[test]
    fn renders_empty_states_and_help() {
        let (mut app, _rx) = test_app();
        let out = screen(&mut app, 90, 22);
        assert!(out.contains("Search Spotify"), "{out}");
        assert!(out.contains("/ search songs, artists, albums or playlists"), "{out}");
        assert!(out.contains("Nothing playing"), "{out}");
        assert!(out.contains("connecting"), "{out}");

        app.on_msg(Msg::Device(None));
        let out = screen(&mut app, 90, 22);
        assert!(out.contains("no device") && out.contains("systemctl --user start spotifyd"), "{out}");

        app.help = true;
        let out = screen(&mut app, 90, 32);
        assert!(out.contains("move down") && out.contains("type to filter"), "{out}");
        // Typing filters it: the line far down shows.
        app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('r')));
        app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('a')));
        app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('d')));
        let out = screen(&mut app, 90, 32);
        assert!(out.contains("song radio") && !out.contains("move down"), "{out}");
        app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Esc));
        app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Esc));
        assert!(!app.help, "esc clears the filter, then closes");
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

    #[test]
    fn a_long_list_draws_only_its_window_and_scrolls_like_before() {
        let (mut app, _rx) = test_app();
        app.typing = false;
        app.searched = "x".into();
        let items: Vec<Track> = (0..10_000).map(|i| track(&format!("Song{i:05}"), "A", "B")).collect();
        app.on_msg(Msg::Tracks { list: List::Search("x".into()), offset: 0, page: Page { raw: items.len(), items, total: 10_000 } });
        let out = screen(&mut app, 100, 30);
        assert!(out.contains("Song00000") && !out.contains("Song00100"), "{out}");
        // Far down: the selection is on screen, at the bottom (the scroll moves only as needed).
        app.search.state.select(Some(5000));
        let out = screen(&mut app, 100, 30);
        let offset = app.search.state.offset();
        assert!(out.contains("Song05000") && offset < 5000 && offset > 4970, "{offset}: {out}");
        let last = out.lines().rfind(|l| l.contains("Song0")).unwrap_or_default();
        assert!(last.contains("Song05000"), "{out}");
        // Up a little: no jump.
        app.search.state.select(Some(4998));
        screen(&mut app, 100, 30);
        assert_eq!(app.search.state.offset(), offset);
        // Back to the top.
        app.search.state.select(Some(0));
        let out = screen(&mut app, 100, 30);
        assert_eq!(app.search.state.offset(), 0);
        assert!(out.contains("Song00000"), "{out}");
    }
}
