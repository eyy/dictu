# dictu — roadmap

the task list of record. read it at the start of a session, edit it as work lands: move
statuses, append notes, and add new feedback as a new numbered item. numbers are stable
and never reused, so `#20` means the same thing across sessions. refer to items by number.

statuses: `[x]` done · `[~]` in progress · `[ ]` not started.

items #1–#16 carry over from the previous session's list; #17–#23 come from the feedback
round of 2026-07-24; #24 onwards were found while getting the tree green. #1, #2, #4, #5
predate the surviving record and are reconstructed from the code — the original wording is
lost, the substance is not.

---

## in progress

- **[~] #7 load dictionaries off the ui thread (perf).**
  off-thread indexing is done — `std::thread::spawn` + `async_channel` hands the built
  `Library` back to the main context (`src/main.rs:365`), with an `Indexing…` state on the
  search entry and status label. what remains is the **fst disk cache**: ~4M headwords are
  re-sorted on every launch (`Library::from_loaded`, `src/library.rs:50`), so startup pays
  the full ~18–30s index cost each time. persist the merged index under
  `$XDG_CACHE_HOME/dictu/`, keyed by (path, mtime, size) per dictionary, and mmap it back.

- **[~] #14 dict scope panel (multi-select which dicts to search).**
  the data layer is ready and tested: `prefix_search` takes an `active: &[bool]` mask
  (`src/library.rs:80`), and `Config::exclude` (`src/config.rs`, currently
  `#[allow(dead_code)]`) persists a `!`-prefixed exclusion. missing: the ui — a popover or
  preferences page listing the scanned dictionaries with checkboxes, and the wiring from
  checkbox state → mask → re-search. keep the distinction: the mask is a transient search
  scope, `Config::exclude` is a permanent "never load this again" (see #19). watch out:
  `exclude` rewrites `config.toml` through serde, which discards the comments a
  hand-edited config has (the #19 exclusions are commented) — either preserve them or
  stop hand-commenting.

## next — 2026-07-24 feedback

- **[ ] #15 wordlist: lemma vs inflection, and a language/dictionary tag.**
  *re-raised by feedback, now clearly two requirements.* the noise is easy to see:
  `dictu search dacrima` returns dacrima, dacrimae, dacrimam, dacrimarum, dacrimas… all
  inflections of one lemma, each tripled across the overlapping Latin dictionaries (#19).
  - **lemmas vs inflections:** differentiate them visually in the row, and add a config
    option to hide inflections / search lemmas only. **the `.syn` hypothesis is dead for
    the dictionary we kept** — measured while doing #19: the Whitaker copies stored 34,443
    lemmas in `.idx` and ~1.19M inflections in a 22 MB `.syn`, which would have been a
    clean signal, but `latin infl+lewis` has no `.syn` at all and puts all 1,223,585 forms
    directly in `.idx`. the signal that does survive: an inflected form's `.idx` entry
    points at the *same byte range* as its lemma, and the entry text opens with the lemma
    (`dacrimarum` → an entry beginning "dacrima, dacrimae"). so a headword that differs
    from the lemma its definition opens with is an inflection. cheap to compute at index
    time, no format-specific hack.
  - **language tag:** a small all-caps `LAT` / `HEB` / `FR` after each word, falling back
    to the dictionary's name when the language is ambiguous. check `.ifo` for a lang field;
    otherwise carry it on `DictEntry` from config.

## later

- **[ ] #32 the cli panics on a closed pipe.** `dictu dump … | head -3` ends with
  `failed printing to stdout: Broken pipe` and a panic message, because rust ignores
  SIGPIPE and `println!` panics on the resulting `EPIPE`. these subcommands exist to be
  piped into `head`/`grep`, so they should exit quietly instead — write through
  `writeln!(io::stdout(), …)` and stop on an error.

- **[ ] #13 DSL (ABBYY Lingvo) parser.**
  the highest-value format still missing: six dictionaries in the collection are
  `.dsl`/`.dsl.dz` and invisible to the app — Klein's Etymological Hebrew, Dodson Greek,
  the full Liddell-Scott, HALOT, Larousse Chambers, Lexicon to Pindar. `Format::Dsl` is
  already classified and `.dsl`/`.dsl.dz` pairs deduped (`src/config.rs`);
  `is_supported()` returns false and `open_any` bails. the parser must convert dsl's own
  markup to html to satisfy the `Dictionary::lookup` contract, and handle utf-16.

- **[ ] #8 use `glib::clone!` weak refs in signal closures.**
  handlers capture strong `Ui` clones (`src/main.rs`), which hold the window — a reference
  cycle that keeps widgets alive after close. switch to `glib::clone!(#[weak] …)`.

- **[ ] #12 unified search ui (results across dictionaries).**
  largely delivered by #11. what's left is presentation: showing *which* dictionaries a
  result came from in the row, and deduping the same headword across dictionaries more
  intelligently than the current lowercase `HashSet`.

## done

- **[x] #1 gtk4 + libadwaita app skeleton.** `adw::Application`, `OverlaySplitView`
  sidebar + content, `ToolbarView`/`HeaderBar`.
- **[x] #2 StarDict reader.** `.ifo` metadata, big-endian `.idx`, `.dict`/`.dict.dz` data,
  `.syn` synonyms, `sametypesequence` handling.
- **[x] #4 dictd reader + the LSJ csv table.** dictd `.index` with its base-64 offset
  encoding, and the tab-separated `Ref_LSJ.csv` abbreviations table.
- **[x] #5 html → styled runs, uniform across dictionaries.** a controlled tag subset maps
  to our own semantic `Style`; each dictionary's inline css, colours and classes are
  ignored, so everything renders in one look.
- **[x] #6 rich-text rendering via `gtk::TextView` (no webkit).** styles become cached
  `TextTag`s — bold, italic, mono, sup/sub rise, scale, link colour.
- **[x] #9 `dictu --search` cli + single instance.** `HANDLES_COMMAND_LINE` forwards a
  second invocation's argv to the running instance, which focuses the window and fills the
  search box — the path the global hotkey takes. plus the no-gui `dump` and `search`
  subcommands.
- **[x] #10 memory-map `.dict` + lazy per-entry reads.** plain `.dict` files are mmapped so
  the kernel pages definitions in on demand; `.dz`/gzip is decompressed once, with a 2 GiB
  zip-bomb cap.
- **[x] #11 merged cross-dict lemma index + unified search.** one index of
  `(dict, headword)` pairs sorted case-insensitively across all dictionaries; prefix search
  is a `partition_point` binary search plus a walk of the matching run — two `u32`s per
  headword, no copies of the words.
- **[x] #13-review re-review after the data-layer redesign.** (was #13 in the old list;
  renumbered to avoid colliding with the DSL parser.)
- **[x] #24 tree compiles again.** `prefix_search` had grown an `active: &[bool]` mask
  while its three call sites still passed two arguments — a half-applied edit. call sites
  updated to `&[]` ("all dicts") until #14 lands, and the mask itself is now covered by a
  unit test.
- **[x] #26 `hack/` holds the feedback loop** (was empty).
- **[x] #22 a strip says what's below the fold.** under the definition, when a word is
  defined by dictionaries that don't all fit: `1 more definition below: sample` — the count
  and the names. it tracks scrolling and hides itself when everything is in view, so a
  single-dictionary word never shows it. positions come from `TextMark`s at each section
  (marks survive the buffer being rewritten; line numbers wouldn't), measured after gtk has
  laid the buffer out — measuring during the insert returns nothing. still open: making it
  clickable to jump to that section.
- **[x] #21 definitions have visible structure.** senses were the real problem: Lewis &
  Short runs them together separated by a bare `-`, so a long entry read as a wall of text.
  each sense — `- `, `1.`, `II.` — now gets a hanging indent and space above, so the marker
  sits in the margin and the wrapped lines align under the text; each dictionary's body sits
  indented under its own heading, and the heading carries the space that separates one
  dictionary's answer from the next. worth knowing: a `TextTag`'s `left_margin` *replaces*
  the view's rather than adding to it, so an "indent" below the view's own 18px reads as an
  outdent.
- **[x] #31 the test harness runs on its own display.** it was silently unreliable on the
  live session: under xwayland dictu is the only x client, so `xdotool` reports the pointer
  over it while the click lands in the wayland window drawn on top, and an occluded window
  may not repaint, so screenshots showed frames from minutes earlier. both `hack/e2e.py` and
  `hack/shot.sh` now run the app on a private Xvfb display — exact coordinates, no shadow
  margins, current captures, and nothing touches the desktop you're working on.
- **[x] #20 links in definitions work.** the parser was throwing the `href` away, so link
  text was styled blue and underlined but dead. runs now carry their target (a hebrew-hebrew dictionary
  alone has 29,812 of them, shaped `<A href="bword://word">`, where the target is often
  spelled differently from the visible text — so the href, not the text, is what gets
  followed), the definition pane attaches an invisible tag carrying the target, clicking
  follows it as a new search, and the pointer turns into a hand over one. links leading out
  of the app — http, mailto — are deliberately inert rather than opening a browser.
- **[x] #19 the overlapping Latin dictionaries are down to one.** measured rather than
  guessed: `bgl-Latin_English_Inflected` and `stardic latin english inflected` are the same
  dictionary twice (Whitaker's Words — identical headword sets, identical 22 MB `.syn`,
  identical definitions), and `fulllatininflected[1]` (`latin infl+lewis`) covers **every**
  one of their headwords — 0 missing of 34,443 — while adding full Lewis & Short entries
  with citations for lemmas. so the two Whitaker copies are excluded in `config.toml` via
  the `!` prefix and the superset is kept. every Latin hit now appears once instead of
  three times, and the library drops from 8 dictionaries / 4,009,914 headwords to 6 /
  1,567,076 — which also cuts startup indexing.
- **[x] #30 `dictu lookup <file> <word> [--html]`.** a dev affordance added to answer #19:
  `dump` only shows a dictionary's first five entries, so there was no way to compare two
  dictionaries' coverage of the same word. `--html` prints the raw fragment, which is what
  markup work (#20, #21) needs to see.
- **[x] #16 / #23 keyboard focus moves the way you expect.** `Down` in the search box
  steps into the wordlist and selects the first row (so its definition shows); `Up` from
  that first row comes back out to the search box; and typing any printable character
  anywhere in the window goes to the search box, appending rather than replacing.
  modifier combinations are left alone as shortcuts, and control keys stay with whatever
  has focus. all three are covered by e2e checks that inject real keys.
- **[x] #17 / #18 / #28 the status line under the wordlist.** one label, three
  complaints: digits are grouped (`4,009,914`), nouns agree with their counts
  (`1 dictionary`, not `1 dictionaries`), and while searching it counts what the list
  actually shows — `1 result` — rather than the size of the index. a capped search says
  `500+ results` instead of pretending 500 is the whole truth. covered by unit tests on the
  formatting helpers and two e2e checks on the live label.
- **[x] #27 a real feedback loop: `hack/check.sh`.** format → clippy (`-D warnings`) → 24
  unit tests → build → two cli smoke tests over the `sample/` fixture → **ui end-to-end
  over at-spi** (`hack/e2e.py`, 8 checks driving the real widget tree), plus
  `hack/shot.sh`, which screenshots the running app unattended via XWayland + xdotool +
  `import`. documented in `AGENTS.md`.

## housekeeping

- **[x] #25 first commit made.** the app, then the feedback loop and docs.
- **[x] #29 rustfmt.** the tree was never formatted; `cargo fmt` applied across all 8
  source files and `hack/check.sh` now keeps it that way.

## reference — the collection as dictu sees it

`~/Dictionaries`, 8 of 15 dictionaries currently loadable (4,009,914
headwords):

| loadable | dictionary | format |
| --- | --- | --- |
| yes | Latin_English_Inflected (bgl-converted) | StarDict |
| yes | Latin_English_Inflected (stardic) | StarDict |
| yes | latin infl+lewis | StarDict |
| yes | French - English | StarDict |
| yes | MiddleLiddell | StarDict |
| yes | a hebrew-hebrew dictionary (HEB-HEB) | StarDict |
| yes | מילון אבן ספיר | StarDict |
| yes | Ref_LSJ (abbreviations) | tab-separated csv |
| no (#13) | Klein, Comprehensive Etymological Hebrew | DSL |
| no (#13) | Dodson, Greek-English Lexicon | DSL |
| no (#13) | Liddell & Scott, full | DSL |
| no (#13) | Hebrew and Aramaic Lexicon of the OT (HALOT) | DSL |
| no (#13) | Larousse Chambers français-anglais | DSL |
| no (#13) | Lexicon to Pindar | DSL |

three of the eight loadable dictionaries are the overlapping Latin sets (#19), so the
effective library is smaller than the count suggests.
