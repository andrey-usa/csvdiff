#!/usr/bin/env python3
"""The same comparison from CSV and from Parquet, ours against DuckDB and polars.

Two axes come apart here. Holding the engine constant and changing the format
says what the format is worth; holding the format constant and changing the
engine says what the engine is worth. So both engines are run on both formats
in one sitting, on one machine, and every row returns the same counts -- which
is what makes them six ways of doing exactly the same work rather than six
different jobs.

DuckDB is the first comparison because it reads Parquet natively and is the
fastest general-purpose tool measured anywhere in this project. Its query is
written to match what this tool does: duplicate keys reduced to their first
occurrence before the join, every column read as text, `IS DISTINCT FROM` per
cell.

polars is the second, and it needs a word. Written the way anyone would write it
-- join the two frames, compare every compared cell, sum -- it runs out of memory
at ten million rows and cannot be made to finish, on either codec, with threads
turned down, under a cap just below the whole machine. What it *can* finish is
the columnar shape: dedup, join and diff one column at a time, projecting only
the keys and that column. That is the same strategy this tool's Parquet path
uses, so it is the fair thing to measure -- but it is a rewrite, not a flag, and
it re-reads each file once per column.

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


def polars_columnwise(a_path, b_path, cap_gb):
    """Runs in the child. Prints the counts as JSON.

    Every step is projected to the keys plus one column and collected on the
    streaming engine, because the whole frame does not fit. `ne_missing` is
    polars' IS DISTINCT FROM: two nulls are equal, one null is not.
    """
    import json as _json
    import resource
    resource.setrlimit(resource.RLIMIT_AS, (int(cap_gb * 2**30),) * 2)
    import polars as pl

    scan = ((lambda p: pl.scan_csv(p, infer_schema_length=0)) if a_path.endswith(".csv")
            else (lambda p: pl.scan_parquet(p)))
    cols = [c for c in scan(a_path).collect_schema().names() if c not in KEY and c != IGNORE]
    keys_of = lambda p: scan(p).select(KEY).unique(subset=KEY, keep="first")
    side = lambda p, keep: scan(p).select(KEY + [keep]).unique(subset=KEY, keep="first")
    count = lambda lf: lf.select(pl.len()).collect(engine="streaming").item()

    a_keys, b_keys = count(keys_of(a_path)), count(keys_of(b_path))
    matched = count(keys_of(a_path).join(keys_of(b_path), on=KEY, how="inner"))

    changed_keys = []
    for c in cols:
        joined = side(a_path, c).join(side(b_path, c), on=KEY, how="inner", suffix="__b")
        hits = (joined.filter(pl.col(c).ne_missing(pl.col(f"{c}__b")))
                .select(KEY).collect(engine="streaming"))
        if hits.height:
            changed_keys.append(hits)
    changed = pl.concat(changed_keys).unique().height if changed_keys else 0

    print(_json.dumps({"matched": matched, "changed": changed,
                       "added": b_keys - matched, "removed": a_keys - matched}))


def polars_row(a, b, label, cap_gb, convert_secs=None):
    warm(a, b)
    secs, rss, cpu, code = run([sys.executable, os.path.abspath(__file__),
                                "--polars-child", a, b, str(cap_gb)])
    if code != 0:
        why = [l for l in open("/tmp/bench_pq_err.txt").read().strip().splitlines() if l]
        # polars runs out of memory in several voices: a Rust allocation
        # failure, a Python MemoryError, or errno 12 out of the allocator.
        marks = ("memory allocation", "Cannot allocate memory", "MemoryError", "os error 12")
        oom = any(m in l for l in why for m in marks)
        note = (f"out of memory (capped at {cap_gb:g} GB)" if oom
                else (why[-1][:90] if why else f"exit {code}"))
        size = (os.path.getsize(a) + os.path.getsize(b)) / 2**20
        conv = f"{convert_secs:7.1f}s" if convert_secs is not None else "      —"
        print(f"{label:34} {size:8,.0f}M {conv} {'failed':>10} {cpu:8.1f}s {rss:9,.0f}M   {note}",
              flush=True)
        return
    c = json.loads(open("/tmp/bench_pq_out.txt").read().strip().splitlines()[-1])
    row(label, a, b, secs, rss, cpu,
        f"matched {c['matched']:,} changed {c['changed']:,} "
        f"added {c['added']:,} removed {c['removed']:,}", convert_secs)


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
    if len(sys.argv) > 1 and sys.argv[1] == "--polars-child":
        polars_columnwise(sys.argv[2], sys.argv[3], float(sys.argv[4]))
        return

    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="bench/external/data")
    ap.add_argument("--prefix", default="10m")
    ap.add_argument("--binary", default="cpp/build/csvdiff")
    ap.add_argument("--duckdb", default="bench/external/tools/duckdb")
    ap.add_argument("--keep", action="store_true", help="leave the Parquet files behind")
    ap.add_argument("--polars", action="store_true",
                    help="also run polars, column at a time (slow: ~2 minutes a row at 10M)")
    ap.add_argument("--cap-gb", type=float, default=14.0,
                    help="address-space cap for polars, so an out-of-memory is reportable")
    args = ap.parse_args()

    a_csv = f"{args.data}/{args.prefix}_a.csv"
    b_csv = f"{args.data}/{args.prefix}_b.csv"
    for p in (a_csv, b_csv, args.binary):
        if not os.path.exists(p):
            sys.exit(f"missing {p}")
    duck = args.duckdb if os.path.exists(args.duckdb) else None

    print("Both files, the same comparison: first-occurrence-wins on the key, inner join,\n"
          "per-cell diff over 17 compared columns. Convert is what it cost to write the\n"
          "Parquet from the CSV, and is charged once however many comparisons follow.\n"
          "polars is run one column at a time, because the way anyone would write it does\n"
          "not finish -- see the note at the top of this file.\n")
    print(f"{'engine and input':34} {'size':>9} {'convert':>8} {'compare':>10} "
          f"{'cpu':>9} {'peak RSS':>10}   counts")

    have_polars = False
    if args.polars:
        try:
            import polars as _probe  # noqa: F401
            have_polars = True
        except ImportError:
            print("(polars asked for but not installed; its rows are skipped)\n")

    ours(args.binary, a_csv, b_csv, "ours (C++), CSV")
    if duck:
        theirs(duck, a_csv, b_csv, "DuckDB, CSV")
    if have_polars:
        polars_row(a_csv, b_csv, "polars, CSV", args.cap_gb)

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
        if have_polars:
            polars_row(a_pq, b_pq, f"polars, Parquet {codec}", args.cap_gb, conv)

    if not args.keep:
        for p in made:
            try:
                os.remove(p)
            except OSError:
                pass


main()
