"""Composite-key CSV comparison.

Engine: DuckDB. It streams both files from disk, infers delimiters, hash-joins
on the composite key and spills to disk when data exceeds RAM, so multi-GB files
are fine. It is a required dependency rather than one of several: the byte-level
ports in cpp/, rust/, zig/, java/ and go/ are where the alternatives live, and
they are held to this same contract.

Result contract (JSON-serialisable dict) — consumed by report.py and by any
other sink (JSON sidecar, email, CI gate):

{
  "meta":    {a, b, key, compared, only_in_a, only_in_b, options, engine, seconds, generated}
  "counts":  {a_rows, b_rows, a_keys, b_keys, matched, unchanged, changed, added, removed,
              a_dup_keys, a_dup_rows, b_dup_keys, b_dup_rows}
  "columns": [{name, changed, blanked, filled}]           # per compared column
  "changed": {"cols": key, "rows": [[k1..kn, [[colIdx, old, new], ...]], ...], "truncated": bool}
  "added":   {"cols": key + compared, "rows": [[...]], "truncated": bool}
  "removed": same as added
  "dup_a":   {"cols": key + ["count"], "rows": [[...]], "truncated": bool}
  "dup_b":   same
}
"""
from __future__ import annotations

import os
import time
from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from typing import Any


@dataclass
class Options:
    key: list[str]
    compare: list[str] | None = None      # None -> every common non-key column
    ignore: list[str] = field(default_factory=list)
    trim: bool = False                    # strip surrounding whitespace before comparing
    ignore_case: bool = False
    empty_is_null: bool = False           # treat '' and NULL as equal
    tolerance: float = 0.0                # numeric tolerance (abs) when both sides parse as numbers
    max_rows: int = 50_000                # rows embedded per section in the report
    delimiter: str | None = None          # None -> auto-detect
    encoding: str = "utf-8"
    engine: str = "auto"                  # auto | duckdb
    threads: int | None = None
    memory_limit: str | None = None       # e.g. "4GB" (DuckDB only)
    export_dir: str | None = None         # write full, uncapped changed/added/removed CSVs here


class CompareError(ValueError):
    pass


def compare(a_path: str, b_path: str, opt: Options) -> dict[str, Any]:
    if not opt.key:
        raise CompareError("At least one key column is required.")
    for p in (a_path, b_path):
        if not os.path.isfile(p):
            raise CompareError(f"File not found: {p}")

    t0 = time.perf_counter()
    engine = "duckdb" if opt.engine == "auto" else opt.engine
    if engine != "duckdb":
        raise CompareError(f"Unknown engine: {engine}")
    try:
        import duckdb  # noqa: F401
    except ImportError as exc:
        raise CompareError(
            "DuckDB is required: pip install duckdb. It is the only engine this "
            "implementation carries; the byte-level ports are in cpp/, rust/, zig/, "
            "java/ and go/."
        ) from exc
    result = _compare_duckdb(a_path, b_path, opt)

    result["meta"].update({
        "a": {"name": os.path.basename(a_path), "path": os.path.abspath(a_path),
              "size": os.path.getsize(a_path)},
        "b": {"name": os.path.basename(b_path), "path": os.path.abspath(b_path),
              "size": os.path.getsize(b_path)},
        "engine": engine,
        "seconds": round(time.perf_counter() - t0, 3),
        "generated": datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds"),
        "options": {k: v for k, v in asdict(opt).items() if k not in ("key", "compare", "ignore")},
    })
    return result


def is_identical(result: dict[str, Any]) -> bool:
    c = result["counts"]
    return c["changed"] == 0 and c["added"] == 0 and c["removed"] == 0


# ----------------------------------------------------------------------------
# Column resolution (shared)
# ----------------------------------------------------------------------------

def _resolve_columns(a_cols: list[str], b_cols: list[str], opt: Options):
    missing = [k for k in opt.key if k not in a_cols or k not in b_cols]
    if missing:
        raise CompareError(f"Key column(s) missing from one of the files: {', '.join(missing)}")
    common = [c for c in a_cols if c in b_cols]
    if opt.compare is None:
        compared = [c for c in common if c not in opt.key]
    else:
        bad = [c for c in opt.compare if c not in common]
        if bad:
            raise CompareError(f"Compare column(s) not present in both files: {', '.join(bad)}")
        compared = [c for c in opt.compare if c not in opt.key]
    compared = [c for c in compared if c not in set(opt.ignore)]
    only_a = [c for c in a_cols if c not in b_cols]
    only_b = [c for c in b_cols if c not in a_cols]
    return compared, only_a, only_b


# ----------------------------------------------------------------------------
# DuckDB engine
# ----------------------------------------------------------------------------

def _q(name: str) -> str:
    return '"' + name.replace('"', '""') + '"'


def _lit(s: str) -> str:
    return "'" + s.replace("'", "''") + "'"


def _norm_sql(expr: str, opt: Options) -> str:
    if opt.trim:
        expr = f"trim({expr})"
    if opt.ignore_case:
        expr = f"lower({expr})"
    if opt.empty_is_null:
        expr = f"nullif({expr}, '')"
    return expr


def _diff_sql(a: str, b: str, opt: Options) -> str:
    if opt.tolerance > 0:
        return (f"(CASE WHEN try_cast({a} AS DOUBLE) IS NOT NULL AND try_cast({b} AS DOUBLE) IS NOT NULL "
                f"THEN abs(try_cast({a} AS DOUBLE) - try_cast({b} AS DOUBLE)) > {opt.tolerance!r} "
                f"ELSE ({a} IS DISTINCT FROM {b}) END)")
    return f"({a} IS DISTINCT FROM {b})"


def _compare_duckdb(a_path: str, b_path: str, opt: Options) -> dict[str, Any]:
    import duckdb

    con = duckdb.connect()
    if opt.threads:
        con.execute(f"SET threads = {int(opt.threads)}")
    if opt.memory_limit:
        con.execute(f"SET memory_limit = '{opt.memory_limit}'")
    con.execute("SET preserve_insertion_order = true")

    # null_padding leaves a short row's missing fields absent, which is what the other
    # engines do with them. Without it DuckDB abandons the split on a ragged file and
    # returns it as one column named after the header line, and the comparison then fails
    # with "key column missing" -- a true statement about DuckDB's table and a misleading
    # one about the file.
    read_opts = "all_varchar=true, header=true, sample_size=-1, null_padding=true"
    if opt.delimiter:
        read_opts += f", delim={_lit(opt.delimiter)}"
    if opt.encoding and opt.encoding.lower() not in ("utf-8", "utf8"):
        read_opts += f", encoding={_lit(opt.encoding)}"

    def load(tbl: str, path: str) -> list[str]:
        con.execute(f"CREATE TABLE {tbl}_raw AS SELECT * FROM read_csv({_lit(path)}, {read_opts})")
        return [r[0] for r in con.execute(f"DESCRIBE {tbl}_raw").fetchall()]

    a_cols = load("a", a_path)
    b_cols = load("b", b_path)
    compared, only_a, only_b = _resolve_columns(a_cols, b_cols, opt)
    key = opt.key

    # Normalised projection + stable row number
    for tbl in ("a", "b"):
        proj = ", ".join(f"{_norm_sql(_q(c), opt)} AS {_q(c)}" for c in key + compared)
        con.execute(f"CREATE TABLE {tbl} AS SELECT {proj}, row_number() OVER () AS _rn FROM {tbl}_raw")
        con.execute(f"DROP TABLE {tbl}_raw")

    kq = ", ".join(_q(k) for k in key)
    counts: dict[str, int] = {}
    counts["a_rows"] = con.execute("SELECT count(*) FROM a").fetchone()[0]
    counts["b_rows"] = con.execute("SELECT count(*) FROM b").fetchone()[0]

    # Duplicate keys
    dups = {}
    for tbl in ("a", "b"):
        con.execute(f"CREATE TABLE {tbl}_dup AS SELECT {kq}, count(*) AS n FROM {tbl} GROUP BY ALL HAVING count(*) > 1")
        nk, nr = con.execute(f"SELECT count(*), coalesce(sum(n), 0) FROM {tbl}_dup").fetchone()
        counts[f"{tbl}_dup_keys"], counts[f"{tbl}_dup_rows"] = int(nk), int(nr)
        counts[f"{tbl}_keys"] = counts[f"{tbl}_rows"] - counts[f"{tbl}_dup_rows"] + counts[f"{tbl}_dup_keys"]
        rows = con.execute(f"SELECT * FROM {tbl}_dup ORDER BY n DESC, {kq} LIMIT {opt.max_rows}").fetchall()
        dups[tbl] = {"cols": key + ["count"], "rows": [list(r) for r in rows],
                     "truncated": nk > opt.max_rows}
        # First occurrence per key participates in the join
        con.execute(f"CREATE TABLE {tbl}1 AS SELECT * EXCLUDE(_k) FROM "
                    f"(SELECT *, row_number() OVER (PARTITION BY {kq} ORDER BY _rn) AS _k FROM {tbl}) WHERE _k = 1")

    # Full outer join
    key_sel = ", ".join(f"coalesce(a.{_q(k)}, b.{_q(k)}) AS {_q(k)}" for k in key)
    on = " AND ".join(f"a.{_q(k)} IS NOT DISTINCT FROM b.{_q(k)}" for k in key)
    col_sel, diff_flags = [], []
    for i, c in enumerate(compared):
        a, b = f"a.{_q(c)}", f"b.{_q(c)}"
        d = _diff_sql(a, b, opt)
        col_sel += [f"{a} AS {_q('a_' + str(i))}", f"{b} AS {_q('b_' + str(i))}", f"{d} AS {_q('d_' + str(i))}"]
        diff_flags.append(d)
    changed_expr = " OR ".join(diff_flags) if diff_flags else "false"
    con.execute(f"""
        CREATE TABLE j AS
        SELECT {key_sel},
               CASE WHEN a._rn IS NULL THEN 'added' WHEN b._rn IS NULL THEN 'removed' ELSE 'matched' END AS _status,
               {", ".join(col_sel) + "," if col_sel else ""}
               ({changed_expr}) AS _changed
        FROM a1 a FULL OUTER JOIN b1 b ON {on}
    """)

    for status, chg, n in con.execute("SELECT _status, _changed, count(*) FROM j GROUP BY ALL").fetchall():
        if status == "matched":
            counts["changed" if chg else "unchanged"] = counts.get("changed" if chg else "unchanged", 0) + n
        else:
            counts[status] = counts.get(status, 0) + n
    for k in ("matched", "unchanged", "changed", "added", "removed"):
        counts.setdefault(k, 0)
    counts["matched"] = counts["unchanged"] + counts["changed"]

    # Per-column stats
    columns = []
    if compared:
        agg = ", ".join(
            f"sum({_q('d_%d' % i)}::INT), "
            f"sum(({_q('a_%d' % i)} IS NOT NULL AND {_q('b_%d' % i)} IS NULL)::INT), "
            f"sum(({_q('a_%d' % i)} IS NULL AND {_q('b_%d' % i)} IS NOT NULL)::INT)"
            for i in range(len(compared)))
        stats = con.execute(f"SELECT {agg} FROM j WHERE _status = 'matched'").fetchone()
        for i, c in enumerate(compared):
            columns.append({"name": c, "changed": int(stats[3 * i] or 0),
                            "blanked": int(stats[3 * i + 1] or 0), "filled": int(stats[3 * i + 2] or 0)})

    # Changed rows -> sparse cell diffs
    nk = len(key)
    changed_rows = []
    if compared:
        cols = kq + ", " + ", ".join(f"{_q('a_%d' % i)}, {_q('b_%d' % i)}, {_q('d_%d' % i)}" for i in range(len(compared)))
        cur = con.execute(f"SELECT {cols} FROM j WHERE _status = 'matched' AND _changed ORDER BY {kq} LIMIT {opt.max_rows}")
        for r in cur.fetchall():
            cells = [[i, r[nk + 3 * i], r[nk + 3 * i + 1]] for i in range(len(compared)) if r[nk + 3 * i + 2]]
            changed_rows.append(list(r[:nk]) + [cells])

    def side_rows(status: str, prefix: str):
        cols = kq + "".join(f", {_q(prefix + str(i))}" for i in range(len(compared)))
        rows = con.execute(f"SELECT {cols} FROM j WHERE _status = {_lit(status)} ORDER BY {kq} LIMIT {opt.max_rows}").fetchall()
        return {"cols": key + compared, "rows": [list(r) for r in rows], "truncated": counts[status] > opt.max_rows}

    added = side_rows("added", "b_")
    removed = side_rows("removed", "a_")

    if opt.export_dir:
        os.makedirs(opt.export_dir, exist_ok=True)
        aliases = lambda p: "".join(f", {_q(p + str(i))} AS {_q(c)}" for i, c in enumerate(compared))  # noqa: E731
        con.execute(f"COPY (SELECT {kq}{aliases('b_')} FROM j WHERE _status='added' ORDER BY {kq}) TO "
                    f"{_lit(os.path.join(opt.export_dir, 'added.csv'))} (HEADER)")
        con.execute(f"COPY (SELECT {kq}{aliases('a_')} FROM j WHERE _status='removed' ORDER BY {kq}) TO "
                    f"{_lit(os.path.join(opt.export_dir, 'removed.csv'))} (HEADER)")
        if compared:
            both = "".join(f", {_q('a_%d' % i)} AS {_q(c + ' (A)')}, {_q('b_%d' % i)} AS {_q(c + ' (B)')}"
                           for i, c in enumerate(compared))
            con.execute(f"COPY (SELECT {kq}{both} FROM j WHERE _status='matched' AND _changed ORDER BY {kq}) TO "
                        f"{_lit(os.path.join(opt.export_dir, 'changed.csv'))} (HEADER)")

    con.close()
    return {
        "meta": {"key": key, "compared": compared, "only_in_a": only_a, "only_in_b": only_b,
                 "a_cols": len(a_cols), "b_cols": len(b_cols)},
        "counts": counts,
        "columns": columns,
        "changed": {"cols": key, "rows": changed_rows, "truncated": counts["changed"] > opt.max_rows},
        "added": added, "removed": removed,
        "dup_a": dups["a"], "dup_b": dups["b"],
    }
