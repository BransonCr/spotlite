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

Press `?` in the app for this list.

| Key | Action |
|-----|--------|
| `/` | Search (Enter to run, Esc to stop typing, `Ctrl+U` to clear) |
| `1` `2` `3`, `Tab` | Search / Playlists / Liked |
| `j` `k`, arrows, `g` `G`, `Ctrl+D` `Ctrl+U` | Move (more pages load as you scroll) |
| `Enter` | Play the selected track, or open a playlist |
| `P` | Play the whole playlist |
| `Esc` | Back from a playlist, or up one folder |
| `a` | Add the selected track to the queue |
| `Space` | Play / pause (starts spotifyd if nothing is active) |
| `n` / `p` | Next / previous |
| `←` / `→` | Seek 10 seconds |
| `+` / `-`, `]` / `[`, `Shift+↑` / `Shift+↓` | Volume |
| `m` | Mute / unmute |
| `s` | Shuffle |
| `r` | Repeat: off, all, one |
| `f` | Like / unlike the playing song |
| `R` | Reload the list |
| `q` | Quit (music keeps playing) |
| `Q` | Quit and stop spotifyd: nothing keeps running (spotlite starts it again next time) |
| `v` | Sound visualizer on / off |

### Mouse

- Click a tab, the search box, `‹` (back), or a player button (`◀◀ ▶ ▶▶`, `⇄`, `↻`, `♥`).
- Click a row to select it; double-click to play it or open the playlist.
- Wheel scrolls the list. Over the progress or volume bar, the wheel seeks or changes the volume.
- Click or drag the progress bar to seek, and the volume slider to set the volume.

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
stream (22 kHz mono); a built-in FFT turns it into 48 bands. Off by default.
It runs only while it is on, the music plays, and the window has focus; otherwise no
capture runs and the UI goes back to one redraw per second.
Cost while running (15 fps): about 0.5% CPU in spotlite and a few percent in the terminal,
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
