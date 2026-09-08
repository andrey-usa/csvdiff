#!/usr/bin/env python3
"""The same comparison from CSV and from Parquet, ours against DuckDB.

Two axes come apart here. Holding the engine constant and changing the format
says what the format is worth; holding the format constant and changing the
engine says what the engine is worth. So both engines are run on both formats
in one sitting, on one machine, and every row returns the same counts -- which
is what makes them six ways of doing exactly the same work rather than six
different jobs.

DuckDB is the comparison because it reads Parquet natively and is the fastest
general-purpose tool measured anywhere in this project. Its query is written to
match what this tool does: duplicate keys reduced to their first occurrence
before the join, every column read as text, `IS DISTINCT FROM` per cell.

Parquet is written from the CSV pair with DuckDB, in both snappy and
uncompressed form, and the conversion is timed too -- it is a real cost, and on
a one-off comparison it is larger than everything it saves.

    python scripts/bench_parquet.py --data bench/external/data --prefix 10m

Pass --keep to leave the Parquet files behind for a second run; by default they
are deleted, because at ten million rows the two forms are another 3 GB.
"""
import argparse
import json
import os
import subprocess
import sys
import time

HEADER = ["account_id", "txn_id", "posting_date", "value_date", "currency", "amount", "fee",
          "balance", "status", "channel", "region", "branch_code", "product_code",
          "counterparty", "quantity", "rate", "category", "risk_flag", "note", "updated_at"]
KEY = ["account_id", "txn_id"]
IGNORE = "updated_at"
COMPARED = [c for c in HEADER if c not in KEY and c != IGNORE]


def warm(*paths):
    """Reads the inputs once so the first timed run is not also a disk test."""
    for p in paths:
        with open(p, "rb") as fh:
            while fh.read(1 << 22):
                pass


def run(cmd):
    """Wall time, peak RSS and CPU for exactly this child, from wait4's rusage.

    The kernel's own high-water mark rather than a poll that can miss a spike.
    """
    started = time.monotonic()
    pid = os.fork()
    if pid == 0:
        out = os.open("/tmp/bench_pq_out.txt", os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
        err = os.open("/tmp/bench_pq_err.txt", os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
        os.dup2(out, 1)
        os.dup2(err, 2)
        os.execv(cmd[0], cmd)
        os._exit(127)
    _, status, usage = os.wait4(pid, 0)
    return (time.monotonic() - started, usage.ru_maxrss / 1024,
            usage.ru_utime + usage.ru_stime, os.waitstatus_to_exitcode(status))


def reader(path):
    if path.endswith(".csv"):
        return (f"read_csv('{path}', all_varchar = true, header = true, "
                f"sample_size = -1, null_padding = true)")
    return f"read_parquet('{path}')"


def sql(a_path, b_path):
    """The same rule set the tool applies, said in SQL.

    `first(...)` over a group by the key is first-occurrence-wins, which is what
    stops a join multiplying duplicate keys and is what this project does.
    """
    on = " AND ".join(f"a.{k} = b.{k}" for k in KEY)
    changed = " OR ".join(f"a.{c} IS DISTINCT FROM b.{c}" for c in COMPARED)

    def firsts(alias, path):
        cols = ", ".join(f"first({c}) AS {c}" for c in HEADER if c not in KEY)
        return (f"{alias} AS (SELECT {', '.join(KEY)}, {cols} "
                f"FROM {reader(path)} GROUP BY {', '.join(KEY)})")

    return f"""
WITH {firsts('ua', a_path)},
     {firsts('ub', b_path)}
SELECT (SELECT count(*) FROM ua a JOIN ub b ON {on}) AS n_matched,
       (SELECT count(*) FROM ua) AS n_a,
       (SELECT count(*) FROM ub) AS n_b,
       (SELECT count(*) FROM ua a JOIN ub b ON {on} WHERE {changed}) AS n_changed;
"""


def convert(duck, a_csv, b_csv, codec, dest_a, dest_b):
    started = time.monotonic()
    for src, dest in ((a_csv, dest_a), (b_csv, dest_b)):
        stmt = (f"SET preserve_insertion_order = false; "
                f"COPY (SELECT * FROM {reader(src)}) TO '{dest}' "
                f"(FORMAT PARQUET, COMPRESSION {codec});")
        done = subprocess.run([duck, "-c", stmt], capture_output=True, text=True)
        if done.returncode != 0:
            raise RuntimeError(done.stderr.strip()[:200])
    return time.monotonic() - started


def row(label, a, b, secs, rss, cpu, counts, convert_secs=None):
    size = (os.path.getsize(a) + os.path.getsize(b)) / 2**20
    conv = f"{convert_secs:7.1f}s" if convert_secs is not None else "      —"
    print(f"{label:34} {size:8,.0f}M {conv} {secs:9.2f}s {cpu:8.1f}s {rss:9,.0f}M   {counts}",
          flush=True)


def ours(binary, a, b, label, convert_secs=None):
    out = "/tmp/bench_pq.json"
    if os.path.exists(out):
        os.remove(out)
    warm(a, b)
    secs, rss, cpu, code = run([binary, "compare", a, b, "-k", ",".join(KEY),
                                "-i", IGNORE, "--json", out])
    if code not in (0, 1):
        print(f"{label:34} failed: {open('/tmp/bench_pq_err.txt').read().strip()[:120]}")
        return
    c = json.load(open(out))["counts"]
    row(label, a, b, secs, rss, cpu,
        f"matched {c['matched']:,} changed {c['changed']:,} "
        f"added {c['added']:,} removed {c['removed']:,}", convert_secs)


def theirs(duck, a, b, label, convert_secs=None):
    with open("/tmp/bench_pq.sql", "w") as fh:
        fh.write(sql(a, b))
    warm(a, b)
    secs, rss, cpu, code = run([duck, "-csv", "-c", ".read /tmp/bench_pq.sql"])
    if code != 0:
        print(f"{label:34} failed: {open('/tmp/bench_pq_err.txt').read().strip()[:120]}")
        return
    lines = [l for l in open("/tmp/bench_pq_out.txt").read().strip().splitlines() if l]
    vals = lines[-1].split(",") if len(lines) > 1 else []
    if len(vals) == 4:
        matched, a_keys, b_keys, changed = (int(v) for v in vals)
        counts = (f"matched {matched:,} changed {changed:,} "
                  f"added {b_keys - matched:,} removed {a_keys - matched:,}")
    else:
        counts = "(no counts parsed)"
    row(label, a, b, secs, rss, cpu, counts, convert_secs)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="bench/external/data")
    ap.add_argument("--prefix", default="10m")
    ap.add_argument("--binary", default="cpp/build/csvdiff")
    ap.add_argument("--duckdb", default="bench/external/tools/duckdb")
    ap.add_argument("--keep", action="store_true", help="leave the Parquet files behind")
    args = ap.parse_args()

    a_csv = f"{args.data}/{args.prefix}_a.csv"
    b_csv = f"{args.data}/{args.prefix}_b.csv"
    for p in (a_csv, b_csv, args.binary):
        if not os.path.exists(p):
            sys.exit(f"missing {p}")
    duck = args.duckdb if os.path.exists(args.duckdb) else None

    print("Both files, the same comparison: first-occurrence-wins on the key, inner join,\n"
          "per-cell diff over 17 compared columns. Convert is what it cost to write the\n"
          "Parquet from the CSV, and is charged once however many comparisons follow.\n")
    print(f"{'engine and input':34} {'size':>9} {'convert':>8} {'compare':>10} "
          f"{'cpu':>9} {'peak RSS':>10}   counts")

    ours(args.binary, a_csv, b_csv, "ours (C++), CSV")
    if duck:
        theirs(duck, a_csv, b_csv, "DuckDB, CSV")

    if not duck:
        print("\n(no duckdb to write Parquet with; the Parquet rows are skipped)")
        return

    made = []
    for codec, ext in (("snappy", "parquet"), ("uncompressed", "unc.parquet")):
        a_pq = f"{args.data}/{args.prefix}_a.{ext}"
        b_pq = f"{args.data}/{args.prefix}_b.{ext}"
        try:
            conv = convert(duck, a_csv, b_csv, codec, a_pq, b_pq)
        except Exception as exc:
            print(f"{'Parquet ' + codec:30} conversion failed: {exc}")
            continue
        made += [a_pq, b_pq]
        ours(args.binary, a_pq, b_pq, f"ours (C++), Parquet {codec}", conv)
        theirs(duck, a_pq, b_pq, f"DuckDB, Parquet {codec}", conv)

    if not args.keep:
        for p in made:
            try:
                os.remove(p)
            except OSError:
                pass


main()
