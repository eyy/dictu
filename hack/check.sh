#!/usr/bin/env bash
# the feedback loop: format, lint, test, then prove the binary actually works —
# on the command line and through the real ui. one command, one verdict.
#
#   hack/check.sh              format in place, then everything
#   hack/check.sh --ci         fail (don't fix) on formatting, everything else same
#   hack/check.sh --fast       skip the ui e2e (no display needed)
#
# exit 0 = every stage passed. anything else = read the output above it.

set -uo pipefail

# rustup installs into ~/.cargo/bin, which a non-login shell does not pick up.
export PATH="$HOME/.cargo/bin:$PATH"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

CI=0
FAST=0
for arg in "$@"; do
    case "$arg" in
        --ci) CI=1 ;;
        --fast) FAST=1 ;;
        *) echo "unknown flag: $arg" >&2; exit 2 ;;
    esac
done

FAILED=()

stage() {
    local name="$1"; shift
    printf '\n\033[1m== %s\033[0m\n' "$name"
    if "$@"; then
        printf '\033[32mok\033[0m %s\n' "$name"
    else
        printf '\033[31mFAILED\033[0m %s\n' "$name"
        FAILED+=("$name")
    fi
}

# -- stages -----------------------------------------------------------------

fmt() {
    if [ "$CI" = 1 ]; then
        cargo fmt --check
    else
        cargo fmt && cargo fmt --check
    fi
}

lint() {
    # -D warnings turns clippy's advice into a hard failure, so the loop can't
    # rot. the crate's lint policy (deny unsafe, warn clippy::all) is in Cargo.toml.
    cargo clippy --all-targets -- -D warnings
}

unit() { cargo test; }

build() { cargo build; }

# the cli smoke test: read the in-repo fixture dictionary end to end. no
# dropbox, no display, no network — if this breaks, a parser broke.
smoke_dump() {
    local out
    out=$(./target/debug/dictu dump sample/sample.index) || return 1
    echo "$out" | head -3
    grep -q 'headwords: 6' <<<"$out" || { echo "expected 6 headwords" >&2; return 1; }
    grep -q 'nocturnal burrowing mammal' <<<"$out" || { echo "definition text missing" >&2; return 1; }
}

# the same engine the gui uses, pointed at the fixture via a throwaway config,
# so the assertion doesn't depend on which dictionaries are installed here.
smoke_search() {
    local tmp out
    tmp=$(mktemp -d) || return 1
    mkdir -p "$tmp/dictu"
    printf 'dictionary_dirs = ["%s/sample"]\n' "$REPO" > "$tmp/dictu/config.toml"
    out=$(XDG_CONFIG_HOME="$tmp" ./target/debug/dictu search zeit)
    local status=$?
    rm -rf "$tmp"
    echo "$out"
    [ $status -eq 0 ] || return 1
    grep -q 'zeitgeist' <<<"$out" || { echo "prefix search missed zeitgeist" >&2; return 1; }
    grep -q "2 dicts" <<<"$out" || { echo "fixture dicts not loaded" >&2; return 1; }
}

# drive the real widget tree over at-spi (see hack/e2e.py).
e2e() {
    # it refuses to run beside another window, whose single-instance forwarding
    # would answer with the wrong config. clear the way first — windows only: a
    # `dictu dump|lookup|search` short-circuits before gtk, so it never claims the
    # d-bus name, and a cli search over a real collection runs for half a minute.
    for pid in $(pgrep -x dictu); do
        tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null \
            | grep -qE ' (dump|lookup|search) ' || kill "$pid"
    done
    sleep 2
    python3 hack/e2e.py
}

# -- run --------------------------------------------------------------------

stage "format" fmt
stage "clippy" lint
stage "unit tests" unit
stage "build" build
stage "smoke: dump the fixture dictionary" smoke_dump
stage "smoke: unified search over the fixture" smoke_search
if [ "$FAST" = 1 ]; then
    printf '\n\033[33mskipped\033[0m ui e2e (--fast)\n'
elif [ -z "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
    printf '\n\033[33mskipped\033[0m ui e2e (no display)\n'
else
    stage "ui e2e over at-spi" e2e
fi

printf '\n'
if [ ${#FAILED[@]} -eq 0 ]; then
    printf '\033[32mall checks passed\033[0m\n'
    exit 0
fi
printf '\033[31m%d stage(s) failed:\033[0m %s\n' "${#FAILED[@]}" "${FAILED[*]}"
exit 1
