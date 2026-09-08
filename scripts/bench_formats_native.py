#!/usr/bin/env python3
"""One comparison, four input formats, nothing but this project involved.

Everything here is native. The generator writes CSV, newline-delimited JSON and
Parquet from one field-by-field recipe, so the four files hold the same rows and
were not converted from one another; the engine reads all four without a library
between it and the bytes. That is what makes this a measurement of the formats
rather than of whichever reader happened to be installed.

Each format is generated, measured and deleted in turn. At ten million rows all
four at once would be about 15 GB, which is more than a CI runner has.

The counts are the correctness gate: four formats that disagree about how many
rows changed mean a bug in a reader, not an interesting benchmark, so the run
fails and says which format disagreed.

    python scripts/bench_formats_native.py --rows 10m --data /tmp/bench
"""
import argparse
import json
import os
import subprocess
import sys
import time

KEY = ["account_id", "txn_id"]
IGNORE = "updated_at"

# label, generator flags, and the suffix the generator gives each side.
FORMATS = [
    ("CSV", [], ".csv"),
    ("JSON (ndjson)", ["--format", "json"], ".ndjson"),
    ("Parquet + snappy", ["--format", "parquet", "--compression", "snappy"], ".parquet"),
    ("Parquet uncompressed", ["--format", "parquet", "--compression", "none"], ".unc.parquet"),
]


def warm(*paths):
    """Reads the inputs once so the first timed run is not also a disk test."""
    for p in paths:
        with open(p, "rb") as fh:
            while fh.read(1 << 22):
                pass


def run(cmd, out_path, err_path):
    """Wall, peak RSS and CPU for exactly this child, from wait4's rusage.

    The kernel's own high-water mark, rather than a poll that can miss a spike.
    """
    started = time.monotonic()
    pid = os.fork()
    if pid == 0:
        o = os.open(out_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
        e = os.open(err_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
        os.dup2(o, 1)
        os.dup2(e, 2)
        os.execv(cmd[0], cmd)
        os._exit(127)
    _, status, usage = os.wait4(pid, 0)
    return (time.monotonic() - started, usage.ru_maxrss / 1024,
            usage.ru_utime + usage.ru_stime, os.waitstatus_to_exitcode(status))


def measure(args, label, flags, ext):
    a = f"{args.data}/{args.prefix}_a{ext}"
    b = f"{args.data}/{args.prefix}_b{ext}"
    for p in (a, b):
        if os.path.exists(p):
            os.remove(p)

    started = time.monotonic()
    made = subprocess.run([args.gen, "--rows", args.rows, "--out-dir", args.data,
                           "--prefix", args.prefix, *flags],
                          capture_output=True, text=True)
    if made.returncode != 0:
        raise SystemExit(f"{label}: generating failed: {(made.stderr or made.stdout).strip()[:200]}")
    generate = time.monotonic() - started
    size = (os.path.getsize(a) + os.path.getsize(b)) / 2**20

    report = f"{args.data}/report.json"
    if os.path.exists(report):
        os.remove(report)
    warm(a, b)
    secs, rss, cpu, code = run([args.binary, "compare", a, b, "-k", ",".join(KEY),
                                "-i", IGNORE, "--json", report],
                               f"{args.data}/out.txt", f"{args.data}/err.txt")
    if code not in (0, 1):
        raise SystemExit(f"{label}: comparing failed: "
                         f"{open(f'{args.data}/err.txt').read().strip()[:300]}")
    counts = json.load(open(report))["counts"]
    for p in (a, b):
        os.remove(p)

    rows = counts["a_rows"]
    return {
        "format": label,
        "size_mb": round(size, 1),
        "generate_seconds": round(generate, 2),
        "compare_seconds": round(secs, 2),
        "cpu_seconds": round(cpu, 1),
        # CPU over wall: how many cores were busy. The column that separates an
        # engine that is slow from one that is idle.
        "cores_busy": round(cpu / secs, 2) if secs else 0.0,
        "peak_rss_mb": round(rss),
        "rows_per_second": int(rows / secs) if secs > 0 else 0,
        "counts": counts,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rows", default="100k")
    ap.add_argument("--data", default="data")
    ap.add_argument("--prefix", default="fmt")
    ap.add_argument("--binary", default="cpp/build/csvdiff")
    ap.add_argument("--gen", default="cpp/build/gen-data")
    ap.add_argument("--out", help="write the results here as JSON")
    ap.add_argument("--only", help="comma-separated format labels to run")
    args = ap.parse_args()

    for p in (args.binary, args.gen):
        if not os.path.exists(p):
            sys.exit(f"missing {p}; build it with: (cd cpp && make && make gen-data)")
    os.makedirs(args.data, exist_ok=True)

    wanted = [f.strip().lower() for f in args.only.split(",")] if args.only else None
    results = []
    for label, flags, ext in FORMATS:
        if wanted and label.lower() not in wanted:
            continue
        results.append(measure(args, label, flags, ext))

    # Four formats that disagree about the answer are a bug, not a benchmark.
    baseline = results[0]["counts"] if results else None
    disagreed = [r["format"] for r in results if r["counts"] != baseline]

    print(f"\n{args.rows} rows x 20 columns, both files, keyed on "
          f"({', '.join(KEY)}), ignoring {IGNORE}\n")
    print(f"| Input | Size | Generate | **Compare** | Rows/s | CPU | Cores | Peak RSS |")
    print(f"|---|---:|---:|---:|---:|---:|---:|---:|")
    for r in results:
        print(f"| {r['format']} | {r['size_mb']:,.0f} MB | {r['generate_seconds']}s "
              f"| **{r['compare_seconds']}s** | {r['rows_per_second']:,}/s "
              f"| {r['cpu_seconds']}s | {r['cores_busy']}x | {r['peak_rss_mb']:,} MB |")
    if baseline:
        print(f"\nAll {len(results)} formats agree: matched {baseline['matched']:,}, "
              f"changed {baseline['changed']:,}, added {baseline['added']:,}, "
              f"removed {baseline['removed']:,}, duplicate rows "
              f"{baseline['a_dup_rows']:,} in A and {baseline['b_dup_rows']:,} in B."
              if not disagreed else
              f"\n**{', '.join(disagreed)} disagreed with {results[0]['format']}.**")

    if args.out:
        with open(args.out, "w") as fh:
            json.dump({"rows": args.rows, "cpus": os.cpu_count(), "results": results}, fh, indent=2)

    if disagreed:
        sys.exit(f"formats disagreed: {', '.join(disagreed)}")


main()
