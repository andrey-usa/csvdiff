# csvdiff-ts

The TypeScript port of `csvdiff`, on current Node and TypeScript. Same comparison,
same result contract and the same self-contained HTML report as the Python
implementation in the repository root — CI asserts that both produce identical
counts and column stats on the same input.

Built with **TypeScript 7** and run on **Node 24+** (CI covers 24 and 26). Node runs
the `.ts` sources directly via native type stripping, so there is no build step for
day-to-day use; `npm run build` emits `dist/` for the CI benchmark and for `npm link`.

## Install

```bash
cd ts
npm ci
```

DuckDB and Polars both arrive as prebuilt platform binaries (`@duckdb/node-api`,
`nodejs-polars`); nothing is compiled. Arquero and the TOML profile parser are pure JS.

## Launch modes

**CLI**

```bash
node src/cli.ts compare july.csv august.csv --key order_id,line_no
node src/cli.ts compare july.csv august.csv --key id --compare qty,price --ignore updated_at --trim --tolerance 0.005
node src/cli.ts compare july.csv august.csv --profile orders --open --json summary.json --export-dir out/
```

Exit code 0 = identical, 1 = differences, 2 = error, 3 = duplicate keys (with `--fail-on-dups`),
so it drops into a CI or pipeline gate unchanged.

**Drag-and-drop page**

```bash
node src/cli.ts serve            # http://127.0.0.1:8765
```

Drop two files, type the key or pick a profile, the report opens in a new tab.
Runs on localhost; bind `--host 0.0.0.0` to share on a LAN.

**Profiles** (`csvdiff.toml`, see `../csvdiff.example.toml`) store key/compare/ignore/normalisation
per recurring comparison. The file format is shared with the Python implementation.

The mailbox launcher is not ported; use `csvdiff mail` from the Python implementation.

## Options

| Option | Effect |
|---|---|
| `--key` | composite key, required (or from profile) |
| `--compare` | columns to diff; default every column present in both files except the key |
| `--ignore` | columns to skip (timestamps, run ids) |
| `--trim`, `--ignore-case`, `--empty-is-null` | normalisation before comparing (applies to key and values) |
| `--tolerance` | absolute numeric tolerance where both sides parse as numbers |
| `--delimiter`, `--encoding` | override auto-detection |
| `--max-rows` | rows embedded per report section (default 50 000; counts are always exact) |
| `--export-dir` | full, uncapped changed/added/removed CSVs |
| `--engine` | `auto` (default), `duckdb`, `polars`, `arquero`, or `native` |
| `--threads`, `--memory-limit` | DuckDB resource limits |
| `--no-compress` | plain JSON payload for pre-2023 browsers |

Duplicate keys are counted and listed per file; the first occurrence of each key takes part in the join.

## Engines

Four backends, one result contract. Every engine must return identical `counts` and
`columns` for the same input; CI asserts that on 200k rows and the test suite asserts it
on the example files under three different option sets. `--engine auto` (the default)
takes the first one whose dependency is installed, in the order below.

| Engine | Implementation | Memory model | Use it for |
|---|---|---|---|
| `duckdb` | DuckDB via `@duckdb/node-api` (C++) | out-of-core, spills to disk | the default; anything that does not fit in RAM |
| `polars` | Polars via `nodejs-polars` (Rust) | in-memory, columnar, multithreaded | the fastest option while the data fits in RAM |
| `arquero` | Arquero (pure JavaScript) | in-memory, columnar | environments where no native binary may be installed |
| `native` | this repo, no dependencies | in-memory, row-oriented | a dependency-free fallback |

All four read CSV values as text — no type inference, so `1.0` and `1` stay different
unless a tolerance is set.

**Reading empty fields.** DuckDB, Arquero and the native reader all treat an empty field as
absent, quoted (`""`) or not. Polars keeps a quoted empty as a zero-length string, so the
Polars engine maps `""` to null on load, before any `--trim` / `--ignore-case` /
`--empty-is-null` normalisation. Without that, swapping engines could change a count; a test
guards it.

**Choosing.** Polars is the fastest engine here at scales that fit in memory, and DuckDB is
the one that keeps working when they do not — it is the default for that reason. The two
pure-JS engines are bounded by the V8 heap and are the first to fall over as data grows;
`scripts/bench.ts` records an engine that cannot finish a scale as a failed row (with the
reason: OOM killed, JS heap OOM, exceeds V8 limit, disk full) instead of aborting the run,
so the benchmark table shows exactly where each one stops.

## Development

```bash
npm run typecheck     # tsc --noEmit, strict
npm test              # node --test, runs the .ts sources directly
npm run build         # emit dist/
node scripts/gen-data.ts --rows 10k --out-dir data
node scripts/bench.ts --rows 10k --engine duckdb
```

`scripts/gen-data.ts` builds the same deterministic 20-column pair as the Python, Java, Go and
Rust generators,
keyed on `(account_id, txn_id)` with the same drift recipe, so a benchmark number here is
directly comparable with one from `scripts/bench.py`:

| Drift | Share of rows |
|---|---|
| `status` changed | 3.0% |
| `amount` changed | 1.5% |
| `balance` changed | 1.5% |
| `value_date` blanked | 0.3% |
| `updated_at` changed | 100% (excluded with `--ignore`) |
| rows only in B | 0.10% |
| rows only in A | 0.10% |
| duplicate keys | 0.01% per file |

`bench.ts` records generation time, comparison wall time, throughput, peak RSS of the
comparison process, report size and the resulting counts, then fails if a scale exceeds its
budget (10k: 20s / 1.5 GB, 1M: 120s / 6 GB, 10M: 900s / 12 GB on a 4-vCPU runner).

## Workflows

| Workflow | Trigger | What it does |
|---|---|---|
| `ci-ts.yml` | push, PR touching `ts/**` | typecheck + `node --test` on Node 24 and 26, a 10k smoke comparison per engine, a self-contained-report check, a job asserting all four engines agree on 200k rows, and a job asserting the TypeScript and Python builds agree on 200k rows |
| `benchmark-ts.yml` | manual, weekly cron | generates any scale you name (`10k`, `1m`, `10m`, `40m`, …) x 20 columns and runs any set of engines (`--engines duckdb,polars` or `all`) against the same generated files on one runner, uploads reports, and writes a comparison table plus the fastest engine per scale to the job summary |
| `compare-ts.yml` | manual, or `workflow_call` | compares two files given as repo paths or URLs and publishes the report as an artifact |

## Layout

```
src/engine.ts          compare() entry point, engine registry, shared column resolution
src/engine-duckdb.ts   DuckDB engine (default, out-of-core)
src/engine-polars.ts   Polars engine (Rust, columnar, in-memory)
src/engine-arquero.ts  Arquero engine (pure JS, columnar, in-memory)
src/engine-native.ts   dependency-free in-memory engine
src/report.ts          HTML renderer
src/report-template.ts report template, generated from ../csvdiff/report.py
src/cli.ts             compare / serve
src/server.ts          drag-and-drop page
src/server-page.ts     drop page template, generated from ../csvdiff/server.py
src/config.ts          profiles (csvdiff.toml)
src/csv.ts             RFC 4180 reader for the native engine
src/types.ts           result contract and Options
scripts/gen-data.ts    deterministic test payload generator (20 columns, known drift)
scripts/bench.ts       benchmark harness with budgets and job-summary output
tests/                 node --test
```
