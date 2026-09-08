# Benchmark history

Every run this project has kept, newest first. Each entry names the host, the
date and the way it was measured, because that is what makes two numbers
comparable — and what makes numbers from two different entries **not**
comparable. Compare rows within a table. Never across tables.

Terse verdicts on things the project no longer carries are in
[ARCHIVE.md](ARCHIVE.md).

## How to read these

**Interleaved, not one build at a time.** A machine's speed drifts under the
runs themselves — the page cache fills, the kernel's supply of free 2 MB pages
is picked over. Every build runs once per round, in the same order, and the
rounds repeat. A number taken now and one taken twenty minutes ago compare
machine states, not builds.

**Best, median and worst.** A build that is quick once and slow twice is not
quick, and the spread is where memory pressure shows.

**CPU is the honest measure of work.** Wall time mixes work with how many cores
the design manages to use; CPU seconds do not. Where a build wins on wall and
loses on CPU, it is winning on threading.

**Peak RSS includes the mapped input.** These engines map their files, so
resident pages include the files themselves. The column that carries
information is *above the input*, which subtracts them.

**Counts are gated, not assumed.** Every run below ends with every build
returning identical counts. A build that disagrees fails the run and is named;
none of the tables here contains a build that was fast because it was answering
a different question.

---

## 2026-09-08 — ten million rows, seven builds, three formats

GitHub Actions `ubuntu-latest`, 4 vCPU / 16 GB.
[Run 34270769223](https://github.com/andrey-usa/csvdiff/actions/runs/34270769223),
`benchmark-native.yml` at `82020eb`, five interleaved rounds each.

Seven builds because the four ports' best work was in two trees: this branch
carries the C work, and `claude/data-comparison-rust-zig-jam00m` carried the
Rust and Zig rewrites. Rather than merge two unfinished branches to measure
them, the workflow builds the second tree beside the first and gives it its own
columns. `(alt)` marks those.

10,000,000 rows × 20 columns, keyed on `(account_id, txn_id)`, `--ignore
updated_at`, per-cell diff over seventeen columns.

### CSV — 3,509 MB

| Build | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| Rust (alt) | **8.56s** | 8.58s | 8.67s | 24.8s | 4,717 MB | 1,208 MB |
| Zig (alt) | 8.82s | 8.84s | 9.02s | **24.2s** | 4,411 MB | 902 MB |
| C++ (alt) | 10.09s | 10.15s | 10.16s | 32.1s | 4,401 MB | 893 MB |
| C | 14.82s | 14.87s | 14.88s | 57.3s | 4,225 MB | **716 MB** |
| C++ | 20.13s | 20.18s | 20.26s | 63.6s | 4,404 MB | 895 MB |
| Zig | 21.14s | 21.16s | 21.28s | 36.5s | 4,233 MB | 724 MB |
| Rust | 39.71s | 39.78s | 40.03s | 39.7s | 4,431 MB | 923 MB |

### newline-delimited JSON — 8,487 MB

| Build | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| Rust (alt) | **22.96s** | 23.02s | 23.32s | **70.4s** | 9,696 MB | 1,208 MB |
| C | 23.03s | 23.10s | 23.21s | 85.7s | 9,204 MB | **717 MB** |
| Zig (alt) | 23.39s | 23.42s | 23.55s | 70.9s | 9,403 MB | 916 MB |
| C++ (alt) | 26.25s | 26.30s | 26.40s | 80.8s | 9,382 MB | 895 MB |
| C++ | 27.13s | 27.15s | 27.26s | 83.4s | 9,371 MB | 883 MB |

`Rust` and `Zig` from this tree **refused the pair** — "key column(s) missing
from one of the files" — because they do not read ndjson. A table that quietly
omitted them would read as though they had not been asked.

### uncompressed Parquet — 2,074 MB

| Build | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **1.67s** | 1.67s | 1.72s | **5.6s** | 3,359 MB | **1,286 MB** |
| C++ | 3.26s | 3.28s | 3.31s | 10.8s | 3,554 MB | 1,481 MB |
| C++ (alt) | 3.26s | 3.28s | 3.30s | 10.8s | 3,554 MB | 1,481 MB |
| Rust (alt) | 3.72s | 3.72s | 3.86s | 11.5s | 3,411 MB | 1,338 MB |
| Rust | 4.21s | 4.23s | 4.25s | 12.1s | 3,411 MB | 1,337 MB |
| Zig (alt) | 4.44s | 4.44s | 4.46s | 12.2s | 3,415 MB | 1,341 MB |
| Zig | 7.21s | 7.23s | 7.24s | 12.1s | 3,421 MB | 1,347 MB |

All seven agree: matched 9,990,000, changed 599,320, added 10,000, removed
10,000, duplicate keys 1,000 in A and 500 in B.

### What this run said

**Parquet is a different problem, and C has solved it best.** 1.67s against
3.26s for the next build, on half the CPU. The comparison stops being a
comparison of strings — see [ARCHIVE.md](ARCHIVE.md#techniques).

**On CSV, C was losing, and not on threading.** 57.3 CPU-seconds against the
alt Rust's 24.8 — 2.3x the work for 1.73x the wall time, while using more cores
(3.87 against 2.90). That is a design cost, not a scheduling one, and it is what
the next entry is about.

**The alt C++ is a text-path win only.** CSV 20.13s → 10.09s, Parquet 3.26s →
3.26s, identical to three digits. Whatever its SIMD work does, it does it in the
scanner.

**`main`'s Rust was single-threaded on CSV** — 39.71s wall against 39.7s CPU.

---

## 2026-09-08 — the C CSV path, before and after

One 4-core / 16 GB container, 2,000,000 rows (702 MB), five interleaved rounds,
both binaries built from the same tree minutes apart.

CSV, 702 MB:

| Build | Best | Median | Worst | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|
| C, after | **1.01s** | 1.19s | 1.23s | **3.2s** | 827 MB |
| C, before | 2.69s | 2.89s | 3.15s | 9.7s | 827 MB |

ndjson, 1,697 MB, same sitting:

| Build | Best | Median | Worst | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|
| C, after | **3.54s** | 3.64s | 3.99s | **11.7s** | 1,823 MB |
| C, before | 3.77s | 4.18s | 4.59s | 13.1s | 1,823 MB |

**CSV: 2.66x on wall, 3.03x on CPU, identical counts.** ndjson gets 1.06x, and
the reason is worth recording rather than hiding: a JSON object has to be walked
to its end whatever you want from it, so reading only the key columns saves the
stores and not the scan. Stopping the walk once the keys are found would save
the rest — but the full parse takes the *last* value of a repeated key and an
early exit would take the first, and the two parses have to agree on what a
row's key is or lookups fail. That is a deliberate change, not a tweak, and it
is open.

Both changes are about not parsing what nothing reads: the sweep and the
hash-confirmation probes take only the key columns, and the projection gets from
a file column to its slot through a precomputed chain instead of scanning all
twenty per column. See [c/README.md](c/README.md).
