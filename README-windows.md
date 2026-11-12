# spotlite for Windows

A small Spotify client for the terminal. It replaces the Spotify desktop app (300+ MB of
RAM) with one program, `spotlite.exe`, that shows your music in Windows Terminal and plays
the audio itself through a small background player named `spotlite`.

The Windows build is a snapshot of the Linux version. New snapshots are published on the
[releases page](https://github.com/BransonCr/spotlite/releases) as `win-v*` tags.

## Requirements

- Spotify Premium
- Windows 10 or 11
- Windows Terminal 1.22 or later (built into Windows 11, in the Microsoft Store for
  Windows 10). The old console works too, with half-block pictures.

## Install

1. Download `spotlite.exe` from the
   [releases page](https://github.com/BransonCr/spotlite/releases). The zip next to it has the
   same `spotlite.exe` and this README.
2. Move `spotlite.exe` to a folder you keep, for example `%LOCALAPPDATA%\Programs\spotlite`.
3. Add that folder to your `PATH` so `spotlite` works from any terminal. In PowerShell:

   ```powershell
   $dir = "$env:LOCALAPPDATA\Programs\spotlite"
   [Environment]::SetEnvironmentVariable("Path", [Environment]::GetEnvironmentVariable("Path", "User") + ";$dir", "User")
   ```

   Open a new terminal tab afterwards.

## Create a Spotify app

spotlite needs the Client ID of your own Spotify app:

1. Go to <https://developer.spotify.com/dashboard> and log in.
2. Create an app with any name. Redirect URI: `http://127.0.0.1:8898/callback`. API: Web API.
3. Copy the Client ID. No client secret is needed.

## First run

Run `spotlite` in Windows Terminal.

1. Paste the Client ID when asked. It is saved to `%APPDATA%\spotlite\config.toml`.
2. The browser opens to log in the player. Accept and go back to the terminal.
3. The login screen appears. Press Enter, and accept in the browser again so spotlite can read
   your library.

Both logins are saved. Later starts go straight to the app.

`Space` plays, `?` lists every key, `q` quits.

## Quitting

- `q` quits the app but keeps the player running, so the music continues and your phone can
  still control it. The next `spotlite` picks it up.
- `Q` quits and stops the player.
- Closing the terminal tab works like `q`. Open tabs, the last search and the theme are
  restored next time.

If nothing is playing when spotlite starts (after `Q` or a reboot), it loads the last song at
the same position, paused.

## Media keys

The player appears in the Windows media flyout (next to the volume) with the song, artist and
cover. Keyboard media keys and the flyout buttons play, pause and skip.

## Keys

`?` shows this list in the app (type to filter). `.` or a right-click opens every action for
the selected song.

| Key | Action |
|-----|--------|
| `/` | Search (Enter runs it, Esc stops typing, `Ctrl+U` clears) |
| `t` | In Search: switch between songs, artists, albums and playlists |
| `1` to `9`, `Tab` | Tabs: Search, Playlists, Liked, Recent, then open pages |
| `H` / `L` | Previous / next tab |
| `x` | Close the current artist, album or radio tab |
| `j` `k`, arrows, `g` `G`, `Ctrl+D` `Ctrl+U` | Move (more rows load as you scroll) |
| `Esc`, `h` | Back |
| `c` | Jump to the playing song |
| `Enter` | Play the song, or open the playlist, album or folder |
| `P` | Play the whole playlist or album |
| `.` | Action menu for the selected song |
| `a` | Add to queue |
| `i` / `b` | Go to the artist / album |
| `o` | Song radio |
| `F` | Radio filters (on a radio page) |
| `d` | Discover: songs outside your usual taste (on a radio page) |
| `S` | Save the radio list as a new playlist |
| `e` / `E` | Add to a playlist / to the last playlist again |
| `D` | Remove from the open playlist (`u` undoes) |
| `Ctrl+F` | Filter the list |
| `,` / `;` | Sort by title, artist, album, time / reverse |
| `w` / `W` | Mark the song / mark all |
| `u` | Undo the last remove, add, like or unpin |
| `y` | Play on another device |
| `Space` | Play / pause (starts the player if needed) |
| `n` / `p` | Next / previous |
| `Left` / `Right` | Seek 10 seconds |
| `+` / `-`, `]` / `[`, `Shift+Up` / `Shift+Down` | Volume |
| `m` | Mute |
| `s` | Shuffle |
| `r` | Repeat: off, all, one |
| `f` | Like / unlike |
| `T` | Theme picker |
| `:`, `Ctrl+P` | Command palette |
| `v` | Visualizer |
| `V` | Sidebar effect: off, aura, spectrum |
| `R` | Reload the list |
| `q` | Quit (music keeps playing) |
| `Q` | Quit and stop the player |

Mouse: click tabs, buttons and rows, double-click to play or open, right-click for the action
menu, scroll the list, and click or drag the progress and volume bars. Click the artist or the
cover in the player to open the artist or album.

## Features

- Search songs, artists, albums and playlists
- Your playlists, in the folders you made in Spotify
- Liked Songs and the last 50 songs played
- Artist and album pages, each in its own tab
- Song radio with filters, and Discover for songs outside your usual taste
- Filter, sort, multi-select and undo in every list
- Album and playlist covers
- A spectrum visualizer and a sidebar aura that moves with the music
- Seven themes: amber (default), rose-pine, gruvbox, nord, catppuccin, tokyo-night, mono

## Configuration

`%APPDATA%\spotlite\config.toml`. Only `client_id` is required:

```toml
client_id = "your-client-id"

# nerd_font = true         # see "Nerd Fonts" below
# device_name = "spotlite"  # the player's name in Spotify Connect
# redirect_port = 8898      # login redirect: http://127.0.0.1:<port>/callback
# daemon_port = 8899        # the player's local control port

[keys]                      # remap keys; `spotlite keys` lists the names
# next = "N"
# like = ["f", "ctrl-l"]
```

### Nerd Fonts

Themes use Nerd Font glyphs (powerline tabs, hearts, arrows) when available. Windows
Terminal's default font doesn't include them, so they're off by default. To turn them on:

1. Install a Nerd Font, for example CaskaydiaCove Nerd Font from
   <https://www.nerdfonts.com/font-downloads>.
2. In Windows Terminal, set it under Settings > Profiles > Defaults > Appearance > Font face.
3. Add `nerd_font = true` to `config.toml`.

### Custom themes

`spotlite theme new mine amber` creates a theme named `mine` based on amber in
`%APPDATA%\spotlite\themes\mine.toml`. Each key is commented with its options, and a running
spotlite reloads the theme when you save the file.

## Commands

```
spotlite            start the app
spotlite auth       log in again (both logins)
spotlite devices    list Spotify Connect devices
spotlite folders    print your playlist folders
spotlite keys       list actions and their keys
spotlite theme      list, set, copy, create and check themes
spotlite toggle     play/pause
spotlite next       next song
spotlite prev       previous song
spotlite --version  print the version
```

## Files

| What | Where |
|---|---|
| Settings and custom themes | `%APPDATA%\spotlite\` |
| Logins, cache, covers, saved tabs | `%LOCALAPPDATA%\spotlite\` |
| Player log | `%LOCALAPPDATA%\spotlite\daemon.log` |
| Crash notes | `%LOCALAPPDATA%\spotlite\crash.log` |
| Share cards | `Pictures\spotlite\` |

## Troubleshooting

- "Spotify does not see the device": check `daemon.log`. If the port is taken, set
  `daemon_port = 8900` (or another free port) in `config.toml`.
- No sound: the player uses the default output device. Pick the right one in the volume
  flyout, then press `Q` and start spotlite again.
- Broken pictures in the old console or an older Windows Terminal: set
  `$env:SPOTLITE_NO_IMAGES=1` before starting.
- Boxes instead of hearts and arrows: remove `nerd_font = true`, or set a Nerd Font in Windows
  Terminal.
- To log in again: `spotlite auth`.

## Uninstall

1. Press `Q` in spotlite to stop the player.
2. Delete the folder with `spotlite.exe`, `%APPDATA%\spotlite` and `%LOCALAPPDATA%\spotlite`.
3. Optionally delete the app at <https://developer.spotify.com/dashboard> and remove its access
   at <https://www.spotify.com/account/apps/>.

## Spotify API limits

- A Spotify app in development mode allows 5 users, and the owner needs Premium.
- Search returns at most 10 results per type.
