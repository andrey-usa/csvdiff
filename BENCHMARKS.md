# Engine benchmarks

csvdiff can run its comparison on seven different backends. They return identical
results, so the only question is which one to reach for. This is the measurement
behind the answer.

Reproduce any of it with:

```bash
pip install -e ".[bench]"
python scripts/bench_matrix.py --scales 10k,100k,1m --repeat 3
python scripts/bench_matrix.py --scales 3m,10m --repeat 1 --memory-limit 8GB
```

## The short answer

**Use polars when both files fit in memory, DuckDB when they might not.**

polars is the fastest engine at every scale that fits in RAM — about 3x DuckDB at 1M
and 3M rows — but it is bounded by memory, and every in-memory engine falls off the
same cliff at the same place. DuckDB spills to disk, which is why it stays the
default: an engine that is three times slower is better than one that is killed.

The full ranking, in memory: **polars first, then pyarrow and DataFusion (which trade
places with each other), then DuckDB, the standard library engine just behind it, and
pandas an order of magnitude back** — 16x slower than polars, and 4x slower than the
engine that needs no dependencies at all.

## What is measured

`scripts/gen_data.py` writes a deterministic pair of files: 20 columns, composite key
`(account_id, txn_id)`, and a fixed drift recipe (3% status changes, 1.5% amount, 1.5%
balance, 0.3% blanked dates, `updated_at` different on every row, 0.1% added, 0.1%
removed, 0.01% duplicate keys). Every run compares them with `-k account_id,txn_id -i
updated_at`, so all engines do the same work over 18 compared columns and produce the
same report.

`scripts/bench_matrix.py` runs each (scale, engine) pair through `scripts/bench.py` in
a **child process**, so the peak RSS is the comparison's own and not the harness's. Two
times are reported:

- **compare** — wall clock for the whole child: interpreter start, imports, the
  comparison and rendering the HTML report. This is what a user waits for.
- **in-process** — `engine.compare()` alone, from `meta.seconds`. At 10k rows almost
  all the wall time is Python and library startup, so this is the column to read there.

Hardware: 4 vCPU, 15 GB RAM, Linux, CPython 3.11.15, all files on local disk. DuckDB
was given `--memory-limit 8GB` at 3M and above. Versions: duckdb 1.5.5, polars 1.44.1,
pyarrow 25.0.1, datafusion 54.0.0, pandas 3.0.5, koala-diff 0.3.2. These are relative
numbers on one shared machine — the ordering is the finding, not the absolute seconds.

## Results

10k and 100k are the median of 3 runs; 3M and 10M are single runs.

### 10,000 rows x 20 columns — 4 MB of CSV

| engine | compare | in-process | vs fastest | throughput | peak RSS | agrees |
|---|---|---|---|---|---|---|
| `koala` | **0.75s** | 0.077s | 1.00x | 26,669/s | 316 MB | n/a |
| `python` | **0.77s** | 0.113s | 1.03x | 25,976/s | 281 MB | yes |
| `polars` | **0.85s** | 0.177s | 1.13x | 23,531/s | 346 MB | yes |
| `arrow` | **0.88s** | 0.212s | 1.17x | 22,729/s | 382 MB | yes |
| `datafusion` | **0.99s** | 0.335s | 1.32x | 20,204/s | 410 MB | yes |
| `duckdb` | **1.05s** | 0.403s | 1.40x | 19,049/s | 326 MB | reference |
| `pandas` | **1.42s** | 0.736s | 1.89x | 14,085/s | 318 MB | yes |

### 100,000 rows x 20 columns — 37 MB of CSV

| engine | compare | in-process | vs fastest | throughput | peak RSS | agrees |
|---|---|---|---|---|---|---|
| `koala` | **1.08s** | 0.404s | 1.00x | 185,199/s | 450 MB | n/a |
| `polars` | **1.11s** | 0.335s | 1.03x | 180,193/s | 584 MB | yes |
| `arrow` | **1.3s** | 0.57s | 1.20x | 153,857/s | 631 MB | yes |
| `datafusion` | **1.65s** | 0.948s | 1.53x | 121,221/s | 696 MB | yes |
| `python` | **1.92s** | 1.171s | 1.78x | 104,174/s | 413 MB | yes |
| `duckdb` | **3.26s** | 2.547s | 3.02x | 61,354/s | 617 MB | reference |
| `pandas` | **6.96s** | 6.225s | 6.44x | 28,737/s | 586 MB | yes |

### 1,000,000 rows x 20 columns — 367 MB of CSV

| engine | compare | in-process | vs fastest | throughput | peak RSS | agrees |
|---|---|---|---|---|---|---|
| `polars` | **4.3s** | 2.9s | 1.00x | 465,151/s | 2,612 MB | yes |
| `datafusion` | **7.94s** | 6.735s | 1.85x | 251,908/s | 2,393 MB | yes |
| `arrow` | **10.32s** | 9.019s | 2.40x | 193,812/s | 2,579 MB | yes |
| `duckdb` | **13.1s** | 11.927s | 3.05x | 152,683/s | 2,479 MB | reference |
| `python` | **13.36s** | 12.004s | 3.11x | 149,711/s | 1,717 MB | yes |
| `pandas` | **69.87s** | 68.709s | 16.25x | 28,626/s | 2,803 MB | yes |
| `koala` | — | | | | | aborts (see below) |

### 3,000,000 rows x 20 columns — 1,101 MB of CSV

| engine | compare | in-process | vs fastest | throughput | peak RSS | agrees |
|---|---|---|---|---|---|---|
| `polars` | **14.18s** | 12.278s | 1.00x | 423,162/s | 6,671 MB | yes |
| `arrow` | **19.32s** | 17.715s | 1.36x | 310,582/s | 6,090 MB | yes |
| `datafusion` | **24.68s** | 23.139s | 1.74x | 243,130/s | 6,306 MB | yes |
| `duckdb` | **46.86s** | 45.355s | 3.30x | 128,050/s | 6,108 MB | reference |
| `python` | **59.53s** | 57.746s | 4.20x | 100,797/s | 4,693 MB | yes |
| `pandas` | **234.65s** | 233.018s | 16.55x | 25,571/s | 7,554 MB | yes |
| `koala` | — | | | | | aborts (see below) |

### 10,000,000 rows x 20 columns — 3,671 MB of CSV

| engine | compare | in-process | vs fastest | throughput | peak RSS | agrees |
|---|---|---|---|---|---|---|
| `duckdb` | **145.64s** | 143.21s | 1.00x | 137,335/s | 9,039 MB | reference |
| `polars` | — | | | | | **killed, out of memory** |
| `arrow` | — | | | | | **killed, out of memory** |
| `datafusion` | — | | | | | **killed, out of memory** |
| `python` | — | | | | | **killed, out of memory** |


At 10k rows the wall clock is almost entirely interpreter start and imports — every
engine does the actual comparison in well under half a second, and the `in-process`
column is the only meaningful one. From 100k up the comparison dominates and the
ordering stops moving: polars first, pyarrow and DataFusion behind it, DuckDB roughly
3x off the pace, the standard library engine close to DuckDB, pandas an order of
magnitude behind everything.

## The memory cliff

The 10M run is the whole argument for the default. Both files are 1.8 GB of CSV; the
machine has 15 GB of RAM. Only DuckDB finished, and it finished comfortably.

| rows | polars | pyarrow | DataFusion | DuckDB | stdlib | pandas |
|---|---|---|---|---|---|---|
| 1M | 2.6 GB | 2.6 GB | 2.4 GB | 2.5 GB | 1.7 GB | 2.8 GB |
| 3M | 6.7 GB | 6.1 GB | 6.3 GB | 6.1 GB | 4.7 GB | 7.6 GB |
| 10M | killed | killed | killed | **9.0 GB** | killed | not run |

Peak RSS grows at roughly 2 GB per million rows x 20 columns for the in-memory
engines, i.e. about 6x the size of the CSV on disk. DuckDB tracks the same curve until
it hits its `--memory-limit` and then spills instead of growing, which is the
difference between 145 seconds and being killed. The standard library engine is the
most frugal of the in-memory group — it only holds file A, and streams file B past it —
but "most frugal in-memory engine" is still an in-memory engine, and it dies at 10M too.

So the rule is not "polars is faster, switch to polars". It is:

- **Under ~2M rows x 20 columns on a 16 GB machine:** `--engine polars`, 3x faster.
- **Above that, or when you do not know the size in advance:** leave it on DuckDB.
- **In a container with a hard memory limit:** DuckDB with `--memory-limit` set below
  the cgroup limit is the only engine that degrades gracefully rather than being killed.

## Where the time goes

DuckDB's showing needs one qualification: this harness compares two CSV files
end-to-end, and DuckDB's CSV sniffing and single-threaded `preserve_insertion_order`
scan are a large part of its number. It is optimised for a different shape of problem —
data that is already in a database, or files far larger than memory — and it wins the
only test where that matters. The alternative engines are faster here because reading
two CSVs into RAM and hash-joining them is exactly what polars and Arrow are built for.

pandas is slow for a structural reason, not a tuning one: `_compare_pandas` iterates
matched rows in Python to build the sparse cell diffs. Every other engine computes the
diff flags as vectorised column operations and only touches Python for the rows that
actually go into the report. That is also why the standard library engine — which
iterates in Python too, but does it once, streaming, without building intermediate
frames — beats pandas by 4-16x.

## koala-diff

koala-diff is the closest off-the-shelf equivalent to this tool, so it is in the
matrix. It does not run the benchmark's workload at all:

```
RuntimeError: ABORTED: Potential Cartesian Product Explosion detected!
File A: 750100 duplicates, File B: 750299 duplicates.
```

The key here is `(account_id, txn_id)`, which is unique. koala's duplicate guard
appears to look at the leading key column only — `account_id` repeats about four times
across the file — so any composite key whose first column is not itself unique is
rejected before the comparison starts. On a genuinely unique single-column key it runs
fine, so here it is against the same 1M rows keyed on `txn_id` with every column
compared:

### 1,000,000 rows x 20 columns — 367 MB of CSV

| engine | compare | in-process | vs fastest | throughput | peak RSS | agrees |
|---|---|---|---|---|---|---|
| `koala` | **4.55s** | 3.834s | 1.00x | 439,593/s | 1,961 MB | n/a |
| `polars` | **4.63s** | 3.441s | 1.02x | 431,997/s | 3,476 MB | yes |
| `arrow` | **8.25s** | 7.356s | 1.81x | 242,442/s | 3,027 MB | yes |
| `duckdb` | **11.86s** | 10.929s | 2.61x | 168,646/s | 2,990 MB | reference |


It ties with polars on wall clock and loses to it in-process, while producing much
less: no `--ignore`, no `--tolerance`, no normalisation, no duplicate-key resolution,
no changed / added / removed row lists (its 0.02 MB report is counts and per-column
mismatch totals, against 0.39 MB of actual rows for the others), and type inference
that makes `1.0` equal `1` — the exact surprise this tool exists to avoid. It is a good
tool for a different question. As a csvdiff engine it is registered `contract=False`
and excluded from `auto`, from `engines.available(contract_only=True)` and from the
cross-engine equality tests.

## Do they actually agree?

Speed is only interesting if the answers match, so agreement is checked at three levels:

- `tests/test_engines.py` runs every installed engine against DuckDB on the example
  files and compares **all seven sections** of the result — counts, per-column stats,
  the changed / added / removed row lists and both duplicate-key lists — plus a fixture
  of NULL spellings CSV readers disagree about (quoted empty, bare empty,
  whitespace-only, a NULL inside the key) under four normalisation settings, and a
  byte comparison of the `--export-dir` CSVs.
- The `engines-agree` CI job repeats the `counts` and `columns` check on 200k rows.
- `scripts/bench_matrix.py` re-checks agreement on every benchmark run and prints
  `MISMATCH` for any engine that drifts; `engines.yml` fails the job when it does.

Every engine in the tables above agrees, at every scale. Three real differences had to
be fixed to get there, all of them the kind of thing that silently changes a
reconciliation result:

- **polars read a quoted empty field as an empty string** where DuckDB reads NULL, so
  a blanked column counted as changed-but-not-blanked. `polars_engine._norm` maps `''`
  to NULL before any other normalisation.
- **DataFusion's parser binds `IS NOT DISTINCT FROM` looser than `AND`**, so the
  multi-column null-safe join condition failed to plan until each comparison got its
  own parentheses. Without them there is no null-safe hash join.
- **pyarrow has no `try_cast` and its hash join follows SQL null semantics**, so
  `--tolerance` masks non-numeric values before casting, and rows are joined on a copy
  of the key with NULLs replaced by a sentinel.

## Reproducing

```bash
pip install -e ".[bench]"
python scripts/bench_matrix.py --scales 10k,100k,1m --repeat 3
python scripts/bench_matrix.py --scales 3m,10m --repeat 1 --memory-limit 8GB
python scripts/bench_matrix.py --scales 1m --key txn_id --ignore "" --engines koala,polars,duckdb
```

Results land in `bench/matrix.json` and `bench/matrix.md`. In CI, run the
**Engine benchmark** workflow (`gh workflow run "Engine benchmark" -f scales=10k,1m`);
it installs every engine, runs the matrix and fails if any engine disagrees with DuckDB.
