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

## Ports

The same tool exists in five languages, all to one result contract: the same JSON, the same HTML
template, the same exit codes, so a report from any of them is interchangeable and a benchmark
number from one is directly comparable with a number from another.

| | Directory | Engines | Notes |
|---|---|---|---|
| Python | `.` (this) | duckdb, pandas | the reference; also has `serve` and `mail` |
| TypeScript | [`ts/`](ts/) | duckdb, polars, arquero, native | Node 26, TypeScript 7 |
| Java | [`java/`](java/) | duckdb, tablesaw, native | Java 26, Maven |
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
