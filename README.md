# spotlite

A small Spotify client for the terminal. It replaces the official Electron app
(300+ MB of RAM) with two light parts:

| Part | Job | Idle RAM |
|------|-----|----------|
| [`spotifyd`](https://github.com/Spotifyd/spotifyd) | Spotify Connect daemon (librespot). Plays the audio. | ~10–30 MB |
| `spotlite` | Terminal UI. Searches, browses, and controls playback through the Spotify Web API. | ~7 MB, 3 threads, ~0.3% CPU |

`spotlite` is a small Rust binary (~4.3 MB release build).

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

4. Install spotlite, log in, and check that the `spotlite` device is listed:

   ```sh
   cargo install --locked --path .
   spotlite auth
   spotlite devices
   spotlite
   ```

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
| `c` | Jump to the playing song in the list |
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
| `e` | Add the selected song to one of your playlists |
| `D` | Remove the selected song from the open playlist (asks first) |
| `Space` | Play / pause (starts spotifyd if nothing is active) |
| `n` / `p` | Next / previous |
| `←` / `→` | Seek 10 seconds |
| `+` / `-`, `]` / `[`, `Shift+↑` / `Shift+↓` | Volume |
| `m` | Mute / unmute |
| `s` | Shuffle |
| `r` | Repeat: off, all, one |
| `f` | Like / unlike the selected song (else the playing one) |
| `T` | Theme picker (moving previews, Enter keeps, Esc goes back) |
| `v` | Sound visualizer on / off |
| `R` | Reload the list |
| `q` | Quit (music keeps playing) |
| `Q` | Quit and stop spotifyd: nothing keeps running (spotlite starts it again next time) |

### Search and Recent

`t` changes what the search lists: **Songs**, **Artists**, **Albums** or **Playlists** (or
click one in the list title). Enter on an artist or album opens its page; on one of your
playlists it opens the playlist; on someone else's playlist it plays it (the 2026 dev-mode
API lists songs only of your own playlists).

**Recent** (`4`) is the last 50 songs you played, loaded again each time you go to it.

### Artists, albums, radio

Each artist, album and radio opens in its own tab, after Search, Playlists, Liked and
Recent. Go to Liked and back with `Esc`, and the radio is still there. Opening the same one
again goes to its tab. `x` closes a tab; with more than 8 open, the oldest closes. The open
tabs come back the next time spotlite starts (each one loads when you go to it); they are
saved in `~/.local/state/spotlite/state.json`.

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
- Right-click a row for its action menu.
- Click a row to select it; double-click to play it or open the playlist.
- Wheel scrolls the list. Over the progress or volume bar, the wheel seeks or changes the volume.
- Click or drag the progress bar to seek, and the volume slider to set the volume.

### Themes

`T` opens the theme picker. Seven themes are built in: **amber** (the default),
**rose-pine**, **gruvbox**, **nord**, **catppuccin**, **tokyo-night** and **mono**. Each one
sets the colours, a second accent for the gradient on the progress bar and the logo, the
panel corners, the mark on the selected row, the logo character and the progress-bar
characters. The background is always your terminal's own.

A theme is one commented TOML file (see [`themes/`](themes/)). Your own themes go in
`~/.config/spotlite/themes/<name>.toml`; a file with the name of a built-in theme replaces
it. Keys you leave out take the amber value, and a typo or a bad value is reported, not
ignored. A running spotlite reloads the theme when you save its file, so you can edit a
theme and watch it change.

The same from a shell, for scripts and AI agents:

```
spotlite theme                   list themes (* = in use), with any broken ones
spotlite theme set nord          use nord (a running spotlite changes within a second)
spotlite theme file nord         print nord's file, copying the built-in one there first
spotlite theme new dusk nord     make "dusk" from nord, print its file
spotlite theme check dusk        report mistakes in a theme file
```

```toml
# ~/.config/spotlite/themes/dusk.toml (part)
accent   = "#88c0d0"  # highlights, focused panel, start of the progress gradient
accent2  = "#b48ead"  # end of the progress gradient
corners  = "rounded"  # rounded, plain, thick or double
marker   = "▍"        # one character before the selected row
logo     = "◆"        # one character before "spotlite" and the playing song
bar      = "━─"       # filled and empty parts of the progress bar
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

### Cover art

Playlists show their cover in each row, track lists show a small album thumbnail,
and the player shows the album art.
In ghostty and kitty the pictures use the kitty graphics protocol; other terminals
get sixel or iTerm2 pictures, or coloured half blocks. A third thread downloads,
scales and encodes each picture once; up to 300 are kept in memory.
Set `SPOTLITE_NO_IMAGES=1` to turn pictures off.

## How it works

- The UI thread only draws and reads keys. A second thread makes every Web API
  call, so the UI never waits on the network.
- When spotifyd is the active device, play/pause, skip, seek and volume go to it over
  MPRIS (`playerctl`, about 8 ms) and the screen changes at once; a Web API read 0.5 s
  later confirms it. Other devices use the Web API (about 250 ms per call).
- The player state is read every 5 seconds; the progress bar moves locally in
  between. When idle, the UI wakes once a second.
- Liked Songs and your own playlists play as a context, so playback continues
  past the tracks that are loaded.
- If spotifyd restarts, spotlite finds the device again and retries once.
  On a rate limit (429), it waits for the time Spotify asks for.

## Commands

```
spotlite            terminal UI
spotlite auth       log in (token in ~/.cache/spotlite/token.json, mode 600)
spotlite devices    list Spotify Connect devices
spotlite folders    print your playlist folders
spotlite theme ...  list, set, copy, make and check themes (see Themes)
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

## Web API limits (Development Mode, 2026)

- 5 users per app, owner must have Premium.
- Search returns 10 results at most.
- Playlist tracks are only readable for playlists you own or collaborate on.
  Enter opens those; other playlists play as a whole.
- Liking songs needs the `user-library-modify` permission. If you logged in
  with an older spotlite, run `spotlite auth` again.
