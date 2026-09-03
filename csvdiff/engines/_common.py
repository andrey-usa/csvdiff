"""Helpers shared by the alternative engines.

Every engine has to reproduce the DuckDB reference semantics exactly, because
CI asserts that all of them return identical `counts` and `columns`. The rules
that are easy to get wrong live here so there is one place to read them:

* an unquoted empty field is NULL, and only an empty field — `NA`, `NULL` and
  `nan` stay text (no type inference anywhere, `1.0` != `1`);
* normalisation (trim / lower / empty-is-null) applies to key columns too;
* two values differ when they are `DISTINCT FROM` each other, i.e. NULL equals
  NULL and NULL differs from anything else;
* with a tolerance, values differ only when *both* parse as numbers and the
  absolute difference exceeds it — otherwise fall back to DISTINCT FROM;
* rows are ordered by the key ascending with NULLs last (DuckDB's default);
* duplicate keys are listed by descending count, then by key.
"""
from __future__ import annotations

from contextlib import contextmanager
from typing import Any, Iterable

# Rows whose key contains a NULL still have to join with each other. Engines
# whose hash join follows plain SQL equality (pyarrow) join on a copy of the
# key with NULLs mapped to this sentinel; the original values are what gets
# reported, so it never reaches the report.
NULL_SENTINEL = "\x00\x01NULL\x01\x00"


def sort_key(values: Iterable[Any]) -> tuple:
    """Key function reproducing `ORDER BY ... ASC` in DuckDB: NULLs last."""
    return tuple(x for v in values for x in ((1, "") if v is None else (0, str(v))))


def values_differ(x: Any, y: Any, tolerance: float) -> bool:
    """`x IS DISTINCT FROM y`, with the numeric tolerance escape hatch."""
    if tolerance > 0 and x is not None and y is not None:
        try:
            return abs(float(x) - float(y)) > tolerance
        except (TypeError, ValueError):
            pass
    return x != y


def counts_from_parts(a_rows: int, b_rows: int, dup: dict[str, dict[str, int]],
                      matched: int, changed: int, added: int, removed: int) -> dict[str, int]:
    """Assemble the `counts` block in the documented order."""
    counts = {"a_rows": int(a_rows), "b_rows": int(b_rows)}
    for side in ("a", "b"):
        rows = counts[f"{side}_rows"]
        dk, dr = int(dup[side]["keys"]), int(dup[side]["rows"])
        counts[f"{side}_dup_keys"], counts[f"{side}_dup_rows"] = dk, dr
        counts[f"{side}_keys"] = rows - dr + dk
    counts["matched"] = int(matched)
    counts["unchanged"] = int(matched) - int(changed)
    counts["changed"] = int(changed)
    counts["added"] = int(added)
    counts["removed"] = int(removed)
    return counts


def result(key: list[str], compared: list[str], only_a: list[str], only_b: list[str],
           a_ncols: int, b_ncols: int, counts: dict[str, int], columns: list[dict[str, Any]],
           changed_rows: list[list[Any]], added_rows: list[list[Any]], removed_rows: list[list[Any]],
           dup_rows: dict[str, list[list[Any]]], max_rows: int) -> dict[str, Any]:
    """Build the result dict documented at the top of `csvdiff/engine.py`."""
    return {
        "meta": {"key": key, "compared": compared, "only_in_a": only_a, "only_in_b": only_b,
                 "a_cols": a_ncols, "b_cols": b_ncols},
        "counts": counts,
        "columns": columns,
        "changed": {"cols": key, "rows": changed_rows, "truncated": counts["changed"] > max_rows},
        "added": {"cols": key + compared, "rows": added_rows, "truncated": counts["added"] > max_rows},
        "removed": {"cols": key + compared, "rows": removed_rows, "truncated": counts["removed"] > max_rows},
        "dup_a": {"cols": key + ["count"], "rows": dup_rows["a"], "truncated": counts["a_dup_keys"] > max_rows},
        "dup_b": {"cols": key + ["count"], "rows": dup_rows["b"], "truncated": counts["b_dup_keys"] > max_rows},
    }


def sniff_delimiter(path: str, encoding: str = "utf-8") -> str:
    """Delimiter of `path`, for the engines whose CSV reader cannot detect one.

    DuckDB sniffs; polars, pyarrow, DataFusion and the standard library reader
    do not, so they all go through this. Falls back to a comma.
    """
    import csv

    with open(path, "r", encoding=encoding, errors="replace", newline="") as f:
        sample = f.read(64 * 1024)
    if not sample:
        return ","
    try:
        return csv.Sniffer().sniff(sample, delimiters=",;\t|").delimiter
    except csv.Error:
        return ","


def require_utf8(encoding: str, engine: str) -> None:
    """Guard for engines whose reader is UTF-8 only."""
    from ..engine import CompareError

    if encoding and encoding.lower().replace("_", "-") not in ("utf-8", "utf8", "ascii"):
        raise CompareError(f"The {engine} engine reads UTF-8 only; "
                           f"use --engine duckdb or --engine pandas for {encoding}.")


@contextmanager
def csv_out(path: str, header: list[str]):
    """Open an export CSV and yield a `write(rows)` callable.

    Exports have to be byte-identical across engines, and the writers that ship
    with pyarrow (quotes every string) and pandas (its own NULL rendering) are
    not, so every engine that does not use DuckDB's `COPY` writes through this.
    """
    import csv

    with open(path, "w", encoding="utf-8", newline="") as f:
        writer = csv.writer(f, lineterminator="\n")
        writer.writerow(header)
        yield lambda rows: writer.writerows(
            ["" if v is None else v for v in row] for row in rows)
