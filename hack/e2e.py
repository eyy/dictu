#!/usr/bin/env python3
"""end-to-end ui tests for dictu, driven through at-spi (the accessibility bus).

gtk4 exports every widget over at-spi automatically, so we can read the real
widget tree of a running dictu — the search entry's text, the rows in the
wordlist, the definition pane's contents — and assert on it. no screenshots, no
ocr, no synthetic clicks into whatever happens to have focus.

the app under test gets a throwaway XDG_CONFIG_HOME pointing at the repo's
`sample/` fixture dictionary, so the assertions don't depend on which
dictionaries the developer happens to have installed.

it also gets its own Xvfb display, which matters for more than tidiness: on the
real gnome session the app runs under xwayland, so it is the only *x* client
around — `xdotool` cheerfully reports the pointer as being over it while the
click actually lands in whatever wayland window is drawn on top. on a private
display the app is the only window there, coordinates are exact (no compositor
shadow margins), and the tests never steal the user's focus or pointer.

run it: hack/e2e.py [-v]        -v echoes what the harness sees while it waits
        hack/e2e.py --tree      dump the live widget tree (roles + names) instead
                                of asserting — how you find the selector to use
exit code is the verdict: 0 all checks passed, 1 something failed.
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402  (must follow require_version)

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BINARY = os.path.join(REPO, "target", "debug", "dictu")
SAMPLE_DIR = os.path.join(REPO, "sample")

# the fixture dictionary's headwords (sample/sample.index), minus the
# `00-database-short` control entry that the dictd reader hides.
SAMPLE_WORDS = ["aardvark", "byte", "dictionary", "gnome", "rust", "zeitgeist"]
# sample/links/links.csv adds entries whose definitions carry real <a> links, plus
# a deliberately long "byte" — a headword the dictd fixture also has, so that one
# dictionary's answer pushes the other's below the fold.
LINK_WORDS = ["cf", "qv", "ext", "only", "byte", "λόγος"]
# 7 + 6 headwords across the two fixture dictionaries — the dictd fixture files
# "byte" twice, which is what the entry-numbering check needs.
IDLE_STATUS = "13 words · 2 dictionaries"

APP_NAME = "dictu"
# the status line always starts with a count, which is how we pick it out of the
# window's other labels.
COUNT_LINE = re.compile(r"^[\d,]+\+? (word|result|dictionar)")
READY_TIMEOUT = 60.0  # generous: indexing a real collection can take a while.
POLL = 0.15

VERBOSE = "-v" in sys.argv


def log(*args):
    if VERBOSE:
        print("   ", *args, file=sys.stderr)


# -- at-spi helpers ---------------------------------------------------------


def find_app(name=APP_NAME):
    """the at-spi application node for `name`, or None if it isn't on the bus."""
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        if app is not None and app.get_name() == name:
            return app
    return None


def descendants(node, depth=0, max_depth=25):
    """every node under `node`, depth-first, including itself."""
    yield node
    if depth >= max_depth:
        return
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None:
            yield from descendants(child, depth + 1, max_depth)


def by_role(root, role):
    """all descendants whose at-spi role name is `role`."""
    return [n for n in descendants(root) if n.get_role_name() == role]


def text_of(node):
    """a node's text via the Text interface, else its accessible name."""
    try:
        return Atspi.Text.get_text(node, 0, Atspi.Text.get_character_count(node))
    except Exception:
        return node.get_name() or ""


def underlined_range(node, text, word):
    """the offsets at-spi reports as underlined around `word`, which is how the
    link tag is checked: it must cover exactly the link's characters. gtk4 exposes
    text attributes but NOT character geometry, so this is the precise check
    available — clicking is aimed separately, see `link_click_column`."""
    offset = text.find(word)
    if offset < 0:
        return None
    attributes, start, end = Atspi.Text.get_attribute_run(node, offset, True)
    if attributes.get("underline") != "single":
        return None
    return start, end


def is_focused(node):
    return node.get_state_set().contains(Atspi.StateType.FOCUSED)


def window_is_active(app):
    """whether the app's window has keyboard focus. gtk drops injected keys for an
    unfocused window, so the keyboard checks depend on this."""
    return any(
        frame.get_state_set().contains(Atspi.StateType.ACTIVE) for frame in by_role(app, "frame")
    )


def wait_for(predicate, timeout, what):
    """poll `predicate` until it returns something truthy. raises on timeout."""
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            last = predicate()
        except Exception as err:  # a11y tree is racy while the ui builds.
            last = None
            log(f"waiting for {what}: {err}")
        if last:
            return last
        time.sleep(POLL)
    raise TimeoutError(f"timed out after {timeout:.0f}s waiting for {what}")


# -- the app under test ----------------------------------------------------


class PrivateDisplay:
    """an Xvfb server of our own, exported as DISPLAY for everything we spawn.
    at-spi is unaffected — it lives on the session bus, not on the display."""

    def __init__(self, size="1200x800x24"):
        self.size = size
        self.proc = None
        self.name = None

    def __enter__(self):
        for number in range(99, 120):
            if os.path.exists(f"/tmp/.X{number}-lock"):
                continue
            self.name = f":{number}"
            self.proc = subprocess.Popen(
                ["Xvfb", self.name, "-screen", "0", self.size],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            break
        if self.name is None:
            raise RuntimeError("no free display number between :99 and :119")

        os.environ["DISPLAY"] = self.name
        wait_for(
            lambda: os.path.exists(f"/tmp/.X11-unix/X{self.name[1:]}") or None,
            10,
            f"Xvfb on {self.name}",
        )
        log(f"private display {self.name}")
        return self

    def __exit__(self, *_exc):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        return False


class AppUnderTest:
    """a dictu instance with a throwaway config pointing at sample/."""

    def __init__(self):
        self.tmp = tempfile.mkdtemp(prefix="dictu-e2e-")
        self.proc = None

    def __enter__(self):
        config_dir = os.path.join(self.tmp, "dictu")
        os.makedirs(config_dir)
        with open(os.path.join(config_dir, "config.toml"), "w") as fh:
            fh.write(f'dictionary_dirs = ["{SAMPLE_DIR}"]\n')

        env = dict(os.environ)
        env["XDG_CONFIG_HOME"] = self.tmp
        # and a throwaway cache, so a run neither reads nor leaves an index cache
        # in the real one (roadmap #7) — every run indexes the fixture from scratch.
        env["XDG_CACHE_HOME"] = self.tmp
        # gtk4 talks at-spi regardless of the display backend, but a11y has to
        # be switched on explicitly for the bridge to be registered promptly.
        env["GTK_A11Y"] = "atspi"
        env["NO_AT_BRIDGE"] = "0"
        # x11 on our private display; key handling is backend-independent, and
        # only on x11 can focus be handed to the window under test at all.
        env["GDK_BACKEND"] = "x11"
        # the cairo renderer keeps the x drawable up to date, so `import` captures
        # the current frame rather than the one the window first painted.
        env["GSK_RENDERER"] = "cairo"

        log(f"launching {BINARY} with XDG_CONFIG_HOME={self.tmp}")
        self.proc = subprocess.Popen(
            [BINARY],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        return self

    def __exit__(self, *_exc):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        shutil.rmtree(self.tmp, ignore_errors=True)
        return False

    def window_id(self):
        """the x11 id of the mapped toplevel (gtk also maps a 1x1 helper window).
        lowest id wins: a gtk4 popover is a surface of its own, its x window is
        named "dictu" — which xdotool's case-insensitive search matches too — and it
        is created later, so taking the last match would aim keys and clicks at the
        popover's origin instead of the window's."""
        ids = subprocess.run(
            ["xdotool", "search", "--name", "^Dictu$"], capture_output=True, text=True
        ).stdout.split()
        for candidate in sorted(ids, key=int):
            geometry = subprocess.run(
                ["xdotool", "getwindowgeometry", candidate], capture_output=True, text=True
            ).stdout
            if "1x1" not in geometry:
                return candidate
        return None

    def focus_window(self):
        """give the window under test keyboard focus — gtk drops injected keys for
        an unfocused window. there is no window manager on the private display, so
        plain XSetInputFocus is both available and enough (`windowactivate` needs
        _NET_ACTIVE_WINDOW, which nothing sets there)."""
        wid = self.window_id()
        if wid is None:
            return False
        subprocess.run(["xdotool", "windowfocus", wid], check=False, timeout=10)
        time.sleep(0.5)
        return True

    def press(self, key):
        """send one keypress to the window under test. `--window` targets it
        directly, so a key can never land in one of the user's own windows: if
        focus moved away, gtk drops the event instead."""
        subprocess.run(["xdotool", "key", "--window", self.window_id(), key], check=False, timeout=10)
        time.sleep(0.4)

    def type_text(self, text):
        subprocess.run(
            ["xdotool", "type", "--window", self.window_id(), text], check=False, timeout=10
        )
        time.sleep(0.4)

    def click_at(self, x, y):
        """click at coordinates relative to the window under test. the pointer being
        moved is the private display's, not the user's."""
        subprocess.run(
            ["xdotool", "mousemove", "--window", self.window_id(), str(x), str(y)],
            check=False,
            timeout=10,
        )
        time.sleep(0.25)
        subprocess.run(["xdotool", "click", "1"], check=False, timeout=10)
        time.sleep(0.5)

    def forward(self, *args):
        """run `dictu <args>`, which the single-instance app forwards to the
        running window (the path the global hotkey takes)."""
        env = dict(os.environ)
        env["XDG_CONFIG_HOME"] = self.tmp
        env["XDG_CACHE_HOME"] = self.tmp
        subprocess.run([BINARY, *args], env=env, timeout=30, check=False)


# -- the widgets we assert on ---------------------------------------------


class Widgets:
    """the three widgets the tests read. gtk4 maps `SearchEntry` to the at-spi
    role "entry" and `TextView` to "text" — they are NOT the same role, so each
    is found by its own."""

    def __init__(self, app):
        self.app = app
        entries = by_role(app, "entry")
        if not entries:
            raise LookupError("no search entry in the widget tree")
        self.search = entries[0]
        # by name, not by position: the scope panel holds a second list of the same
        # role, and it sits earlier in the tree (the header bar comes first).
        lists = [n for n in by_role(app, "list") if (n.get_name() or "") == "Wordlist"]
        if not lists:
            raise LookupError("no wordlist in the widget tree")
        self.results = lists[0]
        views = by_role(app, "text")
        if not views:
            raise LookupError("no definition pane in the widget tree")
        self.definition = views[0]

    def row_words(self):
        """the words in the wordlist. a row is a box holding the word and a dim
        language tag, so read the row's accessible name (the app sets it to the word)
        rather than sweeping up every label inside it."""
        return [
            (row.get_name() or "").strip()
            for row in by_role(self.results, "list item")
            if (row.get_name() or "").strip()
        ]

    def status_text(self):
        """every non-empty label in the window."""
        texts = [(node.get_name() or "").strip() for node in by_role(self.app, "label")]
        return [t for t in texts if t]

    def row_tags(self):
        """the dim tag at the end of each wordlist row — the language, or the
        dictionary's name when the language can't be named."""
        tags = []
        for row in by_role(self.results, "list item"):
            labels = [(node.get_name() or "").strip() for node in by_role(row, "label")]
            if labels:
                tags.append(labels[-1])
        return tags

    def fold_line(self):
        """the strip under the definition naming what is below the fold, or "" when
        it is hidden. hidden widgets stay in the a11y tree, so SHOWING is what
        distinguishes them."""
        for node in by_role(self.app, "label"):
            name = (node.get_name() or "").strip()
            if "below:" in name and node.get_state_set().contains(Atspi.StateType.SHOWING):
                return name
        return ""

    def status_line(self):
        """the dim count line under the wordlist, e.g. "6 words · 1 dictionary"
        or "1 result" — identified by starting with a number."""
        for text in self.status_text():
            if COUNT_LINE.match(text):
                return text
        return ""

    def definition_text(self):
        return text_of(self.definition)


# -- the checks ------------------------------------------------------------


class Results:
    def __init__(self):
        self.passed = []
        self.failed = []

    def check(self, name, condition, detail=""):
        if condition:
            self.passed.append(name)
            print(f"  pass  {name}")
        else:
            self.failed.append(name)
            print(f"  FAIL  {name}" + (f" — {detail}" if detail else ""))
        return condition


def print_tree(node, depth=0):
    """the widget tree as at-spi sees it — role, accessible name, text."""
    name = (node.get_name() or "")[:48]
    body = ""
    if node.get_role_name() in ("text", "entry"):
        body = f" text={text_of(node)[:48]!r}"
    print("  " * depth + f"{node.get_role_name()} name={name!r}{body}")
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None:
            print_tree(child, depth + 1)


def dump_tree():
    """launch the app against the fixture and print its widget tree."""
    Atspi.init()
    with PrivateDisplay(), AppUnderTest() as app_proc:
        node = wait_for(find_app, READY_TIMEOUT, "dictu on the a11y bus")
        wait_for(lambda: safe(Widgets, node), READY_TIMEOUT, "the widget tree")
        app_proc.forward("--search", "aardvark")
        time.sleep(1.5)  # let the search settle so rows are in the tree.
        safe(open_scope, node)  # popover widgets join the tree only while it is open.
        print_tree(node)
    return 0


def wait_ready():
    """block until an ALREADY-RUNNING dictu has finished indexing, then print its
    status line. used by hack/shot.sh so a screenshot never catches "Indexing…"."""
    Atspi.init()
    node = wait_for(find_app, READY_TIMEOUT, "dictu on the a11y bus")
    widgets = wait_for(lambda: safe(Widgets, node), READY_TIMEOUT, "the widget tree")
    print(wait_for(lambda: widgets.status_line() or None, READY_TIMEOUT, "indexing to finish"))
    return 0


def open_scope_only():
    """pop the search-scope panel up on an ALREADY-RUNNING dictu, so hack/shot.sh
    can screenshot it. its widgets are in the a11y tree only while it is open."""
    Atspi.init()
    node = wait_for(find_app, READY_TIMEOUT, "dictu on the a11y bus")
    boxes = open_scope(node)
    print(" ".join(sorted(boxes)))
    return 0


def main():
    if not os.path.exists(BINARY):
        print(f"e2e: {BINARY} not built — run cargo build first", file=sys.stderr)
        return 1

    # these modes attach to a running instance, so they must skip the check below.
    if "--wait-ready" in sys.argv:
        return wait_ready()
    if "--open-scope" in sys.argv:
        return open_scope_only()

    # a stale instance would swallow our single-instance forwarding and answer
    # with the wrong config, so refuse to run alongside one.
    stale = running_windows()
    if stale:
        print(
            f"e2e: another dictu window is running (pid {stale[0]}) — "
            "stop it first, it would intercept the single-instance forwarding",
            file=sys.stderr,
        )
        return 1

    if "--tree" in sys.argv:
        return dump_tree()

    Atspi.init()
    r = Results()
    print("e2e: driving the ui over at-spi")

    with PrivateDisplay(), AppUnderTest() as app_proc:
        node = wait_for(find_app, READY_TIMEOUT, "dictu on the a11y bus")
        r.check("app appears on the accessibility bus", node is not None)

        widgets = wait_for(lambda: safe(Widgets, node), READY_TIMEOUT, "the widget tree")
        r.check("search entry, wordlist and definition pane exist", widgets is not None)

        # the status line says "Indexing…" until the worker thread hands the
        # index over; that transition is the app telling us it is ready.
        # the status line reads "Indexing dictionaries…" until the worker thread
        # hands the index over; a line starting with a count means it's ready.
        status = wait_for(lambda: widgets.status_line() or None, READY_TIMEOUT, "indexing to finish")
        r.check("indexing finishes and the status line updates", bool(status), f"status={status!r}")
        log(f"status line: {status!r}")

        # roadmap #17/#18/#28: the idle line reports library size, with thousands
        # separators and nouns that agree with their counts.
        r.check(
            "idle status reports library size with agreeing plurals",
            status == IDLE_STATUS,
            f"expected {IDLE_STATUS!r}, got {status!r}",
        )

        # search via the single-instance path — the same route the global hotkey
        # uses — then read what the wordlist actually shows.
        app_proc.forward("--search", "zeit")
        rows = wait_for(
            lambda: [w for w in widgets.row_words() if w] or None,
            15,
            "wordlist rows for 'zeit'",
        )
        log(f"rows: {rows}")
        r.check(
            "searching 'zeit' shows the matching headword",
            rows == ["zeitgeist"],
            f"expected ['zeitgeist'], got {rows}",
        )

        # roadmap #18: while searching, the line counts what the list shows.
        result_line = wait_for(
            lambda: widgets.status_line() if "result" in widgets.status_line() else None,
            10,
            "the result count",
        )
        r.check(
            "the status line counts results, not index entries",
            result_line == "1 result",
            f"expected '1 result', got {result_line!r}",
        )

        # a prefix that matches nothing must clear the list, not keep stale rows.
        app_proc.forward("--search", "qqqq")
        cleared = wait_for(
            lambda: not [w for w in widgets.row_words() if w] or None,
            15,
            "the wordlist to clear",
        )
        r.check("a non-matching prefix clears the wordlist", bool(cleared))

        # every fixture headword must be reachable by its own full name.
        found = []
        for word in SAMPLE_WORDS:
            app_proc.forward("--search", word)
            try:
                wait_for(
                    lambda w=word: w in [x for x in widgets.row_words() if x] or None,
                    10,
                    f"row for {word}",
                )
                found.append(word)
            except TimeoutError:
                log(f"missing row for {word}")
        r.check(
            "every fixture headword is findable",
            found == SAMPLE_WORDS,
            f"found {found} of {SAMPLE_WORDS}",
        )

        # selecting a row must render that word's definition in the pane.
        app_proc.forward("--search", "aardvark")
        wait_for(lambda: "aardvark" in widgets.row_words() or None, 10, "the aardvark row")
        if select_first_row(widgets.results):
            definition = wait_for(
                lambda: (widgets.definition_text() or None) if "aardvark" in widgets.definition_text() else None,
                10,
                "the definition text",
            )
            log(f"definition: {definition[:120]!r}")
            r.check(
                "selecting a row renders its definition",
                "nocturnal" in definition.lower(),
                f"definition was {definition[:120]!r}",
            )
        else:
            r.check("selecting a row renders its definition", False, "could not activate a row")

        # roadmap #15: each row says which language it is. a greek headword is
        # tagged from its script; a latin-script word in a fixture dictionary whose
        # name says nothing falls back to that dictionary's name.
        app_proc.forward("--search", "λόγος")
        greek = wait_for(lambda: widgets.row_tags() or None, 10, "the greek row's tag")
        r.check(
            "a headword's script tags its language",
            greek == ["GRC"],
            f"expected ['GRC'], got {greek}",
        )
        app_proc.forward("--search", "zeit")
        fallback = wait_for(lambda: widgets.row_tags() or None, 10, "the fallback tag")
        r.check(
            "an unnameable language falls back to the dictionary's name",
            fallback == ["sample"],
            f"expected ['sample'], got {fallback}",
        )

        # roadmap #36: a search arriving from outside (the global hotkey's
        # `--search`) selects its first result by itself, so the window shows a
        # definition rather than a list to click. focus must stay in the search box.
        app_proc.forward("--search", "aardvark")
        selected = wait_for(
            lambda: Atspi.Selection.get_n_selected_children(widgets.results) or None,
            10,
            "the first row to select itself",
        )
        definition = widgets.definition_text()
        r.check(
            "a search from the hotkey selects its first result",
            selected == 1 and "nocturnal" in definition.lower(),
            f"selected={selected}, definition={definition[:80]!r}",
        )
        r.check(
            "selecting it does not steal the search box's focus",
            is_focused(widgets.search),
            "focus left the search entry",
        )

        # roadmap #35: a headword filed under several entries in ONE dictionary gets
        # them numbered, instead of run together as a single answer. the dictd
        # fixture files "byte" twice for exactly this.
        app_proc.forward("--search", "byte")
        wait_for(lambda: "byte" in widgets.row_words() or None, 10, "the byte row")
        select_first_row(widgets.results)
        numbered = wait_for(
            lambda: widgets.definition_text() if "1 of 2" in widgets.definition_text() else None,
            10,
            "the entry numbering",
        )
        r.check(
            "several entries under one headword are numbered",
            "1 of 2" in numbered and "2 of 2" in numbered,
            f"definition was {numbered[:120]!r}",
        )

        # roadmap #22: when several dictionaries define a word and they don't all
        # fit, a strip under the definition says how many are left and which.
        app_proc.forward("--search", "cf")
        wait_for(lambda: "cf" in widgets.row_words() or None, 10, "the cf row")
        select_first_row(widgets.results)
        time.sleep(0.8)
        r.check(
            "no fold strip when one dictionary answers",
            widgets.fold_line() == "",
            f"strip read {widgets.fold_line()!r}",
        )

        app_proc.forward("--search", "byte")
        wait_for(lambda: "byte" in widgets.row_words() or None, 10, "the byte row")
        select_first_row(widgets.results)
        fold = wait_for(lambda: widgets.fold_line() or None, 10, "the fold strip")
        log(f"fold strip: {fold!r}")
        r.check(
            "the fold strip counts and names the dictionaries below",
            fold == "1 more definition below: sample",
            f"expected '1 more definition below: sample', got {fold!r}",
        )

        # roadmap #14: the scope panel decides which dictionaries the search covers.
        # start from an empty search box, so the status line is the idle one.
        app_proc.forward("--search", "")
        wait_for(
            lambda: widgets.status_line() == IDLE_STATUS or None, 10, "the idle status line"
        )
        open_scope(node)
        rows = scope_rows(node)
        r.check(
            "the scope panel lists every dictionary with its size",
            rows == [("links", "6 headwords"), ("sample", "7 headwords")],
            f"panel rows read {rows}",
        )

        toggle_scope(app_proc, node, "sample")
        narrowed = wait_for(
            lambda: widgets.status_line() if "of 2" in widgets.status_line() else None,
            10,
            "the narrowed scope in the status line",
        )
        r.check(
            "deselecting a dictionary narrows the scope and the status line says so",
            narrowed == "6 words · 1 of 2 dictionaries",
            f"status={narrowed!r}",
        )

        # searching is done with the panel shut, the way a user would: a click that
        # lands in the popover while the search box is being filled from another
        # process is one race not worth chasing.
        close_scope(node)

        # "zeit" lives only in the dictd fixture, which is now out of scope. the
        # status line reaches the ui over the bus a beat after the rows do, so wait
        # on both rather than reading one and assuming the other.
        app_proc.forward("--search", "zeit")
        gone = wait_for(
            lambda: widgets.status_line()
            if not widgets.row_words() and "result" in widgets.status_line()
            else None,
            10,
            "'zeit' to leave the wordlist",
        )
        r.check(
            "a word from a deselected dictionary drops out of the wordlist",
            gone == "0 results · 1 of 2 dictionaries",
            f"rows={widgets.row_words()}, status={gone!r}",
        )

        # "cf" is in links.csv, which is still selected.
        app_proc.forward("--search", "cf")
        kept = wait_for(lambda: widgets.row_words() or None, 10, "the cf row")
        counted = wait_for(
            lambda: widgets.status_line() if "1 result" in widgets.status_line() else None,
            10,
            "the scoped result count",
        )
        r.check(
            "a word from a selected dictionary is still found, count included",
            kept == ["cf"] and counted == "1 result · 1 of 2 dictionaries",
            f"rows={kept}, status={counted!r}",
        )

        # nothing selected is a state of its own, not an empty result. the toggle
        # re-runs the search that is already in the box ("cf"), so no forwarding here.
        open_scope(node)
        toggle_scope(app_proc, node, "links")
        empty = wait_for(
            lambda: widgets.status_line()
            if "selected" in widgets.status_line()
            and "No dictionaries selected" in widgets.definition_text()
            else None,
            10,
            "the empty-scope status line",
        )
        r.check(
            "with nothing selected the ui says so rather than looking broken",
            empty == "0 dictionaries selected" and widgets.row_words() == [],
            f"status={empty!r}, rows={widgets.row_words()}, pane={widgets.definition_text()[:40]!r}",
        )

        # and back: reselecting restores both the wordlist and the counts.
        toggle_scope(app_proc, node, "links")
        toggle_scope(app_proc, node, "sample")
        close_scope(node)
        app_proc.forward("--search", "zeit")
        restored = wait_for(lambda: widgets.row_words() or None, 10, "the wordlist to come back")
        unscoped = wait_for(
            lambda: widgets.status_line() if "result" in widgets.status_line() else None,
            10,
            "the unscoped result count",
        )
        r.check(
            "reselecting brings the words back, and the scope note goes away",
            restored == ["zeitgeist"] and unscoped == "1 result",
            f"rows={restored}, status={unscoped!r}",
        )
        app_proc.forward("--search", "")
        idle = wait_for(
            lambda: widgets.status_line() if "word" in widgets.status_line() else None,
            10,
            "the idle status line",
        )
        r.check(
            "a full scope reads as the whole library again",
            idle == IDLE_STATUS,
            f"expected {IDLE_STATUS!r}, got {idle!r}",
        )

        # keyboard behaviour (roadmap #16, #23). synthetic keys land in whichever
        # window has focus, so skip rather than type into the user's terminal.
        app_proc.forward("--search", "aardvark")
        wait_for(lambda: "aardvark" in widgets.row_words() or None, 10, "the aardvark row")
        app_proc.focus_window()
        if not window_is_active(node):
            print("  skip  keyboard checks (could not focus the test window)")
        else:
            app_proc.press("Down")
            r.check(
                "Down from the search box selects the first row",
                Atspi.Selection.get_n_selected_children(widgets.results) == 1
                and "nocturnal" in widgets.definition_text().lower(),
                f"selected={Atspi.Selection.get_n_selected_children(widgets.results)}",
            )

            app_proc.press("Up")
            r.check(
                "Up from the first row returns to the search box",
                is_focused(widgets.search),
                "search entry did not regain focus",
            )

            # focus the list again, then type: the character must reach the search
            # box rather than being swallowed by the list.
            app_proc.press("Down")
            app_proc.type_text("x")
            entry_text = text_of(widgets.search)
            r.check(
                "typing while the wordlist has focus goes to the search box",
                entry_text == "aardvarkx" and is_focused(widgets.search),
                f"entry read {entry_text!r}, focused={is_focused(widgets.search)}",
            )

            # roadmap #20: links. first that the link tag covers exactly the link's
            # own characters, then that clicking one follows it — across
            # dictionaries, since "cf" lives in links.csv and "byte" in the dictd
            # fixture.
            app_proc.forward("--search", "cf")
            wait_for(lambda: "cf" in widgets.row_words() or None, 10, "the cf row")
            select_first_row(widgets.results)
            definition = wait_for(
                lambda: widgets.definition_text() if "byte" in widgets.definition_text() else None,
                10,
                "cf's definition",
            )
            log(f"cf definition: {definition!r}")
            span = underlined_range(widgets.definition, definition, "byte")
            offset = definition.find("byte")
            r.check(
                "a link is styled over exactly its own characters",
                span == (offset, offset + len("byte")),
                f"expected {(offset, offset + 4)}, at-spi reported {span}",
            )

            # aim at the "only" entry, whose definition is nothing but a link, so a
            # click can find it without character geometry (gtk4 exposes none).
            app_proc.forward("--search", "only")
            wait_for(lambda: "only" in widgets.row_words() or None, 10, "the only row")
            select_first_row(widgets.results)
            wait_for(
                lambda: "byte" in widgets.definition_text() or None, 10, "only's definition"
            )
            followed = link_click_column(app_proc, widgets)
            r.check(
                "clicking a link looks up its target, across dictionaries",
                followed is not None,
                "no click in the link's line followed it",
            )

        r.check("the app is still running (no crash)", app_proc.proc.poll() is None)

    print(f"\ne2e: {len(r.passed)} passed, {len(r.failed)} failed")
    return 1 if r.failed else 0


def running_windows():
    """pids of dictu processes that would answer our single-instance forwarding.
    `dictu dump|lookup|search …` short-circuits before any gtk setup, so it never
    claims the d-bus name — worth telling apart, since a cli search over a real
    collection runs for half a minute and would otherwise block the whole suite."""
    pids = subprocess.run(["pgrep", "-x", "dictu"], capture_output=True, text=True).stdout.split()
    windows = []
    for pid in pids:
        try:
            with open(f"/proc/{pid}/cmdline", "rb") as fh:
                argv = fh.read().decode(errors="replace").split("\0")
        except OSError:
            continue  # it exited while we looked.
        if argv[1:2] and argv[1] in ("dump", "lookup", "search"):
            continue
        windows.append(pid)
    return windows


def safe(fn, *args):
    """call `fn`, returning None instead of raising (for use inside wait_for)."""
    try:
        return fn(*args)
    except Exception as err:
        log(f"{fn.__name__} not ready: {err}")
        return None


def link_click_column(app_proc, widgets):
    """click down a column near the definition pane's left edge until one click
    lands on the link and follows it. gtk4 reports no character geometry over
    at-spi, so the y is searched rather than computed: the pane's width comes from
    at-spi, the window's position from xdotool, and the fixture entry's definition
    is a link and nothing else, so the link starts at the left margin. returns the
    followed definition, or None if no click hit it."""
    # on the private display there is no compositor, so the x window and the
    # logical window are the same size and at-spi's WINDOW coords can be used
    # directly. x is well inside the fixture's one wide link — which is a single
    # gap-free token on purpose, since the space between two words belongs to
    # neither link and a click there follows nothing.
    pane = Atspi.Component.get_extents(widgets.definition, Atspi.CoordType.WINDOW)
    x = pane.x + 70
    log(f"clicking down x={x} (pane at {pane.x},{pane.y})")
    for y in range(pane.y + 30, pane.y + 260, 8):
        app_proc.click_at(x, y)
        text = widgets.definition_text()
        if "unit of digital information" in text.lower() and text_of(widgets.search) == "byte":
            log(f"link followed by the click at ({x}, {y})")
            return text
    return None


def scope_toggle(app):
    """the header-bar button that pops the search-scope panel up. gtk4 renders a
    MenuButton as a push button wrapping a toggle button, and only the inner toggle
    carries the "click" action."""
    for node in descendants(app):
        if node.get_role_name() == "toggle button" and node.get_name() == "Search scope":
            return node
    raise LookupError("no scope button in the widget tree")


def scope_boxes(app):
    """the scope panel's check boxes, by dictionary name. the popover is a surface
    of its own, so its widgets are in the tree only while it is open."""
    return {
        node.get_name(): node
        for node in descendants(app)
        if node.get_role_name() == "check box" and node.get_name()
    }


def scope_rows(app):
    """each scope row as (dictionary, size), read off the row's two labels."""
    lists = [n for n in by_role(app, "list") if (n.get_name() or "") == "Dictionaries"]
    if not lists:
        return []
    rows = []
    for row in by_role(lists[0], "list item"):
        labels = [(n.get_name() or "").strip() for n in by_role(row, "label")]
        rows.append(tuple(labels))
    return rows


def open_scope(app):
    """pop the scope panel up; returns its check boxes by dictionary name."""
    Atspi.Action.do_action(scope_toggle(app), 0)
    return wait_for(lambda: scope_boxes(app) or None, 10, "the scope panel to open")


def close_scope(app):
    Atspi.Action.do_action(scope_toggle(app), 0)
    return wait_for(lambda: not scope_boxes(app) or None, 10, "the scope panel to close")


def is_checked(box):
    return box.get_state_set().contains(Atspi.StateType.CHECKED)


def toggle_scope(app_proc, app, dictionary):
    """flip one dictionary's check box, and prove it flipped. clicking is the only
    route — a check box exposes no Action, unlike a button — and at-spi reports its
    WINDOW extents in the toplevel's coordinates even though the popover is a
    surface of its own, so they can be aimed at directly. a click that misses
    dismisses the popover, so a miss reopens it and aims again."""
    for _ in range(3):
        box = scope_boxes(app).get(dictionary)
        if box is None:
            open_scope(app)
            continue
        was = is_checked(box)
        extents = Atspi.Component.get_extents(box, Atspi.CoordType.WINDOW)
        app_proc.click_at(extents.x + extents.width // 2, extents.y + extents.height // 2)
        try:
            return wait_for(
                lambda: is_checked(scope_boxes(app)[dictionary]) != was or None,
                3,
                f"the {dictionary!r} check box to flip",
            )
        except (TimeoutError, KeyError):
            log(f"the click on {dictionary!r} missed; reopening the panel")
    raise TimeoutError(f"could not toggle {dictionary!r} in the scope panel")


def select_first_row(results):
    """select the wordlist's first row. gtk4's `ListBoxRow` exposes no Action,
    so go through the list's Selection interface — the same thing a click does."""
    try:
        return Atspi.Selection.select_child(results, 0)
    except Exception as err:
        log(f"select_child failed: {err}")
        return False


if __name__ == "__main__":
    sys.exit(main())
