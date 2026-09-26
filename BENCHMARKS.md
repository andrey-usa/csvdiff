# Benchmark history

Every run this project has kept, newest first. Each entry names the host, the
date and the way it was measured, because that is what makes two numbers
comparable — and what makes numbers from two different entries **not**
comparable. Compare rows within a table. Never across tables.

Terse verdicts on things the project no longer carries are in
[ARCHIVE.md](ARCHIVE.md).

## How to read these

**One CPU per table, and CI does not give you that for free.** Every number is
only comparable with another taken on the same processor. On GitHub's hosted
fleet that is not a formality: one `ubuntu-latest` label covers several
generations, so two jobs in one workflow run can land on a Xeon Platinum 8370C
and an EPYC 7763 — different cache, different core layout, a different AVX-512
story. A ladder that runs one size per job, or a matrix that runs one format per
job, produces pieces measured on *different machines*; read as a single curve
they measure the fleet as much as the code. So `bench_formats_ports.py` records
the CPU in its JSON beside the numbers, and `scripts/bench_group.py` groups on it
and prints one table per processor, naming any rung that is missing from a group
rather than filling it in from another. A run whose summary shows two CPUs has
produced two tables, whatever it looks like.

**Interleaved, not one build at a time.** A machine's speed drifts under the
runs themselves — the page cache fills, the kernel's supply of free 2 MB pages
is picked over. Every build runs once per round and the rounds repeat. A number
taken now and one taken twenty minutes ago compare machine states, not builds.

**The starting build rotates between rounds.** In a fixed order one build is
always first into a cold page cache and one is always last, and that position
quietly becomes part of its number. Rotating the start spreads both ends over
every build instead of assigning them. It also decides who survives a run that
is killed partway: a table that always ran the ports in the same order left the
ones at the back with no measurement at all, which reads like slowness and is
not. Tables are still printed in the declared order, so two runs can be read
side by side, and each format's rows are written out as soon as they are
measured rather than at the end.

**Best, median and worst.** A build that is quick once and slow twice is not
quick, and the spread is where memory pressure shows.

**CPU is the honest measure of work.** Wall time mixes work with how many cores
the design manages to use; CPU seconds do not. Where a build wins on wall and
loses on CPU, it is winning on threading.

**Peak RSS includes the mapped input.** These engines map their files, so
resident pages include the files themselves. The column that carries
information is *above the input*, which subtracts them.

**A negative *above the input* is not a benchmark.** It is the same column read
the other way: a port that peaked *below* the bytes it mapped did not hold its
own input, because the kernel was taking pages back while it was still using
them. Every time in such a row is a page-fault ranking, not a parsing one, and
does not belong beside a rung that fit. `bench_formats_ports.py` now says so
before the rung runs -- it refuses to call a pair a benchmark once it passes
80% of the machine's RAM, leaving room for the indexes the ports build on top
-- and `bench_group.py` says so again afterwards, naming the rows that did it.
The rung still runs either way: which port degrades worst under paging is its
own kind of answer, and a number with a stated caveat beats no number.

**Counts are gated, not assumed.** Every run below ends with every build
returning identical counts. A build that disagrees fails the run and is named;
none of the tables here contains a build that was fast because it was answering
a different question.

**Same counts is not the same task.** The gate above proves the ports agree on
*what changed*. It says nothing about what else they were asked to produce, and
for most of this file's life they were not asked for the same thing. Every
harness passed `--json` to all four ports, and only the C++ port emits row
samples: C and Zig write `counts` and `columns` -- 1,196 bytes on a 4M pair --
Rust adds a `meta` block for 3,057, and C++ writes **4,917,335 bytes naming
58,600 rows**. C has no flag to turn samples on, because it has no such feature.

So `--json` cost C 39,250 instructions on a 400k pair, 0.0%, and cost C++ 808
million, 44%: it switches on a second full random-probed pass over B and then
materialises, sorts and writes every sampled row. Measured on a 4M pair at four
threads, warmed and with the two modes interleaved, **C++ is 1.81x C on the task
all four ports perform and 2.25x once C++ alone is asked for a report** -- about
a third of the published CSV gap was the report.

The timed runs no longer pass `--json`. The counts gate still needs the
document, so it gets its own untimed run first. `scripts/report_cost.py` prints
what each port's document contains beside what producing it costs, in that
order, because the shape is the reason for the cost. This is the same fault as
the `-march=native` one below, and `ports()` already fixed the matching case for
Rust by passing `-o /dev/null` so its HTML was not charged against ports that
render none; C++'s samples were the half that was missed.

**What this harness can and cannot resolve.** Measured, not assumed: running one
build against a copy of itself, where the true answer is 1.00x.

| Comparing two builds by | Same build twice reads as |
|---|---:|
| separate bests, one build's runs then the other's (hyperfine's model) | 15.8% apart, worst of eight |
| separate bests, interleaved rounds | 8.3% apart, worst of eight |
| separate bests, interleaved, 5 rounds → 15 rounds | 9.1% → **9.4%** |
| the median of the per-round paired ratios | **1.01x** |

The third row is the one that matters: three times the rounds did not help,
because what this machine does is *drift*, not jitter, and averaging does not
touch drift. Comparing the two builds *within* each round does, because the two
runs are seconds apart under one machine state. `scripts/bench_ab.sh` does that
and prints the middle half of the per-round ratios beside the median; where that
half straddles 1.00x, there is no result to report.

So: a difference under about 10% is invisible to a ratio of bests here, and
about 3% is the floor for the paired one. Ratios below those are the machine.

**Every cross-port table before the 11:23 run was built unfairly**, and is kept
rather than deleted because the numbers within each port are still that port's.
C and C++ carried `-march=native`; Rust and Zig were given a generic baseline,
which compiles their wide scanners out entirely — the Rust one is selected by
`cfg!(target_feature = "avx2")` and the Zig one by the cpu it is handed. So the
C-versus-Rust and C-versus-Zig ratios in those tables are partly build flags,
and correcting it cost about two thirds of the published C lead on CSV and on
Parquet. Every port is now built for the runner it is measured on. It was the
other branch's agent that pointed this out.

---

## 2026-09-26 (zig parquet b pass) — Zig walked B's keys through A's table to count what it already knew

The CI phase runs put Zig's Parquet match sweep at twice C's join. It ran both
directions: A's keys against B's table for the pairs, and B's keys against A's
table only to count B's keys with no mate. That count needs no pass. A key matches
from either side or from neither, so B's distinct keys with a mate number exactly
the pairs the A pass found, and `added` is B's distinct keys minus that. C and C++
run the B pass only when a report samples added rows, the Rust port stopped in
#121, and Zig never prints row samples at all.

| 2M Parquet pair | main | this |
|---|---:|---:|
| match sweep phase | 0.712 s | 0.391 s |

Paired, 4 vCPU Xeon @ 2.10 GHz:

| | wall [mid half] | cpu [mid half] |
|---|---|---|
| 2M, 15 rounds | **1.13x** 1.07-1.18 | 1.14x 1.12-1.21 |
| 10M, 9 rounds | **1.19x** 1.05-1.22 | 1.23x 1.12-1.25 |

Counts match C's on four pairs, including 1M against 2M both ways round, where
1,001,000 added rows are derived rather than counted. A new case in
`zig/test.sh` runs files of 20k and 7k rows both ways and requires the Parquet
answer to equal the CSV engine's, which still counts `added` directly. It
fails if the subtraction is dropped (checked by breaking it).

---

## 2026-09-26 (cpp parquet report) — C++ built a Parquet report on every run and printed a line of counts

Found by reading every port's `CSVDIFF_PHASES=1` output side by side. On 2M rows of Parquet
the C++ port's `report` phase took 0.046-0.050 s of a 0.54 s run that asked
for no report: with no `--json` it ends in one line of counts. Everything in
that phase is rows nobody reads: the key of up to `max_rows` (50,000) changed
pairs decoded into strings, the added and removed rows built from their
columns, three sorts over them, and a pass over every distinct key of both
files for the duplicate-key sections.

The CSV engine has skipped the same work under `row_lists` since #106; the
Parquet one never asked. It does now, and the counts never depended on any of
it.

| 2M Parquet pair, 4 vCPU Xeon @ 2.10 GHz | before | after |
|---|---:|---:|
| `report` phase | 0.046-0.050 s | 0.001 s |
| wall, paired, 21 rounds | | 1.09x [1.04-1.16] |
| wall at 10M, paired, 11 rounds | | 1.03x [1.01-1.06] |

The wall figures sit at this host's floor (0.88-1.08 the same day), and at 10M
the saving is a smaller share because the work was capped at 50,000 rows and
doesn't grow with the file. The claim is the phase, which is deterministic. The
point is the one #106 and #119 made: a benchmark of four ports should time four
ports doing the same job. Summary and `--json` output are identical to main's.

---

## 2026-09-26 (speculative split) — Rust's sweep did not scale because of the step before it

The Rust sweep took as long at four threads as at one on the 4M CSV pair,
0.34 s a side, where C's halves. Timed from inside: each of the two chunks
finished in 0.15-0.22 s, and `chunk_bounds` took **0.13-0.16 s a side before
either started**. At two chunks a file, its quote count is one task over half
the file, on one thread. A notes entry once said removing the count "didn't
help". Timed directly, it is nearly half the phase.

It isn't slow counting. A vectorisable rewrite changed nothing, and a second
pass over the same bytes still took 148 ms (80 ms on a native build). It's
reading 368 MB a side on one thread before the parallel part begins.

So the count isn't done up front any more. For CSV the split is guessed at
the next newline, exactly as for JSON, and **checked afterwards**. Chunk 0
starts at a real row, so the row it finishes on ends at the first real row
start past its boundary. If that is where chunk 1 began, the boundary was real,
and the same argument carries down the line. If any guess was wrong (a quoted
newline at the split), the sweep runs again on counted bounds, which is exactly
what it did before. A chunk that started on a wrong guess parsed garbage, so its
rows and its errors are discarded unread.

---

## 2026-09-26 (cpp join prefetch) — the one join in four that did not prefetch

At one thread the C++ join was 1.43x C's on the 4M CSV pair (1.36 s against
0.95 s), which puts the gap in per-row work and not in threading. Callgrind on
a 400k pair says it isn't instructions: 1.82 B against C's 1.71 B, 7% apart.
So it's memory. Every join probes the other file's table once per key, in this
file's order, and the table is far too big to cache. C, Rust and Zig all ask
for that line rows ahead, since the hash that decides it is already in hand. C++
did it in its insert loop and never in either join pass.

`RowIndex::prefetch(hash)`, called 24 rows ahead (the insert's distance) in the
A pass and the B pass.

4M CSV pair, `-k account_id,txn_id -i updated_at`, 4 vCPU Xeon @ 2.10 GHz,
paired against main, 15 rounds, reports identical:

| | wall [mid half] | cpu [mid half] |
|---|---|---|
| four threads (two chunks a file) | **1.20x** 1.15-1.24 | **1.18x** 1.13-1.21 |
| two threads (one chunk a file: nothing to guess) | 0.99x 0.94-1.10 (no result) | 1.00x 0.95-1.05 |

The two-thread row is the control: no split, no count, no change.

Two tests cover the guess. `chunk_boundaries_land_between_rows_in_a_quoted_file`
already existed; seven of each row's eight newlines are quoted, so the guess
almost always fails and the fallback runs. It fails if the check is disabled.
The new `a_wrong_split_guess_cannot_fail_the_run` places a legal 7.9 MB quoted
field across the middle of the file so the guessed chunk reads a field of over
8 MB and raises an error that belongs to no row. It fails if chunk errors are
surfaced before their boundary is checked.

C and C++ count the same way before their sweeps: C serially, C++ in one task
at two chunks a file.

---

| one thread | **1.17x** 1.12-1.21 | 1.13x 1.08-1.17 |
| four threads | **1.08x** 1.06-1.12 | **1.12x** 1.11-1.14 |
| ndjson 2M, four threads | 1.03x 0.99-1.11 (no result) | 1.07x 1.00-1.14 |

The join phase alone at one thread: 1.28-1.46 s to 1.03-1.20 s. On ndjson on
main, the serial quote count (#127) is most of a four-thread run and hides it.
Parquet goes through `pqdiff.cpp`, which already prefetches.

---

## 2026-09-26 (cpp slot guess) — the name lookup change, in C++

#129 (C) and #130 (Rust, Zig) try the member after slot *i* as slot *i + 1*
before hashing its name, and hash the misses a word at a time rather than a
byte at a time. This is the same change for C++: `canon_[i]` holds what
`slot_for` answers for `wanted_[i]`, so a right guess gives the table's slot
even when one name fills two.

2M-row ndjson pair, `-k account_id,txn_id -i updated_at`, 4 vCPU Xeon @
2.10 GHz, paired against main, 15 rounds. Reports identical:

| | wall [mid half] | cpu [mid half] |
|---|---|---|
| one thread | **1.11x** 1.08-1.19 | 1.11x 1.08-1.16 |
| four threads | **1.09x** 1.04-1.10 | **1.10x** 1.05-1.12 |

The same day's floor was 0.88-1.08 wall and 0.95-1.06 CPU, so the CPU columns
are the result and the wall columns agree with them. Identical `--json`
reports, too, on a hand-built file with members reversed, shuffled, repeated
and missing, including `-k id --compare id,b,c`, where one name fills two
slots. `cpp/test.sh` passes.

---

## 2026-09-26 (json slot guess) — the C name lookup change, in Rust and Zig

Rust and Zig find a member's slot the way C did before #129: an FNV hash a byte
at a time, then a table probe. Same three changes, adapted to each port:
the member after slot *i* is tried as *i + 1* first (`canon[i]` holds the
table's own answer, so a right guess returns the table's slot even when one
name fills two), and a word-at-a-time name hash for the misses. Rust's and
Zig's names already carry their length, so there was no `strlen` to remove.

2M-row ndjson pair, `-k account_id,txn_id -i updated_at`, 4 vCPU Xeon @
2.10 GHz, paired, each against the same port with #128. Reports identical:

| | wall [mid half] | cpu [mid half] |
|---|---|---|
| Rust, one thread | **1.16x** 1.10-1.25 | 1.14x 1.09-1.20 |
| Rust, four threads | **1.09x** 1.04-1.15 | **1.11x** 1.07-1.15 |
| Zig, one thread | **1.14x** 1.12-1.18 | 1.12x 1.10-1.14 |
| Zig with #126, four threads | **1.12x** 1.11-1.15 | **1.13x** 1.11-1.17 |
| Zig on main, four threads, run 1 | 0.93x 0.87-1.03 | 0.93x 0.80-0.96 |
| Zig on main, four threads, run 2 | 0.87x 0.78-1.04 (no result) | 0.74x 0.66-1.04 |
| floor that day: one build against itself | 0.95x 0.88-1.08 | 0.98x 0.95-1.06 |

The two Zig-on-main rows are the configuration #126 is about: the sweep split
two ways a file under `smp_allocator`, where the same build's user time wanders.
In run 2 the new build's own CPU went from 2.20 s best to 3.32 s median. The
change allocates nothing, and with the split off (#126) or at one thread it
measures clean and positive. Those rows are recorded, not explained.

A unit test in each port puts one name in two slots and fails if a right guess
returns its own slot instead of the table's (checked by breaking it).
`c/test.sh --with-ports`: 89/89.

---

## 2026-09-26 (json names) — C looked up every member's name as if it had never seen the row before

Profiled with callgrind on a 200k-row ndjson pair at one thread (an x86-64-v3
build, because valgrind cannot decode AVX-512): the join's full parse of each
A row was **52%** of all instructions, about 5,900 a row, and the name lookup
inside it was the largest single part -- `parser_slot_for` at 98 instructions
a member, twenty members a row. Each lookup hashed the name a byte at a time
(FNV: a serial multiply per byte), then took a `strlen` of the candidate before
comparing it.

Three changes, all in how a name finds its slot:

- **Guess first.** Rows of one file list their names in the same order, and
  usually in slot order, so the member after slot *i* is tried as slot *i + 1*
  with a length compare and a `memcmp` before any hashing. A wrong guess costs
  those two compares and falls through to the table. `want_slot[i]` holds what
  the table answers for `want[i]`, so a right guess returns exactly the slot
  the table would have, even when one name fills two slots.
- **The table's entries carry the name's hash and length**, so a probe rejects
  a different name on one compare and no lookup calls `strlen`.
- **A word-at-a-time name hash** in place of the byte loop.

| | main | this |
|---|---:|---:|
| instructions, 200k pair, one thread | 2.63 B | **2.13 B** (-19%) |

Paired, 2M-row pair (849 MB a side), `-k account_id,txn_id -i updated_at`,
4 vCPU Xeon @ 2.10 GHz:

| | wall [mid half] | cpu [mid half] |
|---|---|---|
| one thread, against main | **1.17x** 1.13-1.22 | 1.14x 1.11-1.20 |
| four threads, against main | 1.04x 0.97-1.07 (no result) | 1.07x 1.06-1.09 |
| four threads, both with #127, 25 rounds | 1.12x 1.05-1.20 | 1.09x 1.04-1.15 |
| floor: one build against itself, same day | 0.95x 0.88-1.08 | 0.98x 0.95-1.06 |

The one-thread row and the instruction count are the result. At four threads
on main the serial quote count (#127) is most of the run and hides it. On top
of #127 the wall figure is consistent but its lower edge is inside this
machine's floor, so it's reported and not claimed.

Reports are byte-identical to main's on the 2M pair and on a hand-built file
with members reversed, shuffled, repeated and missing, under three key sets.
`c/test.sh --with-ports`: 89/89.

C++, Rust and Zig look names up the same way (FNV byte loop, table
probe) and are candidates for the same change.

---

## 2026-09-26 (json keys) — Zig and Rust parsed every field of an ndjson row to find two

The sweep only needs a row's key. For CSV every port stops at the last key
column. For ndjson, C and C++ stop as soon as each key slot is filled, and skip
the rest of the object with one scan for the newline. Zig and Rust did not:
their key-only parser went through the same object walk as the full one, so
every string of every row went through `skip_json_string` to find two values
near the front.

They now stop the same way. That needs the rule C states: **a key column takes
its first value**. The key-only parse stops at the first one, and a full parse
that kept the last value of a repeated name would disagree on the row's key,
so the lookup would miss its own row. Compared columns keep last-wins. Before
this, Zig and Rust used last-wins for a repeated key name and C and C++ used
first-wins; now all four agree. A unit test in each port pins it down.

2M-row ndjson pair (849 MB a side, `-k account_id,txn_id -i updated_at`, four
cores), reports identical apart from timing. The session's container moved
host partway through: the two Zig-on-main rows ran on the earlier host, the
rest on a 2.10 GHz Xeon. Each ratio compares two builds on one machine, and no
row is compared across hosts:

| | sweep, one thread (a side) | wall old/new, paired [mid half] | cpu old/new |
|---|---|---|---|
| Rust (main) | 0.78 s → 0.28 s | **1.34x** 1.31-1.42 | 1.46x |
| Zig (main) | 1.10 s → 0.57 s | **1.33x** 1.16-1.44 | 1.42x |
| Zig (with #126) | 1.21 s → 0.30 s at the default | **1.71x** 1.63-1.79 | 1.47x |

The Zig gain on main is capped by the split sweep under `smp_allocator`
(#126): with the split on, the default-thread sweep only went from 0.61 s to
0.59 s, even though the work per row fell by half. With #126's single chunk a
file it takes the whole saving.

---

## 2026-09-26 (json bounds) — C and C++ counted quotes in ndjson, where they mean nothing

Both ports split a large file for the sweep by counting the `"` bytes before
each nominal split: in CSV a newline inside quotes is not a row boundary, and
the parity says whether a split point is inside a field. They did it for every
dialect. In ndjson a raw newline cannot appear inside a string (RFC 8259 forbids
unescaped control characters), so every newline ends a record and the count
answers nothing. Rust and Zig already skipped it for JSON; C and C++ did not.

ndjson quotes every key and most values, so the count stops every few bytes.
On a 2M-row pair (849 MB a side, `-k account_id,txn_id -i updated_at`, four
cores, 2.80 GHz Xeon):

| | before | after |
|---|---|---|
| C++ `chunk bounds`, a side (`CSVDIFF_PHASES=1`) | 0.50 s | 0.000 s |
| C `both indexes` (the count is serial and unmarked in C) | 0.83 s | 0.33 s |

Paired, 15 interleaved rounds (`scripts/bench_ab.sh`), reports identical apart
from the elapsed-seconds field:

| port | wall old/new [mid half] | cpu old/new [mid half] |
|---|---|---|
| C (main) | **1.45x** 1.37-1.51 | 1.29x 1.18-1.34 |
| C++ (with #125's split) | **1.66x** 1.59-1.70 | 1.36x 1.35-1.39 |

The C++ number is measured on top of #125, which turns the sweep split on at
two threads a file. On main at four cores C++ does not split (`budget >= 8`),
so the change does nothing there until #125 merges -- but it does on any runner
with eight or more.

The count was also wrong, not only slow: an escaped `\"` toggles the parity,
and a split whose count comes out odd walks to the end of the file looking for
a newline outside quotes, finds none, and the chunk is dropped -- the sweep
runs on fewer threads than it was given. The output is right either way, which
is why no test saw it.

---

## 2026-09-24 (zig split) — the sweep got slower with more threads, and it was the allocator

Zig was the slowest port on the 4M CSV pair and its sweep was the whole of it.
Two earlier explanations were checked and ruled out -- it is compiled for the
host, and its sweep is not oversubscribed -- and a third, widening the scan
step, could not be resolved on a noisy machine. The one-thread control settled
where to look:

| sweep, a side | 1 thread | 4 threads |
|---|---:|---:|
| C | 0.342s | 0.181s |
| **Zig** | **0.35s** | **0.46-0.58s** |

**Zig's sweep got slower with more threads.** At four, each file is split two
ways, and each of those threads ran at well under half the speed of one.

### Isolated, one variable at a time

| build | sweep at 4 threads |
|---|---:|
| shipped: no libc, `smp_allocator` | 0.46-0.58s |
| libc linked, `c_allocator` | **0.30s** |
| libc linked, `smp_allocator` forced back | 0.49s |

Linking libc changes two things -- the allocator and `memcpy`/`memset` -- and
the third row separates them. **It is the allocator.**

What it is *not*, each measured:

* **The serial quote count.** `chunkBounds` counts quotes over half the file on
  the calling thread before any sweep thread starts. Skipping it -- correct on
  this file, which has none -- moved nothing: 0.465s against 0.454s.
* **Syscalls.** `strace -c`: 430 against glibc's 373, 0.11s against 0.06s.
* **Page faults.** 112,984 against 124,652 for glibc; flat across thread counts.
* **Blocking.** Single-digit voluntary context switches at every thread count.
* **The sweep's own arrays growing.** `ArrayList` grows 1.5x a step, one
  `mremap` each: 192 inside the sweep window. Pre-sizing them took that to 60
  and moved the sweep from 0.49s to 0.42s -- still slower than one thread.

What differs is **user time**, 3.60s against 2.67s for identical code, with no
allocation on the per-row path at all. Where in user space it goes is not
established here; the leading candidate is where the page allocator places the
arrays rather than any call into it, and without a profiler on this host that
stays a candidate.

### What changed

Each file is swept on one thread. Four-core runner, 4M CSV pair:

| | shipped | no split | paired [mid half] |
|---|---:|---:|---|
| whole run, wall | 1.743s | 1.144s | **1.38x** [1.25-1.84] |
| whole run, CPU | 5.91s | 2.79s | **2.02x** [1.70-2.43] |
| `--max-memory 1500` | 3.71s | 1.76s | one run each |

Half the CPU. Under a budget -- which is what this port is for -- the budgeted
allocator takes a lock, and the gain is larger again. Parquet goes through
`pqdiff.zig` and does not read `per_file`: 0.98x [0.89-1.02], no result.

**Linking libc would buy the split back and more** -- 1.72x wall and 2.15x CPU
against this change's 1.38x and 2.02x -- and would put Zig on the same allocator
as the other three ports. This port links no libc deliberately (see the zstd
entry), so that is left as a decision rather than taken as a side effect of a
sweep.

---

## 2026-09-24 (cpp sweep) — #109 measured the right thing and named the wrong cause

#109 tried splitting the C++ sweep across threads and found it did not pay, on a
curve where every width was worse than not splitting at all:

    per_file  1     2      3      4      6      8
    vs two    0.923 1.000  0.965  0.936  0.936  0.903

It concluded *"the sweep does not scale on this machine at any width"*, and set
`per_file = budget >= 8 ? budget / 2 : 1`, which on every four-core runner means
one chunk and no split.

**The measurement was right and the cause was not.** Two threads' `Chunk`s sat
forty-eight bytes apart in one `std::vector<Chunk>`, and `push_back` on either of
their vectors touches the struct's pointers on every row. The split was paying
for a cache line moving between cores once per row, not for the chunking.

With `Chunk` padded to a cache line -- the same fix the C port needed, found the
same afternoon -- the split is worth having:

| against no split, 4M pair, `--threads 4` | wall | CPU |
|---|---|---|
| `per_file = budget / 2` | **1.11x** | 0.96x |
| `per_file = budget` | 1.11x | 0.94x |

Same wall either way and `budget / 2` costs less CPU for it, which is also the
rule the C port has always used: both files are swept at once, so half the
machine each is the whole of it.

Shipped against new, 4M pair, `--threads 4`:

| | shipped | new | paired [mid half] |
|---|---:|---:|---|
| sweep phase | 0.391s | 0.211s | **1.822x** [1.780-2.000] |
| whole run, wall | 1.350s | 1.225s | **1.11x** [1.07-1.16] |
| whole run, CPU | 3.74s | 3.87s | 0.96x [0.94-1.00] |

Eleven percent of the wall for four percent more CPU is what threading is for,
and the sweep phase itself nearly halves.

**The lesson is not that #109 was careless.** It measured nine interleaved
rounds, tabulated six widths, and drew the only conclusion those numbers
support. A width curve cannot distinguish "this work does not parallelise" from
"this work parallelises and something else is serialising it", and nothing in
the phase timings said which. What separated them here was the one-thread
control from the false-sharing entry below: a cost that vanishes when there is
only one thread is not a cost of the work.

---

## 2026-09-24 (false sharing) — the C port's threads were fighting over two cache lines

The C join scaled to **2.8x** on four cores. The Rust port does the same work in
the same time on one core -- 1.26s against 1.33s serial -- and reaches **3.9x**.
Same machine, same pair, same afternoon.

### The obvious cause, priced and rejected

C cuts the join into one range per thread; Rust pulls sixty-one chunks from a
queue, and its comment says why: a chunk that turns out expensive should delay
one worker rather than three. So the queue was built for C first.

| | speedup |
|---|---:|
| one range per thread | 2.80x |
| sixty-one chunks from a queue | 2.76x |

**Nothing.** Imbalance was not it.

### What it was

`CmpPart` is one per thread in a single `calloc`ed array, about ninety bytes
apart. Two of them share a cache line, so `out->matched++` on one thread dirties
the line another thread is incrementing its own counters in, and the line moves
between cores. Four million rows of that shows up as nothing in particular and
everything in the scaling.

`Chunk` has it worse: one per sweep thread, forty bytes apart, and `chunk_push`
touches `n`, `cap` and both pointers on *every row*.

Both padded to a cache line, on the 4M CSV pair, phases alternating and paired
per run:

| | before | after | paired [mid half] |
|---|---:|---:|---|
| join | 0.485s | 0.352s | **1.384x** [1.260-1.448] |
| sweep | 0.218s | 0.152s | **1.354x** [1.322-1.676] |
| whole run, wall | 1.075s | 0.772s | **1.35x** [1.26-1.47] |
| whole run, CPU | 3.09s | 2.32s | **1.36x** [1.26-1.39] |

The join's speedup goes 2.80x to 3.91x, which is Rust's 3.85x.

### The check that makes it a diagnosis rather than a number

False sharing costs nothing when there is nothing to share. So the same pair of
builds, on the same join, at one thread and at four:

| threads | paired [mid half] |
|---|---|
| 1 | **0.987x** [0.933-1.041] |
| 4 | **1.210x** [1.136-1.344] |

Exactly nothing at one thread, and the whole of it at four. That is the
signature, and without it "padding made it faster" would have been a result
without a reason.

### On two different processors, which was not on purpose

The container was replaced part-way through this work and came back on a
different CPU -- a 2.80GHz Xeon where the numbers above were taken on a 2.10GHz
one. Everything was rebuilt and re-measured there, which by the rule at the top
of this file is a second table and not a continuation of the first:

| | 2.10GHz Xeon | 2.80GHz Xeon |
|---|---|---|
| whole run, wall | 1.35x [1.26-1.47] | 1.22x [1.14-1.33] |
| whole run, CPU | 1.36x [1.26-1.39] | 1.22x [1.15-1.32] |
| join at 1 thread | 0.987x [0.933-1.041] | 0.982x [0.928-1.044] |
| join at 4 threads | 1.210x [1.136-1.344] | 1.282x [1.069-1.333] |
| the floor, one build against itself | 0.99x [0.92-1.07] | 1.02x [0.96-1.09] |

Different sizes, same shape, and the one-thread control is nothing on both. Two
machines agreeing on a mechanism is worth more than either agreeing with itself,
and it was an accident.

### It is not a general truth about the design

Both sibling ports have the same shape and neither wants the fix:

| port | the same padding |
|---|---|
| Zig -- one `Chunk` per sweep thread in a `gpa.alloc` array, `at.append` per row | **0.966x** [0.948-1.138] -- no result |
| C++ -- `std::vector<Chunk>` and `std::vector<Part>`, `push_back` and `matched++` per row | **0.95x** [0.93-0.99] -- slower, and inside the floor |

The same is true of C's own Parquet path: `Part` there is one per join thread in
a `calloc`ed array, forty bytes apart, and `join_part` writes `pa[n++]` for every
key it matches. Padded, the join phase measures 1.082x [1.000-1.230] and the
whole run 1.00x [0.95-1.02] -- no result, so it is not in this change either.
That path's join is a tenth of a second of a third of a second, and an integer
compare per key rather than a row parse.

So this is not "per-thread accumulators must be padded". It is about what the
per-row update compiles to, and about how much of the run is spent doing it. `chunk_push` is a call through a pointer and has to
reload `n` and `cap` from the struct every row; `ArrayList.append` and
`std::vector::push_back` are fully visible to the optimiser, which keeps them in
registers across the loop and touches the struct only when it grows. The line
that moves between cores in the C port is barely read in the other two, and
padding there only adds sixty-four bytes of footprint per chunk.

The phase floor on this host, one build against itself by the same alternating
method, is 0.99x [0.91-1.12]. Every number kept above is clear of it; the two
that are not -- the queue, and the Zig padding -- are reported as no result and
not built.

---

## 2026-09-24 (zig scan width) — the phase got faster and the run did not

With the Rust port's measurement corrected, the 4M CSV table on this host reads:

| Port | Best | Median | CPU |
|---|---:|---:|---:|
| Rust | 0.62s | 0.63s | 1.9s |
| C | 0.66s | 0.72s | 2.1s |
| C++ | 0.67s | 0.71s | 1.8s |
| **Zig** | **0.92s** | **0.96s** | **3.0s** |

Zig is the outlier and the port nobody has examined. Its phases say where: the
sweep is 0.41s a side against C's 0.20s, and everything else is within noise.
That is the whole of the 0.9s of extra CPU.

Zig's scan step is eight bytes -- SWAR, no CPU feature at all -- where
`-Dscan=32` puts the same question to a vector register. Building it:

| | sweep, a side | whole run, paired |
|---|---:|---|
| `-Dscan=8` (shipped) | 0.41s | -- |
| `-Dscan=32` | **0.30s** | **0.87x** -- *slower* |
| `-Dscan=64` | 0.31s | not pursued |

The sweep is 27% faster. What the *run* does is the part this host could not
settle, and the rest of this entry is that story rather than a result.

The direct alternating measurement says what the paired one cannot: `scan=32` is
**bimodal** and `scan=8` is not. Ten pairs, milliseconds:

    scan=8   1275 1308 1316 1326 1344 1375 1382 1382 1409 1459
    scan=32  1271 1288 1292 1343 | 1544 1596 1631 1654 1661 1683

Four runs at parity, six about 20% slower, nothing in between, and the same
split in the per-run paired ratios. A phase measurement cannot see this at all:
every phase of `scan=32` is faster in every run, including the ones where the
whole run is 300 ms slower. `bench_ab.sh` sees the cost but not the shape: over
25 rounds it reports 0.89x [0.84-0.94], with `scan=32`'s best-to-median spread
twice `scan=8`'s.

### The floor, and why none of this is a result

`bench_ab.sh --self-test` runs one build against itself. On this host, fifteen
rounds:

| | wall | mid half |
|---|---|---|
| the same binary, twice | 0.99x | **0.92-1.07** |

**The noise floor here is about eight percent.** The scan-width measurement is
0.89x [0.84-0.94] over twenty-five rounds -- outside 1.00, and overlapping the
floor's own band. It is at the edge of what this machine can resolve, not
clearly past it, and "13% slower" is more than the data carries.

Two other things that looked like results and were not:

* **Frequency licensing** as the mechanism. It is the obvious suspect and the
  evidence is against it: the Rust port scans **thirty-two bytes** on this same
  host -- `VECTOR_WIDTH = 32` in `turbo/field.rs`, built
  `-C target-cpu=x86-64-v3` -- and its runs are tight, 0.62s best against 0.63s
  median. Whatever unsettles the Zig build at that width leaves the Rust one
  alone.
* **Two measurements where `scan=32` came out faster.** Both ran eight of one
  build and then eight of the other, which is the one-build-at-a-time method
  this file opens by rejecting. They are not evidence of anything. The two
  interleaved measurements agree with each other; these do not belong beside
  them.

So: the sweep is faster at thirty-two bytes, the run is not measurably faster,
and this host cannot tell whether it is slower. **The question needs a quieter
machine, and the ladder already runs on one** -- the 10m CI rows have `Zig v32`
ahead of `Zig`, and that is the measurement to trust until a paired one on a
quiet host says otherwise.

**And it is the host's answer, not the port's.** The 10m CI rows in the entry of
2026-09-19 have `Zig v32` at 1.72-1.82s against `Zig` at 1.87-1.97s, which is
the opposite. So the width that wins depends on the processor, which is why it
is a build option and why it cannot simply become the default.

Not built, and not refused either -- **unresolved**. Recorded because "Zig scans
eight bytes where C and Rust scan thirty-two" is the first thing anyone looking
at that row will try, and because the trap is not the idea but the measurement:
a 27% phase win that the whole-run number will not confirm on a machine whose
floor is 8%.

### Two things checked and found not to be true

Both are the obvious explanations for the Zig row, and both are wrong:

* **"Zig is not compiled for the host."** C and C++ probe and add
  `-march=native`; Rust gets `-C target-cpu=x86-64-v3`; `zig build` is given
  nothing. But Zig's default target *is* the host: rebuilt from a cleared cache,
  `zig build --release=fast` and the same with `-Dcpu=native` are byte-identical,
  and the binary carries `vpcmpeqb`, `vpbroadcastb` and `vmovdqa32`. The note in
  `scripts/build_ports.sh` is right.
* **"The sweep is oversubscribed."** #109 found the C++ sweep splitting each file
  `threads` ways while both files were in flight, which is twice the cores. Zig
  already halves it -- `const per_file = @max(1, total / 2)` -- and has for as
  long as the file has existed.

The sweep gap is real and is neither of these.

---

## 2026-09-24 (parallel insert) — deterministic, and it does not pay

Every port builds its index the same way: find and hash the rows on every core,
then insert them on one. The C port says why in as many words -- *"first
occurrence wins, and which occurrence is first depends on the order rows arrive,
so threading it would make the answer depend on the scheduler"* -- and every
other port repeats it. The phase is a third of the run, so "insert on one core"
is the largest deliberate serialisation in this project, and it has never been
priced.

**The determinism objection is answerable.** Partition the rows by `hash & (P-1)`
and every occurrence of a key lands in the same partition, so first-occurrence
still wins and file order still decides it: P sub-tables, built in parallel,
give the same answer as one table built serially. A lookup then picks its
sub-table from the same bits.

Priced with a probe that builds the sub-tables beside the real index and drops
them -- nothing downstream sees them -- on the 4M CSV pair at four threads,
which is two per file:

| | serial | partitioned |
|---|---:|---:|
| partition pass | -- | 0.05s |
| insert | 0.24s | 0.15s |
| **total** | **0.24s** | **0.20s** |

The insert itself does parallelise -- 0.24s to 0.15s on two ways -- and the pass
that makes it possible costs most of what that saves. At four ways on a quiet
machine the ceiling is about 0.17s against 0.24s, which is 0.07s of a 0.62s run:
**11%, before paying for anything.**

And the thing it would have to pay for is not in the table. Every lookup on the
join's hot path would gain a sub-table selection -- one more dependent load
before the probe that is already the port's worst cache miss. That cost is on
the phase this project has spent the most effort on, and 11% is not enough
headroom to go looking for it.

Not built. Recorded because the comment in four ports says the insert *cannot*
be threaded, and that is not true -- it can, deterministically, and it is not
worth it. Those are different reasons and the second one is the real one.

---

## 2026-09-24 (parquet join) — the Rust port walked B for a sample nobody asked for

Parquet is the widest ratio in the published tables and nobody had looked at it:
at ten million rows C does the CSV-equivalent work in 1.11s against Rust's 2.12s.
Phases on a 2M pair, warm page cache, three runs each, `--summary` so the report
is not in the number:

| phase | C | Rust |
|---|---:|---:|
| read key columns | 0.044s | 0.024s |
| build key indexes | 0.09s | 0.09s |
| **join / match sweep** | **0.047s** | **0.108s** |
| compared columns | 0.139s | 0.158s |

Everything is close except one phase, and that phase is 2.3x. The first
measurement said otherwise -- compared columns 0.269s against 0.155s -- and was
a cold page cache on the first touch of a 160 MB file. Warm it and that gap is
1.14x.

### The B pass is for the sample, not the count

A key matches from either side or from neither, so the number of B's keys with
an A counterpart *is* the pair count the A pass already produced, and `added` is
B's distinct keys minus it. The pass over B is a second full random-probed walk
of A's table, and all it adds is the report's added-rows **sample**.

`csvdiff.cpp` has run it only when something will print the sample since it was
measured there, and `pqdiff.cpp` since #106 -- the comment there says so in as
many words. The Rust port's `pqdiff.rs` never asked.

Skipping it under `--summary`, `scripts/bench_ab.sh`, 15 rounds interleaved,
paired per round, on this host (4 vCPU Xeon @ 2.10GHz):

| pair | wall [mid half] | CPU [mid half] |
|---|---|---|
| 2M x 2M | **1.15x** 1.09-1.20 | **1.18x** 1.11-1.20 |
| 1M x 2M | **1.29x** 1.19-1.33 | **1.21x** 1.16-1.28 |

The match sweep itself goes **0.108s to 0.050s**, against C's 0.047s. The second
pair is the bigger win because the skipped pass is over twice as many keys.

`bench_ab.sh --self-test` on the same pair and the same fifteen rounds puts this
host's floor at 0.99x [0.92-1.04]. Both rows above sit clear of it -- the 2M
pair's middle half starts at 1.09, above the floor's own upper bound -- which is
worth stating because on the same afternoon this machine could not resolve an
8% question at all (see the zig scan-width entry).

Counts are identical, which is the thing that had to hold, and the case that
tests it is two files of different sizes with duplicate keys on both sides:
1,001,000 added rows derived rather than counted, and the same number either
way. A test asserts it in both directions, and fails when the derivation is
removed.

### While measuring: the C++ summary line names the wrong engine

`main.cpp` prints `| turbo <seconds>s` unconditionally, so a Parquet run reports
`turbo`. C, Rust and Zig all print `parquet`. Nothing is wrong with the run --
it did take the columnar path -- but the line is the first thing anyone reads
when checking which engine a number came from.

---

## 2026-09-22 (summary only) — the same fault as #106, on the other side

#106 found the cross-port tables timing four different tasks, because only the
C++ port emitted row samples and every harness passed `--json` to all four. The
fix was to stop passing it. The same fault was sitting on the other side of the
table the whole time, and it is bigger.

**The Rust port writes a report whether or not one is asked for.** Its `--out`
defaults to `<a>__vs__<b>.html`; C, C++ and Zig write nothing without an output
flag. Run the ladder's exact invocation in an empty directory:

    C     compare a.csv b.csv -k account_id,txn_id  ->  (nothing)
    C++   compare a.csv b.csv -k account_id,txn_id  ->  (nothing)
    Rust  compare a.csv b.csv -k account_id,txn_id  ->  a__vs__b.html
    Zig   compare a.csv b.csv -k account_id,txn_id  ->  (nothing)

`ports()` knew about it and half-fixed it: `report = ["-o", "/dev/null"]`. That
moves the *write*. The render still ran, the gzip still ran, and so did
everything the engine does to feed them — up to `--max-rows` rows per section
decoded into `String`s, sorted and cell-diffed, plus a walk of every duplicated
key to build that section.

### What it costs

`--summary` turns all of it off: the CLI writes nothing and `opt.row_lists`
tells the engine not to build what it would have written. Measured on this host
(4 vCPU Xeon @ 2.80GHz, 15 GB), `scripts/bench_ab.sh`, 15 rounds interleaved,
paired per round, against the unmodified binary:

| pair | wall [mid half] | CPU [mid half] |
|---|---|---|
| 4M CSV, 20 columns | **1.31x** 1.28-1.34 | **1.18x** 1.16-1.19 |
| 2M Parquet, 20 columns | **1.27x** 1.24-1.29 | **1.11x** 1.09-1.12 |

Counts are identical with and without, which is the thing that had to hold: a
section records how many rows it dropped, so the totals never depended on any of
them being kept.

### The project already had the evidence

`--matrix` carried a `Rust engine` row — the same binary with `--max-rows 1` —
and the entry at 2026-09-19 says of it, in as many words:

> **Rust engine is first on csv at both sizes** -- 1.67s at 10m and 3.22s at 20m

That row existed *because* someone noticed the report was in the number. It was
in the matrix, behind a flag, while the published table kept charging it. With
`--summary` on the main row the two measure the same thing, so the extra row is
gone — for the reason `ports()` already gives about `csvdiff-swar`, which was
"the same binary under a second name".

### A measurement that lied, and why

The first paired run of this used two one-line `bash` wrappers around one binary
— one appending `--summary`, one not — because that is the quick way to A/B a
flag. It reported **1.08x [1.05-1.13]** where the binary-against-binary probe
had said 1.28x, and the `--summary` arm was bimodal: best 1.092s against a
median of 1.289s. Timing the same two invocations directly, alternating, gave a
flat 1400ms against 1130ms every round. The wrappers were the artifact. Building
a second binary and comparing binaries — which is what `bench_ab.sh` is for —
put it back at 1.31x. **Do not put a shell script between this harness and the
thing being measured.**

### What changed

* `--summary` on the Rust CLI: prints the counts line and writes nothing. It
  refuses `--out`, `--json` and `--export-dir` rather than picking a winner,
  because guessing which was meant is how a report silently stops appearing.
* `Options::row_lists`, defaulting to `true`, so a library caller asking for a
  `Diff` still gets its rows. Both engines honour it, `turbo` and `pqdiff`.
* Every port-comparison script trades `-o /dev/null` for `--summary`;
  `gate_flags` puts the report back for the counts gate, which needs the JSON
  document and is not timed. `report_cost.py` keeps `-o /dev/null`, since
  pricing the report is what it is for.

Every Rust row in RESULTS.md was measured the old way and is an upper bound
until the ladder runs again.

---

## 2026-09-21 (duplicate keys, C++) — the same shape, and the helper was already there

`dup_section` keeps one row per duplicated key and, of that row, the key columns.
It was decoding the whole row to get them:

    auto values = row_values(s, idx, firsts[i], width, opt);
    values.resize(key_size);

`value_of` turns every field into a `Val`, so a twenty-column file built eighteen
strings per duplicated key and threw them away. `key_values` has sat four hundred
lines above that since #101, which added it for the changed rows, and the Parquet
path's own `dup_section` was already calling it. Only the CSV path was not.

Measured on this host (4 vCPU Xeon @ 2.80GHz, 15 GB), `scripts/bench_ab.sh`,
**15 rounds** interleaved, paired per round. The pair is 2M rows × 20 columns
with every key repeated ten times: **200,000 duplicated keys**, which is what
this section is proportional to. `--json`, because `dup_section` runs under
`row_lists` and that flag is the only thing that sets it.

| | before | after | paired ratio [mid half] |
|---|---:|---:|---|
| wall (median) | 1.056s | 0.737s | **1.48x** 1.43-1.64 |
| CPU (median) | 1.63s | 1.33s | **1.29x** 1.22-1.38 |

Seven rounds first gave 1.64x wall and 1.38x CPU with a mid half twice as wide
(1.13-1.53 on CPU). Fifteen is what the band above is worth quoting from; the
seven-round numbers are recorded because the point of the mid half is that it
tells you when you have not run enough rounds. The JSON is identical across the
change apart from `meta`'s `seconds`.

**Proportional to duplicated keys, not to rows.** A file without repeated keys
never enters the section, and on the standard 4M pair — 400 duplicated keys
against 200,000 here — the equivalent Rust change measured 1.02x, the noise
floor. The number above is what the section costs when a file actually has
duplicates, which is the case it exists for.

The Rust port had the identical shape at `duplicate_section`; that is a separate
change. C and Zig report duplicate *counts* and no rows, so there is nothing
there to fix.

---

## 2026-09-21 (duplicate keys) — the section decoded every column to keep two

The Rust port's duplicate-key section decodes one row per duplicated key and
keeps the key columns. It was decoding the *whole* row to do it:

    let values = row_values(side, idx, *row, opt);   // side.width columns
    (values[..key_size].to_vec(), *n as i64)          // key_size of them kept

`row_values` turns every field into a `String`, so a twenty-column file built
eighteen strings per duplicated key and dropped them, then cloned the two it
wanted into a second vector. The index already carries a parser that stops at
the key — `keys_of`, which the insert path uses on every row — so the fix is to
call it instead of decoding past the key at all.

Measured on this host (4 vCPU Xeon @ 2.80GHz, 15 GB), `scripts/bench_ab.sh`,
7 rounds interleaved, paired per round. The pair is 2M rows × 20 columns with
every key repeated ten times: **200,000 duplicated keys**, which is what this
section is proportional to.

| | before | after | paired ratio [mid half] |
|---|---:|---:|---|
| wall (median) | 1.135s | 0.658s | **1.71x** 1.68-1.73 |
| CPU (median) | 1.74s | 1.27s | **1.36x** 1.34-1.38 |

The report is byte-identical across the change: the HTML payload decompresses to
the same 2,480,022 bytes apart from `meta`, which carries the timestamp and the
elapsed time.

**It is proportional to duplicated keys, not to rows.** On the standard 4M pair,
which has 400 duplicated keys against 200,000 here, the same change measures
1.02x CPU [1.02-1.03] — real but at the paired noise floor, and not worth
quoting on its own. A file with no repeated key does not enter the section at
all. The number above is what the section costs when a file actually has
duplicates, which is the case it exists for.

C++ has the identical shape at `dup_section` — `row_values(...)` then
`values.resize(key_size)` — and already has a `key_values` helper beside it that
#101 added for the changed rows. C and Zig report duplicate *counts* and no rows,
so there is nothing there to fix.

---

## 2026-09-21 (insert peak) — the memory gap was a transient, and it was an ordering bug

The 10M ladder that confirmed the three C++ fixes also showed C++ carrying 1,242
MB of Budget -- peak anonymous memory -- against C's 766 MB. That column is what
`--memory-cap` bounds through `RLIMIT_DATA`, and it is what decides whether a 40m
rung survives, so 476 MB of it is worth finding.

Reproduced on a 4M pair: **C++ 384 MB against C's 276 MB.**

### It is a spike, not a footprint

Sampling `VmData` every 10 ms through the run:

| | C | C++ |
|---|---|---|
| shape | climbs to 276 MB by t=0.28s, **flat for the rest** | **spikes to 384 MB** at t=0.28-0.56s, then **273 MB** |
| steady state | 276 MB | 273 MB |

The two ports end at the same footprint. Only the spike differed, and it sits
exactly on the index insert.

### The cause is which allocations are alive at once

Both ports hold two copies of every row's start and hash for a while: the chunks
the sweep filled and the flat arrays being filled from them. The note above this
port's insert loop has always said so, and freed each chunk as it was taken so
the mark would be *the flat arrays plus one chunk rather than plus all of them*.

What it did not account for is what else is resident underneath that
duplication. C++ allocated the table, `first_row_` and `occurrences_` **before**
the transfer, so all three were alive through the whole of it -- 134 MB of table
and 80 MB of the other two, a side, at ten million rows.

C does it the other way round, and says so:

    Copied chunk by chunk, and each chunk released as it is taken, so the
    high-water mark is the flat arrays plus one chunk rather than plus all of
    them.

-- and then allocates the table *after* that loop has finished.

### Ported

The chunks are drained into `row_start_` and `row_hash_` and released first;
only then is the table built, and `insert` now reads the row it is given from
arrays already filled rather than pushing into them.

| | RSS above the input | VmData |
|---|---:|---:|
| before | 366 MB | 384 MB |
| **after** | **255 MB** | **275 MB** |
| C | 249 MB | 276 MB |

**-109 MB at 4M, and C++ now matches C to a megabyte.**

This also retires the entry two above. The +147 MB of *above the input* that the
`per_file` change showed on CI is more than reversed, and the reading there --
that the extra was file-backed and reclaimable while anonymous fell -- was
right about the mechanism and looking at a second, larger effect sitting beside
it.

### Time, for completeness

Thirteen paired rounds, 4M CSV pair, `--threads 4`:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| counts only | 0.97s | 0.95s | 0.976 | 0.930-0.994 | 11/13 |
| with `--json` | 1.54s | 1.51s | 0.967 | 0.943-1.047 | 8/13 |

The first clears 1.00 and the second does not, so the honest summary is that the
restructure costs no time and may save a little. It is a memory change; the
clock is the control here, not the claim.

Output byte-identical across ten case and option combinations, including the
duplicate-key fixture -- the order rows enter the index is unchanged, chunks
being concatenated in the same order they were before. cpp 36/36, cross-port
89/89.

---

## 2026-09-21 (sweep realloc) — a real cost with no room to pay it back

The sweep is the widest remaining ratio against C, 1.28x, and one difference
between the ports is how each accumulates what it finds. C's `chunk_push` grows
with `realloc`, which the allocator can often extend in place. C++'s
`std::vector::push_back` cannot: every growth allocates, copies and frees. At 4M
rows a side that is roughly 128 MB of `memcpy` the C port does not do, and it
would get worse with two chunks reallocating at once -- which matches the
measurement that made the two-way split the worst width available.

Bounded before building, per the rule in AGENTS.md, with a deliberately unsound
build: an **oracle** reserve taken from a row count a real implementation would
have to estimate.

| | no reserve | oracle reserve | |
|---|---:|---:|---|
| sweep | 0.318s | 0.263s | **-17.3%** |
| wall | 1.000s | 0.984s | **-1.5%** |

The reasoning was right and the prize is not there. The reallocation genuinely
costs 17% of the phase. The phase is a quarter of one side of a critical path
that the join still dominates, so 17% of it is **1.5% of the run** -- below the
3% floor this file gives the paired ratio, and that is the *ceiling*, measured
with a row count no real implementation gets for free. Anything built here would
capture less than a number already too small to report.

Not built. Recorded because "reserve the sweep's vectors" is a reasonable idea
that will occur to somebody else, and the useful part of this entry is that it
has been priced: the cost is real, the room to recover it is not.

Ten minutes of probing instead of a day of implementing, which is what that rule
is for.

---

## 2026-09-21 (slot tag) — C++ probed three cache lines deep to answer what one word can answer

With the join's threading fixed, the phase comparison against C on the same 4M
pair at four threads read:

| phase | C | C++ | ratio |
|---|---:|---:|---:|
| sweep, per side | 0.209s | 0.302s | 1.35x |
| **index insert, per side** | **0.128s** | **0.301s** | **2.25x** |
| **join and compare** | **0.357s** | **0.689s** | **1.93x** |
| wall | 0.901s | 1.325s | 1.47x |

Both of the wide ratios probe the same open-addressed table, which was the hint.

### What C does and this port did not

C's slot is a `uint32_t` carrying a key index in its low bits and the top bits of
that key's hash above it. A probe that lands on the wrong key is rejected by the
word it has already loaded. The comment beside it has been there all along:

    Without it, rejecting a collision costs two more dependent loads --
    `first_row[at]`, then `row_hash[candidate]` -- each a miss on an array far
    too big to cache, and each waiting on the one before it.

That is exactly what this port was doing, in `insert` and in `lookup` both:

    at        = table_[slot]          // miss
    candidate = first_row_[at]        // miss, waiting on the first
    row_hash_[candidate]              // miss, waiting on the second

Three dependent cache misses to reject a key, where one word decides it.

### Ported

The slot width is chosen from the row count rather than fixed, so the slot stays
four bytes and the table stays the size it was -- at ten million rows the index
needs 24 bits and the tag gets the other 8. A tag that runs out of bits degrades
to **no tag, not to a wrong answer**, because the key comparison behind it is
unchanged. `rehash()` recomputes the width and repacks, since a table that
doubled is one expecting more keys.

| phase | before | after | |
|---|---:|---:|---|
| index insert | 0.296s | **0.218s** | -26% |
| join and compare | 0.670s | **0.478s** | -29% |
| wall | 1.281s | **1.050s** | -18% |

Thirteen paired rounds, 4M CSV pair, `--threads 4`, summary identical between
arms in both modes first:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| counts only | 1.33s | 1.01s | **0.762** | 0.748-0.789 | **13/13** |
| with `--json` | 1.97s | 1.52s | **0.779** | 0.744-0.818 | **13/13** |

**Both modes, every round.** CPU falls 3.51s to 2.85s and 5.32s to 4.11s, so this
is less work and not only a better spread of it -- which is what removing two
dependent misses per collision should look like, and is the check that it is not
a scheduling accident.

Against C on the same pair, C++ goes from **1.47x** to **1.17x** of its wall.

### Verified beyond the clock

The counts gate is load-bearing here rather than a formality: a tag that rejected
a key it should have kept would change `matched` and `dup keys`, and all four
ports agree on both. cpp 36/36, cross-port 89/89. The `--json` payload is
byte-identical to the previous build across eight case and option combinations,
including the duplicate-key fixture.

---

## 2026-09-21 (join_ways) — the join reserved a thread for a pass that was not running

With the report out of the timed path, C++ at `--threads 2` was **1.01x** of
`--threads 1`. The second thread bought nothing at all. C's is worth 1.46x.

| `--threads` | 1 | 2 | 4 |
|---|---:|---:|---:|
| C wall | 1.67s | 1.14s | 0.89s |
| C speedup | — | **1.46x** | 1.89x |
| C++ wall | 2.83s | **2.79s** | 1.46s |
| C++ speedup | — | **1.01x** | 1.94x |

`join_ways = budget - 1`, unconditionally. At a budget of two that is **one**, so
the join -- 56% of the wall -- ran on a single core however many were free.

The subtraction exists because `b_side` runs concurrently with the join. But
`b_side` is gated on `row_lists`, which is false whenever `--json` was not given.
The default invocation reserved a thread for a pass that was never going to
start.

    join_ways = row_lists ? max(1, budget - 1) : budget

| `--threads` | 1 | 2 | 4 |
|---|---:|---:|---:|
| before | 2.83s | 2.79s | 1.46s |
| after | 2.81s | **1.84s** | **1.31s** |
| speedup 1→t, after | — | **1.53x** | 2.15x |

Thirteen paired rounds, 4M CSV pair, `--threads 4`, summary identical between
arms in both modes first:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| counts only | 1.48s | 1.28s | **0.861** | 0.843-0.882 | **13/13** |
| with `--json` | 1.91s | 1.90s | 1.014 | 0.946-1.041 | 5/13 |

**14% on the path the benchmark times.** CPU is 3.37s in both arms -- identical
work, spread over more of the machine, which is what a scheduling fix looks like
and what a work reduction does not. The `--json` row straddles 1.00 because that
path's arithmetic is unchanged, which is the control this change happens to come
with.

Core utilisation at four threads goes 2.28x to 2.64x. C's is 3.06x, so this
closes about half of what was left.

### What this does not settle

Whether `budget - 1` is right on the `--json` path either. `b_ways = join_ways`,
so `b_side` does not take one thread, it takes as many as the join does: at four
threads the two sides together ask for six. Subtracting one from the budget trims
that by a sixth and no measurement here says it is the correct trim. The change
leaves it exactly as it was and confines itself to the case where the second pass
does not exist.

A first draft of the comment on this line said the B-side pass takes one thread
of the budget. It does not, and the line is worth reading twice before it is
changed again.

---

## 2026-09-20 (per_file) — the sweep was split two ways, which is the worst width available

The sweep is the phase that does not scale, and the reason turned out to be the
number it was split into. `per_file = budget / 2` gives **two** on a four-core
machine, which is every CI runner this project uses.

Wall time on a 4M CSV pair at `--threads 4`, nine interleaved rounds, with the
split forced to each width:

| per_file | 1 | **2** | 3 | 4 | 6 | 8 |
|---|---:|---:|---:|---:|---:|---:|
| vs two | 0.923 | **1.000** | 0.965 | 0.936 | 0.936 | 0.903 |

Two is not a point on a curve. It is the worst value available, and every other
width beats it.

### Why

Chunking costs a whole extra pass over the bytes before a single row is read.
`chunk_bounds` has to count quotes to know whether a nominal split lands inside
a quoted field, and a newline inside a quoted field is not a row boundary.

Split four ways, that pass is a quarter of the file per thread and the sweep it
enables is four times narrower. Split **two** ways it is half the file and
**single-threaded** -- the counter loop runs `for i in 1..nominal.size()` and
`nominal` has one entry, so it spawns nothing -- to make the sweep only twice as
narrow, while running four concurrent read streams where there were two.

Measured separately, with a `chunk bounds` phase mark added for the purpose:

| per_file | chunk bounds | sweep | total |
|---|---:|---:|---:|
| 1 | 0.000s | 0.316s | 0.316s |
| 2 | 0.054s | 0.358s | **0.412s** |
| 4 | 0.064s | 0.248s | 0.312s |

The bounds pass is only 0.054s of the 0.096s regression; the rest is the sweep
itself getting slower when it is split in two. And note the totals: **one chunk
and four chunks cost the same.** On this machine the split buys nothing at any
width and pays for itself at none.

### What changed

`per_file = budget >= 8 ? budget / 2 : 1`. Machines with eight threads or more
keep exactly what they had; the 4-to-7 range, where two and three were both
measured as losses, stops splitting.

Thirteen paired rounds, 4M CSV pair, `--threads 4`, summary identical between
arms in both modes first:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| counts only | 1.72s | 1.61s | **0.956** | 0.924-0.974 | **12/13** |
| with `--json` | 2.18s | 2.03s | 0.939 | 0.909-1.003 | 10/13 |

The first row is the path the benchmark now times and it clears. The second
straddles 1.00 at the top of its band and is therefore no result, median
notwithstanding. CPU seconds fall 4.47 to 3.61 and 6.36 to 5.54.

### What is not established

That the sweep cannot scale anywhere. This is one four-core VM, and on it the
phase costs the same at one chunk as at four. A machine with more memory
channels may well split it profitably, which is why the change leaves
eight-thread-and-wider budgets alone rather than concluding from four cores that
chunking is never worth it.

The `chunk bounds` mark stays. It is what made this visible, and it reads 0.000s
for every budget that no longer splits.

### On CI it also moved a memory column, and not the way it reads

The 10M ladder that measured this (run 35525106448, EPYC 7763, same processor
as the run before it) shows C++ on CSV going 2.82s to 2.77s and its CPU 7.3s to
6.4s, **-12%**. It also shows *above the input* going 883 MB to 1,030 MB, and a
reader would be right to stop at that.

It is not a leak, and it is not the kind of memory that ends a rung. Measured on
a 4M pair with both numbers taken from `/proc`:

| per_file | RSS above the input | VmData (anonymous) |
|---|---:|---:|
| 1 | 372 MB | **384 MB** |
| 2 | 311 MB | **400 MB** |

The extra resident memory is **file-backed**: one thread streaming a mapped file
end to end leaves more of it resident than two threads each covering half. Those
pages are reclaimable. The anonymous memory -- the part that cannot be handed
back, and the part `--memory-cap` bounds through `RLIMIT_DATA` -- went *down* by
16 MB, because one chunk allocates one pair of growing vectors where two chunks
allocate two.

The ladder's own table says the same thing from the other side: the Budget
column, which is peak `VmData`, fell 1,281 MB to 1,243 MB in the same run where
*above the input* rose. Two columns, one cause, and the one that decides whether
a 40m rung survives is the one that improved.

This is also the reason this file keeps both columns rather than only peak RSS.

---

## 2026-09-20 (phases, corrected path) — the sweep is worse than this file said

The phase split was last taken with `--json` passed, which is no longer what the
benchmark times. Re-measured without it, 4M CSV pair, five rounds, medians, the
two index sides taken as a critical path because they run concurrently:

| phase | 1 thread | 4 threads | scales | share of the 4t wall |
|---|---:|---:|---:|---:|
| sweep | 0.306s | 0.419s | **0.73x** | **24.6%** |
| index insert | 0.372s | 0.325s | 1.14x | 19.1% |
| join and compare | 2.245s | 0.954s | 2.35x | 56.1% |
| assemble | 0.001s | 0.001s | — | 0.1% |

Wall 2.96s at one thread, 1.70s at four: **1.74x**.

Two checks that this is a decomposition and not a fit. The critical path sums to
**1.699s against a 1.70s wall**, 100%. And the non-scaling phases are 24.6% +
19.1% = **43.7%** against the **43.3%** Amdahl implies from that speedup — two
routes to one number, four-tenths of a point apart.

### What changed from the last reading

`assemble` was 11% when it was first measured and is **0.1%** now. Gating it
behind `row_lists` and then building only the cells the report prints took it
out of the timed path; with `--json` still passed on the same pair it is 7.8%,
so the phase is not gone, it is simply no longer in what the benchmark measures.

**The sweep got worse, not better: 0.79x → 0.73x.** Nothing changed it; the old
number was measured with the report still running, and a wall inflated by 19%
made every non-scaling share look smaller than it is. It is now the largest
non-scaling phase by a wider margin, and it is the only phase in the run that
is *slower* on four threads than on one.

That is the whole of the remaining C++ gap, and it now has two independent
statements of the same thing. From the CI table: C++ spends 7.3 CPU-seconds on
10M CSV to C's 6.4 — 1.14x, matching the 1.07x instruction ratio — and takes
1.56x the wall, at 2.61x cores against C's 3.53x. From the phases: 43.7% of a
four-thread run does not scale, and the largest part of it runs backwards.

On ndjson the same table makes it starker still. C++ spends **14.9 CPU-seconds
against C's 16.7** — the least of any port — and loses, 5.88s to 5.24s, on 2.54x
cores against 3.19x. At C's utilisation those same seconds finish in **4.67s**
and lead the format. Nothing has to get cheaper for that.

---

## 2026-09-20 (10M, before and after) — what the report was costing, on CI

The change in the entry below, measured where it matters. Two dispatched 10M
ladders, csv and parquet, scanner matrix on, three repeats — and **both landed
on the same processor**, an EPYC 7763, so for once this is a before and an after
and not two tables.

| Build | Format | before (`--json` to every port) | after (the common task) | change |
|---|---|---:|---:|---:|
| **C++** | **csv** | **3.38s** | **2.87s** | **-15.1%** |
| **C++** | **parquet** | **1.92s** | **1.76s** | **-8.3%** |
| C | csv | 1.81s | 1.82s | +0.6% |
| C | parquet | 1.26s | 1.31s | +4.0% |
| Rust | csv | 2.06s | 2.07s | +0.5% |
| Rust | parquet | 2.42s | 2.47s | +2.1% |
| Zig | csv | 1.87s | 1.97s | +5.3% |
| Zig | parquet | 2.41s | 2.42s | +0.4% |
| C++ avx2 | csv | 3.34s | 2.87s | -14.1% |
| Rust avx2 | csv | 2.07s | 2.08s | +0.5% |
| Rust engine | csv | 1.61s | 1.67s | +3.7% |
| Zig v32 | csv | 1.72s | 1.82s | +5.8% |

**Only C++ moved.** Every other row is inside ±6%, which is what a best-of-three
unpaired number does on one machine across two sittings — and is the reason this
file says not to read a single such row as a result. The C++ rows are 15% and 8%,
well outside it, and they are the only rows where the work changed.

| | before | after |
|---|---:|---:|
| C++/C on csv | 1.87x | **1.58x** |
| C++/C on parquet | 1.52x | **1.34x** |

A third of the CSV gap above parity was the report, which is what the 4M
measurement in the entry below predicted before the run was dispatched.

C++'s CPU seconds fall 10.9 to 7.6, **-30%**, and its core utilisation with them,
3.22x to 2.64x. That is the right direction and worth saying why: the work
removed was the *parallel* B-side pass, so taking it away leaves a run that is
less busy as well as shorter. Wall fell 15% while CPU fell 30%.

### The AVX2 rung, answered

`C++ avx2` csv is **2.87s** against plain `C++` at **2.87s**. Identical, on the
clean task, on one machine, in one sitting.

That settles a question this file has had two wrong answers to. The first read
the old matrix rows (-1.7%, -4.4%, +2.8%, +2.0%) as a wash, which they could not
establish — best-of-two unpaired cannot resolve anything under about 10%, as the
methodology section says. The second, on finding the rung is worth 221M
instructions (8.4%) and that C gets it automatically from `__AVX2__` where C++
hides it behind a hand-set `CSVDIFF_SCAN_AVX2`, treated it as a live prospect.

It is not. The instructions are real and the time is not, for the same reason
`#105` cut 81M instructions from the JSON writer for no wall change: on the task
both ports perform C++ already executes only 1.07x C's instructions while taking
1.81x the wall. **The remaining gap is not instruction count**, and the rung
stays off.

---

## 2026-09-20 (four tasks) — the cross-port tables were never measuring one job

Profiling C against C++ to find the CSV gap, the call counts came out equal in
one mode and not the other. Without `--json`, on the same 400k pair:

| | C | C++ |
|---|---:|---:|
| `next_of2` calls | 12,078,700 | **12,078,700** |
| row parses | 1,623,746 | **1,623,746** |
| equality calls | 1,205,561 | **1,206,742** |

Identical. With `--json`, C's counts do not move at all and C++'s become
15,932,660 / 2,496,104 / 2,413,364.

### Why

Only the C++ port emits row samples. `scripts/report_cost.py`, on a 4M pair:

| Port | top-level keys | rows named | bytes |
| --- | --- | ---: | ---: |
| C | columns, counts | 0 | 1,196 |
| C++ | added, changed, columns, counts, dup_a, dup_b, meta, removed | **58,600** | **4,917,335** |
| Rust | columns, counts, meta | 0 | 3,057 |
| Zig | columns, counts | 0 | 1,196 |

C has no flag to turn them on, because it has no such feature. Producing them
switches on the B-side pass -- gated on `row_lists` and priced in this file at
1.29x of wall -- and then materialises, sorts and writes every sampled row.

| | C | C++ |
|---|---:|---:|
| instructions, counts only | 1,705,815,289 | 1,821,191,662 (**1.07x**) |
| instructions, with `--json` | 1,705,854,539 | 2,629,295,857 (**1.54x**) |
| what `--json` costs | +39,250 (0.0%) | +808,104,195 (**+44.4%**) |

Wall, 4M pair, four threads, page cache warmed, the two modes interleaved and
the arms rotated:

| mode | C | C++ | ratio |
|---|---:|---:|---:|
| counts only | 1.02s | 1.84s | **1.81x** |
| `--json` | 1.06s | 2.38s | **2.25x** |

**About a third of the published CSV gap was the report.** Every harness passed
`--json` to every port, so every cross-port table in this file compared three
ports writing about a kilobyte against one writing five megabytes.

### What changed

The timed runs no longer pass `--json`. The counts gate still needs the
document, so it runs once per port, untimed, before the rounds. Both harnesses,
same reasoning, cross-referenced comments.

This is the same fault as the `-march=native` one at the top of this file, and
`ports()` had already fixed the matching case for Rust -- `-o /dev/null`, so its
HTML is not charged against ports that render none, with a comment calling it
"the only row here that is not comparing like with like". C++'s samples were the
other half, and nobody had looked.

`json_sample_cost.py` is replaced by `report_cost.py`, which prints the document
shape *before* the cost. The old one printed only the cost, and the entry that
used it read "C 3%, C++ 36%, Rust 0%, Zig 1%" as those ports skipping the
samples or building them cheaply. They have no samples. Timing alone could not
have said so, and the new one puts the reason above the number.

### What this says about the remaining gap, which is the useful part

On the task all four ports perform, C++ executes **1.07x** C's instructions and
takes **1.81x** the wall. The gap is not instruction count. It is what those
instructions do -- memory traffic and threading -- which agrees with the sweep
measurement three entries down: 23% of a four-thread run, scaling **0.79x**,
getting worse as threads go up.

That retires a line of work. `#105` cut 81M instructions from the JSON writer
and moved wall time not at all, and its entry recorded the null without knowing
why. This is why. Enabling the AVX2 scanner rung in C++ -- which C gets
automatically from `__AVX2__` where C++ gates it behind a hand-set
`CSVDIFF_SCAN_AVX2` -- is worth 221M instructions, 8.4%, and should be expected
to move wall time about as much as the last one did: not at all. It is left
unmade pending a paired measurement rather than shipped on the instruction count.

---

## 2026-09-19 (json runs) — a sentry per character, and no wall time to show for removing it

Callgrind on C and C++ over the same 400k CSV pair, one thread, to find where
the C++ CSV gap actually is. It turned up something unrelated to the gap and
worth fixing on its own: **C++ spent 3.9% of the entire run inside
`std::ostream` bookkeeping**, writing JSON.

| | instructions | share |
|---|---:|---:|
| `std::ostream::put(char)` | 67,721,944 | 2.50% |
| `std::ostream::sentry::sentry` | 37,451,137 | 1.38% |
| `write_string` itself | 18,368,272 | 0.68% |

`write_string` did `o << c` per character. That is not a byte appended to a
buffer: each one constructs an `ostream::sentry`, which checks the stream state
and flushes whatever is tied to it, for the single character that follows. The
C port's JSON writer does not appear in its profile at all.

It now writes each run of ordinary bytes in one `write` and handles only the
escapes individually.

| | before | after |
|---|---:|---:|
| `std::ostream::put` | 67.7M | **19.0M** |
| `ostream::sentry` | 37.5M | **13.8M** |
| whole program | 2,710,450,723 | **2,629,295,945** |

**-81.2M instructions, -3.0% of the program.**

### And it buys no wall time, which is the point of writing it down

Three paired measurements on the 4M CSV pair, arms rotated, four threads:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| `--json`, default `--max-rows` | 2.70s | 2.64s | 1.009 | 0.920-1.071 | 7/15 |
| `--json`, default `--max-rows`, again | 2.61s | 2.60s | 0.989 | 0.968-1.040 | 7/13 |
| `--json --max-rows 240000` | 3.71s | 3.63s | 0.985 | 0.915-1.024 | 8/13 |

Every band straddles 1.00, so by the rule at the top of this file there is **no
result** in any of them. The third row is the one that was supposed to show it:
`--max-rows` caps the report at 50,000 rows however large the input, so at 4M the
JSON payload is fixed at 4.9 MB while everything else scales with the file.
Raising the cap to take all 240,031 changed rows makes the payload 17.3 MB,
3.5x larger -- and it still does not clear the noise.

CPU seconds fall 8.75 to 8.55 on that row, about 2.3% and in the right
direction, but that is a median of each arm's runs rather than a paired
statistic and it is not what this file gates on. It is mentioned, not claimed.

So: 3.9% of a *profile* is 0% of a *run*, because the phase it lives in is a
small and fixed part of a large one. Kept anyway -- it is strictly less work for
byte-identical output, and it removes a real pathology rather than trading one
cost for another -- but recorded as the null result it is.

### The coverage check that nearly passed for the wrong reason

The first escape-torture pair mutated only alternate rows, and the values
carrying a backslash, a carriage return and the C0 controls happened to land on
the unchanged ones. Unchanged rows are not in the report, so those three escape
branches emitted **zero** occurrences and the byte-identity check passed without
ever exercising them. Rebuilt so every row differs; all eight branches then fire
-- quote 7, backslash 6, `\n` 6, `\r` 6, `\t` 8, `\u00XX` 62, raw 0x7f 2, raw
UTF-8 8 -- and the twelve case/option comparisons pass on that.

---

## 2026-09-19 (run 35427804365) — the first ladder CI grouped by itself

The entry below reconstructs a ladder whose grouped table CI crashed on and
then reported as absent. This is the next dispatch, on `main` with that fixed:
10m and 20m rows, csv and parquet, scanner matrix on. **CI produced this table
itself.** It is the first one in this file that did.

Checked rather than assumed, both scripts against this run's own rung artifacts:

| `scripts/bench_group.py` at | exit | result |
|---|---:|---|
| `bc6dd61`, before the fix | 1 | `TypeError` -- would have published nothing again |
| `22350ac`, what this run executed | **0** | the table below |

The input that separates them is the same two `C++ avx2` parquet rows: that
build does not read parquet, and saying so is all it takes.

**2 different CPUs produced these 30 measurements.** Each table below is one CPU. Rows may be compared inside a table and **not** between tables -- that is not a formality here, it is the difference between measuring a change and measuring which machine the job landed on.

| CPU | cores | widest vector | measurements |
| --- | ---: | --- | ---: |
| AMD EPYC 7763 64-Core Processor | 4 | avx2 | 22 |
| AMD EPYC 9V74 80-Core Processor | 4 | avx2 | 8 |

### AMD EPYC 7763 64-Core Processor — 4 cores, avx2

*22 measurements · sizes 10m, 20m · formats csv, parquet · 2 reported nothing*

| Build       | Format  | Size | Compare |    Rows/s |   CPU | Cores | Peak RSS | Above the input |   Budget |
| ----------- | ------- | ---- | ------: | --------: | ----: | ----: | -------: | --------------: | -------: |
| C           | csv     | 20m  |   3.58s | 5,593,912 | 12.7s | 3.54x | 8,448 MB |        1,430 MB | 1,480 MB |
| C           | parquet | 10m  |   1.31s | 7,622,042 |  4.2s | 3.17x | 2,924 MB |        1,390 MB | 1,496 MB |
| C           | parquet | 20m  |   2.92s | 6,847,166 |  8.9s | 3.04x | 5,760 MB |        2,691 MB | 3,065 MB |
| C++         | csv     | 20m  |   6.81s | 2,939,003 | 21.8s | 3.21x | 8,758 MB |        1,740 MB | 2,501 MB |
| C++         | parquet | 10m  |   2.02s | 4,951,311 |  6.4s | 3.15x | 2,853 MB |        1,318 MB | 1,505 MB |
| C++         | parquet | 20m  |   3.88s | 5,155,681 | 12.5s | 3.23x | 5,662 MB |        2,593 MB | 2,928 MB |
| Rust        | csv     | 20m  |   3.83s | 5,228,614 | 12.6s | 3.30x | 8,759 MB |        1,742 MB | 2,458 MB |
| Rust        | parquet | 10m  |   2.42s | 4,131,195 |  8.1s | 3.37x | 2,871 MB |        1,337 MB | 1,464 MB |
| Rust        | parquet | 20m  |   4.64s | 4,307,106 | 15.9s | 3.43x | 5,707 MB |        2,638 MB | 2,889 MB |
| Zig         | csv     | 20m  |   3.58s | 5,585,027 | 11.9s | 3.33x | 8,762 MB |        1,744 MB | 2,101 MB |
| Zig         | parquet | 10m  |   2.42s | 4,134,378 |  8.6s | 3.56x | 2,760 MB |        1,225 MB | 1,738 MB |
| Zig         | parquet | 20m  |   4.84s | 4,136,384 | 17.2s | 3.55x | 5,491 MB |        2,422 MB | 3,424 MB |
| C++ avx2    | csv     | 20m  |   6.51s | 3,073,689 | 21.4s | 3.29x | 8,763 MB |        1,746 MB | 2,502 MB |
| C++ avx2    | parquet | 10m  |       - |         - |     - |     - |        - |               - |        - |
| C++ avx2    | parquet | 20m  |       - |         - |     - |     - |        - |               - |        - |
| Rust avx2   | csv     | 20m  |   3.78s | 5,293,589 | 12.7s | 3.35x | 8,760 MB |        1,742 MB | 2,458 MB |
| Rust avx2   | parquet | 10m  |   2.53s | 3,946,760 |  8.5s | 3.36x | 2,889 MB |        1,355 MB | 1,464 MB |
| Rust avx2   | parquet | 20m  |   4.85s | 4,128,055 | 16.5s | 3.41x | 5,707 MB |        2,638 MB | 2,881 MB |
| Rust engine | csv     | 20m  |   3.22s | 6,202,945 | 10.8s | 3.35x | 8,741 MB |        1,723 MB | 2,458 MB |
| Rust engine | parquet | 10m  |   2.17s | 4,605,960 |  7.4s | 3.42x | 2,800 MB |        1,265 MB | 1,462 MB |
| Rust engine | parquet | 20m  |   4.19s | 4,769,273 | 15.0s | 3.58x | 5,651 MB |        2,582 MB | 2,878 MB |
| Zig v32     | csv     | 20m  |   3.32s | 6,023,713 | 11.2s | 3.37x | 8,761 MB |        1,743 MB | 2,101 MB |
| Zig v32     | parquet | 10m  |   2.47s | 4,048,223 |  8.7s | 3.54x | 2,745 MB |        1,210 MB | 1,724 MB |
| Zig v32     | parquet | 20m  |   4.88s | 4,097,989 | 17.4s | 3.57x | 5,467 MB |        2,397 MB | 3,424 MB |


### AMD EPYC 9V74 80-Core Processor — 4 cores, avx2

*8 measurements · sizes 10m · formats csv*

| Build       | Format | Size | Compare |    Rows/s |   CPU | Cores | Peak RSS | Above the input |   Budget |
| ----------- | ------ | ---- | ------: | --------: | ----: | ----: | -------: | --------------: | -------: |
| C           | csv    | 10m  |   1.81s | 5,515,241 |  6.3s | 3.48x | 4,225 MB |          716 MB |   766 MB |
| C++         | csv    | 10m  |   3.58s | 2,792,076 | 11.6s | 3.24x | 4,386 MB |          877 MB | 1,275 MB |
| Rust        | csv    | 10m  |   2.12s | 4,724,588 |  6.8s | 3.21x | 4,409 MB |          900 MB | 1,233 MB |
| Zig         | csv    | 10m  |   2.02s | 4,954,220 |  6.5s | 3.24x | 4,389 MB |          880 MB |   973 MB |
| C++ avx2    | csv    | 10m  |   3.68s | 2,716,618 | 11.0s | 2.99x | 4,387 MB |          878 MB | 1,275 MB |
| Rust avx2   | csv    | 10m  |   2.12s | 4,726,242 |  6.7s | 3.19x | 4,409 MB |          900 MB | 1,233 MB |
| Rust engine | csv    | 10m  |   1.67s | 6,001,401 |  5.5s | 3.30x | 4,372 MB |          863 MB | 1,233 MB |
| Zig v32     | csv    | 10m  |   1.82s | 5,502,940 |  6.3s | 3.45x | 4,383 MB |          874 MB | 1,048 MB |


> **AMD EPYC 9V74 80-Core Processor is missing 20m.** Those rungs ran on other silicon and are in another table; this ladder is partial and its slope cannot be read as one curve.

> **2 of 32 rows carry no number:** C++ avx2 on parquet. A build that cannot read a format reports one of these; it is a dash above, not a zero and not a slow result.

### What is in it

**Two CPUs again, and one of them got a single rung.** The EPYC 7763 took three
of the four, the 9V74 took 10m csv alone, and the ladder says so rather than
drawing a curve through both. This is the fourth consecutive dispatch to land on
more than one processor; it is the normal case on this fleet, not the unlucky
one.

**Rust engine is first on csv at both sizes** -- 1.67s at 10m and 3.22s at 20m,
against C's 1.81s and 3.58s. Read it within its own table: the 10m and 20m rows
are on different machines and are not two points on one line.

**C++ is last on csv by a wide margin**, 3.58s against C's 1.81s at 10m and
6.81s against 3.58s at 20m -- about 2x, and the same 2x the profile entries have
been chasing. #101 landed between this run and the one below, and it does not
close this: it takes 5% off the `--json` path, and the gap is 100%.

**On parquet the order is different and C leads**: 1.31s at 10m and 2.92s at 20m
against C++'s 2.02s and 3.88s, so C++ trails by 1.5x and 1.3x rather than 2x.
Parquet hands the ports typed columns instead of bytes to scan, which removes
most of what the csv gap is made of.

**Nothing paged.** Every row reports a positive *above the input* -- 716 to
2,691 MB -- so the check added in the entry above stays silent here, which is
the behaviour it was built for. csv and parquet at 20m are 8.4 GB and 5.1 GB
against roughly 16 GB of RAM; it was ndjson at 20m, at 17 GB, that did not fit.

---

## 2026-09-19 (the ladder CI threw away) — six rungs, three CPUs, and a grouped table that never printed

Run 35425371164 dispatched the ladder at 10m and 20m rows across csv, ndjson and
parquet, with the scanner matrix on. All six rungs finished. The collect job
reported **success**. It published no grouped table at all, and said so in one
calm line:

    _no rung JSON to group_

There was rung JSON. Fifty-one measurements of it, on three different
processors. What actually happened is in the log the summary did not show:

    File "scripts/bench_group.py", line 87, in table
        + [f"{sec:.2f}s",
    TypeError: unsupported format string passed to NoneType.__format__

### Two faults, and the second is the worse one

**`bench_group.py` could not render what its own sibling writes.**
`bench_formats_ports.py` emits `{"format", "port", "seconds": None}` and nothing
else for a build that could not run a format -- here, `C++ avx2` and
`C++ avx512`, which do not read parquet -- and its own `table_of` prints that as
a row of dashes. The grouping script used `r.get("seconds", 0.0)`, which does
not help: the key is *present* and its value is `None`, so the default never
applies. Three such rows out of fifty-four, and the first one reached took the
grouped table for the entire ladder with it.

**The workflow masked it.** The step was

    python3 scripts/bench_group.py rungs/ || echo "_no rung JSON to group_"

so a crash and an empty directory produced the same line and the same green
tick. Forty minutes of runner time, six rungs that all worked, and the one
artefact the job exists to produce was absent with nothing anywhere saying why.

A traceback also exits 1, so an exit code could not tell the two apart. "Nothing
to group" now has its own code (3), the step prints the traceback into the
summary, and any other non-zero exit fails the job. Verified against all three
paths: the real six rungs print the table and pass; an empty directory prints
the note and passes; an injected bad row prints its traceback and **fails**.

### What the run actually measured

**3 different CPUs produced these 51 measurements.** Each table below is one CPU. Rows may be compared inside a table and **not** between tables -- that is not a formality here, it is the difference between measuring a change and measuring which machine the job landed on.

| CPU | cores | widest vector | measurements |
| --- | ---: | --- | ---: |
| AMD EPYC 7763 64-Core Processor | 4 | avx2 | 23 |
| INTEL(R) XEON(R) PLATINUM 8573C | 4 | avx512 | 18 |
| AMD EPYC 9V45 96-Core Processor | 4 | avx512 | 10 |

### AMD EPYC 7763 64-Core Processor — 4 cores, avx2

*23 measurements · sizes 10m, 20m · formats csv, ndjson, parquet · 1 reported nothing*

| Build       | Format  | Size | Compare |    Rows/s |   CPU | Cores |  Peak RSS | Above the input |   Budget |
| ----------- | ------- | ---- | ------: | --------: | ----: | ----: | --------: | --------------: | -------: |
| C           | csv     | 20m  |   3.57s | 5,605,825 | 12.6s | 3.54x |  8,448 MB |        1,430 MB | 1,480 MB |
| C           | ndjson  | 20m  |  38.16s |   524,102 | 50.7s | 1.33x | 15,054 MB |       -1,921 MB | 1,582 MB |
| C           | parquet | 10m  |   1.31s | 7,634,570 |  4.2s | 3.19x |  2,903 MB |        1,369 MB | 1,503 MB |
| C++         | csv     | 20m  |   6.66s | 3,005,460 | 21.3s | 3.20x |  8,754 MB |        1,736 MB | 2,501 MB |
| C++         | ndjson  | 20m  |  36.52s |   547,750 | 56.7s | 1.55x | 15,039 MB |       -1,936 MB | 2,620 MB |
| C++         | parquet | 10m  |   2.07s | 4,833,409 |  6.4s | 3.09x |  2,822 MB |        1,287 MB | 1,510 MB |
| Rust        | csv     | 20m  |   3.83s | 5,221,203 | 12.6s | 3.29x |  8,752 MB |        1,734 MB | 2,458 MB |
| Rust        | ndjson  | 20m  |  27.74s |   721,070 | 63.3s | 2.28x | 15,132 MB |       -1,843 MB | 2,458 MB |
| Rust        | parquet | 10m  |   2.48s | 4,037,034 |  8.2s | 3.30x |  2,886 MB |        1,351 MB | 1,466 MB |
| Zig         | csv     | 20m  |   3.57s | 5,596,034 | 11.9s | 3.33x |  8,755 MB |        1,738 MB | 2,101 MB |
| Zig         | ndjson  | 20m  |  55.70s |   359,113 | 63.7s | 1.14x | 15,058 MB |       -1,917 MB | 2,101 MB |
| Zig         | parquet | 10m  |   2.47s | 4,041,723 |  8.6s | 3.49x |  2,775 MB |        1,241 MB | 1,703 MB |
| C++ avx2    | csv     | 20m  |   6.49s | 3,080,841 | 20.4s | 3.15x |  8,763 MB |        1,745 MB | 2,502 MB |
| C++ avx2    | ndjson  | 20m  |  39.75s |   503,148 | 57.5s | 1.45x | 15,113 MB |       -1,862 MB | 2,575 MB |
| C++ avx2    | parquet | 10m  |       - |         - |     - |     - |         - |               - |        - |
| Rust avx2   | csv     | 20m  |   3.73s | 5,366,909 | 12.5s | 3.36x |  8,761 MB |        1,743 MB | 2,458 MB |
| Rust avx2   | ndjson  | 20m  |  46.22s |   432,753 | 65.1s | 1.41x | 15,077 MB |       -1,898 MB | 2,458 MB |
| Rust avx2   | parquet | 10m  |   2.52s | 3,963,931 |  8.4s | 3.33x |  2,886 MB |        1,352 MB | 1,466 MB |
| Rust engine | csv     | 20m  |   3.17s | 6,307,378 | 10.8s | 3.40x |  8,728 MB |        1,710 MB | 2,458 MB |
| Rust engine | ndjson  | 20m  |  41.28s |   484,559 | 62.0s | 1.50x | 13,468 MB |       -3,507 MB | 1,946 MB |
| Rust engine | parquet | 10m  |   2.17s | 4,602,346 |  7.5s | 3.46x |  2,830 MB |        1,295 MB | 1,462 MB |
| Zig v32     | csv     | 20m  |   3.47s | 5,762,032 | 12.1s | 3.48x |  8,767 MB |        1,749 MB | 2,101 MB |
| Zig v32     | ndjson  | 20m  |  48.10s |   415,854 | 63.0s | 1.31x | 15,079 MB |       -1,896 MB | 2,028 MB |
| Zig v32     | parquet | 10m  |   2.47s | 4,047,652 |  8.8s | 3.55x |  2,746 MB |        1,211 MB | 1,724 MB |


### INTEL(R) XEON(R) PLATINUM 8573C — 4 cores, avx512

*18 measurements · sizes 10m, 20m · formats ndjson, parquet · 2 reported nothing*

| Build       | Format  | Size | Compare |    Rows/s |   CPU | Cores | Peak RSS | Above the input |   Budget |
| ----------- | ------- | ---- | ------: | --------: | ----: | ----: | -------: | --------------: | -------: |
| C           | ndjson  | 10m  |   5.93s | 1,687,352 | 19.0s | 3.21x | 9,204 MB |          716 MB |   766 MB |
| C           | parquet | 20m  |   3.12s | 6,413,184 |  9.7s | 3.12x | 5,760 MB |        2,691 MB | 3,065 MB |
| C++         | ndjson  | 10m  |   7.54s | 1,326,874 | 22.4s | 2.97x | 9,371 MB |          883 MB | 1,281 MB |
| C++         | parquet | 20m  |   4.38s | 4,563,154 | 14.5s | 3.31x | 5,640 MB |        2,570 MB | 2,912 MB |
| Rust        | ndjson  | 10m  |   6.48s | 1,543,208 | 24.0s | 3.70x | 9,388 MB |          901 MB | 1,233 MB |
| Rust        | parquet | 20m  |   4.99s | 4,007,009 | 16.9s | 3.39x | 5,683 MB |        2,613 MB | 2,883 MB |
| Zig         | ndjson  | 10m  |   6.07s | 1,646,855 | 23.0s | 3.78x | 9,343 MB |          856 MB |   955 MB |
| Zig         | parquet | 20m  |   4.58s | 4,364,710 | 16.5s | 3.59x | 5,479 MB |        2,410 MB | 3,424 MB |
| C++ avx2    | ndjson  | 10m  |   7.69s | 1,301,249 | 21.5s | 2.80x | 9,366 MB |          879 MB | 1,281 MB |
| C++ avx2    | parquet | 20m  |       - |         - |     - |     - |        - |               - |        - |
| C++ avx512  | ndjson  | 10m  |   8.29s | 1,206,070 | 23.6s | 2.84x | 9,362 MB |          874 MB | 1,285 MB |
| C++ avx512  | parquet | 20m  |       - |         - |     - |     - |        - |               - |        - |
| Rust avx2   | ndjson  | 10m  |   6.53s | 1,531,632 | 24.4s | 3.74x | 9,388 MB |          901 MB | 1,233 MB |
| Rust avx2   | parquet | 20m  |   5.00s | 4,001,437 | 17.3s | 3.45x | 5,753 MB |        2,683 MB | 2,884 MB |
| Rust engine | ndjson  | 10m  |   6.13s | 1,630,411 | 23.5s | 3.83x | 9,359 MB |          872 MB | 1,233 MB |
| Rust engine | parquet | 20m  |   4.49s | 4,453,351 | 16.0s | 3.57x | 5,629 MB |        2,560 MB | 2,870 MB |
| Zig v32     | ndjson  | 10m  |   5.58s | 1,793,841 | 20.8s | 3.73x | 9,319 MB |          831 MB |   955 MB |
| Zig v32     | parquet | 20m  |   4.63s | 4,316,001 | 16.3s | 3.53x | 5,501 MB |        2,432 MB | 3,423 MB |
| Zig v64     | ndjson  | 10m  |   5.58s | 1,793,566 | 21.3s | 3.82x | 9,356 MB |          869 MB | 1,023 MB |
| Zig v64     | parquet | 20m  |   4.58s | 4,368,440 | 16.5s | 3.60x | 5,506 MB |        2,437 MB | 3,344 MB |


### AMD EPYC 9V45 96-Core Processor — 4 cores, avx512

*10 measurements · sizes 10m · formats csv*

| Build       | Format | Size | Compare |    Rows/s |  CPU | Cores | Peak RSS | Above the input |   Budget |
| ----------- | ------ | ---- | ------: | --------: | ---: | ----: | -------: | --------------: | -------: |
| C           | csv    | 10m  |   1.11s | 9,010,413 | 3.7s | 3.30x | 4,225 MB |          716 MB |   766 MB |
| C++         | csv    | 10m  |   2.93s | 3,418,677 | 8.7s | 2.98x | 4,384 MB |          875 MB | 1,275 MB |
| Rust        | csv    | 10m  |   1.41s | 7,090,295 | 4.4s | 3.10x | 4,409 MB |          900 MB | 1,233 MB |
| Zig         | csv    | 10m  |   1.26s | 7,940,846 | 4.0s | 3.16x | 4,387 MB |          878 MB |   973 MB |
| C++ avx2    | csv    | 10m  |   2.88s | 3,478,114 | 8.7s | 3.01x | 4,385 MB |          876 MB | 1,275 MB |
| C++ avx512  | csv    | 10m  |   2.92s | 3,422,224 | 9.0s | 3.08x | 4,385 MB |          876 MB | 1,275 MB |
| Rust avx2   | csv    | 10m  |   1.36s | 7,333,594 | 4.3s | 3.14x | 4,410 MB |          901 MB | 1,233 MB |
| Rust engine | csv    | 10m  |   1.06s | 9,452,704 | 3.3s | 3.15x | 4,351 MB |          842 MB | 1,105 MB |
| Zig v32     | csv    | 10m  |   1.21s | 8,253,428 | 3.8s | 3.15x | 4,330 MB |          821 MB |   955 MB |
| Zig v64     | csv    | 10m  |   1.21s | 8,260,922 | 4.1s | 3.41x | 4,389 MB |          880 MB |   972 MB |


> **AMD EPYC 9V45 96-Core Processor is missing 20m.** Those rungs ran on other silicon and are in another table; this ladder is partial and its slope cannot be read as one curve.

> **3 of 54 rows carry no number:** C++ avx2 on parquet, C++ avx512 on parquet. A build that cannot read a format reports one of these; it is a dash above, not a zero and not a slow result.

### Read this before reading the ndjson rows

**The 20m ndjson rung did not measure the engines.** Its input is 16,975 MB on a
host with 15,989 MB of RAM. Every port reports a peak RSS of about 15,050 MB and
an *above the input* of roughly **-1,900 MB** -- negative, which is the kernel
evicting mapped pages that the port still wants. The rung took 13m50s where the
20m csv rung took 1m37s.

So 27.74s to 55.70s across the four ports there is a ranking of page-fault
behaviour under a working set that does not fit, not of parsing. The 10m ndjson
rung on the Xeon is a real measurement -- 9,204 MB peak, *+716 MB* above its
input -- and its ordering is different: C 5.93s, Zig 6.07s, Rust 6.48s, C++
7.54s, where at 20m Rust is first and Zig last. That is the same caution this
file has recorded twice before, arriving a third time by a new route.

### And the grouping earned its keep

Three CPUs in one dispatch: **EPYC 7763** (avx2) took three rungs, **Xeon
Platinum 8573C** (avx512) two, **EPYC 9V45** (avx512) one. A single merged table
would have put 10m csv from the 9V45 next to 20m csv from the 7763 and called it
a curve. The grouped output instead says, in the run's own words:

> **AMD EPYC 9V45 96-Core Processor is missing 20m.** Those rungs ran on other
> silicon and are in another table; this ladder is partial and its slope cannot
> be read as one curve.

Which is the whole point, and is exactly what the run could not print.

---

## 2026-09-19 (lazy cells) — the changed-row report materialised nine values for every one it printed

The entry below gated the whole report tail behind `--json`. That made the
default invocation 1.14x faster and, as it said at the time, left the `--json`
path exactly where it was. This is that path.

### The arithmetic

`assemble` built both whole rows of every kept changed pair as `std::string`s,
then walked them to find which cells differed. On the 4M pair at the default
`--max-rows`:

| | |
|---|---:|
| kept changed rows | 50,000 |
| columns in a row | 19 |
| `Val`s materialised | 50,000 x 2 sides x 19 = **1,900,000** |
| differing cells found | 52,455 (1.05 per row) |
| `Val`s the JSON actually carries | 50,000 x key(2) + 52,455 x 2 = **204,910** |

A factor of **9.3**. `json.cpp` emits a changed row as its key plus the cells
that differ -- added and removed rows do print every column, and still do -- so
the other 1.7 million strings were allocated, sorted past, compared and freed
without ever being looked at.

### What it does now

Only the key is materialised before the sort, because the sort is what needs it.
After the sort each row is re-parsed and the differing cells are found from the
`Field`s -- `cell_differs` reads mapped bytes and allocates nothing -- so a cell
becomes a `std::string` only if it is going to be printed.

Re-parsing rather than carrying the `Field`s through the sort is deliberate:
parsing a row is field arithmetic over bytes already mapped, and holding them
instead would have the sort move 16 MB to avoid it. The allocations were the
cost, not the parse.

One behavioural note: the report now names differing cells with `cell_differs`,
the same predicate the join used to decide the row was changed. The old code
re-derived the test at `Val` level and said the same thing by a different route.
Agreeing with the join by construction is the better of the two.

### The trap: a second call site cost 3% in a mode that never runs it

The first measurement had `--json` at 0.979 and summary-only at **1.033 slower,
0 rounds faster of 11** -- in a mode where `row_lists` is false and every line of
the diff is gated off. Nothing changed on that path, and it got consistently
slower.

`nm -C` on the two binaries:

    base:  (no cell_differs symbol -- inlined into the join, its only caller)
    new:   t csvdiff::(anonymous namespace)::cell_differs(...)

Calling `cell_differs` from the report gave it a second call site, and that was
enough for GCC to stop inlining it and emit one shared copy. The join's inner
loop lost its inlined body, and the join runs in **both** modes. Marking it
`[[gnu::always_inline]] inline` put the symbol back to nothing; the report loop
is cold, so a second inlined copy there is free.

Worth stating plainly: a 3% regression from adding a call, on a path whose source
was untouched. Only the paired harness caught it, and only because it measures
both modes.

### What it is worth

Thirteen paired rounds on the 4M CSV pair, arms rotated, four threads, summary
identical between arms in both modes before any timing:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| summary only, no `--json` | 2.06s | 2.08s | 1.004 | 0.988-1.012 | 4/13 |
| with `--json` | 2.74s | 2.59s | **0.945** | 0.919-0.957 | **11/13** |

Summary-only straddles 1.00, which is no result and is the right answer for code
that mode does not execute. CPU seconds fall 7.62 to 7.43.

Per-phase, seven interleaved rounds, medians:

| phase | base | new | delta |
|---|---:|---:|---:|
| assemble | 0.333s | 0.210s | **-0.123s** |
| join and compare | 1.458s | 1.403s | -0.055s |
| A / B sweep, A / B insert | | | within noise |

`assemble` is **37% cheaper**. The two sweeps run concurrently and trade places
between runs; the join delta is inside the band that summary-only's 1.004 calls
nothing.

**Unlike the entry below, this one does move the published tables.**
`bench_ports.py` and `bench_formats_ports.py` both pass `--json`, so every
cross-port number in this file was measured on exactly this path.

Verified beyond the counts gate: the `--json` payload is byte-identical to the
previous build, apart from `meta.seconds`, across seven option combinations --
none, `--trim`, `--ignore-case`, `--empty-is-null`, `--tolerance 0.5`,
`--trim --ignore-case --empty-is-null`, and `--tolerance 1000 --trim`. Counts
gates: cpp 36/36, cross-port 89/89.

### What is still on the table

Making `Val` a view instead of an owning string would remove the remaining
allocations, and it is not blocked by a lifetime argument -- it is blocked by an
API one. `Slab a(a_path), b(b_path)` are locals in `compare()` and `Result` is
returned by value, so a view into the mapped bytes would dangle at the return.
Changing that means changing what `compare()` hands back, which is a different
change from this one.

---

## 2026-09-18 (report gate) — C++ built the row samples nobody asked for

`assemble` was the one phase in the entry below that nobody had looked at. It is
11% of a four-thread run and does not scale. Looking at it found a whole phase of
work done for output that is thrown away.

### What it was doing

`main.cpp` sets `row_lists = !json_path.empty()` and says why:

    Nothing but the summary line is printed unless --json was given, and the
    summary is counts. Collecting the row samples then costs a pass over B for
    output nobody asked for.

That flag already skipped the B-side pass. It did not skip the rest. Sub-marks
inside `assemble` on a 4M pair, four threads:

| | |
|---|---:|
| pairs / `row_values` | 0.119-0.139s |
| the three sorts | 0.024-0.035s |
| per-cell diff | 0.027-0.038s |
| both duplicate sections | 0.009-0.013s |

At the default `--max-rows` of 50,000 the first line is two `row_values` per kept
changed row, each a `std::string` per cell -- about **two million allocations**,
followed by sorting and diffing them, for a `Result` whose row lists are read
only by `to_json`, which runs only when `--json` was given.

So the whole tail from the first `row_values` to the duplicate sections is now
inside `if (opt.row_lists)`. The counts, the column stats and the three
truncation flags all come from the join and the indexes rather than from here, so
they are unaffected.

### What it is worth

Eleven paired rounds on a 4M CSV pair, arms rotated, four threads, summary
identical between arms in both modes before any timing:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| summary only, no `--json` | 1.65s | 1.45s | **0.874** | 0.849-0.888 | **11/11** |
| with `--json` | 1.89s | 1.86s | 0.990 | 0.969-1.000 | 9/11 |

1.14x on the default invocation. With `--json` the gate is open and nothing
changes, which is the point.

**The published tables will not move.** `bench_ports.py` and
`bench_formats_ports.py` both pass `--json`, so every benchmark in this file
measures the path this does not touch. What it speeds up is
`csvdiff compare a b -k id`, which is what the tool does when you just want the
counts.

Verified beyond the counts gate: the `--json` payload is byte-identical between
the two builds apart from `meta.seconds`.

### The other three ports do not have this problem, and not because they gate it

What `--json` costs each port on the same pair, measured rather than read --
`scripts/json_sample_cost.py`:

| port | no `--json` | `--json` | samples cost |
|---|---:|---:|---:|
| C | 0.822s | 0.848s | 3% |
| C++ (after this change) | 1.467s | 1.990s | **36%** |
| Rust | 1.230s | 1.235s | 0% |
| Zig | 1.022s | 1.034s | 1% |

Two readings fit 0-3%: those ports skip the samples too, or they build them so
cheaply it does not show. The profile entry further down settles it -- C renders
its rows by slicing the mapped bytes and allocates nothing, where C++ makes a
`std::string` per cell. So there is no equivalent waste in them to remove, and
this change does not port.

It also prices the thing that entry pointed at. C++ is **1.78x C without the
samples and 2.35x with them**: over half of what is left of its CSV gap in report
mode is the row materialisation, which is the `Val`-as-a-view change that entry
declined to make. The gate removes the cost when the output is unwanted; it does
nothing about the cost when it is wanted.

## 2026-09-18 (where it goes) — the sweep is the phase that does not scale

The correction below removed the wrong answer to "where does the serial time go"
without supplying a right one. This supplies one, and it is not the phase anyone
here has been chasing.

Every phase scaled separately, 1 thread against 4, taking the max of the two
concurrent index sides rather than their sum. 4M CSV pair, median of five runs.
`scripts/phase_scaling.py`.

| phase | 1 thread | 4 threads | scales | % of the 4-thread wall |
|---|---:|---:|---:|---:|
| wall | 2.923s | 1.679s | 1.74x | |
| index region (max side) | 0.538s | 0.631s | **0.85x** | 38% |
| &nbsp;&nbsp;of which **sweep** | 0.304s | 0.387s | **0.79x** | **23%** |
| &nbsp;&nbsp;of which insert | 0.247s | 0.242s | 1.02x | 14% |
| join and compare | 2.061s | 0.799s | **2.58x** | 48% |
| assemble | 0.188s | 0.190s | 0.99x | 11% |

The critical path sums to 1.620s against a 1.679s wall -- 96%, the rest being
startup, the mapping and teardown. **This time the arithmetic closes**, which is
the check the two entries below failed.

### The sweep gets slower with more threads

It is labelled `(parallel)`. It is the phase that is supposed to benefit. It goes
from 0.304s to 0.387s when the thread count goes up, and at 23% of wall it is the
**largest non-scaling phase in the run** -- larger than the insert that the last
three entries were about.

The thread arithmetic explains how that is possible, not why it happens.
`per_file` is `threads / 2`, and the two sides run at once:

| `--threads` | threads per side | total on four cores |
|---|---:|---:|
| 1 | 1 | 2 |
| 4 | 2 | 4 |

At one thread the two sweeps get a core each with two cores spare. At four they
have four threads on four cores and nothing spare. Doing the same bytes with
twice the threads in *more* wall time is the signature of a phase limited by
something other than instruction issue -- memory bandwidth is the obvious
candidate on a scan of 1.5 GB, and this host has no `perf` to name it.

### What actually does not scale

| | share of wall | scales |
|---|---:|---:|
| sweep | 23% | 0.79x |
| insert | 14% | 1.02x |
| assemble | 11% | 0.99x |
| **total flat or worse** | **48%** | |

Forty-eight per cent, against the 45% the 1-to-4 speedup implies by Amdahl. Two
routes to one number -- which is the agreement the correction below believed it
had and did not.

So the serial half is real, and it is **three phases, not one**. In order: the
sweep, then the insert, then assemble. Every entry in this file that treated the
insert as the bottleneck was ranking the second-largest.

### What this changes about what is worth doing

Parallelising the insert is worth about 1.12x, as the correction below says. The
sweep is bigger but a *worse* target, not a better one: it already has the
threads and is losing on them, so more parallelism is the thing it is failing at.
Its question is bandwidth and layout, not cores.

`assemble` at 11% has still never been looked at, and is the only one of the three
that nobody has explained.

## 2026-09-18 (correction) — the insert is 15% of the run, not half of it

The scaling entry below says every port spends 38-54% of a four-core run in a
serial index insert, calls that the largest opportunity this project has
measured, and puts an upper bound of 1.27x to 1.56x on parallelising it. **The
attribution is wrong and the bound with it.** On the critical path the insert is
14-16%.

### The error

Each port builds the two indexes *at the same time*, on two threads. The C++
source says so where it starts them:

    The two indexes share nothing, so they are built at the same time, and
    each is split further into chunks.

So `A index insert` and `B index insert` are two phases that **overlap**. That
entry added them together and divided by wall, which counts a concurrent pair as
if it were a sequence. What the critical path sees is the longer of the two, not
their sum.

The arithmetic said so at the time and was not checked: the phases of a C++
four-thread run sum to 2.15s against a 1.62s wall. A phase breakdown that adds up
to more than the run it came from has overlapping phases in it, and that is the
whole of this correction.

### The corrected numbers

4M CSV pair, four threads, median of five runs. `scripts/serial_share.py`.

| port | wall | insert, summed | insert, max of the two sides | published | **actual** |
|---|---:|---:|---:|---:|---:|
| C | 0.832s | 0.228s | 0.115s | 48% | **14%** |
| C++ | 1.672s | 0.482s | 0.246s | 43% | **15%** |
| Rust | 1.212s | 0.334s | 0.168s | 29% | **14%** |
| Zig | 1.023s | 0.323s | 0.166s | 33% | **16%** |

And the upper bound, recomputed. If the insert parallelised perfectly across four
cores *within each side*:

| port | now | insert at 4x | gain | published |
|---|---:|---:|---:|---:|
| C | 0.832s | 0.746s | 1.12x | 1.56x |
| C++ | 1.672s | 1.487s | 1.12x | 1.48x |
| Rust | 1.212s | 1.086s | 1.12x | 1.27x |
| Zig | 1.023s | 0.898s | 1.14x | 1.33x |

About 1.12x, not 1.27-1.56x. Still real, no longer the largest thing on the
table, and no longer worth the design it would take -- sharding the index by hash
would break the first-occurrence ordering that `first_rows()` guarantees and the
truncated-report path depends on, and that is a lot of risk for a tenth.

### What survives

**The ports really are 38-54% serial.** That number came from the 1-to-4 speedup
by Amdahl, independently of any phase breakdown, and nothing here touches it. C++
still gains nothing from its second thread; Zig is still slower at four than at
three.

What is gone is the claim to know *where* that serial time goes. The insert is
15% of it. The other 25-35% is unattributed: `assemble` does not scale and is
11%, and the rest has not been measured. **The scaling curve was right and the
explanation underneath it was wrong**, which is the more useful half to have
lost, because it was the half being planned against.

### The habit that would have caught it

Both of the last two corrections in this file were arithmetic, not statistics. A
0.7s phase saving that moves a 3.27s total by 0.1s is a phase measured once. A
phase breakdown summing to 2.15s inside a 1.62s wall has concurrency in it.
Neither needed a better instrument -- only adding the numbers up and asking
whether the total was possible.

## 2026-09-18 (insert prefetch) — the first bite out of the serial half

The entry below found every port spending 38-54% of a four-core run in a serial
index insert, and named parallelising it as the largest opportunity measured
here. This is not that. It is the cheap thing to try first, and it works.

### Why a prefetch

The insert is latency-bound rather than throughput-bound. Each row lands on a
slot its hash chose, the table is 32 MB at ten million rows across both sides,
and nothing about one row predicts the next one's cache line. Measured at about
**90 ns a row** — 0.74s for the eight million rows of a 4M pair's two indexes —
which is the shape of a trip to memory, not of the dozen instructions the probe
runs.

The hash of the row 24 ahead is already in hand when the loop starts, so the line
it will want can be asked for now. `pqdiff.cpp` already does exactly this in its
columnar join, with the same constant; this is that, on the other side of the
index.

### What it is worth

The phase itself, median of seven runs each:

| index insert | base | new | |
|---|---:|---:|---:|
| 1 thread | 0.769s | 0.615s | **0.800x** |
| 4 threads | 0.740s | 0.572s | **0.773x** |

End to end, eleven paired rounds on a 4M pair, arms rotated, counts gated:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| csv, 4 threads | 1.96s | 1.85s | **0.947** | 0.929-0.975 | 9/11 |
| csv, 1 thread | 3.27s | 3.17s | 0.986 | 0.953-0.999 | 10/11 |
| ndjson, 4 threads | 3.74s | 3.63s | **0.975** | 0.965-0.984 | **11/11** |
| ndjson, 1 thread | 5.56s | 5.49s | 0.989 | 0.979-1.014 | 7/11 |

Both formats gain at four threads, which is the default. Both single-thread bands
sit at or across one, so nothing is claimed there. No format regressed, which is
worth stating explicitly after the `same` inline two entries below did.

### A correction, and why the phase number is smaller than it first looked

The first reading of this was a single run of each arm and said the insert went
from 1.315s to 0.599s — better than two to one. It was wrong. Across seven runs
the baseline phase ranges 0.725-0.906s, and that 1.315s sits well outside it: a
cold first run, measured once and believed.

The tell was arithmetic rather than statistics. A 0.7s saving on a 3.27s
single-threaded run should have moved the total to about 0.78, and the paired
rounds said 0.986. When the phase and the total disagree by that much, the phase
was measured once.

### What is left

This takes about a fifth off the serial insert. It does not make it parallel, and
the entry below's headline stands: the insert is still the serial half, still 38-54%
of a four-core run, and still the largest thing on the table. A prefetch hides
latency; it does not add cores.

### Correction: C++ was the only port without this

This entry first closed by saying the other three ports had the same loop and
had not been given the same treatment, and that C was the obvious next one. That
is wrong, and it was wrong when written. All three already prefetch their text
index insert:

| port | site |
|---|---|
| C | `c/csvdiff.c`, `__builtin_prefetch` on `row_hash[r + PREFETCH_AHEAD]` |
| Rust | `rust/src/engine/turbo.rs`, `idx.prefetch(soon)` on `chunk.hash[i + PREFETCH_AHEAD]` |
| Zig | `zig/src/csvdiff.zig`, `self.prefetch(hashes[i + PREFETCH_AHEAD])` |

C++ was the only one missing it, which is the same shape as the row-end fix
further down this file: three ports carrying a technique and one that never got
it. It also explains why C++'s insert was the worst of the four in absolute terms
before this change.

The error was a truncated search -- `grep prefetch rust/src | head -5` returned
five hits from `pqdiff.rs` and stopped before `turbo.rs`, and "no hits in the
text path" was concluded from a list that had been cut short. A `head` on a
search whose *absence* is the finding is not a search.

So there is no port left to apply this to, and C's insert being 48% of its run is
what the technique leaves behind rather than what it would remove.

## 2026-09-17 (scaling) — every port is half serial, and it is the same half

The C++ gap work kept turning up a number beside the one it was chasing: on four
cores, C++ got 1.70x. So did the others, roughly, and nobody had asked why. This
is that question, and the answer is the largest single opportunity this project
has measured — it is shared by all four ports, it is not language-specific, and
it is worth more than every port-level change in this file put together.

### The curve

4M CSV pair, five rounds, ports rotated. Cell is wall / cores busy.

| port | 1 thread | 2 | 3 | 4 | 1→4 | implied serial |
|---|---|---|---|---|---:|---:|
| C | 1.52s / 1.27x | 1.02s / 1.87x | 0.89s / 2.25x | 0.82s / 2.91x | 1.86x | 38% |
| C++ | 2.74s / 1.20x | **2.78s** / 1.19x | 1.91s / 1.72x | 1.61s / 2.42x | 1.70x | 45% |
| Rust | 2.00s / 1.30x | 1.36s / 1.89x | 1.13s / 2.25x | 1.10s / 2.70x | 1.81x | 40% |
| Zig | 1.48s / 1.32x | 1.02s / 1.95x | 0.89s / 2.24x | **0.97s** / 3.11x | 1.53x | 54% |

Four independent implementations, all between 1.53x and 1.86x on four cores, all
implying a serial fraction between 38% and 54%. Two of them get *worse* somewhere:
C++ gains nothing from its second thread, Zig loses time going from three to four
while burning 3.11 cores.

Four ports of one design landing on one number is the design, not the code.

### Where the serial half is

`CSVDIFF_PHASES=1` says it outright, and the ports label it themselves. At four
threads on the same pair:

| port | serial index insert | % of wall | parallel join | 1→4 |
|---|---:|---:|---:|---:|
| C | 0.394s | **48%** | 0.275s | 1.86x |
| C++ | 0.692s | **43%** | 0.756s | 1.70x |
| Rust | 0.315s | **29%** | 0.247s | 1.81x |
| Zig | 0.324s | **33%** | 0.268s | 1.53x |

The phase is *named* `index insert (serial)` in three of the four and
`insert in order` in C. The shape is the same everywhere: sweep the rows in
parallel, **insert them into the hash index on one thread**, then join in
parallel.

In C the serial insert is now *longer than the parallel join it feeds* — 0.394s
against 0.275s. The port has optimised the parallel half until the serial half
is the bigger one.

C++'s own phase timings show it directly, 1 thread against 4:

| phase | 1 thread | 4 threads | scales |
|---|---:|---:|---|
| sweep (parallel) | 0.576s | 0.722s | — |
| **index insert (serial)** | **0.538s** | **0.515s** | **no** |
| join and compare | 1.883s | 0.756s | 2.49x |
| **assemble** | **0.177s** | **0.177s** | **no** |

The join scales 2.49x. The insert does not move at all, and neither does
assemble. 0.692s of a 1.61s run, which is the 43% above and within a point of the
45% Amdahl implied by the speedup — two independent routes to the same number.

### What it would be worth

If the insert parallelised perfectly on these four cores:

| port | now | insert at 4x | gain |
|---|---:|---:|---:|
| C | 0.82s | 0.52s | **1.56x** |
| C++ | 1.61s | 1.09s | 1.48x |
| Rust | 1.10s | 0.86s | 1.27x |
| Zig | 0.97s | 0.73s | 1.33x |

Perfect parallelisation is not on offer and this is an upper bound, not a
forecast. But even half of it is larger than anything else measured in this file,
it applies to every format rather than one, and it applies to all four ports at
once.

### What it does not say

This is one host, four cores, one format, one size. The serial fraction is a
*fraction*, so it should hold shape at other sizes, but that is an argument and
not a measurement. Nothing here says the insert *can* be parallelised — a hash
index built in a deterministic order is not trivially shardable, and the ports
agree on counts partly because they insert in one order. Whatever replaces it has
to keep the cross-port oracle green, which is the constraint that makes this
interesting rather than obvious.

The harness is `scripts/scaling_curve.py`.

## 2026-09-17 (C++ writes) — the write misses are the report path, not the join

The entry below ended by saying the next person should go after C++'s writes,
since its last-level write misses are 2.96x C's where its read misses are fewer.
This is that, one level down, and it lands somewhere the entry did not predict.

Last-level write misses by function, 200k CSV pair, one thread:

| C — 219,142 total | | C++ — 649,350 total | |
|---|---:|---|---:|
| `sweep_part` | 79,716 | **`row_values`** | **209,251 (32%)** |
| `__memcpy` | 75,084 | `RowIndex::RowIndex` | 107,191 (17%) |
| `build_part` | 47,951 | `__memcpy` | 99,748 (15%) |
| `__memset` | 15,623 | `RowIndex::sweep` | 76,476 (12%) |
| | | `vector<int>::_M_fill_assign` | 66,046 (10%) |
| | | `_int_malloc` | 43,141 (7%) |

`row_values` with its `memcpy` and its `malloc` is **54% of C++'s write misses**.
That is the *report* path: for every row that is reported — changed, added or
removed — it builds a `std::vector<Val>` where `Val` is
`std::optional<std::string>`, so every reported cell is an allocation and a copy.
C renders the same rows by slicing the mapped bytes and allocates nothing.

By subsystem, with the groups matched between the ports:

| | C | C++ | ratio |
|---|---:|---:|---:|
| scan + parse, instructions | 539.3 M | 621.1 M | 1.15x |
| scan + parse, writes | 49.6 M | 45.9 M | **0.92x** |
| index build + sweep, instructions | 73.8 M | 148.1 M | 2.01x |
| index build + sweep, writes | 7.8 M | 15.7 M | 2.03x |
| string machinery, instructions | 1.1 M | 67.6 M | 61x |
| string machinery, writes | 0.6 M | 17.2 M | 29x |

The hot path is fine. On scan and parse — 55% of C++'s instructions — it is 1.15x
on work and actually writes *fewer* bytes than C. The excess is the index (2.0x)
and the strings (29x on writes, against a C path that has none).

### What this is worth, and what it is not

The string machinery is 68M of C++'s 1,139M instructions: **6%**. But it is 54%
of the write misses, and the envelope in the entry below put the whole write-miss
excess at 8-26% of the run. So this is plausibly worth more than its instruction
share and certainly not worth 1.88x. **It does not explain the gap** — nothing
found so far does, and scan+parse at 1.15x on 55% of the instructions says the
gap is not concentrated anywhere obvious.

It is also dataset-dependent in a way the rest is not: this pair has 6% of its
rows changed, and `row_values` runs once per reported row. A diff where most rows
changed would pay far more; one with no differences would pay none.

### The change it implies, and why it is not made here

`Val` is `std::optional<std::string>` in `cpp/src/csvdiff.hpp`, used across the
text and columnar paths. Making it a view over the mapped bytes would remove the
allocation and the copy, and it is a real cross-file change with a lifetime
condition to prove: the `Slab` must outlive every report that borrows from it.
That is a design decision and it should be made deliberately on these numbers
rather than folded into a benchmark entry. The numbers are here so it can be.

`vector<int>::_M_fill_assign` at 66k misses is `slots_.assign(n, -1)` rebuilding
the parser's slot table; it is not investigated here and is the other loose end.

## 2026-09-17 (C++ CSV gap) — what it is not, and the one lead that survived

No change here. C++ is the slowest port on CSV by a wide margin and the entry
below took the first bite out of it; this is the attempt to find the rest, and
most of it is a list of explanations that turned out to be wrong. That list is
the useful part, because each one is a day somebody else now does not spend.

### It is not the parallel path

The published table is four-threaded and the profile is single-threaded, which
is two different claims about one gap. Measured on a 4M CSV pair, seven paired
rounds, ports rotated:

| | wall | cpu | cores |
|---|---:|---:|---:|
| C, 1 thread | 1.41s | 1.76s | 1.25x |
| C, 4 threads | 0.79s | 2.36s | 2.99x |
| C++, 1 thread | 2.65s | 3.19s | 1.20x |
| C++, 4 threads | 1.59s | 3.82s | 2.40x |

C++/C is **1.883x on one thread** and 2.013x on four. The gap is already there
before any thread is started, so threading is not where to look. (Both ports
scale poorly — 1.78x and 1.67x from one core to four, on three cores' worth of
CPU. That is its own finding and not this one.)

### It is not the instruction count

Instructions, same 200k pair, single-threaded: C 853M, C++ 1,139M, **1.336x**.
Wall at that same size: **1.680x**. A third of the gap is work; the rest is not.

### It is not size or cache pressure at scale

The obvious objection to profiling 200k and measuring 4M is that the working
sets differ. They do, and it does not rescue the count:

| C++/C, one thread | 200k | 4M |
|---|---:|---:|
| wall | 1.680x | 1.868x |

The gap grows with size, so there is *some* scale component, but the bulk of it
is present at 200k where the profile was taken.

### It is not cache misses or branch mispredictions — per instruction, C++ is better

`--cache-sim=yes --branch-sim=yes`, 200k pair, one thread:

| | C | C++ | ratio |
|---|---:|---:|---:|
| instructions | 852,809,297 | 1,139,272,760 | 1.34x |
| data reads | 201,163,763 | 283,073,026 | 1.41x |
| data writes | 77,417,513 | 113,068,563 | 1.46x |
| L1 read misses | 3,185,854 | 5,009,893 | 1.57x |
| **last-level read misses** | **1,208,062** | **1,157,321** | **0.96x** |
| last-level write misses | 219,142 | 649,350 | 2.96x |
| branch mispredicts | 4,131,063 | 4,971,806 | 1.20x |
| mispredict rate | 3.05% | 2.68% | — |

C++ takes **fewer** trips to main memory for reads than C does, in absolute
terms, and mispredicts a *smaller* fraction of its branches. Normalised per
instruction it is ahead on both: 1.59 last-level misses per 1k instructions
against C's 1.67. Whatever costs C++ its 1.68x, this model does not see it.

The caveat that keeps this from being conclusive: callgrind simulates a generic
two-level cache, not this processor's. A model that says "no difference" is
weaker evidence than a model that finds one.

### The lead that survived

Two numbers in that table do not fit the pattern. C++ issues **1.05x the data
reads and 1.09x the data writes per instruction**, and its last-level *write*
misses are **2.96x** C's — 430k extra, where its read misses are fewer.

Writes that miss are not free: the line has to be fetched for ownership before
the store retires. A back-of-envelope on 430k extra misses, at 60 to 150 cycles
and 2.1 to 2.8 GHz, lands between 8% and 26% of a 120 ms run. That is a wide
band from assumed constants and it is **not a measurement** — but it is the only
quantity found here that is both large enough to matter and worse in C++.

So: the next person to pick this up should go after the writes, not the reads,
not the branches, and not the thread count. Where C++ stores and C does not is
the question, and this host has no `perf` to answer it directly.

## 2026-09-17 (memory cap) — the confound that was not one, and nine rounds that lied

The entry below marked one column of the ndjson table as suspect. The ladder run
on a Xeon Platinum 8370C is the only one where Rust came last, and it is also the
only one measured by the ladder harness, which passes `--memory-cap` where the
other does not. Two differences, one result: that is a confound, and the column
was discounted pending a test.

This is the test. One machine, one binary, one input, the cap set to exactly what
`bench-ladder.yml` computes (`MemTotal - 2048 MB`, here 14,047 MB), applied with
`RLIMIT_DATA` in the child. Arms rotated each round. Twenty-one paired rounds on a
4M ndjson pair:

| capped / free | median | middle half | slower in |
|---|---:|---|---|
| Rust | 1.002 | 0.980-1.017 | 11/21 |
| C | 1.002 | 0.971-1.018 | 11/21 |

Nothing, for either port. **The cap is ruled out**, the 8370C column is an
ordinary result from an ordinary CPU, and the reading that the ndjson ordering
varies by processor survives with one fewer caveat.

### The part worth keeping

The first attempt ran nine rounds, and said this:

| capped / free | median | middle half | slower in |
|---|---:|---|---|
| Rust | **1.034** | 0.952-1.093 | 6/9 |
| C | 0.989 | 0.979-1.024 | 3/9 |

Rust 3.4% slower under the cap, slower in two thirds of the rounds, and C
untouched. That is a tidy story, it is the story the confound predicted, and it
is false: twelve more rounds took the median from 1.034 to 1.002 and the count
from 6/9 to 11/21.

The middle half said so at the time — 0.952-1.093 straddles one, and the rule at
the top of this file is that a straddling band is not a result. The median and
the win count were the tempting numbers and both were noise. This is the cheapest
possible reminder that the band is the part to read, and that a ratio which
agrees with the hypothesis you already have is the one to re-run.

## 2026-09-17 (C++ predicates) — one inline pays, the next one costs, and a fourth CPU

C++ is 2.46x behind C on CSV in the table below, which is the widest gap in it —
wider than anything on ndjson. This is the first cut at it, and the second half
of the entry is the more useful half.

### Where the gap is

callgrind, 200k CSV pair, one thread, `-march=x86-64-v3 -g`:

| | C | C++ |
|---|---:|---:|
| total instructions | 855 M | 1,198 M |
| row parse | `parse_csv_row` 36.2% | `parse_csv` 30.0% |
| scan | `next_of2` 26.9% | `next_of2` 21.8% |
| hash | `hash_field` 12.2% | inlined |
| **absent predicate** | **none** | **`is_absent` 6.5%** |

1.40x on instructions against 2.46x on the clock, so most of the wall-time gap is
still not explained by the count — the same thing the profile entry below found
across ports, now inside one format.

The line that stands out is `is_absent`. C++ asks it 2.45M times on a 200k pair —
1.21M from `same`, 800k from the index sweep, 437k from `compare` — for 78M
instructions, and **C has no equivalent at all**: its absent case is a sentinel
the caller tests inline. The fast path is two loads, a shift and a compare, about
ten instructions. It was costing thirty-two, because the function also calls
`value_of`, which builds a `std::string`, and a body that can allocate is over
the compiler's inlining threshold.

### Splitting the cold tail out

Move the normalising branch into its own function and mark the predicate
`inline`. The 2.45M calls become zero and the count falls 1,198M to 1,139M
(0.951x).

Eleven paired rounds on a 4M pair, arms rotated, counts gated:

| | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| csv | 1.64s | 1.58s | **0.978** | 0.947-0.994 | 9/11 |
| ndjson | 3.48s | 3.50s | 0.984 | 0.978-1.020 | 7/11 |

CSV is a result. ndjson crosses one and nothing is claimed for it.

### The half that is worth more: the same trick on `same` is a regression

`same` has the identical shape — a cheap common case in front of a `value_of`
call — so it got the identical treatment in the first attempt. Measured:

| both inlined | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| csv | 1.74s | 1.65s | 0.978 | 0.947-1.001 | 8/11 |
| ndjson | 3.31s | 3.43s | **1.034** | **1.023-1.069** | **1/11** |

ndjson's whole middle half sits above one, in one round of eleven was it faster.
That is a real slowdown, and it took the CSV band with it: 0.947-1.001 straddles
where 0.947-0.994 did not.

And the instruction count *disagrees with the clock in both directions at once*:

| | instructions | csv | ndjson |
|---|---:|---|---|
| base | 1,198 M | — | — |
| `is_absent` inlined | **1,139 M** (0.951x) | 0.978 | 0.984 |
| both inlined | 1,153 M (0.962x) | 0.978 | **1.034** |

Inlining `same` *added* 14M instructions over inlining `is_absent` alone —
an inlined `same` duplicates the `is_absent` work at each of its call sites —
and it still executed fewer instructions than the base while running ndjson
slower than the base. Fewer calls is not less work, and less work is not less
time.

This sharpens what the profile entry below concluded. That one found instruction
counts useless for ranking *ports* and noted that within one port, one change,
they tracked the wall to a thousandth. Both halves survive, with a condition: the
row-end fix removed *work* — whole scans that stopped happening — and there the
count tracked. This change removes *call overhead* and adds *code size*, and
there it does not. The count is a good proxy for work removed and a bad one for
work rearranged.

Only `is_absent` is shipped.

### A fourth CPU, and the ndjson order replicates

The 10M CI run below was repeated on `main`. It landed on a processor none of the
earlier runs used — **Intel Xeon Platinum 8573C, avx512** — and its ndjson
ordering is the EPYC 9V74's, exactly:

| ndjson, 10M | EPYC 9V74 avx2 | Xeon Platinum 8573C avx512 |
|---|---|---|
| 1st | Zig 7.20s | Zig 5.47s |
| 2nd | C 7.41s | C 5.68s |
| 3rd | Rust 8.04s | Rust 6.34s |
| 4th | C++ 10.39s | C++ 7.87s |

Two different vendors, two different vector widths, same order. That is the first
ndjson ordering this project has reproduced on independent hardware, and it makes
the entry below's "three machines, three winners" too strong a reading: the
orderings are not arbitrary, they are just not universal. Zig-then-C is what two
CI runs agree on.

The one that disagrees most (Xeon Platinum 8370C, where Rust came last) is also
the only one measured by the *other* harness, `bench_formats_ports.py` under the
ladder, which additionally passes `--memory-cap`. Same flags otherwise, same
generator output size. **That confound was tested and ruled out** — see the entry
above this one. The cap costs neither port measurable time, so the 8370C column
is an ordinary CPU result and stands.

## 2026-09-17 (10M, on CI) — the ndjson ranking is not portable

Ten million rows on GitHub's runners, which is where this table belongs: the
container numbers first written here were one machine nobody else can rent.
Running it properly turned up something bigger than the table.

### One CPU, all three formats

`benchmark-native.yml` runs every format in a single job, which is the only
arrangement that gets one processor across all three.
[Run 35181504756](https://github.com/andrey-usa/csvdiff/actions/runs/35181504756),
**AMD EPYC 9V74, 4 cores, AVX2**, three interleaved runs each, counts gated.

| Port | csv 3,509 MB | ndjson 8,487 MB | parquet 2,074 MB |
|---|---:|---:|---:|
| C | **1.83s** (6.4s cpu) | 7.41s (**24.4s cpu**) | **1.53s** (4.9s cpu) |
| C++ | 4.51s (13.9s) | 10.39s (30.7s) | 2.59s (8.4s) |
| Rust | 2.10s (6.8s) | 8.04s (30.4s) | 2.79s (9.2s) |
| Zig | 2.15s (7.1s) | **7.20s** (27.6s) | 2.93s (10.3s) |

C leads CSV by 1.15x over Rust and Parquet by 1.69x over C++. Zig takes ndjson
by 1.03x over C, which is inside what a ratio of bests can resolve here — a tie,
and C reaches it on 24.4 CPU-seconds against Zig's 27.6. C++ is last on both
text formats, 2.46x behind C on CSV.

### The finding: the ndjson ranking is not a property of the code

Three machines measured this same tree at 10M inside one hour:

| ndjson, 10M | container Xeon @2.10GHz avx512 | CI Xeon Platinum 8370C avx512 | CI EPYC 9V74 avx2 |
|---|---|---|---|
| 1st | **Rust** 5.08s | **C** 7.95s | **Zig** 7.20s |
| 2nd | Zig 5.48s | Zig 8.26s | C 7.41s |
| 3rd | C 6.13s | C++ 8.96s | Rust 8.04s |
| 4th | C++ 8.76s | **Rust** 9.96s | C++ 10.39s |

Rust is first on one and last on another with nothing changed between them. This
file already said never to compare across tables; what it did not say, because
nothing here had been arranged to notice, is that **one CI workflow run is not
one table**. The ladder fans a size or a format out per job to get parallelism,
and those jobs land on different hardware — the run above put csv and parquet on
an EPYC 7763 and ndjson on a Xeon Platinum 8370C, in one dispatch.

So the harness now records the CPU in its JSON and `scripts/bench_group.py`
groups on it, printing one table per processor and naming what is missing from
each. The first grouped run caught the split immediately.

What survives all three columns: C++ is last or second-to-last on ndjson
everywhere, and C is never worse than third. Those are the claims worth acting
on. "Rust leads ndjson", published here earlier today on the strength of the
container column, is not one of them.

### Also not comparable, and it looks like it should be

The two harnesses generate different Parquet. `bench_formats_ports.py` uses the
Rust generator and `bench_ports.py` the C one, and for the same ten million rows
they emit 1,535 MB and 2,074 MB. Their Parquet rows cannot be read against each
other even on one CPU. CSV and ndjson are byte-identical between the two.

## 2026-09-17 (json row end) — the fix C wrote down and three ports never took

The profile below ended without a change, on the grounds that it was a map and
not a move. This is the move it pointed at, and it is not new work: C made it
already, and left the argument in a comment.

### What the call counts said

The profile reported C++ spending 38.6% of its instructions in `next_of2`
against C's 13.2%. Counting calls rather than shares says why, and it is not
that the scanner is worse:

| on a 200k ndjson pair | C | C++ |
|---|---:|---:|
| `next_of2` calls | 11.3 M | 56.3 M |
| instructions per call | 34.9 | 31.4 |

Same cost per call, five times as many calls. Two call sites account for it:

| C++ call site | calls |
|---|---:|
| `skip_json_string`, inlined into `parse_json` | 27.5 M |
| **`end_of_json_row`** | 22.4 M |

C's `parse_json_row` has one site and no second: 10.9 M, all of it string
skipping. Its `end_of_json_row` does not call `next_of2` at all.

### The change

C++, Rust and Zig all walked the tail of a row by alternating a scan for `\n`
or `"` with a full walk over every string they landed on -- to avoid mistaking a
newline inside a quoted value for the end of the row. That cannot happen. RFC
8259 forbids the raw control characters U+0000 to U+001F inside a string, and a
newline is U+000A, so a valid JSON string cannot contain one; it must be written
`\n`. The framing of ndjson depends on exactly that. C rewrote the function to a
single scan for one byte on those grounds some time ago. The other three never
took it.

They have it now. It matters more than it sounds because of the early exit: the
key-only parse stops as soon as it has the key columns, so on a twenty-column
row keyed on two, *most of every row* is tail -- and the tail was being walked
string by string.

### What it is worth

Paired and interleaved, arms rotated each round, eleven rounds on a 4M-row pair,
CSV alongside as the control the change should not touch:

| ndjson | base | new | paired median | middle half | faster in |
|---|---:|---:|---|---|---|
| C++ | 5.70s | 4.51s | **0.792** | 0.777-0.805 | 11/11 |
| Rust | 2.60s | 2.52s | **0.964** | 0.946-0.976 | 9/11 |
| Zig | 2.92s | 2.86s | 0.976 | 0.949-1.016 | 7/11 |

| csv (control) | paired median | middle half | faster in |
|---|---|---|---|
| C++ | 1.006 | 0.975-1.010 | 4/11 |
| Rust | 0.995 | 0.986-1.010 | 7/11 |
| Zig | 0.993 | 0.971-1.015 | 6/11 |

C++ is 1.26x on ndjson, and its band does not come near one. Rust's is smaller
but its band clears one too, so it is a result. **Zig's crosses one and nothing
is claimed for it** -- the change is kept there because it is the same function
in four ports and it removes work, not because this host could measure it. Every
CSV band crosses one, which is what a control is for.

### And the profile says the same thing twice

Re-profiled against the *same* command as the baseline:

| C++, 200k pair | base | new |
|---|---:|---:|
| total instructions | 8.254 B | 6.520 B |
| `next_of2` calls | 66.7 M | 22.4 M |
| `next_of1` calls | 16.0 M | 17.1 M |
| out-of-line `skip_json_string` calls | 11.2 M | 0 |

0.790 on instructions against 0.792 measured on the clock. Worth sitting with
next to the entry below, which found instruction counts useless for ranking the
*ports* against each other: within one port, one change, same binary and same
input, they tracked the wall to a thousandth. The currency is fine. It just
does not convert between ports.

Output is byte-identical to each port's own baseline at 4M rows on both formats,
and the C suite's cross-port oracle passes at 89 checks.

## 2026-09-16 (profiles) — where ndjson time goes, and why instruction counts were the wrong thing to count

The entry below closed the scan seam by saying the measurement it wanted was a
profile, and that four experiments had been run without one. This is the profile.

`valgrind --tool=callgrind`, 200k ndjson pair, one thread, instruction counts.
There is no `perf` on this host; callgrind counts instructions rather than
sampling cycles, which turns out to be the point. The binaries are built
`-march=x86-64-v3` because valgrind 3.22 cannot execute the AVX-512 that
`-march=native` emits here -- that matches what the Rust benchmark build already
uses and differs from what C and C++ normally ship, so these are shares within a
v3 build.

### The three ports

| | C | C++ | Rust |
|---|---:|---:|---:|
| total instructions | 2.63 B | 4.59 B | **5.38 B** |
| row parse | `parse_json_row` 27.0% | `parse_json` 34.7% | `RowParser::parse` 25.8% |
| string skip / scan | `next_of2` 13.2%, `next_of1` 12.4% | **`next_of2` 38.6%** | `skip_json_string` 27.6% |
| key name to slot | `parser_slot_for` 22.0% | inlined into `parse_json` | `slot_for` 25.6% |

Two things fall out, and the second undoes the first.

**C++ really does scan more.** 38.6% of its instructions are in `next_of2`,
against C's 13.2% -- 1.77 B instructions against 0.35 B, a factor of five for the
same bytes. C splits the work, leaning on the single-target `next_of1` for 12.4%
where C++ asks `next_of2` for two targets every time. That is a real difference
and a place to look.

**And instruction count does not predict the time.** Rust executes *more*
instructions than either -- 5.38 B against C++'s 4.59 B -- and is the fastest
port on this format by a wide margin: 2.05s against C's 2.79s and C++'s 5.07s at
four million rows. Even allowing that the Rust run renders a report the others do
not (`miniz_oxide::deflate` is 2.9% of it), the ordering does not come close to
reversing.

So the currency is not instructions. It is what those instructions do to the
memory system and the branch predictor, and this host has no `perf` to measure
that with.

### Which is the honest end of the scan seam

Four scan experiments were run against the assumption that fewer steps per field
means less time. That assumption is what the table above refutes: the port
executing the most instructions per row is the one finishing first. The rung paid
in C++ and nowhere else, and the reading in the entry below -- that it pays where
the scan is a big enough share -- survives, because C++'s scan share is 38.6%
where C's is 13.2%. What does not survive is any expectation that shaving
instructions is generally the lever here.

All three ports concentrate in the same three places: parse the row, skip the
strings, turn a key name into a slot. Between a fifth and a quarter of ndjson
work is that last one in the two ports where it is not inlined away, which is
more than the scan costs C. None of that is a change; it is the map the next
change should be chosen from, and it is the first one this project has for any
port but C.

## 2026-09-16 (rust scan) — the rung again, and what three ports say about it

Rust's scan has the same hole the C++ one had, and this time with none of Zig's
excuses. `field.rs` is a real ladder -- `VECTOR_WIDTH` where the build has AVX2
or AVX-512BW, then eight bytes of SWAR, then bytewise -- with nothing at sixteen.
And `build_ports.sh` sets `-C target-cpu=x86-64-v3`, so the thirty-two byte step
is live in every benchmark build: a twenty-two byte ndjson field fails its first
bounds test and falls to three eight-byte steps. That is the case the C++ entry
fixed, in the port that leads ndjson.

A `sse2_hits` beside the existing `vector_hits`, gated on `target_arch` alone
because SSE2 is in the x86-64 baseline, stacked between the wide step and the
word. Thirteen paired rounds each, 4M pairs, alternating who goes first:

| format | base | with the rung | paired ratio | rounds won |
|--------|-----:|--------------:|--------------|-----------:|
| ndjson | 2.10s | 2.08s | 1.000 [0.963-1.009] | 7 of 13 |
| csv    | 1.00s | 1.00s | 1.005 [0.981-1.013] | 6 of 13 |

Nothing. Both bands straddle one, both win counts are coin flips, and CPU
disagrees with itself between the two formats (0.996 and 1.017). Reverted.

### Three ports, one rung, one winner

| port | ladder | sixteen-byte rung | worth |
|------|--------|-------------------|-------|
| C | 32 / 16 / 8 / 1 | had it already | 0.934 on CSV, nothing on ndjson |
| C++ | 32 opt-in / 8 / 1 | **added** | **0.958 on ndjson**, nothing on CSV |
| Zig | one width, `orelse 8` | replaced, not stacked | nothing either format |
| Rust | 32 / 8 / 1 | added, stacked | nothing either format |

The Zig result had an explanation -- a single width replaces the narrow step
instead of standing above it, so short fields fall past to the tail. Rust has no
such excuse and still shows nothing, so that explanation does not cover this.

What is left is how much of each port's run the scan actually is. C++ reads the
same ndjson in 4.8s where Rust reads it in 2.1s, and a rung that removes a fixed
number of steps per field is worth a larger share of the slower run. C's own
profile is the only direct evidence any of these ports has on the question -- 43%
of CSV instructions in `next_of2`, 27% above it in `parse_csv_row` -- and nothing
equivalent has been measured for the other three. That is the measurement this
seam actually wanted, and it was never taken; four scan experiments were run
against an assumption about where the time goes rather than a profile of it.

So the rung is not a portable win, it is a win in the port where the scan is a
big enough share to matter, and which ports those are is unmeasured. Recorded to
close the seam: the next person should profile before adding a rung, not after.

## 2026-09-16 (zig scan) — the same rung, and it does not transfer

The entry below gave the C++ scan a sixteen-byte rung and got 4% on ndjson for
it. Zig's scan reads the same bytes and looks like it has the same hole -- one
width, chosen at build time, `orelse 8`, with 32 and 64 as opt-ins and sixteen
not even offered. So it was tried, and it is not there.

`zig build --release=fast` against `-Dscan=16`, same 4M pairs, eleven paired
rounds each, alternating which binary goes first:

| format | scan 8 | scan 16 | paired ratio | rounds won |
|--------|-------:|--------:|--------------|-----------:|
| ndjson | 2.27s | 2.23s | 1.001 [0.964-1.018] | 5 of 11 |
| csv    | 1.14s | 1.14s | 0.993 [0.867-1.156] | 7 of 11 |

Both bands straddle one and both win counts are coin flips. CPU leans the same
way in both, 0.973 and 0.980, which is the only hint of anything and is not
enough on its own.

**The likely reason is that it is not the same change.** C's ladder and now
C++'s put sixteen bytes *above* an eight-byte step: a field too short for the
wide loop still gets the narrow one. Zig has a single width, so `-Dscan=16`
*replaces* the eight-byte step, and a field shorter than sixteen bytes falls
past it to the byte-at-a-time tail. On ten-byte CSV fields that is the whole
field, which is the shape of the result.

So the untested variant is a ladder rather than a wider single step, and that is
a change to how `scan.zig` is built rather than a flag -- its width is one
constant threaded through the module, deliberately, so "a benchmark of an
instruction set should not be measuring a function pointer". A ladder would be
compile-time nested loops and would not break that, but it is a refactor on the
strength of two CPU medians a couple of per cent apart, and that is not a reason
this file accepts. Recorded so the next person reaching for `-Dscan=16` knows it
was measured and what the measurement missed.

## 2026-09-16 (scanners) — the rung C++ was missing, and it pays where C says it should not

The entry below turned the wide step on by default and measured it slower on
ndjson. That was the wrong end of the ladder. C's scan has three rungs and this
port has two:

| step | C | C++ before | C++ now |
|------|---|------------|---------|
| 32 bytes, AVX2 | from `__AVX2__` | opt-in only | opt-in only |
| **16 bytes, SSE2** | **always on x86-64** | **absent** | **always on x86-64** |
| 8 bytes, SWAR | yes | yes | yes |
| one byte | yes | yes | yes |

With no middle rung a field shorter than the wide step fell from 32 straight to
8, which is why turning the wide step on cost time rather than saving it: the
broadcasts were set up for a loop whose first bounds test failed. SSE2 is not a
question about the target -- x86-64 always has it -- so this rung needs no
`-march=`, no define, and no per-host variation, and anything that is not x86
falls through to the same SWAR loop as before. `next_of1` is left alone, as C
leaves its own: it is not in the profile.

Eleven and fifteen paired rounds, 4M pairs, alternating which binary goes first,
the two differing only in this rung:

| format | 8-byte | with 16-byte | paired ratio | rounds won |
|--------|-------:|-------------:|--------------|-----------:|
| ndjson | 5.01s | **4.82s** | **0.958** [0.949-0.975] | **11 of 11** |
| csv    | 2.06s | 2.07s | 1.009 [0.983-1.025] | 5 of 15 |

CPU agrees: 13.63s to 12.99s on ndjson, and 4.95s to 4.90s on CSV. So ndjson
gains about 4% and CSV is unchanged -- a band that straddles one, on the format
where the first eleven rounds had suggested a 4% loss that fifteen did not
reproduce.

**Which is the opposite of where C finds it.** C claims 0.934 on CSV and nothing
on ndjson; this port gets nothing on CSV and 0.958 on ndjson. Both measurements
stand -- they are different code reading the same bytes -- and the readable
difference is how long a field is. C's note prices its CSV fields at about ten
bytes, where eight needs two steps and sixteen needs one. The ndjson here is
1,780 MB over 4M rows of twenty fields, so about twenty-two bytes a field: three
eight-byte steps against two sixteen-byte ones. Why C++ does not collect the CSV
win that C does is not established here, and the likeliest reading is that this
port's CSV path leans on `next_of2` less than the 43% of instructions C's profile
attributes to it. That is a reading, not a measurement.

Counts identical across all four ports on both formats, 240,031 changed and
4,000 added, and the 36 tests of the C++ suite pass.

## 2026-09-16 (scanners) — a row that measured itself, and a wider step that does not pay

Chasing the ndjson gap of the entry below into the scanner turned up one defect
and one negative result. The defect is real and is fixed here. The wider step is
not, and is not.

### The two ports choose their step differently

```c
/* c/csvdiff.c   */  #if defined(__AVX2__)            /* predefined by -march=native */
// cpp/src/csvdiff.cpp  #if defined(CSVDIFF_SCAN_AVX2)   // set only by the variant builds
```

C selects on the macro the compiler predefines, and its own note says so:
"chosen at compile time from what the build targets... the Makefile builds with
`-march=native`, which defines these where the CPU has them". C++ selects on a
macro only `make scanners` passes. So the ordinary `make` binary -- the one every
table in this file calls "C++" -- compiles the eight-byte SWAR loop and nothing
else, on any machine.

### Which made one row a build measured against itself

`build/csvdiff-swar` was built from the same sources with the same flags and no
define. That is the recipe for `build/csvdiff`. Rebuilt clean from `main`, the
two come out **byte for byte identical** -- same size, same SHA-256. Every matrix
table's `C++ swar` row has been the `C++` row under a second name, which is
visible in the tables once you look: the 2M CSV table reads 1.21s and 925 MB for
both.

The target is gone and so is the row. The plain build takes the eight-byte step,
so it *is* the SWAR baseline that `C++ avx2` and `C++ avx512` are compared
against; a fourth build was never needed to say so.

### And the wider step does not pay here

The obvious fix -- have C++ choose on `__AVX2__` like C -- was built and
measured. 4M pairs, nine rounds, alternating which binary goes first, the two
builds differing only in that define:

| format | SWAR | AVX2 | |
|--------|-----:|-----:|---|
| ndjson | 5.12s | 5.42s | **1.059x slower, faster in 0 of 9** |
| csv    | 2.10s | 2.07s | 0.986x, faster in 5 of 9 |

A clear loss on ndjson and a coin flip on CSV. CPU agrees: 13.91s against 14.76s
on ndjson.

The shape fits the work. These scans stop at the next quote, backslash or
delimiter, and in this data that is ten to forty bytes away -- so a 32-byte step
often cannot run its loop at all, `at + 32 <= end` failing on the first test,
and the broadcasts are set up for a scan the SWAR loop would have finished in one
load. A wide step needs a long way to run.

So the default keeps the eight-byte step and the wide ones stay the explicit
opt-in they already were. Reverted, and recorded here so nobody spends the
afternoon on it again.

**What this does not claim.** C reaches the same decision the other way and its
own entry credits the wider step with a gain. That entry turns out to agree with
this one about ndjson: it measured the wider step at 0.984 there with a middle
half of 0.942-1.057, "crossing one, so nothing is claimed", and puts the reason
as "ndjson's fields are longer, so the scan was already finding hits in one
step". Only the CSV win, 0.934, is claimed for it. So the two ports agree that a
wider step buys nothing on ndjson; what they disagree about is CSV, and the entry
above this one is where that goes.

## 2026-09-16 (ndjson) — where C++'s time goes, and what it is not

The 2026-09-14 entry left C++ at 2.4x Rust on ndjson with the byte proof in
place, and said that whatever is slow there is not the row comparison. This is
the profile it asked for. Local 4 vCPU container (`cascadelake`), 4M ndjson pair,
1,780 MB a side, warm.

### First, the ports are not being asked for the same thing

| port | JSON top-level keys | added rows in the report |
|------|---------------------|-------------------------:|
| C    | `columns`, `counts` | 0 |
| Rust | `columns`, `counts`, `meta` | 0 |
| Zig  | `columns`, `counts` | 0 |
| C++  | `added`, `changed`, `columns`, `counts`, `dup_a`, `dup_b`, `meta`, `removed` | 3 |

The entry below found this between C and C++ on Parquet. It is wider than that:
**C++ is the only port of the four that collects row samples at all**, on any
format. Every cross-port row in this file has been comparing one tool that writes
a report against three that write counts.

Seven rounds, rotating which port goes first:

| port | with `--json` | summary only | CPU, with | CPU, without |
|------|-------------:|-------------:|----------:|-------------:|
| Rust |  2.12s |  2.05s |  6.86s |  6.81s |
| Zig  |  2.23s |  2.27s |  7.77s |  8.03s |
| C    |  2.68s |  2.79s |  7.74s |  7.80s |
| C++  |  6.18s |  5.07s | 19.36s | 13.89s |

Only C++ moves. The report costs it 1.11s of wall and 5.47s of CPU -- more than a
quarter of its CPU (28%) is output the other three do not produce.

**And the gap survives it.** 2.92x Rust with the report, **2.47x without**. On
Parquet about two thirds of the reported difference turned out to be output; here
it is about a sixth. The 2.4x is real.

### Where it is

Warm, four threads, the phases each port marks:

| phase | C | Rust | Zig | C++ |
|-------|--:|-----:|----:|----:|
| A sweep | 0.543s | 0.865s | 1.303s | **2.282s** |
| index insert (A) | 0.112s | 0.182s | 0.209s | 0.762s |
| join and compare | 0.879s | 0.822s | 0.670s | **3.056s** |

Two gaps, not one, and the larger in absolute terms is the join.

### What it is not

**Not threading.** The sweep at one thread against four:

| port | 1 thread | 4 threads |
|------|---------:|----------:|
| C    | 0.536s | 0.554s |
| Rust | 1.651s | 0.831s |
| C++  | 2.097s | 2.279s |

C++'s sweep does not scale -- and neither does C's, which is four times faster in
absolute terms. `per_file` is `budget / 2`, so one thread a side becomes two, and
that doubling buys nothing in either port. What is left is per-byte cost: C++
sweeps 1,780 MB at about 0.85 GB/s where C does it at 3.3 GB/s.

The join does scale, and still trails: C 3.063s to 0.917s (3.3x), Rust 2.765s to
0.732s (3.8x), C++ 5.994s to 2.118s (2.8x). C++ is roughly twice C's cost per
core there as well.

**Not work thrown away.** The columnar path had a pass it ran and discarded, and
that was worth 1.10x when gated. There is no equivalent here: `csvdiff.cpp`
already gates B's pass on `row_lists`, and the per-partition `changed` and
`removed` lists are capped at `max_rows` where they are filled, so nothing
unbounded is collected for a report nobody asked for. This was checked before
looking for anything cleverer, because it is the shape that paid twice on
Parquet.

So the remaining gap is the cost of parsing an ndjson row and of comparing two,
in both phases, at roughly 2x to 4x the sibling port written in C. That is a
question about the JSON field parser rather than about structure, and it is
recorded here rather than guessed at.

These rows are one host and one sitting. They do not compare with the 4M table of
2026-09-14, which ran elsewhere -- C++ reads 4.35s there and 5.07s here for the
same work, which is the hardware and not a regression.

## 2026-09-16 (C++ columnar) — what two changes were worth, and a ranking this host cannot give

Two changes landed in the C++ Parquet path: the prefetch of #77 and the derived
`added` of #79. Measured together against the tree before either, on one host in
one sitting, 8M Parquet pair, eleven paired rounds, alternating which binary goes
first.

| | before | after | wall | CPU |
|---|------:|------:|-----:|----:|
| with `--json` (report wanted) | 2.50s | **1.99s** | **0.796x** | 0.767x |
| no `--json` (summary only)    | 2.40s | **1.69s** | **0.703x** | 0.652x |

Faster in 11 of 11 rounds in both modes. The two figures differ because the two
changes have different reach: the prefetch helps every run, and deriving `added`
only helps the runs that were not going to print the sample.

The parts add up, which is the check worth having. 0.796x is 1.26x, which is what
the prefetch measured on its own; 1.26x against the derived pass's own 1.10x is
1.39x, against 1.42x measured for the pair. Nothing here is the sum of two
numbers taken at different times -- the cumulative row is its own measurement and
agrees with the parts.

### `compared columns` is not the scaling problem it looked like

An earlier reading of this phase had C scaling 4.2x against C++'s 3.0x. That came
from two single runs taken under different conditions and does not survive being
measured properly:

| threads | C | C++ | ratio |
|--------:|--:|----:|------:|
| 1 | 1.708s | 2.075s | 1.21x |
| 2 | 0.882s | 1.157s | 1.31x |
| 4 | 0.532s | 0.655s | 1.23x |

C scales 3.21x over four cores and C++ 3.17x. The gap is flat at about 1.2x
whatever the thread count, so it is per-core efficiency on structurally identical
code -- both ports block the same way, take the same dictionary fast path, and
run the same number of lanes. That is micro-optimisation with an uncertain
payoff, not the structural win the join turned out to be, and it is left alone.

### The ranking against C is host-dependent, so it is not recorded

Two sittings, the same two sources, opposite answers:

| host | C++ / C, `--json` | C++ / C, summary |
|------|------------------:|-----------------:|
| `emeraldrapids` (2026-09-15) | 1.34x | 1.12x |
| `cascadelake` (2026-09-16)   | **0.80x** | **0.72x** |

Both are same-sitting, interleaved, alternating-order measurements, and neither
is wrong. The container moved between them: `rustc --print target-cpus` resolved
`emeraldrapids` yesterday and `cascadelake` today, on a machine eight minutes old.
Both ports compile with `-march=native`, so the hardware changing changed the
binaries too, and two variables moved at once.

It is not the huge-page term from the entry below, which was the first suspicion:
C is granted its 262,144 kB of `AnonHugePages` on every run today, on a host with
14 GB free and nothing to compact.

So this file gets no "C++ has overtaken C on Parquet" row, because one host says
it has and another says it has not. What can be said is narrower and holds: on
the host that ran them, the C++ columnar path costs 0.70 to 0.80 of what it cost
before these two changes, and the ports are now close enough on Parquet that
which one leads depends on the machine.

## 2026-09-15 (method) — the C and C++ rows are not answering the same question

Every table here that puts C against C++ compares a run that emits counts with a
run that emits counts *and* a sample of the rows behind them. The harness asks
both for `--json` and checks that the counts agree, which they do. What it
cannot see is that producing that JSON is a different amount of work in the two
ports.

| port | JSON top-level keys |
|------|---------------------|
| C    | `columns`, `counts` |
| C++  | `added`, `changed`, `columns`, `counts`, `dup_a`, `dup_b`, `meta`, `removed` |

C's columnar path derives `added` as a number and never collects a row for it --
`b_ways` is zero unless `CSVDIFF_VERIFY_ADDED` is set. It does not collect
`removed`, `changed`, or the duplicate lists either, on Parquet or on CSV. Asked
for a 50k pair, C reports 0 added rows and 0 removed rows beside its exact
`counts.added` of 50; C++ reports three of each.

### What the gap is when both are asked for the same output

8M Parquet pair, eleven rounds, alternating which port goes first.

| | C median | C++ median | C++ / C |
|---|---:|---:|---:|
| summary only, no `--json` | 1.96s | 2.21s | **1.12x** |
| with `--json` | 1.85s | 2.48s | 1.34x |

**C costs the same either way** -- 1.96s against 1.85s, the difference inside
this host's noise -- because it does not collect samples in either mode. C++
goes 2.21s to 2.48s, which is the collection. So the 1.34x that a table reports
is 1.12x of engine and the rest of a report C is not writing.

CPU says it more plainly: C 4.89s and 4.46s, unchanged by the flag; C++ 6.17s
and 7.44s.

### What to do about it

Nothing in the ports. The C++ report is a feature and the C port not having one
is a choice, and neither is a bug to be fixed by making the other match. What is
wrong is reading a single ratio as "how much faster is C at comparing files",
because at the sizes these tables cover about two thirds of the reported
difference on Parquet is output.

The honest comparisons are the two rows above, and they answer different
questions: 1.12x is the engines, 1.34x is the tools. A future table that wants
the first should pass no `--json` and read the summary line, which every port
prints and which costs the same to produce in all four.

One more thing is visible in the same run and belongs to the entry below rather
than this one: C's median with `--json` is 1.85s, and its median over the rounds
where it *ran first* is 1.32s. That is the huge-page compaction effect, not the
report.

## 2026-09-15 (measurement) — the C port's time depends on what ran before it

`alloc_huge` is worth more than its own note claims and costs more than anyone
has been charging it. On this host the C port runs a 8M Parquet pair in 1.34s or
in 2.10s, on one binary, decided by nothing but whether another large process
ran immediately before it.

Found while porting `alloc_huge` to C++ -- a change that is **not** in this
commit and should not be made without reading what follows.

### The signature

Two binaries, nothing else running, alternating which goes first each round.
`cpp` here is the C++ port without huge pages, as a reference that does not use
them; `C` is `c/csvdiff` as it ships.

| round | order | cpp wall | C wall | C cpu |
|------:|-------|---------:|-------:|------:|
| 1 | cpp, C | 2.24s | 1.98s | 4.97s |
| 2 | C, cpp | 2.42s | **1.30s** | **3.65s** |
| 3 | cpp, C | 2.50s | 2.03s | 5.11s |
| 4 | C, cpp | 2.40s | **1.33s** | **3.71s** |
| 5 | cpp, C | 2.36s | 2.13s | 5.29s |
| 6 | C, cpp | 2.41s | **1.34s** | **3.79s** |
| 7 | cpp, C | 2.37s | 2.10s | 5.26s |
| 8 | C, cpp | 2.41s | **1.45s** | **3.96s** |
| 9 | cpp, C | 2.46s | 2.23s | 5.56s |
| 10 | C, cpp | 2.50s | **1.38s** | **3.89s** |
| 11 | cpp, C | 2.37s | 2.12s | 5.27s |

C first: 1.30-1.45s. C second: 1.98-2.23s. **1.57x on the medians**, with no
overlap between the two groups at all. The reference column has no such split --
2.24 to 2.50 with the parity of the round making no difference to it.

### Why

This host reports `madvise` for both `/sys/kernel/mm/transparent_hugepage/enabled`
and `.../defrag`. The second one is the part that matters: under `defrag=madvise`
a range that asks for `MADV_HUGEPAGE` gets **direct compaction** -- the kernel
assembles 2 MB pages while the calling thread waits. How long that takes depends
on how fragmented memory is at that instant, and a 2.4 GB process that exited a
moment ago is what fragments it.

So the cost is not a property of the run. It is a property of what the machine
was doing beforehand.

**And it is charged as the process's own CPU.** 3.65-3.96s when C goes first,
4.97-5.56s when it goes second, for identical work. This file says above that
"CPU is the honest measure of work" because wall mixes work with how well a
design spreads it. That holds against threading and does not hold against this:
the extra seconds are the kernel compacting memory on the process's behalf, and
they land in the process's system time either way.

### What it means for the tables

Rotation spreads this rather than removing it. With four builds each is first
once every four rounds, so a port using huge pages is measured mostly in its
contended state, and its `Best` column is the one round where it was not. That
is a reading of C's own spread rather than a claim about a particular table --
in three runs of the same 8M pair today C's median came out 2.06s, 1.35s and
2.12s, and its 50M rung moved 16.60s to 8.65s on unchanged code.

A user running `csvdiff` once on a quiet machine gets the 1.34s behaviour. A
ladder running four ports back to back mostly gets the 2.10s one. Both are real;
they are answers to different questions, and this file has been reporting one of
them while sounding like the other.

### The C++ change this came from is not being made

Porting `alloc_huge` to the C++ columnar index reproduces all of it: 2.03-2.55s
when the process goes first, beating the same build without huge pages at its
best of 2.17s, and 2.73-3.10s when it goes second. Median over mixed positions
is **1.16x slower**, so on the evidence available it does not pay here.

The case for it is not dead -- it wins outright on a quiet machine, and the two
ports would then differ in language rather than in allocator. But it is a change
whose sign depends on the environment, and that is a decision to take
deliberately rather than to inherit from a benchmark run on one container. The
patch is not in this commit.

**Decided, 2026-09-16: not shipped.** The win is real and so is the cost, and the
cost is paid where this project does its measuring. Benchmarks run the ports back
to back, which is the contended case, so the 1.16x would land on every table C++
appears in -- and it would give C++ the same position-dependent term this entry
documents in C, where an identical binary reads 1.34s or 2.10s depending on what
ran before it. One port carrying that is a finding; two is a measurement problem.
A user comparing two files once on a quiet machine is the case that gains, and
that case is not what these tables measure. Reopening it would want a host where
`defrag` is not `madvise`, or a threshold measured rather than guessed.

## 2026-09-15 (week over week) — 20M of Parquet, against the tree from seven days ago

There was no 20M baseline to compare against: `BENCHMARKS.md` at `20aa3fe`
(2026-09-08 23:43, the last commit of that day) records 2M and 10M and nothing
else. So the baseline is built here rather than quoted — the week-ago tree
checked out beside this one, built on this host, and run against the same pair
in the same sitting.

Local 4 vCPU / 16 GB container, one uncompressed 20M Parquet pair (3,069 MB for
the two files), five interleaved rounds each, `--repeats 5` through
`scripts/bench_ports.py`. `wk` rows are `20aa3fe`.

| Port   |   Best | Median |  Worst |   CPU | Peak RSS | Above the input |
| ------ | -----: | -----: | -----: | ----: | -------: | --------------: |
| C      |  4.70s |  4.82s |  5.66s | 11.8s | 5,896 MB |        2,827 MB |
| C++    |  7.55s |  7.87s |  8.46s | 24.0s | 5,771 MB |        2,701 MB |
| Rust   |  6.03s |  6.31s |  6.53s | 19.2s | 5,719 MB |        2,650 MB |
| Zig    |  5.69s |  5.86s |  6.13s | 19.5s | 5,653 MB |        2,584 MB |
| wk C   |  4.44s |  4.96s |  5.58s | 12.3s | 5,912 MB |        2,843 MB |
| wk C++ |  7.44s |  7.55s |  8.38s | 23.3s | 5,867 MB |        2,797 MB |
| wk Zig | 19.14s | 19.90s | 20.34s | 26.4s | 5,535 MB |        2,465 MB |

`counts: identical everywhere`, across all seven builds — a week-old binary and
today's agree to the row on 20,002,000 of them.

### Read the two controls first

The 50M entry below is the reason this table has controls at all: C's Parquet
path went 16.60s to 8.65s there on *identical code*, because the two runs were
different sittings. Here both trees run in one sitting, and two of the three
pairs are controls:

| Port | week ago → today | ranges |
|------|------------------|--------|
| C   | 4.96s → 4.82s, **-3%** | 4.44-5.58 against 4.70-5.66, overlapping |
| C++ | 7.55s → 7.87s, **+4%** | 7.44-8.38 against 7.55-8.46, overlapping |

No C++ commit this week touches the columnar Parquet path -- its changes are the
ndjson byte proof and the text-path phase timings. C's Parquet commits are the
memory-ceiling work and the snappy/LZ4 codecs, and this pair is uncompressed, so
the codec path is not entered. Both move a few per cent, in *opposite*
directions, with ranges that overlap. That is the sitting holding still, and it
is what licenses the row below.

### Zig: 19.90s to 5.86s

**3.4x**, and the ranges do not come close to touching: 19.14-20.34 against
5.69-6.13. Nothing about this one is subtle.

CPU tells you what kind of change it was. Wall fell 3.4x; CPU fell from 26.4s to
19.5s, which is 26%. A change that did 3.4x less work would have taken CPU down
with it. This one mostly stopped waiting: cores-busy goes from about 1.3x to
about 3.3x on a four-core box. (The harness reports the minimum CPU of the five
runs, so these ratios divide it by the median wall; against the best wall they
read 1.4x and 3.4x, which is the same story.)

**Not bisected.** Two commits from 2026-09-09 fit the shape -- `4949283` "derive
`added` instead of walking B for it", which removes a pass, and `ed276df` "the
same, where the counting was not even threaded", which is a serial phase becoming
parallel and is the one that matches a wall-without-CPU gain. `1f2b070` (#73)
lands in the same window and is worth 12% of CPU on CSV. Which of the three owns
how much is a question this table does not answer, and the attribution is offered
as inspection rather than measurement.

### The Rust row has no partner, and why

The week-ago Rust port cannot be built in this container. On 2026-09-08 `duckdb`
(bundled) and `polars` were unconditional dependencies -- `rust/Cargo.toml` had no
`[features]` section at all, so `--no-default-features` removes neither -- and that
file's own comment puts a debug build of the pair at tens of gigabytes. There
were 7.5 GB free. Today's Rust row is recorded for the standings, not as half of
a comparison.

For the same reason of size, this is Parquet and not CSV: a 20M CSV pair is about
7 GB against 3.1 GB for Parquet, and the CSV pair does not fit beside two trees'
builds.

### Where 20M of Parquet stands now

| Port | Median |   CPU | Cores (CPU over median) |
|------|-------:|------:|------------------------:|
| C    | 4.82s  | 11.8s |                    2.4x |
| Zig  | 5.86s  | 19.5s |                    3.3x |
| Rust | 6.31s  | 19.2s |                    3.0x |
| C++  | 7.87s  | 24.0s |                    3.0x |

C leads on wall with by far the least CPU -- 11.8s against everyone else's 19-24s
-- while spreading it the least. Zig passed Rust and C++ this week. C++ is last
and is the only port here whose Parquet path nobody has touched.

One sitting, one host, one pair. The ratios within the table are what it
supports; none of these numbers compares with a hosted-runner row elsewhere in
this file.

## 2026-09-14 (scale) — the 50M rung, with repeats, and the 1.04x is gone

Dispatched to settle the one number three rounds of work rested on: Rust's 1.04x
cores at 50M of Parquet, recorded at `--repeats 1`. Same rung, `--repeats 3`,
`--first Rust`, no matrix, 13,941 MB cap.

| Build | Compare |    Rows/s |   CPU | Cores |  Peak RSS | Above the input |   Budget |
|-------|--------:|----------:|------:|------:|----------:|----------------:|---------:|
| C     |   8.65s | 5,779,353 | 25.2s | 2.92x | 14,578 MB |        6,905 MB | 7,972 MB |
| C++   |  13.83s | 3,616,663 | 44.4s | 3.21x | 14,607 MB |        6,934 MB | 7,941 MB |
| Rust  |  11.97s | 4,178,617 | 40.6s | **3.39x** | 14,842 MB |    7,169 MB | 7,916 MB |
| Zig   |  12.56s | 3,982,747 | 44.2s | 3.52x | 14,337 MB |        6,663 MB | **9,217 MB** |

`counts: identical everywhere`. Beside the run it replaces:

| Build | then (repeats 1) | now (repeats 3) |
|-------|-----------------:|----------------:|
| Zig   | 12.13s, 3.05x | 12.56s, 3.52x |
| C     | 16.60s, 1.42x | **8.65s, 2.92x** |
| C++   | 19.08s, 2.19x | 13.83s, 3.21x |
| Rust  | 29.70s, 1.04x | **11.97s, 3.39x** |

**Read the C row first.** C's Parquet path has not changed between the two runs,
and it went 16.60s to 8.65s: 1.9x, on identical code. Three of the four rows
improved by more than any change that landed in between could explain. The
previous table was a slow sitting, and the Rust row was the extreme of it rather
than a separate phenomenon.

Rust's reader did gain the key-column fan-out in between, worth 1.08x wall on an
8M pair locally. That is not 29.70s becoming 11.97s, and it does not touch the C
row at all.

So **README item 2 is withdrawn**, and with it the question it posed. Nothing
about how the Rust reader walks its mapping needs explaining, because it does not
wait: at 3.39x it is the second most parallel of the four.

What the investigation produced is worth more than the claim that started it, and
all of it stands on its own: the columnar path had no phase timings and now has
them; its key read was two jobs whatever the key and is now a queue; `--threads`
did nothing on that path and now does; and the memory cap turned out to bound a
quantity the table was not reporting. A wrong number can still be a useful thing
to have chased. It is a cheaper one to check first.

### The third time

This file has now withdrawn three claims for the same reason: the Rust B-side
index insert asymmetry, the C++ sweep at 2.3x, and this. Each was one observation
on a shared runner, each survived long enough to be built on, and each dissolved
on re-measurement. `--repeats` costs minutes.

### One row that is about a port

Zig's budget is **9,217 MB** against the other three at about 7,940, while it
holds the *least* above the input at 6,663 MB. That is the `VmData` gap the
memory entry two below found, reproducing at 50M on Parquet, on a different host
and a different format from where it was found -- and it is the only number in this table that is
about a port rather than about a runner. The `Budget` column earned its place on
its first ladder rung.

## 2026-09-14 (zig) — the index reserved four lists it should have let grow

The previous entry found Zig reserving 61% more `VmData` than it touches, in a
236 MB transient at the sweep-to-insert boundary, and named that as why it is the
first port to refuse 100M rows of CSV. This is the change, and it is not the one
that entry predicted.

### Two candidates, and the first was wrong

The sweep's chunks grow by `append` in the text branch, with no reservation,
because the row count is not known in advance -- where the columnar branch beside
it asks for `hi - lo` once. Doublings hold the old buffer while they fill the new
one, which is exactly the shape `RLIMIT_DATA` punishes, so this looked like the
answer.

Reserving them from the first row's length (chunk bytes over that length, plus an
eighth) made it **worse**: 955 MB against 955 MB baseline, three interleaved
rounds each, 997 every time. The slack is reserved and the doublings were never
the peak -- the trace already said so, climbing smoothly through the sweep and
stepping only at the insert. Reverted, and recorded here so nobody spends the
afternoon on it again.

It also ruled out the allocator: `--threads 1` peaks at 888 MB against four
threads' 955, so per-thread arenas are not what this is.

### What it is: four lists reserved beside the chunks they copy from

`RowIndex.init` reserved `row_at` (u64), `row_hash` (u64), `first_row` (i32) and
`occurrences` (u32) to `total` before the loop that inserts the chunks and frees
them one at a time. Peak `VmData` on a 6M CSV pair, interleaved, three rounds:

| reserved | peak VmData |
|----------|------------:|
| all four (baseline) | 955, 955, 955 MB |
| row_at + row_hash only | 885, 869, 885 MB |
| first_row + occurrences only | 790, 815, 804 MB |
| **none** | **752, 735, 761 MB** |

Monotonic, and additive within noise: the two row lists cost about 150 MB and the
two key lists about 75, which is the 16 and 8 bytes a row they hold.

**The sizing above them is measured and stays.** The table is sized once because a
rehash is a full random-access pass over something too big to cache, twelve of
them at ten million rows. These four reservations were added from that argument by
analogy, and the analogy does not hold: growing an `ArrayList` is a linear copy,
not a random-access pass -- and the copies happen *while the chunks are being
freed*, so the lists grow into pages the loop has just given back instead of
reserving fresh ones beside them.

### Which is faster as well as narrower

`scripts/bench_ab.sh`, 6M CSV pair, paired and interleaved:

| against | rounds | wall [mid half] | CPU [mid half] |
|---------|-------:|----------------:|---------------:|
| all four reserved | 9 | **1.22x** 1.18-1.24 | **1.12x** 1.06-1.16 |
| row lists still reserved | 7 | 1.18x 1.15-1.30 | 1.20x 1.11-1.23 |

12% less CPU than the baseline, and a further 17% less than the half-measure, so
dropping all four is both the simplest version and the best one. 40 of the Zig
port's own tests pass and the counts match C, C++ and Rust on the same 6M pair.

A reservation that costs time as well as address space is an unusual result and
worth stating plainly: the memory it hands back is memory the insert loop was
about to reuse, and taking it fresh instead is what the extra 12% was buying.

### Confirmed by CI, on a runner, at a third of the rows

The `bench-2m.yml` run on the pull request, against the one on the pull request
before it -- 2M rows of CSV, same workflow, same size, the change the only
difference:

| Build | before: above / budget / wall | after: above / budget / wall |
|-------|------------------------------:|-----------------------------:|
| Zig     | 206 MB / 290 MB / 0.51s | **186 MB / 243 MB / 0.41s** |
| Zig v32 | 210 MB / 300 MB / 0.45s | **201 MB / 243 MB / 0.36s** |

Budget down 16%, wall down 20%, on a hosted runner at a third of the rows the
local measurement used. Both halves of the claim reproduce somewhere other than
where they were found, which is the bar this file asks of a number before it
counts -- and the two rows are the same change seen through two scanner builds.

The other ports are unchanged in that table, as they should be: C++ 224/254,
Rust 301/304, both within a megabyte of where they were.

## 2026-09-14 (memory) — the cap measures a quantity the table did not report

The previous entry left Zig failing the 100M CSV rung while C, C++ and Rust
finished, and read it as "Zig holds more per row". **It holds less than Rust.**
The ceiling is decided by something the table was not printing.

`--memory-cap` sets `RLIMIT_DATA`, and that limit is checked against `mm->data_vm`
-- the *virtual* size of the private writable mappings: heap, anonymous mmap,
thread stacks. Peak RSS is a different number in two directions at once. It counts
resident pages of the mapped **input**, which the limit does not bound; and it
does not count a mapping that is reserved and never touched, which the limit does.
So a port that holds an old buffer while it fills a new one is charged for both by
the cap and for neither by the column beside it.

### Measured, 6M rows of CSV, local 4 vCPU container, input 2,105 MB

`VmData` sampled from /proc at 5ms; the cap column is a sweep, coarse to 100 MB.

| Port | peak VmData | peak RSS | above the input | VmData / above | refused below |
|------|------------:|---------:|----------------:|---------------:|--------------:|
| C    |   448 MB | 2,510 MB | 405 MB | 1.11x |   500 MB |
| C++  |   747 MB | 2,616 MB | 511 MB | 1.46x |   700 MB |
| Rust |   748 MB | 2,802 MB | 697 MB | 1.07x |   700 MB |
| Zig  | **955 MB** | 2,697 MB | 592 MB | **1.61x** | **1,000 MB** |

**The refusal threshold tracks `VmData`, not RSS**, in all four. Rust holds the
most and needs the second smallest budget; Zig holds the second least and needs
the largest. Reserving 61% more than it touches is what puts Zig first into the
wall, and none of it is visible in the column the ladder printed.

### Which predicts the whole ceiling table, from one 6M run

Scaling `VmData` linearly and comparing against the ladder's 13,941 MB cap:

| rows | C | C++ | Rust | Zig |
|-----:|--:|----:|-----:|----:|
| 100M (x16.7) |  7,467 MB ✓ | 12,450 MB ✓ | 12,467 MB ✓ | **15,917 MB ✗** |
| 150M (x25)   | 11,200 MB ✓ | 18,675 MB ✗ | 18,700 MB ✗ | 23,875 MB ✗ |

Eight cells, and the ladder agrees with all eight: three of four at 100M with Zig
the one that fails, C alone at 150M. Read the yes/no and not the megabytes -- the
hash table rounds to a power of two, so the true curve is a staircase around that
line, and C++ and Rust land inside 11% of the cap at 100M, which is closer than
this arithmetic deserves to be trusted for. What it does establish is that the
ceilings are not mysterious: they are `VmData` against the cap, and one local run
puts every port on the right side of it.

### Where Zig's extra 363 MB goes

`VmData` sampled every 5ms through one run, as the max in each 5% of it:

```
   0.00s      18 MB
   0.26s      96 MB ####
   0.53s     203 MB ########
   0.79s     260 MB ##########
   0.88s     409 MB #################
   0.97s     749 MB ###############################
   1.05s     955 MB ########################################
   1.14s     955 MB ########################################
   1.23s     821 MB ##################################
   1.32s     719 MB ##############################
   1.67s     719 MB ##############################
```

A **spike of 236 MB that is given back**: it climbs to 260 MB through the sweep,
tops out at 955, and settles at 719 for the rest of the run. Rust's trace over the
same pair has no comparable step -- it rises to 666 MB, falls to 539, and reaches
its 700 MB peak at the end, in the report.

The spike sits at the sweep-to-insert boundary, which the phase marks put at
1.09s and 1.10s for the two sides. A reading of `RowIndex.init` in
`zig/src/csvdiff.zig` that fits the size: it reserves `row_at` (u64), `row_hash`
(u64), `first_row` (i32) and `occurrences` (u32) -- 24 bytes per row per side --
and does so *before* the loop that consumes the sweep's chunks, which hold an
address and a hash each, 16 bytes per row per side. At 6M that is 288 MB of index
arrays reserved while 192 MB of chunks is still held, against a measured 236 MB.
The code already anticipates the shape of this in a comment on that loop --
*"Each chunk is released as soon as it has been inserted. Holding all of them to
the end would keep two copies of every row's address and hash alive at once"* --
and releasing them one at a time bounds the overlap without removing it.

That is a reading consistent with the number, not a proven decomposition, and
narrowing it is its own change: the chunks already hold exactly what two of those
four arrays want, so the copy and the transient could both go if the index adopted
the chunk memory instead of reserving beside it.

### The harness prints it now

`scripts/bench_formats_ports.py` gains a **Budget** column: peak `VmData`, sampled
on the 50ms poll the timeout already runs, so it costs one open and one read per
tick and no extra wakeups. Without it the table could not explain its own
refusals -- every ladder rung that came back with "out of memory" was reporting
the one memory figure that does not decide it.

Polled and not from rusage, which has no equivalent, so it understates a short
spike: three 50ms passes over that Zig run read 955, 888 and 888 against the 5ms
poll's 955, an undershoot of up to 7% on a run lasting 1.6s. The rungs this column
exists for run for minutes, where a phase boundary is sampled many times.

### Confirmed on a different host, at a different size

The first `bench-2m.yml` run carrying the column, 2M rows of CSV on a hosted
runner -- another machine, a third of the rows, and the two columns side by side:

| Build | above the input | budget | ratio |
|-------|----------------:|-------:|------:|
| C     | 126 MB | 176 MB | 1.40x |
| C++   | 224 MB | 254 MB | 1.13x |
| Rust  | 301 MB | 304 MB | **1.01x** |
| Zig   | 206 MB | 290 MB | **1.41x** |

Same ordering as the local 6M run: Rust holds the most and reserves almost nothing
beyond it, Zig holds less and reserves half as much again. The ratios are not the
same as the 6M ones and should not be -- the fixed costs are a larger share at 2M,
and these runs last 0.4-0.5s, which is the regime where the note above says the
poll undershoots. `Zig v64` reading 260 MB against `Zig` and `Zig v32` at 290 and
300, for three builds that differ only in scanner width, is that undershoot on
display. At 0.5s the column is indicative; at a ladder rung it is not.

## 2026-09-14 (parallelism) — the Rust Parquet key read was two jobs whatever the key

Written down here because the change landed in #69 without an entry, and the
numbers belong in this file rather than only in a commit message.

The columnar Parquet reader read every key column of A on one thread and every
key column of B on the caller's. Two jobs however wide the key is, for a phase
that decodes pages for every row in the file, in a tool whose subject is a
*composite* key. Zig's reader has fanned this out to `2 * key_size` since its
columnar path was threaded.

Finding it needed instrumentation the path did not have: `Phases` lived inside
`engine/turbo.rs`, so the one path in any of the four ports that could not be
asked where its time went was this one. It is `rust/src/phases.rs` now and the
Parquet reader marks the same six phases C++ and Zig mark. 8M rows,
`account_id,txn_id`, local 4 vCPU container, warm:

```
  key columns (2 ways)         0.637s
  intern dicts (serial)        0.000s
  index build (2 ways)         0.468s
  match sweep (par)            0.514s
  compared columns (par)       2.563s
  assemble                     0.155s
```

`intern dicts` is zero because neither key column is dictionary-coded at this
width — the generator gives up on a dictionary past its per-row-group limit — so
the shared-id path is skipped outright.

### The phase, five interleaved rounds of each build

| build | key columns phase |
|-------|-------------------|
| two jobs | 0.726s 0.623s 0.636s 0.662s 0.733s |
| queue    | 0.378s 0.348s 0.354s 0.360s 0.345s |

Non-overlapping bands, about **1.9x**, which is what 2 jobs going to 4 on four
cores should be. A three-column key — six jobs over four lanes, so the lanes go
round the queue more than once — gives 0.678-0.818s against 0.347-0.371s, about
2.0x.

### The whole run, 11 paired rounds through `scripts/bench_ab.sh`

| | best | median | paired ratio [mid half] |
|---|-----:|-------:|------------------------:|
| wall, two jobs | 3.873s | 3.926s | |
| wall, queue    | 3.570s | 3.622s | **1.08x** [1.06-1.09] |
| CPU, two jobs  | 10.77s | 10.94s | |
| CPU, queue     | 10.57s | 10.79s | 1.01x [0.99-1.02] |

Cores-busy 2.79x to 2.97x over five interleaved runs each; peak RSS unchanged at
about 3.2 GB, since both sides' key columns were already in flight together.

**`bench_ab.sh` calls this "no result", and it is right to.** It judges on CPU,
deliberately, because wall mixes work with how well a design spreads it. CPU is
exactly what this change does not move: the same work, on more cores. The wall
column, whose middle half is 1.06-1.09 and does not straddle 1.00, is the one to
read, and the phase table above is the direct measurement. A harness built to
answer "did this do less work" cannot answer "did this spread it better", and
saying so is cheaper than rebuilding it.

### `--threads` did nothing at all on this path

The columnar reader asked `available_parallelism()` directly. Same 8M pair:

| | before | after |
|---|-------:|------:|
| `--threads 1` | 2.74x cores | **1.19x** |
| `--threads 2` | 2.81x cores | **2.02x** |

The budget is `Options::thread_budget()` now, read by both engines. The residual
0.19x at one thread is the report's gzip lanes, which still ask the machine
because `render` is handed a result and not the options that produced it.

Worth carrying to any table that used the flag: `bench-contention.yml` passes
`--threads`, so its Rust Parquet rows were never actually constrained.

### And the hypothesis it killed

`parallel::spawn` degrades to running the work inline when the OS refuses a
thread, and under `RLIMIT_DATA` a refused thread is exactly what a tight cap
produces — a stack is a private anonymous mapping. That is a clean explanation
for the 50M rung's 1.04x, and it is wrong. The same 8M pair with the cap walked
down to the refusal point:

| cap | wall | cores | outcome |
|-----|-----:|------:|---------|
| none     | 6.41s | 2.77x | ok |
| 4,096 MB | 4.89s | 2.47x | ok |
| 3,000 MB | 5.66s | 2.41x | ok |
| 2,800 MB | 4.87s | 2.59x | ok |
| 2,700 MB | 4.41s | 2.74x | ok |
| 2,600 MB | 3.63s | 2.75x | refused: `one handle per row needs 61 MB` |
| 2,500 MB | 4.14s | 2.55x | refused: `the uncompressed column needs 99 MB` |

Cores hold to the edge and then it refuses cleanly. The cap does not serialise
this reader.

## 2026-09-14 (scale) — the CSV ceiling is not one number, it is four

Two rungs, one job each, `100m` and `150m` of CSV, `--repeats 1`, `--matrix`,
`--first Rust`, on separate 16 GB runners with each port capped at **13,941 MB**
through `RLIMIT_DATA`.

README item 3 said "CSV finishes at 150M and dies at 200M". That is a claim about
the fleet, and the fleet does not behave that way. **Each port has its own
ceiling, and they are two rungs apart.**

### 100M — three of four, and the one that fails is the one that wins at Parquet

Input 35,088 MB, generated in 131.9s.

| Build       | Compare |  Rows/s |    CPU | Cores |  Peak RSS |
|-------------|--------:|--------:|-------:|------:|----------:|
| C           | 214.75s | 465,703 | 110.6s | 0.51x | 13,439 MB |
| C++         | 246.09s | 406,401 | 219.2s | 0.89x | 13,415 MB |
| Rust        | 239.54s | 417,514 | 106.9s | 0.45x | 13,507 MB |
| Zig         |       — |       — |      — |     — | refused: `out of memory` |
| Rust engine | 216.34s | 462,279 | 106.2s | 0.49x | 13,450 MB |
| C++ swar    | 200.88s | 497,867 | 222.6s | 1.11x | 15,025 MB |
| C++ avx2    | 205.64s | 486,331 | 209.5s | 1.02x | 15,016 MB |
| Rust avx2   | 240.31s | 416,167 | 121.2s | 0.50x | 13,458 MB |
| Zig v32     |       — |       — |      — |     — | refused: `out of memory` |

`counts: identical everywhere` across the seven that finished.

**Zig is the first to run out, and that is the interesting part.** At 50M of
Parquet Zig is the fastest of the four and the most parallel, at 3.05x cores. On
CSV it is the only one of the four that cannot do 100M at all. Whatever its text
path holds per row, it holds more of it than the other three.

### 150M — C, alone

Input 52,632 MB, generated in 197.7s.

| Build | Compare |  Rows/s |    CPU | Cores |  Peak RSS |
|-------|--------:|--------:|-------:|------:|----------:|
| C     | 335.07s | 447,715 | 165.9s | 0.50x | 13,357 MB |

Everything else refused, each naming what it could not fit:

| Build | Refusal |
|-------|---------|
| Rust | `out of memory: one hash per row needs 1144 MB` |
| Rust engine | `out of memory: the key index needs 2048 MB` |
| Rust avx2 | `out of memory: one offset per row needs 1144 MB` |
| C++, C++ swar, C++ avx2 | `std::bad_alloc` |
| Zig, Zig v32 | `out of memory` |

1144 MB is 150,015,000 × 8 bytes, so those two Rust messages are the same
structure size under two names — which of the per-row arrays reaches the cap
first is not a property worth reading into. `the key index needs 2048 MB` is the
power-of-two table sizing, one rung above.

So, measured rather than asserted:

| rows | C | C++ | Rust | Zig |
|-----:|:-:|:---:|:----:|:---:|
| 100M | yes | yes | yes | **no** |
| 150M | yes | no | no | no |

The old line was true of the C port and of nothing else. Where 200M comes into it
is untested here and stays untested: C is the only port with a rung left to find,
and one port's ceiling is not the tool's.

### Everything is waiting, and that is what the cores column is for

0.45x to 1.11x, against roughly 3x for the same ports on a 2M pair. A 35 GB input
cannot stay in a 16 GB page cache, so every port spends most of its wall clock on
reads. Nothing here is a comparison of engines; it is a comparison of how well
each one tolerates a file it cannot hold.

Worth setting beside the 50M Parquet table, which was read as the same effect.
There, **one** port collapsed to 1.04x while another held 3.05x on the same input.
Here, where the input genuinely cannot be cached, they collapse *together* —
0.45x, 0.50x, 0.51x, 0.89x. That is what page-cache pressure looks like when it is
the explanation, and it is not the shape the 50M Parquet table has. It does not
say what that table is; it does say the two are not obviously the same thing.

### The "Above the input" column stops meaning anything here

It is peak RSS minus the input's size, and it read **-21,649 MB** at 100M and
**-39,276 MB** at 150M. A negative memory overhead is not a finding, it is the
column being asked a question it was not built for: it measures what a port holds
*beyond* an input it can keep resident, and past 16 GB of input there is no such
quantity.

The same size explains a figure that otherwise looks like a cap violation: C++
swar peaks at 15,025 MB against a 13,941 MB cap. `RLIMIT_DATA` bounds the heap and
private anonymous mappings, not the file-backed one, and peak RSS counts resident
pages of the mapping. So at these sizes the RSS column is mostly reporting how much
of the *input* happened to be resident at the peak, not how much the port
allocated. Both columns want suppressing, or relabelling, once the input passes
the host's memory. Not done here — this entry is the measurement, and changing
what the harness prints is its own change.

## 2026-09-14 (ndjson) — two explanations for the C++ sweep, both wrong

The phase timings put C++'s JSON sweep at about 1.9x Rust's and made it the
largest thing left on ndjson. Two candidates suggested themselves from reading the
code. Neither survived measurement, and both are recorded here so the next person
does not spend the afternoon on them again.

### Not the vector scanner

C++'s `next_of1` / `next_of2` have AVX2 and AVX-512 paths behind
`CSVDIFF_SCAN_AVX2` / `CSVDIFF_SCAN_AVX512`, which **only the scanner-variant
builds define** -- `make` alone produces a binary that takes the SWAR path.
Rust's equivalent is behind `target_feature = "avx2"`, which its
`-C target-cpu=x86-64-v3` build does enable. So the published C++ ndjson figures
looked like they came from an unvectorised build being compared against a
vectorised one, which would have been the same kind of unfair default as the
`--engine turbo` flag.

It is not. The AVX2 build is *slower* on this workload:

| Build | sweep readings, 4M rows |
|-------|-------------------------|
| default (SWAR) | 2.18s 2.21s 2.21s 2.23s 2.45s 2.46s |
| `-DCSVDIFF_SCAN_AVX2` | 2.57s 2.59s 2.60s 2.62s 2.63s 2.64s |

Which makes sense once measured rather than assumed. The scanner variants were
built to find *delimiters* -- long runs between one and the next, where a 32-byte
load pays. JSON scanning is short hops: the next quote, the next backslash, a few
bytes away. The vector setup costs more than the bytes it skips. The default build
is the right one here, and `--matrix` will show AVX2 losing on ndjson.

### Not the early exit either

`parse_json` stops walking the object as soon as every key slot is filled. Since
`end_of_json_row` then still has to walk the rest of the line quote-aware to find
the newline, the saving looked illusory -- the tail gets scanned anyway, twice
over for the string skips.

Removing the `break` (safe: first-wins for key columns is enforced by the
`out[slot] == kAbsent` test, not by the exit) changes nothing:

| Build | sweep readings |
|-------|----------------|
| with the early exit | 2.27s 2.29s 2.29s 2.31s 2.37s 2.43s |
| without it | 2.25s 2.29s 2.29s 2.34s 2.37s 2.29s |

Overlapping bands, no signal either way.

### What that leaves

The gap is real and reproduces; the two structural explanations available from
reading the code are not it. Whatever C++ spends the extra time on is inside the
per-row work itself, and finding it wants a profiler rather than another
hypothesis -- which is where this stops rather than guessing a third time.

## 2026-09-14 (instrumentation) — where C++'s ndjson time actually goes

The byte proof took 15% off the C++ ndjson row and left it 2.4x Rust, which said
the row comparison was not the problem and nothing about what was. C, Rust and Zig
all print phase timings under `CSVDIFF_PHASES`; C++ had them only in `pqdiff.cpp`,
for the columnar Parquet path. So the one port that needed profiling was the one
that could not be profiled.

With the same switch and the same shape added to the text path, 4M rows a side,
warm, the two are directly comparable:

| Phase | Rust | C++ | C++ / Rust |
|-------|-----:|----:|-----------:|
| A sweep (parallel) | 0.822s | 1.945s | 2.4x |
| B sweep (parallel) | 0.887s | 2.027s | 2.3x |
| A index insert (serial) | 0.267s | 0.269s | 1.0x |
| B index insert (serial) | 0.668s | 0.272s | **0.4x** |
| join and compare | 0.943s | 1.743s | 1.8x |
| assemble | 0.070s | 0.365s | 5.2x |

**The sweep is the answer.** Parsing and hashing every row of JSON is the largest
phase in both ports and where most of the gap lives.

**One sitting is not enough to price it, though, and the first version of this
entry proved that twice over.** Repeated -- four runs per port, both sides, same
pair, same host:

| Port | Eight sweep readings | Median |
|------|---------------------|-------:|
| Rust | 1.119-1.193s | 1.156s |
| C++ | 2.199-2.285s | 2.238s |

So the sweep gap is about **1.9x**, not the 2.3x the single sitting above showed.
Both ports read slower in this sitting than in that one; the ratio is what
survives, and it is the ratio that is tight -- sixteen readings, no overlap
between the two bands.

The index insert row is worse: it was published here as "Rust's B insert is 2.5x
its A insert, which is a Rust question". It is not. Repeated, the same pair gives
0.778/0.812, then 0.161/0.158, then 0.601/0.319 with **A** the slower side. The
insert is a few tenths of a second, it runs while the other side's parallel sweep
is still going, and which side loses depends on how that lands. The claim is
withdrawn.

What that leaves: the sweep gap reproduces and is worth working on. Nothing else
in the table above has been shown to.

### A second thing the instrumentation bought immediately

The first CSV run through the new timer reported a **30.5s sweep** on a 2M pair.
It is 0.2s warm. Those files had not been touched in hours and the page cache had
been churned by several GB of Parquet fixtures, so that is a cold read at about
25 MB/s, not a finding.

That is the same trap that produced a nonsensical Rust reading earlier the same
day -- a first phases run whose sweep exceeded the process's own wall time -- and
in both cases the timer is what made it obvious rather than something to be
puzzled over. Any number from this file's phase output is worth taking twice.

## 2026-09-14 (ndjson) — the fourth port, and the placement is a property of the port

C++ was the last port without the ndjson byte proof. It is also the one where the
placement question, which cost the Rust port a first attempt, answers itself:
C++'s `lookup` calls `keys_of`, not `fields_of`, so the mate's row is still
unparsed when the lookup returns and the proof belongs exactly where C puts it --
after the lookup, before `fields_of`, beside the CSV proof already there.

4M rows a side, 9 rounds, paired and interleaved, against the same build without
it:

| Build | wall best | median | cpu best | median | Verdict |
|-------|----------:|-------:|---------:|-------:|---------|
| C++ | 4.877s | 5.225s | 13.15s | 13.93s | — |
| C++ + proof | 4.145s | 4.389s | 11.55s | 11.75s | **15% less work** |

Counts and per-column figures identical, and identical to C, Rust and Zig on the
adversarial fixture.

### What the four ports say about placement

Three ports, three days, one proof, and the right place for it differs by port:

| Port | What its lookup parses | Where the proof goes | Cost of the other choice |
|------|------------------------|----------------------|--------------------------|
| C | keys only | after the lookup | — |
| C++ | keys only (`keys_of`) | after the lookup | — |
| Rust | the mate's whole row | *inside* the lookup | 2% lost instead of 14% gained |
| Zig | the mate's whole row | *inside* the lookup | not paid: known in advance |

The proof is the same in all four. What differs is what the surrounding code has
already done by the time it runs, and that is not visible from the proof itself --
which is why the Rust attempt had to be measured twice to find it. The ports that
parse the mate inside the lookup also had to widen the run to cover the keys
(`width`, not `nc`), because a proof running before the key comparison has to
stand in for it.

### Where ndjson stands with all four carrying it

Same 4M pair, best of three, one sitting, so these rows are comparable with each
other and with nothing else:

| Port | Wall | CPU | Cores |
|------|-----:|----:|------:|
| Rust | **1.84s** | 6.02s | 3.27x |
| Zig | 2.05s | 7.38s | 3.60x |
| C | 2.42s | 6.70s | 2.77x |
| C++ (now) | 4.35s | 11.54s | 2.65x |
| C++ (before) | 5.03s | 13.68s | 2.72x |

C led this column at the start of the week and is third now, without having got
slower: Rust and Zig gained the proof it already had. C++ is still **2.4x Rust**
with the proof in place, on nearly twice the CPU at fewer cores, so whatever is
slow there is not the row comparison and 15% was never going to close it.

## 2026-09-14 (ndjson) — and a third port, where the placement was known in advance

The same port into Zig, done second, with the lesson from the Rust one applied
before writing any code: check where this port's lookup parses the mate.

Zig's `lookup` has the same shape as Rust's — the span check, then `fieldsOf` or
`keysOf` depending on a `Want` — so the proof goes beside the span check, before
the parse, and there was no slow first version to discard this time.

4M rows a side, 9 rounds, paired and interleaved, against the same build without
the proof:

| Build | wall best | median | cpu best | median | Verdict |
|-------|----------:|-------:|---------:|-------:|---------|
| Zig | 2.396s | 2.498s | 9.02s | 9.45s | — |
| Zig + proof | 1.969s | 2.042s | 7.31s | 7.60s | **19% less work** |

Counts and per-column figures identical, and identical to C, C++ and Rust on the
adversarial fixture.

Where the ndjson column stands with three of the four ports carrying it, same 4M
pair, best of three:

| Port | Wall | CPU | Cores |
|------|-----:|----:|------:|
| **Zig (now)** | **2.02s** | 7.50s | 3.71x |
| **Rust (now)** | 2.07s | 7.13s | 3.44x |
| C | 2.26s | 6.69s | 2.96x |
| Zig (before) | 2.38s | 8.74s | 3.67x |
| C++ | 4.75s | 13.43s | 2.83x |

C is now third on wall while still doing the least CPU work of the four: 6.69s
against Zig's 7.50s, at 2.96 cores against 3.71. The remaining ndjson question is
that ratio, not the proof.

Read within this table only — one host, one size, one sitting.

## 2026-09-14 (ndjson) — the byte proof reaches a second port, and where it has to sit

README item 1 had called porting C's ndjson tail scan "the largest thing left
here". This is that port, into Rust, and the interesting part is not the speedup
but that the first version of it was *slower*.

4M rows a side, 9 rounds, `scripts/bench_ab.sh`, paired and interleaved, against
the same build without the proof:

| Version | wall best | median | cpu best | median | Verdict |
|---------|----------:|-------:|---------:|-------:|---------|
| after the key lookup | 2.458s | 2.597s | 8.70s | 9.10s | **2% more work** |
| before the mate is parsed | 2.068s | 2.180s | 7.21s | 7.53s | **14% less work** |

Same proof, same tail scan, same tests. What changed is where it runs.

C puts its proof after `index_lookup` returns a mate, and that is right for C,
because C's lookup compares only the key columns. Rust's `lookup` calls
`fields_of` on the candidate — it parses the mate's **whole row** to compare its
keys — so a proof placed after it cannot save the parse. It can only skip the
seventeen-column field comparison that follows, while adding a ~400-byte memcmp
and a tail scan. That is the 2%, and it is what the first A/B measured.

Moving it beside the CSV proof, which already runs before `fields_of` for exactly
this reason, is what turned 2% lost into 14% gained. It also forced the design to
be more careful than C's: a proof that stands in for the key comparison has to
cover the keys, so the run reaches through the last value of **every** wanted
field rather than the last compared one. `width`, not `nc`.

Where that leaves the ndjson column, same 4M pair, best of three:

| Port | Wall | CPU | Cores |
|------|-----:|----:|------:|
| **Rust (now)** | **2.07s** | 7.23s | 3.49x |
| C | 2.29s | 6.84s | 2.99x |
| Zig | 2.43s | 8.92s | 3.67x |
| Rust (before) | 2.51s | 8.85s | 3.53x |
| C++ | 4.78s | 13.47s | 2.82x |

C still does the least CPU work of the four; Rust gets ahead on wall by spreading
more of it. Read within this table only — it is one host and one size.

Correctness, since a byte proof that is wrong is worse than no proof: counts and
per-column figures identical to the no-proof build at 4M, and identical across C,
C++, Zig and Rust on a fixture built for the ways this goes wrong — a compared
name repeated in the mate's tail, the same values written in a different order,
a value that is a prefix of the mate's longer one, an escaped name, a difference
confined to an ignored column, and the keys written after the last compared
value. Those are `rust/tests/turbo.rs` now, and the first of them was confirmed to
fail when the tail scan is stubbed out.

## 2026-09-14 (contention) — separate hosted runners do not contend, and the first run said otherwise

`bench-contention.yml` had never been dispatched since it was written. It exists
because two documents in this repository contradicted each other — CLAUDE.md's
benchmark rule said two timing jobs "share a host and measure each other's
contention", `scale-ceiling.yml`'s header said "these are separate hosted runners
so they do not contend" — and the `benchmark-host` group, which at the time made
a pull request's benchmark queue behind anybody's dispatched ladder, rested on the
first being true.

It is A/A: the same benchmark alone, then the identical benchmark as one of N jobs
started together.

### The run to believe: 10M rows, crowd of six

| Build | Alone | Crowd median | Ratio | Crowd spread |
|-------|------:|-------------:|------:|-------------:|
| C     | 1.808s | 1.808s | 1.00x | 1.604-2.211s |
| C++   | 4.571s | 4.347s | 0.95x | 3.613-4.573s |
| Rust  | 2.061s | 2.007s | 0.97x | 1.855-2.411s |
| Zig   | 1.814s | 1.784s | 0.98x | 1.755-2.156s |

**Median 0.98x**, and not one build slower than 1.00x. Six jobs at once are
indistinguishable from one alone. `scale-ceiling.yml` was right.

### The run not to believe: 2M rows, crowd of four

| Build | Alone | Crowd median | Ratio | Crowd spread |
|-------|------:|-------------:|------:|-------------:|
| C     | 0.351s | 0.402s | 1.14x | 0.301-0.403s |
| C++   | 0.805s | 1.184s | 1.47x | 0.958-1.210s |
| Rust  | 0.504s | 0.678s | 1.35x | 0.553-0.704s |
| Zig   | 0.402s | 0.379s | 0.94x | 0.302-0.403s |

**Median 1.24x**, and the workflow duly printed "that is what contention looks
like, and `benchmark-host` is earning its keep".

It is wrong, and the table says so if you read the column the workflow's own
closing line points at. **C's fastest crowd run is 0.301s against 0.351s alone,
and Zig's is 0.302s against 0.402s.** Contention does not make a job finish
sooner. What that pattern is, is runner-to-runner variation dominating a
measurement whose runs last a third of a second, compared across two separate
sittings — the failure mode `scripts/bench_ab.sh` was built to defeat, and the
reason it interleaves and pairs instead of running all of A and then all of B.

Both tables are here because the pair is the lesson. A workflow that prints a
verdict will print one whether the measurement can support it or not, and this one
hedged correctly in its own last line while its headline did not. The remedy was
the one it recommends itself for an unclear result: raise the row count until the
runs last seconds, raise the crowd, and look again.

### What this changed, a day later

Nothing at the time: every benchmark still named `benchmark-host`, so a pull
request's 2M run still queued behind a dispatched ladder. Acting on a number is a
separate decision from taking it, and that decision has now been taken — by
watching a pull request's benchmark sit `pending` behind a 100M/150M ladder while
the change it measured waited to be reviewed. Queued 12:33:35, started 12:44:21,
finished 12:47:56: **10m 46s of waiting for a 3m 27s measurement**, and the ladder
it waited on was running on its own separate runners the whole time.

`bench-2m.yml` has a group of its own now, `benchmark-pr-<ref>`, so a pull
request's benchmark waits for nothing but another run on its own branch. The two
**dispatch-only** benchmarks keep `benchmark-host` and so keep serialising against
each other, which is the part of the lock this table did not argue against: a
deliberate measurement wants a quiet repository, and `bench-contention.yml` in
particular needs to stay isolated for its own result to mean anything — an
experiment has to be valid whether or not it finds an effect.

What that gives up is one case: a dispatched contention run can now overlap a pull
request's 2M run. It is dispatched by hand a few times a year, and the remedy is
to dispatch it when nothing is in flight rather than to make every pull request
pay for the case. Both workflow headers say so.

## 2026-09-14 (parallelism) — the Rust Parquet reader gets *better* with size, so 50M is something else

Written to check a claim made in yesterday's README item 2, which said the reader
"parallelises at small sizes and stops somewhere before" 50M. It does not stop.

Local, 4 vCPU / 16 GB container, the columnar Parquet path (`auto`), report on,
best of three by wall with that run's CPU:

| Rows | Wall | CPU | Cores | Peak RSS |
|-----:|-----:|----:|------:|---------:|
| 500k | 0.20s | 0.45s | 2.30x |   192 MB |
|   2M | 0.62s | 1.61s | 2.58x |   700 MB |
|   8M | 2.08s | 6.18s | 2.97x | 2,446 MB |
|  20M | 5.58s | 17.76s | **3.18x** | 5,753 MB |

Monotonically up. So the 1.04x in the 50M rung is not the reader running out of
work to spread, and the item that said so was wrong on one data point.

What is different at 50M is that it is at the ceiling: 13.7 GB of heap beside a
7,673 MB mapped input on a 15,989 MB host. The input cannot stay in the page
cache, so every miss is a read, and 1.04x with 30.9s of CPU means about twenty of
those thirty seconds went on waiting.

A weak test of that, and its limits. Same 20M pair, warm and then after
`echo 3 > /proc/sys/vm/drop_caches`:

| 20M | Wall | CPU | Cores |
|-----|-----:|----:|------:|
| warm | 5.64s | 17.81s | 3.16x |
| caches dropped | 6.18s | 17.45s | 2.82x |

The right direction, and nothing like far enough. It also cannot be pushed
further here: this file already records that `drop_caches` leaves the hypervisor's
copy warm, so a genuinely cold read is not available in this container. And 20M
does not reach the ceiling anyway — 5.7 GB of heap and 3.2 GB of input on 16 GB
has room to spare, which is the whole difference from the 50M rung.

**The hole in the explanation is the interesting part.** Zig hits 3.05x at 50M with
a *larger* peak than Rust's. If the ceiling alone did this, Zig would stall too. So
the question is not why a run at the ceiling waits, it is why this one reader waits
where another does not — how each walks its mapping, not how much either holds.
Rust's is the narrowest of the four.

Two tables here, and neither is comparable with the other: the sweep is this
container and the 50M row is a hosted runner. What the sweep establishes is a
*direction* within one host, which is all it is used for.

## 2026-09-13 (scale) — Parquet's ceiling is between 50M and 100M, for all four ports

Two rungs, dispatched together, one job each: `50m` and `100m`, parquet,
`--repeats 1`, `--first Rust`, on separate 16 GB runners with each port capped at
**13,941 MB** through `RLIMIT_DATA`.

### 50M — all four finish, and the Rust column is nothing like it was

Input 7,673 MB, generated in 87.6s.

| Build | Compare |    Rows/s |   CPU | Cores |  Peak RSS | Above the input |
|-------|--------:|----------:|------:|------:|----------:|----------------:|
| Zig   |  12.13s | 4,123,944 | 37.0s | 3.05x | 14,232 MB |        6,558 MB |
| C     |  16.60s | 3,012,375 | 23.6s | 1.42x | 13,812 MB |        6,138 MB |
| C++   |  19.08s | 2,620,827 | 41.8s | 2.19x | 14,188 MB |        6,515 MB |
| Rust  |  29.70s | 1,683,942 | 30.9s | 1.04x | 13,738 MB |    **6,064 MB** |

`counts: identical everywhere`. Four out of four, where the last 50M table had
three: the Rust port failed that rung, and the reason was the benchmark passing
`--engine turbo` and retiring the columnar reader, not the port.

Read the last column and then the one before it. **Rust is the narrowest of the
four** — 6,064 MB above the input against Zig's 6,558 — which is the reverse of
the claim the previous entry set out to explain, and it settles it: the Rust
Parquet reader does not hold more than the others. It holds the least.

What it does instead is run on one core. **1.04x cores**, against Zig's 3.05x,
and that is the whole of 29.70s against 12.13s. The same reader showed 2.18x at
500k. So it parallelises at small sizes and stops somewhere before this one, and
that is the open question now — a narrower and more answerable one than "it holds
too much", which was false.

C is worth a glance too, at 1.42x: second fastest on a third of Zig's CPU.

### 100M — nobody, and all four say so

Input 15,347 MB, generated in 222.8s.

```
Rust  FAILED (exit 2): error: the parquet engine failed: out of memory:
                       one dictionary index per row needs 381 MB
C     FAILED (exit 2): error: out of memory reading the parquet file
C++   FAILED (exit 2): error: std::bad_alloc
Zig   FAILED (exit 2): error: out of memory
counts: none -- no port got far enough to answer
```

So the Parquet ceiling on a 16 GB runner is between 50M and 100M rows, for every
port. Not a prediction: a rung that ran, generated 15 GB cleanly, and came back
with four refusals.

This is also the first rung where everything built earlier today paid off at once.
Rust's line names the structure *and* its size, and 381 MiB is exactly
100,005,000 × 4 bytes, so the refusal is precise rather than approximate. Before
today that line read `FAILED (exit -6)` and said nothing, because the port aborted
on signal 6 and the harness sent its stderr to /dev/null. Three separate fixes
have to be in place for those four lines to exist.

### The prediction, and how it did

Written down before the dispatch, from two local points — 500k at 114 MB and 2M at
377 MB above the input — fitted to 26 MB + 175 MB per million rows:

| Rung | Predicted | Measured | Verdict |
|------|----------:|---------:|---------|
| 50m  | 8,793 MB, fits | 6,064 MB | verdict right, **number 45% high** |
| 100m | 17,560 MB, refuses | refused | verdict right |

Both verdicts held and one number was badly wrong. The real slope at scale is
about 121 MB per million rows, not 175. The fit was taken from 500k and 2M, and
the index table sizes in powers of two, so a point at either of those sizes can
sit just after a doubling and carry a step as though it were a slope. Two points
cannot tell the difference; three across a wider range would have.

Worth recording what that error then did: on seeing 6,064 rather than 8,793, the
next guess was that 100M would need about 12 GB and therefore **pass**. It did
not. A slope wrong in one direction made a correct prediction look doubtful, which
is a better argument for writing predictions down before the run than for any
particular fitting method.

## 2026-09-13 (memory) — the Rust port stops aborting, and the Parquet column was wrong

Not a speed entry. The question was whether making every input-scaled allocation
in the Rust port fallible shows up in the clock, and the answer is no: four
paired A/B runs, all four "no result".

Host: 4 vCPU, 16 GB, the container these tables come from. Both builds compiled
the same way; `scripts/bench_ab.sh`, interleaved, paired ratio of per-round
medians. 2M rows a side, and CSV again at 6M because the per-row check is in the
sweep and a longer run has a lower noise floor.

| Format | Rows | Rounds | wall (old/new) | cpu (old/new) | Verdict |
|--------|-----:|-------:|---------------:|--------------:|---------|
| csv     | 2M | 7 | 1.04x 0.94-1.07 | 1.03x 0.98-1.04 | no result |
| parquet | 2M | 7 | 1.01x 0.90-1.05 | 1.00x 0.94-1.02 | no result |
| ndjson  | 2M | 7 | 1.01x 1.00-1.04 | 1.01x 1.00-1.04 | no result |
| csv     | 6M | 9 | 1.03x 1.00-1.04 | 1.01x 0.97-1.05 | no result |

Which is what the shape of the change predicts. `try_reserve` costs the same as
`reserve` on the path that succeeds, and the one addition to a per-row loop is
`len == capacity` before a push — the comparison `push` already makes, with a
different branch on the taken side.

What it bought, measured by walking `RLIMIT_DATA` down 29 rungs from 4000 MB to
4 MB on a 2M-row pair of each format:

| Path | Answered | Refused (exit 2) | Signal 6 | Hung |
|------|---------:|-----------------:|---------:|-----:|
| parquet before | 12 | 0  | 17 | 0 |
| parquet after  | 12 | **17** | **0** | 0 |
| csv before     | 13 | 0  | 15 | 1 |
| csv after      | 13 | 14 | 2  | 0 |
| ndjson before  | 13 | 0  | 15 | 1 |
| ndjson after   | 13 | 14 | 2  | 0 |

Read the first column before the others: 12, 13 and 13 rungs answered, before and
after, the same rungs each time. The ceiling did not move and was never going to
— none of this makes the port fit in less. What changed is that below the ceiling
it now says so.

The two rungs that still abort on the text paths are 300 and 250 MB, and the
allocations that fail there are 6, 10, 12 and 17 bytes — single `String`s in the
report's row samples, which have no fallible form in Rust. That band sits between
220 and 320 MB on this pair and is a constant, because the samples are capped by
`--max-rows`: at 50M it is invisible against a 13 GB comparison, which is why the
Parquet ladder is clean end to end.

Two failures worse than the abort turned up while walking that ladder, neither
visible before because the abort got there first:

- `Scope::spawn` panics when the OS refuses a thread. Under a memory cap that is
  exactly when it happens — a thread's stack is a private mapping, which is what
  `RLIMIT_DATA` bounds — so the run that had just discovered it was short of
  memory exited 101 with `failed to spawn thread: Os { code: 11 }`.
- With `RUST_BACKTRACE=1` set it did not manage even that. Printing the panic
  takes the backtrace lock, symbolising allocates, that allocation fails, and the
  allocation error hook reaches for the lock the panicking thread already holds.
  `gdb` on the stuck process: two threads, both in `futex_do_wait`, one of them
  through `default_alloc_error_hook -> std::sys::backtrace::lock`. On a runner a
  hang is worse than an abort, because the rung burns its whole timeout and
  reports nothing.

### And the Rust Parquet column has been wrong the whole time

Chasing the width above found something bigger than the width. The Rust port
carries two Parquet readers, and `engine.rs` is explicit about what selects
between them:

> `requested`, not `engine`: `auto` has already been resolved to `Turbo` for any
> Parquet input by this point, so testing the resolved value here would send every
> pair to `turbo` and quietly retire the columnar path. **Only an explicit
> `--engine turbo` should do that.**

`scripts/bench_formats_ports.py` passed `--engine turbo` on every Rust row. It had
been bundled into a constant called `report` next to `-o /dev/null`, so it read as
part of suppressing the HTML report and was never the thing anyone looked at.

Same 500k pair, same binary, one flag, identical counts:

| Rust on Parquet | Compare | Peak RSS | Above the input |
|-----------------|--------:|---------:|----------------:|
| `--engine turbo` |  0.43s |   346 MB |          263 MB |
| `auto`           |  0.15s |   174 MB |           91 MB |

So the Rust Parquet column in every table this project has published is of the
reader `auto` does not pick. With the flag dropped, 500k rows, `--repeats 2`, all
three formats, same host:

| Build | Format  | Compare | Above the input |
|-------|---------|--------:|----------------:|
| C     | parquet |   0.15s |           87 MB |
| Zig   | parquet |   0.15s |           73 MB |
| C++   | parquet |   0.20s |           92 MB |
| Rust  | parquet |   0.25s |          114 MB |

Against 1.01s and 257 MB for the same row the day before. A gap that looked like
four times the time and three times the memory is about one and a half times each.
Counts identical everywhere.

It also explains the 50M rung better than "the allocator has no failure path"
does. Turbo materialises one eight-byte field per cell — 50M rows by nineteen
columns is 7.6 GB a side before anything else is allocated — so under a 13,941 MB
cap it never had a chance, whichever way it reported that. Whether the columnar
reader fits at 50M is not a question a 500k run answers; the ladder can.

Two lessons worth more than the numbers. A benchmark flag hidden in a constant
named for something else is a flag nobody reads. And this was found by following
a memory figure that looked too large, which is the same reason the harness prints
"above the input" at all.

## 2026-09-13 (scale) — 50M rows of Parquet, and the port that cannot do it

> **Superseded the same day, and the title is wrong.** The Rust port does do this
> rung — see *Parquet's ceiling is between 50M and 100M* at the top of this file,
> where it finishes in 29.70s and is the narrowest of the four. What failed here
> was the harness, which passed `--engine turbo` and so measured a reader `auto`
> does not pick. The numbers below are real; the conclusion drawn from them was
> not. Left as it was written, because that is what this file is for.

The first numbers this project has for fifty million rows of Parquet. There were
none before because the rung never survived: ten dispatched ladder runs, every
failure `exit 143` with "the runner has received a shutdown signal", during the
comparison, after the data generated cleanly. The runner was being reclaimed for
memory and a reclaimed runner runs no further step, so the rung reported nothing
at all — not even that it had run out of memory.

Capping each port through `RLIMIT_DATA` changed the outcome from nothing to this.
One rung, `50m` / parquet, `--repeats 1`, no scanner matrix, on a 16 GB runner
with the cap at **13,941 MB**. Input 7,673 MB, generated in 87.5s.

| Build | Compare |    Rows/s |   CPU | Cores |  Peak RSS | Above the input |
|-------|--------:|----------:|------:|------:|----------:|----------------:|
| C     |  28.33s | 1,765,240 | 19.0s | 0.67x | 13,740 MB |        6,066 MB |
| C++   |  24.60s | 2,033,134 | 42.7s | 1.74x | 14,075 MB |        6,402 MB |
| Rust  |       — |         — |     — |     — |         — |               — |
| Zig   |  14.68s | 3,405,471 | 36.8s | 2.51x | 14,458 MB |        6,785 MB |

`counts: identical everywhere` across the three that answered: 49,950,000
matched, 2,994,637 changed, 50,000 added and removed.

**Zig is 1.9x C here**, which is the reverse of every other Parquet row in this
document and is the most interesting thing in the table. C spends 0.67 cores
against Zig's 2.51 — it is not slow, it is idle, and at this size being idle
costs 14 seconds.

**Rust aborted on signal 6.** Its default allocator has no failure path, so
where C, C++ and Zig each hit the ceiling and said so, Rust called `abort`. That
is why its row is dashes rather than a refusal, and it is a defect in the port
rather than a property of the size — the other three prove 50M parquet fits.
`--first Rust` is what makes this legible: Rust ran first, failed first, and the
other three still produced their numbers.

Two notes on reading the peak RSS. It exceeds the cap for C++ and Zig, and that
is correct rather than a leak: `RLIMIT_DATA` bounds the heap and anonymous
mappings, while RSS counts the file-backed mapping of the input too. And these
are single-repeat numbers on a shared runner — good enough to say Zig leads and
Rust aborts, not good enough to argue about 24.60s against 28.33s.

---

## 2026-09-10 (build flags) — the Rust column was compiled for a different machine

A defect in the instrument, found while looking at why one port always seemed to
trail. Every table produced through `scripts/build_ports.sh` — which is what
`bench-2m`, `bench-ladder`, `bench-contention`, `formats` and `parity` all use,
so every table a pull request has ever shown — built the Rust port for baseline
x86-64 while building C and C++ for the runner.

Not inferred from timings. From `rustc` itself, on the host that runs these:

```
$ rustc --print cfg | grep target_feature
target_feature="fxsr"  target_feature="sse"  target_feature="sse2"

$ rustc --print cfg -C target-cpu=native | grep target_feature
... avx, avx2, avx512bw, avx512f, avx512vl, bmi1, bmi2, f16c, fma ...
```

Both Makefiles probe for and add `-march=native`, and Zig's default target is
already the host — a `-Dcpu=native` build is byte-identical to one without, which
is why that flag was never missed. Rust was the one port whose default is
baseline, and nothing in `build_ports.sh` supplied otherwise. So the cross-port
rows compared an SSE2 build against an AVX-512 one.

This contradicts what both documents say. README.md and the note above the
10m joint table below both state that every port is compiled for the runner, and
`benchmark-native.yml` does set `RUSTFLAGS` by hand — so the tables *that*
workflow produced are sound. The workflows that build through the shared script
never set it, and those are the ones that run on a pull request.

**What it was worth is not established.** Two interleaved passes on the same
1m-row CSV pair, same host, same binaries:

| pass | rounds | Rust (baseline) | Rust (native) |
|---|---:|---:|---:|
| first | 7 | 0.53s median | 0.46s median |
| second | 9 | 0.53s median | 0.51s median |

Native is ahead in both and behind in neither, but 13% and 4% are not the same
answer and the second pass's ranges overlap. The reason to fix this is not a
speed number — it is that a table comparing four ports has to compile them the
same way to mean anything.

Fixed in `build_ports.sh` rather than in the six workflows, so it cannot be
forgotten again by the seventh — and finding the fix took two red pull requests.
The first attempt resolved the host CPU and put its name in the flag, which was
right about the cache and wrong about something worse. `-C target-cpu` reaches
*every* crate cargo compiles, not just the csvdiff binary the tables measure —
including the proc-macro build tools behind `serde`'s derive, whose compiled
binaries execute on the runner mid-build. On GitHub's VM fleet the resolver
picked `znver4`, and one of those tools died with SIGILL — illegal instruction —
because the guest advertises AVX-512 that it cannot execute: lanes that resolved
`znver3` built fine, while `znver4` lanes and the parity job failed, and CI
(Rust), which builds without the flag, passed on the same SHA. The automatic
default is therefore capped at `x86-64-v3`, a fixed string: it is the
fleet-wide executable floor (every current ubuntu-latest VM runs AVX2 — the
`csvdiff-avx2` scanner build is already unconditional — and C and C++'s
`-march=native` has never faulted on one), keeping the gain over SSE2, and a
uniform fingerprint is now safe where `native` was not, because code a
`rust/target` cache hands back runs on all of them.

### And the build timings in those logs were wrong too

`build_ports.sh` reported each target's duration as `SECONDS - started` computed
in the join loop. The loop joins in order and blocks on each `wait`, so a target
that finished early but sat behind a slow one was charged the wait as well. One
CI run reported `zig (108s)` and `cpp (108s)` beside `cpp-gen (105s)` when the
whole script took 123s — and `cpp-gen` had not started until a slot freed, so its
own build cannot have taken 105s. Everything looked as slow as the slowest thing
in front of it, which is the one reading that makes a parallel build script
useless for deciding what to speed up. Each target times itself now.

## 2026-09-10 (measurement) — what this host cannot tell you about phases

A negative result about the instrument rather than the code, recorded because it
cost an afternoon and would cost the next one.

### The claim that did not survive

`CSVDIFF_PHASES=1` on a five-million-row CSV pair says the index build is about
two thirds of the run, and three single runs at different thread counts said it
got *worse* with more cores:

| threads | both indexes | join and compare |
|---|---:|---:|
| 1 | 0.932s | 1.248s |
| 2 | 0.864s | 0.643s |
| 4 | 1.078s | 0.557s |

That reads as a clear finding: the dominant phase anti-scales, the join scales
2.2x, so the insert -- which is a serial loop in `build_index`, plainly visible
in the source -- is the bottleneck and wants parallelising.

It is not a finding. It is three samples.

### What nine rounds say

Interleaved, same order each round, median and middle half:

| threads | n | min | median | max | middle half |
|---|---:|---:|---:|---:|---|
| 2 | 9 | 0.567s | 0.716s | 1.646s | 0.577-0.963 |
| 4 | 9 | 0.590s | 0.671s | 0.964s | 0.622-0.708 |
| 8 | 9 | 0.556s | 0.663s | 1.016s | 0.614-0.734 |

The middle halves overlap almost entirely. There is no anti-scaling, and if
anything four and eight threads are slightly ahead of two -- the opposite of
what the single runs said. One round of `--threads 2` took 1.646s and another
took 0.567s, on the same binary and the same file, three times apart.

### What that means for work like this

**A phase breakdown from one run is not a measurement here.** The spread on this
host is wider than any effect worth chasing: a change that made the index build
20% faster would sit entirely inside the noise of the thing it improved.

The instrument for this is `scripts/bench_ab.sh`, which pairs and interleaves
and reports a middle-half ratio precisely so that "not established" is one of
the answers it can give. `CSVDIFF_PHASES` is for *where* the time goes, not for
*how much* -- it has no repeats and prints whatever the machine was doing that
second.

This is the same host that could not measure cold-disk I/O, for the same kind of
reason: it is shared, and the hypervisor is between the measurement and the
hardware. A performance change here should be proposed from the source, and
measured on a runner that has been asked for numbers rather than for a shell.

### The one thing that is still true

The insert in `build_index` really is serial -- `run_parts` for the sweep, then
a plain `for (r = 0; r < ix->rows; r++)` with an inner probe loop. That is a
fact about the code and does not depend on any timing. Whether it costs anything
worth the correctness risk of partitioning the table is a question this host
cannot answer, and the answer has to come before the refactor rather than after.

---

## 2026-09-10 (scale) — sixty million rows, and nobody's ceiling

The first run of `scale-ceiling.yml` in its fanned-out shape: every (port, size)
its own job on its own hosted runner, 24 of them, plus four jobs that take the
memory away until the run dies. [Run 34476661822][r], 30 jobs, all green.

[r]: https://github.com/andrey-usa/csvdiff/actions/runs/34476661822

### The headline is what did not happen

Nobody hit a ceiling. All four ports compared **sixty million rows — 21,053 MB
of input for the pair** — on a four-core runner with 16 GB of RAM, and the report
says "ladder ran out, not the port" for every one of them. Two earlier runs had
died at forty million, which is what made sixty look like a reach; both of those
were one job doing several sizes, or several formats, on one disk. A rung with a
runner to itself has about 25 GB free after the toolchain cleanup, and 21 GB
fits.

So the honest statement of the ceiling is: **this ladder was too short to find
one.** Not "the ports reach 60m" — that is where measuring stopped, not where
they stop.

### The memory floor — 10m rows, CSV

The smallest cgroup limit the same comparison finishes inside, found by bisection.
This is the number peak RSS cannot give you: an engine that maps its input has
reclaimable pages, so its RSS is whatever the kernel allowed rather than what it
needed.

| Port | Smallest limit that finishes |
|---|---:|
| **c** | **733 MB** |
| cpp | 892 MB |
| zig | 892 MB |
| rust | 924 MB |

C leads by about 20% over the next three, which sit within 4% of each other.
Consistent with the 126 MB this port needed for two million rows, measured
locally in the memory entry below.

### The wall times from that run are not in this entry, deliberately

They were collected, and they are not comparable. Each cell ran on its own
hosted runner, so two cells differ by hardware as much as by size — and the
numbers show it plainly: Rust took 142s at forty million and 92s at sixty,
Zig 320s at fifty and 99s at sixty. A size curve does not do that. A fleet
spanning CPU generations does.

That is a property of the fan-out, which was built for parallelism and for
surviving a lost runner, and it costs the one thing the sequential shape had:
one port's whole ladder on one machine. `scale-ceiling.yml` is the right
instrument for *how far* and *how little memory*, and the wrong one for *how
fast*. For that, `bench-ladder.yml` puts every port on one host per size.

The report now says so above the table, and prints which CPU each cell drew
whenever they were not all the same, so the caveat can be checked rather than
taken on trust.

---

## 2026-09-10 (rle_fill) — 18.5% of the instructions, none of the time

A negative result, recorded because it would cost the next person the same
afternoon it cost this one.

The entry below profiled the columnar path, found `rle_fill` at 18.5% of
instructions -- second only to `pq_read_column` -- and called it the largest
lever left. It unpacks bit-packed dictionary indices with one eight-byte load,
one shift and one mask per value. A bit-packed group of eight is exactly `width`
bytes, so with `width` known at compile time every shift and the mask fold to
constants: the textbook specialisation.

It was written -- a `switch` over widths 1 to 32, one loop each, from a macro.
All 67 cases passed and the counts were unchanged. Paired and interleaved,
fifteen rounds:

| Input | Before | After | Paired ratio | |
|---|---:|---:|---:|---|
| Parquet, 5m rows | 1.028s | 1.036s | 0.991 (middle half 0.929-1.052) | not established |
| Parquet, 5m rows, snappy | 1.562s | 1.582s | 1.054 (0.990-1.311) | not established |

Both middle halves cross one and the snappy median is worse, so it was thrown
away rather than shipped. The interesting question was why a real reduction in
instructions bought nothing.

### The ceiling

Ask what the whole function is worth. A build with the unpacking deleted --
`memset` in its place, answers wrong on purpose, `bit` still advanced so the rest
of the reader stays in step -- is the ceiling on any optimisation of it:

| | Wall |
|---|---:|
| real unpacking | 1.020s |
| unpacking removed entirely | 1.057s |

Removing the work made it slightly slower, which is noise around zero. **All of
`rle_fill` is worth nothing measurable.** Its instructions issue in the shadow of
the loads they wait on: the phase is memory-bound, and not issuing them buys
exactly what that sounds like.

### What this says about the profile below

`callgrind` counts instructions. It found the CSV field scan, where count and
time did line up -- 6.6% of wall for a wider step. It found `rle_fill`, where
they do not line up at all. An instruction profile picks candidates; it does not
rank them, and the entry below should not have read as though it did.

The ceiling build is the cheap way to tell those apart, and it belongs before the
optimisation rather than after: delete the work, accept wrong answers, time it.
Ten minutes. Not running it first is what this entry cost.

## 2026-09-10 (C field scan) — a wider step where the target has one

One 4-core container. `callgrind` on a single-threaded run first, to find out
where the CSV path actually spends itself:

| Function | Instructions |
|---|---:|
| `next_of2` | **43.2%** |
| `parse_csv_row` | 27.1% |
| `hash_field` | 9.2% |
| `compare_part` | 5.0% |

Seventy per cent of the work is finding field ends. `next_of2` was SWAR at eight
bytes a step; the fields in this data average about ten, so a typical field cost
two iterations. Sixteen bytes costs one, and thirty-two costs one with room to
spare.

Paired and interleaved against the commit before it, fifteen rounds each:

| Input | Before | After | Paired ratio | |
|---|---:|---:|---:|---|
| CSV, 2m rows | 0.469s | **0.442s** | **0.934** (middle half 0.824-0.978) | real |
| ndjson, 2m rows | 1.319s | 1.280s | 0.984 (0.942-1.057) | not established |
| Parquet, 5m rows | 1.012s | 1.048s | 1.004 (0.954-1.393) | not established |

**About 6.6% on CSV, and nothing claimed for the other two.** Both of their
middle halves cross 1.0. ndjson's fields are longer, so the eight-byte step was
already finding hits in one iteration; the columnar Parquet path does not use
this scanner at all and is there as a control, which is what a ratio of 1.004
looks like.

The step is chosen at compile time from what the build targets -- `-march=native`
defines `__AVX2__` where the CPU has it -- so there is no dispatch on the hot
path, and a build for baseline x86-64 gets the SSE2 step that architecture
always has. aarch64 falls through to the SWAR loop unchanged. All three were
built and run against the same inputs, including the awkward fixtures, and give
the same counts.

`next_of1` beside it is left as SWAR. It did not appear in the profile, and a
second copy of this with no number behind it is complexity for its own sake.

### What the profile says is left

The same run, on the columnar path:

| Function | Instructions |
|---|---:|
| `pq_read_column` | 23.5% |
| `rle_fill` | 18.5% |
| `column_part` | 17.1% |
| `plain_slices` | 6.6% |

`rle_fill` unpacks bit-packed dictionary indices one value at a time. That
looked like the largest lever left in the columnar path. It was measured next,
and it is not one -- see the entry above.

## 2026-09-10 (memory) — what each port needs, and what each does when it cannot have it

One 4-core / 15 GB container. Peak RSS from `getrusage(RUSAGE_CHILDREN)`, each
port in its own process so the high-water mark is its own.

2,000,200 against 2,000,100 rows of CSV, 368 MB a side:

| Port | Peak RSS |
|---|---:|
| **C** | **827 MB** |
| Zig | 874 MB |
| C++ | 924 MB |
| Rust | 1,003 MB |

736 MB of that is the two mapped files, so what the ports actually differ over
is the 91-267 MB on top. C is lowest, and the same order holds on the 5m-row
Parquet pair: C 1,630 MB, Zig 1,613 MB, C++ 1,764 MB, Rust 1,853 MB.

### And what happens when it runs out

The same pair under a tightening `ulimit -v`, which makes allocation fail rather
than the kernel intervene:

| Ceiling | C | C++ | Rust | Zig |
|---|---|---|---|---|
| 2048 MB | finishes | finishes | finishes | finishes |
| 1024 MB | **finishes** | `std::bad_alloc`, exit 2 | **abort, exit 134** | **finishes** |
| 512 MB | clean error | clean error | clean error | clean error |

Rust aborts where the other three do not: `memory allocation of 456 bytes
failed`, SIGABRT, because the default Rust allocator has no failure path to
take. C++ surfaces the exception name rather than a sentence. C and Zig finish
the run.

### The band where checking `malloc` does not help

`ulimit -v` is the kind test. Linux's default heuristic overcommit is the real
one, and on this box it has three regions:

| Request | What happens |
|---|---|
| 8 GB | `malloc` succeeds, every page touched, fine |
| **14 GB** | **`malloc` succeeds, SIGKILL on touch** -- exit 137, no message |
| 20 GB | `malloc` returns NULL, refused up front |

The middle row is why a port that checks every allocation still dies without a
diagnosis, and why C now takes `--max-memory`: a ceiling declared before the
allocations that scale with the input, which on this pair is about 124 MB for
2m rows of CSV and about 1,008 MB for the 5m-row Parquet pair. It costs nothing
measurable -- 1.00s against 1.02s on the Parquet pair, which is noise.

## 2026-09-10 (C codecs) — the columnar path, on compressed files

Same container, same 5m-row pair, same method as the codec entry below: median
of five warm runs, every file written by one pyarrow call so the codec is the
only variable.

C read no codec at all when that entry was taken. It reads snappy and LZ4 now,
both written out in `c/parquet.c` rather than linked, and it reads them on the
columnar path -- which is the whole result:

| Port | none | snappy | lz4 |
|---|---:|---:|---:|
| **C** | **0.98s** | **1.61s** | **1.65s** |
| C++ | 1.64s | 2.08s | refused |
| Rust | 1.52s | 2.12s | 5.41s |
| Zig | 1.26s | 1.79s | 5.98s |

C is fastest on every codec it reads. The LZ4 column is the interesting one:
**3.3x Rust and 3.6x Zig**, and not because the decoder is better. Rust and Zig
have no LZ4 in their columnar readers, so an LZ4 file falls through to the row
reader and pays the 3.7x that costs. C decompresses into a buffer the columnar
path reads from, so the codec is the only thing it adds.

Decompression costs C about 0.63s on this pair, 64% over uncompressed, which is
more than the 10-19% the row readers pay for the same codecs. That is the same
arithmetic from the other side: the columnar path is fast enough that a codec is
a larger share of a smaller number.

Gzip and zstd are refused by name. Their decoders are real programs rather than
eighty-line loops, and this port carries no dependency.

### The copy loop, paired

Nine interleaved rounds of the same binary with one change -- the match copy
doing eight bytes at a time where the source is at least eight behind, instead
of one:

| Codec | Byte loop | Eight at a time | Paired ratio |
|---|---:|---:|---:|
| snappy | 1.62s | 1.52s | **0.936** (middle half 0.920-0.950) |
| lz4 | 1.61s | 1.53s | **0.951** (0.929-0.958) |

6.4% and 4.9%, with the middle half clear of 1.0 in both. Small, and the reason
it is worth the five lines is that it is free: a match whose distance is at
least eight reads only bytes already written, so the chunked copy means exactly
what the byte loop meant. A closer match is a repeating run and stays a byte
loop.

## 2026-09-09 (codecs) — what compression costs, and what the fall-through costs

One 4-core / 16 GB container, idle, page cache warm, median of five runs.
5,000,500 against 5,000,250 rows of 20 columns, keyed on `(account_id, txn_id)`,
`-i updated_at`. Every file was written by the same pyarrow call with the same
row-group size and dictionary setting, so within this set the codec is the only
thing that differs.

**What a default run costs**, which is what a reader actually meets:

| Port | none | snappy | gzip | zstd | lz4 |
|---|---:|---:|---:|---:|---:|
| C | 1.11s | refused | refused | refused | refused |
| C++ | 1.54s | 2.35s | refused | refused | refused |
| Rust | **1.48s** | 2.29s | 6.49s | 5.20s | 5.37s |
| Zig | 1.32s | **1.96s** | 7.38s | **13.96s** | 6.11s |

C's four refusals are of their time: it reads snappy and LZ4 now, and the entry
above has those numbers. Read the rest as four numbers and a trap. Rust and Zig take the columnar path for
uncompressed and snappy and fall through to the row reader for the other three,
so the 3.5-4x jump at gzip is **not** what gzip costs. It is the fall-through,
already measured at 3.7x in the entry below.

**What the codec costs**, with the reader held still — `--engine turbo` on all
five, so nothing routes:

| Codec | Wall | vs uncompressed | Bytes read | Smaller by |
|---|---:|---:|---:|---:|
| none | 5.17s | 1.00x | 1,073.1 MB | 1.00x |
| snappy | 5.67s | 1.10x | 530.3 MB | 2.02x |
| gzip | 6.17s | 1.19x | 344.2 MB | 3.12x |
| **zstd** | **5.15s** | **1.00x** | **281.0 MB** | **3.82x** |
| lz4 | 5.38s | 1.04x | 528.2 MB | 2.03x |

Decompression costs between nothing and 19% of wall time. **zstd costs nothing
measurable and reads 3.82x fewer bytes** — it was 1.00x on both runs of this,
taken an hour apart on a restarted container. On a host where bytes read cost
anything at all, that is not a trade-off, it is free.

So the practical answer to "what should I write?" is zstd, and the reason a zstd
pair looks expensive today is the router, not the codec.

**Zig's zstd is the exception, and it is a real one.** Its three fall-through
codecs are 7.38s (gzip), 6.11s (lz4) and 13.96s (zstd): zstd is **2.3x its own
lz4**, where Rust's zstd is its *cheapest* codec at 0.96x its lz4. Same files,
same machine. That is Zig's zstd decoder, not the format, and it is on the open
list now.

**Answered: it is std's decoder, and the wall time is not ours to take.** Two
suspects were priced and both came back small. Twenty-five gdb samples of a 1M
zstd pair land in `compress.zstd.Decompress.readInFrame`, `decodeLiterals`,
`ReverseBitReader` and `HuffmanTree.query` -- the decode loop itself, not the
reader around it. Sharing the 8.13 MB window across a column's pages instead of
allocating one per page is **1.03x cpu and no result on wall**. Sizing the
decode arena from `total_uncompressed_size` rather than the compressed one --
short by 17x on this fixture -- measured nothing at all. Two columns and six
cost the same 0.79s despite 3.2x the bytes, because the columns decode in
parallel and the critical path is the widest one.

The remaining difference is what the two ports link. Rust's zstd is
`zstd-sys` -- C libzstd 1.5.7, a decade of hand-tuning -- while Zig's is std's
pure-Zig decoder in a port that deliberately links no libc. Closing it means
either giving that up or writing a zstd decoder, so **the wall-clock half of
this item is closed, not open**. What the investigation did find was a memory
bug, below.

**A per-page window is invisible on the clock and 3x on the budget.** The same
1M zstd pair holds 291 MB of live data and needed `--max-memory 4800` to
finish, against 1600 once a column's pages share one window. This port runs on
a fixed buffer that hands memory back and cannot reuse it, so 146 pages of
8.13 MB windows are spent for good -- about 1.2 GB per file. The clock said 3%
and nearly buried it; the budget is where a per-page allocation shows up in
this port, and it is the number to check first next time.

### What this run cannot tell you

The bytes-saved half is unmeasured, and the honest reason is that this host
cannot measure it. `drop_caches` empties the guest's page cache but the
hypervisor still holds the blocks, so "cold" runs came back within 25% of warm
ones — measuring the host's cache, not a disk. The one genuine cold read
available, the first touch after a container restart, took **2m17s for 2.5 GB**,
about 18 MB/s, which is neither reproducible nor representative of anything.

So: the CPU side of the question is answered and the I/O side is not. On storage
where reading 1,073 MB rather than 281 MB costs real time, zstd's 3.82x wins by
however much that is worth; on this container it wins by nothing and loses by
nothing. Anyone with a characterisable disk can finish the other half.

Taken because "compression is unmeasured" had been on the open list since the
Parquet readers landed, and because timing it turned up a SIGSEGV in the Zig
port on every compressed file, which had to be fixed before three of these
twenty numbers existed at all.

## 2026-09-09 (parquet readers) — what the columnar path is worth

One 4-core / 16 GB container, `scripts/bench_ab.sh`, seven interleaved rounds,
2,000,000 rows of 20-column uncompressed Parquet, 154 MB a side. Two flag sets
of one binary: the default (the columnar path) against `--engine turbo`.

| Path | Wall best | Wall median | CPU best | CPU median |
|---|---:|---:|---:|---:|
| columnar (`parquet`) | **0.636s** | **0.665s** | **1.69s** | **1.74s** |
| `turbo` | 1.964s | 2.429s | 4.80s | 5.53s |

Paired per-round ratio 0.27x wall (middle half 0.26-0.29) and 0.31x CPU
(0.31-0.33). The columnar path is **3.7x** on wall and takes a third of the CPU,
which is the whole argument for keeping it: it joins on the key columns and
compares whole columns as integers without ever building a row, and `turbo`
decodes pages into rows to answer the same question.

Taken because the two readers had just stopped being interchangeable. A Parquet
pair used to go to the columnar path unconditionally — `--engine turbo` was
accepted and ignored — so a file it does not read (zstd, from a reader in the
field) was refused by a binary that reads zstd through `turbo`. The router now
falls through on a capability refusal. This is the number that says the
fall-through has to stay a fall-through: routing everything to `turbo` would
have cost 3.7x on every Parquet pair, and an earlier draft of that change did
exactly that by testing the *resolved* engine, where `auto` is already `Turbo`.

Counts agree between the two paths on the same pair, and between an
uncompressed pair and a zstd pair of the same rows — 199,800 matched, 12,123
changed, 200 added, 200 removed at 200k. That equality is what the routing is
allowed to lean on.

---

## 2026-09-09 (verification) — the same two changes, through `bench_ab.sh`

The `added` changes in both ports were measured with a hand-rolled interleaved
loop that compared the two builds' medians. `scripts/bench_ab.sh` landed the same
afternoon and compares them the right way -- paired within each round, on CPU,
with a verdict. Re-running both through it, one 4-core / 16 GB container:

| Change | Rounds | CPU ratio | Middle half | Verdict |
|---|---:|---:|---:|---|
| Rust, `added` from a bitmap | 9 | **1.14x** | 1.08–1.21 | 13% less work |
| Zig, `added` derived | 9 | 1.11x | 0.91–1.12 | **no result** |
| Zig, `added` derived | 25 | **1.29x** | 1.20–1.45 | 22% less work |

Both hold. The Zig row is the interesting one: at nine rounds the harness refused
to call it, and it was right to -- the run is 0.4s, so the same absolute noise is
a much larger share of it than in Rust's 0.7s. Twenty-five rounds separate them
cleanly. **How many rounds a comparison needs scales with how short the run is**,
and the harness saying "no result" is a request for more rounds before it is
evidence of anything.

The self-test says what this particular container can resolve, and it is not what
the harness's own notes assume:

| Warmup | Rounds | A/A wall | A/A cpu | cpu middle half |
|---:|---:|---:|---:|---:|
| 1 | 7 | 1.11x | 1.09x | 0.93–1.26 |
| 3 | 9 | 1.05x | 1.03x | 0.97–1.04 |

One warmup round is not enough here on a 350 MB pair: the first round is still
paying for the page cache, and it lands on whichever build runs first. Three
warmups bring the A/A test back to a straddling middle half — but the point
estimate stays near 1.03x, so **there is a systematic few per cent in favour of
whichever build runs second**, and a change measured at 3% on this machine has
not been measured at all.

That last figure retires a number this file might otherwise have kept: a scan
that replaced A's row parse measured -2.1% by the older method, which is inside
the bias. Recorded as "did not pay" in the entry below, and it stays there --
"no result" is the more accurate way to say it.

---

## 2026-09-09 (generators) — the two generators a reader can reach

One 4-core / 16 GB container, `scripts/bench_ab.sh`, seven interleaved rounds,
2,000,000 rows of 20-column CSV — 736 MB across the pair each time.

| Generator | Wall best | Wall median | CPU best | CPU median |
|---|---:|---:|---:|---:|
| `c/gen-data` | **1.05s** | **1.37s** | **2.41s** | **2.57s** |
| `rust/target/release/gen-data` | 5.49s | 5.90s | 5.06s | 5.43s |

Paired per-round ratio 0.23x wall (middle half 0.19-0.24) and 0.49x CPU
(0.48-0.54): the Rust generator does about twice the work and takes four times
the wall clock for it. The wall gap is wider than the CPU gap because the C one
renders rows in waves across all four cores — 2.41s of CPU inside 1.05s of wall
— while the Rust one is a single thread, 5.06s inside 5.49s.

The two write **byte-identical output**: `cmp` on both files of the pair at
2,000,000 rows, same `--seed` default. That matters more than the ratio. The
generator is the only way a Windows reader without WSL gets a pair to compare
at all, and it has to be the same pair the Linux tables were taken on, or the
numbers here stop meaning anything. `c/test.sh --with-ports` checks the C
generator against the C++ one on sixteen shapes including every thread count;
the Rust one is checked here by hand and by the `windows-latest` CI job, which
now runs the README's own generate-then-compare pair of commands.

So: the C generator for anything large, and it is what every table in this file
was taken on. The Rust one for Windows without WSL, or for a single toolchain —
at 10k or 100k rows the difference is under a tenth of a second and irrelevant.

---

## 2026-09-09 (after the `added` pass) — what a row parse costs, and a change that did not pay

One 4-core / 16 GB container, 1,000,000 rows, thirteen interleaved rounds,
`--max-rows 1` so the capped report does not sit in the denominator.

| Build | Median | Best |
|---|---:|---:|
| Rust, as merged | 0.2585s | 0.2521s |
| Rust, scanning A's run instead of parsing it | 0.2530s | 0.2389s |
| | **-2.1%** | -5.2% |

**Not kept.** Two per cent of the engine is about 1.7% of the Rust row at ten
million, which is inside what that benchmark can resolve, and the change costs a
const-generic split of the hottest loop in the port plus a second probe walk for
every pair the bytes do not settle.

The reasoning that led there was sound and the measurement is the useful part.
With the byte shortcut in place, a matched pair whose bytes agree needs one thing
from A's row — where the run of bytes ends — and the parse packs nineteen `Field`
words to find it that nobody then reads. That is 94% of pairs on this payload. A
scan that finds the same end and stores nothing should have been most of a parse
cheaper.

It is not, and that is the finding: **the scanning is what a row parse costs,
not the packing.** Isolating them says the same thing — a build that parses A and
stops runs the join phase in 0.072s against 0.059s for one that scans and stops,
so packing nineteen fields is 0.013s of a 0.137s phase. Against that, the 6% of
pairs the bytes do not settle now scan twice and probe twice, and the remainder
is small enough to argue with.

Where the join's time goes at one million rows, after the `added` pass was
removed, is now:

| Phase | | |
|---|---:|---:|
| row values (parallel) | 0.126s | 21% |
| join chunks (parallel) | 0.115s | 19% |
| assemble | 0.110s | 18% |
| sweep, A and B at once | 0.070s | 12% |
| index insert | 0.065s | 11% |
| sorts (serial) | 0.065s | 11% |
| changed cells (parallel) | 0.024s | 4% |

The join is no longer the largest thing in the run; building the report is. That
is a statement about one million rows and not about ten: the row sections are
capped at fifty thousand, so everything under "report" here is roughly constant
while the rest grows with the file. At ten million the same split reads 1.81s of
engine against 0.40s of report.

---

## 2026-09-09 (profiling) — three questions, and what the answers cost

One 4-core / 16 GB container. Not a table of ports against each other: three
things the README listed as open, measured until they stopped being open.

### 1. The serial index insert

The claim was that CPU over wall bounds the remaining parallelism at about
1.25x, and that pipelining the insert against the sweep had not been tried.
Both halves need correcting.

**The prize is real but small, and it shrinks with width.** The insert is
0.08–0.18s per file inside a ~0.47s run at 2M rows and 20 columns. At 200
columns it is **0.006s of a 0.43s run — 1.4%**.

**And it is blocked by allocation rather than by ordering**, which is why
nobody had tried it: `row_start`, `row_hash` and the table capacity are all
sized from the total row count, and that is only known once the last chunk is
swept. An insert cannot start on chunk 0 while chunk 3 is still being read,
because it has nowhere to put anything yet. Past that needs a counting pass
over the bytes first — about 0.35s at 10M against a 0.09s prize — or a size
estimate, which puts back the rehash that sizing-once removed.

### 2. ndjson, at 0.34 GB/s per core against CSV's 0.45

The format costs **1.46x more per byte**, and the standing hypothesis was the
per-field name lookup, which CSV does not do at all. An unsound build that
resolves fields positionally instead of by name prices it:

| | Wall | CPU |
|---|---:|---:|
| name lookup removed entirely | 1.10x | **1.14x** |

So 12% of CPU, against a 46% gap. It is not the explanation, and nothing else
structural is hiding either: what is left is the format — two delimiters to
scan for instead of one, escape handling, and a quote pair around every value.
ndjson is 2.4x the bytes of CSV for the same rows and costs 3.5x the CPU; the
1.46x between those is the format being self-describing, not a defect.

That number moved, and how it moved is worth keeping. The same probe measured
**9% before** the byte proof and the key-only parse landed. The lookup did not
get slower; everything around it got faster.

### 3. Wide files

200,000 rows at 20 columns against the same rows widened to 200, same keys,
same diffs — only the ratio of key work to cell work changes.

| | Input | Wall | CPU | Per core-second |
|---|---:|---:|---:|---:|
| 20 columns | 70 MB | 0.053s | 0.16s | 0.42 GB/s |
| 200 columns | 661 MB | 0.435s | 1.58s | **0.41 GB/s** |

**Throughput is flat across a tenfold change in width.** What changes is where
the time is: the join goes from about half the run to **74%**, the whole index
build falls to 24%, and the serial insert of question 1 becomes 1.4%.

**The SIMD question does not re-open there.** That was the reason to look: a
200-column row is 1.6 KB, and the byte proof scans nearly all of it. Widening
the compare from 8 bytes a step to 32 measured **no result at either width** —
so even where the scan is longest, the compare is not what the loop waits on.

### A trap worth naming

The first wide run reported `changed 199,800` of 199,800 matched. `--ignore
updated_at` named a column that does not exist in a widened file, where the
eleven copies are `updated_at_0` … `updated_at_10`; nothing was ignored, every
row differed, and the byte proof never fired. **All four ports accept an
`--ignore` name that matches nothing, in silence** — where `--key` with a name
that matches nothing is an error. The asymmetry is the trap: a missing key
makes the answer impossible, and a missing ignore quietly makes it wider.

---

## 2026-09-09 (joint run, fourth) — one tree, four ports

One GitHub Actions runner (4 vCPU / 16 GB), 10,000,000 rows × 20 columns, keyed
on `(account_id, txn_id)`, `--ignore updated_at`, five interleaved rounds each.
Every port from `82bf4c5`, compiled for the runner. Run `34365328117`.

**The first table with no second checkout.** The three above it measured this
tree against `claude/data-comparison-rust-zig-jam00m` out of a parallel
checkout, with all the provenance trouble that carried — a branch that moved
mid-run twice, and a pinned SHA to stop it. That branch is merged. Four ports,
one tree, one build configuration.

### CSV — input 3,509 MB

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **2.14s** | 2.15s | 2.19s | **7.7s** | 4,225 MB | **716 MB** |
| C++ | 5.19s | 5.35s | 5.49s | 16.2s | 4,387 MB | 878 MB |
| Rust | 2.46s | 2.47s | 2.51s | 8.2s | 4,408 MB | 899 MB |
| Zig | 2.71s | 2.72s | 2.90s | 9.7s | 4,396 MB | 887 MB |

### ndjson — input 8,487 MB

Four ports, where every earlier table had two of them missing: Rust and Zig
could not read ndjson before the merge.

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **7.40s** | 7.44s | 7.48s | **24.3s** | 9,204 MB | **716 MB** |
| C++ | 20.53s | 20.78s | 21.08s | 67.8s | 9,367 MB | 880 MB |
| Rust | 14.32s | 14.36s | 14.43s | 55.5s | 9,387 MB | 899 MB |
| Zig | 13.88s | 13.97s | 14.00s | 54.3s | 9,377 MB | 890 MB |

### Parquet — input 2,074 MB, uncompressed

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **1.48s** | 1.49s | 1.53s | **4.8s** | 3,371 MB | 1,297 MB |
| C++ | 3.43s | 3.44s | 3.53s | 11.3s | 3,554 MB | 1,480 MB |
| Rust | 2.80s | 2.82s | 2.84s | 9.1s | 3,548 MB | 1,474 MB |
| Zig | 2.96s | 2.97s | 2.98s | 10.5s | 3,349 MB | **1,276 MB** |

Counts agree across all four in every table: matched 9,990,000, changed
599,320, added 10,000, removed 10,000, duplicate keys 1,000 in A and 500 in B.

**What the day did to the C++ column.** It was the port dragging every table
down — 17.51s on CSV in the morning's run, and 7.96s for the better of the two
branches' versions. It is 5.19s here, after the merge took the quicker one and
four changes brought it up to the current design. Those changes measured 2.24x
of CPU on this host at two million rows; this is the same direction at ten
million, in a table that gates on counts.

**And the ranking that is now interesting.** On CSV, C leads Rust by 1.15x and
spends 7.7 CPU-seconds against its 8.2. Two ports that started this project
orders of magnitude apart are within noise of each other on the format most
people have. ndjson is where the ports still differ by a factor — 1.9x — and
the reason is one specific thing: the byte proof settles a CSV row from its raw
bytes, and settling a JSON one first has to rule out a repeated name, which
only the C port does.

---

## 2026-09-09 (joint run, third) — the first one built fairly

One GitHub Actions runner (4 vCPU / 16 GB), 10,000,000 rows × 20 columns, keyed
on `(account_id, txn_id)`, `--ignore updated_at`, five interleaved rounds each.
This tree at `ef796d0`; the pinned columns are
`claude/data-comparison-rust-zig-jam00m` at commit `c12f102`, checked out beside
it and built on the same runner. Run `34345272610`.

**Every port compiled for the runner**: `-march=native` for C and C++,
`-C target-cpu=native` for Rust, `-Dcpu=native` for Zig. The three tables above
this one were not — Rust and Zig were built for a generic baseline, which
compiles their wide scanners out — so this is the first joint table whose
cross-port ratios mean what they say.

### CSV — input 3,509 MB

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **1.97s** | 2.00s | 2.06s | **6.7s** | 4,225 MB | **716 MB** |
| C++ | 17.51s | 17.89s | 18.17s | 53.9s | 4,437 MB | 929 MB |
| Rust | 27.27s | 27.52s | 27.85s | 27.3s | 4,416 MB | 907 MB |
| Zig | 17.29s | 17.59s | 17.90s | 30.6s | 4,234 MB | 725 MB |
| C++ (c12f102) | 7.96s | 8.22s | 8.34s | 23.0s | 4,390 MB | 881 MB |
| Rust (c12f102) | 2.81s | 2.88s | 3.07s | 9.5s | 4,408 MB | 899 MB |
| Zig (c12f102) | 2.80s | 3.06s | 3.82s | 9.9s | 4,395 MB | 886 MB |

### ndjson — input 8,487 MB

Five builds, not seven: this tree's Rust and Zig do not read ndjson, and the
harness drops a build that cannot read the pair rather than timing a failure.

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **6.05s** | 6.17s | 6.29s | **19.6s** | 9,204 MB | **716 MB** |
| C++ | 18.13s | 18.36s | 19.36s | 52.4s | 9,383 MB | 895 MB |
| C++ (c12f102) | 17.68s | 18.03s | 18.44s | 50.7s | 9,411 MB | 924 MB |
| Rust (c12f102) | 9.95s | 10.22s | 10.60s | 38.1s | 9,387 MB | 899 MB |
| Zig (c12f102) | 9.07s | 9.25s | 9.49s | 35.1s | 9,369 MB | 882 MB |

### Parquet — input 2,074 MB, uncompressed

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **1.67s** | 1.70s | 1.72s | **4.8s** | 3,288 MB | 1,214 MB |
| C++ | 3.15s | 3.19s | 3.27s | 9.7s | 3,250 MB | 1,177 MB |
| Rust | 3.75s | 3.81s | 3.85s | 10.6s | 3,299 MB | 1,225 MB |
| Zig | 7.27s | 7.33s | 7.43s | 10.4s | 3,208 MB | 1,134 MB |
| C++ (c12f102) | 3.17s | 3.25s | 3.33s | 9.7s | 3,250 MB | 1,176 MB |
| Rust (c12f102) | 2.46s | 2.51s | 2.55s | 7.3s | 3,305 MB | 1,232 MB |
| Zig (c12f102) | 2.17s | 2.21s | 2.30s | 7.2s | 3,202 MB | **1,129 MB** |

Counts agree across every build in every table: matched 9,990,000, changed
599,320, added 10,000, removed 10,000, duplicate keys 1,000 in A and 500 in B.

**What fair flags cost the headline.** The C lead over the next build, before
and after — same tree, same rows, same runner class, the only change being that
Rust and Zig are now compiled for the machine they run on:

| Format | Lead as published | Lead now |
|---|---:|---:|
| CSV | 2.12x | **1.42x** |
| ndjson | 1.45x | 1.50x |
| Parquet | 1.91x | **1.30x** |

Two thirds of the CSV margin and two thirds of the Parquet margin were build
flags. ndjson is the exception, and only because both sides moved at once: the C
byte proof landed in this tree and a key-only join landed in theirs.

**What is still C's.** The memory column on both text formats, by 165 MB and
more — a field there is one 64-bit word and never becomes a string. And the CPU
column everywhere: 1.5x less work than the next build on CSV, 1.8x on ndjson,
1.5x on Parquet. On Parquet that 1.5x of work shows up as only 1.30x of wall,
which is their Zig using four cores better than this one does.

**Do not compare this table with the three above it.** Different build flags on
half the columns, and this runner's ndjson numbers moved 20% between two runs
this morning with no code change at all.

---

## 2026-09-09 (joint run, second) — the byte proof at ten million rows

One GitHub Actions runner (4 vCPU / 16 GB), 10,000,000 rows × 20 columns, keyed
on `(account_id, txn_id)`, `--ignore updated_at`, five interleaved rounds each.
C and this tree's C++/Rust/Zig from `c65f23b`; the `(jam00m)` columns from
`claude/data-comparison-rust-zig-jam00m` at `168ab59`, checked out beside it and
built on the same runner. Run `34315176054`.

### CSV — input 3,509 MB

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **2.05s** | 2.06s | 2.09s | **7.4s** | 4,225 MB | **716 MB** |
| C++ | 19.77s | 19.93s | 20.05s | 63.3s | 4,391 MB | 882 MB |
| Rust | 35.57s | 36.31s | 37.34s | 35.6s | 4,420 MB | 911 MB |
| Zig | 19.99s | 20.08s | 20.45s | 34.4s | 4,234 MB | 725 MB |
| C++ (jam00m) | 10.16s | 10.20s | 10.40s | 32.6s | 4,391 MB | 882 MB |
| Rust (jam00m) | 5.23s | 5.26s | 5.31s | 19.3s | 4,408 MB | 899 MB |
| Zig (jam00m) | 4.35s | 4.38s | 4.52s | 16.4s | 4,396 MB | 887 MB |

### ndjson — input 8,487 MB

Five builds, not seven: this tree's Rust and Zig do not read ndjson, and the
harness drops a build that cannot read the pair rather than timing a failure.

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **10.32s** | 10.34s | 10.40s | **36.3s** | 9,204 MB | **717 MB** |
| C++ | 27.32s | 27.50s | 27.85s | 84.4s | 9,413 MB | 926 MB |
| C++ (jam00m) | 26.11s | 26.19s | 26.42s | 80.8s | 9,388 MB | 901 MB |
| Rust (jam00m) | 16.14s | 16.18s | 16.77s | 62.8s | 9,387 MB | 899 MB |
| Zig (jam00m) | 14.95s | 15.13s | 15.16s | 58.8s | 9,375 MB | 888 MB |

### Parquet — input 2,074 MB, uncompressed

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **1.40s** | 1.41s | 1.43s | **4.6s** | 3,371 MB | 1,298 MB |
| C++ | 3.24s | 3.27s | 3.30s | 10.6s | 3,553 MB | 1,480 MB |
| Rust | 4.04s | 4.11s | 4.14s | 11.8s | 3,396 MB | 1,322 MB |
| Zig | 6.72s | 6.88s | 7.14s | 11.3s | 3,387 MB | 1,313 MB |
| C++ (jam00m) | 3.24s | 3.29s | 3.32s | 10.7s | 3,553 MB | 1,480 MB |
| Rust (jam00m) | 2.67s | 2.72s | 2.76s | 8.8s | 3,395 MB | 1,322 MB |
| Zig (jam00m) | 2.72s | 2.74s | 2.75s | 9.7s | 3,319 MB | **1,245 MB** |

Counts agree across every build in every table: matched 9,990,000, changed
599,320, added 10,000, removed 10,000, duplicate keys 1,000 in A and 500 in B.

**What this run is for.** The byte proof landed between it and the run three
hours earlier, and CSV is where it shows: **3.06s → 2.05s with CPU 11.1s →
7.4s**, taking C's lead over the next build from 1.37x to 2.12x on the format
that carries the most bytes per row of work.

**And what it is a warning about.** Do not read the other two formats that way.
Against the earlier run, *every* build's ndjson number here is about 20% slower
and *every* build's Parquet number a few percent faster — C, C++, Rust and Zig
alike, from two branches, with no change to any of those ports. That is the
runner, not the code, and it is exactly why the rule at the top of this file
says to compare rows within a table and never across tables. The CSV claim above
survives only because the change is 1.5x and the drift is 20%; a 20% claim made
the same way would be worth nothing.

---

## 2026-09-09 (later still, again) — proving a row unchanged from its bytes

One 4-core / 16 GB container, 2,000,000 rows, every binary from this tree,
interleaved rounds.

| CSV, 702 MB | Before | After | | CPU before | CPU after |
|---|---:|---:|---:|---:|---:|
| `--ignore updated_at` (the benchmark shape) | 0.705s | **0.476s** | 1.48x | 2.32s | **1.53s** |
| nothing ignored, so the proof always refuses | 0.929s | 0.867s | — | 3.04s | 2.79s |

The join looked up a mate and then parsed its row a second time and compared all
seventeen columns field by field. Rows that match usually match because they are
the same row with one column moved, so comparing the two rows' raw bytes from
the front — a word at a time — proves every compared column equal when the
agreement reaches the byte that closes the last of them, and the mate is never
parsed. On this pair that is 94% of matched rows.

The second row is the shape the proof cannot help: `updated_at` compared rather
than ignored means every row really has changed, and the scan is work for
nothing. After 64 refusals in a row it is attempted only every 64th row. Two
sittings put that case at 3.6% of CPU either side of parity — it is inside this
host's noise, which is what the backoff is for; without it the loss was 5% and
repeatable.

The proof refuses where equal bytes would not mean equal columns: two headers
ordering the same columns differently, a mate whose field carries on where this
one stopped (`cc` against `cccccccc`, or a quoted field the mate continues with
a doubled quote), and JSON, where a value is found by name and a repeated name
takes its last value — a duplicate past the diverging byte would carry a value
the prefix never saw. Each has a test, and each guard was checked by removing it
and watching the test fail.

Counts unchanged across 30 generated shapes (CSV and ndjson), the awkward
fixture, and the cross-port suite with the Rust port as the oracle. That suite
is what caught the `cc`/`cccccccc` case; the fast suite now catches it too.

### The shape this project was not measuring

Every table here compares two generated sides where a timestamp column moves on
every row and is ignored. The other common shape is two snapshots of one table:
compare every column, and most rows are untouched byte for byte. Same host, same
sitting, eleven rounds — 2,000,000 rows, 5% of them carrying one changed cell,
nothing ignored:

| | Wall | CPU |
|---|---:|---:|
| before the proof (`5498bc4`) | 0.794s | 2.68s |
| with it (`3ecbab1`) | **0.538s** | **1.74s** |

1.48x, which is the same win the benchmark shape gets, for the same reason: the
proof does not care *why* the rows agree.

### The day's five C changes, one sitting

Each commit built from its own tree and run against the others, seven
interleaved rounds, best of each. This is the only honest way to add them up:
the four entries below were each measured against their own predecessor in their
own sitting, and those numbers do not chain.

| 2,000,000 rows | CSV | CPU | ndjson | CPU |
|---|---:|---:|---:|---:|
| day start (`22cc05b`) | 1.174s | 3.56s | 3.472s | 11.95s |
| + ndjson framed on the newline (`b8dbf44`) | 1.169s | 3.56s | 2.110s | 6.79s |
| + a tag in the text index's slot (`b3ae48e`) | 0.835s | 2.79s | 1.867s | 6.06s |
| + `added` derived (`5498bc4`) | 0.705s | 2.32s | 1.750s | 5.50s |
| + the byte proof (`3ecbab1`) | **0.476s** | **1.53s** | 1.785s | 5.69s |
| | **2.47x** | **2.33x** | **1.95x** | **2.10x** |

None of it is from threading harder — CPU falls with wall throughout. The byte
proof does nothing for ndjson by design, and its last ndjson row is that: noise
around no change.

---

## 2026-09-09 (joint run) — ten million rows, seven builds, one runner

One GitHub Actions runner (4 vCPU / 16 GB), 10,000,000 rows × 20 columns, keyed
on `(account_id, txn_id)`, `--ignore updated_at`, five interleaved rounds each.
C and this tree's C++/Rust/Zig from `5498bc4`; the `(jam00m)` columns from
`claude/data-comparison-rust-zig-jam00m` at `168ab59`, checked out beside it and
built on the same runner. Run `34311464257`.

### CSV — input 3,509 MB

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **3.06s** | 3.06s | 3.11s | **11.1s** | 4,225 MB | **716 MB** |
| C++ | 18.67s | 18.94s | 19.38s | 59.4s | 4,428 MB | 919 MB |
| Rust | 32.03s | 32.22s | 32.40s | 32.0s | 4,416 MB | 907 MB |
| Zig | 21.74s | 21.84s | 21.93s | 37.8s | 4,234 MB | 725 MB |
| C++ (jam00m) | 8.90s | 8.97s | 8.99s | 27.5s | 4,391 MB | 882 MB |
| Rust (jam00m) | 4.94s | 4.95s | 4.96s | 17.9s | 4,408 MB | 899 MB |
| Zig (jam00m) | 4.20s | 4.20s | 4.59s | 15.6s | 4,397 MB | 888 MB |

### ndjson — input 8,487 MB

Five builds, not seven: this tree's Rust and Zig do not read ndjson, and the
harness drops a build that cannot read the pair rather than timing a failure.

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **8.59s** | 8.61s | 8.70s | **29.3s** | 9,204 MB | **716 MB** |
| C++ | 22.04s | 22.37s | 22.68s | 66.2s | 9,404 MB | 917 MB |
| C++ (jam00m) | 21.18s | 21.42s | 21.67s | 64.0s | 9,374 MB | 887 MB |
| Rust (jam00m) | 13.17s | 13.19s | 13.25s | 50.9s | 9,387 MB | 899 MB |
| Zig (jam00m) | 12.62s | 13.18s | 13.22s | 49.5s | 9,374 MB | 886 MB |

### Parquet — input 2,074 MB, uncompressed

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| C | **1.67s** | 1.75s | 1.77s | **5.5s** | 3,377 MB | 1,303 MB |
| C++ | 3.31s | 3.34s | 3.35s | 10.9s | 3,553 MB | 1,480 MB |
| Rust | 3.96s | 3.98s | 4.01s | 11.6s | 3,395 MB | 1,321 MB |
| Zig | 6.60s | 6.74s | 6.76s | 10.6s | 3,421 MB | 1,347 MB |
| C++ (jam00m) | 3.19s | 3.22s | 3.31s | 10.4s | 3,478 MB | 1,404 MB |
| Rust (jam00m) | 2.95s | 2.98s | 3.01s | 9.9s | 3,396 MB | 1,322 MB |
| Zig (jam00m) | 2.83s | 2.86s | 2.87s | 10.0s | 3,292 MB | **1,218 MB** |

Counts agree across every build in every table: matched 9,990,000, changed
599,320, added 10,000, removed 10,000, duplicate keys 1,000 in A and 500 in B.

**What moved since the 2026-09-08 joint run** (the only other run at this size
on this harness, so the only one these are comparable with):

| Format | C then | C now | | Best rival then | Best rival now |
|---|---:|---:|---:|---|---|
| CSV | 4.16s | **3.06s** | 1.36x | Zig 4.55s | Zig 4.20s |
| ndjson | 20.13s | **8.59s** | 2.34x | Zig 15.58s | Zig 12.62s |
| Parquet | 1.63s | 1.67s | — | Zig 4.42s | Zig 2.83s |

C's ndjson row went from last of four to first of five, and the format C led by
2.7x on Parquet it now leads by 1.7x — their Zig closed 4.42s to 2.83s in the
same day. Parquet's 1.63s → 1.67s is inside this harness's noise at 10M and is
not read as a regression; the same binary's Parquet path measured 1.44x quicker
at two million rows the same morning.

---

## 2026-09-09 (later still) — the second join pass, removed

One 4-core / 16 GB container, 2,000,000 rows, five interleaved rounds.

| Format | Before | After | | CPU before | CPU after |
|---|---:|---:|---:|---:|---:|
| CSV, 702 MB | 0.89s | **0.68s** | 1.31x | 3.0s | **2.3s** |
| Parquet, 415 MB | 0.62s | **0.43s** | 1.44x | 1.7s | **1.2s** |

Both engines looked every key of B up in A, over a second random-probed table,
to count `added` — a number the A pass already implies, since `added` is B's
distinct keys minus `matched`. Counts agree with four other ports on both
formats, and `CSVDIFF_VERIFY_ADDED=1` runs the removed pass to check the
derivation rather than trusting it.

The CSV path across the day's changes was first written up here by chaining
three sittings, which is the one thing this file says not to do. The entry above
replaces it with all five commits built and run against each other in one
sitting.

---

## 2026-09-09 (later) — a tag in the text index's slot

One 4-core / 16 GB container, 2,000,000 rows, five interleaved rounds, both
binaries from the same tree.

| Format | Before | After | | CPU before | CPU after | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| CSV, 702 MB | 1.07s | **0.85s** | 1.26x | 3.5s | **2.7s** | 126 MB, unchanged |
| ndjson, 1,697 MB | 2.31s | **1.93s** | 1.20x | 7.2s | **6.3s** | 126 MB, unchanged |

The Parquet index has held a tag beside the position for weeks; the text index
held only the position, so rejecting a collision cost two further dependent
misses. The slot is still four bytes — the width is taken from the row count,
24 bits of position and 8 of tag at ten million rows — so the memory column does
not move, which is the column this port leads on.

Counts agree across every port in both tables.

---

## 2026-09-09 — the other branch's Parquet, on its own runner

Not this project's harness and **not comparable with the tables below** — one
tree, its own workflow (`bench-10m.yml`, since split into `bench-2m.yml` and
`bench-ladder.yml`), its own runner. Recorded because they
are real measurements of ports this project's table has columns for, and because
the next joint run will want a baseline to be surprised against.

Ten million rows, uncompressed Parquet, from
`claude/data-comparison-rust-zig-jam00m`:

| Build | Before | After | | Run |
|---|---:|---:|---:|---|
| Zig columnar | 4.627s | 3.624s | −21.7% | [22](https://github.com/andrey-usa/csvdiff/actions/runs/34304998876) |
| Rust columnar | 5.711s | 3.688s | −35.4% | [23](https://github.com/andrey-usa/csvdiff/actions/runs/34305285040) |

Both from prefetching the index probe — the change this port measured at
1.79s → 1.39s on its build and 1.21s → 0.88s on its join. Their write-up notes
one divergence worth keeping: **huge pages on the slot table are a loss on their
host**, +0.7% to +3.2% on wall with CPU down 3%, where this port measured
1.38s → 0.59s. Two machines, one change, opposite verdicts. Same shape as the
AVX-512 result, and a reason neither of us should commit that advice as general.

For where those ports stood against this one on a single runner, see the joint
run below; it is the last measurement in which all four were built together.

---

## 2026-09-09 (later) — the C ndjson path

One 4-core / 16 GB container, 2,000,000 rows, five interleaved rounds, both
binaries from the same tree minutes apart.

| Format | Build | Best | Median | CPU |
|---|---|---:|---:|---:|
| ndjson, 1,697 MB | **after** | **2.18s** | 2.20s | **6.9s** |
| ndjson | before | 3.45s | 3.58s | 11.9s |
| CSV, 702 MB | after | 1.19s | 1.31s | 3.7s |
| CSV | before | 1.27s | 1.36s | 3.9s |

**1.58x wall and 1.72x CPU on ndjson; CSV unchanged**, which is the control —
neither change is on that path.

Phase timings, added to the text path in this change, are what found it:

| Phase | CSV | ndjson | ndjson / CSV |
|---|---:|---:|---:|
| sweep rows (per side) | 0.23s | 0.83s | **3.2x** |
| insert in order | 0.27s | 0.27s | 1.0x |
| join and compare | 0.82s | 2.03s | 2.5x |

ndjson is 2.42x the bytes. The insert never touches the file and is identical;
the join is in proportion; the sweep was the outlier, and it was the row scanner
rather than the field parsing.

---

## 2026-09-09 — seven builds against the other branch's rewritten ports

GitHub Actions `ubuntu-latest`, 4 vCPU / 16 GB.
[Run 34291857114](https://github.com/andrey-usa/csvdiff/actions/runs/34291857114)
at `7aef55a`, alt ref `claude/data-comparison-rust-zig-jam00m` at `0569a39` —
ten commits on from the run below, with both ports' index and join rewritten and
AVX2 added. Five interleaved rounds each.

`Rust` and `Zig` without a suffix are this tree's, which is `main`'s: they are
kept in the table because leaving a superseded build out is how a project talks
itself into a number, but they are not the current state of those ports and the
`(alt)` rows are.

### CSV — 3,509 MB

| Build | Best | Median | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|
| **C** | **4.16s** | 4.21s | **14.9s** | 4,225 MB | **716 MB** |
| Zig (alt) | 4.55s | 4.56s | 17.1s | 4,396 MB | 887 MB |
| Rust (alt) | 5.68s | 5.69s | 19.9s | 4,408 MB | 899 MB |
| C++ (alt) | 10.12s | 10.18s | 32.2s | 4,445 MB | 936 MB |
| C++ | 20.15s | 20.18s | 63.7s | 4,392 MB | 883 MB |
| Zig *(superseded)* | 21.19s | 21.28s | 36.6s | 4,233 MB | 724 MB |
| Rust *(superseded)* | 39.59s | 39.62s | 39.6s | 4,435 MB | 927 MB |

### newline-delimited JSON — 8,487 MB

| Build | Best | Median | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|
| **Zig (alt)** | **15.58s** | 15.71s | **61.2s** | 9,367 MB | 879 MB |
| Rust (alt) | 16.14s | 16.17s | 61.6s | 9,387 MB | 899 MB |
| C | 20.13s | 20.38s | 74.5s | 9,204 MB | **717 MB** |
| C++ (alt) | 26.36s | 26.52s | 81.1s | 9,368 MB | 880 MB |
| C++ | 27.40s | 27.49s | 84.0s | 9,373 MB | 885 MB |

`Rust` and `Zig` from this tree refuse ndjson and are absent by their own report.

### uncompressed Parquet — 2,074 MB

| Build | Best | Median | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|
| **C** | **1.63s** | 1.66s | **5.4s** | 3,373 MB | 1,299 MB |
| C++ | 3.28s | 3.29s | 10.9s | 3,555 MB | 1,482 MB |
| C++ (alt) | 3.28s | 3.29s | 10.8s | 3,554 MB | 1,481 MB |
| Rust (alt) | 3.79s | 3.82s | 11.7s | 3,396 MB | 1,322 MB |
| Rust *(superseded)* | 4.23s | 4.24s | 12.1s | 3,409 MB | 1,336 MB |
| Zig (alt) | 4.42s | 4.43s | 12.1s | 3,372 MB | 1,299 MB |
| Zig *(superseded)* | 7.16s | 7.26s | 12.1s | 3,405 MB | 1,331 MB |

All seven agree, same counts as every run above.

### What this run said

**C keeps CSV, by 9%.** 4.16s against their rewritten Zig's 4.55s, where three
hours earlier the gap was 1.73x the other way. Their Zig went 8.15s to 4.55s in
the same span.

**C lost ndjson**, and to the thing it was warned about: their Zig does it in
15.58s against C's 20.13s. The C JSON path still walks every object to its
closing brace, which is the open item the parse work could not remove.

**C keeps Parquet by 2x**, on the least CPU of any build in the table.

**The alt build now takes 63 seconds, not 21 minutes,** because the workflow
stopped compiling a bundled DuckDB and the polars chain for a job that runs
`--engine turbo`. The whole run went from 72 minutes to 31.

---

## 2026-09-08 (later still) — the C generator, on every core

One 4-core / 16 GB container, 2,000,000 rows, nine interleaved rounds, both
binaries built from the same tree minutes apart.

| Format | Bytes written | Before | After | | CPU before | CPU after |
|---|---:|---:|---:|---:|---:|---:|
| CSV | 702 MB | 2.67s | **1.17s** | 2.28x | 2.66s | **2.61s** |
| ndjson | 1,697 MB | 5.39s | **1.91s** | 2.82x | 5.37s | **4.70s** |
| Parquet | 415 MB | 6.06s | **2.61s** | 2.32x | 6.06s | 6.22s |

Where the Parquet parallelism actually is, measured separately in one sitting:

| Parquet, what is threaded | Wall |
|---|---:|
| nothing | 6.49s |
| the columns of a row group | 5.28s |
| the two sides | 3.26s |
| both | **2.91s** |

**2.3-2.8x with CPU flat or down.** Threading normally buys elapsed time with
total work; this did not, because the restructure also stopped the row loop
fetching its parameters from a struct on every row — at one thread the new code
already beats the old serial code.

**Splitting a row group's columns is worth only 1.28x**, which is a quarter of
what is there: most of a Parquet run is the serial feeding of forty million cell
values into the column arenas, not the encoding of them. The two sides at once
are worth 2.0x on their own.

**No C++ column, deliberately.** This host cannot measure that generator
consistently — the same binary writing the same CSV came out anywhere from 0.78s
to 3.84s across sittings. The earlier C-against-C++ generator table published in
`c/README.md` is withdrawn for that reason and for a second one: its Parquet row
compared C++'s snappy output against this port's uncompressed, 199 MB against
415 MB, because snappy is that generator's default.

Byte identity against the C++ generator holds across sixteen shapes, including
thread counts of 1, 3, 4 and 7, a row count ending inside a wave, and a single
row. Eight are in `c/test.sh --with-ports`.

---

## 2026-09-08 (later) — the same seven builds, after the C parse work

Same workflow, same seven builds, same rows, one runner over.
[Run 34282793806](https://github.com/andrey-usa/csvdiff/actions/runs/34282793806)
at `c2d6aa4`, five interleaved rounds each. The alt ref was unchanged at
`168ab59`, so its four columns are the same binaries as the run below and the
5-8% they moved is this pair of runners disagreeing — which is the useful
number to have when reading the one column that moved by 3.65x.

### CSV — 3,509 MB

| Build | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **4.06s** | 4.10s | 4.11s | **14.5s** | 4,225 MB | **716 MB** |
| Rust (alt) | 8.13s | 8.20s | 8.24s | 23.6s | 4,717 MB | 1,208 MB |
| Zig (alt) | 8.15s | 8.20s | 8.25s | 22.4s | 4,419 MB | 910 MB |
| C++ (alt) | 10.23s | 10.26s | 10.36s | 32.8s | 4,393 MB | 884 MB |
| C++ | 19.87s | 19.94s | 20.09s | 63.6s | 4,395 MB | 887 MB |
| Zig | 20.12s | 20.17s | 20.31s | 34.7s | 4,235 MB | 726 MB |
| Rust | 37.25s | 37.48s | 37.62s | 37.2s | 4,435 MB | 927 MB |

### newline-delimited JSON — 8,487 MB

| Build | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **20.37s** | 20.46s | 20.60s | 74.8s | 9,204 MB | **716 MB** |
| Zig (alt) | 22.14s | 22.14s | 22.31s | **67.5s** | 9,417 MB | 929 MB |
| Rust (alt) | 22.85s | 22.97s | 23.15s | 70.7s | 9,696 MB | 1,208 MB |
| C++ (alt) | 26.12s | 26.14s | 26.40s | 80.7s | 9,371 MB | 884 MB |
| C++ | 27.39s | 27.41s | 27.58s | 84.7s | 9,370 MB | 882 MB |

`Rust` and `Zig` from this tree refused the pair again: they do not read ndjson.

### uncompressed Parquet — 2,074 MB

| Build | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **1.55s** | 1.66s | 1.68s | **5.2s** | 3,307 MB | **1,233 MB** |
| C++ | 3.22s | 3.25s | 3.28s | 10.6s | 3,553 MB | 1,480 MB |
| C++ (alt) | 3.25s | 3.28s | 3.32s | 10.7s | 3,554 MB | 1,480 MB |
| Rust (alt) | 3.58s | 3.59s | 3.64s | 11.1s | 3,411 MB | 1,338 MB |
| Rust | 3.96s | 3.98s | 4.02s | 11.5s | 3,411 MB | 1,337 MB |
| Zig (alt) | 4.11s | 4.12s | 4.15s | 11.5s | 3,420 MB | 1,346 MB |
| Zig | 6.86s | 6.91s | 6.96s | 11.5s | 3,385 MB | 1,312 MB |

All seven agree, same counts as every run above.

### What this run said

**The C CSV path went 14.82s to 4.06s — 3.65x, where two million rows had
predicted 2.66x.** CPU fell 57.3s to 14.5s. The gain grew with the size because
the parses removed were not only instructions: at ten million rows the index no
longer fits in cache, and a parse that touches twenty fields instead of two
touches memory it then has to get back.

**It is now first on all three formats,** and by the measure that is hardest to
argue with: on CSV it uses 14.5 CPU-seconds where the next build uses 22.4, so
it is not winning on threading. Three hours earlier it was last-but-two on CSV
with 57.3.

**JSON is now the laggard.** It is 2.42x the CSV's bytes and 5.0x its time,
where before this change it was 1.55x. Nothing about the JSON path got worse —
CSV got out from under it, and the object walk this change could not remove is
what is left.

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
