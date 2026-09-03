"""Polars engine — lazy scan, vectorised normalisation, in-memory hash join.

Both files are scanned with the schema forced to String (no inference, so `1.0`
and `1` stay different), normalised in the scan and collected once; everything
after that is a single full outer join plus aggregations. Polars keeps the join
in memory, so it is the fastest option while both files fit in RAM and the wrong
one when they do not — that is what `--engine duckdb` is for.
"""
from __future__ import annotations

from typing import Any

from ..engine import Options, _resolve_columns
from . import _common as C

SUFFIX = "__b"


def _scan(path: str, opt: Options):
    import polars as pl

    # infer_schema=False keeps every column a string, so `1.0` and `1` stay
    # different; an empty field reads as null, which is what DuckDB does too.
    return pl.scan_csv(path, has_header=True, infer_schema=False, quote_char='"',
                       separator=opt.delimiter or C.sniff_delimiter(path, opt.encoding))


def _norm(col, opt: Options):
    # A quoted empty field ("") reads as an empty string in polars but as NULL in
    # DuckDB's all-varchar reader, which is the reference. Normalise it first, so
    # a later --trim still leaves '' (and only --empty-is-null turns that to NULL).
    col = col.replace("", None)
    if opt.trim:
        col = col.str.strip_chars()
    if opt.ignore_case:
        col = col.str.to_lowercase()
    if opt.empty_is_null:
        col = col.replace("", None)
    return col


def _differs(a, b, opt: Options):
    """`a IS DISTINCT FROM b`, honouring --tolerance."""
    import polars as pl

    distinct = a.eq_missing(b).not_()
    if opt.tolerance <= 0:
        return distinct
    an, bn = a.cast(pl.Float64, strict=False), b.cast(pl.Float64, strict=False)
    numeric = an.is_not_null() & bn.is_not_null()
    return pl.when(numeric).then((an - bn).abs() > opt.tolerance).otherwise(distinct)


def compare(a_path: str, b_path: str, opt: Options) -> dict[str, Any]:
    import polars as pl

    C.require_utf8(opt.encoding, "polars")
    a_names = _scan(a_path, opt).collect_schema().names()
    b_names = _scan(b_path, opt).collect_schema().names()
    compared, only_a, only_b = _resolve_columns(a_names, b_names, opt)
    key, nk, nc = opt.key, len(opt.key), len(compared)
    cols = key + compared

    frames, dup_rows, dup_counts, singles = {}, {}, {}, {}
    for side, path in (("a", a_path), ("b", b_path)):
        df = (_scan(path, opt)
              .select([_norm(pl.col(c), opt).alias(c) for c in cols])
              .collect(engine="streaming"))
        frames[side] = df
        d = (df.group_by(key).len("n")
             .filter(pl.col("n") > 1)
             .sort(["n"] + key, descending=[True] + [False] * nk, nulls_last=True))
        dup_counts[side] = {"keys": d.height, "rows": int(d["n"].sum()) if d.height else 0}
        dup_rows[side] = [list(r) for r in d.head(opt.max_rows).iter_rows()]
        # First occurrence of each key is the one that joins.
        singles[side] = df.unique(subset=key, keep="first", maintain_order=True)

    A, B = singles["a"], singles["b"]
    j = (A.with_columns(pl.lit(True).alias("_in_a"))
         .join(B.with_columns(pl.lit(True).alias("_in_b")), on=key, how="full",
               suffix=SUFFIX, nulls_equal=True, coalesce=True))

    flags = [_differs(pl.col(c), pl.col(c + SUFFIX), opt).fill_null(False).alias(f"_d{i}")
             for i, c in enumerate(compared)]
    matched_expr = pl.col("_in_a").fill_null(False) & pl.col("_in_b").fill_null(False)
    j = j.with_columns(flags + [matched_expr.alias("_matched")])
    changed_flag = (pl.any_horizontal([pl.col(f"_d{i}") for i in range(nc)]) if nc
                    else pl.lit(False)).alias("_changed")
    j = j.with_columns(changed_flag)

    matched = j.filter(pl.col("_matched"))
    counts = C.counts_from_parts(
        a_rows=frames["a"].height, b_rows=frames["b"].height, dup=dup_counts,
        matched=matched.height, changed=int(matched["_changed"].sum()) if matched.height else 0,
        added=int(j.filter(pl.col("_in_a").is_null()).height),
        removed=int(j.filter(pl.col("_in_b").is_null()).height))

    if nc and matched.height:
        stats = matched.select([e for i, c in enumerate(compared) for e in (
            pl.col(f"_d{i}").sum().alias(f"c{i}"),
            (pl.col(c).is_not_null() & pl.col(c + SUFFIX).is_null()).sum().alias(f"b{i}"),
            (pl.col(c).is_null() & pl.col(c + SUFFIX).is_not_null()).sum().alias(f"f{i}"))]).row(0)
        columns = [{"name": c, "changed": int(stats[3 * i]), "blanked": int(stats[3 * i + 1]),
                    "filled": int(stats[3 * i + 2])} for i, c in enumerate(compared)]
    else:
        columns = [{"name": c, "changed": 0, "blanked": 0, "filled": 0} for c in compared]

    def ordered(df):
        return df.sort(key, nulls_last=True).head(opt.max_rows)

    changed_rows = []
    if nc:
        sel = key + [x for i, c in enumerate(compared) for x in (c, c + SUFFIX, f"_d{i}")]
        for r in ordered(matched.filter(pl.col("_changed"))).select(sel).iter_rows():
            cells = [[i, r[nk + 3 * i], r[nk + 3 * i + 1]] for i in range(nc) if r[nk + 3 * i + 2]]
            changed_rows.append(list(r[:nk]) + [cells])

    def side_rows(missing: str, suffix: str):
        sub = ordered(j.filter(pl.col(missing).is_null()))
        return [list(r) for r in sub.select(key + [c + suffix for c in compared]).iter_rows()]

    added_rows = side_rows("_in_a", SUFFIX)
    removed_rows = side_rows("_in_b", "")

    if opt.export_dir:
        _export(j, matched, key, compared, opt)

    return C.result(key, compared, only_a, only_b, len(a_names), len(b_names),
                    counts, columns, changed_rows, added_rows, removed_rows, dup_rows, opt.max_rows)


def _export(j, matched, key: list[str], compared: list[str], opt: Options) -> None:
    import os

    import polars as pl

    os.makedirs(opt.export_dir, exist_ok=True)
    for missing, suffix, name in (("_in_a", SUFFIX, "added.csv"), ("_in_b", "", "removed.csv")):
        (j.filter(pl.col(missing).is_null()).sort(key, nulls_last=True)
         .select(key + [pl.col(c + suffix).alias(c) for c in compared])
         .write_csv(os.path.join(opt.export_dir, name)))
    if compared:
        (matched.filter(pl.col("_changed")).sort(key, nulls_last=True)
         .select(key + [e for c in compared for e in (pl.col(c).alias(f"{c} (A)"),
                                                      pl.col(c + SUFFIX).alias(f"{c} (B)"))])
         .write_csv(os.path.join(opt.export_dir, "changed.csv")))
