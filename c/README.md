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
| **C** | **0.92s** | **0.93s** | 1.01s | **2.5s** | 754 MB |
| C++ | 1.03s | 1.06s | 1.13s | 2.8s | 765 MB |

**Ten million rows** (2,074 MB of input), five runs each:

| Build | Best | Median | Worst | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|
| **C** | **4.78s** | 5.87s | 8.27s | **13.1s** | 3,438 MB |
| C++ | 4.88s | **5.19s** | **5.63s** | 14.5s | 3,402 MB |

**C is about 12% faster at two million rows, and does about 10% less work at
both sizes.** 2.5s of CPU against 2.8s, and 13.1s against 14.5s. That is the
one measurement here that is stable across sizes and runs.

**At ten million rows the two are even, and C is much more variable** — 8.27s at
worst against C++'s 5.63s. That spread is memory, not code: 2 GB of mapped input
plus 3.4 GB of working set, run back to back, evicts page cache, and whichever
binary runs against a cold cache pays for it. At two million rows, where nothing
is under pressure, C's spread is 0.09s. Reporting the ten-million best as a win
for C would be reporting the noise.

**The format is worth 4.6x, and it is the same 4.6x in both languages.** The
same two million rows as CSV, same machine, same runs:

| Build | Format | Compare | CPU |
|---|---|---:|---:|
| C | CSV | 11.55s | 11.5s |
| C | Parquet | **0.92s** | **2.5s** |
| C++ | CSV | 5.05s | 13.2s |
| C++ | Parquet | **1.03s** | **2.8s** |

Wall clock says Parquet is worth 12.6x in C and 4.9x in C++, but that comparison
is contaminated: the C **CSV** path is single-threaded and the C **Parquet**
path is not, so most of that 12.6x is threads. CPU time removes the threading
and leaves the format on its own — 11.5s to 2.5s in C, 13.2s to 2.8s in C++.
**4.6x and 4.7x: the format is worth the same in both languages, which is
another way of saying it is not a language question at all.**

Where the time goes, ten million rows, uncompressed Parquet:

| Phase | C | C++ |
|---|---:|---:|
| read key columns | 0.32s | 0.23s |
| code key dictionaries | 0.00s | 0.00s |
| build key indexes | 2.00s | 1.21s |
| join | 1.50s | 1.92s |
| compared columns | 2.79s | 2.24s |

The two split the join differently — C spends more building the indexes and less
probing them — but index plus join is 3.50s against 3.13s, and the phase
boundary between them is partly a question of which phase pays for faulting the
pages in. `code key dictionaries` is zero in both because `account_id` and
`txn_id` are high-cardinality enough that the writer gives up on a dictionary
for them, so there is nothing to intern; on `currency` and `status` it is the
whole comparison.

One bug worth naming, because it cost a second and looked like a language gap.
The two key indexes were built one after the other, each on half the cores. The
hash sweep inside a build is parallel but the insertion after it is serial, so
run in sequence the machine sits half idle through both serial tails. Overlapped
— one side's insertion against the other side's sweep — that phase went from
2.18s to 2.00s and the port went from losing to even.

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
