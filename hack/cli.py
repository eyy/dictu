#!/usr/bin/env python3
"""checks that drive dictu's command line, over the `sample/` fixture.

these are the ones that were never really about widgets. asking "does an
unaccented query find the accented headword" through at-spi means a private
display, a gtk main loop, a window, and forty seconds; asking it through
`dictu search` means a subprocess and a few milliseconds. the answers come from
the same `Collection` either way, which is what roadmap #46 built the cli for.

what stays in hack/e2e.py is what genuinely needs the widgets: focus, keyboard
routing, the scope popover, link geometry, the fold strip.

the cli never starts gtk (see `main`: `cli::run` returns before the application
is built), so unlike the e2e suite this needs no display, takes no machine-wide
lock, and cannot disturb — or be disturbed by — a dictu the user has open.

run it: hack/cli.py [-v]
exit code is the verdict: 0 all checks passed, 1 something failed.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BINARY = os.path.join(REPO, "target", "debug", "dictu")

# the fixture's headwords: sample/sample.index (dictd) and sample/links/links.csv
# (tab-separated), minus the `00-database-short` control entry the reader hides.
SAMPLE_WORDS = ["aardvark", "byte", "dictionary", "gnome", "rust", "zeitgeist"]
LINK_WORDS = ["cf", "qv", "ext", "only", "byte", "λόγος"]
VERBOSE = "-v" in sys.argv


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


class Fixture:
    """a throwaway config and cache pointing at sample/, so these checks never
    read the developer's own collection and exercise the index cache from cold."""

    def __enter__(self):
        self.home = tempfile.mkdtemp(prefix="dictu-cli-")
        os.makedirs(os.path.join(self.home, "dictu"))
        with open(os.path.join(self.home, "dictu", "config.toml"), "w") as fh:
            fh.write(f'dictionary_dirs = ["{REPO}/sample"]\n')
        return self

    def __exit__(self, *_exc):
        shutil.rmtree(self.home, ignore_errors=True)
        return False

    def run(self, *args):
        """`dictu <args>` against the fixture; returns (exit code, stdout)."""
        env = dict(os.environ, XDG_CONFIG_HOME=self.home, XDG_CACHE_HOME=self.home)
        done = subprocess.run(
            [BINARY, *args], env=env, capture_output=True, text=True, timeout=120
        )
        if VERBOSE:
            print(f"    $ dictu {' '.join(args)}\n{done.stdout.rstrip()}")
        return done.returncode, done.stdout

    def json(self, *args):
        """the same, parsed — which also asserts the output *is* json."""
        code, out = self.run(*args, "--json")
        return code, json.loads(out)


def words_in(answer):
    """the row words of a `search --json` answer, in order."""
    return [row["word"] for row in answer["rows"]]


def main():
    if not os.path.exists(BINARY):
        print(f"cli: {BINARY} is not built", file=sys.stderr)
        return 1

    r = Results()
    print("cli: driving the command line over the fixture")
    with Fixture() as fixture:
        # the collection is what the config says it is, and both fixture files
        # loaded — a count alone cannot tell you the second one did.
        code, scope = fixture.json("scope")
        labels = sorted(d["label"] for d in scope["dictionaries"])
        r.check(
            "both fixture dictionaries load",
            code == 0 and labels == ["links", "sample"],
            f"exit={code}, labels={labels}",
        )
        r.check(
            "each reports its own size",
            [d["headwords"] for d in sorted(scope["dictionaries"], key=lambda d: d["label"])]
            == [6, 7],
            f"{scope['dictionaries']}",
        )

        # an empty query is the collection itself, and the window says the same
        # sentence for it — the two front ends share the line (#46).
        code, out = fixture.run("search", "")
        r.check(
            "an empty query answers with the library's size",
            code == 0 and out.strip() == "13 words · 2 dictionaries",
            f"exit={code}, {out.strip()!r}",
        )

        code, answer = fixture.json("search", "zeit")
        r.check(
            "a prefix finds its headword",
            code == 0 and words_in(answer) == ["zeitgeist"],
            f"exit={code}, {words_in(answer)}",
        )
        r.check(
            "and says it was not cut off",
            answer["truncated"] is False,
            f"truncated={answer['truncated']}",
        )

        code, answer = fixture.json("search", "zzzz")
        r.check(
            "a prefix that matches nothing is an empty answer, not a failure",
            code == 0 and answer["rows"] == [],
            f"exit={code}, rows={answer['rows']}",
        )

        missing = [
            word
            for word in SAMPLE_WORDS + LINK_WORDS
            if words_in(fixture.json("search", word)[1]) == []
        ]
        r.check("every fixture headword is findable", not missing, f"missing {missing}")

        # roadmap #39: the index is keyed by a normalized form, so an unaccented
        # query — with a plain sigma, which lowercasing alone never folded —
        # reaches the accented headword.
        _, answer = fixture.json("search", "λογοσ")
        r.check(
            "an unaccented query finds an accented headword",
            words_in(answer) == ["λόγος"],
            f"{words_in(answer)}",
        )
        # and the other half of the rule: an accent the query spells out is a
        # claim, so a grave is not answered with an acute.
        _, answer = fixture.json("search", "λὸγος")
        r.check(
            "a query's own accent rules out a different one",
            words_in(answer) == [],
            f"{words_in(answer)}",
        )

        # roadmap #12: a row names every dictionary that has its word. "byte" is
        # in both fixtures, and filed twice in the dictd one — which must still
        # count as one dictionary answering.
        _, answer = fixture.json("search", "byte")
        row = next((row for row in answer["rows"] if row["word"] == "byte"), None)
        r.check(
            "a row counts every dictionary that has the word",
            row is not None and row["dictionaries"] == 2,
            f"{row}",
        )
        _, answer = fixture.json("search", "aardvark")
        alone = next((row for row in answer["rows"] if row["word"] == "aardvark"), None)
        r.check(
            "and counts one when only one has it",
            alone is not None and alone["dictionaries"] == 1,
            f"{alone}",
        )

        # roadmap #33: neither fixture files an alias, so folding must change
        # nothing here — the check that it is not over-eager.
        plain = fixture.json("search", "byte")[1]
        folded = fixture.json("search", "byte", "--fold-forms")[1]
        r.check(
            "folding leaves a collection with no pointers alone",
            words_in(plain) == words_in(folded),
            f"{words_in(plain)} vs {words_in(folded)}",
        )

        # the limit is a count of rows, and "truncated" means truncated rather
        # than "you asked for exactly this many" (a review found that one).
        # no two fixture headwords share a first letter, so the only way to cut
        # this collection is to ask for nothing — which still has to *say* there
        # was more, since that is the flag's whole job.
        _, answer = fixture.json("search", "byte", "--limit", "0")
        r.check(
            "a limit cuts the answer and says so",
            answer["rows"] == [] and answer["truncated"] is True,
            f"rows={len(answer['rows'])}, truncated={answer['truncated']}",
        )
        _, answer = fixture.json("search", "aardvark", "--limit", "1")
        r.check(
            "a limit that is merely reached is not a truncation",
            answer["truncated"] is False,
            f"truncated={answer['truncated']}",
        )

        # `define` is what the pane would show, and a miss has to fail — or
        # `define x | grep -q y` would call a missing word a success.
        code, answer = fixture.json("define", "byte")
        dicts = [d["dictionary"] for d in answer["definitions"]]
        r.check(
            "define answers from every dictionary that has the word",
            code == 0 and sorted(dicts) == ["links", "sample"],
            f"exit={code}, {dicts}",
        )
        r.check(
            "and keeps a dictionary's several entries apart",
            any(len(d["entries"]) == 2 for d in answer["definitions"]),
            f"{[len(d['entries']) for d in answer['definitions']]}",
        )
        code, answer = fixture.json("define", "notaword")
        r.check(
            "a missing word fails, in the shape that was asked for",
            code == 1 and answer == {"word": "notaword", "definitions": []},
            f"exit={code}, {answer}",
        )

        # the file-shaped commands read a dictionary the collection knows nothing
        # about, which is the job they exist for.
        code, out = fixture.run("dump", os.path.join(REPO, "sample", "sample.index"))
        r.check(
            "dump reads a file directly",
            code == 0 and "headwords: 7" in out,
            f"exit={code}, {out.splitlines()[:2]}",
        )
        code, _ = fixture.run(
            "lookup", os.path.join(REPO, "sample", "sample.index"), "notaword"
        )
        r.check("lookup fails on a word a file does not have", code == 1, f"exit={code}")

        # and a mistyped command is an error rather than a window (#46).
        code, _ = fixture.run("serach", "byte")
        r.check("a mistyped command fails", code != 0, f"exit={code}")

    print(f"\ncli: {len(r.passed)} passed, {len(r.failed)} failed")
    return 1 if r.failed else 0


if __name__ == "__main__":
    sys.exit(main())
