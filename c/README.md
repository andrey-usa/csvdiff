# csvdiff — C

The same byte-level comparison as the Rust, Java, C++ and Zig `turbo` engines,
in one file of C99-shaped C11, to find out what the memory floor actually is
when nothing is allocated that the design does not require.

```bash
make                       # cc, whatever that is
CC=clang make              # or clang, which is worth 0.6s here
./csvdiff compare a.csv b.csv -k id --json summary.json
```

## The design, unchanged

Both files are `mmap`ed with `MADV_SEQUENTIAL`. A field is one `uint64_t`:
40 bits of offset, 23 bits of length, and the top bit set when the field
contains a doubled quote and has to be unescaped before it is read. Delimiters
are found eight bytes at a time — `diff = word ^ broadcast(c)`, then
`(diff - 0x0101…) & ~diff & 0x8080…`, then `trailing_zeros >> 3`. Keys go into
an open-addressing table sized to a power of two. Nothing becomes a
heap-allocated string except the column names in the summary.

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

## Layout

One file, in reading order:

| Section of `csvdiff.c` | What it holds |
|---|---|
| `Slab` | the mapped file and its `madvise` hint |
| SWAR helpers | `find_byte`, the packed `Field`, `logical_len`, `logical_copy` |
| `RowParser` | quoted fields, CRLF, ragged rows, the last row without a newline |
| `RowIndex` | open addressing, first-occurrence-wins, duplicate counting |
| `main` | the command line; exit 0 identical, 1 differences, 2 error |
