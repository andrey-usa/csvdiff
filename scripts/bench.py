#!/usr/bin/env python3
"""Benchmark one scale end to end and record the numbers.

  python scripts/bench.py --rows 1m --engine duckdb --out-dir bench

Runs the comparison in a child process so peak RSS is measured honestly, writes
bench/<scale>-<engine>.json, appends a row to $GITHUB_STEP_SUMMARY when running
in Actions, and exits non-zero if a time or memory budget is exceeded.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from csvdiff import engines  # noqa: E402
from scripts.gen_data import parse_rows  # noqa: E402

# Budgets on a 4-vCPU / 16 GB GitHub-hosted runner. Generous by ~2x so the gate
# catches real regressions, not noise.
BUDGETS = {                     # rows: (compare seconds, peak RSS MB)
    10_000: (20, 1_500),
    1_000_000: (120, 6_000),
    10_000_000: (900, 12_000),
}
KEY = "account_id,txn_id"
IGNORE = "updated_at"


def budget_for(rows: int):
    for n in sorted(BUDGETS):
        if rows <= n:
            return BUDGETS[n]
    return BUDGETS[max(BUDGETS)]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rows", "-n", default="10k")
    ap.add_argument("--engine", choices=engines.NAMES, default="auto")
    ap.add_argument("--out-dir", "-o", default="bench")
    ap.add_argument("--data-dir", default="data")
    ap.add_argument("--keep-data", action="store_true", help="Do not delete the CSVs afterwards")
    ap.add_argument("--threads", type=int)
    ap.add_argument("--memory-limit", default=None, help="DuckDB memory limit, e.g. 8GB")
    ap.add_argument("--no-budget", action="store_true", help="Report numbers without failing on budget")
    ap.add_argument("--key", default=KEY, help=f"Key columns (default {KEY})")
    ap.add_argument("--ignore", default=IGNORE, help=f"Columns to skip (default {IGNORE}; '' compares them all)")
    ns = ap.parse_args()

    rows = parse_rows(ns.rows)
    label = ns.rows.lower()
    os.makedirs(ns.out_dir, exist_ok=True)
    a = os.path.join(ns.data_dir, f"{label}_a.csv")
    b = os.path.join(ns.data_dir, f"{label}_b.csv")

    gen_seconds = 0.0
    if not (os.path.exists(a) and os.path.exists(b)):
        t0 = time.perf_counter()
        subprocess.run([sys.executable, "scripts/gen_data.py", "-n", ns.rows, "-o", ns.data_dir],
                       check=True)
        gen_seconds = round(time.perf_counter() - t0, 1)

    report = os.path.join(ns.out_dir, f"{label}-{ns.engine}.html")
    summary = os.path.join(ns.out_dir, f"{label}-{ns.engine}-summary.json")
    runner = ("import resource, sys; from csvdiff.cli import main; rc = main(sys.argv[1:]); "
              "print('PEAK_RSS_KB', resource.getrusage(resource.RUSAGE_SELF).ru_maxrss, file=sys.stderr); "
              "sys.exit(rc)")
    cmd = [sys.executable, "-c", runner, "compare", a, b, "-k", ns.key,
           "--engine", ns.engine, "-o", report, "--json", summary]
    if ns.ignore:
        cmd += ["-i", ns.ignore]
    if ns.threads:
        cmd += ["--threads", str(ns.threads)]
    if ns.memory_limit:
        cmd += ["--memory-limit", ns.memory_limit]

    t0 = time.perf_counter()
    proc = subprocess.run(cmd, capture_output=True, text=True)
    wall = round(time.perf_counter() - t0, 2)
    print(proc.stdout, end="")
    print(proc.stderr, end="", file=sys.stderr)
    if proc.returncode not in (0, 1):
        killed = " (killed by signal, most likely out of memory)" if proc.returncode < 0 else ""
        print(f"compare failed with exit code {proc.returncode}{killed}", file=sys.stderr)
        return 2
    if not os.path.exists(summary):
        print(f"compare wrote no summary at {summary}", file=sys.stderr)
        return 2

    # The child reports its own peak; ru_maxrss is KB on Linux, bytes on macOS.
    peak_kb = next((int(l.split()[1]) for l in proc.stderr.splitlines() if l.startswith("PEAK_RSS_KB")), 0)
    if platform.system() == "Darwin":
        peak_kb //= 1024
    peak_mb = round(peak_kb / 1024, 1)

    with open(summary) as f:
        counts = json.load(f)["counts"]
    rec = {
        "rows": rows, "scale": label, "engine": ns.engine, "key": ns.key, "ignore": ns.ignore,
        "generate_seconds": gen_seconds, "compare_seconds": wall, "peak_rss_mb": peak_mb,
        "input_mb": round((os.path.getsize(a) + os.path.getsize(b)) / 1e6, 1),
        "report_mb": round(os.path.getsize(report) / 1e6, 2),
        "rows_per_second": int((counts["a_rows"] + counts["b_rows"]) / wall) if wall else None,
        "counts": counts,
        "runner": f"{platform.system()} {platform.machine()} py{platform.python_version()} "
                  f"{os.cpu_count()} cpu",
    }
    with open(os.path.join(ns.out_dir, f"{label}-{ns.engine}.json"), "w") as f:
        json.dump(rec, f, indent=2)

    budget_s, budget_mb = budget_for(rows)
    ok = wall <= budget_s and peak_mb <= budget_mb
    line = (f"| {label} | {ns.engine} | {rec['input_mb']:,} MB | {gen_seconds}s | **{wall}s** | "
            f"{rec['rows_per_second']:,}/s | {peak_mb:,} MB | {rec['report_mb']} MB | "
            f"{counts['changed']:,} | {counts['added']:,} | {counts['removed']:,} | "
            f"{'pass' if ok else f'over budget ({budget_s}s / {budget_mb} MB)'} |")
    step = os.environ.get("GITHUB_STEP_SUMMARY")
    if step:
        with open(step, "a") as f:
            if os.path.getsize(step) == 0:
                f.write("| scale | engine | input | generate | compare | throughput | peak RSS | "
                        "report | changed | added | removed | budget |\n"
                        "|---|---|---|---|---|---|---|---|---|---|---|---|\n")
            f.write(line + "\n")
    print(line)

    if not ns.keep_data:
        for p in (a, b):
            os.path.exists(p) and os.remove(p)
    return 0 if (ok or ns.no_budget) else 1


if __name__ == "__main__":
    raise SystemExit(main())
