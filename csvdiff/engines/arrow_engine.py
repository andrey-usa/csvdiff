"""pyarrow engine — the Arrow CSV reader plus an Acero hash join, no planner.

This is the low-level version of what DataFusion and polars do for you: read
both files into Arrow tables of strings, group to find duplicate keys, join, and
compute the diff flags with `pyarrow.compute` kernels. Worth measuring because
the Arrow CSV reader is one of the fastest around, and because it shows the cost
of doing the plumbing by hand.

Acero's hash join follows SQL equality, where NULL never matches NULL, so rows
are joined on a copy of the key with NULLs mapped to `_common.NULL_SENTINEL`
and the originals are restored afterwards.
"""
from __future__ import annotations

import os
from typing import Any

from ..engine import Options, _resolve_columns
from . import _common as C

SUFFIX = "__b"
NUMERIC = r"^\s*[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?\s*$"


def _read(path: str, opt: Options):
    import pyarrow as pa
    from pyarrow import csv as pacsv

    delim = opt.delimiter or C.sniff_delimiter(path, opt.encoding)
    names = pacsv.open_csv(path, parse_options=pacsv.ParseOptions(delimiter=delim)).schema.names
    return names, pacsv.read_csv(
        path,
        read_options=pacsv.ReadOptions(use_threads=True),
        parse_options=pacsv.ParseOptions(delimiter=delim),
        convert_options=pacsv.ConvertOptions(
            column_types={n: pa.string() for n in names},
            strings_can_be_null=True, null_values=[""], true_values=[], false_values=[]))


def _norm(col, opt: Options):
    import pyarrow as pa
    import pyarrow.compute as pc

    if opt.trim:
        col = pc.utf8_trim_whitespace(col)
    if opt.ignore_case:
        col = pc.utf8_lower(col)
    if opt.empty_is_null:
        col = pc.if_else(pc.equal(col, ""), pa.scalar(None, pa.string()), col)
    return col


def _distinct(a, b):
    """`a IS DISTINCT FROM b` over two string arrays."""
    import pyarrow.compute as pc

    both_null = pc.and_(pc.is_null(a), pc.is_null(b))
    return pc.and_(pc.fill_null(pc.not_equal(a, b), True), pc.invert(both_null))


def _differs(a, b, opt: Options):
    import pyarrow as pa
    import pyarrow.compute as pc

    distinct = _distinct(a, b)
    if opt.tolerance <= 0:
        return distinct
    # There is no try_cast kernel: mask the non-numeric values before casting.
    def num(col):
        ok = pc.fill_null(pc.match_substring_regex(col, NUMERIC), False)
        return ok, pc.cast(pc.if_else(ok, col, pa.scalar("0")), pa.float64())

    ok_a, fa = num(a)
    ok_b, fb = num(b)
    numeric = pc.and_(ok_a, ok_b)
    return pc.if_else(numeric, pc.greater(pc.abs(pc.subtract(fa, fb)), opt.tolerance), distinct)


def compare(a_path: str, b_path: str, opt: Options) -> dict[str, Any]:
    import pyarrow as pa
    import pyarrow.compute as pc

    C.require_utf8(opt.encoding, "arrow")
    names, tables = {}, {}
    for side, path in (("a", a_path), ("b", b_path)):
        names[side], tables[side] = _read(path, opt)
    compared, only_a, only_b = _resolve_columns(names["a"], names["b"], opt)
    key, nk, nc = opt.key, len(opt.key), len(compared)
    jk = [f"_jk{i}" for i in range(nk)]

    singles, dup_rows, dup_counts, rowcount = {}, {}, {}, {}
    for side in ("a", "b"):
        t = tables[side]
        rowcount[side] = t.num_rows
        cols = {c: _norm(t.column(c), opt) for c in key + compared}
        joinable = {name: pc.fill_null(cols[k], C.NULL_SENTINEL) for name, k in zip(jk, key)}
        t = pa.table({**joinable, **{c: cols[c] for c in compared},
                      "_rn": pa.array(range(t.num_rows), type=pa.int64()),
                      f"_in_{side}": pa.array([True] * t.num_rows, type=pa.bool_())})
        tables[side] = None
        g = t.select(jk + ["_rn"]).group_by(jk).aggregate([("_rn", "min"), ("_rn", "count")])
        n = g.column("_rn_count")
        dup = g.filter(pc.greater(n, 1))
        dup_counts[side] = {"keys": dup.num_rows,
                            "rows": int(pc.sum(dup.column("_rn_count")).as_py() or 0)}
        dup = _restore_keys(dup, jk, key)
        dup = dup.sort_by([("_rn_count", "descending")] + [(k, "ascending") for k in key])
        dup_rows[side] = _to_rows(dup.slice(0, opt.max_rows), key + ["_rn_count"])
        # First occurrence of each key is the one that joins.
        singles[side] = t.take(g.column("_rn_min").combine_chunks())

    j = singles["a"].join(singles["b"], keys=jk, join_type="full outer", right_suffix=SUFFIX)
    del singles
    in_a = pc.fill_null(j.column("_in_a"), False)
    in_b = pc.fill_null(j.column("_in_b"), False)
    matched_mask = pc.and_(in_a, in_b)
    j = _restore_keys(j, jk, key)

    flags = [_differs(j.column(c), j.column(c + SUFFIX), opt) for c in compared]
    changed_mask = flags[0] if nc else pa.chunked_array([[]], type=pa.bool_())
    for f in flags[1:]:
        changed_mask = pc.or_(changed_mask, f)
    if nc:
        j = j.append_column("_changed", changed_mask)
        for i, f in enumerate(flags):
            j = j.append_column(f"_d{i}", f)
    else:
        j = j.append_column("_changed", pa.array([False] * j.num_rows))

    matched = j.filter(matched_mask)
    counts = C.counts_from_parts(
        rowcount["a"], rowcount["b"], dup_counts,
        matched=matched.num_rows,
        changed=int(pc.sum(pc.cast(matched.column("_changed"), pa.int64())).as_py() or 0),
        added=int(pc.sum(pc.cast(pc.invert(in_a), pa.int64())).as_py() or 0),
        removed=int(pc.sum(pc.cast(pc.invert(in_b), pa.int64())).as_py() or 0))

    columns = []
    for i, c in enumerate(compared):
        x, y = matched.column(c), matched.column(c + SUFFIX)
        total = lambda mask: int(pc.sum(pc.cast(mask, pa.int64())).as_py() or 0)  # noqa: E731
        columns.append({"name": c, "changed": total(matched.column(f"_d{i}")),
                        "blanked": total(pc.and_(pc.is_valid(x), pc.is_null(y))),
                        "filled": total(pc.and_(pc.is_null(x), pc.is_valid(y)))})

    order = [(k, "ascending") for k in key]
    changed_rows = []
    if nc:
        top = matched.filter(matched.column("_changed")).sort_by(order).slice(0, opt.max_rows)
        keys = [top.column(k).to_pylist() for k in key]
        cells = [(top.column(c).to_pylist(), top.column(c + SUFFIX).to_pylist(),
                  top.column(f"_d{i}").to_pylist()) for i, c in enumerate(compared)]
        for r in range(top.num_rows):
            triples = [[i, old[r], new[r]] for i, (old, new, d) in enumerate(cells) if d[r]]
            changed_rows.append([k[r] for k in keys] + [triples])

    def side_rows(mask, suffix: str):
        sub = j.filter(mask).sort_by(order).slice(0, opt.max_rows)
        return _to_rows(sub, key + [c + suffix for c in compared])

    added_rows = side_rows(pc.invert(in_a), SUFFIX)
    removed_rows = side_rows(pc.invert(in_b), "")

    if opt.export_dir:
        os.makedirs(opt.export_dir, exist_ok=True)

        def dump(table, source: list[str], header: list[str], name: str) -> None:
            with C.csv_out(os.path.join(opt.export_dir, name), header) as write:
                for batch in table.select(source).to_batches():
                    write(zip(*[c.to_pylist() for c in batch.columns]))

        for mask, suffix, name in ((pc.invert(in_a), SUFFIX, "added.csv"),
                                   (pc.invert(in_b), "", "removed.csv")):
            dump(j.filter(mask).sort_by(order), key + [c + suffix for c in compared],
                 key + compared, name)
        if nc:
            dump(matched.filter(matched.column("_changed")).sort_by(order),
                 key + [x for c in compared for x in (c, c + SUFFIX)],
                 key + [x for c in compared for x in (f"{c} (A)", f"{c} (B)")], "changed.csv")

    return C.result(key, compared, only_a, only_b, len(names["a"]), len(names["b"]),
                    counts, columns, changed_rows, added_rows, removed_rows, dup_rows, opt.max_rows)


def _restore_keys(table, jk: list[str], key: list[str]):
    """Turn the sentinel join columns back into the original key columns."""
    import pyarrow as pa
    import pyarrow.compute as pc

    for col, name in zip(jk, key):
        values = table.column(col)
        table = table.drop_columns([col]).append_column(
            name, pc.if_else(pc.equal(values, C.NULL_SENTINEL), pa.scalar(None, pa.string()), values))
    return table


def _to_rows(table, columns: list[str]) -> list[list[Any]]:
    cols = [table.column(c).to_pylist() for c in columns]
    return [list(r) for r in zip(*cols)] if cols else []
