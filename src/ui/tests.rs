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
    assert_eq!(app.side_fx, SideFx::Aura, "moving previews");
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

/// A panel for the aura: 36 × 30 cells with a cover at its top, a line of text, and music.
const AURA_AREA: Rect = Rect { x: 0, y: 0, width: 36, height: 30 };
const AURA_COVER: Rect = Rect { x: 4, y: 0, width: 28, height: 14 };

fn aura_music(i: usize) -> viz::Frame {
    let mut f = viz::Frame::default();
    for (b, l) in f.levels.iter_mut().enumerate() {
        *l = 0.3 + 0.45 * (b as f32 * 0.4 + i as f32 * 0.21).sin().abs();
    }
    if i % 15 < 2 {
        f.levels[..6].fill(0.95);
    }
    f
}

/// The aura after `secs` of `sound`, drawn over a cover (a picture) and a line of text.
fn aura_drawn(kind: theme::Aura, colours: Option<[u8; 3]>, secs: f32, sound: impl Fn(usize) -> viz::Frame, reveal: f32) -> Buffer {
    let geo = crate::aura::Geo::new(AURA_AREA, Some(AURA_COVER));
    let mut aura = crate::aura::Aura::default();
    if let Some(c) = colours {
        aura.set_colours(&[c; crate::images::SWATCH * crate::images::SWATCH]);
    }
    let steps = (secs * 30.0) as usize;
    for i in 0..steps {
        aura.advance(&sound(i), geo, kind, 1.0 / 30.0);
    }
    let mut buf = Buffer::empty(AURA_AREA);
    for y in AURA_COVER.top()..AURA_COVER.bottom() {
        for x in AURA_COVER.left()..AURA_COVER.right() {
            buf[(x, y)].skip = true;
        }
    }
    buf.set_string(8, 16, "Teardrop by Massive", Style::new().fg(Color::Rgb(1, 2, 3)));
    draw_aura(&mut buf, AURA_AREA, Some(AURA_COVER), &aura, &sound(steps), kind, reveal);
    buf
}

/// The cells the aura drew, and the light in them (the sum of their colours).
fn aura_light(buf: &Buffer) -> (usize, u32) {
    let lit: Vec<u32> = buf
        .content
        .iter()
        .filter(|c| !c.skip && c.symbol() != " " && c.fg != Color::Rgb(1, 2, 3))
        .map(|c| if let Color::Rgb(r, g, b) = c.fg { r as u32 + g as u32 + b as u32 } else { 0 })
        .collect();
    (lit.len(), lit.iter().sum())
}

#[test]
fn every_aura_draws_round_the_cover_and_keeps_text_and_pictures() {
    for (name, kind) in theme::Aura::ALL {
        let buf = aura_drawn(kind, Some([200, 60, 30]), 4.0, aura_music, 1.0);
        let (cells, _) = aura_light(&buf);
        assert!(cells >= 25, "{name}: it shows: {cells} cells");
        for (i, ch) in "Teardrop by Massive".chars().enumerate() {
            let cell = &buf[(8 + i as u16, 16)];
            if ch != ' ' {
                assert_eq!((cell.symbol(), cell.fg), (ch.to_string().as_str(), Color::Rgb(1, 2, 3)), "{name}: text is kept");
            }
        }
        for y in AURA_COVER.top()..AURA_COVER.bottom() {
            for x in AURA_COVER.left()..AURA_COVER.right() {
                assert_eq!(buf[(x, y)].symbol(), " ", "{name}: the picture is left alone");
            }
        }
        assert!(buf.content.iter().all(|c| c.bg == Color::Reset), "{name}: only glyphs, the background shows through");
        // It stays behind the text: no cell is near full light.
        let brightest = buf.content.iter().filter(|c| c.fg != Color::Rgb(1, 2, 3)).filter_map(|c| if let Color::Rgb(r, g, b) = c.fg { Some(r.max(g).max(b)) } else { None }).max();
        assert!(brightest.is_some_and(|v| v > 40), "{name}: it can be seen: {brightest:?}");
        let gone = aura_drawn(kind, Some([200, 60, 30]), 4.0, aura_music, 0.0);
        assert_eq!(aura_light(&gone).0, 0, "{name}: gone, nothing");
        // Without a cover (a short panel), and in a panel too small: no panic.
        let mut small = Buffer::empty(Rect::new(0, 0, 12, 6));
        let mut aura = crate::aura::Aura::default();
        aura.advance(&aura_music(0), crate::aura::Geo::new(small.area, None), kind, 0.5);
        aura.advance(&aura_music(1), crate::aura::Geo::new(small.area, None), kind, 0.5);
        draw_aura(&mut small, Rect::new(0, 0, 12, 6), None, &aura, &aura_music(1), kind, 1.0);
        draw_aura(&mut small, Rect::new(0, 0, 3, 1), None, &aura, &aura_music(1), kind, 1.0);
        draw_aura(&mut small, Rect::new(4, 2, 40, 40), Some(Rect::new(30, 30, 9, 9)), &aura, &aura_music(1), kind, 0.5);
    }
}

#[test]
fn every_aura_moves_to_the_music() {
    for (name, kind) in theme::Aura::ALL {
        let quiet = aura_light(&aura_drawn(kind, Some([200, 60, 30]), 4.0, |_| viz::Frame::default(), 1.0));
        let loud = aura_light(&aura_drawn(kind, Some([200, 60, 30]), 4.0, aura_music, 1.0));
        assert!(loud.1 > quiet.1 * 3 / 2, "{name}: more light with music: {quiet:?} then {loud:?}");
        assert!(quiet.0 < AURA_AREA.area() as usize / 4, "{name}: calm in silence: {} cells", quiet.0);
    }
}

#[test]
fn every_aura_fills_the_whole_pane() {
    for (name, kind) in theme::Aura::ALL {
        let buf = aura_drawn(kind, Some([200, 60, 30]), 12.0, aura_music, 1.0);
        let drawn = |x: std::ops::Range<u16>, y: std::ops::Range<u16>| y.flat_map(|y| x.clone().map(move |x| (x, y))).filter(|&p| buf[p].symbol() != " ").count();
        // Below the cover and the text: the lower part of the pane, in four strips.
        let strips = (0..4).filter(|i| drawn(i * 9..i * 9 + 9, 18..30) >= 3).count();
        assert!(strips >= 3, "{name}: light across the lower pane: {strips} of 4 strips");
        assert!(drawn(0..36, 26..30) >= 6, "{name}: light at the bottom of the pane: {}", drawn(0..36, 26..30));
    }
}

#[test]
fn the_aura_has_the_colours_of_the_cover() {
    for (name, kind) in theme::Aura::ALL {
        let sum = |buf: &Buffer| {
            buf.content.iter().filter(|c| c.fg != Color::Rgb(1, 2, 3)).fold([0u32; 3], |mut s, c| {
                if let Color::Rgb(r, g, b) = c.fg {
                    s = [s[0] + r as u32, s[1] + g as u32, s[2] + b as u32];
                }
                s
            })
        };
        let red = sum(&aura_drawn(kind, Some([220, 30, 30]), 3.0, aura_music, 1.0));
        let blue = sum(&aura_drawn(kind, Some([30, 30, 220]), 3.0, aura_music, 1.0));
        assert!(red[0] * blue[2] > red[2] * blue[0], "{name}: a red cover gives redder light than a blue one: {red:?} {blue:?}");
        // No cover's colours known: the theme's own.
        assert!(aura_light(&aura_drawn(kind, None, 3.0, aura_music, 1.0)).0 > 0, "{name}");
    }
}

/// Writes pictures' data of the aura in the real sidebar, one file per theme, for
/// `tools/aura-preview.py`: `SPOTLITE_PREVIEW=<dir> cargo test aura_preview -- --ignored`.
#[test]
#[ignore]
fn aura_preview() {
    use crate::sidebar::SideFx;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let Some(dir) = std::env::var_os("SPOTLITE_PREVIEW").map(std::path::PathBuf::from) else { return };
    // A cover: a warm sky over a cold sea.
    let mut swatch = [[0u8; 3]; crate::images::SWATCH * crate::images::SWATCH];
    for (i, px) in swatch.iter_mut().enumerate() {
        let (u, v) = ((i % 8) as f32 / 7.0, (i / 8) as f32 / 7.0);
        let sky = [250.0 - 60.0 * u, 120.0 + 90.0 * u * (1.0 - v), 60.0 + 120.0 * v];
        let sea = [20.0 + 40.0 * u, 110.0 + 80.0 * (1.0 - u), 190.0 - 40.0 * v];
        *px = std::array::from_fn(|c| if v < 0.55 { sky[c] } else { sea[c] } as u8);
    }
    for (name, _) in theme::BUILTIN {
        theme::apply(name).unwrap();
        let (mut app, _rx) = test_app();
        app.typing = false;
        app.on_msg(playing(&track("Teardrop", "Massive Attack", "Mezzanine")));
        app.on_key(KeyEvent::from(KeyCode::Char('<')));
        for shot in ["quiet", "loud"] {
            let mut terminal = Terminal::new(TestBackend::new(120, 44)).unwrap();
            while app.sidebar.animating() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                terminal.draw(|f| draw(f, &mut app)).unwrap();
            }
            app.sidebar.step();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            let mut buf = terminal.backend().buffer().clone();
            let side = app.hits.side;
            let content = match theme::c().frame {
                theme::Frame::Full => Rect::new(side.x + 2, side.y + 4, side.width - 4, side.height - 5),
                theme::Frame::Rule => Rect::new(side.x + 1, side.y + 3, side.width - 3, side.height - 3),
                theme::Frame::None => Rect::new(side.x + 1, side.y + 3, side.width - 2, side.height - 3),
            };
            let cover = app.side_cover;
            let geo = crate::aura::Geo::new(content, cover);
            let kind = theme::c().aura;
            app.aura.set_colours(&swatch);
            let dt = if theme::motion() { 1.0 / 30.0 } else { 0.0 };
            let sound = |i: usize| if shot == "loud" { aura_music(i) } else { viz::Frame { levels: [0.04; viz::BANDS] } };
            for i in 0..150 {
                app.aura.advance(&sound(i), geo, kind, dt);
            }
            assert_eq!(app.side_fx, SideFx::Off);
            draw_aura(&mut buf, content, cover, &app.aura, &sound(150), kind, 1.0);
            let mut out = format!("{} {} {} {} {}\n", side.width + 2, side.height, side.x, side.y, name);
            if let Some(c) = cover {
                out += &format!("cover {} {} {} {}\n", c.x, c.y, c.width, c.height);
            }
            out += &format!("bg {}\n", if let Color::Rgb(r, g, b) = theme::c().viz_low { format!("{r} {g} {b}") } else { "0 0 0".into() });
            for y in side.top()..side.bottom() {
                for x in side.left()..side.right() + 2 {
                    let c = &buf[(x, y)];
                    let col = |c: Color, d: &str| if let Color::Rgb(r, g, b) = c { format!("{r} {g} {b}") } else { d.to_owned() };
                    if c.symbol() != " " || c.bg != Color::Reset {
                        out += &format!("{x} {y} {} {} {}\n", col(c.fg, "200 200 200"), col(c.bg, "-1 -1 -1"), c.symbol());
                    }
                }
            }
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}-{shot}.txt")), out).unwrap();
        }
    }
    theme::apply(theme::DEFAULT).unwrap();
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

#[test]
fn a_loading_list_shows_skeleton_rows_and_popups_dim_the_rest() {
    let (mut app, _rx) = test_app();
    app.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('3')));
    app.liked.loading = true;
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let bars = buf.content.iter().filter(|c| c.bg == theme::blend(theme::c().selected, theme::c().border, 0.25)).count();
    assert!(bars > 100, "skeleton bars: {bars}");
    app.liked.loading = false;
    app.help = true;
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal.draw(|f| draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    assert!(buf[(1, 1)].modifier.contains(Modifier::DIM), "behind help is dim");
}
