//! the normalized keys search runs on (roadmap #39,
//! `docs/search-index-plan.md` §1). two are derived; only the **bare** key is
//! stored, because it is the one the index is sorted by.
//!
//! **fold key** — NFC, then *full* case folding, in that order.
//! NFC is what makes greek typable: Dodson files `ό` as U+1F79 (oxia) and every
//! greek keyboard produces U+03CC (tonos), and NFC maps the first onto the
//! second. folding is what makes final sigma typable: `to_lowercase()` leaves
//! `ς` alone, so `λόγος` and `λόγοσ` would stay two different words, and 139,079
//! headwords here carry a final sigma.
//! the order is load-bearing, and not because either step subsumes the other
//! (`casefold(U+1F79) = U+1F79`): folding turns U+0345, combining ypogegrammeni,
//! into a plain iota, so marks stored out of canonical order — which HALOT does,
//! for 3,259 headwords — fold into a *different* word unless NFC has reordered
//! them first. `α + U+0345 + U+0301` is `ᾴ`; fold before normalizing and the
//! acute lands on the iota, giving `αί` instead of `άι`.
//!
//! **bare key** — the fold key, decomposed, stripped of its combining marks,
//! recomposed. this is what makes hebrew typable: 99,388 of the 121,615 hebrew
//! headwords carry niqqud and 74,174 of those have no unpointed sibling anywhere
//! in the collection, so NFC alone leaves them unreachable by anyone typing
//! hebrew normally — and fuzzy could not rescue them either, `מֶ֫לֶךְ` against
//! `מלך` being edit distance 4. the bare key is what the index is sorted by, so
//! an unpointed query reaches every pointed spelling of a word.
//!
//! that permissiveness is deliberately **one-way**, and `marks_allow` is the
//! other half: a query that spells out diacritics must not be answered with a
//! headword that contradicts them. `מלך` finds `מֶלֶךְ` and `מָלָךְ` both;
//! `מֶלֶךְ` finds only the first.
//!
//! the plan says category `Mn`; this treats general category `M` (`Mn`, `Mc`,
//! `Me`) as marks, which is what the normalization crate already answers and so
//! costs no second table. the two differ only for scripts with spacing or
//! enclosing marks — none of which this collection has a dictionary for.

use std::borrow::Cow;

use caseless::Caseless;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// the fold key of a headword or a query. borrowed when the word is already its
/// own key, which is most of the collection (the latin dictionary alone is 1.2M
/// lowercase ascii headwords) — worth the `Cow`, since the cold path derives
/// this 1.8M times.
pub fn fold(word: &str) -> Cow<'_, str> {
    // ascii is NFC by construction and folds to plain lowercase, so the
    // normalization machinery has nothing to do here.
    if word.is_ascii() {
        return match word.bytes().any(|b| b.is_ascii_uppercase()) {
            true => Cow::Owned(word.to_ascii_lowercase()),
            false => Cow::Borrowed(word),
        };
    }
    Cow::Owned(word.nfc().default_case_fold().collect())
}

/// the bare key, derived from a *fold* key — never from the raw word, so the two
/// keys agree about case and canonical form and differ only in the marks.
pub fn bare(fold: &str) -> Cow<'_, str> {
    if fold.is_ascii() {
        return Cow::Borrowed(fold); // nothing ascii decomposes.
    }
    let stripped: String = fold
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .nfc()
        .collect();
    match stripped == fold {
        true => Cow::Borrowed(fold),
        false => Cow::Owned(stripped),
    }
}

/// does `word` spell every mark `query` did? both are known to share a bare
/// prefix already, so this walks the two decompositions in step and compares
/// only the marks hanging off each base character: the query's must be a
/// **subset** of the headword's.
///
/// subset, not equality, is what makes partial pointing usable — `מֶלך` typed
/// with one vowel still reaches `מֶלֶךְ` — while `מֶלֶךְ` no longer answers with
/// `מָלָךְ`. the asymmetry is the whole rule: a query says how specific it is,
/// and a headword may be more specific than the query but never contradict it.
///
/// `word` is the raw headword, folded and decomposed lazily here rather than
/// through a stored key: this runs once per candidate in a prefix run, only for
/// queries that carry marks at all, and building a `String` per candidate is the
/// cost worth avoiding.
pub fn marks_allow(query_fold: &str, word: &str) -> bool {
    let mut query = query_fold.nfd().peekable();
    let mut word = word.nfc().default_case_fold().nfd().peekable();
    let mut theirs: Vec<char> = Vec::new();
    loop {
        // bases align, because the bare keys prefix-matched to get here; the
        // query runs out first, since it is the prefix.
        let Some(base) = query.next() else {
            return true;
        };
        if word.next() != Some(base) {
            return false;
        }
        theirs.clear();
        while let Some(mark) = word.next_if(|c| is_combining_mark(*c)) {
            theirs.push(mark);
        }
        while let Some(mark) = query.next_if(|c| is_combining_mark(*c)) {
            if !theirs.contains(&mark) {
                return false;
            }
        }
    }
}

/// every headword's key, in slot order, packed into one blob with `u32` offsets
/// — the shape the cache image stores, and the reason the key is materialized at
/// all. the old sort derived `to_lowercase()` inside the comparison, one
/// `String` per compare; normalizing there would cost far more, so the key is
/// built once here. a `Vec<String>` instead would be 1.8M allocations and 44 MB
/// of headers for ~20 MB of text.
pub struct KeyTable {
    blob: String,
    /// `off[i]..off[i + 1]` is slot `i`'s key; one longer than the slot count.
    off: Vec<u32>,
}

impl KeyTable {
    pub fn with_capacity(slots: usize) -> Self {
        let mut off = Vec::with_capacity(slots + 1);
        off.push(0);
        Self {
            // ~12 bytes a key across this collection; a guess that saves the
            // early doubling, not a bound.
            blob: String::with_capacity(slots * 12),
            off,
        }
    }

    pub fn push(&mut self, key: &str) {
        self.blob.push_str(key);
        self.off.push(self.blob.len() as u32);
    }

    pub fn count(&self) -> usize {
        self.off.len() - 1
    }

    /// slot `i`'s key. panics on an out-of-range slot: this table is built in
    /// process from our own headword lists, so a bad index is a bug here, not
    /// the corrupt-file case `index_cache` has to tolerate.
    pub fn get(&self, slot: u32) -> &str {
        let at = slot as usize;
        &self.blob[self.off[at] as usize..self.off[at + 1] as usize]
    }

    /// the whole blob, for the cache image to copy out in sorted order.
    pub fn byte_len(&self) -> usize {
        self.blob.len()
    }

    /// slot ids sorted by their key: the searchable order. the slot tie-break
    /// makes equal keys come out in dictionary order (and makes the sort
    /// reproducible without paying for a stable one).
    pub fn order(&self) -> Vec<u32> {
        let mut order: Vec<u32> = (0..self.count() as u32).collect();
        order.sort_unstable_by(|&a, &b| self.get(a).cmp(self.get(b)).then(a.cmp(&b)));
        order
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the headline #39 fix: the oxia Dodson stores and the tonos a greek
    /// keyboard types are the same word once NFC has run.
    #[test]
    fn fold_maps_oxia_onto_tonos() {
        let stored = "λ\u{1f79}γος"; // U+1F79 greek small omicron with oxia.
        let typed = "λ\u{3cc}γος"; // U+03CC greek small omicron with tonos.
        assert_ne!(stored, typed, "the fixture must be the two spellings");
        assert_eq!(fold(stored), fold(typed));
        // tonos, and the final sigma folded along the way.
        assert_eq!(fold(stored), "λ\u{3cc}γοσ");
    }

    /// final sigma is the half lowercasing does not do.
    #[test]
    fn fold_settles_final_sigma_and_case() {
        assert_eq!(fold("λόγος"), fold("λόγοσ"));
        assert_eq!(fold("ΛΌΓΟΣ"), fold("λόγος"));
        // and what std would have given instead, which is why caseless is here.
        assert_ne!("λόγος".to_lowercase(), "λόγοσ".to_lowercase());
    }

    /// the ordering trap. folding turns U+0345 (combining ypogegrammeni) into a
    /// plain iota, so marks left in non-canonical order — `ᾴ` written with the
    /// ypogegrammeni before the acute — fold into a different word unless NFC
    /// has reordered them first.
    #[test]
    fn nfc_must_run_before_folding() {
        let out_of_order = "α\u{345}\u{301}";
        let composed = "\u{1fb4}"; // ᾴ, the same word canonically composed.
        assert_eq!(fold(out_of_order), fold(composed));

        // fold first and the acute lands on the iota instead of the alpha.
        let wrong: String = out_of_order.chars().default_case_fold().nfc().collect();
        assert_ne!(wrong, fold(out_of_order).as_ref());
        assert_ne!(wrong, fold(composed).as_ref());
    }

    /// niqqud is what the bare key exists for: 74,174 pointed hebrew headwords
    /// have no unpointed sibling to find them by.
    #[test]
    fn bare_strips_hebrew_niqqud() {
        let pointed = "מֶ֫לֶךְ"; // as a hebrew-hebrew dictionary and Klein file it.
        assert_ne!(pointed, "מלך");
        assert_eq!(bare(&fold(pointed)), "מלך");
        // an unpointed headword is its own bare key, so both reach it.
        assert_eq!(bare(&fold("מלך")), "מלך");
    }

    #[test]
    fn bare_strips_greek_accents_and_leaves_the_rest() {
        assert_eq!(bare(&fold("λόγος")), "λογοσ");
        assert_eq!(bare(&fold("ἀνθρωπος")), "ανθρωποσ");
        // no marks to drop: the bare key is the fold key, borrowed not rebuilt.
        assert!(matches!(bare(&fold("rex")), Cow::Borrowed(_)));
        assert_eq!(bare(&fold("Rex")), "rex");
    }

    /// the fold key is what the bare key is derived from, so folding still shows
    /// through it — a query typed with a final sigma finds a stripped headword.
    #[test]
    fn bare_keeps_the_folding() {
        assert_eq!(bare(&fold("Λόγοσ")), bare(&fold("λόγος")));
    }

    /// the asymmetry: a headword may be more specific than the query, never
    /// less, and never differently.
    #[test]
    fn marks_allow_is_a_subset_test() {
        let query = fold("מֶלֶךְ");
        assert!(marks_allow(&query, "מֶלֶךְ"), "the same pointing");
        assert!(!marks_allow(&query, "מָלָךְ"), "a different pointing");
        assert!(!marks_allow(&query, "מלך"), "no pointing at all");
        // half-pointed reaches the full spelling, which equality would not.
        assert!(marks_allow(&fold("מֶלך"), "מֶלֶךְ"));
        // a prefix of a longer headword still qualifies.
        assert!(marks_allow(&fold("מֶלֶךְ"), "מֶלֶךְ אֶבְיוֹן"));
    }

    /// the filter compares *fold* keys, so an oxia headword and a tonos query
    /// are the same accent by the time their marks are compared.
    #[test]
    fn marks_allow_compares_normalized_marks() {
        assert!(marks_allow(&fold("λ\u{3cc}γος"), "λ\u{1f79}γος"));
        assert!(marks_allow(&fold("Λόγος"), "λόγος"), "case is folded first");
        assert!(!marks_allow(&fold("λόγος"), "λὸγος"), "acute is not grave");
        assert!(!marks_allow(&fold("λόγος"), "λογος"), "acute is not bare");
        // a headword that runs out before the query does is no answer.
        assert!(!marks_allow(&fold("λόγος"), "λόγ"));
    }

    #[test]
    fn key_table_packs_and_orders() {
        let mut table = KeyTable::with_capacity(3);
        for key in ["beta", "alpha", "beta"] {
            table.push(key);
        }
        assert_eq!(table.count(), 3);
        assert_eq!(table.get(0), "beta");
        assert_eq!(table.get(1), "alpha");
        assert_eq!(table.byte_len(), 13);
        // sorted by key, equal keys in slot order.
        assert_eq!(table.order(), vec![1, 0, 2]);
    }

    #[test]
    fn key_table_handles_an_empty_key() {
        // a headword of nothing but marks bares down to nothing at all.
        let folded = fold("\u{5b0}");
        let bare_of_marks = bare(&folded);
        assert!(bare_of_marks.is_empty());
        let mut table = KeyTable::with_capacity(2);
        table.push(&bare_of_marks);
        table.push("a");
        assert_eq!(table.get(0), "");
        assert_eq!(table.order(), vec![0, 1]);
    }
}
