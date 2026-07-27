//! the command line. every subcommand here answers a question the window also
//! answers, through the same `Collection` — which is the point: what the cli cannot
//! reach is entangled with the ui, and what it can reach is the api the window
//! should have been using (roadmap #46).
//!
//! two kinds of command live here. the **collection** ones — `search`, `define`,
//! `scope`, `index` — open the configured dictionaries and ask the collection, so
//! their answers are the app's answers. the **file** ones — `dump`, `lookup` —
//! take a path and bypass the collection entirely; they are for looking at a
//! dictionary the app has not been told about yet, which is a different job.

use std::io::{self, Write};
use std::path::Path;

use gtk::glib;

use crate::collection::{self, Collection};
use crate::library::Library;
use crate::{config, dict};

/// run a subcommand if `args` names one. `None` means "this invocation is for the
/// window" — a bare `dictu`, or `dictu --search WORD` from the hotkey. anything
/// else in the first position is a mistyped subcommand and says so, rather than
/// silently raising the window and exiting 0.
pub fn run(args: &[String]) -> Option<glib::ExitCode> {
    let json = flag(args, "--json");
    let positional = positionals(args);
    let word = |at: usize| positional.get(at).copied();

    Some(match args.get(1).map(String::as_str)? {
        "search" => match limit_of(args) {
            Ok(limit) => search(word(0), args, json, limit),
            Err(bad) => complain(&format!("not a row count: {bad}")),
        },
        "define" => define(word(0), args, json),
        "scope" => scope(json),
        "index" => index(json),
        "dump" => dump(word(0)),
        "lookup" => lookup(word(0), word(1), flag(args, "--html")),
        "--help" | "-h" | "help" => usage(),
        // an option in the first position is the window's (`--search WORD`); a word
        // is a subcommand that does not exist.
        other if other.starts_with('-') => return None,
        other => complain(&format!("no such command: {other}\n\n{USAGE}")),
    })
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

/// the arguments that are not options, in order — so a flag may come before the
/// word as easily as after it. `--limit` is the only option that takes a value, so
/// it is the only one whose value has to be stepped over.
fn positionals(args: &[String]) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = args.iter().skip(2).peekable();
    while let Some(arg) = rest.next() {
        if arg == "--limit" {
            rest.next();
        } else if !arg.starts_with('-') {
            found.push(arg.as_str());
        }
    }
    found
}

/// the row limit asked for, if any. a value that isn't a number is an error rather
/// than a silent fallback: a measurement taken at a limit you did not ask for is
/// how a wrong number reaches a commit message.
fn limit_of(args: &[String]) -> Result<usize, &str> {
    let Some(at) = args.iter().position(|arg| arg == "--limit") else {
        return Ok(collection::ROW_LIMIT);
    };
    let given = args.get(at + 1).map(String::as_str).unwrap_or("");
    given.parse().map_err(|_| given)
}

fn usage() -> glib::ExitCode {
    printing(|out| {
        writeln!(out, "{USAGE}")?;
        Ok(glib::ExitCode::SUCCESS)
    })
}

const USAGE: &str = "\
dictu — an offline dictionary for classical languages

  dictu                                  open the window
  dictu --search WORD                    open it (or the running one) on a word

over the configured collection:
  dictu search QUERY [--fold-forms] [--limit N] [--json]
  dictu define WORD  [--json]            what the pane would show for it
  dictu scope [--json]                   the dictionaries, with their sizes
  dictu index [--json]                   what is loaded, and what it cost

over one file, whether or not it is in the collection:
  dictu dump FILE                        its name, size and first few entries
  dictu lookup FILE WORD [--html]        one word's entries";

/// open the collection the config points at, with the reader's settings applied.
fn collection_from(args: &[String]) -> Collection {
    let config = config::Config::load_or_create().unwrap_or_default();
    let entries = config::scan(&config.dictionary_dirs);
    let mut collection = Collection::empty();
    collection.open(Library::open(&entries));
    collection.set_fold_forms(args.iter().any(|arg| arg == "--fold-forms"));
    collection
}

fn search(query: Option<&str>, args: &[String], json: bool, limit: usize) -> glib::ExitCode {
    let Some(query) = query else {
        return complain("usage: dictu search QUERY [--fold-forms] [--limit N] [--json]");
    };
    let collection = collection_from(args);
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
            writeln!(out, "{{\"query\":{}, \"rows\":[", quoted(query))?;
            for (at, row) in rows.iter().enumerate() {
                let members: Vec<String> = row
                    .members
                    .iter()
                    .map(|(dict, spelling)| {
                        format!(
                            "{{\"dictionary\":{}, \"spelling\":{}}}",
                            quoted(collection.dict_label(*dict)),
                            quoted(spelling)
                        )
                    })
                    .collect();
                let comma = if at + 1 == rows.len() { "" } else { "," };
                writeln!(
                    out,
                    "  {{\"word\":{}, \"dictionaries\":{}, \"members\":[{}]}}{comma}",
                    quoted(&row.word),
                    row.dicts().len(),
                    members.join(", ")
                )?;
            }
            writeln!(out, "], \"truncated\":{truncated}}}")?;
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
fn define(word: Option<&str>, args: &[String], json: bool) -> glib::ExitCode {
    let Some(word) = word else {
        return complain("usage: dictu define WORD [--json]");
    };
    let collection = collection_from(args);
    let Some(row) = collection.resolve(word) else {
        // say so through `printing` — and in the shape that was asked for, since a
        // caller piping `--json` wants "no definitions" parseable rather than a
        // syntax error. it still *fails*, or `define x | grep -q` would call a
        // missing word a success.
        printing(|out| {
            match json {
                true => writeln!(out, "{{\"word\":{}, \"definitions\":[]}}", quoted(word))?,
                false => writeln!(out, "no entry for {word:?}")?,
            }
            Ok(glib::ExitCode::SUCCESS)
        });
        return glib::ExitCode::FAILURE;
    };
    let defs = collection.definitions(&row);

    printing(|out| {
        if json {
            writeln!(out, "{{\"word\":{}, \"definitions\":[", quoted(&row.word))?;
            for (at, definition) in defs.iter().enumerate() {
                let entries: Vec<String> = definition
                    .entries
                    .iter()
                    .map(|entry| quoted(&dict::html_to_text(entry)))
                    .collect();
                let comma = if at + 1 == defs.len() { "" } else { "," };
                writeln!(
                    out,
                    "  {{\"dictionary\":{}, \"entries\":[{}]}}{comma}",
                    quoted(&definition.label),
                    entries.join(", ")
                )?;
            }
            writeln!(out, "]}}")?;
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
    let collection = collection_from(&[]);
    printing(|out| {
        if json {
            let dicts: Vec<String> = (0..collection.dict_count())
                .map(|at| {
                    format!(
                        "  {{\"label\":{}, \"headwords\":{}}}",
                        quoted(collection.dict_label(at)),
                        collection.dict_headwords(at)
                    )
                })
                .collect();
            writeln!(out, "{{\"dictionaries\":[\n{}\n]}}", dicts.join(",\n"))?;
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
    let collection = collection_from(&[]);
    let opened = began.elapsed();
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
            writeln!(
                out,
                "{{\"dictionaries\":{}, \"headwords\":{}, \"open_ms\":{}, \"cache_bytes\":{}, \"cache_dir\":{}}}",
                collection.dict_count(),
                collection.total_headwords(),
                opened.as_millis(),
                cached,
                quoted(&cache.display().to_string())
            )?;
            return Ok(glib::ExitCode::SUCCESS);
        }
        writeln!(out, "{}", collection.library_size())?;
        writeln!(out, "opened in {:.2}s", opened.as_secs_f64())?;
        writeln!(
            out,
            "cache      {:.0} MB in {}",
            cached as f64 / 1_048_576.0,
            cache.display()
        )?;
        Ok(glib::ExitCode::SUCCESS)
    })
}

fn dump(path: Option<&str>) -> glib::ExitCode {
    let Some(path) = path else {
        return complain("usage: dictu dump FILE");
    };
    let dict = match dict::open_any(Path::new(path), Some(&config::cache_dir())) {
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
fn lookup(path: Option<&str>, word: Option<&str>, html: bool) -> glib::ExitCode {
    let (Some(path), Some(word)) = (path, word) else {
        return complain("usage: dictu lookup FILE WORD [--html]");
    };
    let dict = match dict::open_any(Path::new(path), Some(&config::cache_dir())) {
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

/// a json string. hand-rolled because this is the only json the app emits and a
/// serializer for six fields is a dependency for nothing — but escaped properly,
/// since dictionary text is full of quotes, backslashes and newlines.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_survive_dictionary_text() {
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("say \"x\""), "\"say \\\"x\\\"\"");
        assert_eq!(quoted("a\\b"), "\"a\\\\b\"");
        assert_eq!(quoted("two\nlines"), "\"two\\nlines\"");
        // greek and hebrew go through as themselves; json is utf-8.
        assert_eq!(quoted("λόγος"), "\"λόγος\"");
        assert_eq!(quoted("\u{1}"), "\"\\u0001\"");
    }

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    /// `run` claims a subcommand, hands the window everything option-shaped, and
    /// refuses a word it does not know rather than silently opening the window.
    #[test]
    fn only_the_window_gets_what_is_not_a_command() {
        assert!(
            run(&argv(&["dictu"])).is_none(),
            "a bare launch is the window's"
        );
        assert!(
            run(&argv(&["dictu", "--search", "rex"])).is_none(),
            "so is the hotkey's argv"
        );
        assert!(run(&argv(&["dictu", "--help"])).is_some());
        // a mistyped command is an error, not a window: `dictu serach x || fail`
        // has to be able to fail.
        assert_eq!(
            run(&argv(&["dictu", "serach", "rex"])),
            Some(glib::ExitCode::FAILURE)
        );
    }

    /// a flag may come before the word as easily as after it, and `--limit`'s value
    /// is not a word.
    #[test]
    fn options_and_words_can_come_in_any_order() {
        assert_eq!(positionals(&argv(&["dictu", "search", "rex"])), ["rex"]);
        assert_eq!(
            positionals(&argv(&["dictu", "search", "--json", "rex"])),
            ["rex"]
        );
        assert_eq!(
            positionals(&argv(&["dictu", "search", "--limit", "10", "rex"])),
            ["rex"]
        );
        assert_eq!(
            positionals(&argv(&["dictu", "lookup", "--html", "file.ifo", "rex"])),
            ["file.ifo", "rex"]
        );
        assert!(positionals(&argv(&["dictu", "search", "--json"])).is_empty());
    }

    /// a limit that is not a number is an error, not a quiet fallback — the whole
    /// point of the flag is measuring at a number you chose.
    #[test]
    fn a_limit_that_is_not_a_number_is_refused() {
        assert_eq!(
            limit_of(&argv(&["dictu", "search", "rex"])),
            Ok(ROW_LIMIT_FOR_TEST)
        );
        assert_eq!(
            limit_of(&argv(&["dictu", "search", "rex", "--limit", "12"])),
            Ok(12)
        );
        assert_eq!(
            limit_of(&argv(&["dictu", "search", "rex", "--limit", "banana"])),
            Err("banana")
        );
        // and a flag with nothing after it is just as wrong
        assert_eq!(
            limit_of(&argv(&["dictu", "search", "rex", "--limit"])),
            Err("")
        );
    }

    const ROW_LIMIT_FOR_TEST: usize = collection::ROW_LIMIT;
}
