//! the window: the widgets, the state they read, and every signal handler.
//!
//! this module knows about gtk and about `Collection`, and nothing else knows
//! about either combination — `main` starts the application and hands a forwarded
//! search here, `collection` answers questions about words without having heard of
//! a widget, and `render` beside this dresses a definition (roadmap #46).

use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::collection::{self, Collection};
use crate::library::Library;
use crate::{config, dict, language, library, shortcut};

mod render;
use render::{
    LINK_PREFIX, entry_tag, head_tag, link_tag, pair_tag, source_tag, structure_body, style_tag,
};

/// the opening page's picture: the incipit of Garland's *Dictionarius*, cut from the
/// Bodleian's IIIF image of St John's College MS 235. baked into the binary rather than
/// installed beside it, so a build run from anywhere still has its own front page.
/// see `assets/ATTRIBUTION.md` — the licence is CC BY-NC and the credit is on the page.
///
/// stored at 520px wide because a `TextView` does **not** scale an inline paintable: it
/// draws it at its own size and clips whatever does not fit. 520 sits inside the pane at
/// the default window width with its margins to spare. narrow the window far enough and
/// the right edge goes — the honest fix is a child widget that can shrink, and it is not
/// worth the resize plumbing for a picture nobody looks at while dragging a window edge.
const INCIPIT: &[u8] = include_bytes!("../../assets/incipit-62r.jpg");

/// one wordlist entry, as the model holds it: the lemma's row, and the label of every
/// dictionary that has it — resolved once, when the search ran, because the factory
/// that renders a row has no business knowing about the collection.
///
/// carried in a `BoxedAnyObject` rather than a `GObject` subclass with properties:
/// nothing here is bound, sorted or filtered by gtk (the order is the library's), so
/// properties would be sixty lines of boilerplate bought for nothing.
struct Listed {
    row: library::Row,
    /// every dictionary that answers, by the name the reader gave it — the tooltip.
    names: Vec<String>,
    /// what the row's tag says: one entry per answering dictionary, its language
    /// where one can be named and its short name where none can (#59, #65).
    ///
    /// resolved here rather than in the row factory, because naming a language now
    /// takes the dictionary's *derived* name (#52) and the factory has only what
    /// this struct hands it. the factory dedupes and joins; deciding is this job.
    tags: Vec<String>,
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
    /// the opening page and the definition pane, one shown at a time (#48).
    pane: gtk::Stack,
    /// the sidebar's two pages: the wordlist, and the shelf shown in its place when
    /// nothing has been typed.
    sidebar: gtk::Stack,
    shelf_list: gtk::ListBox,
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
    /// the config as loaded, kept so a scope change can be written back (#71).
    config: Rc<RefCell<config::Config>>,
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
        // set unconditionally, so an empty word *clears* a guard left over from a previous
        // one. it has to: the emission that would otherwise clear it is the synchronous
        // `search-changed("")` an emptied box sends, and that is precisely the one the
        // block below hides from us. leaving it armed meant a word searched from the
        // hotkey, cleared, and then typed again by hand was swallowed — the search simply
        // did not happen, and the wordlist sat there empty with the word in the box.
        *self.forwarded.borrow_mut() = (!word.is_empty()).then(|| word.to_owned());
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

    /// show the opening page — the incipit of the book that named the whole idea (#48).
    fn show_cover(&self) {
        self.pane.set_visible_child_name("cover");
        // the strip lives *below* the stack, not inside it, so switching pages does not
        // take it with us: without this it sits under the opening page still announcing
        // "1 more definition below" about a definition nobody can see.
        self.fold.set_visible(false);
    }

    /// show the definition pane, which every method that writes into it wants.
    fn show_pane(&self) {
        self.pane.set_visible_child_name("definition");
    }

    /// which page the sidebar is on: the shelf until something is typed (#69).
    ///
    /// deliberately not tied to the pane's switch above, which looks like the same
    /// question and is not. a *typed* search selects nothing (#57), so the pane keeps
    /// the opening page while the wordlist fills with results — and the first version
    /// of this drove both from `show_cover`, which left the shelf sitting over a live
    /// wordlist for exactly that case. the sidebar follows the query; the pane follows
    /// what has been chosen.
    fn show_shelf(&self, nothing_typed: bool) {
        self.sidebar
            .set_visible_child_name(if nothing_typed { "shelf" } else { "words" });
    }

    /// plain message in the definition pane (hint / "no definition").
    fn set_message(&self, text: &str) {
        self.show_pane();
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
        self.show_shelf(query.is_empty());
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
                let tags = row
                    .dicts()
                    .iter()
                    .map(|&dict| {
                        language::tag(&row.word, collection.dict_derived(dict))
                            .map(str::to_owned)
                            .unwrap_or_else(|| collection.dict_short(dict).to_owned())
                    })
                    .collect();
                glib::BoxedAnyObject::new(Listed {
                    row: row.clone(),
                    names,
                    tags,
                })
            })
            .collect();
        self.model.splice(0, self.model.n_items(), &listed);

        if query.is_empty() {
            self.show_cover();
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
                // what the collection actually has, which after #71 is what was
                // remembered rather than "everything". hard-coding `true` here left
                // a panel that said every dictionary was searched while the search
                // itself was narrowed — the e2e check for the restart caught it.
                .active(self.collection.borrow().is_dict_active(index))
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
                // one line each, ellipsized rather than reflowed. two reasons, and the
                // second is the one that bites: a folder-name title like "Greek-English
                // Lexicon by John Jeffrey Dodson (Grc-Eng)" wrapped to two lines, so the
                // rows had ragged heights — and a wrapping label's width depends on its
                // height, which inside a list that never scrolls sideways is a
                // contradiction gtk reports on every launch ("reports a minimum width of
                // 14, but minimum width for height … Expect overlapping widgets").
                // ellipsis is a stopgap for names this long; #52 gives them real ones.
                .title_lines(1)
                .subtitle_lines(1)
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

    /// fill the shelf: one row per dictionary, with what it holds (#69). asked for —
    /// "when nothing is searched, i want to see a list of my dicts" — and it goes in
    /// the sidebar, in the wordlist's place, because that is where a list belongs.
    ///
    /// same material as `build_scope` and deliberately not the same widget: this one
    /// has no checkboxes and no handlers, because it says what is *there* rather than
    /// what is searched. #45/#52/#64 rewrite both, and will decide then whether one
    /// list can be both.
    fn build_shelf(&self) {
        let collection = self.collection.borrow();
        if !collection.is_ready() {
            return;
        }
        for index in 0..collection.dict_count() {
            let label = collection.dict_label(index);
            let row = adw::ActionRow::builder()
                // a dictionary's name is data — escape it, the row renders markup.
                .title(glib::markup_escape_text(label))
                .subtitle(collection::quantity(
                    collection.dict_headwords(index),
                    "headword",
                    "headwords",
                ))
                // one line each: a folder-name title long enough to wrap makes a
                // wrapping label's width depend on its height, which inside a list
                // that never scrolls sideways is the contradiction gtk complains
                // about on every launch. #52 gives these names worth reading.
                .title_lines(1)
                .subtitle_lines(1)
                .build();
            self.shelf_list.append(&row);
        }
    }

    /// a checkbox changed: update the mask, then re-run whatever is in the search
    /// box so the wordlist and the status line follow immediately.
    fn set_dict_active(&self, index: usize, active: bool) {
        self.collection.borrow_mut().set_dict_active(index, active);
        // and remember it for next time (#71). the path is cloned out first so the
        // collection is not still borrowed while the config is written.
        let path = self
            .collection
            .borrow()
            .dict_path(index)
            .map(Path::to_owned);
        if let Some(path) = path
            && let Err(err) = self.config.borrow_mut().remember_scope(&path, active)
        {
            // a config that cannot be written is worth saying out loud, and worth
            // nothing more than that: the scope still changed, for this session.
            eprintln!("dictu: could not remember the search scope: {err:#}");
        }
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
                // this writes into the definition buffer, so that is the page to be on —
                // otherwise the message lands behind the opening page and the window
                // looks like it ignored the word.
                self.show_pane();
                let buffer = self.definition.buffer();
                self.clear_sections(&buffer);
                buffer.set_text(&format!("No definition for “{word}”."));
                self.update_fold();
            }
        }
    }

    fn show_row(&self, row: &library::Row) {
        self.show_pane();
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
            buffer.insert_with_tags(&mut iter, label, &[&source_tag(&buffer)]);
            // and which languages this dictionary goes between, where its title says so
            // (#60). beside the name rather than at the pane's right edge: a text view
            // right-aligns a whole line, and pinning a run to the edge means a tab stop
            // at a pixel that stops being the edge the moment the pane is resized.
            // off the file's own name, not the heading: #52 lets the reader call this
            // "Gaffiot", and the pair is written in the folder name it came from.
            if let Some((from, to)) = language::pair(&definition.derived) {
                let chip = format!("   {from} → {to}");
                buffer.insert_with_tags(&mut iter, &chip, &[&pair_tag(&buffer)]);
            }
            buffer.insert_with_tags(&mut iter, "\n", &[&source_tag(&buffer)]);

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

pub(crate) fn build(
    app: &adw::Application,
    config: &Rc<RefCell<config::Config>>,
    entries: &[config::DictEntry],
) -> Ui {
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

    // the shelf: what is in the library, shown where the wordlist would be if anything
    // had been typed (#69). a boxed list rather than more rows like the wordlist's, so
    // that at a glance it is plainly not search results — a card with separators and a
    // heading over it, against flat rows with a language tag.
    let shelf_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    shelf_list.add_css_class("boxed-list");
    // the heading above it is a separate widget, so without this the list is anonymous
    // to a screen reader — and to the harness, which tells the three lists in this
    // window apart by name.
    shelf_list.update_property(&[gtk::accessible::Property::Label("Your dictionaries")]);
    // the other empty state, and a different one: nothing typed is not the same as
    // nothing to type against (#61 — a window that looks broken costs more than it
    // saves). gtk shows this exactly while the list has no rows, and the list is only
    // ever on screen after indexing, so it cannot be read as "still loading".
    let nothing = label(
        "No dictionaries.\n\nAdd a folder to dictionary_dirs in config.toml, \
         then start dictu again.",
    );
    nothing.add_css_class("dim-label");
    nothing.set_margin_top(24);
    shelf_list.set_placeholder(Some(&nothing));

    let heading = gtk::Label::builder()
        .label("Your dictionaries")
        .xalign(0.0)
        .margin_bottom(2)
        .build();
    heading.add_css_class("heading");
    heading.add_css_class("dim-label");

    let shelf = gtk::Box::new(gtk::Orientation::Vertical, 6);
    shelf.set_margin_top(6);
    shelf.set_margin_bottom(6);
    // the boxed-list card draws its own edge, which wants a little air the flat
    // wordlist does not.
    shelf.set_margin_start(2);
    shelf.set_margin_end(2);
    shelf.append(&heading);
    shelf.append(&shelf_list);
    let shelf_scroll = gtk::ScrolledWindow::new();
    shelf_scroll.set_child(Some(&shelf));
    shelf_scroll.set_vexpand(true);

    // one or the other, never both — the same arrangement the pane already has, and
    // switched by the same two methods, so the two halves of the window cannot end up
    // in different states.
    let sidebar_pages = gtk::Stack::new();
    sidebar_pages.add_named(&results_scroll, Some("words"));
    sidebar_pages.add_named(&shelf_scroll, Some("shelf"));
    // the wordlist first: nothing is shown until indexing finishes, and an empty
    // wordlist under "Indexing dictionaries…" is the honest picture of that.
    sidebar_pages.set_visible_child_name("words");
    sidebar_pages.set_vexpand(true);
    sidebar.append(&sidebar_pages);
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

    // the opening page is a *widget*, not text in the buffer (#48). a `TextView` draws an
    // inline paintable at its own size and clips the rest, so a picture in there could
    // neither centre itself nor shrink; a `Picture` in a box does both for free, and a
    // label can carry a real link.
    // clamped, because a `Picture` set to `Contain` scales *up* to whatever it is given:
    // on a wide pane the incipit grew to fill it instead of sitting at its own size. the
    // clamp caps the page at the picture's natural width and centres it, and below that
    // width everything — picture and prose — shrinks together.
    let clamp = adw::Clamp::builder()
        .maximum_size(560)
        .tightening_threshold(420)
        .child(&cover_page())
        .build();
    let cover_scroll = gtk::ScrolledWindow::new();
    cover_scroll.set_child(Some(&clamp));
    cover_scroll.set_hexpand(true);
    cover_scroll.set_vexpand(true);

    // one or the other, never both: the opening page until there is something to read.
    let pane = gtk::Stack::new();
    pane.add_named(&cover_scroll, Some("cover"));
    pane.add_named(&def_scroll, Some("definition"));
    pane.set_visible_child_name("definition");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&pane);
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
        // it used to say "this session's searches", which was true and is not any
        // more (#71): the choice is written to config.toml as it is made. what is
        // still config.toml's business is what gets *loaded*, and saying so is what
        // keeps this from reading like a way to remove a dictionary.
        .label("Remembered for next time. What gets loaded at all is config.toml's business.")
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

    // the dictionary list scrolls, and that is not a nicety: a popover asks for its
    // natural height, and fifteen rows of title-plus-subtitle ask for more than a
    // screen — at which point gtk maps nothing at all and the button appears to do
    // nothing when pressed (#61). the two-dictionary fixture is small enough that
    // every e2e check passed while the real collection could not open the panel.
    // capping it here means the panel fits whatever the collection grows to.
    let scope_scroll = gtk::ScrolledWindow::builder()
        .child(&scope_list)
        .propagate_natural_height(true)
        .max_content_height(420)
        // never scrolls sideways — a dictionary name ellipsizes instead — but then the
        // width cannot be left to the child: an ellipsizing row reports a width that
        // depends on its height, and gtk says so out loud ("minimum width of 18, but
        // minimum width for height … is 45. Expect overlapping widgets"). asking for a
        // width settles it, and it is a popover: it wants a deliberate one anyway.
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(340)
        .build();

    let scope_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    scope_box.set_margin_top(6);
    scope_box.set_margin_bottom(6);
    scope_box.set_margin_start(6);
    scope_box.set_margin_end(6);
    scope_box.append(&scope_title);
    scope_box.append(&scope_hint);
    scope_box.append(&scope_scroll);
    // outside the scroller: the options belong to the panel, not to the list of
    // dictionaries, and they should not scroll away under a long collection.
    scope_box.append(&options_list);

    let scope_button = gtk::MenuButton::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text("Search scope")
        .popover(&gtk::Popover::builder().child(&scope_box).build())
        .sensitive(false) // there is nothing to scope until indexing finishes.
        .build();
    scope_button.update_property(&[gtk::accessible::Property::Label("Search scope")]);

    // a cog, and it opens preferences on the first press. it began as a menu with one
    // item in it, which is a menu asking to be a button (#67): the window has exactly one
    // thing to configure, so a hamburger only added a step to reach it.
    let settings_button = gtk::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text("Preferences")
        // the window action the button stands for, so the same thing happens however it
        // is reached — the button, or `activate-action` from anywhere else.
        .action_name("win.preferences")
        .build();
    settings_button.update_property(&[gtk::accessible::Property::Label("Preferences")]);

    // both at the start: the scope filter is used often enough to sit leftmost, with the
    // cog beside it. the window's own controls keep the other end to themselves.
    let header = adw::HeaderBar::new();
    header.pack_start(&scope_button);
    header.pack_start(&settings_button);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Dictu")
        .default_width(900)
        .default_height(600)
        .content(&toolbar_with(&header, &split))
        .build();

    let preferences_action = gio::SimpleAction::new("preferences", None);
    preferences_action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| preferences(&window).present()
    ));
    window.add_action(&preferences_action);

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
        pane,
        sidebar: sidebar_pages.clone(),
        shelf_list: shelf_list.clone(),
        status,
        fold: fold.clone(),
        sections: Rc::new(RefCell::new(Vec::new())),
        shown: Rc::new(RefCell::new(None)),
        forwarded: Rc::new(RefCell::new(None)),
        search_changed: RefCell::new(None),
        collection: Rc::new(RefCell::new(Collection::empty())),
        config: config.clone(),
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
        move |_, key, _, state| match key {
            // escape empties the box, and an empty box is the opening page again
            // (#70). here rather than on the entry, because this controller is the
            // one place that sees the key wherever focus sits — in the wordlist or
            // the definition pane just as much as in the box.
            gdk::Key::Escape => {
                ui.search.set_text("");
                glib::Propagation::Stop
            }
            _ => ui.redirect_typing(key, state),
        }
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
        // the remembered scope is worked out here too, where the entries already
        // are, rather than kept alive on the main thread for the one moment the
        // library arrives.
        let library = Library::open(&entries_owned);
        let scope = library.scope_from(&entries_owned);
        let _ = tx.send_blocking((library, scope));
    });
    // weakly, and upgraded only after the await: indexing takes ~30s, so the
    // window can be closed while this is still pending.
    let ui_ready = Rc::downgrade(&ui);
    glib::spawn_future_local(async move {
        if let Ok((library, scope)) = rx.recv().await {
            let Some(ui) = ui_ready.upgrade() else { return };
            ui.collection.borrow_mut().open(library, scope);
            // size the scope before anything searches: the re-run below reads it.
            ui.build_scope();
            ui.build_shelf();
            // and put it on screen: `populate_results` below runs only if a word is
            // already waiting, so with an empty box nothing else would.
            ui.show_shelf(ui.search.text().trim().is_empty());
            ui.search.set_sensitive(true);
            ui.search
                .set_placeholder_text(Some("Search all dictionaries…"));
            ui.show_library_size();
            // the opening page, now that there is a collection to open it in front of.
            ui.show_cover();
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
/// the opening page: the incipit of the book that named the whole idea (#48).
///
/// John of Garland wrote his *Dictionarius* in Paris about 1200 — a list of the trades his
/// students saw in the street, latin with old french between the lines — and that line is
/// where the word "dictionary" comes from. the picture is a detail of a copy made a century
/// later which survived by being used as binding waste.
///
/// widgets rather than text in the definition buffer, which is what lets the picture centre
/// itself and shrink with the pane: `can_shrink` plus `Contain` scales it down to whatever
/// width there is, and its natural 520px is the ceiling, so a wide pane centres it instead
/// of stretching it.
///
/// the credit is on the page, not only in `assets/ATTRIBUTION.md`: the licence is CC BY-NC,
/// so attribution is a condition rather than a courtesy — and it carries the link to the
/// page the picture came from, so the claim is checkable rather than asserted.
fn cover_page() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 14);
    page.set_margin_top(28);
    page.set_margin_bottom(28);
    page.set_margin_start(24);
    page.set_margin_end(24);
    page.set_valign(gtk::Align::Center);

    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(INCIPIT)) {
        let picture = gtk::Picture::for_paintable(&texture);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_halign(gtk::Align::Center);
        // its own aspect ratio, so shrinking the pane shortens it rather than squashing it
        picture.set_height_request(80);
        picture.add_css_class("card");
        page.append(&picture);
    }

    let latin = label(
        "Dictionarius dicitur iste libellus a dictionibus magis necessarias, \
                       quas tenet quilibet scolaris…",
    );
    latin.add_css_class("title-4");
    latin.set_attributes(Some(&{
        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrInt::new_style(gtk::pango::Style::Italic));
        attrs
    }));
    page.append(&latin);

    let gloss = label(
        "“This little book is called a dictionarius, from the more necessary words \
         that every scholar keeps…”",
    );
    gloss.add_css_class("dim-label");
    page.append(&gloss);

    let source = label("John of Garland, Dictionarius — Paris, c. 1200; this copy c. 1300–1315.");
    source.add_css_class("dim-label");
    source.add_css_class("caption");
    page.append(&source);

    // markup, so the shelfmark is a link: gtk opens it for us on activation.
    let credit = label("");
    credit.set_use_markup(true);
    credit.set_markup(
        "<a href=\"https://digital.bodleian.ox.ac.uk/objects/\
         4021d35f-e1df-409f-a5b9-96dfa8cd417b/\">St John's College MS 235, fragment 62r</a> \
         · Bodleian Libraries, University of Oxford · Photo © The President and Fellows of \
         St John's College, Oxford · CC BY-NC 4.0",
    );
    credit.add_css_class("dim-label");
    credit.add_css_class("caption");
    page.append(&credit);

    page
}

/// a wrapped, centred label of the width a page of prose wants.
fn label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(58)
        .halign(gtk::Align::Center)
        .build()
}

/// the widgets one wordlist row is made of, built empty. a factory reuses these as
/// the reader scrolls, so they are made once and filled by `bind_row` — which is why
/// the tag and the count exist even for a row that wants neither, hidden rather than
/// absent.
///
/// two labels: the word, and where it comes from. there was a third holding a `·N`
/// count of answering dictionaries; #65 replaced it with the languages themselves.
fn row_widgets() -> gtk::Box {
    let word = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .hexpand(true)
        .build();

    let tag = gtk::Label::builder()
        .xalign(1.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        // wide enough for "LAT · FR · GRC"; the cap is for a long fallback title, and
        // the language codes should never be the thing that gets cut.
        .max_width_chars(16)
        .build();
    tag.add_css_class("dim-label");
    tag.add_css_class("caption");

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(12);
    row.set_margin_end(12);
    row.append(&word);
    row.append(&tag);
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

    // which languages answer, and never how many dictionaries did (#65). five latin
    // dictionaries are still one language and read `LAT`; a latin and a french one read
    // `LAT FR`, which is the thing worth knowing before clicking. the count that used
    // to sit here read as a multiplier — `LAT ·2` looks like "latin twice" — and the
    // number of dictionaries is already answered by the pane and by this tooltip.
    let mut languages: Vec<&str> = Vec::new();
    for tag in listed.tags.iter().map(String::as_str) {
        if !languages.contains(&tag) {
            languages.push(tag);
        }
    }
    if let Some(tag) = word.next_sibling().and_downcast::<gtk::Label>() {
        // cleared, not just hidden: these widgets are recycled, so anything left on one
        // is the *previous* row's — and a hidden label is still in the a11y tree, where
        // a stale tag would be read out under a word it has nothing to do with. every
        // branch sets every field.
        match languages.is_empty() {
            false => {
                // separated the way the status line separates its facts ("1,941,344
                // words · 15 dictionaries"), because a plain space read as one word:
                // "LAT FR" was too close together to see as two languages. the glyph is
                // free again now that no count uses it.
                tag.set_label(&languages.join(" · "));
                tag.set_tooltip_text(Some(&names.join("\n")));
                tag.set_visible(true);
            }
            true => {
                tag.set_label("");
                tag.set_tooltip_text(None);
                tag.set_visible(false);
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

/// the preferences window: what the global shortcut is, and how to change it (#67).
///
/// the shortcut is GNOME's, not ours — see `crate::shortcut` — so this window reads the
/// reader's own configuration and writes it back. on a desktop that stores shortcuts some
/// other way it says so instead of offering a control that could not work.
fn preferences(parent: &adw::ApplicationWindow) -> adw::PreferencesWindow {
    let window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .title("Preferences")
        .default_width(520)
        .default_height(320)
        .build();

    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder()
        .title("Global shortcut")
        .description(
            "Looks up whatever is selected, from any application. \
             This is a GNOME shortcut, so changing it here changes it for the desktop.",
        )
        .build();

    let row = adw::ActionRow::builder()
        .title("Look up the selection")
        .title_lines(1)
        .build();
    let value = gtk::Label::builder().build();
    value.add_css_class("dim-label");
    let button = gtk::Button::builder()
        .label("Change…")
        .valign(gtk::Align::Center)
        .build();
    row.add_suffix(&value);
    row.add_suffix(&button);
    group.add(&row);
    page.add(&group);
    window.add(&page);

    // shown, not assumed: whatever dconf holds right now, or the plain truth that there
    // is nothing bound yet.
    let refresh = {
        let value = value.clone();
        let row = row.clone();
        move || match shortcut::current() {
            Some(current) => {
                value.set_label(&current.accelerator);
                row.set_subtitle(&current.command);
            }
            None => {
                value.set_label("None");
                row.set_subtitle("no shortcut is bound yet");
            }
        }
    };
    refresh();

    if !shortcut::available() {
        value.set_label("unavailable");
        row.set_subtitle("this desktop does not store shortcuts in GNOME's media-keys schema");
        button.set_sensitive(false);
        return window;
    }

    button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        refresh,
        move |button| {
            capture_shortcut(&window, button, refresh.clone());
        }
    ));
    window
}

/// take the next key combination the reader presses and make it the shortcut.
///
/// gtk4 has no widget for this, so it is a key controller on a small modal: the first
/// combination that is not a bare modifier wins, Escape leaves things alone, and one that
/// would swallow ordinary typing is refused with a reason rather than written.
fn capture_shortcut(
    parent: &adw::PreferencesWindow,
    button: &gtk::Button,
    refresh: impl Fn() + Clone + 'static,
) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading("Press the new shortcut")
        .body("Hold a modifier — Super, Control or Alt — and press a key. Escape cancels.")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.set_close_response("cancel");

    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        button,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, state| {
            if key == gdk::Key::Escape {
                dialog.close();
                return glib::Propagation::Stop;
            }
            // a modifier on its own is the reader still reaching for the key.
            if matches!(
                key,
                gdk::Key::Control_L
                    | gdk::Key::Control_R
                    | gdk::Key::Alt_L
                    | gdk::Key::Alt_R
                    | gdk::Key::Super_L
                    | gdk::Key::Super_R
                    | gdk::Key::Shift_L
                    | gdk::Key::Shift_R
            ) {
                return glib::Propagation::Stop;
            }
            let wanted = state & gtk::accelerator_get_default_mod_mask();
            let accelerator = gtk::accelerator_name(key, wanted);
            if !shortcut::sensible(&accelerator) {
                dialog.set_body(&format!(
                    "{accelerator} would swallow that key everywhere. \
                     Hold Super, Control or Alt as well — or use a function key."
                ));
                return glib::Propagation::Stop;
            }
            let command = shortcut::current().map(|c| c.command).unwrap_or_else(|| {
                // nothing bound yet: bind the script that reads the selection, which is
                // what the shortcut is for. #66's installer is what puts it there.
                format!(
                    "{}/.local/bin/dictu-lookup",
                    glib::home_dir().to_string_lossy()
                )
            });
            match shortcut::set(&accelerator, &command) {
                Ok(()) => {
                    refresh();
                    dialog.close();
                }
                Err(err) => dialog.set_body(&format!("could not set it: {err:#}")),
            }
            let _ = button;
            glib::Propagation::Stop
        }
    ));
    dialog.add_controller(keys);
    dialog.present();
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
        let config = std::rc::Rc::new(std::cell::RefCell::new(crate::config::Config {
            dictionary_dirs: Vec::new(),
            dictionary: std::collections::BTreeMap::new(),
        }));
        let ui = build(&app, &config, &[]);
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
