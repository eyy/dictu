//! app configuration: a `config.toml` sitting next to the executable, holding
//! the list of directories to scan (recursively) for dictionary files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::dict::{self, Format};

/// the on-disk config. `#[derive(Serialize, Deserialize)]` lets serde/toml
/// convert this struct to and from the toml text for us.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// write the config back to disk as toml, creating the directory if needed.
    fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// exclude a path (gitignore-style `!` prefix) and persist — a permanent
    /// "never load this again", the counterpart to the session-only search scope.
    // deliberately still unwired: the scope panel (roadmap #14) keeps its choice in
    // memory, because `save` rewrites config.toml through serde and would drop the
    // comments a hand-edited config has (the #19/#34 exclusions are commented).
    // permanent removal stays a hand edit until save preserves comments.
    #[allow(dead_code)]
    pub fn exclude(&mut self, path: &Path) -> Result<()> {
        let entry = format!("!{}", path.display());
        if !self.dictionary_dirs.iter().any(|e| e == &entry) {
            self.dictionary_dirs.push(entry);
        }
        self.save()
    }
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
            // only list formats we can actually open (keeps raw .bgl and the
            // not-yet-supported dsl/csv out of the picker).
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
            // keep only if there's no plain .dsl next to it.
            let plain_sibling = strip_suffix_ci(&e.path, ".dz");
            !plain_sibling.map(|p| plain.contains(&p)).unwrap_or(false)
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
}
