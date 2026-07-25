//! reader for the ABBYY Lingvo DSL format: one text file (`.dsl`, or dictzipped
//! `.dsl.dz`) holding a `#KEY "value"` header and then card after card —
//! headword lines at column 0, body lines indented with a tab or spaces, and
//! several headwords allowed to share one body.
//!
//! two things set it apart from our other formats. the text is usually utf-16
//! (little-endian with a bom; sometimes utf-8), and the body carries dsl's own
//! square-bracket markup, which we convert to html so that the single renderer
//! serving every format can read it — `Dictionary::lookup`'s contract.
//!
//! the file is decoded once and kept; the index holds only a byte range per
//! card, and markup is converted lazily in `lookup`, so a 110 MB lexicon costs
//! its text plus its headwords, not a parsed entry per card.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{Dictionary, load_dict_bytes};

/// a card body's byte range `(start, end)` in `text`. `u32` halves the index
/// against a `usize` pair; the largest dsl here decodes to ~110 MB.
type Range = (u32, u32);

pub struct DslDictionary {
    name: String,
    headwords: Vec<String>,
    /// headword -> the body ranges filed under it. a `Vec` because one headword
    /// can head several cards, and one card several headwords.
    index: HashMap<String, Vec<Range>>,
    /// the whole file, decoded once. bodies are sliced out of it on demand.
    text: String,
}

impl DslDictionary {
    /// open a dictionary given the path to its `.dsl` or `.dsl.dz`.
    pub fn open(path: &Path) -> Result<Self> {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("dsl path has no usable file name")?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        // hand the last extension to the shared loader: it mmaps a plain `.dsl`
        // and gunzips a `.dsl.dz` — or a `.dsl` that turns out to be gzip.
        let (stem, ext) = file_name
            .rsplit_once('.')
            .context("dsl path has no extension")?;
        let bytes = load_dict_bytes(dir, stem, &[ext])
            .with_context(|| format!("reading {}", path.display()))?;

        let text = decode(bytes.as_slice());
        // the raw bytes are dead once decoded — drop the map/buffer before we
        // spend memory on the index.
        drop(bytes);

        // `x.dsl.dz` leaves ".dsl" on the stem; a second file_stem strips it.
        let fallback = Path::new(stem)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(stem);
        Self::from_text(text, fallback)
    }

    /// parse already-decoded text. pure (no i/o), so tests drive it directly.
    fn from_text(text: String, fallback_name: &str) -> Result<Self> {
        if text.len() > u32::MAX as usize {
            bail!("dsl file is larger than 4 GiB");
        }
        let mut name = None;
        let mut headwords = Vec::new();
        let mut index: HashMap<String, Vec<Range>> = HashMap::new();
        // headword lines waiting for the body they share, and that body so far.
        let mut pending: Vec<&str> = Vec::new();
        let mut body: Option<Range> = None;
        let mut in_header = true;

        for (offset, line) in lines_with_offsets(&text) {
            // only tab and space indent a body line. `char::is_whitespace` would
            // also match the nbsp some headwords start with (20 in klein), and
            // read those cards as bodies with no headword.
            let indented = line.starts_with('\t') || line.starts_with(' ');
            if line.trim().is_empty() {
                continue; // blank lines separate cards; they never end a body.
            }
            if in_header && !indented && line.starts_with('#') {
                name = name.or_else(|| directive(line, "#NAME"));
                continue;
            }
            in_header = false;

            if indented {
                let end = offset + line.len() as u32;
                // the body grows to the last indented line; blank lines in the
                // middle come along in the slice, trailing ones don't.
                body = Some(match body {
                    Some((start, _)) => (start, end),
                    None => (offset, end),
                });
            } else {
                if let Some(range) = body.take() {
                    // a column-0 line after a body starts a new card.
                    file_card(&pending, range, &mut headwords, &mut index);
                    pending.clear();
                }
                pending.push(line);
            }
        }
        if let Some(range) = body {
            file_card(&pending, range, &mut headwords, &mut index);
        }

        // no cards and no header: whatever this file is, it isn't dsl. failing
        // here beats handing the ui a dictionary of garbage headwords.
        if headwords.is_empty() && name.is_none() {
            bail!("no dsl header or entries found — not a DSL file?");
        }

        Ok(Self {
            name: name.unwrap_or_else(|| fallback_name.to_string()),
            headwords,
            index,
            text,
        })
    }
}

impl Dictionary for DslDictionary {
    fn name(&self) -> &str {
        &self.name
    }

    fn headwords(&self) -> &[String] {
        &self.headwords
    }

    fn lookup(&self, headword: &str) -> Vec<String> {
        self.index
            .get(headword)
            .into_iter()
            .flatten()
            .filter_map(|&(start, end)| self.text.get(start as usize..end as usize))
            // `~` in a body stands for the headword, so conversion needs it.
            .map(|body| body_to_html(body, headword))
            .filter(|html| !html.is_empty())
            .collect()
    }
}

// -- decoding --------------------------------------------------------------

/// decode the file, honouring its byte-order mark. dsl is nominally utf-16, but
/// converted dictionaries here are sometimes utf-8; with no bom at all we take
/// the nul bytes an ascii-heavy utf-16le file is full of as the signal.
fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xff, 0xfe, rest @ ..] => from_utf16(rest, true),
        [0xfe, 0xff, rest @ ..] => from_utf16(rest, false),
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ if looks_utf16le(bytes) => from_utf16(bytes, true),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// utf-16 code units -> text, decoded in place: no `Vec<u16>` copy of a 170 MB
/// file. a lone surrogate becomes U+FFFD instead of failing the load, and a
/// trailing odd byte is dropped.
///
/// ascii is pushed straight through, because that shortcut is what makes opening
/// liddell-scott bearable: dsl is markup- and abbreviation-heavy, so most code
/// units are ascii, and skipping the decode machinery for them is 4x faster in
/// the unoptimized build the app actually runs as (6.7s -> 1.8s).
fn from_utf16(bytes: &[u8], little_endian: bool) -> String {
    let (low, high) = if little_endian { (0, 1) } else { (1, 0) };
    let unit = |pair: &[u8]| {
        let (a, b) = (pair[low], pair[high]);
        u16::from(b) << 8 | u16::from(a)
    };

    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while rest.len() >= 2 {
        let mut ascii = 0;
        while ascii + 1 < rest.len() && rest[ascii + high] == 0 && rest[ascii + low] < 0x80 {
            out.push(rest[ascii + low] as char);
            ascii += 2;
        }
        rest = &rest[ascii..];
        if rest.len() < 2 {
            break;
        }
        // one non-ascii char: two code units are enough for any of them, and a
        // surrogate pair is the only case that consumes both.
        let units = rest.chunks_exact(2).take(2).map(unit);
        let Some(decoded) = char::decode_utf16(units).next() else {
            break;
        };
        let ch = decoded.unwrap_or(char::REPLACEMENT_CHARACTER);
        rest = &rest[if ch as u32 > 0xffff { 4 } else { 2 }..];
        out.push(ch);
    }
    // capacity was sized to the utf-16 byte count; these scripts all encode smaller
    // in utf-8, and this string lives as long as the dictionary does — the slack was
    // ~225 MB across the collection. give it back.
    out.shrink_to_fit();
    out
}

/// utf-16le without a bom, by NUL ratio: ascii encoded as utf-16le is a byte then a
/// NUL, so such a file is about half NULs. the sample is only the first kilobyte —
/// in practice the `#NAME`/`#INDEX_LANGUAGE` header, which is ascii even in the
/// hebrew and greek dictionaries, and that is what makes the ratio dependable.
/// (a NUL is perfectly *legal* in utf-8, so this is a ratio test and not a validity
/// one: a misread decodes to nonsense and `from_text` then bails rather than
/// indexing it. a bom-less file whose first kilobyte is non-ascii would be refused,
/// which is the right way to be wrong.)
fn looks_utf16le(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(1024)];
    !sample.is_empty() && sample.iter().filter(|&&b| b == 0).count() * 5 > sample.len()
}

// -- structure -------------------------------------------------------------

/// every line with its byte offset, splitting on `\n` and dropping a `\r` —
/// files here mix both endings, sometimes within one header.
fn lines_with_offsets(text: &str) -> impl Iterator<Item = (u32, &str)> {
    let mut offset = 0u32;
    text.split('\n').map(move |line| {
        let at = offset;
        offset += line.len() as u32 + 1;
        (at, line.trim_end_matches('\r'))
    })
}

/// the value of a header directive: `#NAME\t"Some Dictionary"` -> the name.
fn directive(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?;
    let value = rest.trim().trim_matches('"').trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// file one card's body under every headword it is listed with. the same
/// headword can appear twice in one card (halot does that 9.5k times), so the
/// same range is never filed twice — it would show the entry twice.
fn file_card(
    hw_lines: &[&str],
    body: Range,
    headwords: &mut Vec<String>,
    index: &mut HashMap<String, Vec<Range>>,
) {
    for line in hw_lines {
        for key in index_variants(line) {
            match index.entry(key) {
                Entry::Vacant(slot) => {
                    headwords.push(slot.key().clone());
                    slot.insert(vec![body]);
                }
                Entry::Occupied(mut slot) => {
                    let ranges = slot.get_mut();
                    if !ranges.contains(&body) {
                        ranges.push(body);
                    }
                }
            }
        }
    }
}

/// how many optional `(…)` groups we expand, i.e. 8 variants at most. beyond
/// that only the everything-kept form is filed, rather than 2^n of them.
const MAX_OPTIONAL_GROUPS: u32 = 3;

/// the searchable forms of one headword line. escapes are resolved, `{…}`
/// display-only text is dropped, and `(…)` marks an optional part — so
/// `ad lib(itum)` files under both "ad libitum" and "ad lib", and halot's 6.5k
/// `(*)`-marked hebrew roots are findable by the bare root.
fn index_variants(line: &str) -> Vec<String> {
    // the line as (text, optional?) segments; parens themselves never survive.
    let mut segments: Vec<(String, bool)> = vec![(String::new(), false)];
    let mut groups = 0u32;
    // depth, not a flag: halot writes `((\*)II) אבד`, and a nested `(` used to fall
    // through to the literal arm — filing the card under "II) אבד" and leaving the
    // bare root unable to find it. the whole nest is one optional group.
    let mut depth = 0usize;
    let mut brace_depth = 0usize;
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        match ch {
            // an escape makes the next char literal, brackets and braces included.
            '\\' => {
                // the escaped char is consumed either way, so a `\}` inside
                // braces cannot end the display-only run early.
                if let Some(next) = chars.next()
                    && brace_depth == 0
                {
                    push_segment(&mut segments, next, depth > 0);
                }
            }
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            _ if brace_depth > 0 => {}
            '(' => {
                depth += 1;
                if depth == 1 {
                    groups += 1;
                    segments.push((String::new(), true));
                }
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    segments.push((String::new(), false));
                }
            }
            _ => push_segment(&mut segments, ch, depth > 0),
        }
    }

    let combinations = if groups == 0 || groups > MAX_OPTIONAL_GROUPS {
        1u32 // one variant: every optional group kept.
    } else {
        1 << groups
    };
    let keep_all = groups == 0 || groups > MAX_OPTIONAL_GROUPS;

    let mut out: Vec<String> = Vec::new();
    for mask in 0..combinations {
        let mut variant = String::new();
        let mut group = 0;
        for (text, optional) in &segments {
            if !optional {
                variant.push_str(text);
                continue;
            }
            if keep_all || mask & (1 << group) != 0 {
                variant.push_str(text);
            }
            group += 1;
        }
        // dropping a group can leave doubled or edge whitespace behind.
        let variant = variant.split_whitespace().collect::<Vec<_>>().join(" ");
        if !variant.is_empty() && !out.contains(&variant) {
            out.push(variant);
        }
    }
    out
}

/// append one char to the segment being built, starting a new segment when the
/// optional-ness changes.
fn push_segment(segments: &mut Vec<(String, bool)>, ch: char, optional: bool) {
    match segments.last_mut() {
        Some((text, opt)) if *opt == optional => text.push(ch),
        _ => segments.push((ch.to_string(), optional)),
    }
}

// -- markup ----------------------------------------------------------------

/// convert one card's body to an html fragment: every dsl line is a block, and
/// `[mN]` nests it N deep.
fn body_to_html(body: &str, headword: &str) -> String {
    let mut out = String::with_capacity(body.len() + 16);
    for line in body.lines() {
        let line = line.trim_end_matches('\r').trim_start_matches(['\t', ' ']);
        let (indent, line) = leading_indent(line);
        let inner = inline_to_html(line, headword);
        if inner.trim().is_empty() {
            continue; // a line that was only markup adds nothing to render.
        }
        // nested <blockquote> carries the indent depth into the renderer, which
        // treats it as block-level today and can indent it later; <div> is the
        // unindented block. markup.rs collapses the resulting newlines.
        if indent == 0 {
            out.push_str("<div>");
            out.push_str(&inner);
            out.push_str("</div>");
        } else {
            for _ in 0..indent {
                out.push_str("<blockquote>");
            }
            out.push_str(&inner);
            for _ in 0..indent {
                out.push_str("</blockquote>");
            }
        }
    }
    out
}

/// pull a leading `[mN]` (dsl's indent level for the block) off a line. `[m]`
/// and `[m0]` are level 0; anything else leaves the line untouched.
fn leading_indent(line: &str) -> (usize, &str) {
    let Some((name, closing, len)) = tag_at(line) else {
        return (0, line);
    };
    let Some(digits) = name.strip_prefix('m').filter(|_| !closing) else {
        return (0, line);
    };
    match digits.parse::<usize>() {
        Ok(level) => (level.min(9), &line[len..]),
        // "[m]" — no level, so the outermost one.
        Err(_) if digits.is_empty() => (0, &line[len..]),
        Err(_) => (0, line),
    }
}

/// the html element a dsl tag becomes, or `None` to drop the tag and keep its
/// contents. dropping is the default for everything unlisted — colour (`[c]`),
/// zone markers (`[trn]`, `[!trs]`, `[com]`), `[lang]`, `[m]`, stress marks and
/// whatever a future dictionary invents — because losing a distinction beats
/// leaking raw brackets into the definition.
fn element_for(tag: &str) -> Option<&'static str> {
    match tag {
        "b" => Some("b"),
        "i" => Some("i"),
        "u" => Some("u"),
        "sup" => Some("sup"),
        "sub" => Some("sub"),
        // an example renders italic; <cite> is the tag markup.rs italicizes
        // that also records *why* it is italic.
        "ex" => Some("cite"),
        // [p] marks a label or abbreviation ("v.", "Od."), italic as lingvo
        // shows them — and there are 822k of them in liddell-scott.
        "p" => Some("i"),
        // [t] is a phonetic transcription; monospace keeps ipa legible.
        "t" => Some("tt"),
        _ => None,
    }
}

/// convert one line of dsl markup to inline html: text escaped, the tags we can
/// render become elements, cross-references become `bword://` links, and
/// anything else is dropped.
fn inline_to_html(line: &str, headword: &str) -> String {
    let mut out = String::with_capacity(line.len() + 16);
    // elements we've opened, so a close tag can only close something real —
    // real dsl is often mis-nested ([b]…[c]…[/b]…[/c]).
    let mut open: Vec<&'static str> = Vec::new();
    // inside [s]…[/s]: a sound/image file name, not text to show.
    let mut skipping = false;
    let mut rest = line;

    while let Some(ch) = rest.chars().next() {
        let mut advance = ch.len_utf8();
        match ch {
            // an escape makes the next char literal — this is what keeps `\[`
            // out of the tag parser.
            '\\' => {
                if let Some(next) = rest[1..].chars().next() {
                    advance = 1 + next.len_utf8();
                    if !skipping {
                        escape_char(&mut out, next);
                    }
                }
            }
            '[' => match tag_at(rest) {
                None => {
                    // no `]` closes it: a literal bracket.
                    if !skipping {
                        out.push('[');
                    }
                }
                Some((name, closing, len)) => {
                    advance = len;
                    match name.as_str() {
                        "s" => skipping = !closing,
                        "ref" if !closing => {
                            // [ref]word[/ref] — the target is its text.
                            let after = &rest[len..];
                            let (inner, used) = match find_closing(after, "ref") {
                                Some((from, to)) => (&after[..from], len + to),
                                None => (after, rest.len()),
                            };
                            advance = used;
                            if !skipping {
                                push_link(&mut out, &plain(inner));
                            }
                        }
                        _ if skipping => {}
                        _ => match (element_for(&name), closing) {
                            (Some(el), false) => {
                                out.push('<');
                                out.push_str(el);
                                out.push('>');
                                open.push(el);
                            }
                            (Some(el), true) => close_element(&mut out, &mut open, el),
                            (None, _) => {}
                        },
                    }
                }
            },
            // <<word>> is the other spelling of a cross-reference.
            '<' if rest.starts_with("<<") => match rest[2..].find(">>") {
                Some(end) => {
                    if !skipping {
                        push_link(&mut out, &plain(&rest[2..2 + end]));
                    }
                    advance = 2 + end + 2;
                }
                None => {
                    if !skipping {
                        out.push_str("&lt;");
                    }
                }
            },
            // {{…}} is a comment in some dictionaries: drop it whole.
            '{' if rest.starts_with("{{") => {
                advance = match rest[2..].find("}}") {
                    Some(end) => 2 + end + 2,
                    None => rest.len(),
                };
            }
            // `~` stands for the card's headword.
            '~' => {
                if !skipping {
                    escape_into(&mut out, headword);
                }
            }
            _ => {
                if !skipping {
                    escape_into(&mut out, &rest[..advance]);
                }
            }
        }
        rest = &rest[advance..];
    }

    // a tag left open at end of line closes here: markup is converted per line,
    // so what we hand the parser is always balanced.
    while let Some(el) = open.pop() {
        out.push_str("</");
        out.push_str(el);
        out.push('>');
    }
    out
}

/// parse a `[tag]` at the start of `s`: lowercased name (closing slash and
/// arguments stripped), whether it closes, and how many bytes it spans. `None`
/// when nothing closes it, or a `[` opens first — then it is literal text.
fn tag_at(s: &str) -> Option<(String, bool, usize)> {
    let close = s.strip_prefix('[')?.find(']')? + 1;
    let inner = &s[1..close];
    if inner.contains('[') {
        return None;
    }
    let (closing, name) = match inner.strip_prefix('/') {
        Some(name) => (true, name),
        None => (false, inner),
    };
    // "[c blue]" and "[lang id=1033]" are the `c` and `lang` tags. tag case
    // varies in the wild ([SUP] in halot), so names are lowercased.
    let name = name
        .split([' ', '='])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    Some((name, closing, close + 1))
}

/// where the `[/tag]` closing an element is, if anywhere on this line.
fn find_closing(s: &str, tag: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(offset) = s[from..].find('[') {
        let at = from + offset;
        match tag_at(&s[at..]) {
            Some((name, true, len)) if name == tag => return Some((at, at + len)),
            Some((_, _, len)) => from = at + len,
            None => from = at + 1,
        }
    }
    None
}

/// close `el`, and anything opened after it — mis-nested dsl would otherwise
/// produce mis-nested html. a close tag for something never opened is dropped.
fn close_element(out: &mut String, open: &mut Vec<&'static str>, el: &'static str) {
    if let Some(at) = open.iter().rposition(|&o| o == el) {
        let mut inner: Vec<&str> = open.drain(at..).collect();
        while let Some(el) = inner.pop() {
            out.push_str("</");
            out.push_str(el);
            out.push('>');
        }
    }
}

/// a cross-reference. `bword://` is the scheme the ui already follows (see
/// `markup::link_target`), so dsl references are clickable with no ui change.
fn push_link(out: &mut String, target: &str) {
    if target.is_empty() {
        return;
    }
    out.push_str("<a href=\"bword://");
    escape_into(out, target);
    out.push_str("\">");
    escape_into(out, target);
    out.push_str("</a>");
}

/// the plain text of a reference target: escapes resolved, markup dropped.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(ch) = rest.chars().next() {
        let mut advance = ch.len_utf8();
        match ch {
            '\\' => {
                if let Some(next) = rest[1..].chars().next() {
                    advance = 1 + next.len_utf8();
                    out.push(next);
                }
            }
            '[' => match tag_at(rest) {
                Some((_, _, len)) => advance = len,
                None => out.push('['),
            },
            _ => out.push_str(&rest[..advance]),
        }
        rest = &rest[advance..];
    }
    out.trim().to_string()
}

/// append text with html's specials escaped — `"` included, so a headword
/// holding a quote can't break out of a link's href.
fn escape_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        escape_char(out, ch);
    }
}

fn escape_char(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        _ => out.push(ch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::markup;

    /// the fixture, as a dsl author would write it: two headwords sharing one
    /// card, indent levels, escapes, both reference spellings, and `~`.
    const SAMPLE: &str = concat!(
        "#NAME\t\"Test Lexicon\"\r\n",
        "#INDEX_LANGUAGE\t\"Greek\"\r\n",
        "#CONTENTS_LANGUAGE\t\"English\"\r\n",
        "\r\n",
        "λόγος\r\n",
        "logos\r\n",
        "\t[m0][b]λόγος[/b], [p]n.[/p][/m]\r\n",
        "\t[m1][trn]word, ~ as spoken[/trn][/m]\r\n",
        "\t[m2][c blue]see also[/c] [ref]ῥῆμα[/ref] and <<λέγω>>[/m]\r\n",
        "\t[m2]a bracket: \\[sic\\] and x[sup]2[/sup][/m]\r\n",
        "\r\n",
        "ad lib(itum)\r\n",
        "\ta phrase\r\n",
    );

    fn sample() -> DslDictionary {
        DslDictionary::from_text(SAMPLE.to_string(), "fallback").unwrap()
    }

    /// encode `text` as utf-16 with a bom, the way real dsl files are stored.
    fn utf16(text: &str, little_endian: bool) -> Vec<u8> {
        let bom: u16 = 0xfeff;
        std::iter::once(bom)
            .chain(text.encode_utf16())
            .flat_map(|unit| {
                if little_endian {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect()
    }

    #[test]
    fn decodes_utf16_le_be_and_utf8() {
        // ascii, hebrew, greek and an astral char (the only case stored as a
        // surrogate pair) — every path through the utf-16 decoder.
        let text = "#NAME\t\"Δ\"\nא\n\tbody 𝔄\n";
        assert_eq!(decode(&utf16(text, true)), text);
        assert_eq!(decode(&utf16(text, false)), text);
        // utf-8 with and without a bom.
        let mut with_bom = vec![0xef, 0xbb, 0xbf];
        with_bom.extend_from_slice(text.as_bytes());
        assert_eq!(decode(&with_bom), text);
        assert_eq!(decode(text.as_bytes()), text);
        // utf-16le without a bom is guessed from its nul bytes.
        let bomless: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode(&bomless), text);
        // a lone surrogate is replaced rather than failing the load.
        assert_eq!(decode(&[0xff, 0xfe, 0x00, 0xd8]), "\u{fffd}");
    }

    #[test]
    fn reads_the_header_name_and_falls_back_to_the_stem() {
        assert_eq!(sample().name(), "Test Lexicon");
        let no_header = DslDictionary::from_text("word\n\tdef\n".into(), "fallback").unwrap();
        assert_eq!(no_header.name(), "fallback");
    }

    #[test]
    fn headwords_share_one_body() {
        let dict = sample();
        // both column-0 lines of the first card are findable, and each returns
        // the one body they share — with `~` reading as the word looked up.
        let greek = dict.lookup("λόγος");
        let latin = dict.lookup("logos");
        assert_eq!(greek.len(), 1);
        assert_eq!(latin.len(), 1);
        assert!(markup::to_text(&greek[0]).contains("word, λόγος as spoken"));
        assert!(markup::to_text(&latin[0]).contains("word, logos as spoken"));
        assert!(dict.lookup("nothing").is_empty());
        // the header lines are not headwords.
        assert!(!dict.headwords().iter().any(|w| w.starts_with('#')));
    }

    #[test]
    fn expands_the_headword_for_a_tilde() {
        let text = markup::to_text(&sample().lookup("λόγος")[0]);
        assert!(text.contains("word, λόγος as spoken"), "got {text:?}");
    }

    #[test]
    fn optional_parens_file_both_forms() {
        let dict = sample();
        // the paren group is optional: both spellings find the card, and both
        // are listed. the parens themselves never survive.
        assert_eq!(dict.lookup("ad libitum").len(), 1);
        assert_eq!(dict.lookup("ad lib").len(), 1);
        assert!(dict.headwords().contains(&"ad libitum".to_string()));
        assert!(dict.headwords().contains(&"ad lib".to_string()));
        // two groups -> four forms; braces are display-only and drop out.
        assert_eq!(index_variants("a(b)c(d)"), ["ac", "abc", "acd", "abcd"]);
        assert_eq!(index_variants("{\\[}σαν{\\]}"), ["σαν"]);
        // more groups than we expand: just the everything-kept form.
        assert_eq!(index_variants("a(1)b(2)c(3)d(4)"), ["a1b2c3d4"]);
    }

    #[test]
    fn maps_the_markup_subset_to_html() {
        let html = &sample().lookup("λόγος")[0];
        // rendered tags survive...
        assert!(html.contains("<b>λόγος</b>"));
        assert!(html.contains("<sup>2</sup>"));
        assert!(html.contains("<i>n.</i>")); // [p] -> italic.
        // ...and the ones we can't render leave no brackets behind.
        for leaked in ["[c", "[/c]", "[trn]", "[m1]", "[m2]", "[p]"] {
            assert!(!html.contains(leaked), "{leaked} leaked into {html}");
        }
        // \[ and \] are literal text, not markup.
        assert!(markup::to_text(html).contains("a bracket: [sic]"));
    }

    #[test]
    fn indent_levels_nest_as_blocks() {
        let html = &sample().lookup("λόγος")[0];
        // [m0] is a plain block, [m1]/[m2] nest one and two deep.
        assert!(html.contains("<div><b>λόγος</b>"));
        assert!(html.contains("<blockquote>word,"));
        assert!(html.contains("<blockquote><blockquote>"));
        // every line of the card is a block, so they render on separate lines.
        assert_eq!(markup::to_text(html).lines().count(), 4);
    }

    #[test]
    fn cross_references_become_followable_links() {
        let html = &sample().lookup("λόγος")[0];
        assert!(
            html.contains(r#"<a href="bword://ῥῆμα">ῥῆμα</a>"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<a href="bword://λέγω">λέγω</a>"#),
            "{html}"
        );
        // and the ui can actually follow what we emitted.
        let targets: Vec<String> = markup::to_runs(html)
            .iter()
            .filter_map(|run| run.href.as_deref())
            .filter_map(markup::link_target)
            .collect();
        assert_eq!(targets, ["ῥῆμα", "λέγω"]);
    }

    #[test]
    fn drops_media_and_comments_but_keeps_their_neighbours() {
        let html = body_to_html("\t[m1]hear [s]word.wav[/s]{{editor note}}now[/m]", "w");
        let text = markup::to_text(&html);
        assert_eq!(text, "hear now");
    }

    #[test]
    fn mis_nested_markup_still_yields_balanced_html() {
        // real dsl closes tags out of order; the renderer must still see a tree.
        let html = body_to_html("\t[b][i]both[/b] italic[/i]", "w");
        assert_eq!(html.matches("<i>").count(), html.matches("</i>").count());
        assert_eq!(html.matches("<b>").count(), html.matches("</b>").count());
        assert_eq!(markup::to_text(&html), "both italic");
        // a stray close tag for nothing open is dropped, not emitted.
        assert!(!body_to_html("\tplain[/b]", "w").contains("</b>"));
    }

    #[test]
    fn escapes_text_so_it_cannot_be_read_as_html() {
        let html = body_to_html("\tR&D <*> a<b>tag</b>", "w");
        assert!(html.contains("&amp;") && html.contains("&lt;*&gt;"));
        // the literal "<b>" in the source is text, not a bold element.
        assert_eq!(markup::to_text(&html), "R&D <*> a<b>tag</b>");
    }

    #[test]
    fn garbage_and_truncation_fail_without_panicking() {
        // random binary: no header, no cards.
        let junk: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        assert!(DslDictionary::from_text(decode(&junk), "x").is_err());
        // odd byte count in utf-16 (a cut-off file) drops the stray byte.
        let mut cut = utf16("#NAME\t\"T\"\nα\n\tdef\n", true);
        cut.pop();
        let dict = DslDictionary::from_text(decode(&cut), "x").unwrap();
        assert_eq!(dict.name(), "T");
        // a card whose body never arrived is simply not filed.
        let dangling = DslDictionary::from_text("#NAME\t\"T\"\nword\n".into(), "x").unwrap();
        assert!(dangling.headwords().is_empty());
        assert!(dangling.lookup("word").is_empty());
        // and an unterminated tag or reference must not run off the end.
        assert!(!body_to_html("\t[b unclosed", "w").is_empty());
        assert!(body_to_html("\t[ref]dangling", "w").contains("bword://dangling"));
        assert!(body_to_html("\t<<dangling", "w").contains("&lt;"));
    }

    #[test]
    fn open_reads_a_gzipped_dsl_from_disk() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("dictu-dsl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // utf-16le with a bom, dictzip-compatible gzip — the klein/dodson shape.
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&utf16(SAMPLE, true)).unwrap();
        let path = dir.join("Klein_v1_0.dsl.dz");
        std::fs::write(&path, gz.finish().unwrap()).unwrap();

        let dict = DslDictionary::open(&path).unwrap();
        assert_eq!(dict.name(), "Test Lexicon");
        assert_eq!(dict.lookup("logos").len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}
