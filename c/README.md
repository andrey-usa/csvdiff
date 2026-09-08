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

## The text design

Both files are `mmap`ed. A field is one `uint64_t`: 40 bits of offset, 23 bits
of length, and the top bit set when the field is escaped and has to be decoded
before it is read. Nothing becomes a heap-allocated string except the column
names in the summary.

**Rows are found on every core, and inserted on one.** A row's hash depends on
nothing but that row, so finding and hashing divides perfectly; insertion does
not, because first-occurrence-wins depends on the order rows arrive, and
threading it would make the answer depend on the scheduler.

The difficulty in dividing is that a thread starting mid-file cannot tell
whether it is inside a quoted field, and a newline in one is not a row boundary.
Parity settles it: every `"` toggles in-quote state — including both halves of a
doubled quote, which toggles twice and so leaves it alone, which is exactly
right — so counting quotes before a nominal split says whether it is inside a
field, and the boundary walks forward from there to a real row start. Counting
one byte is far cheaper than parsing.

**CSV and JSON differ in two places and nowhere else.** How a row is found: CSV
scans for an unquoted newline, JSON walks one object per line. And how a value
is read back: CSV doubles a quote to escape it, JSON puts a backslash in front,
and `\uXXXX` becomes UTF-8 — because a JSON writer may escape a character or
write it literally, and the two spell the same value, so they must compare
equal. Everything above that — the hash, the table, the join, the counts — does
not know which it is reading.

A JSON object is addressed by key where a CSV row is addressed by column number,
so each parser holds the names it wants in a small open-addressed table: a key
in the file costs one hash rather than a walk of twenty names, which at twenty
columns would be four hundred comparisons a row. A JSON file has no header row,
so its column names are the keys of its first object.

## The CSV specifics

Delimiters are found eight bytes at a time — `diff = word ^ broadcast(c)`, then
`(diff - 0x0101…) & ~diff & 0x8080…`, then `trailing_zeros >> 3`. Once the last
needed column has been read the rest of the row is skipped to its newline: on
twenty columns keyed on the first two, most of a row is never delimited.

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
- **A JSON file's columns are the keys of its first object**, so a key that
  appears only in later objects is not a column. That is the rule the C++ port
  uses too, and it is the only one that costs a line rather than a pass over the
  file. A nested object or array is not a cell value and is left absent rather
  than guessed at.

`test.sh` holds it to the Rust port's answers on `tests/fixtures/awkward_*.csv`
— the fixture assembled from every shape that has broken an engine in this
project — including the per-column `changed` / `blanked` / `filled` counts.

## What the floor turned out to be

Two million rows of CSV, interleaved runs, one 4-core container. The two mapped
inputs are 702 MB and mapped pages count as resident, so the column that
carries information is the last one: what the engine allocates on top of the
files it is reading.

| Port | Best | Median | CPU | Peak RSS | Above the mapped files |
|---|---:|---:|---:|---:|---:|
| **C** | **1.95s** | **2.01s** | **7.0s** | 827 MB | 126 MB |
| C++ | 3.85s | 4.16s | 10.2s | 934 MB | 232 MB |
| Zig | 4.53s | 4.57s | 7.9s | 827 MB | **125 MB** |
| Rust | 9.45s | 10.13s | 9.4s | 999 MB | 298 MB |

**C is not the floor — it ties with Zig.** 126 MB against 125 MB. That was the
finding when this port was single-threaded and slowest of the four, and it
survives the port becoming the fastest, which is the point: the floor belongs to
the *design*, not to the language. Once the row index, the offset array and the
hash table are sized the same way there is nothing left for a language to save.

**The compiler moves more than it used to.** The table above is built with `cc`,
which is gcc here. The same source under clang:

| Build | Best | Median | CPU |
|---|---:|---:|---:|
| gcc 13.3 | 1.99s | 2.03s | 7.0s |
| **clang 18.1** | **1.47s** | **1.56s** | **5.2s** |

1.35x, where on the single-threaded version of this file it was 1.12x. Any
single-number comparison of C against Rust against Zig that does not say which
compiler produced each binary is reporting the toolchain and calling it the
language — and the margin for that mistake grew when the code got harder to
optimise.

## What threading the text path was worth

The same two million rows, before and after, interleaved in one sitting:

| Build | Best | Median | Worst | CPU |
|---|---:|---:|---:|---:|
| serial | 8.08s | 8.23s | 8.58s | 8.1s |
| rows found on every core | 6.01s | 6.20s | 6.39s | 7.6s |
| **and compared on every core** | **1.91s** | **1.92s** | **2.15s** | **6.9s** |

**4.2x, and most of it is the second step rather than the first.** Splitting the
sweep is the change that looks like the optimisation — it is where the parsing
is — and on its own it was worth 1.34x. The comparison after it was still
running on one thread, re-parsing two rows per key and re-hashing a key the
sweep had already hashed, and that was three quarters of the run.

CPU falls as well as wall clock, from 8.1s to 6.9s, which is a different saving:
the table is now sized once from the row count instead of doubling from 4,096
(thirteen rehashes at ten million rows, each a full pass of random probes), and
each key is hashed once rather than twice.

## Parsing what is read, and nothing else

Threading made the text path fast enough to see what it was actually doing, and
what it was doing was parsing the same row about seven times per key. Two of
those were the sweep; three more were the A side of the join — the row, the
probe that confirms a hash match, and the mate; two more were the added-key
pass over B. Every one of them delimited all twenty columns.

Most of them did not need twenty. The sweep hashes the key and remembers where
the row starts; the probe compares keys. Only the two parses that feed the
per-cell diff want the whole row. So there is a **key-only parse** now: it reads
to the last key column and then scans once to the newline, which on a
twenty-column file keyed on the first two is two delimiters instead of twenty.
Five of the seven parses go through it.

The second one is smaller in the source and was worth about as much. Placing a
parsed field meant finding which slot wanted it:

```c
for (size_t i = 0; i < p->width; i++)
    if (p->source[i] == column) out[i] = field;
```

That is `width` comparisons per column and so `width * width` per row — four
hundred at twenty columns, eight billion over a ten-million-row pair. It is
also, exactly, the four hundred comparisons the JSON path already had a hash
table to avoid; the CSV path had simply never been given the same treatment.
`source` is inverted once into `col_first[column]` plus a chain, and a column
costs one load.

Two million rows, interleaved, one sitting:

| Build | Best | Median | Worst | CPU |
|---|---:|---:|---:|---:|
| **after** | **1.01s** | **1.19s** | **1.23s** | **3.2s** |
| before | 2.69s | 2.89s | 3.15s | 9.7s |

**2.66x on wall, 3.03x on CPU.** The CPU figure is the one that matters: this
removed work rather than spreading it, which is the only kind of saving that
still helps when the cores run out.

**At ten million rows on a four-core runner it is 3.65x** — 14.82s to 4.06s,
CPU 57.3s to 14.5s. The gain grew with the size, which is the part worth
understanding: past the point where the index stops fitting in cache, a parse
that touches twenty fields instead of two is not only more instructions, it is
memory it then has to fetch back. Two million rows underestimated it by a third.
That run also put this port first on all three formats, and on CSV it now uses
the least CPU of any build measured — 14.5s against the next one's 22.4 — so it
is not winning on threading.

ndjson gets 1.06x from the same change, and the reason is worth stating. A JSON
object has to be walked to its closing brace whatever you want out of it, so
reading only the keys saves the stores and not the scan. Stopping the walk once
the keys are found would save the rest — but the full parse takes the *last*
value of a repeated key and an early exit would take the first, and the two have
to agree on what a row's key is or the lookups miss. That is a deliberate
change, not a tweak.

**What did not change: the answer.** Counts are identical to the previous
binary's on every fixture, and `test.sh` grew four checks that the old code
would have passed and that the new code could plausibly have broken — a key in
the third of four columns, a key in the last, two keys at both ends, and
everything after the key ignored. The first version of that helper read the
previous case's report when a run refused its flags, and reported the previous
case's answer as this one's; it deletes the file first now and treats a missing
one as a failure.

## The generator against the C++ one

Both write the same bytes, so this is a clean measurement of two
implementations of one recipe. Two million rows, interleaved, on one four-core
container:

| Format | C | C++ | C CPU | C++ CPU |
|---|---:|---:|---:|---:|
| CSV | 2.53s | **1.83s** | **2.5s** | 3.3s |
| ndjson | 5.55s | **3.19s** | 5.5s | 5.4s |
| Parquet | 4.80s | **1.74s** | 4.8s | 4.6s |

**C++ is 1.4x to 2.8x faster on wall clock, and the whole of it is threading.**
Per CPU-second the two are within a few per cent on every format, and on CSV the
C generator does slightly less work. The C++ generator formats rows in waves
across every core and spreads a row group's per-column work the same way; this
one runs on one thread, because generating fixtures is not the thing this port
exists to be fast at and a second threading design is a second thing to get
wrong.

Two defects turned up in writing that table, both mine, neither visible from
the output because the bytes were right the whole time:

**The dictionary was built by scanning what had been seen.** O(rows x distinct),
which at 122,880 rows a row group and a dictionary of 8,192 is a billion
comparisons per column. Parquet generation took **113 seconds** at two million
rows against the C++ generator's 1.9. Through an open-addressed table it is
4.8s -- **24x** -- and the files are byte for byte what they were.

**The text writers called `fwrite` once per row**, and looked up each column
name's length with `strlen` twenty times a row. A row is a couple of hundred
bytes, so the per-call cost was being paid two million times for a memcpy either
way. One megabyte of output per write, and the name lengths computed once, took
ndjson from 8.29s to 5.55s.

The lesson is the one the rest of this file keeps finding: `cmp` said the
generator was correct from the first commit, and correctness said nothing at all
about whether it was fast. Neither did the test suite, which generates at most
sixty thousand rows -- where the quadratic dictionary costs 40 ms and hides.

## Newline-delimited JSON

The same rows, in the shape a log pipeline emits. Two million of them are 1,697
MB against CSV's 702 MB, which is most of the difference in the numbers:

| Port | Best | Median | CPU | Peak RSS |
|---|---:|---:|---:|---:|
| **C** | **3.17s** | **3.23s** | **10.8s** | **1,823 MB** |
| C++ | 4.51s | 4.76s | 11.6s | 1,930 MB |

Only these two ports read it — on `main` the Rust and Zig ports refuse an
ndjson file — so this table has two rows rather than four, and `test.sh`
cross-checks the JSON path against C++ rather than against Rust for the same
reason. A check against a port that refuses the file would be a check that
always passes.

## What Parquet turned out to be worth

One four-core container, 16 GB. Every table below comes from a single sitting
with the ports **interleaved** — each runs once per round, and the rounds are
what repeat. That is not fussiness: this machine's speed drifts under the runs
themselves, as the page cache fills and the kernel's supply of free 2 MB pages
is picked over and replenished, so a number taken now and one taken twenty
minutes ago compare machine states rather than builds. An earlier draft of this
file quoted a 1.83x speedup that was really 1.50x for exactly that reason.

`scripts/bench_ports_parquet.py` produces these, and fails if the ports disagree
about how many rows changed.

**Ten million rows, uncompressed Parquet** (2,074 MB of input), five rounds:

| Port | Best | Median | Worst | CPU | Peak RSS | Above the input |
|---|---:|---:|---:|---:|---:|---:|
| **C** | **2.78s** | **3.02s** | **3.37s** | **7.4s** | 3,414 MB | 1,341 MB |
| C++ | 3.99s | 4.03s | 4.34s | 11.9s | 3,477 MB | 1,404 MB |
| Rust | 5.30s | 5.40s | 5.88s | 13.9s | 3,436 MB | 1,362 MB |
| Zig | 9.49s | 10.28s | 10.64s | 13.0s | 3,419 MB | 1,346 MB |

**Two million rows, uncompressed Parquet** (415 MB), five rounds:

| Port | Best | Median | Worst | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|
| **C** | **0.63s** | **0.67s** | **0.75s** | **1.4s** | 728 MB |
| C++ | 0.83s | 0.87s | 0.91s | 2.3s | 767 MB |
| Rust | 1.39s | 1.42s | 1.46s | 3.0s | 839 MB |
| Zig | 1.94s | 1.99s | 2.03s | 2.7s | **695 MB** |

**The same two million rows as CSV** (702 MB), four rounds — the control:

| Port | Best | Median | CPU | Peak RSS |
|---|---:|---:|---:|---:|
| C | 7.89s | 8.46s | 7.9s | 839 MB |
| C++ | **3.77s** | **3.79s** | 9.9s | 942 MB |
| Rust | 8.63s | 8.98s | 8.6s | 1,000 MB |
| Zig | 4.42s | 4.80s | **7.7s** | **827 MB** |

Three things fall out of putting those side by side.

**The format is worth 2.9x to 5.6x, in every language.** Measured in CPU rather
than wall clock, because the ports thread the two paths differently and wall
clock would be reporting that instead:

| Port | CSV | Parquet | The format is worth |
|---|---:|---:|---:|
| C | 7.9s | 1.4s | **5.6x** |
| C++ | 9.9s | 2.3s | **4.3x** |
| Rust | 8.6s | 3.0s | **2.9x** |
| Zig | 7.7s | 2.7s | **2.9x** |

**The order changes completely between the two formats.** On CSV this port is
slowest of the four and C++ is twice as quick; on Parquet it is the fastest and
C++ is half a second behind. Nothing about either language changed — the C CSV
path is single-threaded and its Parquet path is not, and Parquet moves the work
from parsing bytes to walking arrays, which is a different problem with
different winners. A ranking of languages taken from one format is a ranking of
that format's implementations.

**Peak memory is the same everywhere, to within 2%.** 3,414 to 3,477 MB across
four languages at ten million rows. That is the design, not the language, and it
is the same conclusion this port reached on CSV.

## Where the time went, and what moved it

The first working version of this path and the current one, built from the same
source tree and run interleaved in one sitting, ten million rows:

| Build | Best | Median | Worst | CPU | Peak RSS |
|---|---:|---:|---:|---:|---:|
| before tuning (`34775da`) | 3.83s | 4.23s | 4.41s | 10.6s | 3,468 MB |
| **after tuning** | **2.56s** | **3.06s** | **3.28s** | **6.8s** | **3,403 MB** |

**1.50x on wall clock and 1.56x on CPU.** Phase by phase, uncontended:

| Phase | Before | After |
|---|---:|---:|
| read key columns | 0.26s | 0.22s |
| code key dictionaries | 0.00s | 0.00s |
| build key indexes | 1.77s | **0.59s** |
| join | 1.16s | **0.76s** |
| compared columns | 1.31s | **0.95s** |

Six changes were kept and three were measured and thrown away:

| Change | Phase | Verdict |
|---|---|---|
| Pre-size the plain-value array from the footer's row count | compared columns | 1.31s → 1.17s |
| Prefetch the slot an insert or probe will land on | build + join | 1.79 → 1.39s, 1.21 → 0.88s |
| Fuse the index-rebase pass into the push; hoist two invariant branches | compared columns | 1.11s → 1.06s |
| **Put the slot table on huge pages** | build key indexes | **1.38s → 0.59s** |
| Fold eight bytes at a time instead of one | build key indexes | 1.77s → 1.79s — kept, but it does nothing |
| Put the *column* arrays on huge pages too | compared columns | 1.00s → 1.96s — **reverted** |
| Put the *key column* arrays on huge pages | join | 2.03s → 2.79s overall — **reverted** |
| Shard the index so insertion runs on every core | build key indexes | 1.95s → 2.84s overall — **reverted** |

Four things worth writing down.

**The insertion was a page-table problem, not a memory-latency one.** Prefetching
the next slot helped, as it should when every insert is a cache miss, but an
insert still cost 123 ns afterwards — far more than a DRAM access. At ten million
keys the table is 128 MB, which is 32,768 pages of 4 KB against a TLB holding
perhaps 1,500, so nearly every probe took a page walk *on top of* its cache miss.
Asking for 2 MB pages makes the same table 64 entries: 1.38s to 0.59s, in three
lines, and it is the largest single change here.

**Huge pages are about how often, not how big — measured three times.** The
column buffers are the same eighty megabytes the slot table is, walked just as
randomly, and huge-paging them made things *worse*: 1.00s to 1.96s. So was the
narrower version that took only the four key columns, which are read once and
probed at random rows and looked like the ideal case: 2.03s to 2.79s. With
`transparent_hugepage` set to `madvise` every request makes the kernel compact
memory to find a 2 MB run, and that cost scales with the number of asks, not
their size. Two allocations win; six lose; sixteen lose badly.

**Sharding the index cost more than the serial insertion it removed.** Insertion
is serial within a side, so only two of four cores work through it — the obvious
fix is to split the table by hash, which is safe here because equal keys always
hash into the same shard and so first-occurrence-wins survives. It was built,
and it was 2.84s against 1.95s. The CPU column said why: 9.2s against 5.8s.
Routing rows to shards means either scanning every row once per shard, which is
eight times the sequential reads, or bucketing them first, which needs 120 MB a
side. The prize was the 0.18s of serial insertion left after the huge-page fix;
the cheapest routing that gets it costs more than that. Reverted.

**The obvious optimisation did nothing.** Widening `fold_bytes` to eight bytes a
step is what you would do first to a phase that hashes twenty million keys, and
it measured 1.77s to 1.79s. Splitting the phase said why: the parallel hash sweep
is 0.17s of it and the serial insertion is 1.23s. It stays because it costs
nothing and helps a longer key, but as a fact rather than a saving.

One bug found by writing it down. Replacing a per-value dictionary-index check
with a single max-reduction is a real saving — a max has no loop-carried
dependency, so it vectorises — but the first version reduced over `int32_t`. A
bit width of 32 can decode a value with the top bit set, which is negative,
passes a signed max, and then indexes the dictionary at a vast offset. The
per-value check it replaced caught that by accident, because the cast to
`size_t` made it enormous. The reduction is unsigned now.

## Layout

Four files. `csvdiff.c` is still one file in reading order; the Parquet reader
and its join sit beside it rather than inside it, because neither has anything
to say to a text parser.

| Section of `csvdiff.c` | What it holds |
|---|---|
| `Slab` | the mapped file, its `madvise` hint, and which dialect it is |
| SWAR helpers | `find_byte`, the packed `Field`, `logical_len`, `logical_copy` |
| JSON | object scanning, and `\uXXXX` and backslash escapes decoded to UTF-8 |
| `RowParser` | quoted fields, CRLF, ragged rows, JSON objects addressed by key |
| `chunk_bounds` / `sweep_part` | splitting a file at real row starts, by quote parity |
| `RowIndex` | open addressing, first-occurrence-wins, duplicate counting |
| `compare_part` | the join and the per-column counts, over ranges of one side's keys |
| `emit` | the report every path ends at, so they cannot drift apart |
| `main` | the command line and the dispatch; exit 0 identical, 1 differences, 2 error |

| Section of `parallel.c` | What it holds |
|---|---|
| `run_parts` | one shape for every parallel phase; a thread that will not spawn runs inline |
| `alloc_huge` | 2 MB pages, for the rare long-lived allocation that is probed at random |

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
