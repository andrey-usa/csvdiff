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
(4 vCPU / 16 GB), every port built from this tree and compiled for that runner,
all four interleaved in one sitting, five rounds each, 2026-09-09. Each cell is
**wall · CPU · memory above the mapped input**.

| Format | Input | C | C++ | Rust | Zig |
|---|---:|---|---|---|---|
| CSV | 3,509 MB | **2.14s** · 7.7s · 716 MB | 5.19s · 16.2s · 878 MB | 2.46s · 8.2s · 899 MB | 2.71s · 9.7s · 887 MB |
| ndjson | 8,487 MB | **7.40s** · 24.3s · 716 MB | 20.53s · 67.8s · 880 MB | 14.32s · 55.5s · 899 MB | 13.88s · 54.3s · 890 MB |
| Parquet | 2,074 MB | **1.48s** · 4.8s · 1,297 MB | 3.43s · 11.3s · 1,480 MB | 2.80s · 9.1s · 1,474 MB | 2.96s · 10.5s · **1,276 MB** |

All four returned identical counts — matched 9,990,000, changed 599,320, added
10,000, removed 10,000, duplicate keys 1,000 in A and 500 in B. That is the
run's gate, not a footnote: builds that disagree about how many rows changed
mean a bug in one of them, so the run fails and names it.

Three things this table says.

**On CSV the four ports have almost converged.** C leads Rust by 1.15x and does
7.7 CPU-seconds against its 8.2 — a gap you would not design around. That is
what a text path looks like once every port stops parsing columns nobody reads
and stops comparing rows it can prove equal from their bytes. The interesting
column is no longer the fastest one.

**ndjson is where the ports still differ**, and by 1.9x: C spends 24.3 CPU
seconds where the next build spends 54.3. The proof that settles a CSV row from
its raw bytes has to rule out a repeated name before it can settle a JSON one,
and only one port does that so far.

**Memory is the flattest ranking and the widest margin.** 716 MB above the
mapped input on both text formats against 878 and up — a field here is one
64-bit word and never becomes a string. Parquet is the exception, and the one
row C does not lead: Zig's reader peaks 21 MB lower.

> **Where the columns come from.** All four are built from this tree, on one
> runner, in one sitting — the two branches that used to be measured against
> each other were merged, so there is no second checkout and no pinned commit
> any more. Every port is compiled for the machine it runs on: `-march=native`
> for C and C++, `-C target-cpu=native` for Rust, `-Dcpu=native` for Zig.
>
> Compare rows within this table. Not with the tables in
> [BENCHMARKS.md](BENCHMARKS.md) above or below it: this runner's ndjson
> numbers moved 20% between two runs one morning with no code change at all.
---

## Building and running

> **Run every command in this section from the repository root** — the directory
> holding `c/`, `rust/` and this file. Every path below is written relative to it,
> in bash and in PowerShell alike, so that one rule covers the whole section.
>
> ```powershell
> cd C:\path\to\csvdiff     # PowerShell: the folder holding c\, rust\ and README.md
> ```
> ```bash
> cd ~/path/to/csvdiff       # bash
> ```
>
> If you are inside a port's directory, `cd ..` first. From `c\`, `c/gen-data`
> is `No such file or directory`, and `.\gen-data` would run but write its pair
> into `c\data\` rather than the `data\` the commands below then read.

### What runs where

Every cell below has a job behind it. `platforms.yml` runs all four ports on
Linux x86-64, Linux aarch64 and macOS on Apple silicon, and three of them on
Windows; the Rust port's Windows lane lives in `ci-rust.yml`.

| Port | Linux x86-64 | Linux aarch64 | macOS (arm64) | Windows |
|---|---|---|---|---|
| **C** | suite | suite | suite | native, agrees with Rust |
| **C++** | suite | suite | suite | native, agrees with Rust |
| **Rust** | suite | suite | suite | native, suite |
| **Zig** | suite | suite | suite | native, agrees with Rust |

"suite" means that port's own test suite ran there and passed — `c/test.sh`,
`cpp/test.sh`, `zig/test.sh`, `cargo test`. The three Windows cells are weaker
on purpose and the difference is worth knowing: those suites are written for a
POSIX shell and reach for binaries by their extensionless names, so they do not
run there yet. What runs instead is `scripts/win_smoke.sh`, which asks the
question those suites ask — the generator's bytes, then answers over CSV,
ndjson and Parquet at one and four threads, each against the Rust port on the
same files. Enough to catch a wrong answer, not the same coverage.

No WSL in the table any more. The three POSIX ports build natively on Windows
with mingw-w64 gcc: the mapping calls are `CreateFileMapping` and a view of it
(`c/win32.h`, `cpp/src/win32.hpp`, and `mapHandle` in `zig/src/slab.zig`), the
C port's threads are `CreateThread`, and every `open` carries `O_BINARY` so the
generator's files stay byte-identical. WSL still works and is still a fine way
to run the bash suites; it is no longer the only way to run the ports.

The Rust row taught the rule the rest of this table follows. It first said
"native" on the strength of a grep for `std::os::unix` finding nothing — which
cannot see inside a dependency, and `memmap2::Advice` is gated `#[cfg(unix)]` in
that crate, so the port did not compile on Windows at all. A reader hit it
within the hour. A claim about a platform is worth what the runner that proves
it is worth, and nothing above is claimed without one.

`test.sh` and `scripts/bench_ab.sh` are bash. On Windows they need WSL, MSYS2 or
Git Bash; `scripts/bench_ports.py` needs Python 3.

### Building

Each port builds from its own directory with its own toolchain, in seconds, and
pulls in nothing that contains a comparison engine.

```bash
# Linux, macOS, or WSL
(cd c    && make)                    # cc or clang, C11. Also builds c/gen-data
(cd cpp  && make)                    # g++ or clang++, C++20
(cd rust && cargo build --release)   # edition 2024. Also builds a gen-data
(cd zig  && zig build --release=fast)
```

```powershell
# Windows, PowerShell. The Rust port needs nothing but cargo.
# --manifest-path so this runs from the root like everything else; the
# binaries still land in rust\target\release\.
cargo build --release --manifest-path rust\Cargo.toml
```

The C, C++ and Zig ports build natively on Windows too. The first two want a
POSIX-ish shell for `make`, which is what [MSYS2](https://www.msys2.org/)
provides — `pacman -S mingw-w64-x86_64-gcc make`, then from its **MINGW64**
shell:

```bash
CC=gcc  make -C c    && make -C c   gen-data
CXX=g++ make -C cpp  && make -C cpp gen-data
```

The Zig port needs no shell at all, since `zig build` is the whole build system:

```powershell
cd zig; zig build --release=fast; cd ..
```

WSL is still a good way to run the bash test suites, and it is what the next
section is about. It is no longer needed to build anything. A fresh WSL has
neither a current Rust nor any Zig at all, and the reasons are not obvious from
inside it, so read on before you start.

### If you are on WSL

**WSL is a second machine.** It has its own filesystem, its own package manager
and its own toolchains, and it shares none of them with Windows. A rustup
install on the Windows side puts `rustc.exe` in your Windows profile; those are
Windows binaries that build Windows executables, and a Linux `cargo build`
cannot use them. So "Rust is new on Windows but old in WSL" is not a
misconfiguration — they are two separate installations, and only one of them
has been kept current.

What makes it confusing is that WSL appends the Windows `PATH` to its own, so
`cargo.exe` and `rustc.exe` **are** found from inside WSL. The toolchain looks
present. It just is not a Linux one.

Two specific symptoms, and what causes each:

| Symptom | Cause |
|---|---|
| `error: rustc 1.86.0 is not supported by the following packages: csvdiff@1.0.0 requires rustc 1.90` | Rust came from `apt install rustc cargo`. Distro packages are frozen at whatever the release shipped and never follow stable. |
| `zig: command not found` | Zig is not in Ubuntu's default repositories in a usable version. It is normally installed by unpacking a tarball, so a fresh WSL has none. |

Fix both from inside WSL. **Rust**, via rustup rather than apt:

```bash
# inside WSL -- where is the old one coming from?
which -a rustc                           # /usr/bin/rustc means apt installed it

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
rustc --version                          # 1.90 or newer
```

rustup installs to `~/.cargo/bin` and puts it at the front of `PATH`, which is
usually enough on its own. If `rustc --version` still reports the old one, open
a new shell; if that does not do it, `/usr/bin` is still winning:

```bash
export PATH="$HOME/.cargo/bin:$PATH"     # add to ~/.bashrc to keep it
```

Removing apt's copy with `sudo apt remove rustc cargo` also works, but check
what else goes with it first — `apt` takes reverse-dependencies too, and
shadowing it by `PATH` is the smaller change.

**Zig 0.16.0**, the version CI builds with:

```bash
# inside WSL
curl -sSL https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz \
  | sudo tar -xJ -C /opt
echo 'export PATH="/opt/zig-x86_64-linux-0.16.0:$PATH"' >> ~/.bashrc
exec bash
zig version                              # 0.16.0
```

Without `sudo`, or on a machine where `/opt` is not yours to write to, the same
release is on PyPI and needs no root:

```bash
pip install ziglang==0.16.0
mkdir -p ~/.local/bin
printf '#!/bin/sh\nexec python3 -m ziglang "$@"\n' > ~/.local/bin/zig
chmod +x ~/.local/bin/zig
```

Both routes are what the `parity` and `formats` workflows use, so they are the
two that are known to work.

> **Clone into the WSL home directory, not `/mnt/c/`.** A Windows drive is
> reachable from WSL as `/mnt/c/Users/you/...`, but it is a network filesystem
> underneath, and it is slow enough to change what you measure — which for a
> repository whose whole subject is throughput makes every number here
> meaningless. `git clone` into `~/` and work there. To open the result in a
> Windows editor, `\\wsl$\Ubuntu\home\you\csvdiff` reaches it from the
> Windows side.

It is not only slow there. The Zig build is the first thing to break on a
Windows drive, and it does not break in a way that names the cause:

```
error: failed to rename compilation results ('.zig-cache/tmp/14aaf8cf27fc0150')
  into local cache ('.zig-cache/o/7691fdba880312cb541a0fac7ef59a22'): AccessDenied
```

Zig commits a build by renaming it into `.zig-cache/o/` and hardlinking, and
the Windows drive mount supports neither properly. Two commands say which of
the two causes you have:

```bash
pwd                          # under /mnt/c ?
ls -ld .zig-cache .zig-cache/o
```

| What you see | Fix |
|---|---|
| `pwd` is under `/mnt/c` | Point the cache at a native path. The source tree can stay on the Windows drive; only `.zig-cache` cannot. |
| `.zig-cache` is owned by `root` | A `sudo zig build` left it that way: `sudo rm -rf .zig-cache zig-out`, then build **without** sudo. |

```bash
export ZIG_LOCAL_CACHE_DIR=~/.cache/zig-csvdiff
export ZIG_GLOBAL_CACHE_DIR=~/.cache/zig-global
(cd zig && zig build --release=fast && bash test.sh)   # test.sh needs the same two exports
```

This fixes the build; it does not fix the speed. For anything you intend to
measure, clone into `~/` as above -- the 9p mount is slow enough on its own
to spoil a timing regardless of where the cache lives.

The Zig install above needs `sudo` only to write into `/opt`. Nothing in this
repository should be built with it — and if `/opt` is not yours to write to,
the PyPI route needs no root at all.

### Getting a pair to compare

**Already have two files?** Skip this section, and substitute your paths and your
key columns everywhere below. Nothing here is required.

Otherwise generate a pair. Both generators write the same bytes from the same
`--seed`, so it only matters which toolchain you have:

```bash
# Linux, macOS, or WSL
c/gen-data --rows 1m --out-dir data --prefix demo
```

```powershell
# Windows, PowerShell — after the build above.
# The leading .\ is not decoration: PowerShell will not run a program from the
# current directory without it, and a relative path is safer than trusting PATH.
.\rust\target\release\gen-data.exe --rows 1m --out-dir data --prefix demo
```

Either writes `data/demo_a.csv` and `data/demo_b.csv`, 184 MB each: 1,000,100
and 1,000,050 rows of 20 columns, keyed on `(account_id, txn_id)`, with 1,000
keys only in A, 1,000 only in B, a scattering of changed values, and
`updated_at` moved on **every** row — which is what the `-i` flag below is for.
`--rows` also takes `10k`, `10m`, and so on, in either spelling (`--rows 10m` or
`--rows=10m`); `--format json|parquet` writes the same rows in the other two
formats. `rust/target/release/gen-data --help` lists the rest. Both generators
**reject a flag they do not know** rather than ignoring it — the Rust one used
to skip unknown flags silently, so `--rows=50m` produced the 10k default and
said nothing about it.

**The C generator is the faster of the two**, by 4.3x on wall time: 2m rows in
1.05s against the Rust generator's 5.49s on this 4-vCPU runner (paired rounds,
`scripts/bench_ab.sh`), because it renders rows in waves across all cores while
the Rust one is single-threaded. Both write all three formats. Reach for the
Rust generator when you want a single toolchain — it needs only cargo, where
the C one wants a C compiler and `make`. Both build natively on Windows.

### Looking at one file first

Before comparing anything you usually need two facts about a file: what its
columns are called, and what the values look like. Both are one command, and
neither reads more of the file than it has to.

```bash
# Linux, macOS, or WSL
rust/target/release/csvdiff columns data/demo_a.parquet   # names, one per line
rust/target/release/csvdiff head    data/demo_a.parquet   # the first 10 rows
rust/target/release/csvdiff head    data/demo_a.csv -n 3  # or however many
```

```powershell
# Windows, PowerShell
.\rust\target\release\csvdiff.exe columns data\demo_a.parquet
.\rust\target\release\csvdiff.exe head    data\demo_a.parquet
.\rust\target\release\csvdiff.exe head    data\demo_a.csv -n 3
```

```
account_id    txn_id           posting_date  value_date  currency  amount
------------  ---------------  ------------  ----------  --------  ----------
ACC-00000000  TXN-00000000000  2026-02-10    2026-03-10  USD       1792961.62
ACC-00007919  TXN-00000000001  2026-02-05    2026-01-10  JPY       844636.46
ACC-00015838  TXN-00000000002  2026-03-18    2026-08-19  GBP       1142001.50
```

Both read **CSV, newline-delimited JSON and Parquet**, and the format is decided
by the bytes rather than the extension. `head --csv` prints machine-readable
rows instead of the aligned table.

**Neither pays for the file.** `columns` on Parquet reads the footer; `head`
stops decoding at the first page — ten rows of a 384 MB Parquet file take
**0.18s**, not the seconds a full decode would. That is the whole point of the
option: columnar data is otherwise only readable here by comparing it against
something.

`columns` writes the names to stdout and its one-line summary to stderr, so the
names pipe cleanly into the `--key` you were about to write:

```bash
# every column as the key -- which is SQL's EXCEPT, see "Duplicate keys" below
KEY=$(rust/target/release/csvdiff columns data/demo_a.csv 2>/dev/null | paste -sd,)
```

```powershell
$KEY = (.\rust\target\release\csvdiff.exe columns data\demo_a.csv) -join ','
```

The values come back through the engine's own parsers — a quoted CSV field is
unquoted here exactly as the comparison would unquote it, and the same rows
written as CSV, as ndjson and as Parquet preview identically. A preview that
disagreed with the comparison would be worse than none, so a test asserts they
do not.

### Running

```bash
# Linux, macOS, or WSL

# the smallest useful invocation: two files and a key
c/csvdiff compare data/demo_a.csv data/demo_b.csv -k account_id,txn_id

# -i skips columns that always move; --json writes the counts and samples;
# --threads caps the cores it takes
c/csvdiff compare data/demo_a.csv data/demo_b.csv -k account_id,txn_id \
  -i updated_at --json summary.json --threads 4

# the Rust port is the one with the self-contained HTML report
rust/target/release/csvdiff compare data/demo_a.csv data/demo_b.csv \
  -k account_id,txn_id -i updated_at -o report.html
```

```powershell
# Windows, PowerShell — the same three, with the Rust port throughout.
# A line is continued with a backtick, not a backslash, and paths use \.

.\rust\target\release\csvdiff.exe compare data\demo_a.csv data\demo_b.csv -k account_id,txn_id

.\rust\target\release\csvdiff.exe compare data\demo_a.csv data\demo_b.csv `
  -k account_id,txn_id -i updated_at --json summary.json --threads 4

.\rust\target\release\csvdiff.exe compare data\demo_a.csv data\demo_b.csv `
  -k account_id,txn_id -i updated_at -o report.html
```

The second and third print `matched 999,000 (changed 60,049) | added 1,000 |
removed 1,000`. Without `-i updated_at` the first one reports all 999,000
matched rows as changed, correctly: that column really did move on every row.

**Three PowerShell details** worth having up front, because each of them stops a
copied command dead:

| | PowerShell | bash |
|---|---|---|
| run a program here | `.\rust\target\release\csvdiff.exe` — the `.\` is required | `rust/target/release/csvdiff` |
| continue a line | a backtick `` ` `` at the end | a backslash `\` |
| the exit code | `$LASTEXITCODE` | `$?` |

`$LASTEXITCODE` matters more than it looks: **a comparison that finds differences
exits 1**, which is the normal result and not a failure. In a script that stops
on errors, catch it rather than let it end the run:

```powershell
.\rust\target\release\csvdiff.exe compare data\demo_a.csv data\demo_b.csv -k account_id,txn_id
if ($LASTEXITCODE -ge 2) { throw "csvdiff failed with $LASTEXITCODE" }
```

**Exit codes** make any of them a CI or pipeline gate directly:

| Code | Meaning |
|---:|---|
| `0` | the two files are identical on the compared columns |
| `1` | differences found — this is a normal result, not a failure |
| `2` | an error: a missing key column, an unreadable file, out of memory |
| `3` | duplicate keys, where `--fail-on-dups` is given — the Rust port only |

`1` is the usual outcome, so a script that treats any non-zero status as failure
will misread a successful comparison. Test for `2` and above.

Format is detected from the bytes, not the extension. A Parquet file may only be
compared against another Parquet file; CSV and ndjson compare against each other.

### What each port carries

| | Reads | Notable | Not there |
|---|---|---|---|
| **[`c/`](c/)** | CSV, ndjson, Parquet (uncompressed, snappy, lz4) | fastest on all three formats, and on every codec it reads; lowest peak memory of the four; `--max-memory MB` bounds what grows with the input; threaded on every path; writes all three formats itself (`c/gen-data`) | no HTML report, no `--trim` / `--ignore-case` / `--tolerance`; gzip and zstd |
| **[`cpp/`](cpp/)** | CSV, ndjson, Parquet (snappy) | the full normalisation flags; `--ignore-case` is ASCII-only and refuses non-ASCII by name | no HTML report; no codec but snappy |
| **[`rust/`](rust/)** | CSV, ndjson, Parquet (uncompressed, snappy, gzip, zstd, lz4) | the full contract with the **HTML report**; engines `turbo` (default), `sortmerge` (spills to disk) and `native` | brotli, and LZO |
| **[`zig/`](zig/)** | CSV, ndjson, Parquet (uncompressed, snappy, gzip, zstd, lz4) | `--max-memory MB` is **enforced** by a fixed buffer, not hoped for | no HTML report; brotli, and LZO |

No port reads brotli or LZO; Zig's codec table names both as unsupported and
Rust's rejects brotli by name. C reads snappy and LZ4 and refuses gzip and zstd:
the first two are byte-copy loops written out in `c/parquet.c`, and the other
two are real decoders that would mean a dependency this port does not take. The codec lists above said otherwise until a
reader's zstd file was refused — see **Parquet codecs** below.

**Two ports take a memory ceiling, and they mean different things by it.** Zig's
`--max-memory` is enforced by a fixed buffer: nothing outside it is allocated at
all. C's is a declaration made before the allocations that scale with the input
-- the per-row index arrays and the Parquet column buffers -- so it bounds the
part that grows rather than the whole process, and the mapped files are not in
it. Both turn "the machine ran out" into an error naming the ceiling. Neither is
on by default.

That distinction is worth the sentence because checking what `malloc` returns is
necessary and not sufficient. Under Linux's default heuristic overcommit there
is a band -- on a 15 GB machine, around 14 GB -- where `malloc` hands back a
pointer and the kernel kills the process when the pages are touched: exit 137,
no message, nothing to catch. A ceiling declared up front is the only thing that
turns that into a diagnosis.

Every port builds from its own toolchain alone, in seconds, and carries no
runtime dependency with a comparison engine in it. What was removed to get
there, and what it measured before it went, is in [ARCHIVE.md](ARCHIVE.md).

### Parquet codecs

The Rust and Zig ports each carry **two** Parquet readers, and which one runs
matters. The table below is Rust's; Zig splits the same way and routes the same
way, and its report names the reader that ran too.

| | Reads | Speed |
|---|---|---|
| the columnar path (`parquet` in the report) | uncompressed and snappy, `BYTE_ARRAY` columns, v1 pages | the fast one — it joins on key columns and compares whole columns without ever building a row |
| `turbo` (`turbo` in the report) | the above plus gzip, zstd and lz4, other column types, v2 pages | decodes pages into rows; **3.7x** the columnar path's wall time on a pair both can read |

**A codec costs almost nothing; the fall-through costs 3.7x.** Held to one
reader, snappy is 1.10x an uncompressed run, gzip 1.19x, lz4 1.04x, and zstd
1.00x while reading 3.82x fewer bytes — so the 3.5-4x a default zstd run shows
is the routing below, not the decompression. Write zstd. The numbers are in
[BENCHMARKS.md](BENCHMARKS.md#2026-09-09-codecs--what-compression-costs-and-what-the-fall-through-costs).

By default a Parquet pair goes to the columnar path, and **falls through to
`turbo` when that path does not read the file** — a codec it does not carry, a
column type it does not decode. `--engine turbo` takes it there directly. The
report's engine field says which one ran, so a run that fell through is visible
rather than silent.

That fall-through is new. Before it, a real file — NYC TLC trip data, zstd —
was refused outright:

```
error: the parquet engine failed: only uncompressed and snappy parquet are read here: …
```

by a binary that reads zstd perfectly well, with `--engine turbo` ignored
because the Parquet-pair branch ran before the engine was consulted. A refusal
on capability grounds now routes; a corrupt file or a missing key column still
fails, and says so as the columnar path's own error.

### Duplicate keys

Every port joins on the **first occurrence** of each key, counts *keys* rather
than rows, and reports duplicates as their own section. That is a choice, and a
stated one: a plain `FULL OUTER JOIN` multiplies duplicates into the diff
instead, silently. See [ARCHIVE.md](ARCHIVE.md#the-field-measured-once-2026-survey).

---

## Working on it

```bash
(cd c && bash test.sh)                # 42 checks, a few seconds, no other toolchain
(cd c && bash test.sh --with-ports)   # adds the cross-port oracles: 60

# the data, in any of the three formats, on every core
c/gen-data --rows 10m --out-dir /tmp/d --prefix p [--format json|parquet] [--threads N]

# two builds of one engine -- the inner loop of working on a port
scripts/bench_ab.sh old/csvdiff new/csvdiff -- compare A.csv B.csv -k id
scripts/bench_ab.sh --self-test c/csvdiff  -- compare A.csv B.csv -k id

# every port that reads the pair, interleaved, with a counts gate and peak RSS
python3 scripts/bench_ports.py A.csv B.csv --repeats 5
```

### Measuring, and what this machine will let you see

`bench_ab.sh` answers "did that change pay?" and is the one to reach for while
working. It runs both builds once per round and reports the **median of the
per-round ratios**, with the middle half of them beside it; where that half
straddles 1.00x it says there is no result rather than leaving a ratio to be
argued about.

That design is not taste. Running all of one build's rounds and then all of the
other's — which is what `hyperfine` and most harnesses do — reads **two copies
of the same binary as 15.8% apart** on a shared runner, and going from five
rounds to fifteen makes it *worse*, because what a shared machine does is drift
rather than jitter and averaging does not touch drift. Comparing the two builds
inside each round does: the same A/A test then reads 1.01x. `--self-test` runs
exactly that, one build against a copy of itself, so the claim can be rechecked
on whatever machine you are on rather than believed. The numbers are in
[BENCHMARKS.md](BENCHMARKS.md#how-to-read-these).

`hyperfine` is still worth having installed for a quick look — its warmup,
standard deviation and `1.07 ± 0.20 times faster` are all better than a bare
ratio, and that ± is the honest part. Just do not read its point estimate on
anything under about 1.2x here: on two identical binaries it named a winner
twice out of three and changed its mind about which.

`c/gen-data` writes the same bytes as the C++ generator and is checked against
it on sixteen shapes, including every thread count — it renders rows in waves
across all cores, and a threading bug that shifted one row would produce a file
that is still valid, still parses, and is wrong. Five million rows of CSV take
3.94s on four cores; the numbers are in [BENCHMARKS.md](BENCHMARKS.md).

`scripts/bench_ports.py` also takes ports built elsewhere, through
`CSVDIFF_PORTS_EXTRA` — a JSON array of `[label, path, flags]`. That is how a
branch's build gets measured against this one on the same rows in the same
sitting, which is the only way two builds can be compared at all. The benchmark
workflow uses it: `alt_ref` checks a second branch out beside this one, builds
its ports, and gives each its own column.

Every port has a `test.sh` holding it to the Rust port's answers on
`tests/fixtures/awkward_*.csv` — a fixture built from every shape that has
broken an engine here: non-ASCII case folding, a Kelvin sign that folds from
three bytes to one, doubled quotes as both value and key, CRLF, ragged rows, a
blank row, and a short key in the last bytes of the file.

| Workflow | Runs |
|---|---|
| `ci-c.yml` | the C port on gcc and clang, sanitizers, and the cross-port checks |
| `ci-rust.yml` | `fmt`, `clippy -D warnings`, `cargo test`, and the engines agreeing on 200k rows |
| `parity.yml` | every port returns identical counts, and every generator emits byte-identical files |
| `benchmark-native.yml` | C and C++ on every push; all four, and any second ref, on demand |

The generator carries money in integer cents and applies drift to those
integers, never to a float, so byte-identity between generators does not depend
on any language's floating-point rounding.

```
c/     the leading port on all three formats: CSV, ndjson, Parquet, plus gen-data and test.sh
cpp/   the C++ port, and the generator that also writes Snappy
rust/  the full contract and the HTML report
zig/   the enforced memory budget
scripts/bench_ab.sh      two builds of one engine, paired by round, with a no-result verdict
scripts/bench_ports.py   every port on one pair, interleaved, with a counts gate
scripts/bench_scale.py   one engine across every size, generating and deleting in turn
tests/fixtures/          every shape that has broken an engine here
```

---

## What's open

1. **The byte proof reaches ndjson in one port out of four.** Settling a matched
   row from its raw bytes is not C's alone — all four do it for CSV, each with
   the same guard that two headers ordering the same columns differently would
   break it, and Zig's `sharedTail` says so in as many words. What is C-only is
   the JSON form, because a name repeated in one object takes its last value and
   a prefix cannot see that; C rules it out with a bounded scan of the mate's
   tail. That is the gap behind C's 1.9x on ndjson, and porting the tail scan is
   the largest thing left here.
2. **Zig does not build for Windows** — two things, neither of them the one
   this list used to name. `src/slab.zig` calls `std.posix.mmap`, whose `MAP`
   type is `void` there, so the port needs a `CreateFileMapping` backend before
   it compiles at all; and `src/main.zig` reads `CSVDIFF_PHASES` through
   `environ.getPosix`, which on Windows reaches a Zig 0.16 stdlib error inside
   `process/Environ.zig`. macOS was the other half of the claim and it was
   simply wrong: the port cross-compiles for `aarch64-macos` and `x86_64-macos`,
   and CI does that on every run now. What *was* Linux-bound was worse than a
   build error — `Phases.now()` called `std.os.linux.clock_gettime`
   unconditionally, which compiles on macOS, because `std.os.linux` is a
   namespace and not a target check, and then issues Linux syscall numbers to a
   kernel that does not use them. That one is fixed: a `builtin.os.tag` switch,
   with a `@compileError` for any target that has no clock rather than a
   plausible-looking number.
3. **100M rows.** 50M is measured and is where the input stops fitting in RAM.
   About 100 MB of index per million rows predicts 10 GB at 100M, which is where
   `sortmerge` stops being the conservative choice and becomes the only one.
4. **Zig's zstd decoder is 2.3x its own lz4**, where Rust's zstd is its
   *cheapest* codec — 13.96s against 6.11s on the same 5m-row pair, same
   machine, both through the row reader. gzip and lz4 are within reach of Rust's;
   zstd alone is not, so this is the decoder in `zig/src/codec.zig` rather than
   anything about the format. It is the one number in the codec table that has
   no explanation yet.
5. **What compression saves in bytes read is still unmeasured**, though what it
   *costs* now is. The container these numbers come from cannot answer the other
   half: `drop_caches` leaves the hypervisor's copy warm, and the one genuine
   cold read available ran at about 18 MB/s. On a host with a characterisable
   disk, zstd reading 281 MB where uncompressed reads 1,073 MB is worth whatever
   that difference costs there.

`--ignore` used to be on this list, accepted in silence in all four ports where
`--key` and `--compare` refuse. It is an error now in all four — a name that
*neither* file has, since `--ignore` is subtractive and a name only one side
carries is real. The check found a `-i x,y,z` against a `z2` column in this
repository's own C suite, where the counts came out the same either way and
nothing could see it.

**Compression has been measured off this list**, and the numbers are in
[BENCHMARKS.md](BENCHMARKS.md#2026-09-09-codecs--what-compression-costs-and-what-the-fall-through-costs).
The short answer is that a codec costs between nothing and 19% of wall time with
the reader held still, and zstd costs nothing measurable while reading 3.82x
fewer bytes. The 3.5-4x a default run shows for gzip, zstd and lz4 is the
fall-through to the row reader, not the codec.

Three other things have been measured off it, and those numbers are in
[BENCHMARKS.md](BENCHMARKS.md#2026-09-09-profiling--three-questions-and-what-the-answers-cost):
the serial insert (blocked by allocation rather than ordering, and 1.4% of a
200-column run), ndjson's cost per byte (the format, not a defect — name lookups
are 12% of a 46% gap), and wide files (throughput flat from 20 columns to 200,
and the SIMD question does not re-open there).
