#!/usr/bin/env bash
# screenshot the running app, without asking a human to do it.
#
#   hack/shot.sh                          shot of the real collection, ready state
#   hack/shot.sh -o /tmp/x.png dacrima    search for a word first, then shoot
#   hack/shot.sh --sample zeit            use the sample/ fixture — seconds, not ~30s
#
# how it works, and why it works now when earlier attempts didn't: gnome denies
# the org.gnome.Shell.Screenshot d-bus api to third-party callers, and grim needs
# wlr-screencopy, which mutter doesn't implement. so instead we run the app on
# XWayland (GDK_BACKEND=x11) on the REAL session, where gtk4 still gets hardware
# gl — then xdotool can find the window and ImageMagick's `import` can grab it.
# (the earlier dead end was Xvfb, which has no gl, so every frame came out black.)
#
# caveat: XWayland draws server-side decorations, so this cannot show wayland
# client-side-decoration bugs. it shows window CONTENT faithfully, which is what
# layout, typography and markup work needs.

set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

OUT="${TMPDIR:-/tmp}/dictu-shot.png"
SAMPLE=0
KEEP=0
SELECT=0
QUERY=""

while [ $# -gt 0 ]; do
    case "$1" in
        -o) OUT="$2"; shift 2 ;;
        --sample) SAMPLE=1; shift ;;
        --select) SELECT=1; shift ;;  # select the first result, so a definition shows
        --keep) KEEP=1; shift ;;   # leave the app running afterwards
        -*) echo "unknown flag: $1" >&2; exit 2 ;;
        *) QUERY="$1"; shift ;;
    esac
done

for tool in xdotool import; do
    command -v "$tool" >/dev/null || { echo "shot: need $tool" >&2; exit 2; }
done

cargo build 2>&1 | tail -2 || exit 1

# one instance at a time: a second launch forwards its argv to the first and
# exits, so a stale process would leave us shooting the old build. kill by exact
# process NAME — `pkill -f target/debug/dictu` also matches the shell running
# this script and kills it (that's an exit 144 out of nowhere).
pgrep -x dictu | xargs -r kill
sleep 2   # d-bus name release; relaunching instantly fails with NoReply.

CONFIG_ENV=()
TMPCFG=""
if [ "$SAMPLE" = 1 ]; then
    TMPCFG=$(mktemp -d)
    mkdir -p "$TMPCFG/dictu"
    printf 'dictionary_dirs = ["%s/sample"]\n' "$REPO" > "$TMPCFG/dictu/config.toml"
    CONFIG_ENV=(XDG_CONFIG_HOME="$TMPCFG")
fi

cleanup() {
    [ "$KEEP" = 1 ] || pgrep -x dictu | xargs -r kill
    [ -n "$TMPCFG" ] && rm -rf "$TMPCFG"
}
trap cleanup EXIT

# GSK_RENDERER=cairo matters: with gtk4's default gl renderer, `import` reads a
# stale x pixmap — you get the window as it looked when it first painted, no
# matter what is on screen now. the cairo renderer draws into the x drawable, so
# a capture is always current. (this, not the absence of gl, is why the earlier
# xvfb attempts came out blank.)
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
    # --search goes through the single-instance path, so no synthetic keystrokes
    # and no stealing the user's focus.
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

import -window "$ID" "png:$OUT" || exit 1
echo "wrote $OUT ($(identify -format '%wx%h' "$OUT"))"
