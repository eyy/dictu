//! the whole collection of loaded dictionaries, with unified search across all
//! of them. each dict is opened once (its `.idx` parsed, its `.dict` mmapped —
//! see `dict`), then a single case-insensitively-sorted index over every
//! headword gives fast prefix search without rescanning millions of words.

use crate::config::DictEntry;
use crate::dict::{self, Dictionary};

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
    /// two u32s per entry (the words themselves stay in each dict).
    sorted: Vec<(u32, u32)>,
}

impl Library {
    /// open every supported entry, skipping (and logging) any that fail, then
    /// build the merged sorted index. slow for large collections — run this on
    /// a worker thread (see the ui).
    pub fn open(entries: &[DictEntry]) -> Self {
        let dicts = entries
            .iter()
            .filter_map(|entry| match dict::open_any(&entry.path) {
                Ok(dict) => Some(Loaded {
                    label: entry.label.clone(),
                    dict,
                }),
                Err(err) => {
                    eprintln!("dictu: skipping {}: {err:#}", entry.path.display());
                    None
                }
            })
            .collect();
        Self::from_loaded(dicts)
    }

    fn from_loaded(dicts: Vec<Loaded>) -> Self {
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
        Self { dicts, sorted }
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
        self.sorted.len()
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
        // first index whose lowercased word is >= needle.
        let start = self
            .sorted
            .partition_point(|&(d, h)| self.lower_at(d, h) < needle);

        let mut hits = Vec::new();
        for &(d, h) in &self.sorted[start..] {
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

fn word_at(dicts: &[Loaded], dict: u32, headword: u32) -> &str {
    &dicts[dict as usize].dict.headwords()[headword as usize]
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

    fn lib() -> Library {
        Library::from_loaded(vec![
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
        ])
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
}
