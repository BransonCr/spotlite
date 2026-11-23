#!/usr/bin/env python3
"""Times spotlite: start to ready, a search, a play from a song list, a play from Liked.

Runs two builds one after the other (A B, then B A, ...), each in a pseudo terminal, with
`SPOTLITE_LOG` on, and reads the timing marks the builds write (`mark ...` lines, see
src/log.rs). Each run starts the build's own player daemon first, so it needs
`backend = "builtin"` and a build with `--features player`. It stops a running
`spotlite daemon` (the music stops) and plays real songs.

  python3 contrib/bench.py --base OLD_BINARY --new NEW_BINARY [--runs 5]

The saved state (tabs, search) is copied to a temporary folder for each run, so the real
one does not change. The exit status is 1 when a median of the new build is more than
`--target` (0.5) times the base build's.
"""

import argparse
import json
import os
import pty
import re
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import fcntl
import time
from pathlib import Path

QUERIES = ["radiohead creep", "daft punk one more time", "miles davis so what", "nirvana lithium", "fleetwood mac dreams"]
METRICS = [
    ("open", "start", "ready"),
    ("first-frame", "start", "first-frame"),
    ("search", "search-go", "search-shown"),
    ("play-list", "play-key", "playing"),
    ("play-context", "play-key", "playing"),
]
TARGETED = ["open", "search", "play-list", "play-context"]
STATE = Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local/state")) / "spotlite"
AUDIO_CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "spotlite/audio"
# What a terminal answers to "which device are you" (DA1): the picture probe stops there.
DA1 = re.compile(rb"\x1b\[0?c")


class Log:
    """The marks a run wrote, as (seconds, name, detail)."""

    def __init__(self, path):
        self.path = path

    def marks(self):
        out = []
        try:
            text = self.path.read_text(errors="replace")
        except FileNotFoundError:
            return out
        for line in text.splitlines():
            m = re.match(r"(\d+\.\d+) mark (\S+) ?(.*)", line)
            if m:
                out.append((float(m.group(1)), m.group(2), m.group(3)))
        return out

    def wait(self, name, detail=None, after=0.0, timeout=25.0):
        end = time.time() + timeout
        while time.time() < end:
            for t, n, d in self.marks():
                if n == name and t >= after and (detail is None or d == detail):
                    return t, d
            time.sleep(0.01)
        raise TimeoutError(f"no mark '{name} {detail or ''}' within {timeout} s")


def stop_daemons():
    subprocess.run(["pkill", "-f", r"spotlite daemon$"], check=False)
    for _ in range(100):
        if subprocess.run(["pgrep", "-f", r"spotlite daemon$"], capture_output=True).returncode != 0:
            return
        time.sleep(0.05)
    subprocess.run(["pkill", "-9", "-f", r"spotlite daemon$"], check=False)
    time.sleep(0.3)


def prepare_state(state_dir):
    """A copy of the saved state on the Search tab, songs mode, nothing playing."""
    dest = state_dir / "spotlite"
    dest.mkdir(parents=True)
    for name in ("state.json", "history.json"):
        if (STATE / name).exists():
            shutil.copy(STATE / name, dest / name)
    path = dest / "state.json"
    if path.exists():
        st = json.loads(path.read_text())
        st.update({"tab": "Search", "active": None, "search_kind": "Songs", "search": ""})
        if isinstance(st.get("playback"), dict):
            st["playback"]["playing"] = False
        path.write_text(json.dumps(st))


class Terminal:
    """The build in a 120x40 pseudo terminal that answers the device query."""

    def __init__(self, binary, env):
        pid, fd = pty.fork()
        if pid == 0:
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
            os.execve(binary, [binary], env)
        self.pid, self.fd = pid, fd
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        self.alive = True
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        tail = b""
        while self.alive:
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                return
            if not data:
                return
            tail = (tail + data)[-64:]
            if DA1.search(tail):
                tail = b""
                os.write(self.fd, b"\x1b[?62;22c")

    def keys(self, text, gap=0.0):
        for ch in text:
            os.write(self.fd, ch.encode())
            if gap:
                time.sleep(gap)

    def close(self):
        try:
            self.keys("q")
        except OSError:
            pass
        for _ in range(100):
            done, _ = os.waitpid(self.pid, os.WNOHANG)
            if done:
                break
            time.sleep(0.05)
        else:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        self.alive = False
        os.close(self.fd)


def one_run(binary, i, settle):
    tmp = Path(tempfile.mkdtemp(prefix="spotlite-bench-"))
    try:
        state = tmp / "state"
        prepare_state(state)
        log = Log(tmp / "marks.log")
        env = dict(os.environ, XDG_STATE_HOME=str(state), SPOTLITE_LOG=str(log.path), TERM="xterm-256color")
        env.pop("TMUX", None)
        stop_daemons()
        shutil.rmtree(AUDIO_CACHE, ignore_errors=True)
        daemon = subprocess.Popen([binary, "daemon"], env=env, stdin=subprocess.DEVNULL,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        dlog = state / "spotlite/daemon.log"
        end = time.time() + 30
        while time.time() < end and not (dlog.exists() and "connected" in dlog.read_text(errors="replace")):
            time.sleep(0.05)
        time.sleep(1.0)
        term = Terminal(binary, env)
        got = {}
        try:
            start, _ = log.wait("start")
            got["first-frame"] = log.wait("first-frame")[0] - start
            got["open"] = log.wait("ready")[0] - start
            time.sleep(settle)
            q = QUERIES[i % len(QUERIES)]
            term.keys("/")
            time.sleep(0.2)
            term.keys(q, gap=0.02)
            term.keys("\r")
            go, _ = log.wait("search-go", q)
            shown, _ = log.wait("search-shown", q, after=go)
            got["search"] = shown - go
            # A person reads the results before Enter.
            time.sleep(1.0)
            t0 = time.time() - 0.05
            term.keys("\r")
            key, uri = log.wait("play-key", after=t0)
            got["play-list"] = log.wait("playing", uri, after=key)[0] - key
            time.sleep(2.0)
            term.keys("3")
            time.sleep(0.5)
            term.keys("g")
            term.keys("j" * (2 + 3 * i), gap=0.03)
            time.sleep(1.0)
            t0 = time.time() - 0.05
            term.keys("\r")
            key, uri = log.wait("play-key", after=t0)
            got["play-context"] = log.wait("playing", uri, after=key)[0] - key
        finally:
            term.close()
            stop_daemons()
            daemon.wait(timeout=5)
        return got
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", required=True)
    ap.add_argument("--new", required=True)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--target", type=float, default=0.5)
    ap.add_argument("--settle", type=float, default=2.0, help="seconds to wait after ready before the search")
    ap.add_argument("--json", help="also write every run to this file")
    args = ap.parse_args()
    builds = {"base": os.path.abspath(args.base), "new": os.path.abspath(args.new)}
    results = {"base": [], "new": []}
    for i in range(args.runs):
        order = ["base", "new"] if i % 2 == 0 else ["new", "base"]
        for name in order:
            for attempt in range(2):
                try:
                    got = one_run(builds[name], i, args.settle)
                    break
                except TimeoutError as e:
                    print(f"run {i + 1} {name}: {e}; again", file=sys.stderr)
            else:
                got = {}
            results[name].append(got)
            print(f"run {i + 1} {name:4} " + "  ".join(f"{k} {v * 1000:6.0f}" for k, v in got.items()), flush=True)
    if args.json:
        Path(args.json).write_text(json.dumps(results, indent=1))
    print()
    print(f"{'metric':13} {'base ms (median)':>17} {'new ms (median)':>16} {'new/base':>9}")
    ok = True
    for metric, *_ in METRICS:
        b = [r[metric] for r in results["base"] if metric in r]
        n = [r[metric] for r in results["new"] if metric in r]
        if not b or not n:
            print(f"{metric:13} {'-':>17} {'-':>16}")
            ok &= metric not in TARGETED
            continue
        mb, mn = statistics.median(b), statistics.median(n)
        ratio = mn / mb if mb else float("inf")
        flag = ""
        if metric in TARGETED:
            passed = ratio <= args.target and len(n) == args.runs
            ok &= passed
            flag = "pass" if passed else "FAIL"
        print(f"{metric:13} {mb * 1000:17.0f} {mn * 1000:16.0f} {ratio:9.2f} {flag}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
