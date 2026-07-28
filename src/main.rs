// dictu — a gnome dictionary app.
//
// the app reads config.toml for a list of directories, scans them for
// dictionaries, and builds one merged index across ALL of them (off the main
// thread, with an "Indexing…" state). the search box queries every dictionary
// at once; a result shows its definition from each dict that has it.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

mod cli;
mod collection;
mod config;
mod dict;
mod index_cache;
mod keys;
mod language;
mod library;
mod ui;

const APP_ID: &str = "io.github.eyy.Dictu";

use ui::Ui;

fn main() -> glib::ExitCode {
    let raw: Vec<String> = std::env::args().collect();

    // every subcommand lives in `cli`, over the same `Session` the window uses;
    // `None` means this invocation is for the window (roadmap #46).
    if let Some(code) = cli::run(&raw) {
        return code;
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

    // the single Ui, built on the first invocation and reused after. this cell is
    // its one strong owner — the handlers inside it all hold weak handles.
    let ui_cell: Rc<RefCell<Option<Ui>>> = Rc::new(RefCell::new(None));
    app.connect_command_line(move |app, cmdline| {
        let argv: Vec<String> = cmdline
            .arguments()
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();

        let ui = ui_cell
            .borrow_mut()
            .get_or_insert_with(|| ui::build(app, &entries))
            .clone();

        // the hotkey passes the selected word here. what filling the box and landing
        // on an answer takes is the window's business, not this function's — it is a
        // careful piece of work about gtk signals (see `search_from_outside`).
        if let Some(word) = parse_flag(&argv, "--search") {
            ui.search_from_outside(&word);
        }
        ui.present();
        0
    });

    app.run_with_args(&raw)
}

/// value following `flag` in `args` (e.g. `--open /path`), if present.
fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| a == flag)?;
    args.get(idx + 1).cloned()
}
