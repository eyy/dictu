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
unit tests    cargo test — 24 tests, all in-tree, no external data
build         cargo build
smoke: dump   reads sample/ end to end, asserts 6 headwords + real definition text
smoke: search the merged-index engine over sample/, asserts a prefix hit
ui e2e        hack/e2e.py — the real widget tree, over at-spi
```

flags: `--fast` skips the ui stage (no display needed), `--ci` treats formatting as a
failure rather than fixing it.

do not build ui changes blind. every visual change gets a screenshot; every behavioural
change gets an e2e check.

## ui e2e over at-spi

gtk4 exports every widget on the accessibility bus, so the ui is scriptable — no ocr, no
synthetic clicks into whatever holds focus. `hack/e2e.py` launches dictu against a
throwaway config pointing at `sample/`, so assertions never depend on which dictionaries
are installed.

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
- for keyboard behaviour (roadmap #4, #5) use `Atspi.generate_keyboard_event`; that
  *does* go to the focused window, so only in a check that owns the app.
- `python3-pyatspi` is **not** installed and isn't needed — `gi.repository.Atspi` works.
- the `dbind-WARNING … /org/a11y/atspi/cache` line on startup is noise; ignore it.

## screenshots

```bash
hack/shot.sh                            # real collection, ready state, -> $TMPDIR/dictu-shot.png
hack/shot.sh --sample zeit -o /tmp/x.png    # fixture + a search; seconds, not ~30s
```

this works and needs no human. the route: run the app on **XWayland**
(`GDK_BACKEND=x11`) on the *real* session — where gtk4 still gets hardware gl — then
`xdotool` finds the window and ImageMagick's `import -window` grabs it. the script waits
for the ui's own ready signal first, so it never captures `Indexing…`.

caveat: XWayland gives server-side decorations, so this cannot show wayland
client-side-decoration bugs. it renders window *content* faithfully, which is what
layout, typography and markup work needs. for a CSD or window-placement question, ask
the user for a real screenshot.

routes that do **not** work here (don't re-derive them):

1. gnome shell d-bus (`org.gnome.Shell.Screenshot`) — `AccessDenied: Screenshot is not
   allowed`. gnome 46 gates that api to its own components; third parties are expected to
   go through the xdg desktop portal, which prompts the user, so it's useless unattended.
2. `grim` — needs wlr-screencopy, which mutter doesn't implement.
3. `Xvfb` + `import` — xvfb has no gl, so gtk4's renderer produces **black** frames
   unless you force `GSK_RENDERER=cairo`, and x11 has no CSD. superseded by the live
   XWayland route above.
4. headless `cage` + `grim` — captured black, and cage crashed on exit, popping apport
   dialogs onto the user's desktop.

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
merged ~4M-headword index (~18–30s), then search enables and the status line reads
`N words · M dictionaries`. there is no on-disk index cache yet (roadmap #2), so this
happens on **every** launch — an "it hangs at startup" report is almost always this.

**global hotkey:** `Super+\` → `~/.local/bin/dictu-lookup`, which reads the primary
selection (`wl-paste --primary --no-newline`) and execs `dictu --search "$word"`. it
hard-codes the debug binary path, so it follows `cargo build`.

## no-gui verification

both subcommands short-circuit `main` before any gtk setup, so they work with no display.

```bash
./target/debug/dictu dump sample/sample.index        # name, headword count, first 5 entries
./target/debug/dictu dump "$D/HEB-HEB a hebrew-hebrew dictionary/hebrew-hebrew.ifo"
./target/debug/dictu search abbrevi                  # whole collection, top 20 hits
```

`dump` takes a dictionary's *entry* file — `.ifo` (stardict), `.index` (dictd), `.csv`.
`search` scans the configured dirs and builds the full index first, so budget ~30s
against the real collection; `time` it when testing load performance.

real test data: `~/Dictionaries` — 8 loadable dicts, 4,009,914 headwords.
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

## gotchas

1. cargo not on `$PATH` — the single most common wasted cycle.
2. `sudo` has no tty in a tool call; hand it to the user.
3. `pkill -f target/debug/dictu` kills your own wrapper shell (exit 144) *and* the
   instance the user is looking at. use `pgrep -x dictu`.
4. relaunch race: kill, wait ~2s, then start.
5. `.dsl` is not parsed at all yet (roadmap #13) — six dictionaries in the collection are
   invisible to the app because of it. `.bgl` needs pyglossary first.
6. stale doc: `src/config.rs`'s module comment says the config sits "next to the
   executable". it doesn't — `Config::path()` is the xdg path above.

## conventions

- commit messages: plain descriptive titles, no `feat:`/`fix:` prefixes. reference the
  roadmap item number in the body (`ref roadmap #17`) since this project has no issue
  tracker. never amend, never push — pushing is a human step.
- comments: lowercase, terse, single-line by default; explain *why*, not what.
- keep `roadmap.md` current in the same commit as the work it describes.
