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
    fold_forms: bool,
}

/// one dictionary's answer for a word: its label, and every entry it files.
pub struct Definition {
    pub dict: usize,
    pub label: String,
    pub entries: Vec<String>,
}

impl Collection {
    pub fn empty() -> Self {
        Self {
            library: None,
            scope: Vec::new(),
            fold_forms: false,
        }
    }

    /// hand over the built index. the scope opens fully: a dictionary the reader
    /// has never seen is one they have not excluded.
    pub fn open(&mut self, library: Library) {
        self.scope = vec![true; library.dict_count()];
        self.library = Some(library);
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
                    entries,
                }),
            }
        }
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
