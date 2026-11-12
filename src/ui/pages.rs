//! Artist, album, radio and random pages.

use super::*;

/// " ‹ Name " for a page title; records the back button.
/// The title of a page's list: "‹ Back · Radio". The name is in the page header above, so it
/// is not repeated here. The whole title is the back button.
pub(super) fn page_title(app: &mut App, area: Rect, kind: &str) -> Line<'static> {
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

/// The top part of a page: a cover, then up to three lines of text.
pub(super) fn draw_page_header(f: &mut Frame, images: &mut Images, area: Rect, cover: &[ImageRef], lines: Vec<Line<'static>>) {
    let rows = area.height;
    let cols = images.square_cols(rows);
    let [art, _, text] =
        Layout::horizontal([Constraint::Length(cols), Constraint::Length(2), Constraint::Min(10)]).areas(area);
    draw_cover(f, images, cover, art);
    let lines: Vec<Line> = lines.into_iter().map(|l| fit_line(l, text.width as usize)).collect();
    let [mid] = Layout::vertical([Constraint::Length(lines.len() as u16)]).flex(Flex::Center).areas(text);
    f.render_widget(Paragraph::new(lines), mid);
}

pub(super) fn draw_artist(f: &mut Frame, app: &mut App, area: Rect) {
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

pub(super) fn draw_album(f: &mut Frame, app: &mut App, area: Rect) {
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
pub(super) fn album_artists(app: &App) -> String {
    let Some(Page::Album(page)) = app.page() else { return String::new() };
    page.tracks
        .items
        .first()
        .map(|t| format!("{} · ", t.artist_names()))
        .unwrap_or_default()
}

pub(super) fn draw_radio(f: &mut Frame, app: &mut App, area: Rect) {
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

pub(super) fn draw_random(f: &mut Frame, app: &mut App, area: Rect) {
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
