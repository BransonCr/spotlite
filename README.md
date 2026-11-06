# spotlite

A small Spotify client for the terminal. It replaces the official Electron app
(300+ MB of RAM) with two light parts:

| Part | Job | Idle RAM |
|------|-----|----------|
| [`spotifyd`](https://github.com/Spotifyd/spotifyd) | Spotify Connect daemon (librespot). Plays the audio. | ~10–30 MB |
| `spotlite` | Terminal UI. Searches, browses, and controls playback through the Spotify Web API. | ~5 MB, 1 thread, 0% CPU |

`spotlite` is a single-threaded Rust binary (~2.3 MB release build). It polls the
player state every 5 seconds and moves the progress bar locally between polls.

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
   cargo install --path .
   spotlite auth
   spotlite devices
   spotlite
   ```

## Keys

| Key | Action |
|-----|--------|
| `/` | Search (Enter to run, Esc to stop typing) |
| `1` `2` `3`, `Tab` | Search / Playlists / Liked |
| `j` `k`, arrows, `g` `G` | Move |
| `Enter` | Play the selected track or playlist |
| `Space` | Play / pause |
| `n` / `p` | Next / previous |
| `+` / `-` | Volume |
| `s` | Shuffle |
| `r` | Reload the list and find the device again |
| `q` | Quit |

## Commands

```
spotlite            terminal UI
spotlite auth       log in (token in ~/.cache/spotlite/token.json, mode 600)
spotlite devices    list Spotify Connect devices
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
- Playlist tracks are only readable for playlists you own or collaborate on,
  so spotlite plays a playlist as a whole (`context_uri`) instead of listing it.
