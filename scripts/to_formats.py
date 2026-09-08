#!/usr/bin/env python3
"""Rewrites a CSV as newline-delimited JSON or as Parquet, values unchanged.

The comparison this project does is textual -- `1.0` and `1` are different
unless a tolerance is set -- so a conversion that let a reader guess types would
change the answer rather than the format. Every column is read as VARCHAR and
written as a string, which is what makes the three formats of one dataset
comparable at all: they hold the same values, spelled the same way.

    python3 scripts/to_formats.py data/10m_a.csv --format ndjson
    python3 scripts/to_formats.py data/10m_a.csv --format parquet --compression zstd

DuckDB does the writing because it is the fastest thing here that can, and
because it is already the project's default engine.

For the *generated* benchmark payloads there is a shorter route that needs
neither DuckDB nor Python: `rust/target/release/gen-data --format ndjson` or
`--format parquet` writes them directly. This script is for the files you were
sent rather than the ones this project makes.
"""
import argparse
import os
import sys


def convert(src, fmt, compression, out, row_group_size):
    import duckdb

    reader = (f"read_csv('{src}', all_varchar = true, header = true, "
              f"sample_size = -1, null_padding = true)")
    if fmt == "ndjson":
        stmt = f"COPY (SELECT * FROM {reader}) TO '{out}' (FORMAT JSON);"
    else:
        stmt = (f"COPY (SELECT * FROM {reader}) TO '{out}' (FORMAT PARQUET, "
                f"COMPRESSION {compression}, ROW_GROUP_SIZE {row_group_size});")
    duckdb.execute(stmt)
    return out


def main(argv):
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("csv", nargs="+", help="the CSV files to convert")
    p.add_argument("--format", choices=["ndjson", "parquet"], required=True)
    p.add_argument("--compression", default="snappy",
                   choices=["uncompressed", "snappy", "gzip", "zstd"])
    p.add_argument("--row-group-size", type=int, default=1_000_000)
    p.add_argument("--out-dir", default=None,
                   help="where to write (default: beside the input)")
    args = p.parse_args(argv)

    for src in args.csv:
        stem = os.path.splitext(os.path.basename(src))[0]
        suffix = "ndjson" if args.format == "ndjson" else "parquet"
        directory = args.out_dir or os.path.dirname(os.path.abspath(src))
        out = os.path.join(directory, f"{stem}.{suffix}")
        convert(src, args.format, args.compression, out, args.row_group_size)
        size = os.path.getsize(out) / (1 << 20)
        print(f"{out}  {size:,.0f} MB")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
