# csvdiff

Compare two tables on a composite key and get the counts, the per-column
statistics and — from the Rust port — a self-contained HTML report. Key columns,
compared columns and normalisation rules are parameters, so one tool serves
every recurring comparison.

It is **four byte-level ports to one result contract** — C, C++, Rust and Zig.
They read the same files, return the same counts and the same exit codes, which
is what makes a number from one directly comparable with a number from another.

- [BENCHMARKS.md](BENCHMARKS.md) — every run, with CPU and memory
- [ARCHIVE.md](ARCHIVE.md) — what was tried, what it was worth, what was removed

---

## The example

Ten million rows × 20 columns, keyed on `(account_id, txn_id)`, `--ignore
updated_at`, a per-cell diff over seventeen columns. One GitHub Actions runner
(4 vCPU / 16 GB), every port compiled for that runner, all builds interleaved in
one sitting, five rounds each, 2026-09-09. Each cell is **wall · CPU · memory
above the mapped input**, for each port's fastest build.

| Format | Input | C | C++ | Rust | Zig |
|---|---:|---|---|---|---|
| CSV | 3,509 MB | **1.97s** · 6.7s · 716 MB | 7.96s · 23.0s · 881 MB | 2.81s · 9.5s · 899 MB | 2.80s · 9.9s · 886 MB |
| ndjson | 8,487 MB | **6.05s** · 19.6s · 716 MB | 17.68s · 50.7s · 924 MB | 9.95s · 38.1s · 899 MB | 9.07s · 35.1s · 882 MB |
| Parquet | 2,074 MB | **1.67s** · 4.8s · 1,214 MB | 3.15s · 9.7s · 1,177 MB | 2.46s · 7.3s · 1,232 MB | 2.17s · 7.2s · **1,129 MB** |

All builds returned identical counts — matched 9,990,000, changed 599,320, added
10,000, removed 10,000, duplicate keys 1,000 in A and 500 in B. That is the
run's gate, not a footnote: builds that disagree about how many rows changed
mean a bug in one of them, so the run fails and names it.

Three things this table says.

**The C lead is real and it is smaller than this project used to claim.** CSV by
1.42x and Parquet by 1.30x, where the tables published earlier the same day said
2.12x and 1.91x. The difference is not code: C and C++ had carried
`-march=native` since the first table while Rust and Zig were built for a
generic baseline, which compiles their wide scanners out entirely. Every port is
now built for the machine it runs on. The lead that survives that is the one
worth having.

**Parquet is still a different problem,** but a narrower one: 1.67s against
1.97s for the same rows in CSV on three-fifths of the bytes, because with both
dictionaries interned into one id space a cell comparison becomes
`int32 != int32`. It is also the one format where C does not hold the memory
column — Zig's reader peaks 85 MB lower.

**Read the CPU column.** Wall time mixes work with how many cores a design
manages to use; CPU seconds do not. C does 1.5x less work than the next build on
CSV and 1.8x less on ndjson, which is where the byte proof shows: most matched
rows are settled from their raw bytes without either side being parsed. On
Parquet it leads by 1.5x on CPU but only 1.30x on wall — that gap is Zig using
the four cores better, not doing less.

> **Where the columns come from.** C and this tree's C++ are `ef796d0`. The
> Rust, Zig and best C++ columns are `claude/data-comparison-rust-zig-jam00m`
> pinned at commit `c12f102`, checked out beside this tree and built on the same
> runner — pinned to a commit rather than a branch name because that branch
> moved mid-run twice while this table was being produced.
>
> Both trees are built the same way: `-march=native` for C and C++,
> `-C target-cpu=native` for Rust, `-Dcpu=native` for Zig. Each tree keeps its
> own defaults for anything else, including that branch's `-Dscan=` width, which
> is its tuning question rather than this workflow's to guess at.
>
> The ndjson row was measured over five builds rather than seven: at the time,
> only that branch's Rust and Zig read ndjson, so this tree's two were dropped by
> the harness rather than given a slow number. After the merge all four ports
> read all three formats, and the next run will have seven columns everywhere.
---

## Using it

Each port builds from its own directory, with its own toolchain and no system
dependencies:

```bash
(cd c    && make)                    # cc or clang, C11. Also builds c/gen-data
(cd cpp  && make)                    # g++ or clang++, C++20
(cd rust && cargo build --release)   # edition 2024
(cd zig  && zig build --release=fast)
```

```bash
c/csvdiff compare july.csv august.csv -k order_id,line_no
c/csvdiff compare july.csv august.csv -k id -i updated_at --json summary.json --threads 4
rust/target/release/csvdiff compare july.csv august.csv -k id -o report.html
```

Exit codes: **0** identical, **1** differences found, **2** error (**3**
duplicate keys, where `--fail-on-dups` is supported). That makes any of them a
drop-in CI or pipeline gate.

Format is detected from the bytes. A Parquet file may only be compared against
another Parquet file; CSV and ndjson compare against each other.

### What each port carries

| | Reads | Notable | Not there |
|---|---|---|---|
| **[`c/`](c/)** | CSV, ndjson, uncompressed Parquet | fastest on all three formats; threaded on every path; writes all three formats itself (`c/gen-data`) | no HTML report, no `--trim` / `--ignore-case` / `--tolerance` / `--compare` |
| **[`cpp/`](cpp/)** | CSV, ndjson, Parquet **including Snappy** | the full normalisation flags; `--ignore-case` is ASCII-only and refuses non-ASCII by name | no HTML report |
| **[`rust/`](rust/)** | CSV, ndjson, Parquet | the full contract with the **HTML report**; engines `turbo` (default), `sortmerge` (spills to disk) and `native` | — |
| **[`zig/`](zig/)** | CSV, ndjson, Parquet | `--max-memory MB` is **enforced** by a fixed buffer, not hoped for | no HTML report |

Every port builds from its own toolchain alone, in seconds, and carries no
runtime dependency with a comparison engine in it. What was removed to get
there, and what it measured before it went, is in [ARCHIVE.md](ARCHIVE.md).

### Duplicate keys

Every port joins on the **first occurrence** of each key, counts *keys* rather
than rows, and reports duplicates as their own section. That is a choice, and a
stated one: a plain `FULL OUTER JOIN` multiplies duplicates into the diff
instead, silently. See [ARCHIVE.md](ARCHIVE.md#the-field-measured-once-2026-survey).

---

## Working on it

```bash
(cd c && bash test.sh)                # 24 checks, a few seconds, no other toolchain
(cd c && bash test.sh --with-ports)   # adds the cross-port oracles: 42

# the data, in any of the three formats, on every core
c/gen-data --rows 10m --out-dir /tmp/d --prefix p [--format json|parquet] [--threads N]

# two builds of one engine -- the inner loop of working on a port
scripts/bench_ab.sh old/csvdiff new/csvdiff -- compare A.csv B.csv -k id
scripts/bench_ab.sh --self-test c/csvdiff  -- compare A.csv B.csv -k id

# every port that reads the pair, interleaved, with a counts gate and peak RSS
python3 scripts/bench_ports.py A.csv B.csv --repeats 5
```

### Measuring, and what this machine will let you see

`bench_ab.sh` answers "did that change pay?" and is the one to reach for while
working. It runs both builds once per round and reports the **median of the
per-round ratios**, with the middle half of them beside it; where that half
straddles 1.00x it says there is no result rather than leaving a ratio to be
argued about.

That design is not taste. Running all of one build's rounds and then all of the
other's — which is what `hyperfine` and most harnesses do — reads **two copies
of the same binary as 15.8% apart** on a shared runner, and going from five
rounds to fifteen makes it *worse*, because what a shared machine does is drift
rather than jitter and averaging does not touch drift. Comparing the two builds
inside each round does: the same A/A test then reads 1.01x. `--self-test` runs
exactly that, one build against a copy of itself, so the claim can be rechecked
on whatever machine you are on rather than believed. The numbers are in
[BENCHMARKS.md](BENCHMARKS.md#how-to-read-these).

`hyperfine` is still worth having installed for a quick look — its warmup,
standard deviation and `1.07 ± 0.20 times faster` are all better than a bare
ratio, and that ± is the honest part. Just do not read its point estimate on
anything under about 1.2x here: on two identical binaries it named a winner
twice out of three and changed its mind about which.

`c/gen-data` writes the same bytes as the C++ generator and is checked against
it on sixteen shapes, including every thread count — it renders rows in waves
across all cores, and a threading bug that shifted one row would produce a file
that is still valid, still parses, and is wrong. Five million rows of CSV take
3.94s on four cores; the numbers are in [BENCHMARKS.md](BENCHMARKS.md).

`scripts/bench_ports.py` also takes ports built elsewhere, through
`CSVDIFF_PORTS_EXTRA` — a JSON array of `[label, path, flags]`. That is how a
branch's build gets measured against this one on the same rows in the same
sitting, which is the only way two builds can be compared at all. The benchmark
workflow uses it: `alt_ref` checks a second branch out beside this one, builds
its ports, and gives each its own column.

Every port has a `test.sh` holding it to the Rust port's answers on
`tests/fixtures/awkward_*.csv` — a fixture built from every shape that has
broken an engine here: non-ASCII case folding, a Kelvin sign that folds from
three bytes to one, doubled quotes as both value and key, CRLF, ragged rows, a
blank row, and a short key in the last bytes of the file.

| Workflow | Runs |
|---|---|
| `ci-c.yml` | the C port on gcc and clang, sanitizers, and the cross-port checks |
| `ci-rust.yml` | `fmt`, `clippy -D warnings`, `cargo test`, and the engines agreeing on 200k rows |
| `parity.yml` | every port returns identical counts, and every generator emits byte-identical files |
| `benchmark-native.yml` | C and C++ on every push; all four, and any second ref, on demand |

The generator carries money in integer cents and applies drift to those
integers, never to a float, so byte-identity between generators does not depend
on any language's floating-point rounding.

```
c/     the leading port on all three formats: CSV, ndjson, Parquet, plus gen-data and test.sh
cpp/   the C++ port, and the generator that also writes Snappy
rust/  the full contract and the HTML report
zig/   the enforced memory budget
scripts/bench_ab.sh      two builds of one engine, paired by round, with a no-result verdict
scripts/bench_ports.py   every port on one pair, interleaved, with a counts gate
scripts/bench_scale.py   one engine across every size, generating and deleting in turn
tests/fixtures/          every shape that has broken an engine here
```

---

## What's open

1. **ndjson, again.** It has the byte proof now — 1.29x of CPU, against a 1.47x
   ceiling measured with a deliberately unsound build first — so what is left of
   the join is the 6% of rows that really changed and the rows the proof
   refuses. Past that, the floor is the byte scanning itself: this format costs
   3.6x the Parquet time on four times the bytes, and no amount of join work
   changes that.
2. **Where the C CSV path stops scaling.** The join has given up most of what
   it was doing, so the sequential table insertion is now the larger share of
   the run rather than a tail on it. Sharding it by hash was measured and lost
   — the routing costs more than the serial insert it replaces — so what is
   left is to pipeline it against the sweep, inserting a chunk's rows while the
   next chunk is still being read. Total CPU over wall says the whole remaining
   prize is about 1.25x.
3. **Reconciling the two Zig Parquet readers.** This tree and
   `claude/data-comparison-rust-zig-jam00m` each wrote one; `git merge` reports
   them as an add/add conflict.
4. **100M rows.** 50M is measured and is where the input stops fitting in RAM.
   About 100 MB of index per million rows predicts 10 GB at 100M, which is where
   `sortmerge` stops being the conservative choice and becomes the only one.
5. **Wide files.** Everything here is 20 columns. 200 would change the ratio of
   key work to cell work, and probably the ranking — and would re-open the SIMD
   question, since longer rows mean longer scans.
