#!/usr/bin/env python3
"""compare `dictu bench` against a recorded baseline, and fail if something got
slower.

every performance number in this project has been measured by hand and then
written into a commit message, where nothing checks it again. this is the part
that checks it again: opening the collection, searching it, and the memory high
water mark — the three things a reader or their machine actually feels.

it measures the **real collection**, because that is the only one whose size is
interesting; the fixture is thirteen words. that makes the baseline personal to
this machine and this shelf of dictionaries, so it is a file you re-record rather
than a constant anyone can assert:

    hack/speed.py --record      # write hack/speed-baseline.json from this run
    hack/speed.py               # compare, and fail on a regression
    hack/speed.py --cold        # include the index rebuild (slow, ~10s, 800 MB)

it refuses to compare across a changed collection: add a dictionary and every
number moves for a reason that is not the code, so it says so and asks for a
re-record instead of crying regression.
"""

import json
import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BINARY = os.path.join(REPO, "target", "release", "dictu")
BASELINE = os.path.join(REPO, "hack", "speed-baseline.json")

# how much worse than the baseline is a regression rather than a slow machine.
# generous on purpose: this runs on a laptop, beside a browser, and the point is
# to catch a change that made something twice as slow, not to police 20%.
TOLERANCE = 1.6
# opening the collection gets its own, wider one. it is the only measurement that
# touches the disk, and taking the best of RUNS still left it swinging between
# 925 and 1310 ms over an unchanged binary — page cache, and nothing to do with us.
OPEN_TOLERANCE = 2.0
# and a floor per measurement, under which a ratio is noise rather than news.
FLOOR_NS = 100
FLOOR_MS = 150
FLOOR_MB = 64


# how many times to run the whole thing. opening the collection reads 217 MB of
# cache, so the first run of the day pays for a cold page cache and the next does
# not — measured 3375 ms then 1406 ms over the same unchanged binary. the best of
# several is the only number that means anything here; one run means nothing.
RUNS = 3


def bench(cold):
    """the best of `RUNS` runs, metric by metric."""
    args = [BINARY, "bench", "--json"] + (["--cold"] if cold else [])
    runs = []
    for _ in range(RUNS):
        done = subprocess.run(args, capture_output=True, text=True, timeout=900)
        if done.returncode != 0:
            print(f"speed: bench failed: {done.stderr.strip()}", file=sys.stderr)
            return None
        runs.append(json.loads(done.stdout))
        if cold:
            break  # a cold run is only cold once; a second would be measuring a warm one

    best = runs[0]
    for run in runs[1:]:
        best["open_ms"] = min(best["open_ms"], run["open_ms"])
        best["peak_rss_mb"] = min(best["peak_rss_mb"], run["peak_rss_mb"])
        for at, query in enumerate(run["queries"]):
            kept = best["queries"][at]
            kept["best_ns"] = min(kept["best_ns"], query["best_ns"])
    return best


def compare(now, before):
    """(regressions, lines) — one line per measurement, worst first."""
    checks = [
        ("open", "ms", now["open_ms"], before["open_ms"], FLOOR_MS, OPEN_TOLERANCE, 0),
        ("peak rss", "MB", now["peak_rss_mb"], before["peak_rss_mb"], FLOOR_MB,
         TOLERANCE, 0),
    ]
    was = {q["query"]: q["best_ns"] for q in before["queries"]}
    for query in now["queries"]:
        if query["query"] in was:
            # nanoseconds are what gets compared and microseconds are what gets
            # read: 8000 ns is a number, 8 µs is a sentence
            checks.append((f"search {query['query']!r}", "µs", query["best_ns"],
                           was[query["query"]], FLOOR_NS, TOLERANCE, 1000))

    regressions, lines = [], []
    for name, unit, current, baseline, floor, tolerance, per in checks:
        # a ratio against a tiny baseline says nothing; a floor keeps this honest
        ratio = current / baseline if baseline else 1.0
        over = current > floor and ratio > tolerance
        verdict = "SLOWER" if over else "ok"
        shown, was_shown = (f"{current / per:.1f}", f"{baseline / per:.1f}") if per \
            else (current, baseline)
        lines.append((ratio, f"  {verdict:>6}  {name:<22} {shown:>8} {unit}"
                             f"   was {was_shown} ({ratio:.2f}×)"))
        if over:
            regressions.append(name)
    lines.sort(reverse=True)
    return regressions, [line for _, line in lines]


def main():
    if not os.path.exists(BINARY):
        print(f"speed: {BINARY} is not built (cargo build --release)", file=sys.stderr)
        return 1
    record = "--record" in sys.argv
    # read the baseline before measuring: with nothing to compare against, the
    # measuring is half a minute spent on an answer nobody is going to look at
    if not record:
        if not os.path.exists(BASELINE):
            print("speed: no baseline yet — run hack/speed.py --record")
            return 0
        with open(BASELINE) as fh:
            before = json.load(fh)

    cold = "--cold" in sys.argv
    now = bench(cold)
    if now is None:
        return 1

    if record:
        with open(BASELINE, "w") as fh:
            json.dump(now, fh, indent=2, ensure_ascii=False)
            fh.write("\n")
        print(f"speed: recorded {now['headwords']} headwords, "
              f"open {now['open_ms']} ms, peak {now['peak_rss_mb']} MB")
        return 0

    # a different collection moves every number for a reason that is not the code
    if (now["headwords"], now["dictionaries"]) != (
        before["headwords"],
        before["dictionaries"],
    ):
        print(
            f"speed: the collection changed — {before['headwords']} words in "
            f"{before['dictionaries']} dictionaries when the baseline was recorded, "
            f"{now['headwords']} in {now['dictionaries']} now.\n"
            "       nothing to compare; re-record with hack/speed.py --record"
        )
        return 0
    if before.get("cold") != now["cold"]:
        print(
            f"speed: baseline is a {'cold' if before.get('cold') else 'warm'} run and "
            f"this is {'cold' if now['cold'] else 'warm'}; not comparable"
        )
        return 0

    regressions, lines = compare(now, before)
    print(f"speed: {now['headwords']} words in {now['dictionaries']} dictionaries, "
          f"{'cold' if cold else 'warm'}")
    for line in lines:
        print(line)
    if regressions:
        print(f"\nspeed: {len(regressions)} slower than baseline: {', '.join(regressions)}")
        print(f"       (a real change? re-record. tolerance is {TOLERANCE}×, {OPEN_TOLERANCE}× for open)")
        return 1
    print("\nspeed: nothing got slower")
    return 0


if __name__ == "__main__":
    sys.exit(main())
