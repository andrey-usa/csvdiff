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
tree, its own workflow (`bench-10m.yml`), its own runner. Recorded because they
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
