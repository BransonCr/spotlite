# Changelog

## Unreleased

### Windows and a built-in player
- Windows support (Windows Terminal), released as snapshots tagged `win-v*` with a zip of
  `spotlite.exe`. See `README-windows.md` and `docs/windows.md`.
- Built-in player (`spotlite daemon`): librespot plays the audio, so spotifyd isn't needed.
  Default on Windows; on Linux use `--features player` and `backend = "builtin"`. It runs in
  the background, keeps playing after `q` and stops with `Q`. Fast controls, song changes and
  the visualizer work as with spotifyd, and media keys work through MPRIS on Linux and the
  media flyout on Windows.
- The first run asks for the Client ID and writes `config.toml`.
- New `config.toml` keys: `backend`, `daemon_port`, `nerd_font` (off by default on Windows).

### Looks
- The sidebar effect Liquid (`V`) is gone. In its place: Aura, light that comes off the cover
  of the playing song, in the cover's colours, and moves to the music. Each theme has its own
  form: grooves (amber), stardust (rose-pine), pixels (gruvbox), aurora (nord), ribbons
  (catppuccin), traces (tokyo-night), halftone (mono). The colours fade from one cover to the
  next. The aura draws glyphs only, never over text or between words.
- The aura fills the whole sidebar pane, to its bottom edge, while music plays. When the
  music stops it goes back to the cover.
- Theme key `aura` takes the place of `liquid`. A theme file with `liquid` still loads.

### Playback feels instant
- The song of the last run shows at start (cover, sidebar, player), paused at its place,
  before Spotify answers. When Spotify drops an idle session, the song stays too. Space starts
  it again where it was.
- A new song shows the moment you play it, skip, or it starts by itself. When Spotify is
  late, polls come every 0.4 s until it agrees, instead of 8 s later. A failed command
  shows the truth again at once.
- spotifyd's own player events (MPRIS, through `playerctl --follow`) show song changes and
  play/pause from media keys and phones at once.
- Answers from Spotify show at once (the UI no longer waits for its next frame to read them).
  Coming back to the window checks the player.
- At start, spotlite looks for spotifyd again every 2 s while it starts, so the last song
  resumes even when spotifyd was not running.

### Mouse
- The row in the open-tabs list stays lit while the pointer is on its ×; the × turns red.
- Buttons, tabs, sliders, sidebar rows and menu rows light up under the pointer; the artist
  name is underlined.
- A drag of the progress bar, the volume or the scrollbar always ends when you let go, also
  over the sidebar or outside the window (before, the progress bar could freeze).

## 0.2.0

### Find and act faster
- Filter any list as you type (`Ctrl+F`); the match is underlined, and the whole list loads
  so nothing is missed.
- Sort by title, artist, album or time (`,`; `;` turns it round). Filtered and sorted
  lists play in the order they show.
- Mark songs (`w`, `W` for all) and queue, add, remove or like all of them at once.
- Undo (`u`) a remove, an add, a like or an unpin. Remove no longer asks first.
- `E` adds to the last playlist again; the picker lists recent playlists first and skips
  songs a loaded playlist already has.
- `c` opens the list the playing song comes from and selects it.
- Earlier searches with `↑` `↓` in the search box; the palette also finds playlists,
  earlier searches, themes and genres.
- `y` moves playback to another Spotify Connect device.
- Every key can be changed under `[keys]` in `config.toml`; `spotlite keys` lists them.
  Help filters as you type and scrolls.
- Text boxes have a cursor, word moves and deletes, and paste.
- Other people's playlists open and list their songs.

### Fewer requests, faster everywhere
- Polls every 8 s while playing, 20 s paused, 30 s idle (was 5 s always).
- A song change costs 0 or 1 requests (was 2); likes, adds and removes change lists in
  place.
- Saved copies of Liked Songs, playlists, albums, artist pages and the playlists list;
  playlist songs over the client protocol.
- Player commands have their own thread; covers load on 3 threads, newest first.
- Lists draw only the rows on screen; the UI sleeps when nothing moves.
- Calm offline state, retries on server errors, crash notes in `crash.log`.

### Looks
- Placeholder rows while a list loads; a dimmed backdrop behind pop-ups; song lists show
  their total length; an error is not hidden by the next message.
- 256-colour fallback, and `NO_COLOR` support.
- Sidebar effects (`V`): a liquid of 48 drops that move to the music, one look per theme,
  or the spectrum on its side. Genre picker with samples (`z`, then Tab).

## 0.1.0
- First version: search, playlists with folders, Liked Songs, artist and album pages, song
  radio with filters and discover, true shuffle, themes, visualizer, sidebar, share cards.
