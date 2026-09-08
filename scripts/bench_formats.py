#!/usr/bin/env python3
"""The same comparison, from five input formats, with the engine held constant.

The earlier format table measured *reading* one file. This measures a whole
comparison -- key join, per-cell diff, per-column counts -- changing only the
input format. DuckDB is the constant because it reads all five natively and
spills to disk rather than dying: polars was tried first and was killed by the
OOM killer at ten million rows on CSV alone, twice, which is a result in itself
but not one that produces a table.

Duplicate keys are reduced to their first occurrence before the join, matching
what this project does, so the counts are comparable rather than inflated.

Our own C++ engine on CSV is measured in the same sitting, so the two axes come
apart: DuckDB across formats says what the format is worth, and DuckDB-on-CSV
against ours-on-CSV says what the engine is worth.

Each format is converted, compared, then deleted -- all five at ten million rows
would be about 18 GB.
"""
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
D = os.path.join(ROOT, "bench/external/data")
A_CSV, B_CSV = f"{D}/10m_a.csv", f"{D}/10m_b.csv"
DUCK = os.path.join(ROOT, "bench/external/tools/duckdb")
CPP = os.path.join(ROOT, "cpp/build/csvdiff")
KEY = ["account_id", "txn_id"]
IGNORE = "updated_at"
HEADER = ["account_id", "txn_id", "posting_date", "value_date", "currency", "amount", "fee",
          "balance", "status", "channel", "region", "branch_code", "product_code",
          "counterparty", "quantity", "rate", "category", "risk_flag", "note", "updated_at"]
COMPARED = [c for c in HEADER if c not in KEY and c != IGNORE]


def warm(*paths):
    for p in paths:
        with open(p, "rb") as fh:
            while fh.read(1 << 22):
                pass


def reader(path):
    """How DuckDB is told to read this file, all columns as text.

    Reading everything as VARCHAR is what the survey does elsewhere: a
    comparison that reformats a number before comparing it is answering a
    different question.
    """
    if path.endswith(".csv"):
        return (f"read_csv('{path}', all_varchar = true, header = true, "
                f"sample_size = -1, null_padding = true)")
    if path.endswith(".ndjson"):
        return f"read_json('{path}', format = 'newline_delimited', records = true)"
    return f"read_parquet('{path}')"


def sql(a_path, b_path):
    on = " AND ".join(f"a.{k} = b.{k}" for k in KEY)
    changed = " OR ".join(f"a.{c} IS DISTINCT FROM b.{c}" for c in COMPARED)
    per_col = ",\n  ".join(
        f"count(*) FILTER (WHERE a.{c} IS DISTINCT FROM b.{c}) AS d_{c}" for c in COMPARED)
    # first(...) over a group by the key is first-occurrence-wins, the same rule
    # this project joins on.
    def firsts(alias, path):
        cols = ",\n    ".join(f"first({c}) AS {c}" for c in HEADER)
        return (f"{alias} AS (SELECT {', '.join(KEY)},\n    {cols}\n"
                f"  FROM {reader(path)} GROUP BY {', '.join(KEY)})")
    return f"""
WITH {firsts('ua', a_path)},
     {firsts('ub', b_path)},
     j AS (SELECT * FROM ua a INNER JOIN ub b ON {on})
SELECT
  (SELECT count(*) FROM j) AS matched,
  (SELECT count(*) FROM ua) AS a_keys,
  (SELECT count(*) FROM ub) AS b_keys,
  (SELECT count(*) FROM ua a INNER JOIN ub b ON {on} WHERE {changed}) AS changed;
"""


def run(cmd):
    started = time.monotonic()
    pid = os.fork()
    if pid == 0:
        out = os.open("/tmp/duck_out.txt", os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
        err = os.open("/tmp/duck_err.txt", os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
        os.dup2(out, 1)
        os.dup2(err, 2)
        os.execv(cmd[0], cmd)
        os._exit(127)
    _, status, usage = os.wait4(pid, 0)
    # CPU as well as RSS, from the one rusage: wall time says how long you
    # waited, CPU over wall says how many cores were busy while you did.
    return (time.monotonic() - started, usage.ru_maxrss / 1024,
            usage.ru_utime + usage.ru_stime, os.waitstatus_to_exitcode(status))


def convert(fmt):
    ext = {"parquet-zstd": "parquet", "parquet-none": "unc.parquet", "ndjson": "ndjson"}[fmt]
    started = time.monotonic()
    out = []
    for side, src in (("a", A_CSV), ("b", B_CSV)):
        path = f"{D}/fmt_{side}.{ext}"
        if fmt == "ndjson":
            stmt = (f"COPY (SELECT * FROM {reader(src)}) TO '{path}' "
                    f"(FORMAT JSON);")
        else:
            comp = "zstd" if fmt == "parquet-zstd" else "uncompressed"
            stmt = (f"COPY (SELECT * FROM {reader(src)}) TO '{path}' "
                    f"(FORMAT PARQUET, COMPRESSION {comp});")
        done = subprocess.run([DUCK, "-c", stmt], capture_output=True, text=True)
        if done.returncode != 0:
            raise RuntimeError(done.stderr.strip()[:200])
        out.append(path)
    return out, time.monotonic() - started


def measure(label, a_path, b_path, convert_secs=None):
    warm(a_path, b_path)
    size = (os.path.getsize(a_path) + os.path.getsize(b_path)) / 2**20
    secs, rss, cpu, code = run([DUCK, "-csv", "-c", sql(a_path, b_path)])
    conv = f"{convert_secs:7.1f}s" if convert_secs is not None else "      —"
    if code != 0:
        why = open("/tmp/duck_err.txt").read().strip().splitlines()
        print(f"{label:26} {size:8,.0f}M {conv} {'failed':>10}   "
              f"{(why[0] if why else 'exit ' + str(code))[:70]}")
        return
    rows = [r for r in open("/tmp/duck_out.txt").read().strip().splitlines() if r]
    vals = rows[-1].split(",") if len(rows) > 1 else []
    if len(vals) == 4:
        matched, a_keys, b_keys, changed = (int(v) for v in vals)
        counts = (f"matched {matched:,} changed {changed:,} "
                  f"added {b_keys - matched:,} removed {a_keys - matched:,}")
    else:
        counts = "(no counts parsed)"
    print(f"{label:26} {size:8,.0f}M {conv} {secs:9.2f}s {cpu:7.1f}s "
          f"{cpu / secs if secs else 0:5.2f}x {rss:9,.0f}M   {counts}", flush=True)


def main():
    only = sys.argv[1] if len(sys.argv) > 1 else None
    print("10M rows, both files. Same comparison: first-occurrence-wins on the key,\n"
          "inner join, per-cell diff over 17 compared columns.\n")
    print(f"{'engine and input':26} {'size':>9} {'convert':>8} {'compare':>10} "
          f"{'cpu':>7} {'cores':>6} {'peak RSS':>10}   counts")

    if only in (None, "csv"):
        measure("DuckDB, CSV", A_CSV, B_CSV)
    for fmt in ("parquet-zstd", "parquet-none", "ndjson"):
        if only not in (None, fmt):
            continue
        try:
            (a, b), conv = convert(fmt)
        except Exception as exc:
            print(f"{'DuckDB, ' + fmt:26} conversion failed: {exc}")
            continue
        try:
            measure(f"DuckDB, {fmt}", a, b, conv)
        finally:
            for p in (a, b):
                try:
                    os.remove(p)
                except OSError:
                    pass

    if only in (None, "cpp"):
        out = "/tmp/fmt_cpp.json"
        if os.path.exists(out):
            os.remove(out)
        secs, rss, cpu, _ = run([CPP, "compare", A_CSV, B_CSV, "-k", ",".join(KEY),
                                 "-i", IGNORE, "--threads", "4", "--json", out])
        c = json.load(open(out))["counts"]
        size = (os.path.getsize(A_CSV) + os.path.getsize(B_CSV)) / 2**20
        print(f"{'ours (C++), CSV':26} {size:8,.0f}M       — {secs:9.2f}s {cpu:7.1f}s "
              f"{cpu / secs if secs else 0:5.2f}x {rss:9,.0f}M   "
              f"matched {c['matched']:,} changed {c['changed']:,} "
              f"added {c['added']:,} removed {c['removed']:,}")


main()
