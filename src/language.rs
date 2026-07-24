//! the short, all-caps tag shown after a word in the wordlist, so a result from a
//! collection spanning six languages says which one it belongs to.
//!
//! there is no language field in the StarDict `.ifo` format, so this works from what
//! is actually available: the script the headword is written in — which settles
//! hebrew and greek outright — and failing that, the language named in the
//! dictionary's own title. when neither answers, the caller shows the dictionary's
//! name instead, which is the honest fallback.

/// the tag for `word` from a dictionary titled `dict_name`, or `None` when neither
/// the script nor the title says anything (show the dictionary's name then).
pub fn tag(word: &str, dict_name: &str) -> Option<&'static str> {
    script_tag(word).or_else(|| named_language(dict_name))
}

/// the language a word's script names on its own. only for scripts that belong to
/// one language in practice — latin script says nothing, so it isn't listed.
fn script_tag(word: &str) -> Option<&'static str> {
    word.chars().filter(|c| c.is_alphabetic()).find_map(|c| {
        Some(match u32::from(c) {
            0x0590..=0x05FF | 0xFB1D..=0xFB4F => "HEB",
            // greek and coptic, plus the extended block the classical dictionaries
            // use for polytonic accents.
            0x0370..=0x03FF | 0x1F00..=0x1FFF => "GRC",
            0x0400..=0x04FF => "CYR",
            0x0600..=0x06FF => "ARA",
            _ => return None,
        })
    })
}

/// a language named in the dictionary's title. the source language is checked before
/// english, because a bilingual title like "French - English" describes headwords in
/// the first of the two.
fn named_language(dict_name: &str) -> Option<&'static str> {
    let name = dict_name.to_lowercase();
    // (needle, tag), most specific first.
    const NAMES: &[(&str, &str)] = &[
        ("latin", "LAT"),
        ("french", "FR"),
        ("français", "FR"),
        ("fr-en", "FR"),
        ("greek", "GRC"),
        ("grc", "GRC"),
        ("lsj", "GRC"),
        ("liddell", "GRC"),
        ("pindar", "GRC"),
        ("hebrew", "HEB"),
        ("heb", "HEB"),
        ("aramaic", "ARC"),
        ("german", "DEU"),
        ("spanish", "SPA"),
        ("italian", "ITA"),
        ("russian", "RUS"),
        ("arabic", "ARA"),
        ("english", "ENG"),
    ];
    NAMES
        .iter()
        .find(|(needle, _)| name.contains(needle))
        .map(|(_, tag)| *tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_settles_hebrew_and_greek_whatever_the_dictionary_is_called() {
        assert_eq!(tag("שׁוֹשָׁן", "a hebrew-hebrew dictionary"), Some("HEB"));
        assert_eq!(tag("λόγος", "MiddleLiddell"), Some("GRC"));
        // polytonic accents live in the extended greek block.
        assert_eq!(tag("ἀρετή", "whatever"), Some("GRC"));
        assert_eq!(tag("книга", "whatever"), Some("CYR"));
    }

    #[test]
    fn latin_script_falls_back_to_the_dictionary_title() {
        assert_eq!(
            tag("virtus", "Dictionary latininfl -> english"),
            Some("LAT")
        );
        assert_eq!(tag("bonjour", "French - English.csv (fr-en)"), Some("FR"));
        // the source language wins over the target: headwords are french here.
        assert_eq!(tag("maison", "French - English"), Some("FR"));
        assert_eq!(tag("D. H. Lys.", "Ref_LSJ"), Some("GRC"));
    }

    #[test]
    fn unknown_titles_get_no_tag() {
        // the caller shows the dictionary's own name in this case.
        assert_eq!(tag("word", "Some Glossary"), None);
        assert_eq!(tag("", "Some Glossary"), None);
    }

    #[test]
    fn punctuation_and_digits_do_not_decide_the_script() {
        // leading punctuation must not stop the scan reaching the hebrew letters.
        assert_eq!(tag("־שׁ", "x"), Some("HEB"));
        assert_eq!(tag("1. λόγος", "x"), Some("GRC"));
    }
}
