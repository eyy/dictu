//! reader for the StarDict format: a set of sibling files sharing a base name:
//!   - `.ifo`  — text metadata (`key=value` lines).
//!   - `.idx`  — binary index: repeated `word\0` + u32 offset + u32 size.
//!   - `.dict` / `.dict.dz` — the definition data (optionally gzip/dictzip).
//!   - `.syn`  — optional synonyms: `word\0` + u32 index-into-.idx.
//!
//! all integers in `.idx`/`.syn` are big-endian. offsets/sizes point into the
//! *decompressed* `.dict` data.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{DictBytes, Dictionary, load_dict_bytes};

/// a byte range `(offset, size)` into the decompressed `.dict` data.
type Range = (u64, u32);
/// headword -> the ranges holding its definition(s).
type WordIndex = HashMap<String, Vec<Range>>;

pub struct StarDict {
    name: String,
    headwords: Vec<String>,
    /// how each entry's data block is typed (the `.ifo` `sametypesequence`).
    /// `Some("h")` = one html field; `None` = each field is type-prefixed.
    same_type_sequence: Option<String>,
    /// word -> byte ranges in `data`. a `Vec` because a word (or a synonym) can
    /// map to more than one entry.
    index: WordIndex,
    /// the `.dict` bytes, memory-mapped (lazy) rather than read into RAM.
    data: DictBytes,
}

impl StarDict {
    /// open a dictionary given the path to its `.ifo` file.
    pub fn open(ifo_path: &Path) -> Result<Self> {
        let ifo_text = fs::read_to_string(ifo_path)
            .with_context(|| format!("reading {}", ifo_path.display()))?;
        let ifo = parse_ifo(&ifo_text);

        let dir = ifo_path.parent().unwrap_or_else(|| Path::new("."));
        let stem = ifo_path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("ifo path has no usable stem")?;

        // the .idx (possibly gzipped as .idx.gz). we read it via the shared
        // loader too, then parse its bytes — no need to hold it after open().
        let idx =
            load_dict_bytes(dir, stem, &["idx", "idx.gz"]).context("locating/reading .idx")?;

        // mmap the plain .dict (lazy) or gunzip a .dict.dz into memory.
        let data =
            load_dict_bytes(dir, stem, &["dict", "dict.dz"]).context("locating/reading .dict")?;

        // idxoffsetbits is 32 unless the ifo says 64.
        let offset_bits: u8 = ifo
            .get("idxoffsetbits")
            .and_then(|v| v.parse().ok())
            .unwrap_or(32);
        if offset_bits != 32 && offset_bits != 64 {
            bail!("unsupported idxoffsetbits={offset_bits}");
        }

        let name = ifo
            .get("bookname")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| stem.to_string());
        let same_type_sequence = ifo.get("sametypesequence").cloned();

        // raw_order holds every entry's range in raw .idx order — the .syn file
        // indexes into THAT, not the filtered/deduped display list.
        let (mut headwords, raw_order, mut index) = parse_idx(idx.as_slice(), offset_bits)?;

        // fold in synonyms if a .syn file sits alongside.
        let syn_path = dir.join(format!("{stem}.syn"));
        if syn_path.exists() {
            let syn =
                fs::read(&syn_path).with_context(|| format!("reading {}", syn_path.display()))?;
            apply_syn(&syn, &raw_order, &mut index, &mut headwords);
        }

        Ok(Self {
            name,
            headwords,
            same_type_sequence,
            index,
            data,
        })
    }

    /// pull the definition text out of one entry's data block, according to the
    /// StarDict field-typing rules.
    fn entry_text(&self, offset: u64, size: u32) -> Option<String> {
        let start = usize::try_from(offset).ok()?;
        let end = start.checked_add(size as usize)?;
        let block = self.data.as_slice().get(start..end)?;

        match self.same_type_sequence.as_deref() {
            // one type char -> the whole block is a single field of that type.
            Some(seq) if seq.chars().count() == 1 => {
                Some(decode_field(seq.chars().next().unwrap(), block))
            }
            // several type chars -> N fields laid out per the sequence.
            Some(seq) => Some(fields_by_sequence(seq, block)),
            // no sequence -> each field carries its own leading type byte.
            None => Some(fields_type_prefixed(block)),
        }
    }
}

impl Dictionary for StarDict {
    fn name(&self) -> &str {
        &self.name
    }

    fn headwords(&self) -> &[String] {
        &self.headwords
    }

    fn lookup(&self, headword: &str) -> Option<String> {
        let ranges = self.index.get(headword)?;
        let joined = ranges
            .iter()
            .filter_map(|&(offset, size)| self.entry_text(offset, size))
            .collect::<Vec<_>>()
            .join("\n<hr/>\n");
        (!joined.is_empty()).then_some(joined)
    }
}

// -- .ifo ------------------------------------------------------------------

/// parse the `.ifo` into a key->value map. the first line is a magic banner we
/// skip; the rest are `key=value`.
fn parse_ifo(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

// -- .idx / .syn -----------------------------------------------------------

/// parse the `.idx` into: the deduped display headwords, the raw per-entry
/// ranges in file order (for `.syn` resolution), and the lookup map.
fn parse_idx(idx: &[u8], offset_bits: u8) -> Result<(Vec<String>, Vec<Range>, WordIndex)> {
    let mut headwords = Vec::new();
    let mut raw_order: Vec<Range> = Vec::new();
    let mut seen = HashSet::new();
    let mut index: WordIndex = HashMap::new();

    let int_bytes = if offset_bits == 64 { 8 } else { 4 };
    let mut pos = 0;
    while pos < idx.len() {
        // headword: bytes up to the next NUL. if the file is truncated mid-entry
        // we stop cleanly, keeping what parsed, rather than failing the load.
        let Some(nul) = idx[pos..].iter().position(|&b| b == 0) else {
            break;
        };
        let word = String::from_utf8_lossy(&idx[pos..pos + nul]).into_owned();
        pos += nul + 1;

        // then offset (32 or 64 bit) and size (always 32 bit), big-endian.
        let Some(offset) = read_be_uint(idx, pos, int_bytes) else {
            break;
        };
        pos += int_bytes;
        let Some(size) = read_be_uint(idx, pos, 4) else {
            break;
        };
        let size = size as u32;
        pos += 4;

        raw_order.push((offset, size));
        // index every range (a word can have several sense entries)...
        index.entry(word.clone()).or_default().push((offset, size));
        // ...but the visible list shows each word once, and skips the `##...`
        // metadata pseudo-entries some converters emit.
        if !word.starts_with("##") && seen.insert(word.clone()) {
            headwords.push(word);
        }
    }

    Ok((headwords, raw_order, index))
}

/// merge a `.syn` file: each record is `synonym\0` + u32 index into the RAW
/// `.idx` order. the synonym resolves to that entry's data and joins the list.
fn apply_syn(syn: &[u8], raw_order: &[Range], index: &mut WordIndex, headwords: &mut Vec<String>) {
    let mut seen: HashSet<String> = headwords.iter().cloned().collect();
    let mut pos = 0;
    while pos < syn.len() {
        let Some(nul) = syn[pos..].iter().position(|&b| b == 0) else {
            break;
        };
        let word = String::from_utf8_lossy(&syn[pos..pos + nul]).into_owned();
        pos += nul + 1;
        let Some(entry_index) = read_be_uint(syn, pos, 4) else {
            break;
        };
        pos += 4;

        // index into the raw file order — NOT the filtered/deduped display list,
        // whose positions are shifted by skipped `##` / duplicate entries.
        if let Some(&range) = raw_order.get(entry_index as usize) {
            index.entry(word.clone()).or_default().push(range);
            if !word.starts_with("##") && seen.insert(word.clone()) {
                headwords.push(word);
            }
        }
    }
}

/// read a big-endian unsigned integer of `n` bytes (1..=8) starting at `pos`.
fn read_be_uint(buf: &[u8], pos: usize, n: usize) -> Option<u64> {
    let bytes = buf.get(pos..pos + n)?;
    Some(bytes.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64))
}

// -- .dict field decoding --------------------------------------------------

/// is this StarDict field type textual (lowercase) rather than binary media?
fn is_text_type(type_char: char) -> bool {
    // m,l,g,t,x,y,k,w,h,n,r ... are text-ish; W (wav), P (picture) are binary.
    type_char.is_ascii_lowercase()
}

/// decode a single field's bytes as text (or empty string for binary types).
fn decode_field(type_char: char, bytes: &[u8]) -> String {
    if is_text_type(type_char) {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        String::new()
    }
}

/// fields laid out per an explicit `sametypesequence`. every field but the last
/// is either NUL-terminated (text/lowercase) or u32-length-prefixed (binary/
/// uppercase); the last field runs to the end of the block.
fn fields_by_sequence(seq: &str, block: &[u8]) -> String {
    let types: Vec<char> = seq.chars().collect();
    let mut out = Vec::new();
    let mut pos = 0;
    for (i, &type_char) in types.iter().enumerate() {
        let last = i == types.len() - 1;
        let field = if last {
            &block[pos.min(block.len())..]
        } else if type_char.is_ascii_uppercase() {
            let Some(len) = read_be_uint(block, pos, 4) else {
                break;
            };
            pos += 4;
            let end = (pos + len as usize).min(block.len());
            let f = &block[pos..end];
            pos = end;
            f
        } else {
            let Some(nul) = block[pos..].iter().position(|&b| b == 0) else {
                break;
            };
            let f = &block[pos..pos + nul];
            pos += nul + 1;
            f
        };
        if is_text_type(type_char) {
            out.push(String::from_utf8_lossy(field).into_owned());
        }
    }
    out.join("\n")
}

/// fields with no `sametypesequence`: each field begins with a type byte, then
/// text (NUL-terminated) or binary (u32 length + data).
fn fields_type_prefixed(block: &[u8]) -> String {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < block.len() {
        let type_char = block[pos] as char;
        pos += 1;
        let field = if type_char.is_ascii_uppercase() {
            let Some(len) = read_be_uint(block, pos, 4) else {
                break;
            };
            pos += 4;
            let end = (pos + len as usize).min(block.len());
            let f = &block[pos..end];
            pos = end;
            f
        } else {
            let nul = block[pos..]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(block.len() - pos);
            let f = &block[pos..pos + nul];
            pos += nul;
            if pos < block.len() {
                pos += 1; // skip the NUL.
            }
            f
        };
        if is_text_type(type_char) {
            out.push(String::from_utf8_lossy(field).into_owned());
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_idx_and_looks_up_html() {
        // sametypesequence=h -> each block is one html field.
        //   "alpha" -> "<b>alpha</b>" at offset 0, size 12
        //   "beta"  -> "<i>beta</i>"  at offset 12, size 11
        let data = b"<b>alpha</b><i>beta</i>".to_vec();
        assert_eq!(data.len(), 23);

        // build an idx by hand: word\0 + u32BE offset + u32BE size.
        let mut idx = Vec::new();
        let push = |idx: &mut Vec<u8>, w: &str, off: u32, size: u32| {
            idx.extend_from_slice(w.as_bytes());
            idx.push(0);
            idx.extend_from_slice(&off.to_be_bytes());
            idx.extend_from_slice(&size.to_be_bytes());
        };
        push(&mut idx, "alpha", 0, 12);
        push(&mut idx, "beta", 12, 11);

        let (headwords, _raw, index) = parse_idx(&idx, 32).unwrap();
        let dict = StarDict {
            name: "T".into(),
            headwords,
            same_type_sequence: Some("h".into()),
            index,
            data: DictBytes::Owned(data),
        };

        assert_eq!(dict.headwords(), &["alpha".to_string(), "beta".to_string()]);
        assert_eq!(dict.lookup("alpha").unwrap(), "<b>alpha</b>");
        assert_eq!(dict.lookup("beta").unwrap(), "<i>beta</i>");
        assert!(dict.lookup("gamma").is_none());
    }

    #[test]
    fn synonyms_resolve_against_raw_idx_order() {
        // a `##` metadata entry FIRST makes the filtered display list and the raw
        // .idx order diverge: raw index 1 = "color", but display index 1 is OOB.
        // the .syn index follows the RAW order, which is the bug we fixed.
        let data = b"META<b>color</b>".to_vec(); // "META"=raw0 (##), color=raw1
        let mut idx = Vec::new();
        let push = |idx: &mut Vec<u8>, w: &str, off: u32, size: u32| {
            idx.extend_from_slice(w.as_bytes());
            idx.push(0);
            idx.extend_from_slice(&off.to_be_bytes());
            idx.extend_from_slice(&size.to_be_bytes());
        };
        push(&mut idx, "##name", 0, 4);
        push(&mut idx, "color", 4, 12);

        let (mut headwords, raw, mut index) = parse_idx(&idx, 32).unwrap();
        assert_eq!(headwords, &["color".to_string()]); // ## hidden from display.

        // .syn: "colour" -> RAW index 1 (color).
        let mut syn = Vec::new();
        syn.extend_from_slice(b"colour\0");
        syn.extend_from_slice(&1u32.to_be_bytes());
        apply_syn(&syn, &raw, &mut index, &mut headwords);

        let dict = StarDict {
            name: "T".into(),
            headwords,
            same_type_sequence: Some("h".into()),
            index,
            data: DictBytes::Owned(data),
        };
        assert_eq!(dict.lookup("colour").unwrap(), "<b>color</b>");
        assert!(dict.headwords().contains(&"colour".to_string())); // synonym listed.
    }

    #[test]
    fn ifo_parsing_skips_banner() {
        let ifo = "StarDict's dict ifo file\nversion=2.4.2\nbookname=My Dict\nsametypesequence=h\n";
        let map = parse_ifo(ifo);
        assert_eq!(map.get("bookname").unwrap(), "My Dict");
        assert_eq!(map.get("sametypesequence").unwrap(), "h");
        assert!(!map.contains_key("StarDict's dict ifo file"));
    }
}
