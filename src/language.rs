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
    //
    // the abbreviated forms matter as much as the full ones: a title is as likely to
    // say `(Lat-Fra)` as "Latin", and until it was taught the short one this could not
    // name Gaffiot or Lewis & Short at all — so a latin word showed its dictionary's
    // folder name, or, where two of them had it, nothing but a count, because two
    // dictionaries that both answer `None` are two that "disagree" (#59).
    //
    // `lat-` is spelt with its dash so it is the *source* half of a `Src-Tgt` pair and
    // cannot match a target: `Grc-Lat` is a greek dictionary. the greek, hebrew and
    // french titles here need no such entry — `grc`, `heb` and `français` below already
    // match them — which is why this group has one member rather than five.
    const NAMES: &[(&str, &str)] = &[
        ("lat-", "LAT"),
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

    /// roadmap #59, both symptoms of it. the collection's latin dictionaries call
    /// themselves "Lat-Fra" and "Lat-Eng" rather than "latin", so `jactitabundus` — a
    /// word only Gaffiot has — was tagged with the dictionary's *name*, and `Jabolenus`,
    /// which two of them have, showed a bare count because the two "disagreed" about a
    /// language neither had managed to name.
    #[test]
    fn an_abbreviated_source_language_still_names_it() {
        assert_eq!(tag("jactitabundus", "Gaffiot 2016 (Lat-Fra)"), Some("LAT"));
        assert_eq!(
            tag("Jabolenus", "Lewis and Short 1879 (Lat-Eng)"),
            Some("LAT")
        );
        // and the third latin dictionary already agreed, which is what makes the row
        // read "LAT ·3" rather than "·3".
        assert_eq!(tag("rex", "bgl-Latin_English_Inflected"), Some("LAT"));
    }

    /// the whole shelf, by the titles it actually carries — so a change to the needles
    /// cannot quietly unname a dictionary that was already named.
    #[test]
    fn every_dictionary_in_the_collection_is_named() {
        for (title, expected) in [
            ("Gaffiot 2016 (Lat-Fra)", "LAT"),
            ("Lewis and Short 1879 (Lat-Eng)", "LAT"),
            ("bgl-Latin_English_Inflected", "LAT"),
            ("Bailly 2020 (Grc-Fra)", "GRC"),
            ("Greek-English Lexicon - Liddell & Scott", "GRC"),
            (
                "Greek-English Lexicon by John Jeffrey Dodson (Grc-Eng)",
                "GRC",
            ),
            ("Lexicon to Pindar (Grc-Eng)", "GRC"),
            ("Middle_Liddell_stardict", "GRC"),
            ("LSJ sources", "GRC"),
            (
                "Comprehensive Etymological Dictionary of the Hebrew Language (Heb-Eng)",
                "HEB",
            ),
            ("Etymological ABBRV (Heb-Eng)", "HEB"),
            ("HEB-HEB a hebrew-hebrew dictionary", "HEB"),
            ("Hebrew and Aramaic Lexicon of the Old Testament", "HEB"),
            ("Larousse Chambers français-anglais", "FR"),
        ] {
            // a latin-script word, so the title is what has to answer
            assert_eq!(tag("word", title), Some(expected), "{title}");
        }
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
