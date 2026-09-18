# Where the four ports stand

**Ten million rows, all three formats, all four ports, measured on CI.**
Run 2026-09-17.

[BENCHMARKS.md](BENCHMARKS.md) is the record of *how it got here* — every run,
newest first, with the reasoning and the dead ends. [ARCHIVE.md](ARCHIVE.md) is
what was tried and removed. This file is the snapshot.

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

## Results — AMD EPYC 9V74, 4 cores, AVX2

One job, one runner, one sitting, all three formats, three interleaved runs
each, ports rotated. `benchmark-native.yml` at 10m.
[Run 35181504756](https://github.com/andrey-usa/csvdiff/actions/runs/35181504756).

Every port compiled for that machine: `-march=native` for C and C++,
`-C target-cpu=x86-64-v3` for Rust (capped at the fleet's executable floor —
resolving the host CPU picked `znver4` and a build-time tool died with SIGILL),
`-Dcpu=native` for Zig.

### CSV — 3,509 MB

| Port | Best | Median | Worst | CPU | Above the input | vs best |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **1.83s** | 1.83s | 1.83s | 6.4s | **716 MB** | — |
| Rust | 2.10s | 2.12s | 2.13s | 6.8s | 901 MB | 1.15x |
| Zig | 2.15s | 2.23s | 2.29s | 7.1s | 874 MB | 1.17x |
| C++ | 4.51s | 4.74s | 4.74s | 13.9s | 879 MB | 2.46x |

### ndjson — 8,487 MB

| Port | Best | Median | Worst | CPU | Above the input | vs best |
|---|---:|---:|---:|---:|---:|---:|
| **Zig** | **7.20s** | 7.35s | 7.37s | 27.6s | 879 MB | — |
| C | 7.41s | 7.45s | 7.48s | **24.4s** | **716 MB** | 1.03x |
| Rust | 8.04s | 8.05s | 8.06s | 30.4s | 901 MB | 1.12x |
| C++ | 10.39s | 10.63s | 10.88s | 30.7s | 879 MB | 1.44x |

### Parquet — 2,074 MB

| Port | Best | Median | Worst | CPU | Above the input | vs best |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **1.53s** | 1.54s | 1.74s | **4.9s** | 1,297 MB | — |
| C++ | 2.59s | 2.61s | 2.66s | 8.4s | 1,483 MB | 1.69x |
| Rust | 2.79s | 2.80s | 2.82s | 9.2s | 1,365 MB | 1.82x |
| Zig | 2.93s | 2.93s | 2.95s | 10.3s | **1,264 MB** | 1.92x |

---

## What this table says

**C leads two formats of three and is never worse than second.** It also spends
the least CPU on every format it leads, so it is not winning on threading.

**Zig takes ndjson, narrowly** — 1.03x over C, which is inside the noise this
host can resolve for a ratio of bests. Call it a tie and note that C gets there
on 24.4 CPU-seconds against Zig's 27.6: C does less work, Zig uses more cores.

**C++ is last on both text formats and by a long way on CSV** — 2.46x behind C
there, 1.44x behind Zig on ndjson. It is the port with the most headroom. Its
Parquet column is second, so what is behind is the text path specifically.

That gap has been taken apart in BENCHMARKS.md and most of it is still
unexplained. What is ruled out: it is not the thread count (1.88x on a single
thread, before any thread is started), not the instruction count (1.34x), not
cache pressure at scale (1.68x at 200k too), and not misses or mispredictions —
per instruction C++ takes fewer trips to main memory for reads than C does and
mispredicts a smaller share of its branches. The one surviving lead was writes, and it has since been traced: 54% of C++'s
last-level write misses are in `row_values`, which allocates a `std::string` per
cell of every *reported* row where C slices the mapped bytes. The hot path is
fine — on scan and parse, 55% of C++'s instructions, it is 1.15x C's work and
writes fewer bytes. Nothing found so far explains the 1.88x.

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
speedup. **Where that time goes is mostly unattributed.** All four sweep in
parallel, insert into the hash index one thread per side, then join in parallel —
and the insert, which looks like the obvious culprit and was published here as
one, is only 14-16% of wall once you account for the two sides being built
concurrently rather than in sequence. Parallelising it is worth about 1.12x, not
the 1.27-1.56x this page claimed. `assemble` is another 11% and does not scale.
The remaining quarter to third has not been measured.

See [BENCHMARKS.md](BENCHMARKS.md) for the correction and how the error was
made.

---

## The ndjson ranking is not a property of the code

Four machines have measured this same tree at 10M. The ndjson ordering is not
the same on all of them — but it is not arbitrary either:

| ndjson, 10M | container<br>Xeon @2.10GHz, avx512 | CI ladder<br>Xeon Plat. 8370C, avx512 | CI<br>EPYC 9V74, avx2 | CI<br>Xeon Plat. 8573C, avx512 |
|---|---|---|---|---|
| 1st | **Rust** 5.08s | **C** 7.95s | **Zig** 7.20s | **Zig** 5.47s |
| 2nd | Zig 5.48s | Zig 8.26s | C 7.41s | C 5.68s |
| 3rd | C 6.13s | C++ 8.96s | Rust 8.04s | Rust 6.34s |
| 4th | C++ 8.76s | **Rust** 9.96s | C++ 10.39s | C++ 7.87s |

**The last two columns agree exactly** — different vendor, different vector
width, same order. That is the first ndjson ordering this project has reproduced
on independent hardware, and Zig-then-C is what the two clean CI runs say.

Rust is still first on one machine and last on another, so no ordering here is
universal, and any published one needs its CPU printed beside it. But "three
machines, three winners" was too strong a reading of the first three columns: the
orderings are not arbitrary, they are just not portable.

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

---

## What is established, and what is not

| Claim | Evidence |
|---|---|
| C leads CSV and Parquet on the EPYC 9V74 | the table above, counts-gated |
| C++ is last on both text formats | this table, and every table before it |
| C uses the least memory above its input on text | every table this project has published |
| Zig leads ndjson on the EPYC 9V74 | the table above — but by 1.03x, inside the noise floor |
| Any port "leads ndjson" in general | **refuted** — Rust is first on one CPU and last on another |
| Zig leads ndjson on CI hardware | **reproduced** on two CI CPUs (EPYC 9V74, Xeon Plat. 8573C), C ~1.03x behind |
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
# All three formats in ONE job, which is the only way to get one CPU
# across formats. This is what the table above came from.
gh workflow run benchmark-native.yml -f rows=10m -f formats=csv,json,parquet -f repeats=3

# The ladder: one job per size and format, so FASTER but several CPUs.
# Its collect job groups by processor and says so.
gh workflow run bench-ladder.yml -f sizes=10m,20m -f formats=csv,ndjson,parquet

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
