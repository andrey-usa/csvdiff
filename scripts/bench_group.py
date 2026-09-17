#!/usr/bin/env python3
"""Merge benchmark JSON from several jobs, grouped by the CPU that produced it.

The problem this exists for is specific to hosted CI. A ladder fans one size out
per job to run them in parallel, and a matrix fans one format out per job, and
then a `collect` step merges the pieces into one table. Every one of those jobs
landed on a *different machine*, and GitHub's hosted fleet mixes processor
generations behind a single `ubuntu-latest` label -- a run can get a Xeon
Platinum 8370C or an EPYC 7763, with different cache and a different AVX-512
story. Merging them produces a table whose rows were measured on different
hardware, which is the exact thing this project's own rule forbids:

    Compare rows within a table. Never across tables.

A merged table was silently breaking that rule. This script will not: it groups
on `host.key` first and prints one table per CPU. Where a group holds only part
of the ladder it says so rather than filling the gap from another group, because
a rung measured on other silicon is not that rung's number.

Input is any number of JSON files written by `bench_formats_ports.py --json-out`
(or a directory of them, which is what `download-artifact` produces).

    python3 scripts/bench_group.py rungs/ --md-out ladder.md
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

from mdtable import render

# Row order inside a group: the declared port order, then format, then size, so
# two groups print their rows in the same order and can be read side by side
# even though their numbers may not be compared.
PORTS = ["C", "C++", "Rust", "Zig"]
FORMATS = ["csv", "ndjson", "parquet"]


def load(paths: list[Path]) -> list[dict]:
    """Every JSON file under the given files or directories, newest schema only."""
    files: list[Path] = []
    for p in paths:
        files.extend(sorted(p.rglob("*.json")) if p.is_dir() else [p])

    runs = []
    for f in files:
        try:
            d = json.loads(f.read_text())
        except (OSError, json.JSONDecodeError) as e:
            print(f"  skipped {f}: {e}", file=sys.stderr)
            continue
        if "results" not in d:
            continue
        # A file written before the host block existed cannot be grouped, and
        # guessing which CPU it came from is exactly the error this guards. It
        # goes in its own group, named for what it is.
        h = d.get("host") or {"key": "unrecorded CPU", "cpu": "unrecorded",
                              "cores": d.get("cores", 0), "isa": "?"}
        for r in d["results"]:
            runs.append({**r, "_host": h, "_rows": d.get("rows_arg", "?"),
                         "_file": f.name})
    return runs


def sort_key(r: dict) -> tuple:
    port = r.get("port", "")
    fmt = r.get("format", "")
    return (PORTS.index(port) if port in PORTS else len(PORTS), port,
            FORMATS.index(fmt) if fmt in FORMATS else len(FORMATS), fmt,
            str(r.get("_rows", "")))


def table(rows: list[dict], show_rows: bool) -> str:
    head = ["Build", "Format"] + (["Size"] if show_rows else []) + [
        "Compare", "Rows/s", "CPU", "Cores", "Peak RSS", "Above the input", "Budget"]
    align = ["l", "l"] + (["l"] if show_rows else []) + ["r"] * 7
    body = []
    for r in sorted(rows, key=sort_key):
        sec = r.get("seconds", 0.0)
        n = r.get("rows", 0)
        body.append(
            [r.get("port", "?"), r.get("format", "?")]
            + ([str(r.get("_rows", "?"))] if show_rows else [])
            + [f"{sec:.2f}s",
               f"{int(n / sec):,}" if sec > 0 else "-",
               f"{r.get('cpu', 0):.1f}s",
               f"{r.get('cores', 0):.2f}x",
               f"{r.get('rss', 0):,.0f} MB",
               f"{r.get('above', 0):,.0f} MB",
               f"{r.get('data', 0):,.0f} MB"])
    return render(head, align, body)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("paths", nargs="+", type=Path,
                    help="JSON files, or directories to search for them")
    ap.add_argument("--md-out", type=Path, help="write the grouped markdown here")
    args = ap.parse_args(argv)

    runs = load(args.paths)
    if not runs:
        print("no benchmark JSON found", file=sys.stderr)
        return 1

    groups: dict[str, list[dict]] = defaultdict(list)
    for r in runs:
        groups[r["_host"]["key"]].append(r)

    # Largest group first: the one with the most measurements is the one a
    # reader wants at the top, and on a fleet that mostly hands out one model it
    # is also the whole ladder.
    ordered = sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0]))
    show_rows = len({r["_rows"] for r in runs}) > 1

    out = []
    if len(ordered) > 1:
        out.append(
            f"**{len(ordered)} different CPUs produced these {len(runs)} measurements.** "
            "Each table below is one CPU. Rows may be compared inside a table and "
            "**not** between tables -- that is not a formality here, it is the "
            "difference between measuring a change and measuring which machine "
            "the job landed on.\n")
        out.append("| CPU | cores | widest vector | measurements |")
        out.append("| --- | ---: | --- | ---: |")
        for key, rows in ordered:
            h = rows[0]["_host"]
            out.append(f"| {h.get('cpu','?')} | {h.get('cores','?')} | "
                       f"{h.get('isa','?')} | {len(rows)} |")
        out.append("")
    else:
        h = ordered[0][1][0]["_host"]
        out.append(f"One CPU for all {len(runs)} measurements: "
                   f"**{h.get('cpu','?')}**, {h.get('cores','?')} cores, "
                   f"{h.get('isa','?')}. Rows below are comparable.\n")

    for key, rows in ordered:
        h = rows[0]["_host"]
        if len(ordered) > 1:
            out.append(f"### {h.get('cpu','?')} — {h.get('cores','?')} cores, "
                       f"{h.get('isa','?')}\n")
        sizes = sorted({str(r["_rows"]) for r in rows})
        fmts = sorted({r.get("format", "?") for r in rows})
        out.append(f"*{len(rows)} measurements · sizes {', '.join(sizes)} · "
                   f"formats {', '.join(fmts)}*\n")
        out.append(table(rows, show_rows))
        out.append("")

    # A ladder is only a ladder if one CPU measured every rung. Say so plainly
    # rather than leaving a reader to notice which rows are missing.
    if len(ordered) > 1 and show_rows:
        all_sizes = {str(r["_rows"]) for r in runs}
        for key, rows in ordered:
            mine = {str(r["_rows"]) for r in rows}
            if mine != all_sizes:
                missing = ", ".join(sorted(all_sizes - mine))
                out.append(f"> **{rows[0]['_host'].get('cpu','?')} is missing "
                           f"{missing}.** Those rungs ran on other silicon and are "
                           f"in another table; this ladder is partial and its "
                           f"slope cannot be read as one curve.")
        out.append("")

    md = "\n".join(out)
    print(md)
    if args.md_out:
        args.md_out.parent.mkdir(parents=True, exist_ok=True)
        args.md_out.write_text(md)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
