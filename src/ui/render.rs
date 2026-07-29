//! definition typography: the `gtk::TextTag`s a rendered definition is dressed
//! in, and the paragraph structure laid over its body.
//!
//! tags are looked up by name and created once per buffer, so a definition with a
//! thousand italic runs holds one italic tag. what is public here is what the
//! window actually asks for; the scales, the link colour and the sense-detection
//! are this module's business.

use gtk::prelude::*;

use crate::dict;

// definition typography. bumped a bit larger per feedback; the markup scales
// (sup/headers/…) multiply on top of BODY_SCALE.
const BODY_SCALE: f64 = 1.3;
const HEAD_SCALE: f64 = 1.6;
const LINK_COLOR: &str = "#3584e4"; // gnome accent blue, same for every dict.

/// tag-name prefix marking a link tag; what follows is the target headword. the
/// separator is a unit separator, which can't occur in a headword.
pub(super) const LINK_PREFIX: &str = "link\u{1f}";

/// an otherwise inert tag whose *name* carries a link's target, so a click can
/// recover it from the buffer. the blue-and-underlined look comes from the run's
/// own style tag, not from here.
pub(super) fn link_tag(buffer: &gtk::TextBuffer, target: &str) -> gtk::TextTag {
    let name = format!("{LINK_PREFIX}{target}");
    buffer.tag_table().lookup(&name).unwrap_or_else(|| {
        let tag = gtk::TextTag::builder().name(&name).build();
        buffer.tag_table().add(&tag);
        tag
    })
}

/// the bold+large headword tag (cached in the buffer's tag table).
pub(super) fn head_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
    buffer.tag_table().lookup("head").unwrap_or_else(|| {
        let tag = gtk::TextTag::builder()
            .name("head")
            .weight(700)
            .scale(HEAD_SCALE)
            .pixels_below_lines(4)
            .build();
        buffer.tag_table().add(&tag);
        tag
    })
}

/// the dim, small label marking which dictionary a definition came from. the
/// generous space above it is what separates one dictionary's answer from the
/// next; the letter spacing makes it read as a heading rather than as text.
pub(super) fn source_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
    buffer.tag_table().lookup("source").unwrap_or_else(|| {
        let tag = gtk::TextTag::builder()
            .name("source")
            .weight(700)
            .scale(0.85)
            .foreground("#808080")
            .letter_spacing(600)
            .pixels_above_lines(22)
            .pixels_below_lines(10)
            .build();
        buffer.tag_table().add(&tag);
        tag
    })
}

/// the "3 of 71" counter above an entry, when a headword has more than one in the
/// same dictionary. quieter than the dictionary heading — it separates answers
/// within a section, it doesn't start a new one.
pub(super) fn entry_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
    buffer.tag_table().lookup("entry").unwrap_or_else(|| {
        let tag = gtk::TextTag::builder()
            .name("entry")
            .scale(0.8)
            .foreground("#808080")
            .left_margin(28)
            .pixels_above_lines(16)
            .pixels_below_lines(2)
            .build();
        buffer.tag_table().add(&tag);
        tag
    })
}

/// give one dictionary's definition its shape: the whole body sits indented under
/// its heading, and each sense inside it gets a hanging indent plus air above, so
/// senses read as a list instead of one paragraph. `from`/`to` are line indices.
pub(super) fn structure_body(buffer: &gtk::TextBuffer, from: i32, to: i32) {
    let (Some(start), Some(end)) = (buffer.iter_at_line(from), buffer.iter_at_line(to)) else {
        return;
    };
    // create the body tag before the sense tag: gtk resolves conflicting tags by
    // insertion order, so the sense indent has to be the later of the two.
    let body = body_tag(buffer);
    let sense = sense_tag(buffer);
    buffer.apply_tag(&body, &start, &end);

    for line in from..to {
        let Some(line_start) = buffer.iter_at_line(line) else {
            continue;
        };
        let mut line_end = line_start;
        if !line_end.ends_line() {
            line_end.forward_to_line_end();
        }
        let text = buffer.text(&line_start, &line_end, false);
        if starts_a_sense(&text) {
            buffer.apply_tag(&sense, &line_start, &line_end);
        }
    }
}

/// whether a line opens a new sense — a dash marker (Lewis & Short) or a numbered
/// or roman-numbered one (Liddell, and most glossaries).
fn starts_a_sense(line: &str) -> bool {
    let line = line.trim_start();
    // `¶ 1` is how Gaffiot divides its senses, and markup.rs gives each one a line of
    // its own (#58); the number after it is not followed by a dot, so the digit rule
    // below would miss it.
    if line.starts_with('¶') {
        return true;
    }
    if let Some(rest) = line.strip_prefix('-') {
        return rest.starts_with(' ');
    }
    let marker: String = line
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, 'I' | 'V' | 'X' | 'i' | 'v' | 'x'))
        .collect();
    if marker.is_empty() {
        return false;
    }
    matches!(line[marker.len()..].chars().next(), Some('.') | Some(')'))
}

/// the whole of one dictionary's definition, indented under its heading. note a
/// tag's `left_margin` REPLACES the view's (18), it doesn't add to it — so this has
/// to exceed 18 to read as an indent at all.
fn body_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
    buffer.tag_table().lookup("body").unwrap_or_else(|| {
        let tag = gtk::TextTag::builder().name("body").left_margin(28).build();
        buffer.tag_table().add(&tag);
        tag
    })
}

/// one sense: hanging indent, so its marker sits out in the margin and the wrapped
/// lines line up under the text rather than under the marker.
fn sense_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
    buffer.tag_table().lookup("sense").unwrap_or_else(|| {
        let tag = gtk::TextTag::builder()
            .name("sense")
            .left_margin(46)
            .indent(-16)
            .pixels_above_lines(10)
            .build();
        buffer.tag_table().add(&tag);
        tag
    })
}

/// a TextTag realizing one markup `Style`, cached by a name derived from the
/// style so we create each distinct combination only once.
pub(super) fn style_tag(buffer: &gtk::TextBuffer, style: dict::markup::Style) -> gtk::TextTag {
    let name = format!(
        "r{}{}{}{}{}{:.3}",
        style.bold as u8,
        style.italic as u8,
        style.link as u8,
        style.mono as u8,
        if style.sup { 2 } else { u8::from(style.sub) },
        style.scale,
    );
    if let Some(tag) = buffer.tag_table().lookup(&name) {
        return tag;
    }
    let mut builder = gtk::TextTag::builder()
        .name(&name)
        .scale(style.scale * BODY_SCALE);
    if style.bold {
        builder = builder.weight(700);
    }
    if style.italic {
        builder = builder.style(gtk::pango::Style::Italic);
    }
    if style.mono {
        builder = builder.family("monospace");
    }
    if style.link {
        builder = builder
            .foreground(LINK_COLOR)
            .underline(gtk::pango::Underline::Single);
    }
    if style.sup {
        builder = builder.rise(6000); // pango units (~5.8pt up)
    } else if style.sub {
        builder = builder.rise(-3000);
    }
    let tag = builder.build();
    buffer.tag_table().add(&tag);
    tag
}
