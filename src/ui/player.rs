//! The player: song, cover, controls, progress and volume.

use super::*;

/// The player shows the time left (set from `App::remaining` at each draw).
pub(super) static REMAINING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn draw_now_playing(f: &mut Frame, app: &mut App, area: Rect) {
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
pub(super) fn draw_stage(f: &mut Frame, app: &mut App, area: Rect) {
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
pub(super) fn heart_button(liked: bool, burst: Option<(std::time::Instant, bool)>) -> Vec<Span<'static>> {
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
pub(super) fn draw_like_burst(f: &mut Frame, burst: Option<(std::time::Instant, bool)>, heart: Rect, room: Rect) {
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
pub(super) fn draw_transport(f: &mut Frame, app: &mut App, line: Rect) {
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
pub(super) fn draw_player_compact(f: &mut Frame, app: &mut App, inner: Rect) {
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
