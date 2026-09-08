# csvdiff — C

The same byte-level comparison as the Rust, Java, C++ and Zig `turbo` engines,
in C99-shaped C11, to find out what the memory floor actually is when nothing is
allocated that the design does not require.

It also reads **uncompressed Parquet**, which is not a second parser but a second
*comparison*: a column store is joined on its key columns and then diffed one
column at a time, and never becomes rows at all. Both paths give the same
answer on the same data, which is what `test.sh` checks.

```bash
make                       # cc, whatever that is
CC=clang make              # or clang, which is worth 0.6s here
./csvdiff compare a.csv b.csv         -k id --json summary.json
./csvdiff compare a.parquet b.parquet -k id --json summary.json
```

## The CSV design, unchanged

Both files are `mmap`ed with `MADV_SEQUENTIAL`. A field is one `uint64_t`:
40 bits of offset, 23 bits of length, and the top bit set when the field
contains a doubled quote and has to be unescaped before it is read. Delimiters
are found eight bytes at a time — `diff = word ^ broadcast(c)`, then
`(diff - 0x0101…) & ~diff & 0x8080…`, then `trailing_zeros >> 3`. Keys go into
an open-addressing table sized to a power of two. Nothing becomes a
heap-allocated string except the column names in the summary.

## The Parquet design

Uncompressed only, and a compressed column is refused by name. This port exists
to measure a reader, and a codec in the middle answers a different question.

The reader hands back *offsets into the mapped file* rather than strings: a
PLAIN byte array is a four-byte length followed by its bytes, already contiguous
and already in the mapping, so a value stays an offset and a length — the same
eight-byte packing the CSV path uses for a field. With nothing decompressed
there is no second buffer, so every offset is into the mapping, always.

Dictionary columns stay encoded. The comparison reads the key columns, joins on
them once into a list of matched `(a_row, b_row)` pairs, then walks the compared
columns one at a time, releasing each before the next. Where both sides of a
column are dictionary encoded the two dictionaries are interned into **one
shared id space** — one hash per *distinct* value, not per row — after which
"did this cell change" is `int32 != int32`, which the compiler vectorises, and
the mismatch mask is read eight bytes at a time by the same SWAR idiom the CSV
scanner uses to find a delimiter.

The index carries each key's hash tag *inside* the slot: forty bits of position
and the top of the hash in one word, so a probe that misses is settled by the
word it already loaded rather than taking a second cache miss into a separate
array of hashes.

Unlike the CSV path, this one is threaded — both files' key columns are read at
once, both indexes are built at once, both directions of the join are split into
ranges, and the compared columns are claimed from a shared counter by one worker
per core. A column is the natural unit of parallel work here, and pretending
otherwise would measure the wrong thing.

## What it is and is not

A **floor measurement and a parity port**, not a fifth product. It carries the
comparison and the JSON counts, not the HTML report and not the full option
set:

- **No `--trim`, `--ignore-case` or `--tolerance`.** Each of these needs either
  a Unicode table or a number parser on the hot path, and each would change the
  thing being measured. The C++ port draws this line differently — it
  implements `--ignore-case` and refuses non-ASCII input — which is also
  defensible; this port simply does not offer the flag.
- **No `--export-dir`, no `--profile`, no HTML.** The five full ports produce
  those.
- **Uncompressed Parquet only, and BYTE_ARRAY columns only.** Snappy, zstd,
  gzip, data page v2 and nested columns are each refused by name. The C++ port
  carries Snappy; this one deliberately does not.

`test.sh` holds it to the Rust port's answers on `tests/fixtures/awkward_*.csv`
— the fixture assembled from every shape that has broken an engine in this
project — including the per-column `changed` / `blanked` / `filled` counts.

## What the floor turned out to be

One million rows, best of three, one 4-core container. The two mapped inputs
are 351 MB, and mapped pages count as resident, so the column that matters is
the last one: what the engine allocates on top of the files it is reading.

| Build | Compare | Peak RSS | Above the mapped files |
|---|---:|---:|---:|
| C, clang 18 | **4.77s** | 417 MB | 67 MB |
| C, gcc 13 | 5.34s | 417 MB | 67 MB |
| C++, clang 18 | 3.64s | 509 MB | 158 MB |
| C++, gcc 13 | 6.27s | 509 MB | 158 MB |
| Zig 0.16, ReleaseFast | 5.31s | 414 MB | 63 MB |
| Rust, `--engine turbo` | 5.25s | 583 MB | 232 MB |

Ten million rows, best of two, mapped inputs 3679 MB:

| Build | Compare | Peak RSS | Above the mapped files |
|---|---:|---:|---:|
| C, gcc 13 | 61.2s | 4224 MB | 545 MB |
| C++, clang 18 | **37.8s** | 4334 MB | 655 MB |
| Zig 0.16, ReleaseFast | 59.9s | 4224 MB | 545 MB |
| Rust, `--engine turbo` | 49.1s | 4412 MB | 733 MB |

Two things fell out of this that were not the point of writing it.

**C is not the floor — it ties with Zig.** 67 MB against 63 MB at a million
rows, and identical at ten million. That is the honest answer: the floor
belongs to the *design*, not to the language. Once the row index, the offset
array and the hash table are sized the same way, there is nothing left for a
language to save — the 4 MB between C and Zig at a million rows is allocator
bookkeeping, not a structural difference, and it disappears entirely at ten
million.

**The compiler moves more than the language does.** clang builds this source
1.12x faster than gcc, and the same swap on the C++ port is worth 1.72x. The
fastest and the slowest byte-level build in this whole table are both C++,
from the same file, 1.7x apart. Any single-number comparison of C against Rust
against Zig that does not say which compiler produced each binary is reporting
the toolchain and calling it the language.

## What Parquet turned out to be worth

One four-core container, 16 GB, interleaved runs so a slow patch of the machine
hits both binaries equally. Both ports read the same files and are checked to
produce the same counts and the same per-column statistics on every run.

**Two million rows, uncompressed Parquet** (415 MB of input), seven runs each:

| Build | Best | Median | Worst | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|
| **C** | **0.53s** | **0.74s** | **0.85s** | **1.5s** | **740 MB** |
| C++ | 1.02s | 1.06s | 1.13s | 2.8s | 769 MB |

**Ten million rows** (2,074 MB of input), five runs each:

| Build | Best | Median | Worst | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|
| **C** | **3.86s** | **4.02s** | **5.22s** | **10.6s** | **3,280 MB** |
| C++ | 5.18s | 5.23s | 5.34s | 15.2s | 3,394 MB |

**The format is worth about 4.6x, and it is the same 4.6x in both languages.**
The same two million rows as CSV, same machine:

| Build | Format | Compare | CPU |
|---|---|---:|---:|
| C | CSV | 11.55s | 11.5s |
| C | Parquet | **0.53s** | **1.5s** |
| C++ | CSV | 5.05s | 13.2s |
| C++ | Parquet | **1.03s** | **2.8s** |

Wall clock says Parquet is worth 21x in C and 4.9x in C++, but that comparison
is contaminated: the C **CSV** path is single-threaded and the C **Parquet**
path is not. CPU time takes the threading back out and leaves the format on its
own — 11.5s to 1.5s and 13.2s to 2.8s. **7.7x and 4.7x**, and the part of the
gap that is not the format is the tuning below.

## Where the time went, and what moved it

The first working version of this path took **4.84s** on ten million rows
(uncontended, best of five, warm cache). It now takes **2.65s**. Every step was
measured rather than reasoned about, and one of them was reverted:

| Change | Phase | Before | After |
|---|---|---:|---:|
| Pre-size the plain-value array from the footer's row count | compared columns | 1.31s | 1.17s |
| Fold eight bytes at a time instead of one | build key indexes | 1.77s | 1.79s |
| Prefetch the slot an insert or probe will land on | build + join | 1.79s / 1.21s | 1.39s / 0.88s |
| Fuse the index-rebase pass into the push; hoist two invariant branches | compared columns | 1.11s | 1.06s |
| **Put the slot table on huge pages** | build key indexes | 1.38s | **0.59s** |
| Put the column arrays on huge pages too | compared columns | 1.00s | 1.96s — **reverted** |

Uncontended phase profile, before and after:

| Phase | Before | After |
|---|---:|---:|
| read key columns | 0.26s | 0.22s |
| code key dictionaries | 0.00s | 0.00s |
| build key indexes | 1.77s | **0.59s** |
| join | 1.16s | **0.76s** |
| compared columns | 1.31s | **0.95s** |
| **wall** | **4.84s** | **2.65s** |

Four things are worth writing down.

**The hash was never the problem.** Widening `fold_bytes` to eight bytes a step
is the obvious optimisation for a phase that hashes twenty million keys, and it
did nothing at all — 1.77s to 1.79s. Splitting the phase said why: the parallel
hash sweep is 0.17s of it and the serial insertion is 1.23s. The wide fold is
still in the code because it costs nothing and helps a longer key, but it is
kept as a fact rather than as a saving.

**The insertion was a page-table problem, not a memory-latency one.** Prefetching
the next slot helped, as it should when every insert is a cache miss, but an
insert still cost 123 ns afterwards — far more than a DRAM access. At ten
million keys the table is 128 MB, which is 32,768 pages of 4 KB against a TLB
holding perhaps 1,500, so nearly every probe took a page walk *on top of* its
cache miss. Asking for 2 MB pages makes the same table 64 entries, and the phase
went from 1.38s to 0.59s. This was the single largest change here, and it is
three lines.

**Huge pages are about how often, not how big.** The column buffers are the same
eighty megabytes the slot table is, walked just as randomly, and putting them on
huge pages made things *worse* — 1.00s to 1.96s. The two slot tables are
allocated once each; a column buffer is allocated thirty-four times, on four
threads at once, and with `transparent_hugepage` set to `madvise` every one of
those asks makes the kernel compact memory to find a 2 MB run. The change was
reverted and the reason left in `grow()` so nobody tries it again.

**A hand-rolled bounds check is easy to get subtly wrong.** Replacing a
per-value dictionary-index check with one max-reduction over the page is a real
saving — a max has no loop-carried dependency, so it vectorises — but the first
version reduced over `int32_t`. A bit width of 32 can decode a value with the
top bit set, which is negative as `int32_t`, passes a signed max, and then
indexes the dictionary at a vast offset. The per-value check it replaced caught
that by accident, because the cast to `size_t` made it enormous. The reduction
is unsigned now.

## Layout

Three files. `csvdiff.c` is still one file in reading order; the Parquet path is
beside it rather than inside it, because a reader and a join have nothing to say
to a CSV parser.

| Section of `csvdiff.c` | What it holds |
|---|---|
| `Slab` | the mapped file and its `madvise` hint |
| SWAR helpers | `find_byte`, the packed `Field`, `logical_len`, `logical_copy` |
| `RowParser` | quoted fields, CRLF, ragged rows, the last row without a newline |
| `RowIndex` | open addressing, first-occurrence-wins, duplicate counting |
| `emit` | the report both paths end at, so the two cannot drift apart |
| `main` | the command line and the dispatch; exit 0 identical, 1 differences, 2 error |

| Section of `parquet.c` | What it holds |
|---|---|
| `Thrift` | the compact protocol, with a `bad` flag where C++ would throw |
| `FileMeta` | the footer: schema, row groups, chunk offsets and codecs |
| `PageHead` | v1 data pages and dictionary pages |
| `Rle` | the RLE / bit-packed hybrid, one 64-bit load per value |
| `plain_slices` | PLAIN byte arrays, straight into the mapping |
| `pq_read_column` | one column across every row group, degrading to plain if a page does |

| Section of `pqdiff.c` | What it holds |
|---|---|
| `Map` | the mapping, `MADV_WILLNEED` because columns are separate sequential runs |
| `Ids` | one id space for two dictionaries |
| `Keys` | the key columns, reduced to shared ids where both sides allow it |
| `Index` | the tag-in-slot table, and `lookup` |
| `run_parts` | one shape for every parallel phase; a thread that will not spawn runs inline |
| `scan_mask` | the mismatch mask, eight pairs at a time |
| `pq_compare` | keys, join, columns, rollup |
