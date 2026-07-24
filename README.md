# dictu

a gnome dictionary app for large offline dictionary collections. it scans directories of
dictionary files, builds one merged index across all of them, and answers a single search
box with every definition a word has, from every dictionary that has it.

built for classical-language work — inflected Latin, Greek, Hebrew lexica of a few million
headwords — where the usual answer is a heavyweight app or a browser tab. dictu is a
native gtk4 window, reads the files where they already sit, and never touches the network.

status: **alpha**, and honestly so. it works daily; several rough edges and two unwritten
format parsers are tracked in [`roadmap.md`](roadmap.md).

```
$ dictu search dacrima
8 dicts, 4009914 headwords total
  [bgl-Latin_English_Inflected] dacrima
  [fulllatininflected[1]] dacrima
  [stardic latin english inflected] dacrima
  ...
```

## features

**one search across every dictionary.** all headwords from all dictionaries go into a
single case-insensitive index. typing a prefix searches the whole collection at once;
picking a word shows its definition from each dictionary that defines it, one under the
other, each under its source label.

**no browser engine.** definitions arrive as html. dictu parses it with html5ever and maps
a controlled subset of tags — `b`, `i`, `sup`/`sub`, headers, `a`, `tt`, block elements —
onto its own text styles, then renders those with `gtk::TextView` tags. every dictionary's
own inline css, colours and classes are **ignored on purpose**, so a 19th-century lexicon
and a modern glossary render in the same consistent look rather than a collage.

**built for millions of headwords.** the index stores two `u32`s per headword, not the
words themselves; prefix search is a binary search plus a walk of the matching run.
`.dict` files are memory-mapped, so definitions are paged in by the kernel on demand
instead of read into RAM (the largest dictionary here is 186 MB). indexing runs on a
worker thread, so the window is up and responsive while it works.

**single instance with a lookup hotkey.** `dictu --search WORD` forwards to the
already-running window, focuses it and fills the search box. bound to a shell hotkey
(`Super+\` → read the primary selection → `dictu --search "$word"`), it turns any selected
word anywhere on the desktop into a lookup.

**no-gui subcommands.** `dictu dump <file>` prints one dictionary's metadata and first
entries; `dictu search <query>` runs the real search engine and prints the hits. both work
without a display, which is how parsing and search get verified.

### dictionary formats

| format | files | status |
| --- | --- | --- |
| StarDict | `.ifo` + `.idx` + `.dict`/`.dict.dz`, optional `.syn` synonyms | supported |
| dictd | `.index` + `.dict`/`.dict.dz` | supported |
| csv | tab-separated (the LSJ author/work abbreviation table) | supported |
| ABBYY Lingvo DSL | `.dsl`, `.dsl.dz` | detected, not parsed yet — [roadmap #13](roadmap.md) |
| Babylon | `.bgl` | convert to StarDict with pyglossary first |

gzip and dictzip (`.dz`) are read transparently, with a decompression cap as a zip-bomb
guard.

## the parts

| path | what it does |
| --- | --- |
| `src/main.rs` | the gtk4/libadwaita ui — sidebar search + wordlist, definition pane, the `TextTag` styling, the off-thread index load — plus argv handling and the `dump`/`search` subcommands |
| `src/config.rs` | `config.toml` (xdg), and the recursive directory scan that discovers dictionaries and classifies them by format |
| `src/library.rs` | the whole collection: opens every dictionary, builds the merged sorted index, answers prefix searches and cross-dictionary lookups |
| `src/dict/mod.rs` | the `Dictionary` trait every format implements, format classification, mmap and gzip plumbing |
| `src/dict/stardict.rs` | StarDict reader — `.ifo` metadata, big-endian `.idx`, `sametypesequence`, `.syn` synonyms |
| `src/dict/dictd.rs` | dictd reader — the tab-separated `.index` with dictd's own base-64 offset encoding |
| `src/dict/csv.rs` | the tab-separated LSJ abbreviations table |
| `src/dict/markup.rs` | html → styled text runs. ui-agnostic (no gtk), so it's unit-testable |
| `sample/` | a small hand-built dictd fixture used by the smoke tests and ui e2e |
| `hack/` | the feedback loop — `check.sh` (format, lint, test, smoke, e2e), `e2e.py` (drives the real ui over at-spi), `shot.sh` (screenshots it) |

the data layer knows nothing about gtk, and the ui knows nothing about dictionary file
layouts — `Dictionary` is the whole contract between them: a name, a list of headwords, and
`lookup(headword) -> Option<html>`. adding a format means implementing that trait and
teaching `classify` its extension.

## configuration

`~/.config/dictu/config.toml`, written with defaults on first run:

```toml
dictionary_dirs = ["/home/you/dict"]
```

each plain path is scanned recursively for dictionaries. a path prefixed with `!` excludes
everything beneath it — the inverse of gitignore's `!` — which is how a dictionary is
dropped from the collection without moving files.

## building

needs rust (2024 edition) and the gtk4 + libadwaita development headers:

```bash
sudo apt install -y libgtk-4-dev libadwaita-1-dev build-essential pkg-config
cargo build
```

then run the checks — format, lint, unit tests, cli smoke tests, and a ui end-to-end pass
that drives the real widget tree over the accessibility bus:

```bash
hack/check.sh
```

[`AGENTS.md`](AGENTS.md) documents the whole working process: the loop, how to screenshot
the app unattended, the at-spi harness, and the gotchas worth not rediscovering.
