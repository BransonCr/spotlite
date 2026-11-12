#!/usr/bin/env bash
# Test the Windows version of spotlite. Four commands, from quick to real:
#
#   tools/windows-test.sh try [--reset]   On this machine, as a new Windows user would see it:
#                                         an empty profile, the built-in player instead of
#                                         spotifyd, plain glyphs. Stops spotifyd while it runs.
#   tools/windows-test.sh exe             Build the real spotlite.exe and the release zip in
#                                         dist/ (needs mingw-w64-gcc).
#   tools/windows-test.sh serve           Serve dist/ to a Windows VM: in the VM, open
#                                         http://10.0.2.2:8000 and download the zip.
#   tools/windows-test.sh ci              Build the zip on GitHub's Windows runner and download
#                                         it to dist/ (the branch must be pushed first).
#
# Then follow the checklist in docs/windows.md.
set -euo pipefail
cd "$(dirname "$0")/.."

ROOT=$PWD
PROFILE=$ROOT/target/wintest
DIST=$ROOT/dist
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

try() {
    if [[ ${1:-} == --reset ]]; then
        rm -rf "$PROFILE"
        echo "The test profile is empty again."
    fi
    cargo build --features player
    bin=$ROOT/target/debug/spotlite
    mkdir -p "$PROFILE"/{config,cache,state}
    if [[ ! -f $PROFILE/config/spotlite/config.toml && -f ~/.config/spotlite/config.toml ]]; then
        echo "Your Client ID (for the first-run question):"
        sed -n 's/^client_id *= *"\(.*\)"/  \1/p' ~/.config/spotlite/config.toml
        echo
    fi
    # spotifyd and the built-in player both call themselves "spotlite": stop spotifyd for the
    # test, and start it again after.
    had_spotifyd=0
    if systemctl --user --quiet is-active spotifyd; then
        had_spotifyd=1
        systemctl --user stop spotifyd
        echo "spotifyd is stopped while you test. It starts again when you quit."
    fi
    finish() {
        if pgrep -f "^$bin daemon" >/dev/null; then
            read -rp "The test player still plays (q keeps it on). Stop it? [Y/n] " answer
            [[ ${answer,,} == n* ]] || pkill -f "^$bin daemon" || true
        fi
        if ((had_spotifyd)); then
            systemctl --user start spotifyd
            echo "spotifyd runs again."
        fi
    }
    trap finish EXIT
    XDG_CONFIG_HOME=$PROFILE/config XDG_CACHE_HOME=$PROFILE/cache XDG_STATE_HOME=$PROFILE/state \
        SPOTLITE_BACKEND=builtin SPOTLITE_NERD=0 "$bin" || true
    echo "The player's log: $PROFILE/state/spotlite/daemon.log"
}

zip_up() {
    local exe=$1 name=$2
    local out=$DIST/spotlite-$name-x86_64-windows.zip
    rm -rf "$DIST/spotlite" "$out"
    mkdir -p "$DIST/spotlite"
    cp "$exe" "$DIST/spotlite/spotlite.exe"
    cp README-windows.md "$DIST/spotlite/README.md"
    (cd "$DIST" && 7z a -tzip -bso0 "$out" spotlite)
    echo "Made $out"
}

exe() {
    if ! command -v x86_64-w64-mingw32-gcc >/dev/null; then
        echo "Needs the Windows cross compiler: sudo pacman -S mingw-w64-gcc" >&2
        exit 1
    fi
    rustup target add x86_64-pc-windows-gnu >/dev/null
    cargo build --release --locked --target x86_64-pc-windows-gnu
    zip_up target/x86_64-pc-windows-gnu/release/spotlite.exe "win-v$VERSION-test"
}

serve() {
    ls "$DIST"/*.zip >/dev/null 2>&1 || { echo "No zip in dist/: run '$0 exe' or '$0 ci' first." >&2; exit 1; }
    echo "In the Windows VM, open http://10.0.2.2:8000 (QEMU) or http://<this PC's IP>:8000 and"
    echo "download the zip. Ctrl+C stops this."
    python3 -m http.server 8000 --directory "$DIST"
}

ci() {
    local branch
    branch=$(git branch --show-current)
    if ! git ls-remote --exit-code --heads origin "$branch" >/dev/null; then
        echo "Push the branch first: git push -u origin $branch" >&2
        exit 1
    fi
    gh workflow run windows-release.yml --ref "$branch"
    sleep 5
    local run
    run=$(gh run list --workflow windows-release.yml --branch "$branch" --limit 1 --json databaseId --jq '.[0].databaseId')
    echo "Building on GitHub (about 10 minutes): run $run"
    gh run watch "$run" --exit-status
    mkdir -p "$DIST"
    gh run download "$run" --name spotlite-windows --dir "$DIST"
    ls "$DIST"/*.zip
}

case ${1:-} in
    try) shift; try "$@" ;;
    exe) exe ;;
    serve) serve ;;
    ci) ci ;;
    *) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
