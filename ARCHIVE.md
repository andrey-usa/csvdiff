# Archive

What this project tried, and what it was worth. Terse on purpose: one line of
verdict, the number that earned it, and nothing else. The long write-ups these
replaced are in `git log`, and the runs the numbers come from are in
[BENCHMARKS.md](BENCHMARKS.md).

Numbers here were measured on three different hosts across the project's life
and are **not comparable across sections** — each one is only evidence for the
claim beside it.

---

## Implementations no longer carried

The project began as a DuckDB-backed Python tool with Java, TypeScript, Go and
dataframe engines beside it, to answer whether a bespoke engine was worth
writing. It is answered: on ten million rows the byte-level ports are one to two
orders of magnitude faster and hold their working set within a few hundred MB of
their input. None of these run in CI or the benchmarks; the code is in `git log`.

| Implementation | Good | Bad |
|---|---|---|
| **Python + DuckDB** | the original; correct, and the baseline everything else had to beat | 120.97s at 10M against the ports' 15-40s, 9.9 GB peak |
| **polars** (Rust, TS) | fastest thing measured at 1M — 2.30s, quicker than any port at the time | could not reach 10M at all: runner killed, OOM |
| **datacompy** | per-column summary out of the box | OOM at 10M; pairs duplicate keys *positionally*, so its counts depend on row order |
| **Java** (5 engines) | the design matrix that isolated SWAR from the Vector API; `turbo` reached 25.41s at 10M | ~0.5s JVM startup dominates small inputs; 5.4 GB peak, twice the C port's |
| **TypeScript** | polars binding was competitive at 1M | V8 refuses strings over 512 MB — a wall no machine size moves |
| **Go** | quickest small-input startup in the project, 0.04s at 10k | `native` went **super-linear**, 6.35s at 1M to 161.67s at 10M, fighting the allocator at 15.2 GB peak |
| **Go via DuckDB** | same C++ engine as every other DuckDB binding | slowest of them all — 179.02s against Python's 120.97s, which is what cgo costs at that call rate |
| **GraalVM native-image** | instant startup | 90-107s at 1M, 17-21x the JIT; did not finish 20M |

**The two findings worth keeping.** Memory, not speed, decides 10M — nine of
nineteen engines measured did not finish it, and every survivor was one that
never builds a string per cell. And the same library differs by binding: five
DuckDB bindings doing identical work spread over 48%.

## The field, measured once (2026 survey)

The same comparison run through the tools people actually reach for. Four of
eight could not do 10M at all.

| Tool | Good | Bad |
|---|---|---|
| **csvdiff (Go, aswinkarthik)** | fastest dedicated CSV diff in wide use; 3.32s at 1M; agrees with us exactly | two hashes per row, so it cannot say *which cell* changed; OOM at 10M |
| **DuckDB CLI / clickhouse-local** | clickhouse did 1M in 2.23s, quicker than anything here including us | `FULL OUTER JOIN` **multiplies duplicate keys** — `changed` lands 7 too high at 1M, 91 at 10M, and nothing in the output says why |
| **daff** | real cell-level diff | 46.87s at 1M; `readFileSync` hits V8's 512 MB string cap, so it cannot open a large file on any machine |
| **csv-diff** | small, popular | one key column, no column-ignore — it cannot express this task |
| **sort(1) + join(1)** | 251 MB at 1M because `sort` spills; the right *algorithm* | no idea what CSV quoting is, so a comma in a field is silently wrong; counts only |

**Two independent SQL engines make the identical mistake**, to the row. The
inflation is a property of the operator, not the engine — picking a better one
does not change it.

**Nothing in the field reports duplicate keys.** They fold them in silently or
multiply them into the answer. This project joins on the first occurrence,
counts keys rather than rows, and reports duplicates as their own section.

---

## Techniques

### Worth it

| Technique | Verdict |
|---|---|
| **The byte-level design** — map the file, a field is one 64-bit word (40-bit offset, 23-bit length, escape bit), nothing becomes a string | the whole reason this class survives at 10M: memory grows with the number of rows, not the bytes in them |
| **SWAR** — find a byte in eight with `(diff - 0x0101…) & ~diff & 0x8080…` | no intrinsics, no feature detection, and it **ties the Vector API at 10M and beats it by 19% at 20M** |
| **Reading Parquet natively, columnwise** | 23.25s → 4.89s on the same comparison. The win is not a faster parser: with both dictionaries interned into one id space, comparing a cell becomes `int32 != int32` |
| **Chunked parallel sweep, ordered insert** | quote parity finds row boundaries mid-file — every `"` toggles state, a doubled quote toggles twice — so parsing splits while first-occurrence-wins stays deterministic |
| **Parallel join over key ranges** | the actual long pole: 20M from 54.60s (2 threads) to 33.32s (4), 2.53x cpu/wall |
| **Sizing the hash table once** | thirteen rehashes at 10M, each a full pass of random probes, gone |
| **Software prefetch at distance 24** | every insert is a cache miss on a table too big to hold, and the hash is already in hand |
| **Threading the generator** | 2.3x to 2.8x on wall for no more CPU -- flat on CSV, 12% *down* on ndjson. This was in the "not worth it" table above on the strength of a comparison that turned out to be unsound; it was a scope decision recorded as a measurement |
| **Reading only the key columns where only keys are read** | C's CSV path: 2.66x at two million rows, **3.65x at ten million** — the gain grows with the size, because past cache the parses removed were memory traffic and not only instructions |
| **Huge pages for rare, long-lived, randomly-probed allocations** | a 128 MB slot table is 32,768 4 KB pages against ~1,500 TLB entries |
| **Writing the generator's two sides at once** | 2.0x on Parquet by itself, where splitting a row group's twenty columns — the split the design points at — is 1.28x |
| **Deleting the dataframe engines from the Rust port** | a cold build went from twenty minutes to twenty seconds; nothing measured them any more |

Measured on `claude/data-comparison-rust-zig-jam00m`, not here, and taken from
that branch's own record; what this project measured of them is the joint run in
[BENCHMARKS.md](BENCHMARKS.md), where its Zig does CSV in 4.55s and ndjson in
15.58s.

| Technique (other branch) | Verdict |
|---|---|
| **Not parsing every matched row twice** | the same finding the C port made independently, in both of those ports |
| **Sizing the index once and tagging its slots** | a failed probe is settled by the word already loaded; the C port had this, those two did not |
| **Joining both sides from one queue** | replaces two passes with one, and stops deep-copying the sorted rows |
| **Writing the index's zeroes rather than asking the kernel** | faulting a fresh mapping in costs more than touching memory already owned |
| **Prefetch distance 32** | their tuning; this port measured 24 on the same shape, so the optimum is a property of the machine as much as the code |
| **AVX-512** | wins on a CPU that has it, which hosted runners mostly do not — their own note says the 64-byte builds skip themselves |

### Not worth it — measured, and recorded so it is not retried

| Technique | Verdict |
|---|---|
| **Huge pages in a loop** | each request makes the kernel compact memory. Two allocations win; six lose; sixteen lose badly. Column arrays 1.00s → 1.96s, key columns 2.03s → 2.79s (measured twice) |
| **Sharding the index insertion** | 2.84s against 1.95s, CPU 9.2s against 5.8s — the routing costs more than the serial insertion it parallelises |
| **Widening the hash to 8 bytes at a time** | no measurable effect |
| **A wave size other than 8,192 rows in the generator** | 2k, 32k and 131k all cost more on CSV (0.93s against 1.29s at 32k); ndjson is nearly flat, so the sensitive format chose it |
| **Splitting a row group's columns as the main Parquet lever** | 1.28x, against 2.0x for writing the two sides at once. Most of a Parquet run is the serial feeding of cell values into the column arenas, not the encoding |
| **Chasing a CPU regression that was noise** | an hour spent on a 1.9s→3.5s "regression" in the threaded generator that a nine-round measurement showed was 2.66s→2.61s. On this container a wall-clock difference under about 1.5x is not signal; use min-of-N CPU in one interleaved sitting |
| **GPU offload** | not attempted. Per-row cost doubles between 1M and 3M as the index leaves L3, putting the plausible crossover at 370k-4M rows — but every GPU-side number would have been an estimate, since there is no GPU here |

### Things that were true and stopped being true

- **"The scan is bandwidth-bound, so more threads won't help."** Wrong, and one
  script disproved it: four copies of the single-threaded binary each finish
  *faster* than one alone (0.90x). The ceiling was the design.
- **"Input format matters less than the engine."** Wrong. Parquet took the same
  comparison from 23.25s to 4.89s — more than any engine change measured here.
- **"`shard` (Vector API) beats `turbo` (SWAR)."** True at 10M by 0.08s, which
  was noise on single runs. At twice the size SWAR is 19% ahead.
- **"The C port is fastest on every format."** True for about three hours. The
  other branch's rewritten Zig now takes ndjson, 15.58s against 20.13s.
- **"The C++ generator is faster than the C one and the gap stays."** Withdrawn:
  its Parquet row compared snappy output against uncompressed, and the host
  cannot measure that generator to better than a five-fold spread anyway.
