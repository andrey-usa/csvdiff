# csvdiff

Composite-key CSV comparison producing a single self-contained HTML report.
Key columns, compared columns and normalisation are runtime parameters — nothing about
a specific dataset belongs in the code.

## Commands

```bash
pip install duckdb pandas -r requirements-dev.txt && pip install -e . --no-deps
pytest                                        # full suite, ~30s
python scripts/gen_data.py --rows 10k --out-dir data
python scripts/bench.py --rows 10k --engine duckdb
csvdiff compare data/10k_a.csv data/10k_b.csv -k account_id,txn_id -i updated_at --open
csvdiff serve                                 # drop page on :8765
gh workflow run Benchmark -f scales=10k       # CI
```

## Layout

| Path | Role |
|---|---|
| `csvdiff/engine.py` | comparison; DuckDB primary, pandas fallback. Result contract is documented at the top of the file |
| `csvdiff/report.py` | HTML renderer — one template string, no build step |
| `csvdiff/cli.py` | `compare` / `serve` / `mail` |
| `csvdiff/server.py`, `mailbot.py` | drop page and mailbox launchers |
| `csvdiff/config.py` | profiles from `csvdiff.toml` |
| `scripts/gen_data.py`, `scripts/bench.py` | test payloads and benchmark harness |

## Invariants

- **Both engines must return identical `counts` and `columns`.** CI asserts this on 200k rows.
  Any change to one engine needs the matching change in the other.
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

- Standard library only outside the engine; DuckDB is the sole runtime dependency and pandas is
  an optional fallback. Do not add a web framework, a JS bundler, or a templating library.
- The report JS is plain ES2020 in `report.py`. It must keep working when opened from `file://`.
- Prefer editing the existing virtualised grid over adding a table library; the grid renders only
  the visible rows and that is the reason large reports open instantly.

## Gotchas

- `pandas` path holds everything in memory: ~2.8 GB for 1M rows x 20 columns. Use DuckDB for
  anything above ~1M rows, and never benchmark 10M on the pandas engine.
- `resource.ru_maxrss` is KB on Linux, bytes on macOS — `scripts/bench.py` handles both.
- Duplicate keys: the first occurrence of each key joins, the rest are reported separately.
  Changing that changes the matched/added/removed counts, so it is a behaviour change, not a fix.
- The report decodes its gzip payload with `DecompressionStream`, which needs a 2023+ browser.
  `--no-compress` is the escape hatch.
