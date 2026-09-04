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

## Ports

The same tool exists in five languages, all to one result contract: the same JSON, the same HTML
template, the same exit codes, so a report from any of them is interchangeable and a benchmark
number from one is directly comparable with a number from another.

| | Directory | Engines | Notes |
|---|---|---|---|
| Python | `.` (this) | duckdb, pandas | the reference; also has `serve` and `mail` |
| TypeScript | [`ts/`](ts/) | duckdb, polars, arquero, native | Node 26, TypeScript 7 |
| Java | [`java/`](java/) | duckdb, turbo, swar, shard, mmap, simd, tablesaw, native | Java 26, Maven; five byte-level engines on SWAR, the Vector API and FFM |
| Go | [`go/`](go/) | duckdb, native | Go 1.24 |
| Rust | [`rust/`](rust/) | duckdb, polars, native | edition 2024 |

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
