# csvdiff

Compare two CSV files on a composite key and get a self-contained HTML report.
Key columns, compared columns and normalisation rules are parameters, so the same
tool serves every recurring comparison.

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
| `--threads`, `--memory-limit` | DuckDB resource limits |
| `--no-compress` | plain JSON payload for pre-2023 browsers |

Duplicate keys are counted and listed per file; the first occurrence of each key takes part in the join.

A row with more or fewer fields than the header is a difference to report, not a file to refuse: the
missing fields read as absent and the extra ones are ignored. All five implementations agree on this,
and the test suites hold them to it. The one exception is Java's `tablesaw` engine, whose reader has
no option to allow it; it refuses such a file and says which engines will take it.

## Why DuckDB

Both files are read as text (no type inference surprises such as `1.0` vs `1`),
hash-joined on the key in parallel, and spilled to disk when they don't fit in RAM.
Multi-GB files compare in seconds to low minutes on a laptop, with one wheel as the
only dependency. Polars is comparably fast in memory but not out-of-core; pandas is
5-20x slower and memory-bound.

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

## Running it in GitHub

Three workflows in `.github/workflows`:

| Workflow | Trigger | What it does |
|---|---|---|
| `ci.yml` | push, PR | pytest on Python 3.11/3.12/3.13, a 10k smoke comparison on both engines, a check that the report has no external references, and a job asserting DuckDB and pandas return identical counts on 200k rows |
| `benchmark.yml` | manual, weekly cron | generates 10k / 1M / 10M rows x 20 columns, compares, enforces time and memory budgets, uploads reports, writes a results table to the job summary |
| `compare.yml` | manual, or `workflow_call` from another pipeline | compares two files given as repo paths or URLs and publishes the report as an artifact |

```bash
git init && git add . && git commit -m "csvdiff"
gh repo create csvdiff --private --source=. --push

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

### Test payloads

`scripts/gen_data.py` builds a deterministic pair of files with 20 columns keyed on
`(account_id, txn_id)`. File B drifts from A by a fixed recipe, so every run has a known answer:

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

```bash
python scripts/gen_data.py --rows 10m --out-dir data     # DuckDB writes this in seconds
python scripts/bench.py --rows 10m --engine duckdb --threads 4 --memory-limit 8GB
```

`bench.py` records generation time, comparison wall time, throughput, peak RSS of the comparison
process, report size and the resulting counts, then fails if a scale exceeds its budget
(10k: 20s / 1.5 GB, 1M: 120s / 6 GB, 10M: 900s / 12 GB on a 4-vCPU runner).

Measured on this repo's own sandbox, single vCPU, pandas fallback — DuckDB on a 4-vCPU runner
is several times faster:

| scale | input | generate | compare | peak RSS | report |
|---|---|---|---|---|---|
| 10k | 3.7 MB | 0.3s | 0.7s | 99 MB | 33 KB |
| 1M | 368 MB | 23s | 55s | 2.8 GB | 1.1 MB |

The 1M report holds 60,049 changed rows, caps the embedded list at 50,000 and still weighs 1.1 MB.
Use `--export-dir` when the full list matters; the counts in the report are always exact.

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

## Suggested additions

Not built, ordered by how often they pay off in recurring comparisons:

1. **Column mapping** (`--map a_name=b_name`) when the two producers name columns differently.
2. **Value normalisers per column**: date formats (`2026-09-01` vs `09/01/2026`), currency, thousands
   separators, leading zeros, `Y/N` vs `true/false`. A `[normalise]` table in the profile.
3. **Thresholds as CI gates**: `--max-changed 0.5%`, `--max-added 100` → non-zero exit; JSON summary
   already exists for dashboards.
4. **History**: keep each run's `summary.json` and show a trend line (changed % per run) per profile.
5. **Notifications**: post the text summary plus a link to the report to Teams/Slack/Jira on failure.
6. **XLSX export** of changed/added/removed for people who live in Excel.
7. **Scheduled runs** via a small `csvdiff watch` that picks up new files from a folder or SFTP by
   name pattern, pairs them, and mails the report.
8. **Key suggestion**: propose candidate composite keys by scanning for column sets that are unique.
9. **Fuzzy key matching** (normalised whitespace/case is done; next is Levenshtein for near-duplicate ids).
10. **Compare more than two files** (a chain A→B→C) or a CSV against a database query.

## Benchmark results

Five implementations, seventeen engines, three scales, all on **byte-identical input** — 20 columns
keyed on `(account_id, txn_id)` with `--ignore updated_at` — on 4-vCPU / 16 GB GitHub-hosted
runners. Reproduce with the `benchmark-*.yml` workflows; every number below is one run, so treat
differences under about 5% as noise.

### 10k rows (3.7 MB)

Process startup, not comparison. A JVM costs ~0.5s before it reads a byte.

| Language | Engine | Compare | Throughput | Peak RSS |
|---|---|---|---|---|
| Go | `native` | **0.04s** | 500,050/s | 44 MB |
| Rust | `polars` | 0.05s | 400,040/s | 64 MB |
| Rust | `native` | 0.08s | 250,025/s | **38 MB** |
| TypeScript | `polars` | 0.19s | 105,273/s | 142 MB |
| TypeScript | `native` | 0.20s | 100,010/s | 124 MB |
| TypeScript | `arquero` | 0.34s | 58,829/s | 152 MB |
| Rust | `duckdb` | 0.36s | 55,561/s | 114 MB |
| Python | `duckdb` | 0.51s | 39,219/s | 175 MB |
| Go | `duckdb` | 0.52s | 38,465/s | 192 MB |
| TypeScript | `duckdb` | 0.52s | 38,465/s | 238 MB |
| Java | `native` | 0.64s | 31,253/s | 138 MB |
| Java | `mmap` | 0.67s | 29,854/s | 122 MB |
| Java | `simd` | 0.70s | 28,574/s | 128 MB |
| Java | `swar` | 0.78s | 25,644/s | 110 MB |
| Java | `turbo` | 0.81s | 24,694/s | 109 MB |
| Python | `pandas` | 0.82s | 24,392/s | 100 MB |
| Java | `shard` | 0.88s | 22,730/s | 116 MB |
| Java | `tablesaw` | 1.31s | 15,269/s | 183 MB |
| Java | `duckdb` | 1.33s | 15,039/s | 244 MB |

### 1M rows (368 MB)

| Language | Engine | Compare | Throughput | Peak RSS |
|---|---|---|---|---|
| TypeScript | `polars` | **2.30s** | 869,630/s | 2,507 MB |
| Rust | `polars` | 2.57s | 778,268/s | 2,289 MB |
| Java | `simd` | 3.61s | 554,058/s | 908 MB |
| Java | `shard` | 3.89s | 514,177/s | 666 MB |
| Java | `turbo` | 4.02s | 497,550/s | 650 MB |
| Java | `swar` | 4.07s | 491,437/s | **622 MB** |
| Java | `mmap` | 4.37s | 457,700/s | 644 MB |
| Java | `native` | 6.26s | 319,513/s | 3,006 MB |
| Go | `native` | 6.35s | 314,984/s | 1,935 MB |
| Java | `tablesaw` | 8.53s | 234,484/s | 2,054 MB |
| TypeScript | `native` | 9.41s | 212,555/s | 3,241 MB |
| Python | `duckdb` | 11.57s | 172,873/s | 2,395 MB |
| Rust | `duckdb` | 11.80s | 169,504/s | 2,087 MB |
| TypeScript | `duckdb` | 11.86s | 168,646/s | 2,691 MB |
| Rust | `native` | 12.22s | 163,678/s | 2,896 MB |
| Java | `duckdb` | 12.80s | 156,262/s | 2,394 MB |
| TypeScript | `arquero` | 17.22s | 116,152/s | 3,487 MB |
| Go | `duckdb` | 21.04s | 95,064/s | 2,271 MB |
| Python | `pandas` | 52.89s | 37,817/s | 2,748 MB |

### 10M rows (3.68 GB)

Nine of nineteen engines do not finish. This is where the design decisions show.

| Language | Engine | Compare | Throughput | Peak RSS |
|---|---|---|---|---|
| Java | `shard` | **25.33s** | 789,637/s | 5,415 MB |
| Java | `turbo` | 25.41s | 787,151/s | 5,396 MB |
| Java | `mmap` | 31.52s | 634,565/s | 5,280 MB |
| Java | `swar` | 32.60s | 613,543/s | **5,270 MB** |
| Python | `duckdb` | 120.97s | 165,342/s | 9,906 MB |
| TypeScript | `duckdb` | 122.11s | 163,799/s | 10,145 MB |
| Rust | `duckdb` | 122.32s | 163,518/s | 9,423 MB |
| Java | `duckdb` | 128.34s | 155,848/s | 8,851 MB |
| Go | `native` | 161.67s | 123,718/s | 15,174 MB |
| Go | `duckdb` | 179.02s | 111,728/s | 9,560 MB |
| Java | `native` | ✗ Java heap OOM after 28.0s | | |
| Java | `tablesaw` | ✗ Java heap OOM after 64.3s | | |
| Java | `simd` | ✗ Java heap OOM after 2.9s | | |
| Rust | `native` | ✗ runner killed (OOM) | | |
| Rust | `polars` | ✗ runner killed (OOM) | | |
| TypeScript | `polars` | ✗ runner killed (OOM) | | |
| Python | `pandas` | ✗ runner killed (OOM) | | |
| TypeScript | `native` | ✗ V8 512 MB string cap, after 1.1s | | |
| TypeScript | `arquero` | ✗ V8 512 MB string cap, after 0.5s | | |

### The out-of-core engine

`sortmerge` came later than the tables above and now exists in Java, Go and Rust. All three produce
byte-identical answers — on ten million rows: 599,320 changed, 10,000 added, 10,000 removed, 1,000
and 500 duplicate keys.

On the GitHub runners the other tables use, against the fastest engine and the plain in-memory one:

| Scale | Engine | Compare | Throughput | Peak RSS |
|---|---|---|---:|---:|
| 10k | `turbo` | 0.64s | 31,253/s | 109 MB |
| 10k | `sortmerge` | 0.83s | 24,099/s | 123 MB |
| 10k | `native` | 0.79s | 25,319/s | 126 MB |
| 1M | `turbo` | **3.65s** | 547,986/s | 657 MB |
| 1M | `sortmerge` | 7.25s | 275,883/s | 2,032 MB |
| 1M | `native` | 5.91s | 338,435/s | 3,000 MB |
| 10M | `turbo` | **28.18s** | 709,776/s | 5,652 MB |
| 10M | `sortmerge` | 69.21s | 288,997/s | **1,224 MB** |
| 10M | `native` | ✗ Java heap OOM after 31.5s | | |

Sorting costs about 2.5x the time of a hash join, which is the trade it is making and not a defect.
What it buys shows at ten million rows: `turbo` needs 5,652 MB there and `sortmerge` needs 1,224 MB —
**4.6x less** — while `native`, which holds both files as rows, does not finish at all.

#### The three ports side by side

The table above is Java. These are all three, on one 4-core / 16 GB container rather than the
runners, so compare within this table and not across to the ones above. A million rows is the best of
three runs; ten million is a single run, because at that size the in-memory engines want more memory
than the machine has and a single timing is honest enough for a gap this wide.

| Scale | Engine | Compare | Peak RSS |
|---|---|---:|---:|
| 1M | rust `polars` | **4.46s** | 2,250 MB |
| 1M | java `turbo` | 5.73s | 633 MB |
| 1M | **rust `sortmerge`** | 11.94s | **152 MB** |
| 1M | go `native` | 12.24s | 1,857 MB |
| 1M | java `sortmerge` | 13.85s | 1,265 MB |
| 1M | go `sortmerge` | 16.27s | 224 MB |
| 1M | rust `native` | 21.08s | 2,896 MB |
| 10M | java `turbo` | **42.41s** | 5,321 MB |
| 10M | java `sortmerge` | 151.00s | 1,106 MB |
| 10M | go `sortmerge` | 217.21s | 313 MB |
| 10M | **rust `sortmerge`** | 217.33s | **208 MB** |

**Rust `sortmerge` compares 3.68 GB of CSV in 208 MB** — under six per cent of what the two files
hold, and a twenty-fifth of what `turbo` needs to reach the same answer.

The surprise is at a million rows, where **Rust `sortmerge` is faster than Rust `native`** — 11.94s
against 21.08s — while using nineteen times less memory. Sorting is supposed to cost more than a hash
join, and in Java and Go it does. It does not here because `native` allocates an owned `String` key
per row and hashes it into a map, and that costs more than sorting the rows does. The out-of-core
engine wins on both axes in Rust, which is not the trade-off the Java numbers describe.

Go is the shape the design predicts: `sortmerge` uses eight times less memory than `native` and takes
a third longer.

Note the shape of `sortmerge`'s memory across scales — 2,032 MB at a million rows on the runners and
1,224 MB at ten million. That is not a mistake. Peak RSS measures what the JVM was *allowed* to keep,
not what the engine needs; at a million rows the heap is generous and the collector has no reason to
run. The Go and Rust ports have no such allowance, which is why their figures are so much lower for
the same work.

The figure that actually answers "how little will it run in" is the smallest heap each Java engine
can finish a million rows in, found by binary search (`scripts/min_heap.sh`, `--max-rows 1000`):

| Engine | Smallest heap that finishes |
|---|---:|
| `duckdb` | 47 MB (but see below) |
| **`sortmerge`** | **63 MB** |
| `swar` / `mmap` | 127 MB |
| `turbo` / `shard` | 159 MB |
| `simd` | 478 MB |
| `tablesaw` | 1,259 MB |
| `native` | 2,470 MB |

368 MB of CSV compared in a 63 MB heap, against 2,470 MB for the row-at-a-time engine: a **39x**
difference in what the machine has to provide, which is the whole point of the design.

Two caveats, or that table misleads. It measures **JVM heap**, which is what `-Xmx` controls and what
fails first in a container — not total memory. `duckdb` looks smallest at 47 MB because it does its
work in C++ outside the heap entirely; its actual footprint was 1,273 MB. The mapping engines
(`turbo`, `swar`, `shard`, `mmap`) likewise map the file outside the heap. And the report cap matters:
these runs embed 1,000 rows per section, because the embedded sections are the one part of a
comparison that grows with the answer rather than the input.

### Winners

| Scale | Winner | Time | Runner-up |
|---|---|---|---|
| 10k | **Go `native`** | 0.04s | Rust `polars` 0.05s |
| 1M | **TypeScript `polars`** | 2.30s | Rust `polars` 2.57s |
| 10M | **Java `shard`** | 25.33s | Java `turbo` 25.41s |

**By language** — each language's best engine at each scale:

| Language | 10k | 1M | 10M |
|---|---|---|---|
| Java | `native` 0.64s | `simd` 3.61s | **`shard` 25.33s** |
| Rust | **`polars` 0.05s** | `polars` 2.57s | `duckdb` 122.32s |
| Go | **`native` 0.04s** | `native` 6.35s | `native` 161.67s |
| TypeScript | `polars` 0.19s | **`polars` 2.30s** | `duckdb` 122.11s |
| Python | `duckdb` 0.51s | `duckdb` 11.57s | `duckdb` 120.97s |

**By library** — the best implementation of each, across all five languages:

| Library | 10k | 1M | 10M | Verdict |
|---|---|---|---|---|
| **bespoke** (hand-written) | Go 0.04s | Java `simd` 3.61s | Java `shard` **25.33s** | 🥇 wins overall; only class that survives 10M in-memory |
| **Polars** | Rust 0.05s | TS **2.30s** | ✗ OOM | fastest to 1M, cannot reach 10M |
| **DuckDB** | Rust 0.36s | Python 11.57s | Python 120.97s | only *library* that scales; Python's binding is the best of five |
| **Tablesaw** | Java 1.31s | Java 8.53s | ✗ OOM | last in its class |
| **Arquero** | TS 0.34s | TS 17.22s | ✗ V8 cap | small data only |
| **pandas** | Python 0.82s | Python 52.89s | ✗ OOM | slowest engine that finished 1M |

### What the numbers say

**The winner flips with scale.** Go wins 10k on startup cost, Polars wins 1M on columnar speed, and
Java's byte-level engines win 10M because they are the only ones still standing. Any single "fastest
implementation" claim would be wrong at two sizes out of three.

**Memory decides 10M, not speed.** TypeScript `polars` is the fastest thing measured at 1M and cannot
run 10M at all. Java `swar` uses 5.3 GB where Go `native` uses 15.2 GB for the same job — that is the
"no String per cell" design, and it is what buys the win.

**Scaling from 1M to 10M is not linear:**

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

**The same library differs by binding.** DuckDB at 10M: Python 120.97s, TypeScript 122.11s, Rust
122.32s, Java 128.34s, Go 179.02s. Identical C++ engine doing identical work; Go's cgo binding costs
~48% over Python's.

**SWAR and the Vector API are a tie.** `shard` (Vector) 25.33s against `turbo` (SWAR) 25.41s at 10M is
0.3% — noise. On one thread the Vector API was 3% ahead here (`mmap` 31.52s vs `swar` 32.60s), while
on a local 4-core machine SWAR was ~7% ahead of the Vector API in both pairings. Across single runs on
shared runners, the honest reading is that the two techniques cost the same. SWAR's real advantage is
elsewhere: it needs **no incubator module**, so `turbo` is the fastest engine that runs on a stock
`java -jar` with no flags, and it uses slightly less memory because no vector machinery is loaded.

## Against the field

The section above compares this project with itself: five languages, eighteen engines, one design.
That says which language and which technique is faster. It does not say whether the design is any
good, because every one of those engines was written here.

So the same comparison — the same two files, the same composite key, the same ignored column — was
run through the tools people actually reach for. Reproduce with:

```bash
python scripts/bench_external.py --rows 1m --mem-cap-gb 12
```

Each tool runs three times and the time is the median, the memory the largest seen — except at ten
million rows, which is a single run because several of the tools take minutes to fail. Both inputs are
read once before anything is timed, so no tool is charged for the page-cache miss the others avoid.
Peak memory is the high-water mark of the whole process tree, so a shell pipeline is credited with
what `sort` actually used. Each tool's address space is capped, because at ten million rows several
of them want more memory than the machine has, and without a cap the kernel does not fail them — it
kills whatever it likes.

These runs are on a 4-core / 16 GB container, not on the GitHub runners the section above uses, so
compare tools within these tables and not across to those.

### The field

| Tool | What it is | Expresses this task? |
|---|---|---|
| **csvdiff (this project)** | bespoke, `turbo` and `sortmerge` engines | yes |
| **csvdiff (Go, aswinkarthik)** | the fastest dedicated CSV diff in wide use; xxHash of key and row | yes, but the answer is coarser |
| **DuckDB CLI** | a full outer join written by hand in SQL | yes |
| **daff** | the tabular-diff library behind `git daff`; alignment-based, `--id` pins a key | yes |
| **datacompy** (Capital One) | the reconciliation library, on pandas or Polars | yes |
| **pandas** | the outer merge people write before finding a library | yes |
| **csv-diff** (Simon Willison) | small, popular, row dicts | **no** — one key column, no column-ignore |
| **sort(1) + join(1)** | the shell pipeline | **no** — no idea what CSV quoting is |

Surveyed and not run: **data-diff** (Datafold) bisects checksums to compare tables across a network,
which is the wrong problem here — both files are already local, so there is nothing to avoid
transferring. **qsv** has no keyed diff subcommand of this shape.

### 10 000 rows (3.7 MB)

| Tool | Approach | Time | Peak RSS | changed / added / removed | Agrees | Notes |
| --- | --- | ---: | ---: | --- | --- | --- |
| csvdiff (this project, turbo) | bespoke | 0.70s | 111 MB | 600 / 10 / 10 | reference | cell-level diff, duplicate-key report, self-contained HTML |
| csvdiff (this project, sortmerge) | bespoke | 0.64s | 137 MB | 600 / 10 / 10 | yes | cell-level diff, duplicate-key report, self-contained HTML |
| csvdiff (Go, aswinkarthik) | hash-only | 0.03s | 18 MB | 600 / 10 / 10 | yes | row hash only: says a row changed, not which cell |
| DuckDB CLI (hand-written SQL) | SQL | 0.20s | 42 MB | 600 / 10 / 10 | yes | counts only: no cell diff, no duplicate-key report, no report file |
| daff (JS) | alignment diff | 0.30s | 120 MB | 600 / 11 / 11 | dup keys only | cell-level diff; no duplicate-key concept — a repeated key reads as an insert |
| datacompy (pandas) | dataframe | 0.88s | 188 MB | 600 / 11 / 11 | dup keys only | cell-level diff and a per-column summary; whole frame in memory |
| datacompy (polars) | dataframe | 0.56s | 178 MB | 600 / 11 / 11 | dup keys only | same library, columnar backend |
| pandas (hand-written merge) | dataframe | 0.52s | 141 MB | 600 / 10 / 10 | yes | counts only unless you write more; duplicate keys multiply through the merge |
| csv-diff (Python) | row dicts | 0.24s | 67 MB | 600 / 10 / 10 | yes | single key column and no column-ignore, so the input has to be reshaped first |
| sort(1) + join(1) | shell pipeline | 0.08s | 5 MB | 600 / 10 / 10 | yes | counts only; no CSV quoting, no duplicate-key concept, no diff |

### 1 000 000 rows (368 MB)

| Tool | Approach | Time | Peak RSS | changed / added / removed | Agrees | Notes |
| --- | --- | ---: | ---: | --- | --- | --- |
| csvdiff (this project, turbo) | bespoke | 4.94s | 667 MB | 60,049 / 1,000 / 1,000 | reference | cell-level diff, duplicate-key report, self-contained HTML |
| csvdiff (this project, sortmerge) | bespoke | 17.24s | 2,031 MB | 60,049 / 1,000 / 1,000 | yes | cell-level diff, duplicate-key report, self-contained HTML |
| csvdiff (Go, aswinkarthik) | hash-only | 2.81s | 1,482 MB | 60,049 / 1,000 / 1,000 | yes | row hash only: says a row changed, not which cell |
| DuckDB CLI (hand-written SQL) | SQL | 7.57s | 1,273 MB | 60,056 / 1,000 / 1,001 | no (changed +7, removed +1) | counts only: no cell diff, no duplicate-key report, no report file |
| daff (JS) | alignment diff | 40.61s | 4,247 MB | 60,049 / 1,050 / 1,100 | dup keys only | cell-level diff; no duplicate-key concept — a repeated key reads as an insert |
| datacompy (pandas) | dataframe | 29.83s | 2,272 MB | 60,049 / 1,049 / 1,099 | no (added +49, removed +99) | cell-level diff and a per-column summary; whole frame in memory |
| datacompy (polars) | dataframe | 4.09s | 2,666 MB | 60,049 / 1,049 / 1,099 | no (added +49, removed +99) | same library, columnar backend |
| pandas (hand-written merge) | dataframe | 17.34s | 1,799 MB | 60,056 / 1,000 / 1,001 | no (changed +7, removed +1) | counts only unless you write more; duplicate keys multiply through the merge |
| sort(1) + join(1) | shell pipeline | 8.23s | 251 MB | 60,056 / 1,000 / 1,001 | no (changed +7, removed +1) | counts only; no CSV quoting, no duplicate-key concept, no diff |

### 10 000 000 rows (3.68 GB)

| Tool | Approach | Time | Peak RSS | changed / added / removed | Agrees | Notes |
| --- | --- | ---: | ---: | --- | --- | --- |
| csvdiff (this project, turbo) | bespoke | 34.45s | 5,392 MB | 599,320 / 10,000 / 10,000 | reference | cell-level diff, duplicate-key report, self-contained HTML |
| csvdiff (this project, sortmerge) | bespoke | 126.39s | 1,853 MB | 599,320 / 10,000 / 10,000 | yes | cell-level diff, duplicate-key report, self-contained HTML |
| csvdiff (Go, aswinkarthik) | hash-only | — | — | — | — | **out of memory** |
| DuckDB CLI (hand-written SQL) | SQL | 73.17s | 8,765 MB | 599,411 / 10,000 / 10,001 | no (changed +91, removed +1) | counts only: no cell diff, no duplicate-key report, no report file |
| daff (JS) | alignment diff | — | — | — | — | **V8 string cap: cannot read a file over 512 MB** |
| datacompy (pandas) | dataframe | — | — | — | — | **out of memory** |
| datacompy (polars) | dataframe | — | — | — | — | **out of memory** |
| pandas (hand-written merge) | dataframe | — | — | — | — | **out of memory** |
| csv-diff (Python) | row dicts | — | — | — | — | **out of memory** |
| sort(1) + join(1) | shell pipeline | 119.75s | 2,483 MB | 599,411 / 10,000 / 10,001 | no (changed +91, removed +1) | counts only; no CSV quoting, no duplicate-key concept, no diff |

### Where the answers differ

Nothing in the table disagrees about which *cells* changed. Every disagreement is one of two design
choices about **duplicate keys**, and the generated data carries them precisely so that it shows: at a
million rows, 100 keys appear twice in A and 50 appear twice in B, one of which is duplicated in
both.

**Tools with no concept of a duplicate key** — daff, datacompy — read the repeated row as an insert
on one side and a delete on the other. daff's `added +50 / removed +100` is exactly those duplicates,
one extra row reported for each one.

datacompy comes out at `+49 / +99`, one lower on each side, and the missing one is not a rounding
difference: it pairs duplicate occurrences positionally, so the second copy in A is matched against
the second copy in B. Exactly one key in this data is duplicated on *both* sides
(`ACC-00023757,TXN-00000000003`), and for that key the two leftovers cancel. It is a defensible
answer — but it is an answer to a question the tool never asks the user, and it means the count
depends on the order the duplicates appear in.

**Tools that join** — the DuckDB SQL, the pandas merge, the shell pipeline — multiply them instead. A
key twice in A and once in B joins to two rows; the one key twice on both sides joins to four. That is
151 extra rows at a million, and each one that happens to be a *changed* row is counted again — which
is why `changed` lands 7 too high there and 91 too high at ten million. The number is not a property
of the tool but of which rows happened to be duplicated, and that is the point: the join silently
inflates the diff by an amount nobody can predict, and nothing in the output says a duplicate key was
involved.

This project reports duplicates as their own section, joins on the first occurrence of each key, and
counts *keys* rather than rows. That is a choice too — but it is a stated one, and it is why the
counts here differ from a plain `FULL OUTER JOIN` on the same data.

The Go tool avoids both traps and agrees with us exactly. Its raw output marks 60,053 rows modified
where we report 60,049, and that gap is the same row-versus-key distinction rather than a
disagreement: it marks rows, we count keys, and the four extra are duplicate rows of keys already
counted. Reduced to distinct keys its answer is identical to ours, which is what the table shows.

### What the field says

**At ten million rows, most of the field cannot run at all.** Six of the nine approaches fail on
3.68 GB of input in a 12 GB budget: the Go tool, both datacompy backends, the pandas merge, csv-diff,
and daff. The interesting thing is not that they are slow — it is that "fastest" stops being the
question. What is left is this project's two engines, a hand-written SQL join, and a shell pipeline.

**daff's ceiling is not memory.** It reads the file with `readFileSync`, and V8 refuses to build a
string longer than 512 MB. No amount of RAM moves that limit, so daff cannot open a file this size on
any machine. Every other failure above is a genuine memory exhaustion; this one is a wall.

**The backend decides a dataframe's speed, not the library.** At a million rows datacompy takes
29.83s on pandas and 4.09s on Polars — the same library, the same call, 7.3x apart. That gap is
wider than any language difference in this whole project. If a dataframe reconciliation is slow, the
first question is which backend it is on.

**The dedicated tool is faster and hungrier.** csvdiff (Go) finishes a million rows in 2.81s against
our 4.94s, and uses 1,482 MB against our 667 MB — while storing strictly less: two hashes per row,
which is why it can say a row changed but not which cell. Then at ten million it runs out of memory
and we do not. That is the "no String per cell" design earning its keep against a tool that
deliberately keeps less.

**Everything agrees about cells. Nothing agrees about duplicate keys.** Not one tool in this table
found a changed cell another missed. Every difference in the counts is duplicate-key handling, and
**none of the eight external tools reports duplicate keys at all** — they either fold them in
silently or multiply them into the answer. On data that has any, three of these approaches will hand
you an inflated diff and say nothing about why.

**The shell pipeline is better than it has any right to be.** `sort | join` does a million rows in
8.23s and 251 MB, and ten million in 119.75s and 2,483 MB — less memory than anything else that
finished, because `sort` spills. It is the right instinct: an external sort-merge join is the correct
algorithm for data larger than memory. What it cannot do is parse CSV — a comma, a quote or a newline
inside a field and the answer is silently wrong — or tell you which cell changed.

That instinct is why the `sortmerge` engine exists. It is the same algorithm with a real CSV parser
and the full result contract, and it is the only thing here that produces the *correct* ten-million-row
answer in under 2 GB. It now exists in Java, Go and Rust; the Rust port does that comparison in
208 MB, which is less than the shell pipeline needs and a correct answer besides.

### The other memory question

Peak RSS says what a tool used with room to spare; it does not say what it needs. The smallest heap
each Java engine can finish a million rows in — and why `sortmerge`'s 63 MB against `native`'s
2,470 MB is the number that matters — is in
[The out-of-core engine](#the-out-of-core-engine) above.

## Ports

The same tool exists in five languages, all to one result contract: the same JSON, the same HTML
template, the same exit codes, so a report from any of them is interchangeable and a benchmark
number from one is directly comparable with a number from another.

| | Directory | Engines | Notes |
|---|---|---|---|
| Python | `.` (this) | duckdb, pandas | the reference; also has `serve` and `mail` |
| TypeScript | [`ts/`](ts/) | duckdb, polars, arquero, native | Node 26, TypeScript 7 |
| Java | [`java/`](java/) | duckdb, turbo, swar, shard, mmap, simd, tablesaw, sortmerge, native | Java 26, Maven; five byte-level engines on SWAR, the Vector API and FFM, plus an out-of-core sort-merge join |
| Go | [`go/`](go/) | duckdb, sortmerge, native | Go 1.24 |
| Rust | [`rust/`](rust/) | duckdb, polars, sortmerge, native | edition 2024 |

Two things are enforced in CI by `.github/workflows/parity.yml`, on every change to any of them:
every implementation returns identical counts and column stats for one dataset, and all five data
generators emit byte-identical files. The generator carries money in integer cents and applies the
drift to those integers, never to a float, so byte-identity does not depend on any language's
floating-point rounding rule.

## Project layout

```
csvdiff/engine.py    comparison (DuckDB + pandas fallback), result contract documented at top
csvdiff/report.py    HTML renderer
csvdiff/cli.py       compare / serve / mail
csvdiff/server.py    drag-and-drop page
csvdiff/mailbot.py   IMAP/SMTP watcher
csvdiff/config.py    profiles (csvdiff.toml)
CLAUDE.md            project context for Claude Code
.claude/             slash commands, permissions, report-editing skill
scripts/gen_data.py  deterministic test payload generator (20 columns, known drift)
scripts/bench.py     benchmark harness with budgets and job-summary output
.github/workflows/   ci, benchmark, on-demand comparison
examples/            sample files and a generated report
tests/               pytest
```
