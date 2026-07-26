//! the whole collection of loaded dictionaries, with unified search across all
//! of them. each dict is opened once (its index mapped or parsed, its `.dict`
//! mmapped — see `dict`), then a sorted index over every headword gives fast
//! prefix search without rescanning millions of words.
//!
//! the index is sorted by the **bare** key (`keys`) — case, canonical form,
//! greek final sigma and every combining mark settled — so an unpointed hebrew
//! query reaches the pointed headwords it should. a query that does spell out
//! its diacritics is honored by filtering the run it lands on; see
//! `prefix_search`.
//!
//! that sort is the second half of startup's cost — 4.4 s over 1.47M headwords —
//! so it is persisted too, and mapped straight back when the collection hasn't
//! changed (`index_cache::Order`, roadmap #7).

use std::path::{Path, PathBuf};

use crate::config::DictEntry;
use crate::dict::{self, DictBytes, Dictionary};
use crate::index_cache::{self, Order};
use crate::keys::{self, KeyTable};

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
    /// where each dictionary's headwords start in slot space — the lists laid
    /// end to end, which is what the merged order indexes. one longer than
    /// `dicts`, so the last entry is the total.
    base: Vec<u32>,
    /// every headword's slot, sorted by its bare key, with the keys themselves —
    /// the merged index. one u32 per headword plus its key (the words stay in
    /// each dict), read straight out of the mapped cache file on a warm start.
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
        let mut dicts = dicts;
        disambiguate_labels(&mut dicts, sources);
        let base = slot_bases(&dicts);
        let total = *base.last().unwrap_or(&0) as usize;
        // the order is only this collection's while every dictionary is the file
        // it was, and still holds as many headwords as it did.
        let fingerprint = merged_fingerprint(&dicts, sources);
        let sorted = cache
            .map(index_cache::order_path)
            .and_then(|path| Order::load(&path, &fingerprint))
            .filter(|order| order.count() == total)
            .unwrap_or_else(|| build_order(&dicts, &fingerprint, cache));
        Self {
            dicts,
            base,
            sorted,
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

    /// fast prefix search over the merged index: up to `limit` hits, O(log n) to
    /// locate each run + O(limit) to collect. `active` scopes the search — a hit
    /// is skipped when `active[dict]` is false (missing/short `active` =
    /// included, so `&[]` means "all dicts").
    ///
    /// the search is **asymmetric in how specific the query is**. it runs on the
    /// bare key, which ignores every diacritic on both sides, so `מלך` reaches
    /// `מֶלֶךְ` and `מָלָךְ` alike — the case that matters, since 74,174 pointed
    /// hebrew headwords have no unpointed spelling anywhere in the collection.
    /// a query that *does* spell out diacritics then filters that run: a
    /// headword must carry every mark the query typed (`keys::marks_allow`), so
    /// `מֶלֶךְ` no longer answers with `מָלָךְ` while `מֶלך`, pointed half way,
    /// still reaches `מֶלֶךְ`.
    pub fn prefix_search(&self, query: &str, limit: usize, active: &[bool]) -> Vec<Hit> {
        let fold = keys::fold(query.trim());
        let needle = keys::bare(&fold);
        // an empty needle is every word: a query of nothing, or of nothing but
        // combining marks, which bare down to nothing at all.
        if needle.is_empty() {
            return Vec::new();
        }
        // a query whose bare key is its fold key spelled no diacritics, so there
        // is nothing to hold candidates to.
        let marked = needle != fold;
        let needle = needle.as_bytes();

        // first position whose key is >= needle. hand-rolled rather than
        // slice::partition_point because the order is a mapped file, not a slice.
        let (mut lo, mut hi) = (0, self.sorted.count());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.sorted.key(mid) < needle {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        let mut hits = Vec::new();
        for i in lo..self.sorted.count() {
            if !self.sorted.key(i).starts_with(needle) {
                break; // sorted, so the prefix run has ended.
            }
            let Some((d, h)) = self.locate(self.sorted.slot(i)) else {
                continue; // a corrupt order names no dictionary; skip it.
            };
            if !active.get(d as usize).copied().unwrap_or(true) {
                continue; // dict deselected in the scope panel.
            }
            let word = self.word(d, h);
            if marked && !keys::marks_allow(&fold, word) {
                continue; // the headword contradicts a diacritic the query typed.
            }
            hits.push(Hit {
                word: word.to_string(),
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

    /// which dictionary and headword a slot names. the lists lie end to end, so
    /// it is the last base not past the slot; `None` for a slot past the end,
    /// which only a corrupt order produces.
    fn locate(&self, slot: u32) -> Option<(u32, u32)> {
        let dict = self.base.partition_point(|&b| b <= slot).checked_sub(1)?;
        // the last base is the total, so it names no dictionary.
        (dict + 1 < self.base.len()).then(|| (dict as u32, slot - self.base[dict]))
    }
}

/// where each dictionary's headwords begin once the lists are laid end to end,
/// with the total last — the slot numbering the merged order is written in.
fn slot_bases(dicts: &[Loaded]) -> Vec<u32> {
    std::iter::once(0)
        .chain(dicts.iter().scan(0u32, |at, loaded| {
            *at += loaded.dict.headwords().len() as u32;
            Some(*at)
        }))
        .collect()
}

/// the cold path: key every headword, sort by that key, and publish the two as
/// the cache image.
///
/// the keys are materialized once here rather than derived per comparison — the
/// sort this replaces called `to_lowercase()` inside the comparison, and
/// normalizing there would cost far more (a fold is several passes over the
/// string, and a binary search alone would do ~21 of them per keystroke).
fn build_order(dicts: &[Loaded], fingerprint: &str, cache: Option<&Path>) -> Order {
    let total: usize = dicts.iter().map(|d| d.dict.headwords().len()).sum();
    let mut bare = KeyTable::with_capacity(total);
    for word in dicts.iter().flat_map(|loaded| loaded.dict.headwords()) {
        bare.push(&keys::bare(&keys::fold(word)));
    }
    let image = Order::image(&bare.order(), &bare, fingerprint);

    // publish it and read the very same bytes back, so a cold start and a warm
    // one answer off an identical structure. a cache we can't write (or can't
    // map back, which two dictus racing on one directory could produce) costs
    // the next launch time, not correctness: the image we hold is still good.
    let published = cache
        .map(index_cache::order_path)
        .and_then(|path| match index_cache::store(&path, &image) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                eprintln!("dictu: not caching the merged index: {err:#}");
                None
            }
        })
        .and_then(|bytes| Order::open(bytes, fingerprint));
    published.unwrap_or_else(|| {
        Order::open(DictBytes::Owned(image), fingerprint)
            .expect("an order we just built must validate")
    })
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

/// make every display label unique.
///
/// a label is its dictionary's parent folder, which is usually the friendly thing
/// to show — but a folder can hold two dictionaries. Klein's lexicon sits beside
/// its own abbreviations list, so both arrive as "Comprehensive Etymological
/// Dictionary of the Hebrew Language by Ernest Klein (Heb-Eng)", and the scope
/// panel offers two rows nobody can tell apart. a colliding label falls back to
/// the name the dictionary calls itself (StarDict's `bookname`, DSL's `#NAME`),
/// and to the file stem when even those match.
///
/// labels that are already unique are left exactly as they are: `hack/check.sh`
/// greps them and the e2e fixtures are named for their folders.
fn disambiguate_labels(dicts: &mut [Loaded], sources: &[PathBuf]) {
    for _ in 0..2 {
        let colliding = repeated_labels(dicts);
        if colliding.is_empty() {
            return;
        }
        for (index, loaded) in dicts.iter_mut().enumerate() {
            if !colliding.contains(&loaded.label) {
                continue;
            }
            let internal = loaded.dict.name().trim().to_string();
            if !internal.is_empty() && internal != loaded.label {
                loaded.label = internal;
                continue;
            }
            // same folder, same internal name: the file is all that is left.
            if let Some(stem) = sources
                .get(index)
                .and_then(|path| path.file_stem())
                .and_then(|stem| stem.to_str())
            {
                loaded.label = format!("{} ({stem})", loaded.label);
            }
        }
    }
}

/// the labels held by more than one dictionary.
fn repeated_labels(dicts: &[Loaded]) -> std::collections::HashSet<String> {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for loaded in dicts {
        *counts.entry(loaded.label.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(label, _)| label.to_string())
        .collect()
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
        /// what the dictionary calls itself, as `bookname`/`#NAME` would.
        internal: String,
    }
    impl Dictionary for Mock {
        fn name(&self) -> &str {
            &self.internal
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
                        internal: "mock".into(),
                        words: vec!["Apple".into(), "apricot".into()],
                    }),
                },
                Loaded {
                    label: "B".into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        words: vec!["apple".into(), "grape".into()],
                    }),
                },
            ],
            &[],
            None,
        )
    }

    fn labelled(label: &str, internal: &str, words: &[&str]) -> Loaded {
        Loaded {
            label: label.into(),
            dict: Box::new(Mock {
                internal: internal.into(),
                words: words.iter().map(|w| (*w).to_string()).collect(),
            }),
        }
    }

    #[test]
    fn colliding_labels_fall_back_to_what_the_dictionary_calls_itself() {
        // the real case: Klein's lexicon and Klein's abbreviations list share a
        // folder, so both arrive with the folder's name.
        let folder = "Comprehensive Etymological Dictionary … Ernest Klein (Heb-Eng)";
        let mut dicts = vec![
            labelled(folder, "Comprehensive Etymological (Heb-Eng)", &["a"]),
            labelled(folder, "Etymological ABBRV (Heb-Eng)", &["b"]),
            labelled("Larousse Chambers français-anglais", "Larousse", &["c"]),
        ];
        let sources = vec![
            PathBuf::from("/d/klein.dsl"),
            PathBuf::from("/d/abbrv.dsl"),
            PathBuf::from("/e/larousse.dsl"),
        ];
        disambiguate_labels(&mut dicts, &sources);

        assert_eq!(dicts[0].label, "Comprehensive Etymological (Heb-Eng)");
        assert_eq!(dicts[1].label, "Etymological ABBRV (Heb-Eng)");
        // a label that was already unique is left alone, internal name or not.
        assert_eq!(dicts[2].label, "Larousse Chambers français-anglais");
    }

    #[test]
    fn labels_that_collide_even_internally_fall_back_to_the_file() {
        let mut dicts = vec![
            labelled("same", "same", &["a"]),
            labelled("same", "same", &["b"]),
        ];
        let sources = vec![PathBuf::from("/d/one.ifo"), PathBuf::from("/d/two.ifo")];
        disambiguate_labels(&mut dicts, &sources);
        assert_eq!(dicts[0].label, "same (one)");
        assert_eq!(dicts[1].label, "same (two)");
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
        // a query of nothing but combining marks bares down to an empty key,
        // which must not read as "every word".
        assert!(lib().prefix_search("\u{5b0}", 10, &[]).is_empty());
    }

    /// one dictionary of real hebrew headwords, three ways of pointing the same
    /// three letters (a hebrew-hebrew dictionary and Klein file all of these).
    fn hebrew() -> Library {
        Library::from_loaded(
            vec![Loaded {
                label: "A".into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    words: vec!["מֶלֶךְ".into(), "מָלָךְ".into(), "מלך".into()],
                }),
            }],
            &[],
            None,
        )
    }

    /// unpointed: the permissive direction, and the one that matters — 74,174
    /// pointed headwords have no unpointed spelling to be found by.
    #[test]
    fn an_unpointed_query_finds_every_pointing() {
        assert_eq!(words_of(&hebrew(), "מלך").len(), 3);
    }

    /// pointed: the query says how specific it is, and a headword that
    /// contradicts it is not an answer.
    #[test]
    fn a_pointed_query_excludes_a_different_pointing() {
        assert_eq!(words_of(&hebrew(), "מֶלֶךְ"), ["מֶלֶךְ"]);
        assert_eq!(words_of(&hebrew(), "מָלָךְ"), ["מָלָךְ"]);
    }

    /// half-pointed: subset, not equality, or typing one vowel would find
    /// nothing at all.
    #[test]
    fn a_partly_pointed_query_still_reaches_the_full_pointing() {
        assert_eq!(words_of(&hebrew(), "מֶלך"), ["מֶלֶךְ"]);
    }

    /// the same rule in greek, where the accent rather than the vowel carries it.
    #[test]
    fn an_accented_greek_query_excludes_the_other_accent() {
        let library = Library::from_loaded(
            vec![Loaded {
                label: "A".into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    words: vec!["λόγος".into(), "λὸγος".into(), "λογος".into()],
                }),
            }],
            &[],
            None,
        );
        assert_eq!(words_of(&library, "λογος").len(), 3);
        assert_eq!(words_of(&library, "λόγος"), ["λόγος"]);
        // and a headword is never listed twice for matching in several ways.
        assert_eq!(words_of(&library, "λόγοσ"), ["λόγος"]);
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

    /// the phase A gate, mechanized: a greek word typed with tonos finds the
    /// oxia the dictionary stores, an unpointed hebrew query finds the pointed
    /// headword, final sigma matches either way round — and all of it still
    /// holds on the *second* open, off the cache file. that last clause is the
    /// one that catches a merged order left sorted by a key nothing searches
    /// with any more (see `index_cache::VERSION`).
    #[test]
    fn normalized_keys_hold_across_a_warm_cache() {
        use std::os::unix::fs::MetadataExt;

        let dir = std::env::temp_dir().join(format!("dictu-keys-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let cache = dir.join("cache");
        let entries = vec![write_dict(
            &dir.join("d"),
            "d",
            &[
                "λ\u{1f79}γος", // as Dodson files it: U+1F79 oxia.
                "מֶ֫לֶךְ",          // as a hebrew-hebrew dictionary files it: with niqqud.
                "מָלָךְ",          // the same letters, pointed differently.
                "Ἀγαθός",       // capital, accented, final sigma.
            ],
        )];
        let order_file = index_cache::order_path(&cache);

        let mut published = None;
        for run in ["cold", "warm"] {
            let library = Library::open_with_cache(&entries, Some(&cache));
            let inode = std::fs::metadata(&order_file).expect("an order file").ino();

            // typed with tonos, stored with oxia.
            assert_eq!(
                words_of(&library, "λ\u{3cc}γος"),
                ["λ\u{1f79}γος"],
                "{run}: tonos must find oxia"
            );
            // typed unpointed, stored pointed — every pointing answers.
            assert_eq!(
                words_of(&library, "מלך").len(),
                2,
                "{run}: unpointed hebrew must find both pointings"
            );
            // typed pointed: the other pointing is not an answer.
            assert_eq!(
                words_of(&library, "מֶ֫לֶךְ"),
                ["מֶ֫לֶךְ"],
                "{run}: a pointed query must exclude a different pointing"
            );
            // typed half-pointed: still reaches the fully pointed headword.
            assert_eq!(
                words_of(&library, "מֶלך"),
                ["מֶ֫לֶךְ"],
                "{run}: partial niqqud must still reach the full spelling"
            );
            // final sigma either way round, and case-insensitively.
            assert_eq!(
                words_of(&library, "αγαθος"),
                ["Ἀγαθός"],
                "{run}: bare, lowercase, final sigma"
            );
            assert_eq!(
                words_of(&library, "αγαθοσ"),
                ["Ἀγαθός"],
                "{run}: bare, lowercase, non-final sigma"
            );
            assert_eq!(
                words_of(&library, "Ἀγαθοσ"),
                ["Ἀγαθός"],
                "{run}: accented, non-final sigma"
            );

            // the second run must answer off the mapped file, not a rebuild —
            // a rebuild would have published a new inode.
            match published {
                None => published = Some(inode),
                Some(cold) => assert_eq!(
                    inode, cold,
                    "the warm run rebuilt the cache instead of reading it"
                ),
            }
        }

        std::fs::remove_dir_all(&dir).ok();
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
