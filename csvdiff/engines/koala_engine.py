"""koala-diff engine — an external reference point, not a drop-in engine.

koala-diff (https://pypi.org/project/koala-diff/) is a Rust data-diff built on
polars. It is in the benchmark because it is the closest off-the-shelf tool to
what csvdiff does, but it answers a different question and cannot satisfy the
result contract:

* it infers column types, so `1.0` and `1` compare equal — csvdiff compares text;
* it has no `--ignore`, no normalisation and no tolerance, so it always compares
  every non-key column;
* duplicate keys are warned about, not resolved: rows are joined many-to-many
  instead of on the first occurrence;
* it returns per-column mismatch counts and a handful of sample values, so the
  changed / added / removed row sections cannot be built from its output.

The registry marks it `contract=False`; `engines.available(contract_only=True)`
leaves it out, and the cross-engine equality test skips it. What it does give is
an honest wall-clock number for "a Rust CSV differ on the same files".
"""
from __future__ import annotations

import contextlib
import io
import sys
from typing import Any

from ..engine import CompareError, Options, _resolve_columns
from . import _common as C

WARNING = ("warning: the koala engine is a benchmark reference — it infers column types, "
           "ignores --ignore/--tolerance/--trim and does not resolve duplicate keys, and it "
           "cannot fill the changed/added/removed row lists.")


def compare(a_path: str, b_path: str, opt: Options) -> dict[str, Any]:
    from koala_diff import DataDiff

    if opt.export_dir:
        raise CompareError("The koala engine cannot write --export-dir output.")
    print(WARNING, file=sys.stderr)

    names = {side: _header(path, opt) for side, path in (("a", a_path), ("b", b_path))}
    compared, only_a, only_b = _resolve_columns(names["a"], names["b"], opt)
    key = opt.key

    with contextlib.redirect_stdout(io.StringIO()):
        raw = DataDiff(key_columns=list(key)).compare(a_path, b_path)

    matched = int(raw["joined_count"])
    counts = C.counts_from_parts(
        a_rows=int(raw["total_rows_a"]), b_rows=int(raw["total_rows_b"]),
        dup={"a": {"keys": 0, "rows": 0}, "b": {"keys": 0, "rows": 0}},
        matched=matched, changed=int(raw["modified_rows_count"]),
        added=int(raw["added"]), removed=int(raw["removed"]))

    stats = raw.get("column_stats", {})
    columns = [{"name": c, "changed": int(stats.get(c, {}).get("non_match_count", 0)),
                "blanked": 0, "filled": 0} for c in compared]

    result = C.result(key, compared, only_a, only_b, len(names["a"]), len(names["b"]),
                      counts, columns, [], [], [], {"a": [], "b": []}, opt.max_rows)
    result["meta"]["partial"] = WARNING
    for section in ("changed", "added", "removed"):
        result[section]["truncated"] = counts[section] > 0
    return result


def _header(path: str, opt: Options) -> list[str]:
    import csv

    delim = opt.delimiter or C.sniff_delimiter(path, opt.encoding)
    with open(path, "r", encoding=opt.encoding, newline="") as f:
        return next(csv.reader(f, delimiter=delim), [])
