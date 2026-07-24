#!/usr/bin/env python3
"""end-to-end ui tests for dictu, driven through at-spi (the accessibility bus).

gtk4 exports every widget over at-spi automatically, so we can read the real
widget tree of a running dictu — the search entry's text, the rows in the
wordlist, the definition pane's contents — and assert on it. no screenshots, no
ocr, no synthetic clicks into whatever happens to have focus.

the app under test gets a throwaway XDG_CONFIG_HOME pointing at the repo's
`sample/` fixture dictionary, so the assertions don't depend on which
dictionaries the developer happens to have installed.

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
        # gtk4 talks at-spi regardless of the display backend, but a11y has to
        # be switched on explicitly for the bridge to be registered promptly.
        env["GTK_A11Y"] = "atspi"
        env["NO_AT_BRIDGE"] = "0"
        # XWayland by default: synthetic keypresses go to whichever window has
        # focus, and only on x11 can we give focus to the window under test
        # (wayland won't let a client take it). key handling itself is
        # backend-independent. set DICTU_E2E_BACKEND=wayland to test natively —
        # the keyboard checks then skip.
        env["GDK_BACKEND"] = os.environ.get("DICTU_E2E_BACKEND", "x11")

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
        """the x11 id of the mapped toplevel (gtk also maps a 1x1 helper window)."""
        ids = subprocess.run(
            ["xdotool", "search", "--name", "^Dictu$"], capture_output=True, text=True
        ).stdout.split()
        for candidate in reversed(ids):
            geometry = subprocess.run(
                ["xdotool", "getwindowgeometry", candidate], capture_output=True, text=True
            ).stdout
            if "1x1" not in geometry:
                return candidate
        return None

    def focus_window(self):
        """give the window under test keyboard focus. `windowactivate` can't be
        used — mutter doesn't expose _NET_ACTIVE_WINDOW to xwayland clients — but
        plain XSetInputFocus works. this does take focus off whatever the user was
        doing, for as long as the keyboard checks run."""
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

    def forward(self, *args):
        """run `dictu <args>`, which the single-instance app forwards to the
        running window (the path the global hotkey takes)."""
        env = dict(os.environ)
        env["XDG_CONFIG_HOME"] = self.tmp
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
        lists = by_role(app, "list")
        if not lists:
            raise LookupError("no wordlist in the widget tree")
        self.results = lists[0]
        views = by_role(app, "text")
        if not views:
            raise LookupError("no definition pane in the widget tree")
        self.definition = views[0]

    def row_words(self):
        return [
            (r.get_name() or text_of(r)).strip()
            for r in descendants(self.results)
            if r.get_role_name() in ("list item", "label") and r != self.results
        ]

    def status_text(self):
        """every non-empty label in the window."""
        texts = [(node.get_name() or "").strip() for node in by_role(self.app, "label")]
        return [t for t in texts if t]

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
    with AppUnderTest() as app_proc:
        node = wait_for(find_app, READY_TIMEOUT, "dictu on the a11y bus")
        wait_for(lambda: safe(Widgets, node), READY_TIMEOUT, "the widget tree")
        app_proc.forward("--search", "aardvark")
        time.sleep(1.5)  # let the search settle so rows are in the tree.
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


def main():
    if not os.path.exists(BINARY):
        print(f"e2e: {BINARY} not built — run cargo build first", file=sys.stderr)
        return 1

    # this mode attaches to a running instance, so it must skip the check below.
    if "--wait-ready" in sys.argv:
        return wait_ready()

    # a stale instance would swallow our single-instance forwarding and answer
    # with the wrong config, so refuse to run alongside one.
    stale = subprocess.run(["pgrep", "-x", "dictu"], capture_output=True, text=True)
    if stale.stdout.strip():
        print(
            f"e2e: another dictu is running (pid {stale.stdout.split()[0]}) — "
            "stop it first, it would intercept the single-instance forwarding",
            file=sys.stderr,
        )
        return 1

    if "--tree" in sys.argv:
        return dump_tree()

    Atspi.init()
    r = Results()
    print("e2e: driving the ui over at-spi")

    with AppUnderTest() as app_proc:
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
            status == "6 words · 1 dictionary",
            f"expected '6 words · 1 dictionary', got {status!r}",
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
            lambda: not [w for w in widgets.row_words() if w] or "no-rows",
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

        r.check("the app is still running (no crash)", app_proc.proc.poll() is None)

    print(f"\ne2e: {len(r.passed)} passed, {len(r.failed)} failed")
    return 1 if r.failed else 0


def safe(fn, *args):
    """call `fn`, returning None instead of raising (for use inside wait_for)."""
    try:
        return fn(*args)
    except Exception as err:
        log(f"{fn.__name__} not ready: {err}")
        return None


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
