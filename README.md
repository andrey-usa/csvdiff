# csvdiff

Compare two tables on a composite key and get a self-contained HTML report. Key
columns, compared columns and normalisation rules are parameters, so the same
tool serves every recurring comparison.

The same tool exists in five languages to one result contract, plus three
byte-level parity ports. That is what makes the benchmark sections below a
like-for-like comparison rather than a collection of anecdotes.

**Jump to:** [Using it](#using-it) · [Four formats, one engine](#four-formats-one-engine) ·
[Which engine at which size](#which-engine-at-which-size) ·
[Benchmarks](#benchmarks) · [Scaling to 50M](#how-the-fastest-build-scales) ·
[Formats](#input-formats-is-csv-the-problem) · [Techniques](#techniques) · [Ports](#ports) ·
[Reproducing](#reproducing-the-numbers) · [Open questions](#open-questions)

---

# Four formats, one engine

The C++ port reads **CSV, newline-delimited JSON and Parquet natively** — no
library between it and the bytes — and the generator *writes* all three from one
field-by-field recipe. So a format comparison here involves nothing but this
project: no third-party reader, no conversion step, no intermediate CSV.

Measured on a stock 4-cpu GitHub Actions runner by
[`.github/workflows/benchmark-formats.yml`](.github/workflows/benchmark-formats.yml),
which runs on every push that touches `cpp/`. Every row is the same comparison:
first-occurrence-wins on `(account_id, txn_id)`, inner join, per-cell diff over
seventeen columns.

### 10,000,000 rows × 20 columns

| Input | Size | Generate | **Compare** | Rows/s | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| CSV | 3,509 MB | 9.07s | **10.51s** | 951,238/s | 31.4s | 4,389 MB |
| JSON (ndjson) | 8,488 MB | 31.83s | **14.44s** | 692,409/s | 42.1s | 9,363 MB |
| Parquet + snappy | 992 MB | 6.37s | **2.81s** | 3,555,956/s | 9.1s | 4,006 MB |
| **Parquet uncompressed** | 2,074 MB | **4.87s** | **2.35s** | **4,250,766/s** | 7.2s | **3,468 MB** |

### 100,000 rows × 20 columns

| Input | Size | Generate | **Compare** | Rows/s | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| CSV | 35 MB | 0.05s | **0.24s** | 424,367/s | 0.6s | 58 MB |
| JSON (ndjson) | 85 MB | 0.10s | **0.29s** | 341,462/s | 0.8s | 108 MB |
| Parquet + snappy | 10 MB | 0.16s | **0.08s** | 1,298,720/s | 0.2s | 47 MB |
| **Parquet uncompressed** | 21 MB | 0.17s | **0.04s** | **2,260,539/s** | 0.1s | 47 MB |

**All four formats return identical counts** — at 10M: matched 9,990,000,
changed 599,320, added 10,000, removed 10,000, duplicate rows 2,000 in A and
1,000 in B. That is the workflow's correctness gate, not a footnote: four
readers that disagree about how many rows changed is a bug, so the run fails and
names the format that disagreed.

Three things worth taking from these tables.

**Parquet is 4.5x CSV and 6.1x JSON at ten million rows**, on a fifth of the
bytes and 21% less memory. Not because the parser got faster — because the
comparison stops being a comparison of strings. See
[Reading Parquet natively](#reading-parquet-natively).

**Writing Parquet is quicker than writing CSV.** 4.87s against 9.07s, because
there are 1.4 GB fewer bytes to put on disk and the encoding is cheaper than
formatting text. The route this project used to take — write the CSV, then have
DuckDB convert it — cost 20.9s + 67.9s on a comparable machine.

**JSON costs what its bytes cost.** 2.4x the size of the CSV and 1.4x the
comparison time, with every field carrying its name. It is worth having because
it is the input people actually receive, not because it is fast.

---

# Using it

## Install

```bash
pip install duckdb            # engine
pip install -e .              # gives you the `csvdiff` command
```

Python 3.14. Everything except the engine is standard library, and DuckDB is the
only engine this implementation carries — the alternatives are the byte-level
[ports](#ports), held to the same result contract.

## Launch modes

**CLI**

```bash
csvdiff compare july.csv august.csv --key order_id,line_no
csvdiff compare july.csv august.csv --key id --compare qty,price --ignore updated_at --trim --tolerance 0.005
csvdiff compare july.csv august.csv --profile orders --open --json summary.json --export-dir out/
```

Exit code 0 = identical, 1 = differences, 2 = error, 3 = duplicate keys (with `--fail-on-dups`).
That makes it a drop-in CI / pipeline gate.

**Drag-and-drop page**

```bash
csvdiff serve            # http://127.0.0.1:8765
```

Drop two files, type the key or pick a profile, the report opens in a new tab.
Runs on localhost; bind `--host 0.0.0.0` to share on a LAN.

**Email**

```bash
export CSVDIFF_MAIL_PASSWORD=...
csvdiff mail             # polls the mailbox in [mail] of csvdiff.toml
```

Send a mail with two CSV attachments and a subject such as

```
csvdiff key=order_id,line_no ignore=updated_at tolerance=0.005 trim
csvdiff profile=orders
csvdiff profile=orders a=july.csv b=august.csv
```

The reply contains a text summary and the report as an attachment (gzipped above 9 MB).
`allowed_senders` restricts who can trigger it.

**Profiles** (`csvdiff.toml`, see `csvdiff.example.toml`) store key/compare/ignore/normalisation
per recurring comparison so nobody retypes them.

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
| `--engine` | `duckdb`, which is the only one this implementation carries; the alternatives are the [ports](#ports) |
| `--threads`, `--memory-limit` | DuckDB resource limits |
| `--no-compress` | plain JSON payload for pre-2023 browsers |

Duplicate keys are counted and listed per file; the first occurrence of each key takes part in the
join. That is a deliberate choice with visible consequences — see
[Duplicate keys](#duplicate-keys-the-one-thing-nothing-agrees-on).

A row with more or fewer fields than the header is a difference to report, not a file to refuse: the
missing fields read as absent and the extra ones are ignored. All five implementations agree on this
and the test suites hold them to it. The one exception is Java's `tablesaw` engine, whose reader has
no option to allow it; it refuses such a file and says which engines will take it.

## Why DuckDB is the default engine

Both files are read as text (no type-inference surprises such as `1.0` vs `1`), hash-joined on the
key in parallel, and spilled to disk when they don't fit in RAM. Multi-GB files compare in seconds to
low minutes on a laptop, with one wheel as the only dependency. Polars is comparably fast in memory
but not out-of-core. The [benchmarks](#benchmarks) below put numbers on both, and on the bespoke
engines that beat them.

## The report

One HTML file, no network, no fonts, no frameworks.

- Reconciliation bar on top: unchanged / changed / removed / added at a glance; click a
  number to jump to that list.
- Row counts, unique keys and duplicates per file, columns present in only one file.
- Tabs: Changed (sparse cell diffs as old→new chips), Added, Removed, Duplicate keys,
  Columns (changed / blanked / filled counts per column with a proportional bar).
- Filter box, per-column chips on the Changed tab, sortable headers, detail drawer per row,
  arrow-key navigation, `/` to search, `1`-`5` to switch tabs, download the filtered rows as CSV.
- Only differing cells are stored; the payload is gzip+base64 and decoded natively by the
  browser. A report with 50 000 changed rows is typically 1-3 MB and opens in well under a second.
- The grid is virtualised: it renders the visible ~40 rows, so 50 000 rows scroll like 50.
- Dark mode follows the OS; works on a phone.

The 1M benchmark payload produces 60,049 changed rows; the report caps the embedded list at 50,000
and still weighs 1.1 MB. Use `--export-dir` when the full list matters — the counts in the report are
always exact regardless of the cap.

## Running it in GitHub

| Workflow | Trigger | What it does |
|---|---|---|
| `ci.yml` | push, PR | pytest, a 10k smoke comparison, and a check that the report has no external references |
| `parity.yml` | push, PR | every implementation must return identical counts and column stats, and all five data generators must emit byte-identical files |
| `benchmark.yml` | manual, weekly cron | generates 10k / 1M / 10M rows × 20 columns, compares, enforces time and memory budgets, uploads reports, writes a results table to the job summary |
| `compare.yml` | manual, or `workflow_call` | compares two files given as repo paths or URLs and publishes the report as an artifact |

```bash
gh workflow run Benchmark -f scales=all -f engine=duckdb
gh workflow run "Compare CSVs" -f file_a=data/july.csv -f file_b=data/august.csv \
  -f key=account_id,txn_id -f ignore=updated_at -f fail_on_diff=true
gh run watch
```

Call the comparison from another workflow:

```yaml
jobs:
  nightly-reconciliation:
    uses: <owner>/csvdiff/.github/workflows/compare.yml@main
    with:
      file_a: https://internal.example/exports/ledger_prev.csv
      file_b: https://internal.example/exports/ledger_curr.csv
      key: account_id,txn_id
      ignore: updated_at
      options: "--trim --tolerance 0.005"
      fail_on_diff: true
```

Set the repository variable `PUBLISH_PAGES=true` to publish benchmark reports to GitHub Pages
instead of downloading artifacts.

## Working on it with Claude Code

```bash
cd csvdiff && claude
```

`CLAUDE.md` is loaded automatically and carries the invariants that are easy to break: both engines
must return identical counts, the result dict in `engine.py` is the API, the report stays a single
file with a sparse payload, and SQL identifiers go through `_q()` / `_lit()` rather than `repr`.

| Slash command | What it does |
|---|---|
| `/bench [10k\|1m\|10m]` | runs a scale and reports time, throughput, peak RSS against budget |
| `/compare <a> <b> <key> [flags]` | runs a comparison and summarises the discrepancies, flagging setup mistakes such as a non-unique key |
| `/ci-fix` | pulls the latest failing run's logs, reproduces locally, fixes the cause |

`.claude/settings.json` pre-approves the test, benchmark and `gh run` commands, asks before
`git push` or `gh repo create`, and keeps `csvdiff.toml` out of reach since it holds mailbox
settings. `.claude/skills/csvdiff-report/` covers changes to the HTML report specifically.

---

# Which engine at which size

First, the format, because it is worth more than the engine choice below it:

| Your input arrives as | Do this | Why |
|---|---|---|
| Parquet | compare it as Parquet, with the [`cpp/`](cpp/) port | 4.5x CSV at ten million rows, on a fifth of the bytes — [why](#reading-parquet-natively) |
| CSV, compared once | compare the CSV | converting costs more than the one comparison saves |
| CSV, compared again and again | convert once, then compare Parquet | the conversion pays back from about the fifth comparison |
| newline-delimited JSON | compare it as JSON, or as one side against a CSV | the two formats meet at the join; it is 1.4x slower than CSV and 2.4x the bytes |

Then the engine:

| Your input | Use | Why |
|---|---|---|
| Up to ~100k rows | anything | every engine finishes well under a second; startup cost dominates, so pick on convenience |
| 100k – 2M rows | `polars` where you have it, else `turbo` | columnar wins this band outright; the byte-level engines are close behind on a quarter of the memory |
| 2M – 20M rows, memory to spare | `turbo` | byte-level scanning; the only class that stays fast *and* still finishes at 10M+ |
| 2M+ and you can build C++ | the [`cpp/`](cpp/) port, `--threads 4` | same design across every core: 2.5x the JVM on the same box and 1.5 GB lighter — but counts and JSON only, no HTML report |
| 2M+ and you want the report too | Rust `turbo` | the same threading as the C++ port, and the only build here that produces the HTML report at that speed |
| Your input is JSON or Parquet | any of the three native ports | all three read CSV, newline-delimited JSON and Parquet natively; either side of the comparison may be in any of the three formats |
| A Parquet pair | any of the three native ports | a Parquet-to-Parquet comparison never reconstructs a row: it joins on the key columns and then compares whole columns as integers |
| 2M+ and you prefer Zig | the [`zig/`](zig/) port | same design and threading, lowest memory of anything here — counts and JSON only |
| Any size, memory constrained | `sortmerge` | spills to disk — 3.68 GB of CSV compared in 208 MB in the Rust port |
| Larger than tested, or unknown | `sortmerge` | the only engine whose memory does not grow with the input |
| You need a hard guarantee | Zig port, `--max-memory MB` | a `FixedBufferAllocator`, so the bound is enforced rather than hoped for |

**The winner genuinely flips with scale**, which is why there is no single recommendation. Go's
row-at-a-time engine wins at 10k on startup cost alone, Polars wins at 1M on columnar throughput, and
at 10M the byte-level engines win because they are the only ones still standing. Any single "fastest
implementation" claim would be wrong at two sizes out of three.

**Above roughly 2M rows the question stops being speed and becomes memory.** TypeScript `polars` is
the fastest thing measured at 1M and cannot run 10M at all. The engines that survive are the ones
that never build a string per cell.

---

# Benchmarks

## How to read these

Every number is measured, none is projected. But they come from **three different hosts**, and mixing
them would be the easiest way to publish a lie:

| Set | Host | What it measures | Method |
|---|---|---|---|
| **A** | GitHub runners, 4 vCPU / 16 GB | five language ports, nineteen engines | single runs — treat <5% as noise |
| **B** | one 4-core / 16 GB container | one design, six toolchains, plus JVM execution modes | best of 3 (10k, 1M), best of 2 (20M) |
| **C** | one 4-core / 16 GB container | the external-tool survey | median of 3 (10k, 1M), of 2 (10M) |

**Compare rows within a table, never across tables.** Sets B and C ran on an otherwise idle machine
with the page cache warmed before anything was timed; Set A did not.

All sets use **byte-identical input** from `scripts/gen_data.py` — 20 columns keyed on
`(account_id, txn_id)`, `--ignore updated_at`, with a known drift recipe (see
[Test payloads](#test-payloads)).

Cells read `time · peak RSS`. Peak RSS is the process tree's high-water mark; for the memory-mapping
engines it **includes the mapped input files**, which is why Set B carries a separate "above the
mapped files" table.

Input sizes: 10k = 3.7 MB · 1M = 368 MB · 10M = 3.68 GB · 20M = 7.36 GB.

## Set A — five languages, nineteen engines

| Engine | 10k | 1M | 10M |
|---|---|---|---|
| **Byte-level, bespoke** | | | |
| Java `shard` — Vector API, all cores | 0.88s · 116 MB | 3.89s · 666 MB | **25.33s** · 5,415 MB |
| Java `turbo` — SWAR, all cores | 0.81s · 109 MB | 4.02s · 650 MB | 25.41s · 5,396 MB |
| Java `mmap` — Vector API, one thread | 0.67s · 122 MB | 4.37s · 644 MB | 31.52s · 5,280 MB |
| Java `swar` — SWAR, one thread | 0.78s · 110 MB | 4.07s · 622 MB | 32.60s · **5,270 MB** |
| Java `simd` — Vector API, on the heap | 0.70s · 128 MB | 3.61s · 908 MB | ✗ heap OOM at 2.9s |
| **Row-at-a-time, bespoke** | | | |
| Go `native` | **0.04s** · 44 MB | 6.35s · 1,935 MB | 161.67s · 15,174 MB |
| Java `native` | 0.64s · 138 MB | 6.26s · 3,006 MB | ✗ heap OOM at 28.0s |
| TypeScript `native` | 0.20s · 124 MB | 9.41s · 3,241 MB | ✗ V8 512 MB string cap at 1.1s |
| Rust `native` | 0.08s · **38 MB** | 12.22s · 2,896 MB | ✗ runner killed (OOM) |
| **Polars** | | | |
| TypeScript `polars` | 0.19s · 142 MB | **2.30s** · 2,507 MB | ✗ runner killed (OOM) |
| Rust `polars` | 0.05s · 64 MB | 2.57s · 2,289 MB | ✗ runner killed (OOM) |
| **DuckDB** | | | |
| Python `duckdb` | 0.51s · 175 MB | 11.57s · 2,395 MB | 120.97s · 9,906 MB |
| TypeScript `duckdb` | 0.52s · 238 MB | 11.86s · 2,691 MB | 122.11s · 10,145 MB |
| Rust `duckdb` | 0.36s · 114 MB | 11.80s · 2,087 MB | 122.32s · 9,423 MB |
| Java `duckdb` | 1.33s · 244 MB | 12.80s · 2,394 MB | 128.34s · 8,851 MB |
| Go `duckdb` | 0.52s · 192 MB | 21.04s · 2,271 MB | 179.02s · 9,560 MB |
| **Other libraries** | | | |
| Java `tablesaw` | 1.31s · 183 MB | 8.53s · 2,054 MB | ✗ heap OOM at 64.3s |
| TypeScript `arquero` | 0.34s · 152 MB | 17.22s · 3,487 MB | ✗ V8 512 MB string cap at 0.5s |

**Nine of nineteen engines do not finish at 10M.** That is where the design decisions show, and it is
why the recommendation table turns on memory rather than speed.

**`shard`'s win at 10M does not survive to 20M.** The 0.08s between `shard` and `turbo` here is
noise on single runs; measured properly at twice the size, `turbo` is 19% ahead. See
[SWAR, and how it compares to real SIMD](#swar-and-how-it-compares-to-real-simd).

**10k measures process startup, not comparison.** A JVM costs about half a second before it reads a
byte, which is essentially the whole spread in that column.

**Memory decides 10M, not speed.** Java `swar` uses 5.3 GB where Go `native` uses 15.2 GB for the
same job. That is the "no string per cell" design, and it is what buys the win — Go's engine is
faster than Java's at every size it survives, and it is the memory that ends it.

**The same library differs by binding.** DuckDB at 10M: Python 120.97s, TypeScript 122.11s, Rust
122.32s, Java 128.34s, Go 179.02s. Identical C++ engine doing identical work; Go's cgo binding costs
about 48% over Python's.

**Scaling from 1M to 10M is not linear, and the direction is informative:**

| Engine | 1M → 10M | Factor |
|---|---|---|
| Java `shard` | 3.89s → 25.33s | **×6.5** (sub-linear) |
| Java `turbo` | 4.02s → 25.41s | ×6.3 |
| Java `mmap` | 4.37s → 31.52s | ×7.2 |
| Java `swar` | 4.07s → 32.60s | ×8.0 |
| Go `duckdb` | 21.04s → 179.02s | ×8.5 |
| Java `duckdb` | 12.80s → 128.34s | ×10.0 |
| Rust `duckdb` | 11.80s → 122.32s | ×10.4 |
| Python `duckdb` | 11.57s → 120.97s | ×10.5 |
| Go `native` | 6.35s → 161.67s | **×25.5** (super-linear) |

The parallel Java engines go *sub*-linear — thread setup is amortised and JVM startup stops
mattering. Go `native` goes super-linear: at 15.2 GB peak it spends most of the run fighting the
garbage collector.

## Set B — one design, six toolchains

Set A varies language *and* design at once. This set fixes the design — the same byte-level engine,
described under [The byte-level design](#the-byte-level-design) — and varies only the toolchain.

**Every entry computes which cell changed.** The counts-only SQL joins from Set C are excluded on
purpose: at this size the comparison worth making is between things doing the same work. Everything
that finished agrees exactly, counts and per-column stats alike.

| Build | Threads | 1M | 20M |
|---|---|---|---|
| C++, clang 20 | 4 | **1.72s** · 514 MB | **33.32s** · 9,012 MB |
| C++, clang 20 | 2 | 2.57s · 512 MB | 54.60s · 8,574 MB |
| Zig 0.17-dev | 2 | 2.72s · 414 MB | 63.30s · **8,446 MB** |
| Zig 0.16 | 2 | 3.13s · 414 MB | — |
| C++, clang 20 | 1 | 3.57s · 509 MB | 94.74s · 8,573 MB |
| C, clang 20 | 1 | 3.57s · 417 MB | 85.93s · 8,447 MB |
| C++, clang 18 | 1 | 3.62s · 509 MB | 89.21s · 8,573 MB |
| Zig 0.17-dev | 1 | 4.47s · 414 MB | 117.92s · 8,446 MB |
| C, gcc 14 | 1 | 5.27s · 417 MB | 129.46s · 8,447 MB |
| Rust `turbo` | 1 | 5.33s · 583 MB | 104.07s · 8,677 MB |
| Zig 0.16 | 1 | 5.37s · 414 MB | 130.50s · 8,446 MB |
| Java 26 `turbo`, HotSpot | 4 | 5.54s · 628 MB | 83.79s · 10,499 MB |
| C++, g++ 14 | 1 | 5.73s · 509 MB | 129.41s · 8,573 MB |
| C++, g++ 13 | 1 | 6.33s · 509 MB | 136.29s · 8,573 MB |
| Java 25 `turbo`, Graal JIT | 4 | 7.98s · 777 MB | 114.33s · 10,720 MB |
| GraalVM native-image, Serial GC | 1 | 90.62s · 481 MB | ✗ did not finish |
| GraalVM native-image, G1 GC | 1 | 107.00s · 768 MB | ✗ did not finish |

The **Threads** column is there because an earlier version of this table did not
have it, and the omission produced a wrong conclusion. Java's `turbo` builds its
index on every core; the C, C++, Zig and Rust ports were strictly
single-threaded, which `cpu/wall` over the whole run makes unarguable — C++
1.00x, C 1.00x, Zig 1.00x, Rust 1.00x, Java `turbo` 1.54x. Comparing those
directly measured the thread count and called it a language. The C++ and Zig
ports are now threaded too, and the rows say which is which.

At 10k every native build is 0.05-0.10s and every JVM one 0.95-1.66s; that
column measured process startup rather than comparison, so it is dropped here
and left in [Set A](#set-a--five-languages-nineteen-engines) where startup is
the point.

What the engine actually allocates, with the mapped inputs subtracted:

| Build | 1M (351 MB mapped) | 20M (7,018 MB mapped) |
|---|---:|---:|
| Zig, either version or thread count | **63 MB** | **1,428 MB** |
| C, either compiler | 67 MB | 1,429 MB |
| native-image, Serial GC | 130 MB | — |
| C++, one thread | 158 MB | 1,555 MB |
| C++, threaded | 161 MB | 1,556 MB |
| Rust `turbo` | 232 MB | 1,659 MB |
| Java 26 `turbo`, HotSpot | 277 MB | 3,482 MB |
| Java 25 `turbo`, Graal JIT | 426 MB | 3,703 MB |

**Given the same cores, the native builds win comfortably.** C++ on four threads does 20M in 33.32s
against Java's 83.79s — **2.5x** — on 9.0 GB against Java's 10.5. Zig on two threads manages 63.30s.
Single-threaded C++, at 94.74s, is within 13% of Java's four-core engine.

**Java's `turbo` gets 1.18x cpu/wall at this size; C++ on four threads gets 2.53x.** Both name
themselves parallel, and the gap between those two figures is most of the gap in the wall times. How
the C++ port got there — and the measurement that redirected the work halfway — is under
[Using more than two cores](#using-more-than-two-cores).

These runs vary more than the smaller ones: the same single-threaded binary measured 84.03s in one
sitting and 94.74s in another, a 12% spread, so read differences under about 15% here as noise. The
2.5x is not one of them.

The Java row is `turbo`, the right choice at this size: `shard`, which edged `turbo` at 10M on the
Set A runners, is **19% slower at 20M**. The full four-engine matrix is under
[SWAR, and how it compares to real SIMD](#swar-and-how-it-compares-to-real-simd).

**The compiler moves more than the language does.** At 1M the fastest and slowest builds are both
C++, from the same source with the same flags, **1.8x apart**. clang beats gcc by 1.6x on C++ and
1.5x on C. Any comparison of C against Rust against Zig that does not name the compiler behind each
binary is reporting the toolchain and calling it the language — which includes Set A, where the Rust
and Go numbers come from whichever toolchain the runner had.

**A newer compiler is worth real time for free.** clang 20 over clang 18, g++ 14 over g++ 13, and
Zig 0.17-dev over 0.16 — **17% at 1M and 11% at 20M**, the largest single-version gain measured
anywhere here. None of it required touching the source.

**The memory floor belongs to the design, not the language.** The C port was written specifically to
find the floor and did not find one: 67 MB above the mapped files against Zig's 63 MB at 1M, and
1,429 against 1,428 at 20M. Once the row index, the offset array and the hash table are sized the
same way, there is nothing left for a language to save. Rust's and C++'s larger figures are not
runtime overhead either — they are the same structures sized more generously.

**The memory ordering is stable at every scale**: Zig lowest, C a few megabytes behind, C++ next,
Rust after that, the JVM last by a wide margin. Whatever else changes with scale, this does not.

**Technique, not language, was most of what Set A measured.** Set A shows Java's byte-level `turbo`
at 4.02s on a million rows and Rust's row-oriented `native` at 12.22s — 3x apart, in a comparison
people would read as "Java beat Rust". Give Rust the same design and the question disappears.
Measured back to back on one host: Rust `native` 21.97s · 2,896 MB against Rust `turbo` 5.22s ·
583 MB. **4.2x faster on a fifth of the memory — same language, same compiler, same binary**, with
only the design changed. Whatever Set A is measuring, most of it is not the language.

Earlier 10M runs of four of these builds, same host, before the newer toolchains existed: C++
clang 18 37.8s · 4,334 MB · 655 MB above; Rust 49.1s · 4,412 MB · 733 above; Zig 0.16 59.9s ·
4,224 MB · 545 above; C gcc 13 61.2s · 4,224 MB · 545 above. The 10M column is missing from the
table only because that dataset was deleted to make disk room for 20M; it regenerates
deterministically.

### One jar, four execution modes

Set B runs Java on HotSpot and Java on the Graal JIT with *different bytecode* — the GraalVM build
targets release 25 because GraalVM for JDK 26 was not out — so on its own it cannot separate the
compiler from the class-file version. This does. Same jar, same engine, same input, at 1M, as its own
run (so read it against itself, not against the table above):

| Execution mode | Compare | Peak RSS |
|---|---:|---:|
| HotSpot C2, Java 25 jar | **5.25s** | 633 MB |
| C2 inside GraalVM (`-XX:-UseJVMCICompiler`) | 5.26s | 663 MB |
| HotSpot C2, Java 26 jar | 5.45s | 631 MB |
| Graal JIT | 8.71s | 785 MB |
| native-image, Serial GC (the default) | 93.11s | **484 MB** |
| native-image, G1 GC (`--gc=G1`) | 108.28s | 720 MB |

**The class-file version is noise.** 5.25s against 5.45s is inside run-to-run variation, so Set B was
not confounded after all — it really is measuring the compiler.

**The Graal JIT is 1.66x slower than C2 on this workload**, and switching JVMCI off *inside GraalVM*
recovers C2's time to the hundredth of a second. So it is the compiler, not the distribution, not the
JDK. Graal earns its reputation on abstraction-heavy code that needs aggressive inlining and escape
analysis; this engine is a tight loop reading memory-mapped bytes, which is where C2 has had two
decades of tuning.

**Ahead-of-time compilation costs 17-21x here, and buys the lowest memory on the JVM.** native-image
produces correct answers and the smallest peak RSS in the set — 484 MB, under C2's 631 MB. It is also
seventeen times slower with the default collector and twenty-one with G1, enough to swamp its one
structural advantage: at 10k rows, where startup should dominate and a JIT has no time to warm up,
the whole process still takes 1.16s against HotSpot's 0.95s. There is no size in this project at
which it comes out ahead on time.

Why is a hypothesis rather than a measurement — this was not profiled. But the shape of the workload
points somewhere specific: the engine does almost nothing except read through `MemorySegment`s backed
by a shared `Arena`, so if the access path HotSpot intrinsifies has a weaker ahead-of-time
counterpart, a program made entirely of those reads would pay for it on every one. A program that
spent its time elsewhere would not.

At 20M the ratio makes the run impractical. With a correctly built binary and a bounded heap it was
stopped after 10 minutes, against 82 seconds on HotSpot; earlier attempts under Serial and G1 were
stopped at 30 and 21 minutes, and the first — before the heap was bounded — was killed by the kernel,
because native-image sizes its heap from physical RAM without knowing about the 7 GB of files the
engine is about to map. The row says "did not finish" rather than carrying an extrapolation.

Getting a working image at all took three builds, which is part of the answer too:

- `Arena.ofShared` is refused unless the image is built with `-H:+SharedArenaSupport`.
- Jackson needs reachability metadata for the result records. The tracing agent supplies it, but only
  for the paths the traced run actually took — tracing the small fixture missed an array type that a
  real comparison serializes, and the failure surfaced only at the end of a long run.

```bash
/opt/graalvm/bin/java -agentlib:native-image-agent=config-output-dir=cfg \
  -jar csvdiff.jar compare a.csv b.csv -k id --engine turbo -o out.html --json out.json
native-image -jar csvdiff.jar -o csvdiff-native --no-fallback -march=native -O2 \
  -H:+UnlockExperimentalVMOptions -H:+SharedArenaSupport -H:ConfigurationFileDirectories=cfg
```

## How the fastest build scales

Sets A to C compare engines against each other at fixed sizes. This is the other
question: one engine, one thread count, every size from ten thousand rows to fifty million —
the C++ port on four threads, which is the fastest thing measured here.

Fifty million rows is 17.5 GB of input, more than the disk and the RAM of the machine that ran it,
so each pair is generated, measured and deleted in turn (`scripts/bench_scale.py`).

| Rows | Input | Wall | Rows/s | cpu/wall | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| 10k | 4 MB | 0.03s | 394,000 | 1.38x | 12 MB | 8 MB |
| 1M | 351 MB | 1.83s | 548,000 | 2.27x | 514 MB | 164 MB |
| 10M | 3,509 MB | 16.66s | **600,000** | 2.54x | 4,462 MB | 953 MB |
| 20M | 7,018 MB | 33.53s | 596,000 | 2.45x | 9,041 MB | 2,023 MB |
| 50M | 17,544 MB | 126.85s | 394,000 | 2.25x | 13,435 MB | *evicted* |

Counts at every size match the drift recipe exactly — at 50M, 2,994,637 changed, 50,000 added,
50,000 removed, 5,000 and 2,500 duplicate keys.

**Throughput is flat from 1M to 20M** — 548k, 600k, 596k rows a second. Twenty times the data for
twenty times the time, with no penalty for size. The 10k row is startup, not comparison.

**Then it falls 34% at 50M**, and the reason is in the last column. The two files are 17.5 GB and the
machine has 16 GB, so the page cache cannot hold them: pages are read, evicted and read again. This
is the first size in this project where the input does not fit in RAM, and the cost of that is what
the drop measures — not anything about the algorithm.

**At 50M the process is smaller than the files it is comparing.** Peak RSS is 13,435 MB against
17,544 MB of input, which is why the "above" column has nothing to report: the kernel reclaimed
mapped pages the engine had already passed. A design that read the files into memory would have
needed 17.5 GB and failed; this one uses whatever is left and keeps going. That is the same property
`sortmerge` is built for, arrived at by mapping rather than by spilling.

**Memory above the input grows linearly with rows** — 164 MB at 1M, 953 at 10M, 2,023 at 20M, so
about 100 MB per million rows. That is the row index, the offset array and the hash table, and it is
what sets the real ceiling: at a billion rows it would want roughly 100 GB, which is where
`sortmerge` stops being the conservative choice and becomes the only one.

**Generating the data got its own fix, twice.** The ladder needs 29 GB of input across five sizes,
and the Python generator writes a million rows in 30.5s — around 25 minutes for 50M alone. All the
generators emit byte-identical files (`parity.yml` enforces it), so the harness is free to pick on
speed. Go was the first answer; a threaded C++ one ([`cpp/tools/gen_data.cpp`](cpp/tools/gen_data.cpp))
is the last one worth having:

| Generating 10M rows (3.7 GB) | Time | vs the disk |
|---|---:|---:|
| Python | ~305s | 18x |
| Go | 43.0s | 2.6x |
| **C++, four threads** | **17.7s** | **1.07x** |
| `dd` writing the same bytes | 16.6s | 1.00x |

Every row is a pure function of its index, so rows can be formatted without seeing any other row.
They are, in waves: N threads fill their own buffers, the buffers are written in order, and memory
is bounded by the wave rather than the file. At 17.7s against `dd`'s 16.6s there is nothing left to
win — generation is now pure I/O.

## Set D — ten million rows on a hosted runner

Sets A to C were run before three changes that moved every number in them: the
key hash, the field store, and the join's second hash. This set is the state of
the three native ports afterwards, and it runs on `ubuntu-latest` through
[`bench-10m.yml`](.github/workflows/bench-10m.yml) rather than on a container
here — a hosted runner is the one host anyone reading this can rent for nothing.

Every payload is written by this project's own generator, so the CSV and the
Parquet hold the same values spelled the same way; Parquet is uncompressed,
because this is a question about readers and a codec in the middle answers a
different one. Ten million rows, best of two, four threads, counts identical in
every row of the table.

**Every row carries CPU time as well as wall time, and the ratio of the two.**
CPU divided by wall is how many of the runner's four cores were actually busy,
and it is the column that says *why* a row is where it is. Two builds at the
same wall time, one at 1.4x cores and one at 3.6x, are not the same result: the
first has headroom the second has already spent, and the fix for each is a
different fix. A row that is slow at 1.4x is not using the machine; a row that
is slow at 3.9x is using all of it and needs less work, not more threads. Both
numbers come from the same `wait4` rusage as the peak RSS beside them.

| Build | Input | Compare | Rows/s | Peak RSS | Above the input |
|---|---|---:|---:|---:|---:|
| C++ | CSV, 3,509 MB | **8.98s** | 1,113,348 | 4,385 MB | 876 MB |
| Zig | CSV | 10.38s | 963,198 | 4,391 MB | 882 MB |
| Rust | CSV | 10.39s | 963,012 | 4,443 MB | 935 MB |
| Rust, engine only | CSV | 9.17s | 1,090,193 | 4,428 MB | 920 MB |
| C++ AVX2 | CSV | **8.33s** | 1,201,185 | 4,390 MB | 881 MB |
| Zig AVX2 | CSV | 9.64s | 1,037,543 | 4,432 MB | 923 MB |
| Rust | Parquet, 1,535 MB | 8.33s | 1,200,126 | 8,415 MB | 6,880 MB |
| Zig | Parquet | 9.13s | 1,095,262 | 8,278 MB | 6,743 MB |
| Rust, engine only | Parquet | 7.12s | 1,404,186 | 8,440 MB | 6,906 MB |
| Zig AVX2 | Parquet | 9.08s | 1,101,023 | 8,275 MB | 6,740 MB |

Every row returns `matched 9,990,000 · changed 599,320 · added 10,000 ·
removed 10,000`, and the harness refuses to print the table if any row disagrees.
[The run.](https://github.com/andrey-usa/csvdiff/actions/runs/34188270810)

**Rust and Zig now match the C++ port**, which is what this branch set out to
find out. The three are within 16% of each other on CSV, having started three
times apart.

**Two rows are not comparing like with like, and say so.** The Rust port renders
the HTML report; the C++ and Zig ports produce counts and JSON only. `Rust,
engine only` is the same comparison with `--max-rows 1`, which is the row to read
against the other two ports. The difference between its rows — about a second at
this size — is the report: fifty thousand changed rows decoded into strings,
sorted, gzipped and embedded.

**The Parquet rows are superseded and kept for the comparison they lose.** They
measure a reader that decodes the pages into *rows* and hands them to the same
byte-level engine the CSV rows use — the honest way to read Parquet if you want
one reader for `a.parquet` against `b.csv`, and the wrong way if both sides are
Parquet. While this branch was measuring it, `main` landed a columnar path that
never reconstructs a row: it joins on the key columns and then compares whole
columns as `int32` ids. That is 2.35s and 3.5 GB on the same rows against 8.33s
and 8.4 GB here — [the design is described
below](#the-design-never-reconstruct-a-row) — so it is now the Parquet path in
all three ports, and the row-materialising reader survives only as the fallback
for the mixed case it was the only answer to. **The next run of this table
re-measures those rows**; they are left in place rather than deleted because
"the fastest input and the largest memory" was a real finding about a real
design, and the reason it stopped being true is the interesting part.

**The vector scanner beats SWAR by about 7%**, in both ports, which reverses what
this repository said before the key hash was fixed. That story is under
[SWAR, and how it compares to real SIMD](#swar-and-how-it-compares-to-real-simd).

## Set C — against the field

Sets A and B compare this project with itself. That says which language and which technique is
faster; it does not say whether the design is any good, because every one of those engines was
written here. So the same comparison — same two files, same composite key, same ignored column — was
run through the tools people actually reach for.

| Tool | What it is | Expresses this task? |
|---|---|---|
| **csvdiff (this project)** | bespoke, `turbo` and `sortmerge` engines | yes |
| **csvdiff (Go, aswinkarthik)** | the fastest dedicated CSV diff in wide use; xxHash of key and row | yes, but the answer is coarser |
| **DuckDB CLI** | a full outer join written by hand in SQL | yes |
| **clickhouse-local** | the same join, on the fastest CSV reader in the survey | yes |
| **daff** | the tabular-diff library behind `git daff`; alignment-based, `--id` pins a key | yes |
| **datacompy** (Capital One) | the reconciliation library, on Polars | yes |
| **csv-diff** (Simon Willison) | small, popular, row dicts | **no** — one key column, no column-ignore |
| **sort(1) + join(1)** | the shell pipeline | **no** — no idea what CSV quoting is |

Surveyed and not run: **data-diff** (Datafold) bisects checksums to compare tables across a network,
which is the wrong problem here — both files are already local, so there is nothing to avoid
transferring. **qsv** has no keyed diff subcommand of this shape.

| Tool | 10k | 1M | 10M | Answer |
|---|---|---|---|---|
| csvdiff, `turbo` | 0.91s · 102 MB | 6.26s · 664 MB | 44.77s · 6,217 MB | reference |
| csvdiff, `sortmerge` | 0.90s · 139 MB | 13.93s · 1,625 MB | 144.98s · **1,594 MB** | agrees |
| csvdiff (Go, aswinkarthik) | **0.05s** · 22 MB | **3.32s** · 1,519 MB | ✗ out of memory | agrees |
| DuckDB CLI (SQL) | 0.26s · 47 MB | 8.09s · 1,151 MB | **91.94s** · 8,746 MB | +7 changed, +1 removed |
| clickhouse-local (SQL) | 0.19s · 191 MB | **2.23s** · 1,417 MB | 152.57s · 8,345 MB | +7 changed, +1 removed |
| clickhouse-local (spilling join) | 0.22s · 197 MB | 6.71s · 1,954 MB | 507.21s · 6,711 MB | +7 changed, +1 removed |
| daff (JS) | 0.48s · 121 MB | 46.87s · 4,134 MB | ✗ V8 512 MB string cap | dup keys only |
| datacompy (Polars) | 0.79s · 178 MB | 5.64s · 2,685 MB | ✗ out of memory | +49 added, +99 removed |
| csv-diff (Python) | 0.34s · 67 MB | 25.26s · 3,420 MB | ✗ out of memory | agrees |
| sort(1) + join(1) | 0.11s · **4 MB** | 10.58s · 251 MB | 118.47s · 2,483 MB | +7 changed, +1 removed |

Times alone hide what each tool gives you for them:

| Tool | Cell-level diff | Duplicate-key report | Report file |
|---|---|---|---|
| csvdiff (this project) | yes | yes | self-contained HTML |
| csvdiff (Go) | no — row hash only | no | no |
| DuckDB / ClickHouse SQL | no — counts only | no | no |
| daff | yes | no — a repeat reads as an insert | diff table |
| datacompy | yes, plus per-column summary | no — pairs duplicates positionally | text summary |
| csv-diff | yes | no | no |
| sort + join | no — counts only | no | no |

**At ten million rows most of the field cannot run at all.** Four of the eight external entries fail
on 3.68 GB in a 12 GB budget. The interesting thing is not that they are slow — it is that "fastest"
stops being the question.

**daff's ceiling is not memory.** It reads the file with `readFileSync`, and V8 refuses to build a
string longer than 512 MB. No amount of RAM moves that limit, so daff cannot open a file this size on
any machine. Every other failure above is genuine memory exhaustion; this one is a wall.

**The dedicated tool is faster and hungrier.** csvdiff (Go) finishes 1M in 3.32s against our 6.26s
and uses 1,519 MB against our 664 MB — while storing strictly less: two hashes per row, which is why
it can say a row changed but not which cell. Then at 10M it runs out of memory and we do not.

**The fastest SQL has a cliff, and the way down costs 3.3x.** clickhouse-local does 1M in 2.23s — the
fastest anything in this table, ours included, and 3.6x quicker than the DuckDB CLI. Then the shape
of the query stops working: it loads both files into memory and joins with a hash, so at 10M it
either takes 152.57s and 8,345 MB or aborts on its own memory limit, depending on how much RAM the
machine happens to have free when it starts. Rewritten to stream from `file()` with
`join_algorithm = 'partial_merge'` — a spilling sort-merge join, the same algorithm as our
`sortmerge` engine — it finishes reliably at 507.21s and 6,711 MB. That is 3.5x this project's
`sortmerge` in time and 4.2x in memory, for counts rather than a diff.

One caveat, because it changes what that row means: the spilling variant is the only entry measured
outside the 12 GB address-space cap. Under `RLIMIT_AS` it does not report a memory error, it
segfaults — ClickHouse reserves far more address space than it makes resident, so the cap fires on a
mapping rather than on real use. Capped, the table would have recorded the instrument instead of the
tool. Its measured 6,711 MB is well inside the budget every other row was held to.

**The shell pipeline is better than it has any right to be.** `sort | join` does 1M in 10.58s and
251 MB, and 10M in 118.47s and 2,483 MB — quicker there than either ClickHouse variant and on less
memory than anything else that finished, because `sort` spills. It is the right instinct: an external
sort-merge join is the correct algorithm for data larger than memory. What it cannot do is parse CSV
— a comma, a quote or a newline inside a field and the answer is silently wrong — or tell you which
cell changed. That instinct is why the `sortmerge` engine exists.

---

# Techniques

## The byte-level design

Shared by Java's `turbo` / `swar` / `shard` / `mmap`, Rust's `turbo`, and the C, C++ and Zig ports.
It is the configuration the One Billion Row Challenge entries converged on, applied to a keyed
comparison:

1. **Map both files** into the address space (`mmap`, or the FFM API on the JVM) with a sequential
   access hint. No read buffer, no copy.
2. **A field is one 64-bit word** — 40 bits of offset, 23 bits of length, and the top bit set when
   the field contains a doubled quote and must be unescaped before it is read. Two sentinels:
   all-ones for absent, all-ones-minus-one for a field too long to pack.
3. **Find delimiters eight bytes at a time** with SWAR (below).
4. **Open-address the keys** in a table sized to a power of two, first-occurrence-wins, so duplicate
   keys are counted rather than joined.
5. **Nothing becomes a string** except the column names in the summary.

The consequence that matters at scale: memory grows with the *number of rows*, not with the bytes in
them. That is what "no string per cell" buys, and it is why this class is the only one still standing
at 10M in Set A.

## SWAR, and how it compares to real SIMD

**SWAR** — SIMD Within A Register — finds a byte within eight bytes using ordinary 64-bit
arithmetic:

```
diff  = word ^ broadcast(target)             // zero bytes where the byte matches
hits  = (diff - 0x0101…) & ~diff & 0x8080…   // high bit set in each matching byte
index = trailing_zeros(hits) >> 3            // which byte it was
```

No intrinsics, no CPU feature detection, no incubator module — just integer ops every target already
has.

Java implements both techniques over the same parser, which turns the comparison into a controlled
experiment. The five byte-level engines are a design matrix:

| | one thread | all cores |
|---|---|---|
| **Vector API** (real SIMD) | `mmap` | `shard` |
| **SWAR** (bit trick) | `swar` | `turbo` |

plus `simd` — the Vector API reading from the **heap** instead of a mapping, which isolates mapping
from vectorising.

**They tie at 10M and SWAR wins at 20M.** Running the matrix at both scales, with the incubator
module added for every run so the comparison is not confounded by it:

| | Vector API | SWAR | |
|---|---|---|---|
| 10M, all cores *(Set A runners)* | `shard` 25.33s | `turbo` 25.41s | tie — 0.3% |
| 10M, one thread *(Set A runners)* | `mmap` 31.52s | `swar` 32.60s | Vector by 3% |
| **20M, all cores** *(Set B host)* | `shard` 96.26s | `turbo` **80.59s** | **SWAR by 19%** |
| **20M, one thread** *(Set B host)* | `mmap` 119.53s | `swar` **106.91s** | **SWAR by 12%** |

These four are their own run, so the `turbo` figure here (80.59s) and the one in the Set B table
(85.90s) are two sittings of the same thing, about 6% apart — which is roughly the spread measured
below. Compare within each table.

At 10M the honest reading was a tie: 0.3% on single runs on shared runners is noise, and the
one-thread pairing pointed the other way. At 20M, best-of-two on an idle machine, SWAR wins both
pairings by margins well outside the run-to-run spread — which for these runs is about 5%, measured
by running `turbo` with and without the vector module on the classpath (80.59s against 84.87s, a
difference it cannot causally have, since `turbo` never calls the Vector API).

*Why* it reverses is not established: the scale and the host both changed between the two pairs of
rows, so this does not isolate which is responsible. What it does settle is that "the two techniques
cost the same" is not true at every size, and that the engine shipping as the default is the right
one at the largest size tested.

Peak RSS at 20M, where the two techniques are within 20 MB of each other in every pairing: `shard`
10,519 MB, `turbo` 10,500 MB, `mmap` 10,027 MB, `swar` 10,037 MB, against 7,018 MB of mapped input.
All four return `changed 1,197,876 · added 20,000 · removed 20,000`, matching the C, C++, Zig and
Rust ports exactly.

**Real SIMD does not help the native ports either, and that was tested five ways.** The obvious move
is to replace SWAR's eight bytes per step with a vector register's thirty-two or sixty-four. In C++,
on a machine with AVX-512, every variant was *slower* than plain SWAR at 1M:

| Scanner | 1M | vs SWAR |
|---|---:|---:|
| SWAR, 8 bytes per step | **3.76s** | — |
| AVX2 throughout, 32 bytes | 3.92s | +4% |
| AVX-512 throughout, 64 bytes | 4.35s | +16% |
| One SWAR step, then AVX2 | 4.17s | +11% |
| One SWAR step, then AVX-512 | 4.38s | +16% |
| The same, with the escalation out of line | 4.71s | +25% |

The reason is the data, not the instruction set: **fields in this file average 9.2 bytes**, so a
single SWAR step usually finds the delimiter, and a 64-byte load to answer a 9-byte question reads
eight times the memory it needs. Trying to escalate only on long runs did not rescue it either —
with a 9.2-byte mean, the first step misses about half the time, so the escalation is not the rare
branch that design assumes. Nor is it AVX-512 frequency licensing: AVX2 alone still lost. Restricting
the compare to one column, which leaves a ~120-byte tail to skip per row, still lost (2.55s against
2.22s).

This is the same conclusion the JVM reached by a completely different route, and the agreement is
worth more than either result alone: real SIMD through the Vector API lost to SWAR by 19% at 20M, and
real SIMD through AVX2/AVX-512 intrinsics loses to SWAR by 4-25% in C++. **For CSV of this shape the
scan is not the place to spend a vector register.**

### That conclusion did not survive fixing the hash

It was true of the engine that measured it, and the engine changed. The key hash
was FNV-1a a byte at a time over a twenty-six-byte key, computed four times a row
— both files, indexed then probed — which is about a billion dependent
multiply-xor steps at ten million rows. A hash is internal, so the only property
it owes anyone is that the index build and the join probe agree about it; taking
eight bytes at a time keeps that. Two of those four hashes then went away
entirely, because the sweep had already stored the answer the join was
recomputing. At a million rows on one container, best of three: C++ 1.76s → 0.94s,
Zig 1.91s → 0.89s, Rust 3.22s → 1.58s.

**With the hash no longer the bottleneck, the scan is a larger share of what is
left, and the vector register wins.** At ten million rows on a hosted runner —
one binary per scanner, so nothing is measuring a branch:

| Build | Scanner | 10M CSV | Rows/s |
|---|---|---:|---:|
| C++ | AVX2, 32 bytes | **8.33s** | 1,201,185 |
| C++ | SWAR, 8 bytes | 8.98s | 1,113,659 |
| Zig | AVX2, 32 bytes | **9.64s** | 1,037,543 |
| Zig | SWAR, 8 bytes | 10.38s | 963,198 |

**7% either way, in two implementations that share no code.** That the two agree
on the size of it is worth more than either number: the same change, made
independently in C++ intrinsics and in Zig's `@Vector`, moved the same workload
by the same amount.

What this does *not* say is that the textbook was right all along. The earlier
table is not wrong — SIMD really did lose on that engine, and it lost because
fields average 9.2 bytes and a 32-byte load answers a 9-byte question. What
changed is everything else: with a billion multiply-xor steps removed, the same
wasted loads now sit on the critical path where they used to be hidden behind
the hash. **A scanner benchmark measures the engine around it**, which is the
part of this that generalises.

AVX-512 is unmeasured here rather than lost: the runners this ran on do not have
it, so the 64-byte builds were skipped by name. `make scanners` and
`zig build -Dscan=64 -Dcpu=native` build them for a machine that does.

**There are now two scanners, and the same question has to be asked of both.**
The columnar Parquet path has no delimiters to find, but it produces a mismatch
mask — one byte per compared cell — and reads it with the identical idiom. The
two workloads look nothing alike from the scanner's point of view:

| | CSV: find a delimiter | Parquet: find a changed cell |
|---|---|---|
| Hit rate | a field every 9.2 bytes | about 0.7% of cells |
| An 8-byte step is empty | never | 94.6% of the time |
| A 32-byte step is empty | never | 80% of the time |
| What a wider register saves | loads *and* branches | branches only — the mask is already in L1 |
| What it wastes | reading past the delimiter | nothing; the whole block is read either way |

On CSV a wider register reads memory it does not need, which is what the first
table above measured. On the mismatch mask there is no such waste — every byte
is examined either way — so the wider register is pure branch reduction, and the
prediction is that it should win by more there than it does on CSV. The scanner
builds are run against both formats for that reason; `bench_formats_ports.py
--matrix` prints the rows side by side.

**SWAR's other advantages are structural.** It needs no incubator module, so `turbo` is the fastest
engine that runs on a stock `java -jar` with no flags, and it uses slightly less memory because no
vector machinery is loaded. That is also why the C, C++, Zig and Rust ports all use SWAR: it is
portable in a way `--add-modules jdk.incubator.vector` is not.

**Four cores buy less than you would expect.** `turbo` against `swar` is 1.33x and `shard` against
`mmap` is 1.24x at 20M — well short of the 4x the core count suggests, because the scan is bound by
memory bandwidth rather than by instruction throughput. The single-threaded engines also finish in
about 470 MB less, which is the per-thread index and scratch structures the parallel ones allocate.

**Mapping matters more than vectorising.** `simd` (Vector API, heap) is the fastest Java engine at 1M
— 3.61s — and the *first* to die at 10M, 2.9 seconds in, because it holds the file on the heap.
`mmap` is the same vectorised scan over a mapping and survives. Between the two, the mapping is worth
more than the SIMD.

## Using more than two cores

The first threading here was structural — one thread per file, one per join direction — so it stopped
at two and left half a four-core box idle. Whether more would help turned on a question worth
answering before writing any code: **is the scan bound by memory bandwidth?** If it were, splitting
the work further would buy nothing.

It is not. Running N copies of the single-threaded binary at once, on the same warm input:

| Copies | Mean per copy | vs alone |
|---|---:|---:|
| 1 | 4.61s | 1.00x |
| 2 | 4.37s | 0.95x |
| 4 | 4.15s | **0.90x** |

Four copies each doing the whole job finish *faster* than one copy alone — four cores' worth of
throughput, with the small gain coming from the machine staying busy rather than idling between
runs. So the ceiling was the design, not the hardware.

**Chunked index building.** Each file is split at row boundaries, parsed and hashed in parallel, then
inserted into the table on one thread in file order. Insertion has to stay ordered: first occurrence
of a key wins, and the duplicate counts follow from that, so doing it in parallel would make the
answer depend on thread scheduling.

Finding the boundaries is the interesting part. A newline inside a quoted field is not a row
boundary, and a thread starting mid-file cannot tell whether it is inside one. **Quote parity settles
it:** every `"` toggles in-quote state — including both halves of a doubled quote, which toggles
twice and so correctly leaves the state alone — so the number of quotes before a position says
whether that position is inside a field. Counting them is a scan for one byte, far cheaper than
parsing, and it splits across the same threads. From the boundary, a two-state machine walks to the
first newline outside quotes; that is the row start.

**And it bought nothing.** Instrumenting the phases said why:

| Phase, per file at 1M | Time |
|---|---:|
| Sweep — parse and hash every row | 0.34s |
| Insert — build the table in order | 0.13s |
| *Everything else (the join)* | *~1.8s* |

The index build is about 20% of the run. Chunking it perfectly could not have moved the total much,
and the measurement is the only reason that was obvious rather than mysterious.

**The join was the long pole.** A's side — look up every distinct key in B, re-parse both rows,
compare every column — now splits over contiguous ranges of A's keys. Each range keeps its own
counts, column stats and capped row lists, merged in range order so the rows surviving `--max-rows`
are the same rows one thread would have kept.

| 20M rows | Wall | cpu/wall |
|---|---:|---:|
| No threading | 94.74s | 1.00x |
| Two threads (structural) | 54.60s | 1.71x |
| **Four threads (chunked + parallel join)** | **33.32s** | **2.53x** |

2.8x over single-threaded, and past four threads it gets slower again on a four-core box. It costs
memory — 2,055 MB above the mapped files against 1,556 for the two-thread build, which is the
per-chunk row and hash arrays — and `--threads 1` restores the old behaviour.

Correctness is checked where chunking can actually break it: a 5.4 MB file whose rows carry newlines
and doubled quotes inside quoted fields, above the 4 MB threshold where splitting turns on. Identical
counts at 1, 2, 3, 4 and 8 threads, matching the Rust port, the Python reference and the awkward
fixture.

**The Rust and Zig ports now do the same.** Both were single-threaded or structurally two-threaded
when the paragraph above was written; both now split each file into row-aligned chunks and A's side
of the join into ranges, with the same rule about what may not be reordered — the index insert stays
in file order, because first occurrence of a key wins and the duplicate counts follow from that, and
the join's ranges are merged in order so the rows surviving `--max-rows` are the rows one thread
would have kept. Each port's test suite asserts one answer at 1, 2, 3, 4 and 8 threads on the same
awkward file.

## Input formats: is CSV the problem?

CSV is text that has to be parsed a byte at a time. Parquet and JSON are the
obvious alternatives, so: ten million rows, both files, **the same comparison** —
first-occurrence-wins on the key, inner join, per-cell diff over seventeen
columns — with the engine held constant and only the input format changing.

DuckDB is the constant because it reads all of them natively and spills rather
than dying. Polars was tried first and was killed by the OOM killer at ten
million rows on CSV alone, twice.

**This section is the historical one**, kept because the reasoning in it is
instructive and because one of its conclusions turned out to be wrong. The
current answer, measured on our own reader and our own generator with no third
party involved, is at the top: [Four formats, one
engine](#four-formats-one-engine).

What follows is what the question looked like when the only way to read Parquet
here was to ask DuckDB. The C++ port now reads all three natively —
[JSON](#reading-json-natively) and [Parquet](#reading-parquet-natively) — and the
Parquet answer turned out to be the opposite of what the table below predicts for
us, for reasons a format table with a third-party engine in it cannot see.

| Engine and input | Size | Convert | **Compare** | Peak RSS |
|---|---:|---:|---:|---:|
| DuckDB, CSV | 3,509 MB | — | 54.78s | 12,372 MB |
| DuckDB, Parquet + zstd | 544 MB | 60.4s | 18.39s | 11,667 MB |
| DuckDB, Parquet uncompressed | 2,088 MB | 55.3s | **15.41s** | 11,772 MB |
| DuckDB, JSON (ndjson) | 8,487 MB | 108.6s | 17.08s | 11,260 MB |
| **ours (C++), CSV** | 3,509 MB | — | **12.34s** | **4,336 MB** |

Every row returns identical counts — matched 9,990,000, changed 599,320, added
10,000, removed 10,000 — so these are five ways of doing exactly the same work.

**For DuckDB the format is worth 3.6x.** CSV 54.78s against uncompressed Parquet
15.41s is a far bigger gap than the format is worth to us, because DuckDB spends
proportionally much more of its run parsing text.

**Converting costs about four times what it saves.** Writing the Parquet takes
55-60s to save 39s on the comparison. A one-off comparison is never worth
converting for; it only pays when the same file is compared repeatedly, which is
exactly the recurring-reconciliation case this tool is built for.

**JSON is fast here and slow elsewhere, which is a warning about single numbers.**
DuckDB compares ndjson in 17.08s, quicker than its own CSV path, despite the file
being 2.4x larger. Reading one file with polars, the ranking reverses — 9.56s for
ndjson against 4.17s for CSV. The format's cost is a property of the reader, not
of the format:

| Reading one 10M file with polars | Size | Full read | Six columns |
|---|---:|---:|---:|
| Parquet + zstd | 288 MB | 0.89s | 0.31s |
| Parquet, uncompressed | 966 MB | **0.56s** | **0.18s** |
| CSV | 1,755 MB | 4.17s | 1.89s |
| Arrow IPC | 3,537 MB | 1.01s | 0.34s |
| JSON (ndjson) | 4,244 MB | 9.56s | 5.46s |

Arrow IPC is worth a line of its own: every column here is a string, and offsets
plus data come to **twice what the text does**. It is fast to read and the largest
file in the table.

### Reading JSON natively

All three native ports read newline-delimited JSON as well as CSV, and the two
meet at the join — a CSV export compares against the JSON the same pipeline
emits, with the key order on each side free to differ.

| Input | Size | Compare | Rows/s | Peak RSS |
|---|---:|---:|---:|---:|
| CSV | 3,509 MB | **9.51s** | 1,051,760 | **4,371 MB** |
| JSON (ndjson) | 8,487 MB | 17.91s | 558,350 | 9,349 MB |

Identical counts from both. **JSON is 1.9x slower, and that is the format rather
than the reader**: it is 2.4x the bytes and every field carries its name. Against
DuckDB on the same ndjson — 17.08s, 11,260 MB — this is level on time and 17%
under on memory, where on CSV it is 5.8x faster. It is worth having for the input
people actually receive, not for speed.

(That table is the C++ port before the hash changes; the shape of the answer is
what matters and it has not moved — JSON is 2.4x the bytes and every field
carries its name.)

It fits because a JSON value is a contiguous run of bytes, so a field stays an
offset and a length into the mapping exactly as it does for CSV. The one new rule
is the escape: CSV doubles a quote, JSON puts a backslash in front of one. Both go
through `for_each_byte`, the single route every comparison reads a field through,
so hashing and equality cannot come to different conclusions about the same value.
`\uXXXX` and surrogate pairs are decoded rather than compared as written, because
a writer may emit a character either way and both spellings have to compare equal.

### Reading Parquet natively

**This section used to argue that a Parquet reader was the wrong thing to build.
It was wrong, and the way it was wrong is worth keeping.**

The argument had two premises. The first: reconstructing row N for the join means
holding a field offset per row per column, 1.42 GB at ten million rows, so
Parquet could end up costing *more* memory than CSV. The second: parsing is only
15% of a run in this engine, so a format that made parsing free would save 15%.

The second premise was true and irrelevant. The first was true only of a design
that reconstructs rows — and the whole point of a columnar format is that you
don't have to.

| | Predicted | Measured |
|---|---:|---:|
| Peak RSS, Parquet uncompressed | 2.75 GB | **3.41 GB** |
| Peak RSS, our CSV path | 4.41 GB | 4.37 GB |
| Time saved | ≤15% | **80%** |

**What the reader refuses, it refuses by name.** Nested or repeated schemas,
`BYTE_STREAM_SPLIT`, LZO and Brotli produce an error naming the feature rather
than a wrong answer. The encodings and codecs it does read are pinned by fixtures
pyarrow wrote — plain and dictionary, five codecs, version-2 delta pages, several
row groups — because a reader tested only against files its own writer produced
tests nothing.

**One Parquet file against a text one is the case the columnar path cannot
take**, because there is no column to compare a byte stream against. The Rust and
Zig ports answer it anyway, by decoding the pages into rows and handing them to
the byte-level engine — `rust/src/engine/turbo/parquet.rs` and `zig/src/pqread.zig`.
That costs exactly what the columnar path exists to avoid, which is why it is the
fallback and not the path: a Parquet pair never goes near it.


The table below is a different measurement from the [one at the
top](#four-formats-one-engine): it exists to place us against DuckDB and polars
on the same files, where that one exists to place the formats against each other
with nothing but this project involved. Measured on one machine in one sitting,
`python scripts/bench_parquet.py --polars`. Every row that finished returns
identical counts — matched 9,990,000,
changed 599,320, added 10,000, removed 10,000 — so these are eight ways of doing
exactly the same work.

| Engine and input | Size | Convert | **Compare** | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|
| ours (C++), CSV | 3,509 MB | — | 23.78s | 63.7s | 4,370 MB |
| DuckDB, CSV | 3,509 MB | — | 86.25s | 171.6s | 11,708 MB |
| polars, CSV | 3,509 MB | — | *out of memory* | 91.8s | 5,670 MB |
| ours (C++), Parquet + snappy | 1,043 MB | 67.9s | 6.85s | 19.0s | 3,840 MB |
| DuckDB, Parquet + snappy | 1,043 MB | 67.9s | 25.73s | 96.3s | 11,707 MB |
| polars, Parquet + snappy | 1,043 MB | 67.9s | 129.83s | 479.7s | 4,238 MB |
| **ours (C++), Parquet uncompressed** | 2,088 MB | 83.0s | **4.83s** | **14.1s** | **3,412 MB** |
| DuckDB, Parquet uncompressed | 2,088 MB | 83.0s | 22.71s | 84.8s | 11,985 MB |
| polars, Parquet uncompressed | 2,088 MB | 83.0s | 121.31s | 452.5s | 4,328 MB |

> This table is a fresh sitting on a slower machine than the tables above it —
> our CSV path reads 23.78s here and 12.34s there. Compare rows *within* the
> table, not across tables. polars is capped at 14 GB of address space on a 15 GB
> machine, so an out-of-memory is a reported failure rather than an OOM kill.

**4.9x our own CSV path, 4.7x DuckDB on the same file, 25x polars, and a third of
DuckDB's memory.** The format is worth 4.9x to us and 3.8x to DuckDB, which
inverts the earlier finding — when the parser was the only thing the format could
improve, CSV was 15% of the run; when the format changes the *shape* of the
comparison, it is most of it.

**Converting still costs more than it saves, once.** 68-83s to write the Parquet
against 17-19s saved per comparison. It pays from the fifth comparison of the
same file onward — which is the recurring-reconciliation case, but not the
one-off one.

But that is the cost of *converting*, and for benchmark data there is nothing to
convert from. The generator writes Parquet column by column from the same recipe
it writes CSV from, which makes the CSV in the middle unnecessary:

| Making a 10M pair | Time | Needs |
|---|---:|---|
| CSV, natively | 20.9s | — |
| …then DuckDB converts it to snappy Parquet | +67.9s | the 3.5 GB of CSV |
| …then DuckDB converts it to uncompressed Parquet | +83.0s | the 3.5 GB of CSV |
| **Parquet + snappy, natively** | **12.9s** | — |
| **Parquet uncompressed, natively** | **15.2s** | — |

`cpp/build/gen-data --rows 10m --format parquet --compression snappy`. **6.9x
quicker than the route through DuckDB, and it never writes the CSV at all.** The
files it makes are slightly smaller than DuckDB's (520 MB against 547 MB at
snappy), DuckDB reads them to the same digest as the CSV, and comparing our file
against DuckDB's file of the same ten million rows reports zero differences.

It also gives the tests something they did not have: the Parquet checks in
`cpp/test.sh` used to be skipped wherever DuckDB was not installed, because
DuckDB was the only way to produce a Parquet file. Six of them now run anywhere
the port builds, including the case where a column's dictionary gives up partway
— `--dict-limit 175 --row-group-size 300` makes three of the twenty columns come
out mixed. The two DuckDB checks that remain are the ones only a foreign writer
can give: that DuckDB reads what we wrote, and that our file and DuckDB's file of
the same rows compare as identical.

#### What polars had to be rewritten into

The polars rows come with an asterisk, and it is the most interesting result in
the table. Written the way anyone would write it — join the two frames, compare
every compared cell, sum — **polars cannot finish this at ten million rows on
either Parquet form**, and no amount of pushing helps: streaming engine, threads
turned down to two, a cap just under the whole machine. It dies at 7-11 GB. On
CSV it was killed by the OOM killer twice before any of this, which is why DuckDB
is the constant in the format table above.

Taking it apart says exactly where:

| polars, 10M, Parquet + snappy | Wall | Peak RSS | |
|---|---:|---:|---|
| First-occurrence-wins on the key | 1.79s | 1,584 MB | fine |
| Inner join, count the rows | 2.74s | 2,560 MB | fine |
| …and compare 17 columns per cell | — | 7,091 MB | **out of memory** |

The join is not the problem and the dedup is not the problem. The per-cell diff
is — which is the one thing this tool exists to do.

What polars *can* finish is the columnar shape: dedup, join and diff **one column
at a time**, projecting only the keys and that column, then union the keys of the
rows that differed. Peak drops to 4.2 GB and the answer is exactly right. That is
the same strategy this port's Parquet path uses — so the table is measuring
polars doing our design by hand, and it is still 19-25x slower, at 24-32x the CPU,
because each pass re-reads the file.

On CSV even that is not enough: forty passes over a 1.8 GB text file, each
re-parsing and re-deduplicating it, runs out of memory too. The polars CSV row in
the table is the columnar version failing, not the naive one.

Which is also why polars is the one engine here that is *slower* on uncompressed
Parquet than on snappy: forty passes over twice the bytes costs more than
decompressing them once. We read each column exactly once, so for us the ranking
goes the other way.

#### The design: never reconstruct a row

Four steps, and no step ever holds more than a few columns:

1. **Read only the key columns.** Two columns, both files, held for the whole run.
2. **Join on them once**, producing a list of matched `(a_row, b_row)` pairs —
   40 MB per side at ten million rows.
3. **Walk the compared columns one at a time**, each read, diffed, and released
   before the next is asked for. Four workers, so four columns are in flight;
   that is a memory choice, not a parallelism one.
4. **Never build a row.** A changed row is assembled at the end, for the fifty
   thousand rows that reach the report, out of what each column pass kept.

The last one is what the earlier argument missed. A row list of 1.42 GB is only
needed if the join has to see whole rows. It doesn't: it needs the key columns,
and everything after it is per-column.

#### The trick: two dictionaries, one id space

Parquet stores a low-cardinality column as small integers indexing a table of its
distinct values. Two files' dictionaries are unrelated — A's `currency` #3 is not
B's #3 — so the obvious thing is to expand both back to strings and compare
those, which throws the encoding away.

Instead both dictionaries are interned into **one shared id space**, once per
column. That is a few thousand string comparisons for a column of ten million
rows. After it, two cells are equal exactly when their ids are, and the diff is:

```c
for (i = 0; i < m; ++i) xa[i] = a_id[a_index[pair_a[base + i]]];
for (i = 0; i < m; ++i) xb[i] = b_id[b_index[pair_b[base + i]]];
for (i = 0; i < m; ++i) neq[i] = xa[i] != xb[i];   // one packed compare per 8
```

Two gathers through a table small enough to sit in L2, then an `int32` compare
the compiler turns into `vpcmpeqd`. A null is id `-1` on both sides, so
"both absent" and "one absent" fall out of the same comparison with no branch.

The mismatch mask is then read **eight bytes at a time**, the same SWAR idiom
[the CSV scanner uses to find a delimiter](#swar-and-how-it-compares-to-real-simd)
— here applied to finding a changed cell. About 0.7% of cells differ per column,
so seven bytes in eight of that mask are zero and skipping them wholesale is most
of the loop:

```c
std::memcpy(&w, neq + i, 8);
while (w) {
    unsigned byte = __builtin_ctzll(w) >> 3;
    hit(base + i + byte);
    w &= ~(0xFFULL << (byte * 8));
}
```

A column either side stores plainly falls back to comparing bytes, and so does
any column under `--tolerance`, where equality is not transitive and cannot be
given an id.

#### What the measurements moved, in order

Every step below was measured, not assumed. The first version was **12.6s** and
barely beat CSV; four changes took it to 4.9s, and none of them were in the
comparison itself:

| Change | 10M, snappy | Why |
|---|---:|---|
| First working version | 12.60s | |
| Parallelise both join directions | 10.85s | B's side was single-threaded and became the tail |
| Snappy: copy words, append in place | 8.30s | a back-reference was being copied a byte at a time |
| Pack a slice into one 64-bit word | 7.20s | 8 bytes a value, not 16 — half the memory *and* half the bandwidth |
| Bulk RLE decode, preallocated arrays | 4.69s | one 64-bit load per dictionary index instead of a refill loop |
| Hash tag inside the index slot | **4.08s** | a probe that misses is settled by the word it already loaded |

Two of these are the same lesson twice. **Packing a slice** into 40 bits of
offset, 23 of length and one null bit is exactly what the CSV engine already does
to a field, for exactly the same reason — and it was worth 15% of the run and
1.1 GB of memory. **Putting the hash tag in the index slot** rather than in a
parallel array removes the second cache miss from every failed probe; at ten
million keys those second misses *were* the join.

The snappy fix is the plainest of all: the decoder was writing back-references
with `out.push_back(out[from + i])`, one byte at a time. Copying eight bytes at a
time where the distance allows it, and decompressing straight into the buffer the
offsets refer to rather than into a scratch string that then gets appended, took
the whole read phase from 2.24s to 0.68s.

#### The same path in three languages

The columnar design is now in C++, Rust and Zig, and all three return the same
counts on the same files. One machine, one sitting, ten million rows:

| Port | CSV | Parquet + snappy | Parquet uncompressed | Peak RSS (uncompressed) |
|---|---:|---:|---:|---:|
| **C++** | 25.17s | 7.91s | **5.15s** | 3,421 MB |
| **Rust** | 58.66s | 8.58s | 6.22s | 3,409 MB |
| **Zig** | 33.82s | 16.47s | 14.20s | 3,417 MB |

**The format is worth more than the language.** Rust's own CSV engine takes
58.66s on these rows and its Parquet path 6.22s — 9.4x from changing what the
comparison reads, in one language, with the same rules and the same answer. C++
gains 4.9x and Zig 2.4x, the difference being how well each port already
threaded its CSV path.

The three are much closer to each other on Parquet than on CSV, and their peak
memory is within 12 MB of each other — the design, not the language, is what
sets both.

Where they still differ is threading, not code. Per core the three are close:
15.4s, 16.1s and 18.5s of CPU on the uncompressed file. C++ and Rust spread the
column pass and both directions of the join; Zig spreads only the column pass,
so its key phases now dominate its run. That is the next thing to do to the Zig
port, and it is worth about 2x.

#### What it refuses

There is a writer as well as a reader — `cpp/tools/pq_write.{hpp,cpp}`, used by
the generator — but it is benchmark and test scaffolding, not part of the engine,
and it writes only the envelope the reader accepts. Everything below is about the
reader.

The reader implements what this job meets and names the rest rather than guessing:
`BYTE_ARRAY` columns, PLAIN and dictionary encodings, uncompressed and snappy,
data page v1. Nested columns, other types, other codecs and page v2 are errors
that say which. Both files must be Parquet — comparing a column store against a
byte stream would mean building rows out of one of them, which is the cost this
path exists to avoid.

One case it does *not* refuse, because DuckDB writes it on any high-cardinality
string at scale: a column the writer starts as a dictionary and gives up on
partway. Those columns fold into the plain form as they are read, which copies
eight-byte handles rather than values.

#### How it is checked

The claim is that this is the *same* comparison, so the test is equality of the
whole report. `cpp/test.sh` converts the awkward fixture to Parquet — snappy and
uncompressed, large and tiny row groups — runs both paths, and requires the two
JSON documents to be identical: counts, per-column statistics, every changed
cell, every added and removed row, the duplicate sections, with only the file
names and the timing removed. Nine option combinations, plus nulls against empty
strings, plus a purpose-built mixed-encoding column, plus the refusal of a
mixed Parquet/text pair.

At ten million rows the check is the counts themselves: our CSV path, our Parquet
path and DuckDB on both formats independently agree on matched 9,990,000 and
changed 599,320.

## Using less memory than the report says

Peak RSS is what a process used with room to spare. For an engine that maps its
input that is close to meaningless — mapped pages are reclaimable, so the figure
is whatever the kernel let it keep, not what the work needed.
`scripts/memory_floor.sh` measures the number that matters by binary-searching the
smallest memory cgroup a comparison actually finishes in.

Ten million rows, 3.3 GB of CSV, four threads:

| | Reported peak RSS | Smallest limit that finishes |
|---|---:|---:|
| Holding chunk buffers to the end | 4,562 MB | 1,016 MB |
| **Freeing each chunk as it is consumed** | 4,375 MB | **857 MB** |

**The engine compares 3.3 GB of CSV inside 857 MB**, about 10% slower than
unconstrained. Peak RSS is five times that figure and moves 4% between the two
builds, where the floor moves 16% — so it was measuring the machine's spare
capacity, not the engine.

The change itself is small: the parallel sweep built per-chunk arrays of row
starts and hashes and held all of them while copying into the index, so every
row's start and hash existed twice at the peak. Released as each chunk is
consumed, the two curves cross instead of adding. It costs about 4% of time.

One trap worth writing down, since it cost an hour: each probe needs a **fresh**
cgroup. Lowering the limit on a cgroup that already has pages charged to it
fails silently, and the run then passes because nothing was actually constrained.

## The out-of-core engine

`sortmerge` batches rows, sorts each batch, spills it to disk, and does a k-way merge with the tie
break on run number so first-occurrence-wins still holds. It exists in Java, Go and Rust, and all
three produce byte-identical answers.

On the Set A runners, against the fastest engine and the plain in-memory one:

| Scale | `turbo` | `sortmerge` | `native` |
|---|---|---|---|
| 10k | 0.64s · 109 MB | 0.83s · 123 MB | 0.79s · 126 MB |
| 1M | **3.65s** · 657 MB | 7.25s · 2,032 MB | 5.91s · 3,000 MB |
| 10M | **28.18s** · 5,652 MB | 69.21s · **1,224 MB** | ✗ heap OOM at 31.5s |

Sorting costs about 2.5x the time of a hash join, which is the trade it makes and not a defect. What
it buys shows at 10M: `turbo` needs 5,652 MB and `sortmerge` needs 1,224 MB — **4.6x less** — while
`native`, which holds both files as rows, does not finish at all.

The three ports side by side, on one 4-core container rather than the runners:

| Scale | Engine | Compare | Peak RSS |
|---|---|---:|---:|
| 1M | Rust `polars` | **4.46s** | 2,250 MB |
| 1M | Java `turbo` | 5.73s | 633 MB |
| 1M | **Rust `sortmerge`** | 11.94s | **152 MB** |
| 1M | Go `native` | 12.24s | 1,857 MB |
| 1M | Java `sortmerge` | 13.85s | 1,265 MB |
| 1M | Go `sortmerge` | 16.27s | 224 MB |
| 1M | Rust `native` | 21.08s | 2,896 MB |
| 10M | Java `turbo` | **42.41s** | 5,321 MB |
| 10M | Java `sortmerge` | 151.00s | 1,106 MB |
| 10M | Go `sortmerge` | 217.21s | 313 MB |
| 10M | **Rust `sortmerge`** | 217.33s | **208 MB** |

**Rust `sortmerge` compares 3.68 GB of CSV in 208 MB** — under six per cent of what the two files
hold, a twenty-fifth of what `turbo` needs for the same answer, and less than the shell pipeline
needs while being correct besides.

**The surprise is at 1M, where Rust `sortmerge` beats Rust `native`** — 11.94s against 21.08s — while
using nineteen times less memory. Sorting is supposed to cost more than a hash join, and in Java and
Go it does. It does not here because `native` allocates an owned `String` key per row and hashes it
into a map, and that costs more than sorting the rows does. The out-of-core engine wins on both axes
in Rust, which is not the trade-off the Java numbers describe. Go is the shape the design predicts:
`sortmerge` uses eight times less memory than `native` and takes a third longer.

## Which memory number to trust

Three different numbers get called "memory" here and they answer different questions.

**Peak RSS** is what a process used with room to spare. It is what the tables report, and for the
mapping engines it *includes the mapped input files* — hence Set B's separate "above the mapped
files" table. Watch for the trap: Java `sortmerge` shows 2,032 MB at 1M and 1,224 MB at 10M on the
runners. Not a mistake — peak RSS measures what the JVM was *allowed* to keep, and at 1M the heap is
generous so the collector has no reason to run.

**Smallest heap that finishes** answers "how little will it run in", found by binary search
(`scripts/min_heap.sh`, 1M rows, `--max-rows 1000`):

| Engine | Smallest heap that finishes |
|---|---:|
| `duckdb` | 47 MB (but see below) |
| **`sortmerge`** | **63 MB** |
| `swar` / `mmap` | 127 MB |
| `turbo` / `shard` | 159 MB |
| `simd` | 478 MB |
| `tablesaw` | 1,259 MB |
| `native` | 2,470 MB |

368 MB of CSV compared in a 63 MB heap against 2,470 MB for the row-at-a-time engine: a **39x**
difference in what the machine has to provide.

Two caveats, or that table misleads. It measures **JVM heap** — what `-Xmx` controls and what fails
first in a container — not total memory. `duckdb` looks smallest at 47 MB because it does its work in
C++ outside the heap entirely; its actual footprint was 1,273 MB. The mapping engines likewise map
the file outside the heap. And the report cap matters: these runs embed 1,000 rows per section,
because the embedded sections are the one part of a comparison that grows with the answer rather than
the input.

**An enforced bound** is the only one of the three that is a guarantee rather than an observation.
The Zig port's `--max-memory MB` is a `FixedBufferAllocator`: on 1M rows the comparison is refused at
200 MB and completes at 204 MB, with no way to quietly exceed what it was given. Every other port can
only be *observed* to stay small. That is the one thing in this project a different language actually
bought.

## Duplicate keys: the one thing nothing agrees on

Nothing in Set C disagrees about which *cells* changed. Every disagreement is a design choice about
duplicate keys, and the generated data carries them precisely so it shows: at 1M, 100 keys appear
twice in A and 50 twice in B, one of which is duplicated in both.

**Tools with no concept of a duplicate key** — daff, datacompy — read the repeated row as an insert
on one side and a delete on the other. daff's `+50 / +100` is exactly those duplicates, one extra row
reported for each.

datacompy comes out at `+49 / +99`, one lower on each side, and the missing one is not rounding: it
pairs duplicate occurrences positionally, so the second copy in A is matched against the second copy
in B. Exactly one key here is duplicated on *both* sides (`ACC-00023757,TXN-00000000003`), and for
that key the two leftovers cancel. Defensible — but it answers a question the tool never asks the
user, and it makes the count depend on the order the duplicates appear in.

**Tools that join** — the DuckDB SQL, the ClickHouse SQL, the shell pipeline —
multiply them instead. A key twice in A and once in B joins to two rows; the one key twice on both
sides joins to four. That is 151 extra rows at 1M, and each that happens to be a *changed* row is
counted again, which is why `changed` lands 7 too high there and 91 too high at 10M. The number is
not a property of the tool but of which rows happened to be duplicated: the join silently inflates
the diff by an amount nobody can predict, and nothing in the output says a duplicate key was
involved.

**Two independent SQL engines make the identical mistake.** DuckDB and ClickHouse return exactly
`60,056 / 1,000 / 1,001` at 1M and exactly `599,411 / 10,000 / 10,001` at 10M — the same
seven-too-high `changed` count, down to the row, and the shell pipeline lands on both numbers too.
They share no code; what they share is `FULL OUTER JOIN`. The inflation is a property of the
operator, not of any engine: the answer you get from SQL here is the answer *SQL* gives, and picking
a better engine does not change it.

**This project** reports duplicates as their own section, joins on the first occurrence of each key,
and counts *keys* rather than rows. That is a choice too, but a stated one, and it is why these
counts differ from a plain `FULL OUTER JOIN` on the same data.

The Go tool avoids both traps and agrees with us exactly. Its raw output marks 60,053 rows modified
where we report 60,049; that gap is the same row-versus-key distinction, and reduced to distinct keys
its answer is identical.

**Not one of the ten external entries reports duplicate keys at all.** They either fold them in
silently or multiply them into the answer. On data that has any, four of these approaches will hand
you an inflated diff and say nothing about why.

---

# Ports

Five full ports, all to one result contract: the same JSON, the same HTML template, the same exit
codes, so a report from any of them is interchangeable and a benchmark number from one is directly
comparable with a number from another.

| | Directory | Engines | Notes |
|---|---|---|---|
| Python | `.` (this) | duckdb | the reference; also has `serve` and `mail` |
| TypeScript | [`ts/`](ts/) | duckdb, polars, arquero, native | Node 26, TypeScript 7 |
| Java | [`java/`](java/) | duckdb, turbo, swar, shard, mmap, simd, tablesaw, sortmerge, native | Java 26, Maven; five byte-level engines on SWAR, the Vector API and FFM, plus an out-of-core sort-merge join |
| Go | [`go/`](go/) | duckdb, sortmerge, native | Go 1.24 |
| Rust | [`rust/`](rust/) | duckdb, polars, sortmerge, turbo, native | edition 2024; `turbo` reads CSV, JSON **and Parquet** and threads the whole comparison; a Parquet pair goes down a columnar path that never reconstructs a row |

Three more carry the byte-level engine and the JSON counts only — benchmark and parity ports, so the
same design can be measured in four languages without three more HTML renderers to keep in step:

| | Directory | Built with | Scope |
|---|---|---|---|
| C | [`c/`](c/) | `cc` or `clang`, C11 | one file, single-threaded on purpose — it exists to find the memory floor; no `--trim`, `--ignore-case` or `--tolerance` |
| C++ | [`cpp/`](cpp/) | `g++` or `clang++`, C++20 | reads CSV, **newline-delimited JSON** and **Parquet**, including one of each; a Parquet pair takes the columnar path; `--threads N` splits the work across every core; `--ignore-case` is ASCII-only and refuses non-ASCII by name |
| Zig | [`zig/`](zig/) | Zig 0.16 or 0.17-dev, `--release=fast` | reads CSV, JSON **and Parquet**, columnar when both sides are Parquet; `--threads N`; `--max-memory MB` is enforced, not advisory — on both paths, including the arena Parquet decodes into |

The C++ and Zig ports build both indexes at once and run the two directions of the join at once,
which is what Java's `turbo` has always done. C++ goes further, splitting each file into row-aligned
chunks and A's side of the join into ranges — see
[Using more than two cores](#using-more-than-two-cores). Two things had to change to make that sound: the probe
buffer each index kept for key comparison is now supplied by the caller, which is why `lookup` is
const and two threads can probe one index; and Zig's `--max-memory` uses the lock-taking
`FixedBufferAllocator`, since a bump pointer without a lock would hand both threads the same bytes.
The budget it enforces is unchanged. A thread that cannot be spawned falls back to doing both halves
in turn.

The Zig source builds unchanged on 0.17.0-dev; only `build.zig` needs a newer API (`b.args` moved),
so a dev toolchain compiles it with `zig build-exe src/main.zig -O ReleaseFast`. The Java port
compiles at release 25 as well as 26, which is what makes it runnable on GraalVM.

Each has a `test.sh` holding it to the Rust port's answers on `tests/fixtures/awkward_*.csv` — a
fixture built from every shape that has broken an engine in this project: non-ASCII case folding, a
Kelvin sign that folds to one byte from three, doubled quotes as both value and key, CRLF, ragged
rows, a blank row, and a short key in the last bytes of the file.

`.github/workflows/parity.yml` enforces two things on every change: every implementation returns
identical counts and column stats for one dataset, and all five data generators emit byte-identical
files. The generator carries money in integer cents and applies the drift to those integers, never to
a float, so byte-identity does not depend on any language's floating-point rounding rule.

---

# Reproducing the numbers

## Test payloads

`scripts/gen_data.py` builds a deterministic pair with 20 columns keyed on `(account_id, txn_id)`.
`cpp/build/gen-data` builds the same bytes far faster, and writes **CSV, newline-delimited JSON and
Parquet** from one field-by-field recipe rather than converting one into another — so the four inputs
of a format comparison hold the same rows by construction.
`rust/target/release/gen-data` takes the same `--format csv|ndjson|parquet` and writes the same rows,
so a format comparison uses payloads this project wrote rather than a converter's idea of them — its
Parquet writer is in [`rust/src/gendata/parquet.rs`](rust/src/gendata/parquet.rs), and pyarrow and
DuckDB both read what it produces.

File B drifts from A by a fixed recipe, so every run has a known answer:

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

## The harnesses

```bash
# Set A — budgets, throughput, job-summary output
python scripts/gen_data.py --rows 10m --out-dir data
python scripts/bench.py --rows 10m --engine duckdb --threads 4 --memory-limit 8GB

# Set B — one design across toolchains
python scripts/gen_data.py --rows 20m --out-dir bench/external/data --prefix 20m
python scripts/bench_native.py --rows 20m --repeats 2
python scripts/bench_native.py --rows 1m --only jvm     # execution modes
cpp/build/csvdiff compare a.csv b.csv -k id --threads 4  # C++ thread scaling

# one engine across every size, generating and deleting each pair in turn
python scripts/bench_scale.py --sizes 10k,1m,10m,20m,50m --threads 4

# the generator that keeps up with the disk -- as CSV, or straight to Parquet
(cd cpp && make gen-data) && cpp/build/gen-data --rows 10m --out-dir data --prefix 10m
cpp/build/gen-data --rows 10m --out-dir data --format parquet --compression snappy
cpp/build/gen-data --rows 10m --out-dir data --format parquet --compression none \
  --dict-limit 175 --row-group-size 300   # forces mixed-encoding columns

# the smallest memory limit a comparison finishes in
scripts/memory_floor.sh cpp/build/csvdiff compare a.csv b.csv -k id --threads 4

# the four native formats against each other, nothing else involved
#   -- this is what .github/workflows/benchmark-formats.yml runs
(cd cpp && make && make gen-data)
python scripts/bench_formats_native.py --rows 10m --data /tmp/bench --out formats.json

# the historical version: the same comparison from CSV, Parquet and JSON with a
# third-party engine held constant
python scripts/bench_formats.py

# ours against DuckDB and polars, on CSV and on Parquet, in one sitting
python scripts/bench_parquet.py --data bench/external/data --prefix 10m --polars

# the same, generating everything natively first -- no DuckDB, no CSV in the middle
python scripts/bench_parquet.py --native 10m --data /tmp/bench --prefix 10m

# our own readers on all three formats, in all three native ports, one host,
# with peak RSS on every row -- and, with --matrix, one binary per scanner
python3 scripts/bench_formats_ports.py --rows 10m --formats csv,parquet --matrix

# our own reader on any of the three formats
cpp/build/csvdiff compare a.ndjson b.ndjson -k id --threads 4
cpp/build/csvdiff compare a.parquet b.parquet -k id --threads 4
CSVDIFF_PHASES=1 cpp/build/csvdiff compare a.parquet b.parquet -k id   # phase timings
rust/target/release/csvdiff compare a.parquet b.csv -k id --engine turbo -o /dev/null
zig/zig-out/bin/csvdiff compare a.parquet b.parquet -k id --threads 4

# the four Java byte-level engines head to head (Vector API needs the module)
java --add-modules jdk.incubator.vector -jar java/target/csvdiff.jar \
  compare a.csv b.csv -k account_id,txn_id -i updated_at --engine shard -o /dev/null

# Set C — the external field
python scripts/bench_external.py --rows 1m --mem-cap-gb 12

# smallest heap each Java engine finishes in
scripts/min_heap.sh
```

`bench_formats_native.py` is the one to reach for when the question is about formats: it generates
each of the four, measures it and deletes it before the next, because at ten million rows all four at
once is about 15 GB. It exits non-zero if the four disagree about the counts, which is the point --
four readers of the same rows that reach different answers is a bug, not a benchmark.

`bench.py` records generation time, comparison wall time, throughput, peak RSS, report size and the
counts, then fails if a scale exceeds its budget (10k: 20s / 1.5 GB, 1M: 120s / 6 GB, 10M: 900s /
12 GB on a 4-vCPU runner).

`bench_native.py` and `bench_external.py` both take peak RSS from `wait4`'s rusage for that exact
child — the kernel's own high-water mark rather than a poll that can miss a spike — and warm the page
cache before timing anything. Alternative toolchains are looked for in `/tmp` and `/opt`, and
external tools on `PATH` or in `bench/external/tools/`; whichever are missing are skipped **by name**
rather than dropped, so a short table means a build is absent, not that it lost.

`bench_external.py` caps each tool's address space, because at 10M several want more memory than the
machine has and without a cap the kernel does not fail them — it kills whatever it likes. There is
exactly one exemption, [explained above](#set-c--against-the-field).

## Project layout

```
csvdiff/engine.py         comparison (DuckDB), result contract at top
csvdiff/report.py         HTML renderer
csvdiff/cli.py            compare / serve / mail
csvdiff/server.py         drag-and-drop page
csvdiff/mailbot.py        IMAP/SMTP watcher
csvdiff/config.py         profiles (csvdiff.toml)
c/ cpp/ zig/              byte-level parity ports (one file each, plus test.sh)
go/ java/ rust/ ts/       full ports
scripts/gen_data.py       deterministic payload generator (20 columns, known drift)
scripts/bench.py          Set A harness, with budgets
scripts/bench_native.py   Set B harness, byte-level builds and JVM execution modes
scripts/bench_external.py Set C harness, the external field
scripts/min_heap.sh       binary-searches the smallest heap each Java engine finishes in
tests/fixtures/awkward_*  every shape that has broken an engine here
.github/workflows/        ci, parity, benchmark, on-demand comparison
CLAUDE.md, .claude/       project context, slash commands, report-editing skill
```

---

# Open questions

Sizes and shapes not yet answered, roughly in the order they would pay off:

1. **Where is the crossover between `polars` and `turbo`?** Polars wins at 1M and cannot reach 10M.
   The band between is unmeasured; 2M / 4M / 8M would find the exact point the recommendation
   changes.
2. **100M rows, and `sortmerge` at 50M.** 50M is now measured for the fastest build — see
   [How the fastest build scales](#how-the-fastest-build-scales) — and it is where the input stops
   fitting in RAM. What is still open is the same size through `sortmerge`, which should hold its
   memory flat where the mapping engines cannot, and 100M, where "about 100 MB per million rows"
   predicts 10 GB of index.
3. **Does `sortmerge` ever beat `turbo` on time?** It does in Rust at 1M. Whether that holds at
   larger sizes, or in the other ports, is open.
4. **Isolate the SWAR-versus-Vector reversal.** SWAR ties the Vector API at 10M and wins by 19% at
   20M, but the host changed with the scale. Running both scales on one host would say whether it is
   the size or the machine.
5. **More than two threads in the Zig port.** Answered for C++ — see
   [Using more than two cores](#using-more-than-two-cores), which took it from 1.71x to 2.53x
   cpu/wall and 20M from 54.60s to 33.32s. The guess in this slot that the scan was already
   bandwidth-bound was wrong, and the test that disproved it took one script. Zig still has the
   two-thread version only.
6. **Shard the index insertion on the CSV path.** Phase timing puts it at 3.09s of a 12.02s run at
   ten million rows, single-threaded. Sharding the hash table by key so each thread owns a slice
   would parallelise it while keeping first-occurrence-wins. This slot used to add "and it is worth
   more than any input format would be"; the [Parquet path](#reading-parquet-natively) took the same
   comparison from 23.25s to 4.89s, so that half of the claim is settled and wrong.
7. **Where does the C++ port stop scaling?** 2.53x cpu/wall on four cores is short of four, and the
   remaining sequential parts — table insertion, B's side of the join — are the obvious suspects. A
   box with more cores would say whether the design or the machine is the limit.
8. **Wide files.** Everything here is 20 columns. A 200-column file changes the ratio of key work to
   cell work, and probably the ranking. It would also re-open the SIMD question: longer rows mean
   longer scans, which is the one shape where a vector register might pay.
9. **Many small comparisons** rather than one big one — where JVM startup dominates and
   native-image's startup advantage might finally pay for its throughput.
10. **A newer GraalVM.** The AOT result is from Oracle GraalVM 25; if the FFM access path improves,
   the 17-21x should move.

## Suggested additions

Not built, ordered by how often they pay off in recurring comparisons:

1. **Column mapping** (`--map a_name=b_name`) when the two producers name columns differently.
2. **Value normalisers per column**: date formats (`2026-09-01` vs `09/01/2026`), currency, thousands
   separators, leading zeros, `Y/N` vs `true/false`. A `[normalise]` table in the profile.
3. **Thresholds as CI gates**: `--max-changed 0.5%`, `--max-added 100` → non-zero exit; the JSON
   summary already exists for dashboards.
4. **History**: keep each run's `summary.json` and show a trend line (changed % per run) per profile.
5. **Notifications**: post the text summary plus a link to the report to Teams/Slack/Jira on failure.
6. **XLSX export** of changed/added/removed for people who live in Excel.
7. **Scheduled runs** via a small `csvdiff watch` that picks up new files from a folder or SFTP by
   name pattern, pairs them, and mails the report.
8. **Key suggestion**: propose candidate composite keys by scanning for column sets that are unique.
9. **Fuzzy key matching** (normalised whitespace/case is done; next is Levenshtein for near-duplicate ids).
10. **Compare more than two files** (a chain A→B→C) or a CSV against a database query.
