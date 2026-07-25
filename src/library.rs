//! the whole collection of loaded dictionaries, with unified search across all
//! of them. each dict is opened once (its index mapped or parsed, its `.dict`
//! mmapped — see `dict`), then a single case-insensitively-sorted index over
//! every headword gives fast prefix search without rescanning millions of words.
//!
//! that sort is the second half of startup's cost — 4.4 s over 1.47M headwords —
//! so it is persisted too, and mapped straight back when the collection hasn't
//! changed (`index_cache::Order`, roadmap #7).

use std::path::{Path, PathBuf};

use crate::config::DictEntry;
use crate::dict::{self, Dictionary};
use crate::index_cache::{self, Order};

/// one opened dictionary plus its display label.
struct Loaded {
    label: String,
    dict: Box<dyn Dictionary>,
}

/// a single search result: a headword and which dictionary it came from.
pub struct Hit {
    pub word: String,
    pub dict: usize,
}

pub struct Library {
    dicts: Vec<Loaded>,
    /// `(dict index, headword index)` for every headword across all dicts,
    /// sorted by the lowercased headword — the merged lemma index. holds only
    /// two u32s per entry (the words themselves stay in each dict), and on a warm
    /// start those u32s are read straight out of the mapped cache file.
    sorted: Order,
}

impl Library {
    /// open every supported entry, skipping (and logging) any that fail, then
    /// build the merged sorted index. slow for large collections the first time —
    /// run this on a worker thread (see the ui).
    pub fn open(entries: &[DictEntry]) -> Self {
        Self::open_with_cache(entries, Some(&crate::config::cache_dir()))
    }

    /// as `open`, but with the on-disk cache directory spelled out — `None` parses
    /// everything from scratch and writes nothing (what the tests want).
    pub fn open_with_cache(entries: &[DictEntry], cache: Option<&Path>) -> Self {
        let mut dicts = Vec::new();
        let mut sources = Vec::new();
        for entry in entries {
            match dict::open_any(&entry.path, cache) {
                Ok(dict) => {
                    dicts.push(Loaded {
                        label: entry.label.clone(),
                        dict,
                    });
                    sources.push(entry.path.clone());
                }
                Err(err) => eprintln!("dictu: skipping {}: {err:#}", entry.path.display()),
            }
        }
        Self::from_loaded(dicts, &sources, cache)
    }

    fn from_loaded(dicts: Vec<Loaded>, sources: &[PathBuf], cache: Option<&Path>) -> Self {
        let total: usize = dicts.iter().map(|d| d.dict.headwords().len()).sum();
        // the order is only this collection's while every dictionary is the file
        // it was, and still holds as many headwords as it did.
        let fingerprint = merged_fingerprint(&dicts, sources);
        let cached = cache
            .map(index_cache::order_path)
            .and_then(|path| Order::load(&path, &fingerprint))
            .filter(|order| order.count() == total);
        if let Some(sorted) = cached {
            return Self { dicts, sorted };
        }

        // one (dict, headword) pair per headword, sorted case-insensitively.
        // sort_by_cached_key lowercases each key once (transient), not per compare.
        let mut sorted: Vec<(u32, u32)> = dicts
            .iter()
            .enumerate()
            .flat_map(|(di, loaded)| {
                (0..loaded.dict.headwords().len()).map(move |hi| (di as u32, hi as u32))
            })
            .collect();
        sorted.sort_by_cached_key(|&(d, h)| word_at(&dicts, d, h).to_lowercase());

        // best effort — a cache we can't write costs the next launch time, not
        // correctness.
        if let Some(path) = cache.map(index_cache::order_path) {
            let image = Order::image(&sorted, &fingerprint);
            if let Err(err) = index_cache::store(&path, &image) {
                eprintln!("dictu: not caching the merged index: {err:#}");
            }
        }
        Self {
            dicts,
            sorted: Order::Owned(sorted),
        }
    }

    pub fn dict_count(&self) -> usize {
        self.dicts.len()
    }

    pub fn dict_label(&self, index: usize) -> Option<&str> {
        self.dicts.get(index).map(|d| d.label.as_str())
    }

    /// how many headwords one dictionary holds — shown per row in the scope panel,
    /// and summed to say how much a narrowed scope covers.
    pub fn dict_headwords(&self, index: usize) -> usize {
        self.dicts
            .get(index)
            .map_or(0, |loaded| loaded.dict.headwords().len())
    }

    pub fn total_headwords(&self) -> usize {
        self.sorted.count()
    }

    /// fast prefix search over the merged index: case-insensitive, returns up to
    /// `limit` hits in sorted order. O(log n) to locate + O(limit) to collect.
    /// `active` scopes the search — a hit is skipped when `active[dict]` is
    /// false (missing/short `active` = included, so `&[]` means "all dicts").
    pub fn prefix_search(&self, query: &str, limit: usize, active: &[bool]) -> Vec<Hit> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        // first index whose lowercased word is >= needle. hand-rolled rather than
        // slice::partition_point because the order may be a mapped file, not a slice.
        let (mut lo, mut hi) = (0, self.sorted.count());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let (d, h) = self.sorted.pair(mid);
            if self.lower_at(d, h) < needle {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        let mut hits = Vec::new();
        for i in lo..self.sorted.count() {
            let (d, h) = self.sorted.pair(i);
            if !self.lower_at(d, h).starts_with(&needle) {
                break; // sorted, so the prefix run has ended.
            }
            if !active.get(d as usize).copied().unwrap_or(true) {
                continue; // dict deselected in the scope panel.
            }
            hits.push(Hit {
                word: self.word(d, h).to_string(),
                dict: d as usize,
            });
            if hits.len() >= limit {
                break;
            }
        }
        hits
    }

    /// every dictionary that defines an exact headword, with all of its entries for
    /// that word — a headword can be filed under many (see `Dictionary::lookup`).
    /// dictionaries with nothing to say are left out.
    pub fn lookup_all(&self, word: &str) -> Vec<(usize, Vec<String>)> {
        self.dicts
            .iter()
            .enumerate()
            .map(|(index, loaded)| (index, loaded.dict.lookup(word)))
            .filter(|(_, entries)| !entries.is_empty())
            .collect()
    }

    fn word(&self, dict: u32, headword: u32) -> &str {
        word_at(&self.dicts, dict, headword)
    }

    fn lower_at(&self, dict: u32, headword: u32) -> String {
        self.word(dict, headword).to_lowercase()
    }
}

/// the headword a `(dict, headword)` pair names. bounds-checked rather than
/// indexed: on a warm start the pairs come out of a file, and a corrupt one must
/// search oddly rather than panic.
fn word_at(dicts: &[Loaded], dict: u32, headword: u32) -> &str {
    dicts
        .get(dict as usize)
        .and_then(|loaded| loaded.dict.headwords().get(headword as usize))
        .map(String::as_str)
        .unwrap_or_default()
}

/// what makes a cached merged order this collection's: every dictionary's label,
/// its source files' identity, and how many headwords it contributed — the pairs
/// index into those headword lists, so a changed count invalidates them.
fn merged_fingerprint(dicts: &[Loaded], sources: &[PathBuf]) -> String {
    dicts
        .iter()
        .zip(sources)
        .map(|(loaded, path)| {
            format!(
                "{}\u{1f}{}\u{1f}{}",
                loaded.label,
                loaded.dict.headwords().len(),
                index_cache::fingerprint(&dict::source_files(path)),
            )
        })
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mock {
        words: Vec<String>,
    }
    impl Dictionary for Mock {
        fn name(&self) -> &str {
            "mock"
        }
        fn headwords(&self) -> &[String] {
            &self.words
        }
        fn lookup(&self, headword: &str) -> Vec<String> {
            self.words
                .iter()
                .filter(|w| *w == headword)
                .map(|w| format!("<b>{w}</b> def"))
                .collect()
        }
    }

    /// two mock dicts, sorted in memory — `None` for the cache, so these tests
    /// never touch the filesystem (the cache itself is tested in `index_cache`
    /// and `dict::stardict`).
    fn lib() -> Library {
        Library::from_loaded(
            vec![
                Loaded {
                    label: "A".into(),
                    dict: Box::new(Mock {
                        words: vec!["Apple".into(), "apricot".into()],
                    }),
                },
                Loaded {
                    label: "B".into(),
                    dict: Box::new(Mock {
                        words: vec!["apple".into(), "grape".into()],
                    }),
                },
            ],
            &[],
            None,
        )
    }

    #[test]
    fn prefix_search_is_case_insensitive_and_sorted() {
        let hits = lib().prefix_search("ap", 10, &[]);
        // "Apple", "apple", "apricot" all prefix-match "ap" (case-insensitive);
        // "grape" does not (prefix, not substring). sorted order.
        let words: Vec<&str> = hits.iter().map(|h| h.word.as_str()).collect();
        assert!(words.contains(&"Apple") && words.contains(&"apple") && words.contains(&"apricot"));
        assert!(!words.contains(&"grape"));
    }

    #[test]
    fn prefix_search_respects_limit_and_empty() {
        assert!(lib().prefix_search("", 10, &[]).is_empty());
        assert_eq!(lib().prefix_search("ap", 2, &[]).len(), 2);
        assert!(lib().prefix_search("zzz", 10, &[]).is_empty());
    }

    #[test]
    fn prefix_search_honors_the_active_scope() {
        // dict A holds "Apple"/"apricot", dict B "apple"/"grape".
        let only_b = lib().prefix_search("ap", 10, &[false, true]);
        assert_eq!(only_b.len(), 1);
        assert_eq!(only_b[0].word, "apple");
        // a short mask leaves later dicts included, so this still sees dict B.
        assert_eq!(lib().prefix_search("ap", 10, &[false]).len(), 1);
        assert!(lib().prefix_search("ap", 10, &[false, false]).is_empty());
    }

    #[test]
    fn lookup_all_spans_dicts() {
        let defs = lib().lookup_all("apple");
        // "apple" exact in dict B; dict A has "Apple" (different case) -> lookup
        // is exact/case-sensitive, so only B matches here.
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].0, 1);
    }

    #[test]
    fn total_headwords_counts_all() {
        assert_eq!(lib().total_headwords(), 4);
    }

    #[test]
    fn per_dict_headwords_add_up_to_the_total() {
        let lib = lib();
        assert_eq!(lib.dict_headwords(0), 2);
        assert_eq!(lib.dict_headwords(1), 2);
        assert_eq!(lib.dict_headwords(2), 0, "no such dictionary");
    }

    /// a two-word StarDict on disk — the merged order can only be tested through
    /// real dictionaries, since it is keyed by their files.
    fn write_dict(dir: &Path, stem: &str, words: &[&str]) -> DictEntry {
        std::fs::create_dir_all(dir).unwrap();
        let mut idx = Vec::new();
        let mut data = Vec::new();
        for word in words {
            let definition = format!("<b>{word}</b>");
            idx.extend_from_slice(word.as_bytes());
            idx.push(0);
            idx.extend_from_slice(&(data.len() as u32).to_be_bytes());
            idx.extend_from_slice(&(definition.len() as u32).to_be_bytes());
            data.extend_from_slice(definition.as_bytes());
        }
        let ifo = dir.join(format!("{stem}.ifo"));
        std::fs::write(&ifo, "StarDict's dict ifo file\nsametypesequence=h\n").unwrap();
        std::fs::write(dir.join(format!("{stem}.idx")), &idx).unwrap();
        std::fs::write(dir.join(format!("{stem}.dict")), &data).unwrap();
        DictEntry {
            path: ifo,
            format: crate::dict::Format::StarDict,
            label: stem.to_string(),
        }
    }

    fn words_of(library: &Library, query: &str) -> Vec<String> {
        library
            .prefix_search(query, 10, &[])
            .into_iter()
            .map(|hit| hit.word)
            .collect()
    }

    #[test]
    fn the_merged_order_is_cached_and_invalidated() {
        use std::os::unix::fs::MetadataExt;

        let dir = std::env::temp_dir().join(format!("dictu-lib-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let cache = dir.join("cache");
        // two dictionaries, so the order really is merged across them.
        let a = write_dict(&dir.join("a"), "a", &["Apple", "apricot"]);
        let b = write_dict(&dir.join("b"), "b", &["apple", "grape"]);
        let entries = vec![a, b.clone()];

        let cold = Library::open_with_cache(&entries, Some(&cache));
        // case-insensitive across dicts, ties by dictionary then stored order.
        assert_eq!(words_of(&cold, "ap"), ["Apple", "apple", "apricot"]);
        assert_eq!(cold.total_headwords(), 4);
        let order_file = index_cache::order_path(&cache);
        let published = std::fs::metadata(&order_file).expect("an order file").ino();

        // second open: the very same answers, off the mapped file — a rebuild
        // would have published a new inode.
        let warm = Library::open_with_cache(&entries, Some(&cache));
        assert_eq!(words_of(&warm, "ap"), ["Apple", "apple", "apricot"]);
        assert_eq!(words_of(&warm, "gr"), ["grape"]);
        assert_eq!(warm.lookup_all("apple").len(), 1);
        assert_eq!(warm.total_headwords(), 4);
        assert_eq!(std::fs::metadata(&order_file).unwrap().ino(), published);

        // one dictionary gains a word: the order it was sorted into is now wrong,
        // so it has to be rebuilt rather than mapped.
        write_dict(&dir.join("b"), "b", &["apple", "grape", "azalea"]);
        let rebuilt = Library::open_with_cache(&entries, Some(&cache));
        assert_eq!(
            words_of(&rebuilt, "a"),
            ["Apple", "apple", "apricot", "azalea"]
        );
        assert_eq!(rebuilt.total_headwords(), 5);
        assert_ne!(std::fs::metadata(&order_file).unwrap().ino(), published);

        std::fs::remove_dir_all(&dir).ok();
    }
}
