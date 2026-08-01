//! the on-disk index cache: what a launch would otherwise re-derive from every
//! dictionary's `.idx`, laid out so the next launch can memory-map it instead of
//! parsing anything (roadmap #7).
//!
//! two kinds of file live under `$XDG_CACHE_HOME/dictu/`:
//!
//! - one **index** per dictionary (`<name>-<hash>.didx`): its `headword -> byte
//!   ranges` map and its display headword list. this is the expensive half —
//!   parsing the 28 MB Latin `.idx` into a `HashMap<String, Vec<Range>>` cost
//!   7.4 s of a 16 s startup, and 5.5 s of that was building the map itself.
//! - one **merged order** file (`merged.dord`): the cross-dictionary sort order
//!   (`library`), which cost a further 4.4 s. it now sorts by a normalized key
//!   (`keys`) and carries that key beside the order, because deriving it per
//!   comparison is what the old `to_lowercase()` sort did, and normalizing costs
//!   far more than lowercasing (roadmap #39).
//!
//! an index can also carry a **payload**: the bytes its ranges point into, for a
//! format whose data doesn't already sit on disk in a mappable shape. StarDict
//! leaves it empty and maps its own `.dict`; DSL puts its decoded text there,
//! because decoding utf-16 is most of what opening a DSL dictionary costs (2.6 s
//! of 4.3 s across the nine here) and the decoded text is what its ranges index.
//!
//! every file carries a fingerprint of the source files it was derived from
//! (path + mtime + size of each), and is used only when that still matches. a
//! missing, stale, truncated or otherwise unreadable file is simply not used —
//! the caller rebuilds. nothing here returns an error a launch has to care about.
//!
//! **why a sorted blob and not an `fst`** (the roadmap said "fst disk cache"):
//! measured on the 1,223,585-headword Latin dictionary, debug build — an
//! `fst::Map` of its keys is 474 KB against this blob's 40.7 MB, but the fst can
//! only carry `key -> u64`, so the 22 MB of byte-range side tables remain either
//! way (22.5 MB against 40.7 MB all told); it takes 1.30 s to build against
//! 0.33 s; and 20k lookups cost the same (89 ms against 100 ms). its one real
//! advantage, prefix search off the map, doesn't apply: dictu's prefix search is
//! normalized and spans dictionaries, so it runs on the merged order, never on
//! one dictionary's exact-byte map. 18 MB of a memory-mapped cache file is not
//! worth a dependency and a 4x slower rebuild.

use std::borrow::Cow;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};

use crate::dict::{DictBytes, mmap_file};
use crate::keys::KeyTable;

/// a byte range `(offset, size)` into a dictionary's data.
pub type Range = (u64, u32);

/// bump on any layout change — old files then fail the header check and are
/// rebuilt rather than misread. **3**: the merged order is sorted by the bare
/// key and carries it (#39), so a v2 file is sorted by a key nothing searches
/// with any more — the one kind of staleness a warm cache would answer wrongly
/// rather than not at all. **4**: that key now also drops a trailing homograph
/// number (#43), which reorders it again. **5**: the per-dictionary index carries
/// one more count — how many of its display headwords are lemmas rather than
/// `.syn` aliases (#33). **6**: a dsl headword's optional `(…)` groups no longer all
/// become listed words — a *leading* one is a marker rather than the word (#72) — so a
/// v5 index lists headwords this one does not, which is staleness a warm cache would
/// answer with confidently.
const VERSION: u32 = 6;

const INDEX_MAGIC: &[u8; 8] = b"DICTUIDX";
const INDEX_HEADER: usize = 112;
const ORDER_MAGIC: &[u8; 8] = b"DICTUMRG";
const ORDER_HEADER: usize = 52;

/// identity of the files an index was derived from: path, mtime and size of
/// each. any of them changing (or vanishing) changes this string, which is what
/// makes a replaced dictionary rebuild instead of being answered from a cache.
pub fn fingerprint(sources: &[PathBuf]) -> String {
    let mut out = format!("v{VERSION}");
    for path in sources {
        out.push('|');
        out.push_str(&path.to_string_lossy());
        let Ok(meta) = fs::metadata(path) else {
            out.push_str(":missing");
            continue;
        };
        // a pre-epoch mtime can't be expressed as a duration; the size still
        // discriminates, and such a file is pathological anyway.
        match meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        {
            Some(age) => out.push_str(&format!(
                ":{}.{:09}:{}",
                age.as_secs(),
                age.subsec_nanos(),
                meta.len()
            )),
            None => out.push_str(&format!(":?:{}", meta.len())),
        }
        // mtime and size alone have a blind spot: a tool that deliberately
        // preserves the timestamp — `cp -p`, `rsync --times`, a dropbox sync
        // restoring an older version — can change the content and leave both
        // unchanged, and the cache then answers from a stale index with no sign
        // that anything is wrong. this collection lives in dropbox, so that is a
        // real path rather than a hypothetical. sampling three windows of the
        // content closes it for anything but a change that lands entirely
        // outside them, and costs ~192 KiB of reads per file.
        out.push(':');
        out.push_str(&sample_hash(path, meta.len()));
    }
    out
}

/// a cheap content fingerprint: fnv-1a over the first, middle and last window of
/// the file. deliberately not a full hash — the largest dictionary here is 186 MB
/// and warm startup is the whole point of this cache — so it trades certainty for
/// ~1 ms, catching any edit that touches a sampled window.
fn sample_hash(path: &Path, len: u64) -> String {
    const WINDOW: u64 = 64 * 1024;
    let Ok(mut file) = fs::File::open(path) else {
        return "unreadable".to_string();
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut buf = vec![0u8; WINDOW as usize];
    for start in [
        0,
        len.saturating_sub(WINDOW) / 2,
        len.saturating_sub(WINDOW),
    ] {
        if file.seek(SeekFrom::Start(start)).is_err() {
            continue;
        }
        let mut filled = 0;
        while filled < buf.len() {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(_) => break,
            }
        }
        for byte in &buf[..filled] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// where a dictionary's cached index goes: a readable stem so the directory can
/// be understood by eye, plus a hash of the full path so two dictionaries with
/// the same file name never collide.
pub fn index_path(dir: &Path, source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("dict");
    let readable: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .take(40)
        .collect();
    let hash = hash64(source.to_string_lossy().as_bytes());
    dir.join(format!("{readable}-{hash:016x}.didx"))
}

/// where the merged cross-dictionary order goes. one file, overwritten whenever
/// the collection changes, so it can't accumulate stale variants.
pub fn order_path(dir: &Path) -> PathBuf {
    dir.join("merged.dord")
}

// -- per-dictionary index --------------------------------------------------

/// what a format hands the cache. `entries` is every `(headword, range)` pair in
/// the order the format stored them; `display` names the entries the ui lists, by
/// index into `entries`, in display order (the caller has already dropped
/// whatever its format considers non-words). `name` and `payload` are for formats
/// that only learn their own name by parsing, and whose ranges point into bytes
/// that aren't a file we could map — both default to empty.
#[derive(Default)]
pub struct Built<'a> {
    pub entries: &'a [(Cow<'a, str>, Range)],
    pub display: &'a [u32],
    pub name: &'a str,
    pub payload: &'a [u8],
    /// where a format's *aliases* begin in `entries` — StarDict appends its
    /// `.syn` records after the `.idx` ones, and those records are where a
    /// dictionary files its inflected forms (Whitaker: 1.18M latin forms; 
    /// a hebrew-hebrew dictionary: hebrew plurals and construct forms). `None` means every entry is a
    /// headword in its own right, which is true of every other format here.
    pub aliases_from: Option<usize>,
}

/// lay an index out as the bytes of a cache file. the first occurrence of each
/// distinct headword in `display` is what survives — dedup happens here because
/// this is where the key table exists.
pub fn build(source: Built<'_>, fingerprint: &str) -> Vec<u8> {
    let Built {
        entries,
        display,
        name,
        payload,
        aliases_from,
    } = source;
    // sort entry indices by headword. byte order and `str` order agree, so the
    // reader can binary-search the blob without decoding utf-8.
    let mut order: Vec<u32> = (0..entries.len() as u32).collect();
    order.sort_by(|&a, &b| entries[a as usize].0.cmp(&entries[b as usize].0));

    // the distinct keys, in sorted order, each with the ranges of every entry
    // that shares it (a word can have several sense entries, and a synonym adds
    // one more). `key_of` maps an entry back to its key, which is how `display`
    // gets deduped without a second hash set.
    let mut keys: Vec<&str> = Vec::new();
    let mut range_start: Vec<u32> = Vec::new();
    let mut ranges: Vec<Range> = Vec::with_capacity(entries.len());
    let mut key_of: Vec<u32> = vec![0; entries.len()];
    for &entry in &order {
        let (word, range) = &entries[entry as usize];
        if keys.last() != Some(&word.as_ref()) {
            keys.push(word.as_ref());
            range_start.push(ranges.len() as u32);
        }
        key_of[entry as usize] = (keys.len() - 1) as u32;
        ranges.push(*range);
    }
    // range_start[i] opens key i's run of ranges; one more entry closes the last.
    range_start.push(ranges.len() as u32);

    // the display list keeps the first appearance of each key, and a format lists
    // its own headwords before any alias — so the survivors are partitioned, and
    // one count says where the aliases start. a word that is *both* (filed and
    // pointed at) keeps its earlier, lemma position, which is the right answer.
    let aliases_from = aliases_from.unwrap_or(entries.len()) as u32;
    let mut seen = vec![false; keys.len()];
    let mut lemmas = 0usize;
    let display: Vec<u32> = display
        .iter()
        .filter_map(|&entry| {
            let key = *key_of.get(entry as usize)?;
            let first = !std::mem::replace(&mut seen[key as usize], true);
            if first && entry < aliases_from {
                lemmas += 1;
            }
            first.then_some(key)
        })
        .collect();

    // key offsets into the blob, then the blob itself.
    let mut key_off: Vec<u32> = Vec::with_capacity(keys.len() + 1);
    let mut blob: Vec<u8> = Vec::new();
    key_off.push(0);
    for key in &keys {
        blob.extend_from_slice(key.as_bytes());
        key_off.push(blob.len() as u32);
    }

    let fp = fingerprint.as_bytes();
    let key_off_at = INDEX_HEADER + fp.len();
    let blob_at = key_off_at + key_off.len() * 4;
    let range_off_at = blob_at + blob.len();
    let ranges_at = range_off_at + range_start.len() * 4;
    let display_at = ranges_at + ranges.len() * 12;
    let name_at = display_at + display.len() * 4;
    let payload_at = name_at + name.len();

    let mut out = Vec::with_capacity(payload_at + payload.len());
    out.extend_from_slice(INDEX_MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    out.extend_from_slice(&(ranges.len() as u32).to_le_bytes());
    out.extend_from_slice(&(display.len() as u32).to_le_bytes());
    out.extend_from_slice(&(lemmas as u32).to_le_bytes());
    out.extend_from_slice(&(fp.len() as u32).to_le_bytes());
    for at in [
        key_off_at,
        blob_at,
        blob.len(),
        range_off_at,
        ranges_at,
        display_at,
        name_at,
        name.len(),
        payload_at,
        payload.len(),
    ] {
        out.extend_from_slice(&(at as u64).to_le_bytes());
    }
    debug_assert_eq!(out.len(), INDEX_HEADER);
    out.extend_from_slice(fp);
    for v in &key_off {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&blob);
    for v in &range_start {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for &(offset, size) in &ranges {
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
    }
    for v in &display {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(payload);
    out
}

/// one dictionary's `headword -> ranges` index, read straight off the cache
/// image: the words and their ranges stay in the mapped file, so opening a
/// dictionary costs an `mmap` rather than millions of allocations.
pub struct Index {
    bytes: DictBytes,
    n_keys: usize,
    n_ranges: usize,
    n_display: usize,
    /// how many of the display headwords are the dictionary's own, rather than
    /// aliases it points at them; see `Built::aliases_from`.
    n_lemmas: usize,
    key_off_at: usize,
    blob_at: usize,
    blob_len: usize,
    range_off_at: usize,
    ranges_at: usize,
    display_at: usize,
    name: std::ops::Range<usize>,
    payload: std::ops::Range<usize>,
}

impl Index {
    /// memory-map `path` and accept it only if it is this exact source's index.
    /// `None` for anything else — absent, stale, truncated, a future version.
    pub fn load(path: &Path, fingerprint: &str) -> Option<Self> {
        let bytes = mmap_file(path).ok()?;
        Self::open(DictBytes::Mapped(bytes), fingerprint)
    }

    /// adopt an image already in memory — the freshly built one, when writing it
    /// out failed. the same validation runs, so both paths behave identically.
    pub fn open(bytes: DictBytes, fingerprint: &str) -> Option<Self> {
        let raw = bytes.as_slice();
        if raw.len() < INDEX_HEADER || &raw[..8] != INDEX_MAGIC {
            return None;
        }
        if u32_le(raw, 8)? != VERSION {
            return None;
        }
        let n_keys = u32_le(raw, 12)? as usize;
        let n_ranges = u32_le(raw, 16)? as usize;
        let n_display = u32_le(raw, 20)? as usize;
        let n_lemmas = u32_le(raw, 24)? as usize;
        // a count that cannot be true of this file is refused here rather than
        // quietly hiding a whole dictionary from the fold-forms filter.
        if n_lemmas > n_display {
            return None;
        }
        let fp_len = u32_le(raw, 28)? as usize;
        if raw.get(INDEX_HEADER..INDEX_HEADER.checked_add(fp_len)?)? != fingerprint.as_bytes() {
            return None; // a cache of some other state of these files.
        }

        let key_off_at = offset_le(raw, 32)?;
        let blob_at = offset_le(raw, 40)?;
        let blob_len = offset_le(raw, 48)?;
        let range_off_at = offset_le(raw, 56)?;
        let ranges_at = offset_le(raw, 64)?;
        let display_at = offset_le(raw, 72)?;
        let name_at = offset_le(raw, 80)?;
        let name_len = offset_le(raw, 88)?;
        let payload_at = offset_le(raw, 96)?;
        let payload_len = offset_le(raw, 104)?;

        // every section must lie inside the file: a truncated cache is rejected
        // here rather than read past its end later.
        let offsets = n_keys.checked_add(1)?.checked_mul(4)?;
        for (at, len) in [
            (key_off_at, offsets),
            (blob_at, blob_len),
            (range_off_at, offsets),
            (ranges_at, n_ranges.checked_mul(12)?),
            (display_at, n_display.checked_mul(4)?),
            (name_at, name_len),
            (payload_at, payload_len),
        ] {
            if at.checked_add(len)? > raw.len() {
                return None;
            }
        }

        Some(Self {
            n_lemmas,
            n_keys,
            n_ranges,
            n_display,
            key_off_at,
            blob_at,
            blob_len,
            range_off_at,
            ranges_at,
            display_at,
            name: name_at..name_at + name_len,
            payload: payload_at..payload_at + payload_len,
            bytes,
        })
    }

    /// the name the format found for itself while parsing, or empty when the
    /// format reads its name from somewhere cheap (StarDict's `.ifo`).
    pub fn name(&self) -> &str {
        std::str::from_utf8(&self.bytes.as_slice()[self.name.clone()]).unwrap_or_default()
    }

    /// the bytes the ranges point into, for a format that stored them here. empty
    /// when the format maps its own data file instead.
    pub fn payload(&self) -> &[u8] {
        &self.bytes.as_slice()[self.payload.clone()]
    }

    /// whether the display headword at `index` is an alias the dictionary points
    /// at one of its own entries — an inflected form, in the three dictionaries
    /// here that ship a `.syn`. they sit after the dictionary's own headwords in
    /// display order, so this is a comparison rather than a lookup.
    pub fn is_alias(&self, index: usize) -> bool {
        index >= self.n_lemmas
    }

    /// the display headwords, in display order — the one part a caller has to
    /// materialize, because `Dictionary::headwords` hands out a `&[String]`.
    pub fn headwords(&self) -> Vec<String> {
        (0..self.n_display)
            .map(|i| match self.display_key(i) {
                Some(key) => String::from_utf8_lossy(self.key(key)).into_owned(),
                // an unreadable slot yields an empty word rather than a plausible
                // wrong one, and the count is kept: the merged order indexes into
                // this list, so its length has to hold.
                None => String::new(),
            })
            .collect()
    }

    /// the byte ranges holding an exact headword's definition(s), by binary
    /// search over the sorted key blob.
    pub fn ranges(&self, headword: &str) -> Option<Vec<Range>> {
        let needle = headword.as_bytes();
        let (mut lo, mut hi) = (0, self.n_keys);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.key(mid) < needle {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= self.n_keys || self.key(lo) != needle {
            return None;
        }
        let from = self.range_bound(lo)? as usize;
        let to = self.range_bound(lo + 1)? as usize;
        Some(
            (from..to.min(self.n_ranges))
                .filter_map(|i| self.range(i))
                .collect(),
        )
    }

    fn key(&self, i: usize) -> &[u8] {
        let raw = self.bytes.as_slice();
        let bound = |i: usize| u32_le(raw, self.key_off_at + i * 4).map(|v| v as usize);
        let (Some(from), Some(to)) = (bound(i), bound(i + 1)) else {
            return &[];
        };
        // offsets come out of the file, so they are checked, not trusted: an
        // out-of-order pair yields an empty key rather than a panic.
        if from > to || to > self.blob_len {
            return &[];
        }
        raw.get(self.blob_at + from..self.blob_at + to)
            .unwrap_or(&[])
    }

    fn range_bound(&self, i: usize) -> Option<u32> {
        u32_le(self.bytes.as_slice(), self.range_off_at + i * 4)
    }

    fn range(&self, i: usize) -> Option<Range> {
        let at = self.ranges_at + i * 12;
        let raw = self.bytes.as_slice();
        Some((u64_le(raw, at)?, u32_le(raw, at + 8)?))
    }

    fn display_key(&self, i: usize) -> Option<usize> {
        let key = u32_le(self.bytes.as_slice(), self.display_at + i * 4)? as usize;
        (key < self.n_keys).then_some(key)
    }
}

// -- merged cross-dictionary order -----------------------------------------

/// the merged cross-dictionary order: every headword's **slot** — its position
/// in the dictionaries' headword lists laid end to end — sorted by its bare key
/// (`keys`), and the keys themselves.
///
/// only the bare key is stored. it is the one the binary search compares, and it
/// is the permissive one: an unpointed query reaches every pointed spelling.
/// telling those spellings apart again is `keys::marks_allow`, which runs over
/// the candidates a prefix run turns up — a few hundred per keystroke — so the
/// fold key it needs is derived there rather than carried here for 1.8M words.
///
/// the keys are stored **in sorted order**, not slot order, so a binary search
/// reads position `mid`'s key straight out of the blob with no slot indirection,
/// and the prefix walk that follows is sequential in the mapped file. everything
/// is read a field at a time: an mmap can't be a `&[u32]` without unsafe
/// alignment games, and a binary search only touches ~21 positions anyway.
pub struct Order {
    bytes: DictBytes,
    n: usize,
    order_at: usize,
    off_at: usize,
    blob_at: usize,
    blob_len: usize,
}

impl Order {
    /// memory-map the merged order, if it is this collection's.
    pub fn load(path: &Path, fingerprint: &str) -> Option<Self> {
        let bytes = DictBytes::Mapped(mmap_file(path).ok()?);
        Self::open(bytes, fingerprint)
    }

    /// adopt an image already in memory — the freshly built one, when writing it
    /// out failed. the same validation runs, so both paths behave identically.
    pub fn open(bytes: DictBytes, fingerprint: &str) -> Option<Self> {
        let raw = bytes.as_slice();
        if raw.len() < ORDER_HEADER || &raw[..8] != ORDER_MAGIC || u32_le(raw, 8)? != VERSION {
            return None;
        }
        let n = u32_le(raw, 12)? as usize;
        let fp_len = u32_le(raw, 16)? as usize;
        if raw.get(ORDER_HEADER..ORDER_HEADER.checked_add(fp_len)?)? != fingerprint.as_bytes() {
            return None;
        }
        let order_at = offset_le(raw, 20)?;
        let off_at = offset_le(raw, 28)?;
        let blob_at = offset_le(raw, 36)?;
        let blob_len = offset_le(raw, 44)?;

        // every section must lie inside the file, so a truncated order is
        // refused here rather than read past its end later.
        for (at, len) in [
            (order_at, n.checked_mul(4)?),
            (off_at, n.checked_add(1)?.checked_mul(4)?),
            (blob_at, blob_len),
        ] {
            if at.checked_add(len)? > raw.len() {
                return None;
            }
        }
        Some(Self {
            bytes,
            n,
            order_at,
            off_at,
            blob_at,
            blob_len,
        })
    }

    /// the bytes of a cache file holding one sorted order: `order` lists slot ids
    /// sorted by their key, `keys` holds every slot's key.
    pub fn image(order: &[u32], keys: &KeyTable, fingerprint: &str) -> Vec<u8> {
        let n = order.len();
        debug_assert_eq!(n, keys.count());

        let fp = fingerprint.as_bytes();
        // the slot order, then the key offsets, then the keys themselves.
        let order_at = ORDER_HEADER + fp.len();
        let off_at = order_at + n * 4;
        let blob_at = off_at + (n + 1) * 4;

        let mut out = Vec::with_capacity(blob_at + keys.byte_len());
        out.extend_from_slice(ORDER_MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(n as u32).to_le_bytes());
        out.extend_from_slice(&(fp.len() as u32).to_le_bytes());
        for at in [order_at, off_at, blob_at, keys.byte_len()] {
            out.extend_from_slice(&(at as u64).to_le_bytes());
        }
        debug_assert_eq!(out.len(), ORDER_HEADER);
        out.extend_from_slice(fp);
        for &slot in order {
            out.extend_from_slice(&slot.to_le_bytes());
        }
        // offsets into the blob, which is written in sorted order so a position
        // — not a slot — indexes it.
        let mut at: u32 = 0;
        out.extend_from_slice(&at.to_le_bytes());
        for &slot in order {
            at += keys.get(slot).len() as u32;
            out.extend_from_slice(&at.to_le_bytes());
        }
        for &slot in order {
            out.extend_from_slice(keys.get(slot).as_bytes());
        }
        debug_assert_eq!(at as usize, keys.byte_len());
        out
    }

    /// how many headwords the order holds. deliberately not `len()`, which would
    /// drag an unused `is_empty()` along to satisfy clippy.
    pub fn count(&self) -> usize {
        self.n
    }

    /// the slot at sorted position `i`. `0` for an out-of-range or unreadable
    /// position — a corrupt file then searches oddly, never panics.
    pub fn slot(&self, i: usize) -> u32 {
        if i >= self.n {
            return 0;
        }
        u32_le(self.bytes.as_slice(), self.order_at + i * 4).unwrap_or(0)
    }

    /// the key at sorted position `i` — the thing the binary search compares.
    /// empty for anything unreadable, as `Index::key` is.
    pub fn key(&self, i: usize) -> &[u8] {
        if i >= self.n {
            return &[];
        }
        let raw = self.bytes.as_slice();
        let bound = |i: usize| u32_le(raw, self.off_at + i * 4).map(|v| v as usize);
        let (Some(from), Some(to)) = (bound(i), bound(i + 1)) else {
            return &[];
        };
        // offsets come out of the file, so they are checked, not trusted.
        if from > to || to > self.blob_len {
            return &[];
        }
        raw.get(self.blob_at + from..self.blob_at + to)
            .unwrap_or(&[])
    }
}

// -- writing ---------------------------------------------------------------

/// publish `image` at `path` and map it back, so the cold and warm paths end up
/// holding the very same bytes. written to a sibling temp file and renamed, so a
/// crash or a second dictu mid-write can never leave a half file behind.
pub fn store(path: &Path, image: &[u8]) -> Result<DictBytes> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&temp, image).with_context(|| format!("writing {}", temp.display()))?;
    if let Err(err) = fs::rename(&temp, path) {
        fs::remove_file(&temp).ok();
        return Err(err).with_context(|| format!("publishing {}", path.display()));
    }
    Ok(DictBytes::Mapped(mmap_file(path)?))
}

// -- helpers ---------------------------------------------------------------

/// FNV-1a. only used to keep two dictionaries' cache files apart, so it needs to
/// be stable across runs (a `DefaultHasher` isn't) rather than strong.
fn hash64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// little-endian ints read out of the mapped file. `None` when the slice isn't
/// there, which is how a truncated cache is rejected instead of trusted.
fn u32_le(raw: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        raw.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn u64_le(raw: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        raw.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// a section offset out of the header, as something the slice can be indexed by.
fn offset_le(raw: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u64_le(raw, at)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// two words, one of them with two sense entries, plus a synonym-style extra
    /// pair on an existing word — the shapes a real `.idx` + `.syn` produce.
    fn sample() -> (Vec<(Cow<'static, str>, Range)>, Vec<u32>) {
        let entries = vec![
            (Cow::Borrowed("beta"), (10, 5)),
            (Cow::Borrowed("alpha"), (0, 3)),
            (Cow::Borrowed("alpha"), (20, 7)),
            (Cow::Borrowed("alpha"), (20, 7)),
        ];
        // display order follows the file: beta, alpha (the repeats dedupe away).
        (entries, vec![0, 1, 2, 3])
    }

    /// the cache image for a set of entries, with nothing in the optional
    /// sections — what StarDict writes.
    fn image_of(entries: &[(Cow<'_, str>, Range)], display: &[u32], fp: &str) -> Vec<u8> {
        build(
            Built {
                entries,
                display,
                ..Default::default()
            },
            fp,
        )
    }

    fn index_of(image: Vec<u8>, fp: &str) -> Index {
        Index::open(DictBytes::Owned(image), fp).expect("image should validate")
    }

    #[test]
    fn round_trips_headwords_and_ranges() {
        let (entries, display) = sample();
        let index = index_of(image_of(&entries, &display, "fp"), "fp");

        // display order preserved, duplicates collapsed.
        assert_eq!(index.headwords(), vec!["beta".to_string(), "alpha".into()]);
        // every range of a repeated headword is kept, in sorted-entry order.
        assert_eq!(
            index.ranges("alpha").unwrap(),
            vec![(0, 3), (20, 7), (20, 7)]
        );
        assert_eq!(index.ranges("beta").unwrap(), vec![(10, 5)]);
        assert!(index.ranges("gamma").is_none());
        assert!(index.ranges("").is_none());
        // a prefix of a real key is not a hit (binary search lands next to it).
        assert!(index.ranges("alph").is_none());
    }

    #[test]
    fn handles_multibyte_keys() {
        // byte order and str order agree, so hebrew/greek keys binary-search
        // correctly without decoding the blob.
        let entries = vec![
            (Cow::Borrowed("ἄλφα"), (1, 1)),
            (Cow::Borrowed("שלום"), (2, 2)),
            (Cow::Borrowed("zeta"), (3, 3)),
        ];
        let index = index_of(image_of(&entries, &[0, 1, 2], "fp"), "fp");
        assert_eq!(index.ranges("שלום").unwrap(), vec![(2, 2)]);
        assert_eq!(index.ranges("ἄλφα").unwrap(), vec![(1, 1)]);
        assert_eq!(index.ranges("zeta").unwrap(), vec![(3, 3)]);
    }

    #[test]
    fn a_cache_of_other_sources_is_refused() {
        let (entries, display) = sample();
        let image = image_of(&entries, &display, "fp-old");
        assert!(Index::open(DictBytes::Owned(image), "fp-new").is_none());
    }

    #[test]
    fn a_corrupt_or_truncated_image_is_refused() {
        let (entries, display) = sample();
        let image = image_of(&entries, &display, "fp");

        // truncated anywhere past the header: sections no longer fit.
        let cut = image.len() - 4;
        assert!(Index::open(DictBytes::Owned(image[..cut].to_vec()), "fp").is_none());
        // header alone, no sections.
        assert!(Index::open(DictBytes::Owned(image[..INDEX_HEADER].to_vec()), "fp").is_none());
        // empty, and not one of ours.
        assert!(Index::open(DictBytes::Owned(Vec::new()), "fp").is_none());
        assert!(
            Index::open(DictBytes::Owned(b"not a dictu cache at all".to_vec()), "fp").is_none()
        );
        // right magic, wrong version.
        let mut wrong = image.clone();
        wrong[8..12].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(Index::open(DictBytes::Owned(wrong), "fp").is_none());
        // a garbled key-offset table must not panic — an unreadable key just
        // fails to match.
        let mut garbled = image.clone();
        let table = INDEX_HEADER + 2; // inside the key offsets, past the fingerprint.
        garbled[table..table + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let index = index_of(garbled, "fp");
        assert!(index.ranges("alpha").is_none() || index.ranges("beta").is_none());
    }

    #[test]
    fn store_publishes_a_mappable_file() {
        let dir = std::env::temp_dir().join(format!("dictu-cache-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        let (entries, display) = sample();
        let fp = fingerprint(&[dir.join("nothing-here")]);
        let path = index_path(&dir, Path::new("/dicts/Some Dict.ifo"));

        // nothing cached yet.
        assert!(Index::load(&path, &fp).is_none());
        let bytes = store(&path, &image_of(&entries, &display, &fp)).unwrap();
        // no temp files left behind, and the mapped-back image is usable.
        assert!(matches!(bytes, DictBytes::Mapped(_)));
        let index = Index::load(&path, &fp).expect("a fresh cache is a hit");
        assert_eq!(index.ranges("beta").unwrap(), vec![(10, 5)]);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fingerprint_follows_size_and_mtime() {
        let dir = std::env::temp_dir().join(format!("dictu-fp-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("d.idx");

        assert!(fingerprint(std::slice::from_ref(&file)).ends_with(":missing"));
        fs::write(&file, b"one").unwrap();
        let first = fingerprint(std::slice::from_ref(&file));
        assert!(!first.ends_with(":missing"));
        // same bytes, same fingerprint.
        assert_eq!(first, fingerprint(std::slice::from_ref(&file)));
        // a different size is a different dictionary.
        fs::write(&file, b"one more").unwrap();
        let grown = fingerprint(std::slice::from_ref(&file));
        assert_ne!(first, grown);
        // and so is the same size with a newer mtime (a replaced file).
        fs::write(&file, b"two more").unwrap();
        assert_ne!(grown, fingerprint(std::slice::from_ref(&file)));

        fs::remove_dir_all(&dir).ok();
    }

    /// three headwords keyed for the index: one greek word whose accent the key
    /// drops, and two plain ones.
    fn bare_keys() -> KeyTable {
        let mut keys = KeyTable::with_capacity(3);
        for word in ["rex", "λόγος", "amo"] {
            keys.push(&crate::keys::bare(&crate::keys::fold(word)));
        }
        keys
    }

    #[test]
    fn order_round_trips_and_is_validated() {
        let keys = bare_keys();
        let image = Order::image(&keys.order(), &keys, "fp");
        let dir = std::env::temp_dir().join(format!("dictu-order-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        let path = order_path(&dir);

        assert!(Order::load(&path, "fp").is_none()); // nothing cached yet.
        store(&path, &image).unwrap();
        let order = Order::load(&path, "fp").unwrap();
        assert_eq!(order.count(), 3);

        // sorted by the key, and each position's key is the key of the slot at
        // that position. the accent is gone, the final sigma is folded.
        let listed: Vec<(u32, String)> = (0..order.count())
            .map(|i| {
                (
                    order.slot(i),
                    String::from_utf8(order.key(i).to_vec()).unwrap(),
                )
            })
            .collect();
        assert_eq!(
            listed,
            vec![
                (2, "amo".to_string()),
                (0, "rex".into()),
                (1, "λογοσ".into())
            ]
        );
        // out of range: empty and zero, not a panic.
        assert_eq!(order.slot(99), 0);
        assert!(order.key(99).is_empty());

        // a different collection, and a truncated file, are both misses.
        assert!(Order::load(&path, "other").is_none());
        store(&path, &image[..image.len() - 4]).unwrap();
        assert!(Order::load(&path, "fp").is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_order_is_refused_or_reads_empty() {
        let keys = bare_keys();
        let image = Order::image(&keys.order(), &keys, "fp");

        // not ours, header-only, and the version this replaces.
        assert!(Order::open(DictBytes::Owned(b"nope".to_vec()), "fp").is_none());
        assert!(Order::open(DictBytes::Owned(image[..ORDER_HEADER].to_vec()), "fp").is_none());
        let mut old = image.clone();
        old[8..12].copy_from_slice(&(VERSION - 1).to_le_bytes());
        assert!(
            Order::open(DictBytes::Owned(old), "fp").is_none(),
            "a v{} order is sorted by a key nothing searches with now",
            VERSION - 1
        );

        // a garbled key-offset table reads empty rather than panicking.
        let mut garbled = image.clone();
        let table = ORDER_HEADER + 2 + 3 * 4; // inside the key offsets.
        garbled[table..table + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let order = Order::open(DictBytes::Owned(garbled), "fp").unwrap();
        assert!((0..order.count()).any(|i| order.key(i).is_empty()));
    }

    #[test]
    fn cache_paths_separate_same_named_dictionaries() {
        let dir = Path::new("/cache");
        let a = index_path(dir, Path::new("/dicts/a/latin.ifo"));
        let b = index_path(dir, Path::new("/dicts/b/latin.ifo"));
        assert_ne!(a, b);
        // readable stem, and no path separators smuggled into the file name.
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("latin-") && name.ends_with(".didx"));
    }
}
