//! the collection as the app sees it: every loaded dictionary, plus the choices
//! a reader has made about them — which are in scope, and whether rows that only
//! repeat a definition are folded away.
//!
//! this is the whole api. the window and the command line are two front ends over
//! it and neither knows anything the other doesn't; the test of a change belonging
//! here rather than in the ui is whether you could ask it from a terminal.
//! nothing in this file knows that gtk exists.

use crate::library::{Library, Row};

/// how many rows a search hands back before it stops. the wordlist builds one
/// widget per row, which is the real ceiling; the cli uses the same number so a
/// count measured there is the count the window would show.
pub const ROW_LIMIT: usize = 500;

pub struct Collection {
    /// `None` until the worker thread finishes indexing.
    library: Option<Library>,
    /// one flag per dictionary, in library order. empty means "everything" — the
    /// state before the scope panel exists, which is also what `Library` reads an
    /// empty mask as.
    scope: Vec<bool>,
    /// dictionary indices in the reader's order (#45): what the lists show, and the
    /// order a word's definitions come back in. a permutation over the library, not
    /// the library's own numbering — see `Library::order_from`.
    order: Vec<usize>,
    fold_forms: bool,
}

/// one dictionary's answer for a word: what to call it, and every entry it files.
pub struct Definition {
    pub dict: usize,
    /// what to show as the heading — the reader's name for it (#52).
    pub label: String,
    /// what to read its languages off — the file's own name, which is where the
    /// pairs are written (#60).
    pub derived: String,
    pub entries: Vec<String>,
}

impl Collection {
    pub fn empty() -> Self {
        Self {
            library: None,
            scope: Vec::new(),
            order: Vec::new(),
            fold_forms: false,
        }
    }

    /// hand over the built index, and what the reader last chose to search (#71).
    /// a flag per dictionary in library order; anything short is padded with `true`,
    /// since a dictionary nobody has an opinion about is one nobody has excluded.
    pub fn open(&mut self, library: Library, scope: Vec<bool>, order: Vec<usize>) {
        let mut scope = scope;
        scope.resize(library.dict_count(), true);
        self.scope = scope;
        self.set_order(order);
        self.library = Some(library);
    }

    /// the reading order, as dictionary indices. anything missing from it is appended
    /// in library order, so an order that has gone stale — a dictionary added since
    /// it was written — still names every dictionary exactly once.
    pub fn set_order(&mut self, order: Vec<usize>) {
        // sized by the largest index it was handed, not by the count: this is public
        // and takes whatever it is given, and indexing `seen` by a number past the end
        // of the collection would take the app down rather than ignore a bad order.
        let top = order.iter().copied().max().map_or(0, |index| index + 1);
        let mut seen = vec![false; self.dict_count().max(top)];
        self.order = order
            .into_iter()
            .filter(|&index| !std::mem::replace(&mut seen[index], true))
            .collect();
        self.order
            .extend((0..self.dict_count()).filter(|&index| !seen[index]));
    }

    /// every dictionary, in the reader's order — what the panel and the shelf list.
    pub fn dicts_in_order(&self) -> &[usize] {
        &self.order
    }

    /// where a dictionary comes in that order. unranked ones sort last, which only
    /// happens between `open` and the order being worked out.
    pub fn rank(&self, dict: usize) -> usize {
        self.order
            .iter()
            .position(|&index| index == dict)
            .unwrap_or(usize::MAX)
    }

    pub fn is_ready(&self) -> bool {
        self.library.is_some()
    }

    fn library(&self) -> Option<&Library> {
        self.library.as_ref()
    }

    pub fn dict_count(&self) -> usize {
        self.library().map_or(0, Library::dict_count)
    }

    pub fn dict_label(&self, index: usize) -> &str {
        self.library()
            .and_then(|library| library.dict_label(index))
            .unwrap_or("")
    }

    /// where a dictionary was loaded from — its identity in the config, which is
    /// what a remembered scope is written against.
    pub fn dict_path(&self, index: usize) -> Option<&std::path::Path> {
        self.library().and_then(|library| library.dict_path(index))
    }

    /// the file's own name for a dictionary, which is where a language is read from
    /// rather than the name the reader gave it (#52).
    pub fn dict_derived(&self, index: usize) -> &str {
        self.library()
            .and_then(|library| library.dict_derived(index))
            .unwrap_or("")
    }

    /// the shortest name a dictionary has — what the wordlist tag falls back to when
    /// no language can be named for a row.
    pub fn dict_short(&self, index: usize) -> &str {
        self.library()
            .and_then(|library| library.dict_short(index))
            .unwrap_or("")
    }

    pub fn dict_headwords(&self, index: usize) -> usize {
        self.library()
            .map_or(0, |library| library.dict_headwords(index))
    }

    pub fn total_headwords(&self) -> usize {
        self.library().map_or(0, Library::total_headwords)
    }

    pub fn is_dict_active(&self, index: usize) -> bool {
        self.scope.get(index).copied().unwrap_or(true)
    }

    pub fn set_dict_active(&mut self, index: usize, active: bool) {
        if let Some(flag) = self.scope.get_mut(index) {
            *flag = active;
        }
    }

    /// true when there are dictionaries and the reader has turned all of them off
    /// — a state of its own, not an empty result.
    pub fn nothing_selected(&self) -> bool {
        self.dict_count() > 0 && !self.scope.is_empty() && !self.scope.iter().any(|on| *on)
    }

    /// what the scope covers: how many dictionaries, and how many headwords they
    /// hold between them.
    pub fn scope_size(&self) -> (usize, usize) {
        if self.scope.is_empty() {
            return (self.dict_count(), self.total_headwords());
        }
        self.scope
            .iter()
            .enumerate()
            .filter(|(_, active)| **active)
            .fold((0, 0), |(dicts, words), (index, _)| {
                (dicts + 1, words + self.dict_headwords(index))
            })
    }

    pub fn set_fold_forms(&mut self, fold: bool) {
        self.fold_forms = fold;
    }

    /// the rows for a query, under the current scope and fold setting.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Row> {
        self.library().map_or_else(Vec::new, |library| {
            library.search_where(query, limit, &self.scope, self.fold_forms)
        })
    }

    /// a page of rows, and whether there are more: asked for one over the limit, so
    /// that "more" means more rather than "exactly as many as you asked for". the
    /// search overshoots a limit by design (it never cuts a key-run in half), so the
    /// extra row is a probe, not a guarantee — anything past the limit is dropped.
    pub fn page(&self, query: &str, limit: usize) -> (Vec<Row>, bool) {
        let mut rows = self.search(query, limit + 1);
        let more = rows.len() > limit;
        rows.truncate(limit);
        (rows, more)
    }

    /// the row a spelling belongs to — for a link target or a word arriving from
    /// outside, neither of which came from a row to begin with.
    pub fn resolve(&self, word: &str) -> Option<Row> {
        self.library()
            .and_then(|library| library.resolve(word, &self.scope))
    }

    /// what to show for a row: each in-scope dictionary that has it, asked for the
    /// spelling *it* files, in library order. a dictionary filing two spellings of
    /// one row against the same entry answers once — a hebrew-hebrew dictionary does that 28,494
    /// times, and printing it twice would claim two senses.
    pub fn definitions(&self, row: &Row) -> Vec<Definition> {
        let Some(library) = self.library() else {
            return Vec::new();
        };
        let mut found: Vec<Definition> = Vec::new();
        for (dict, spelling) in &row.members {
            if !self.is_dict_active(*dict) {
                continue;
            }
            let entries = library.entries(*dict, spelling);
            if entries.is_empty() {
                continue;
            }
            match found.last_mut() {
                Some(last) if last.dict == *dict => {
                    for entry in entries {
                        if !last.entries.contains(&entry) {
                            last.entries.push(entry);
                        }
                    }
                }
                _ => found.push(Definition {
                    dict: *dict,
                    label: self.dict_label(*dict).to_owned(),
                    derived: self.dict_derived(*dict).to_owned(),
                    entries,
                }),
            }
        }
        // and in the reader's order (#45). sorted at the end rather than by walking
        // the order outside and the members inside: the merge above depends on equal
        // dictionaries being adjacent, which is how `members` comes, and a stable sort
        // keeps that while moving whole sections.
        found.sort_by_key(|definition| self.rank(definition.dict));
        found
    }

    /// the line under the wordlist: how many rows, and every way the answer was
    /// narrowed on the way to them. both front ends print this, so they cannot
    /// drift into describing the same search differently.
    pub fn status(&self, rows: usize, truncated: bool) -> String {
        let counted = match truncated {
            true => format!("{}+ results", thousands(rows)),
            false => quantity(rows, "result", "results"),
        };
        self.noting_narrowing(&counted)
    }

    /// the same line for an empty query, where the answer is the collection
    /// itself: how many words, in how many dictionaries — and *of* how many, when
    /// the reader has narrowed it.
    ///
    /// deliberately without the fold note: folding drops rows from a result, never
    /// headwords from the library, so this is the one count it cannot change.
    pub fn library_size(&self) -> String {
        let (dicts, words) = self.scope_size();
        let total = self.dict_count();
        let named = match dicts < total {
            true => format!(
                "{dicts} of {}",
                quantity(total, "dictionary", "dictionaries")
            ),
            false => quantity(dicts, "dictionary", "dictionaries"),
        };
        format!("{} · {named}", quantity(words, "word", "words"))
    }

    /// every way the answer was narrowed, appended in the order the reader chose
    /// them: the scope first, then the fold.
    fn noting_narrowing(&self, line: &str) -> String {
        let (dicts, total) = (self.scope_size().0, self.dict_count());
        let scoped = match dicts < total {
            true => format!(
                "{line} · {dicts} of {}",
                quantity(total, "dictionary", "dictionaries")
            ),
            false => line.to_owned(),
        };
        match self.fold_forms {
            true => format!("{scoped} · forms folded"),
            false => scoped,
        }
    }
}

/// `n` with thousands separators: 4009914 -> "4,009,914". `rchunks` groups from
/// the right, which is where digit grouping starts.
pub fn thousands(n: usize) -> String {
    n.to_string()
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|group| String::from_utf8_lossy(group).into_owned())
        .collect::<Vec<_>>()
        .join(",")
}

/// `n` with its noun, pluralized and separated: `1 result`, `13 words`.
pub fn quantity(n: usize, singular: &str, plural: &str) -> String {
    let noun = match n {
        1 => singular,
        _ => plural,
    };
    format!("{} {noun}", thousands(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #45: the order names every dictionary exactly once, whatever it is handed.
    /// it comes from a config file, so it can be stale in every direction — written
    /// when there were three dictionaries, or five, or with a line duplicated by a
    /// hand edit — and a list that named one twice would show it twice.
    #[test]
    fn a_stale_order_still_names_every_dictionary_once() {
        let mut collection = Collection::empty();
        // no library: dict_count is 0, so only what it is given survives.
        collection.set_order(vec![2, 0, 2, 1]);
        assert_eq!(collection.dicts_in_order(), [2, 0, 1], "a repeat was kept");
        // and an index past the end of the collection is survivable, which it was
        // not: `seen` used to be sized by the count and this indexed past it.
        collection.set_order(vec![9, 0]);
        assert_eq!(collection.dicts_in_order(), [9, 0]);

        collection.set_order(Vec::new());
        assert!(collection.dicts_in_order().is_empty());
    }

    /// and `rank` is the inverse of it, which is what sorts a word's definitions.
    #[test]
    fn rank_is_where_a_dictionary_comes_in_the_order() {
        let mut collection = Collection::empty();
        collection.set_order(vec![3, 1, 0]);
        assert_eq!(collection.rank(3), 0);
        assert_eq!(collection.rank(1), 1);
        assert_eq!(collection.rank(0), 2);
        // one nobody ordered sorts last rather than first, so an order that has not
        // been worked out yet cannot silently promote a dictionary.
        assert_eq!(collection.rank(9), usize::MAX);
    }

    /// a collection with no index answers everything rather than panicking: the
    /// window is up and asking before the worker thread has finished.
    #[test]
    fn an_unopened_collection_is_answerable() {
        let collection = Collection::empty();
        assert!(!collection.is_ready());
        assert_eq!(collection.dict_count(), 0);
        assert_eq!(collection.total_headwords(), 0);
        assert_eq!(collection.scope_size(), (0, 0));
        assert!(collection.search("rex", 10).is_empty());
        assert!(collection.resolve("rex").is_none());
        assert!(
            !collection.nothing_selected(),
            "no dictionaries is not a choice"
        );
        assert_eq!(collection.dict_label(3), "");
    }

    #[test]
    fn the_status_line_names_every_narrowing() {
        let mut collection = Collection::empty();
        assert_eq!(collection.status(1, false), "1 result");
        assert_eq!(collection.status(4009, false), "4,009 results");
        assert_eq!(collection.status(500, true), "500+ results");
        collection.set_fold_forms(true);
        assert_eq!(collection.status(3, false), "3 results · forms folded");
    }

    #[test]
    fn counting_says_it_in_words() {
        assert_eq!(quantity(1, "result", "results"), "1 result");
        assert_eq!(quantity(0, "word", "words"), "0 words");
        assert_eq!(thousands(1941344), "1,941,344");
    }
}
