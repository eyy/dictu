#!/usr/bin/env python3
"""end-to-end ui tests for dictu, driven through at-spi (the accessibility bus).

these are the checks that need the widgets: focus, keyboard routing, the scope
popover, link geometry, the fold strip, and that what the collection answers
actually reaches the screen. the ones that were only ever about *answers* — is a
headword findable, does an unaccented query reach an accented entry — live in
hack/cli.py now, where they cost a subprocess instead of a display (roadmap #46).

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

**run it through `hack/check.sh`, or under `dbus-run-session`.** run bare, dictu
registers its accessibility tree on the user's own session bus and this file then
enumerates every application there, repeatedly, while it waits — gnome-shell 46
segfaulted twice under that, taking the whole desktop with it. a private bus costs
nothing and removes the entire class:

    dbus-run-session -- python3 hack/e2e.py [-v]

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
# how long to sleep between polls. small on purpose: the cheap predicates here ask
# one cached node a question, which costs 0.1–3 ms, so a 40 ms sleep was most of
# what a wait cost. what stops this from hammering the bus is the predicates
# themselves — a tree walk is 14–38 ms and paces its own loop — and that the bus is
# private, so nothing outside the harness is on the other end of it.
POLL = 0.005
# how many tabs to spend looking for one check box. the focus chain runs to about a
# dozen; this is a runaway guard, not a budget.
TAB_LIMIT = 30
# where in the definition pane the fixture's link sits, measured: the band that
# follows it runs from pane+107 to pane+128. `link_click_column` tries the nearest
# candidate to this first and then works outward, so it is a hint that saves clicks,
# never a requirement — the whole column is still searched if the layout moves.
LINK_BAND = 118

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


def wait_for_quiet(predicate, timeout=1.0):
    """poll until `predicate` holds, and say whether it did — for the places where
    not holding is an answer rather than an error: a probe click that followed no
    link, or an assertion that is about to fail and wants to report what it saw.
    returns as soon as it is true, so the timeout is a ceiling, not a cost."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # the a11y tree is racy while the ui rebuilds
            pass
        time.sleep(POLL)
    return False


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

        # and no animations. every transition the harness waits on — the scope
        # popover mapping above all — otherwise spends a few hundred ms being
        # pretty at a suite that has no eyes. a gtk setting rather than anything of
        # ours, so nothing under test behaves differently for it.
        gtk_dir = os.path.join(self.tmp, "gtk-4.0")
        os.makedirs(gtk_dir)
        with open(os.path.join(gtk_dir, "settings.ini"), "w") as fh:
            fh.write("[Settings]\ngtk-enable-animations=false\n")

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
        return True

    def press(self, key):
        """send one keypress to the window under test. it returns as soon as
        xdotool does, which is *before* gtk has seen the event — so every caller
        waits for the effect it expects rather than sleeping on a guess. `--window` targets it
        directly, so a key can never land in one of the user's own windows: if
        focus moved away, gtk drops the event instead."""
        subprocess.run(
            ["xdotool", "key", "--window", self.window_id(), key], check=False, timeout=10
        )

    def press_focused(self, key):
        """send one keypress to whatever holds keyboard focus, rather than to the
        toplevel. the scope panel needs this: a gtk4 popover is an x surface of its
        own, and `press` above deliberately aims at the *lowest* window id to keep
        keys out of the popover — so it is the wrong tool for keys meant for one.
        naming no window is safe here only because the private display has no other
        client that could catch a stray key."""
        subprocess.run(["xdotool", "key", key], check=False, timeout=10)

    def type_text(self, text):
        subprocess.run(
            ["xdotool", "type", "--window", self.window_id(), text], check=False, timeout=10
        )

    def click_at(self, x, y):
        """click at coordinates relative to the window under test. the pointer being
        moved is the private display's, not the user's.

        one caller is left — the link check, where clicking *is* the behaviour under
        test. the scope panel used to come through here too and now uses the
        keyboard (see `toggle_scope`), which is six times cheaper.

        the two sleeps are the only ones left in this file, and they are here
        because what they wait for cannot be observed: at-spi exposes no pointer
        position, and no "this surface is now taking input" for a surface that has
        just mapped. measured, not assumed: what a click needs is about 0.3s of
        quiet around it, and it does not much matter which side it goes on —
        0.1/0.2 lands 20 of 20, while 0.1/0.0 loses 3 and 0.05/0.0 loses 11, each
        loss costing a three-second timeout. everywhere else the harness waits for
        the effect."""
        subprocess.run(
            ["xdotool", "mousemove", "--window", self.window_id(), str(x), str(y)],
            check=False,
            timeout=10,
        )
        time.sleep(0.1)
        subprocess.run(["xdotool", "click", "1"], check=False, timeout=10)
        time.sleep(0.2)

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
        self._status = None  # see status_line: the label, once we have found it

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
        or "1 result" — identified by starting with a number.

        the label it lives in is remembered between calls, because more waits poll
        this than anything else and finding it means walking every label in the
        window: 24 ms a look against 0.3 ms for asking one node its name. gtk keeps
        the same accessible when the text changes (measured over four searches), and
        anything else — a rebuilt label, a dead node — fails the pattern below and
        falls back to the walk, so a stale cache costs a look, not a wrong answer."""
        if self._status is not None:
            try:
                text = (self._status.get_name() or "").strip()
                if COUNT_LINE.match(text):
                    return text
            except Exception:  # the node went away with the widget
                pass
            self._status = None

        for node in by_role(self.app, "label"):
            text = (node.get_name() or "").strip()
            if COUNT_LINE.match(text):
                self._status = node
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

        # roadmap #65: the tag names every *language* that answers, and never how many
        # dictionaries did. "byte" is in both fixtures — and filed twice in the dictd
        # one, which must not make it say anything twice. neither fixture names a
        # language, so each falls back to its own title, and the two are listed once
        # each: this is also the check that the fallback deduplicates.
        # wait for the row itself before reading its tag: the search entry debounces,
        # so a tag read straight after `forward` can still be the last query's.
        app_proc.forward("--search", "byte")
        wait_for(lambda: [w for w in widgets.row_words() if w] == ["byte"] or None, 10, "the byte row")
        both = widgets.row_tags()
        r.check(
            "the tag names each source that answers, once, and no count",
            both == ["links · sample"] or both == ["sample · links"],
            f"expected the two fixture names separated, and no count, got {both}",
        )
        # and one dictionary is named on its own, with nothing appended.
        app_proc.forward("--search", "aardvark")
        wait_for(
            lambda: [w for w in widgets.row_words() if w] == ["aardvark"] or None,
            10,
            "the aardvark row",
        )
        alone = widgets.row_tags()
        r.check(
            "a single source is named alone",
            alone == ["sample"],
            f"expected ['sample'], got {alone}",
        )

        # roadmap #57: a search the reader *types* selects nothing. the wordlist is a
        # model now, and gtk's `SingleSelection` selects the first item every time that
        # model changes unless told not to — which would put a definition on screen
        # after every keystroke, and choose one for you. `set_autoselect(false)` is the
        # single line that stops it, and nothing else here would notice if it went.
        #
        # the pane is asserted too, because it is the visible half of the mistake: an
        # auto-selection replaces this hint with a definition.
        app_proc.forward("--search", "")  # clear the box, and focus it
        wait_for(lambda: not [w for w in widgets.row_words() if w] or None, 10, "an empty list")
        app_proc.type_text("byte")
        wait_for(lambda: "byte" in widgets.row_words() or None, 10, "the typed byte row")
        # an absence, so the same rule as the debounce check above: leave it a beat in
        # which it could have gone wrong rather than asserting on the same instant.
        time.sleep(0.3)
        typed = Atspi.Selection.get_n_selected_children(widgets.results)
        r.check(
            "a typed search picks no row for you",
            typed == 0 and "Type to search" in widgets.definition_text(),
            f"selected={typed}, pane={widgets.definition_text()[:60]!r}",
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
        # roadmap #56: that search ran the instant it arrived instead of waiting out
        # `SearchEntry`'s 150ms debounce — and the debounced signal still comes. if it
        # is not suppressed it rebuilds the wordlist, which deselects every row, and
        # the definition above vanishes a sixth of a second after being asserted.
        #
        # so this one has to outlast the debounce, which is the rare case where a sleep
        # is the check: what must be waited for is the absence of an effect, at a
        # deadline gtk owns. 0.4s is the 150ms delay with room for a loaded machine.
        time.sleep(0.4)
        after = widgets.definition_text()
        r.check(
            "and the debounce catching up does not undo it",
            Atspi.Selection.get_n_selected_children(widgets.results) == 1
            and "nocturnal" in after.lower(),
            f"selected={Atspi.Selection.get_n_selected_children(widgets.results)}, "
            f"definition={after[:80]!r}",
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
        # an absence cannot be waited for — it is already true before the pane has
        # rendered anything, so asserting straight away would pass without looking.
        # wait for the definition instead: the strip is built in the same pass, so
        # once the text is there, an empty strip is the answer and not a race.
        wait_for(
            lambda: "compare" in widgets.definition_text().lower() or None,
            10,
            "the cf definition",
        )
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
        # roadmap #61: and the list of dictionaries sits inside a scroller, so the panel
        # fits however many the reader has. a popover asks for its natural height, and
        # fifteen unscrolled rows ask for more than a screen — at which point gtk maps
        # nothing and the button looks broken. this fixture has two dictionaries and can
        # never reproduce that, so what is checked is the structure that prevents it.
        r.check(
            "the dictionary list is inside a scroller, so the panel cannot outgrow the screen",
            scope_list_scrolls(node),
            "the Dictionaries list has no scroll-pane ancestor",
        )

        # roadmap #12: deselecting a dictionary while its definition is on screen has
        # to take that definition off the screen too. the row's count updates either
        # way, so a pane left behind would contradict the number beside it.
        close_scope(node)
        app_proc.forward("--search", "byte")  # in both fixtures
        # wait for the row, then select it deliberately. `--search` does auto-select
        # its first result (#36), but that is a one-shot flag consumed by whichever
        # repopulate runs first, so leaning on it here made this check fail about
        # one run in five. #36 has a check of its own; this one is about the pane.
        wait_for(
            lambda: [w for w in widgets.row_words() if w] == ["byte"] or None,
            10,
            "the byte row",
        )
        select_first_row(widgets.results)
        wait_for(
            lambda: "Sense 1" in widgets.definition_text() or None,
            10,
            "both dictionaries' definitions of byte",
        )
        open_scope(node)
        toggle_scope(app_proc, node, "links")
        pane = wait_for(
            lambda: widgets.definition_text()
            if "Sense 1" not in widgets.definition_text()
            else None,
            10,
            "the deselected dictionary to leave the definition pane",
        )
        r.check(
            "deselecting a dictionary clears its definition from the pane",
            "eight bits" in pane and widgets.row_tags() == ["sample"],
            f"pane={pane[:80]!r}, tags={widgets.row_tags()}",
        )
        toggle_scope(app_proc, node, "links")
        app_proc.forward("--search", "")
        wait_for(lambda: widgets.status_line() == IDLE_STATUS or None, 10, "the idle status line")

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

        # roadmap #33: the wordlist can be asked to show each definition once rather
        # than once per form pointing at it. neither fixture has an alias table, so
        # the list itself must not move — what must change is the status line,
        # because a list that quietly got shorter would be a mystery.
        app_proc.forward("--search", "byte")
        wait_for(
            lambda: [w for w in widgets.row_words() if w] == ["byte"] or None,
            10,
            "the byte row",
        )
        open_scope(node)
        toggle_scope(app_proc, node, "Fold repeated forms")
        noted = wait_for(
            lambda: widgets.status_line() if "folded" in widgets.status_line() else None,
            10,
            "the folded note in the status line",
        )
        r.check(
            "the status line says when repeated forms are folded away",
            noted == "1 result · forms folded",
            f"status={noted!r}",
        )
        r.check(
            "a dictionary that files no pointers is unaffected by the toggle",
            [w for w in widgets.row_words() if w] == ["byte"],
            f"rows={widgets.row_words()}",
        )
        toggle_scope(app_proc, node, "Fold repeated forms")
        back = wait_for(
            lambda: widgets.status_line() if "folded" not in widgets.status_line() else None,
            10,
            "the status line to drop the note",
        )
        r.check(
            "turning it back off restores the count",
            back == "1 result",
            f"status={back!r}",
        )
        close_scope(node)

        # keyboard behaviour (roadmap #16, #23). synthetic keys land in whichever
        # window has focus, so skip rather than type into the user's terminal.
        app_proc.forward("--search", "aardvark")
        wait_for(lambda: "aardvark" in widgets.row_words() or None, 10, "the aardvark row")
        app_proc.focus_window()
        try:
            wait_for(lambda: window_is_active(node) or None, 5, "the window to take focus")
        except TimeoutError:
            pass
        if not window_is_active(node):
            print("  skip  keyboard checks (could not focus the test window)")
        else:
            app_proc.press("Down")
            selected = wait_for_quiet(
                lambda: Atspi.Selection.get_n_selected_children(widgets.results) == 1
                and "nocturnal" in widgets.definition_text().lower()
            )
            r.check(
                "Down from the search box selects the first row",
                selected,
                f"selected={Atspi.Selection.get_n_selected_children(widgets.results)}",
            )

            app_proc.press("Up")
            r.check(
                "Up from the first row returns to the search box",
                wait_for_quiet(lambda: is_focused(widgets.search)),
                "search entry did not regain focus",
            )

            # focus the list again, then type: the character must reach the search
            # box rather than being swallowed by the list.
            app_proc.press("Down")
            app_proc.type_text("x")
            wait_for_quiet(lambda: text_of(widgets.search) == "aardvarkx")
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
    # the step only has to be smaller than the band it is hunting for, and that band
    # was measured rather than guessed: clicking every 3px down the column follows
    # the link from pane+107 to pane+128, so it is ~24px tall — one line of text —
    # and any step under 21 lands in it, where the 8 this used to step took eleven
    # clicks at 0.45s each to walk past.
    #
    # and the nearest candidate to that band goes first. it is a search either way,
    # over exactly the same column, so nothing here depends on the layout holding
    # still — a shifted band is found on the second or third click instead of the
    # first, which is what the old top-down order paid on every single run.
    pane = Atspi.Component.get_extents(widgets.definition, Atspi.CoordType.WINDOW)
    x = pane.x + 70
    log(f"clicking down x={x} (pane at {pane.x},{pane.y})")
    likely = pane.y + LINK_BAND
    for y in sorted(range(pane.y + 30, pane.y + 260, 16), key=lambda y: abs(y - likely)):
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


def scope_list_scrolls(app):
    """is the panel's dictionary list inside a scroll pane? walked upwards from the
    list itself, because that is the relationship that matters — a scroller elsewhere
    in the popover would not bound the thing that grows with the collection."""
    lists = [n for n in by_role(app, "list") if (n.get_name() or "") == "Dictionaries"]
    if not lists:
        return False
    node = lists[0]
    for _ in range(6):  # the popover is a few anonymous boxes deep; this is a guard
        parent = node.get_parent()
        if parent is None:
            return False
        if parent.get_role_name() == "scroll pane":
            return True
        node = parent
    return False


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


def scope_state(app):
    """one walk over the window: (check boxes by name, every focusable node).

    both in one pass, and passed around afterwards rather than looked up again:
    walking the window costs 38 ms with the panel open, while asking a node it
    already found whether it is focused costs 0.13 ms, and nothing moves while the
    panel stays up."""
    boxes, focusables = {}, []
    for node in descendants(app):
        states = node.get_state_set()
        if states.contains(Atspi.StateType.FOCUSABLE):
            focusables.append(node)
        if node.get_role_name() == "check box" and node.get_name():
            boxes[node.get_name()] = node
    return boxes, focusables


def focus_at(focusables):
    """where in the focus chain the focus is, as an index — enough to tell that Tab
    moved it, which is the only thing a tab has to be waited on for. every member of
    the chain counts, including the ones outside the panel: a tab that lands on the
    definition pane has to be observable too, or it would be waited out in full."""
    return next((at for at, node in enumerate(focusables) if is_focused(node)), None)


def focused_name(app):
    """whatever holds keyboard focus, as (role, name) — enough to tell that focus
    moved, which is what Tab has to be waited on for."""
    return next(((n.get_role_name(), n.get_name()) for n in descendants(app) if is_focused(n)), None)


def toggle_scope(app_proc, app, dictionary):
    """flip one dictionary's check box, and prove it flipped.

    the keyboard, not the pointer. a check box exposes no Action — measured, gtk4
    gives it exactly zero, unlike the button that opens the panel — so it has to be
    driven the way a person drives it, and Tab is the cheap way in: gtk routes the
    key itself, where `grab_focus()` is a request gtk4 declines outright (atspi_error
    1, twenty times out of twenty).

    clicking the box's centre also works and is what this used to do. it costs six
    times as much: aiming needs coordinates, a click that misses dismisses the
    popover instead of toggling anything, and landing one needs ~0.3s of quiet
    around it that nothing in at-spi can be waited on — measured 0.673s per flip
    against 0.111s here, both at 0 misses in 20. and the quiet is not negotiable:
    at 0.15s the miss rate is 11 in 20, and every miss costs a 3s timeout.

    Tab cannot miss, and where focus landed is a state to wait for rather than a
    guess to sleep through. focus survives a toggle, so repeated flips of one box
    tab only once.

    everything here is read off one walk of the window (see `scope_state`), because
    the walk is 38 ms and each question about what it found is a tenth of a
    millisecond."""
    if scope_boxes(app).get(dictionary) is None:
        open_scope(app)
    boxes, focusables = scope_state(app)
    box = boxes.get(dictionary)
    if box is None:
        raise LookupError(f"no {dictionary!r} check box in the scope panel")

    # tab until the box we want has focus. the chain is the whole window's, not the
    # popover's — entry, scroll pane, definition pane, then each dictionary's row
    # *and* its check box — so the walk is longer than the panel looks; it cycles,
    # so any starting point reaches any box, and adjacent boxes are two tabs apart.
    for _ in range(TAB_LIMIT):
        if is_focused(box):
            break
        was_at = focus_at(focusables)
        app_proc.press_focused("Tab")
        wait_for(lambda: focus_at(focusables) != was_at or None, 3, "focus to move")
    else:
        raise TimeoutError(f"could not focus {dictionary!r} in the scope panel")

    was = is_checked(box)
    app_proc.press_focused("space")
    return wait_for(lambda: is_checked(box) != was or None, 3, f"{dictionary!r} to flip")


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
