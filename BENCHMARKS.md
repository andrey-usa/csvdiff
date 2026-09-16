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

**Counts are gated, not assumed.** Every run below ends with every build
returning identical counts. A build that disagrees fails the run and is named;
none of the tables here contains a build that was fast because it was answering
a different question.

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
