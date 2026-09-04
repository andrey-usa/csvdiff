# csvdiff (Java)

The Java port of `csvdiff`, on the current release of the language. Same comparison, same result
contract and the same self-contained HTML report as the Python implementation in the repository
root and the TypeScript one in `../ts` — CI asserts all three produce identical counts and column
stats on the same input.

Built and run on **Java 26** (Temurin), with Maven. `-Xlint:all -Werror` is on, so a new warning
fails the build. Three of the engines use the Foreign Function and Memory API and the incubating
Vector API — see [The fast three](#the-fast-three).

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
| `--engine` | `auto` (default), `duckdb`, `shard`, `mmap`, `simd`, `tablesaw`, or `native` |
| `--threads`, `--memory-limit` | DuckDB resource limits |
| `--no-compress` | plain JSON payload for pre-2023 browsers |

Duplicate keys are counted and listed per file; the first occurrence of each key takes part in the join.

## Engines

Six backends, one result contract. Every engine must return identical `counts` and `columns` for
the same input; the test suite asserts it on the example files under three option sets, and CI
asserts it on 200k rows. `--engine auto` takes the first one that can load, in the order below.

| Engine | Implementation | Memory model | Use it for |
|---|---|---|---|
| `duckdb` | DuckDB over JDBC (C++) | out-of-core, spills to disk | the default; anything that does not fit in RAM |
| `shard` | FFM mapping, Vector API scan, parallel index | off-heap bytes, index on heap | the fastest here on a multi-core machine |
| `mmap` | FFM mapping, Vector API scan | off-heap bytes, index on heap | large files on one core |
| `simd` | heap slab, Vector API scan | in-heap, bounded by 2 GB per file | what SIMD alone is worth |
| `tablesaw` | Tablesaw (pure Java) | in-memory, columnar | a dataframe-shaped comparison point |
| `native` | this project, over FastCSV | in-memory, row-oriented | the dependency-light baseline |

All six read CSV values as text — no type inference, so `1.0` and `1` stay different unless a
tolerance is set — and all six treat an empty field as absent whether or not it is quoted.

Tablesaw parses and stores the columns but the join is done by `RowStore`, because Tablesaw's own
full outer join materialises an intermediate table this workload cannot afford at scale; the engine
is still a fair measure of Tablesaw's parse and column storage, which is where its time goes.

### The fast three

`simd`, `mmap` and `shard` are one implementation with a single variable changed at a time, so that
putting them side by side in the benchmark says what each technique is actually worth.

They share a design that the other engines do not have: **no `String` is built for a cell unless
that cell reaches the report.** A row-at-a-time engine allocates twenty Strings per row — two
hundred million objects at ten million rows — and throws away all but the few thousand that end up
embedded. Here a field is an offset and a length into the file's bytes, hashing and comparison read
those bytes in place, and only the capped report sections are ever decoded.

| Technique | Where | What it buys |
|---|---|---|
| **Vector API** (`jdk.incubator.vector`) | `Scanner` | Finding the delimiter, newline and quote is the inner loop of CSV parsing. A `ByteVector` answers it for 32 or 64 bytes at once and returns a mask whose lowest set bit is the next hit, so a 184-byte row costs five or six vector compares instead of 184 branches. |
| **FFM API** (`java.lang.foreign`) | `Files2`, `Slab` | `mmap` and `shard` map the file with `FileChannel.map` into an `Arena`, so its bytes are the OS page cache and never enter the heap. The heap then holds only the index. It also unifies the two engines: a heap array and a mapped file are both a `MemorySegment`, so one scanner serves both. |
| **Primitive index** | `RowIndex` | Row offsets and key hashes in `long[]`, keys in an open-addressed `int[]` table. No nodes, no boxing, no `String` keys. |
| **Parallel parse** | `ShardedBuilder` | Rows are independent, so the file is cut into one chunk per core at row boundaries and each thread indexes its own. First-occurrence-wins depends on row order, so the shards are merged in file order afterwards — the merge only places pre-computed hashes, which is a fraction of the parse. |

Measured on a 4-vCPU machine, 1M rows x 20 columns, `-Xmx8g`:

| engine | compare | peak RSS |
|---|---|---|
| `shard` | **2.8s** | 630 MB |
| `mmap` | 3.0s | 627 MB |
| `simd` | 3.6s | 873 MB |
| `tablesaw` | 9.2s | 2141 MB |
| `native` | 9.1s | 3186 MB |
| `duckdb` | 10.9s | 2296 MB |

Their limits differ, which is the other reason to keep all three. `simd` reads the file into a
single Java array, so it refuses a file over 2 GB and needs the bytes plus the index inside `-Xmx`.
`mmap` and `shard` are bounded by neither, and alongside `duckdb` they are the only engines here
that can compare a file larger than memory.

**The Vector API is still an incubator module**, so it has to be asked for:

```bash
java --add-modules jdk.incubator.vector -jar target/csvdiff.jar compare a.csv b.csv -k id --engine shard
```

Without it the three engines report themselves unavailable, `--engine auto` skips them, and asking
for one by name gives a message saying what to add. The benchmark harness passes the flag to the
child JVM itself.

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
| `ci-java.yml` | push, PR touching `java/**` | `mvn verify` on Java 26, a 10k smoke comparison, a self-contained-report check, and a job asserting all six engines agree on 200k rows |
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
  engine/fast/Scanner.java  Vector API scan for delimiter, newline and quote
  engine/fast/Slab.java     a file's bytes: heap array or FFM mapping, plus the escape buffer
  engine/fast/Bytes.java    hashing, equality, ordering and decoding, on raw bytes
  engine/fast/RowParser.java  projecting split, skipping the columns nobody asked for
  engine/fast/RowIndex.java   row offsets, key hashes and the open-addressed key table
  engine/fast/FastJoin.java   the join, materialising only what the report embeds
  engine/fast/ShardedBuilder.java  one chunk per core, merged in file order
  engine/fast/{Simd,Mmap,Shard}Engine.java  the three configurations
  bench/GenData.java        deterministic test payload generator
  bench/Bench.java          benchmark harness with budgets and job-summary output
src/main/resources/dev/csvdiff/report.html   report template, shared with the other implementations
src/test/java/dev/csvdiff/                   JUnit 5
```
