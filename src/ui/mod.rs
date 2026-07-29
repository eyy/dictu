//! the window: the widgets, the state they read, and every signal handler.
//!
//! this module knows about gtk and about `Collection`, and nothing else knows
//! about either combination — `main` starts the application and hands a forwarded
//! search here, `collection` answers questions about words without having heard of
//! a widget, and `render` beside this dresses a definition (roadmap #46).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::collection::{self, Collection};
use crate::library::Library;
use crate::{config, dict, language, library};

mod render;
use render::{LINK_PREFIX, entry_tag, head_tag, link_tag, source_tag, structure_body, style_tag};

/// one wordlist entry, as the model holds it: the lemma's row, and the label of every
/// dictionary that has it — resolved once, when the search ran, because the factory
/// that renders a row has no business knowing about the collection.
///
/// carried in a `BoxedAnyObject` rather than a `GObject` subclass with properties:
/// nothing here is bound, sorted or filtered by gtk (the order is the library's), so
/// properties would be sixty lines of boilerplate bought for nothing.
struct Listed {
    row: library::Row,
    names: Vec<String>,
}

/// the collection, shared across signal handlers. it exists from the start and
/// answers everything before indexing finishes too — `Collection::is_ready` is
/// what asks whether the words are there yet.
type SharedCollection = Rc<RefCell<Collection>>;

/// the widgets + state a load touches, bundled so signal closures capture one
/// cheap handle instead of half a dozen individual widget handles.
///
/// behind an `Rc` so handlers can hold a `Weak` rather than a strong clone
/// (roadmap #8): the window owns the widgets and each widget owns its signal
/// handlers, so a handler holding the `Ui` — which holds the window — closes a
/// cycle that keeps the whole tree alive for the life of the process.
pub(crate) type Ui = Rc<UiInner>;

pub(crate) struct UiInner {
    /// a weak handle to ourselves, for the deferred work a method schedules (the
    /// idle fold measurement in `show_word`) — `&self` can't produce one.
    this: Weak<UiInner>,
    window: adw::ApplicationWindow,
    search: gtk::SearchEntry,
    /// the wordlist, and the two objects behind it. the model is the list — appending
    /// to it *is* showing a row — and the selection is what says which one is current,
    /// handing back the item itself rather than a position (#57).
    results: gtk::ListView,
    model: gio::ListStore,
    selection: gtk::SingleSelection,
    definition: gtk::TextView,
    status: gtk::Label,
    /// the strip under the definition naming what is still below the fold.
    fold: gtk::Label,
    /// one entry per dictionary section in the definition currently shown: its
    /// label, and a mark at the line it starts on. marks (not line numbers) because
    /// they survive the buffer being rewritten under them.
    sections: Rc<RefCell<Vec<(String, gtk::TextMark)>>>,
    /// the word the definition pane is showing, so a scope change can render it
    /// again under the new scope: rebuilding the wordlist deselects every row
    /// without telling anyone which one the pane was left on. the word rather
    /// than the row, because a row is only the dictionaries that were in scope
    /// when it was built — re-selecting one has to be able to bring it back.
    shown: Rc<RefCell<Option<String>>>,
    /// the `search-changed` handler, so a forwarded search can fill the box without
    /// that handler seeing the *halves* of the change: `set_text` is a delete followed
    /// by an insert, and `SearchEntry` debounces a search but not a clearing — so the
    /// delete arrives as a synchronous `search-changed("")` for a box the reader never
    /// emptied. filled in once the handler below is connected.
    search_changed: RefCell<Option<glib::SignalHandlerId>>,
    /// the word a search arriving from outside (`--search`, i.e. the global hotkey)
    /// has *already* been searched for, so the debounced `search-changed` that
    /// follows `set_text` can be ignored rather than rebuilding the same wordlist and
    /// deselecting the row it just put on screen. cleared by whichever
    /// `search-changed` arrives first, so it never outlives the one it is about.
    forwarded: Rc<RefCell<Option<String>>>,
    /// the collection and what the reader has decided about it: which
    /// dictionaries are in scope, whether repeated forms are folded. every
    /// question about words goes through here — and so does every question the
    /// command line asks, which is why it lives in a module that has never heard
    /// of gtk.
    collection: SharedCollection,
    /// the scope panel's rows, filled once the dictionaries are known, and the
    /// header button that pops it up.
    scope_list: gtk::ListBox,
    scope_button: gtk::MenuButton,
}

impl UiInner {
    /// show the window, whatever it was doing.
    pub(crate) fn present(&self) {
        self.window.present();
    }

    /// a word arriving from outside the window — `--search`, i.e. the global hotkey.
    /// fill the search box with it and land on an answer, rather than on a list still
    /// to be clicked.
    ///
    /// and search it *now*. `SearchEntry` debounces `search-changed` by 150 ms so that
    /// typing does not re-search on every keystroke — but a word arriving whole from
    /// outside has nothing to coalesce, so waiting that out was lag on the one path
    /// where the reader has already settled on the word (#56).
    pub(crate) fn search_from_outside(&self, word: &str) {
        // the debounced signal still arrives afterwards, and running the search a
        // second time would rebuild the wordlist and so deselect the row whose
        // definition is by then on screen — so tell the handler it has been dealt with.
        //
        // armed for *every* non-empty word, including one the box already holds:
        // `set_text` is a delete followed by an insert, so it emits `changed` even when
        // the text it leaves behind is identical. arming only on a real change looked
        // tidier and left the second of two identical searches unguarded, which is
        // exactly the sequence the e2e suite runs. the one case that emits nothing is
        // empty replacing empty, and an empty query has no row to lose.
        if !word.is_empty() {
            *self.forwarded.borrow_mut() = Some(word.to_owned());
        }
        // and block the handler across the change itself, so the delete half of
        // `set_text` — a `search-changed("")` that arrives synchronously, since an
        // empty box is not debounced — is neither searched for nor able to consume the
        // guard meant for the insert's debounced signal. it did exactly that before,
        // which cost a whole-library search on every hotkey press and left the real
        // signal free to deselect the row.
        let handler = self.search_changed.borrow();
        match handler.as_ref() {
            Some(handler) => {
                self.search.block_signal(handler);
                self.search.set_text(word);
                self.search.unblock_signal(handler);
            }
            None => self.search.set_text(word),
        }
        drop(handler);
        self.search.grab_focus();
        self.populate_results(word);
        self.select_first_row();
    }

    /// plain message in the definition pane (hint / "no definition").
    fn set_message(&self, text: &str) {
        self.definition.buffer().set_text(text);
    }

    /// rebuild the results list from a prefix search across the whole index,
    /// deduping repeated headwords (a word in several dicts appears once; the
    /// per-dict definitions show when it's selected).
    fn populate_results(&self, query: &str) {
        self.model.remove_all();
        let collection = self.collection.borrow();
        if !collection.is_ready() {
            return;
        }

        let query = query.trim();
        // searching nothing is a state, not an empty result: say so instead of
        // showing an empty list that looks broken. with nothing loaded at all the
        // honest answer names the config, not a menu that would be empty.
        if collection.nothing_selected() {
            self.set_message("No dictionaries selected.\n\nPick one in the search-scope menu.");
            self.show_scope_only();
            return;
        }

        // one row per lemma, however its dictionaries spell it (#43), naming every
        // dictionary that has it (#12). the grouping is the library's: it is the
        // only place that knows which spellings are the same word.
        let (rows, more) = collection.page(query, collection::ROW_LIMIT);
        // the model *is* the wordlist. spliced in one go rather than appended row by
        // row, so the view hears once that the list changed instead of five hundred
        // times at the row cap.
        let listed: Vec<glib::BoxedAnyObject> = rows
            .iter()
            .map(|row| {
                let names = row
                    .dicts()
                    .iter()
                    .map(|&dict| collection.dict_label(dict).to_owned())
                    .collect();
                glib::BoxedAnyObject::new(Listed {
                    row: row.clone(),
                    names,
                })
            })
            .collect();
        self.model.splice(0, self.model.n_items(), &listed);

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
        // the collection writes the line, so the window and the command line cannot
        // drift into describing one search differently.
        self.status.set_text(&collection.status(rows.len(), more));
    }

    /// select the first row, which is what renders its definition. focus stays where
    /// it is, so a search arriving from the hotkey shows an answer without taking the
    /// search box away from you mid-typing.
    fn select_first_row(&self) {
        if self.model.n_items() > 0 {
            self.selection.set_selected(0);
        }
    }

    /// the same, but move focus into the wordlist too — what Down does. gtk's own row
    /// navigation takes over from there.
    fn focus_first_row(&self) {
        self.select_first_row();
        self.results.grab_focus();
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
        let collection = self.collection.borrow();
        if !collection.is_ready() {
            return;
        }
        if collection.nothing_selected() {
            self.show_scope_only();
            return;
        }
        self.status.set_text(&collection.library_size());
    }

    /// the scope panel's own state, said out loud: not "0 results", which reads as
    /// "your search found nothing", but that nothing is being searched.
    fn show_scope_only(&self) {
        self.status.set_text(&collection::quantity(
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
        let scoped_out = self.collection.borrow().nothing_selected();
        let shown = self.shown.borrow().clone();
        if let (false, Some(word)) = (scoped_out, shown) {
            self.show_word(&word);
        }
    }

    /// fill the scope panel once the dictionaries are known: one row per
    /// dictionary, its size under its name, everything selected to begin with.
    fn build_scope(&self) {
        // read what is needed and let the borrow go: the handlers installed below
        // take a `borrow_mut`, and #45 will rewrite this loop into one that can fire
        // them while it runs.
        let (ready, dicts) = {
            let collection = self.collection.borrow();
            (collection.is_ready(), collection.dict_count())
        };
        if !ready {
            return;
        }
        for index in 0..dicts {
            let (label, headwords) = {
                let collection = self.collection.borrow();
                (
                    collection.dict_label(index).to_owned(),
                    collection.dict_headwords(index),
                )
            };
            let label = label.as_str();
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
                .subtitle(collection::quantity(headwords, "headword", "headwords"))
                .activatable_widget(&check)
                .build();
            row.add_prefix(&check);
            // weakly, like every other handler (#8): this checkbox lives in the
            // popover, which the header bar owns, which the window owns — a strong
            // handle here would rebuild the very cycle #8 removed, and it would do it
            // where the cycle test cannot see it, since `build_scope` runs from the
            // indexing future rather than from `build`. `&self` has no `Rc` for
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
        self.scope_button.set_sensitive(dicts > 0);
    }

    /// a checkbox changed: update the mask, then re-run whatever is in the search
    /// box so the wordlist and the status line follow immediately.
    fn set_dict_active(&self, index: usize, active: bool) {
        self.collection.borrow_mut().set_dict_active(index, active);
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
        let row = { self.collection.borrow().resolve(word) };
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
        let collection = self.collection.borrow();
        if !collection.is_ready() {
            return;
        }
        self.shown.replace(Some(row.word.clone()));
        // scoped, like the wordlist: a definition from a dictionary the reader
        // deselected would contradict the status line, and the fold strip would go
        // further and advertise that dictionary by name. each dictionary is asked
        // for the spelling *it* files, so the pane never comes up empty for a word
        // the list just showed.
        let defs = collection.definitions(row);

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
            collection::quantity(below.len(), "more definition", "more definitions"),
            below.join(" · "),
        ));
    }
}

pub(crate) fn build(app: &adw::Application, entries: &[config::DictEntry]) -> Ui {
    // -- sidebar: search box, results list, status line ---------------------
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Indexing…"));
    search.set_sensitive(false); // enabled once the index finishes building.

    // the wordlist is a model with a view over it, not a pile of widgets we rebuild
    // (#57): appending to the model is what shows a row, and the selection hands back
    // the item rather than a position, so there is no index to keep honest.
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    // gtk would otherwise select the first item every time the model changes, which
    // means a definition appearing on screen after every keystroke. selecting a row is
    // the reader's business, or the hotkey's (see `search_from_outside`).
    selection.set_autoselect(false);
    selection.set_can_unselect(true);

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        item.set_child(Some(&row_widgets()));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        bind_row(item);
    });

    let results = gtk::ListView::new(Some(selection.clone()), Some(factory));
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
        model: model.clone(),
        selection: selection.clone(),
        definition,
        status,
        fold: fold.clone(),
        sections: Rc::new(RefCell::new(Vec::new())),
        shown: Rc::new(RefCell::new(None)),
        forwarded: Rc::new(RefCell::new(None)),
        search_changed: RefCell::new(None),
        collection: Rc::new(RefCell::new(Collection::empty())),
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
    let search_changed = search.connect_search_changed(glib::clone!(
        #[weak]
        ui,
        move |entry| {
            let text = entry.text();
            // a forwarded search ran the moment it arrived (see `connect_command_line`)
            // and this is only gtk's debounce catching up on it. taking the guard
            // either way keeps it from outliving the search it belongs to.
            if ui.forwarded.borrow_mut().take().as_deref() == Some(text.as_str()) {
                return;
            }
            ui.populate_results(&text)
        }
    ));
    *ui.search_changed.borrow_mut() = Some(search_changed);

    // the selection hands back the item itself, so the row whose definition is shown is
    // the row that was chosen — not whatever now sits at the position it used to hold.
    selection.connect_selected_item_notify(glib::clone!(
        #[weak]
        ui,
        move |selection| {
            let Some(boxed) = selection
                .selected_item()
                .and_downcast::<glib::BoxedAnyObject>()
            else {
                return;
            };
            let listed = boxed.borrow::<Listed>();
            ui.show_row(&listed.row);
        }
    ));

    fold_forms.connect_toggled(glib::clone!(
        #[weak]
        ui,
        move |toggle| {
            ui.collection
                .borrow_mut()
                .set_fold_forms(toggle.is_active());
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
            let on_first_row = ui.selection.selected() == 0;
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
            ui.collection.borrow_mut().open(library);
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
/// the widgets one wordlist row is made of, built empty. a factory reuses these as
/// the reader scrolls, so they are made once and filled by `bind_row` — which is why
/// the tag and the count exist even for a row that wants neither, hidden rather than
/// absent.
///
/// three labels, not two: a long dictionary name ellipsizing away must not be able to
/// take the count with it, since the count is the part that cannot be guessed by
/// reading the row.
fn row_widgets() -> gtk::Box {
    let word = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .hexpand(true)
        .build();

    let tag = gtk::Label::builder()
        .xalign(1.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(12)
        .build();
    tag.add_css_class("dim-label");
    tag.add_css_class("caption");

    let count = gtk::Label::builder().xalign(1.0).build();
    count.add_css_class("dim-label");
    count.add_css_class("caption");

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(12);
    row.set_margin_end(12);
    row.append(&word);
    row.append(&tag);
    row.append(&count);
    row
}

/// fill a recycled row from the item it has been bound to.
fn bind_row(item: &gtk::ListItem) {
    let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
        return;
    };
    let Some(row) = item.child().and_downcast::<gtk::Box>() else {
        return;
    };
    let listed = boxed.borrow::<Listed>();
    let names: Vec<&str> = listed.names.iter().map(String::as_str).collect();

    let Some(word) = row.first_child().and_downcast::<gtk::Label>() else {
        return;
    };
    word.set_label(&listed.row.word);
    word.set_tooltip_text(Some(&listed.row.word));

    // the language, but only while every dictionary agrees on it: `LAT ·2` over a
    // latin and a french dictionary reads as "two latin dictionaries", and the tag
    // is derived from one dictionary while the count spans them all.
    let named: Vec<&str> = names
        .iter()
        .map(|&d| language::tag(&listed.row.word, d).unwrap_or(d))
        .collect();
    let agreed = named
        .first()
        .filter(|first| named.iter().all(|n| n == *first));
    if let Some(tag) = word.next_sibling().and_downcast::<gtk::Label>() {
        // cleared, not just hidden: these widgets are recycled, so anything left on
        // one is the *previous* row's — and a hidden label is still in the a11y tree,
        // where a stale `·2` would be read out under a word that only one dictionary
        // has. every branch sets every field.
        match agreed {
            Some(agreed) => {
                tag.set_label(agreed);
                tag.set_tooltip_text(Some(&names.join("\n")));
                tag.set_visible(true);
            }
            None => {
                tag.set_label("");
                tag.set_tooltip_text(None);
                tag.set_visible(false);
            }
        }
        if let Some(count) = tag.next_sibling().and_downcast::<gtk::Label>() {
            match names.len() > 1 {
                true => {
                    count.set_label(&format!("·{}", names.len()));
                    count.set_tooltip_text(Some(&names.join("\n")));
                    count.set_visible(true);
                }
                false => {
                    count.set_label("");
                    count.set_tooltip_text(None);
                    count.set_visible(false);
                }
            }
        }
    }

    // the row reads as its word. gtk would otherwise name it from every label inside,
    // so a screen reader (and the e2e harness) would hear the language tag and the
    // count as part of the headword.
    // the row reads as its word. gtk would otherwise name it from every label inside,
    // so a screen reader (and the e2e harness) would hear the language tag and the
    // count as part of the headword — while the labels themselves stay in the tree,
    // because `·2` is information a reader wants, not decoration.
    //
    // through `ListItem`, not by reaching for the `GtkListItemWidget` behind it: setting
    // an accessible property on gtk's own internal widget kills the process with
    // `gtk_widget_insert_after: assertion 'GTK_IS_WIDGET (widget)' failed` on the next
    // row it lays out. this is the api that exists for the job.
    item.set_accessible_label(&listed.row.word);
}

#[cfg(test)]
mod tests {
    use super::{Rc, build};
    use adw::prelude::*;
    use gtk::gio;

    /// the point of roadmap #8: the handlers `build` connects hold weak handles,
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
        let ui = build(&app, &[]);
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
