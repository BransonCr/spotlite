//! The tab bar at the top and the key hints and messages at the bottom.

use super::*;

pub(super) fn draw_header(f: &mut Frame, app: &mut App, area: Rect) {
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
pub(super) fn draw_tabs_list(f: &mut Frame, app: &mut App, header: Rect) {
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
pub(super) fn tab_chip(key: Option<usize>, glyph: Option<char>, label: &str, count: Option<usize>, active: bool) -> Vec<Span<'static>> {
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

pub(super) fn hints(app: &App) -> Vec<(String, &'static str)> {
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
pub(super) fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
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
