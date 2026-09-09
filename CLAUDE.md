# csvdiff

Composite-key table comparison — CSV, newline-delimited JSON and Parquet — as byte-level ports in
C, C++, Rust and Zig, held to one result contract. Key columns, compared columns and
normalisation are runtime parameters; nothing about a specific dataset belongs in the code.

The DuckDB-backed Python implementation, the Java, TypeScript and Go ports, and the dataframe
engines beside them are no longer in the project -- including the Rust port's own `duckdb` and
`polars` engines, removed once the comparison they existed for was settled. Their results are in
ARCHIVE.md.

## Commands

```bash
(cd c && make)                                # csvdiff and gen-data; the leading port
(cd c && bash test.sh)                        # its own checks, a few seconds
(cd c && bash test.sh --with-ports)           # plus cross-port and generator parity
(cd cpp && make && make gen-data && bash test.sh)

c/gen-data --rows 10k --out-dir data --prefix p              # CSV
c/gen-data --rows 10k --out-dir data --prefix p --format json
c/gen-data --rows 10k --out-dir data --prefix p --format parquet

c/csvdiff compare data/p_a.csv data/p_b.csv -k account_id,txn_id -i updated_at
python scripts/bench_ports.py data/p_a.csv data/p_b.csv --repeats 5
gh workflow run "Benchmark (native)" -f rows=10m -f all_ports=true
```

## Layout

| Path | Role |
|---|---|
| `c/` | the leading port on every format measured. `csvdiff.c` (CSV and ndjson), `parquet.c` + `pqdiff.c` (the columnar path), `parallel.c`, `gen-data.c` + `pqwrite.c` (its own generator) |
| `cpp/` | the C++ port, and the generator that also writes Snappy |
| `rust/`, `zig/` | the other byte-level ports, same result contract |
| `scripts/bench_ports.py` | every port on one pair, interleaved, with a counts gate |
| `scripts/gen_data.py` | the original Python generator, kept as the reference recipe |

## Invariants

- **Every port must return identical `counts` and `columns`.** `parity.yml` asserts this on 200k
  rows, for every format each port reads. A change to one port needs the matching change in the
  others, or a reason it does not apply.
- **The result contract is the API.** `engine.compare()` returns the dict documented in
  `engine.py`; `report.py`, the CLI, the server and the mailbot all consume only that. Add a
  field rather than reshaping an existing one.
- **CSV values are read as text.** No type inference — `1.0` and `1` are different unless a
  tolerance is set. Do not add dtype guessing.
- **The report is one file with no external references.** No CDN, no fonts, no frameworks.
  CI fails if any `src=` or `href=` points outside the document.
- **Only differing cells are embedded.** Changed rows carry `[colIndex, old, new]` triples, not
  full rows. Keeping this sparse is what keeps a 60k-change report near 1 MB.
- **Row sections are capped** by `--max-rows` (default 50k); counts are always exact and the UI
  says when a list is truncated. `--export-dir` writes the uncapped CSVs.
- **SQL is built by string interpolation.** Column and table names go through `_q()`, string
  values and paths through `_lit()`. Never interpolate with `!r` — Python repr is not SQL.
- **The generators must emit byte-identical files.** `c/test.sh --with-ports` holds `c/gen-data`
  against `cpp/build/gen-data` across every format and option. Changing the drift recipe means
  changing it in both, deliberately — a benchmark number from one generator is only comparable with
  a number from another if the bytes agree.

## Style

- No runtime dependency carries a comparison engine any more: the four ports are the engine. Do
  not add a dataframe library, a web framework, a JS bundler, or a templating library.
- The report JS is plain ES2020 in `report.py`. It must keep working when opened from `file://`.
- Prefer editing the existing virtualised grid over adding a table library; the grid renders only
  the visible rows and that is the reason large reports open instantly.

## Gotchas

- `resource.ru_maxrss` is KB on Linux, bytes on macOS — the harnesses in `scripts/` handle both.
- Duplicate keys: the first occurrence of each key joins, the rest are reported separately.
  Changing that changes the matched/added/removed counts, so it is a behaviour change, not a fix.
- The report decodes its gzip payload with `DecompressionStream`, which needs a 2023+ browser.
  `--no-compress` is the escape hatch.
- **One benchmark at a time, repository-wide.** Two timing jobs running at once share a host and
  measure each other's contention, which spoils both — including the one already running that
  somebody is waiting on. Check for a run in progress before pushing to a path that triggers a
  benchmark or dispatching one by hand. The benchmark workflows name a single `benchmark-host`
  concurrency group so GitHub queues them; a per-ref group does not, because another branch is
  another group.
