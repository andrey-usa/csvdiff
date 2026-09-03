# csvdiff

Composite-key CSV comparison producing a single self-contained HTML report.
Key columns, compared columns and normalisation are runtime parameters — nothing about
a specific dataset belongs in the code.

## Commands

```bash
pip install -e ".[bench]" -r requirements-dev.txt   # every engine; or just `pip install duckdb`
pytest                                        # full suite, ~30s
python scripts/gen_data.py --rows 10k --out-dir data
python scripts/bench.py --rows 10k --engine duckdb
python scripts/bench_matrix.py --scales 10k,1m --repeat 3   # every engine, ranked
csvdiff compare data/10k_a.csv data/10k_b.csv -k account_id,txn_id -i updated_at --open
csvdiff serve                                 # drop page on :8765
gh workflow run Benchmark -f scales=10k       # CI
gh workflow run "Engine benchmark" -f scales=10k,1m
```

## Layout

| Path | Role |
|---|---|
| `csvdiff/engine.py` | comparison entry point and the DuckDB / pandas engines. Result contract is documented at the top of the file |
| `csvdiff/engines/` | the engine registry and the alternative implementations (polars, DataFusion, pyarrow, standard library, koala-diff). Reference semantics every engine has to match are in `_common.py` |
| `csvdiff/report.py` | HTML renderer — one template string, no build step |
| `csvdiff/cli.py` | `compare` / `serve` / `mail` |
| `csvdiff/server.py`, `mailbot.py` | drop page and mailbox launchers |
| `csvdiff/config.py` | profiles from `csvdiff.toml` |
| `scripts/gen_data.py`, `scripts/bench.py` | test payloads and benchmark harness |
| `scripts/bench_matrix.py` | runs every engine over every scale, checks agreement, writes `bench/matrix.md` |
| `BENCHMARKS.md` | measured results and the reasoning behind the default engine |

## Invariants

- **Every engine must return identical `counts` and `columns`.** CI asserts this on 200k rows and
  `tests/test_engines.py` asserts the row sections and the `--export-dir` bytes too. A change to one
  engine needs the matching change in all of them. DuckDB is the reference: when engines disagree,
  DuckDB defines the right answer. The one exception is `koala`, registered with `contract=False`
  because the tool it wraps cannot express the contract; it is a benchmark reference, not a fallback.
- **The engine registry is the only place that knows the engine list.** `csvdiff/engines/__init__.py`
  drives `--engine`, `auto`, the tests and the benchmark matrix. Adding an engine means adding a
  `Spec` there, not another branch in `compare()`.
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
- **The generator's drift recipe is asserted in tests.** Changing rates in `scripts/gen_data.py`
  means updating `tests/test_gen_data.py` deliberately, not to make it pass.

## Style

- Standard library only outside the engines. DuckDB is the default runtime dependency; polars,
  pyarrow, DataFusion, pandas and koala-diff are optional extras, and the `python` engine keeps the
  tool working with none of them installed. Engine imports stay inside the engine module so an
  uninstalled engine costs nothing. Do not add a web framework, a JS bundler, or a templating library.
- The report JS is plain ES2020 in `report.py`. It must keep working when opened from `file://`.
- Prefer editing the existing virtualised grid over adding a table library; the grid renders only
  the visible rows and that is the reason large reports open instantly.

## Gotchas

- Only DuckDB is out-of-core. polars, DataFusion, pyarrow and pandas hold the join in memory, and
  the `python` engine holds file A in a dict, so above ~1M rows x 20 columns they need several GB and
  above ~10M they are killed. Use DuckDB there, and never benchmark 10M on the pandas engine.
- CSV readers disagree about NULL. A quoted empty field is NULL for DuckDB and pyarrow but an empty
  string for polars, which is why `polars_engine._norm` maps `''` to NULL before anything else. Any
  new engine needs the same check against `tests/test_engines.py::test_null_handling_matches_duckdb`.
- `resource.ru_maxrss` is KB on Linux, bytes on macOS — `scripts/bench.py` handles both.
- Duplicate keys: the first occurrence of each key joins, the rest are reported separately.
  Changing that changes the matched/added/removed counts, so it is a behaviour change, not a fix.
- The report decodes its gzip payload with `DecompressionStream`, which needs a 2023+ browser.
  `--no-compress` is the escape hatch.
