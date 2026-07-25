//! reader for the StarDict format: a set of sibling files sharing a base name:
//!   - `.ifo`  — text metadata (`key=value` lines).
//!   - `.idx`  — binary index: repeated `word\0` + u32 offset + u32 size.
//!   - `.dict` / `.dict.dz` — the definition data (optionally gzip/dictzip).
//!   - `.syn`  — optional synonyms: `word\0` + u32 index-into-.idx.
//!
//! all integers in `.idx`/`.syn` are big-endian. offsets/sizes point into the
//! *decompressed* `.dict` data.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::index_cache::{self, Index};

use super::{DictBytes, Dictionary, load_dict_bytes, sibling};

/// a byte range `(offset, size)` into the decompressed `.dict` data.
type Range = index_cache::Range;
/// every `(headword, range)` pair the files hold, in stored order — `.idx` first,
/// then whatever the `.syn` adds. words borrow from the mapped files where they
/// are valid utf-8, which is nearly always, so a 1.4M-entry `.idx` allocates
/// almost nothing while being read.
type Entries<'a> = Vec<(Cow<'a, str>, Range)>;

pub struct StarDict {
    name: String,
    headwords: Vec<String>,
    /// how each entry's data block is typed (the `.ifo` `sametypesequence`).
    /// `Some("h")` = one html field; `None` = each field is type-prefixed.
    same_type_sequence: Option<String>,
    /// headword -> byte ranges in `data`, read off the memory-mapped cache image
    /// rather than held as a `HashMap` (roadmap #7): building that map was 5.5 s
    /// of a 16 s startup on the biggest dictionary here.
    index: Index,
    /// the `.dict` bytes, memory-mapped (lazy) rather than read into RAM.
    data: DictBytes,
}

/// the files a StarDict's cached index depends on: the `.ifo` it was described
/// by, the `.idx`/`.syn` it was built from, and the `.dict` its byte ranges point
/// into — a new `.dict` under an unchanged `.idx` would make those ranges lie.
pub(crate) fn source_files(ifo_path: &Path) -> Vec<PathBuf> {
    let dir = ifo_path.parent().unwrap_or_else(|| Path::new("."));
    let Some(stem) = ifo_path.file_stem().and_then(|s| s.to_str()) else {
        return vec![ifo_path.to_path_buf()];
    };
    let mut files = vec![ifo_path.to_path_buf()];
    files.extend(sibling(dir, stem, &["idx", "idx.gz"]));
    files.extend(sibling(dir, stem, &["dict", "dict.dz"]));
    files.extend(sibling(dir, stem, &["syn"]));
    files
}

impl StarDict {
    /// open a dictionary given the path to its `.ifo` file. `cache` is the
    /// directory holding cached indexes; with `Some`, a matching cache is mapped
    /// instead of parsing the `.idx`, and a fresh one is written when there isn't.
    pub fn open(ifo_path: &Path, cache: Option<&Path>) -> Result<Self> {
        let ifo_text = fs::read_to_string(ifo_path)
            .with_context(|| format!("reading {}", ifo_path.display()))?;
        let ifo = parse_ifo(&ifo_text);

        let dir = ifo_path.parent().unwrap_or_else(|| Path::new("."));
        let stem = ifo_path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("ifo path has no usable stem")?;

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

        // the .ifo is cheap to re-read every launch (~50µs), so only the .idx
        // work is cached — keyed by every file it was derived from.
        let fingerprint = index_cache::fingerprint(&source_files(ifo_path));
        let cache_file = cache.map(|dir| index_cache::index_path(dir, ifo_path));
        let cached = cache_file
            .as_deref()
            .and_then(|path| Index::load(path, &fingerprint));

        let index = match cached {
            Some(index) => index,
            None => Self::build_index(dir, stem, offset_bits, cache_file.as_deref(), &fingerprint)?,
        };

        Ok(Self {
            name,
            headwords: index.headwords(),
            same_type_sequence,
            index,
            data,
        })
    }

    /// parse the `.idx` (+ `.syn`) and lay the result out as a cache image, which
    /// is then published and mapped back — so a cold start and a warm one hold the
    /// very same bytes and there is only one lookup path to get right. a cache
    /// that can't be written is kept in memory instead; nothing here fails a load.
    fn build_index(
        dir: &Path,
        stem: &str,
        offset_bits: u8,
        cache_file: Option<&Path>,
        fingerprint: &str,
    ) -> Result<Index> {
        // the .idx (possibly gzipped as .idx.gz). we read it via the shared
        // loader too, then parse its bytes — no need to hold it after open().
        let idx =
            load_dict_bytes(dir, stem, &["idx", "idx.gz"]).context("locating/reading .idx")?;

        // `.syn` records index into the raw `.idx` order, so its bytes have to
        // outlive the parse alongside the `.idx`'s (both are borrowed from).
        let syn_path = dir.join(format!("{stem}.syn"));
        let syn = match syn_path.exists() {
            true => Some(
                fs::read(&syn_path).with_context(|| format!("reading {}", syn_path.display()))?,
            ),
            false => None,
        };

        let (mut entries, mut display) = parse_idx(idx.as_slice(), offset_bits);
        if let Some(syn) = syn.as_deref() {
            apply_syn(syn, &mut entries, &mut display);
        }
        let image = index_cache::build(&entries, &display, fingerprint);

        // best effort: a read-only or full cache directory costs speed, not
        // correctness.
        let bytes = match cache_file.map(|path| index_cache::store(path, &image)) {
            Some(Ok(mapped)) => mapped,
            Some(Err(err)) => {
                eprintln!("dictu: not caching the index for {stem}: {err:#}");
                DictBytes::Owned(image)
            }
            None => DictBytes::Owned(image),
        };
        Index::open(bytes, fingerprint).context("reading back the index we just built")
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

    fn lookup(&self, headword: &str) -> Vec<String> {
        self.index
            .ranges(headword)
            .into_iter()
            .flatten()
            .filter_map(|(offset, size)| self.entry_text(offset, size))
            .filter(|entry| !entry.is_empty())
            .collect()
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

/// parse the `.idx` into every `(headword, range)` pair in file order, plus which
/// of those entries the ui lists — the `##...` metadata pseudo-entries some
/// converters emit are dropped here, and repeated words are deduped later, by
/// `index_cache::build`, which is where the key table exists.
fn parse_idx(idx: &[u8], offset_bits: u8) -> (Entries<'_>, Vec<u32>) {
    let mut entries: Entries = Vec::new();
    let mut display: Vec<u32> = Vec::new();

    let int_bytes = if offset_bits == 64 { 8 } else { 4 };
    let mut pos = 0;
    while pos < idx.len() {
        // headword: bytes up to the next NUL. if the file is truncated mid-entry
        // we stop cleanly, keeping what parsed, rather than failing the load.
        let Some(nul) = idx[pos..].iter().position(|&b| b == 0) else {
            break;
        };
        let word = String::from_utf8_lossy(&idx[pos..pos + nul]);
        pos += nul + 1;

        // then offset (32 or 64 bit) and size (always 32 bit), big-endian.
        let Some(offset) = read_be_uint(idx, pos, int_bytes) else {
            break;
        };
        pos += int_bytes;
        let Some(size) = read_be_uint(idx, pos, 4) else {
            break;
        };
        pos += 4;

        if !word.starts_with("##") {
            display.push(entries.len() as u32);
        }
        entries.push((word, (offset, size as u32)));
    }

    (entries, display)
}

/// merge a `.syn` file: each record is `synonym\0` + u32 index into the RAW
/// `.idx` order. the synonym resolves to that entry's data and joins the list.
fn apply_syn<'a>(syn: &'a [u8], entries: &mut Entries<'a>, display: &mut Vec<u32>) {
    // the .syn indexes into the raw .idx order, NOT the filtered/deduped display
    // list, whose positions are shifted by skipped `##` / duplicate entries. so
    // resolve against the entries that were there before this function appends.
    let raw = entries.len();
    let mut pos = 0;
    while pos < syn.len() {
        let Some(nul) = syn[pos..].iter().position(|&b| b == 0) else {
            break;
        };
        let word = String::from_utf8_lossy(&syn[pos..pos + nul]);
        pos += nul + 1;
        let Some(entry_index) = read_be_uint(syn, pos, 4) else {
            break;
        };
        pos += 4;

        let entry_index = entry_index as usize;
        if entry_index >= raw {
            continue; // out of range: a corrupt .syn record, skipped.
        }
        let range = entries[entry_index].1;
        if !word.starts_with("##") {
            display.push(entries.len() as u32);
        }
        entries.push((word, range));
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

    use std::os::unix::fs::MetadataExt;

    /// one `.idx` record: `word\0` + u32BE offset + u32BE size.
    fn push_idx(idx: &mut Vec<u8>, word: &str, offset: u32, size: u32) {
        idx.extend_from_slice(word.as_bytes());
        idx.push(0);
        idx.extend_from_slice(&offset.to_be_bytes());
        idx.extend_from_slice(&size.to_be_bytes());
    }

    /// one `.syn` record: `synonym\0` + u32BE index into the raw `.idx` order.
    fn push_syn(syn: &mut Vec<u8>, word: &str, raw_index: u32) {
        syn.extend_from_slice(word.as_bytes());
        syn.push(0);
        syn.extend_from_slice(&raw_index.to_be_bytes());
    }

    /// a fresh temp directory of its own, so tests can run in parallel.
    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dictu-sd-{}-{test}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// write a real StarDict on disk (the only way to exercise `open`, which is
    /// where the cache lives) and return its `.ifo` path.
    fn write_dict(dir: &Path, idx: &[u8], data: &[u8], syn: Option<&[u8]>) -> PathBuf {
        let ifo = dir.join("t.ifo");
        fs::write(
            &ifo,
            "StarDict's dict ifo file\nbookname=T\nsametypesequence=h\n",
        )
        .unwrap();
        fs::write(dir.join("t.idx"), idx).unwrap();
        fs::write(dir.join("t.dict"), data).unwrap();
        if let Some(syn) = syn {
            fs::write(dir.join("t.syn"), syn).unwrap();
        }
        ifo
    }

    /// the two-word fixture: sametypesequence=h, so each block is one html field.
    ///   "alpha" -> "<b>alpha</b>" at offset 0, size 12
    ///   "beta"  -> "<i>beta</i>"  at offset 12, size 11
    fn two_words(dir: &Path) -> PathBuf {
        let mut idx = Vec::new();
        push_idx(&mut idx, "alpha", 0, 12);
        push_idx(&mut idx, "beta", 12, 11);
        write_dict(dir, &idx, b"<b>alpha</b><i>beta</i>", None)
    }

    #[test]
    fn parses_idx_and_looks_up_html() {
        let dir = temp_dir("plain");
        let dict = StarDict::open(&two_words(&dir), None).unwrap();

        assert_eq!(dict.name(), "T");
        assert_eq!(dict.headwords(), &["alpha".to_string(), "beta".to_string()]);
        assert_eq!(dict.lookup("alpha"), ["<b>alpha</b>"]);
        assert_eq!(dict.lookup("beta"), ["<i>beta</i>"]);
        assert!(dict.lookup("gamma").is_empty());
        // no cache directory was named, so nothing was written next to the dict.
        assert!(!dir.join("t.didx").exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn synonyms_resolve_against_raw_idx_order() {
        // a `##` metadata entry FIRST makes the filtered display list and the raw
        // .idx order diverge: raw index 1 = "color", but display index 1 is OOB.
        // the .syn index follows the RAW order, which is the bug we fixed.
        let dir = temp_dir("syn");
        let mut idx = Vec::new();
        push_idx(&mut idx, "##name", 0, 4); // "META" = raw 0
        push_idx(&mut idx, "color", 4, 12); // "<b>color</b>" = raw 1
        let mut syn = Vec::new();
        push_syn(&mut syn, "colour", 1);
        push_syn(&mut syn, "nonesuch", 99); // out of range: skipped, not fatal.
        let ifo = write_dict(&dir, &idx, b"META<b>color</b>", Some(&syn));

        let dict = StarDict::open(&ifo, None).unwrap();
        // ## hidden from the display list, synonym appended after the real words.
        assert_eq!(dict.headwords(), &["color".to_string(), "colour".into()]);
        assert_eq!(dict.lookup("colour"), ["<b>color</b>"]);
        assert_eq!(dict.lookup("color"), ["<b>color</b>"]);
        // the `##` entry is still resolvable, it is only hidden from the list.
        assert_eq!(dict.lookup("##name"), ["META"]);
        assert!(dict.lookup("nonesuch").is_empty());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_cache_is_written_once_and_reused() {
        let dir = temp_dir("hit");
        let cache = dir.join("cache");
        let ifo = two_words(&dir);
        let cache_file = index_cache::index_path(&cache, &ifo);

        let cold = StarDict::open(&ifo, Some(&cache)).unwrap();
        let written = fs::metadata(&cache_file).expect("a cache file").ino();

        // second open: same answers, and the cache file was not rewritten (a
        // rebuild publishes by rename, which would give a different inode).
        let warm = StarDict::open(&ifo, Some(&cache)).unwrap();
        assert_eq!(warm.headwords(), cold.headwords());
        assert_eq!(warm.lookup("alpha"), cold.lookup("alpha"));
        assert_eq!(warm.lookup("beta"), ["<i>beta</i>"]);
        assert!(warm.lookup("gamma").is_empty());
        assert_eq!(fs::metadata(&cache_file).unwrap().ino(), written);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_changed_dictionary_rebuilds_its_cache() {
        let dir = temp_dir("stale");
        let cache = dir.join("cache");
        let ifo = two_words(&dir);
        let cache_file = index_cache::index_path(&cache, &ifo);

        StarDict::open(&ifo, Some(&cache)).unwrap();
        let first = fs::metadata(&cache_file).unwrap().ino();

        // a third word: the .idx grows, so its size and mtime both change.
        let mut idx = Vec::new();
        push_idx(&mut idx, "alpha", 0, 12);
        push_idx(&mut idx, "beta", 12, 11);
        push_idx(&mut idx, "gamma", 23, 12);
        fs::write(dir.join("t.idx"), &idx).unwrap();
        fs::write(dir.join("t.dict"), b"<b>alpha</b><i>beta</i><u>gamma</u>").unwrap();

        let rebuilt = StarDict::open(&ifo, Some(&cache)).unwrap();
        assert_eq!(rebuilt.headwords().len(), 3);
        assert_eq!(rebuilt.lookup("gamma"), ["<u>gamma</u>"]);
        assert_ne!(
            fs::metadata(&cache_file).unwrap().ino(),
            first,
            "a changed .idx must be re-indexed, not answered from the old cache"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_cache_falls_back_to_the_idx() {
        let dir = temp_dir("corrupt");
        let cache = dir.join("cache");
        let ifo = two_words(&dir);
        let cache_file = index_cache::index_path(&cache, &ifo);
        StarDict::open(&ifo, Some(&cache)).unwrap();

        // every way a cache file can be unusable, one after another: truncated,
        // emptied, and filled with something else entirely.
        let good = fs::read(&cache_file).unwrap();
        for broken in [
            good[..good.len() / 2].to_vec(),
            Vec::new(),
            b"garbage".to_vec(),
        ] {
            fs::write(&cache_file, &broken).unwrap();
            let dict = StarDict::open(&ifo, Some(&cache)).unwrap();
            assert_eq!(dict.headwords(), &["alpha".to_string(), "beta".into()]);
            assert_eq!(dict.lookup("beta"), ["<i>beta</i>"]);
            // and it was replaced with a usable one rather than left broken.
            assert_eq!(fs::read(&cache_file).unwrap(), good);
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unwritable_cache_directory_still_opens_the_dictionary() {
        let dir = temp_dir("nocache");
        let ifo = two_words(&dir);
        // a *file* where the cache directory should be: create_dir_all fails, so
        // storing fails, and the freshly built index has to be used in memory.
        let blocked = dir.join("blocked");
        fs::write(&blocked, b"not a directory").unwrap();

        let dict = StarDict::open(&ifo, Some(&blocked)).unwrap();
        assert_eq!(dict.headwords(), &["alpha".to_string(), "beta".into()]);
        assert_eq!(dict.lookup("alpha"), ["<b>alpha</b>"]);

        fs::remove_dir_all(&dir).ok();
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
