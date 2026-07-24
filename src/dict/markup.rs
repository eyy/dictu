//! turn a dictionary's html definition into a flat list of styled text runs.
//!
//! we map a controlled subset of tags (b/i/sup/sub/a/headers/…) to OUR own
//! semantic styling and ignore each dict's inline css, colors, and classes — so
//! every dictionary renders in one consistent look. this module is ui-agnostic
//! (no gtk): the ui turns `Style` into TextView tags. no browser engine — just
//! html5ever parsing via `scraper`.

use ego_tree::NodeRef;
use scraper::{Html, Node};

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

/// a contiguous run of text sharing one style.
pub struct Run {
    pub text: String,
    pub style: Style,
}

/// parse `html` and return whitespace-normalized styled runs.
pub fn to_runs(html: &str) -> Vec<Run> {
    let doc = Html::parse_fragment(html);
    let mut builder = Builder::default();
    walk(doc.tree.root(), Style::default(), &mut builder);
    builder.runs
}

/// the plain text of the html (runs concatenated) — for the `dump` cli and any
/// plain-text fallback.
pub fn to_text(html: &str) -> String {
    to_runs(html).into_iter().map(|r| r.text).collect()
}

/// walk the dom, pushing text into `builder` with the current inherited style.
fn walk(node: NodeRef<Node>, style: Style, builder: &mut Builder) {
    let mut child = node.first_child();
    while let Some(current) = child {
        match current.value() {
            Node::Text(text) => builder.text(text, style),
            Node::Element(el) => {
                let name = el.name();
                // never render the *contents* of these — that's css/js, not text.
                if name != "style" && name != "script" {
                    let block = is_block(name);
                    if block || name == "br" {
                        builder.newline();
                    }
                    walk(current, child_style(name, style), builder);
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
    fn text(&mut self, s: &str, style: Style) {
        for ch in s.chars() {
            if ch.is_whitespace() {
                if self.any_output {
                    self.space_pending = true;
                }
            } else {
                self.flush_pending();
                self.push(ch, style);
            }
        }
    }

    fn newline(&mut self) {
        if self.any_output {
            self.newline_pending = true;
            self.space_pending = false; // a newline supersedes a pending space.
        }
    }

    /// emit a pending newline or space (newline wins) before real content.
    fn flush_pending(&mut self) {
        if self.newline_pending {
            self.push('\n', Style::default());
        } else if self.space_pending {
            self.push(' ', Style::default());
        }
        self.newline_pending = false;
        self.space_pending = false;
    }

    /// append one char, extending the last run if the style matches.
    fn push(&mut self, ch: char, style: Style) {
        self.any_output = true;
        match self.runs.last_mut() {
            Some(last) if last.style == style => last.text.push(ch),
            _ => self.runs.push(Run {
                text: ch.to_string(),
                style,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn sup_carries_flag_and_shrinks() {
        let runs = to_runs("x<sup>2</sup>");
        let sup = runs.iter().find(|r| r.text == "2").unwrap();
        assert!(sup.style.sup && sup.style.scale < 1.0);
    }
}
