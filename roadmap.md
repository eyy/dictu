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

nothing open right now.

## next — 2026-07-24 feedback

- **[ ] #33 wordlist: tell lemmas from inflections** (the other half of #15).
  the noise is easy to see: `dictu search rex` returns 27 results, 26 of them inflections of
  one lemma. differentiate them in the row, and add a config option to hide inflections /
  search lemmas only.
  **the `.syn` hypothesis is dead for the dictionary we kept** — measured while doing #19:
  the Whitaker copies stored 34,443 lemmas in `.idx` and ~1.19M inflections in a 22 MB
  `.syn`, which would have been a clean signal, but `latin infl+lewis` has no `.syn` at all
  and puts all 1,223,585 forms directly in `.idx`. two signals survive, both worth a spike
  before any ui work:
  1. **shared byte ranges** — an inflection's `.idx` entry points at the *same* range as its
     lemma, so headwords can be grouped by range and the shortest of each group taken as the
     lemma (`amo` over `amare`/`amavi`; `dacrima` over `dacrimae`). costs a pass over ~1.2M
     entries at index time, which #7 (already slow) has to absorb — measure it.
  2. **the entry's opening words** — an inflection's definition opens with its lemma
     (`dacrimarum` → an entry beginning "dacrima, dacrimae"). exact, but reading a definition
     per row is too slow to do for 500 rows per keystroke unless only a prefix is read.

  measured evidence for how bad the noise is, from the `.idx` of `latin infl+lewis`:
  1,427,152 entries for 1,223,585 unique headwords, and the worst offenders are *auxiliary*
  forms attached to every verb that uses them — `esse` and `eris` appear in 100 entries each,
  `ero` and `isti` in 99, `sum` in 73, `sam` in 71. the dictionary's generator inflected the
  auxiliary along with the verb, so searching a form of *esse* pulls up a hundred unrelated
  verbs. treating these as inflections rather than headwords is what fixes it.

## deferred — needs you

these are blocked on a decision or an action only you can take. nothing else waits on them.

- **#37 Wiktionary, phase A** — the cheap path is dropping a prebuilt StarDict build of the
  language pair you want into the collection, which needs (a) picking the languages and
  (b) a download. both are yours; the app needs no change to read them.
- **a look at the real window.** the harness screenshots on a private display with the cairo
  renderer, so it cannot show a wayland client-side-decoration or gl-renderer problem. if
  something looks wrong on your actual desktop that the screenshots don't reproduce, that is
  the reason.
- **the stray character in commit `81fd7b1`'s body** (`a真 frame`). unpushed history, so it is
  fixable, but rewriting it is your call — the convention here is never to amend.

## later

- **[ ] #42 fuzzy search, typable greek and hebrew, lemmas-only — see
  `docs/search-index-plan.md`.** the plan of record for #33, #39, #12 and fuzzy. no fst, no
  database: a bounded scan measures **2.4 ms at 3 characters, 24 ms at 6, 57–67 ms worst
  realistic case** over the real headwords in a release build, which is fast enough that
  fuzzy needn't even be a fallback. independently reviewed, and the review overturned three
  of the draft's premises against the actual files — worth reading before starting:
  the byte-range lemma signal **does not exist** (100% singleton groups over 1,427,152 latin
  `.idx` entries), NFC alone leaves **74,174 pointed hebrew headwords** untypable, and the
  cheap-looking length filter passes 60% of the corpus at the modal query length. the signal
  that does work — an inflection's entry opens with its lemma in bold — costs 333 ms and
  turns `rex` from 27 rows into 1.



- **[ ] #39 lookup matches exact bytes, so Greek can't be typed.** Dodson stores 4,686 of
  its headwords in non-NFC form (`ό` as U+1F79 oxia), and every greek keyboard — and every
  other greek dictionary here — produces U+03CC tonos, which finds nothing. HALOT adds 3,259
  headwords with non-canonical hebrew mark order. app-wide, not DSL's doing: `lookup` is an
  exact `HashMap` hit and `prefix_search` only lowercases. normalise both the index key and
  the query to NFC in `Library`, where every format benefits at once.

- **[x] #40 two dictionaries can share one name.** labels come from the parent folder, and a
  folder can hold two dictionaries: Klein's lexicon sits beside its own abbreviations list, so
  the scope panel showed two identical rows differing only in their headword count. a
  colliding label now falls back to what the dictionary calls itself — StarDict's `bookname`,
  DSL's `#NAME` — giving `Comprehensive Etymological (Heb-Eng)` and `Etymological ABBRV
  (Heb-Eng)`; if even those match, the file stem is appended. labels that were already unique
  are untouched, verified against the whole collection.
  **still open, and not a collision:** several unique labels are filesystem artifacts rather
  than names — `dict` (that is `Ref_LSJ.csv` sitting in the collection root), plus
  `fulllatininflected[1]`, `Grc-Eng_L&S_Greek-English Lexicon_or_2.dsl` and
  `Middle_Liddell_stardict`. preferring the internal name for those too is a bigger judgement
  call, since for the latin dictionary the folder name (`fulllatininflected[1]`) and the
  internal one (`Dictionary latininfl -> english`) are both poor.

- **[ ] #41 a headword whose only entry renders empty stays in the wordlist.** `lookup`
  drops entries that convert to nothing, but the headword was already filed — so a card that
  is only an audio reference (`[s]snd.wav[/s]`) leaves a row that shows nothing when
  selected. no occurrences in the current collection (0 of 475,360 entries), so it is latent;
  the fix is to reconcile the two at index time rather than filter at lookup time.


- **[ ] #37 offline Wiktionary, chosen by language pair.** pick `French > English` in the
  config and get every French entry from the **English** Wiktionary, glossed in english,
  offline. the appeal is obvious: it covers the modern languages this collection has no
  dictionary for, and it is maintained by someone else.

  **prior art, since this is a well-trodden path — do not write a wikitext parser.**
  Wiktionary entries are generated by templates and Lua (Scribunto) modules, so parsing the
  XML dump directly yields mangled junk; that is precisely why the tool below exists, and it
  expands the templates by *running the Lua*.
  1. **wiktextract** (Tatu Ylonen) is the canonical extractor, published as JSONL at
     **kaikki.org** and refreshed about weekly. the english edition, all languages, is
     `raw-wiktextract-data.jsonl` — 23.1 GB raw, 2.6 GB compressed. separate per-*edition*
     extracts exist too (the French *edition* is 6.2 GB).
  2. kaikki also published per-language slices *of the english edition*, which is exactly the
     shape asked for here: `kaikki.org-dictionary-French.jsonl`, 544 MB, **388,993 word forms
     / 544,835 senses**. it is marked **DEPRECATED and due for removal**, so don't build on
     that url — filter the big jsonl by `lang_code` ourselves instead.
  3. **Vuizur/Wiktionary-Dictionaries** already publishes these as **StarDict** (also tabfile
     and kindle), per language, built from kaikki/wiktextract data. dictu reads StarDict
     today, so this is a zero-code way to have the feature this week — worth trying *before*
     writing anything.
  4. **DBnary** models many editions as Ontolex/RDF — better structured, awkward to consume
     from rust. **Aard2 `.slob`** and **Kiwix `.zim`** are offline *containers* for full pages
     rather than lexicons; useful as ux prior art, not as a data source. **pyglossary** (already
     used here for BGL→StarDict) converts between several of these.

  order, now that the requirement is a few languages rather than all of Wiktionary (see
  `docs/search-index-plan.md` for the settled architecture):
  - **A.** drop Vuizur's StarDict build of the wanted pair into the collection and judge it.
    dictu has read StarDict since day one, so this is a config entry and no code at all.
  - **B.** only if those builds read poorly **and** lemmas-only is wanted for those languages
    too — which needs wiktextract's `forms` with inflection tags, and the prebuilt
    dictionaries almost certainly flatten those away — write a one-off import *outside* the
    app that emits a dictionary file plus a lemma-flag sidecar in the shape the app already
    understands. no database inside dictu; it stays a reader.
  - the per-language kaikki slice that matches this exactly is deprecated, so filter the big
    jsonl by `lang_code` if it comes to B.

  **licensing is not optional here:** Wiktionary text is CC BY-SA (dual-licensed GFDL), so a
  derived dictionary must attribute and stay share-alike, and kaikki asks that Ylonen's LREC
  2022 paper be cited. the per-dictionary heading already shown above each definition is the
  natural place to carry that attribution.

- **[ ] #38 saving `config.toml` destroys its comments.** `Config::save` (`src/config.rs`)
  rewrites the whole file through serde, so the hand-written notes in the real config —
  which say *why* the #19 Latin duplicates and the #34 French dictionary are excluded —
  would be gone the first time anything persisted a change. that is why #14's scope panel
  keeps its choice in memory and why `Config::exclude` is still unwired: a "remove this
  dictionary for good" ui isn't possible until saving edits the file in place. `toml_edit`
  (already in the tree as toml 0.8's own dependency) preserves comments and layout; the
  test that proves it has to round-trip a commented fixture, not a generated one.

- **[ ] #12 unified search ui (results across dictionaries).**
  largely delivered by #11. what's left is presentation: a row shows the tag of only the
  *first* dictionary that had the word, even when the definition pane goes on to show
  several — the row should say when more than one dictionary answers. (this was visible as
  `rex` reading `FR`; excluding that dictionary in #34 hid the symptom, not the cause.) and
  deduping the same headword across dictionaries more intelligently than the current
  lowercase `HashSet`.

## done

- **[x] #13 DSL (ABBYY Lingvo) reader.** `src/dict/dsl.rs`. the collection's nine `.dsl` /
  `.dsl.dz` files load — 471,446 headwords, a third more than the app could read before —
  and the six dictionaries the format was hiding are searchable: Klein's Etymological
  Hebrew, Dodson Greek, both Liddell-Scotts, HALOT, Larousse Chambers, Lexicon to Pindar.
  the file is decoded once (utf-16 le/be by bom, utf-8 with or without one, utf-16le
  guessed from its nul bytes when there is none) and kept; the index holds one byte range
  per card and markup is converted **in `lookup`**, so nothing parsed is stored per entry.
  ascii code units skip the decode machinery, which is what makes opening the 170 MB
  Liddell-Scott 2.7s rather than 9.8s in the unoptimized build we run.
  what it renders: `[b] [i] [u] [sup] [sub]`, `[ex]` and `[p]` (italic — 822k `[p]`s in
  Liddell-Scott alone), `[mN]` as nested blocks, `\[` escapes, `~` as the headword, and
  both `[ref]…[/ref]` and `<<…>>` as `bword://` links, so dsl cross-references are
  clickable through #20 with no ui change. what it drops: colour (`[c]`, the most common
  tag in every one of these files), zone markers (`[trn] [!trs] [com]`), `[lang]`, `[s]`
  media (contents and all), `{{…}}` comments, and every unknown tag — dropping a
  distinction beats leaking brackets into the text. mis-nested markup (real dsl is full of
  `[b]…[c]…[/b]…[/c]`) is closed at the line end so the renderer always gets a tree.
  `(…)` in a headword is dsl's optional part, so it is filed both ways — that is what makes
  HALOT's 6.5k `(*)`-marked hebrew roots findable by the bare root, and `ad lib(itum)`
  findable as either.
  three things this exposed, none of them dsl's fault: the L&S folder holds the *same*
  dictionary twice (`Grc-Eng_L&S_….dsl.dz` and the identical `….dsl/Grc-Eng.dsl` inside a
  directory that ends in `.dsl`, which `dedupe_dsl` doesn't catch) — 115k duplicate
  headwords, ~3.5s and ~200 MB of startup for nothing; that nested folder also makes the
  wordlist label read `Grc-Eng_L&S_Greek-English Lexicon_or_2.dsl`, since `label_for` uses
  the parent folder's name; and lookup is exact bytes, so Dodson's `ό` (U+1F79 oxia) is a
  different word from a typed `ό` (U+03CC tonos) — nothing normalizes either side.
  reviewed before merging, which caught three things worth the delay. halot writes nested
  optional groups — `((*)II) root` — and the parser tracked a flag rather than a depth, so
  942 junk headwords were filed (`II) root`) and ~471 cards were unreachable from the bare
  root, defeating a goal the branch claimed; `אבד` went from 2 entries to 3 once fixed. the
  decoded text kept its utf-16 capacity for the life of the process, ~225 MB of slack across
  the collection, since these scripts all encode smaller in utf-8. and the Liddell-Scott
  folder ships the same lexicon twice — a `.dz` beside a **directory** whose name ends in
  `.dsl`, which `dedupe_dsl` could not see because a directory is never a scanned entry —
  costing 115k duplicate headwords, ~190 MB, and every greek word listed under two labels.
  measured after: 14 dictionaries / 1,941,292 headwords → **13 / 1,825,792**, peak RSS
  917 MB → 791 MB.
  still open, and app-wide rather than DSL's fault: **lookup matches exact bytes**, so
  Dodson's 4,686 non-NFC greek headwords (U+1F79 oxia) can't be found by typing the U+03CC
  tonos every greek keyboard produces, and HALOT adds 3,259 with non-canonical hebrew mark
  order. normalising both the index key and the query to NFC belongs in `Library`, not here.
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
- **[x] #7 load dictionaries off the ui thread, and stop re-indexing on every launch.**
  the off-thread half was already done (`std::thread::spawn` + `async_channel` hands the
  built `Library` to the main context, with an `Indexing…` state). the disk cache is now
  there too, in `src/index_cache.rs`, under `$XDG_CACHE_HOME/dictu/`.
  **measured first, because it decided the design** (debug build, the real 5-dictionary /
  1,469,846-headword collection, `dictu search rex` = a full index build): 16.1 s total, of
  which parsing the `.idx` files was 10.3 s and the merged case-insensitive sort 4.4 s. the
  Latin dictionary alone (28 MB `.idx`, 1,427,152 entries) took 7.4 s — and only 0.3 s of
  that was scanning the bytes; 1.7 s was allocating a `String` per headword and **5.5 s was
  building the `HashMap<String, Vec<Range>>`** (a `Vec` allocation and two `String` clones
  per entry). so caching only the sort order would have left two thirds of the cost in
  place: the cache had to carry the headword → byte-range mapping itself.
  it does. each dictionary gets a `<name>-<hash>.didx` holding its keys (sorted, so lookup
  is a binary search over the mapped bytes), their byte ranges, and its display headword
  list; the merged order is 8 bytes per headword in `merged.dord`. both are memory-mapped
  back, both carry a fingerprint of every source file they were derived from (path + mtime
  + size of the `.ifo`, `.idx`, `.dict` and `.syn` — the `.dict` too, since a new data file
  under an unchanged index would silently move every range), and anything stale, truncated
  or unreadable is ignored in favour of rebuilding. writes go through a temp file and a
  rename, so a half-written cache can never be read.
  **then #13 landed nine DSL dictionaries under it**, so the measurement was redone against
  the collection as it now is — 13 dictionaries, 1,825,792 headwords, `dictu search rex`
  10.2 s uncached. DSL was 4.3 s of the 5.5 s spent opening dictionaries, and **2.6 s of that
  was decoding utf-16** (1.4 s for the 170 MB Liddell-Scott alone), the rest parsing cards.
  that settles a question the format raises: a DSL card's range points into the *decoded*
  text, so caching the index alone would still have decoded every file on every launch and
  saved only the 1.7 s of parsing. so an index can now carry a **payload** — the bytes its
  ranges point into — and DSL stores its decoded text there. StarDict leaves it empty,
  because its `.dict` is already a file we can map.
  **result: 10.2 s → 0.52 s warm** (peak RSS 792 MB → 164 MB, since the decoded text is now
  paged out of a mapped file rather than held as ~206 MB of `String`). the *cold* path is
  7.1 s, better than the 10.2 s it replaces because laying out the blob is cheaper than the
  hash map it used to build. the cache costs 279 MB on disk, ~240 MB of which is DSL text.
  **no `fst`, though the item asked for one** — measured rather than assumed: an `fst::Map`
  of the Latin keys is 474 KB against the blob's 40.7 MB, but it can only carry
  `key -> u64`, so the 22 MB of range side-tables stay either way (22.5 MB against 40.7 MB
  all told), it takes 1.30 s to build against 0.33 s, and 20k lookups cost the same (89 ms
  against 100 ms). its one real advantage — prefix search off the map — doesn't apply,
  because dictu's prefix search is case-insensitive *across* dictionaries and so runs on the
  merged order, never on one dictionary's exact-byte map. 18 MB of a memory-mapped file was
  not worth a dependency and a 4x slower rebuild; the note is in `index_cache`'s module doc.
  **still open, in rough order of value:**
  1. `headwords()` still materializes a `Vec<String>` per dictionary at open — nearly all of
     what the 0.52 s warm start now is — purely because the `Dictionary` trait hands out a
     `&[String]`. an indexed accessor (`headword(i) -> &str`) would let the words stay in the
     mapped file and take the warm start to near-nothing.
  2. dictd and the csv still parse from scratch — they are text formats that cost
     milliseconds *here*, but a large dictd dictionary would want the same treatment, and
     `index_cache::build` takes whatever a format hands it.
  3. the cache is big (279 MB) because DSL text is stored decoded. a `.dsl` that is already
     plain utf-8 could be mapped in place instead of copied, but every large one here is
     utf-16, so it would buy nothing today.
  4. nothing ever evicts a `.didx` for a dictionary removed from the config; entries are
     keyed by path, so they are overwritten when a dictionary changes but orphaned when one
     goes away.

- **[x] #9 `dictu --search` cli + single instance.** `HANDLES_COMMAND_LINE` forwards a
  second invocation's argv to the running instance, which focuses the window and fills the
  search box — the path the global hotkey takes. plus the no-gui `dump` and `search`
  subcommands.
- **[x] #8 signal handlers hold weak refs, not strong `Ui` clones.** the cycle was real:
  the window owns the widgets, each widget owns its handlers, and every handler held a
  strong `Ui` — which holds the window. `Ui` is a plain struct, not a `GObject`, so
  `#[weak]` had nothing to attach to; it is now `Rc<UiInner>`, which `glib::clone!` *can*
  downgrade (`Downgrade` is implemented for `Rc`), so the eight handlers read
  `glib::clone!(#[weak] ui, …)` and every call site keeps working unchanged — the alias
  hides the `Rc` the way `SharedLibrary` already does. the alternative, capturing each
  widget a closure needs weakly, was rejected: the handlers call `Ui` methods that touch
  four or five fields each, so it would have meant upgrading half the struct per closure.
  upgrade failure is a quiet early return everywhere (`#[upgrade_or]
  Propagation::Proceed` for the key handlers). the honest limit: the app itself still owns
  one strong `Ui` for its lifetime, deliberately — that is the single-instance window
  being reused — so this changes no runtime behaviour; what it changes is that dropping
  the ui now actually frees it. proved mechanically by a unit test that builds the real
  widget tree (skipped when `adw::init` finds no display), drops the ui and asserts the
  `Weak` no longer upgrades, then destroys the window and asserts the window and the
  definition pane are finalized too. re-adding one strong capture makes it fail, so it
  isn't vacuous.
  reviewed independently before merging, and the review found the *guard* weaker than
  the fix: the test skipped silently with no display (libtest reports an early return as
  a pass, so the bug could be reintroduced and still go green), and concurrent runs lost
  the race for the test app's bus name, which made the window assertions non-load-bearing.
  both fixed — `DICTU_REQUIRE_DISPLAY` (set by `hack/check.sh`) turns a headless skip into
  a failure, and the test app is `NON_UNIQUE`. verified: reintroducing one strong capture
  fails the test, headless-with-the-flag fails, and six concurrent runs are clean.
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
- **[x] #15 (language tag) every wordlist row says which language it is.** there is no
  language field in a StarDict `.ifo`, so the tag comes from what's actually there: the
  script the headword is written in, which settles hebrew and greek outright whatever the
  dictionary is called, and failing that the language named in the dictionary's own title
  (`Dictionary latininfl -> english` → `LAT`, `French - English.csv (fr-en)` → `FR`,
  `Ref_LSJ` → `GRC`). when neither answers, the row shows the dictionary's name, which is
  the honest fallback the feedback asked for. `src/language.rs`, unit-tested, plus two e2e
  checks. the lemma-vs-inflection half is now #33.
  **known limit, found the hard way:** a title can lie about its direction. the
  French-English dictionary called itself `French - English.csv (fr-en)` but its headwords
  are english (`'em`, `'twas`, `house`) with french definitions — en→fr — so every row got a
  confident `FR` that should have been `ENG`. the script test can't help when both languages
  use latin script. it is excluded now (#34), so nothing in the collection hits this, but if
  a latin-script bilingual dictionary is ever added back, sample its headwords instead of
  trusting the title.
- **[x] #14 the search scope is yours to choose.** a popover off the header bar lists every
  loaded dictionary with its headword count and a checkbox; unchecking one drops it out of
  the `active` mask and re-runs whatever is in the search box, so the wordlist follows
  immediately. a popover rather than an `adw` preferences window **because this is not a
  setting**: it is a transient scope you flip mid-search, so it belongs one click from the
  search box and dismisses itself. (it also needs no `v1_5` feature bump for
  `AdwPreferencesDialog`.) the status line stays honest about what was actually searched —
  `6 words · 1 of 2 dictionaries`, `1 result · 1 of 2 dictionaries` — and says the scope
  note only when the scope is narrowed, so a full library reads exactly as it did before.
  deselecting everything is its own state (`0 dictionaries selected`, and the pane says to
  pick one) rather than an empty list that looks broken. six e2e checks drive the real
  popover over at-spi.
  **nothing is persisted, deliberately.** `Config::exclude` would have to `save`, and save
  rewrites `config.toml` through serde, discarding the comments that explain the #19/#34
  exclusions — see #38. permanent removal stays a hand edit; this panel is per-session.
  **known limit:** the mask scopes the *wordlist*, not the definition pane —
  `Library::lookup_all` still answers from every dictionary, so a word selected while a
  dictionary is out of scope can still show that dictionary's definition. arguably right
  (you asked for that word), but if it should follow the scope, that is a one-line change
  where the pane is built.

  reviewed by two independent passes before merging, which between them found more than
  the feature: a library with **no** dictionaries was being reported as "0 dictionaries
  selected" and pointed at an empty menu (the empty mask was doing double duty as "not
  built yet"); the definition pane and the fold strip went on naming a dictionary the user
  had just excluded, contradicting the status line beside them; Space on the new header
  button was being swallowed into the query, so the panel couldn't be opened by keyboard;
  typing while the panel was open vanished, because a popover is its own surface and the
  window-level key controller never sees it; `1 headwords`; a stale fixture assertion that
  would have gone red only after a conflict-free merge; and a `close_scope` wait that could
  never fail. all fixed here. the panel's checkbox handler also had to be rewritten weakly —
  as a strong capture it rebuilt the reference cycle #8 had just removed, in the one place
  #8's test cannot see (it runs from the indexing future, not `build_ui`).
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
- **[x] #36 a hotkey search lands on an answer.** `Super+F2` (`dictu --search WORD`) now
  selects the first result itself, so the window shows a definition instead of a list to
  click, and focus stays in the search box so you can keep refining. it is a flag consumed by
  the next set of results, not a timer, because `SearchEntry` debounces `search-changed` —
  there is no moment after `set_text` when the rows are known to exist.
  it also fixed a worse bug on the same path: firing the hotkey with **nothing running** both
  starts the app and fills the search box, so that search ran before there was an index and
  found nothing, and nothing ever re-ran it — the box sat there with a word and an empty
  wordlist until you typed another character. the search is now re-run once the index
  arrives (`rex` cold: 0 rows before, 27 rows with the first selected after).
- **[x] #35 entries under one headword are numbered.** `lookup` used to join every entry
  for a headword into one string with `<hr/>`, which `markup.rs` renders as a plain newline —
  so the 71 entries `sam` is filed under arrived as one run-together paragraph, which is what
  made noisy data look like a broken app. the trait now returns the entries (`Vec<String>`,
  empty meaning "not in this dictionary") and the pane prints `1 of 71` above each. the cli
  says it too: `dictu lookup … sam` reports `(71 entries)` and marks each one, and `dump`
  flags any headword with more than one. the dictd fixture files `byte` twice so an e2e check
  covers it.
- **[x] #32 the cli stops quietly on a closed pipe.** `dictu dump … | head -3` used to end
  with `failed printing to stdout: Broken pipe` and exit 101: rust ignores SIGPIPE, so
  `println!` panics on the `EPIPE` that `head` leaves behind. `dump`, `lookup` and `search`
  now write through one locked stdout handle and stop at the first failed write — a closed
  pipe is the reader's choice, so nothing goes to stderr and the exit code is 0. the crate
  denies unsafe, so restoring the SIGPIPE default was never an option; fixing the writes is
  also the faster path for the 20-line loops. `hack/check.sh`'s dump smoke stage asserts a
  truncated pipe leaves stderr empty. (the entry itself went missing from this list by
  accident in the #35 commit; restored here.)
- **[x] #34 the French-English dictionary is excluded** (on request). its title misstates
  its direction — see the note under #15 — and it was also the reason `rex` showed `FR`
  while the Latin dictionary defined it too. the library is 5 dictionaries / 1,469,846
  headwords now.
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

`~/Dictionaries`, 14 of 18 dictionaries loadable and kept (1,941,292
headwords):

| loadable | dictionary | format | headwords |
| --- | --- | --- | --- |
| no (#19) | Latin_English_Inflected (bgl-converted) | StarDict | |
| no (#19) | Latin_English_Inflected (stardic) | StarDict | |
| yes | latin infl+lewis | StarDict | |
| no (#34) | French - English (actually en→fr) | StarDict | |
| yes | MiddleLiddell | StarDict | |
| yes | a hebrew-hebrew dictionary (HEB-HEB) | StarDict | |
| yes | מילון אבן ספיר | StarDict | |
| yes | Ref_LSJ (abbreviations) | tab-separated csv | |
| yes | Klein, Comprehensive Etymological Hebrew | DSL | 27,620 |
| yes | Klein abbreviations (`_abrv`) | DSL | 186 |
| yes | Dodson, Greek-English Lexicon | DSL | 10,688 |
| yes | Liddell-Scott (`Liddell-Scott.dsl`) | DSL | 130,454 |
| yes | Liddell&Scott (`….dsl.dz`) | DSL | 115,076 |
| yes | Liddell&Scott again (`….dsl/Grc-Eng.dsl`) | DSL | 115,076 |
| yes | Hebrew and Aramaic Lexicon of the OT (HALOT) | DSL | 19,579 |
| yes | Larousse Chambers français-anglais | DSL | 47,419 |
| yes | Lexicon to Pindar | DSL | 5,348 |

the five StarDict/csv dictionaries account for 1,469,846 of those headwords and the nine
DSL files for 471,446. the last two Liddell&Scott rows are the same 115,076 headwords
twice — see the note under #13 — so excluding
`!…/Greek-English Lexicon - Liddell & Scott/Grc-Eng_L&S_Greek-English Lexicon_or_2.dsl`
in `config.toml` costs nothing and saves ~3.5s and ~200 MB at startup.
