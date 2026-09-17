# Where the four ports stand

**Ten million rows, all three formats, all four ports, one host, one sitting.**
Measured 2026-09-17. This is the current-state document: what the ports do
today, and which of the differences are real.

[BENCHMARKS.md](BENCHMARKS.md) is the record of *how it got here* — every run,
newest first, with the reasoning and the dead ends. [ARCHIVE.md](ARCHIVE.md) is
what was tried and removed. This file is the snapshot.

---

## The one rule

**Compare rows within a table. Never across tables.** This project has measured
the same code 20% apart on one machine in one morning with nothing changed, and
a port's time swing 1.57x by nothing but where it sat in the running order. Two
numbers are comparable when they were taken on one host, in one sitting,
interleaved, with the running order rotated. Otherwise they are two machine
states wearing the labels of two builds.

Everything below comes from a single invocation of
`scripts/bench_formats_ports.py`, which enforces exactly that.

**On CI the rule needs help.** GitHub's hosted fleet puts several processor
generations behind one `ubuntu-latest` label, so a workflow that fans sizes or
formats out across jobs gets its pieces from different machines. The harness
records the CPU in its JSON and `scripts/bench_group.py` groups on it, printing
one table per processor and naming what is missing from each. A CI summary
showing two CPUs is showing two tables.

---

## The workload

| | |
|---|---|
| Rows | 10,001,000 in A, 10,000,500 in B, 20 columns |
| Key | `(account_id, txn_id)` |
| Ignored | `updated_at` |
| Compared | a per-cell diff over the remaining 17 columns |
| Generator | this project's own, so all three formats hold the same values spelled the same way |

Every port returned identical counts. That is the run's gate, not a footnote —
a port that disagrees fails the run and is named, so no number below belongs to
a build that was fast because it answered a different question.

```
matched 9,990,000 · changed 599,320 · unchanged 9,390,680
added 10,000 · removed 10,000
duplicate keys 1,000 in A / 500 in B
```

**Host:** 4 cores, 15 GB, Linux 6.18. Every port compiled for it —
`-march=native` for C and C++, `-C target-cpu=x86-64-v3` for Rust (capped at the
fleet's executable floor; resolving the host CPU picked `znver4` and a
build-time tool died with SIGILL), `-Dcpu=native` for Zig.

---

## Results

`Above` is peak RSS minus the mapped input, which is the number that carries
information — these engines map their files, so resident pages include the file
itself. `Cores` is CPU seconds over wall seconds: how many of the four were
actually busy. It separates *slow* from *idle*.

### CSV — 3,509 MB

| Port | Wall | Rows/s | CPU | Cores | Above | Budget |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **1.92s** | 5,217,931 | 5.9s | 3.10x | **716 MB** | **792 MB** |
| Rust | 2.32s | 4,308,836 | 6.4s | 2.77x | 900 MB | 977 MB |
| Zig | 2.47s | 4,047,906 | 7.4s | 3.00x | 842 MB | 955 MB |
| C++ | 4.13s | 2,421,448 | 12.9s | 3.13x | 865 MB | 1,253 MB |

### ndjson — 8,487 MB

| Port | Wall | Rows/s | CPU | Cores | Above | Budget |
|---|---:|---:|---:|---:|---:|---:|
| **Rust** | **5.08s** | 1,967,398 | 18.1s | 3.57x | 900 MB | 1,233 MB |
| Zig | 5.48s | 1,823,473 | 20.1s | **3.67x** | 857 MB | 973 MB |
| C | 6.13s | 1,630,636 | **18.1s** | 2.95x | **716 MB** | **792 MB** |
| C++ | 8.76s | 1,141,972 | 25.6s | 2.92x | 861 MB | 1,251 MB |

### Parquet — 1,535 MB

| Port | Wall | Rows/s | CPU | Cores | Above | Budget |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **1.67s** | 6,003,366 | **4.7s** | 2.83x | 1,421 MB | 1,516 MB |
| Zig | 2.12s | 4,723,672 | 7.0s | 3.32x | **1,200 MB** | 1,700 MB |
| C++ | 2.47s | 4,045,579 | 7.3s | 2.94x | 1,307 MB | 1,491 MB |
| Rust | 2.53s | 3,949,412 | 7.8s | 3.07x | 1,360 MB | 1,466 MB |

---

## What the tables say

**No port leads on all three formats any more.** C leads CSV and Parquet; Rust
leads ndjson; Zig leads Parquet on memory. The single-winner table this project
published for most of its life is gone, and it went because the other three
ports closed on the text path rather than because C got slower.

**ndjson is still where the ports differ most, but the ordering has inverted.**
C leads CSV by 1.21x over Rust and *trails* Rust by 1.21x on ndjson. It is also
the flattest on CPU: C and Rust spend the same 18.1 CPU seconds on ndjson, and
Rust is 1.21x faster in wall time entirely by using 3.57 cores where C uses 2.95.
On this format C is not slower, it is less parallel — which is a different
problem with a different fix.

**C++ is last on every text format and by a wide margin**, 2.15x behind C on CSV
and 1.72x behind Rust on ndjson. It is the port with the most headroom and the
one worth working on. Its Parquet column is competitive, so what is behind is
the text path specifically: the profile says the time is in the row parse and in
turning a key name into a slot.

**Memory is the flattest ranking and C's clearest win.** 716 MB above the mapped
input on both text formats against 842 and up, and the same 716 MB whether the
input is 3.5 GB or 8.5 GB — a field there is one 64-bit word and never becomes a
string. Parquet inverts it: the pages are decoded into an arena rather than
mapped, so every port pays over a gigabyte and Zig pays least.

**Budget is not peak RSS and the two disagree.** `Budget` is peak `VmData`, the
virtual size of the private writable mappings, and it is what `--max-memory` is
checked against. A port that holds an old buffer while it fills a new one is
charged for both there and neither in peak RSS. Zig's Parquet row is the
example: the lowest `Above` in the table and the highest `Budget` in it.

---

## What is established, and what is not

| Claim | Evidence |
|---|---|
| C leads on CSV and Parquet wall time | this table, counts-gated |
| Rust leads on ndjson wall time | this table, counts-gated |
| C++ is last on both text formats | this table, and every table before it |
| C uses the least memory above its input on text | this table, and it has never not been true |
| The ndjson row-end fix is worth 1.26x on C++, 1.04x on Rust | [paired A/B, 11 rounds, CSV as control](BENCHMARKS.md) |
| The same fix is worth anything on Zig | **not established** — the band crosses 1.00x |
| Instruction count predicts wall time across ports | **refuted** — Rust executes the most and is fastest on ndjson |
| Instruction count tracks wall time within one port | [0.790 against 0.792 measured](BENCHMARKS.md) |

---

## Against the older published table

[README.md](README.md) carries a 10M table from 2026-09-09 taken on a GitHub
Actions runner. **It is a different host, so the rule above forbids comparing
the two directly.** What can be said is weaker and still worth saying: C is the
port that changed least in that window, so its movement is a rough proxy for the
host difference, and the other three moved much further than it did.

| ndjson, 10M | 2026-09-09 (CI runner) | 2026-09-17 (this host) | moved |
|---|---:|---:|---:|
| C | 7.40s | 6.13s | 1.21x |
| C++ | 20.53s | 8.76s | 2.34x |
| Rust | 14.32s | 5.08s | 2.82x |
| Zig | 13.88s | 5.48s | 2.53x |

Taking C's 1.21x as the host term leaves roughly 1.94x for C++, 2.33x for Rust
and 2.10x for Zig on ndjson from the work in between. **Treat those as an
order of magnitude, not a measurement** — one port is a poor control, and the
Parquet input is not even the same size in the two runs (2,074 MB then,
1,535 MB now), so the Parquet rows are not comparable at all. The paired A/B
runs in BENCHMARKS.md are the real evidence for any single change.

---

## Reproducing this

```sh
# every port, every format, interleaved, with the counts gate
python3 scripts/bench_formats_ports.py --rows 10m --repeats 3

# one format at a time if the disk is tight -- 10M of all three is about 14 GB
python3 scripts/bench_formats_ports.py --rows 10m --formats csv,parquet
python3 scripts/bench_formats_ports.py --rows 10m --formats ndjson

# two builds of one port, which is the only way to price a single change
bash scripts/bench_ab.sh
```

`bench_ab.sh` is the one to reach for when asking *did my change help*. It
prints the middle half of the per-round paired ratios beside the median, and
where that half straddles 1.00x there is no result to report. On this class of
machine about 10% is invisible to a ratio of bests and about 3% is the floor
for the paired one.
