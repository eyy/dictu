// dictu — a gnome dictionary app.
//
// the app reads config.toml for a list of directories, scans them for
// dictionaries, and builds one merged index across ALL of them (off the main
// thread, with an "Indexing…" state). the search box queries every dictionary
// at once; a result shows its definition from each dict that has it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod cli;
mod config;
mod dict;
mod index_cache;
mod keys;
mod language;
mod library;
mod session;
use library::Library;
use session::Session;

const APP_ID: &str = "io.github.eyy.Dictu";

/// the loaded index, shared across signal handlers. `None` until indexing
/// finishes on the worker thread.
type SharedSession = Rc<RefCell<Session>>;

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

/// the widgets + state a load touches, bundled so signal closures capture one
/// cheap handle instead of half a dozen individual widget handles.
///
/// behind an `Rc` so handlers can hold a `Weak` rather than a strong clone
/// (roadmap #8): the window owns the widgets and each widget owns its signal
/// handlers, so a handler holding the `Ui` — which holds the window — closes a
/// cycle that keeps the whole tree alive for the life of the process.
type Ui = Rc<UiInner>;

struct UiInner {
    /// a weak handle to ourselves, for the deferred work a method schedules (the
    /// idle fold measurement in `show_word`) — `&self` can't produce one.
    this: Weak<UiInner>,
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
    /// the wordlist's rows, in order. a row is a box of labels rather than a
    /// word, so it is found by index rather than read back out of a widget — and
    /// it carries the spellings each dictionary files it under, which is what the
    /// definition pane needs to find the entries again (#43).
    words: Rc<RefCell<Vec<library::Row>>>,
    /// the word the definition pane is showing, so a scope change can render it
    /// again under the new scope: rebuilding the wordlist deselects every row
    /// without telling anyone which one the pane was left on. the word rather
    /// than the row, because a row is only the dictionaries that were in scope
    /// when it was built — re-selecting one has to be able to bring it back.
    shown: Rc<RefCell<Option<String>>>,
    /// set when a search arrived from outside (`--search`, i.e. the global hotkey):
    /// the next set of results selects its first row on its own. a flag rather than a
    /// timer because `SearchEntry` debounces `search-changed`, so there is no moment
    /// after `set_text` at which the rows are known to exist yet.
    auto_select: Rc<Cell<bool>>,
    /// the collection and what the reader has decided about it: which
    /// dictionaries are in scope, whether repeated forms are folded. every
    /// question about words goes through here — and so does every question the
    /// command line asks, which is why it lives in a module that has never heard
    /// of gtk.
    session: SharedSession,
    /// the scope panel's rows, filled once the dictionaries are known, and the
    /// header button that pops it up.
    scope_list: gtk::ListBox,
    scope_button: gtk::MenuButton,
}

impl UiInner {
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
        let session = self.session.borrow();
        if !session.is_ready() {
            return;
        }

        let query = query.trim();
        // searching nothing is a state, not an empty result: say so instead of
        // showing an empty list that looks broken. with nothing loaded at all the
        // honest answer names the config, not a menu that would be empty.
        if session.nothing_selected() {
            self.set_message("No dictionaries selected.\n\nPick one in the search-scope menu.");
            self.show_scope_only();
            return;
        }

        // one row per lemma, however its dictionaries spell it (#43), naming every
        // dictionary that has it (#12). the grouping is the library's: it is the
        // only place that knows which spellings are the same word.
        let rows = session.search(query, session::ROW_LIMIT);
        // recorded before the widgets exist: appending a row can select it, and
        // the handler reads this list by index.
        self.words.replace(rows.clone());

        for row in &rows {
            let names: Vec<&str> = row
                .dicts()
                .iter()
                .map(|&dict| session.dict_label(dict))
                .collect();
            self.results.append(&word_row(&row.word, &names));
            // name the row after its word: the row is a box of two labels now, so
            // without this a screen reader (and the e2e harness) would read the
            // language tag as part of the entry.
            if let Some(listed) = self.results.last_child().and_downcast::<gtk::ListBoxRow>() {
                listed.update_property(&[gtk::accessible::Property::Label(&row.word)]);
            }
        }

        if query.is_empty() {
            self.set_message("Type to search all dictionaries.");
            // deliberately without the folded note: folding drops rows from a
            // result set, never headwords from the library, so the size is the one
            // count it cannot change.
            self.show_library_size();
            return;
        }
        if rows.is_empty() {
            self.set_message(&format!("No matches for “{query}”."));
        }
        // the session writes the line, so the window and the command line cannot
        // drift into describing one search differently.
        self.status
            .set_text(&session.status(rows.len(), rows.len() >= session::ROW_LIMIT));

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
        // Space activates a focused button, so leave that one key alone: otherwise
        // the scope button can't be opened from the keyboard and, worse, the space
        // lands in the query — focus parks on that button whenever the panel closes.
        // every other printable character still goes to the search box.
        if key == gdk::Key::space && self.focus_is_on_a_button() {
            return glib::Propagation::Proceed;
        }
        // control characters (escape, backspace, tab, the arrows) are navigation,
        // not text — leave them to the widget that has focus.
        let Some(ch) = key.to_unicode().filter(|c| !c.is_control()) else {
            return glib::Propagation::Proceed;
        };
        // if the scope panel had the keyboard, typing means you're done with it.
        self.scope_button.popdown();
        self.search.grab_focus();
        self.search.set_text(&format!("{}{ch}", self.search.text()));
        self.search.set_position(-1);
        glib::Propagation::Stop
    }

    /// whether focus sits on something that treats Space as activation rather than
    /// as text — `CheckButton` is not a `Button` subclass in gtk4, so check both.
    fn focus_is_on_a_button(&self) -> bool {
        gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|focused| focused.is::<gtk::Button>() || focused.is::<gtk::CheckButton>())
    }

    /// whether focus is in the search box. gtk4 puts focus on the `GtkText`
    /// *inside* a `SearchEntry`, so the entry itself is only an ancestor of it.
    fn search_has_focus(&self) -> bool {
        let search: &gtk::Widget = self.search.upcast_ref();
        // spelled out because `Root` and `GtkWindow` both define `focus`.
        gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|focused| &focused == search || focused.is_ancestor(&self.search))
    }

    /// the idle status line: how much is *in scope* — with every dictionary
    /// selected that is the whole library, which is what it used to say.
    fn show_library_size(&self) {
        let session = self.session.borrow();
        if !session.is_ready() {
            return;
        }
        if session.nothing_selected() {
            self.show_scope_only();
            return;
        }
        self.status.set_text(&session.library_size());
    }

    /// the scope panel's own state, said out loud: not "0 results", which reads as
    /// "your search found nothing", but that nothing is being searched.
    fn show_scope_only(&self) {
        self.status.set_text(&session::quantity(
            0,
            "dictionary selected",
            "dictionaries selected",
        ));
    }

    /// render the open definition again after the wordlist was rebuilt under a new
    /// setting: rebuilding drops the selection silently, so without this the pane
    /// keeps showing a dictionary just deselected, or forms just folded, beside a
    /// row that has stopped counting them.
    ///
    /// except when nothing is selected at all — that state is a message
    /// `populate_results` has already written into the pane, telling the user how to
    /// get out of it, and overwriting it with "No definition for X" would take the
    /// instruction away and clear `shown` on the way past.
    fn render_shown_again(&self) {
        let scoped_out = self.session.borrow().nothing_selected();
        let shown = self.shown.borrow().clone();
        if let (false, Some(word)) = (scoped_out, shown) {
            self.show_word(&word);
        }
    }

    /// fill the scope panel once the dictionaries are known: one row per
    /// dictionary, its size under its name, everything selected to begin with.
    fn build_scope(&self) {
        let session = self.session.borrow();
        if !session.is_ready() {
            return;
        }
        for index in 0..session.dict_count() {
            let label = session.dict_label(index);
            let check = gtk::CheckButton::builder()
                .active(true)
                .valign(gtk::Align::Center)
                .build();
            // name the checkbox after its dictionary: the row's title is a separate
            // widget, so otherwise the box is anonymous to a screen reader.
            check.update_property(&[gtk::accessible::Property::Label(label)]);
            let row = adw::ActionRow::builder()
                // a dictionary's name is data — escape it, the row renders markup.
                .title(glib::markup_escape_text(label))
                .subtitle(session::quantity(
                    session.dict_headwords(index),
                    "headword",
                    "headwords",
                ))
                .activatable_widget(&check)
                .build();
            row.add_prefix(&check);
            // weakly, like every other handler (#8): this checkbox lives in the
            // popover, which the header bar owns, which the window owns — a strong
            // handle here would rebuild the very cycle #8 removed, and it would do it
            // where the cycle test cannot see it, since `build_scope` runs from the
            // indexing future rather than from `build_ui`. `&self` has no `Rc` for
            // `glib::clone!` to downgrade, so use the self-weak the idle path uses.
            let this = self.this.clone();
            check.connect_toggled(move |check| {
                if let Some(ui) = this.upgrade() {
                    ui.set_dict_active(index, check.is_active());
                }
            });
            self.scope_list.append(&row);
        }
        // nothing to scope when nothing loaded: leave the button dead rather than
        // popping up an empty list.
        self.scope_button.set_sensitive(session.dict_count() > 0);
    }

    /// a checkbox changed: update the mask, then re-run whatever is in the search
    /// box so the wordlist and the status line follow immediately.
    fn set_dict_active(&self, index: usize, active: bool) {
        self.session.borrow_mut().set_dict_active(index, active);
        self.populate_results(&self.search.text());
        self.render_shown_again();
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
    /// a spelling typed or clicked rather than picked from the list — a link
    /// target, or the word the hotkey arrived with. the library decides which row
    /// it belongs to, since the dictionary that has it may spell it with marks the
    /// caller did not (#43).
    fn show_word(&self, word: &str) {
        let row = { self.session.borrow().resolve(word) };
        match row {
            Some(row) => self.show_row(&row),
            // nothing on these letters at all: say so, rather than leaving the
            // last word's definition sitting there under a new heading.
            None => {
                self.shown.replace(None);
                let buffer = self.definition.buffer();
                self.clear_sections(&buffer);
                buffer.set_text(&format!("No definition for “{word}”."));
                self.update_fold();
            }
        }
    }

    fn show_row(&self, row: &library::Row) {
        let word = row.word.as_str();
        let session = self.session.borrow();
        if !session.is_ready() {
            return;
        }
        self.shown.replace(Some(row.word.clone()));
        // scoped, like the wordlist: a definition from a dictionary the reader
        // deselected would contradict the status line, and the fold strip would go
        // further and advertise that dictionary by name. each dictionary is asked
        // for the spelling *it* files, so the pane never comes up empty for a word
        // the list just showed.
        let defs = session.definitions(row);

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
        for definition in &defs {
            let (label, entries) = (&definition.label, &definition.entries);
            // remember where this dictionary's answer starts, so the strip below the
            // pane can say which ones are still out of sight.
            let mark = buffer.create_mark(None, &iter, true);
            self.sections.borrow_mut().push((label.clone(), mark));
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
        // once the layout has settled rather than measuring nothing here. weakly:
        // the window can be closed between now and the next main-loop turn.
        let ui = self.this.clone();
        glib::idle_add_local_once(move || {
            if let Some(ui) = ui.upgrade() {
                ui.update_fold();
            }
        });
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
            session::quantity(below.len(), "more definition", "more definitions"),
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
    // named so it can be told apart from the scope panel's list, which is another
    // list of the same role.
    results.update_property(&[gtk::accessible::Property::Label("Wordlist")]);

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

    // -- header: the search-scope panel -------------------------------------
    // a popover, not an adw preferences window: this is a transient search scope
    // you flip while searching, not a setting — it belongs one click from the
    // search box, and it dismisses itself when you go back to typing.
    let scope_list = gtk::ListBox::new();
    scope_list.set_selection_mode(gtk::SelectionMode::None);
    scope_list.add_css_class("boxed-list");
    scope_list.update_property(&[gtk::accessible::Property::Label("Dictionaries")]);

    let scope_title = gtk::Label::builder()
        .label("Search scope")
        .xalign(0.0)
        .build();
    scope_title.add_css_class("heading");
    let scope_hint = gtk::Label::builder()
        .label(
            "Narrows this session's searches. What gets loaded at all is config.toml's business.",
        )
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(34)
        .build();
    scope_hint.add_css_class("dim-label");
    scope_hint.add_css_class("caption");

    // roadmap #33. searching `rex` walked into 26 forms of *rego*, every one of
    // them answering with the same definition, so the wordlist can be told to show
    // each definition once rather than once per form that points at it.
    let fold_forms = gtk::CheckButton::builder()
        .valign(gtk::Align::Center)
        .build();
    fold_forms.update_property(&[gtk::accessible::Property::Label("Fold repeated forms")]);
    let fold_row = adw::ActionRow::builder()
        .title("Fold repeated forms")
        .subtitle("One row per definition, not one per form pointing at it")
        .activatable_widget(&fold_forms)
        .build();
    fold_row.add_prefix(&fold_forms);
    // its own list rather than a bare check box: the same shape as the dictionary
    // rows above, which is what makes it reachable to a screen reader (and to the
    // harness, which aims at a check box's own extents).
    let options_list = gtk::ListBox::new();
    options_list.set_selection_mode(gtk::SelectionMode::None);
    options_list.add_css_class("boxed-list");
    options_list.update_property(&[gtk::accessible::Property::Label("Search options")]);
    options_list.append(&fold_row);

    let scope_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    scope_box.set_margin_top(6);
    scope_box.set_margin_bottom(6);
    scope_box.set_margin_start(6);
    scope_box.set_margin_end(6);
    scope_box.append(&scope_title);
    scope_box.append(&scope_hint);
    scope_box.append(&scope_list);
    scope_box.append(&options_list);

    let scope_button = gtk::MenuButton::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text("Search scope")
        .popover(&gtk::Popover::builder().child(&scope_box).build())
        .sensitive(false) // there is nothing to scope until indexing finishes.
        .build();
    scope_button.update_property(&[gtk::accessible::Property::Label("Search scope")]);

    let header = adw::HeaderBar::new();
    header.pack_end(&scope_button);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Dictu")
        .default_width(900)
        .default_height(600)
        .content(&toolbar_with(&header, &split))
        .build();

    // new_cyclic so the ui can hold a weak handle to itself; every closure below
    // captures `#[weak] ui` and quietly does nothing once the ui is gone.
    let ui = Rc::new_cyclic(|this| UiInner {
        this: this.clone(),
        window: window.clone(),
        search: search.clone(),
        results: results.clone(),
        definition,
        status,
        fold: fold.clone(),
        sections: Rc::new(RefCell::new(Vec::new())),
        words: Rc::new(RefCell::new(Vec::new())),
        shown: Rc::new(RefCell::new(None)),
        auto_select: Rc::new(Cell::new(false)),
        session: Rc::new(RefCell::new(Session::empty())),
        scope_list,
        scope_button,
    });

    // scrolling changes what's below the fold, so the strip follows it.
    def_scroll.vadjustment().connect_value_changed(glib::clone!(
        #[weak]
        ui,
        move |_| ui.update_fold()
    ));

    // typing searches the whole index; selecting a result shows its def(s).
    search.connect_search_changed(glib::clone!(
        #[weak]
        ui,
        move |entry| ui.populate_results(&entry.text())
    ));

    results.connect_row_selected(glib::clone!(
        #[weak]
        ui,
        move |_list, row| {
            let Some(row) = row else { return };
            let selected = usize::try_from(row.index())
                .ok()
                .and_then(|index| ui.words.borrow().get(index).cloned());
            if let Some(selected) = selected {
                ui.show_row(&selected);
            }
        }
    ));

    fold_forms.connect_toggled(glib::clone!(
        #[weak]
        ui,
        move |toggle| {
            ui.session.borrow_mut().set_fold_forms(toggle.is_active());
            ui.populate_results(&ui.search.text());
            ui.render_shown_again();
        }
    ));

    // clicking a link in a definition looks that word up (roadmap #20).
    let click = gtk::GestureClick::new();
    // capture phase: the textview's own drag-select gesture claims the sequence
    // otherwise, and this handler never hears about the release.
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_released(glib::clone!(
        #[weak]
        ui,
        move |gesture, _clicks, x, y| {
            let Some(target) = ui.link_at(x, y) else {
                return; // an ordinary click: let the textview place the cursor.
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            ui.follow_link(&target);
        }
    ));
    ui.definition.add_controller(click);

    // and the pointer says so before you click.
    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    motion.connect_motion(glib::clone!(
        #[weak]
        ui,
        move |_, x, y| {
            let cursor = if ui.link_at(x, y).is_some() {
                "pointer"
            } else {
                "text"
            };
            ui.definition.set_cursor_from_name(Some(cursor));
        }
    ));
    ui.definition.add_controller(motion);

    // Down from the search box steps into the wordlist (roadmap #16).
    let entry_keys = gtk::EventControllerKey::new();
    entry_keys.connect_key_pressed(glib::clone!(
        #[weak]
        ui,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, _| {
            if key != gdk::Key::Down {
                return glib::Propagation::Proceed;
            }
            ui.focus_first_row();
            glib::Propagation::Stop
        }
    ));
    search.add_controller(entry_keys);

    // and Up from the first row comes back out to the search box. anywhere else
    // in the list, gtk's own row navigation is what you want.
    let list_keys = gtk::EventControllerKey::new();
    list_keys.connect_key_pressed(glib::clone!(
        #[weak]
        ui,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, _| {
            let on_first_row = ui
                .results
                .selected_row()
                .is_some_and(|row| row.index() == 0);
            if key != gdk::Key::Up || !on_first_row {
                return glib::Propagation::Proceed;
            }
            ui.search.grab_focus();
            glib::Propagation::Stop
        }
    ));
    results.add_controller(list_keys);

    // typing anywhere in the window goes to the search box (roadmap #23). the
    // capture phase sees the key before the focused widget does.
    let window_keys = gtk::EventControllerKey::new();
    window_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    window_keys.connect_key_pressed(glib::clone!(
        #[weak]
        ui,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, state| ui.redirect_typing(key, state)
    ));
    window.add_controller(window_keys);

    // a popover holds the keyboard while open, on a surface the window controller
    // above is not part of — without this, typing into the open scope panel is
    // silently swallowed instead of going to the search box (roadmap #23).
    if let Some(popover) = ui.scope_button.popover() {
        let popover_keys = gtk::EventControllerKey::new();
        popover_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        popover_keys.connect_key_pressed(glib::clone!(
            #[weak]
            ui,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, state| ui.redirect_typing(key, state)
        ));
        popover.add_controller(popover_keys);
    }

    // build the merged index OFF the main thread, then hand it back over an
    // async-channel to the main context. glib::spawn_future_local runs on the
    // main thread, so touching the !Send Rc `ui` there is fine.
    let (tx, rx) = async_channel::bounded(1);
    let entries_owned: Vec<config::DictEntry> = entries.to_vec();
    std::thread::spawn(move || {
        let _ = tx.send_blocking(Library::open(&entries_owned));
    });
    // weakly, and upgraded only after the await: indexing takes ~30s, so the
    // window can be closed while this is still pending.
    let ui_ready = Rc::downgrade(&ui);
    glib::spawn_future_local(async move {
        if let Ok(library) = rx.recv().await {
            let Some(ui) = ui_ready.upgrade() else { return };
            ui.session.borrow_mut().open(library);
            // size the scope before anything searches: the re-run below reads it.
            ui.build_scope();
            ui.search.set_sensitive(true);
            ui.search
                .set_placeholder_text(Some("Search all dictionaries…"));
            ui.show_library_size();
            ui.set_message("Type to search all dictionaries.");
            // a word may already be waiting: firing the hotkey with nothing running
            // starts the app AND fills the search box, and that search ran while
            // there was no index to search, so it found nothing. run it again now
            // that there is one, or the box sits there with a word and no results.
            let waiting = ui.search.text();
            if !waiting.trim().is_empty() {
                ui.populate_results(&waiting);
            }
            ui.search.grab_focus();
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

/// one wordlist row: the word, a dim tag saying which language it is — or which
/// dictionary, when the language can't be named (see `language::tag`) — and, when
/// more than one dictionary has the word, how many. `dicts` is every dictionary
/// that has it, in index order; the tooltip names them all.
fn word_row(word: &str, dicts: &[&str]) -> gtk::Box {
    let label = gtk::Label::builder()
        .label(word)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .tooltip_text(word)
        .hexpand(true)
        .build();

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(12);
    row.set_margin_end(12);
    row.append(&label);

    // the language, but only while every dictionary agrees on it: `LAT ·2` over a
    // latin and a french dictionary reads as "two latin dictionaries", and the tag
    // is derived from one dictionary while the count spans them all.
    let named: Vec<&str> = dicts
        .iter()
        .map(|&d| language::tag(word, d).unwrap_or(d))
        .collect();
    if let Some(first) = named
        .first()
        .filter(|first| named.iter().all(|n| n == *first))
    {
        let tag = gtk::Label::builder()
            .label(*first)
            .xalign(1.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .tooltip_text(dicts.join("\n"))
            .max_width_chars(12)
            .build();
        tag.add_css_class("dim-label");
        tag.add_css_class("caption");
        row.append(&tag);
    }

    // its own label, so a long dictionary name ellipsizing away cannot take the
    // count with it — the count is the part that can't be guessed from the row.
    if dicts.len() > 1 {
        let count = gtk::Label::builder()
            .label(format!("·{}", dicts.len()))
            .xalign(1.0)
            .tooltip_text(dicts.join("\n"))
            .build();
        count.add_css_class("dim-label");
        count.add_css_class("caption");
        row.append(&count);
    }
    row
}

#[cfg(test)]
mod tests {
    use super::{Rc, build_ui};
    use adw::prelude::*;
    use gtk::gio;

    /// the point of roadmap #8: the handlers `build_ui` connects hold weak handles,
    /// so dropping the last strong one frees the ui — and the widget tree goes with
    /// it when the window closes. with strong clones, the handlers owned by the
    /// window's own widgets kept both alive for the life of the process.
    #[test]
    fn handlers_do_not_keep_the_ui_alive() {
        // a real widget tree needs a display. skipping is the only option headless,
        // but say so out loud: libtest reports a silent early return as a pass, and
        // this test is the only thing standing between us and the cycle coming back.
        // DICTU_REQUIRE_DISPLAY makes the skip a failure where a display is expected
        // (hack/check.sh sets it), so the guard can't quietly go inert.
        if adw::init().is_err() {
            assert!(
                std::env::var_os("DICTU_REQUIRE_DISPLAY").is_none(),
                "no display, but DICTU_REQUIRE_DISPLAY is set: this test would have \
                 been skipped, leaving the reference-cycle guard inert"
            );
            eprintln!("skipping handlers_do_not_keep_the_ui_alive: no display");
            return;
        }
        let app = adw::Application::builder()
            .application_id("io.github.eyy.Dictu.Test")
            // NON_UNIQUE so concurrent runs (several worktrees, an agent per branch)
            // don't race for the bus name: the loser registers as a *remote* app,
            // never emits `startup`, refuses to adopt the window with a
            // Gtk-CRITICAL — and then owns nothing, which quietly voids the window
            // assertions below.
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        // register first: an unregistered GApplication refuses to take ownership of
        // a window, and it owning the window is what makes `destroy()` below the
        // thing that releases it.
        let _ = app.register(gio::Cancellable::NONE);
        let ui = build_ui(&app, &[]);
        let weak_ui = Rc::downgrade(&ui);
        // watch the widgets themselves too, not just our own bookkeeping.
        let window = ui.window.clone();
        let weak_window = window.downgrade();
        let definition = ui.definition.downgrade();
        assert!(weak_ui.upgrade().is_some(), "the ui should be alive here");

        drop(ui);
        assert!(
            weak_ui.upgrade().is_none(),
            "a signal handler still holds a strong Ui"
        );

        // and with nothing holding the ui, closing the window drops the tree.
        window.destroy();
        drop(window);
        assert!(
            weak_window.upgrade().is_none(),
            "the window outlived the ui"
        );
        assert!(
            definition.upgrade().is_none(),
            "the definition pane outlived the window"
        );
    }
}
