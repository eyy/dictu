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
use gtk::{gio, glib};

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
    // stats; `dictu search <query>` runs unified search across all configured
    // dicts and prints the hits.
    let subcommand = raw.get(1).map(String::as_str);
    if subcommand == Some("dump") {
        return dump(raw.get(2).map(String::as_str));
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
        } else if seen.is_empty() {
            self.set_message(&format!("No matches for “{query}”."));
        }
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
                let tag = style_tag(&buffer, run.style);
                buffer.insert_with_tags(&mut iter, &run.text, &[&tag]);
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
            let (words, dicts) = (library.total_headwords(), library.dict_count());
            *ui_ready.library.borrow_mut() = Some(library);
            ui_ready.search.set_sensitive(true);
            ui_ready
                .search
                .set_placeholder_text(Some("Search all dictionaries…"));
            ui_ready
                .status
                .set_text(&format!("{words} words · {dicts} dictionaries"));
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
