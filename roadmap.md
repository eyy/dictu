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
  scope, `Config::exclude` is a permanent "never load this again" (see #19).

## next — 2026-07-24 feedback

- **[ ] #16 arrow-down in the search bar moves focus to the wordlist.**
  *re-raised by feedback.* today the search entry keeps focus and the list is mouse-only.
  add a key controller on the entry: `Down` focuses the `ListBox` and selects its first
  row; `Up` from the first row returns to the entry. verifiable in `hack/e2e.py` with
  `Atspi.generate_keyboard_event`.

- **[ ] #23 typing anywhere in the app refocuses the search bar.**
  the mirror of #16 — once focus can leave the entry, getting back must be trivial. a
  window-level `EventControllerKey` that forwards printable keypresses to the
  `SearchEntry` (appending, not replacing). must not swallow arrow keys the list uses.

- **[ ] #15 wordlist: lemma vs inflection, and a language/dictionary tag.**
  *re-raised by feedback, now clearly two requirements.* the noise is easy to see:
  `dictu search dacrima` returns dacrima, dacrimae, dacrimam, dacrimarum, dacrimas… all
  inflections of one lemma, each tripled across the overlapping Latin dictionaries (#19).
  - **lemmas vs inflections:** differentiate them visually in the row, and add a config
    option to hide inflections / search lemmas only. the signal probably lives in StarDict
    `.syn` synonyms and in entries whose body is only a cross-reference — spike it before
    building ui.
  - **language tag:** a small all-caps `LAT` / `HEB` / `FR` after each word, falling back
    to the dictionary's name when the language is ambiguous. check `.ifo` for a lang field;
    otherwise carry it on `DictEntry` from config.

- **[ ] #19 cut the overlapping Latin dictionaries — keep the best one.**
  three near-duplicate inflected Latin sets, all StarDict, all loaded:
  `bgl-Latin_English_Inflected/` and `stardic latin english inflected/` (both
  `Latin_English_Inflected.*`, one converted from BGL) and `fulllatininflected[1]/`
  (`latin infl+lewis.*`). every Latin hit currently appears three times. compare headword
  counts, definition depth (Lewis? Whitaker?) and markup quality with `dictu dump`, keep
  one, exclude the other two with the config's `!` prefix. record the decision here.

- **[ ] #20 links in definitions don't work (noticed in a hebrew-hebrew dictionary).**
  `markup.rs` maps `<a>`/`<kref>` to a link *style* — blue and underlined — but discards
  the `href`, so nothing is clickable. carry the target on `Style`/`Run`, put it on the
  TextView tag, and handle click + pointer-cursor hover to run the target as a new lookup.
  both `bword://`-style and bare-headword targets appear in this collection.

- **[ ] #21 clearer visual separation between the parts of a definition.**
  a definition renders as one continuous flow — headword, then each dictionary's html under
  a dim label (`Ui::show_word`, `src/main.rs`). give the parts real structure: spacing and a
  rule between per-dictionary sections, and visible separation of senses/numbered
  subentries within one definition. the largest visual-quality item on the list.

- **[ ] #22 indicate what's below the fold in the definition panel.**
  when several dictionaries define a word, everything past the first is invisible until you
  scroll. show a footer or sticky strip naming how many further definitions there are and
  which dictionaries they come from — ideally clickable to jump.

## later

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
