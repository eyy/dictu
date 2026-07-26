//! dictionary formats. each format lives in its own submodule and implements
//! the shared `Dictionary` trait below, so the ui can treat them uniformly.

pub mod csv;
pub mod dictd;
pub mod dsl;
pub mod markup;
pub mod stardict;

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::read::MultiGzDecoder;
use memmap2::Mmap;

/// a file's bytes: memory-mapped for plain files (paged in lazily by the OS — no
/// big up-front read), or owned when we had to gunzip a `.dz`, or when we built
/// the bytes ourselves (see `index_cache`).
pub(crate) enum DictBytes {
    Mapped(Mmap),
    Owned(Vec<u8>),
}

impl DictBytes {
    pub(crate) fn as_slice(&self) -> &[u8] {
        match self {
            DictBytes::Mapped(m) => m,
            DictBytes::Owned(v) => v,
        }
    }
}

/// load a `.dict` sibling: mmap it if plain, gunzip it into memory if `.dz` (or
/// a plain file that turns out to be gzip). `exts` are tried in order.
pub(crate) fn load_dict_bytes(dir: &Path, stem: &str, exts: &[&str]) -> Result<DictBytes> {
    for ext in exts {
        let path = dir.join(format!("{stem}.{ext}"));
        if !path.exists() {
            continue;
        }
        let map = mmap_file(&path)?;
        // a .dz (or a plain file with the gzip magic) must be decompressed; a
        // truly-plain .dict stays memory-mapped.
        let gzipped = ext.ends_with("dz") || ext.ends_with("gz") || map.starts_with(&[0x1f, 0x8b]);
        return if gzipped {
            Ok(DictBytes::Owned(gunzip_capped(&map)?))
        } else {
            Ok(DictBytes::Mapped(map))
        };
    }
    bail!("none of {exts:?} found for base {stem}");
}

/// the first of `exts` that exists next to `dir/stem` — which sibling
/// `load_dict_bytes` would pick, without reading it. the cache key needs to name
/// the file, not its contents.
pub(crate) fn sibling(dir: &Path, stem: &str, exts: &[&str]) -> Option<PathBuf> {
    exts.iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|path| path.exists())
}

/// every file a dictionary's index is derived from, plus the data file its byte
/// ranges point into. the cached index is only valid while all of them are
/// unchanged, so this is what the cache key is built over.
pub(crate) fn source_files(path: &Path) -> Vec<PathBuf> {
    match classify(path) {
        Some(Format::StarDict) => stardict::source_files(path),
        _ => vec![path.to_path_buf()],
    }
}

/// memory-map a file read-only.
pub(crate) fn mmap_file(path: &Path) -> Result<Mmap> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    // SAFETY: opened read-only and used as immutable bytes. the accepted mmap
    // caveat applies: if another process TRUNCATES this file in place while we
    // hold the map, reading a now-past-EOF page raises SIGBUS (a hard crash, not
    // a rust panic). we accept this — dictionaries are static assets; editors
    // and sync clients (incl. Dropbox) replace via atomic rename, which leaves
    // our map on the old inode with stale-but-valid bytes rather than faulting.
    // our own cache files, mapped through here too, are published the same way
    // (see `index_cache::store`), so the same reasoning covers them.
    #[allow(unsafe_code)]
    let map =
        unsafe { Mmap::map(&file) }.with_context(|| format!("mmapping {}", path.display()))?;
    Ok(map)
}

/// max bytes we'll decompress from a single gzip/dictzip stream — a sanity cap
/// against decompression bombs. real dictionaries are well under this (the
/// largest here is ~186 MB).
const MAX_DECOMPRESSED: u64 = 2 * 1024 * 1024 * 1024;

/// gunzip `raw` fully, refusing to expand past `MAX_DECOMPRESSED`. reading one
/// byte past the cap lets us tell "hit the cap" from "exactly the cap".
pub(crate) fn gunzip_capped(raw: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    // MultiGzDecoder (not GzDecoder) so a concatenated multi-member gzip is read
    // in full — GzDecoder stops after the first member, silently dropping data.
    MultiGzDecoder::new(raw)
        .take(MAX_DECOMPRESSED + 1)
        .read_to_end(&mut out)?;
    if out.len() as u64 > MAX_DECOMPRESSED {
        bail!("refusing to decompress past {MAX_DECOMPRESSED} bytes (possible zip bomb)");
    }
    Ok(out)
}

/// anything the ui can search and read definitions from.
///
/// a trait is rust's interface/typeclass: it names a set of methods a type must
/// provide. each format implements it, and the ui holds a `Box<dyn Dictionary>`
/// — a heap-allocated "some dictionary, we don't care which format" value.
///
/// the `Send` supertrait lets a `Box<dyn Dictionary>` cross to a worker thread,
/// so the (slow) index build can run off the gtk main thread. all our impls are
/// `Send` (they hold only String/Vec/HashMap/mmap, all Send).
pub trait Dictionary: Send {
    /// human-friendly name (from the file's metadata, else its filename).
    fn name(&self) -> &str;

    /// every headword, in stored order — used to populate the word list.
    fn headwords(&self) -> &[String];

    /// every entry filed under an exact headword, in stored order; empty when the
    /// headword isn't in this dictionary. by contract each is an **html fragment**
    /// — formats that store plain text should escape it, and formats with their own
    /// markup (dsl) convert to html — so one renderer handles every format.
    ///
    /// a headword can have many entries, and not just a handful: the inflected latin
    /// dictionary files `esse` under 100 of them, having inflected the auxiliary
    /// along with every verb that takes one. they come back separately rather than
    /// pre-joined so the ui can number them and set them apart, instead of showing a
    /// hundred entries as one run-together answer.
    fn lookup(&self, headword: &str) -> Vec<String>;

    /// whether the headword at `index` (into `headwords`) is an **alias** this
    /// dictionary points at one of its own entries, rather than a word it files in
    /// its own right. StarDict `.syn` records are the only such thing here, and
    /// three of the loaded dictionaries ship one: Whitaker's latin (1.18M forms of
    /// 37,777 words), a hebrew-hebrew dictionary (2.9 MB of hebrew plurals, construct forms and
    /// unpointed spellings — `כאבים` resolves to `כאב`'s entry, marked `מן כאב`),
    /// and Even Sapir (a single record). formats without that notion answer
    /// `false`, which is the honest default — a spelling variant on a dsl card is
    /// still a way of writing the headword, not a pointer at it.
    fn is_alias(&self, _index: usize) -> bool {
        false
    }
}

/// the dictionary file formats dictu knows about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    StarDict,
    Bgl,
    Dsl,
    Csv,
    Dictd,
}

impl Format {
    /// short human label for the ui.
    // nothing calls it since every format now has an `open_any` arm and the
    // "not supported yet" message it used to fill is gone; kept for the panel
    // that will show each dictionary's format.
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Format::StarDict => "StarDict",
            Format::Bgl => "Babylon",
            Format::Dsl => "Lingvo DSL",
            Format::Csv => "CSV",
            Format::Dictd => "dictd",
        }
    }

    /// whether `open_any` can actually read this format today. the directory
    /// scanner uses this to keep unsupported formats out of the picker (BGL is
    /// pre-converted to StarDict, so it is never opened directly).
    pub fn is_supported(self) -> bool {
        matches!(
            self,
            Format::StarDict | Format::Dictd | Format::Csv | Format::Dsl
        )
    }
}

/// classify a path as the *primary* file of a dictionary, or `None` for
/// auxiliary/unknown files (.idx, .dict, .syn, .ann, .bmp, …). used by the
/// directory scanner and by `open_any`.
pub fn classify(path: &Path) -> Option<Format> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".ifo") {
        Some(Format::StarDict)
    } else if name.ends_with(".bgl") {
        Some(Format::Bgl)
    } else if name.ends_with(".dsl") || name.ends_with(".dsl.dz") {
        Some(Format::Dsl)
    } else if name.ends_with(".csv") {
        Some(Format::Csv)
    } else if name.ends_with(".index") {
        Some(Format::Dictd)
    } else {
        None
    }
}

/// open whichever dictionary format `path` points at.
/// returns a boxed trait object so callers don't care about the concrete type.
///
/// `cache` is the directory holding the on-disk index cache (`index_cache`), or
/// `None` to parse the dictionary from scratch and cache nothing. only StarDict
/// uses it so far — it is the format whose `.idx` parse dominates startup; the
/// dictd `.index` and the LSJ csv are text files that parse in milliseconds.
pub fn open_any(path: &Path, cache: Option<&Path>) -> Result<Box<dyn Dictionary>> {
    match classify(path) {
        Some(Format::StarDict) => Ok(Box::new(stardict::StarDict::open(path, cache)?)),
        Some(Format::Dictd) => Ok(Box::new(dictd::DictdDictionary::open(path)?)),
        Some(Format::Csv) => Ok(Box::new(csv::CsvDictionary::open(path)?)),
        Some(Format::Dsl) => Ok(Box::new(dsl::DslDictionary::open(path, cache)?)),
        // bgl is pre-converted to StarDict offline rather than parsed in-app.
        Some(Format::Bgl) => {
            bail!("BGL isn’t read directly — convert it to StarDict with pyglossary first")
        }
        None => bail!("unrecognized dictionary file: {}", path.display()),
    }
}

/// the plain text of an html definition (for the `dump` cli and fallbacks).
/// the ui renders rich runs via `markup::to_runs` instead.
pub fn html_to_text(html: &str) -> String {
    markup::to_text(html)
}

#[cfg(test)]
mod tests {
    use super::html_to_text;

    #[test]
    fn html_to_text_handles_multibyte_and_entities() {
        // greek/hebrew text with a stray '&' and numeric/named entities — must
        // not panic, must decode entities, must keep the unicode.
        let input = "Ελληνικά &amp; R&D &#945; &#x3b2;";
        let out = html_to_text(input);
        assert!(out.contains("Ελληνικά"));
        assert!(out.contains('&')); // &amp; and R&D both yield '&'.
        assert!(out.contains('α') && out.contains('β')); // numeric entities.
    }

    #[test]
    fn html_to_text_drops_style_keeps_content() {
        let out = html_to_text("<style>a{color:blue}</style><b>hi</b> there");
        assert!(out.contains("hi") && out.contains("there"));
        assert!(!out.contains("color")); // <style> body must not leak.
    }
}
