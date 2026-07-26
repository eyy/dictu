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
    /// matching is by whole path components (`Path::starts_with`), **not** by
    /// string prefix: excluding `.../Foo` does not exclude `.../Foo.dsl.dz`, so
    /// name the file exactly when excluding one file out of a folder. paths
    /// aren't canonicalized either, so keep includes and excludes spelled the
    /// same way (no mixing a symlink with its target).
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

    /// this config applied to `existing`, as text: **entries this config has and
    /// the file doesn't are appended, and nothing else is touched.** comments, key
    /// order, foreign keys and layout survive because `toml_edit` never reprints
    /// what it wasn't asked to change. `None` means there is no file yet, so one
    /// is generated.
    ///
    /// appending is deliberately the only edit. removing an entry raises a question
    /// toml cannot answer — a note written after an entry on the same line is
    /// stored against the *next* one, so "delete this entry and its comment" needs
    /// a convention about which comment belongs to whom, and getting that
    /// convention wrong writes a *false* reason next to a dictionary that is still
    /// loaded. permanently removing a dictionary stays a hand edit (see #38).
    ///
    /// pure, and separate from `save`, so a test can round-trip a commented fixture
    /// without a real config to overwrite.
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

        let present: Vec<String> = array
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        // one entry per line if the file already does that, or if we are writing
        // the file ourselves and it will hold more than one.
        let lines = array.iter().any(|value| prefix_of(value).contains('\n'))
            || (existing.is_none() && self.dictionary_dirs.len() > 1);
        let indent = indent_of(array);

        for dir in &self.dictionary_dirs {
            if present.contains(dir) {
                continue;
            }
            array.push_formatted(
                Value::from(dir.as_str()).decorated(
                    if lines {
                        format!("\n{indent}")
                    } else if array.is_empty() {
                        String::new()
                    } else {
                        " ".to_owned()
                    }
                    .as_str(),
                    "",
                ),
            );
            if lines {
                array.set_trailing_comma(true);
                if !array.trailing().as_str().is_some_and(|t| t.contains('\n')) {
                    array.set_trailing("\n");
                }
            }
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
fn indent_of(array: &Array) -> String {
    array
        .iter()
        .map(prefix_of)
        .find(|prefix| prefix.contains('\n'))
        .and_then(|prefix| prefix.rsplit('\n').next().map(str::to_owned))
        .filter(|indent| !indent.is_empty() && indent.chars().all(char::is_whitespace))
        .unwrap_or_else(|| "    ".to_owned())
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
    fn an_inline_array_is_not_reflowed() {
        let text = "dictionary_dirs = [\"/a\", \"/b\"]\n";
        let mut config: Config = toml::from_str(text).unwrap();
        config.dictionary_dirs.push("/c".into());
        let out = config.edited(Some(text)).unwrap();
        assert_eq!(out, "dictionary_dirs = [\"/a\", \"/b\", \"/c\"]\n");
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

    /// the whole contract, over the shapes a hand-written file comes in: appending
    /// adds what is missing and leaves every other byte where it was.
    #[test]
    fn appending_leaves_the_rest_of_the_file_alone() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut roll = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };

        for case in 0..400 {
            let count = roll(4) as usize;
            let lines = roll(2) == 0 || count > 1;
            let comma = roll(4) > 0;
            let mut text = String::from("# a config someone wrote by hand.\ndictionary_dirs = [");
            for i in 0..count {
                let name = format!("/d{i}");
                if lines {
                    text.push('\n');
                    if roll(2) == 0 {
                        text.push_str(&format!("    # about {name}\n"));
                    }
                    text.push_str("    ");
                } else if i > 0 {
                    text.push(' ');
                }
                text.push_str(&format!("\"{name}\""));
                if i + 1 < count || (lines && comma) {
                    text.push(',');
                }
            }
            if lines {
                text.push('\n');
            }
            text.push_str("]\ntheme = \"dark\"\n");

            let mut dirs: Vec<String> = (0..count).map(|i| format!("/d{i}")).collect();
            dirs.push("/new".to_owned());
            let config = Config {
                dictionary_dirs: dirs.clone(),
            };
            let out = config
                .edited(Some(&text))
                .unwrap_or_else(|e| panic!("case {case}: {e}\n{text}"));
            let back: Config = toml::from_str(&out).unwrap_or_else(|e| {
                panic!("case {case} wrote invalid toml: {e}\n{text}\n---\n{out}")
            });

            assert_eq!(
                back.dictionary_dirs, dirs,
                "case {case}\n{text}\n---\n{out}"
            );
            for line in text.lines().filter(|l| l.contains('#')) {
                assert!(
                    out.contains(line.trim()),
                    "case {case} lost {line:?}\n{text}\n---\n{out}"
                );
            }
            assert!(
                out.contains("theme = \"dark\""),
                "case {case} lost a foreign key\n{out}"
            );
            assert_eq!(
                config.edited(Some(&out)).unwrap(),
                out,
                "case {case} is not idempotent\n{out}"
            );
        }
    }

    /// what the app cannot do, said out loud: dropping an entry from the list does
    /// not drop it from the file. removing a dictionary for good is a hand edit,
    /// because deciding which comment died with it is not toml's question to answer.
    #[test]
    fn removing_an_entry_is_not_something_saving_does() {
        let text = "dictionary_dirs = [\n    \"/a\",\n    \"/b\",\n]\n";
        let out = config(&["/a"]).edited(Some(text)).unwrap();
        assert_eq!(
            out, text,
            "a save rewrote the file it was meant to leave alone"
        );
    }
    /// the `!` rule matches whole path components, which is easy to get wrong when
    /// excluding one file out of a folder — `Foo` does not exclude `Foo.dsl.dz`.
    #[test]
    fn an_exclude_matches_whole_components_not_string_prefixes() {
        let root = std::env::temp_dir().join(format!("dictu-prefix-{}", std::process::id()));
        let dir = root.join("lexicon");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("greek_or_1.ifo"), "").unwrap();
        fs::write(dir.join("greek_or_2.ifo"), "").unwrap();

        let root_s = root.to_string_lossy().into_owned();
        let partial = format!("!{}", dir.join("greek_or_2").display());
        assert_eq!(
            scan(&[root_s.clone(), partial]).len(),
            2,
            "a partial file name excluded something"
        );
        let exact = format!("!{}", dir.join("greek_or_2.ifo").display());
        assert_eq!(
            scan(&[root_s, exact]).len(),
            1,
            "the exact file name did not"
        );

        fs::remove_dir_all(&root).ok();
    }
}
