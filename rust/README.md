# csvdiff (Rust)

The Rust port of `csvdiff`. Same comparison, same result contract and the same self-contained HTML
report as the Python implementation in the repository root, the TypeScript one in `../ts`, the Java
one in `../java` and the Go one in `../go` — CI asserts all five produce identical counts and column
stats on the same input, and that their five data generators emit byte-identical files.

Rust **edition 2024**, stable toolchain. `cargo fmt --check`, `cargo clippy -D warnings` and
`cargo test` all gate CI.

## Install

```bash
cd rust
cargo build --release        # target/release/csvdiff
```

DuckDB is compiled from the bundled amalgamation, so the first build is slow and needs a C++
compiler; nothing else is required.

## Use

```bash
cargo run --release -- compare july.csv august.csv --key order_id,line_no
cargo run --release -- compare july.csv august.csv --key id --compare qty,price \
    --ignore updated_at --trim --tolerance 0.005
cargo run --release -- compare july.csv august.csv --profile orders \
    --json summary.json --export-dir out/
```

Exit code 0 = identical, 1 = differences, 2 = error, 3 = duplicate keys (with `--fail-on-dups`),
so it drops into a CI or pipeline gate unchanged.

**Profiles** (`csvdiff.toml`, see `../csvdiff.example.toml`) store key/compare/ignore/normalisation
per recurring comparison. The file format is shared with the other implementations.

The drag-and-drop page and the mailbox watcher are not ported; use the Python implementation for
`csvdiff serve` and `csvdiff mail`.

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
| `--engine` | `auto` (default), `duckdb`, `polars`, or `native` |
| `--threads`, `--memory-limit` | DuckDB resource limits |
| `--no-compress` | plain JSON payload for pre-2023 browsers |

Duplicate keys are counted and listed per file; the first occurrence of each key takes part in the join.

## Engines

Three backends, one result contract. Every engine must return identical `counts` and `columns` for
the same input; the test suite asserts it cell for cell on every engine, and CI asserts it on 200k
rows. `--engine auto` takes the first one that can actually load, in the order below.

| Engine | Implementation | Memory model | Use it for |
|---|---|---|---|
| `duckdb` | DuckDB through `duckdb-rs` (bundled C++) | out-of-core, spills to disk | the default; anything that does not fit in RAM |
| `polars` | Polars, natively | in-memory, columnar, multi-threaded | the dataframe comparison point |
| `native` | this project, over the `csv` crate | in-memory, row-oriented | the dependency-light baseline |

All three read CSV values as text — no type inference, so `1.0` and `1` stay different unless a
tolerance is set — and all three treat an empty field as absent whether or not it is quoted. Polars
needs help with that last rule: it reads an unquoted empty field as null but keeps a *quoted* empty
as a zero-length string, so the engine normalises it back before any user-supplied normalisation
runs. A test pins that behaviour.

This is the same Polars that the TypeScript port drives through `nodejs-polars`, so the pair
measures what the Node binding costs over the Rust original.

Only `duckdb` is unbounded by RAM.

## Development

```bash
cargo test
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo run --release --bin gen-data -- --rows 10k --out-dir data
cargo run --release --bin bench -- --rows 10k --engine duckdb
```

`gen-data` builds the same deterministic 20-column pair as the Python, TypeScript, Java and Go
generators, keyed on `(account_id, txn_id)` with the same splitmix hash and the same drift recipe.
Money is carried in integer cents and the drift is applied to those integers, never to a float, so
the files come out byte for byte identical without depending on any language's rounding rule:

| Drift | Share of rows |
|---|---|
| `status` changed | 3.0% |
| `amount` changed (+12.34) | 1.5% |
| `balance` changed (+1%, half up) | 1.5% |
| `value_date` blanked | 0.3% |
| `updated_at` changed | 100% (excluded with `--ignore`) |
| rows only in B | 0.10% |
| rows only in A | 0.10% |
| duplicate keys | 0.01% per file |

`bench` runs the comparison in a child process so peak RSS is measured honestly, records generation
time, wall time, throughput, report size and the resulting counts, and fails if a scale exceeds its
budget (10k: 20s / 1.5 GB, 1M: 120s / 6 GB, 10M: 900s / 12 GB on a 4-vCPU runner). An engine that
cannot handle a scale is recorded as a failed row with the reason rather than aborting the run.

The dev profile sets `debug = 0`: debug info for Polars plus a bundled DuckDB runs to tens of
gigabytes, and a failing test here is reproduced from its own output rather than a core dump.

## Workflows

| Workflow | Trigger | What it does |
|---|---|---|
| `ci-rust.yml` | push, PR touching `rust/**` | `cargo fmt --check`, `clippy -D warnings`, `cargo test`, a 10k smoke comparison, a self-contained-report check, and a job asserting all three engines agree on 200k rows |
| `benchmark-rust.yml` | manual, weekly cron | generates any scale you name and runs any set of engines, one isolated job per (scale, engine), then writes a comparison table and the fastest engine per scale to the job summary |

## Layout

```
src/main.rs               argument parsing and exit codes
src/lib.rs                the library's public surface
src/contract.rs           the result contract
src/options.rs            runtime parameters and engine names
src/engine.rs             compare() entry point and the engine registry
src/engine/duckdb.rs      DuckDB over duckdb-rs
src/engine/polars.rs      Polars frames, joins and expressions
src/engine/native.rs      csv-crate parse, RowStore join
src/rowstore.rs           de-duplication, join and sparse cell diffs for the in-memory engine
src/columns.rs            column resolution, normalisation, cell equality, key ordering
src/sections.rs           capping and the uncapped CSV exports
src/report.rs             HTML renderer
src/report.html           report template, shared with the other implementations
src/profiles.rs           csvdiff.toml profiles
src/gendata.rs            deterministic test payload generator
src/bin/gen_data.rs       its command line
src/bin/bench.rs          benchmark harness with budgets and job-summary output
tests/compare.rs          the comparison contract, asserted against every engine
```
