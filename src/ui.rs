use crate::app::{App, Tab};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, LineGauge, List, ListItem, ListState, Paragraph, Tabs};
use ratatui::Frame;

const HELP: &str = "/ search  ⏎ play  ␣ pause  n/p next/prev  +/- vol  s shuffle  tab switch  r reload  q quit";

pub fn format_ms(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn draw(f: &mut Frame, app: &App) {
    let [tabs_area, body, now_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let titles = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{} {}", i + 1, t.title()));
    let current = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    f.render_widget(
        Tabs::new(titles)
            .select(current)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .divider(" "),
        tabs_area,
    );

    match app.tab {
        Tab::Search => draw_search(f, app, body),
        Tab::Playlists => {
            let items = app.playlists.as_ref().map(|pls| {
                pls.iter()
                    .map(|p| {
                        let owner = p.owner.as_ref().and_then(|o| o.display_name.as_deref()).unwrap_or("");
                        ListItem::new(Line::from(vec![
                            Span::raw(p.name.clone()),
                            Span::raw(format!("  {owner}")).dim(),
                        ]))
                    })
                    .collect()
            });
            draw_list(f, app, body, items, "No playlists. Press r to reload.");
        }
        Tab::Liked => {
            let items = app.liked.as_ref().map(|ts| ts.iter().map(track_item).collect());
            draw_list(f, app, body, items, "No liked songs. Press r to reload.");
        }
    }

    draw_now_playing(f, app, now_area);

    let status = if app.status.is_empty() {
        Span::raw(HELP).dim()
    } else {
        Span::raw(app.status.as_str())
    };
    f.render_widget(Paragraph::new(status), status_area);
}

fn track_item(t: &crate::api::Track) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::raw(t.name.clone()),
        Span::raw(format!("  {}", t.artist_names())).dim(),
        Span::raw(format!("  {}", format_ms(t.duration_ms))).dim(),
    ]))
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let [input_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    let border = if app.typing { Style::new() } else { Style::new().dim() };
    f.render_widget(
        Paragraph::new(app.query.as_str()).block(Block::bordered().title(" search ").border_style(border)),
        input_area,
    );
    if app.typing {
        let x = input_area.x + 1 + app.query.chars().count() as u16;
        f.set_cursor_position(Position::new(x.min(input_area.right() - 2), input_area.y + 1));
    }
    let items = Some(app.results.iter().map(track_item).collect());
    draw_list(f, app, list_area, items, "Type a query and press Enter.");
}

fn draw_list(f: &mut Frame, app: &App, area: Rect, items: Option<Vec<ListItem>>, empty: &str) {
    let items = items.unwrap_or_default();
    if items.is_empty() {
        f.render_widget(Paragraph::new(empty).dim(), area.inner(ratatui::layout::Margin::new(1, 0)));
        return;
    }
    let list = List::new(items)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    let mut state = ListState::default().with_selected(Some(app.selected()));
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_now_playing(f: &mut Frame, app: &App, area: Rect) {
    let pb = app.playback.as_ref();
    let mut flags = Vec::new();
    if let Some(d) = pb.and_then(|p| p.device.as_ref()) {
        flags.push(d.name.clone());
        if let Some(v) = d.volume_percent {
            flags.push(format!("vol {v}%"));
        }
    }
    if pb.is_some_and(|p| p.shuffle_state) {
        flags.push("shuffle".into());
    }
    let icon = if pb.is_some_and(|p| p.is_playing) { "▶" } else { "⏸" };
    let block = Block::bordered()
        .title(format!(" {icon} "))
        .title_bottom(Line::from(format!(" {} ", flags.join(" · "))).right_aligned().dim());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(track) = pb.and_then(|p| p.item.as_ref()) else {
        f.render_widget(Paragraph::new("Nothing playing.").dim(), inner);
        return;
    };
    let [title_area, gauge_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    let album = track.album.as_ref().map(|a| format!(" · {}", a.name)).unwrap_or_default();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(track.name.clone()).bold(),
            Span::raw(format!("  {}{album}", track.artist_names())).dim(),
        ])),
        title_area,
    );
    let progress = app.progress_ms();
    let ratio = if track.duration_ms > 0 {
        (progress as f64 / track.duration_ms as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    f.render_widget(
        LineGauge::default()
            .ratio(ratio)
            .label(format!("{} / {}", format_ms(progress), format_ms(track.duration_ms)))
            .filled_style(Style::new())
            .unfilled_style(Style::new().dim()),
        gauge_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Device, Playback, Spotify, Track};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_results_and_now_playing() {
        let track = Track {
            uri: "spotify:track:1".into(),
            name: "Teardrop".into(),
            duration_ms: 330_000,
            artists: vec![crate::api::Named { name: "Massive Attack".into() }],
            album: Some(crate::api::Named { name: "Mezzanine".into() }),
        };
        let mut app = App::new(Spotify::offline());
        app.typing = false;
        app.query = "teardrop".into();
        app.results = vec![track.clone()];
        app.playback = Some(Playback {
            device: Some(Device {
                id: Some("d".into()),
                name: "spotlite".into(),
                is_active: true,
                volume_percent: Some(40),
            }),
            is_playing: false,
            progress_ms: Some(65_000),
            shuffle_state: true,
            item: Some(track),
        });
        let out = screen(&app);
        assert!(out.contains("1 Search"), "{out}");
        assert!(out.contains("› Teardrop  Massive Attack  5:30"), "{out}");
        assert!(out.contains("Mezzanine"), "{out}");
        assert!(out.contains("1:05 / 5:30"), "{out}");
        assert!(out.contains("spotlite · vol 40% · shuffle"), "{out}");
    }

    #[test]
    fn renders_empty_state() {
        let out = screen(&App::new(Spotify::offline()));
        assert!(out.contains("Type a query and press Enter."), "{out}");
        assert!(out.contains("Nothing playing."), "{out}");
    }

    #[test]
    fn formats_minutes_and_seconds() {
        assert_eq!(format_ms(0), "0:00");
        assert_eq!(format_ms(61_500), "1:01");
        assert_eq!(format_ms(3_600_000), "60:00");
    }
}
