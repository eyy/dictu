# dictu — roadmap

the task list of record. read it at the start of a session, edit it as work lands: move
statuses, append notes, and add new feedback as a new numbered item. numbers are stable
and never reused, so `#20` means the same thing across sessions. refer to items by number.

statuses: `[~]` in progress · `[ ]` not started. **a finished item leaves this file**: it
gets a note of at most three lines — what changed, what it cost, and the one thing that was
surprising — and moves to `done.md`. the fuller version lives in the commit that closed it,
which is where it belongs, next to the diff that proves it (`git log --grep '#45'`).

that split happened when this file reached 1,577 lines, 1,344 of them finished work. the
archive is worth keeping and was worth reading — an open item argues with closed ones by
number, and `done.md` is where those numbers are — but a task list that is 85% archive stops
being a task list.

items #1–#16 carry over from the previous session's list; #17–#23 come from the feedback
round of 2026-07-24; #24 onwards were found while getting the tree green. #1, #2, #4, #5
predate the surviving record and are reconstructed from the code — the original wording is
lost, the substance is not.

---

## in progress

nothing open right now.

## next — 2026-07-24 feedback

**this section is in the order it makes sense to do it in, not the order it arrived.**
the tools that lie to you, then the features. the reasoning per item is in the item; what
the order encodes now is: #63 makes the speed check trustworthy, and it stopped the gate twice in one afternoon by
measuring a busy machine; #68 is the largest thing here and the first that would put dictu
on the network, so it is worth doing awake rather than at the end of a session.

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

- **[ ] #74 smarter search: typo tolerance, and a query in latin letters that finds a
  greek or hebrew word.** *(asked for. **not to be started without a written plan and
  explicit approval** — recorded here as a request, not as a decision.)*
  asked for as "levinstein distance, typos fixes, script convertor (helios should find
  ηλιος)". three things, and they are not equally new.
  **the first two are #42's phase C**, which is planned and measured already: a bounded
  edit-distance scan over the merged keys, 2.4 ms at three characters and 57–67 ms worst
  realistic case, fast enough that it need not even be a fallback. that work does not need
  designing again; it needs doing, and #42 is where it is written down.
  **the third is genuinely new, and is not a distance problem at all.** `helios` and `ηλιος`
  are not two spellings a distance apart — they share no character. and it cannot happen by
  accident today: `keys::fold` and `keys::bare` both return early for ascii, so an ascii
  query and a greek headword have no key in common by construction. this is a
  *transliteration*, and transliteration is a table, a direction, and a pile of choices:
  1. **which romanisation.** greek alone has several in use — `helios` (h for the rough
     breathing, e for eta), `hlios` (beta code, h *is* eta), ISO 843 — and they disagree on
     the letters a reader is most likely to type. hebrew is worse, because the vowels are
     not written: `adam` must reach `אָדָם`, `shalom` must reach `שָׁלוֹם`, and `sh` is one
     letter. a table that serves a classicist and one that serves someone typing what they
     heard are not the same table.
  2. **which direction.** romanise the *query* into the script (no reindexing, but one query
     becomes many candidates, since `e` could be eta or epsilon) — or store a romanised key
     per headword beside the folded one (search stays a single lookup, at the cost of a
     bigger key table and a cache rebuild). the second fits the merged-index design; the
     first is cheaper to try.
  3. **how it composes with fuzzy.** if a romanised query is *also* typo-tolerant then two
     error models multiply, and `helois` has to survive both. that may be the right answer
     and it is certainly the expensive one.
  4. **how a reader knows why a row is there.** `ηλιος` appearing for `helios` is delightful
     when it is what you meant and baffling when it is not. exact, normalized, romanised and
     fuzzy are four different reasons a row matched, and the wordlist currently says nothing
     about which — #12's attribution and #59's language tag are the precedents for saying it.
  5. **what it may cost.** #42's numbers are the bar to beat, and they were measured, not
     guessed. a romanised key per headword is another 1.9M keys.
  so: a plan first, in `docs/`, in the shape #42's was — with the premises checked against
  the real files rather than assumed, since that review overturned three of them last time.

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
  **#74 asks for the rest of this and one thing beyond it** — a query in latin letters that
  finds a greek word, which is transliteration rather than distance and needs its own plan.
  **phases A (normalized keys, #39), B (attribution, #12) and D (#33) have landed** — D for
  none of the reasons planned here: #44 replaced the dictionary whose inflections needed
  detecting, and what shipped folds rows that repeat a definition rather than identifying
  lemmas at all. **only C, the fuzzy scan, remains.**

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

## reference — the collection as dictu sees it

there used to be a table here, and it had gone wrong in every column: 14 dictionaries when
there are 15, 1,941,292 headwords when there are 1,936,120, HALOT at 19,579 when #72 took it
to 13,932, and every name written the way the folder spells it rather than the way #52 does.

it was also written twice over. what is loaded is what `dictu scope` prints — with sizes,
and with the path each one is configured under. what is *not* loaded, and why, is in the
comments beside each `!` exclusion in `config.toml`, which is where the reasoning belongs
because it is where someone would go to undo it.

    dictu scope          # the collection, in reading order
    dictu index          # what a launch loads, and what it costs
