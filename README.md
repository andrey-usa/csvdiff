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
(4 vCPU / 16 GB), all builds interleaved in one sitting, five rounds each,
2026-09-08. Each cell is **wall · CPU · memory above the mapped input**, for
each port's fastest build.

| Format | Input | C | C++ | Rust | Zig |
|---|---:|---|---|---|---|
| CSV | 3,509 MB | **4.16s** · 14.9s · 716 MB | 10.12s · 32.2s · 936 MB | 5.68s · 19.9s · 899 MB | 4.55s · 17.1s · 887 MB |
| ndjson | 8,487 MB | 20.13s · 74.5s · 717 MB | 26.36s · 81.1s · 880 MB | 16.14s · 61.6s · 899 MB | **15.58s** · 61.2s · 879 MB |
| Parquet | 2,074 MB | **1.63s** · 5.4s · 1,299 MB | 3.28s · 10.9s · 1,482 MB | 3.79s · 11.7s · 1,322 MB | 4.42s · 12.1s · 1,299 MB |

All builds returned identical counts — matched 9,990,000, changed 599,320, added
10,000, removed 10,000, duplicate keys 1,000 in A and 500 in B. That is the
run's gate, not a footnote: builds that disagree about how many rows changed
mean a bug in one of them, so the run fails and names it.

Three things this table says.

**Parquet is a different problem.** 1.63s against 4.16s for the same rows in
CSV, on three-fifths of the bytes, because with both dictionaries interned into
one id space a cell comparison becomes `int32 != int32` rather than a string
comparison. No port is within 2x of the C one there.

**Nobody holds all three.** C takes Parquet by 2x and CSV by 9%; Zig takes
ndjson by 23%.

**Read the CPU column, not the wall column.** Wall time mixes work with how many
cores a design manages to use; CPU seconds do not. Every row here is within 4x
of its own CPU on four cores, so the ranking is about work rather than
scheduling — which is exactly how the C port found the change that took its CSV
row from 14.82s to 4.16s in a day: not by threading harder, but because 57
CPU-seconds against a rival's 24 said it was doing twice the work.

> **Where the columns come from, and how stale they are.** Rust and Zig, and
> C++ on the text formats, are built from
> `claude/data-comparison-rust-zig-jam00m` at `0569a39`; C and C++ on Parquet
> from this tree. One runner built both, because two builds measured in two
> sittings are not compared at all.
>
> Both trees have moved since. The C ndjson path is 1.58x quicker than the cell
> above (measured at two million rows, not yet at ten), and that branch's
> Parquet is near 3.6s in its own runs. Neither number belongs in this table
> until one runner produces them together — which is the whole point of the
> table, and the reason it is a snapshot with a date on it rather than a
> scoreboard.

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
| **[`c/`](c/)** | CSV, ndjson, uncompressed Parquet | fastest on Parquet; threaded on every path; writes all three formats itself (`c/gen-data`) | no HTML report, no `--trim` / `--ignore-case` / `--tolerance` / `--compare` |
| **[`cpp/`](cpp/)** | CSV, ndjson, Parquet **including Snappy** | the full normalisation flags; `--ignore-case` is ASCII-only and refuses non-ASCII by name | no HTML report |
| **[`rust/`](rust/)** | CSV, Parquet | the full contract with the **HTML report**; engines `turbo` (default), `sortmerge` (spills to disk) and `native` | ndjson |
| **[`zig/`](zig/)** | CSV, Parquet | `--max-memory MB` is **enforced** by a fixed buffer, not hoped for | no HTML report, no ndjson |

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
(cd c && bash test.sh)                # 20 checks, a few seconds, no other toolchain
(cd c && bash test.sh --with-ports)   # adds the cross-port oracles: 38

# the data, in any of the three formats, on every core
c/gen-data --rows 10m --out-dir /tmp/d --prefix p [--format json|parquet] [--threads N]

# every port that reads the pair, interleaved, with a counts gate
python3 scripts/bench_ports.py A.csv B.csv --repeats 5
```

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
c/     the leading port on Parquet: CSV, ndjson, Parquet, plus gen-data and test.sh
cpp/   the C++ port, and the generator that also writes Snappy
rust/  the full contract and the HTML report
zig/   the enforced memory budget
scripts/bench_ports.py   every port on one pair, interleaved, with a counts gate
scripts/bench_scale.py   one engine across every size, generating and deleting in turn
tests/fixtures/          every shape that has broken an engine here
```

---

## What's open

1. **ndjson, again.** 1.58x came from framing rows on the newline byte and
   letting the key-only parse stop once it has the keys. What is left is the
   join, which still parses a matched row in full on both sides.
2. **Where the C CSV path stops scaling.** 14.9 CPU-seconds over 4.16s of wall
   is 3.58 of four cores, and what is left sequential is the table insertion.
3. **Reconciling the two Zig Parquet readers.** This tree and
   `claude/data-comparison-rust-zig-jam00m` each wrote one; `git merge` reports
   them as an add/add conflict.
4. **100M rows.** 50M is measured and is where the input stops fitting in RAM.
   About 100 MB of index per million rows predicts 10 GB at 100M, which is where
   `sortmerge` stops being the conservative choice and becomes the only one.
5. **Wide files.** Everything here is 20 columns. 200 would change the ratio of
   key work to cell work, and probably the ranking — and would re-open the SIMD
   question, since longer rows mean longer scans.
