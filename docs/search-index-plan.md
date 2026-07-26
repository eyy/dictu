# plan: fuzzy search, typable Greek and Hebrew, and lemmas-only

status: **proposed**, not started. roadmap item **#42**. covers #33, #39, #12 and fuzzy.

revision 3. r1 proposed an `fst` matcher with a side table (too much machinery). r2 dropped
it but kept r1's unexamined evidence. r3 is written against measurements taken from the real
files; where r2 was wrong it says so, because the wrong premises are more instructive than
the right conclusion.

## the architecture decision (settled)

| layer | choice |
|---|---|
| matcher — prefix and fuzzy | **sorted arrays + a bounded scan.** no new structure |
| definition bodies | mmapped source files, as today |
| lemma flags, normalized keys, attributes | columns in the #7 cache image |
| Wiktionary (#37) | **just another StarDict directory** — see below |

**`fst`: no.** it optimises matching, which is not the bottleneck, and it cannot filter —
it maps keys to ids, so "lemmas only, in these three dictionaries" still needs postings and
side tables on top. the two features interact against it: with lemmas-only on, the latin
candidate set falls from 1,223,585 to **37,273**, and a scan over 37k keys is microseconds.
it would buy index complexity to speed up the case that is already free.

**SQLite: no.** it was justified only by importing all of Wiktionary — 23 GB of JSONL needing
real ETL. the requirement is a handful of languages, and someone already publishes those as
StarDict, a format this app has read since day one. no database in the app.

**tantivy: no.** it is the right answer for full-text search *inside definitions*, and that
is explicitly not wanted. this is the one requirement that would have overturned everything
above, so it is recorded here rather than left implicit.

**scale check with Wiktionary.** French-from-English is 388,993 forms; three or four
languages is about +1.2M keys, so ~3M total, putting the worst-case scan near 120 ms
single-threaded and ~25 ms threaded — under the falsifier, and that is a 12-character query
at distance 2, not a common one.

## the short version

no new index structure, no database. three of these four are a column and a filter on the
sorted index we already have, and fuzzy is a bounded scan — **measured at 2.4 ms for a
3-character query, 24 ms at 6, and 57–67 ms worst realistic case** over 1.69M real
headwords in a release build, single-threaded. that is fast enough that fuzzy does not even
have to be a fallback.

what changed from r2: the primary lemma signal turned out not to exist in the data, NFC
alone leaves most of the Hebrew collection unreachable, and the cost model was wrong about
why the scan is fast. all three were checkable in minutes against files on this disk.

## where we are

`Library` holds `sorted: Vec<(u32, u32)>` — one `(dict, headword)` pair per headword,
1,825,792 of them, ordered by the lowercased headword; prefix search is `partition_point`
plus a walk, already filtered by the scope mask (#14). warm startup is 0.55 s at 164 MB
(#7). the structure answers one question — "which headwords start with this prefix" — and
this plan gives it two more keys, one more column, and one more entry point.

## 1. keys you can actually type (#39)

**the rule: matching is asymmetric in how specific the query is.**

- a query written **without** diacritics matches headwords **with or without** them;
- a query written **with** diacritics must **not** match headwords that contradict them.

so `מלך` finds both `מֶלֶךְ` and `מָלָךְ`, while `מֶלֶךְ` finds only the first. this is
script-agnostic and applies to every combining mark in the collection: greek accents,
breathings and iota subscript (`λογος` → `λόγος`, but `λόγος` ↛ `λὀγος`), hebrew niqqud and
dagesh, and **latin macrons** — Lewis & Short writes `rēx` and `virtūs`, so `rex` finds the
macronised entry while `rēx` deliberately does not find the unmarked Whitaker one. that last
consequence is worth knowing rather than discovering: typing macrons *narrows*, which is the
point, but it hides the un-macronised duplicates.

**one sorted order, not two.** sort and binary-search on the **bare key**:
NFC → full case fold → NFD → drop every combining mark (`Mn`) → recompose. that alone gives
the permissive direction, which is the one that matters most here: of 121,615 hebrew
headwords, 99,388 carry niqqud and **74,174 have no unpointed sibling anywhere in the
collection**, so without it they are unreachable by normal hebrew typing — and fuzzy cannot
rescue them either, since `מֶ֫לֶךְ` vs `מלך` is distance 4. greek mostly escapes by luck
(Liddell&Scott ships an unaccented copy and Dodson/HALOT a bare alias per card); hebrew has
no such luck.

the ordering inside the key is load-bearing and neither step subsumes the other: NFC maps
greek oxia U+1F79 to tonos U+03CC, and case folding maps final sigma `ς` to `σ` — which
`to_lowercase()` does *not*, and 139,079 headwords end in one. `casefold(U+1F79)` is still
U+1F79, so it must be **NFC first, then fold**. rust's `std` has no case folding, only
lowercasing, so this needs a crate.

**then filter the candidate run by mark compatibility.** derive the query's fold key (NFC →
fold, marks kept) alongside its bare key; if they are equal the query specifies nothing and
every candidate is accepted. otherwise accept a candidate only where the query's marks are a
**subset** of the headword's, aligned on base characters. subset rather than equality so
partial pointing behaves: `מֶלך` still reaches `מֶלֶךְ` while still excluding `מָלָךְ`.

the fold key therefore does not need materializing — it is only wanted for candidates in the
prefix run, which is bounded by `SEARCH_LIMIT`, so a few hundred derivations per keystroke.
only the bare key goes in the cache image, as a contiguous blob + `u32` offsets (and a `u8`
char-length array for phase C). that is well under the +30 MB an earlier draft budgeted for
two materialized keys and two orders.

77,990 headwords are non-NFC today (37,699 greek, 40,291 hebrew).

## 2. fuzzy

**measured, release build, 1,692,990 real headwords, best of three:**

| query | chars | k | time |
|---|---|---|---|
| `amo` | 3 | 1 | 2.4 ms |
| `dacrim` | 6 | 2 | 24 ms |
| `consuetudino` | 12 | 2 | 59 ms |
| `ἀνθρωπολογι` | 11 | 2 | 64 ms |

worst realistic case 57–67 ms single-threaded, 15–17 ms across 8 threads; ~72 ms scaled to
the full 1,825,792.

three conditions, all load-bearing:

- **materialized keys.** the numbers above assume the contiguous blob + precomputed char
  lengths from §1. deriving the key per candidate instead costs **73 ms for a 3-character
  query** — `to_lowercase()` over 1.69M keys is ~65 ms before any Levenshtein runs. same
  algorithm, 30× worse, purely from layout.
- **characters, not bytes.** Hebrew and Greek are 2 bytes per character; byte-wise distance
  is meaningless here. decoding into a reused `Vec<char>` is already inside the timings.
- **off the main thread, debounced, cancellable.** `search-changed` runs
  `populate_results` synchronously on the GTK main context, and once a prefix misses, every
  longer prefix misses too — so a naive implementation freezes the UI for 60 ms *per
  keystroke*, not once.

**when it runs:** on a settled (debounced) query when there is no **exact** headword —
*not* "when there are no results". r2 had it wrong: in this collection a junk prefix match
is the normal outcome (`esse` returns 100 entries of unrelated verbs), so "only when nothing
matched" suppresses the suggestion exactly where the index is noisiest. render it as a "did
you mean" group *below* the results, not instead of them. fuzzy still runs at most once per
settled query, so the budget above is unchanged.

## 3. lemmas only (#33)

**the byte-range signal does not exist.** grouping all 1,427,152 Latin `.idx` entries by
`(offset, size)` yields 1,427,152 groups — every inflection has its own physical copy of the
definition. MiddleLiddell, a hebrew-hebrew dictionary and Even Sapir are likewise 100% singletons. r2 said
"start with this one, measure the pass"; measuring it returns a no-op.

**use the entry's own text instead.** an inflection's definition opens with its lemma in
bold, so the lemma is the headword that matches the leading `<b>…</b>`. measured end to end
in release Rust:

```
read 27 MB idx + 177 MB dict    70 ms
parse 1,427,152 entries         30 ms
first-<b> pass                  63 ms   -> 48,673 distinct lemmas, 0 entries lacked <b>
macron-folded headword match   170 ms   -> 37,273 of 1,223,585 headwords are lemmas (3.0%)
TOTAL                          333 ms
```

it hits the gate directly: `rex` goes from 27 rows to 1. spot-checked `dacrimarum`→`dacrima`,
`amavi`→`amo`, `reges`→`rego`, `leni`→`lenio`. the 37,273 count cross-validates against the
34,443 lemmas in the Whitaker `.syn` we discarded in #19. note the matching needs the
**fold key** from §1, because Lewis & Short writes the bold lemma with macrons (`rēx`, `amō`)
while Whitaker writes it bare — exact matching finds 35,115, macron-folded finds 37,273.

**scope it honestly.** this is a Latin feature. Liddell-Scott, Klein, a hebrew-hebrew dictionary and
MiddleLiddell are one headword per body, so there is nothing to filter; a global "search
lemmas only" toggle is a no-op for 12 of the 13 dictionaries. where DSL *does* pair
headwords — HALOT 100% of cards, Dodson 97.9% — the pair is pointed + bare (`אָב` / `אב`), so
"shortest wins" would hide the real headword and keep the search alias. label the setting
for what it does or scope it per dictionary.

## 4. which dictionaries answer (#12)

`populate_results` already walks every hit and keeps only the first dictionary per word.
collect them all instead — genuinely local. **but** `prefix_search` stops after
`SEARCH_LIMIT` = 500 *entries*, not 500 distinct words, so on a dense prefix the dictionaries
near the cut are arbitrarily truncated and a row would claim "2 dictionaries" when three
define the word. that is new wrongness, since today's row makes no completeness claim. fix
by resolving attribution with a targeted `lookup_all` for the rows actually rendered, or by
counting the limit in distinct words.

## phasing

- **A — the two keys (#39).** greek and hebrew become typable.
  gate: a tonos query finds an oxia headword; an unpointed hebrew query finds a pointed one;
  **run it twice** — the second run must not rebuild the cache and must still pass. that
  second clause is the one that matters: changing the sort key without bumping the cache
  `VERSION` leaves the mapped merged order sorted by the old key and binary-searched with
  the new one, which is wrong results for anyone with a warm cache and green CI for everyone
  else.
- **B — attribution (#12). done.** the limit counts rows and stops only at a key boundary,
  so no row is half-attributed; a row counts the dictionaries that have the spelling it
  shows, matching what selecting it looks up. gate met both ways: the fixtures' shared
  "byte" reports two dictionaries (filed twice in one of them, counted once), and over the
  real collection four truncating prefixes returned 601 rows whose dictionary lists were
  identical to `lookup_all`'s. one thing the gate exposed: `λόγος` is two
  visually identical rows, tagged `GRC ·2` each — normalization variants that the wordlist
  never merged. pre-existing, now legible, tracked as roadmap #43.
- **C — fuzzy.** gate: the four measurements above reproduced in a release build on this
  collection; the scan runs off the main thread; a newer query cancels an in-flight one; a
  one-character typo finds the word.
- **D — lemmas only (#33).** depends on A for the fold key. gate: `rex` shows 1 row instead
  of 27 with the setting on; index-time pass under one second (measured: 333 ms).

## what was rejected

- **`fst`** — the scan is 67 ms and fuzzy is not as-you-type; that is the whole argument.
  (r2 also credited fst with "sub-millisecond fuzzy at distance 2", which is traversal only —
  DFA construction at distance 2 grows sharply with query length. don't reject it on a number
  that would lose the argument if it were true.) it also cannot hold the attributes, so it
  arrives with a side table: two structures where we have one.
- **SQLite** — the natural home for a side table, a poor matcher: no Levenshtein, and
  `LIKE 'x%'` is no better than a binary search. promote to it only if #12 and #33
  bookkeeping start feeling like queries, or Wiktionary (#37) needs real ETL.
- **redb, LMDB** — mutable stores; this data is static once built.
- **rkyv** — would retire the hand-rolled cache format, a real risk area, but adds no query
  power. a separate argument.
- **tantivy** — the right answer only for full-text search *inside* definitions.
- **bucketing fuzzy candidates by first character** — struck from the escalation ladder. it
  makes a first-character typo unreachable by construction, and Greek breathing marks
  (`ἀ`/`ἁ`/`α`) and Hebrew alef/ayin confusion are the most common first-character errors
  here. it is a correctness regression dressed as an optimisation.

## the falsifier

if the worst-case measured scan exceeds **150 ms single-threaded** on the target machine,
escalate — restrict fuzzy to lemmas first (37,273 keys instead of 1,223,585 for Latin), then
trigram postings. not first-character buckets, ever. the realistic trigger is Wiktionary
(#37): +389k–1.9M keys would put the single-threaded worst case around 150–250 ms, and that
is the point at which `fst` earns its keep.
