# Where the four ports stand

**Ten million rows, all three formats, all four ports, measured on CI.**
Run 2026-09-20.

Measured with **no `--json`**, so every port is doing the job all four perform.
Passing it timed four different tasks: only C++ emits row samples, and producing
them cost it 15% of its CSV wall and 30% of its CPU. The previous edition of this
file carried that asymmetry; see
[BENCHMARKS.md](BENCHMARKS.md) for the before and after on one processor.

---

## The one rule, and what CI does to it

**Compare rows within a table. Never across tables.** This project has measured
the same code 20% apart on one machine in one morning, and a port's time swing
1.57x by nothing but where it sat in the running order.

**On hosted CI a "table" is smaller than it looks.** GitHub puts several
processor generations behind one `ubuntu-latest` label. A workflow that fans
sizes or formats out across jobs — which is how the ladder gets parallelism —
collects its pieces from *different machines*. Read as one curve they measure
the fleet as much as the code.

So the harness records the CPU in its JSON beside the numbers, and
`scripts/bench_group.py` groups on it and prints one table per processor. A CI
summary showing two CPUs is showing two tables. The section below on
[the ndjson ranking](#the-ndjson-ranking-is-not-a-property-of-the-code) is what
happens when you ignore this.

---

## The workload

| | |
|---|---|
| Rows | 10,001,000 in A, 10,000,500 in B, 20 columns |
| Key | `(account_id, txn_id)` |
| Ignored | `updated_at` |
| Compared | a per-cell diff over the remaining 17 columns |

Every port returned identical counts on every format. That is the run's gate,
not a footnote — a port that disagrees fails the run and is named.

```
matched 9,990,000 · changed 599,320 · unchanged 9,390,680
added 10,000 · removed 10,000
duplicate keys 1,000 in A / 500 in B
```

---

## Results — 10m rows, three formats

[Run 35502348822](https://github.com/andrey-usa/csvdiff/actions/runs/35502348822),
`bench-ladder.yml`, three interleaved runs each, ports rotated, **no `--json`**.

**Two processors, so two tables.** The ladder fans one job per format and the
fleet gave csv and parquet an EPYC 7763 and ndjson an EPYC 9V74. Rows may be
compared inside a table and not between them — that is the rule at the top of
this file, and it is the normal case here rather than bad luck: four
consecutive dispatches have landed on more than one CPU.

Every port compiled for the machine it ran on: `-march=native` for C and C++,
`-C target-cpu=x86-64-v3` for Rust (capped at the fleet's executable floor —
resolving the host CPU picked `znver4` and a build-time tool died with SIGILL),
`-Dcpu=native` for Zig.

### CSV — AMD EPYC 7763, 4 cores, avx2

| Port | Compare | CPU | Cores | Above the input | vs best |
|---|---:|---:|---:|---:|---:|
| **C** | **1.81s** | **6.4s** | 3.53x | **716 MB** | — |
| Zig | 1.86s | 6.1s | 3.25x | 878 MB | 1.03x |
| Rust | 2.02s | 6.5s | 3.21x | 901 MB | 1.12x |
| C++ | 2.82s | 7.3s | **2.61x** | 883 MB | 1.56x |

### Parquet — AMD EPYC 7763, 4 cores, avx2

| Port | Compare | CPU | Cores | Above the input | vs best |
|---|---:|---:|---:|---:|---:|
| **C** | **1.32s** | **4.1s** | 3.14x | 1,362 MB | — |
| C++ | 1.71s | 5.4s | 3.17x | 1,308 MB | 1.30x |
| Zig | 2.42s | 8.6s | 3.57x | **1,221 MB** | 1.83x |
| Rust | 2.47s | 8.1s | 3.29x | 1,357 MB | 1.87x |

### ndjson — AMD EPYC 9V74, 4 cores, avx512

| Port | Compare | CPU | Cores | Above the input | vs best |
|---|---:|---:|---:|---:|---:|
| **C** | **5.24s** | 16.7s | 3.19x | **716 MB** | — |
| Zig | 5.79s | 22.1s | 3.82x | 878 MB | 1.11x |
| C++ | 5.88s | **14.9s** | **2.54x** | 878 MB | 1.12x |
| Rust | 6.24s | 23.4s | 3.76x | 901 MB | 1.19x |

---

## What this table says

**C leads all three formats.** It also spends the least CPU on two of them, so
it is not winning on threading.

**C++ is no longer 2.46x behind on CSV.** It is 1.56x, and the difference is not
a change to the engine: the old figure was taken with `--json`, which only C++
answers with row samples. That was a third of the gap.

**The whole of what remains is in one column, and it is `Cores`.**

| | C | C++ | Rust | Zig |
|---|---:|---:|---:|---:|
| CSV | 3.53x | **2.61x** | 3.21x | 3.25x |
| ndjson | 3.19x | **2.54x** | 3.76x | 3.82x |
| Parquet | 3.14x | 3.17x | 3.29x | 3.57x |

On the two text formats C++ runs a quarter of the machine idle that every other
port keeps busy. On Parquet — which takes the columnar path and never touches
the scanner — it is ordinary, and its result there is second.

**On ndjson C++ does less work than C and is still slower.** 14.9 CPU-seconds
against C's 16.7, the lowest of any port, and 5.88s of wall against 5.24s. At
C's 3.19x utilisation those same 14.9 seconds would finish in **4.67s** and lead
the format outright. Nothing needs to get cheaper for that; it needs to run at
the same width as everyone else's.

**CSV says the same thing more quietly.** C++ spends 7.3 CPU-seconds to C's 6.4,
1.14x — which matches the 1.07x instruction ratio measured directly on a 400k
pair. But its wall is 1.56x. Work explains a seventh of the gap and threading
explains the rest.

This is the open question, and it is a narrower one than the file used to carry.
It also matches the phase measurement in BENCHMARKS.md from a different
direction: the sweep is 23% of a four-thread run and scales **0.79x** — it gets
*slower* as threads are added, which is what a phase limited by something other
than instruction issue does.

**Memory is the flattest ranking and C's clearest win.** 716 MB above the mapped
input on *both* text formats — the same number whether the input is 3.5 GB or
8.5 GB, because a field there is one 64-bit word and never becomes a string.
Parquet inverts the column: pages are decoded into an arena rather than mapped,
so every port pays over a gigabyte and Zig pays least.

---

## The biggest number here is one no port owns

Every port scales badly from one core to four, and by nearly the same amount:

| port | 1 thread | 4 threads | 1→4 | implied serial | insert, critical path |
|---|---:|---:|---:|---:|---:|
| C | 1.52s | 0.82s | 1.86x | 38% | 14% |
| C++ | 2.74s | 1.61s | 1.70x | 45% | 15% |
| Rust | 2.00s | 1.10s | 1.81x | 40% | 14% |
| Zig | 1.48s | 0.97s | 1.53x | 54% | 16% |

(4M CSV pair, this host. `scripts/scaling_curve.py` and `scripts/serial_share.py`.)

Every port is between a third and a half serial, measured from its own 1-to-4
speedup. For C++ that time is accounted for. Re-measured on the path the
benchmark now times — **no `--json`**, 4M CSV pair, five rounds, medians, the two
index sides taken as a critical path because they run concurrently:

| phase | share of the 4-thread wall | scales 1→4 |
|---|---:|---:|
| sweep | **24.6%** | **0.73x** |
| index insert | 19.1% | 1.14x |
| join and compare | 56.1% | 2.35x |
| assemble | 0.1% | — |

Only the join scales. The **sweep is the largest non-scaling phase and gets
slower with more threads** — the signature of a phase limited by memory bandwidth
rather than by cores, on a scan of 1.5 GB. It is worse than this page used to
say: 0.73x, not 0.79x.

`assemble` is 0.1% and no longer worth a row. It was 11% when this table was
first taken and 7.8% on the same pair with `--json` still passed; gating it
behind the flag and then building only the cells the report prints took it out
of the measured path entirely.

Two checks that this decomposition is real, not fitted. The critical path sums
to **1.699s against a 1.70s wall** — 100%. And the non-scaling phases are
24.6% + 19.1% = **43.7%**, against the **43.3%** Amdahl implies from the
measured 1.74x speedup: two routes to one number, agreeing to four-tenths of a
point.

See [BENCHMARKS.md](BENCHMARKS.md) for the correction and how the error was
made.

---

## The ndjson ranking is not a property of the code

Four machines have measured this same tree at 10M. The ndjson ordering is not
the same on all of them — but it is not arbitrary either:

| ndjson, 10M | container<br>Xeon @2.10GHz, avx512 | CI ladder<br>Xeon Plat. 8370C, avx512 | CI<br>EPYC 9V74, avx2 | CI<br>Xeon Plat. 8573C, avx512 | CI 09-20<br>EPYC 9V74, avx512<br>*no `--json`* |
|---|---|---|---|---|---|
| 1st | **Rust** 5.08s | **C** 7.95s | **Zig** 7.20s | **Zig** 5.47s | **C** 5.24s |
| 2nd | Zig 5.48s | Zig 8.26s | C 7.41s | C 5.68s | Zig 5.79s |
| 3rd | C 6.13s | C++ 8.96s | Rust 8.04s | Rust 6.34s | C++ 5.88s |
| 4th | C++ 8.76s | **Rust** 9.96s | C++ 10.39s | C++ 7.87s | **Rust** 6.24s |

**Columns three and four agree exactly** — different vendor, different vector
width, same order. That was the first ndjson ordering this project reproduced on
independent hardware.

**Column five does not agree with either, on the same CPU model as column
three.** C and Zig swap, and Rust falls from third to last. The two are 1.03x
apart in column three, which this file already called a tie, so the swap at the
top is the noise floor behaving as documented — but Rust moving 3rd → 4th is
not, and no code change between the runs touched Rust's ndjson path. Read it as
one more column, not a correction of the others.

Rust is first on one machine and last on two, so no ordering here is universal,
and any published one needs its CPU printed beside it. "Three machines, three
winners" was too strong a reading of the first three columns — the orderings are
not arbitrary, they are just not portable — and five columns have not made them
more portable.

**The 8370C column was suspected of a harness confound. It is not.** It is the
only one measured by the ladder harness, which additionally passes
`--memory-cap`, and it is the column where Rust looks worst — so the cap was
tested directly: one machine, one binary, one input, the cap set to exactly what
the ladder computes, twenty-one paired rounds with the arms rotated.

| ndjson 4M, capped / free | median | middle half | slower in |
|---|---:|---|---|
| Rust | 1.002 | 0.980–1.017 | 11/21 |
| C | 1.002 | 0.971–1.018 | 11/21 |

A coin flip for both. The cap costs nothing, so it does not explain that column,
which therefore stands as an ordinary result from an ordinary CPU.

(Nine rounds of the same experiment had given Rust 1.034, slower in 6 of 9, which
looked like a finding. It was not. That is the noise floor at the top of this
file doing exactly what it says it does.)

What survives every column: **C++ is last or second-to-last on ndjson
everywhere**, and C is never worse than third. Those are the claims worth acting
on.

Column five moves C++ from last to third, and that is the report coming out of
the measurement rather than the engine getting faster — the same 30% of CPU it
stopped spending on CSV.

---

## What is established, and what is not

| Claim | Evidence |
|---|---|
| C leads all three formats in the 09-20 run | the tables above, counts-gated, no `--json` |
| C++ is last on both text formats | **no longer true on ndjson** — third of four once `--json` is not timed |
| C++'s remaining gap is threading, not work | 1.14x C's CPU on CSV and 1.56x its wall; 2.61x cores against 3.53x |
| C uses the least memory above its input on text | every table this project has published |
| Zig leads ndjson on the EPYC 9V74 | **not reproduced** — a second 9V74 run put C first by 1.11x |
| Any port "leads ndjson" in general | **refuted** — Rust is first on one CPU and last on another |
| Zig leads ndjson on CI hardware | **contested** — two CI CPUs said so, a third run on one of them says C |
| The ndjson row-end fix is worth 1.26x on C++, 1.04x on Rust | [paired A/B, 11 rounds, CSV as control](BENCHMARKS.md) |
| The same fix is worth anything on Zig | **not established** — the band crosses 1.00x |
| Instruction count predicts wall time across ports | **refuted** |
| Instruction count tracks wall time within one port | [0.790 against 0.792 measured](BENCHMARKS.md) |

---

## Two things that are not comparable, and look like they should be

**The two harnesses generate different Parquet.** `bench_formats_ports.py` uses
the Rust generator and `bench_ports.py` uses the C one, and for the same ten
million rows they emit 1,535 MB and 2,074 MB respectively. The Parquet rows from
the two harnesses are therefore not comparable even on one CPU. The CSV and
ndjson inputs are byte-identical between them (3,509 MB and 8,487 MB either way).

**The README's headline table is a different host.** It is kept as the
GitHub-runner reference point from 2026-09-09 and is not the latest measurement.

---

## Reproducing this

```sh
# The ladder: one job per size and format, so FASTER but several CPUs.
# Its collect job groups by processor and says so. This is what the tables
# above came from -- and why they are two tables and not one.
gh workflow run bench-ladder.yml -f sizes=10m -f formats=csv,ndjson,parquet -f repeats=3

# All three formats in ONE job, which is the only way to get one CPU
# across formats -- at the cost of running them in series.
gh workflow run benchmark-native.yml -f rows=10m -f formats=csv,json,parquet -f repeats=3

# Locally, one host by construction
python3 scripts/bench_formats_ports.py --rows 10m --repeats 3

# Merge any collection of result JSON, grouped by the CPU that produced it
python3 scripts/bench_group.py rungs/

# Two builds of one port -- the only way to price a single change
bash scripts/bench_ab.sh
```

`bench_ab.sh` is the one to reach for when asking *did my change help*. It prints
the middle half of the per-round paired ratios beside the median, and where that
half straddles 1.00x there is no result to report. On this class of machine about
10% is invisible to a ratio of bests and about 3% is the floor for the paired
one — which is why the 1.03x above is called a tie.
