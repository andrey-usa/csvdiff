#!/usr/bin/env python3
"""Run every engine over every scale and rank them.

  python scripts/bench_matrix.py --scales 10k,1m --engines duckdb,polars,arrow
  python scripts/bench_matrix.py --scales 10k,1m,10m --repeat 3

Each (scale, engine) pair runs `scripts/bench.py` in a child process, so peak RSS
is the comparison's own. With `--repeat` the median run is reported. Results from
the reference engine (DuckDB by default) are the yardstick: an engine whose
`counts` or `columns` differ is reported as MISMATCH and disqualified, however
fast it was. Writes bench/matrix.json and bench/matrix.md, and appends the table
to $GITHUB_STEP_SUMMARY under Actions.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from csvdiff import engines  # noqa: E402
from scripts.bench import KEY, budget_for  # noqa: E402
from scripts.gen_data import parse_rows  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def run_one(scale: str, engine: str, ns, attempt: int) -> dict:
    """One bench.py run. Returns its record, or a failure record."""
    out_dir = os.path.join(ns.out_dir, f"run{attempt}")
    cmd = [sys.executable, "scripts/bench.py", "--rows", scale, "--engine", engine,
           "--out-dir", out_dir, "--data-dir", ns.data_dir, "--keep-data", "--no-budget"]
    if ns.threads:
        cmd += ["--threads", str(ns.threads)]
    if ns.memory_limit:
        cmd += ["--memory-limit", ns.memory_limit]
    if ns.key:
        cmd += ["--key", ns.key]
    if ns.ignore is not None:
        cmd += ["--ignore", ns.ignore]
    started = time.perf_counter()
    try:
        proc = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=ns.timeout)
    except subprocess.TimeoutExpired:
        return {"engine": engine, "scale": scale, "status": f"timeout after {ns.timeout}s"}
    record_path = os.path.join(out_dir, f"{scale}-{engine}.json")
    if proc.returncode != 0 or not os.path.exists(record_path):
        tail = (proc.stderr or proc.stdout).strip().splitlines()
        reason = tail[-1][:160] if tail else f"exit {proc.returncode}"
        # A child killed by the OOM killer comes back as -9 with no output.
        if proc.returncode == -9:
            reason = "killed (out of memory)"
        return {"engine": engine, "scale": scale, "status": reason,
                "compare_seconds": round(time.perf_counter() - started, 2)}
    with open(record_path) as f:
        record = json.load(f)
    record["status"] = "ok"
    with open(os.path.join(out_dir, f"{scale}-{engine}-summary.json")) as f:
        summary = json.load(f)
    record["columns"] = summary["columns"]
    # compare() alone, without interpreter start, imports and report rendering —
    # at small scales that overhead is most of the wall time.
    record["engine_seconds"] = summary["meta"]["seconds"]
    record["report_path"] = os.path.join(out_dir, f"{scale}-{engine}.html")
    return record


def median_run(runs: list[dict]) -> dict:
    ok = [r for r in runs if r.get("status") == "ok"]
    if not ok:
        return runs[-1]
    ok.sort(key=lambda r: r["compare_seconds"])
    best = ok[len(ok) // 2]
    best["runs"] = [r["compare_seconds"] for r in ok]
    return best


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scales", default="10k,1m", help="Comma-separated row counts")
    ap.add_argument("--engines", default=None,
                    help=f"Comma-separated engines (default: installed = {','.join(engines.available())})")
    ap.add_argument("--reference", default="duckdb", help="Engine the others are checked against")
    ap.add_argument("--repeat", type=int, default=1, help="Runs per pair; the median is reported")
    ap.add_argument("--timeout", type=int, default=3600, help="Seconds before a run is abandoned")
    ap.add_argument("--out-dir", "-o", default="bench")
    ap.add_argument("--data-dir", default="data")
    ap.add_argument("--keep-data", action="store_true")
    ap.add_argument("--threads", type=int)
    ap.add_argument("--memory-limit", default=None)
    ap.add_argument("--key", default=None, help=f"Key columns (default {KEY})")
    ap.add_argument("--ignore", default=None, help="Columns to skip; '' compares every column")
    ns = ap.parse_args()

    scales = [s.strip().lower() for s in ns.scales.split(",") if s.strip()]
    chosen = [e.strip() for e in ns.engines.split(",")] if ns.engines else engines.available()
    missing = [e for e in chosen if e not in engines.SPECS or not engines.is_available(e)]
    if missing:
        print(f"not installed: {', '.join(missing)}", file=sys.stderr)
        chosen = [e for e in chosen if e not in missing]
    os.makedirs(ns.out_dir, exist_ok=True)

    results: dict[str, dict[str, dict]] = {}
    for scale in scales:
        rows = parse_rows(scale)
        print(f"\n=== {scale} ({rows:,} rows x 20 columns) ===", flush=True)
        results[scale] = {}
        for engine in chosen:
            print(f"--- {engine} ", end="", flush=True)
            runs = [run_one(scale, engine, ns, i) for i in range(ns.repeat)]
            record = median_run(runs)
            results[scale][engine] = record
            if record.get("status") == "ok":
                print(f"{record['compare_seconds']}s, {record['peak_rss_mb']} MB", flush=True)
            else:
                print(f"FAILED: {record['status']}", flush=True)
        _verify(results[scale], ns.reference)
        if not ns.keep_data:
            for suffix in ("a", "b"):
                path = os.path.join(ns.data_dir, f"{scale}_{suffix}.csv")
                os.path.exists(path) and os.remove(path)

    report = _tables(results, scales, chosen, ns.reference)
    with open(os.path.join(ns.out_dir, "matrix.json"), "w") as f:
        json.dump(results, f, indent=2)
    with open(os.path.join(ns.out_dir, "matrix.md"), "w") as f:
        f.write(report)
    print("\n" + report)
    step = os.environ.get("GITHUB_STEP_SUMMARY")
    if step:
        with open(step, "a") as f:
            f.write(report)
    return 0


def _verify(scale_results: dict[str, dict], reference: str) -> None:
    """Flag engines whose counts or columns drift from the reference engine."""
    ref = scale_results.get(reference)
    if not ref or ref.get("status") != "ok":
        return
    for engine, record in scale_results.items():
        if engine == reference or record.get("status") != "ok":
            continue
        if not engines.SPECS[engine].contract:
            record["agreement"] = "n/a"
            continue
        same = record["counts"] == ref["counts"] and record["columns"] == ref["columns"]
        record["agreement"] = "yes" if same else "MISMATCH"
        if not same:
            diff = {k: (v, ref["counts"].get(k)) for k, v in record["counts"].items()
                    if ref["counts"].get(k) != v}
            record["counts_diff"] = diff
            print(f"    {engine} disagrees with {reference}: {diff}", flush=True)


def _tables(results: dict, scales: list[str], chosen: list[str], reference: str) -> str:
    lines = ["## Engine comparison\n"]
    for scale in scales:
        rows = parse_rows(scale)
        budget_s, budget_mb = budget_for(rows)
        ok = [r for r in results[scale].values() if r.get("status") == "ok"]
        fastest = min((r["compare_seconds"] for r in ok), default=None)
        lines += [f"\n### {scale} — {rows:,} rows x 20 columns "
                  f"(budget {budget_s}s / {budget_mb:,} MB)\n",
                  "| engine | compare | in-process | vs fastest | throughput | peak RSS | "
                  "report | agrees | notes |", "|---|---|---|---|---|---|---|---|---|"]
        order = sorted(results[scale].items(),
                       key=lambda kv: (kv[1].get("status") != "ok", kv[1].get("compare_seconds", 1e9)))
        for engine, record in order:
            if record.get("status") != "ok":
                lines.append(f"| `{engine}` | — | | | | | | | {record['status']} |")
                continue
            ratio = record["compare_seconds"] / fastest if fastest else 1
            note = engines.SPECS[engine].blurb
            if record.get("agreement") == "MISMATCH":
                note = f"**disagrees with {reference}**: {record.get('counts_diff')}"
            lines.append(
                f"| `{engine}` | **{record['compare_seconds']}s** | {record['engine_seconds']}s | "
                f"{ratio:.2f}x | {record['rows_per_second']:,}/s | {record['peak_rss_mb']:,} MB | "
                f"{record['report_mb']} MB | {record.get('agreement', 'reference')} | {note} |")
    runner = next((r.get("runner") for s in scales for r in results[s].values() if r.get("runner")), "")
    first = next((r for s in scales for r in results[s].values() if r.get("runner")), {})
    ignored = first.get("ignore") or "nothing"
    lines.append(f"\nKey `{first.get('key', KEY)}`, ignoring `{ignored}`. Runner: {runner}.\n")
    return "\n".join(lines)


if __name__ == "__main__":
    raise SystemExit(main())
