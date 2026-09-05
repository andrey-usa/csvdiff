# csvdiff (Go)

The Go port of `csvdiff`. Same comparison, same result contract and the same self-contained HTML
report as the Python implementation in the repository root, the TypeScript one in `../ts` and the
Java one in `../java` — CI asserts all four produce identical counts and column stats on the same
input, and that their four data generators emit byte-identical files.

Built and run on **Go 1.24**. `gofmt`, `go vet` and `go test -race` all gate CI.

## Install

```bash
cd go
go build ./...          # or: go install ./cmd/csvdiff
```

DuckDB arrives as a prebuilt native library through `go-duckdb`, so the build needs cgo; everything
else is the standard library.

## Use

```bash
go run ./cmd/csvdiff compare july.csv august.csv --key order_id,line_no
go run ./cmd/csvdiff compare july.csv august.csv --key id --compare qty,price \
    --ignore updated_at --trim --tolerance 0.005
go run ./cmd/csvdiff compare july.csv august.csv --profile orders \
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
| `--engine` | `auto` (default), `duckdb`, `sortmerge`, or `native` |
| `--threads`, `--memory-limit` | DuckDB resource limits |
| `--no-compress` | plain JSON payload for pre-2023 browsers |

Duplicate keys are counted and listed per file; the first occurrence of each key takes part in the join.

## Engines

Three backends, one result contract. Every engine must return identical `counts` and `columns` for
the same input; the test suite asserts it cell for cell, and CI asserts it on 200k rows.
`--engine auto` takes the first one that can actually load, in the order below.

| Engine | Implementation | Memory model | Use it for |
|---|---|---|---|
| `duckdb` | DuckDB through `go-duckdb` (cgo) | out-of-core, spills to disk | the default; anything that does not fit in RAM |
| `sortmerge` | external sort, merge join | bounded memory, spills to disk | files past what memory can index |
| `native` | this project, over `encoding/csv` | in-memory, row-oriented | the dependency-light baseline |

Both read CSV values as text — no type inference, so `1.0` and `1` stay different unless a tolerance
is set — and both treat an empty field as absent whether or not it is quoted.

Go has no dataframe library in the class of Polars or Tablesaw, so unlike the TypeScript and Java
ports there is no third, columnar engine here: `encoding/csv` plus a map is what a Go program would
actually reach for, and that is what `native` measures.

Only `duckdb` is unbounded by RAM.

## Development

```bash
go test ./...                                       # add -race as CI does
go run ./cmd/gen-data --rows 10k --out-dir data
go run ./cmd/bench --rows 10k --engine duckdb
```

`gen-data` builds the same deterministic 20-column pair as the Python, TypeScript and Java
generators, keyed on `(account_id, txn_id)` with the same splitmix hash and the same drift recipe.
The files come out byte for byte identical — a unit test pins their digests — so a benchmark number
here is directly comparable with one from any of the others:

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

`bench` runs the comparison in a child process so peak RSS is measured honestly, records generation
time, wall time, throughput, report size and the resulting counts, and fails if a scale exceeds its
budget (10k: 20s / 1.5 GB, 1M: 120s / 6 GB, 10M: 900s / 12 GB on a 4-vCPU runner). An engine that
cannot handle a scale is recorded as a failed row with the reason rather than aborting the run.

## Workflows

| Workflow | Trigger | What it does |
|---|---|---|
| `ci-go.yml` | push, PR touching `go/**` | `gofmt`, `go vet`, `go test -race`, a 10k smoke comparison, a self-contained-report check, a job asserting both engines agree on 200k rows, and a job asserting Go, Python, TypeScript and Java agree on one dataset and that their generators are byte-identical |
| `benchmark-go.yml` | manual, weekly cron | generates any scale you name and runs any set of engines, one isolated job per (scale, engine), then writes a comparison table and the fastest engine per scale to the job summary |

## Layout

```
cmd/csvdiff/main.go               argument parsing and exit codes
cmd/gen-data/main.go              deterministic test payload generator
cmd/bench/main.go                 benchmark harness with budgets and job-summary output
internal/csvdiff/
  contract.go                     the result contract
  options.go                      runtime parameters and engine names
  engine.go                       Compare() entry point and the engine registry
  engine_duckdb.go                DuckDB over go-duckdb
  engine_native.go                encoding/csv parse, RowStore join
  rowstore.go                     de-duplication, join and sparse cell diffs for the in-memory engine
  columns.go                      column resolution, normalisation, cell equality, key ordering
  sections.go                     capping and the uncapped CSV exports
  report.go                       HTML renderer
  report.html                     report template, shared with the other implementations
  profiles.go                     csvdiff.toml profiles
```
