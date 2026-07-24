//! reader for the dictd DICT format: a `.index` file plus a `.dict` (or
//! gzip-compressed `.dict.dz`) data file.
//!
//! the `.index` is plain utf-8 text, one entry per line, tab-separated:
//!     headword <TAB> offset <TAB> length
//! `offset` and `length` are byte positions into the *decompressed* `.dict`
//! data, encoded in dictd's own base-64 variant (see `b64_decode`).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::{DictBytes, Dictionary, load_dict_bytes};

/// dictd's base-64 alphabet. index of a char = its 6-bit value (so 'A' = 0).
/// note this is the *standard* base64 order, but used to encode a big-endian
/// integer rather than a byte stream.
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub struct DictdDictionary {
    name: String,
    /// visible headwords in file order (control entries like `00-database-*`
    /// are filtered out).
    headwords: Vec<String>,
    /// headword -> byte ranges `(offset, length)` into `data`. a `Vec` because
    /// the same headword can legitimately appear more than once.
    index: HashMap<String, Vec<(usize, usize)>>,
    /// the `.dict` bytes, memory-mapped (lazy) rather than read into RAM.
    data: DictBytes,
}

impl DictdDictionary {
    /// open a dictionary given the path to its `.index` file. the matching
    /// `.dict` / `.dict.dz` is located alongside it.
    pub fn open(index_path: &Path) -> Result<Self> {
        let index_text = fs::read_to_string(index_path)
            .with_context(|| format!("reading index {}", index_path.display()))?;

        // locate the data file: same stem, but `.dict` or `.dict.dz`.
        let stem = index_path
            .file_stem() // strips the ".index" extension.
            .and_then(|s| s.to_str())
            .context("index path has no usable file stem")?;
        let dir = index_path.parent().unwrap_or_else(|| Path::new("."));

        // mmap the plain .dict (lazy) or gunzip a .dict.dz into memory.
        let data = load_dict_bytes(dir, stem, &["dict", "dict.dz"])
            .with_context(|| format!("locating .dict for {}", index_path.display()))?;

        let name = stem.to_string();
        Self::from_parts(name, &index_text, data)
    }

    /// build from already-loaded parts. pure (no file i/o), so tests can drive
    /// it with in-memory fixtures.
    fn from_parts(name: String, index_text: &str, data: DictBytes) -> Result<Self> {
        let mut headwords = Vec::new();
        let mut index: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        let mut title = None;

        for line in index_text.lines() {
            if line.is_empty() {
                continue;
            }
            // split into at most 3 fields; a 4th (rare) is ignored. a malformed
            // line is skipped rather than failing the whole load — a truncated
            // or slightly-corrupt file should still open with what's valid.
            let mut fields = line.splitn(3, '\t');
            let (Some(headword), Some(offset), Some(length)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(offset), Ok(length)) = (b64_decode(offset), b64_decode(length)) else {
                continue; // bad/overflowing base64 — skip this entry.
            };

            // control entries carry metadata, not real words.
            if let Some(rest) = headword.strip_prefix("00-database-") {
                if rest.starts_with("short") {
                    // the short title lives in this entry's definition text.
                    if let Some(text) = slice_text(data.as_slice(), offset, length) {
                        title = Some(text.trim().to_string());
                    }
                }
                continue; // never show control entries in the word list.
            }

            index
                .entry(headword.to_string())
                .or_default()
                .push((offset, length));
            headwords.push(headword.to_string());
        }

        Ok(Self {
            // prefer the embedded short title if we found one.
            name: title.unwrap_or(name),
            headwords,
            index,
            data,
        })
    }
}

impl Dictionary for DictdDictionary {
    fn name(&self) -> &str {
        &self.name
    }

    fn headwords(&self) -> &[String] {
        &self.headwords
    }

    fn lookup(&self, headword: &str) -> Vec<String> {
        // one headword may map to several definitions; each comes back separately.
        self.index
            .get(headword)
            .into_iter()
            .flatten()
            .filter_map(|&(offset, length)| slice_text(self.data.as_slice(), offset, length))
            .filter(|entry| !entry.is_empty())
            .collect()
    }
}

/// slice `[offset, offset+length)` out of the data as text, lossily decoding
/// any non-utf8 bytes. returns None if the range is out of bounds.
fn slice_text(data: &[u8], offset: usize, length: usize) -> Option<String> {
    let end = offset.checked_add(length)?;
    let bytes = data.get(offset..end)?;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// decode dictd's base-64 integer: each char is a 6-bit big-endian digit.
/// e.g. "A" -> 0, "R" -> 17, "BA" -> 1*64 + 0 = 64.
fn b64_decode(s: &str) -> Result<usize> {
    let mut n = 0usize;
    for b in s.bytes() {
        let value = B64
            .iter()
            .position(|&c| c == b)
            .with_context(|| format!("invalid base64 digit {:?} in index", b as char))?;
        // checked so a hostile/overlong field can't overflow (debug panic /
        // release wraparound) — see review finding.
        n = n
            .checked_mul(64)
            .and_then(|x| x.checked_add(value))
            .context("base64 integer overflow in index")?;
    }
    Ok(n)
}

// `#[cfg(test)]` compiles this module only for `cargo test`, so tests add no
// weight to the shipped binary. tests live beside the code they exercise and
// can reach private items (from_parts, b64_decode).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_dictd_base64() {
        assert_eq!(b64_decode("A").unwrap(), 0);
        assert_eq!(b64_decode("R").unwrap(), 17);
        assert_eq!(b64_decode("Z").unwrap(), 25);
        assert_eq!(b64_decode("BA").unwrap(), 64); // 1*64 + 0
    }

    #[test]
    fn parses_index_and_looks_up() {
        // two entries laid out back-to-back in the .dict data.
        //   "apple\n  A fruit.\n"      -> offset 0,  length 17
        //   "banana\n  A yellow fruit.\n" -> offset 17, length 25
        let data = b"apple\n  A fruit.\nbanana\n  A yellow fruit.\n".to_vec();
        assert_eq!(data.len(), 42);
        // offsets/lengths in dictd base64: 0="A", 17="R", 25="Z".
        let index = "apple\tA\tR\nbanana\tR\tZ\n";

        let dict =
            DictdDictionary::from_parts("test".into(), index, DictBytes::Owned(data)).unwrap();

        assert_eq!(
            dict.headwords(),
            &["apple".to_string(), "banana".to_string()]
        );
        assert!(dict.lookup("apple")[0].contains("A fruit."));
        assert!(dict.lookup("banana")[0].contains("yellow fruit."));
        assert!(dict.lookup("missing").is_empty());
    }

    #[test]
    fn open_reads_index_and_gzipped_dict_from_disk() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        // a unique temp dir (process id avoids clashes; no rng/clock needed).
        let dir = std::env::temp_dir().join(format!("dictu-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let stem = "sample";

        let data = b"pear\n  A sweet fruit.\n";
        // write the .index (offset 0, length = data.len() = 22 -> "W").
        fs::write(dir.join(format!("{stem}.index")), "pear\tA\tW\n").unwrap();
        // write a gzip-compressed .dict.dz, exactly as dictzip files decode.
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(data).unwrap();
        fs::write(dir.join(format!("{stem}.dict.dz")), gz.finish().unwrap()).unwrap();

        let dict = DictdDictionary::open(&dir.join(format!("{stem}.index"))).unwrap();
        assert_eq!(dict.headwords(), &["pear".to_string()]);
        assert!(dict.lookup("pear")[0].contains("sweet fruit."));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hides_control_entries_and_reads_title() {
        // a 00-database-short control entry supplies the title and is not a word.
        let data = b"My Test Dictionary\nkiwi\n  A green fruit.\n".to_vec();
        // "My Test Dictionary\n" = 19 bytes (offset 0), then kiwi entry at 19.
        // 19 in base64 = "T"; kiwi entry length = 22 -> "W".
        let index = "00-database-short\tA\tT\nkiwi\tT\tW\n";

        let dict =
            DictdDictionary::from_parts("fallback".into(), index, DictBytes::Owned(data)).unwrap();

        assert_eq!(dict.name(), "My Test Dictionary");
        assert_eq!(dict.headwords(), &["kiwi".to_string()]); // control entry hidden.
    }
}
