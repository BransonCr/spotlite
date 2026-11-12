# Windows port maintenance

spotlite is developed on Linux. Windows gets snapshots: every so often `main` is brought up to
date for Windows, tagged `win-v<version>` and released as a zip. Between snapshots the Windows
build is not maintained, and Linux work does not wait on it.

## Layout

One codebase, one branch. Most of the code is platform independent. The exceptions:

| Path | Contents |
|---|---|
| `src/platform/` (`unix.rs`, `windows.rs`) | Close signals, file permissions, detached processes, clipboard, fonts, the media window, the Nerd Font default |
| `src/engine/` | Player backends: `spotifyd.rs` (systemctl, playerctl, parec) and `builtin.rs` (`spotlite daemon` over the socket protocol in `proto.rs`) |
| `src/daemon/` | The built-in player: librespot playback and Connect, the audio tap, media controls |
| `build.rs` | Sets `cfg(builtin_player)` on Windows, and on Linux with `--features player` |
| `Cargo.toml` | `[target.'cfg(windows)'.dependencies]`: player crates and `windows-sys` |

The built-in player also builds on Linux (`--features player`, `backend = "builtin"`), so most
of the Windows code path can be tested there. Only `platform/windows.rs` and the Windows side of
`daemon/media.rs` are Windows-only.

Guideline for Linux work (optional, makes snapshots easier): put code that runs external
programs, uses system paths or calls Unix-only APIs in `platform` or `engine`. For example, a
new `Command::new("notify-send")` would become `platform::notify()` with a Windows version or
a no-op.

## CI

Two jobs in `.github/workflows/ci.yml` run on every push and are allowed to fail:

- `windows`: clippy, tests and a release build on `windows-latest`
- `linux-player`: clippy and tests with `--features player`

A failure there means Linux work used something the Windows build doesn't have. It can wait
until the next snapshot.

To type-check the Windows build locally without the Windows SDK (ring gets stub headers):

```sh
rustup target add x86_64-pc-windows-msvc
mkdir -p /tmp/winhdr && cd /tmp/winhdr
echo '#define assert(x) ((void)0)' > assert.h
printf '#include <stddef.h>\nvoid abort(void);\n' > stdlib.h
printf '#include <stddef.h>\nvoid *memcpy(void *, const void *, size_t); void *memset(void *, int, size_t); void *memmove(void *, const void *, size_t); int memcmp(const void *, const void *, size_t);\n' > string.h
cd -
CC_x86_64_pc_windows_msvc=clang-cl CFLAGS_x86_64_pc_windows_msvc="/imsvc /tmp/winhdr" \
  cargo clippy --target x86_64-pc-windows-msvc --all-targets --locked -- -D warnings
```

## Testing

`tools/windows-test.sh` has four commands:

- `try`: runs spotlite here the way a new Windows user would see it: a fresh profile in
  `target/wintest/`, the built-in player instead of spotifyd, and plain glyphs. It asks for
  the Client ID and does both browser logins. spotifyd is stopped during the test and started
  again afterwards. `try --reset` clears the profile; without it you get the returning-user
  flow.
- `exe`: builds `spotlite.exe` and the release zip in `dist/` (needs `mingw-w64-gcc`).
- `ci`: builds the same zip on GitHub's Windows runner (push the branch first).
- `serve`: serves `dist/` over HTTP so a Windows machine or VM can download the zip
  (`http://10.0.2.2:8000` from a QEMU guest).

`try` covers the player, the socket, the logins and the first run. The console close event,
clipboard, fonts, media flyout, WASAPI audio and terminal graphics need a real Windows machine.

## Releasing a snapshot

1. Check the latest `windows` CI job on `main` and fix what it reports. Usually it's a new
   system call that needs a `platform` or `engine` function.
2. Get a build from a manual run of the `windows-release` workflow (Actions, windows-release,
   Run workflow; the zip is the run's artifact) or from `tools/windows-test.sh exe`, and go
   through the checklist below on Windows 11.
3. Update [README-windows.md](../README-windows.md) for new keys, features and settings. It
   ships in the zip.
4. Add a `## Windows snapshot win-v<version>` section to `CHANGELOG.md` with the user-facing
   changes since the last snapshot.
5. `git tag win-v<version>` and `git push origin win-v<version>`. The `windows-release`
   workflow builds the zip and creates the GitHub release.

`<version>` is the `version` in `Cargo.toml`. A second snapshot of the same version gets a
suffix, e.g. `win-v0.2.0-2`.

## Checklist

In Windows Terminal, from an unzipped release build:

- [ ] First run asks for the Client ID, writes `%APPDATA%\spotlite\config.toml`, logs the
      player in, then shows the login screen.
- [ ] `spotlite devices` lists `spotlite`.
- [ ] A playlist plays. Space, `n`, `p`, Left/Right, `+`/`-` respond immediately.
- [ ] A song change from the phone shows up immediately.
- [ ] `v` (visualizer) and `V` (aura) move with the music.
- [ ] Playlist folders and song radio (`o`) load.
- [ ] Covers show (sixel in Windows Terminal 1.22+, half blocks otherwise).
- [ ] Media keys and the volume flyout show and control the song.
- [ ] The share card is saved to `Pictures\spotlite` and its path is on the clipboard.
- [ ] After `q` and a new `spotlite`, the music never stopped and the same player is used.
- [ ] Closing the tab with its X keeps the open tabs for next time.
- [ ] `Q` stops the music and leaves no `spotlite.exe` in Task Manager.
- [ ] The old console (`conhost.exe`) works, with half-block covers.
