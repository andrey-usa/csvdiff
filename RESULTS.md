# Where the four ports stand

**Ten million rows, all three formats, all four ports, measured on CI.**
Run 2026-09-26, after #117–#133.

Measured with **no `--json`**, and with `--summary` for Rust, so every port is
doing the job all four perform: count and compare, and write nothing. Both
flags were once in the measurement for one port only -- `--json` made C++ emit
row samples, and Rust rendered a report whether asked or not -- and each was
worth more than a tenth of that port's time. [BENCHMARKS.md](BENCHMARKS.md) has
the before and after of both.

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

**And a ratio of bests hides about a tenth.** Each cell is the best of three
runs. Differences under about 10% between two rows are not a ranking; this file
calls them a tie.

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

[Run 36241447420](https://github.com/andrey-usa/csvdiff/actions/runs/36241447420),
`bench-ladder.yml` on `2b38eb0`, three runs each, best kept, ports rotated.

**One processor for all twelve measurements: AMD EPYC 7763, 4 cores, avx2.**
The ladder fans one job per format, and this time the fleet happened to hand
all three the same CPU, so for once the rows are comparable *across* formats as
well as within them. That is luck, not design; the previous run got two CPUs.

Every port compiled for the machine it ran on: `-march=native` for C and C++,
`-C target-cpu=x86-64-v3` for Rust (capped at the fleet's executable floor),
`-Dcpu=native` for Zig.

`09-20` is the same cell from [run 35502348822](https://github.com/andrey-usa/csvdiff/actions/runs/35502348822),
given only where that run used the same CPU — CSV and Parquet. Its ndjson ran on
an EPYC 9V74 and has no column here.

### CSV — 3,509 MB a side

| Port | Compare | CPU | Cores | Above the input | vs best | 09-20 |
|---|---:|---:|---:|---:|---:|---:|
| **Rust** | **1.66s** | 5.5s | 3.33x | 818 MB | — | 2.02s |
| C | 1.71s | 6.0s | 3.49x | **716 MB** | 1.03x | 1.81s |
| C++ | 1.76s | 6.2s | **3.54x** | 723 MB | 1.06x | 2.82s |
| Zig | 1.76s | **4.8s** | 2.71x | 1,020 MB | 1.06x | 1.86s |

### ndjson — 8,487 MB a side

| Port | Compare | CPU | Cores | Above the input | vs best |
|---|---:|---:|---:|---:|---:|
| **C++** | **3.57s** | 13.6s | 3.79x | 726 MB | — |
| Rust | 3.58s | 13.4s | 3.75x | 876 MB | 1.00x |
| C | 3.93s | 15.0s | **3.83x** | **716 MB** | 1.10x |
| Zig | 4.03s | **12.1s** | 3.00x | 1,022 MB | 1.13x |

### Parquet — 1,535 MB a side

| Port | Compare | CPU | Cores | Above the input | vs best | 09-20 |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **1.26s** | **4.0s** | 3.20x | 1,368 MB | — | 1.32s |
| C++ | 1.71s | 5.4s | 3.15x | 1,308 MB | 1.36x | 1.71s |
| Rust | 1.96s | 6.9s | 3.50x | 1,334 MB | 1.56x | 2.47s |
| Zig | 2.42s | 8.6s | **3.56x** | **1,226 MB** | 1.92x | 2.42s |

---

## What this table says

**CSV is a four-way tie.** 1.66s to 1.76s, 6% from first to last — inside the
tenth this file says a ratio of bests cannot see. Six days ago the same CPU
spread the four ports over 1.81s to 2.82s, 1.56x.

**C++ closed the whole of its CSV gap, and it was threading, as this file
said.** 2.82s → 1.76s, and `Cores` 2.61x → 3.54x, the highest of the four. The
previous edition's open question was why C++ left a quarter of the machine idle
on text; the answer was two things. Its sweep split had been switched off at
four cores because it measured slower, and it measured slower because two
threads' chunks shared a cache line (#125). The join also never prefetched
the other table's line, which the other three ports all did (#132).

**Rust's CSV number fell 18%, and most of that is the measurement.**
2.02s → 1.66s. It was the only port rendering a report nobody asked for (#119);
the rest is its sweep no longer waiting on a serial pass over half the file
before splitting (#133).

**Zig spends the least CPU and keeps the most of the machine idle.** 4.8s of CPU
on CSV against C's 6.0, and 12.1s on ndjson against 13.4–15.0, but 2.71x and
3.00x cores where the others run 3.3–3.8x. That is #126: its sweep now runs one
thread a file, because splitting it under the allocator Zig uses without libc
made it slower rather than faster. Linking libc measured 1.72x against #126's
1.38x on one host. That's a decision for the maintainer, not a change for a
benchmark to make.

**ndjson: C++ and Rust tie for first, and C is third.** On the one CPU in this
run, C++ is first by nothing over Rust and 1.10x ahead of C. C++ was last or
second-to-last on ndjson in every table this project had printed. What changed
is #127 (a quote count that means nothing in JSON, and cost C++ half a second
a side on a 2M-row pair) and #131 (name lookup). C spends the most CPU on ndjson here, 15.0s
against C++'s 13.6s, with the highest utilisation. It's doing more work, not
using the machine worse, and that is the next thing to look at.

**Parquet is now the widest spread, and C leads it by a margin that is real.**
1.36x to C++, 1.56x to Rust, 1.92x to Zig. Rust's 2.47s → 1.96s is #119 and
#121 (a pass over B for a report sample nobody asked for). C++ and Zig are
unchanged to the hundredth, which is also a check on the fleet: same CPU, same
code, same number six days apart.

**Memory above the input: C and C++ now match on text.** 716 and 723 MB on
CSV, 716 and 726 on ndjson, the same figure whether the input is 3.5 GB or
8.5 GB, since a field there is a 64-bit word and never a string. Rust is ~100–160 MB
above them and Zig ~300 MB. On Parquet pages are decoded into an arena rather
than mapped, so every port pays over a gigabyte, and Zig pays least.

---

## The biggest number here is one no port owns

> **Measured before #124–#133.** The tables in this section were taken on a
> local 4M CSV pair with the builds of 2026-09-22. The C and C++ sweeps have
> since been unshared and re-split (#124, #125), Rust's no longer waits on a
> serial pre-pass (#133), and Zig's no longer splits (#126), so the sweep row
> below does not describe any port as it now stands. The method is still the
> one to use; the numbers need re-taking.

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

| ndjson, 10M | container<br>Xeon @2.10GHz, avx512 | CI ladder<br>Xeon Plat. 8370C, avx512 | CI<br>EPYC 9V74, avx2 | CI<br>Xeon Plat. 8573C, avx512 | CI 09-20<br>EPYC 9V74, avx512<br>*no `--json`* | CI 09-26<br>EPYC 7763, avx2<br>*after #117–#133* |
|---|---|---|---|---|---|---|
| 1st | **Rust** 5.08s | **C** 7.95s | **Zig** 7.20s | **Zig** 5.47s | **C** 5.24s | **C++** 3.57s |
| 2nd | Zig 5.48s | Zig 8.26s | C 7.41s | C 5.68s | Zig 5.79s | Rust 3.58s |
| 3rd | C 6.13s | C++ 8.96s | Rust 8.04s | Rust 6.34s | C++ 5.88s | C 3.93s |
| 4th | C++ 8.76s | **Rust** 9.96s | C++ 10.39s | C++ 7.87s | **Rust** 6.24s | Zig 4.03s |

The sixth column is different code, not only a different CPU: #127, #128,
#129, #130 and #131 all change the ndjson path, in every port. It is here for
the record and not as a reproduction of anything to its left.

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

What survived the first five columns — **C++ is last or second-to-last on
ndjson everywhere** — does not survive the sixth, where C++ is first. That is
the code changing under the claim, which is what the claim was for. C is still
never worse than third.

Column five moves C++ from last to third, and that is the report coming out of
the measurement rather than the engine getting faster — the same 30% of CPU it
stopped spending on CSV.

---

## What is established, and what is not

| Claim | Evidence |
|---|---|
| All four ports agree on every count, every format | the gate on every rung of the 09-26 run |
| CSV is a tie among all four at 10M on the EPYC 7763 | 1.66–1.76s, inside the tenth a ratio of bests cannot see |
| C leads Parquet | 1.36x over the next port in the 09-26 run; also first in 09-20 on the same CPU |
| C++'s CSV gap was threading, not work | 2.61x → 3.54x cores and 2.82s → 1.76s on the same CPU after #125 and #132 |
| C and C++ use the least memory above their input on text | 716–726 MB on both text formats, 09-26 |
| Zig does the least work on text and runs narrowest | least CPU and fewest cores on CSV and ndjson, 09-26; see #126 for why |
| C++ is last on ndjson | **no longer true** — first in the 09-26 run, after #127 and #131 |
| Any port "leads ndjson" in general | **refuted** — six columns, four different winners |
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
# above came from; on 09-26 it happened to land on one CPU, on 09-20 on two.
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
