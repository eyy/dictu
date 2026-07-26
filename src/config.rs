//! app configuration: a `config.toml` sitting next to the executable, holding
//! the list of directories to scan (recursively) for dictionary files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Value};

use crate::dict::{self, Format};

/// the on-disk config. serde reads it; writing goes through `edited`, which
/// edits the document rather than regenerating it, so hand-written comments
/// survive a save.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// a list of paths. a plain path is a directory scanned recursively for
    /// dictionaries; a path prefixed with `!` excludes anything under it (used
    /// to hide individual dicts the user removed from the panel). note this is
    /// the inverse of gitignore's `!` (which re-includes) — here `!` excludes.
    /// exact string prefixes; paths aren't canonicalized, so keep includes and
    /// excludes spelled the same way (no mixing a symlink with its target).
    #[serde(default)]
    pub dictionary_dirs: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        // default: the user's sys/dict collection. built from $HOME so it isn't
        // hard-coded to one machine's absolute path.
        let dictionary_dirs = home_dir()
            .map(|home| {
                vec![
                    home.join("Dictionaries")
                        .to_string_lossy()
                        .into_owned(),
                ]
            })
            .unwrap_or_default();
        Self { dictionary_dirs }
    }
}

impl Config {
    /// where the config lives: `$XDG_CONFIG_HOME/dictu/config.toml`, i.e. the
    /// idiomatic `~/.config/dictu/config.toml`.
    pub fn path() -> PathBuf {
        config_home().join("dictu").join("config.toml")
    }

    /// load the config, or create it with defaults (and write it) if absent.
    pub fn load_or_create() -> Result<Self> {
        let path = Self::path();
        if path.exists() {
            let text =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
        } else {
            let config = Self::default();
            config.save().ok(); // best-effort; missing config isn't fatal.
            Ok(config)
        }
    }

    /// write the config back to disk, creating the directory if needed. an
    /// existing file is *edited*, not regenerated — see `edited`.
    fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let existing = fs::read_to_string(&path).ok();
        let text = self.edited(existing.as_deref())?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// this config applied to `existing`, as text. a hand-written config.toml says
    /// *why* a dictionary is excluded, and serde round-tripping would drop every
    /// word of it — so the document is edited in place instead: entries that
    /// survive keep their own comments, and what the app doesn't own (other keys,
    /// blank lines, the file's layout) is carried through. `None` means there is no
    /// file yet, so one is generated.
    ///
    /// one thing does change that we did not ask to change: the parser normalizes
    /// CRLF line endings to LF across the whole file. content is preserved, bytes
    /// on untouched lines are not.
    ///
    /// pure, and separate from `save`, so a test can round-trip a commented
    /// fixture without a real config to overwrite.
    fn edited(&self, existing: Option<&str>) -> Result<String> {
        let mut doc = match existing {
            Some(text) => text
                .parse::<DocumentMut>()
                .context("parsing the existing config.toml")?,
            None => DocumentMut::new(),
        };

        let array = doc
            .entry("dictionary_dirs")
            .or_insert(Item::Value(Value::Array(Array::new())))
            .as_array_mut()
            .context("dictionary_dirs is in the config but is not an array")?;

        // whether the file writes one entry per line. keep whichever it chose —
        // reflowing an inline array is a change nobody asked for — and read it
        // before editing, since the entry that proves it may be the one removed.
        let mut lines = array.iter().any(|value| prefix_of(value).contains('\n'))
            || array.trailing().as_str().is_some_and(|t| t.contains('\n'));
        lines |= existing.is_none() && self.dictionary_dirs.len() > 1;

        // take the entries out: from here the layout is ours to write back.
        let mut items: Vec<Value> = array.iter().cloned().collect();
        let mut trailing = array.trailing().as_str().unwrap_or("").to_owned();
        let indent = indent_of(&items);
        // every comment above the entry it is about, so that removing an entry
        // removes its comments and only its comments.
        let header = hoist_comments(&mut items, &mut trailing, &indent);

        // rebuild in the order `self` asks for, each surviving entry keeping the
        // decor — its comments — it came with. rebuilding rather than filtering is
        // what makes the file follow a reordered list, and collapses a duplicate
        // the file happened to hold twice.
        let mut taken = vec![false; items.len()];
        let mut wanted: Vec<Value> = Vec::new();
        for dir in &self.dictionary_dirs {
            let found = items
                .iter()
                .position(|value| value.as_str() == Some(dir.as_str()));
            match found.filter(|i| !taken[*i]) {
                Some(i) => {
                    taken[i] = true;
                    wanted.push(items[i].clone());
                }
                None => wanted.push(Value::from(dir.as_str())),
            }
        }
        // a value we can't read as a string stays: we don't know what it is, and
        // deleting what it doesn't understand is the one thing this must not do.
        for (i, item) in items.iter().enumerate() {
            if !taken[i] && item.as_str().is_none() {
                wanted.push(item.clone());
            }
        }

        array.clear();
        for (i, mut value) in wanted.into_iter().enumerate() {
            let prefix = prefix_of(&value);
            let prefix = match (lines, prefix.contains('\n'), i) {
                // an entry that already sits on its own line keeps its exact decor.
                (true, true, _) => prefix,
                (true, false, _) => format!("\n{indent}"),
                (false, _, 0) => String::new(),
                (false, _, _) => " ".to_owned(),
            };
            let decor = value.decor_mut();
            decor.set_prefix(prefix);
            // the newline before `]` belongs to the array's trailing text; left on
            // the last entry it would be emitted before the comma.
            decor.set_suffix("");
            array.push_formatted(value);
        }

        // the comment on the `[` line is about the key, not about whichever entry
        // happened to be written first, so it goes back where it was.
        if let Some(comment) = header
            && let Some(first) = array.get_mut(0)
        {
            let decor = first.decor_mut();
            let prefix = decor.prefix().and_then(|p| p.as_str()).unwrap_or("");
            decor.set_prefix(format!(" {comment}{prefix}"));
        }

        array.set_trailing_comma(lines && !array.is_empty());
        if lines {
            // whatever else was written before the bracket (a comment on its own
            // line) is kept; a bare newline is the minimum.
            array.set_trailing(if trailing.contains('\n') {
                trailing
            } else {
                "\n".to_owned()
            });
        } else {
            array.set_trailing(trailing.trim_end().to_owned());
        }
        Ok(doc.to_string())
    }

    /// exclude a path (gitignore-style `!` prefix) and persist — a permanent
    /// "never load this again", the counterpart to the session-only search scope.
    // still unwired, but no longer unwirable: saving preserves comments now
    // (roadmap #38), so a "remove this dictionary for good" ui is possible. the
    // scope panel (#14) is deliberately session-only and stays that way until
    // there is a ui that says it means forever.
    #[allow(dead_code)]
    pub fn exclude(&mut self, path: &Path) -> Result<()> {
        let entry = format!("!{}", path.display());
        if !self.dictionary_dirs.iter().any(|e| e == &entry) {
            self.dictionary_dirs.push(entry);
        }
        self.save()
    }
}

/// where the on-disk index cache lives: `$XDG_CACHE_HOME/dictu`, i.e. the
/// idiomatic `~/.cache/dictu`. everything in it is derived data — deleting it
/// costs one slow launch and nothing else (see `index_cache`).
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| home_dir().map(|h| h.join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dictu")
}

/// a dictionary discovered on disk (not yet loaded — loading is deferred until
/// the user selects it, since some files are hundreds of MB).
#[derive(Debug, Clone)]
pub struct DictEntry {
    pub path: PathBuf,
    pub format: Format,
    /// display label — the containing folder's name (the collection is
    /// organized one dictionary per folder), else the file stem.
    pub label: String,
}

/// recursively scan the configured `entries` for primary dictionary files.
/// gitignore-style: plain entries are include-dirs; `!`-prefixed entries exclude
/// anything beneath them. deduplicates DSL `.dsl`/`.dsl.dz` pairs and sorts by
/// label.
pub fn scan(entries: &[String]) -> Vec<DictEntry> {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    for entry in entries {
        match entry.strip_prefix('!') {
            Some(path) => excludes.push(PathBuf::from(path)),
            None => includes.push(PathBuf::from(entry)),
        }
    }

    let mut files = Vec::new();
    for dir in &includes {
        collect(dir, &mut files);
    }

    let mut entries: Vec<DictEntry> = files
        .into_iter()
        .filter_map(|path| {
            // skip anything under an excluded path (a removed dict).
            if excludes.iter().any(|ex| path.starts_with(ex)) {
                return None;
            }
            let format = dict::classify(&path)?;
            // only list formats we can actually open — which keeps raw .bgl out of
            // the picker, since it is converted to StarDict offline instead.
            if !format.is_supported() {
                return None;
            }
            let label = label_for(&path);
            Some(DictEntry {
                path,
                format,
                label,
            })
        })
        .collect();

    dedupe_dsl(&mut entries);
    entries.sort_by_key(|e| e.label.to_lowercase());
    entries
}

/// walk `dir` recursively, pushing every file path into `out`. unreadable
/// directories are skipped; symlinked directories are not followed (avoids
/// loops). errors are swallowed — a bad dir shouldn't crash startup.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(reader) = fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() && !file_type.is_symlink() {
            collect(&path, out);
        } else if file_type.is_file() {
            out.push(path);
        }
    }
}

/// drop a `.dsl.dz` when its uncompressed `.dsl` sibling is also present.
fn dedupe_dsl(entries: &mut Vec<DictEntry>) {
    let plain: std::collections::HashSet<PathBuf> = entries
        .iter()
        .filter(|e| e.format == Format::Dsl && ends_with_ci(&e.path, ".dsl"))
        .map(|e| e.path.clone())
        .collect();

    entries.retain(|e| {
        if e.format == Format::Dsl && ends_with_ci(&e.path, ".dsl.dz") {
            // keep only if there's no plain .dsl next to it. `is_dir` matters as much
            // as the set: one dictionary here ships as `X.dsl.dz` beside a *directory*
            // named `X.dsl/` holding the identical file, and a directory is never a
            // scanned entry — so without this the same 115k-headword lexicon loads
            // twice, costing ~190 MB and showing every greek word under two labels.
            let plain_sibling = strip_suffix_ci(&e.path, ".dz");
            !plain_sibling
                .map(|p| plain.contains(&p) || p.is_dir())
                .unwrap_or(false)
        } else {
            true
        }
    });
}

fn label_for(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| path.display().to_string())
}

fn ends_with_ci(path: &Path, suffix: &str) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|n| n.to_ascii_lowercase().ends_with(suffix))
        .unwrap_or(false)
}

fn strip_suffix_ci(path: &Path, suffix: &str) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let cut = name.len().checked_sub(suffix.len())?;
    // get(cut..) is None if cut isn't a char boundary, so we never slice
    // mid-codepoint; a successful get proves cut is valid for name[..cut] too.
    name.get(cut..)?
        .eq_ignore_ascii_case(suffix)
        .then(|| path.with_file_name(&name[..cut]))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `$XDG_CONFIG_HOME` if set and non-empty, else `~/.config` (per the XDG base
/// directory spec), else the current dir as a last resort.
fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// an array entry's leading decor — the whitespace and comments written before it.
fn prefix_of(value: &Value) -> String {
    value
        .decor()
        .prefix()
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_owned()
}

/// the indentation the file puts before an entry, taken from the first entry that
/// sits on its own line. four spaces when the file has nothing to say.
fn indent_of(items: &[Value]) -> String {
    items
        .iter()
        .map(prefix_of)
        .find(|prefix| prefix.contains('\n'))
        .and_then(|prefix| prefix.rsplit('\n').next().map(str::to_owned))
        .filter(|indent| indent.chars().all(char::is_whitespace))
        .unwrap_or_else(|| "    ".to_owned())
}

/// rewrite every same-line comment as a comment above the entry it is about, and
/// return the one that belongs to the key rather than to any entry.
///
/// this is the whole trick. toml_edit stores decor *before* a value, so a comment
/// written after entry X — `"/a", # why a` — is filed under X+1, and a comment on
/// the `dictionary_dirs = [` line is filed under the first entry. left that way,
/// removing an entry deletes a comment about the entry before it, and appending
/// one steals the comment that trailed the last. moving each comment above its own
/// entry first makes ownership match storage, and everything after this is a
/// straightforward keep-or-drop.
fn hoist_comments(items: &mut [Value], trailing: &mut String, indent: &str) -> Option<String> {
    // the last entry's same-line comment is written after the final comma, which
    // is the array's trailing text rather than any entry's decor.
    if let (Some(last), Some(comment)) = (items.len().checked_sub(1), take_comment(trailing)) {
        put_above(&mut items[last], &comment, indent);
    }
    for i in (1..items.len()).rev() {
        let mut prefix = prefix_of(&items[i]);
        if let Some(comment) = take_comment(&mut prefix) {
            items[i].decor_mut().set_prefix(prefix);
            put_above(&mut items[i - 1], &comment, indent);
        }
    }
    // what is left on the first entry's line was written beside the `[`.
    let first = items.first_mut()?;
    let mut prefix = prefix_of(first);
    let comment = take_comment(&mut prefix)?;
    first.decor_mut().set_prefix(prefix);
    Some(comment)
}

/// take the comment from `text`'s first line, if that is what the line holds,
/// leaving the rest of the text (and its newline) behind.
fn take_comment(text: &mut String) -> Option<String> {
    let head = text.split('\n').next().unwrap_or("");
    let comment = head.trim();
    if !comment.starts_with('#') {
        return None;
    }
    let comment = comment.to_owned();
    let rest = text[head.len()..].to_owned();
    *text = if rest.is_empty() {
        "\n".to_owned()
    } else {
        rest
    };
    Some(comment)
}

/// write `comment` on its own line directly above `value`.
fn put_above(value: &mut Value, comment: &str, indent: &str) {
    let prefix = prefix_of(value);
    let (prefix, indent) = match prefix.rsplit_once('\n') {
        // the entry is on its own line: keep its decor and its indentation.
        Some((_, own)) if own.chars().all(char::is_whitespace) => (prefix.clone(), own.to_owned()),
        // it is not, so it is about to be — give it the file's indentation.
        _ => (format!("\n{indent}"), indent.to_owned()),
    };
    value
        .decor_mut()
        .set_prefix(format!("{prefix}{comment}\n{indent}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_dsl_pairs_keeping_plain() {
        let mut entries = vec![
            DictEntry {
                path: "/d/x.dsl".into(),
                format: Format::Dsl,
                label: "x".into(),
            },
            DictEntry {
                path: "/d/x.dsl.dz".into(),
                format: Format::Dsl,
                label: "x".into(),
            },
            DictEntry {
                path: "/d/y.dsl.dz".into(),
                format: Format::Dsl,
                label: "y".into(),
            },
        ];
        dedupe_dsl(&mut entries);
        let paths: Vec<_> = entries.iter().map(|e| e.path.to_str().unwrap()).collect();
        // x.dsl.dz dropped (x.dsl present); y.dsl.dz kept (no plain sibling).
        assert!(paths.contains(&"/d/x.dsl"));
        assert!(!paths.contains(&"/d/x.dsl.dz"));
        assert!(paths.contains(&"/d/y.dsl.dz"));
    }

    #[test]
    fn default_config_points_at_sys_dict() {
        let config = Config::default();
        // with HOME set (it is in tests), we expect one dir ending in sys/dict.
        if std::env::var_os("HOME").is_some() {
            assert_eq!(config.dictionary_dirs.len(), 1);
            assert!(config.dictionary_dirs[0].ends_with("Dictionaries"));
        }
    }

    #[test]
    fn scan_honors_gitignore_style_excludes() {
        // scan classifies by extension without opening files, so empty .ifo
        // fixtures are enough to exercise include/exclude.
        let root = std::env::temp_dir().join(format!("dictu-scan-{}", std::process::id()));
        let (a, b) = (root.join("DictA"), root.join("DictB"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("a.ifo"), "").unwrap();
        fs::write(b.join("b.ifo"), "").unwrap();

        let root_s = root.to_string_lossy().into_owned();
        let all = scan(std::slice::from_ref(&root_s));
        assert_eq!(all.len(), 2, "both dicts found without excludes");

        let excl = format!("!{}", b.display());
        let filtered = scan(&[root_s, excl]);
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].path.starts_with(&a));

        fs::remove_dir_all(&root).ok();
    }
    /// the point of #38: a hand-written config says *why* a dictionary is
    /// excluded, and a save must not eat a word of it.
    const COMMENTED: &str = r#"# dictu configuration.
# paths are scanned recursively; a "!" prefix excludes.

dictionary_dirs = [
    "/home/u/Dropbox/sys/dict",
    # the same latin dictionary a second time (roadmap #19)
    "!/home/u/Dropbox/sys/dict/latin-dup",
    # eng>fr despite its title, so its lemmas tag wrong (roadmap #34)
    "!/home/u/Dropbox/sys/dict/larousse",
]

# not a key we own; it has to survive untouched.
theme = "dark"
"#;

    #[test]
    fn saving_keeps_every_comment_and_foreign_key() {
        let mut config: Config = toml::from_str(COMMENTED).unwrap();
        config
            .dictionary_dirs
            .push("!/home/u/Dropbox/sys/dict/new".into());
        let out = config.edited(Some(COMMENTED)).unwrap();

        for comment in [
            "# dictu configuration.",
            "# paths are scanned recursively",
            "# the same latin dictionary a second time (roadmap #19)",
            "# eng>fr despite its title, so its lemmas tag wrong (roadmap #34)",
            "# not a key we own; it has to survive untouched.",
        ] {
            assert!(out.contains(comment), "lost {comment:?}:\n{out}");
        }
        assert!(
            out.contains(r#"theme = "dark""#),
            "lost a foreign key:\n{out}"
        );
        // the new entry is there, on its own line like its neighbours.
        assert!(
            out.contains("\n    \"!/home/u/Dropbox/sys/dict/new\","),
            "the appended entry broke the layout:\n{out}"
        );
        // and the file still reads back as the config we asked to write.
        let back: Config = toml::from_str(&out).unwrap();
        assert_eq!(back.dictionary_dirs, config.dictionary_dirs);
    }

    #[test]
    fn a_dropped_entry_takes_its_own_reason_with_it() {
        let mut config: Config = toml::from_str(COMMENTED).unwrap();
        config.dictionary_dirs.retain(|d| !d.ends_with("larousse"));
        let out = config.edited(Some(COMMENTED)).unwrap();

        assert!(!out.contains("larousse"), "the entry stayed:\n{out}");
        assert!(
            !out.contains("roadmap #34"),
            "its reason outlived it:\n{out}"
        );
        assert!(
            out.contains("roadmap #19"),
            "took a neighbour's too:\n{out}"
        );
    }

    #[test]
    fn an_inline_array_is_not_reflowed() {
        let text = "dictionary_dirs = [\"/a\", \"/b\"]\n";
        let mut config: Config = toml::from_str(text).unwrap();
        config.dictionary_dirs.push("/c".into());
        let out = config.edited(Some(text)).unwrap();
        assert_eq!(out, "dictionary_dirs = [\"/a\", \"/b\", \"/c\"]\n");
    }

    #[test]
    fn a_fresh_config_reads_back_as_itself() {
        let config = Config {
            dictionary_dirs: vec!["/a".into(), "/b".into()],
        };
        let out = config.edited(None).unwrap();
        let back: Config = toml::from_str(&out).unwrap();
        assert_eq!(back.dictionary_dirs, config.dictionary_dirs);
    }
    /// toml_edit files a same-line comment under the *next* entry, so removing that
    /// next entry would delete a note about an entry that is staying.
    #[test]
    fn a_comment_after_an_entry_survives_its_neighbour_being_removed() {
        let text = "dictionary_dirs = [\n    \"/a\", # the good one\n    \"/b\",\n]\n";
        let config = Config {
            dictionary_dirs: vec!["/a".into()],
        };
        let out = config.edited(Some(text)).unwrap();

        assert!(
            out.contains("# the good one"),
            "lost the note about /a:\n{out}"
        );
        assert!(
            !out.contains("/b"),
            "kept the entry it was asked to drop:\n{out}"
        );
        let back: Config = toml::from_str(&out).unwrap();
        assert_eq!(back.dictionary_dirs, ["/a"]);
    }

    /// we don't know what a non-string entry is, so we don't delete it — the whole
    /// point of this function is to not lose what it doesn't understand.
    #[test]
    fn a_value_we_cannot_read_is_left_alone() {
        let text = "dictionary_dirs = [\"/a\", 42]\n";
        let config = Config {
            dictionary_dirs: vec!["/a".into()],
        };
        let out = config.edited(Some(text)).unwrap();
        assert!(
            out.contains("42"),
            "dropped a value it could not read:\n{out}"
        );
    }
    fn config(dirs: &[&str]) -> Config {
        Config {
            dictionary_dirs: dirs.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    /// appending must not walk the last entry's note down onto the new entry —
    /// that comment lives after the final comma, in the array's trailing text.
    #[test]
    fn appending_does_not_steal_the_last_entrys_note() {
        let text = "dictionary_dirs = [\n    \"/a\",\n    \"/b\", # why b is out\n]\n";
        let out = config(&["/a", "/b", "/c"]).edited(Some(text)).unwrap();

        let lines: Vec<&str> = out.lines().collect();
        let note = lines
            .iter()
            .position(|l| l.contains("why b is out"))
            .unwrap();
        assert!(
            lines[note + 1].contains("\"/b\""),
            "the note left /b:\n{out}"
        );
        assert!(
            !out.contains("\"/c\", # why b"),
            "the note followed the new entry:\n{out}"
        );
    }

    /// and a note about an entry that is going must not be left on one that stays:
    /// a wrong reason is worse than no reason.
    #[test]
    fn a_removed_entrys_reason_does_not_land_on_a_survivor() {
        let text = "dictionary_dirs = [\n    \"/a\", # about a\n    \"/b\", # about b\n    \"/c\", # about c\n]\n";
        let out = config(&["/a", "/c"]).edited(Some(text)).unwrap();

        assert!(
            out.contains("# about a") && out.contains("# about c"),
            "lost a survivor's note:\n{out}"
        );
        assert!(
            !out.contains("# about b"),
            "kept a dead entry's reason:\n{out}"
        );
        assert!(!out.contains("\"/b\""), "kept the entry itself:\n{out}");
    }

    /// a comment on the key's own line is about the key, and outlives any entry.
    #[test]
    fn the_comment_on_the_bracket_line_is_not_an_entrys_to_lose() {
        let text = "dictionary_dirs = [ # every path we scan\n    \"/a\",\n    \"/b\",\n]\n";
        let out = config(&["/b"]).edited(Some(text)).unwrap();

        assert!(
            out.contains("# every path we scan"),
            "lost the key's own comment:\n{out}"
        );
        assert!(!out.contains("\"/a\""), "{out}");
    }

    /// the file follows the list it is given — order included, which is what a
    /// reorderable dictionary list will need (#45).
    #[test]
    fn the_written_order_is_the_order_asked_for() {
        let text = "dictionary_dirs = [\n    \"/a\", # about a\n    \"/b\",\n]\n";
        let out = config(&["/b", "/a"]).edited(Some(text)).unwrap();
        let back: Config = toml::from_str(&out).unwrap();

        assert_eq!(
            back.dictionary_dirs,
            ["/b", "/a"],
            "wrote a different order:\n{out}"
        );
        assert!(
            out.contains("# about a"),
            "reordering dropped a comment:\n{out}"
        );
    }

    /// an entry the file holds twice collapses to the one the list asks for.
    #[test]
    fn a_duplicate_in_the_file_is_not_written_twice() {
        let text = "dictionary_dirs = [\"/a\", \"/a\"]\n";
        let out = config(&["/a"]).edited(Some(text)).unwrap();
        let back: Config = toml::from_str(&out).unwrap();
        assert_eq!(back.dictionary_dirs, ["/a"], "{out}");
    }

    /// the property the individual cases are examples of: whatever the file's
    /// comment placement, every surviving entry keeps its own note, every removed
    /// entry takes its note with it, and the result is the list we asked to write.
    #[test]
    fn comments_follow_their_own_entry_whatever_the_placement() {
        // a small deterministic prng: this has to be reproducible, and one bad
        // arrangement out of hundreds is exactly what the hand-written cases missed.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut roll = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };

        let names = ["/a", "/b", "/c", "/d", "/e"];
        for case in 0..600 {
            let count = 1 + roll(names.len() as u64) as usize;
            let multiline = roll(2) == 0 || count > 1;
            let header = roll(3) == 0 && multiline;

            let mut text = String::from("dictionary_dirs = [");
            if header {
                text.push_str(" # the paths we scan");
            }
            for (i, name) in names.iter().take(count).enumerate() {
                // 0: no comment, 1: above the entry, 2: after it on the same line
                let placement = if multiline { roll(3) } else { 0 };
                if multiline {
                    text.push('\n');
                    if placement == 1 {
                        text.push_str(&format!("    # about {name}\n"));
                    }
                    text.push_str("    ");
                } else if i > 0 {
                    text.push(' ');
                }
                text.push_str(&format!("\"{name}\""));
                if i + 1 < count || multiline {
                    text.push(',');
                }
                if placement == 2 {
                    text.push_str(&format!(" # about {name}"));
                }
            }
            if multiline {
                text.push('\n');
            }
            text.push_str("]\n");

            // keep a random subset, in a random rotation, plus sometimes a new one
            let mut wanted: Vec<String> = names
                .iter()
                .take(count)
                .filter(|_| roll(3) > 0)
                .map(|n| (*n).to_owned())
                .collect();
            if roll(3) == 0 {
                wanted.push("/new".to_owned());
            }
            if wanted.len() > 1 && roll(2) == 0 {
                wanted.rotate_left(1);
            }

            let config = Config {
                dictionary_dirs: wanted.clone(),
            };
            let out = config
                .edited(Some(&text))
                .unwrap_or_else(|e| panic!("case {case} errored: {e}\n{text}"));
            let back: Config = toml::from_str(&out).unwrap_or_else(|e| {
                panic!("case {case} wrote invalid toml: {e}\n{text}\n---\n{out}")
            });

            assert_eq!(
                back.dictionary_dirs, wanted,
                "case {case} wrote the wrong list\n{text}\n---\n{out}"
            );
            for name in names.iter().take(count) {
                let note = format!("# about {name}");
                let kept = wanted.iter().any(|w| w == name);
                // a note is only in the fixture when its entry drew placement 1 or 2
                if text.contains(&note) {
                    assert_eq!(
                        out.contains(&note),
                        kept,
                        "case {case}: note for {name} (kept={kept}) went the wrong way\n{text}\n---\n{out}"
                    );
                }
            }
            if header && !wanted.is_empty() {
                assert!(
                    out.contains("# the paths we scan"),
                    "case {case} lost the key's comment\n{text}\n---\n{out}"
                );
            }
        }
    }
}
