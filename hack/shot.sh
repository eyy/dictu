#!/usr/bin/env bash
# screenshot the app, without asking a human to do it.
#
#   hack/shot.sh                          the real collection, ready state
#   hack/shot.sh -o /tmp/x.png dacrima    search for a word first, then shoot
#   hack/shot.sh --sample --select cf     the sample/ fixture, first row selected
#   hack/shot.sh --sample --scope         with the search-scope panel open
#
# how, and why it took a few tries: gnome denies the org.gnome.Shell.Screenshot
# d-bus api to third parties, and grim needs wlr-screencopy, which mutter doesn't
# implement — so there is no way to grab the app on the user's own screen. instead
# the app runs on a private Xvfb display, where it is the only window, and
# ImageMagick's `import` grabs it. that also means this never touches the desktop
# you are working on.
#
# two settings are load-bearing:
#   GSK_RENDERER=cairo   with gtk4's gl renderer, `import` returns a STALE x
#                        pixmap — the window as it first painted, whatever it
#                        shows now. cairo draws into the x drawable instead.
#   GDK_BACKEND=x11      Xvfb is an x server; also the only backend whose windows
#                        can be handed focus, which --select needs.
#
# caveat: no compositor here, so this cannot reproduce a wayland client-side-
# decoration or gl-renderer bug. window content is faithful, which is what
# layout, typography and markup work needs.

set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

OUT="${TMPDIR:-/tmp}/dictu-shot.png"
SIZE="900x700x24"
SAMPLE=0
SELECT=0
SCOPE=0
QUERY=""

while [ $# -gt 0 ]; do
    case "$1" in
        -o) OUT="$2"; shift 2 ;;
        --sample) SAMPLE=1; shift ;;      # fixture dictionaries: seconds, not ~20s
        --select) SELECT=1; shift ;;      # select the first result, so a definition shows
        --scope) SCOPE=1
                 # the popover hangs off the right edge of a 900-wide window, so the
                 # default screen would clip the very thing this flag captures.
                 [ "$SIZE" = "900x700x24" ] && SIZE="1200x800x24"
                 shift ;;
        --size) SIZE="$2"; shift 2 ;;     # e.g. --size 1400x900x24
        -*) echo "unknown flag: $1" >&2; exit 2 ;;
        *) QUERY="$1"; shift ;;
    esac
done

for tool in Xvfb xdotool import; do
    command -v "$tool" >/dev/null || { echo "shot: need $tool" >&2; exit 2; }
done

cargo build 2>&1 | tail -2 || exit 1

# the same machine-wide lock the e2e stage takes: this also kills stray instances,
# so it must not run while someone else's test app is up (see hack/check.sh).
exec 9>"${TMPDIR:-/tmp}/dictu-e2e.lock"
flock -w 900 9 || { echo "shot: gave up waiting for another run to finish" >&2; exit 1; }

# one instance at a time: a second launch forwards its argv to the first and
# exits, so a leftover process would answer instead of the one we just started.
# kill by exact process NAME — `pkill -f target/debug/dictu` also matches the
# shell running this script and kills it (an exit 144 out of nowhere). windows
# only: `dictu dump|lookup|search` never claims the d-bus name, so it can't answer
# for us and doesn't deserve killing.
kill_windows() {
    for pid in $(pgrep -x dictu); do
        # positionally, as main() dispatches it: a substring test would spare
        # `dictu --search dump`, which IS a window and would answer for us.
        case "$(tr '\0' '\n' < "/proc/$pid/cmdline" 2>/dev/null | sed -n 2p)" in
            dump|lookup|search) continue ;;
        esac
        kill "$pid" 2>/dev/null
    done
}
kill_windows
sleep 2   # d-bus name release; relaunching instantly fails with NoReply.

# a display of our own, on the first free number.
DISPLAY_NUM=""
for n in $(seq 96 119); do
    [ -e "/tmp/.X${n}-lock" ] && continue
    DISPLAY_NUM="$n"
    break
done
[ -n "$DISPLAY_NUM" ] || { echo "shot: no free display number" >&2; exit 1; }
Xvfb ":$DISPLAY_NUM" -screen 0 "$SIZE" >/dev/null 2>&1 &
XVFB_PID=$!
export DISPLAY=":$DISPLAY_NUM"

CONFIG_ENV=()
TMPCFG=""
if [ "$SAMPLE" = 1 ]; then
    TMPCFG=$(mktemp -d)
    mkdir -p "$TMPCFG/dictu"
    printf 'dictionary_dirs = ["%s/sample"]\n' "$REPO" > "$TMPCFG/dictu/config.toml"
    CONFIG_ENV=(XDG_CONFIG_HOME="$TMPCFG")
fi

cleanup() {
    kill_windows
    kill "$XVFB_PID" 2>/dev/null
    [ -n "$TMPCFG" ] && rm -rf "$TMPCFG"
}
trap cleanup EXIT

for _ in $(seq 20); do
    [ -e "/tmp/.X11-unix/X$DISPLAY_NUM" ] && break
    sleep 0.2
done

env "${CONFIG_ENV[@]}" GDK_BACKEND=x11 GSK_RENDERER=cairo nohup ./target/debug/dictu \
    > "${TMPDIR:-/tmp}/dictu-shot.log" 2>&1 &

# don't shoot the "Indexing…" state: ask the ui itself when it's ready.
echo "waiting for the index…"
STATUS=$(env "${CONFIG_ENV[@]}" timeout 180 python3 hack/e2e.py --wait-ready) || {
    echo "shot: app never became ready; log:" >&2
    tail -5 "${TMPDIR:-/tmp}/dictu-shot.log" >&2
    exit 1
}
echo "ready: $STATUS"

if [ -n "$QUERY" ]; then
    # --search goes through the single-instance path: no synthetic keystrokes.
    env "${CONFIG_ENV[@]}" ./target/debug/dictu --search "$QUERY"
    sleep 1
fi

# the toplevel is the window with real geometry; gtk also maps a 1x1 helper.
ID=""
for candidate in $(xdotool search --name '^Dictu$'); do
    geom=$(xdotool getwindowgeometry "$candidate" | awk '/Geometry/ {print $2}')
    [ "$geom" = "1x1" ] && continue
    ID="$candidate"
done
[ -n "$ID" ] || { echo "shot: no mapped Dictu window found" >&2; exit 1; }

if [ "$SELECT" = 1 ]; then
    # Down steps from the search box into the wordlist and selects the first row,
    # which is what renders a definition. needs focus: gtk drops keys otherwise.
    xdotool windowfocus "$ID"
    sleep 0.5
    xdotool key --window "$ID" Down
    sleep 0.8
fi

# a gtk4 popover is an x window of its own, sitting over the toplevel — so with the
# scope panel open the whole display is captured, not just the window. that only
# works because this is our own Xvfb (under xwayland `import -window root` fails).
TARGET="$ID"
if [ "$SCOPE" = 1 ]; then
    env "${CONFIG_ENV[@]}" timeout 60 python3 hack/e2e.py --open-scope || {
        echo "shot: could not open the scope panel" >&2; exit 1; }
    sleep 0.5
    TARGET="root"
fi

import -window "$TARGET" "png:$OUT" || exit 1
echo "wrote $OUT ($(identify -format '%wx%h' "$OUT"))"
