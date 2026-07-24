// dictu — a gnome dictionary app.
//
// the app reads config.toml for a list of directories, scans them for
// dictionaries, and builds one merged index across ALL of them (off the main
// thread, with an "Indexing…" state). the search box queries every dictionary
// at once; a result shows its definition from each dict that has it.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod config;
mod dict;
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

        // the hotkey passes the selected word here; fill the search box with it.
        if let Some(word) = parse_flag(&argv, "--search") {
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
                let def = dict.lookup(word).unwrap_or_default();
                let text = dict::html_to_text(&def);
                let preview: String = text.chars().take(100).collect();
                println!("  {word:?} -> {preview:?}");
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
    let Some(definition) = dict.lookup(word) else {
        println!("{}: no entry for {word:?}", dict.name());
        return glib::ExitCode::FAILURE;
    };
    println!("{} — {word}\n", dict.name());
    println!(
        "{}",
        if html {
            definition.clone()
        } else {
            dict::html_to_text(&definition)
        }
    );
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
        for hit in &hits {
            if !seen.insert(hit.word.to_lowercase()) {
                continue;
            }
            let row = gtk::Label::builder()
                .label(&hit.word)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .tooltip_text(&hit.word)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(12)
                .margin_end(12)
                .build();
            self.results.append(&row);
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
    }

    /// move focus into the wordlist and select its first row — which also shows
    /// that word's definition. gtk's own row navigation takes over from there.
    fn focus_first_row(&self) {
        let Some(row) = self.results.row_at_index(0) else {
            return;
        };
        self.results.select_row(Some(&row));
        row.grab_focus();
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
        buffer.set_text("");
        if defs.is_empty() {
            buffer.set_text(&format!("No definition for “{word}”."));
            return;
        }

        let mut iter = buffer.start_iter();
        buffer.insert_with_tags(&mut iter, &format!("{word}\n"), &[&head_tag(&buffer)]);
        for (dict_index, html) in &defs {
            let label = library.dict_label(*dict_index).unwrap_or("");
            buffer.insert_with_tags(&mut iter, &format!("\n{label}\n"), &[&source_tag(&buffer)]);
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
        }
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
            .build();
        buffer.tag_table().add(&tag);
        tag
    })
}

/// the dim, small label marking which dictionary a definition came from.
fn source_tag(buffer: &gtk::TextBuffer) -> gtk::TextTag {
    buffer.tag_table().lookup("source").unwrap_or_else(|| {
        let tag = gtk::TextTag::builder()
            .name("source")
            .weight(700)
            .scale(0.85)
            .foreground("#808080")
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

    // adw::OverlaySplitView: idiomatic sidebar+content (handles csd sizing).
    let split = adw::OverlaySplitView::new();
    split.set_sidebar(Some(&sidebar));
    split.set_content(Some(&def_scroll));
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
        library: Rc::new(RefCell::new(None)),
    };

    // typing searches the whole index; selecting a result shows its def(s).
    let ui_search = ui.clone();
    search.connect_search_changed(move |entry| ui_search.populate_results(&entry.text()));

    let ui_select = ui.clone();
    results.connect_row_selected(move |_list, row| {
        let Some(row) = row else { return };
        let Some(label) = row.child().and_downcast::<gtk::Label>() else {
            return;
        };
        ui_select.show_word(&label.label());
    });

    // clicking a link in a definition looks that word up (roadmap #20).
    let ui_link = ui.clone();
    let click = gtk::GestureClick::new();
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
