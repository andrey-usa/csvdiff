#!/usr/bin/env python3
"""Writes the small files the format readers' tests are held to.

The Parquet reader in the Rust and Zig ports decodes its own pages, so the tests
have to cover what a page can actually be: plain and dictionary encodings, the
version-2 delta encodings, five compression codecs, several row groups, nulls,
and the typed columns that have to be rendered as text before they can be
compared. Those are all *writer* choices, which is why these are written by
pyarrow rather than by this project -- a reader tested only against files its own
writer produced tests nothing.

The same 300-row table is written as CSV and as newline-delimited JSON too, so a
test can assert the thing that actually matters: the comparison returns the same
counts whichever format each side arrives in.

The output is committed, because the tests must run without pyarrow installed.
Regenerate with:

    pip install pyarrow
    python3 scripts/make_parquet_fixtures.py tests/fixtures/formats
"""
import csv
import datetime as dt
import json
import os
import sys

ROWS = 300


def sides():
    """Two versions of one table, with a known drift between them.

    B changes one column in every fourth row, blanks one value, drops a row, adds
    a row and repeats a key, so the counts a test asserts are not all zero and
    each of them is reached by a different path through the engine.
    """
    a = {
        "id": [f"K-{i:04d}" for i in range(ROWS)],
        "status": [["open", "closed", "pending", "settled"][i % 4] for i in range(ROWS)],
        "amount": [f"{i * 3}.{i % 100:02d}" for i in range(ROWS)],
        "note": ["" if i % 7 == 0 else f"line {i}" for i in range(ROWS)],
        "maybe": [None if i % 5 == 0 else f"v{i % 11}" for i in range(ROWS)],
    }
    b = {k: list(v) for k, v in a.items()}
    for i in range(ROWS):
        if i % 4 == 0:
            b["status"][i] = "reversed"
        if i % 50 == 0:
            b["amount"][i] = ""
    # A row only in B, then a key repeated within B.
    for column, extra in (("id", "K-9000"), ("status", "open"), ("amount", "1.00"),
                          ("note", "added"), ("maybe", "v1")):
        b[column].append(extra)
    for column in b:
        b[column].append(b[column][0])
    # A row only in A.
    for column in a:
        a[column].append("K-9999" if column == "id" else "only-in-a")
    return a, b


def shortest(value):
    """A float as the ports render it: the shortest digits that read back the
    same, laid out the way JavaScript's number-to-string does it.

    The rule has to be stated somewhere, because "the shortest round trip" does
    not say whether 1e-10 is written out in full. Every port implements this one,
    so a float column compares equal across all of them.
    """
    if value != value or value in (float("inf"), float("-inf")):
        return {float("inf"): "Infinity", float("-inf"): "-Infinity"}.get(value, "NaN")
    if value == 0:
        return "0"
    text = repr(abs(value))
    if "e" in text:
        digits, exponent = text.split("e")
        exponent = int(exponent)
    else:
        digits, exponent = text, 0
    digits = digits.replace(".", "").rstrip("0") or "0"
    point = (text.split("e")[0].find(".") if "." in text else len(text.split("e")[0]))
    n = point + exponent
    k = len(digits)
    sign = "-" if value < 0 else ""
    if k <= n <= 21:
        return sign + digits + "0" * (n - k)
    if 0 < n <= 21:
        return sign + digits[:n] + "." + digits[n:]
    if -6 < n <= 0:
        return sign + "0." + "0" * -n + digits
    tail = digits[0] + ("." + digits[1:] if k > 1 else "")
    return f"{sign}{tail}e{'+' if n > 0 else '-'}{abs(n - 1)}"


def write_csv(path, table):
    with open(path, "w", newline="") as fh:
        out = csv.writer(fh, lineterminator="\n")
        out.writerow(table.keys())
        for row in zip(*table.values()):
            out.writerow(["" if v is None else v for v in row])


def write_ndjson(path, table):
    names = list(table)
    with open(path, "w") as fh:
        for row in zip(*table.values()):
            fh.write(json.dumps(dict(zip(names, row))) + "\n")


def main(argv):
    import pyarrow as pa
    import pyarrow.parquet as pq

    out_dir = argv[0] if argv else "tests/fixtures/formats"
    os.makedirs(out_dir, exist_ok=True)
    a, b = sides()
    written = []

    def record(path):
        written.append(path)
        return path

    write_csv(record(os.path.join(out_dir, "a.csv")), a)
    write_csv(record(os.path.join(out_dir, "b.csv")), b)
    write_ndjson(record(os.path.join(out_dir, "a.ndjson")), a)
    write_ndjson(record(os.path.join(out_dir, "b.ndjson")), b)

    table_a, table_b = pa.table(a), pa.table(b)

    def write(name, table, **kwargs):
        pq.write_table(table, record(os.path.join(out_dir, name)), **kwargs)

    # One file per writer decision the reader has to survive.
    write("a_dict_snappy.parquet", table_a, compression="snappy", use_dictionary=True)
    write("b_dict_snappy.parquet", table_b, compression="snappy", use_dictionary=True)
    write("a_plain_none.parquet", table_a, compression="none", use_dictionary=False)
    write("a_dict_gzip.parquet", table_a, compression="gzip", use_dictionary=True)
    write("a_plain_zstd.parquet", table_a, compression="zstd", use_dictionary=False)
    write("a_plain_lz4.parquet", table_a, compression="lz4", use_dictionary=False)
    # Version 2 pages with the delta encodings, which are a different decoder.
    write("a_delta_v2.parquet", table_a, compression="none", use_dictionary=False,
          version="2.6", data_page_version="2.0",
          column_encoding={"id": "DELTA_BYTE_ARRAY", "status": "DELTA_LENGTH_BYTE_ARRAY",
                           "amount": "DELTA_BYTE_ARRAY", "note": "DELTA_LENGTH_BYTE_ARRAY",
                           "maybe": "DELTA_BYTE_ARRAY"})
    # Several row groups, so a dictionary has to be carried per column chunk.
    write("a_dict_row_groups.parquet", table_a, compression="snappy",
          use_dictionary=True, row_group_size=64)

    # The typed columns: every one of these has to become the text a CSV of the
    # same data would hold. `typed.csv` is that text, written from the same
    # values by this script, so a test compares the reader's rendering against a
    # stated expectation rather than against another copy of itself.
    typed = {
        "id": [f"K-{i:04d}" for i in range(8)],
        "count": [0, 1, -1, 42, 1000, -1000, 2**31 - 1, -(2**31)],
        "big": [0, 1, -1, 2**40, -(2**40), 7, 8, 9],
        "ratio": [0.0, 0.5, -0.25, 1.5, 3.0, 1e10, 1e-10, 2.25],
        "flag": [True, False, True, True, False, False, True, False],
        "day": [dt.date(1970, 1, 1), dt.date(2024, 1, 1), dt.date(2024, 2, 29),
                dt.date(1969, 12, 31), dt.date(2000, 3, 1), dt.date(1999, 12, 31),
                dt.date(2038, 1, 19), dt.date(2026, 9, 7)],
        "at": [dt.datetime(2024, 1, 1, 0, 0, 0), dt.datetime(2024, 1, 1, 12, 34, 56),
               dt.datetime(1970, 1, 1, 0, 0, 0), dt.datetime(2026, 9, 7, 22, 15, 0),
               dt.datetime(1999, 12, 31, 23, 59, 59), dt.datetime(2000, 1, 1, 0, 0, 1),
               dt.datetime(2024, 6, 30, 6, 30, 30), dt.datetime(2024, 12, 25, 18, 45, 15)],
        "money": [0, 1, -1, 12345, -12345, 999999, 100, -100],
    }
    write("typed.parquet", pa.table({
        "id": pa.array(typed["id"]),
        "count": pa.array(typed["count"], pa.int32()),
        "big": pa.array(typed["big"], pa.int64()),
        "ratio": pa.array(typed["ratio"], pa.float64()),
        "flag": pa.array(typed["flag"]),
        "day": pa.array(typed["day"]),
        "at": pa.array(typed["at"], pa.timestamp("us")),
        "money": pa.array(typed["money"], pa.decimal128(12, 2)),
    }), compression="snappy")

    def as_text(name, value):
        """The text the reader is required to render this value as."""
        if name == "flag":
            return "true" if value else "false"
        if name == "day":
            return value.isoformat()
        if name == "at":
            # Microsecond timestamps, with a zero fraction dropped.
            return value.isoformat().replace("+00:00", "")
        if name == "money":
            # pyarrow reads a Python int for a decimal(12, 2) column as that many
            # whole units, so 12345 is stored as 12345.00 rather than as 123.45.
            return f"{value}.00"
        if name == "ratio":
            return shortest(value)
        return str(value)

    write_csv(record(os.path.join(out_dir, "typed.csv")),
              {name: [as_text(name, v) for v in values] for name, values in typed.items()})

    for path in written:
        print(f"{path}  {os.path.getsize(path):,} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
