#!/usr/bin/env bash
# put dictu in the application grid, and point the global shortcut at the same
# launcher, so both always start the latest build.
#
#   hack/desktop/install.sh            install (or refresh) both
#   hack/desktop/install.sh --remove   take them out again
#
# what it writes, all under $HOME and all reversible:
#   ~/.local/bin/dictu                          the launcher (newest build wins)
#   ~/.local/share/applications/…Dictu.desktop  the grid entry
# and it repoints the DICTU= line in ~/.local/bin/dictu-lookup, the script the
# global shortcut runs, so the shortcut stops hard-coding the debug binary.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_ID="io.github.eyy.Dictu"
LAUNCHER="$HOME/.local/bin/dictu"
ENTRY="$HOME/.local/share/applications/$APP_ID.desktop"
HOTKEY="$HOME/.local/bin/dictu-lookup"

if [ "${1:-}" = "--remove" ]; then
    rm -fv "$LAUNCHER" "$ENTRY"
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
    echo "removed. the shortcut still points at the launcher path; rerun without --remove to restore it."
    exit 0
fi

mkdir -p "$(dirname "$LAUNCHER")" "$(dirname "$ENTRY")"

sed "s|@REPO@|$REPO|g" "$REPO/hack/desktop/dictu-launch.in" > "$LAUNCHER"
chmod +x "$LAUNCHER"
echo "wrote $LAUNCHER"

sed "s|@LAUNCHER@|$LAUNCHER|g" "$REPO/hack/desktop/dictu.desktop.in" > "$ENTRY"
echo "wrote $ENTRY"

# the shell caches this directory; without it the entry can take a re-login to show.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$(dirname "$ENTRY")" 2>/dev/null || true
fi
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$ENTRY" && echo "the entry validates"
fi

# and the global shortcut runs a script of its own (it reads the primary selection
# before handing the word over); point its binary at the launcher rather than at
# whichever build was current the day it was written.
if [ -f "$HOTKEY" ]; then
    if grep -q '^DICTU=' "$HOTKEY"; then
        sed -i "s|^DICTU=.*|DICTU=\"$LAUNCHER\"|" "$HOTKEY"
        echo "repointed $HOTKEY at the launcher"
    else
        echo "note: $HOTKEY has no DICTU= line; left alone" >&2
    fi
else
    echo "note: no $HOTKEY — the global shortcut is set up outside this repo" >&2
fi

cat <<NOTE

the icon is still #48's job: the entry names $APP_ID and no such icon is
installed, so the grid shows a placeholder until one is.
NOTE
