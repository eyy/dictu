//! turn a dictionary's html definition into a flat list of styled text runs.
//!
//! we map a controlled subset of tags (b/i/sup/sub/a/headers/…) to OUR own
//! semantic styling and ignore each dict's inline css, colors, and classes — so
//! every dictionary renders in one consistent look. this module is ui-agnostic
//! (no gtk): the ui turns `Style` into TextView tags. no browser engine — just
//! html5ever parsing via `scraper`.

use ego_tree::NodeRef;
use scraper::{Html, Node};

/// how deep the tag nesting may go before we stop descending — see `walk`.
const MAX_NESTING: u32 = 256;

/// semantic styling for a run of text. the ui maps these flags to its own tags.
#[derive(Clone, Copy, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub scale: f64,
    pub sup: bool,
    pub sub: bool,
    pub link: bool,
    pub mono: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            scale: 1.0,
            sup: false,
            sub: false,
            link: false,
            mono: false,
        }
    }
}

/// a contiguous run of text sharing one style (and one link target, if any).
pub struct Run {
    pub text: String,
    pub style: Style,
    /// the raw `href` of the enclosing `<a>`, if the run sits inside one. kept raw
    /// because whether it's followable is the caller's question — see `link_target`.
    pub href: Option<String>,
}

/// parse `html` and return whitespace-normalized styled runs.
pub fn to_runs(html: &str) -> Vec<Run> {
    let doc = Html::parse_fragment(html);
    let mut builder = Builder::default();
    walk(doc.tree.root(), Style::default(), None, &mut builder, 0);
    builder.runs
}

/// the headword a link points at, or `None` if it doesn't point inside the
/// collection. dictionaries here use babylon's `bword://word` (and a bare
/// `bword:word` variant); anything with another scheme leads out of the app —
/// http, mailto — and is deliberately not followable, since a dictionary lookup
/// should never open a browser.
pub fn link_target(href: &str) -> Option<String> {
    let target = href
        .strip_prefix("bword://")
        .or_else(|| href.strip_prefix("bword:"))
        .unwrap_or(href)
        .trim();
    if target.is_empty() || target.contains("://") || target.starts_with('#') {
        return None;
    }
    // a scheme we don't know (mailto:, http:) means "not a headword". a bare
    // word, or the target of a stripped bword link, is one.
    match target.split_once(':') {
        Some((scheme, _)) if !scheme.contains(char::is_whitespace) => None,
        _ => Some(target.to_string()),
    }
}

/// the plain text of the html (runs concatenated) — for the `dump` cli and any
/// plain-text fallback.
pub fn to_text(html: &str) -> String {
    to_runs(html).into_iter().map(|r| r.text).collect()
}

/// walk the dom, pushing text into `builder` with the current inherited style and
/// enclosing link target.
fn walk<'a>(
    node: NodeRef<'a, Node>,
    style: Style,
    href: Option<&'a str>,
    builder: &mut Builder,
    depth: u32,
) {
    // a dictionary file is untrusted input, and this walk is recursive: ~40k nested
    // tags overflow the stack, which aborts the process outright — not a panic the
    // loader could catch and report. the deepest nesting in the real collection is 3,
    // so a cap this high only ever fires on a corrupt or hostile entry, and dropping
    // the rest of that entry beats taking the app down with it.
    if depth > MAX_NESTING {
        return;
    }
    let mut child = node.first_child();
    while let Some(current) = child {
        match current.value() {
            Node::Text(text) => builder.text(text, style, href),
            Node::Element(el) => {
                let name = el.name();
                // never render the *contents* of these — that's css/js, not text.
                if name != "style" && name != "script" {
                    let block = is_block(name);
                    if block || name == "br" {
                        builder.newline();
                    }
                    // an <a> introduces a target; nested elements inherit it.
                    let child_href = el.attr("href").or(href);
                    walk(
                        current,
                        child_style(name, style),
                        child_href,
                        builder,
                        depth + 1,
                    );
                    if block {
                        builder.newline();
                    }
                }
            }
            _ => {}
        }
        child = current.next_sibling();
    }
}

/// derive the child style from a tag name (inheriting the parent's).
fn child_style(name: &str, mut s: Style) -> Style {
    match name {
        "b" | "strong" => s.bold = true,
        "i" | "em" | "cite" | "var" => s.italic = true,
        "sup" => {
            s.sup = true;
            s.scale *= 0.83;
        }
        "sub" => {
            s.sub = true;
            s.scale *= 0.83;
        }
        "a" | "kref" => s.link = true,
        "tt" | "code" | "kbd" | "samp" | "pre" => s.mono = true,
        "small" => s.scale *= 0.9,
        "big" => s.scale *= 1.1,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            s.bold = true;
            s.scale *= 1.15;
        }
        _ => {}
    }
    s
}

/// tags whose boundaries imply a line break.
fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "li"
            | "tr"
            | "ul"
            | "ol"
            | "dl"
            | "dt"
            | "dd"
            | "table"
            | "section"
            | "article"
            | "blockquote"
            | "hr"
            | "center"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
    )
}

/// the paragraph mark some dictionaries divide their senses with, in place of markup.
const PILCROW: char = '¶';

/// accumulates runs, collapsing whitespace and coalescing block breaks: runs of
/// whitespace become a single space, block boundaries a single newline, and
/// leading/trailing space/newlines are dropped.
#[derive(Default)]
struct Builder {
    runs: Vec<Run>,
    space_pending: bool,
    newline_pending: bool,
    any_output: bool,
}

impl Builder {
    fn text(&mut self, s: &str, style: Style, href: Option<&str>) {
        let mut chars = s.chars().peekable();
        while let Some(ch) = chars.next() {
            // `||` divides a sense into sub-paragraphs in Gaffiot, and unlike a pilcrow
            // it names nothing — so the break it asks for replaces it rather than
            // joining it. a lone `|` is left alone; it is a real character elsewhere.
            if ch == '|' && chars.peek() == Some(&'|') {
                chars.next();
                self.newline();
                continue;
            }
            if ch.is_whitespace() {
                if self.any_output {
                    self.space_pending = true;
                }
            } else if ch == PILCROW {
                // a pilcrow *is* a paragraph mark, and some dictionaries mark their
                // senses with one instead of with any markup at all: Gaffiot's entries
                // are a single inline run divided by `¶ 1`, `¶ 2`, … exactly as its
                // print edition divides them (#58). without this the whole article
                // arrives as one paragraph, because there is no block element in it to
                // break on and nothing downstream can invent one.
                //
                // the character is kept rather than swallowed. it is the notation the
                // dictionary itself uses, a reader of Gaffiot knows it, and the line it
                // now begins is what marks the sense out as a sense.
                self.newline();
                self.flush_pending();
                self.push(ch, style, href);
            } else {
                self.flush_pending();
                self.push(ch, style, href);
            }
        }
    }

    fn newline(&mut self) {
        if self.any_output {
            self.newline_pending = true;
            self.space_pending = false; // a newline supersedes a pending space.
        }
    }

    /// emit a pending newline or space (newline wins) before real content. the
    /// separator itself belongs to no link.
    fn flush_pending(&mut self) {
        if self.newline_pending {
            self.push('\n', Style::default(), None);
        } else if self.space_pending {
            self.push(' ', Style::default(), None);
        }
        self.newline_pending = false;
        self.space_pending = false;
    }

    /// append one char, extending the last run when style AND link target match —
    /// two adjacent links must not merge into one run, or the second would inherit
    /// the first one's target. only a new run allocates the href.
    fn push(&mut self, ch: char, style: Style, href: Option<&str>) {
        self.any_output = true;
        match self.runs.last_mut() {
            Some(last) if last.style == style && last.href.as_deref() == href => last.text.push(ch),
            _ => self.runs.push(Run {
                text: ch.to_string(),
                style,
                href: href.map(str::to_string),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absurd_nesting_is_cut_off_rather_than_overflowing_the_stack() {
        // 40k of these aborted the process before the cap; the text up to the cap is
        // still rendered, and nothing panics.
        let html = "<b>".repeat(40_000) + "deep" + &"</b>".repeat(40_000);
        let text = to_text(&html);
        assert!(text.is_empty() || text == "deep");
    }

    #[test]
    fn maps_bold_italic_and_collapses_whitespace() {
        let runs = to_runs("<b>alpha</b>   plain   <i>beta</i>");
        // "alpha" bold, " plain " plain, "beta" italic.
        assert_eq!(runs[0].text, "alpha");
        assert!(runs[0].style.bold);
        assert!(to_text("<b>alpha</b>   plain   <i>beta</i>") == "alpha plain beta");
    }

    #[test]
    fn drops_style_and_script_bodies() {
        let text = to_text("<style>a{color:red}</style><b>hi</b><script>x=1</script> there");
        assert_eq!(text, "hi there");
    }

    #[test]
    fn a_pilcrow_starts_a_line_and_stays() {
        // Gaffiot divides its senses with `¶ N` and no markup at all, so this is the
        // only thing that can turn one article into paragraphs (#58).
        assert_eq!(
            to_text("<b> ¶ 1</b> one <b>¶ 2</b> two"),
            "¶ 1 one\n¶ 2 two"
        );
        // no leading newline from a pilcrow that opens the entry
        assert_eq!(to_text("<b>¶ 1</b> only"), "¶ 1 only");
    }

    #[test]
    fn a_double_pipe_breaks_the_line_and_goes() {
        // a sub-division marker names nothing, so the break replaces it.
        assert_eq!(to_text("one || two"), "one\ntwo");
        // a single pipe is a character like any other.
        assert_eq!(to_text("a | b"), "a | b");
    }

    #[test]
    fn block_tags_become_single_newlines() {
        assert_eq!(to_text("<p>one</p><p>two</p>"), "one\ntwo");
        assert_eq!(to_text("a<br>b"), "a\nb");
        // no leading/trailing newline despite the wrapping <p>s.
        assert!(!to_text("<p>x</p>").starts_with('\n'));
    }

    #[test]
    fn decodes_entities_and_keeps_unicode() {
        // html5ever decodes entities during parsing.
        assert_eq!(to_text("&#945;&#946; &amp; Ελληνικά"), "αβ & Ελληνικά");
    }

    #[test]
    fn captures_the_link_target_not_the_visible_text() {
        // a hebrew-hebrew dictionary's real shape: uppercase <A>, bword scheme, and a target
        // spelled differently from the text shown (here without the geresh).
        let runs = to_runs(r#"see <A href="bword://ציק צק">צִ'יק צָ'ק</A> now"#);
        let link = runs.iter().find(|r| r.style.link).expect("a link run");
        assert_eq!(link.href.as_deref(), Some("bword://ציק צק"));
        assert_eq!(
            link_target(link.href.as_deref().unwrap()).unwrap(),
            "ציק צק"
        );
        // text outside the anchor carries no target.
        assert!(
            runs.iter()
                .any(|r| r.text.contains("see") && r.href.is_none())
        );
    }

    #[test]
    fn adjacent_links_stay_separate_runs() {
        let runs = to_runs(r#"<a href="bword://one">x</a><a href="bword://two">y</a>"#);
        let targets: Vec<_> = runs.iter().filter_map(|r| r.href.as_deref()).collect();
        assert_eq!(targets, ["bword://one", "bword://two"]);
    }

    #[test]
    fn nested_markup_inherits_the_enclosing_link() {
        let runs = to_runs(r#"<a href="bword://lemma"><b>bold</b> plain</a>"#);
        assert!(
            runs.iter()
                .filter(|r| !r.text.trim().is_empty())
                .all(|r| r.href.as_deref() == Some("bword://lemma"))
        );
    }

    #[test]
    fn link_target_only_follows_links_into_the_collection() {
        assert_eq!(link_target("bword://dacrima").unwrap(), "dacrima");
        assert_eq!(link_target("bword:dacrima").unwrap(), "dacrima");
        assert_eq!(link_target("dacrima").unwrap(), "dacrima");
        // multi-word headwords are common in the hebrew dictionaries.
        assert_eq!(link_target("bword://זה לקבל זה").unwrap(), "זה לקבל זה");
        // and these lead out of the app, so they are not followable.
        assert!(link_target("http://example.com").is_none());
        assert!(link_target("https://example.com/x").is_none());
        assert!(link_target("mailto:someone@example.com").is_none());
        assert!(link_target("#anchor").is_none());
        assert!(link_target("").is_none());
    }

    #[test]
    fn sup_carries_flag_and_shrinks() {
        let runs = to_runs("x<sup>2</sup>");
        let sup = runs.iter().find(|r| r.text == "2").unwrap();
        assert!(sup.style.sup && sup.style.scale < 1.0);
    }
}
