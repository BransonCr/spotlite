# spotlite

A small Spotify client for the terminal. It replaces the official Electron app
(300+ MB of RAM) with two light parts:

| Part | Job | Idle RAM |
|------|-----|----------|
| [`spotifyd`](https://github.com/Spotifyd/spotifyd) | Spotify Connect daemon (librespot). Plays the audio. | ~10–30 MB |
| `spotlite` | Terminal UI. Searches, browses, and controls playback through the Spotify Web API. | ~7 MB, 3 threads, ~0.3% CPU |

`spotlite` is a small Rust binary (~4.3 MB release build).

On Windows, see [README-windows.md](README-windows.md). The Windows build plays audio itself
and does not need spotifyd.

## Requirements

- Spotify **Premium**. spotifyd needs it, and since February 2026 every
  Web API app in Development Mode needs the app owner to have Premium.
- `spotifyd` (`pacman -S spotifyd`) and a Rust toolchain.

## Setup

1. Create an app at <https://developer.spotify.com/dashboard>.
   - Redirect URI: `http://127.0.0.1:8898/callback`
   - API: **Web API**
   - Copy the **Client ID**. No client secret is needed (spotlite uses PKCE).
2. Write `~/.config/spotlite/config.toml`:

   ```toml
   client_id = "your-client-id"
   # device_name = "spotlite"   # must match spotifyd's device_name
   # redirect_port = 8898
   ```

3. Configure spotifyd, log in once in the browser, and start it
   (run from the repo folder):

   ```sh
   cd ~/projects/spotlite
   mkdir -p ~/.config/spotifyd
   cp contrib/spotifyd.conf ~/.config/spotifyd/spotifyd.conf
   spotifyd authenticate
   systemctl --user enable --now spotifyd
   ```

   If spotifyd was already running, restart it after you log in:
   `systemctl --user restart spotifyd`.

4. Install spotlite and start it:

   ```sh
   cargo install --locked --path .
   spotlite
   ```

   The first time, spotlite shows a login screen. Press `Enter` to log in with
   Spotify in the browser; spotlite starts when the browser comes back. The login
   is saved in `~/.cache/spotlite/token.json` and refreshed by itself, so the login
   screen comes back only if Spotify ends the login (for example, if you remove
   the app's access in your Spotify account). `spotlite auth` still does the same
   login from a shell, and `spotlite devices` lists the Spotify Connect devices.

## Keys

Press `?` in the app for this list. `.` (or a right-click) opens every action for the
selected song, so you do not need to remember the keys below.

| Key | Action |
|-----|--------|
| `/` | Search (Enter to run, Esc to stop typing, `Ctrl+U` to clear) |
| `t` | In Search: list songs, artists, albums or playlists |
| `1` … `9`, `Tab` | Go to a tab: Search / Playlists / Liked / Recent, then the open pages |
| `H` / `L` | Previous / next tab |
| `x` | Close the artist, album or radio tab you are on |
| `j` `k`, arrows, `g` `G`, `Ctrl+D` `Ctrl+U` | Move (more pages load as you scroll) |
| `Esc`, `h` | Back: close a playlist or go up a folder, else go to the tab you were on before |
| `c` | Jump to the playing song (opens the list it plays from if it is not this one) |
| `Enter` | Play the selected song, or open a playlist, album or folder |
| `P` | Play the whole playlist or album |
| `.` | Action menu for the selected song |
| `a` | Add the selected song to the queue |
| `i` / `b` | Go to the song's artist / album |
| `o` | Song radio from the selected song (or the selected radio on an artist page) |
| `Tab` / `Shift+Tab` | Next / previous section on an artist page |
| `F` | Radio filters (on a radio page) |
| `d` | Discover: new songs outside your taste (on a radio page; again for a new round) |
| `S` | Save the radio list on screen as a new playlist (asks first) |
| `e` / `E` | Add the selected song to one of your playlists / to the last one again |
| `D` | Remove the selected song from the open playlist (`u` puts it back) |
| `Ctrl+F` | Filter the list as you type (Esc clears it) |
| `,` / `;` | Sort the list by title, artist, album, time / turn the sort round |
| `w` / `W` | Mark the song / mark all: the next action is for every marked song |
| `u` | Undo the last change: a remove, an add, a like, an unpin |
| `y` | Play on another device (lists your Spotify Connect devices) |
| `Space` | Play / pause (starts spotifyd if nothing is active) |
| `n` / `p` | Next / previous |
| `←` / `→` | Seek 10 seconds |
| `+` / `-`, `]` / `[`, `Shift+↑` / `Shift+↓` | Volume |
| `m` | Mute / unmute |
| `s` | Shuffle |
| `r` | Repeat: off, all, one |
| `f` | Like / unlike the selected song (else the playing one) |
| `T` | Theme picker (moving previews, Enter keeps, Esc goes back) |
| `:`, `Ctrl+P` | Command palette: type what you want to do, Enter runs it |
| `v` | Sound visualizer on / off |
| `R` | Reload the list |
| `q` | Quit (music keeps playing) |
| `Q` | Quit and stop the player (spotifyd): nothing keeps running (spotlite starts it again next time) |

Every key can be changed in `config.toml`, and `spotlite keys` lists the actions with
their names and keys:

```toml
[keys]
next = "N"                  # one key
like = ["f", "ctrl-l"]      # or several
mute = ""                   # or none
```

A mistake (an unknown name, a key set twice) shows when spotlite starts. Help (`?`) shows
the keys in use; type in it to filter, `↑` `↓` to scroll.

### Lists: filter, sort, marks, undo

- `Ctrl+F` filters the list shown as you type: songs by title, artist and album, playlists
  by name and owner. The match is underlined. Enter keeps the filter and goes back to the
  list; Esc clears it. A list that is not fully loaded loads the rest first, so nothing is
  missed.
- `,` sorts by title, then artist, album and time, then the list's own order; `;` turns it
  round. Enter in a filtered or sorted list plays the songs in the order they show.
- `w` marks a song (◆) and moves down; `W` marks all of them, or none. Queue, add to a
  playlist, remove and like then act on every marked song. Esc clears the marks.
- Remove does not ask first: `u` puts the songs back at their places. `u` also undoes an
  add, a like or an unlike, and an unpin.
- A playlist you open again comes back as you left it: the same place, filter and sort.
- Text boxes (search, palette, pickers, filter) have a cursor: `←` `→`, Home/End, `Ctrl+A`
  `Ctrl+E`, `Ctrl+←` `→` by word, `Ctrl+W`/`Ctrl+⌫` delete a word, `Ctrl+U` `Ctrl+K`, and
  paste. In the search box, `↑` `↓` go through earlier searches.
- The palette (`:`) also finds your playlists, earlier searches, themes and genres by name.

### Search and Recent

`t` changes what the search lists: **Songs**, **Artists**, **Albums** or **Playlists** (or
click one in the list title). Enter on an artist or album opens its page; on one of your
playlists it opens the playlist; someone else's playlist opens too: its songs come over
Spotify's client protocol (the 2026 dev-mode Web API lists only your own).

**Recent** (`4`) is the last 50 songs you played. It loads again when you come back to it
after a new song or after a minute.

### Artists, albums, radio

Each artist, album and radio opens in its own tab, after Search, Playlists, Liked and
Recent. Go to Liked and back with `Esc`, and the radio is still there. Opening the same one
again goes to its tab. `x` closes a tab; with more than 8 open, the oldest closes. Everything comes back the next time spotlite starts: the open tabs (each one loads when
you go to it), the tab you were on, the last search, the visualizer, and what was playing.
If nothing plays when spotlite starts (for example after `Q` or a reboot), it starts the
same song again at the same point, from the same playlist or album, with the same shuffle,
repeat and volume (paused, if it was paused). The state is saved in
`~/.local/state/spotlite/state.json` every few seconds when it changes, and when spotlite
quits or its window closes.

- **Artist page** (`i`): picture, then sections: **Popular** (10 songs), **Radio** (the
  artist's radio and song radios from the top 3 songs), **Albums**, **Singles and EPs**
  (up to 30 releases). `Tab` / `Shift+Tab` jump between sections. The 2026 dev-mode API
  has no top-tracks, related-artists or genres, so the popular songs come from a search
  for the artist's tracks.
- **Album page** (`b`, or Enter on an album): cover, year, tracks; Enter plays the album
  from that song, `P` plays it from the start.
- **Song radio** (`o`): Spotify's own "song radio" (about 50 songs), read over the client
  protocol like the folders. The artist page also has the artist's radio. Spotify shapes it by your listening, and no API offers a
  neutral radio any more, so the filters (`F`) are how to step outside your taste:
  hide songs or artists already in Liked Songs, hide explicit songs, hide the seed's
  artist, at most 1 or 2 songs per artist, songs from near the seed's year, popular
  songs only or deep cuts only. The list updates as you change a filter.
- **Discover** (`d`, or the first entry in `F`): hides every song in your Liked Songs,
  every artist in it, your top artists, your top and recently played songs, and the
  seed's artist; adds the radios of 4 songs it found; and shuffles. Press `d` again for a
  new round: the songs you saw go, the radios of 4 new finds come in, and it shuffles
  again. `F`, then Reset, goes back to the plain radio. `S` saves the list on screen as a
  new private playlist, so a good round is not lost when you close the tab. Top and
  recent songs need the `user-top-read` and `user-read-recently-played` permissions: if
  you logged in with an older spotlite, run `spotlite auth` again (without them, discover
  knows Liked Songs only).
- **Playlists**: `e` adds a song to one of your playlists (type to narrow the list);
  `D` removes it from the open playlist. These need the `playlist-modify-*` permissions:
  if you logged in with an older spotlite, run `spotlite auth` again.

### Mouse

- Click a tab (an open page too), the search box, `‹` (back), or a player button (`◀◀ ▶ ▶▶`, `⇄`, `↻`, `♥`).
- In the player, click the artist name to open the artist, or the cover to open the album.
- Right-click a row for its action menu; it opens at the mouse.
- Click the heart in the player to like or unlike the playing song.
- Click a row to select it; double-click to play it or open the playlist. The row under the
  pointer brightens.
- Drag the scrollbar to move through a long list.
- Click the song's length in the player to see the time left instead.
- Wheel scrolls the list. Over the progress or volume bar, the wheel seeks or changes the volume.
- Click or drag the progress bar to seek, and the volume slider to set the volume.

### Themes

`T` opens the theme picker: moving previews, Enter keeps, Esc goes back. Each of the seven
built-in themes is a different design, not only different colours:

| Theme | Idea | What makes it different |
|-------|------|-------------------------|
| **amber** (default) | Analog studio console | rounded panels, a knob on the progress bar, a VU-meter volume, a smooth wave visualizer |
| **rose-pine** | Dreamy, floating | no lines at all, lowercase titles, pill tabs, star-dust visualizer, a drifting `∿` wave bar |
| **gruvbox** | Retro 8-bit hardware | player on top, thick frames, UPPERCASE `[BRACKET]` tabs, inverse selection, chunky equalizer bars |
| **nord** | Calm, minimal | thin rules instead of boxes, a big cover column on wide terminals, `❯` selection, a mirrored aurora |
| **catppuccin** | Playful pastel | frames with a mauve-to-pink gradient, pill tabs, rainbow equalizer bars, a pulsing mark |
| **tokyo-night** | Neon cyberpunk | glowing frames, powerline tabs, S P A C E D titles, a big cover column, a neon horizon |
| **mono** | Paper and print | newspaper double rules, inverse selection, dotted track-list leaders, no visualizer, no motion |

Animations (the playing mark, the glow, the moving wave) run only while music plays, at 4
frames a second. `motion = false` in a theme, or `SPOTLITE_NO_MOTION=1`, turns them off.

A theme is one commented TOML file (see [`themes/`](themes/)); every key has a comment with
its choices. Your own themes go in `~/.config/spotlite/themes/<name>.toml`; a file with the
name of a built-in theme replaces it. Keys you leave out take the amber value, and a typo or
a bad value is an error that names the choices. A running spotlite reloads the theme when
you save its file, so you can edit a theme and watch it change.

| Key | Choices |
|-----|---------|
| `text` `bright` `muted` `faint` `border` `selected` `accent` `accent2` `red` `green` `love` `viz_low` `viz_high` `viz_line` | colours, `"#rrggbb"` (`love` is the like heart) |
| `layout` | `classic` (player at the bottom), `stage` (big cover column on terminals 140+ wide), `top` (player on top) |
| `frame` / `frame_fx` / `corners` | `full`, `rule`, `none` / `plain`, `gradient`, `glow` / `rounded`, `plain`, `thick`, `double` |
| `titles` | `normal`, `lower`, `upper`, `spaced` |
| `header` / `nerd` | `plain`, `pills`, `brackets`, `powerline` / `true` (default): Nerd Font arrows and hearts, built into ghostty and kitty; `false` for plain `▌` and `♥` |
| `select` / `marker` | `bar`, `block`, `underline`, `arrow` / one character |
| `viz` | `wave`, `bars`, `mirror`, `dots`, `none` |
| `progress` / `bar` | `line`, `knob`, `blocks`, `wave` / two characters, filled then empty |
| `volume` | `bar`, `leds` |
| `playing` / `logo` / `bullet` | `eq`, `spin`, `pulse`, `logo` / one character / one character |
| `leaders` / `motion` | `true` or `false` |

The same from a shell, for scripts and AI agents:

```
spotlite theme                   list themes (* = in use), with any broken ones
spotlite theme set nord          use nord (a running spotlite changes within a second)
spotlite theme file nord         print nord's file, copying the built-in one there first
spotlite theme new dusk nord     make "dusk" from nord, print its file
spotlite theme check dusk        report mistakes in a theme file
SPOTLITE_THEME=gruvbox spotlite  try a theme for one run (this run saves no tabs or theme)
```

### Playlist folders

The Playlists tab shows your folders as you organised them in Spotify (Enter opens a
folder, Esc goes up). The Web API has no folders, so spotlite reads them from your
account's "rootlist" over Spotify's client protocol (librespot), using spotifyd's saved
login in `~/.cache/spotifyd/oauth/credentials.json` (read only). This happens once in a
background thread at start; the tree is cached in `~/.cache/spotlite/folders.json`.
`spotlite folders` prints the tree. Build with `--no-default-features` to leave this out
(about 1.4 MB smaller); the tab is then a flat list.

### Sound visualizer

`v` draws the spectrum of what spotifyd plays as dim steel-blue bars behind the main
panel, only in empty cells, so text stays readable. `parec` records spotifyd's own
stream (22 kHz mono); a built-in FFT turns it into 48 bands, eased and blurred into a
soft wave, with a thin braille line in a lighter steel tracing it. Off by default.
It runs only while it is on and the music plays; otherwise no
capture runs and the UI goes back to one redraw per second.
Cost while running (20 fps): about 0.5% CPU in spotlite and a few percent in the terminal,
which redraws the changed cells. `SPOTLITE_VIZ=1` starts with it on;
`SPOTLITE_VIZ_FPS` sets the frame rate (lower = cheaper).


### Sidebar aura

`V` picks an effect for the sidebar's Playing view: off, the aura, or the spectrum on its
side. The aura is light that comes off the cover of the song. Its colours are the cover's
(8 × 8 points of the picture, faded from one cover to the next), it starts at the cover's
edges, and the music moves it: each band has a direction from the cover, and a bass beat
sends a pulse out. Each theme has its own form, set with `aura` in the theme file:

| `aura` | Theme | What it shows |
| --- | --- | --- |
| `phosphor` | amber | an oscilloscope in amber phosphor: a grid over the pane and a beam that draws a figure of the music (each song its own), with a glow behind it; a bass beat kicks the figure and a flash runs over the grid |
| `grooves` | | thin rings like a record's grooves, bent by the spectrum |
| `stardust` | rose-pine | dust that drifts off the cover, with stars that flash on treble |
| `pixels` | gruvbox | square lights, a level meter that goes out from the cover |
| `aurora` | nord | curtains of light; their hem is the spectrum |
| `ribbons` | catppuccin | three ribbons for bass, middle and treble |
| `traces` | tokyo-night | wires from the cover, with pulses of light on them |
| `halftone` | mono | a print of dots, larger where the music is loud |

While music plays the light fills the whole pane, to its bottom edge; when the music stops
it goes back to the cover.

It draws glyphs only, in empty cells and never between two words, so text stays readable
and the terminal's background shows through. With `motion = false` it does not move in
time; it follows only the levels of the music. `python3 tools/aura-preview.py` draws
pictures of every form without a terminal (see the file's first lines).

### Cover art

Playlists show their cover in each row, track lists show a small album thumbnail,
and the player shows the album art.
In ghostty and kitty the pictures use the kitty graphics protocol; other terminals
get sixel or iTerm2 pictures, or coloured half blocks. A third thread downloads,
scales and encodes each picture once; up to 300 are kept in memory.
Set `SPOTLITE_NO_IMAGES=1` to turn pictures off.

## How it works

- The UI thread only draws and reads keys. Two more threads make the Web API calls: one
  for player commands and polls, one for lists and pages, so play, pause and skip never
  wait behind a long load.
- spotlite asks Spotify as little as it can (Development Mode apps are rate limited hard):
  - The player state is read every 8 s while music plays, 20 s while paused, 30 s with
    nothing loaded, and at once after a command or at the end of a song. The progress bar
    moves locally in between.
  - A song change costs 0 or 1 requests: the ♥ comes from the Liked Songs index, and
    "up next" walks the 20-song queue Spotify sent before.
  - Saved copies in `~/.cache/spotlite`: Liked Songs (all of it, brought up to date with
    usually 1 request), playlists (used while their snapshot is the same), albums, artist
    pages (24 h), the playlists list (10 min), cover pictures, the prepicked genre samples
    and how deep each genre's search goes. `R` asks Spotify again.
  - Playlist songs, song radio, discover and the folder tree come over Spotify's client
    protocol, which does not count against the Web API's limits. One connection serves
    calls that come close together.
  - Liking, adding and removing change the lists in place instead of loading them again.
  - `SPOTLITE_LOG=/tmp/spotlite.log` writes one line per request (and slow frames), to see
    what spotlite asks.
- When spotifyd is the active device, play/pause, skip, seek and volume go to it over
  MPRIS (`playerctl`, about 8 ms) and the screen changes at once; a Web API read 0.5 s
  later confirms it. Other devices use the Web API (about 250 ms per call).
- Lists draw only the rows on screen, so a list of 10 000 songs draws as fast as one of
  20. When nothing moves, the UI sleeps until the next thing is due.
- No network: the header says offline and the poll waits longer each time; one message
  says when it is back.
- Liked Songs and your own playlists play as a context, so playback continues
  past the tracks that are loaded.
- If spotifyd restarts, spotlite finds the device again and retries once.
  On a rate limit (429), it waits for the time Spotify asks for. A play command with no
  device starts spotifyd and waits for it.
- Terminals without 24-bit colour get the nearest 256-colour palette colours
  (`SPOTLITE_COLORS=256` or `truecolor` decides by hand); `NO_COLOR` uses the mono theme.
- A crash leaves a note in `~/.local/state/spotlite/crash.log`.

## Commands

```
spotlite            terminal UI
spotlite auth       log in (token in ~/.cache/spotlite/token.json, mode 600; also the built-in player)
spotlite daemon     the built-in player (spotlite starts it by itself)
spotlite devices    list Spotify Connect devices
spotlite folders    print your playlist folders
spotlite theme ...  list, set, copy, make and check themes (see Themes)
spotlite keys       list every action, its name for [keys], and its keys
spotlite --version  the version
spotlite toggle     play/pause
spotlite next       next track
spotlite prev       previous track
```

## Desktop integration

spotifyd has MPRIS (`use_mpris = true`), so media keys, `playerctl`, and the
waybar `mpris` module work with no extra process:

```jsonc
// waybar
"mpris": {
  "player": "spotifyd",
  "format": "{status_icon} {title} - {artist}",
  "status-icons": { "playing": "▶", "paused": "⏸" }
}
```

```ini
# Hyprland
bind = , XF86AudioPlay, exec, playerctl -p spotifyd play-pause
bind = , XF86AudioNext, exec, playerctl -p spotifyd next
bind = , XF86AudioPrev, exec, playerctl -p spotifyd previous
```

## Built-in player

spotlite can also play audio itself instead of using spotifyd. `spotlite daemon` is a
librespot Spotify Connect device configured like `contrib/spotifyd.conf`. Windows builds
always use it; on Linux, build with `--features player` (needs the ALSA headers) and enable it
in `config.toml`:

```toml
backend = "builtin"     # "spotifyd" (Linux default) or "builtin" (Windows default)
# daemon_port = 8899    # local control socket on 127.0.0.1
```

- spotlite starts the player in the background when needed. `q` leaves it running, `Q` stops
  it. Only one instance runs at a time.
- The player logs in once through the browser (`spotlite auth` repeats it). The login is
  stored in `~/.cache/spotlite/librespot/` and is also used for playlist folders and song radio.
- Commands, song changes and visualizer audio go over the local socket, about as fast as
  spotifyd over MPRIS. Connections must present the token in
  `~/.local/state/spotlite/daemon.token` (mode 600).
- Media keys work through MPRIS under the name `spotlite` (`playerctl -p spotlite`, waybar
  `"player": "spotlite"`).
- Log: `~/.local/state/spotlite/daemon.log`.

## Windows

See [README-windows.md](README-windows.md) for installing and using spotlite on Windows. It
is also the README inside the release zip.

The Windows build comes from the same code: only `src/platform/` and `src/engine/` differ. It
is released as occasional snapshots tagged `win-v*`. [docs/windows.md](docs/windows.md)
explains how snapshots are tested and released.

## Web API limits (Development Mode, 2026)

- 5 users per app, owner must have Premium.
- Search returns 10 results at most.
- Playlist tracks are only readable for playlists you own or collaborate on.
  spotlite reads the others over the client protocol instead.
- Liking songs needs the `user-library-modify` permission. If you logged in
  with an older spotlite, run `spotlite auth` again.
