//! Song lists, search results and the Playlists tab.

use super::*;

pub(super) fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
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
pub(super) fn draw_found(f: &mut Frame, app: &mut App, area: Rect, title: Line, empty: &str) {
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

pub(super) fn draw_tracks(f: &mut Frame, app: &mut App, area: Rect, title: Line, empty: &str) {
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
        .title(Line::from(Span::styled(songs_label(listing, filtering), theme::faint())).right_aligned());
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
pub(super) enum Entry<'a> {
    Folder { name: &'a str, count: usize },
    Playlist(&'a Playlist),
}

pub(super) fn draw_playlists(f: &mut Frame, app: &mut App, area: Rect) {
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
