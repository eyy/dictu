//! the command line. every subcommand here answers a question the window also
//! answers, through the same `Collection` — which is the point: what the cli
//! cannot reach is entangled with the ui, and what it can reach is the api the
//! window should have been using (roadmap #46).
//!
//! two kinds of command live here. the **collection** ones — `search`, `define`,
//! `scope`, `index` — open the configured dictionaries and ask the collection, so
//! their answers are the app's answers. the **file** ones — `dump`, `lookup` —
//! take a path and bypass the collection entirely; they are for looking at a
//! dictionary the app has not been told about yet, which is a different job.
//!
//! parsing is `clap`'s and json is `serde_json`'s. both were hand-rolled here
//! first, and a review found four bugs in twenty lines of argument handling alone
//! — a `--limit` that accepted `banana` and searched at 500, options refused
//! before the word, a mistyped command that raised the window and exited 0. that
//! is the class of thing these crates exist to have already gotten right.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use gtk::glib;
use serde::Serialize;

use crate::collection::{self, Collection};
use crate::library::Library;
use crate::{config, dict};

#[derive(Parser)]
#[command(
    name = "dictu",
    about = "an offline dictionary for classical languages",
    disable_help_subcommand = true
)]
struct Cli {
    /// open the window on a word — what the global hotkey passes
    #[arg(long, value_name = "WORD")]
    search: Option<String>,
    /// answer as json rather than as something to read
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// the rows a query would show, and the dictionaries that answer
    Search {
        query: String,
        /// hide rows that only repeat a definition another row already shows
        #[arg(long)]
        fold_forms: bool,
        /// how many rows before the answer is cut off
        #[arg(long, value_name = "N", default_value_t = collection::ROW_LIMIT)]
        limit: usize,
    },
    /// every definition the pane would show for a word
    Define { word: String },
    /// the dictionaries in the collection, with their sizes
    Scope,
    /// what a launch loads, and what it costs
    Index,
    /// time the things a reader waits for: opening the collection, and searching
    Bench {
        /// rebuild the index first, to time the cold path rather than the mapped one
        #[arg(long)]
        cold: bool,
    },
    /// one dictionary file's name, size and first entries
    Dump { file: PathBuf },
    /// one word's entries from one file, in the collection or not
    Lookup {
        file: PathBuf,
        word: String,
        /// the raw html rather than the rendered text
        #[arg(long)]
        html: bool,
    },
}

/// run a subcommand if `args` names one. `None` means "this invocation is for the
/// window" — a bare `dictu`, or `dictu --search WORD` from the hotkey.
pub fn run(args: &[String]) -> Option<glib::ExitCode> {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        // clap has written the message already — a usage error, or `--help` — and
        // its own exit code says which it was.
        Err(complaint) => {
            let failed = complaint.use_stderr();
            let _ = complaint.print();
            return Some(match failed {
                true => glib::ExitCode::FAILURE,
                false => glib::ExitCode::SUCCESS, // --help and --version
            });
        }
    };
    let json = cli.json;

    Some(match cli.command? {
        Command::Search {
            query,
            fold_forms,
            limit,
        } => search(&query, fold_forms, limit, json),
        Command::Define { word } => define(&word, json),
        Command::Scope => scope(json),
        Command::Index => index(json),
        Command::Bench { cold } => bench(cold, json),
        Command::Dump { file } => dump(&file),
        Command::Lookup { file, word, html } => lookup(&file, &word, html),
    })
}

/// the json shapes. named types rather than strings built by hand: escaping is
/// `serde_json`'s problem, and the shape becomes something a reader can see.
#[derive(Serialize)]
struct SearchOut<'a> {
    query: &'a str,
    rows: Vec<RowOut<'a>>,
    truncated: bool,
}

#[derive(Serialize)]
struct RowOut<'a> {
    word: &'a str,
    dictionaries: usize,
    members: Vec<MemberOut<'a>>,
}

#[derive(Serialize)]
struct MemberOut<'a> {
    dictionary: &'a str,
    spelling: &'a str,
}

#[derive(Serialize)]
struct DefineOut<'a> {
    word: &'a str,
    definitions: Vec<DefinitionOut<'a>>,
}

#[derive(Serialize)]
struct DefinitionOut<'a> {
    dictionary: &'a str,
    entries: Vec<String>,
}

#[derive(Serialize)]
struct ScopeOut<'a> {
    dictionaries: Vec<DictOut<'a>>,
}

#[derive(Serialize)]
struct DictOut<'a> {
    label: &'a str,
    headwords: usize,
}

#[derive(Serialize)]
struct BenchOut {
    headwords: usize,
    dictionaries: usize,
    /// how long `Library::open` took — the wait before the window is usable
    open_ms: u128,
    /// whether that was a rebuild or a mapped cache
    cold: bool,
    /// the high-water mark of this process, which is what the machine feels
    peak_rss_mb: u64,
    queries: Vec<QueryOut>,
}

#[derive(Serialize)]
struct QueryOut {
    query: String,
    rows: usize,
    /// best of several runs: the floor is the honest number for a hot cache, and
    /// the mean would mostly measure whatever else the machine was doing
    best_us: u128,
    worst_us: u128,
}

#[derive(Serialize)]
struct IndexOut {
    dictionaries: usize,
    headwords: usize,
    open_ms: u128,
    cache_bytes: u64,
    cache_dir: String,
}

/// open the collection the config points at.
fn opened(fold_forms: bool) -> Collection {
    let config = config::Config::load_or_create().unwrap_or_default();
    let entries = config::scan(&config.dictionary_dirs);
    let mut collection = Collection::empty();
    collection.open(Library::open(&entries));
    collection.set_fold_forms(fold_forms);
    collection
}

fn search(query: &str, fold_forms: bool, limit: usize, json: bool) -> glib::ExitCode {
    let collection = opened(fold_forms);
    // an empty query is the collection itself, which is what the window says for it
    // too — the two front ends describe one state one way.
    if query.trim().is_empty() {
        return printing(|out| {
            writeln!(out, "{}", collection.library_size())?;
            Ok(glib::ExitCode::SUCCESS)
        });
    }
    let (rows, truncated) = collection.page(query, limit);

    printing(|out| {
        if json {
            let answer = SearchOut {
                query,
                rows: rows
                    .iter()
                    .map(|row| RowOut {
                        word: &row.word,
                        dictionaries: row.dicts().len(),
                        members: row
                            .members
                            .iter()
                            .map(|(dict, spelling)| MemberOut {
                                dictionary: collection.dict_label(*dict),
                                spelling,
                            })
                            .collect(),
                    })
                    .collect(),
                truncated,
            };
            writeln!(out, "{}", serde_json::to_string_pretty(&answer)?)?;
            return Ok(glib::ExitCode::SUCCESS);
        }

        // the same line the window puts under its wordlist, from the same place.
        writeln!(out, "{}", collection.status(rows.len(), truncated))?;
        for row in &rows {
            // one line per dictionary, naming the spelling it files the row under
            // — the row's own spelling is the first of them.
            for (dict, spelling) in &row.members {
                let under = match *spelling == row.word {
                    true => String::new(),
                    false => format!("  (under {spelling})"),
                };
                writeln!(
                    out,
                    "  [{}] {}{under}",
                    collection.dict_label(*dict),
                    row.word
                )?;
            }
        }
        Ok(glib::ExitCode::SUCCESS)
    })
}

/// what the definition pane would show for a word: every in-scope dictionary that
/// answers, in the order the pane stacks them.
fn define(word: &str, json: bool) -> glib::ExitCode {
    let collection = opened(false);
    let Some(row) = collection.resolve(word) else {
        // say so through `printing` — and in the shape that was asked for, since a
        // caller piping `--json` wants "no definitions" parseable rather than a
        // syntax error. it still *fails*, or `define x | grep -q` would call a
        // missing word a success.
        printing(|out| {
            match json {
                true => {
                    let empty = DefineOut {
                        word,
                        definitions: Vec::new(),
                    };
                    writeln!(out, "{}", serde_json::to_string_pretty(&empty)?)?;
                }
                false => writeln!(out, "no entry for {word:?}")?,
            }
            Ok(glib::ExitCode::SUCCESS)
        });
        return glib::ExitCode::FAILURE;
    };
    let defs = collection.definitions(&row);

    printing(|out| {
        if json {
            let answer = DefineOut {
                word: &row.word,
                definitions: defs
                    .iter()
                    .map(|definition| DefinitionOut {
                        dictionary: &definition.label,
                        entries: definition
                            .entries
                            .iter()
                            .map(|entry| dict::html_to_text(entry))
                            .collect(),
                    })
                    .collect(),
            };
            writeln!(out, "{}", serde_json::to_string_pretty(&answer)?)?;
            return Ok(glib::ExitCode::SUCCESS);
        }

        writeln!(out, "{}\n", row.word)?;
        for definition in &defs {
            writeln!(out, "{}", definition.label)?;
            for (at, entry) in definition.entries.iter().enumerate() {
                if definition.entries.len() > 1 {
                    writeln!(out, "--- {} of {} ---", at + 1, definition.entries.len())?;
                }
                writeln!(out, "{}\n", dict::html_to_text(entry))?;
            }
        }
        Ok(glib::ExitCode::SUCCESS)
    })
}

/// the scope panel, as text: every dictionary the app loaded and how big it is.
fn scope(json: bool) -> glib::ExitCode {
    let collection = opened(false);
    printing(|out| {
        if json {
            let answer = ScopeOut {
                dictionaries: (0..collection.dict_count())
                    .map(|at| DictOut {
                        label: collection.dict_label(at),
                        headwords: collection.dict_headwords(at),
                    })
                    .collect(),
            };
            writeln!(out, "{}", serde_json::to_string_pretty(&answer)?)?;
            return Ok(glib::ExitCode::SUCCESS);
        }
        for at in 0..collection.dict_count() {
            writeln!(
                out,
                "{:>9}  {}",
                collection::thousands(collection.dict_headwords(at)),
                collection.dict_label(at)
            )?;
        }
        writeln!(out, "{}", collection.library_size())?;
        Ok(glib::ExitCode::SUCCESS)
    })
}

/// what a launch would load, and what the index cost — the numbers worth quoting
/// when a change is supposed to have made startup cheaper.
fn index(json: bool) -> glib::ExitCode {
    let began = std::time::Instant::now();
    let collection = opened(false);
    let took = began.elapsed();
    let cache = config::cache_dir();
    let cached: u64 = std::fs::read_dir(&cache)
        .map(|dir| {
            dir.filter_map(Result::ok)
                .filter_map(|file| file.metadata().ok())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0);

    printing(|out| {
        if json {
            let answer = IndexOut {
                dictionaries: collection.dict_count(),
                headwords: collection.total_headwords(),
                open_ms: took.as_millis(),
                cache_bytes: cached,
                cache_dir: cache.display().to_string(),
            };
            writeln!(out, "{}", serde_json::to_string_pretty(&answer)?)?;
            return Ok(glib::ExitCode::SUCCESS);
        }
        writeln!(out, "{}", collection.library_size())?;
        writeln!(out, "opened in {:.2}s", took.as_secs_f64())?;
        writeln!(
            out,
            "cache      {:.0} MB in {}",
            cached as f64 / 1_048_576.0,
            cache.display()
        )?;
        Ok(glib::ExitCode::SUCCESS)
    })
}

/// the queries a reader actually waits on, chosen to span what makes the search
/// expensive: a one-character prefix walks the widest run, a long latin one walks
/// a dense paradigm, and greek and hebrew exercise the mark filtering that latin
/// never touches.
const BENCH_QUERIES: [&str; 7] = ["a", "rex", "esse", "consuetudino", "λόγος", "מלך", "כאב"];

/// how many times each query runs. the best of them is reported: a search is
/// deterministic, so a slower run measures the machine, not the code.
const BENCH_RUNS: usize = 5;

fn bench(cold: bool, json: bool) -> glib::ExitCode {
    if cold {
        // the cold path is the one that reads every dictionary and sorts 1.9M
        // headwords; it only happens with no cache to map, so make that true.
        let cache = config::cache_dir();
        if let Err(e) = std::fs::remove_dir_all(&cache)
            && e.kind() != io::ErrorKind::NotFound
        {
            return complain(&format!("could not clear {}: {e}", cache.display()));
        }
    }

    let began = std::time::Instant::now();
    let collection = opened(false);
    let open_ms = began.elapsed().as_millis();

    let queries = BENCH_QUERIES
        .iter()
        .map(|query| {
            let mut best = u128::MAX;
            let mut worst = 0;
            let mut rows = 0;
            for _ in 0..BENCH_RUNS {
                let began = std::time::Instant::now();
                let found = collection.search(query, collection::ROW_LIMIT);
                let took = began.elapsed().as_micros();
                rows = found.len();
                best = best.min(took);
                worst = worst.max(took);
            }
            QueryOut {
                query: (*query).to_owned(),
                rows,
                best_us: best,
                worst_us: worst,
            }
        })
        .collect();

    let answer = BenchOut {
        headwords: collection.total_headwords(),
        dictionaries: collection.dict_count(),
        open_ms,
        cold,
        peak_rss_mb: peak_rss_mb(),
        queries,
    };

    printing(|out| {
        if json {
            writeln!(out, "{}", serde_json::to_string_pretty(&answer)?)?;
            return Ok(glib::ExitCode::SUCCESS);
        }
        writeln!(
            out,
            "{} · {} — opened {} in {} ms, peak {} MB",
            collection::quantity(answer.headwords, "word", "words"),
            collection::quantity(answer.dictionaries, "dictionary", "dictionaries"),
            match cold {
                true => "cold",
                false => "warm",
            },
            answer.open_ms,
            answer.peak_rss_mb
        )?;
        for query in &answer.queries {
            writeln!(
                out,
                "  {:>14}  {:>6.1} ms   {} rows",
                query.query,
                query.best_us as f64 / 1000.0,
                query.rows
            )?;
        }
        Ok(glib::ExitCode::SUCCESS)
    })
}

/// this process's high-water memory, from `/proc/self/status`. the number that
/// matters is the peak, not the current: the cold index build allocates far more
/// than the mapped steady state, and it is the peak that gets a session killed.
fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<u64>()
                .ok()
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

fn dump(path: &Path) -> glib::ExitCode {
    let dict = match dict::open_any(path, Some(&config::cache_dir())) {
        Ok(dict) => dict,
        Err(e) => return complain(&format!("error: {e:#}")),
    };
    printing(|out| {
        writeln!(out, "name:      {}", dict.name())?;
        let headwords = dict.headwords();
        writeln!(out, "headwords: {}", headwords.len())?;
        for word in headwords.iter().take(5) {
            let entries = dict.lookup(word);
            let text = entries
                .first()
                .map(|entry| dict::html_to_text(entry))
                .unwrap_or_default();
            let preview: String = text.chars().take(100).collect();
            // say when a headword has more than one entry; that is easy to miss and
            // it is exactly what made `sam` look broken.
            let more = match entries.len() {
                0 | 1 => String::new(),
                n => format!("  [{n} entries]"),
            };
            writeln!(out, "  {word:?} -> {preview:?}{more}")?;
        }
        Ok(glib::ExitCode::SUCCESS)
    })
}

/// one word's entries from one file — plain text by default, the raw html with
/// `--html`, which is what markup work needs to see.
fn lookup(path: &Path, word: &str, html: bool) -> glib::ExitCode {
    let dict = match dict::open_any(path, Some(&config::cache_dir())) {
        Ok(dict) => dict,
        Err(e) => return complain(&format!("error: {e:#}")),
    };
    let entries = dict.lookup(word);
    if entries.is_empty() {
        // the verdict is about the dictionary, not about whether anyone was still
        // listening: report through `printing` (so a closed pipe stays quiet) but
        // fail regardless, or `lookup … | grep -q x` would call a missing word a
        // success the moment grep exits early.
        printing(|out| {
            writeln!(out, "{}: no entry for {word:?}", dict.name())?;
            Ok(glib::ExitCode::SUCCESS)
        });
        return glib::ExitCode::FAILURE;
    }
    printing(|out| {
        writeln!(
            out,
            "{} — {word} ({})\n",
            dict.name(),
            collection::quantity(entries.len(), "entry", "entries")
        )?;
        for (at, entry) in entries.iter().enumerate() {
            if entries.len() > 1 {
                writeln!(out, "--- {} of {} ---", at + 1, entries.len())?;
            }
            match html {
                true => writeln!(out, "{entry}")?,
                false => writeln!(out, "{}", dict::html_to_text(entry))?,
            }
        }
        Ok(glib::ExitCode::SUCCESS)
    })
}

fn complain(message: &str) -> glib::ExitCode {
    eprintln!("{message}");
    glib::ExitCode::FAILURE
}

/// print through one locked stdout handle, stopping at the first failed write.
/// these commands exist to be piped into `head`/`grep`, and rust ignores SIGPIPE
/// — so `println!` panics once the reader goes away. a closed pipe is the reader's
/// choice, not a failure: say nothing and exit 0.
fn printing(
    write: impl FnOnce(&mut io::StdoutLock) -> io::Result<glib::ExitCode>,
) -> glib::ExitCode {
    let mut out = io::stdout().lock();
    match write(&mut out) {
        Ok(code) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => glib::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: writing to stdout: {e}");
            glib::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the shapes serialize as the contract says, and dictionary text — quotes,
    /// newlines, greek, hebrew — survives being one of the values. this used to
    /// test a hand-rolled escaper; it now tests that the *shape* is what a caller
    /// parses, which is the part still ours to get wrong.
    #[test]
    fn the_json_shape_is_what_a_caller_parses() {
        let answer = DefineOut {
            word: "λόγος",
            definitions: vec![DefinitionOut {
                dictionary: "Bailly 2020 (Grc-Fra)",
                entries: vec!["say \"x\"\nand a second line".to_owned()],
            }],
        };
        let text = serde_json::to_string(&answer).expect("serializes");
        let back: serde_json::Value = serde_json::from_str(&text).expect("parses");
        assert_eq!(back["word"], "λόγος");
        assert_eq!(
            back["definitions"][0]["dictionary"],
            "Bailly 2020 (Grc-Fra)"
        );
        assert_eq!(
            back["definitions"][0]["entries"][0],
            "say \"x\"\nand a second line"
        );
    }

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    /// what the window gets and what the cli claims. the window's share is a bare
    /// launch and the hotkey's `--search WORD`; a word that is not a command is an
    /// error, so `dictu serach x || fail` can fail.
    #[test]
    fn only_the_window_gets_what_is_not_a_command() {
        assert!(
            Cli::try_parse_from(argv(&["dictu"]))
                .unwrap()
                .command
                .is_none()
        );
        let hotkey = Cli::try_parse_from(argv(&["dictu", "--search", "rex"])).unwrap();
        assert!(hotkey.command.is_none());
        assert_eq!(hotkey.search.as_deref(), Some("rex"));
        assert!(Cli::try_parse_from(argv(&["dictu", "serach", "rex"])).is_err());
        assert_eq!(
            run(&argv(&["dictu", "serach", "rex"])),
            Some(glib::ExitCode::FAILURE)
        );
    }

    /// the declaration, not clap: that a limit is a number, that the flags are
    /// global so they may come before the word, and that `--limit` is not read as
    /// one. each of these was a bug when this was twenty hand-written lines.
    #[test]
    fn options_and_words_can_come_in_any_order() {
        let parsed = |list: &[&str]| Cli::try_parse_from(argv(list));
        let query_of = |cli: Cli| match cli.command {
            Some(Command::Search { query, limit, .. }) => (query, limit),
            _ => panic!("expected a search"),
        };

        assert_eq!(
            query_of(parsed(&["dictu", "search", "rex"]).unwrap()),
            ("rex".to_owned(), collection::ROW_LIMIT)
        );
        assert_eq!(
            query_of(parsed(&["dictu", "search", "--limit", "10", "rex"]).unwrap()),
            ("rex".to_owned(), 10)
        );
        assert!(parsed(&["dictu", "search", "--json", "rex"]).unwrap().json);
        // a limit that is not a number is refused, rather than quietly becoming 500
        assert!(parsed(&["dictu", "search", "rex", "--limit", "banana"]).is_err());
        assert!(parsed(&["dictu", "search", "rex", "--limit"]).is_err());
        // and a word is still required
        assert!(parsed(&["dictu", "search"]).is_err());
    }
}
