//! the command line. every subcommand here answers a question the window also
//! answers, through the same `Session` — which is the point: what the cli cannot
//! reach is entangled with the ui, and what it can reach is the api the window
//! should have been using (roadmap #46).
//!
//! two kinds of command live here. the **collection** ones — `search`, `define`,
//! `scope`, `index` — open the configured dictionaries and ask the session, so
//! their answers are the app's answers. the **file** ones — `dump`, `lookup` —
//! take a path and bypass the collection entirely; they are for looking at a
//! dictionary the app has not been told about yet, which is a different job.

use std::io::{self, Write};
use std::path::Path;

use gtk::glib;

use crate::library::Library;
use crate::session::{self, Session};
use crate::{config, dict};

/// run a subcommand if `args` names one. `None` means "no subcommand" — the
/// caller goes on to open the window.
pub fn run(args: &[String]) -> Option<glib::ExitCode> {
    let json = args.iter().any(|arg| arg == "--json");
    let value = |flag: &str| -> Option<&str> {
        let at = args.iter().position(|arg| arg == flag)?;
        args.get(at + 1).map(String::as_str)
    };
    let rest = args
        .get(2)
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'));

    Some(match args.get(1).map(String::as_str)? {
        "search" => search(rest, args, json, value("--limit")),
        "define" => define(rest, args, json),
        "scope" => scope(json),
        "index" => index(json),
        "dump" => dump(rest),
        "lookup" => lookup(
            rest,
            args.get(3).map(String::as_str),
            args.iter().any(|a| a == "--html"),
        ),
        "--help" | "-h" | "help" => usage(),
        _ => return None,
    })
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
  dictu define WORD  [--fold-forms] [--json]
  dictu scope [--json]                   the dictionaries, with their sizes
  dictu index [--json]                   what is loaded, and what it cost

over one file, whether or not it is in the collection:
  dictu dump FILE                        its name, size and first few entries
  dictu lookup FILE WORD [--html]        one word's entries";

/// open the collection the config points at, with the reader's settings applied.
fn session_from(args: &[String]) -> Session {
    let config = config::Config::load_or_create().unwrap_or_default();
    let entries = config::scan(&config.dictionary_dirs);
    let mut session = Session::empty();
    session.open(Library::open(&entries));
    session.set_fold_forms(args.iter().any(|arg| arg == "--fold-forms"));
    session
}

fn search(query: Option<&str>, args: &[String], json: bool, limit: Option<&str>) -> glib::ExitCode {
    let Some(query) = query else {
        return complain("usage: dictu search QUERY [--fold-forms] [--limit N] [--json]");
    };
    let session = session_from(args);
    let limit = limit
        .and_then(|n| n.parse().ok())
        .unwrap_or(session::ROW_LIMIT);
    let rows = session.search(query, limit);
    let truncated = rows.len() >= limit;

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
                            quoted(session.dict_label(*dict)),
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
        writeln!(out, "{}", session.status(rows.len(), truncated))?;
        for row in &rows {
            // one line per dictionary, naming the spelling it files the row under
            // — the row's own spelling is the first of them.
            for (dict, spelling) in &row.members {
                let under = match *spelling == row.word {
                    true => String::new(),
                    false => format!("  (under {spelling})"),
                };
                writeln!(out, "  [{}] {}{under}", session.dict_label(*dict), row.word)?;
            }
        }
        Ok(glib::ExitCode::SUCCESS)
    })
}

/// what the definition pane would show for a word: every in-scope dictionary that
/// answers, in the order the pane stacks them.
fn define(word: Option<&str>, args: &[String], json: bool) -> glib::ExitCode {
    let Some(word) = word else {
        return complain("usage: dictu define WORD [--fold-forms] [--json]");
    };
    let session = session_from(args);
    let Some(row) = session.resolve(word) else {
        // like `lookup`: say so through `printing`, but fail, or `define x | grep -q`
        // would call a missing word a success.
        printing(|out| {
            writeln!(out, "no entry for {word:?}")?;
            Ok(glib::ExitCode::SUCCESS)
        });
        return glib::ExitCode::FAILURE;
    };
    let defs = session.definitions(&row);

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
    let session = session_from(&[]);
    printing(|out| {
        if json {
            let dicts: Vec<String> = (0..session.dict_count())
                .map(|at| {
                    format!(
                        "  {{\"label\":{}, \"headwords\":{}}}",
                        quoted(session.dict_label(at)),
                        session.dict_headwords(at)
                    )
                })
                .collect();
            writeln!(out, "{{\"dictionaries\":[\n{}\n]}}", dicts.join(",\n"))?;
            return Ok(glib::ExitCode::SUCCESS);
        }
        for at in 0..session.dict_count() {
            writeln!(
                out,
                "{:>9}  {}",
                session::thousands(session.dict_headwords(at)),
                session.dict_label(at)
            )?;
        }
        writeln!(out, "{}", session.library_size())?;
        Ok(glib::ExitCode::SUCCESS)
    })
}

/// what a launch would load, and what the index cost — the numbers worth quoting
/// when a change is supposed to have made startup cheaper.
fn index(json: bool) -> glib::ExitCode {
    let began = std::time::Instant::now();
    let session = session_from(&[]);
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
                session.dict_count(),
                session.total_headwords(),
                opened.as_millis(),
                cached,
                quoted(&cache.display().to_string())
            )?;
            return Ok(glib::ExitCode::SUCCESS);
        }
        writeln!(out, "{}", session.library_size())?;
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
            session::quantity(entries.len(), "entry", "entries")
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

    /// `run` returns `None` for anything it does not own, which is what lets the
    /// window open on a bare `dictu` and on `dictu --search WORD`.
    #[test]
    fn only_subcommands_are_claimed() {
        let args =
            |list: &[&str]| -> Vec<String> { list.iter().map(|a| (*a).to_owned()).collect() };
        assert!(run(&args(&["dictu"])).is_none());
        assert!(run(&args(&["dictu", "--search", "rex"])).is_none());
        assert!(run(&args(&["dictu", "nonsense"])).is_none());
        // and it does claim its own, without needing a collection to say so
        assert!(run(&args(&["dictu", "--help"])).is_some());
    }
}
