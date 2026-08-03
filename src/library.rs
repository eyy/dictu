//! the whole collection of loaded dictionaries, with unified search across all
//! of them. each dict is opened once (its index mapped or parsed, its `.dict`
//! mmapped — see `dict`), then a sorted index over every headword gives fast
//! prefix search without rescanning millions of words.
//!
//! the index is sorted by the **bare** key (`keys`) — case, canonical form,
//! greek final sigma and every combining mark settled — so an unpointed hebrew
//! query reaches the pointed headwords it should. a query that does spell out
//! its diacritics is honored by filtering the run it lands on; see
//! `search`.
//!
//! that sort is the second half of startup's cost — 4.4 s over 1.47M headwords —
//! so it is persisted too, and mapped straight back when the collection hasn't
//! changed (`index_cache::Order`, roadmap #7).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::DictEntry;
use crate::dict::{self, DictBytes, Dictionary};
use crate::index_cache::{self, Order};
use crate::keys::{self, KeyTable};

/// one opened dictionary plus its display label.
struct Loaded {
    label: String,
    /// what the file calls it, whatever the reader calls it — see `DictEntry`.
    derived: String,
    /// a short form for where the name does not fit (#52).
    short: Option<String>,
    /// where it was loaded from — the one name for a dictionary that does not
    /// change when the reader renames it (#52), which is what the config keys its
    /// preferences by and what a remembered scope is written against (#71).
    path: PathBuf,
    dict: Box<dyn Dictionary>,
}

/// one wordlist row: a lemma, however the dictionaries spell it. `כאב לב` and
/// `כְּאֵב לֵב` are one row, as are the oxia and tonos spellings of `λόγος` and
/// Gaffiot's `rex (1)` beside Lewis & Short's `rex`.
#[derive(Clone, Debug)]
pub struct Row {
    /// the spelling to show: the most fully marked in the group, because the
    /// pointed form is the headword a reader wants and the bare one is a search
    /// key that happens to be written down.
    pub word: String,
    /// every dictionary that has this lemma, with the spelling it files it under
    /// — which is how the definition pane finds the entries again without
    /// guessing at anyone's normalization.
    pub members: Vec<(usize, String)>,
}

impl Row {
    /// the dictionaries answering for this row, in library order, each once.
    pub fn dicts(&self) -> Vec<usize> {
        let mut dicts: Vec<usize> = self.members.iter().map(|(dict, _)| *dict).collect();
        dicts.dedup();
        dicts
    }
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
                        derived: entry.derived.clone(),
                        short: entry.short.clone(),
                        path: entry.path.clone(),
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

    /// the name to read a language off, which is the file's own rather than the
    /// reader's (#52). every caller asking "what language is this?" wants this one;
    /// every caller showing a name to a person wants `dict_label`.
    pub fn dict_derived(&self, index: usize) -> Option<&str> {
        self.dicts.get(index).map(|d| d.derived.as_str())
    }

    /// the shortest name a dictionary has: its short form where it has one, its
    /// name where it does not.
    pub fn dict_short(&self, index: usize) -> Option<&str> {
        self.dicts
            .get(index)
            .map(|d| d.short.as_deref().unwrap_or(&d.label))
    }

    pub fn dict_path(&self, index: usize) -> Option<&Path> {
        self.dicts.get(index).map(|d| d.path.as_path())
    }

    /// the opening scope, read off what the config remembered about each dictionary
    /// (#71): one flag per dictionary in library order. matched by path rather than
    /// by position, because the two lists differ by exactly the dictionaries that
    /// failed to open — and a positional match would then hand a remembered
    /// exclusion to the wrong dictionary.
    pub fn scope_from(&self, entries: &[DictEntry]) -> Vec<bool> {
        (0..self.dict_count())
            .map(|index| {
                self.dict_path(index)
                    .and_then(|path| entries.iter().find(|entry| entry.path == path))
                    .is_none_or(|entry| entry.scope)
            })
            .collect()
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

    /// fast prefix search over the merged index: O(log n) to locate the run +
    /// O(limit) to collect. `active` scopes the search — a hit is skipped when
    /// `active[dict]` is false (missing/short `active` = included, so `&[]` means
    /// "all dicts").
    ///
    /// **`limit` counts rows and is not an exact ceiling.** a key's worth of
    /// entries is grouped whole before the limit is consulted, so the return can
    /// exceed it by the other spellings in that last key — measured at most 11
    /// over this collection, for a limit of 500. stopping mid-key would split one
    /// lemma across two searches instead.
    ///
    /// the search is **asymmetric in how specific the query is**. it runs on the
    /// bare key, which ignores every diacritic on both sides, so `מלך` reaches
    /// `מֶלֶךְ` and `מָלָךְ` alike — the case that matters, since 74,174 pointed
    /// hebrew headwords have no unpointed spelling anywhere in the collection.
    /// a query that *does* spell out diacritics then filters that run: a
    /// headword must carry every mark the query typed (`keys::marks_allow`), so
    /// `מֶלֶךְ` no longer answers with `מָלָךְ` while `מֶלך`, pointed half way,
    /// still reaches `מֶלֶךְ`.
    pub fn search(&self, query: &str, limit: usize, active: &[bool]) -> Vec<Row> {
        self.search_where(query, limit, active, false)
    }

    /// as `search`, but `fold_forms` hides a row that only *repeats* a definition
    /// already on screen (#33).
    ///
    /// the rule is deliberately narrow, because the obvious one is wrong. a
    /// dictionary's `.syn` records look like an inflection table and are not always
    /// one: Whitaker's really is (1.18M forms of 37,777 words), but a hebrew-hebrew dictionary
    /// mixes plurals in with plene spellings, quoted forms and abbreviations, so
    /// hiding "aliases" as such loses `עגבנייה` — an everyday word whose only
    /// spelling in that dictionary is a `.syn` record. so a row goes only when
    /// **every entry it points at is already shown by a row above it**: 25 of the
    /// 26 forms of *rego* under `rex` repeat one definition and go, while a form
    /// searched on its own is the only way to that definition and stays.
    pub fn search_where(
        &self,
        query: &str,
        limit: usize,
        active: &[bool],
        fold_forms: bool,
    ) -> Vec<Row> {
        let fold = keys::fold(query.trim());
        let needle = keys::bare(&fold);
        // an empty needle is every word: a query of nothing, or of nothing but
        // combining marks, which bare down to nothing at all.
        if needle.is_empty() {
            return Vec::new();
        }
        // a query whose bare key is its fold key spelled no diacritics, so there
        // is nothing to hold candidates to. the homograph number comes off both
        // sides first, or `rex (1)` would read as a query about marks.
        let marked = *needle != *keys::without_homograph(&fold);
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

        let mut rows: Vec<Row> = Vec::new();
        // the definitions already on screen, so a row that only repeats one can be
        // told from a row that is the only way to reach it.
        let mut shown: HashSet<(usize, u64)> = HashSet::new();
        // every entry sharing a key is one lemma's worth of spellings, and they are
        // contiguous — so a run is collected whole and grouped, and the limit is
        // only consulted between runs. stopping mid-run would split a lemma across
        // two rows, or leave one of them missing a dictionary.
        // the third field is whether the dictionary files this spelling as a
        // pointer at one of its own entries, which is what makes a row a candidate
        // for folding away.
        let mut run: Vec<(usize, &str, bool)> = Vec::new();
        let mut run_key: &[u8] = &[];
        for i in lo..self.sorted.count() {
            let key = self.sorted.key(i);
            if !key.starts_with(needle) {
                break; // sorted, so the prefix run has ended.
            }
            if key != run_key {
                self.keep(&mut rows, &mut shown, group(&run), fold_forms);
                run.clear();
                if rows.len() >= limit {
                    return rows;
                }
                run_key = key;
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
            run.push((d as usize, word, self.is_alias(d, h)));
        }
        self.keep(&mut rows, &mut shown, group(&run), fold_forms);
        rows
    }

    /// append the rows of one key, dropping any that only repeat a definition
    /// `shown` already holds. only rows made entirely of aliases are candidates:
    /// a headword a dictionary files itself is never hidden, however much its
    /// entry looks like another's.
    fn keep(
        &self,
        rows: &mut Vec<Row>,
        shown: &mut HashSet<(usize, u64)>,
        grouped: Vec<(Row, bool)>,
        fold_forms: bool,
    ) {
        for (row, all_aliases) in grouped {
            if !fold_forms {
                rows.push(row);
                continue;
            }
            let ids = self.entry_ids(&row);
            if all_aliases && !ids.is_empty() && ids.iter().all(|id| shown.contains(id)) {
                continue; // every definition here is already on screen.
            }
            shown.extend(ids);
            rows.push(row);
        }
    }

    /// the definitions a row points at, as (dictionary, position) pairs.
    fn entry_ids(&self, row: &Row) -> Vec<(usize, u64)> {
        row.members
            .iter()
            .flat_map(|(dict, spelling)| {
                let ids = match self.dicts.get(*dict) {
                    Some(loaded) => loaded.dict.entry_ids(spelling),
                    None => Vec::new(),
                };
                ids.into_iter().map(move |id| (*dict, id))
            })
            .collect()
    }

    /// the row a spelling belongs to — what a link target needs, since the
    /// dictionary that has the word may spell it with marks the link does not
    /// (Bailly stores 97,717 greek keys with oxia and none with tonos, so an
    /// exact-match lookup of a word typed on a greek keyboard finds nothing).
    ///
    /// scoped like the wordlist, so a link cannot be headed by a spelling only a
    /// deselected dictionary files. only rows for *this* word are candidates: the
    /// walk matches a prefix, and answering `rexer` with `rexeram` would be
    /// serving a different word rather than the same one spelled differently.
    pub fn resolve(&self, word: &str, active: &[bool]) -> Option<Row> {
        let fold = keys::fold(word);
        let key = keys::bare(&fold).into_owned();
        let mut rows = self.search(word, 8, active);
        rows.retain(|row| *keys::bare(&keys::fold(&row.word)) == key);
        let found = rows
            .iter()
            .position(|row| {
                row.word == word || row.members.iter().any(|(_, spelling)| spelling == word)
            })
            .or_else(|| rows.iter().position(|row| keys::fold(&row.word) == fold));
        match found {
            Some(row) => Some(rows.swap_remove(row)),
            // the spelling is not in the collection under any of its own marks,
            // but another spelling of the same letters is.
            None => rows.into_iter().next(),
        }
    }

    /// one dictionary's entries for the exact headword it files them under. the
    /// spelling comes from a `Row`, so it is the dictionary's own and needs no
    /// normalizing.
    pub fn entries(&self, dict: usize, word: &str) -> Vec<String> {
        self.dicts
            .get(dict)
            .map(|loaded| loaded.dict.lookup(word))
            .unwrap_or_default()
    }

    fn is_alias(&self, dict: u32, headword: u32) -> bool {
        self.dicts
            .get(dict as usize)
            .is_some_and(|loaded| loaded.dict.is_alias(headword as usize))
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

/// group one key's worth of entries into rows.
///
/// every entry here shares a bare key, so they are spellings of the same letters
/// and differ only in marks (and in the homograph numbers `keys::bare` strips).
/// two questions decide the rows:
///
/// 1. **is it even a different spelling?** case, canonical form and greek
///    oxia-vs-tonos are settled by the fold key, so entries agreeing there are one
///    spelling by any reading — that alone merges the two `λόγος` rows.
/// 2. **does one spelling merely say less than another?** an unpointed spelling is
///    a less specific way of writing a pointed one, so it joins it: `כאב לב` into
///    `כְּאֵב לֵב`. but only when the pointed one is unambiguous — `מלך` could be
///    `מֶלֶךְ` or `מָלָךְ`, which are different words, so it stays a row of its own
///    rather than being filed under a guess.
fn group(run: &[(usize, &str, bool)]) -> Vec<(Row, bool)> {
    // one class per distinct fold key — minus the homograph number, which the
    // bare key already ignores and which is not a way of spelling anything.
    let mut folds: Vec<String> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut all_aliases: Vec<bool> = Vec::new();
    for &(dict, word, alias) in run {
        let fold = keys::without_homograph(&keys::fold(word)).to_owned();
        match folds.iter().position(|seen| *seen == fold) {
            Some(class) => {
                // a spelling with nothing hanging off it is the one to show:
                // `rex`, not the `rex (1)` that happened to sort first.
                if keys::without_homograph(word) == word
                    && keys::without_homograph(&rows[class].word) != rows[class].word
                {
                    rows[class].word = word.to_owned();
                }
                rows[class].members.push((dict, word.to_owned()));
                all_aliases[class] &= alias;
            }
            None => {
                folds.push(fold);
                all_aliases.push(alias);
                rows.push(Row {
                    word: word.to_owned(),
                    members: vec![(dict, word.to_owned())],
                });
            }
        }
    }

    // a class is maximal when no other spells more marks than it does. two
    // classes can allow each other — the same marks in a different order, or one
    // written twice — and then neither is maximal and the run simply does not
    // group, which is the safe way to be wrong (one occurrence in the collection:
    // a hebrew-hebrew dictionary's `חָזַר בִּתְשׁוּבָָה` with a doubled qamats).
    let maximal: Vec<bool> = folds
        .iter()
        .map(|fold| {
            !folds
                .iter()
                .any(|other| other != fold && keys::marks_allow(fold, other))
        })
        .collect();

    // fold each remaining class into the one spelling it can only be — where it
    // could be two, it keeps its own row.
    let mut absorbed = vec![false; rows.len()];
    for class in 0..rows.len() {
        if maximal[class] {
            continue;
        }
        let mut into = folds.iter().enumerate().filter(|(other, fold)| {
            maximal[*other]
                && keys::marks_allow(&folds[class], fold)
                // only where the marks are annotation the writer may leave off.
                // french `mur` is not an unpointed `mûr`, it is a wall.
                && keys::only_optional_marks(fold)
        });
        let (Some((host, _)), None) = (into.next(), into.next()) else {
            // ambiguous: `מלך` under both `מֶלֶךְ` and `מָלָךְ`. this row exists
            // *because* the spelling cannot be attributed to one lemma — so it is
            // also not one folding may drop as a repeat. deleting it files it
            // under a guess by omission: type `טוניקה`, the only spelling a modern
            // reader writes, and you would be left with `טוּנִיקָה` (a tunic) and
            // `טוֹנִיקָה` (a tonic) and no way to tell which you meant.
            all_aliases[class] = false;
            continue;
        };
        let members = std::mem::take(&mut rows[class].members);
        rows[host].members.extend(members);
        // a host that absorbs a spelling the dictionary files itself is no longer
        // made only of pointers.
        all_aliases[host] &= all_aliases[class];
        absorbed[class] = true;
    }

    let mut kept: Vec<(Row, bool)> = rows
        .into_iter()
        .zip(all_aliases)
        .zip(absorbed)
        .filter(|(_, absorbed)| !absorbed)
        .map(|((row, aliases), _)| (row, aliases))
        .collect();
    // one dictionary per row per spelling, in library order, so the count a row
    // shows and the sections the pane renders are in the same order.
    for (row, _) in &mut kept {
        row.members.sort_by_key(|(dict, _)| *dict);
        row.members.dedup();
    }
    kept
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
                // the name on disk, not the reader's: renaming a dictionary changes
                // nothing about the order this fingerprint guards, and charging a
                // rebuild of 1.9M keys for it would make #52 expensive to use.
                loaded.derived,
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
        /// where this dictionary's aliases begin, as a `.syn` would put them.
        aliases_from: usize,
        /// which entry each headword resolves to — several spellings sharing one is
        /// what a `.syn` record does, and what folding looks for.
        entry_of: Vec<u64>,
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
        fn is_alias(&self, index: usize) -> bool {
            index >= self.aliases_from
        }
        fn entry_ids(&self, headword: &str) -> Vec<u64> {
            self.words
                .iter()
                .position(|word| word == headword)
                .and_then(|index| self.entry_of.get(index).copied())
                .into_iter()
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
                    derived: "A".into(),
                    short: None,
                    path: format!("/mock/{}", "A").into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        aliases_from: usize::MAX,
                        entry_of: (0..64).collect(),
                        words: vec!["Apple".into(), "apricot".into()],
                    }),
                },
                Loaded {
                    label: "B".into(),
                    derived: "B".into(),
                    short: None,
                    path: format!("/mock/{}", "B").into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        aliases_from: usize::MAX,
                        entry_of: (0..64).collect(),
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
            derived: label.into(),
            short: None,
            path: format!("/mock/{}", label).into(),
            dict: Box::new(Mock {
                internal: internal.into(),
                words: words.iter().map(|w| (*w).to_string()).collect(),
                aliases_from: usize::MAX,
                entry_of: (0..64).collect(),
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
    fn search_is_case_insensitive_and_sorted() {
        let rows = lib().search("ap", 10, &[]);
        // "Apple"/"apple" prefix-match "ap" and are one row (case is not a
        // spelling); "apricot" is a second; "grape" is a prefix miss, not a
        // substring one.
        let words: Vec<&str> = rows.iter().map(|row| row.word.as_str()).collect();
        assert_eq!(words, ["Apple", "apricot"]);
    }

    #[test]
    fn search_respects_limit_and_empty() {
        assert!(lib().search("", 10, &[]).is_empty());
        // the limit counts rows: "Apple"/"apple" are one, "apricot" the second.
        assert_eq!(lib().search("ap", 2, &[]).len(), 2);
        assert!(lib().search("zzz", 10, &[]).is_empty());
        // a query of nothing but combining marks bares down to an empty key,
        // which must not read as "every word".
        assert!(lib().search("\u{5b0}", 10, &[]).is_empty());
    }

    /// one dictionary of real hebrew headwords, three ways of pointing the same
    /// three letters (a hebrew-hebrew dictionary and Klein file all of these).
    fn hebrew() -> Library {
        Library::from_loaded(
            vec![Loaded {
                label: "A".into(),
                derived: "A".into(),
                short: None,
                path: format!("/mock/{}", "A").into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    aliases_from: usize::MAX,
                    entry_of: (0..64).collect(),
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
                derived: "A".into(),
                short: None,
                path: format!("/mock/{}", "A").into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    aliases_from: usize::MAX,
                    entry_of: (0..64).collect(),
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

    /// roadmap #12: a row names every dictionary that has its word, so the limit
    /// may never cut a word in half. the case that matters is one *spelling* held
    /// by two dictionaries — that is the row whose count would be wrong — so this
    /// fixture files `logos` identically in both, which `lib()` does not.
    #[test]
    fn the_limit_never_truncates_a_word_mid_way() {
        let shared = Library::from_loaded(
            vec![
                Loaded {
                    label: "A".into(),
                    derived: "A".into(),
                    short: None,
                    path: format!("/mock/{}", "A").into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        aliases_from: usize::MAX,
                        entry_of: (0..64).collect(),
                        words: vec!["logos".into(), "logotype".into()],
                    }),
                },
                Loaded {
                    label: "B".into(),
                    derived: "B".into(),
                    short: None,
                    path: format!("/mock/{}", "B").into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        aliases_from: usize::MAX,
                        entry_of: (0..64).collect(),
                        words: vec!["logos".into()],
                    }),
                },
            ],
            &[],
            None,
        );

        // one row's worth of limit, and the row is the last thing the walk sees:
        // both dictionaries' entries for it still have to come back.
        let rows = shared.search("log", 1, &[]);
        assert_eq!(rows.len(), 1, "one row asked for, one row back");
        assert_eq!(rows[0].word, "logos");
        assert_eq!(
            rows[0].dicts(),
            [0, 1],
            "both dictionaries answer for the row"
        );
        // and the run it stopped in is complete, not spilled into the next word.
        assert!(rows.iter().all(|row| row.word != "logotype"));
    }

    /// the case-variant rule, which is the other half of #12 and not the same
    /// thing: `Apple` and `apple` share a key and a row, but a row is attributed
    /// by the spelling it shows, so the ui counts one dictionary for it, not two.
    #[test]
    fn a_key_run_can_hold_two_spellings() {
        let rows = lib().search("ap", 1, &[]);
        assert_eq!(rows.len(), 1);
        // one row, and it remembers that the two dictionaries spell it differently.
        assert_eq!(
            rows[0].members,
            [(0, "Apple".to_owned()), (1, "apple".to_owned())]
        );
    }

    #[test]
    fn search_honors_the_active_scope() {
        // dict A holds "Apple"/"apricot", dict B "apple"/"grape".
        let only_b = lib().search("ap", 10, &[false, true]);
        assert_eq!(only_b.len(), 1);
        assert_eq!(only_b[0].word, "apple");
        // a short mask leaves later dicts included, so this still sees dict B.
        assert_eq!(lib().search("ap", 10, &[false]).len(), 1);
        assert!(lib().search("ap", 10, &[false, false]).is_empty());
    }

    #[test]
    fn a_row_carries_the_spelling_each_dictionary_uses() {
        // "Apple" in dict A and "apple" in dict B are one row now, and each
        // dictionary is remembered with its own spelling — which is what lets the
        // pane ask for entries without any dictionary agreeing on case.
        let lib = lib();
        let row = lib.resolve("apple", &[]).expect("a row for apple");
        assert_eq!(
            row.members,
            [(0, "Apple".to_owned()), (1, "apple".to_owned())]
        );
        assert_eq!(row.dicts(), [0, 1]);
        assert!(!lib.entries(0, "Apple").is_empty());
        assert!(!lib.entries(1, "apple").is_empty());
        // and asking a dictionary for a spelling it does not file gets nothing,
        // which is why the row remembers rather than guesses.
        assert!(lib.entries(0, "apple").is_empty());
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
            derived: stem.to_string(),
            short: None,
            scope: true,
        }
    }

    fn words_of(library: &Library, query: &str) -> Vec<String> {
        library
            .search(query, 10, &[])
            .into_iter()
            .map(|row| row.word)
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
        // case-insensitive across dicts, and "Apple"/"apple" are one row.
        assert_eq!(words_of(&cold, "ap"), ["Apple", "apricot"]);
        assert_eq!(cold.total_headwords(), 4);
        let order_file = index_cache::order_path(&cache);
        let published = std::fs::metadata(&order_file).expect("an order file").ino();

        // second open: the very same answers, off the mapped file — a rebuild
        // would have published a new inode.
        let warm = Library::open_with_cache(&entries, Some(&cache));
        assert_eq!(words_of(&warm, "ap"), ["Apple", "apricot"]);
        assert_eq!(words_of(&warm, "gr"), ["grape"]);
        assert_eq!(
            warm.resolve("apple", &[]).map(|row| row.dicts()),
            Some(vec![0, 1])
        );
        assert_eq!(warm.total_headwords(), 4);
        assert_eq!(std::fs::metadata(&order_file).unwrap().ino(), published);

        // one dictionary gains a word: the order it was sorted into is now wrong,
        // so it has to be rebuilt rather than mapped.
        write_dict(&dir.join("b"), "b", &["apple", "grape", "azalea"]);
        let rebuilt = Library::open_with_cache(&entries, Some(&cache));
        assert_eq!(words_of(&rebuilt, "a"), ["Apple", "apricot", "azalea"]);
        assert_eq!(rebuilt.total_headwords(), 5);
        assert_ne!(std::fs::metadata(&order_file).unwrap().ino(), published);

        std::fs::remove_dir_all(&dir).ok();
    }
    /// roadmap #43. one lemma, one row — but only where the spellings really are
    /// one lemma.
    #[test]
    fn a_bare_spelling_joins_the_pointed_one_it_can_only_be() {
        // one pointed form and its bare spelling: the same word written twice, so
        // one row, showing the form a reader wants rather than the search key.
        let one = Library::from_loaded(
            vec![Loaded {
                label: "A".into(),
                derived: "A".into(),
                short: None,
                path: format!("/mock/{}", "A").into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    aliases_from: usize::MAX,
                    entry_of: (0..64).collect(),
                    words: vec!["כאב לב".into(), "כְּאֵב לֵב".into()],
                }),
            }],
            &[],
            None,
        );
        let rows = one.search("כאב", 10, &[]);
        assert_eq!(rows.len(), 1, "two spellings of one lemma made two rows");
        assert_eq!(rows[0].word, "כְּאֵב לֵב", "the bare spelling won the row");
        assert_eq!(rows[0].members.len(), 2, "both spellings are in the row");
    }

    #[test]
    fn a_bare_spelling_that_could_be_two_words_stays_its_own_row() {
        // `מלך` is compatible with both `מֶלֶךְ` and `מָלָךְ`, which are different
        // words. filing it under either would be a guess, so it stays a row.
        let rows = hebrew().search("מלך", 10, &[]);
        let words: Vec<&str> = rows.iter().map(|row| row.word.as_str()).collect();
        assert_eq!(words.len(), 3, "expected three rows, got {words:?}");
        assert!(words.contains(&"מלך") && words.contains(&"מֶלֶךְ") && words.contains(&"מָלָךְ"));
    }

    #[test]
    fn a_pointed_query_still_rules_out_the_word_it_contradicts() {
        // the #39 rule, unchanged by grouping: the marks a query spells out are
        // a claim, and `מֶלֶךְ` may not be answered with `מָלָךְ`.
        let rows = hebrew().search("מֶלֶךְ", 10, &[]);
        let words: Vec<&str> = rows.iter().map(|row| row.word.as_str()).collect();
        assert_eq!(words, ["מֶלֶךְ"], "got {words:?}");
    }

    #[test]
    fn a_homograph_number_is_not_a_different_word() {
        // Gaffiot files `rex (1)` and `Rex (2)`; Lewis & Short files `rex`. one
        // lemma, three spellings, and the number is the dictionary's bookkeeping.
        let latin = Library::from_loaded(
            vec![
                Loaded {
                    label: "Gaffiot".into(),
                    derived: "Gaffiot".into(),
                    short: None,
                    path: format!("/mock/{}", "Gaffiot").into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        aliases_from: usize::MAX,
                        entry_of: (0..64).collect(),
                        words: vec!["rex (1)".into(), "Rex (2)".into()],
                    }),
                },
                Loaded {
                    label: "L&S".into(),
                    derived: "L&S".into(),
                    short: None,
                    path: format!("/mock/{}", "L&S").into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        aliases_from: usize::MAX,
                        entry_of: (0..64).collect(),
                        words: vec!["rex".into()],
                    }),
                },
            ],
            &[],
            None,
        );
        let rows = latin.search("rex", 10, &[]);
        assert_eq!(
            rows.len(),
            1,
            "got {:?}",
            rows.iter().map(|r| &r.word).collect::<Vec<_>>()
        );
        assert_eq!(rows[0].dicts(), [0, 1]);
        // and each dictionary is remembered with the spelling it actually files,
        // numbers included, so the pane can still find the entries.
        let spellings: Vec<&str> = rows[0].members.iter().map(|(_, w)| w.as_str()).collect();
        assert_eq!(spellings, ["rex (1)", "Rex (2)", "rex"]);
    }

    #[test]
    fn a_link_resolves_to_the_row_however_it_is_spelled() {
        // Bailly stores greek with oxia and none with tonos, so a link written
        // with tonos has to find the oxia headword — the failure that made this
        // more than cosmetic.
        let greek = Library::from_loaded(
            vec![Loaded {
                label: "Bailly".into(),
                derived: "Bailly".into(),
                short: None,
                path: format!("/mock/{}", "Bailly").into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    aliases_from: usize::MAX,
                    entry_of: (0..64).collect(),
                    words: vec!["λ\u{1F79}γος".into()], // oxia
                }),
            }],
            &[],
            None,
        );
        let row = greek.resolve("λόγος", &[]).expect("tonos should find oxia");
        assert_eq!(row.members.len(), 1);
        assert_eq!(
            row.members[0].1, "λ\u{1F79}γος",
            "kept the spelling it is filed under"
        );
    }
    /// the review's sharpest finding: a mark is only "optional" where a reader may
    /// leave it off and still have written the same word. french accents are not.
    #[test]
    fn an_accent_that_is_part_of_the_spelling_keeps_its_own_row() {
        let french = Library::from_loaded(
            vec![Loaded {
                label: "Larousse".into(),
                derived: "Larousse".into(),
                short: None,
                path: format!("/mock/{}", "Larousse").into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    aliases_from: usize::MAX,
                    entry_of: (0..64).collect(),
                    words: vec!["mur".into(), "mûr".into()],
                }),
            }],
            &[],
            None,
        );
        let words: Vec<String> = french
            .search("mur", 10, &[])
            .into_iter()
            .map(|row| row.word)
            .collect();
        assert_eq!(words, ["mur", "mûr"], "a wall is not an unpointed ripe");
    }

    /// and greek accent position is lexical too — `εἰ` (if) is a mark-subset of
    /// `εἶ` (you are), and they are not the same word.
    #[test]
    fn a_greek_breathing_does_not_absorb_into_an_accent() {
        let greek = Library::from_loaded(
            vec![Loaded {
                label: "LSJ".into(),
                derived: "LSJ".into(),
                short: None,
                path: format!("/mock/{}", "LSJ").into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    aliases_from: usize::MAX,
                    entry_of: (0..64).collect(),
                    words: vec!["εἰ".into(), "εἶ".into()],
                }),
            }],
            &[],
            None,
        );
        assert_eq!(greek.search("ει", 10, &[]).len(), 2);
    }

    /// a link to a word the collection does not have must say so, rather than
    /// answering with the next word along the same prefix.
    #[test]
    fn resolving_a_word_that_is_not_there_finds_nothing() {
        let lib = lib();
        assert!(
            lib.resolve("appl", &[]).is_none(),
            "a prefix is not the word"
        );
        assert!(lib.resolve("zzz", &[]).is_none());
        assert!(lib.resolve("apple", &[]).is_some());
    }

    /// and it answers within the scope, so a link cannot be headed by a spelling
    /// only a deselected dictionary files.
    #[test]
    fn resolving_honours_the_scope() {
        let lib = lib();
        assert_eq!(
            lib.resolve("apple", &[false, true]).map(|row| row.dicts()),
            Some(vec![1])
        );
        assert!(lib.resolve("apple", &[false, false]).is_none());
    }
    /// roadmap #33: a row that only repeats a definition already on screen can be
    /// folded away — and a row that is the only way to reach one never is, which is
    /// the whole difference between this and hiding aliases as such.
    #[test]
    fn folding_drops_a_repeat_and_keeps_the_only_way_in() {
        let latin = Library::from_loaded(
            vec![Loaded {
                label: "Whitaker".into(),
                derived: "Whitaker".into(),
                short: None,
                path: format!("/mock/{}", "Whitaker").into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    words: vec![
                        "rego".into(),
                        "rexi".into(),
                        "rexit".into(),
                        "rexerint".into(),
                    ],
                    aliases_from: 2,
                    // the two forms answer with the lemma's entry, as a `.syn` does
                    entry_of: vec![0, 1, 0, 0],
                }),
            }],
            &[],
            None,
        );

        let all: Vec<String> = latin
            .search("rex", 10, &[])
            .into_iter()
            .map(|row| row.word)
            .collect();
        assert_eq!(all, ["rexerint", "rexi", "rexit"]);

        // folded: `rexi` is the dictionary's own word and stays; of the two forms
        // repeating `rego`'s entry, the first stays and the second goes.
        let folded: Vec<String> = latin
            .search_where("rex", 10, &[], true)
            .into_iter()
            .map(|row| row.word)
            .collect();
        assert_eq!(folded, ["rexerint", "rexi"]);

        // and a form searched on its own is the only row reaching that entry, so it
        // survives. this is the case hiding aliases got wrong, and it is why
        // `עגבנייה` — a word a hebrew-hebrew dictionary only files as a `.syn` spelling — stays.
        let alone: Vec<String> = latin
            .search_where("rexit", 10, &[], true)
            .into_iter()
            .map(|row| row.word)
            .collect();
        assert_eq!(alone, ["rexit"]);
    }

    /// a row is folded only when every one of its spellings is a pointer: a word one
    /// dictionary files itself is never hidden because another points at the text.
    #[test]
    fn a_headword_a_dictionary_files_itself_is_never_folded() {
        let mixed = Library::from_loaded(
            vec![
                Loaded {
                    label: "Forms".into(),
                    derived: "Forms".into(),
                    short: None,
                    path: format!("/mock/{}", "Forms").into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        words: vec!["rego".into(), "rex".into()],
                        aliases_from: 1, // "rex" here is only a pointer
                        entry_of: vec![0, 0],
                    }),
                },
                Loaded {
                    label: "L&S".into(),
                    derived: "L&S".into(),
                    short: None,
                    path: format!("/mock/{}", "L&S").into(),
                    dict: Box::new(Mock {
                        internal: "mock".into(),
                        words: vec!["rex".into()], // and here it is the headword
                        aliases_from: usize::MAX,
                        entry_of: vec![0],
                    }),
                },
            ],
            &[],
            None,
        );
        let rows = mixed.search_where("rex", 10, &[], true);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].dicts(),
            [0, 1],
            "both spellings stay: one is a word"
        );
    }
    /// the review's finding, and the case a "is the definition still reachable"
    /// test cannot see: `group` keeps an ambiguous unpointed spelling as its own
    /// row *because* it cannot be attributed to one lemma, so folding may not
    /// delete it either. type `טוניקה` and you must still get `טוניקה` — not a
    /// choice between `טוּנִיקָה` (a tunic) and `טוֹנִיקָה` (a tonic).
    #[test]
    fn an_ambiguous_spelling_is_never_folded_away() {
        let hebrew = Library::from_loaded(
            vec![Loaded {
                label: "a hebrew-hebrew dictionary".into(),
                derived: "a hebrew-hebrew dictionary".into(),
                short: None,
                path: format!("/mock/{}", "a hebrew-hebrew dictionary").into(),
                dict: Box::new(Mock {
                    internal: "mock".into(),
                    words: vec![
                        "טוּנִיקָה".into(),
                        "טוֹנִיקָה".into(),
                        "טוניקה".into(), // the alias, pointing at the first entry
                    ],
                    aliases_from: 2,
                    entry_of: vec![0, 1, 0],
                }),
            }],
            &[],
            None,
        );

        let folded: Vec<String> = hebrew
            .search_where("טוניקה", 10, &[], true)
            .into_iter()
            .map(|row| row.word)
            .collect();
        assert!(
            folded.iter().any(|word| word == "טוניקה"),
            "the typed spelling was folded away: {folded:?}"
        );
        assert_eq!(folded.len(), 3, "and the two words it could mean both stay");
    }
}
