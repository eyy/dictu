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

# DICTU_REQUIRE_DISPLAY turns the reference-cycle test's headless skip into a
# failure: it needs a real widget tree, and libtest reports a silent skip as a
# pass, so without this the guard can go inert without anyone noticing.
unit() {
    if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
        DICTU_REQUIRE_DISPLAY=1 cargo test
    else
        cargo test
    fi
}

build() { cargo build; }

# the cli smoke test: read the in-repo fixture dictionary end to end. no
# dropbox, no display, no network — if this breaks, a parser broke.
smoke_dump() {
    local out
    out=$(./target/debug/dictu dump sample/sample.index) || return 1
    echo "$out" | head -3
    grep -q 'headwords: 7' <<<"$out" || { echo "expected 7 headwords" >&2; return 1; }
    grep -q 'nocturnal burrowing mammal' <<<"$out" || { echo "definition text missing" >&2; return 1; }

    # a reader that walks away mid-output must not take dump down with it (#32):
    # rust ignores SIGPIPE, so println! used to panic on the resulting EPIPE.
    # the reader is `true`, not `head -1`: dump writes only seven short lines, and
    # head usually reads all of them before the writer notices, so that version of
    # this gate caught a reintroduced panic barely half the time (and less on a busy
    # machine). a reader that never reads at all closes the pipe first, every time.
    local err status
    err=$(mktemp) || return 1
    ./target/debug/dictu dump sample/sample.index 2>"$err" | true
    status=${PIPESTATUS[0]}
    if [ -s "$err" ]; then
        echo "closed pipe: expected no stderr, got:" >&2; cat "$err" >&2; rm -f "$err"; return 1
    fi
    rm -f "$err"
    [ "$status" -eq 0 ] || { echo "closed pipe: dump exited $status, expected 0" >&2; return 1; }
}

# the same engine the gui uses, pointed at the fixture via a throwaway config,
# so the assertion doesn't depend on which dictionaries are installed here.
smoke_search() {
    local tmp out
    tmp=$(mktemp -d) || return 1
    mkdir -p "$tmp/dictu"
    printf 'dictionary_dirs = ["%s/sample"]\n' "$REPO" > "$tmp/dictu/config.toml"
    # a throwaway cache dir too, so the index cache (roadmap #7) is exercised from
    # cold here and the real ~/.cache/dictu is left alone.
    out=$(XDG_CONFIG_HOME="$tmp" XDG_CACHE_HOME="$tmp" ./target/debug/dictu search zeit)
    local status=$?
    rm -rf "$tmp"
    echo "$out"
    [ $status -eq 0 ] || return 1
    grep -q 'zeitgeist' <<<"$out" || { echo "prefix search missed zeitgeist" >&2; return 1; }
    grep -q "2 dicts" <<<"$out" || { echo "fixture dicts not loaded" >&2; return 1; }
}

# drive the real widget tree over at-spi (see hack/e2e.py).
e2e() {
    # take a machine-wide lock first. dictu is single-instance over d-bus, and this
    # stage kills stray instances to make sure the one it talks to is its own — so
    # two runs at once (several worktrees, or an agent per branch) kill each other's
    # app mid-test and fail for no reason. the lock is on the whole machine, not the
    # checkout, because the thing being contended is the session bus.
    exec 9>"${TMPDIR:-/tmp}/dictu-e2e.lock"
    if ! flock -w 900 9; then
        echo "e2e: gave up waiting for another run to finish" >&2
        return 1
    fi

    # it refuses to run beside another window, whose single-instance forwarding
    # would answer with the wrong config. clear the way first — windows only: a
    # `dictu dump|lookup|search` short-circuits before gtk, so it never claims the
    # d-bus name, and a cli search over a real collection runs for half a minute.
    # the subcommand is read positionally, exactly as main() dispatches it: a
    # substring match would spare `dictu --search dump`, which IS a window.
    for pid in $(pgrep -x dictu); do
        case "$(tr '\0' '\n' < "/proc/$pid/cmdline" 2>/dev/null | sed -n 2p)" in
            dump|lookup|search) continue ;;
        esac
        kill "$pid" 2>/dev/null
    done
    sleep 2
    python3 hack/e2e.py
    local status=$?
    exec 9>&-
    return $status
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
