//! Pop-ups: menus, pickers, the palette, the undo tree, confirm and help.

use super::*;

pub(super) fn draw_menu(f: &mut Frame, app: &mut App) {
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
pub(super) fn draw_genre_picker(f: &mut Frame, app: &mut App) {
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
pub(super) const SAMPLE_H: u16 = 8;

/// The sample playing (Tab in the genre picker): its cover, song and artist, and the time left,
/// over the bottom of the picker.
pub(super) fn draw_sample(f: &mut Frame, app: &mut App, picker: Rect) {
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

pub(super) fn draw_picker(f: &mut Frame, app: &mut App) {
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
pub(super) fn draw_palette(f: &mut Frame, app: &mut App) {
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
pub(super) fn draw_undo(f: &mut Frame, app: &mut App) {
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

pub(super) fn draw_confirm(f: &mut Frame, app: &mut App) {
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

/// Help: every action of the key table with the keys in use, in sections, in two columns
/// when there is room. Typing filters it; ↑ ↓ scroll it.
pub(super) fn draw_help(f: &mut Frame, app: &App) {
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
