//! Drawing: one frame of the whole screen from the App. The parts are in the modules below;
//! nothing here changes the App except its click targets (`hits`).

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

mod aura;
mod header;
mod lists;
mod pages;
mod player;
mod popups;
mod effects;
mod widgets;
#[cfg(test)]
mod tests;

use header::*;
use lists::*;
use pages::*;
pub use player::*;
use popups::*;
pub use aura::draw_aura;
pub use effects::*;
pub use widgets::*;

/// Height of the player panel, borders included; the album art is this tall minus 2.
const PLAYER_HEIGHT: u16 = 6;
/// Rows per playlist (the cover thumbnail is this tall).
const PLAYLIST_ROW: u16 = 2;
/// Cells of the volume slider's bar.
const VOLUME_BAR: u16 = 12;

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
    let popup = app.menu.is_some()
        || app.genre_picker.is_some()
        || app.picker.is_some()
        || app.confirm.is_some()
        || app.palette.is_some()
        || app.undo.is_some()
        || app.help;
    if popup {
        // What is behind a pop-up steps back, so the pop-up stands out.
        let area = f.area();
        let buf = f.buffer_mut();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].modifier.insert(Modifier::DIM);
            }
        }
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
    if !theme::truecolor() {
        let buf = f.buffer_mut();
        for cell in buf.content.iter_mut() {
            cell.fg = theme::to_256(cell.fg);
            cell.bg = theme::to_256(cell.bg);
        }
    }
}

