"""Apache DataFusion engine — the DuckDB SQL, planned and executed on Arrow.

It runs the same statements as the DuckDB engine (the SQL builders in
`csvdiff/engine.py` are shared), which makes it the cleanest way to see how much
of DuckDB's number is the query planner and how much is the CSV reader. Tables
are materialised in memory, so like polars it is bounded by RAM.
"""
from __future__ import annotations

import csv as _csv
import os
from typing import Any

from ..engine import Options, _diff_sql, _lit, _norm_sql, _q, _resolve_columns
from . import _common as C


def _header(path: str, delimiter: str, encoding: str) -> list[str]:
    with open(path, "r", encoding=encoding, newline="") as f:
        return next(_csv.reader(f, delimiter=delimiter))


def compare(a_path: str, b_path: str, opt: Options) -> dict[str, Any]:
    import pyarrow as pa
    from datafusion import SessionConfig, SessionContext

    C.require_utf8(opt.encoding, "datafusion")
    cfg = SessionConfig()
    if opt.threads:
        cfg = cfg.with_target_partitions(int(opt.threads))
    ctx = SessionContext(cfg)

    def sql(q: str):
        return ctx.sql(q)

    def rows(q: str) -> list[list[Any]]:
        out: list[list[Any]] = []
        for batch in sql(q).collect():
            cols = [c.to_pylist() for c in batch.columns]
            out.extend([list(r) for r in zip(*cols)] if cols else [])
        return out

    def one(q: str) -> list[Any]:
        r = rows(q)
        return r[0] if r else []

    delim = opt.delimiter or C.sniff_delimiter(a_path, opt.encoding)
    names = {}
    for tbl, path in (("a", a_path), ("b", b_path)):
        names[tbl] = _header(path, delim, opt.encoding)
        schema = pa.schema([(n, pa.string()) for n in names[tbl]])
        ctx.register_csv(f"{tbl}_raw", path, schema=schema, has_header=True, delimiter=delim)

    compared, only_a, only_b = _resolve_columns(names["a"], names["b"], opt)
    key, nk, nc = opt.key, len(opt.key), len(compared)
    kq = ", ".join(_q(k) for k in key)
    all_q = ", ".join(_q(c) for c in key + compared)

    for tbl in ("a", "b"):
        proj = ", ".join(f"{_norm_sql(_q(c), opt)} AS {_q(c)}" for c in key + compared)
        sql(f"CREATE TABLE {tbl} AS SELECT {proj}, row_number() OVER () AS _rn FROM {tbl}_raw").collect()

    counts_raw = {"a_rows": one("SELECT count(*) FROM a")[0], "b_rows": one("SELECT count(*) FROM b")[0]}

    dup_rows, dup_counts = {}, {}
    for tbl in ("a", "b"):
        sql(f"CREATE TABLE {tbl}_dup AS SELECT {kq}, count(*) AS n FROM {tbl} "
            f"GROUP BY {kq} HAVING count(*) > 1").collect()
        nkeys, nrows = one(f"SELECT count(*), coalesce(sum(n), 0) FROM {tbl}_dup")
        dup_counts[tbl] = {"keys": int(nkeys), "rows": int(nrows or 0)}
        dup_rows[tbl] = [r[:nk] + [int(r[nk])] for r in
                         rows(f"SELECT {kq}, n FROM {tbl}_dup ORDER BY n DESC, {kq} LIMIT {opt.max_rows}")]
        # First occurrence of each key is the one that joins.
        sql(f"CREATE TABLE {tbl}1 AS SELECT {all_q}, _rn FROM "
            f"(SELECT {all_q}, _rn, row_number() OVER (PARTITION BY {kq} ORDER BY _rn) AS _k FROM {tbl}) "
            f"WHERE _k = 1").collect()

    key_sel = ", ".join(f"coalesce(a.{_q(k)}, b.{_q(k)}) AS {_q(k)}" for k in key)
    # DataFusion's parser binds `IS NOT DISTINCT FROM` looser than `AND`, so each
    # key comparison needs its own parentheses to plan as a null-safe hash join.
    on = " AND ".join(f"(a.{_q(k)} IS NOT DISTINCT FROM b.{_q(k)})" for k in key)
    col_sel, diff_flags = [], []
    for i, c in enumerate(compared):
        x, y = f"a.{_q(c)}", f"b.{_q(c)}"
        d = _diff_sql(x, y, opt)
        col_sel += [f"{x} AS {_q('a_%d' % i)}", f"{y} AS {_q('b_%d' % i)}", f"{d} AS {_q('d_%d' % i)}"]
        diff_flags.append(d)
    changed_expr = " OR ".join(diff_flags) if diff_flags else "false"
    sql(f"""
        CREATE TABLE j AS
        SELECT {key_sel},
               CASE WHEN a._rn IS NULL THEN 'added' WHEN b._rn IS NULL THEN 'removed' ELSE 'matched' END AS _status,
               {", ".join(col_sel) + "," if col_sel else ""}
               ({changed_expr}) AS _changed
        FROM a1 a FULL OUTER JOIN b1 b ON {on}
    """).collect()

    tally = {"matched": 0, "changed": 0, "added": 0, "removed": 0}
    for status, chg, n in rows("SELECT _status, _changed, count(*) FROM j GROUP BY _status, _changed"):
        if status == "matched":
            tally["matched"] += int(n)
            tally["changed"] += int(n) if chg else 0
        else:
            tally[status] += int(n)
    counts = C.counts_from_parts(counts_raw["a_rows"], counts_raw["b_rows"], dup_counts, **tally)

    columns: list[dict[str, Any]] = []
    if nc:
        agg = ", ".join(
            f"sum(CAST({_q('d_%d' % i)} AS INT)), "
            f"sum(CAST(({_q('a_%d' % i)} IS NOT NULL AND {_q('b_%d' % i)} IS NULL) AS INT)), "
            f"sum(CAST(({_q('a_%d' % i)} IS NULL AND {_q('b_%d' % i)} IS NOT NULL) AS INT))"
            for i in range(nc))
        stats = one(f"SELECT {agg} FROM j WHERE _status = 'matched'")
        columns = [{"name": c, "changed": int(stats[3 * i] or 0), "blanked": int(stats[3 * i + 1] or 0),
                    "filled": int(stats[3 * i + 2] or 0)} for i, c in enumerate(compared)]

    changed_rows = []
    if nc:
        cells_q = kq + ", " + ", ".join(f"{_q('a_%d' % i)}, {_q('b_%d' % i)}, {_q('d_%d' % i)}" for i in range(nc))
        for r in rows(f"SELECT {cells_q} FROM j WHERE _status = 'matched' AND _changed "
                      f"ORDER BY {kq} LIMIT {opt.max_rows}"):
            cells = [[i, r[nk + 3 * i], r[nk + 3 * i + 1]] for i in range(nc) if r[nk + 3 * i + 2]]
            changed_rows.append(r[:nk] + [cells])

    def side_rows(status: str, prefix: str):
        cols = kq + "".join(f", {_q(prefix + str(i))}" for i in range(nc))
        return rows(f"SELECT {cols} FROM j WHERE _status = {_lit(status)} ORDER BY {kq} LIMIT {opt.max_rows}")

    added_rows, removed_rows = side_rows("added", "b_"), side_rows("removed", "a_")

    if opt.export_dir:
        os.makedirs(opt.export_dir, exist_ok=True)

        def dump(query: str, header: list[str], name: str) -> None:
            with C.csv_out(os.path.join(opt.export_dir, name), header) as write:
                for batch in sql(query).collect():
                    write(zip(*[c.to_pylist() for c in batch.columns]))

        aliases = lambda p: "".join(f", {_q(p + str(i))}" for i in range(nc))  # noqa: E731
        dump(f"SELECT {kq}{aliases('b_')} FROM j WHERE _status = 'added' ORDER BY {kq}",
             key + compared, "added.csv")
        dump(f"SELECT {kq}{aliases('a_')} FROM j WHERE _status = 'removed' ORDER BY {kq}",
             key + compared, "removed.csv")
        if nc:
            both = "".join(f", {_q('a_%d' % i)}, {_q('b_%d' % i)}" for i in range(nc))
            dump(f"SELECT {kq}{both} FROM j WHERE _status = 'matched' AND _changed ORDER BY {kq}",
                 key + [x for c in compared for x in (f"{c} (A)", f"{c} (B)")], "changed.csv")

    return C.result(key, compared, only_a, only_b, len(names["a"]), len(names["b"]),
                    counts, columns, changed_rows, added_rows, removed_rows, dup_rows, opt.max_rows)
