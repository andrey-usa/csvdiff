# Awkward input

One pair of files holding every shape that has broken an engine in this project, or plausibly
could. Both wrong answers this tool has shipped needed neither an unusual option nor a malformed
file — one wanted a key in the last eight bytes of the file, the other a key outside ASCII — so the
generated benchmark data, which is uniform and pure ASCII, could never have found either.

| Row | What it is there for |
|---|---|
| `CAFÉ` / `café` | a non-ASCII key differing only in case |
| `  padded  ` / `padded` | whitespace around a key, which only `--trim` may ignore |
| `K` (U+212A) / `k` | a case fold that crosses the ASCII boundary and changes the byte length |
| `"has,comma"` | a quoted key holding the delimiter |
| `"a""b"` | a quoted key holding a doubled quote, the one value that is not a slice of the file |
| `"two\nlines"` | a quoted key holding a newline, which row splitting must not treat as a row end |
| `dup` twice | a duplicate key |
| `blank` | an empty value |
| `gone` / `extra` | a key on one side only |
| `z` | a short key in the last row, so it lands within eight bytes of the end of the file |

What the tests assert is not a particular answer but that the implementations cannot disagree about
one. Under the default options `CAFÉ` and `café` are different keys; under `--ignore-case` they are
the same. Either is a defensible answer. Two implementations giving different ones is not.

`parity.yml` runs it across all four ports, and each port's own `test.sh` holds the pair against
the Rust port's answers.

# Formats

`formats/` holds one small table written nine ways, and the point of it is the same as above: what
the tests assert is not a particular answer but that the readers cannot disagree about one. The
comparison has to return identical counts and identical per-column stats whether a side arrives as
CSV, as newline-delimited JSON, or as any of the Parquet encodings a writer might choose.

| File | The writer decision it covers |
|---|---|
| `a.csv`, `b.csv` | the reference answer everything else is held to |
| `a.ndjson`, `b.ndjson` | the same rows as one JSON object per line |
| `a_dict_snappy.parquet` | dictionary encoding, snappy — what DuckDB and pyarrow write by default |
| `a_plain_none.parquet` | no dictionary, no compression: the plain byte-array path |
| `a_dict_gzip.parquet` | gzip pages |
| `a_plain_zstd.parquet` | zstd pages |
| `a_plain_lz4.parquet` | LZ4 pages, whose block format the readers decode themselves |
| `a_delta_v2.parquet` | version-2 data pages with the DELTA_BYTE_ARRAY family, a different decoder |
| `a_dict_row_groups.parquet` | five row groups, so a dictionary has to be carried per column chunk |
| `typed.parquet` + `typed.csv` | every non-string type, against the text it is required to render as |

They are written by pyarrow (`scripts/make_parquet_fixtures.py`), not by this project: a reader
tested only against files its own writer produced tests nothing. `typed.csv` is the stated
expectation for how a typed column becomes text — integers and decimals in full, floats in the
shortest round-trip form, dates `YYYY-MM-DD`, timestamps ISO 8601 — so a change to that rule fails
a test rather than passing quietly in both ports at once.
