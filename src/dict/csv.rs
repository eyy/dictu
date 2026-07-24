//! reader for `Ref_LSJ.csv` — the LSJ author/work abbreviations table.
//!
//! despite the `.csv` name it is TAB-separated. columns:
//!   [0] display abbrev (html)  [1] id  [2] plain abbrev  [3] key
//!   [4] full description (html)  [5] number
//! we key by the plain abbrev (col 2) and serve the description (col 4).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::Dictionary;

pub struct CsvDictionary {
    name: String,
    headwords: Vec<String>,
    index: HashMap<String, String>,
}

impl CsvDictionary {
    pub fn open(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("CSV")
            .to_string();
        Ok(Self::from_text(name, &text))
    }

    /// pure parse (no i/o) so tests can drive it with an in-memory fixture.
    fn from_text(name: String, text: &str) -> Self {
        let mut headwords = Vec::new();
        let mut seen = HashSet::new();
        let mut index = HashMap::new();

        for line in text.lines() {
            let mut fields = line.split('\t');
            // col 2 = plain abbrev (headword), col 4 = html description.
            let word = fields.nth(2).map(str::trim).unwrap_or("");
            let definition = fields.nth(1); // relative to the consumed iterator: col 4.
            let (Some(definition), false) = (definition, word.is_empty()) else {
                continue;
            };

            index
                .entry(word.to_string())
                .or_insert_with(|| definition.to_string());
            if seen.insert(word.to_string()) {
                headwords.push(word.to_string());
            }
        }

        Self {
            name,
            headwords,
            index,
        }
    }
}

impl Dictionary for CsvDictionary {
    fn name(&self) -> &str {
        &self.name
    }

    fn headwords(&self) -> &[String] {
        &self.headwords
    }

    fn lookup(&self, headword: &str) -> Option<String> {
        self.index.get(headword).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tab_separated_abbreviations() {
        // col2 = headword, col4 = definition (tab-separated).
        let text = "Apoll. <i>V</i>\t1\tApoll. V\tApoll._V\t<b>Apollonius</b> Vita\t23\n\
                    Alex. <i>M</i>\t2\tAlex. M\tAlex._M\t<b>Alexander</b> Meta\t23\n";
        let dict = CsvDictionary::from_text("Ref_LSJ".into(), text);

        assert_eq!(
            dict.headwords(),
            &["Apoll. V".to_string(), "Alex. M".to_string()]
        );
        assert_eq!(dict.lookup("Apoll. V").unwrap(), "<b>Apollonius</b> Vita");
        assert!(dict.lookup("missing").is_none());
    }

    #[test]
    fn skips_short_lines() {
        // a line without enough columns is skipped, not a crash.
        let dict = CsvDictionary::from_text("t".into(), "only\ttwo\n\nword\t1\tw\tk\tdef\t9\n");
        assert_eq!(dict.headwords(), &["w".to_string()]);
    }
}
