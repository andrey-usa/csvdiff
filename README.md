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
2026-09-08. Each cell is **wall · CPU · memory above the mapped input**.

| Format | Input | C | C++ | Rust | Zig |
|---|---:|---|---|---|---|
| CSV | 3,509 MB | 14.82s · 57.3s · 716 MB | 10.09s · 32.1s · 893 MB | **8.56s** · 24.8s · 1,208 MB | 8.82s · 24.2s · 902 MB |
| ndjson | 8,487 MB | 23.03s · 85.7s · 717 MB | 26.25s · 80.8s · 895 MB | **22.96s** · 70.4s · 1,208 MB | 23.39s · 70.9s · 916 MB |
| Parquet | 2,074 MB | **1.67s** · 5.6s · 1,286 MB | 3.26s · 10.8s · 1,481 MB | 3.72s · 11.5s · 1,338 MB | 4.44s · 12.2s · 1,341 MB |

All builds returned identical counts — matched 9,990,000, changed 599,320, added
10,000, removed 10,000, duplicate keys 1,000 in A and 500 in B. That is the
run's gate, not a footnote: builds that disagree about how many rows changed
mean a bug in one of them, so the run fails and names it.

Three things this table says.

**Parquet is a different problem.** 1.67s against 14.82s for the same rows in
CSV, on three-fifths of the bytes, because with both dictionaries interned into one id
space a cell comparison becomes `int32 != int32` rather than a string
comparison.

**JSON costs what its bytes cost.** 2.4x the size of the CSV and 1.5x the time,
every field carrying its name. It is worth having because it is the input people
receive, not because it is fast.

**Wall time and CPU disagree, and CPU is the honest one.** On CSV the C port
uses 3.87 cores against Rust's 2.90 and still loses, because it does 2.3x the
work. Where a build wins on wall and loses on CPU, it is winning on threading.

> The CSV and ndjson columns for Rust and Zig come from
> `claude/data-comparison-rust-zig-jam00m`, which carried those ports' newer
> engines; the C, C++ and Parquet numbers come from this tree. The run built
> both and gave each its own column rather than merging two branches to measure
> them. Full tables, including both versions of every port, are in
> [BENCHMARKS.md](BENCHMARKS.md).

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
| **[`rust/`](rust/)** | CSV, Parquet | the full contract with the **HTML report**; engines `turbo`, `sortmerge` (spills to disk), `native`, plus DuckDB and polars | ndjson |
| **[`zig/`](zig/)** | CSV, Parquet | `--max-memory MB` is **enforced** by a fixed buffer, not hoped for | no HTML report, no ndjson |

The Rust port bundles DuckDB and polars, which is most of why it is the slowest
to build. The C port is the quickest, at a couple of seconds.

### Duplicate keys

Every port joins on the **first occurrence** of each key, counts *keys* rather
than rows, and reports duplicates as their own section. That is a choice, and a
stated one: a plain `FULL OUTER JOIN` multiplies duplicates into the diff
instead, silently. See [ARCHIVE.md](ARCHIVE.md#the-field-measured-once-2026-survey).

---

## Working on it

```bash
(cd c && bash test.sh)                # 18 checks, a few seconds, no other toolchain
(cd c && bash test.sh --with-ports)   # adds the cross-port oracles: 30

# the data, in any of the three formats
c/gen-data --rows 10m --out-dir /tmp/d --prefix p [--format json|parquet]

# every port that reads the pair, interleaved, with a counts gate
python3 scripts/bench_ports.py A.csv B.csv --repeats 5
```

`scripts/bench_ports.py` also takes ports built elsewhere, through
`CSVDIFF_PORTS_EXTRA` — a JSON array of `[label, path, flags]`. That is how a
branch's build gets measured against this one on the same rows in the same
sitting, which is the only way two builds can be compared at all.

Every port has a `test.sh` holding it to the Rust port's answers on
`tests/fixtures/awkward_*.csv` — a fixture built from every shape that has
broken an engine here: non-ASCII case folding, a Kelvin sign that folds from
three bytes to one, doubled quotes as both value and key, CRLF, ragged rows, a
blank row, and a short key in the last bytes of the file.

| Workflow | Runs |
|---|---|
| `ci-c.yml` | the C port on gcc and clang, sanitizers, and the cross-port checks |
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

1. **The C CSV path is still second.** 2.66x faster than it was, and the
   remaining gap to the fastest Rust build is the join, not the scan.
2. **Stopping the ndjson walk once the keys are found.** Worth roughly the same
   again on that format, but it needs the full and key-only parses to agree on
   which value of a repeated JSON key wins.
3. **Reconciling the two Zig Parquet readers.** This tree and
   `claude/data-comparison-rust-zig-jam00m` each wrote one; `git merge` reports
   them as an add/add conflict.
4. **100M rows.** 50M is measured and is where the input stops fitting in RAM.
   About 100 MB of index per million rows predicts 10 GB at 100M, which is where
   `sortmerge` stops being the conservative choice and becomes the only one.
5. **Wide files.** Everything here is 20 columns. 200 would change the ratio of
   key work to cell work, and probably the ranking — and would re-open the SIMD
   question, since longer rows mean longer scans.
