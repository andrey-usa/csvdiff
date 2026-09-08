#!/usr/bin/env python3
"""Every native port that reads the format, on one pair of files, interleaved.

The question this answers is narrow on purpose: given the same columnar design
and the same input, how much is left for the language and the toolchain? The
four ports here read Parquet through code written for this project rather than
through a library, so nothing in the table is measuring somebody else's reader.

Two things it is careful about, both of which produced wrong numbers first.

**Interleaved, not one port at a time.** This machine's speed drifts under the
runs themselves -- page cache fills, and the kernel's supply of free 2 MB pages
is picked over and replenished -- so a number taken now and one taken twenty
minutes ago are comparing machine states, not builds. Every port runs once per
round, in the same order, and the rounds are what repeat.

**Best, median and worst, not just best.** A port that is quick once and slow
twice is not quick, and the spread is where memory pressure shows up.

Peak RSS is `wait4`'s rusage for that exact child -- the kernel's own high-water
mark rather than a poll that can miss a spike. These engines map their inputs,
so resident pages include the files; the column that carries information is
`above`, which subtracts them.

A port that cannot read the pair it is given is left out by name rather than
counted as slow or as agreeing with everyone: on `main` the Rust and Zig ports
refuse newline-delimited JSON, and a table that quietly omitted them would read
as though they had not been asked.

Ports built from another checkout can be added with `CSVDIFF_PORTS_EXTRA`, a
JSON array of `[label, path, extra_flags]`. That is how a branch's port is
measured against the same rows on the same host in the same sitting, which is
the only way two builds can be compared at all -- and the counts gate below
applies to them exactly as it does to the built-in four.

    python scripts/bench_ports.py A.parquet B.parquet --repeats 5
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KEY = "account_id,txn_id"
IGNORE = "updated_at"


def ports() -> list[tuple[str, list[str], list[str]]]:
    """(label, argv prefix, extra flags). A port that is not built is skipped by
    name rather than dropped: a missing build is not the same result as a slow
    one."""
    report = ["--engine", "turbo", "-o", "/dev/null"]
    out = []
    for label, path, flags in [
        ("C", ROOT / "c/csvdiff", []),
        ("C++", ROOT / "cpp/build/csvdiff", []),
        ("Rust", ROOT / "rust/target/release/csvdiff", report),
        ("Zig", ROOT / "zig/zig-out/bin/csvdiff", []),
    ]:
        if path.exists():
            out.append((label, [str(path)], flags))
        else:
            print(f"  {label:5s} -- not built ({path})", file=sys.stderr)
    out += extras()
    return out


def extras() -> list[tuple[str, list[str], list[str]]]:
    """Ports from `CSVDIFF_PORTS_EXTRA`: JSON `[[label, path, [flags...]], ...]`.

    A malformed value is an error rather than a silent empty list -- the whole
    reason to pass this is that a column is expected in the table, and a
    benchmark that quietly measured one fewer port than asked for is worse than
    one that refuses to start."""
    raw = os.environ.get("CSVDIFF_PORTS_EXTRA", "").strip()
    if not raw:
        return []
    try:
        spec = json.loads(raw)
        items = [(str(l), [str(p)], [str(f) for f in fl]) for l, p, fl in spec]
    except (ValueError, TypeError) as exc:
        raise SystemExit(f"CSVDIFF_PORTS_EXTRA is not [[label, path, [flags]]]: {exc}")
    out = []
    for label, prefix, flags in items:
        # Resolved against the repository root like the built-in four, so the
        # variable says the same thing wherever it is set from.
        prefix = [str(ROOT / prefix[0])]
        if Path(prefix[0]).exists():
            out.append((label, prefix, flags))
        else:
            print(f"  {label:5s} -- not built ({prefix[0]})", file=sys.stderr)
    return out


def slug(label: str) -> str:
    """A label is a table heading, not a filename; this makes it one."""
    return "".join(c if c.isalnum() else "_" for c in label)


def warm(*paths: str) -> None:
    """Reads the inputs once so the first timed run is not also a disk test."""
    for p in paths:
        with open(p, "rb") as fh:
            while fh.read(1 << 22):
                pass


def run(argv: list[str], err: str) -> tuple[float, float, float, int]:
    """Wall, peak RSS in MB, CPU seconds and exit code, for exactly this child."""
    started = time.monotonic()
    pid = os.fork()
    if pid == 0:
        o = os.open(os.devnull, os.O_WRONLY)
        e = os.open(err, os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
        os.dup2(o, 1)
        os.dup2(e, 2)
        os.execv(argv[0], argv)
        os._exit(127)
    _, status, usage = os.wait4(pid, 0)
    return (time.monotonic() - started, usage.ru_maxrss / 1024,
            usage.ru_utime + usage.ru_stime, os.waitstatus_to_exitcode(status))


def counts(path: str) -> dict | None:
    """The counts block, however a port spells the rest of its report."""
    try:
        with open(path) as fh:
            return json.load(fh).get("counts")
    except (OSError, ValueError):
        return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--repeats", type=int, default=5)
    ap.add_argument("--key", default=KEY)
    ap.add_argument("--ignore", default=IGNORE)
    ap.add_argument("--tmp", default="/tmp")
    args = ap.parse_args()

    builds = ports()
    if not builds:
        raise SystemExit("no ports are built")
    warm(args.a, args.b)
    size = (os.path.getsize(args.a) + os.path.getsize(args.b)) / 2**20

    # One run each first, to find out who can read this pair at all.
    reads: list[tuple[str, list[str], list[str]]] = []
    for label, prefix, flags in builds:
        out = f"{args.tmp}/ports_{slug(label)}.json"
        _, _, _, code = run(prefix + ["compare", args.a, args.b, "-k", args.key,
                                      "-i", args.ignore, "--json", out] + flags,
                            f"{args.tmp}/ports_err.txt")
        if code in (0, 1) and counts(out) is not None:
            reads.append((label, prefix, flags))
        else:
            why = open(f"{args.tmp}/ports_err.txt").read().strip().splitlines()
            print(f"  {label:5s} -- does not read this pair"
                  f"{': ' + why[0][:90] if why else ''}", file=sys.stderr)
    if not reads:
        raise SystemExit("no port read the pair")
    builds = reads

    times: dict[str, list[tuple[float, float, float]]] = {l: [] for l, _, _ in builds}
    answers: dict[str, dict | None] = {}
    for _ in range(args.repeats):
        for label, prefix, flags in builds:
            out = f"{args.tmp}/ports_{slug(label)}.json"
            secs, rss, cpu, code = run(
                prefix + ["compare", args.a, args.b, "-k", args.key, "-i", args.ignore,
                          "--json", out] + flags, f"{args.tmp}/ports_err.txt")
            if code not in (0, 1):
                why = open(f"{args.tmp}/ports_err.txt").read().strip()[:200]
                raise SystemExit(f"{label} failed ({code}): {why}")
            times[label].append((secs, rss, cpu))
            answers[label] = counts(out)

    print(f"\ninput {size:,.0f} MB total, {args.repeats} interleaved runs each\n")
    print("| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |")
    print("|---|---:|---:|---:|---:|---:|---:|")
    for label, _, _ in builds:
        ts = sorted(t for t, _, _ in times[label])
        rss = max(r for _, r, _ in times[label])
        cpu = min(c for _, _, c in times[label])
        print(f"| {label} | {ts[0]:.2f}s | {statistics.median(ts):.2f}s | {ts[-1]:.2f}s | "
              f"{cpu:.1f}s | {rss:,.0f} MB | {rss - size:,.0f} MB |")

    # The correctness gate. Ports that disagree about how many rows changed mean
    # a bug in one of them, not an interesting benchmark, so say which.
    have = {l: a for l, a in answers.items() if a}
    if len(have) > 1:
        first = next(iter(have.values()))
        odd = [l for l, a in have.items() if a != first]
        print(f"\ncounts agree across {len(have)} ports: {not odd}")
        if odd:
            print(f"  disagreeing: {', '.join(odd)}")
            print(f"  {json.dumps(first, sort_keys=True)}")
            for l in odd:
                print(f"  {l}: {json.dumps(have[l], sort_keys=True)}")
            return 1
        print(f"  {json.dumps(first, sort_keys=True)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
