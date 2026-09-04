# csvdiff (Java)

The Java port of `csvdiff`, on the current release of the language. Same comparison, same result
contract and the same self-contained HTML report as the Python implementation in the repository
root and the TypeScript one in `../ts` — CI asserts all three produce identical counts and column
stats on the same input.

Built and run on **Java 26** (Temurin), with Maven. `-Xlint:all -Werror` is on, so a new warning
fails the build.

## Install

```bash
cd java
mvn package        # runs the tests and produces target/csvdiff.jar
```

DuckDB arrives as a prebuilt native library inside its JDBC jar; nothing is compiled. Tablesaw and
FastCSV are pure Java.

## Use

```bash
java -jar target/csvdiff.jar compare july.csv august.csv --key order_id,line_no
java -jar target/csvdiff.jar compare july.csv august.csv --key id --compare qty,price \
     --ignore updated_at --trim --tolerance 0.005
java -jar target/csvdiff.jar compare july.csv august.csv --profile orders \
     --json summary.json --export-dir out/
```

Exit code 0 = identical, 1 = differences, 2 = error, 3 = duplicate keys (with `--fail-on-dups`),
so it drops into a CI or pipeline gate unchanged.

**Profiles** (`csvdiff.toml`, see `../csvdiff.example.toml`) store key/compare/ignore/normalisation
per recurring comparison. The file format is shared with the other two implementations.

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
| `--engine` | `auto` (default), `duckdb`, `tablesaw`, or `native` |
| `--threads`, `--memory-limit` | DuckDB resource limits |
| `--no-compress` | plain JSON payload for pre-2023 browsers |

Duplicate keys are counted and listed per file; the first occurrence of each key takes part in the join.

## Engines

Three backends, one result contract. Every engine must return identical `counts` and `columns` for
the same input; the test suite asserts it on the example files under three option sets, and CI
asserts it on 200k rows. `--engine auto` takes the first one whose library is loadable, in the
order below.

| Engine | Implementation | Memory model | Use it for |
|---|---|---|---|
| `duckdb` | DuckDB over JDBC (C++) | out-of-core, spills to disk | the default; anything that does not fit in RAM |
| `tablesaw` | Tablesaw (pure Java) | in-memory, columnar | a dataframe-shaped comparison point |
| `native` | this project, over FastCSV | in-memory, row-oriented | the dependency-light baseline |

All three read CSV values as text — no type inference, so `1.0` and `1` stay different unless a
tolerance is set — and all three treat an empty field as absent whether or not it is quoted.

Only `duckdb` is unbounded by the heap. Tablesaw parses and stores the columns but the join is done
by `RowStore`, because Tablesaw's own full outer join materialises an intermediate table this
workload cannot afford at scale; the engine is still a fair measure of Tablesaw's parse and column
storage, which is where its time goes.

## Development

```bash
mvn verify                                   # compile with -Werror, then the tests
java -cp target/csvdiff.jar dev.csvdiff.bench.GenData --rows 10k --out-dir data
java -cp target/csvdiff.jar dev.csvdiff.bench.Bench --rows 10k --engine duckdb
```

`GenData` builds the same deterministic 20-column pair as the Python, TypeScript, Go and Rust
generators,
keyed on `(account_id, txn_id)` with the same splitmix hash and the same drift recipe, so a
benchmark number here is directly comparable with one from any of the others. Money is carried in integer cents and the drift is applied to those integers, never to a float, so the files come out byte for byte identical without depending on any language's rounding rule.

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

`Bench` runs the comparison in a child JVM so peak RSS is measured honestly, records generation
time, wall time, throughput, report size and the resulting counts, and fails if a scale exceeds its
budget (10k: 20s / 1.5 GB, 1M: 120s / 6 GB, 10M: 900s / 12 GB on a 4-vCPU runner). An engine that
cannot handle a scale is recorded as a failed row with the reason rather than aborting the run.

## Workflows

| Workflow | Trigger | What it does |
|---|---|---|
| `ci-java.yml` | push, PR touching `java/**` | `mvn verify` on Java 26, a 10k smoke comparison, a self-contained-report check, a job asserting all three engines agree on 200k rows, and a job asserting Java, Python and TypeScript agree on one dataset |
| `benchmark-java.yml` | manual, weekly cron | generates any scale you name and runs any set of engines against the same files on one runner, then writes a comparison table and the fastest engine per scale to the job summary |

## Layout

```
src/main/java/dev/csvdiff/
  CsvDiff.java              compare() entry point and the engine registry
  Contract.java             the result contract, as records
  Options.java              runtime parameters, immutable, with a merging builder
  Columns.java              column resolution, normalisation, cell equality, key ordering
  CompareEngine.java        the one-method interface every backend implements
  ReportRenderer.java       HTML renderer
  Profiles.java             csvdiff.toml profiles
  Cli.java                  argument parsing and exit codes
  engine/DuckDbEngine.java  DuckDB over JDBC
  engine/TablesawEngine.java
  engine/NativeEngine.java  FastCSV parse, RowStore join
  engine/RowStore.java      de-duplication, join and sparse cell diffs for the in-heap engines
  engine/Sections.java      capping and the uncapped CSV exports
  engine/Csv.java           delimiter sniffing and the empty-field rule
  bench/GenData.java        deterministic test payload generator
  bench/Bench.java          benchmark harness with budgets and job-summary output
src/main/resources/dev/csvdiff/report.html   report template, shared with the other implementations
src/test/java/dev/csvdiff/                   JUnit 5
```
