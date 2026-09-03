"""Standard library engine — csv.reader plus a dict hash join, no dependencies.

The point of this one is the floor: it is what the comparison costs with nothing
but CPython, and it is the engine that always exists (`pip install csvdiff` with
no extras still compares files). File A is held in a dict keyed on the composite
key, file B is streamed against it, so peak memory tracks file A only. Rows for
the report are kept in bounded top-K buffers rather than accumulated, which is
what lets it finish a 1M-row comparison inside the memory budget.
"""
from __future__ import annotations

import csv
import os
import sys
from typing import Any, Iterator

from ..engine import Options, _resolve_columns
from . import _common as C

csv.field_size_limit(min(sys.maxsize, 2**31 - 1))


class _TopK:
    """The `k` smallest rows by sort key, without holding the whole section."""

    def __init__(self, k: int):
        self.k = k
        self.buf: list[tuple[tuple, list[Any]]] = []

    def add(self, sort_key: tuple, row: list[Any]) -> None:
        self.buf.append((sort_key, row))
        if len(self.buf) > 2 * self.k:
            self._prune()

    def _prune(self) -> None:
        self.buf.sort(key=lambda t: t[0])
        del self.buf[self.k:]

    def rows(self) -> list[list[Any]]:
        self._prune()
        return [r for _, r in self.buf]


def _reader(path: str, opt: Options) -> Iterator[list[str]]:
    delim = opt.delimiter or C.sniff_delimiter(path, opt.encoding)
    with open(path, "r", encoding=opt.encoding, newline="") as f:
        yield from csv.reader(f, delimiter=delim)


def _projector(header: list[str], wanted: list[str], opt: Options):
    """Row -> normalised values for `wanted`, in that order.

    An empty field is NULL before anything else happens, which is what DuckDB's
    all-varchar reader does. Trimming can then produce an empty string again, and
    only `--empty-is-null` turns that back into NULL.
    """
    idx = [header.index(c) for c in wanted]
    width = len(header)
    trim, lower, empty_null = opt.trim, opt.ignore_case, opt.empty_is_null

    def project(row: list[str]) -> tuple:
        if len(row) < width:
            row = row + [""] * (width - len(row))
        out = []
        for i in idx:
            v = row[i] or None
            if v is not None:
                if trim:
                    v = v.strip()
                if lower:
                    v = v.lower()
                if empty_null and v == "":
                    v = None
            out.append(v)
        return tuple(out)

    return project


def compare(a_path: str, b_path: str, opt: Options) -> dict[str, Any]:
    key = opt.key
    headers = {}
    for side, path in (("a", a_path), ("b", b_path)):
        rows = _reader(path, opt)
        headers[side] = next(rows, [])
        rows.close()
    compared, only_a, only_b = _resolve_columns(headers["a"], headers["b"], opt)
    nk, nc = len(key), len(compared)
    tol = opt.tolerance

    # --- file A into a dict, first occurrence per key wins -------------------
    a_rows = 0
    a_dups: dict[tuple, int] = {}
    a_map: dict[tuple, tuple] = {}
    project_a = _projector(headers["a"], key + compared, opt)
    it = _reader(a_path, opt)
    next(it, None)
    for row in it:
        values = project_a(row)
        a_rows += 1
        k = values[:nk]
        if k in a_map:
            a_dups[k] = a_dups.get(k, 1) + 1
        else:
            a_map[k] = values[nk:]

    # --- stream file B against it -------------------------------------------
    # `--export-dir` is uncapped, so the two streaming sections are written as
    # they are found rather than collected. They come out in file B's order;
    # every other engine sorts them by key.
    exports = _Exports(opt.export_dir, key, compared) if opt.export_dir else None
    b_rows = 0
    b_dups: dict[tuple, int] = {}
    b_seen: set[tuple] = set()
    columns = [{"name": c, "changed": 0, "blanked": 0, "filled": 0} for c in compared]
    matched = n_changed = added = 0
    changed_top, added_top = _TopK(opt.max_rows), _TopK(opt.max_rows)
    project_b = _projector(headers["b"], key + compared, opt)
    it = _reader(b_path, opt)
    next(it, None)
    for row in it:
        values = project_b(row)
        b_rows += 1
        k = values[:nk]
        if k in b_seen:
            b_dups[k] = b_dups.get(k, 1) + 1
            continue
        b_seen.add(k)
        left = a_map.pop(k, None)
        if left is None:
            added += 1
            added_top.add(C.sort_key(k), list(k) + list(values[nk:]))
            if exports:
                exports.added(k, values[nk:])
            continue
        matched += 1
        right = values[nk:]
        cells = []
        for i in range(nc):
            x, y = left[i], right[i]
            if C.values_differ(x, y, tol):
                cells.append([i, x, y])
                col = columns[i]
                col["changed"] += 1
                if y is None:
                    col["blanked"] += 1
                elif x is None:
                    col["filled"] += 1
        if cells:
            n_changed += 1
            changed_top.add(C.sort_key(k), list(k) + [cells])
            if exports:
                exports.changed(k, left, right)

    # --- whatever is left in A was removed -----------------------------------
    removed_top = _TopK(opt.max_rows)
    for k, values in a_map.items():
        removed_top.add(C.sort_key(k), list(k) + list(values))
    removed = len(a_map)

    dup_counts = {"a": {"keys": len(a_dups), "rows": sum(a_dups.values())},
                  "b": {"keys": len(b_dups), "rows": sum(b_dups.values())}}
    counts = C.counts_from_parts(a_rows, b_rows, dup_counts, matched=matched, changed=n_changed,
                                added=added, removed=removed)
    dup_rows = {side: [list(k) + [n] for k, n in
                       sorted(d.items(), key=lambda kv: (-kv[1], C.sort_key(kv[0])))[:opt.max_rows]]
                for side, d in (("a", a_dups), ("b", b_dups))}

    if exports:
        for k in sorted(a_map, key=C.sort_key):
            exports.removed(k, a_map[k])
        exports.close()

    changed_rows = changed_top.rows()
    added_rows, removed_rows = added_top.rows(), removed_top.rows()

    return C.result(key, compared, only_a, only_b, len(headers["a"]), len(headers["b"]),
                    counts, columns, changed_rows, added_rows, removed_rows, dup_rows, opt.max_rows)


class _Exports:
    """Uncapped changed / added / removed CSVs, written while streaming."""

    def __init__(self, directory: str, key: list[str], compared: list[str]):
        os.makedirs(directory, exist_ok=True)
        self._files, self._writers = [], {}
        headers = {
            "changed": key + [x for c in compared for x in (f"{c} (A)", f"{c} (B)")],
            "added": key + compared,
            "removed": key + compared,
        }
        for name, header in headers.items():
            f = open(os.path.join(directory, f"{name}.csv"), "w", encoding="utf-8", newline="")
            self._files.append(f)
            self._writers[name] = csv.writer(f, lineterminator="\n")
            self._writers[name].writerow(header)

    @staticmethod
    def _cells(values) -> list[Any]:
        return ["" if v is None else v for v in values]

    def changed(self, key_values: tuple, left: tuple, right: tuple) -> None:
        interleaved = [v for pair in zip(left, right) for v in pair]
        self._writers["changed"].writerow(self._cells(key_values) + self._cells(interleaved))

    def added(self, key_values: tuple, values: tuple) -> None:
        self._writers["added"].writerow(self._cells(key_values) + self._cells(values))

    def removed(self, key_values: tuple, values: tuple) -> None:
        self._writers["removed"].writerow(self._cells(key_values) + self._cells(values))

    def close(self) -> None:
        for f in self._files:
            f.close()
