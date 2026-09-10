#!/usr/bin/env python3
"""Merges one JSON per port into the two tables the scale run exists to produce.

The ceiling run and the floor run are separate jobs on separate runners -- the
ladder is larger than one runner's disk, and a port that dies at 40m should not
take the others down with it -- so the merge happens here, from artifacts, once
they have all finished.

  python scripts/scale_report.py --ceiling results/ceiling-*/*.json \\
      --floor results/floor-*/*.json --out report.md

Two tables come out. The first is how far each port got and what stopped it;
the second is the smallest memory limit each one finishes a fixed size in, which
is a different question and often a different order.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ORDER = ["c", "cpp", "rust", "zig"]
RUNNERS = ["ubuntu-latest", "ubuntu-24.04-arm", "windows-latest"]


def who(r: dict) -> str:
    """A row is a port on a runner. On one runner the runner is noise, so it is
    only spelled out when the results span more than one."""
    return f"{r.get('port','?')} · {r['runner']}" if r.get("runner") else r.get("port", "?")


def runner_rank(name: str) -> int:
    return RUNNERS.index(name) if name in RUNNERS else len(RUNNERS)


def load(paths: list[str]) -> list[dict]:
    out: list[dict] = []
    for p in paths:
        try:
            data = json.loads(Path(p).read_text())
        except (OSError, json.JSONDecodeError):
            continue
        out.extend(data if isinstance(data, list) else [data])
    return out


def rank(port: str) -> int:
    return ORDER.index(port) if port in ORDER else len(ORDER)


def ceiling_tables(rows: list[dict]) -> str:
    """Every size every port reached, and then the ceiling on its own."""
    if not rows:
        return "_No ceiling results._\n"
    multi = len({r.get("runner", "") for r in rows}) > 1
    if not multi:
        for r in rows:
            r["runner"] = ""
    ports = sorted({who(r) for r in rows},
                   key=lambda n: (runner_rank(n.split(" · ")[1]) if " · " in n else 0,
                                  rank(n.split(" · ")[0])))
    sizes: list[str] = []
    for r in rows:
        if r["size"] not in sizes:
            sizes.append(r["size"])
    sizes.sort(key=lambda s: next(x["rows"] for x in rows if x["size"] == s))

    by = {(who(r), r["size"]): r for r in rows}
    out = ["### How far each build got\n",
           "Wall time in seconds. **failed** is where the port stopped, and the",
           "reason is under the table.\n",
           "| Rows | " + " | ".join(ports) + " |",
           "|---|" + "|".join(["---:"] * len(ports)) + "|"]
    for s in sizes:
        cells = []
        for p in ports:
            r = by.get((p, s))
            if r is None:
                cells.append("–")
            elif r.get("ok"):
                cells.append(f"{r['wall_s']:.2f}s")
            else:
                cells.append("**failed**")
        out.append(f"| {s} | " + " | ".join(cells) + " |")

    out += ["", "### The ceiling\n",
            "| Build | Largest completed | Input at that size | Wall | Peak RSS | Stopped by |",
            "|---|---:|---:|---:|---:|---|"]
    for p in ports:
        mine = [r for r in rows if who(r) == p]
        ok = [r for r in mine if r.get("ok")]
        bad = [r for r in mine if not r.get("ok")]
        if not ok:
            out.append(f"| **{p}** | none | – | – | – | {bad[0]['why'] if bad else 'no result'} |")
            continue
        best = max(ok, key=lambda r: r["rows"])
        stopped = bad[0]["why"] + f" at {bad[0]['size']}" if bad else "ladder ran out, not the port"
        out.append(f"| **{p}** | **{best['size']}** | {best['input_mb']:,.0f} MB | "
                   f"{best['wall_s']:.2f}s | {best['rss_mb']:,.0f} MB | {stopped} |")
    return "\n".join(out) + "\n"


def floor_table(rows: list[dict]) -> str:
    if not rows:
        return "\n_No memory-floor results._\n"
    rows = sorted(rows, key=lambda r: (r.get("floor_mb") or 1 << 30, rank(r.get("port", ""))))
    for r in rows:
        if r.get("runner"):
            r["port"] = who(r)
    out = ["", "### The memory floor\n",
           "The smallest cgroup limit the same comparison finishes inside. Lower is",
           "better, and it is not the same ordering as speed. Peak RSS would not",
           "answer this: mapped pages are reclaimable, so that figure is whatever the",
           "kernel allowed, not what the engine needed.\n",
           "| Build | Size compared | Smallest limit that finishes |",
           "|---|---|---:|"]
    for r in rows:
        mb = r.get("floor_mb")
        out.append(f"| **{r.get('port','?')}** | {r.get('size','?')} | "
                   + (f"**{mb:,} MB** |" if mb else "did not finish |"))
    return "\n".join(out) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ceiling", nargs="*", default=[])
    ap.add_argument("--floor", nargs="*", default=[])
    ap.add_argument("--out", type=Path, default=Path("scale-report.md"))
    ap.add_argument("--title", default="Scale ceiling")
    args = ap.parse_args()

    body = (f"## {args.title}\n\n"
            + ceiling_tables(load(args.ceiling))
            + floor_table(load(args.floor)))
    args.out.write_text(body)
    print(body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
