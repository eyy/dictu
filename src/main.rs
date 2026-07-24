// dictu — a gnome dictionary app.
//
// the app reads config.toml for a list of directories, scans them for
// dictionaries, and builds one merged index across ALL of them (off the main
// thread, with an "Indexing…" state). the search box queries every dictionary
// at once; a result shows its definition from each dict that has it.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod config;
mod dict;
mod language;
mod library;
use library::Library;

const APP_ID: &str = "io.github.eyy.Dictu";

// cap search results shown (gtk::ListBox builds one widget per row).
const SEARCH_LIMIT: usize = 500;

/// the loaded index, shared across signal handlers. `None` until indexing
/// finishes on the worker thread.
type SharedLibrary = Rc<RefCell<Option<Library>>>;

fn main() -> glib::ExitCode {
    let raw: Vec<String> = std::env::args().collect();

    // dev affordances (no gui): `dictu dump <file>` prints one dictionary's
    // stats; `dictu lookup <file> <word> [--html]` prints one entry, which is how
    // two dictionaries' coverage of the same word get compared; `dictu search
    // <query>` runs unified search across all configured dicts and prints the hits.
    let subcommand = raw.get(1).map(String::as_str);
    if subcommand == Some("dump") {
        return dump(raw.get(2).map(String::as_str));
    }
    if subcommand == Some("lookup") {
        return lookup(
            raw.get(2).map(String::as_str),
            raw.get(3).map(String::as_str),
            raw.iter().any(|arg| arg == "--html"),
        );
    }
    if subcommand == Some("search") {
        return search_cli(raw.get(2).map(String::as_str));
    }

    // scan the configured directories for dictionaries once, up front.
    let config = config::Config::load_or_create().unwrap_or_default();
    let entries = Rc::new(config::scan(&config.dictionary_dirs));

    // HANDLES_COMMAND_LINE makes the app single-instance: a second
    // `dictu --search foo` (e.g. from the global hotkey) forwards its argv to
    // the already-running instance's command-line handler below, which focuses
    // the window and fills the search box instead of opening a new window.
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    // the single Ui, built on the first invocation and reused after.
    let ui_cell: Rc<RefCell<Option<Ui>>> = Rc::new(RefCell::new(None));
    app.connect_command_line(move |app, cmdline| {
        let argv: Vec<String> = cmdline
            .arguments()
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();

        let ui = ui_cell
            .borrow_mut()
            .get_or_insert_with(|| build_ui(app, &entries))
            .clone();

        // the hotkey passes the selected word here; fill the search box with it and
        // let the results select their first row, so the definition is on screen by
        // the time you look at the window.
        if let Some(word) = parse_flag(&argv, "--search") {
            ui.auto_select.set(true);
            ui.search.set_text(&word);
            ui.search.grab_focus();
        }
        ui.window.present();
        0
    });

    app.run_with_args(&raw)
}

/// value following `flag` in `args` (e.g. `--open /path`), if present.
fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| a == flag)?;
    args.get(idx + 1).cloned()
}

fn search_cli(query: Option<&str>) -> glib::ExitCode {
    let Some(query) = query else {
        eprintln!("usage: dictu search <query>");
        return glib::ExitCode::FAILURE;
    };
    let config = config::Config::load_or_create().unwrap_or_default();
    let entries = config::scan(&config.dictionary_dirs);
    let lib = library::Library::open(&entries);
    println!(
        "{} dicts, {} headwords total",
        lib.dict_count(),
        lib.total_headwords()
    );
    for hit in lib.prefix_search(query, 20, &[]) {
        let label = lib.dict_label(hit.dict).unwrap_or("?");
        println!("  [{label}] {}", hit.word);
    }
    glib::ExitCode::SUCCESS
}

fn dump(path: Option<&str>) -> glib::ExitCode {
    let Some(path) = path else {
        eprintln!("usage: dictu dump <dictionary-file>");
        return glib::ExitCode::FAILURE;
    };
    match dict::open_any(Path::new(path)) {
        Ok(dict) => {
            println!("name:      {}", dict.name());
            let headwords = dict.headwords();
            println!("headwords: {}", headwords.len());
            for word in headwords.iter().take(5) {
                let entries = dict.lookup(word);
                let text = entries
                    .first()
                    .map(|e| dict::html_to_text(e))
                    .unwrap_or_default();
                let preview: String = text.chars().take(100).collect();
                // say when a headword has more than one entry; that is easy to miss
                // and it is exactly what made `sam` look broken.
                let more = match entries.len() {
                    0 | 1 => String::new(),
                    n => format!("  [{n} entries]"),
                };
                println!("  {word:?} -> {preview:?}{more}");
            }
            glib::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            glib::ExitCode::FAILURE
        }
    }
}

/// print one word's entry from one dictionary — the plain text by default, the
/// raw html with `--html` (which is what markup work needs to see).
fn lookup(path: Option<&str>, word: Option<&str>, html: bool) -> glib::ExitCode {
    let (Some(path), Some(word)) = (path, word) else {
        eprintln!("usage: dictu lookup <dictionary-file> <word> [--html]");
        return glib::ExitCode::FAILURE;
    };
    let dict = match dict::open_any(Path::new(path)) {
        Ok(dict) => dict,
        Err(e) => {
            eprintln!("error: {e:#}");
            return glib::ExitCode::FAILURE;
        }
    };
    let entries = dict.lookup(word);
    if entries.is_empty() {
        println!("{}: no entry for {word:?}", dict.name());
        return glib::ExitCode::FAILURE;
    }
    println!(
        "{} — {word} ({})\n",
        dict.name(),
        quantity(entries.len(), "entry", "entries")
    );
    for (position, entry) in entries.iter().enumerate() {
        if entries.len() > 1 {
            println!("--- {} of {} ---", position + 1, entries.len());
        }
        println!(
            "{}",
            if html {
                entry.clone()
            } else {
                dict::html_to_text(entry)
            }
        );
    }
    glib::ExitCode::SUCCESS
}

/// the widgets + state a load touches, bundled so signal closures capture one
/// cheap `Ui` clone instead of half a dozen individual widget handles.
#[derive(Clone)]
struct Ui {
    window: adw::ApplicationWindow,
    search: gtk::SearchEntry,
    results: gtk::ListBox,
    definition: gtk::TextView,
    status: gtk::Label,
    /// the strip under the definition naming what is still below the fold.
    fold: gtk::Label,
    /// one entry per dictionary section in the definition currently shown: its
    /// label, and a mark at the line it starts on. marks (not line numbers) because
    /// they survive the buffer being rewritten under them.
    sections: Rc<RefCell<Vec<(String, gtk::TextMark)>>>,
    /// the words in the wordlist, in row order — a row is now a box of two labels,
    /// so its word is looked up by index rather than read back out of a widget.
    words: Rc<RefCell<Vec<String>>>,
    /// set when a search arrived from outside (`--search`, i.e. the global hotkey):
    /// the next set of results selects its first row on its own. a flag rather than a
    /// timer because `SearchEntry` debounces `search-changed`, so there is no moment
    /// after `set_text` at which the rows are known to exist yet.
    auto_select: Rc<Cell<bool>>,
    library: SharedLibrary,
}

impl Ui {
    /// plain message in the definition pane (hint / "no definition").
    fn set_message(&self, text: &str) {
        self.definition.buffer().set_text(text);
    }

    /// rebuild the results list from a prefix search across the whole index,
    /// deduping repeated headwords (a word in several dicts appears once; the
    /// per-dict definitions show when it's selected).
    fn populate_results(&self, query: &str) {
        while let Some(child) = self.results.first_child() {
            self.results.remove(&child);
        }
        let borrow = self.library.borrow();
        let Some(library) = borrow.as_ref() else {
            return;
        };

        let query = query.trim();
        // `&[]` scopes to every dict; the scope panel (roadmap #14) passes a real mask.
        let hits = library.prefix_search(query, SEARCH_LIMIT, &[]);
        let mut seen = HashSet::new();
        self.words.borrow_mut().clear();
        for hit in &hits {
            if !seen.insert(hit.word.to_lowercase()) {
                continue;
            }
            let dict_name = library.dict_label(hit.dict).unwrap_or("");
            self.words.borrow_mut().push(hit.word.clone());
            self.results.append(&word_row(&hit.word, dict_name));
            // name the row after its word: the row is a box of two labels now, so
            // without this a screen reader (and the e2e harness) would read the
            // language tag as part of the entry.
            if let Some(row) = self.results.last_child().and_downcast::<gtk::ListBoxRow>() {
                row.update_property(&[gtk::accessible::Property::Label(&hit.word)]);
            }
        }

        if query.is_empty() {
            self.set_message("Type to search all dictionaries.");
            self.show_library_size();
            return;
        }
        if seen.is_empty() {
            self.set_message(&format!("No matches for “{query}”."));
        }
        // count what the list actually shows (deduped), not index entries. the
        // search stops at SEARCH_LIMIT, so say "500+" instead of pretending 500
        // is the whole truth.
        self.status.set_text(&if hits.len() >= SEARCH_LIMIT {
            format!("{}+ results", thousands(seen.len()))
        } else {
            quantity(seen.len(), "result", "results")
        });

        // a search fired from the hotkey should land on an answer, not on a list you
        // still have to click. consumed either way, so a later hand-typed search
        // doesn't inherit it.
        if self.auto_select.replace(false) {
            self.select_first_row();
        }
    }

    /// select the first row, which is what renders its definition. focus stays where
    /// it is, so a search arriving from the hotkey shows an answer without taking the
    /// search box away from you mid-typing.
    fn select_first_row(&self) {
        if let Some(row) = self.results.row_at_index(0) {
            self.results.select_row(Some(&row));
        }
    }

    /// the same, but move focus into the wordlist too — what Down does. gtk's own row
    /// navigation takes over from there.
    fn focus_first_row(&self) {
        self.select_first_row();
        if let Some(row) = self.results.row_at_index(0) {
            row.grab_focus();
        }
    }

    /// send a printable keypress to the search box wherever focus happens to be,
    /// so the search box never has to be aimed for. modified keys are shortcuts
    /// rather than typing, and keys already destined for the search box are left
    /// alone.
    fn redirect_typing(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let shortcut = state.intersects(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK,
        );
        if shortcut || self.search_has_focus() {
            return glib::Propagation::Proceed;
        }
        // control characters (escape, backspace, tab, the arrows) are navigation,
        // not text — leave them to the widget that has focus.
        let Some(ch) = key.to_unicode().filter(|c| !c.is_control()) else {
            return glib::Propagation::Proceed;
        };
        self.search.grab_focus();
        self.search.set_text(&format!("{}{ch}", self.search.text()));
        self.search.set_position(-1);
        glib::Propagation::Stop
    }

    /// whether focus is in the search box. gtk4 puts focus on the `GtkText`
    /// *inside* a `SearchEntry`, so the entry itself is only an ancestor of it.
    fn search_has_focus(&self) -> bool {
        let search: &gtk::Widget = self.search.upcast_ref();
        // spelled out because `Root` and `GtkWindow` both define `focus`.
        gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|focused| &focused == search || focused.is_ancestor(&self.search))
    }

    /// the idle status line: how much is loaded.
    fn show_library_size(&self) {
        let borrow = self.library.borrow();
        let Some(library) = borrow.as_ref() else {
            return;
        };
        self.status.set_text(&format!(
            "{} · {}",
            quantity(library.total_headwords(), "word", "words"),
            quantity(library.dict_count(), "dictionary", "dictionaries"),
        ));
    }

    /// the link target under widget coordinates `(x, y)`, if any — read back off
    /// the invisible tag `show_word` attached to the link's text.
    fn link_at(&self, x: f64, y: f64) -> Option<String> {
        let (bx, by) = self.definition.window_to_buffer_coords(
            gtk::TextWindowType::Widget,
            x.round() as i32,
            y.round() as i32,
        );
        let iter = self.definition.iter_at_location(bx, by)?;
        iter.tags().iter().find_map(|tag| {
            tag.name()
                .and_then(|name| name.strip_prefix(LINK_PREFIX).map(str::to_string))
        })
    }

    /// follow a link: make it the current search, so the wordlist agrees with the
    /// definition on screen and Back-by-retyping still works.
    fn follow_link(&self, target: &str) {
        self.search.set_text(target);
        self.search.set_position(-1);
        self.show_word(target);
    }

    /// show a word's definition(s): the headword bold+large, then each dict that
    /// defines it under a dim source label, its html parsed into styled runs
    /// rendered with OUR uniform tags (dict css ignored → consistent look).
    fn show_word(&self, word: &str) {
        let borrow = self.library.borrow();
        let Some(library) = borrow.as_ref() else {
            return;
        };
        let defs = library.lookup_all(word);

        let buffer = self.definition.buffer();
        self.clear_sections(&buffer);
        buffer.set_text("");
        if defs.is_empty() {
            buffer.set_text(&format!("No definition for “{word}”."));
            self.update_fold();
            return;
        }

        let mut iter = buffer.start_iter();
        buffer.insert_with_tags(&mut iter, &format!("{word}\n"), &[&head_tag(&buffer)]);
        for (dict_index, entries) in &defs {
            let label = library.dict_label(*dict_index).unwrap_or("");
            // remember where this dictionary's answer starts, so the strip below the
            // pane can say which ones are still out of sight.
            let mark = buffer.create_mark(None, &iter, true);
            self.sections.borrow_mut().push((label.to_string(), mark));
            // no blank line: the heading's own space-above is what separates
            // sections, and a literal newline on top of it just leaves a hole.
            buffer.insert_with_tags(&mut iter, &format!("{label}\n"), &[&source_tag(&buffer)]);

            for (position, html) in entries.iter().enumerate() {
                // a word can be filed under dozens of entries in one dictionary
                // (`esse` under 100 of them). number them, so a wall of answers
                // reads as a list and you can see how deep it goes.
                if entries.len() > 1 {
                    let counter = format!("{} of {}\n", position + 1, entries.len());
                    buffer.insert_with_tags(&mut iter, &counter, &[&entry_tag(&buffer)]);
                }

                let body_start = iter.line();
                for run in dict::markup::to_runs(html) {
                    let style = style_tag(&buffer, run.style);
                    // a followable link gets a second, invisible tag carrying its
                    // target, which is how a click maps back to a headword.
                    match run.href.as_deref().and_then(dict::markup::link_target) {
                        Some(target) => {
                            let link = link_tag(&buffer, &target);
                            buffer.insert_with_tags(&mut iter, &run.text, &[&style, &link]);
                        }
                        None => buffer.insert_with_tags(&mut iter, &run.text, &[&style]),
                    }
                }
                buffer.insert(&mut iter, "\n");
                structure_body(&buffer, body_start, iter.line());
            }
        }
        // gtk needs to lay the buffer out before any of it has a position, so ask
        // once the layout has settled rather than measuring nothing here.
        let ui = self.clone();
        glib::idle_add_local_once(move || ui.update_fold());
    }

    /// forget the previous definition's section marks (a mark lives in the buffer
    /// until deleted, so leaving them behind accumulates them).
    fn clear_sections(&self, buffer: &gtk::TextBuffer) {
        for (_, mark) in self.sections.borrow_mut().drain(..) {
            buffer.delete_mark(&mark);
        }
    }

    /// say what is still below the fold: how many further dictionaries define this
    /// word, and which. hidden when everything already fits.
    fn update_fold(&self) {
        let sections = self.sections.borrow();
        let visible = self.definition.visible_rect();
        let fold = visible.y() + visible.height();
        let buffer = self.definition.buffer();

        let below: Vec<&str> = sections
            .iter()
            .filter(|(_, mark)| {
                let iter = buffer.iter_at_mark(mark);
                self.definition.iter_location(&iter).y() >= fold
            })
            .map(|(label, _)| label.as_str())
            .collect();

        self.fold.set_visible(!below.is_empty());
        if below.is_empty() {
            return;
        }
        self.fold.set_text(&format!(
            "{} below: {}",
            quantity(below.len(), "more definition", "more definitions"),
            below.join(" · "),
        ));
    }
}

// definition typography. bumped a bit larger per feedback; the markup scales
// (sup/headers/…) multiply on top of BODY_SCALE.
const BODY_SCALE: f64 = 1.3;
const HEAD_SCALE: f64 = 1.6;
const LINK_COLOR: &str = "#3584e4"; // gnome accent blue, same for every dict.

/// tag-name prefix marking a link tag; what follows is the target headword. the
/// separator is a unit separator, which can't occur in a headword.
const LINK_PREFIX: &str = "link\u{1f}";

/// an otherwise inert tag whose *name* carries a link's target, so a click can
/// recover it from the buffer. the blue-and-underlined look comes from the run's
/// own style tag, not from here.
fn link_tag(buffer: &gtk::TextBuffer, target: &str) -> gtk::TextTag {
    let name = format!("{LINK_PREFIX}{target}");
    buffer.tag_table().lookup(&name).unwrap_or_else(|| {
        let tag = gtk::TextTag::builder().name(&name).build();
        buffer.tag_table().add(&tag);
        tag
    })
}

/// the bold+large headword tag (cached in the buffer's tag table).
fn head_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
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
fn source_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
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
fn entry_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
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
fn structure_body(buffer: &gtk::TextBuffer, from: i32, to: i32) {
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
fn style_tag(buffer: &gtk::TextBuffer, style: dict::markup::Style) -> gtk::TextTag {
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

fn build_ui(app: &adw::Application, entries: &[config::DictEntry]) -> Ui {
    // -- sidebar: search box, results list, status line ---------------------
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Indexing…"));
    search.set_sensitive(false); // enabled once the index finishes building.

    let results = gtk::ListBox::new();
    results.add_css_class("navigation-sidebar");

    let status = gtk::Label::builder()
        .label("Indexing dictionaries…")
        .xalign(0.0)
        .build();
    status.add_css_class("dim-label");
    status.set_margin_start(6);
    status.set_margin_bottom(2);

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 6);
    sidebar.set_margin_top(6);
    sidebar.set_margin_bottom(6);
    sidebar.set_margin_start(6);
    sidebar.set_margin_end(6);
    sidebar.append(&search);
    let results_scroll = gtk::ScrolledWindow::new();
    results_scroll.set_child(Some(&results));
    results_scroll.set_vexpand(true);
    sidebar.append(&results_scroll);
    sidebar.append(&status);

    // definition pane: read-only TextView for rich text + real line spacing.
    let definition = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(18)
        .right_margin(18)
        .top_margin(18)
        .bottom_margin(18)
        .pixels_below_lines(8)
        .pixels_inside_wrap(3)
        .build();
    definition
        .buffer()
        .set_text("Indexing dictionaries…\n\nThis runs once at startup.");
    let def_scroll = gtk::ScrolledWindow::new();
    def_scroll.set_child(Some(&definition));
    def_scroll.set_hexpand(true);
    def_scroll.set_vexpand(true);

    // the strip naming the definitions still below the fold. hidden until there
    // are any, so it costs nothing on a single-dictionary word.
    let fold = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .margin_start(18)
        .margin_end(18)
        .margin_top(4)
        .margin_bottom(6)
        .visible(false)
        .build();
    fold.add_css_class("dim-label");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&def_scroll);
    content.append(&fold);

    // adw::OverlaySplitView: idiomatic sidebar+content (handles csd sizing).
    let split = adw::OverlaySplitView::new();
    split.set_sidebar(Some(&sidebar));
    split.set_content(Some(&content));
    split.set_collapsed(false);
    split.set_min_sidebar_width(260.0);
    split.set_max_sidebar_width(380.0);
    split.set_sidebar_width_fraction(0.32);

    let header = adw::HeaderBar::new();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Dictu")
        .default_width(900)
        .default_height(600)
        .content(&toolbar_with(&header, &split))
        .build();

    let ui = Ui {
        window: window.clone(),
        search: search.clone(),
        results: results.clone(),
        definition,
        status,
        fold: fold.clone(),
        sections: Rc::new(RefCell::new(Vec::new())),
        words: Rc::new(RefCell::new(Vec::new())),
        auto_select: Rc::new(Cell::new(false)),
        library: Rc::new(RefCell::new(None)),
    };

    // scrolling changes what's below the fold, so the strip follows it.
    let ui_scroll = ui.clone();
    def_scroll
        .vadjustment()
        .connect_value_changed(move |_| ui_scroll.update_fold());

    // typing searches the whole index; selecting a result shows its def(s).
    let ui_search = ui.clone();
    search.connect_search_changed(move |entry| ui_search.populate_results(&entry.text()));

    let ui_select = ui.clone();
    results.connect_row_selected(move |_list, row| {
        let Some(row) = row else { return };
        let word = usize::try_from(row.index())
            .ok()
            .and_then(|index| ui_select.words.borrow().get(index).cloned());
        if let Some(word) = word {
            ui_select.show_word(&word);
        }
    });

    // clicking a link in a definition looks that word up (roadmap #20).
    let ui_link = ui.clone();
    let click = gtk::GestureClick::new();
    // capture phase: the textview's own drag-select gesture claims the sequence
    // otherwise, and this handler never hears about the release.
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_released(move |gesture, _clicks, x, y| {
        let Some(target) = ui_link.link_at(x, y) else {
            return; // an ordinary click: let the textview place the cursor.
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        ui_link.follow_link(&target);
    });
    ui.definition.add_controller(click);

    // and the pointer says so before you click.
    let ui_hover = ui.clone();
    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    motion.connect_motion(move |_, x, y| {
        let cursor = if ui_hover.link_at(x, y).is_some() {
            "pointer"
        } else {
            "text"
        };
        ui_hover.definition.set_cursor_from_name(Some(cursor));
    });
    ui.definition.add_controller(motion);

    // Down from the search box steps into the wordlist (roadmap #16).
    let ui_down = ui.clone();
    let entry_keys = gtk::EventControllerKey::new();
    entry_keys.connect_key_pressed(move |_, key, _, _| {
        if key != gdk::Key::Down {
            return glib::Propagation::Proceed;
        }
        ui_down.focus_first_row();
        glib::Propagation::Stop
    });
    search.add_controller(entry_keys);

    // and Up from the first row comes back out to the search box. anywhere else
    // in the list, gtk's own row navigation is what you want.
    let ui_up = ui.clone();
    let list_keys = gtk::EventControllerKey::new();
    list_keys.connect_key_pressed(move |_, key, _, _| {
        let on_first_row = ui_up
            .results
            .selected_row()
            .is_some_and(|row| row.index() == 0);
        if key != gdk::Key::Up || !on_first_row {
            return glib::Propagation::Proceed;
        }
        ui_up.search.grab_focus();
        glib::Propagation::Stop
    });
    results.add_controller(list_keys);

    // typing anywhere in the window goes to the search box (roadmap #23). the
    // capture phase sees the key before the focused widget does.
    let ui_type = ui.clone();
    let window_keys = gtk::EventControllerKey::new();
    window_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    window_keys.connect_key_pressed(move |_, key, _, state| ui_type.redirect_typing(key, state));
    window.add_controller(window_keys);

    // build the merged index OFF the main thread, then hand it back over an
    // async-channel to the main context. glib::spawn_future_local runs on the
    // main thread, so touching the !Send Rc `ui` there is fine.
    let (tx, rx) = async_channel::bounded(1);
    let entries_owned: Vec<config::DictEntry> = entries.to_vec();
    std::thread::spawn(move || {
        let _ = tx.send_blocking(Library::open(&entries_owned));
    });
    let ui_ready = ui.clone();
    glib::spawn_future_local(async move {
        if let Ok(library) = rx.recv().await {
            *ui_ready.library.borrow_mut() = Some(library);
            ui_ready.search.set_sensitive(true);
            ui_ready
                .search
                .set_placeholder_text(Some("Search all dictionaries…"));
            ui_ready.show_library_size();
            ui_ready.set_message("Type to search all dictionaries.");
            // a word may already be waiting: firing the hotkey with nothing running
            // starts the app AND fills the search box, and that search ran while
            // there was no index to search, so it found nothing. run it again now
            // that there is one, or the box sits there with a word and no results.
            let waiting = ui_ready.search.text();
            if !waiting.trim().is_empty() {
                ui_ready.populate_results(&waiting);
            }
            ui_ready.search.grab_focus();
        }
    });

    ui
}

fn toolbar_with(header: &adw::HeaderBar, content: &impl IsA<gtk::Widget>) -> adw::ToolbarView {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(header);
    toolbar.set_content(Some(content));
    toolbar
}

/// one wordlist row: the word, and a dim tag saying which language it is — or which
/// dictionary, when the language can't be named (see `language::tag`).
fn word_row(word: &str, dict_name: &str) -> gtk::Box {
    let label = gtk::Label::builder()
        .label(word)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .tooltip_text(word)
        .hexpand(true)
        .build();

    let tag = gtk::Label::builder()
        .label(language::tag(word, dict_name).unwrap_or(dict_name))
        .xalign(1.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .tooltip_text(dict_name)
        .max_width_chars(12)
        .build();
    tag.add_css_class("dim-label");
    tag.add_css_class("caption");

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(12);
    row.set_margin_end(12);
    row.append(&label);
    row.append(&tag);
    row
}

/// `n` with thousands separators: 4009914 -> "4,009,914". `rchunks` groups from
/// the right, which is where digit grouping starts.
fn thousands(n: usize) -> String {
    n.to_string()
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|group| String::from_utf8_lossy(group).into_owned())
        .collect::<Vec<_>>()
        .join(",")
}

/// a count with separators and the noun that agrees with it: 1 -> "1 dictionary",
/// 8 -> "8 dictionaries".
fn quantity(n: usize, singular: &str, plural: &str) -> String {
    format!(
        "{} {}",
        thousands(n),
        if n == 1 { singular } else { plural }
    )
}

#[cfg(test)]
mod tests {
    use super::{quantity, thousands};

    #[test]
    fn thousands_groups_from_the_right() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(12), "12");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(4_009_914), "4,009,914");
    }

    #[test]
    fn quantity_agrees_with_its_count() {
        assert_eq!(quantity(1, "dictionary", "dictionaries"), "1 dictionary");
        assert_eq!(quantity(8, "dictionary", "dictionaries"), "8 dictionaries");
        assert_eq!(quantity(0, "result", "results"), "0 results");
        assert_eq!(quantity(4_009_914, "word", "words"), "4,009,914 words");
    }
}
