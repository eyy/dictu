# AGENTS.md

instructions for working on dictu. what the app *is* lives in `README.md`; the task list
of record is `roadmap.md` — read it at the start of a session and edit it as work lands.

repo: `~/dev/eyy/dictu`. rust 2024 edition, gtk4 + libadwaita, linux/wayland.

---

## the feedback loop

**one command, before every commit:**

```bash
hack/check.sh
```

it formats, lints, unit-tests, builds, then proves the binary actually works — first on
the command line, then by driving the real ui. exit 0 means all of it passed.

```
format        cargo fmt (in place; --ci fails instead of fixing)
clippy        cargo clippy --all-targets -- -D warnings
unit tests    cargo test — 110 tests, all in-tree, no external data
build         cargo build
smoke: dump   reads sample/ end to end, asserts 7 headwords + real definition text,
              and that a closed pipe kills neither the output nor the process
cli           hack/cli.py — 21 checks driving `dictu search|define|scope|dump|lookup`
              over sample/. no display, no d-bus, under a second
speed         hack/speed.py — 9 measurements against a recorded baseline, release build
ui e2e        hack/e2e.py — 35 checks against the real widget tree, over at-spi, 9–13s
```

the whole loop runs 15–24 seconds, plus ~7 when the speed stage has to rebuild release.
those are ranges because they have to be: the same unchanged suite measured 8.7s and
12.9s within one minute, and 13.5s against 16.9s an hour apart, purely on what else the
machine was doing. that is why `hack/speed.py` compares the app against a recorded run
of its own rather than against a number written in a doc — and why every figure quoted
in this file for a *change* comes from interleaved A/B runs, never before-and-after.

flags: `--fast` skips the ui and speed stages (no display needed), `--ci` treats formatting
as a failure rather than fixing it and skips the machine-local speed stage.

## speed vs a baseline

`hack/speed.py` runs `dictu bench` and fails if anything is more than 1.6× slower than the
last recorded run (2× for opening, the one measurement that touches a disk). it exists
because every performance number in this project was measured by hand into a commit
message, where nothing ever checked it again.

it measures the **real collection**, not `sample/` — thirteen words tell you nothing about
opening two million headwords. so `hack/speed-baseline.json` describes one machine and one
shelf of dictionaries, and on any other it skips itself, quietly and successfully, rather
than reporting a regression that is really just a different bookshelf. re-record after
adding a dictionary, or after a change whose cost you have decided to accept:

```bash
hack/speed.py --record     # this run becomes the baseline
hack/speed.py --cold       # include the index rebuild instead of mapping the cache
```

two things learned the hard way while building it. **best-of-N or nothing:** a single warm
open swung between 925 and 3375 ms over an unchanged binary, purely on page cache, so the
script takes the best of three runs of a bench that itself takes the best of five. and
**mind the floor:** the first version ignored anything under 2 ms as noise, which sounded
prudent until you notice every prefix search is 8–400 µs — a mutation that added 160 µs to
every query passed it silently. the floors are now 100 ns, and the mutation fails 7 checks.

do not build ui changes blind. every visual change gets a screenshot; every behavioural
change gets an e2e check.

## ui e2e over at-spi

gtk4 exports every widget on the accessibility bus, so the ui is scriptable — no ocr, no
guessing from pixels. `hack/e2e.py` launches dictu **on its own Xvfb display** with a
throwaway config pointing at `sample/`, so assertions never depend on which dictionaries
are installed and the tests never touch the desktop you're working on.

the private display is not just tidiness. on the real gnome session dictu runs under
xwayland, where it is the only *x* client: `xdotool` reports the pointer as being over it
while the click actually lands in whatever wayland window is drawn on top — clicks silently
go nowhere (and could go somewhere unwanted). on a private display it is the only window,
there are no compositor shadow margins, so at-spi's window coordinates can be used
directly, and repaints are never skipped for being occluded.

```bash
hack/e2e.py            # run the checks, exit code is the verdict
hack/e2e.py -v         # echo what the harness sees while it waits
hack/e2e.py --tree     # dump the live widget tree — how you find a selector
```

what you need to know to add a check:

- roles are not what you'd guess: `gtk::SearchEntry` is role **`entry`**, `gtk::TextView`
  is role **`text`**, wordlist rows are `list item` wrapping a `label` whose *accessible
  name* is the word. use `--tree` rather than guessing.
- rows expose no Action interface. select one with
  `Atspi.Selection.select_child(list, i)` — that is what a click does.
- read text with `Atspi.Text.get_text`; a row's word is its `get_name()`.
- readiness has a real signal: the status line says `Indexing…` until the worker thread
  hands the index over. wait for that text to change, never a fixed sleep.
- drive the search box through `dictu --search WORD` (the single-instance path the global
  hotkey uses) instead of typing — deterministic, and it doesn't steal the user's focus.
- keyboard and pointer behaviour is tested by injecting real events
  (`AppUnderTest.focus_window` / `.press` / `.type_text` / `.click_at`). gtk drops
  injected keys for an unfocused window, so focus comes first via `xdotool windowfocus`
  (`windowactivate` needs `_NET_ACTIVE_WINDOW`, which nothing sets on a bare display).
- aiming a click needs care: gtk4 exposes text attributes over at-spi but **no character
  geometry** (`get_character_extents` fails, `get_offset_at_point` does not even reply, and
  a label's links are exposed as neither `Hypertext` nor objects with a `link` role), so a
  click can't be aimed at a word directly. the link click test aims at a fixture entry whose
  definition is one wide, **gap-free** link — a space between two words belongs to neither
  link, so a click there follows nothing — and searches down a column for it in 16px steps.
  the step is measured, not guessed: clicking every 3px shows the band that follows the link
  is ~24px tall (pane+107..+128), so anything under 21 lands in it. the candidates are tried
  **nearest that band first** and then outward, which is still a search over the same column
  — a shifted layout costs a second or third click, not a failure — but the usual run pays
  one click instead of six, and a click is 0.45s.
- a popover (the search-scope panel) is a **surface of its own**: its widgets join the
  a11y tree only while it is open, and its x window is *also* named `dictu`, which
  xdotool's case-insensitive `--name '^Dictu$'` matches — so the toplevel is the **lowest**
  matching window id, or keys and clicks get aimed at the popover's origin.
- a `gtk::MenuButton` shows up as a `push button` wrapping a `toggle button`, and only the
  inner toggle carries the `click` action (`Atspi.Action.do_action`) that opens the popover.
  a `check box` has **no** action — measured, exactly zero — so it cannot be driven that way.
- **drive a check box with the keyboard, not the pointer.** `Atspi.Component.grab_focus` on
  one fails outright (`atspi_error 1`), but a real `Tab` works, because gtk routes it itself:
  tab until the box has `FOCUSED`, then press space, then wait for `CHECKED` to flip. every
  step is a state you can wait for. note the focus chain is the **whole window's** — entry,
  scroll pane, definition pane, then each dictionary's row *and* its check box — so it is
  longer than the panel looks, though adjacent boxes are two tabs apart.
  clicking the box works too and is what `toggle_scope` used to do; it costs **six times**
  as much (0.673s per flip against 0.111s) because a click needs coordinates, dismisses the
  popover when it misses, and needs ~0.3s of quiet around it that nothing can be waited on.
  that quiet is not negotiable: at 0.05s of it the miss rate is 11 in 20, and each miss
  costs a 3s timeout — cutting the sleeps makes the suite *slower*, which is why they are
  still there in `click_at`.
- **ask a node you already have, not the tree.** every question over at-spi is d-bus
  round trips: walking the window is 14ms closed and 25ms with the popover open, finding
  the check boxes 38ms, finding the status line among every label 24ms — while asking a
  node you already hold whether it is focused, checked, or what its name is costs
  0.1–0.3ms. so `Widgets.status_line` remembers its label (gtk keeps the same accessible
  when the text changes, and a stale one fails the pattern and falls back to the walk),
  and `toggle_scope` reads the boxes and the focus chain off **one** `scope_state` walk
  instead of looking them up per poll. that, and dropping `POLL` to 5ms once the
  predicates were cheap enough to poll that fast, took the suite from 13.5s to 11s.
- **`set_text` on a `SearchEntry` is not one change, and half of it is not debounced.**
  it is a delete followed by an insert, so it emits `changed` twice — even when the text
  it leaves behind is identical — and while `SearchEntry` debounces a search by 150ms, it
  emits for an *emptied* box immediately. so filling the box from code looks like
  `search-changed("")` right now plus `search-changed("word")` in 150ms. #56 has the
  forwarded path block its own handler across the call for that reason; anything else
  that sets that text needs to know the same. it cost ~2.3s of this suite before #56,
  and two wrong fixes before the trace showed what was actually being emitted.
- **turn animations off.** the harness writes `gtk-4.0/settings.ini` with
  `gtk-enable-animations=false` into the throwaway config. it is a gtk setting, so nothing
  under test behaves differently, and it is worth about a second of popovers being pretty.
- don't interleave `dictu --search` with an open popover: the panel is driven by clicks and
  the search box by another process, and the two together are a race not worth chasing.
- `python3-pyatspi` is **not** installed and isn't needed — `gi.repository.Atspi` works.
- the `dbind-WARNING … /org/a11y/atspi/cache` line on startup is noise; ignore it.

## screenshots

```bash
hack/shot.sh                                  # real collection, ready state, -> $TMPDIR/dictu-shot.png
hack/shot.sh --sample zeit -o /tmp/x.png      # fixture + a search; seconds, not ~30s
hack/shot.sh --sample --select cf             # also select the first row, so a definition shows
hack/shot.sh --sample --scope                 # with the search-scope panel open
```

`--scope` opens the panel through `hack/e2e.py --open-scope` and captures the whole display
rather than the window, because a popover is an x window of its own and `import -window
$ID` would miss it. (`import -window root` works here; it is only under xwayland that it
fails.)

this works and needs no human. the app runs on a **private Xvfb display** where it is the
only window, and ImageMagick's `import -window` grabs it; the script waits for the ui's own
ready signal first, so it never captures `Indexing…`, and nothing appears on your screen.

two things are load-bearing, both learned the hard way:

- **`GSK_RENDERER=cairo`.** with gtk4's default gl renderer, `import -window` reads a
  stale x pixmap — you get the window as it looked when it first painted, however much
  has changed since. it looks like the app is broken when it isn't. the cairo renderer
  draws into the x drawable, so captures are current.
- **capture the window, not the screen.** `import -window root` fails outright under
  xwayland (`Resource temporarily unavailable`), so there is no full-screen path there.

caveat: one window, no compositor, cairo renderer — so this cannot reproduce a
gl-renderer or wayland client-side-decoration bug. window *content* is faithful, which is
what layout, typography and markup work needs. (on the real session, note gtk4's invisible
shadow margins make the x window larger than the logical one — a 900×600 window is a
1022×722 drawable — which is one more reason the harness doesn't run there.)

routes that do **not** work here (don't re-derive them):

1. gnome shell d-bus (`org.gnome.Shell.Screenshot`) — `AccessDenied: Screenshot is not
   allowed`. gnome 46 gates that api to its own components; third parties are expected to
   go through the xdg desktop portal, which prompts the user, so it's useless unattended.
2. `grim` — needs wlr-screencopy, which mutter doesn't implement.
3. grabbing the app **on the live session** — it runs under xwayland there, so a wayland
   window drawn on top is invisible to x: clicks aim at dictu and land elsewhere, and an
   occluded window may not repaint at all, so a capture shows a frame from minutes ago.
   this is what the private display replaces.
4. headless `cage` + `grim` — captured black, and cage crashed on exit, popping apport
   dialogs onto the user's desktop. Xvfb needs none of that.

the manual fallback: the user saves gnome screenshots to `~/Pictures/Screenshots/` as
`Screenshot from YYYY-MM-DD HH-MM-SS.png`. pick the newest by the timestamp **in the
filename** (`ls` order is not sorted) and read it. `identify -format '%wx%h'` tells a
full-screen grab from a window grab.

---

## prerequisites

**cargo is not on `$PATH`** in a non-login shell. every invocation needs it prefixed;
`hack/*.sh` already do this:

```bash
export PATH="$HOME/.cargo/bin:$PATH"      # or: . "$HOME/.cargo/env"
```

**system dev packages** — the `gtk4` crate's build script needs the dev headers, not just
the runtime libs:

```bash
sudo apt install -y libgtk-4-dev libadwaita-1-dev build-essential pkg-config
# verify: pkg-config --modversion gtk4 libadwaita-1   -> 4.14.5 / 1.5.0
```

`sudo` cannot run from a tool call (no tty) — hand any apt install to the user.

**`.bgl` needs pyglossary.** dictu doesn't read babylon files; they're converted to
stardict in place, next to the original, and the scanner then picks up the `.ifo`:

```bash
~/.local/dictu-venv/bin/pyglossary in.BGL out.ifo \
  --read-format=BabylonBgl --write-format=Stardict --ui=cmd
```

(already done for the two `.bgl` dicts in the collection; the venv still exists.)

## build

```bash
cargo build
```

the first build compiles the gtk-rs tree (~200 crates, minutes); rebuilds are seconds.
lint policy lives in `Cargo.toml`, not on the command line — `unsafe_code = "deny"`
crate-wide with one documented `#[allow]` for the memmap2 call, and clippy's `all` group
at warn. `hack/check.sh` promotes those warnings to errors.

## run the gui

no env vars needed on the real wayland session:

```bash
nohup ./target/debug/dictu >/tmp/dictu.log 2>&1 &
```

redirect and background it — gtk complains to stderr and you want it after the fact.

**it is single-instance** (`HANDLES_COMMAND_LINE`): a second launch forwards its argv to
the running instance and exits. so:

- to see new code, kill the old pid first, **wait ~2s**, then relaunch — an immediate
  relaunch loses the d-bus name race and dies with `NoReply`.
- kill by exact process name: `pgrep -x dictu | xargs -r kill`. never
  `pkill -f target/debug/dictu` — that pattern also matches the shell running your own
  command, which then dies with a mysterious exit 144.
- `dictu --search WORD` fills the running window's search box. `--open` no longer exists.

startup: the window appears immediately showing `Indexing…`, a worker thread builds the
merged index, then search enables and the status line reads `N words · M dictionaries`.
the index is **cached on disk** (roadmap #7), so that takes ~0.5s once warm, ~7s the first
time and after any dictionary file changes. the cache is in `~/.cache/dictu` — ~280 MB for
this collection, most of it the decoded text of the DSL dictionaries — and is pure derived
data: `rm -rf ~/.cache/dictu` costs one slow launch and nothing else, which is also how you
force the cold path when timing it. dictu says so on stderr if it can't write there.

**global hotkey:** `Super+\` → `~/.local/bin/dictu-lookup`, which reads the primary
selection (`wl-paste --primary --no-newline`) and execs `dictu --search "$word"`. it
hard-codes the debug binary path, so it follows `cargo build`.

## no-gui verification

both subcommands short-circuit `main` before any gtk setup, so they work with no display.

```bash
./target/debug/dictu dump sample/sample.index        # name, headword count, first 5 entries
./target/debug/dictu dump "$D/HEB-HEB a hebrew-hebrew dictionary/hebrew-hebrew.ifo"
./target/debug/dictu lookup sample/sample.index rust # one entry, as plain text
./target/debug/dictu lookup "$D/fulllatininflected[1]/latin infl+lewis.ifo" virtus --html
./target/debug/dictu search abbrevi                  # whole collection, top 20 hits
```

`dump` and `lookup` take a dictionary's *entry* file — `.ifo` (stardict), `.index`
(dictd), `.csv`. `lookup --html` prints the raw html fragment, which is what markup work
needs to see; without it you get the plain text.
`search` scans the configured dirs and builds the full index first — ~0.5s against the real
collection with a warm cache, ~7s with a cold one. `time` it when testing load performance,
and `rm -rf ~/.cache/dictu` first if the cold path is what you mean to measure.

real test data: `~/Dictionaries` — 14 loadable dicts, 1,941,292 headwords
(the count in `roadmap.md`'s collection table; the old 4M figure predates the excluded
duplicate Latin dictionaries).
`sample/` is a hand-built dictd fixture (6 definitions, gzipped `.dict.dz`, dictd-base64
offsets) and is what the smoke tests and e2e use; no unit test reads it.

## config

`~/.config/dictu/config.toml`, created with defaults on first run:

```toml
dictionary_dirs = ["/home/you/Dictionaries"]
```

each plain path is scanned **recursively**; a `!` prefix excludes everything under a path
(the *inverse* of gitignore's `!`). paths are compared as written, never canonicalized —
don't mix a symlink with its target between an include and its exclude. point
`XDG_CONFIG_HOME` at a temp dir to test against a fixture without touching the real one.

## don't take the desktop down with you

**the ui harness must run on its own d-bus session.** `hack/check.sh` does this
(`dbus-run-session -- python3 hack/e2e.py`); never invoke `hack/e2e.py` bare. dictu
registers its accessibility tree on whatever session bus it finds, and the harness then
enumerates every application on that bus at a 0.04 s poll while it waits — on the user's own
session that means poking gnome-shell's a11y tree thousands of times per run. **gnome-shell
46 segfaulted twice under exactly that**, core-dumping and taking every window on the desktop
with it: 2026-07-27 23:42 and 2026-07-28 18:17, both while the suite was being run
repeatedly. a private bus costs nothing and removes the class.

and prefer `hack/cli.py` while iterating. it asks the same `Collection` through the command
line, needs no display, no d-bus and no lock, and answers in 0.8 s against the e2e suite's
16 s — so there is rarely a reason to run the display harness more than once.


**this happened.** on 2026-07-26 systemd-oomd killed the user's GoLand (11 processes) and
then IBus (4 processes) — their input method, so typing stopped working — because the user
slice crossed 50% memory pressure for 20 seconds. nothing crashed; the machine sacrificed
their apps to make room for ours. it reads, from the other side of the screen, exactly like
the session falling over.

what pushes it there, in order of damage:

1. **cargo's default parallelism.** 14 cores means 14 `rustc` processes, each hundreds of
   MB. cap it: `cargo build -j4` (or `CARGO_BUILD_JOBS=4`).
2. **concurrent agents.** each one builds *and* opens the real collection. run reviewers
   **one at a time** on this project, not two or five.
3. **a cache VERSION bump.** it forces a cold rebuild of the merged index — a **792 MB
   peak** — in every worktree and every agent that runs the binary. bump it when the format
   demands it, then warm the cache once, deliberately, before anything else runs.
4. **release builds.** only when measuring; debug is enough for everything else.

for anything heavy, put a ceiling on it so the kernel throttles *us* rather than the
machine hunting for something to kill:

```sh
systemd-run --user --scope -q -p MemoryHigh=6G -p MemoryMax=8G -- cargo build -j4
```

and check the aftermath rather than guessing, because oomd is silent in the terminal:

```sh
journalctl -b | grep -E "systemd-oomd.*Killed"
```

the cache is derived data and can be cleared, but a full wipe costs a 792 MB rebuild —
delete only the images from superseded versions (the u32 at byte 8 of a `.didx`/`.dord` is
its VERSION).

## reach for the crate first

hand-rolled a json escaper and an argument parser here once. a review found four bugs in
twenty lines of the argument handling — a `--limit` that accepted `banana` and silently
searched at 500, options refused before the word, a mistyped command that raised the window
and exited 0 — and the escaper was correct only because the collection had not yet contained
the characters it got wrong. both are now `clap` and `serde_json`, which cost +14 crates in
a tree gtk4 already dominates.

before writing a utility, ask whether it is *this app's* problem. it is worth writing when no
crate knows the answer — the dictionary formats (StarDict, DSL, dictd), the cache image, the
key normalization for polytonic greek and pointed hebrew, the grouping rules in `library`.
it is not worth writing for parsing, serializing, escaping, number formatting or anything
else a thousand programs need: those have known answers, and ours will be the version with
the bugs.

## gotchas

1. cargo not on `$PATH` — the single most common wasted cycle.
2. `sudo` has no tty in a tool call; hand it to the user.
3. `pkill -f target/debug/dictu` kills your own wrapper shell (exit 144) *and* the
   instance the user is looking at. use `pgrep -x dictu`.
4. relaunch race: kill, wait ~2s, then start.
5. `.dsl` is read now (roadmap #13); `.bgl` still needs pyglossary first.
6. **compressed dictionary data is unpacked into the cache, never at open.** a `.dict.dz`
   or `.dsl.dz` cannot be mapped and read in place, and gunzipping one per launch is not a
   detail: two files cost 817 ms and 188 MB of every start until #54. both readers now
   unpack once into the cached index's **payload** and map it ever after, so a reader whose
   ranges point at bytes that are not a mappable file wants `Built::payload`, not a `Vec`
   held for the life of the process. mind the direction too — a plain `.dict` must *not* be
   copied into the cache, since it is already a file we can map.
7. stale doc: `src/config.rs`'s module comment says the config sits "next to the
   executable". it doesn't — `Config::path()` is the xdg path above.

## conventions

- commit messages: plain descriptive titles, no `feat:`/`fix:` prefixes. reference the
  roadmap item number in the body (`ref roadmap #17`) since this project has no issue
  tracker. never amend, never push — pushing is a human step.
- comments: lowercase, terse, single-line by default; explain *why*, not what.
- keep `roadmap.md` current in the same commit as the work it describes.
