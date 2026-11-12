use super::*;

use crate::api::{PAGE, SEARCH_PAGE};

fn track(n: usize) -> Track {
    serde_json::from_value(serde_json::json!({"uri": format!("spotify:track:{n}"), "name": format!("T{n}")}))
        .unwrap()
}

fn page(range: std::ops::Range<usize>, total: usize) -> ApiPage<Track> {
    let items: Vec<Track> = range.map(track).collect();
    ApiPage { raw: items.len(), items, total }
}

fn drain(rx: &std::sync::mpsc::Receiver<Cmd>) -> Vec<Cmd> {
    rx.try_iter().collect()
}

#[test]
fn liked_pages_load_as_you_scroll() {
    let (mut app, rx) = test_app();
    drain(&rx);
    app.switch_tab(Tab::Liked);
    app.tick();
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Liked, offset: 0 }]), "{cmds:?}");

    app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: page(0..PAGE, 120) });
    app.tick();
    assert!(drain(&rx).is_empty(), "no prefetch while far from the end");

    app.move_selection(45);
    app.tick();
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Liked, offset: 50 }]), "{cmds:?}");
    app.tick();
    assert!(drain(&rx).is_empty(), "no duplicate request while loading");

    app.on_msg(Msg::Tracks { list: List::Liked, offset: 50, page: page(50..100, 120) });
    assert_eq!(app.liked.items.len(), 100);
    assert_eq!(app.liked.selected(), 45);
}

#[test]
fn playlists_load_once_per_page() {
    let (mut app, rx) = test_app();
    drain(&rx);
    app.switch_tab(Tab::Playlists);
    app.tick();
    app.tick();
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Playlists, offset: 0 }]), "{cmds:?}");
}

#[test]
fn search_ignores_stale_results() {
    let (mut app, rx) = test_app();
    app.query = "first".into();
    app.submit_search();
    app.tick();
    app.query = "second".into();
    app.submit_search();
    app.tick();
    drain(&rx);
    app.on_msg(Msg::Tracks { list: List::Search("first".into()), offset: 0, page: page(0..10, 10) });
    assert!(app.search.items.is_empty());
    app.on_msg(Msg::Tracks { list: List::Search("second".into()), offset: 0, page: page(0..10, 30) });
    assert_eq!(app.search.items.len(), 10);
    assert_eq!(app.search.total, Some(30));
}

fn searches(cmds: &[Cmd]) -> Vec<String> {
    cmds.iter()
        .filter_map(|c| match c {
            Cmd::Load { list: List::Search(q), .. } => Some(q.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn search_runs_after_a_pause_in_typing() {
    let (mut app, rx) = test_app();
    app.tab = Tab::Search;
    app.typing = true;
    for c in "bowie".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    assert!(app.busy(), "the loop wakes for the coming search");
    app.tick();
    assert!(searches(&drain(&rx)).is_empty(), "no search before the pause");
    app.search_due = Some(Instant::now());
    assert!(app.tick());
    assert_eq!(searches(&drain(&rx)), ["bowie"]);
    assert!(app.typing, "you can go on typing");
    // Enter on the same text does not search again.
    app.on_key(KeyEvent::from(KeyCode::Enter));
    app.tick();
    assert!(searches(&drain(&rx)).is_empty());
    // One letter is too short; Esc cancels a coming search.
    app.typing = true;
    app.query.clear();
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    app.search_due = Some(Instant::now());
    assert!(!app.tick());
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.search_due.is_none());
}

#[test]
fn frame_period_follows_what_moves() {
    let (mut app, _rx) = test_app();
    let viz = Duration::from_millis(50);
    app.in_flight = 0;
    app.next_poll = Instant::now() + Duration::from_secs(30);
    let idle = app.frame_period(viz);
    assert!(idle > Duration::from_millis(4900) && idle <= Duration::from_secs(5), "nothing moves: {idle:?}");
    app.info("hi");
    assert!(app.frame_period(viz) <= Duration::from_secs(4), "wakes when the message ends");
    app.toast = None;
    app.in_flight = 1;
    assert_eq!(app.frame_period(viz), Duration::from_millis(120));
    app.in_flight = 0;
    app.playback = Some(serde_json::from_value(serde_json::json!({
        "is_playing": true, "progress_ms": 0,
        "item": {"uri": "spotify:track:a", "name": "A", "duration_ms": 1000, "artists": [], "album": {"name": "x", "images": []}}
    })).unwrap());
    app.viz_on = true;
    assert_eq!(app.frame_period(viz), viz);
}

#[test]
fn sidebar_keys_open_switch_and_pin() {
    use crate::sidebar::{Item as SideItem, SideView};
    let (mut app, rx) = test_app();
    app.typing = false;
    app.on_key(KeyEvent::from(KeyCode::Char('\\')));
    assert!(app.sidebar.open && app.sidebar.focus);
    app.on_key(KeyEvent::from(KeyCode::Char('l')));
    assert_eq!(app.sidebar.view, SideView::Library);
    // The cursor starts on Liked Songs; Enter opens that tab.
    assert_eq!(app.side_items(SideView::Library)[app.sidebar.cursor(SideView::Library)], SideItem::Liked);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tab, Tab::Liked);
    // Esc gives the keys back to the list; `*` then pins the selected song.
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(!app.sidebar.focus);
    app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: page(0..3, 3) });
    app.on_key(KeyEvent::from(KeyCode::Char('*')));
    assert_eq!(app.pins.len(), 1);
    assert_eq!(app.side_items(SideView::Library)[1], SideItem::Pin(0));
    // Pins and the sidebar are saved, and come back.
    let st = app.saved_state();
    let json = serde_json::to_string(&st).unwrap();
    let (mut again, _rx2) = test_app();
    again.restore(serde_json::from_str(&json).unwrap());
    assert_eq!(again.pins.len(), 1);
    assert!(again.sidebar.open);
    // `*` again unpins; `\` twice shuts (focus first, then close).
    app.on_key(KeyEvent::from(KeyCode::Char('*')));
    assert!(app.pins.is_empty());
    app.on_key(KeyEvent::from(KeyCode::Char('\\')));
    assert!(app.sidebar.focus);
    app.on_key(KeyEvent::from(KeyCode::Char('\\')));
    assert!(!app.sidebar.open);
    drop(rx);
}

#[test]
fn sidebar_actions_use_the_playing_song() {
    use crate::sidebar::SideView;
    let (mut app, rx) = test_app();
    app.typing = false;
    app.playback = Some(serde_json::from_value(serde_json::json!({
        "is_playing": true, "progress_ms": 0,
        "item": {"uri": "spotify:track:p", "name": "P", "duration_ms": 1000, "artists": [], "album": {"name": "x", "images": []}}
    })).unwrap());
    app.on_key(KeyEvent::from(KeyCode::Char('<')));
    assert_eq!(app.sidebar.view, SideView::Playing);
    drain(&rx);
    app.on_key(KeyEvent::from(KeyCode::Char('C')));
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Card(t) if t.uri == "spotify:track:p")));
    app.on_key(KeyEvent::from(KeyCode::Char('*')));
    assert_eq!(app.pins.first().map(|p| p.uri()), Some("spotify:track:p".into()));
    // Space still plays / pauses while the sidebar has the keys.
    app.on_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Toggle)));
}

#[test]
fn undo_tree_keeps_branches_and_reopens_closed_pages() {
    let (mut app, _rx) = test_app();
    app.typing = false;
    app.switch_tab(Tab::Liked);
    app.open_artist("a1".into(), "Radiohead".into());
    // Backspace goes back; a new place from there starts a branch.
    app.on_key(KeyEvent::from(KeyCode::Backspace));
    assert_eq!(app.loc(), Loc::Base(Tab::Liked));
    app.open_artist("a2".into(), "Portishead".into());
    let liked = app.nav.nodes.iter().position(|n| n.loc == Loc::Base(Tab::Liked)).unwrap();
    assert_eq!(app.nav.nodes[liked].children.len(), 2);
    // Close Portishead, then open the tree with Ctrl+Backspace (^H, as ghostty sends it).
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    app.on_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
    let cursor = app.undo.expect("the tree is open");
    let rows = app.nav.rows();
    assert_eq!(rows[cursor].node, app.nav.cur, "the cursor starts where you are");
    // Go to Portishead: its tab opens again.
    let i = rows.iter().position(|r| app.nav.nodes[r.node].label.ends_with("Portishead")).unwrap();
    app.undo = Some(i);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.undo.is_none());
    assert!(matches!(app.page(), Some(Page::Artist(a)) if a.name == "Portishead"));
    // Ctrl+Backspace while typing a search does not open it.
    app.switch_tab(Tab::Search);
    app.typing = true;
    app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
    assert!(app.undo.is_none());
}

#[test]
fn tab_samples_a_genre_then_goes_back_to_the_song() {
    let (mut app, rx) = test_app();
    app.on_msg(playing_in("spotify:playlist:p1"));
    app.on_key(KeyEvent::from(KeyCode::Char('z')));
    drain(&rx);
    // Tab on the first quick pick: pop, its top song, at once and at the same volume.
    app.on_key(KeyEvent::from(KeyCode::Tab));
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::Sample { genre, nth: 0 }] if genre == "pop"), "{cmds:?}");
    assert_eq!(app.volume(), Some(50), "no fade");
    // Tab again: the next song of the genre.
    app.on_key(KeyEvent::from(KeyCode::Tab));
    assert!(matches!(drain(&rx).as_slice(), [Cmd::Sample { nth: 1, .. }]));
    app.on_msg(Msg::Sampling { genre: "pop".into(), result: Ok(track(7)) });
    assert!(app.sample.as_ref().is_some_and(|s| s.started.is_some()));
    drain(&rx);
    // Ten seconds later it ends: back to the playlist, at the song and place it was.
    app.sample.as_mut().unwrap().started = Some(Instant::now() - SAMPLE_LEN);
    app.tick();
    let cmds = drain(&rx);
    assert!(
        cmds.iter().any(|c| matches!(c, Cmd::Unsample { restore: Some(r) }
            if r.context.as_deref() == Some("spotify:playlist:p1") && r.track == "spotify:track:x" && r.position_ms >= 30_000 && r.playing)),
        "{cmds:?}"
    );
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::Volume(_))), "no fade: {cmds:?}");
    assert_eq!(app.volume(), Some(50));
    assert!(app.sample.is_none() && app.genre_picker.is_some(), "the picker stays open");
    // Esc while a sample plays stops only the sample; Esc again closes the picker.
    app.on_key(KeyEvent::from(KeyCode::Tab));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.sample.is_none() && app.genre_picker.is_some());
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.genre_picker.is_none());
}

#[test]
fn ctrl_backspace_deletes_a_word_in_the_search() {
    let mut t = String::from("daft punk  one more ");
    delete_word(&mut t);
    assert_eq!(t, "daft punk  one ");
    delete_word(&mut t);
    assert_eq!(t, "daft punk  ");
    delete_word(&mut t);
    assert_eq!(t, "daft ");
    delete_word(&mut t);
    assert_eq!(t, "");
    delete_word(&mut t);
    assert_eq!(t, "");
    let mut t = String::from("Björk Jóga");
    delete_word(&mut t);
    assert_eq!(t, "Björk ");

    let (mut app, _rx) = test_app();
    app.switch_tab(Tab::Search);
    app.typing = true;
    app.query = "aphex twin xtal".into();
    // As kitty's keyboard protocol sends it, and as ghostty sends it (^H).
    app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
    assert_eq!(app.query, "aphex twin ");
    app.on_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
    assert_eq!(app.query, "aphex ");
    assert!(app.undo.is_none() && app.typing, "the undo tree stays shut");
    // A plain Backspace still deletes one letter.
    app.on_key(KeyEvent::from(KeyCode::Backspace));
    assert_eq!(app.query, "aphex");
}

#[test]
fn null_entries_still_advance_the_offset() {
    let mut l: Listing<Track> = Listing::default();
    let mut p = page(0..48, 120);
    p.raw = 50; // two removed tracks came back as null
    l.apply(0, p);
    assert_eq!(l.items.len(), 48);
    l.state.select(Some(45));
    assert_eq!(l.wants_page(), Some(50));
}

#[test]
fn empty_page_ends_the_list() {
    let mut l: Listing<Track> = Listing::default();
    l.apply(0, page(0..3, 10));
    assert_eq!(l.wants_page(), Some(3));
    l.apply(3, page(0..0, 10));
    assert_eq!(l.total, Some(3));
    assert!(l.wants_page().is_none());
}

#[test]
fn liked_plays_as_collection_and_search_as_uris() {
    let (mut app, rx) = test_app();
    app.switch_tab(Tab::Liked);
    app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: page(0..5, 5) });
    app.move_selection(2);
    drain(&rx);
    app.play_selected();
    let cmds = drain(&rx);
    // The song shows at once, and its ♥ is asked for at once.
    assert!(matches!(cmds.as_slice(), [Cmd::PlayLiked { from }, Cmd::CheckSaved(c)] if from == "spotify:track:2" && c == from), "{cmds:?}");
    assert_eq!(app.playing_uri(), Some("spotify:track:2"));

    app.switch_tab(Tab::Search);
    app.searched = "x".into();
    app.on_msg(Msg::Tracks { list: List::Search("x".into()), offset: 0, page: page(0..SEARCH_PAGE, 50) });
    app.play_selected();
    let cmds = drain(&rx);
    assert!(
        matches!(cmds.as_slice(), [Cmd::Play(Play::Tracks { uris, offset: 0 }), Cmd::CheckSaved(_)] if uris.len() == SEARCH_PAGE),
        "{cmds:?}"
    );
}

#[test]
fn forbidden_playlist_closes_loading_and_explains() {
    let (mut app, _rx) = test_app();
    let pl: Playlist = serde_json::from_value(serde_json::json!({
        "id": "p1", "uri": "spotify:playlist:p1", "name": "Theirs", "owner": {"id": "someone"}
    }))
    .unwrap();
    app.tab = Tab::Playlists;
    app.open = Some(OpenPlaylist { playlist: pl, tracks: Listing::default() });
    app.tick();
    app.on_msg(Msg::LoadFailed { list: List::Playlist("p1".into()), forbidden: true });
    let open = app.open.as_ref().unwrap();
    assert!(!open.tracks.loading);
    assert!(open.tracks.wants_page().is_none());
    assert!(app.toast.as_ref().unwrap().text.contains("Press P"));
}

fn playing(volume: u8, duration_ms: u64) -> Msg {
    Msg::Playback(Some(Box::new(
        serde_json::from_value(serde_json::json!({
            "device": {"id": "d", "name": "spotlite", "is_active": true, "volume_percent": volume},
            "is_playing": true, "progress_ms": 0,
            "item": {"uri": "spotify:track:x", "name": "X", "duration_ms": duration_ms}
        }))
        .unwrap(),
    )))
}

fn playing_song(uri: &str) -> Playback {
    serde_json::from_value(serde_json::json!({
        "device": {"id": "d", "name": "spotlite", "is_active": true, "volume_percent": 50},
        "is_playing": true, "progress_ms": 0,
        "item": {"uri": uri, "name": "X", "duration_ms": 200000}
    }))
    .unwrap()
}

fn volumes(cmds: &[Cmd]) -> Vec<u8> {
    cmds.iter().filter_map(|c| if let Cmd::Volume(v) = c { Some(*v) } else { None }).collect()
}

#[test]
fn volume_changes_are_coalesced() {
    let (mut app, rx) = test_app();
    app.on_msg(playing(40, 1000));
    drain(&rx);
    for _ in 0..4 {
        app.on_key(KeyEvent::from(KeyCode::Char('+')));
    }
    assert_eq!(app.volume(), Some(60), "shown at once");
    app.tick();
    assert_eq!(volumes(&drain(&rx)), vec![60], "one request for four presses");
    app.on_key(KeyEvent::from(KeyCode::Char('-')));
    app.tick();
    assert!(volumes(&drain(&rx)).is_empty(), "waits for the first to finish");
    app.on_msg(Msg::Done(Kind::Volume));
    app.tick();
    assert_eq!(volumes(&drain(&rx)), vec![55]);
    // A poll with the old volume does not move the slider back.
    app.on_msg(playing(40, 1000));
    assert_eq!(app.volume(), Some(55));
}

#[test]
fn local_controls_use_mpris_and_show_at_once() {
    let (tx, rx) = std::sync::mpsc::channel();
    let (ctl_tx, ctl_rx) = std::sync::mpsc::channel();
    let mut app = App::new(tx, true, Images::disabled()).with_control(ctl_tx, "spotlite".into());
    app.on_msg(playing(50, 200_000));
    app.poll_pending = false;
    drain(&rx);

    app.on_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(!app.playback.as_ref().unwrap().is_playing, "paused on screen at once");
    assert_eq!(ctl_rx.try_recv().unwrap(), Ctl::Pause);
    assert!(drain(&rx).iter().all(|c| !matches!(c, Cmd::Toggle)), "not sent to the Web API");

    // A poll that still says "playing" does not undo the pause.
    app.on_msg(playing(50, 200_000));
    assert!(!app.playback.as_ref().unwrap().is_playing);

    app.on_key(KeyEvent::from(KeyCode::Right));
    assert_eq!(ctl_rx.try_recv().unwrap(), Ctl::Seek(10_000));
    app.on_key(KeyEvent::from(KeyCode::Char('n')));
    assert_eq!(ctl_rx.try_recv().unwrap(), Ctl::Next);
    assert_eq!(app.progress_ms(), 0);
    app.tick();
    assert!(app.next_poll <= Instant::now() + CONFIRM_AFTER, "confirms soon");
}

#[test]
fn other_device_uses_the_web_api() {
    let (tx, rx) = std::sync::mpsc::channel();
    let (ctl_tx, ctl_rx) = std::sync::mpsc::channel();
    let mut app = App::new(tx, true, Images::disabled()).with_control(ctl_tx, "spotlite".into());
    let mut msg = playing(50, 200_000);
    if let Msg::Playback(Some(pb)) = &mut msg {
        pb.device.as_mut().unwrap().name = "Phone".into();
    }
    app.on_msg(msg);
    drain(&rx);
    app.on_key(KeyEvent::from(KeyCode::Char(' ')));
    assert!(ctl_rx.try_recv().is_err());
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Toggle)));
}

fn pl(id: &str, name: &str) -> Playlist {
    serde_json::from_value(serde_json::json!({
        "id": id, "uri": format!("spotify:playlist:{id}"), "name": name, "owner": {"id": "me"}
    }))
    .unwrap()
}

#[test]
fn folders_browse_and_play() {
    let (mut app, rx) = test_app();
    app.on_msg(Msg::User(serde_json::from_value(serde_json::json!({"id": "me", "uri": "spotify:user:me"})).unwrap()));
    app.set_folders(crate::folders::parse_rootlist([
        "spotify:playlist:a",
        "spotify:start-group:1:Gym",
        "spotify:playlist:b",
        "spotify:start-group:2:Heavy",
        "spotify:playlist:c",
        "spotify:end-group:2",
        "spotify:end-group:1",
    ]));
    app.switch_tab(Tab::Playlists);
    drain(&rx);
    app.tick();
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Playlists, offset: 0 }]), "{cmds:?}");
    let page = ApiPage { items: vec![pl("a", "A"), pl("b", "B"), pl("c", "C"), pl("new", "New one")], raw: 4, total: 120 };
    app.on_msg(Msg::Playlists { offset: 0, page });
    app.tick();
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::Load { list: List::Playlists, offset: 4 }]), "loads every page: {cmds:?}");

    let names = |app: &App| -> Vec<String> {
        app.rows.items.iter().map(|r| match r {
            Row::Folder { name, count, .. } => format!("[{name} {count}]"),
            Row::Playlist(p) => p.name.clone(),
        }).collect()
    };
    assert_eq!(names(&app), ["A", "[Gym 2]", "New one"], "untracked playlists go last");

    app.move_selection(1);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(names(&app), ["B", "[Heavy 1]"]);
    assert_eq!(app.folder_names(), ["Gym"]);
    app.move_selection(1);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(names(&app), ["C"]);
    app.on_key(KeyEvent::from(KeyCode::Char('P')));
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Play(Play::Context { uri, .. }) if uri == "spotify:playlist:c")), "{cmds:?}");

    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(names(&app), ["B", "[Heavy 1]"]);
    assert_eq!(app.rows.selected(), 1, "selection comes back");
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.rows.selected(), 1);
    assert!(app.folder_names().is_empty());
}

#[test]
fn a_tree_without_folders_keeps_the_flat_list() {
    let (mut app, _rx) = test_app();
    app.set_folders(crate::folders::parse_rootlist(["spotify:playlist:a"]));
    assert!(!app.foldered());
}

fn track_by(id: &str, artist: (&str, &str), album: (&str, &str)) -> Track {
    serde_json::from_value(serde_json::json!({
        "uri": format!("spotify:track:{id}"), "name": format!("Song {id}"), "duration_ms": 1000,
        "artists": [{"name": artist.1, "uri": format!("spotify:artist:{}", artist.0)}],
        "album": {"name": album.1, "id": album.0, "uri": format!("spotify:album:{}", album.0), "images": []}
    }))
    .unwrap()
}

#[test]
fn menu_opens_artist_album_and_back() {
    let (mut app, rx) = test_app();
    app.switch_tab(Tab::Liked);
    let t = track_by("1", ("ar1", "Bowie"), ("al1", "Low"));
    app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: ApiPage { items: vec![t], raw: 1, total: 1 } });
    drain(&rx);

    app.on_key(KeyEvent::from(KeyCode::Char('.')));
    let labels: Vec<String> = app.menu.as_ref().unwrap().items.iter().map(|i| i.1.clone()).collect();
    assert!(labels.iter().any(|l| l == "Go to artist: Bowie"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "Go to album: Low"), "{labels:?}");
    assert!(!labels.iter().any(|l| l.starts_with("Remove from")), "not an editable playlist: {labels:?}");

    // The key hint runs its entry.
    app.on_key(KeyEvent::from(KeyCode::Char('i')));
    assert!(app.menu.is_none());
    assert_eq!(app.view(), View::Artist);
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Artist(id) if id == "ar1")));

    app.on_msg(Msg::Artist {
        artist: serde_json::from_value(serde_json::json!({"id": "ar1", "name": "David Bowie", "images": []})).unwrap(),
        top: vec![track_by("9", ("ar1", "Bowie"), ("al2", "Heroes"))],
        albums: vec![serde_json::from_value(serde_json::json!({"name": "Low", "id": "al1", "uri": "spotify:album:al1"})).unwrap()],
    });
    let Some(Page::Artist(a)) = app.page() else { panic!() };
    // Popular: 1 song; Radio: artist + 1 song radio; Albums: 1. Each section has a title row.
    assert_eq!((a.name.as_str(), a.rows.items.len()), ("David Bowie", 7));
    assert_eq!(a.rows.selected(), 1, "the cursor starts on the first song, not a title");

    // j skips the "Radio" title; Tab jumps to the next section.
    app.move_selection(1);
    let Some(Page::Artist(a)) = app.page() else { panic!() };
    assert!(matches!(a.rows.current(), Some(ArtistRow::Radio(t)) if t.uri == "spotify:artist:ar1"));
    app.move_selection(-1);
    app.on_key(KeyEvent::from(KeyCode::Tab));
    let Some(Page::Artist(a)) = app.page() else { panic!() };
    assert_eq!(a.rows.selected(), 3);

    // Enter on the artist radio opens it.
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.view(), View::Radio);
    if crate::client::AVAILABLE {
        assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Radio(u) if u == "spotify:artist:ar1")));
    }
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.view(), View::Artist);
    drain(&rx);

    // Tab again: the album section. Enter opens the album, and its tracks load.
    app.on_key(KeyEvent::from(KeyCode::Tab));
    app.on_key(KeyEvent::from(KeyCode::BackTab));
    let Some(Page::Artist(a)) = app.page() else { panic!() };
    assert_eq!(a.rows.selected(), 3, "shift+tab goes back one section");
    app.on_key(KeyEvent::from(KeyCode::Tab));
    app.on_key(KeyEvent::from(KeyCode::Tab));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.view(), View::Album);
    app.tick();
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Load { list: List::Album(id), offset: 0 } if id == "al1")), "{cmds:?}");
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Album(id) if id == "al1")), "details asked for: {cmds:?}");

    // Album tracks get the album (for covers), and Enter plays the album from that song.
    let bare: Track = serde_json::from_value(serde_json::json!({"uri": "spotify:track:a1", "name": "Speed of Life"})).unwrap();
    app.on_msg(Msg::Tracks { list: List::Album("al1".into()), offset: 0, page: ApiPage { items: vec![bare], raw: 1, total: 1 } });
    assert_eq!(app.selected_track().unwrap().album_name(), "Low");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let cmds = drain(&rx);
    assert!(
        cmds.iter().any(|c| matches!(c, Cmd::Play(Play::Context { uri, from: Some(f) }) if uri == "spotify:album:al1" && f == "spotify:track:a1")),
        "{cmds:?}"
    );

    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.view(), View::Artist);
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.view(), View::Liked);

    // Esc went back, but the artist, radio and album tabs stay open.
    let keys: Vec<String> = app.pages.iter().map(Page::key).collect();
    assert_eq!(keys, ["artist:ar1", "radio:spotify:artist:ar1", "album:al1"]);
    // 6 is the second page tab (after Search, Playlists, Liked, Recent).
    app.on_key(KeyEvent::from(KeyCode::Char('6')));
    assert_eq!(app.view(), View::Radio);
    app.on_key(KeyEvent::from(KeyCode::Char('2')));
    assert_eq!(app.view(), View::Playlists);
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.view(), View::Radio, "Esc goes back to the radio tab");
    // Opening the artist again goes to its tab, without a new load.
    drain(&rx);
    app.open_artist("ar1".into(), "Bowie".into());
    assert_eq!((app.view(), app.pages.len()), (View::Artist, 3));
    assert!(drain(&rx).is_empty());
    // L and H walk the tab bar; x closes the tab and goes back.
    app.on_key(KeyEvent::from(KeyCode::Char('L')));
    assert_eq!(app.view(), View::Radio);
    app.on_key(KeyEvent::from(KeyCode::Char('x')));
    assert_eq!(app.pages.len(), 2);
    assert_eq!(app.view(), View::Artist);
    app.on_key(KeyEvent::from(KeyCode::Char('H')));
    assert_eq!(app.view(), View::Recent);
}

#[test]
fn tabs_come_back_at_start_and_load_when_shown() {
    let (mut app, rx) = test_app();
    app.open_artist("ar1".into(), "Bowie".into());
    app.start_radio(track_by("s", ("ar1", "Bowie"), ("al1", "Low")));
    let saved = app.saved_state();
    let text = serde_json::to_string(&saved).unwrap();
    let back: State = serde_json::from_str(&text).unwrap();
    drain(&rx);

    let (mut app, rx) = test_app();
    app.restore(back);
    assert_eq!(app.pages.iter().map(Page::key).collect::<Vec<_>>(), ["artist:ar1", "radio:spotify:track:s"]);
    assert_eq!(app.view(), View::Radio, "the tab shown at quit");
    let cmds = drain(&rx);
    if crate::client::AVAILABLE {
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Radio(u) if u == "spotify:track:s")), "{cmds:?}");
    }
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::Artist(_))), "the artist waits: {cmds:?}");
    app.on_key(KeyEvent::from(KeyCode::Char('5')));
    assert_eq!(app.view(), View::Artist);
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Artist(id) if id == "ar1")));
    // Shown once, it does not load again.
    app.on_key(KeyEvent::from(KeyCode::Char('6')));
    app.on_key(KeyEvent::from(KeyCode::Char('5')));
    assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::Artist(_))));
}

#[test]
fn theme_picker_previews_and_esc_puts_it_back() {
    let (mut app, _rx) = test_app();
    assert_eq!(crate::theme::name(), "amber");
    app.on_key(KeyEvent::from(KeyCode::Char('T')));
    let menu = app.menu.as_ref().unwrap();
    assert!(menu.is_theme());
    assert!(menu.items.iter().any(|i| i.2 == Action::Theme("tokyo-night".into())), "built-ins listed");
    // Moving previews; Esc goes back.
    app.on_key(KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(crate::theme::name(), "rose-pine");
    assert_eq!(crate::theme::c().logo, '✦');
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(crate::theme::name(), "amber");
    assert!(app.menu.is_none());
    // Enter keeps it, and the saved state carries it.
    app.on_key(KeyEvent::from(KeyCode::Char('T')));
    app.on_key(KeyEvent::from(KeyCode::Char('j')));
    app.on_key(KeyEvent::from(KeyCode::Char('j')));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(crate::theme::name(), "gruvbox");
    assert_eq!(app.saved_state().theme.as_deref(), Some("gruvbox"));
    crate::theme::apply("amber").unwrap();
}

#[test]
fn palette_finds_and_runs_actions() {
    let (mut app, _rx) = test_app();
    app.on_key(KeyEvent::from(KeyCode::Char(':')));
    assert!(app.palette.is_some());
    for c in "liked".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    assert_eq!(app.palette_entries()[0].label, "Go to Liked");
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.palette.is_none());
    assert_eq!(app.view(), View::Liked);
    // Ctrl+P opens it too; Esc closes it and does nothing.
    app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert!(app.palette.is_some());
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.palette.is_none());
    assert_eq!(app.view(), View::Liked);
}

fn playing_in(context: &str) -> Msg {
    Msg::Playback(Some(Box::new(
        serde_json::from_value(serde_json::json!({
            "device": {"id": "d", "name": "spotlite", "is_active": true, "volume_percent": 50},
            "is_playing": true, "progress_ms": 30_000, "shuffle_state": false,
            "context": if context.is_empty() { serde_json::Value::Null } else { serde_json::json!({ "uri": context }) },
            "item": {"uri": "spotify:track:x", "name": "X", "duration_ms": 200_000}
        }))
        .unwrap(),
    )))
}

#[test]
fn true_shuffle_plays_the_whole_list_in_a_random_order() {
    let (mut app, rx) = test_app();
    app.on_msg(playing_in("spotify:playlist:p1"));
    drain(&rx);
    // On: the playlist starts again, shuffled by spotlite, from the song playing.
    app.on_key(KeyEvent::from(KeyCode::Char('s')));
    let cmds = drain(&rx);
    assert!(
        cmds.iter().any(|c| matches!(c, Cmd::PlayShuffled { context, from: Some(f), position_ms }
            if context == "spotify:playlist:p1" && f == "spotify:track:x" && *position_ms >= 30_000)),
        "{cmds:?}"
    );
    // The songs play as a list: the player still shows the playlist, and ⇄ is on.
    app.on_msg(Msg::Shuffled { context: "spotify:playlist:p1".into(), uris: vec!["spotify:track:x".into()] });
    app.on_msg(playing_in(""));
    let pb = app.playback.as_ref().unwrap();
    assert_eq!(pb.context.as_ref().map(|c| c.uri.as_str()), Some("spotify:playlist:p1"));
    assert!(pb.shuffle_state);
    // Playing an album now: shuffled too.
    drain(&rx);
    app.send(Cmd::Play(Play::Context { uri: "spotify:album:a".into(), from: None }));
    assert!(matches!(drain(&rx).as_slice(), [Cmd::PlayShuffled { context, from: None, .. }] if context == "spotify:album:a"));
    // Off: back to the playlist's order at the same song.
    app.on_key(KeyEvent::from(KeyCode::Char('s')));
    let cmds = drain(&rx);
    assert!(
        cmds.iter().any(|c| matches!(c, Cmd::PlayAt(Play::Context { uri, from: Some(f) }, _) if uri == "spotify:playlist:p1" && f == "spotify:track:x")),
        "{cmds:?}"
    );
}

#[test]
fn spotify_shuffle_turned_on_elsewhere_becomes_the_true_shuffle() {
    let (mut app, rx) = test_app();
    drain(&rx);
    let Msg::Playback(Some(mut pb)) = playing_in("spotify:playlist:p1") else { panic!() };
    pb.shuffle_state = true;
    app.on_msg(Msg::Playback(Some(pb)));
    assert!(app.true_shuffle);
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Shuffle(false))), "{cmds:?}");
    assert!(cmds.iter().any(|c| matches!(c, Cmd::PlayShuffled { .. })), "{cmds:?}");
}

#[test]
fn randomize_picks_a_genre_then_fills_a_page() {
    let (mut app, rx) = test_app();
    // z opens the picker; typing searches it.
    app.on_key(KeyEvent::from(KeyCode::Char('z')));
    assert!(app.genre_picker.is_some());
    for c in "shoeg".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    drain(&rx);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.genre_picker.is_none());
    assert_eq!(app.view(), View::Random);
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Randomize(g) if g == "shoegaze")));
    app.on_msg(Msg::Random { genre: "shoegaze".into(), result: Ok((0..50).map(track).collect()) });
    assert_eq!(app.tracks().unwrap().items.len(), 50);
    // d rolls again; S asks to save.
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Randomize(_))));
    app.on_msg(Msg::Random { genre: "shoegaze".into(), result: Ok((50..100).map(track).collect()) });
    app.can_edit = true;
    app.on_key(KeyEvent::from(KeyCode::Char('S')));
    let confirm = app.confirm.as_ref().expect("asks first");
    assert!(matches!(&confirm.cmd, Cmd::CreatePlaylist { name, uris, .. } if name == "Random · shoegaze 2" && uris.len() == 50));
}

#[test]
fn a_limited_list_keeps_what_it_shows_and_loads_again_later() {
    let (mut app, rx) = test_app();
    app.switch_tab(Tab::Liked);
    app.tick();
    app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: crate::api::Page { items: vec![track(1)], raw: 1, total: 1 } });
    app.on_msg(Msg::RetryLater { list: List::Liked, secs: 0 });
    assert_eq!(app.liked.items.len(), 1);
    assert!(app.toast.as_ref().is_some_and(|t| !t.error && t.text.contains("saved copy")));
    // The wait is at least 5 s; pretend it is over.
    app.retries[0].1 = Instant::now();
    drain(&rx);
    app.tick();
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Load { list: List::Liked, offset: 0 })));
}

#[test]
fn shuffle_is_kept_until_spotify_agrees() {
    let (mut app, _rx) = test_app();
    app.on_msg(playing(40, 100_000));
    app.on_key(KeyEvent::from(KeyCode::Char('s')));
    // A poll that left before the change still says off: the screen keeps on.
    app.on_msg(playing(40, 100_000));
    assert!(app.playback.as_ref().unwrap().shuffle_state);
    assert!(app.saved_state().playback.unwrap().shuffle, "and it is what is saved");
}

#[test]
fn everything_comes_back_at_start() {
    let (mut app, rx) = test_app();
    app.on_msg(Msg::Playback(Some(Box::new(
        serde_json::from_value(serde_json::json!({
            "device": {"id": "d", "name": "spotlite", "is_active": true, "volume_percent": 55},
            "is_playing": true, "progress_ms": 61_000, "shuffle_state": true, "repeat_state": "context",
            "context": {"uri": "spotify:playlist:p1"},
            "item": {"uri": "spotify:track:x", "name": "X", "duration_ms": 200_000}
        }))
        .unwrap(),
    ))));
    app.query = "bowie".into();
    app.submit_search();
    app.on_key(KeyEvent::from(KeyCode::Char('t')));
    app.on_key(KeyEvent::from(KeyCode::Char('3')));
    app.viz_on = true;
    let text = serde_json::to_string(&app.saved_state()).unwrap();
    drain(&rx);

    // Next start: tab, search, visualizer and the song come back.
    let (mut app, rx) = test_app();
    app.restore(serde_json::from_str(&text).unwrap());
    assert_eq!((app.tab, app.searched.as_str(), app.search_kind, app.viz_on), (Tab::Liked, "bowie", SearchKind::Artists, true));
    // Nothing plays yet: it waits for the player state and for spotifyd.
    app.tick();
    assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::Resume(_))));
    app.on_msg(Msg::Playback(None));
    app.on_msg(Msg::Device(Some("spotlite".into())));
    app.tick();
    let cmds = drain(&rx);
    let Some(Cmd::Resume(sp)) = cmds.iter().find(|c| matches!(c, Cmd::Resume(_))) else { panic!("{cmds:?}") };
    assert_eq!((sp.track.as_str(), sp.context.as_deref()), ("spotify:track:x", Some("spotify:playlist:p1")));
    assert!(sp.position_ms >= 61_000 && sp.playing && sp.shuffle && sp.repeat == "context");
    assert_eq!(sp.volume, Some(55));
    app.tick();
    assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::Resume(_))), "only once");

    // If music already plays at start (spotifyd kept going), it is left alone.
    let (mut app, rx) = test_app();
    app.restore(serde_json::from_str(&text).unwrap());
    app.on_msg(playing(40, 100_000));
    app.on_msg(Msg::Device(Some("spotlite".into())));
    app.tick();
    assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::Resume(_))));
}

#[test]
fn recent_tab_loads_again_when_the_song_changed() {
    let (mut app, rx) = test_app();
    app.on_key(KeyEvent::from(KeyCode::Char('4')));
    assert_eq!(app.view(), View::Recent);
    app.tick();
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Load { list: List::Recent, offset: 0 })));
    let t = track_by("1", ("a", "A"), ("b", "B"));
    app.on_msg(Msg::Tracks { list: List::Recent, offset: 0, page: ApiPage { items: vec![t], raw: 1, total: 1 } });
    app.on_key(KeyEvent::from(KeyCode::Char('1')));
    app.on_key(KeyEvent::from(KeyCode::Char('4')));
    assert_eq!(app.recent.items.len(), 1, "a quick look back costs nothing");
    app.on_msg(Msg::Playback(Some(Box::new(playing_song("spotify:track:new")))));
    app.on_key(KeyEvent::from(KeyCode::Char('1')));
    app.on_key(KeyEvent::from(KeyCode::Char('4')));
    assert!(app.recent.items.is_empty(), "another song played: loads again");
}

#[test]
fn keys_from_the_config_and_paste_work() {
    let (mut app, rx) = test_app();
    let mut custom = std::collections::HashMap::new();
    custom.insert("next".to_owned(), crate::keys::Spec::One("N".into()));
    app.keys = crate::keys::Keys::new(&custom).0;
    app.on_msg(Msg::Playback(Some(Box::new(playing_song("spotify:track:a")))));
    drain(&rx);
    app.on_key(KeyEvent::from(KeyCode::Char('n')));
    assert!(drain(&rx).is_empty(), "n does nothing now");
    app.on_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT));
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Next)));
    // Paste goes to the search box, as one piece, with the cursor after it.
    app.on_paste("massive\nattack");
    assert!(app.typing);
    assert_eq!(app.query, "massive attack");
    app.on_key(KeyEvent::from(KeyCode::Left));
    app.on_key(KeyEvent::from(KeyCode::Char('s')));
    assert_eq!(app.query, "massive attacsk", "typing goes in at the cursor");
}

fn key(c: char) -> KeyEvent {
    KeyEvent::from(KeyCode::Char(c))
}

fn liked_songs(app: &mut App) {
    let songs = vec![
        track_by("1", ("ma", "Massive Attack"), ("me", "Mezzanine")),
        track_by("2", ("po", "Portishead"), ("du", "Dummy")),
        track_by("3", ("ma", "Massive Attack"), ("bl", "Blue Lines")),
        track_by("4", ("tr", "Tricky"), ("ma", "Maxinquaye")),
    ];
    app.liked.fill(songs);
    app.on_key(key('3'));
    assert_eq!(app.view(), View::Liked);
}

#[test]
fn filter_narrows_the_list_and_plays_what_shows() {
    let (mut app, rx) = test_app();
    liked_songs(&mut app);
    app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert!(app.filtering);
    for c in "massive".chars() {
        app.on_key(key(c));
    }
    assert_eq!(app.liked.len(), 2, "two songs by Massive Attack");
    assert_eq!(app.liked.current().map(|t| t.uri.as_str()), Some("spotify:track:1"));
    // Enter keeps the filter and goes to the list; j moves inside what shows.
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(!app.filtering);
    app.on_key(key('j'));
    assert_eq!(app.liked.current().map(|t| t.uri.as_str()), Some("spotify:track:3"));
    drain(&rx);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let cmds = drain(&rx);
    assert!(
        cmds.iter().any(|c| matches!(c, Cmd::Play(Play::Tracks { uris, offset: 1 }) if uris == &["spotify:track:1", "spotify:track:3"])),
        "the shown songs play, from the selected one: {cmds:?}"
    );
    // Esc clears the filter (and only then goes back).
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!((app.liked.len(), app.view()), (4, View::Liked));
    assert_eq!(app.liked.current().map(|t| t.uri.as_str()), Some("spotify:track:3"), "the selection stays on its song");
}

#[test]
fn sort_cycles_and_keeps_the_selection() {
    let (mut app, _rx) = test_app();
    liked_songs(&mut app);
    app.on_key(key('j'));
    app.on_key(key(','));
    assert_eq!(app.liked.sort, Some((SortBy::Title, false)));
    app.on_key(key(','));
    assert_eq!(app.liked.sort, Some((SortBy::Artist, false)));
    let order: Vec<&str> = app.liked.rows().map(|(_, t)| t.artists[0].name.as_str()).collect();
    assert_eq!(order, ["Massive Attack", "Massive Attack", "Portishead", "Tricky"]);
    assert_eq!(app.liked.current().map(|t| t.uri.as_str()), Some("spotify:track:2"), "still on the same song");
    app.on_key(key(';'));
    assert_eq!(app.liked.rows().next().map(|(_, t)| t.artists[0].name.as_str()), Some("Tricky"));
    for _ in 0..3 {
        app.on_key(key(','));
    }
    assert_eq!(app.liked.sort, None, "back to the list's own order");
}

#[test]
fn marks_make_actions_apply_to_many() {
    let (mut app, rx) = test_app();
    liked_songs(&mut app);
    app.on_key(key('w'));
    app.on_key(key('j'));
    app.on_key(key('w'));
    assert_eq!(app.marks.len(), 2);
    drain(&rx);
    app.on_key(key('a'));
    let queued: Vec<String> = drain(&rx).into_iter().filter_map(|c| match c {
        Cmd::Queue { uri, .. } => Some(uri),
        _ => None,
    }).collect();
    assert_eq!(queued, ["spotify:track:1", "spotify:track:3"], "in list order");
    assert!(app.marks.is_empty(), "an action uses the marks up");
    app.on_key(key('W'));
    assert_eq!(app.marks.len(), 4);
    app.library = Some(Library::default());
    app.on_key(key('f'));
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::SetSavedMany { uris, saved: true } if uris.len() == 4)), "{cmds:?}");
    app.on_key(key('u'));
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::SetSavedMany { uris, saved: false } if uris.len() == 4)), "undo: {cmds:?}");
    // Esc first clears marks.
    app.on_key(key('W'));
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.marks.is_empty());
}

#[test]
fn c_opens_the_list_the_song_plays_from() {
    let (mut app, _rx) = test_app();
    let mut pb = playing_song("spotify:track:3");
    pb.context = Some(serde_json::from_value(serde_json::json!({ "uri": "spotify:user:me:collection" })).unwrap());
    app.on_msg(Msg::Playback(Some(Box::new(pb))));
    liked_songs(&mut app);
    app.on_key(key('1'));
    assert_eq!(app.view(), View::Search);
    app.on_key(key('c'));
    assert_eq!(app.view(), View::Liked);
    assert_eq!(app.liked.current().map(|t| t.uri.as_str()), Some("spotify:track:3"));
}

#[test]
fn search_history_and_palette_places() {
    let (mut app, _rx) = test_app();
    app.on_key(key('/'));
    for q in ["bowie", "massive attack"] {
        app.query.set(q);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        app.on_key(key('/'));
    }
    app.query.clear();
    app.on_key(KeyEvent::from(KeyCode::Up));
    assert_eq!(app.query, "massive attack");
    app.on_key(KeyEvent::from(KeyCode::Up));
    assert_eq!(app.query, "bowie");
    app.on_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(app.query, "massive attack");
    app.on_key(KeyEvent::from(KeyCode::Esc));
    // The palette finds a playlist by name and opens it.
    app.user_id = Some("me".into());
    app.on_msg(Msg::Playlists { offset: 0, page: ApiPage { items: vec![pl("p1", "Gym")], raw: 1, total: 1 } });
    app.on_key(key(':'));
    for c in "gym".chars() {
        app.on_key(key(c));
    }
    assert!(app.palette_entries().iter().any(|e| e.label == "Playlist: Gym"));
    let at = app.palette_entries().iter().position(|e| e.label == "Playlist: Gym").unwrap();
    app.palette.as_mut().unwrap().cursor = at;
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.view(), View::Playlist);
    assert_eq!(app.open.as_ref().map(|o| o.playlist.name.as_str()), Some("Gym"));
}

#[test]
fn a_message_waits_until_an_error_was_read() {
    let (mut app, _rx) = test_app();
    app.error("Spotify refused this");
    app.info("Added to queue");
    assert!(app.toast.as_ref().is_some_and(|t| t.error), "the error stays");
    app.toast.as_mut().unwrap().until = Instant::now();
    app.tick();
    assert_eq!(app.toast.as_ref().map(|t| t.text.as_str()), Some("Added to queue"), "then the message shows");
    app.info("Next");
    assert_eq!(app.toast.as_ref().map(|t| t.text.as_str()), Some("Next"), "messages replace messages");
}

#[test]
fn a_song_change_asks_little() {
    let (mut app, rx) = test_app();
    app.library = Some(Library { tracks: ["spotify:track:a".to_owned()].into(), ..Default::default() });
    let queue: Vec<Track> = ["b", "c"].iter().map(|n| track_by(n, ("x", "X"), ("y", "Y"))).collect();
    app.on_msg(Msg::Playback(Some(Box::new(playing_song("spotify:track:a")))));
    let cmds = drain(&rx);
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::CheckSaved(_))), "the ♥ comes from the Liked index");
    assert_eq!(app.saved, Some(("spotify:track:a".into(), true)));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::UpNext)));
    app.on_msg(Msg::UpNext(queue.clone()));
    assert_eq!(app.up_next.as_ref().map(|t| t.uri.as_str()), Some(queue[0].uri.as_str()));
    // The next song is the one Spotify said: the list moves on, nothing is asked.
    app.on_msg(Msg::Playback(Some(Box::new(playing_song(&queue[0].uri)))));
    assert!(drain(&rx).is_empty());
    assert_eq!(app.up_next.as_ref().map(|t| t.uri.as_str()), Some(queue[1].uri.as_str()));
    // Another song (a skip elsewhere): ask again.
    app.on_msg(Msg::Playback(Some(Box::new(playing_song("spotify:track:zz")))));
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::UpNext)));
}

#[test]
fn liking_a_known_song_is_one_request_and_keeps_the_list() {
    let (mut app, rx) = test_app();
    app.library = Some(Library::default());
    let t = track_by("s1", ("x", "X"), ("y", "Y"));
    app.liked.fill(vec![track_by("old", ("x", "X"), ("y", "Y"))]);
    app.search.fill(vec![t.clone()]);
    drain(&rx);
    app.toggle_like(Some(t.clone()));
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::SetSaved { saved: true, .. }]), "{cmds:?}");
    app.on_msg(Msg::Saved { uri: t.uri.clone(), saved: true });
    assert_eq!(app.liked.items.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Song s1", "Song old"], "on top, no reload");
    assert_eq!(app.liked.total, Some(2));
}

#[test]
fn the_poll_slows_down_when_nothing_plays() {
    let (mut app, _rx) = test_app();
    assert_eq!(app.poll_every(), POLL_IDLE);
    let mut pb = playing_song("spotify:track:a");
    app.on_msg(Msg::Playback(Some(Box::new(pb.clone()))));
    // No player events from spotifyd here (no playerctl): polls come more often.
    assert_eq!(app.poll_every(), POLL_REMOTE);
    pb.is_playing = false;
    app.on_msg(Msg::Playback(Some(Box::new(pb))));
    assert_eq!(app.poll_every(), POLL_PAUSED);
}

#[test]
fn search_lists_artists_albums_and_playlists() {
    use crate::api::{Artist, Found};
    let (mut app, rx) = test_app();
    app.query = "bowie".into();
    app.submit_search();
    app.on_key(KeyEvent::from(KeyCode::Char('t')));
    assert_eq!(app.search_kind, SearchKind::Artists);
    app.tick();
    let cmds = drain(&rx);
    assert!(
        cmds.iter().any(|c| matches!(c, Cmd::Load { list: List::Found { query, kind: "artist" }, offset: 0 } if query == "bowie")),
        "{cmds:?}"
    );
    let artist: Artist = serde_json::from_value(serde_json::json!({"id": "ar1", "name": "David Bowie"})).unwrap();
    // An answer for another kind is dropped.
    let other = List::Found { query: "bowie".into(), kind: "album" };
    app.on_msg(Msg::Found { list: other, offset: 0, page: ApiPage { items: vec![Found::Artist(artist.clone())], raw: 1, total: 1 } });
    assert!(app.found.items.is_empty());
    let list = List::Found { query: "bowie".into(), kind: "artist" };
    app.on_msg(Msg::Found { list, offset: 0, page: ApiPage { items: vec![Found::Artist(artist)], raw: 1, total: 1 } });
    assert_eq!(app.list_len(), 1);
    // Enter opens the artist in its own tab.
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.view(), View::Artist);
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Artist(id) if id == "ar1")));
    // Back on Search, the artists are still listed.
    app.on_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!((app.view(), app.found.items.len()), (View::Search, 1));
    for _ in 0..3 {
        app.on_key(KeyEvent::from(KeyCode::Char('t')));
    }
    assert_eq!(app.search_kind, SearchKind::Songs, "t goes round");
}

#[test]
fn like_the_selected_song_and_save_a_radio() {
    let (mut app, rx) = test_app();
    app.switch_tab(Tab::Liked);
    let t = track_by("1", ("a", "A"), ("b", "B"));
    app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: ApiPage { items: vec![t.clone()], raw: 1, total: 1 } });
    drain(&rx);
    // f is for the selected song, not only the playing one.
    app.on_key(KeyEvent::from(KeyCode::Char('f')));
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::ToggleSaved(u) if *u == t.uri)));
    app.on_msg(Msg::Saved { uri: t.uri.clone(), saved: false });
    assert!(app.liked.items.is_empty(), "an unliked song leaves Liked Songs");
    assert_eq!(app.liked.total, Some(0));

    // S on a radio asks, then makes a playlist of the list on screen.
    app.start_radio(track_by("s", ("ar1", "A"), ("al1", "X")));
    let Some(Page::Radio(r)) = app.page_mut() else { panic!() };
    r.tracks.items = vec![t.clone(), track_by("2", ("c", "C"), ("d", "D"))];
    drain(&rx);
    app.on_key(KeyEvent::from(KeyCode::Char('S')));
    assert!(app.confirm.as_ref().unwrap().text.contains("2 songs"));
    app.on_key(KeyEvent::from(KeyCode::Char('y')));
    let cmds = drain(&rx);
    assert!(
        cmds.iter().any(|c| matches!(c, Cmd::CreatePlaylist { name, uris, .. } if name == "Song s radio" && uris.len() == 2)),
        "{cmds:?}"
    );
}

#[test]
fn page_tabs_are_capped() {
    let (mut app, _rx) = test_app();
    for i in 0..MAX_PAGES + 2 {
        app.open_artist(format!("a{i}"), format!("A{i}"));
    }
    assert_eq!(app.pages.len(), MAX_PAGES);
    assert_eq!(app.pages[0].key(), "artist:a2", "the oldest tabs closed");
    assert_eq!(app.page().map(Page::key).as_deref(), Some("artist:a9"));
    for _ in 0..20 {
        app.on_key(KeyEvent::from(KeyCode::Esc));
    }
    assert_eq!(app.view(), View::Search);
}

#[test]
fn radio_filters_update_the_list() {
    use crate::radio::RadioTrack;
    let (mut app, rx) = test_app();
    app.switch_tab(Tab::Liked);
    let seed = track_by("s", ("ar1", "A"), ("al1", "X"));
    app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: ApiPage { items: vec![seed.clone()], raw: 1, total: 1 } });
    drain(&rx);
    app.on_key(KeyEvent::from(KeyCode::Char('o')));
    assert_eq!(app.view(), View::Radio);
    let cmds = drain(&rx);
    if crate::client::AVAILABLE {
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Radio(u) if u == "spotify:track:s")), "{cmds:?}");
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Library)), "{cmds:?}");
    }
    let rt = |id: &str, artist: &str, explicit: bool| RadioTrack {
        uri: format!("spotify:track:{id}"),
        name: id.into(),
        artists: vec![artist.into()],
        artist_uris: vec![format!("spotify:artist:{artist}")],
        album: String::new(),
        year: Some(2000),
        duration_ms: 1000,
        popularity: 50,
        explicit,
        album_uri: None,
        images: Vec::new(),
    };
    let result = Ok((rt("s", "ar1", false), vec![rt("1", "x", true), rt("2", "y", false), rt("3", "y", false)]));
    app.on_msg(Msg::Radio { seed: "spotify:track:s".into(), result });
    assert_eq!(app.tracks().unwrap().items.len(), 3);

    app.on_key(KeyEvent::from(KeyCode::Char('F')));
    // Fourth entry (after Discover): hide explicit songs; the menu stays open and the list updates.
    app.move_popup(3);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert!(app.menu.is_some());
    assert_eq!(app.tracks().unwrap().items.len(), 2);
    assert!(app.menu.as_ref().unwrap().items[3].1.ends_with(": on"));
    // Songs per artist: at most 2, then at most 1.
    app.move_popup(2);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tracks().unwrap().items.len(), 1);
    app.on_key(KeyEvent::from(KeyCode::Esc));

    // Enter plays the filtered list.
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Play(Play::Tracks { uris, .. }) if uris == &["spotify:track:2"])), "{cmds:?}");
}

#[test]
fn discover_rounds_bring_new_songs() {
    use crate::radio::{Library, RadioTrack};
    let (mut app, rx) = test_app();
    let rt = |id: &str, artist: &str| RadioTrack {
        uri: format!("spotify:track:{id}"),
        name: id.into(),
        artists: vec![artist.into()],
        artist_uris: vec![format!("spotify:artist:{artist}")],
        album: String::new(),
        year: Some(2000),
        duration_ms: 1000,
        popularity: 50,
        explicit: false,
        album_uri: None,
        images: Vec::new(),
    };
    app.start_radio(track_by("s", ("ar1", "A"), ("al1", "X")));
    drain(&rx);
    // Before the radio loads, d only says to wait.
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert!(drain(&rx).is_empty());
    app.on_msg(Msg::Library(Library {
        tracks: ["spotify:track:1".to_owned()].into(),
        artists: ["spotify:artist:liked".to_owned()].into(),
        ..Default::default()
    }));
    let radio = vec![rt("1", "x"), rt("2", "liked"), rt("3", "ar1"), rt("4", "y"), rt("5", "z")];
    app.on_msg(Msg::Radio { seed: "spotify:track:s".into(), result: Ok((rt("s", "ar1"), radio)) });
    assert_eq!(app.tracks().unwrap().items.len(), 5);

    // Round 1: only songs you do not know, and their radios are asked for.
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    let uris = |app: &App| {
        let mut u: Vec<String> = app.tracks().unwrap().items.iter().map(|t| t.uri.clone()).collect();
        u.sort();
        u
    };
    assert_eq!(uris(&app), ["spotify:track:4", "spotify:track:5"]);
    let cmds = drain(&rx);
    let Some(Cmd::RadioMore { radio, seeds }) = cmds.iter().find(|c| matches!(c, Cmd::RadioMore { .. })) else {
        panic!("{cmds:?}")
    };
    assert_eq!(radio, "spotify:track:s");
    let mut seeds = seeds.clone();
    seeds.sort();
    assert_eq!(seeds, ["spotify:track:4", "spotify:track:5"]);
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert!(drain(&rx).is_empty(), "one round at a time");
    let found = vec![rt("4", "y"), rt("6", "liked"), rt("7", "w"), rt("8", "v")];
    app.on_msg(Msg::RadioMore { radio: "spotify:track:s".into(), result: Ok(found) });
    assert_eq!(uris(&app), ["spotify:track:4", "spotify:track:5", "spotify:track:7", "spotify:track:8"]);

    // Round 2: the songs of round 1 go.
    app.on_key(KeyEvent::from(KeyCode::Char('d')));
    assert!(app.tracks().unwrap().items.is_empty());
    app.on_msg(Msg::RadioMore { radio: "spotify:track:s".into(), result: Ok(vec![rt("9", "u")]) });
    assert_eq!(uris(&app), ["spotify:track:9"]);
    let Some(Page::Radio(r)) = app.page() else { panic!() };
    assert_eq!(r.round, 2);

    // A restart keeps discover: the same round and songs once the radio loads again.
    let saved: crate::state::State = serde_json::from_str(&serde_json::to_string(&app.saved_state()).unwrap()).unwrap();
    let (mut again, _rx2) = test_app();
    again.restore(saved);
    again.on_msg(Msg::Library(Library {
        tracks: ["spotify:track:1".to_owned()].into(),
        artists: ["spotify:artist:liked".to_owned()].into(),
        ..Default::default()
    }));
    let radio = vec![rt("1", "x"), rt("2", "liked"), rt("3", "ar1"), rt("4", "y"), rt("5", "z")];
    again.on_msg(Msg::Radio { seed: "spotify:track:s".into(), result: Ok((rt("s", "ar1"), radio)) });
    let Some(Page::Radio(r)) = again.page() else { panic!("the radio page is back") };
    assert!(r.filters.discover);
    assert_eq!(r.round, 2);
    assert_eq!(uris(&again), ["spotify:track:9"]);

    // Reset goes back to the plain radio.
    app.on_key(KeyEvent::from(KeyCode::Char('F')));
    let reset = app.menu.as_ref().unwrap().items.len() - 1;
    app.move_popup(reset as isize);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.tracks().unwrap().items.len(), 5);
}

#[test]
fn add_and_remove_with_picker_and_confirm() {
    let (mut app, rx) = test_app();
    app.on_msg(Msg::User(serde_json::from_value(serde_json::json!({"id": "me", "uri": "spotify:user:me"})).unwrap()));
    let theirs: Playlist = serde_json::from_value(serde_json::json!({
        "id": "t", "uri": "spotify:playlist:t", "name": "Theirs", "owner": {"id": "x"}
    }))
    .unwrap();
    app.on_msg(Msg::Playlists {
        offset: 0,
        page: ApiPage { items: vec![pl("p1", "Gym"), pl("p2", "Chill"), theirs], raw: 3, total: 3 },
    });
    app.tab = Tab::Playlists;
    app.open = Some(OpenPlaylist { playlist: pl("p1", "Gym"), tracks: Listing::default() });
    let t = track_by("1", ("a", "A"), ("b", "B"));
    app.on_msg(Msg::Tracks { list: List::Playlist("p1".into()), offset: 0, page: ApiPage { items: vec![t], raw: 1, total: 1 } });
    drain(&rx);

    app.on_key(KeyEvent::from(KeyCode::Char('e')));
    assert_eq!(app.picker_choices().len(), 2, "only your own playlists");
    for c in "chi".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    assert_eq!(app.picker_choices().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Chill"]);
    app.on_key(KeyEvent::from(KeyCode::Enter));
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::AddToPlaylist { playlist_id, uris, .. } if playlist_id == "p2" && uris == &["spotify:track:1"])), "{cmds:?}");

    // E adds to the same playlist again; it is first in the picker now.
    app.on_key(KeyEvent::from(KeyCode::Char('e')));
    assert_eq!(app.picker_choices()[0].name, "Chill", "the one used last comes first");
    app.on_key(KeyEvent::from(KeyCode::Esc));

    // D removes at once (no question), and u puts it back at its place.
    app.on_key(KeyEvent::from(KeyCode::Char('D')));
    assert!(app.confirm.is_none());
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::RemoveFromPlaylist { playlist_id, uris, .. } if playlist_id == "p1" && uris == &["spotify:track:1"])), "{cmds:?}");
    app.on_msg(Msg::Removed { playlist_id: "p1".into(), uris: vec!["spotify:track:1".into()], text: String::new() });
    assert!(app.open.as_ref().unwrap().tracks.items.is_empty());
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::AddToPlaylist { playlist_id, position: Some(0), .. } if playlist_id == "p1")), "{cmds:?}");
    // Then the add to Chill is undone: the song comes out again.
    app.on_key(KeyEvent::from(KeyCode::Char('u')));
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::RemoveFromPlaylist { playlist_id, .. } if playlist_id == "p2")), "{cmds:?}");
}

#[test]
fn mute_and_unmute() {
    let (mut app, rx) = test_app();
    app.on_msg(playing(35, 1000));
    app.on_key(KeyEvent::from(KeyCode::Char('m')));
    assert!(app.muted());
    app.on_key(KeyEvent::from(KeyCode::Char('m')));
    assert_eq!(app.volume(), Some(35));
    assert!(!app.muted());
    drain(&rx);
}

fn click(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn mouse_selects_plays_and_drags() {
    let (mut app, rx) = test_app();
    app.switch_tab(Tab::Liked);
    app.on_msg(Msg::Tracks { list: List::Liked, offset: 0, page: page(0..20, 20) });
    app.on_msg(playing(50, 200_000));
    app.hits.list = Rect::new(2, 5, 60, 10);
    app.hits.row_height = 1;
    app.hits.list_offset = 4;
    app.hits.volume = Rect::new(70, 30, 11, 1);
    app.hits.progress = Rect::new(10, 29, 101, 1);
    drain(&rx);

    app.on_mouse(click(10, 7));
    assert_eq!(app.liked.selected(), 6, "row 2 of the view + offset 4");
    assert!(drain(&rx).is_empty(), "one click only selects");
    app.on_mouse(click(10, 7));
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::PlayLiked { from }, ..] if from == "spotify:track:6"), "{cmds:?}");
    assert_eq!(app.playing_uri(), Some("spotify:track:6"), "shown at once");
    // Back to a song with a length, for the progress bar below.
    app.switch = None;
    app.on_msg(playing(50, 200_000));

    app.on_mouse(click(10, 14 + 5));
    assert_eq!(app.liked.selected(), 6, "click below the list does nothing");

    app.on_mouse(click(75, 30));
    assert_eq!(app.volume(), Some(50), "middle of the slider");
    app.on_mouse(MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), ..click(80, 30) });
    assert_eq!(app.volume(), Some(100));
    app.on_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), ..click(80, 30) });

    app.on_mouse(click(60, 29));
    assert_eq!(app.progress_ms(), 100_000, "drag preview");
    app.on_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), ..click(60, 29) });
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Seek(100_000))), "{cmds:?}");

    app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, ..click(10, 7) });
    assert_eq!(app.liked.selected(), 9);
}

#[test]
fn typing_keys_go_to_the_query() {
    let (mut app, _rx) = test_app();
    app.on_key(KeyEvent::from(KeyCode::Char('/')));
    for c in "q1 /".chars() {
        app.on_key(KeyEvent::from(KeyCode::Char(c)));
    }
    assert!(!app.quit);
    assert_eq!(app.query, "q1 /");
}

fn poll(pb: Option<Playback>) -> Msg {
    Msg::Playback(pb.map(Box::new))
}

#[test]
fn a_late_poll_does_not_put_the_old_song_back() {
    let (mut app, rx) = test_app();
    app.on_msg(poll(Some(playing_song("spotify:track:old"))));
    app.up_next = Some(track(7));
    drain(&rx);
    app.skip(true);
    assert_eq!(app.playing_uri(), Some("spotify:track:7"), "the next song shows at once");
    // Spotify is late: the poll still has the old song. The new one stays; the next poll is soon.
    app.on_msg(poll(Some(playing_song("spotify:track:old"))));
    assert_eq!(app.playing_uri(), Some("spotify:track:7"));
    assert!(app.next_poll <= Instant::now() + SWITCH_POLL);
    // Spotify agrees: done, and normal polls again.
    app.on_msg(poll(Some(playing_song("spotify:track:7"))));
    assert!(app.switch.is_none());
    assert!(app.next_poll > Instant::now() + Duration::from_secs(2));
    // Another song than the one expected (a phone changed it) is taken at once.
    app.up_next = Some(track(8));
    app.skip(true);
    app.on_msg(poll(Some(playing_song("spotify:track:9"))));
    assert_eq!(app.playing_uri(), Some("spotify:track:9"));
    // A command that failed: the truth comes back with the next poll.
    app.up_next = Some(track(10));
    app.skip(true);
    app.on_msg(Msg::PlayerFailed);
    assert!(app.next_poll <= Instant::now());
    app.on_msg(poll(Some(playing_song("spotify:track:9"))));
    assert_eq!(app.playing_uri(), Some("spotify:track:9"));
    // After the wait, a poll is the truth even if it shows the song before.
    app.up_next = Some(track(11));
    app.skip(true);
    app.switch.as_mut().unwrap().at -= SWITCH_HOLD;
    app.on_msg(poll(Some(playing_song("spotify:track:9"))));
    assert_eq!(app.playing_uri(), Some("spotify:track:9"));
}

#[test]
fn the_end_of_a_song_polls_fast_until_the_next_one_shows() {
    let (mut app, rx) = test_app();
    let mut pb = playing_song("spotify:track:a");
    pb.progress_ms = Some(200_000);
    app.on_msg(poll(Some(pb.clone())));
    app.poll_pending = false;
    drain(&rx);
    app.tick();
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Poll)), "polls at the end");
    app.on_msg(poll(Some(pb)));
    assert!(app.next_poll <= Instant::now() + SWITCH_POLL, "and again soon");
    app.on_msg(poll(Some(playing_song("spotify:track:b"))));
    assert_eq!(app.playing_uri(), Some("spotify:track:b"));
    assert!(app.switch.is_none());
}

#[test]
fn the_last_song_stays_when_spotify_has_none() {
    let (mut app, rx) = test_app();
    app.on_msg(poll(Some(playing_song("spotify:track:a"))));
    drain(&rx);
    // Spotify dropped the idle session: the song stays, paused.
    app.on_msg(poll(None));
    assert_eq!(app.playing_uri(), Some("spotify:track:a"));
    assert!(app.ghost);
    assert!(!app.playback.as_ref().unwrap().is_playing);
    // Space starts it again where it was, and shows it playing at once.
    app.on_key(KeyEvent::from(KeyCode::Char(' ')));
    let cmds = drain(&rx);
    assert!(matches!(cmds.as_slice(), [Cmd::Resume(sp)] if sp.track == "spotify:track:a" && sp.playing), "{cmds:?}");
    assert!(app.playback.as_ref().unwrap().is_playing);
    // A poll before Spotify has it does not stop it on screen.
    app.on_msg(poll(None));
    assert!(app.playback.as_ref().unwrap().is_playing);
    app.on_msg(poll(Some(playing_song("spotify:track:a"))));
    assert!(!app.ghost && app.switch.is_none());
}

#[test]
fn the_song_of_the_last_run_shows_at_start_and_starts_again() {
    let (mut app, rx) = test_app();
    let mut song = track(5);
    song.duration_ms = 180_000;
    let state = crate::state::State {
        playback: Some(SavedPlayback {
            track: song.uri.clone(),
            name: song.name.clone(),
            context: None,
            position_ms: 60_000,
            playing: true,
            shuffle: false,
            repeat: "off".into(),
            volume: None,
            item: Some(song),
        }),
        ..Default::default()
    };
    app.restore(state);
    // Before any answer from Spotify: the song, paused, at its place.
    assert_eq!(app.playing_uri(), Some("spotify:track:5"));
    assert_eq!(app.progress_ms(), 60_000);
    assert!(app.ghost);
    drain(&rx);
    // Spotify has nothing, and no device yet: it looks for the device.
    app.on_msg(Msg::Device(None));
    app.on_msg(poll(None));
    app.tick();
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::FindDevice)));
    assert!(app.toast.as_ref().unwrap().text.starts_with("Starting where you left off"));
    assert!(app.busy(), "the loop wakes often while it waits");
    // Not again before the answer; soon after it.
    app.tick();
    assert!(!drain(&rx).iter().any(|c| matches!(c, Cmd::FindDevice)));
    app.on_msg(Msg::Device(None));
    app.find_device_at = Some(Instant::now() - FIND_EVERY);
    app.tick();
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::FindDevice)));
    assert_eq!(app.playing_uri(), Some("spotify:track:5"), "still shown");
    // spotifyd shows up: the song starts again, and polls come fast until it plays.
    app.on_msg(Msg::Device(Some("spotlite".into())));
    app.poll_pending = false;
    app.tick();
    let cmds = drain(&rx);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Resume(sp) if sp.position_ms == 60_000)), "{cmds:?}");
    assert!(app.switch.is_some());
    // The state saved now has the song itself, for the next start.
    assert!(app.saved_state().playback.unwrap().item.is_some());
}

#[test]
fn spotifyd_events_show_a_new_song_at_once() {
    let (tx, rx) = std::sync::mpsc::channel();
    let (ctl_tx, _ctl_rx) = std::sync::mpsc::channel();
    let mut app = App::new(tx, true, Images::disabled()).with_control(ctl_tx, "spotlite".into());
    app.on_msg(poll(Some(playing_song("spotify:track:a"))));
    app.up_next = Some(track(3));
    drain(&rx);
    app.on_msg(Msg::Mpris { playing: true, track: Some(Box::new(track(3))) });
    assert_eq!(app.playing_uri(), Some("spotify:track:3"));
    assert_eq!(app.progress_ms(), 0);
    // A pause from a media key shows at once too.
    app.on_msg(poll(Some(playing_song("spotify:track:3"))));
    app.on_msg(Msg::Mpris { playing: false, track: Some(Box::new(track(3))) });
    assert!(!app.playback.as_ref().unwrap().is_playing);
    // Another device plays: spotifyd's old state does not count.
    let mut phone = playing_song("spotify:track:p");
    phone.device.as_mut().unwrap().name = "Phone".into();
    app.on_msg(poll(Some(phone)));
    app.on_msg(Msg::Mpris { playing: false, track: Some(Box::new(track(3))) });
    assert_eq!(app.playing_uri(), Some("spotify:track:p"));
}

#[test]
fn a_drag_ends_wherever_the_button_is_let_go() {
    let (mut app, rx) = test_app();
    app.on_msg(playing(50, 200_000));
    app.hits.progress = Rect::new(10, 29, 101, 1);
    app.hits.side = Rect::new(0, 2, 30, 20);
    app.sidebar.open = true;
    drain(&rx);
    app.on_mouse(click(60, 29));
    // Dragged over the sidebar, and let go there.
    app.on_mouse(MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), ..click(10, 10) });
    app.on_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), ..click(10, 10) });
    assert!(app.drag.is_none() && app.seek_preview.is_none());
    assert!(drain(&rx).iter().any(|c| matches!(c, Cmd::Seek(0))));
    // A release outside the window is lost: the next click forgets the drag.
    app.on_mouse(click(60, 29));
    app.on_mouse(click(10, 10));
    assert!(app.drag.is_none() && app.seek_preview.is_none());
}

#[test]
fn buttons_light_up_under_the_pointer() {
    let (mut app, _rx) = test_app();
    app.on_msg(playing(50, 200_000));
    app.hits.play = Rect::new(20, 30, 2, 1);
    app.hits.np_artist = Rect::new(20, 28, 8, 1);
    assert!(app.on_hover(Position::new(21, 30)));
    assert_eq!(app.hot_at(Position::new(21, 30)), Some((app.hits.play, Hot::Control)));
    assert!(!app.on_hover(Position::new(20, 30)), "the same button: no new frame");
    assert_eq!(app.hot_at(Position::new(22, 28)).map(|h| h.1), Some(Hot::Link));
    assert!(app.on_hover(Position::new(50, 10)));
    assert_eq!(app.hot_at(Position::new(50, 10)), None);
}

#[test]
fn a_song_played_30_s_goes_to_the_top_of_recent() {
    let (mut app, _rx) = test_app();
    app.switch_tab(Tab::Recent);
    app.recent.apply(0, page(0..3, 3));
    app.recent.state.select(Some(1));
    let mut pb = playing_song("spotify:track:9");
    pb.progress_ms = Some(10_000);
    app.on_msg(poll(Some(pb.clone())));
    assert!(!app.note_listen(), "not 30 s yet");
    pb.progress_ms = Some(31_000);
    app.on_msg(poll(Some(pb.clone())));
    assert!(app.note_listen(), "the list on screen changed");
    let uris: Vec<&str> = app.recent.items.iter().map(|t| t.uri.as_str()).collect();
    assert_eq!(uris, ["spotify:track:9", "spotify:track:0", "spotify:track:1", "spotify:track:2"]);
    assert_eq!(app.recent.selected(), 2, "the selection stays on its song");
    // Once per play; from the start again, it counts again.
    pb.progress_ms = Some(60_000);
    app.on_msg(poll(Some(pb.clone())));
    assert!(!app.note_listen());
    pb.progress_ms = Some(1_000);
    app.on_msg(poll(Some(pb.clone())));
    app.note_listen();
    pb.progress_ms = Some(40_000);
    app.on_msg(poll(Some(pb.clone())));
    assert!(app.note_listen());
    assert_eq!(app.recent.items.len(), 5);
    // A song on another device (a phone) is in Spotify's own list.
    let (mut app, _rx) = test_app();
    let (tx, _crx) = std::sync::mpsc::channel();
    app = app.with_control(tx, "spotlite".into());
    app.recent.apply(0, page(0..1, 1));
    let mut pb = playing_song("spotify:track:8");
    pb.device.as_mut().unwrap().name = "Phone".into();
    pb.progress_ms = Some(40_000);
    app.on_msg(poll(Some(pb)));
    app.note_listen();
    assert_eq!(app.recent.items.len(), 1);
}
