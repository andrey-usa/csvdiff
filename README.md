# csvdiff

Compare two CSV files on a composite key and get a self-contained HTML report.
Key columns, compared columns and normalisation rules are parameters, so the same
tool serves every recurring comparison.

The same tool exists in five languages to one result contract, plus three byte-level
parity ports. That is what makes the benchmark section below a like-for-like
comparison rather than a collection of anecdotes.

**Jump to:** [Using it](#using-it) · [Which engine at which size](#which-engine-at-which-size) ·
[Benchmarks](#benchmarks) · [Techniques](#techniques) · [Ports](#ports) ·
[Reproducing](#reproducing-the-numbers) · [Open questions](#open-questions)

---

# Using it

## Install

```bash
pip install duckdb            # engine
pip install -e .              # gives you the `csvdiff` command
```

Python 3.11+. Everything except the engine is standard library. If DuckDB is not
installed the tool falls back to pandas (same results, in-memory only).

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
| `--engine` | pick an engine explicitly — see [Which engine at which size](#which-engine-at-which-size) |
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
but not out-of-core; pandas is 5-20x slower and memory-bound. The [benchmarks](#benchmarks) below put
numbers on all three, and on the bespoke engines that beat them.

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
| `ci.yml` | push, PR | pytest on Python 3.11/3.12/3.13, a 10k smoke comparison on both engines, a check that the report has no external references, and a job asserting DuckDB and pandas return identical counts on 200k rows |
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
| `/bench [10k\|1m\|10m] [duckdb\|pandas]` | runs a scale and reports time, throughput, peak RSS against budget |
| `/compare <a> <b> <key> [flags]` | runs a comparison and summarises the discrepancies, flagging setup mistakes such as a non-unique key |
| `/engines-agree` | checks DuckDB and pandas still produce identical counts on 200k rows |
| `/ci-fix` | pulls the latest failing run's logs, reproduces locally, fixes the cause |

`.claude/settings.json` pre-approves the test, benchmark and `gh run` commands, asks before
`git push` or `gh repo create`, and keeps `csvdiff.toml` out of reach since it holds mailbox
settings. `.claude/skills/csvdiff-report/` covers changes to the HTML report specifically.

---

# Which engine at which size

| Your input | Use | Why |
|---|---|---|
| Up to ~100k rows | anything | every engine finishes well under a second; startup cost dominates, so pick on convenience |
| 100k – 2M rows | `polars` where you have it, else `turbo` | columnar wins this band outright; the byte-level engines are close behind on a quarter of the memory |
| 2M – 20M rows, memory to spare | `turbo` | byte-level scanning; the only class that stays fast *and* still finishes at 10M+ |
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
| Python `pandas` | 0.82s · 100 MB | 52.89s · 2,748 MB | ✗ runner killed (OOM) |

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

| Build | 10k | 1M | 20M |
|---|---|---|---|
| C++, clang 20 | **0.05s** · 11 MB | **3.57s** · 509 MB | 83.60s · 8,573 MB |
| C, clang 20 | 0.05s · 11 MB | 3.57s · 417 MB | 85.93s · 8,447 MB |
| C++, clang 18 | 0.05s · 11 MB | 3.62s · 509 MB | 89.21s · 8,573 MB |
| Zig 0.17-dev | 0.05s · 11 MB | 4.47s · 414 MB | 116.08s · 8,446 MB |
| C, gcc 14 | 0.05s · 11 MB | 5.27s · 417 MB | 129.46s · 8,447 MB |
| Rust `turbo` | 0.10s · 17 MB | 5.33s · 583 MB | 104.07s · 8,677 MB |
| Zig 0.16 | 0.05s · 11 MB | 5.37s · 414 MB | 130.50s · 8,446 MB |
| Java 26 `turbo`, HotSpot | 0.95s · 103 MB | 5.54s · 628 MB | **82.49s** · 10,504 MB |
| C++, g++ 14 | 0.10s · 11 MB | 5.73s · 509 MB | 129.41s · 8,573 MB |
| C++, g++ 13 | 0.10s · 11 MB | 6.33s · 509 MB | 136.29s · 8,573 MB |
| Java 25 `turbo`, Graal JIT | 1.01s · 201 MB | 7.98s · 777 MB | 114.33s · 10,720 MB |
| GraalVM native-image, Serial GC | 1.16s · 71 MB | 90.62s · 481 MB | ✗ did not finish |
| GraalVM native-image, G1 GC | 1.41s · 175 MB | 107.00s · 768 MB | ✗ did not finish |

What the engine actually allocates, with the mapped inputs subtracted:

| Build | 1M (351 MB mapped) | 20M (7,018 MB mapped) |
|---|---:|---:|
| Zig 0.16 / 0.17-dev | **63 MB** | **1,428 MB** |
| C, either compiler | 67 MB | 1,429 MB |
| native-image, Serial GC | 130 MB | — |
| C++, either compiler | 158 MB | 1,556 MB |
| Rust `turbo` | 232 MB | 1,659 MB |
| Java 26 `turbo`, HotSpot | 277 MB | 3,487 MB |
| Java 25 `turbo`, Graal JIT | 426 MB | 3,703 MB |

**The JVM wins at 20M**, one percent ahead of clang-20 C++ — and pays 3,487 MB where C++ pays 1,556
and Zig 1,428. At the largest size tested, the fastest thing in this project is the one with a
garbage collector. That is not where Set A pointed.

The Java row is `turbo`, and it is the right choice at this size: `shard`, which edged `turbo` at 10M
on the Set A runners, is **19% slower at 20M** (96.26s against 80.59s on this host). The full
four-engine matrix is under
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
| **datacompy** (Capital One) | the reconciliation library, on pandas or Polars | yes |
| **pandas** | the outer merge people write before finding a library | yes |
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
| datacompy (pandas) | 1.06s · 188 MB | 38.70s · 2,274 MB | ✗ out of memory | +49 added, +99 removed |
| datacompy (polars) | 0.79s · 178 MB | 5.64s · 2,685 MB | ✗ out of memory | +49 added, +99 removed |
| pandas (hand-written merge) | 0.89s · 143 MB | 28.21s · 1,825 MB | ✗ out of memory | +7 changed, +1 removed |
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
| pandas merge | counts unless you write more | no — duplicates multiply | no |
| csv-diff | yes | no | no |
| sort + join | no — counts only | no | no |

**At ten million rows most of the field cannot run at all.** Six of the ten external entries fail on
3.68 GB in a 12 GB budget. The interesting thing is not that they are slow — it is that "fastest"
stops being the question.

**daff's ceiling is not memory.** It reads the file with `readFileSync`, and V8 refuses to build a
string longer than 512 MB. No amount of RAM moves that limit, so daff cannot open a file this size on
any machine. Every other failure above is genuine memory exhaustion; this one is a wall.

**The backend decides a dataframe's speed, not the library.** At 1M datacompy takes 38.70s on pandas
and 5.64s on Polars — same library, same call, 6.9x apart. That gap is wider than any language
difference in this whole project. If a dataframe reconciliation is slow, the first question is which
backend it is on.

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

**Tools that join** — the DuckDB SQL, the ClickHouse SQL, the pandas merge, the shell pipeline —
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
| Python | `.` (this) | duckdb, pandas | the reference; also has `serve` and `mail` |
| TypeScript | [`ts/`](ts/) | duckdb, polars, arquero, native | Node 26, TypeScript 7 |
| Java | [`java/`](java/) | duckdb, turbo, swar, shard, mmap, simd, tablesaw, sortmerge, native | Java 26, Maven; five byte-level engines on SWAR, the Vector API and FFM, plus an out-of-core sort-merge join |
| Go | [`go/`](go/) | duckdb, sortmerge, native | Go 1.24 |
| Rust | [`rust/`](rust/) | duckdb, polars, sortmerge, turbo, native | edition 2024 |

Three more carry the byte-level engine and the JSON counts only — benchmark and parity ports, so the
same design can be measured in four languages without three more HTML renderers to keep in step:

| | Directory | Built with | Scope |
|---|---|---|---|
| C | [`c/`](c/) | `cc` or `clang`, C11 | one file; no `--trim`, `--ignore-case` or `--tolerance` |
| C++ | [`cpp/`](cpp/) | `g++` or `clang++`, C++20 | `--ignore-case` is ASCII-only and refuses non-ASCII by name |
| Zig | [`zig/`](zig/) | Zig 0.16 or 0.17-dev, `--release=fast` | `--max-memory MB` is enforced, not advisory |

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

# the four Java byte-level engines head to head (Vector API needs the module)
java --add-modules jdk.incubator.vector -jar java/target/csvdiff.jar \
  compare a.csv b.csv -k account_id,txn_id -i updated_at --engine shard -o /dev/null

# Set C — the external field
python scripts/bench_external.py --rows 1m --mem-cap-gb 12

# smallest heap each Java engine finishes in
scripts/min_heap.sh
```

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
csvdiff/engine.py         comparison (DuckDB + pandas fallback), result contract at top
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
2. **50M and 100M rows.** 20M is the largest tested. `sortmerge` should stay flat in memory and the
   mapping engines should not, but "should" is not a measurement.
3. **Does `sortmerge` ever beat `turbo` on time?** It does in Rust at 1M. Whether that holds at
   larger sizes, or in the other ports, is open.
4. **Isolate the SWAR-versus-Vector reversal.** SWAR ties the Vector API at 10M and wins by 19% at
   20M, but the host changed with the scale. Running both scales on one host would say whether it is
   the size or the machine.
5. **Wide files.** Everything here is 20 columns. A 200-column file changes the ratio of key work to
   cell work, and probably the ranking.
6. **Many small comparisons** rather than one big one — where JVM startup dominates and
   native-image's startup advantage might finally pay for its throughput.
7. **A newer GraalVM.** The AOT result is from Oracle GraalVM 25; if the FFM access path improves,
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
