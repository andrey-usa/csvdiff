# csvdiff — C++

The byte-level comparison, in the language people reach for when they want the
fastest thing they can read. Same design as the Java and Rust `turbo` engines:
the file is mapped, a field is an offset and a length packed into one word,
delimiters are found eight bytes at a time with SWAR, and nothing becomes a
`std::string` unless it reaches the report.

```bash
make                     # g++ by default
CXX=clang++ make         # or clang
build/csvdiff compare a.csv b.csv -k id --json summary.json
```

## What it is and is not

This is a **benchmark and parity port**: it carries the comparison and the JSON
half of the result contract, not the HTML report. The five full ports already
produce that; what is interesting here is the engine.

Two limitations, both stated rather than papered over:

- **`--ignore-case` is ASCII-only.** Folding case outside ASCII needs a Unicode
  table this port does not carry, and folding it partially is worse than not
  folding it at all: `CAFÉ` and `café` would compare equal in the ports that do
  fold and unequal here, with nothing in the output to say why. A non-ASCII byte
  in a folded field is refused by name instead.
- **No `--export-dir` and no `--profile`.** Neither affects the numbers.

## The compiler is worth more than the language

On a million rows, best of three, one 4-core container:

| Build | Compare | Peak RSS |
|---|---:|---:|
| `clang++ 18` | **3.59s** | 509 MB |
| `g++ 13` | 6.49s | 509 MB |

Same source, same flags (`-std=c++20 -O2 -march=native`), **1.8x apart**. For
comparison, Rust and Java running the same design land at 5.53s and 5.55s — so
whether this port is the fastest thing in the project or the slowest of the three
is decided by which compiler built it, not by the language it is written in.

That is worth knowing before reading any single-number language comparison,
including the ones in this repository's own README.

## Layout

| File | What it holds |
|---|---|
| `src/csvdiff.hpp` | the contract: options, counts, column stats, result |
| `src/csvdiff.cpp` | SWAR scanning, the mapped slab, the parser, the index, the join |
| `src/json.cpp` | the JSON half of the result contract, written by hand |
| `src/main.cpp` | the command line; exit 0 identical, 1 differences, 2 error |
