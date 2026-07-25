# plan: fuzzy search, normalized keys, and lemmas-only — without new machinery

status: **proposed**, not started. roadmap item: **#42**. covers #33, #39, #12 and fuzzy
matching.

## the short version

these four wants look like they need a search engine. they don't. three of them are a
column and a filter on the index we already have, and the fourth — fuzzy — is a fallback
that runs when a search finds nothing, not a per-keystroke hot path. so the plan is to add
**no new index structure and no database**, and to revisit that only if measurement says
otherwise.

an earlier draft of this document proposed an `fst` matcher with a side table, optionally
SQLite. that is the right architecture for a search engine and the wrong amount of machinery
for this app — see "what was rejected and why".

## what we have

`Library` holds `sorted: Vec<(u32, u32)>` — one `(dict, headword)` pair per headword,
1,825,792 of them, ordered by the lowercased headword. prefix search is a `partition_point`
binary search plus a walk of the matching run, already filtered by the per-dictionary scope
mask (#14). definitions are read lazily from mmapped files. warm startup is 0.55 s at 164 MB
(roadmap #7).

that structure answers exactly one question: "which headwords start with this prefix". the
plan keeps it and gives it two more columns and one more entry point.

## the four changes

### 1. normalized keys (#39)

Dodson stores 4,686 headwords with U+1F79 (oxia); every greek keyboard emits U+03CC (tonos)
and finds nothing. HALOT adds 3,259 with non-canonical hebrew mark order.

sort and search on an **NFC-normalized, lowercased** form of each headword, keeping the
original for display. one small pure-rust dependency (`unicode-normalization`); no C, no new
file format. the normalized key can live in the existing `.didx` cache image as another
array, so this costs nothing at startup.

this is worth doing on its own and blocks nothing else.

### 2. lemma flag (#33)

serving "lemmas only" is a `Vec<bool>` parallel to `sorted`, filtered in the same walk that
already skips out-of-scope dictionaries — a few lines beside the existing
`active.get(d).copied().unwrap_or(true)`.

**the flag's value is the actual work**, and no storage choice makes it easier. two signals
survive (the `.syn` one died with the latin de-duplication in #19):

- headwords whose `.idx` entries point at the **same byte range** are one lemma and its
  inflections; take the shortest of each group (`amo` over `amare`/`amavi`, `dacrima` over
  `dacrimae`). costs one pass over ~1.2M entries at index time, cached thereafter.
- an inflection's entry **opens with its lemma** (`dacrimarum` → an entry beginning
  "dacrima, dacrimae"). exact, but needs a definition prefix read per headword.

start with the first, measure the pass, and check it against the second on a sample.

### 3. which dictionaries answer (#12)

`populate_results` already walks every hit and dedups by lowercased word; it currently keeps
the first dictionary and discards the rest. collect them instead and let the row say so. no
structural change at all.

### 4. fuzzy, as a fallback (the new feature)

**when** it runs matters more than how: only when the prefix search returns nothing, offered
as "no matches — did you mean…". that turns a latency problem into a non-problem.

a bounded scan is enough:

- filter candidates by length (`|len(a) − len(b)| ≤ k`) — throws away most of the corpus
  immediately;
- banded Levenshtein with early exit once the row minimum exceeds `k`;
- `k` by query length: 1 for ≤4 characters, 2 above;
- operate on **characters, not bytes** — with hebrew and greek at 2 bytes per character, a
  byte-wise distance would be meaningless here. this is the detail most likely to be got
  wrong, and it is also why the off-the-shelf options need checking rather than assuming.
- cap results like `SEARCH_LIMIT` does.

if the scan turns out too slow, the cheap escalations in order are: restrict fuzzy to lemmas
(34,443 keys instead of 1,223,585 for latin), bucket by first normalized character, then
precompute a trigram posting list. only after all three fail is an `fst` justified.

## phasing

each phase is separately useful and separately abandonable.

- **A — normalization (#39).** greek becomes typable. gate: a tonos query finds an oxia
  headword; all 29 e2e checks unchanged.
- **B — dictionary attribution (#12).** rows say when several dictionaries answer. gate: an
  e2e check on a fixture headword both fixtures define.
- **C — fuzzy fallback.** gate: measured latency for 3-, 6- and 12-character queries in
  latin, greek and hebrew, **in a release build**, plus a check that a one-character typo
  finds the word.
- **D — lemma detection and the config option (#33).** gate: `rex` shows 1 row instead of
  27 with the setting on, and the index-time pass costs less than a second on the real
  collection.

## what was rejected and why

- **`fst`** — gives sub-millisecond fuzzy at distance 2 over 1.9M keys and a 474 KB key map.
  both are real, and neither is needed if fuzzy is a fallback rather than as-you-type. it
  also cannot hold the attributes, so it comes with a side table anyway: two structures where
  we currently have one. revisit if fuzzy becomes as-you-type.
- **SQLite** — the natural home for a *side table*, and a poor matcher (no Levenshtein, and
  `LIKE 'x%'` is no better than a binary search). worth promoting to only if #12 and #33's
  bookkeeping start feeling like queries, or a Wiktionary import (#37) needs real ETL.
- **redb, LMDB** — mutable stores; this data is static once built.
- **rkyv** — would retire the hand-rolled cache format, which is a real risk area, but adds
  no query power. a separate argument, not this one.
- **tantivy** — the right answer if we ever want full-text search *inside definitions*. a
  different feature.

## the one thing to measure first

before writing phase C, time a bounded scan over the real 1,825,792 keys in a **release**
build: 3-, 6- and 12-character queries, latin/greek/hebrew, with the length filter and early
exit in place. if that lands under ~100 ms it is a fallback nobody notices, and the whole
`fst`/SQLite question stays closed. if it lands over ~500 ms, escalate in the order above.
