#!/usr/bin/env python3
"""Generate a deterministic pair of CSV files for benchmarking and CI.

Both files have 20 columns and share the composite key (account_id, txn_id).
File B is file A with a controlled amount of drift applied:

  status        changes on ~3.0% of rows
  amount        changes on ~1.5% of rows
  balance       changes on ~1.5% of rows
  value_date    blanked  on ~0.3% of rows
  updated_at    changes on every row (so `--ignore updated_at` has something to do)
  removed       ~0.10% of rows are absent from B
  added         ~0.10% extra rows only in B
  duplicate key ~0.01% of rows are emitted twice in B, and a few in A

Usage:
  python scripts/gen_data.py --rows 1000000 --out-dir data
  python scripts/gen_data.py --rows 10000000 --out-dir data --engine duckdb

DuckDB writes 10M x 20 in a few seconds; the pure-Python fallback streams both
files in one pass and needs no dependencies.
"""
from __future__ import annotations

import argparse
import os
import time

COLUMNS = ["account_id", "txn_id", "posting_date", "value_date", "currency", "amount", "fee",
           "balance", "status", "channel", "region", "branch_code", "product_code", "counterparty",
           "quantity", "rate", "category", "risk_flag", "note", "updated_at"]

STATUS = ["posted", "pending", "settled", "reversed"]
CHANNEL = ["branch", "online", "mobile", "atm", "wire"]
REGION = ["EMEA", "NA", "APAC", "LATAM"]
CURRENCY = ["USD", "EUR", "GBP", "JPY"]
CATEGORY = ["retail", "corporate", "treasury", "cards", "loans"]

# Drift buckets, expressed against a 0..9999 hash bucket per row.
CHG_STATUS, CHG_AMOUNT, CHG_BALANCE, CHG_VALUE_DATE = 300, 150, 150, 30   # 3%, 1.5%, 1.5%, 0.3%
REMOVED_MOD, ADDED_RATIO, DUP_MOD = 1000, 1000, 10000


# ---------------------------------------------------------------------------
# DuckDB path
# ---------------------------------------------------------------------------

def _sql_expressions(side: str) -> str:
    """Column expressions for one side. Deterministic pseudo-randomness from hash(i)."""
    def pick(lst, salt):
        arr = ", ".join(f"'{v}'" for v in lst)
        return f"([{arr}])[(hash(i * 31 + {salt}) % {len(lst)}) + 1]"

    b = side == "b"
    status = pick(STATUS, 11)
    status_b = f"([{', '.join(repr(v) for v in STATUS)}])[(((hash(i * 31 + 11) % {len(STATUS)}) + 1) % {len(STATUS)}) + 1]"
    amount = "round(((hash(i * 31 + 21) % 900000000) / 100.0) - 1000000, 2)"
    balance = "round(((hash(i * 31 + 31) % 2000000000) / 100.0), 2)"
    value_date = "strftime(DATE '2026-01-01' + to_days((hash(i * 31 + 41) % 240)::INT), '%Y-%m-%d')"
    return f"""
      'ACC-' || lpad(((i * 7919) % 250000)::VARCHAR, 8, '0')                        AS account_id,
      'TXN-' || lpad(i::VARCHAR, 11, '0')                                           AS txn_id,
      strftime(DATE '2026-01-01' + to_days((hash(i * 31 + 1) % 240)::INT), '%Y-%m-%d') AS posting_date,
      {f"CASE WHEN bucket < {CHG_VALUE_DATE} THEN '' ELSE {value_date} END" if b else value_date} AS value_date,
      {pick(CURRENCY, 51)}                                                          AS currency,
      {f"CASE WHEN bucket >= {CHG_STATUS} AND bucket < {CHG_STATUS + CHG_AMOUNT} THEN round({amount} + 12.34, 2) ELSE {amount} END" if b else amount} AS amount,
      round(((hash(i * 31 + 61) % 5000) / 100.0), 2)                                AS fee,
      {f"CASE WHEN bucket >= {CHG_STATUS + CHG_AMOUNT} AND bucket < {CHG_STATUS + CHG_AMOUNT + CHG_BALANCE} THEN round({balance} * 1.01, 2) ELSE {balance} END" if b else balance} AS balance,
      {f"CASE WHEN bucket < {CHG_STATUS} THEN {status_b} ELSE {status} END" if b else status} AS status,
      {pick(CHANNEL, 71)}                                                           AS channel,
      {pick(REGION, 81)}                                                            AS region,
      'BR' || lpad(((hash(i * 31 + 91) % 900) + 100)::VARCHAR, 4, '0')              AS branch_code,
      'P' || lpad((hash(i * 31 + 101) % 5000)::VARCHAR, 5, '0')                     AS product_code,
      'CP-' || lpad((hash(i * 31 + 111) % 90000)::VARCHAR, 6, '0')                  AS counterparty,
      (hash(i * 31 + 121) % 500) + 1                                                AS quantity,
      round(((hash(i * 31 + 131) % 1200) / 10000.0), 4)                             AS rate,
      {pick(CATEGORY, 141)}                                                         AS category,
      CASE WHEN hash(i * 31 + 151) % 20 = 0 THEN 'Y' ELSE 'N' END                   AS risk_flag,
      'batch ' || ((i % 997) + 1)::VARCHAR || ' line ' || ((i % 53) + 1)::VARCHAR   AS note,
      '{"2026-09-01" if b else "2026-08-01"} 02:15:00'                              AS updated_at
    """


def gen_duckdb(rows: int, a_path: str, b_path: str, seed: int) -> None:
    import duckdb

    lit = lambda p: "'" + p.replace("'", "''") + "'"  # noqa: E731

    con = duckdb.connect()
    con.execute("SET preserve_insertion_order = false")
    base = f"SELECT i, (hash(i * 31 + {seed}) % 10000) AS bucket FROM range(0, {rows}) t(i)"
    dup_extra = max(1, rows // DUP_MOD)

    con.execute(f"COPY (SELECT {_sql_expressions('a')} FROM ({base}) "
                f"UNION ALL SELECT {_sql_expressions('a')} FROM ({base}) WHERE i < {dup_extra} "
                f") TO {lit(a_path)} (HEADER, FORMAT CSV)")

    added = max(1, rows // ADDED_RATIO)
    added_base = f"SELECT i, (hash(i * 31 + {seed}) % 10000) AS bucket FROM range({rows}, {rows + added}) t(i)"
    con.execute(f"COPY (SELECT {_sql_expressions('b')} FROM ({base}) WHERE i % {REMOVED_MOD} <> 7 "
                f"UNION ALL SELECT {_sql_expressions('b')} FROM ({added_base}) "
                f"UNION ALL SELECT {_sql_expressions('b')} FROM ({base}) WHERE i % {DUP_MOD} = 3 AND i < {rows // 2} "
                f") TO {lit(b_path)} (HEADER, FORMAT CSV)")
    con.close()


# ---------------------------------------------------------------------------
# Pure-Python path (single pass, writes both files together)
# ---------------------------------------------------------------------------

def gen_python(rows: int, a_path: str, b_path: str, seed: int) -> None:
    def h(i: int, salt: int) -> int:                       # splitmix-style, deterministic
        x = (i * 31 + salt + seed) & 0xFFFFFFFFFFFFFFFF
        x = ((x ^ (x >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
        x = ((x ^ (x >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
        return x ^ (x >> 31)

    from datetime import date, timedelta
    d0 = date(2026, 1, 1)
    day = [(d0 + timedelta(days=n)).isoformat() for n in range(240)]

    def row(i: int, b: bool) -> list[str]:
        bucket = h(i, 0) % 10000
        amount = round(((h(i, 21) % 900000000) / 100.0) - 1000000, 2)
        balance = round((h(i, 31) % 2000000000) / 100.0, 2)
        status = STATUS[h(i, 11) % len(STATUS)]
        value_date = day[h(i, 41) % 240]
        if b:
            if bucket < CHG_STATUS:
                status = STATUS[(h(i, 11) + 1) % len(STATUS)]
            elif bucket < CHG_STATUS + CHG_AMOUNT:
                amount = round(amount + 12.34, 2)
            elif bucket < CHG_STATUS + CHG_AMOUNT + CHG_BALANCE:
                balance = round(balance * 1.01, 2)
            if bucket < CHG_VALUE_DATE:
                value_date = ""
        return [f"ACC-{(i * 7919) % 250000:08d}", f"TXN-{i:011d}", day[h(i, 1) % 240], value_date,
                CURRENCY[h(i, 51) % 4], f"{amount:.2f}", f"{(h(i, 61) % 5000) / 100.0:.2f}",
                f"{balance:.2f}", status, CHANNEL[h(i, 71) % 5], REGION[h(i, 81) % 4],
                f"BR{(h(i, 91) % 900) + 100:04d}", f"P{h(i, 101) % 5000:05d}",
                f"CP-{h(i, 111) % 90000:06d}", str((h(i, 121) % 500) + 1),
                f"{(h(i, 131) % 1200) / 10000.0:.4f}", CATEGORY[h(i, 141) % 5],
                "Y" if h(i, 151) % 20 == 0 else "N", f"batch {i % 997 + 1} line {i % 53 + 1}",
                "2026-09-01 02:15:00" if b else "2026-08-01 02:15:00"]

    header = ",".join(COLUMNS) + "\n"
    dup_extra, added = max(1, rows // DUP_MOD), max(1, rows // ADDED_RATIO)
    with open(a_path, "w", newline="", buffering=1 << 20) as fa, \
         open(b_path, "w", newline="", buffering=1 << 20) as fb:
        fa.write(header), fb.write(header)
        ba, bb = [], []
        for i in range(rows):
            ba.append(",".join(row(i, False)))
            if i % REMOVED_MOD != 7:
                bb.append(",".join(row(i, True)))
            if i % DUP_MOD == 3 and i < rows // 2:
                bb.append(",".join(row(i, True)))
            if len(ba) >= 20000:
                fa.write("\n".join(ba) + "\n"), fb.write("\n".join(bb) + "\n")
                ba, bb = [], []
        for i in range(dup_extra):
            ba.append(",".join(row(i, False)))
        for i in range(rows, rows + added):
            bb.append(",".join(row(i, True)))
        fa.write("\n".join(ba) + "\n"), fb.write("\n".join(bb) + "\n")


def parse_rows(s: str) -> int:
    s = s.strip().lower().replace("_", "").replace(",", "")
    mult = {"k": 1_000, "m": 1_000_000, "g": 1_000_000_000}
    return int(float(s[:-1]) * mult[s[-1]]) if s and s[-1] in mult else int(s)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rows", "-n", default="10k", help="Rows in file A: 10000, 10k, 1m, 10m")
    ap.add_argument("--out-dir", "-o", default="data")
    ap.add_argument("--prefix", default=None, help="File name prefix (default: rows label)")
    ap.add_argument("--engine", choices=["auto", "duckdb", "python"], default="auto")
    ap.add_argument("--seed", type=int, default=7)
    ns = ap.parse_args()

    rows = parse_rows(ns.rows)
    os.makedirs(ns.out_dir, exist_ok=True)
    prefix = ns.prefix or ns.rows.lower()
    a = os.path.join(ns.out_dir, f"{prefix}_a.csv")
    b = os.path.join(ns.out_dir, f"{prefix}_b.csv")

    engine = ns.engine
    if engine == "auto":
        try:
            import duckdb  # noqa: F401
            engine = "duckdb"
        except ImportError:
            engine = "python"

    t0 = time.perf_counter()
    (gen_duckdb if engine == "duckdb" else gen_python)(rows, a, b, ns.seed)
    dt = time.perf_counter() - t0
    print(f"{engine}: {rows:,} rows x {len(COLUMNS)} columns in {dt:.1f}s")
    for p in (a, b):
        print(f"  {p}  {os.path.getsize(p) / 1e6:.1f} MB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
