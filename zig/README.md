# csvdiff — Zig

The byte-level comparison with **the memory it may use passed in**, not assumed.

Same design as the Java, Rust and C++ `turbo` engines: the file is mapped, a
field is an offset and a length packed into one word, delimiters are found eight
bytes at a time with SWAR, and nothing becomes a string unless it reaches the
report.

```bash
zig build --release=fast
zig-out/bin/csvdiff compare a.csv b.csv -k id --json summary.json
zig-out/bin/csvdiff compare a.csv b.csv -k id --max-memory 256
```

## Why this port exists

Every other implementation here can only be *measured* for memory and hoped
about. This one takes an allocator, so `--max-memory` is not a target the engine
tries to respect — it is a `FixedBufferAllocator` that cannot hand out more than
it was given. A comparison that would exceed the budget fails at the allocation
that would have crossed the line, naming the budget:

```
$ csvdiff compare 1m_a.csv 1m_b.csv -k account_id,txn_id -i updated_at --max-memory 192
error: the comparison needs more than the 192 MB it was given
$ csvdiff compare 1m_a.csv 1m_b.csv -k account_id,txn_id -i updated_at --max-memory 256
A 1000100 rows | B 1000050 rows | matched 999000 (changed 60049) | ...
```

Bounded memory stops being a number someone reads afterwards and becomes a
property the program cannot violate. On a million rows the threshold is between
192 and 256 MB, and the answer either arrives correct or does not arrive.

## The build mode is worth more than the language

On a million rows, best of three, one 4-core container:

| Build | Compare | Peak RSS |
|---|---:|---:|
| `zig build --release=fast` | **5.13s** | 413 MB |
| `zig build` (Debug, the default) | 32.74s | 420 MB |

**6.4x apart** from the same source. `zig build` with no arguments produces the
slow one, and a `preferred_optimize_mode` in `build.zig` does not change that —
it sets the default for `-Drelease`, which Zig 0.16 spells `--release`.

Against the other ports of the same design:

| | Compare | Peak RSS |
|---|---:|---:|
| C++ (clang 18) | **3.39s** | 509 MB |
| **Zig 0.16** | 5.13s | **413 MB** |
| Rust | 5.18s | 583 MB |
| Java | 5.20s | 632 MB |

Zig, Rust and Java land within 1.4% of each other — one measurement's worth of
noise. Zig holds the least memory of the four. C++ built with clang is half as
fast again as any of them, and built with gcc it is the slowest; see
`../cpp/README.md`.

## What it is and is not

A **benchmark and parity port**: the comparison and a JSON summary, not the HTML
report the five full ports produce.

`--ignore-case` is ASCII-only. Folding outside ASCII needs a Unicode table this
port does not carry, and folding partially is worse than not folding at all —
`CAFÉ` and `café` would compare equal in the ports that do fold and unequal here,
with nothing in the output to say why. A non-ASCII byte in a folded field is
refused by name instead.

## Layout

| File | What it holds |
|---|---|
| `src/scan.zig` | SWAR scanning, with its own unit tests (`zig build test`) |
| `src/csvdiff.zig` | the mapped slab, the parser, the index, the join |
| `src/main.zig` | the command line and the memory budget |
