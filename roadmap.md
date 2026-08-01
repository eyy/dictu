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

**this section is in the order it makes sense to do it in, not the order it arrived.**
finish what is started, then the small thing that unblocks three others, then the tools
that lie to you, then the features. the reasoning per item is in the item; what the order
encodes is: #59 (a dictionary's source language) is what #60, #64 and half of #52 are all
waiting on, so it is worth more than its size; #52 and #45 share one config table and must
land together; #63 and #62 are cheap and stop the feedback loop fighting whoever runs it.
completed items keep their place at the end of the section rather than moving to `## done`,
because their notes are what the open ones argue with.


- **[x] #58 Gaffiot was a wall of text.** reported while testing.
  its entries carry no block markup at all: the senses are `<b> ¶ 1</b>`, `¶ 2`, … exactly as
  the print edition marks them, and `||` divides a sense into sub-paragraphs. so the whole
  article arrived as one paragraph and `structure_body` had no lines to shape. a pilcrow now
  starts a line and keeps its character (the number after it names the sense, and a reader of
  Gaffiot knows the notation); `||` breaks the line and is dropped, since it names nothing.
  only Gaffiot uses either marker — 6 and 9 in `rex`, 3 and 3 in `amor`, none elsewhere — so
  the rules are safe. `rex` goes from one paragraph to six senses.
  **and then the indent, which took four wrong attempts.** a `||` line landed at the body
  margin, reading as though it had escaped the sense it belongs to, so `structure_body` now
  carries the current sense down the lines that follow it — one call covers one entry, so the
  state cannot leak between articles — and tags them with a margin of their own.
  what made it slow is that gtk renders these paragraphs **16px to the left of what the tag
  asks for**, and at-spi reports the tag's own value either way, so it cannot adjudicate
  between them. the 16 is the sense's hanging indent, applied to the paragraph after it as
  well. saying `.indent(0)` on the continuation tag does not override it; nor does removing
  the body tag from the line first, which was the attempt that should have settled it by
  removing the conflict altogether. what does work is asking for `SENSE_MARGIN + SENSE_HANG`
  and letting the leak subtract the hang back off — verified at the pixel, twice: asking 46
  rendered at 336, asking 62 renders at 351, and the sense's own wrapped text is at 351.
  it is written as that sum rather than as `62` so the two stay tied, and the comment says
  plainly that the mechanism is measured and not understood. **the lesson is the one already
  written under "reviews by angle": trace, do not reason.** three of the four attempts were
  reasoning about tag priority, and the thing that produced an answer was measuring the
  leftmost inked pixel of every line at two different tag values.
  a caution for whoever revisits it: my eye read the first screenshot as "aligned with the
  marker" and the pixels said 336 vs 351. eyeballing a 15px indent in a screenshot is not
  measurement, and I twice concluded the opposite of what was true before writing the
  five-line script that reads the ink.
- **[x] #59 a latin word was tagged with its dictionary's name, or with nothing at all.**
  two reports, one cause. `jactitabundus` shows `Gaffiot 2…` in the wordlist where it should
  show `LAT`; `Jabolenus` shows a bare `·2` where it should show `LAT ·2`.
  `language::tag` asks the script first — which settles greek and hebrew and says nothing
  about latin — and then looks for a language *named* in the dictionary's title. its list
  holds `"latin"`, `"french"`, `"greek"`; the titles here say `Lat-Fra` and `Lat-Eng`. so
  Gaffiot and Lewis & Short both answer `None`: a word only Gaffiot has falls back to the
  dictionary's name, and a word in both is two dictionaries that *disagree*, which hides the
  tag and leaves only the count. hence one symptom looking like a naming bug and the other
  like a missing tag.
  **done, and it needed exactly one needle.** the list already held `grc`, `heb` and
  `français`, which match the greek, hebrew and french titles as they stand — the whole gap
  was latin, whose two dictionaries call themselves `Lat-Fra` and `Lat-Eng` while the list
  knew only "latin". so `lat-` went in, spelt with its dash so that it is the *source* half
  of a `Src-Tgt` pair and cannot match a target (`Grc-Lat` is a greek dictionary). the
  entry I wrote predicted five needles; four of them were already there.
  measured on the real collection, which is where the report came from:
  | word | before | after |
  | --- | --- | --- |
  | `jactitabundus` | `Gaffiot 2016 (Lat-Fra)` | `LAT` |
  | `Jabolenus` | `·2` | `LAT ·2` |
  | `rex` | `·3` | `LAT ·3` |
  hebrew and french rows are unchanged (`מלך` → `HEB ·3`, `maison` → `FR`).
  the tests are unit tests over the collection's **real titles as string literals** — there
  is no external data in them, and they are the only place this can be checked, since the
  `sample/` fixture's two dictionaries name no language at all and never could. one test
  covers the two reported words; a second walks all fourteen titles, so a future change to
  the needles cannot quietly unname a dictionary that already worked.
  and a note against my own process: I ran `cargo test` and then read the tags off the app
  without rebuilding it, saw the old answers, and briefly believed the fix had not worked.
  the binary the harness launches is not the one `cargo test` builds. this is the second time
  in this project — it is in "reviews by angle" as *trace, do not reason*, and it wants a
  line of its own about stale binaries.
- **[x] #65 the wordlist tag says which languages, not how many dictionaries.**
  asked for after reading a row: "i dont want a count there. if its 2 or 5 latin dicts, it
  should only say lat. if its a latin dict and a french one it should say lat fr".
  this **reverses the visible half of #12**, and #12 deserves the credit for finding the trap
  rather than the blame for falling in it: it already knew `LAT ·2` was ambiguous *across*
  languages, which is why it only showed a tag when every dictionary agreed. what it did not
  see is that the same string is ambiguous *within* one language — `LAT ·2` reads as "latin
  twice" — and the person who read it that way is the one the row is for.
  so the tag is now the set of languages that answer, each once, in the order the
  dictionaries come: `Jabolenus` and `rex` read **LAT** where they read `LAT ·2` and `LAT ·3`,
  and `sale` and `pater` read **LAT FR** — three latin dictionaries and Larousse, which is the
  thing worth knowing before clicking. `מלך` reads `HEB`, `maison` reads `FR`. a dictionary
  whose title names no language still falls back to the title, and those dedupe too.
  the count is not lost, only moved off the row: the definition pane shows a section per
  dictionary, and the tag's tooltip still lists them by name. #60's per-dictionary chips are
  the same information where there is room for it.
  the row is one widget lighter for it — two labels instead of three — and the e2e checks that
  asserted `·2` now assert the language set, which is also where the deduplication is covered:
  the dictd fixture files "byte" twice and must not say anything twice.
  **and then the separator**, because "LAT FR" joined by a plain space read as one word — "too
  close to each other". they are joined by ` · ` now, which is the separator the status line
  already uses between facts ("1,941,344 words · 15 dictionaries"), so the row borrows an
  idiom rather than inventing one — and the glyph was free the moment the count stopped using
  it. the tag's width cap went 12 → 16 characters at the same time: it is there to cut a long
  fallback title, and `LAT · FR · GRC` should not be what it cuts.
- **[x] #62 the ui harness polices the whole machine no longer; it asks the bus.**
  `hack/e2e.py` refuses to run when *any* dictu process is alive, and `hack/check.sh` kills
  them first — both written when the suite shared the user's session bus, where a stray
  window really would intercept a forwarded `--search`. since #55 gave the harness a private
  bus that cannot happen: what answers our forwarding is whoever owns `io.github.eyy.Dictu`
  **on the bus we are talking to**. so the guard should ask
  `org.freedesktop.DBus.NameHasOwner` instead of scanning `/proc`, and check.sh's
  kill-and-wait-2s should go with it.
  worth doing because the old behaviour was actively hostile: it refused to run while you had
  the app open to read something, and the stage had twice killed a window mid-use. it also
  drops the argv special-case for `dump|lookup|search`, which never claim the name anyway —
  the bus knows that for free.
  **done, and demonstrated both ways.** with a window open on the user's session: the suite
  runs to 37 passed and the app is the same pid afterwards, where before it either refused or
  was killed. and run *bare*, sharing that session bus, it still refuses — with a message that
  now says which name is taken and that another bus would be fine. `check.sh` kills nothing
  any more; the machine-wide lock stays, but its reason is rewritten to what it actually is:
  two suites at once is two Xvfb servers, two gtk apps and two real collections of memory on
  a laptop that is already the limiting factor.
  the guard is four lines of `NameHasOwner` where it was a `/proc` scan with a special case
  for three subcommands. it also answers the question the scan only approximated: not "is a
  dictu running" but "would anything answer *our* forwarding".
- **[x] #60 every dictionary in the definition pane says its languages.**
  asked for while testing: each dictionary's heading in the definition pane wants a language
  chip like the wordlist rows have — `LAT → FR`, `LAT → ENG`, `GRC → FRA` — set to the right
  of the dictionary's name.
  the pair is in the titles already: `(Lat-Fra)`, `(Lat-Eng)`, `(Grc-Fra)`, `(Grc-Eng)`,
  `(Heb-Eng)`, `français-anglais`, `HEB-HEB`. so this wants the same parsing #59 needs for the
  source language, extended to return both halves — one function, two callers. what neither
  gets from a title is a dictionary whose name says nothing (`Middle_Liddell_stardict`,
  `stardic…`, `LSJ sources`), which is where #52's per-dictionary config table becomes the
  answer: a language pair is exactly the kind of thing a reader should be able to set by hand
  when the file does not say.
  **done.** `language::pair` reads the pair off the title and the heading carries it: the
  pane now reads `bgl-Latin_English_Inflected  LAT → ENG`, `Gaffiot 2016 (Lat-Fra)  LAT → FR`.
  the chip is its own tag — dimmer than the heading and *not* letter-spaced, because the
  heading is spaced out like a label and a three-letter code spaced out like that reads as
  three letters rather than one word.
  **adjacency is the whole parsing rule**, and it is what makes the parser safe: the two
  languages must sit either side of one `-` or `_`, which is how every title here says it
  (`(Lat-Fra)`, `Grc-Eng`, `français-anglais`, `Latin_English_Inflected`, `HEB-HEB`). the
  first thing that tried to be cleverer than that read "Hebrew and Aramaic Lexicon of the Old
  Testament" as hebrew→aramaic; those are two *sources* with three words between them. four
  of the fifteen name no pair at all — that one, `Middle_Liddell_stardict`, `LSJ sources` and
  `מילון_אבן_ספיר (BGL)` — and get no chip rather than a guess. all fourteen titles, pair and
  no-pair alike, are a unit test.
  the pair table is deliberately **not** the one `tag` uses: "liddell" tells you a dictionary
  is greek but is not a language, and matching it as half of a pair would make
  `Middle_Liddell_stardict` a pair with whatever came next.
  **beside the name, not at the pane's right edge**, which is what was asked for — a text view
  right-aligns a whole line, and pinning a run to the edge needs a tab stop at a pixel that
  stops being the edge the moment the pane is resized. worth revisiting only if the heading
  ever becomes a widget rather than a line of text.
  and an honest note on value: for a title like `Gaffiot 2016 (Lat-Fra)` the chip says what
  the name already said. it earns its place on the four that name no pair in prose and on the
  ones whose names are folder names — and it will earn it everywhere after #52, when the
  heading reads "Gaffiot" and the languages are no longer hiding in a parenthesis.
- **[x] #66 dictu is in the application grid, and everything leads to the latest build.**
  asked for: "i want dictu to appear in the application screen on my ubuntu; it should always
  lead to the latest build. the shortcut as well."
  the repo now owns three files under `hack/desktop/`: a launcher, a `.desktop` entry, and an
  installer that puts both under `$HOME` and can take them out again. run
  `hack/desktop/install.sh`.
  **"the latest build" is taken literally.** the launcher picks the newer of
  `target/release/dictu` and `target/debug/dictu` by mtime, so `cargo build` and
  `cargo build --release` each take effect the moment they finish and nobody edits a path.
  verified by flipping the two mtimes and watching which one it resolves to. with nothing
  built it says so through `notify-send`, because a launch from the grid has no terminal to
  complain into.
  the shortcut leads there too. it runs a script of its own —
  `~/.local/bin/dictu-lookup`, which reads the primary selection before handing the word
  over — and that script hard-coded the debug binary; the installer repoints its `DICTU=`
  line at the launcher. so the grid and the hotkey can no longer disagree about which build
  is current.
  the entry is templated rather than committed with a path in it (`@REPO@`, `@LAUNCHER@`),
  so nothing in the repository names a home directory. it validates against
  `desktop-file-validate`, and `gtk-launch io.github.eyy.Dictu` starts the app.
  **the icon is still #48.** the entry names `io.github.eyy.Dictu` and no such icon is
  installed, so the grid shows a placeholder next to the name until one is.
- **[x] #67 the app shows the global shortcut, and can change it.**
  asked for with #66: "i want to see and be able to redefine the global shortcut in the
  setting menu." today the shortcut is invisible from inside dictu — it is a GNOME custom
  keybinding, set up by hand outside the app, and the only way to find out what it is is
  `gsettings` or the Settings app. (it is `<Super>F2`; AGENTS.md said `Super+\` for weeks,
  which is exactly the kind of thing a window that showed you would have prevented.)
  it is readable and writable: `org.gnome.settings-daemon.plugins.media-keys`
  `custom-keybindings` is a list of paths, each with `name`, `command` and `binding` under
  `…media-keys.custom-keybinding`. so the app can find the entry whose `command` is our
  launcher-based script, show its `binding`, and rewrite it — creating the entry if there
  is none, which is also how this stops being a manual setup step for a fresh machine.
  wants a **primary menu** in the header bar, which the window does not have yet: the header
  carries only the scope button. so this is a small amount of new furniture — menu button,
  `adw::PreferencesDialog`, a shortcut row — plus the capture, which is the fiddly half: gtk4
  has no "record a shortcut" widget, so it means a key controller that takes the next
  combination and formats it as an accelerator (`<Super>F2`), and refusing the ones that
  would be absurd (a bare letter, a modifier alone).
  **done.** the header bar carries a **cog** beside the scope button, both at the start —
  it began as a hamburger with one item in it, which is a menu asking to be a button — and
  Preferences shows one row: what the shortcut is, what it runs, and a Change… button. it
  reads `<Super>F2` and `/home/you/.local/bin/dictu-lookup` off dconf — the reader's own
  configuration, not a copy of it — and writing goes back to the same place, so the Settings
  app agrees with us and always will.
  the plumbing is `src/shortcut.rs`, which is gtk-free apart from `gio`: `available` (is this
  even a GNOME desktop), `current`, `set`, and `sensible`. it **appends** to
  `custom-keybindings` when nothing is bound rather than writing over `custom0`, because that
  list belongs to every application that has ever added a shortcut — Albert owns `custom0`
  on this machine.
  `sensible` refuses what GNOME would accept without comment: a bare letter, which would
  swallow that letter everywhere on the desktop, and a lone modifier. a function key stands
  alone because nothing else wants one. it parses the accelerator itself rather than calling
  `gtk::accelerator_parse`, which needs gtk initialised and would have made a rule about
  strings into a test that only runs with a display.
  **verified without touching the binding**: drive the capture dialog and press the key it is
  *already* on. the dialog closes only on a successful write, so a close plus an unchanged
  dconf value is the write path proved harmlessly. `<Super>F2` before, `<Super>F2` after.
  **two at-spi gaps found on the way**, both worth knowing before anyone writes an e2e check
  here: a `gio::Menu` item in a popover is exposed with an empty accessible name, and an
  `adw::ActionRow` *suffix* widget is not exposed at all — the Change… button is invisible to
  the harness, which is why that verification went through the keyboard.
  still open, and deliberately: what to do when the chosen combination is already taken by
  something else. GNOME will let two entries claim one key and then honour neither
  predictably, and detecting that means reading every binding in every media-keys schema, not
  just the custom ones.
- **[ ] #69 when nothing is searched, show the dictionaries.** *(the empty pane is no longer
  empty — #48 put the incipit there — so this is now a question of what goes **beside** it,
  or below it, rather than what fills a blank.)*
  asked for: "when nothing is searched, i want to see a list of my dicts". the wordlist is
  empty until you type, and the pane says "Type to search all dictionaries." — which is a
  hint where there could be the collection itself: fifteen dictionaries, each with its
  headword count, which is the one moment there is room to show them.
  the material is already there and already assembled twice: `Collection::dict_count` and
  `dict_label`/`dict_headwords` feed the scope panel's rows, and `library_size` writes the
  status line. so this is a question of *what the empty state is for* rather than of new data.
  worth deciding before building: is the list the **wordlist** (rows in the sidebar, so
  clicking one could scope the search to it, which is #45/#64 territory) or the **definition
  pane** (a proper cover page — the collection, its size, maybe when it was last indexed)?
  the pane is where there is room to be generous, and it is already what shows a message
  today. the sidebar is where a *list* belongs, and the wordlist is a model now (#57), so
  putting dictionaries in it means a second item type or a separate model to swap in.
  and it should say something true when there is nothing: no dictionaries configured is a
  different empty state from nothing typed, and #61 showed how much a control that looks
  broken costs — the same applies to a window that looks empty.
- **[ ] #68 online dictionaries — Urban Dictionary, etymonline.**
  asked for: the collection is fifteen files on disk, and some of what a reader wants next is
  only online. Urban Dictionary for what no lexicon will admit exists, and **etymonline** —
  the Online Etymology Dictionary — which for english is the thing Klein is for hebrew, and
  which this collection has no equivalent of at all.
  the shape is the question, not the fetching. dictu is an *offline* dictionary whose whole
  performance story is mmapped local files, so an online source is a different kind of thing:
  it can fail, it is slow enough to need a spinner, and it must never make the wordlist wait.
  so it is probably **not** another `Dictionary` implementation behind the same trait — that
  trait promises `lookup(word) -> Vec<html>` synchronously, and a network call behind it would
  block the search that #56 just made instant. more likely a section in the definition pane
  that arrives late, on its own, after the local answers are already on screen.
  and it needs deciding whether a lookup leaves the machine at all by default. a dictionary
  that phones home for every word you read is a different privacy proposition from fifteen
  files in Dropbox; the honest default is off, with the reader turning it on per source.
  no scraping questions until that is settled: etymonline has no public api and its terms
  matter, Urban Dictionary has an unofficial one that comes and goes.
- **[x] #72 HALOT's headwords wore a stray `V`, and 5,224 of them were like it.**
  reported: "hebrew and aramic lexicon has lemmas start with a mysterious V". reproduced —
  the wordlist carries `V אָדָם`, `V אֵל`, `V בַּד`, `V בֶּלַע`, `V חמר`.
  the `V` is **ours, not theirs**. HALOT numbers its homonyms in roman numerals inside
  parentheses, and in DSL a parenthesised part of a headword is the *optional* part — the
  bit a search need not type. counted in the file:
  | in the source | how many |
  | --- | --- |
  | `(\*)` — HALOT's mark for an unattested form | 2,134 |
  | `(I)` | 1,680 |
  | `(II)` | 1,552 |
  | `(III)` | 284 |
  | `(IV)` / `(V)` / `(VI)` | 56 / 18 / 6 |
  so `V אָדָם` is the fifth homonym of *ādām*, and it looks like nonsense because the reader
  keeps the optional group's *contents* as a second headword while dropping the parentheses
  that explained them. it indexes both forms — a clean `אָדָם` row exists too — so the effect
  is a duplicate row wearing a numeral. about **5,700 headwords** are affected, which is a
  fifth of HALOT.
  what the spec asks for: an optional group is optional *for matching*, so `(I) אָדָם` should
  be findable as both `אָדָם` and `(I) אָדָם` while being **displayed** as one word. the fix
  is in `dict/dsl.rs`'s headword handling, and it should decide three things: which form is
  the display word (the one without the group), whether the numeral is worth showing at all
  (it distinguishes homonyms, so probably beside the definition rather than in the list), and
  what to do with `(\*)`, which is not a numeral but a genuine mark meaning "unattested".
  #33's folding is the near neighbour here: two rows for one lemma is exactly what it exists
  to prevent, and it cannot help while the two spellings differ by a visible `V`.
  **done, and the rule is about *where* the group sits.** a **leading** optional group is a
  marker — halot uses dsl's optional syntax to keep its homonym numerals and its
  unattested-form asterisk out of the search index — while a **trailing** one is the word
  carrying on: `ad lib(itum)` is one entry and reads "ad libitum". so every variant stays
  searchable and only the unmarked ones are *listed*.
  that distinction matters more than it looks: `build_order` walks `headwords()`, the
  **display** list, so an entry dropped from it is not merely unlisted but **unfindable by
  typing**. the first version of this fix listed one form per line and would have made
  `ad lib` untypeable — the existing test caught it, which is the second time this week a
  test written for the old behaviour turned out to be protecting something real.
  halot then needed a second rule, because it is inconsistent with itself: `((\*)IV) בַּד`
  puts the numeral inside the parens and `(\*)V בַּד` leaves it outside, as ordinary text.
  so a **leading roman numeral standing before a word in another script** is dropped from
  the display too — deliberately narrow, since a latin dictionary may well have `V` or
  `I` as a headword of its own, and one standing alone has nothing after it to strip.
  measured: **1,941,344 → 1,936,120 listed headwords, 5,224 fewer** — and `#53`'s speed
  check refused to compare across the change and asked for a re-record, which is exactly
  what it is for. the cache version went 5 → 6, since a v5 index lists headwords a v6 one
  does not.
- **[ ] #71 remember the scope: autosave it, and restore it next time.**
  asked for: changes in the search-scope panel should be written to the config and be there
  again next launch. today they are not — the panel's own hint says so out loud: "Narrows
  **this session's** searches. What gets loaded at all is config.toml's business."
  so this changes a stated contract, and that hint has to change with it. it also needs a
  home in the file: scope is a per-dictionary flag, exactly like #52's name and #45's rank,
  so it wants the same table keyed by path rather than a fourth list of paths. **do it with
  #45/#52 or it will be rewritten by them.**
  #38 already made writing `config.toml` safe (append-only, comments kept), so the writing
  is not the problem. the questions are *when* — a write per checkbox click is a write per
  click, so debounce it or save on close — and what happens when the file disagrees with
  the disk: a remembered exclusion for a dictionary that is no longer there must be kept
  rather than dropped, or unplugging a drive silently forgets the reader's choice.
  and one honest thing to decide: "narrow this session" is a *useful* mode. if scope becomes
  permanent, the panel probably needs both — a persistent choice, and a way to try something
  without committing to it.
- **[ ] #70 Ctrl+Q should close the app.**
  asked for, and it is the one shortcut every gnome application has. the window has no
  accelerators of its own at all today: `Escape`, `Ctrl+W` and `Ctrl+Q` all do nothing.
  small — an `app.quit` action with `set_accels_for_action`, which also puts it in the
  shell's own shortcut list — but worth doing as a set rather than one key at a time, and
  worth deciding whether `Escape` clears the search box (useful) or closes the window
  (surprising, given the search box is where focus usually is).
- **[ ] #64 group the dictionaries by source language.**
  the scope panel lists fifteen dictionaries in one flat alphabetical run, so the languages
  are interleaved: Bailly (Grc-Fra), then bgl-Latin_English_Inflected, then two Hebrew
  etymological ones, then Gaffiot (Lat-Fra), then two Greek lexica, then a hebrew-hebrew dictionary. a
  reader narrowing a search thinks "the Greek ones" or "just Klein", never "the ones starting
  with B", and the list is now long enough that finding one costs a scroll and a scan.
  wants **#59 first**: grouping by source language means knowing it, and today the tagger
  cannot even name Gaffiot's. once it can, `build_scope` fills `scope_list` in one pass and
  can just as easily fill a group per language — `gtk::ListBox` takes a header function, or
  the panel becomes one `adw::PreferencesGroup` per language with its own title (GRC, LAT,
  HEB, FR), which is the shape libadwaita is built for and reads better in a popover.
  two things to decide rather than assume. **what a group is called** for a dictionary whose
  source is its own target — a hebrew-hebrew dictionary is HEB-HEB, a Hebrew dictionary of Hebrew — and for
  the ones whose titles say nothing at all (`Middle_Liddell_stardict`, `LSJ sources`), which
  is #52's config table again. and **how this meets #45**: if the reader can order the
  dictionaries by hand, does their order sort the groups, sort within a group, or replace the
  grouping? cheapest coherent answer is that grouping is the panel's layout and #45's rank
  orders *within* a group, but that is a decision, not an obvious truth.
  note this is about the *panel*. the order dictionaries answer in — which section comes first
  in the definition pane — is #45, and the two should not be conflated: one is where a control
  sits, the other is what the reader reads first.
- **[ ] #63 the speed check compares against a baseline it cannot know the clock of.**
  it failed during #61 on a change that was a popover's width — and `dictu bench` builds no
  widgets at all, so the code could not have been the cause. every measurement was ~1.5×
  the baseline *uniformly*, including a query that takes 0.3 µs, which is the signature of a
  slower cpu rather than slower code. and so it was: `powersave`, **1,059 MHz average against
  a 5,200 MHz maximum**, load 2.98 after hours of builds. one query crossed the 1.6×
  tolerance and the stage went red.
  so #53 is measuring the machine as much as the app, which is the one thing it was built to
  avoid. the fix is to make the comparison clock-aware rather than to widen the tolerance
  until nothing fails: **calibrate**. run a tiny fixed cpu-bound loop in the same process,
  record its time in the baseline beside everything else, and scale the comparison by how
  much slower that loop is now — a 5× downclock then shows up as a 5× calibration and the
  ratios come out flat. failing that, record the governor and average MHz in the baseline and
  say plainly that a comparison across a different clock is not a comparison.
  what must not happen is re-recording a baseline to make a red stage green: that turns the
  check into a rubber stamp. it did the honest thing here — it noticed something changed —
  it just could not say what.
- **[ ] #52 give every dictionary a name a person would write.** the labels are folder names
  and they read like it: `bgl-Latin_English_Inflected`, `Middle_Liddell_stardict`,
  `HEB-HEB a hebrew-hebrew dictionary`, `Greek-English Lexicon by John Jeffrey Dodson (Grc-Eng)`,
  `מילון_אבן_ספיר (BGL)`. they are the heading over every definition and the tag on every
  row, so they are read more often than anything else in the app.
  **this wants the same config structure #45 does.** a name is a per-dictionary preference,
  exactly like a rank, and `dictionary_dirs` is a list of *paths* with nowhere to hang one.
  a table keyed by path — `[dictionary."…/Middle_Liddell_stardict"] name = "Middle Liddell"`,
  with #45's `order` beside it — gives both a home, and #38 made writing that file safe.
  do the two together or the second will want the first rewritten.
  a name is not enough on its own: the wordlist tag is ellipsized at 12 characters, so
  something long needs a short form too (`Liddell & Scott` → `LSJ`). the language tag already
  covers greek and hebrew rows, which is why this is mostly felt on latin and french ones.
  **the name should not carry the language pair, because #60's chip now does.** every heading
  shows `LAT → FR` beside the name, so `Gaffiot 2016 (Lat-Fra)` says it twice — the name is
  where the *work* goes, the chip is where the languages go. so the names below drop the pair
  wherever it is only a pair, and keep it wherever it is part of the title or says something
  the chip cannot: "Greek-English Lexicon" is what Liddell & Scott is *called*, and "NT Greek"
  after Dodson is a dialect rather than a language.
  and this has to be a hand-written name rather than a strip of the folder name, which is the
  cheap version and does not survive its three obvious cases: `(Lat-Fra)` comes off cleanly,
  `bgl-Latin_English_Inflected` becomes `bgl-_Inflected`, and `Greek-English Lexicon - Liddell
  & Scott` loses the beginning of the work's own title. the chip is derived; the name is not
  derivable, which is the whole reason #52 exists.
  a first cut, to argue with rather than adopt:
  Whitaker's Words · Lewis & Short · Gaffiot · Bailly · Liddell & Scott (LSJ) ·
  Middle Liddell · Dodson (NT Greek) · Lexicon to Pindar · LSJ sources · Klein Etymological ·
  Klein abbreviations · HALOT · a hebrew-hebrew dictionary · Even Sapir · Larousse.
  with the pair dropped, that list is also what the *pane* headings read — `Gaffiot`
  `LAT → FR` rather than `Gaffiot 2016 (Lat-Fra)` `LAT → FR` — which is when #60's chip stops
  repeating anything and starts being the only place the languages are said.

- **[ ] #45 let the user order the dictionaries, and sort results by that order.**
  the scope panel lists dictionaries in scan order (`config::scan` sorts by label) and the
  wordlist inherits whatever the merged index hands back, so which dictionary answers first
  is an accident. it should be a preference: drag the list into the order you trust, and
  have both the rows and the definition pane's sections follow it. two parts — a reorderable
  list in the panel, and an ordering the search respects — and the second is the one with
  teeth: results are ordered by key today, and dictionary rank has to sort *within* a word
  without breaking #12's attribution or the row limit. the order persists in **its own key**,
  written by the app — not by reordering `dictionary_dirs`, which is hand-written and
  commented, and which #38 deliberately only ever appends to.

- **[ ] #49 back and forward.** there is real navigation now and no way to retrace it: a
  definition can be reached by typing, by picking a row, by the global hotkey, and — since
  #20 — by clicking a link inside another definition, which also rewrites the search box.
  follow two cross-references and the way back is gone.
  what a history entry has to hold is the whole question. the pane shows a *row*, but a row
  is built from a query under a scope, so remembering only the word would send you back to a
  definition beside a wordlist that no longer contains it — the exact inconsistency #43 and
  #33 were spent closing. an entry is at least the query text and the word shown; whether it
  also pins the scope and the fold setting is the design decision.
  the plumbing exists: `UiInner::shown` already tracks the word the pane is on, and
  `show_word`/`resolve` are the single door every navigation goes through. buttons belong at
  the start of the header bar, gnome-style, and should answer `Alt+Left`/`Alt+Right` and the
  mouse's back/forward buttons too — those are how anyone actually uses this.

- **[ ] #51 triple-click any word to look it up.** #20 made *links* clickable, which covers
  the cross-references a dictionary chose to mark. everything else in a definition is inert —
  and in these dictionaries most of what you want next is inert: a latin gloss inside a greek
  entry, a hebrew cognate in Klein, a word in a quotation. the global hotkey already does
  this for text anywhere else on the desktop by reading the primary selection; inside our own
  window it should not need a round trip through the clipboard.
  the click plumbing is there — `link_at`/`follow_link` already intercept in the capture
  phase, because the textview's own drag-select otherwise eats the release. a third press is
  another arm of the same handler, claiming the sequence so gtk does not also select the
  line, and landing in `show_word`, which resolves by bare key and so copes with an inflected
  or pointed form.
  two things to get right. **what a word is**: gtk's own boundaries are pango's, which is
  what makes this work for `λόγος` and for hebrew with niqqud, but they will also stop at the
  dots in `Cic. Rep.` and split `rēgis` if the macron is decomposed — worth testing on real
  entries rather than english. and **what it does**: links fill the search box as well as the
  pane, so the wordlist agrees with what is shown; a triple-click should do the same rather
  than inventing a second kind of navigation.

- **[ ] #47 resolve a citation where it is written.** LSJ's prose is mostly references —
  `Cic. Rep. 2, 30`, `Hdt. 2, 35`, `Alex.Aphr. in Metaph.` — and the collection already
  holds the key that decodes them: `LSJ sources`, 2,042 entries mapping each abbreviation to
  its author and work (`Alexander Aphrodisiensis Philosophus, in Aristotelis Metaphysica`).
  today that is a second search you have to run by hand, in a wordlist that also has to show
  you the citation rows. resolving them in place — a hover, or the underline treatment #20
  already built for links — turns a wall of abbreviations into readable prose.
  the interesting parts, none of which are the tooltip:
  1. **finding them in rendered text.** matching has to be longest-first (`Cic. Rep.` before
     `Cic.`), tolerant of the spacing the two sides disagree on (the key is `Alex. Aphr.`,
     LSJ writes `Alex.Aphr.`), and conservative — a rule that fires on `a` or `id.` makes
     the pane unreadable in the other direction. require a dot and a minimum length.
  2. **which dictionary's abbreviations.** this key is LSJ's. Bailly and Gaffiot cite in
     french with their own lists, and Lewis & Short has a third. the map has to be chosen
     per dictionary section, not applied globally, or Gaffiot's `Pl.` (Plaute) will be
     resolved with LSJ's `Pl.` (Plato).
  3. **whether `LSJ sources` should still be a dictionary.** it is one today, so searching
     `Cic` returns seven citation rows before any word. once its content is reachable inside
     definitions, the honest answer may be that it belongs in the index but not the
     wordlist — which is a new idea for the app (a dictionary that answers but does not
     list) and worth deciding deliberately.

- **[x] #48 a logo — the word itself, from the manuscript that named it.** the app had no icon: the shell shows a generic placeholder in the
  dash, the alt-tab switcher and the window list, which is also what a user sees before they
  see anything else. needs an app icon under the id it already claims
  (`io.github.eyy.Dictu`), in the hicolor theme, plus a `.desktop` file so the shell can find
  it — today the app is launched from a terminal and never installed. an `adw::AboutWindow`
  would then have somewhere to put it.
  **the subject is decided: the word itself, from a manuscript or an inscription** — `dictu`
  or `dictionarius`.
  of the two, **`dictionarius` is the stronger**, because it is where the word came from:
  John of Garland's *Dictionarius* (Paris, c. 1200) is the first book to carry the name. he
  wrote it for his students as a list of the trades they saw in the street, in latin **with
  Old French glosses between the lines** — which is very close to what this app is for, and
  makes the icon a small argument rather than a decoration. it survives in ~26 copies.
  `dictu` is harder to find in stone: it mostly occurs inside a phrase (*mirabile dictu*), so
  epigraphy is the less promising half of the brief.
  **where to look**, none of it a single search away: Digital Bodleian catalogues copies of
  the *Dictionarius* (`medieval.bodleian.ox.ac.uk/catalog/work_2560`, which refuses automated
  fetches — browse it), the Biblissima IIIF aggregator, Gallica, e-codices. Wikimedia Commons
  has only three Johannes de Garlandia items and **none of the *Dictionarius***; the closest
  is Biblioteca Medicea Laurenziana Plut. 25 sin. 5, a 13th-century *Synonyma* bound with
  Villedieu's *Doctrinale* — the right hand and the wrong word.
  **what makes this harder than picking a picture.** an app icon is read at 32–64 px in the
  dash, the switcher and the window list, so a manuscript *page* becomes grey mush: the asset
  has to be a **tight crop of one word, or of the initial D**, at high contrast. and gnome
  wants two icons, full-colour *and* symbolic — a photograph cannot be symbolic, so the
  symbolic one has to be a **redrawn letterform** tracing the same hand. so the deliverable
  is two things from one source: a cropped image, and a monochrome redraw of its lettering.
  the installation side is already done: #66's `.desktop` names `io.github.eyy.Dictu`, so an
  icon of that name in the hicolor theme is picked up by the shell with no further work — and
  until one exists the grid shows a placeholder beside the name.
  and **record the provenance beside the asset**: shelfmark, folio, library, licence. many
  libraries assert rights over their scans of public-domain originals, and "strictly for
  private use" is a reason to note where a picture came from, not a reason not to.
  **found, and it is the right page: St John's College MS 235, Fragments 62 & 68** —
  catalogued as "John of Garland, *Dictionarius* (binding fragments)", Anglo-Norman and
  latin, **between 1300 and 1315**, digitised by the Bodleian at ~2940×1975 across four
  images (62r/v, 68r/v).
  ```
  object   digital.bodleian.ox.ac.uk/objects/4021d35f-e1df-409f-a5b9-96dfa8cd417b/
  record   medieval.bodleian.ox.ac.uk/catalog/manuscript_12542
  manifest iiif.bodleian.ox.ac.uk/iiif/manifest/4021d35f-e1df-409f-a5b9-96dfa8cd417b.json
  62r      iiif.bodleian.ox.ac.uk/iiif/image/3b4ed413-f9bd-460f-a857-2d6e5f592d22
  ```
  **fragment 62 recto carries the incipit**, in the lower right column: a red initial
  followed by `ictionarius ū iste libellus adicionib[us] magis` — the word itself, in a hand
  of about 1310 — and the line above it has `dictionarius` again, abbreviated with a macron.
  the region on 62r is roughly `1875,1345,380,120` in the IIIF image's own coordinates.
  **but the initial cannot be the icon: it reads as an O.** it is a lombardic *D*, whose
  stem is absorbed into the bowl, so what survives at 48 px — where an icon actually lives —
  is a plain oval. checked at 300, 192 and 48 px before believing it. an icon whose whole
  job is to say *dictionary* must not read as the wrong letter, so this rules out the
  cheapest use of the find rather than the find itself.
  **and the photograph is claimed**: the manifest states no licence and attributes "Photo: ©
  The President and Fellows of St John's College, Oxford". the manuscript is 700 years old;
  the scan is not, which matters the moment anything leaves this machine.
  so the find is best used as a **wordmark** — the incipit line in the about window and the
  README, cited to the shelfmark — with the *icon* redrawn from it: the same hand, the stem
  straightened until a D reads as a D at 48 px. that also settles the symbolic variant, which
  a photograph could never have been.
  **done, and the choice went the other way: legibility is not critical here.** the icon is a
  square detail of the page — the abbreviated `dcōnarius` with its macron above, the red
  initial's bowl below — at 48/64/128/256 px in the hicolor theme, installed by
  `hack/desktop/install.sh` under the id #66's entry already names. the initial is still an
  O-shape at icon size; that was ruled a feature rather than a fault, since what the icon has
  to be is *a scrap of that manuscript*, not a letter.
  **and the whole incipit is the app's opening page**: with nothing searched, the definition
  pane shows the picture, the latin, a plain gloss of it, the date, and the credit. it replaces
  a one-line hint that said "Type to search all dictionaries." — which the search box's own
  placeholder already says.
  the licence turned out to be **CC BY-NC 4.0** (Digital Bodleian's terms), not the
  all-rights-reserved the manifest's attribution suggested — so the picture may be used with
  attribution, which is *why the credit is on the page* rather than only in
  `assets/ATTRIBUTION.md`: on a non-commercial licence attribution is a condition, so an e2e
  check asserts the credit line is still there. a redesign that quietly dropped it would be a
  licence breach rather than a visual regression.
  **and then it became a page rather than text**, because the picture had to centre when the
  pane is widened and shrink when it is narrowed — which text in a `TextView` cannot do: it
  draws an inline paintable at its own size and clips the rest. so the opening page is now
  widgets in a `gtk::Stack` beside the definition pane, one shown at a time: a `Picture` that
  `can_shrink`, a clamp, and labels. the shelfmark is a **link** to the Bodleian page the
  picture came from, so the credit is checkable rather than merely asserted.
  **`adw::Clamp` is what makes it behave.** a `Picture` set to `Contain` scales *up* as well as
  down, so on a 1500 px pane the incipit grew to fill it instead of sitting at its own size —
  seen in a screenshot, not reasoned about. clamped to 560 it centres at natural size when
  there is room and shrinks with everything else when there is not, verified at 620, 900 and
  1500 px.
  two things it cost, both worth knowing. **a `gtk::Stack` shows only its visible child to
  at-spi**, so the harness could no longer find the definition pane at construction — with the
  fixture, which indexes in milliseconds, the cover is already up — and `Widgets.definition` is
  now resolved on use instead. and the icon still has **no symbolic variant**, because a
  photograph cannot be one; the shell falls back to the colour icon where it wants a symbol.
  the icon has rounded corners at each size, cut with a mask rather than drawn.

- **[ ] #50 an english–english dictionary, oxford if it can be had.** the collection reads
  *into* english and has nothing that defines english itself: thirteen of the fifteen
  dictionaries are greek, latin or hebrew, and the other two are french. every gloss lands
  in a language the app cannot then explain.
  **oxford is the ask and probably the one thing that cannot be bought as data** — the same
  wall #44 hit with every post-1950 lexicon. the OED is a subscription, and the Oxford
  Dictionary of English ships inside other people's apps (Apple's Dictionary.app licenses it)
  rather than as a file anyone sells. worth checking properly rather than assuming, since
  that is what the #44 research was for; if it exists at a price, say the price.
  what certainly exists, free and in formats we already read: **GCIDE** (Webster's 1913 plus
  decades of GNU revisions, dictd, superb prose and blind to anything after ~1990),
  **WordNet** (modern, complete, terse to the point of curt, and structured as synsets rather
  than articles), the **Century Dictionary** (public domain, enormous, scanned), and English
  **Wiktionary**, which #37 already brings in for five other languages and would cover the
  modern vocabulary the others miss.
  the honest shape of the answer is probably "one old and good plus one modern and thin",
  the way Lewis & Short now sits beside Whitaker — so measure the overlap before installing
  three of them.

- **[x] #56 a search from the hotkey waits 150 ms for nothing.** `--search WORD` — the path
  the global hotkey takes — sets the search entry's text, and `gtk::SearchEntry` then
  debounces `search-changed` by its own `search-delay`, 150 ms by default. the debounce is
  there so that typing does not re-search on every keystroke; a word arriving complete from
  outside has nothing to coalesce, so that 150 ms is pure lag on the one path where the user
  has already committed to the word. found while profiling #55, where it is ~2.3 s of the
  suite, but the reason to fix it is the app: press the hotkey and the answer is a sixth of a
  second late for no reason.
  **done:** the forwarded path searches the moment the word arrives and selects the first
  row itself, so `auto_select` is gone — it existed only because the debounce left no moment
  at which the rows were known to exist, and now there is one. measured on the e2e suite,
  interleaved: **12.87 s → 10.2 s**, and the wait for a row after `--search` went from
  ~150 ms to ~15 ms.
  the interesting part is what it took, because two fixes that looked right were not.
  `set_text` is **not one change**: it deletes and then inserts, so it emits `changed`
  twice, and although `SearchEntry` debounces a search it emits for an *emptied* box
  immediately. filling the box therefore looks like `search-changed("")` synchronously plus
  `search-changed("word")` 150 ms later.
  attempt one armed a guard only when the text actually differed — but delete-then-insert
  emits even when the result is identical, so the second of the suite's two consecutive
  `--search aardvark` calls went unguarded. attempt two armed it always, and the
  synchronous `("")` emission consumed the guard before the real signal arrived, which also
  meant a whole-library search on every hotkey press. what settled it was tracing the
  emissions instead of reasoning about them; the fix blocks the handler across `set_text`
  so neither half is visible, and the guard then only has the debounced signal to swallow.
  the failure mode is invisible to an assertion made straight after the search — the row is
  deselected 150 ms later, and `populate_results` leaves the definition pane alone, so the
  pane still reads correctly while nothing is selected. hence a 35th e2e check that outlasts
  the debounce and looks again. it is a guard on the new mechanism rather than on the old
  behaviour: it passes on the pre-#56 code too, where the single debounced search did the
  selecting. it caught both wrong attempts, which is what it is for.  **and a bug of its own, found the next day by an unrelated check.** the guard was armed
  only for a non-empty word and never *cleared* for an empty one — so after a hotkey search
  for `byte`, clearing the box and typing `byte` again by hand did nothing at all: the
  wordlist sat empty with the word in the box. same asymmetry as the rest of this item: the
  emission that would have cleared the guard is the synchronous `search-changed("")` an
  emptied box sends, which is precisely the one the block hides from us. it is set
  unconditionally now, so an empty word clears what a previous one left behind.
  how it surfaced is the part worth keeping: not by review, but because a *cover-page* check
  happened to search a word, clear it, and then type the same word — a sequence no test had
  ever run. the bisect that found it took four trials, each differing by one step.

- **[x] #54 warm start was 1.0 s, and #44 measured 0.39 s — it is 0.23 s now.** found by
  #53 the day it was written, on the same 15 dictionaries and the same 1,941,344 headwords.
  **warm open 1.0 s → 0.23 s and peak RSS 360 MB → 172 MB**, so it ended up well past the
  number it was measured against, and prefix search came along for the ride: `rex` 12.0 →
  7.5 µs, `a` 252 → 151 µs, `esse` 183 → 122 µs, presumably for want of 188 MB of heap.
  no bisect was needed in the end, because profiling the phases answered it outright. the
  merged index everyone would suspect loads in **0.4 ms**; the config and directory scan are
  0.7 ms together. all of it was in opening individual dictionaries — and of those, three
  files were 655 ms of 710:
  | dictionary | `.dict` | open |
  | --- | --- | --- |
  | Lewis and Short 1879 | `.dict.dz`, 21.1 MB | 572 ms |
  | Bailly 2020 | `.dict.dz`, 8.9 MB | 245 ms |
  | the other 13 | plain `.dict` | 0.0–0.1 ms |
  **the two compressed dictionaries were being gunzipped in full, into RAM, on every single
  launch** — and then kept there for the life of the process, which is where a good part of
  that 188 MB went. the other thirteen are memory-mapped for nothing, which is why the
  comment in `build_index` said the payload was unnecessary because "the `.dict` its ranges
  point into is already a file we can map": true of a plain file, false of a `.dz`, and
  nobody had looked since.
  the fix was already in the codebase, one module over: `dsl.rs` unpacks its decoded text
  into the cached index's **payload** once and maps it after. StarDict now does the same when
  its `.dict` is compressed — decided by the gzip magic bytes rather than the file name,
  since a plain `.dict` that is secretly gzip is a case the old loader handled too. a stale
  cache from before this carries an empty payload, so those entries rebuild on sight rather
  than the cache version being bumped: only the two `.dz` dictionaries pay, once, 1.5 s.
  **no fixture covered a compressed `.dict`** — the whole test module writes plain ones — and
  the warm path is precisely where a missing payload would answer *nothing at all* while
  everything else looked fine. two tests now cover it, and the compressed one fails with
  `left: []` if the payload is dropped. the plain one asserts the cache does **not** carry a
  second copy of a file we can already map, which is the mistake the other direction.
  the moral for #53's sake: the number rotted for as long as nothing watched it, and what it
  cost was not subtle — a fifth of a second and 188 MB on every launch, for two files.
- **[x] #61 the search-scope button did nothing, and looked like it meant nothing.**
  reported while testing: "there's a menu button at the top that does nothing at all",
  then "it should have a tooltip; i had no idea what it should do".
  **it did nothing because the panel could not be shown.** a popover asks for its natural
  height, and the dictionary list was appended straight into it with no scroller — fifteen
  rows of title-plus-subtitle ask for more than a screen, and at that point gtk maps nothing
  at all. clicking the button did exactly nothing, silently. the list now sits in a
  `ScrolledWindow` capped at 420px, with the options row left outside it so it cannot scroll
  away; the panel opens with all 15 dictionaries (16 check boxes).
  **the tooltip was there all along.** two diagnoses of mine were wrong before a screenshot
  settled it: the accessible *description* is empty on the button (gtk does not derive one
  from a tooltip — the Minimize/Maximize/Close descriptions come from gtk setting them
  explicitly), so that was no proxy; and setting the tooltip on the `MenuButton`'s inner
  toggle changed nothing, because the outer one already worked. hovering it on a private
  display and taking a picture shows "Search scope" rendered under the button. what made the
  button feel meaningless was that pressing it did nothing — a tooltip on a control that
  appears broken is not read as an explanation.
  **the fixture could not have caught this**, which is the lesson worth keeping: two
  dictionaries make a small popover, so all 36 e2e checks passed against a panel that could
  not open on the real collection. the 37th checks the structure that prevents it — the
  dictionary list has a scroll-pane ancestor — since the fixture can never reproduce the
  height itself.
  **and a tail this item was closed too early on.** the scroller introduced a warning on
  every launch — `GtkBox reports a minimum width of 14, but minimum width for height …
  Expect overlapping widgets` — which `min_content_width` on the scroller did not cure; it
  only delayed it past the moment I checked the log, and I said the log was clean when it was
  merely clean *yet*. the cause was a row, not the scroller: `adw::ActionRow` wraps a long
  title, "Greek-English Lexicon by John Jeffrey Dodson (Grc-Eng)" ran to two lines, and a
  wrapping label's width depends on its height — a contradiction inside a list that never
  scrolls sideways. `title_lines(1)` and `subtitle_lines(1)` fix it at the source: the rows
  are uniform height, one more dictionary fits, and long names ellipsize until #52 gives them
  names worth showing. the fixture does not reproduce this either (its two titles are short),
  so the repro is the real collection with the panel open — 1 critical before, 0 after.
- **[x] #57 the wordlist is a model, not a list of widgets we rebuild.** asked whether
  the ui could be more declarative; of five candidates this is the one worth doing, and it is
  worth doing for correctness rather than for brevity.
  `populate_results` removes every child, builds a `gtk::Box` per row, appends it, then patches
  each row's accessible name — and keeps `words: Vec<Row>` as a shadow copy that the selection
  handler reads **by row position**. its own comment names the hazard: "recorded before the
  widgets exist: appending a row can select it, and the handler reads this list by index." any
  future change that reorders or filters rows without updating that vec in the same breath is
  a wrong definition on screen, silently. #45 (let the reader order the dictionaries) is
  exactly such a change.
  **`gtk::ListView` + `gio::ListStore` + `SignalListItemFactory`** is the gtk4 answer and
  removes the class: you hand gtk a model, and the selection hands you back the *item*, so
  there is no index to keep honest and no shadow copy to desync. it also recycles row widgets
  where `ListBox` instantiates every one, which is the difference between 500 boxes and a
  screenful at the row cap. `ListBox` is the gtk3-era api; `ListView` is what gtk4 added for
  this.
  **cost, so it is not a surprise.** the item needs to be a `glib::Object` with properties
  (word, dictionary count, tag) — 60–80 lines of subclass boilerplate — so the change is
  roughly line-neutral. and **the e2e harness leans on today's widget shape**: `row_words`
  and `row_tags` read `list item` nodes wrapping labels, `select_first_row` goes through
  `Atspi.Selection.select_child`, and the keyboard checks assume `Down` lands on
  `row_at_index(0)`. a `ListView` exposes a different tree, so expect to re-cut those
  accessors and to re-check the six or seven checks that read rows. worth doing *before* #45
  and #52, both of which touch the same rows: a per-row property is the natural home for #52's
  short form and #45's rank, and doing them first means writing that twice.
  **the other four candidates, and why not.** *composite templates* (`.ui` xml or blueprint)
  are the canonical gnome answer and would move ~200 of `build`'s 362 lines out of rust — but
  they need the window to become a `GObject` subclass, which replaces the deliberate
  `Rc`/`Weak` design of #8 and the test guarding it, and they trade a compile error for a
  startup failure when a widget is renamed. shorter in rust, not clearer overall. *property
  bindings* (`bind_property`) would replace the four `set_sensitive`/`set_visible` pokes, but
  binding needs a `GObject` to bind from and the source of truth is `Collection`, which #46
  just finished keeping gtk-free — not worth dragging gtk across that boundary for four call
  sites. *the definition pane* has no declarative form at all: `render.rs` applies `TextTag`s
  to buffer ranges and gtk offers nothing else for that. *relm4* would genuinely be
  declarative and probably shorter, but it is a rewrite that hands the architecture to a
  dependency — a different question from adopting `serde_json`.
  note the irony for whoever picks this up: both of the seriously declarative options require
  *adding* `GObject` boilerplate. declarative here does not mean shorter.
  **done.** `gtk::ListView` over a `gio::ListStore` of `BoxedAnyObject`, a
  `SignalListItemFactory` that builds a row once and fills it per item, and the selection
  handing back the item. `row_at_index`, `selected_row` and the shadow `Vec` are all gone —
  nothing in the module reaches a row by position any more.
  **the prediction about the harness was wrong, and pleasantly so: it needed no changes.** the
  a11y tree came out the same shape a `ListBox` gave — `list item` named by the word, its
  labels readable inside — and at-spi's `Selection` interface works on a `ListView` too, so
  `row_words`, `row_tags` and `select_child` all still mean what they meant. 35 checks passed
  untouched; there are 36 now.
  **it does crash the process if you name the row the obvious way.** the old code set the
  row's accessible label with `update_property` on the `ListBoxRow`; the equivalent here is
  the `GtkListItemWidget` *behind* the `ListItem`, reachable as the child's parent — and
  setting an accessible property on gtk's own internal widget kills the app with
  `gtk_widget_insert_after: assertion 'GTK_IS_WIDGET (widget)' failed` the moment it lays out
  another row. it survived launch and died on the first search, which is what made it look
  like a factory bug; bisecting the factory down to a single bare label found it.
  `ListItem::set_accessible_label` is the api for the job. it wants gtk **4.12**, and that
  bump costs nothing: `adw 1.4`, already required, requires 4.12 itself — the floor was
  always there, only unnamed.
  a dead end worth recording: marking the tag and count labels `AccessibleRole::Presentation`
  also gives the row a clean name, and it *worked* — but it drops them out of the a11y tree
  entirely, which silences the language tag and the `·2` for a screen reader. that count is
  the whole point of #12; it is information, not decoration. rejected.
  **`set_autoselect(false)` is load-bearing.** a `SingleSelection` otherwise selects the first
  item every time the model changes — so a definition would appear after every keystroke, and
  choose a word for you. one line, and nothing in the suite would have noticed it going, so
  there is now a check that a *typed* search picks no row and leaves the hint in the pane. it
  pairs with #36's check that a *forwarded* search does select. removing the line fails it
  with `selected=1` and a definition in the pane.
  and the invariant a recycling factory needs, which the old code never had to think about:
  **every branch sets every field.** hiding the count without clearing it leaves the previous
  row's `·2` on a recycled widget — still in the a11y tree, under a word only one dictionary
  has. the fixture is too small to have reused a row and caught that; it came from reading the
  code rather than from a failure.
  cost, since the entry guessed: **not line-neutral** — `ui/mod.rs` 1,019 → 1,112. the setup
  and bind halves each spell out the whole row, and clearing every field in every branch is
  more code than appending only the labels a row wanted. worth it for a wordlist that cannot
  desync from what it is showing.
- **[x] #46 architectural review: draw the module boundaries properly.** the first two
  steps are done, by building the cli you suggested — and it worked as an argument-settler:
  every question the window asks about words now goes through one place, because the command
  line had to ask the same ones.
  **`collection`** is that place: the loaded collection plus what the reader has decided about
  it — scope, folding — and every query over it (`search`, `resolve`, `definitions`,
  `scope_size`, and the status line both front ends print). it has never heard of gtk, so it
  is testable without a display, and the window's own state shrank from three fields
  (library, scope mask, fold flag) to one.
  **`cli`** is the second front end. collection-shaped commands — `search`, `define`,
  `scope`, `index` — ask the collection, so their answers are the app's answers rather than a
  parallel implementation; `dump` and `lookup` stay file-shaped, for looking at a dictionary
  the app has not been told about. `--json` on the four collection commands, escaped
  properly and checked against a real parser. `main.rs` went 1,511 → ~1,200 lines and no
  longer contains a subcommand.
  the boundary already caught something: the smoke check in `hack/check.sh` was asserting on
  a header only the cli printed, and now asserts on the status line *both* produce — so the
  two cannot drift into describing one search differently. a review then found the one hole
  left in that promise (an empty query: the window answered with the library's size, the cli
  with "0 results") and five other things the new surface got wrong — a `--limit` that
  accepted `banana` and silently used 500, flags refused before the word, `truncated` true
  whenever the count merely *reached* the limit, a `define --json` miss that printed prose
  into a json pipe, and a mistyped subcommand that raised the window and exited 0. all fixed;
  the shape of each is now a test.
  **parsing and json are crates now, not mine** (your call, and the review made the
  case for you): four of that review's eleven findings were bugs in twenty hand-written
  lines of argument handling — a `--limit` that took `banana`, options refused before the
  word, a mistyped command that raised the window and exited 0. `clap` has none of them, and
  answers `dictu serach rex` with *"tip: a similar subcommand exists: 'search'"*. `serde_json`
  replaced a hand-rolled escaper that was correct only because nothing in the collection had
  yet contained the characters it got wrong. cost, measured: **+2 crates for serde_json
  (serde was already here), +12 for clap, 128 → 142 in a tree a gtk app already dominates**,
  and the binary 3.04 → 3.70 MB. what stays hand-written is what no crate knows: the dictionary
  formats, the cache image, the key normalization.
  **the e2e migration is done, and it did not do what the entry predicted.** the checks that
  were only ever about answers — is a headword findable, does an unaccented query reach an
  accented entry — moved to `hack/cli.py`, which asks the same `Collection` through the
  command line: **20 checks in 0.8 s**, against ~0.3 s *each* over at-spi. but removing eight
  of them from the e2e suite saved **one second of twenty-nine**, because that suite's cost
  is fixed overhead and polling, not the checks. the honest speed-up came from tuning what
  was actually slow — the 0.15 s poll interval and the fixed sleeps after every synthetic
  key and click — which took the suite **29.6 s → 17.0 s**, stable over three runs.
  so the migration's value is not speed. it is that `hack/cli.py` needs **no display, no
  d-bus and no machine-wide lock**, so it runs while the app is open (the e2e stage used to kill
  stray instances, which twice killed a window mid-use — #62 ended that), and that data assertions are
  now cheap enough to be generous with: it covers exit codes, json shape, `define`, `dump`,
  limit semantics and folding, none of which the at-spi suite ever checked.
  what stays at-spi is what needs widgets: focus, keyboard routing, the scope popover, link
  geometry, the fold strip, and that what the collection answers reaches the screen.
  **and the window came out last: `main.rs` is 1,255 → 82 lines.** what is left in it is the
  one thing only it can do — start the application, and hand a forwarded word to the window.
  `ui` holds the widgets, the state and every signal handler; `render` beside it holds the
  typography, the `TextTag`s a definition is dressed in and the paragraph structure laid over
  its body.
  the seam is narrow, which is the point: **all `main` knows of the window is four names** —
  the `Ui` handle, `ui::build`, `present`, and `search_from_outside`. that last one is new,
  and is the part worth having done. `main` used to reach into six of `UiInner`'s private
  fields to fill the search box for the hotkey — blocking a signal handler, arming a guard,
  populating, selecting — which is delicate gtk work that had no business being in the
  process's entry point. it is one call now, and the reasoning lives next to the fields it
  reasons about. every field stayed private; `render` kept 6 of its 13 names to itself.
  **what I did not do, deliberately.** the entry called this three jobs and so three modules:
  construction, handlers, typography. typography separated cleanly, but construction and
  handlers did not, and pushing them apart would mean marking all fourteen of `UiInner`'s
  fields `pub(super)` so a sibling module could fill them in — trading the encapsulation this
  item is *for* against a smaller file. `ui/mod.rs` is 1,019 lines and honest; a carrier
  struct threading a dozen widgets between two modules to avoid that would be ceremony. if it
  is split later, the boundary to want is a `build` that returns the widget tree and a `ui`
  that owns it and reacts — not one that knows the struct's insides.

## deferred — needs you

these are blocked on a decision or an action only you can take. nothing else waits on them.

- **#37 Wiktionary, phase A** — **languages chosen: french, spanish, latin, ancient greek,
  german** (all into english). the cheap path is dropping a prebuilt StarDict build of each
  pair into the collection; the app needs no change to read them, so what remains is finding
  builds worth having and downloading them. note two of those overlap with dictionaries you
  now own — Gaffiot and Lewis & Short for latin, LSJ and Bailly for greek — so wiktionary
  earns its place there only for coverage those miss (late, medieval and technical words),
  while for french, spanish and german it is the whole offering. scale check from the plan:
  french-from-english alone is 388,993 forms, so five pairs is roughly +1.2M headwords on
  top of today's 1.94M — worth measuring against the fuzzy budget in #42 before adding all
  five at once.
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
  **phases A (normalized keys, #39), B (attribution, #12) and D (#33) have landed** — D for
  none of the reasons planned here: #44 replaced the dictionary whose inflections needed
  detecting, and what shipped folds rows that repeat a definition rather than identifying
  lemmas at all. **only C, the fuzzy scan, remains.**



- **[x] #39 greek and hebrew are typable.** the index now sorts and searches on a bare key —
  NFC, then full case folding, then combining marks dropped — so a tonos query reaches Dodson's
  oxia headwords, a medial sigma reaches a final one, and an unpointed hebrew query reaches the
  74,174 pointed headwords that have no unpointed spelling anywhere in the collection.
  diacritics still mean something: a query that spells them out only matches headwords whose
  marks are a superset of its own, so `מֶלֶךְ` no longer answers with `מָלָךְ` while `מֶלך`,
  pointed half way, still reaches `מֶלֶךְ`. the same holds for greek accents and for latin
  macrons. phase A of `docs/search-index-plan.md`; the cache format version is bumped, since a
  warm start would otherwise binary-search an order sorted by the old key.

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
  **it has a second edge now.** #33's folding identifies a definition by its byte range and
  never reads it, while `lookup` drops what renders empty — so once this is fixed (or a
  dictionary with audio-only cards arrives), a row could be folded away as a repeat of an
  entry that renders as nothing. whichever way #41 goes, `entry_ids` has to agree with
  `lookup` about which entries exist.


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

## done

- **[x] #33 each definition once, not once per form pointing at it.** searching `rex` walked
  into 26 perfect-tense forms of *rego* — every one of them answering with the same
  definition — and `esse` filled 473 rows with verbs that merely take the auxiliary. the
  scope panel now carries **Fold repeated forms**, and every count the ui shows says when it
  is on.
  **the obvious rule was wrong, and two reviews caught it.** StarDict appends `.syn` records
  after the `.idx` ones, so aliases are cheap to identify — one `u32` in the cache header
  (VERSION 5), no index-time pass, which is what made this look free after #44. the first
  attempt therefore hid aliases as such. but a `.syn` is only *sometimes* an inflection
  table: Whitaker's is, while **a hebrew-hebrew dictionary files the modern plene spelling of tens of
  thousands of its own lemmas that way** — 105,660 of its alias keys share a bare key with
  no headword it files itself. so hiding aliases made everyday words unfindable at the only
  spelling a modern reader types: `עגבנייה` (tomato), `תחבושת`, `חוסר`, `מישפט`, `סיפור`,
  `מילחמה` all returned **nothing**. 280 of 300 sampled plene spellings did.
  the rule that shipped instead never hides the only way to a definition: **a row goes only
  when every entry it points at is already shown by a row above it**, and only when every
  one of its spellings is a pointer, so a word a dictionary files itself is never folded.
  25 of the 26 forms of *rego* repeat one definition and go; a form searched on its own is
  the only row reaching that definition and stays.
  measured at the wordlist's own limit of 500 rows, before → after: `rex` **27 → 3**,
  `esse` **473 → 32**, `amo` **500+ → 91**, `sam` **215 → 78**, `regis` **19 → 7**,
  `כאב` **18 → 13**, and the words above all still return their rows. `מלך` is **17 → 17**,
  though not for the reason first written here: it survives because two other dictionaries
  file it themselves, not because the rule spares aliases. the case that wording was meant
  to cover — an ambiguous unpointed spelling *only* a hebrew-hebrew dictionary holds — is the one a third
  review found broken (below).
  greek is untouched: LSJ, Bailly, Dodson, Pindar and the Middle Liddell ship no `.syn`
  between them.
  **the property, checked rather than argued** — and then a third review found the half of
  it the check could not see. "every definition stays reachable" was true and insufficient:
  folding was deleting rows whose *spelling* was the only one a reader would type. `טוניקה`
  vanished, leaving `טוּנִיקָה` (a tunic) and `טוֹנִיקָה` (a tonic) and no way to say which
  you meant — 2,508 a hebrew-hebrew dictionary aliases have that shape. the cause was a contradiction
  between the two halves of the search: `group` keeps an ambiguous unpointed spelling as its
  own row *because* it cannot be attributed to one lemma, and then folding deleted it, which
  files it under a guess by omission. a class `group` declined to absorb is now one folding
  may not drop either.
  the sweep now asserts both halves — every definition reachable, and every spelling the
  reader typed still a row — over 58 query/limit runs on the real collection: 2,666
  definitions, 1,884 repeat rows folded, none lost, 52 typed spellings kept. the
  mock-fixture versions of both live in `library.rs`.
  a definition's identity is `(dictionary, offset ^ size << 40)` — both halves of the range,
  since two entries can start in the same place and run to different lengths.
  the setting is session-only, like the scope beside it (#45 is where preferences persist).
  the cli's `--fold-forms` drives the same code, and its row limit is now the wordlist's:
  the old 20 silently truncated every measurement taken through it, which is how a wrong
  set of numbers reached a commit message.

- **[x] #44 the collection, chosen.** researched, measured against what was already on
  disk, and applied. **greek needed nothing bought**: the Liddell-Scott.dsl already here is
  130,454 headwords against the best free LSJ build's 127,868, and the Middle Liddell here
  is the only one that exists in dictionary form anywhere — Jacob Rosen's 2015 Perseus
  build, unimproved in eleven years. what the collection did have was **two of everything**:
  `Grc-Eng_L&S_..._or_2` is the same LSJ with the accents stripped from its headwords
  (λόγος is 43,307 bytes there against 43,288 in the accented build — the same book), so it
  answered every greek word a second time. excluded.
  **latin was the whole problem.** `latin infl+lewis` filed 1,223,585 headwords for ~37,000
  words, a copy of the definition pasted onto every inflected form. replaced by two files
  that carry the same information without the duplication: **Lewis & Short 1879**
  (latin-dict release v1.3, StarDict, 49,983 lemmas, CC BY-SA — `rex` is one entry of 5 KB
  of real L&S prose with live citations) and **one of the two excluded Whitaker copies**,
  re-included for its `.syn`: 1.18M inflected forms mapped onto 37,777 lemmas, so a form met
  in a text still finds its word (`rexit` → 1 row, `dacrimarum` → 1 row).
  added for the french side, both modern re-editions and the only two post-1950 lexica that
  exist as files at all: **Bailly 2020** (Gréco/Charbonnet, 110,646 headwords, built
  2025-11-03, the best-edited free greek lexicon there is) and **Gaffiot 2016** (72,165
  entries, revised vowel quantities and corrected references). Gaffiot is only obtainable
  from the internet archive — canadienfrancais.org rebuilt its site and dropped the whole
  uploads tree, while Gréco's own page still points at the dead link — so the file is the
  publisher's original bytes served by
  `web.archive.org/web/20180831042849id_/…/Gaffiot2016Stardict-v1.3-Desktop.zip`
  (6,398,397 B, CC BY-NC-ND, fine for private use). no newer StarDict build exists; Gréco's
  current formats are Epwing, Dictan and PDF, none of which we read.
  measured, whole collection: **1,825,792 headwords → 1,941,344** across 15 dictionaries,
  warm start **0.44 s → 0.39 s**, peak RSS **335 MB → 326 MB**.
  **`rex` still returns 27 rows** — say it plainly, since fixing it was the point. the 26
  extra rows are `rexeram`, `rexerat`, `rexerint`…, real perfect-tense forms of *rego* that
  genuinely begin with those three letters. no dictionary swap can hide them; only a lemma
  filter can, which is #33 — and see there for why this swap is what makes #33 cheap.
  **on modern lexica**, since it was asked: the intersection of "post-1950 scholarly
  lexicon" and "file you can own" is Bailly 2020 and Gaffiot 2016, and nothing else. OLD,
  TLL-as-data, BDAG, the Cambridge Greek Lexicon, Montanari, DGE, Niermeyer and de Vaan are
  subscription or DRM'd app modules; Cambridge is not on Logeion at all and Logeion's
  Montanari is letter λ only; the 1996 LSJ Supplement exists in one ~$101 Logos module; the
  TLL's open access is real but PDF-with-OCR, not a dictionary file.

- **[x] #38 saving `config.toml` keeps its comments.** `Config::save` rewrote the file
  through serde, which would have deleted the notes saying *why* the #19 Latin duplicates
  and the #34 French dictionary are excluded — the first time anything persisted a change.
  it edits the document with `toml_edit` now (the version `toml` already depends on, so no
  second parser), through a pure text transform (`Config::edited`) a test can drive with a
  commented fixture. **saving only ever appends**: entries the config has and the file
  doesn't are added, and nothing else is touched.
  it took three review rounds to arrive at those four words, and the rounds are the point.
  removing an entry sounds symmetrical and isn't: toml_edit stores decor *before* a value,
  so a note written after entry X is filed under X+1 and a note after the last entry lives
  in the array's trailing text — "delete this entry and its comment" needs a convention
  about who owns which comment, and every attempt to encode that convention got it wrong in
  a new way (an append walked the last note onto the newest entry; a removal left a dead
  entry's reason sitting on one that stayed, which states something false rather than
  merely losing something true). the machinery to do it correctly was ~150 lines of
  ownership heuristics guarding an operation nothing calls. it was cut. **permanent
  removal stays a hand edit**, as it already was.
  known and accepted: a note written *after* the last entry on the same line ends up beside
  the entry appended after it — comments belong above their entry. the real config writes
  them that way, and is byte-identical after a save that changes nothing.
  `Config::exclude` (append) is the only writer and is still unwired; the scope panel (#14)
  stays session-only.

- **[x] #12 say which dictionaries answer.** a wordlist row used to wear the tag of the
  first dictionary that had the word and silently drop the rest; it now counts them —
  `GRC ·4` — and names them in its tooltip. the trap was in the search, not the ui:
  `prefix_search`'s limit counted index *entries*, so a dense prefix could cut a word in
  half and leave a row claiming two dictionaries when three define it. the limit now counts
  rows and stops only at a key boundary, which costs nothing because entries sharing a key
  are contiguous. a row counts only the dictionaries that have the spelling it *shows*,
  because selecting it looks that spelling up — a count the pane then contradicts would be
  worse than no count. verified against the real collection: four truncating prefixes,
  601 rows, every row's list identical to `lookup_all`'s (see #43 for what is left).

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
  the mask scopes the definition pane too, not just the wordlist: a dictionary that is out
  of scope does not get to answer, and does not get named in the fold strip either. that
  was not the original plan — see the review note below, which is where it came from.

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
- **[x] #53 a mechanical check for speed.** every performance number in this file was
  measured by hand into a commit message, where nothing ever checked it again — and the
  first thing this check did was find one that had rotted: #44 recorded a **0.39 s** warm
  start on this exact collection, and it is **~1.0 s** now (`Library::open` alone; loading
  the config and scanning the directories are 0.7 ms together). see #54.
  `hack/speed.py` runs `dictu bench --json` and fails a stage when anything is more than
  1.6× the recorded baseline — 2× for opening, the one measurement that touches a disk.
  it measures the real collection, not `sample/`: thirteen words say nothing about opening
  1.9M headwords, so `hack/speed-baseline.json` is one machine's and the check skips itself
  where it does not apply rather than blaming a different bookshelf.
  two mistakes worth keeping, both mine, both caught by checking the check. **one run
  measures nothing:** warm open swung 925–3375 ms over an unchanged binary on page cache
  alone, so this takes the best of three runs of a bench that itself takes the best of five,
  which brought the spread to 0.5%. and **the noise floor was hiding the signal:** the
  first version ignored anything under 2 ms as scheduler noise, which sounds prudent until
  you notice every prefix search here is 8–400 µs — a planted mutation that added 160 µs to
  every query passed it silently. at a 100 ns floor the same mutation fails 7 checks
  (`consuetudino` 0.4 → 158.7 µs), which is the only reason to believe the rest.
  measured, for the record: prefix search is **0.4–252 µs** (not the 2.4 ms in #42, which
  is the *fuzzy* scan), peak RSS **360 MB**, and the whole `hack/check.sh` loop 22 s (29 when the speed stage rebuilds release).
- **[x] #55 the ui suite in 16s instead of 24s.** giving the harness its own d-bus session
  stopped it crashing the desktop and cost ~6s; this gets the 6s back and a little more,
  with all 34 checks passing across four consecutive runs. profiling first, which said
  something surprising: **19 clicks were 8.5 of the 24 seconds**, while the 22 `dictu
  --search` subprocesses everyone would suspect were 1.8s all told.
  so the clicks went. a scope check box exposes no at-spi Action (measured: zero, unlike
  the button that opens the panel), which is why it was being clicked — but `Tab` works
  where `grab_focus()` does not, because gtk routes a real key itself. tab until the box
  has focus, press space, wait for `CHECKED`: every step observable, **0.111s per flip
  against 0.673s**, 0 misses in 20 either way.
  two things I was wrong about on the way, both worth writing down. the pointer sleeps
  cannot be trimmed — a click needs ~0.3s of quiet and it does not matter which side of it
  goes where, so `0.1/0.0` loses 3 flips in 20 and `0.05/0.0` loses 11, each miss costing a
  3s timeout: cutting them makes the suite **slower**. and gtk4 really does expose no
  character geometry, so the link click cannot be aimed — `get_character_extents` fails,
  `get_offset_at_point` never replies, and a label's links are neither `Hypertext` nor
  objects. what it *can* do is step down the column faster: the band that follows the link
  measures ~24px, so 16px steps find it in six clicks where 8px took eleven, 5.1s → 2.9s.
  the rest: animations off via `gtk-enable-animations=false` in the throwaway config (~1s),
  and the last arbitrary sleep in the suite replaced by the wait it stood in for — an
  absence cannot be waited on, but the definition rendering beside it can.
  **then a second round, 13.5 s → 8.9 s**, after profiling said the waits were 73 calls at
  77 ms and the cost was in the *questions*, not the sleeping. every at-spi question is
  d-bus round trips: walking the window is 14–25 ms, finding the scope boxes 38 ms, finding
  the status line among every label 24 ms — while asking a node you already hold is
  0.1–0.3 ms. so the status line remembers its label (gtk keeps the same accessible across a
  text change, verified over four searches, and a stale one fails the pattern and re-walks),
  `toggle_scope` reads the boxes *and* the focus chain off one walk, and `POLL` dropped
  0.04 → 0.005 once the predicates were cheap enough to poll that fast. that was 13.5 → 11 s.
  the link click gave the rest: the candidates are now tried nearest the measured band
  first, which is the same search over the same column — a shifted layout costs a second
  click, not a failure — but the usual run pays one click instead of six. 11 → 8.9 s.
  and a caution learned twice over: **these numbers move with the machine.** the same
  unchanged suite measured 8.7 s and 12.9 s a minute apart, and 13.5 s against 16.9 s an hour
  apart, so every figure above comes from interleaved A/B runs rather than before-and-after
  ones — and the "16 s" this item started from was partly load. all 34 checks passed on
  every one of the ten runs behind these figures. the whole `hack/check.sh` loop is 15–24 s.
  what is left is mostly not the harness's: **2.3 s is the scope popover** opening and
  closing four times (~285 ms a transition, animations already off), and **2.3 s was gtk's
  own `SearchEntry` debounce** — `--search` sets the text and `search-changed` waits out the
  150 ms `search-delay`. the second turned out to be an *app* fault rather than a test one
  and is now fixed in #56: a search arriving from the global hotkey has nothing to coalesce,
  so that delay was lag the reader felt too.

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
