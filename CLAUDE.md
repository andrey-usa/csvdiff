# csvdiff

Composite-key table comparison — CSV, newline-delimited JSON and Parquet — as byte-level ports in
C, C++, Rust and Zig, held to one result contract. Key columns, compared columns and
normalisation are runtime parameters; nothing about a specific dataset belongs in the code.

The DuckDB-backed Python implementation, the Java, TypeScript and Go ports, and the dataframe
engines beside them are no longer in the project -- including the Rust port's own `duckdb` and
`polars` engines, removed once the comparison they existed for was settled. Their results are in
ARCHIVE.md.

## Commands

```bash
(cd c && make)                                # csvdiff and gen-data; the leading port
(cd c && bash test.sh)                        # its own checks, a few seconds
(cd c && bash test.sh --with-ports)           # plus cross-port and generator parity
(cd cpp && make && make gen-data && bash test.sh)

c/gen-data --rows 10k --out-dir data --prefix p              # CSV
c/gen-data --rows 10k --out-dir data --prefix p --format json
c/gen-data --rows 10k --out-dir data --prefix p --format parquet
rust/target/release/gen-data --rows 10k --out-dir data --prefix p   # same bytes, no WSL

c/csvdiff compare data/p_a.csv data/p_b.csv -k account_id,txn_id -i updated_at
rust/target/release/csvdiff columns data/p_a.parquet   # names only, footer read
rust/target/release/csvdiff head data/p_a.parquet -n 10   # stops at the first page
python scripts/bench_ports.py data/p_a.csv data/p_b.csv --repeats 5    # every port, one table
scripts/bench_ab.sh old/csvdiff new/csvdiff -- compare A.csv B.csv -k id   # two builds
scripts/bench_ab.sh --self-test c/csvdiff -- compare A.csv B.csv -k id    # the harness itself
scripts/build_ports.sh c cpp rust zig          # the ports at once, not in a row
gh workflow run "Benchmark (native)" -f rows=10m -f all_ports=true
```

Linux is the only platform anything is *run* on. C, C++ and Zig need POSIX or
better; only the Rust port builds on Windows without WSL, and CI proves that on
`windows-latest`. Zig cross-compiles for both macOS targets, checked in
`parity.yml` on every run — cross-compiling is not running, and the table in
README.md says so.

**A namespace is not a target check.** `std.os.linux.clock_gettime` compiles on
macOS and then issues Linux syscall numbers to a kernel that does not use them;
the port carried that for a diagnostic that is off by default, and the README
had it recorded as a *build* failure, which would have been the kinder one.
Reach for `builtin.os.tag` and let a target with no implementation be a
`@compileError`. Grepping a port for `std.os.unix` or `std.os.linux` proves
nothing either way — it cannot see inside a dependency, which is how the Rust
port shipped a Windows claim it could not honour.

The two generators write byte-identical output. Use `c/gen-data` for anything
large — it is threaded and 4.3x faster on wall time, and every table in
BENCHMARKS.md was taken on it. The Rust one exists so a Windows reader without
WSL can get a pair at all.

## Layout

| Path | Role |
|---|---|
| `c/` | the leading port on every format measured. `csvdiff.c` (CSV and ndjson), `parquet.c` + `pqdiff.c` (the columnar path), `parallel.c`, `gen-data.c` + `pqwrite.c` (its own generator) |
| `cpp/` | the C++ port, and the generator that also writes Snappy |
| `rust/`, `zig/` | the other byte-level ports, same result contract |
| `scripts/bench_ports.py` | every port on one pair, interleaved, with a counts gate |
| `scripts/gen_data.py` | the original Python generator, kept as the reference recipe |

## Invariants

- **Every port must return identical `counts` and `columns`.** `parity.yml` asserts this on 200k
  rows, for every format each port reads. A change to one port needs the matching change in the
  others, or a reason it does not apply.
- **The result contract is the API.** `engine.compare()` returns the dict documented in
  `engine.py`; `report.py`, the CLI, the server and the mailbot all consume only that. Add a
  field rather than reshaping an existing one.
- **CSV values are read as text.** No type inference — `1.0` and `1` are different unless a
  tolerance is set. Do not add dtype guessing.
- **The report is one file with no external references.** No CDN, no fonts, no frameworks.
  CI fails if any `src=` or `href=` points outside the document.
- **Only differing cells are embedded.** Changed rows carry `[colIndex, old, new]` triples, not
  full rows. Keeping this sparse is what keeps a 60k-change report near 1 MB.
- **Row sections are capped** by `--max-rows` (default 50k); counts are always exact and the UI
  says when a list is truncated. `--export-dir` writes the uncapped CSVs.
- **SQL is built by string interpolation.** Column and table names go through `_q()`, string
  values and paths through `_lit()`. Never interpolate with `!r` — Python repr is not SQL.
- **The generators must emit byte-identical files.** `c/test.sh --with-ports` holds `c/gen-data`
  against `cpp/build/gen-data` across every format and option. Changing the drift recipe means
  changing it in both, deliberately — a benchmark number from one generator is only comparable with
  a number from another if the bytes agree.

## Style

- No runtime dependency carries a comparison engine any more: the four ports are the engine. Do
  not add a dataframe library, a web framework, a JS bundler, or a templating library.
- The report JS is plain ES2020 in `report.py`. It must keep working when opened from `file://`.
- Prefer editing the existing virtualised grid over adding a table library; the grid renders only
  the visible rows and that is the reason large reports open instantly.

## Working on this repository

Several agents work here at once, split by topic rather than by branch: one takes the C and C++
ports, another the Rust and Zig ones. Everything lands on `main`, so the split is in what you
touch, not where you push.

- **Branch, then pull request.** Work on a topic branch and open a PR against `main`; do not
  commit to `main` directly. Keep commits small enough to read and say in the message what was
  measured, not only what changed.
- **Wait for the Codex review before considering a PR done.** Every PR gets an automated code
  review. Read it: where a finding is legitimate, fix it and push to the same branch; where it is
  wrong or does not apply, say so on the PR and why. Do not merge past an unanswered review, and
  do not silently ignore one — a finding you disagree with still needs a reply.
- **Someone else may have already done your change in their port.** Before starting a round,
  read what landed on `main` recently. The same finding often applies to all four ports, and the
  right answer can differ per port: `added` is derived arithmetically in C++ and Zig, from a
  bitmap in Rust, because only Rust has to name the rows.

## Measuring

Most of the wrong turns taken here have been measurement, not code. Read this
before timing anything.

- **Use `scripts/bench_ab.sh`, not a hand-rolled loop.** It runs both builds once
  per round and reports the **median of the per-round ratios** with the middle
  half beside it. When that half straddles 1.00x it says *no result* instead of
  leaving a ratio to be argued about.
- **Pair, do not average.** Comparing two builds' separately-taken bests reads
  two copies of *the same binary* as 9.1% apart on this class of runner, and
  fifteen rounds instead of five makes it 9.4% — because what a shared machine
  does is drift, not jitter, and averaging does not touch drift. Paired, the
  same A/A test reads 1.01x. `--self-test` runs it, so the claim can be
  rechecked on whatever machine you are on.
- **Do not reach for `hyperfine` here.** Its model is one build's runs then the
  other's, which is the first case above; on two identical binaries it reported
  "1.07 ± 0.20 times faster" twice and changed its mind about which the third
  time. The ± is the honest part. Its point estimate is not readable under about
  1.2x on this hardware.
- **Bound the prize before building anything.** A deliberately unsound build that
  removes the cost entirely says what the real thing could be worth at most.
  Ten minutes of that has repeatedly replaced a day of implementation — and has
  also justified one, when it showed 1.47x on the table.
- **Check a probe against the counts it still produces, not just the clock.** A
  probe that skipped the index insert measured 2.08x and meant nothing: an empty
  index is an empty join, so it had priced both. `matched 0` in the output said
  so, and nobody looked.
- **Work removed beats work moved.** Taking a fifth of the join's CPU out of the
  C++ port changed its wall clock by nothing, because the join is threaded and
  was not the critical path. Parallelising the pass that *was* the critical path
  also measured nothing — the machine was already busy. Deleting that pass was
  worth 1.28x. Prefer removing a pass to speeding one up, and prove which you
  have done.
- **Run `--self-test` on the machine you are on, before trusting a ratio.** What
  it can resolve is a property of that container, not of the harness. One here
  reported 1.11x for a build against itself at the default one warmup round, and
  1.05x wall / 1.03x cpu at three — so on that machine anything under a few per
  cent is unmeasurable, and the few per cent favours whichever build runs second.
  A 350 MB pair needs more than one warmup round before the page cache stops
  landing on build A.
- **Rounds scale with how short the run is, not with how much you care.** The
  same change read *no result* at nine rounds and 1.29x at twenty-five, because
  the port measured finishes in 0.4s where the other takes 0.7s and the noise is
  absolute rather than proportional. "No result" is a request for more rounds
  before it is evidence of absence.
- **Never compare across tables.** Two numbers from two sittings compare machine
  states. This runner's ndjson figures moved 20% between two runs one morning
  with no code change in any port.

## Gotchas

- `resource.ru_maxrss` is KB on Linux, bytes on macOS — the harnesses in `scripts/` handle both.
- `--ignore` with a name **no file has** is an error in all four ports, like `--key` and
  `--compare` before it. It used to be accepted in silence, which cost a benchmark run that
  reported every row as changed -- and hid a `-i x,y,z` against a `z2` column in this repository's
  own C suite until the check went in. The rule is *neither* file, not both: `--ignore` is
  subtractive, so a name only one side carries is real and does no harm. Keep the four ports
  saying the same thing; `parity.yml` does not compare refusals.
- Duplicate keys: the first occurrence of each key joins, the rest are reported separately.
  Changing that changes the matched/added/removed counts, so it is a behaviour change, not a fix.
- The report decodes its gzip payload with `DecompressionStream`, which needs a 2023+ browser.
  `--no-compress` is the escape hatch.
- **Every command in README.md has to run as written, from a fresh clone.** Its examples named
  `july.csv` and `august.csv` — placeholder names for files nothing in the tree produces — so the
  first thing a new reader pasted failed, and a reader reported it. The examples generate the pair
  first now, and **every path in that section is written from the repository root**, in bash and
  in PowerShell alike -- the PowerShell block used to be written from `rust\`, which is how a
  reader standing in `c/` got `No such file or directory` for `c/gen-data`. Change a flag, a
  column name, a binary name or a working directory, and run the README's commands before claiming
  the change is done; the `windows-latest` job in `ci-rust.yml` runs the PowerShell ones from the
  root for this reason.
- **The Rust port has two Parquet readers.** The columnar path (`engine/pqdiff.rs` on
  `src/parquet.rs`) reads uncompressed and snappy, `BYTE_ARRAY`, v1 pages, and is 3.7x faster on
  wall; `turbo` (`engine/turbo/parquet.rs` on `turbo/codec.rs`) reads gzip, zstd and lz4 too. A
  capability refusal from the first falls through to the second — `Error::unsupported`, read by
  `engine.rs`. Two traps live there: route on the **requested** engine, not the resolved one
  (`auto` is already `Turbo` for any Parquet input, so testing the resolved value retires the fast
  path for everyone), and mark only capability refusals, never a corrupt file or a missing column,
  or a real error comes back wearing the other reader's name.
- **Neither Rust nor Zig reads brotli or LZO.** Both codec tables refuse them by name. The README
  claimed brotli for two ports for weeks, and it took a reader's zstd file to find out that nobody
  had run a codec through either port.
- **A flag that is accepted and ignored is a wrong answer, not a convenience.** `gen-data`'s Rust
  argument loop skipped anything it did not recognise, so `--rows=50m` -- the spelling every GNU
  tool takes -- fell through to the `10k` default and the run printed `rust: 10000 rows` with no
  mention of the flag it dropped; `--seed abc` was `unwrap_or(7)`; `--rows --out-dir data` read
  the next flag as the row count. All three are errors now, and both spellings of every flag work.
  This is the same family as `--ignore` matching nothing, which is fixed above, and it is the one
  to watch for whenever a harness passes flags through: the run completes, the numbers are wrong,
  and nothing says so.
  The same trap sits in `csvdiff`'s own parser from the other side: **a value-less flag has to be
  in `FLAGS` in `main.rs`**, or it eats the token after it -- `head a.csv --csv` reported
  "--csv needs a value".
- **`columns` and `head` must not pay for the file.** `Reader::project` takes a row limit and
  stops on a page boundary at or past it; `columns` on Parquet reads the footer alone. Ten rows of
  a 384 MB Parquet file is 0.18s. If either grows a full read, the option has lost its reason to
  exist. Their values go through the engine's own parsers, and a test asserts CSV, ndjson and
  Parquet preview identically -- a preview that disagreed with the comparison would be worse than
  none.
- **The WSL setup section pins toolchain versions -- keep them level with CI.** The README tells a
  WSL reader to install Zig `0.16.0` and a Rust that satisfies `rust-version` in `rust/Cargo.toml`.
  Both numbers are copied from `.github/workflows/`, and both install routes (the ziglang.org
  tarball and `pip install ziglang`) are the ones `parity` and `formats` use, so they are known to
  work rather than assumed to. Bump the workflows and the README together, or the instructions
  quietly start installing a toolchain CI no longer builds with.
- **The Rust port groups the digits in its summary line and the other three do not.**
  `A 130,131 rows` against `A 130131 rows` -- the counts agree, the strings do not. Every
  cross-port check that compares those lines is therefore fine until someone grows a fixture past
  a thousand rows, and then reads as a disagreement about counts; it cost an afternoon's confusion
  once. All four suites normalise it now (`summary()` in `c/` and `cpp/`, `answer()` in `zig/`,
  each stripping the engine label and the separators), so compare through the helper rather than
  inlining a `sed`. The JSON is the contract and never had this problem -- `c/test.sh` compares
  that for its large fixture, which is the more robust pattern where a check can use it.
- **A Parquet codec is nearly free; the fall-through is not.** Held to one reader, snappy costs
  1.10x an uncompressed run, gzip 1.19x, lz4 1.04x and zstd 1.00x while reading 3.82x fewer bytes.
  A default gzip/zstd/lz4 run looks 3.5-4x worse only because it routes to the row reader, which
  is 3.7x the columnar path on its own. Never read a codec number without checking the engine
  field first -- it is the difference between measuring zstd and measuring the router.
- **In the Zig port, check the budget before you decide an allocation is free.** Its allocator is
  a fixed buffer that hands memory back and *cannot reuse it*, so an allocation inside a loop is
  spent for good however promptly it is freed. Decompressing a 1M-row zstd pair allocated zstd's
  8.13 MB window per page: 1.03x cpu and no result on wall over fifteen paired rounds, and
  `--max-memory 4800` where 1600 now does, for 291 MB of live data. The clock said 3% and nearly
  buried it. `--max-memory`, laddered until it stops refusing, is the measurement that finds this
  class of bug; wall and cpu will not.
- **A codec number is only the codec if the phase table covers the run.** A compressed Parquet run
  used to print 0.19s of phases against a 1.0s wall, because the pass that decompresses and
  materialises the columns sat outside every timer. It is `read columns (par)` now, and it is
  85-90% of such a run. Where the phases do not add up to the wall, the missing pass is the one
  worth looking at -- and two columns costing the same as six is a statement about the *widest*
  column, not about the bytes.
- **`drop_caches` does not give you a cold disk here.** The hypervisor keeps the blocks, so
  "cold" runs come back within 25% of warm ones. The only genuine cold read is the first touch
  after a container restart, and it ran at about 18 MB/s -- not reproducible, not representative.
  Any claim about bytes read costing time needs a host with a characterisable disk.
- **`--compare` lives in two resolvers in the C port**, not one: `csvdiff.c` for text and
  `pqdiff.c` for the columnar path, because they resolve columns against different structures.
  A change to one that skips the other is a flag that works on CSV and is ignored on Parquet.
  The same is true of `--key` and `--ignore`; only Rust has a single `resolve`.
- **A compressed Parquet column in C means `owned`, and `pq_base()` is how you read it.** A
  column is wholly compressed or wholly not -- one whose chunks disagree is refused -- so
  `PqColumn.owned` being non-NULL settles what every slice in that column counts from. Never take
  `mapping->data` as the base directly; `pq_base(&col.c, mapping->data)` is the only correct form,
  and an uncompressed file still resolves to the mapping exactly as before. `owned` moves as it
  grows, which is why a slice is an offset and not a pointer.
- **C carries snappy and LZ4 and refuses gzip and zstd**, and that line is where it stays: the
  first two are byte-copy loops of about eighty lines, the other two are real decoders and would
  be the dependency this port exists without. `c/parquet.h` says so at the top.
- **A checked `malloc` is not OOM resilience.** Under Linux's default overcommit there is a band
  where `malloc` returns a pointer and the kernel kills the process on touch -- exit 137, no
  message. Measured on a 15 GB box: 8 GB allocates and touches fine, 14 GB allocates and is
  killed, 20 GB is refused up front. That band is why C has `--max-memory`, and why the sizes
  that scale with row count go through `budget_take` *before* they are allocated. Adding a new
  allocation that grows with the input means adding it to the budget, or the ceiling quietly
  stops meaning what it says.
- **C's budget bounds what grows, not the process.** The per-row index arrays and the Parquet
  column buffers are in it; the mapped files and a few kilobytes of bookkeeping are not. Say
  "bounds what scales with the input", never "enforced" -- that word belongs to Zig's fixed
  buffer, which is a stronger guarantee.
- **The CSV path is the field scan.** `next_of2` is 43% of its instructions and `parse_csv_row`
  27%, so seventy per cent of the work is finding field ends. That is where a change pays and
  everywhere else is rounding. `next_of2` steps 32 bytes with AVX2, 16 with SSE2 and 8 otherwise,
  chosen at compile time so there is no dispatch on the hot path; aarch64 gets the SWAR loop.
  Any change here has to be run through all three widths -- `-U__AVX2__` and `-U__SSE2__` build
  them -- because only the tails see a row shorter than a step.
- **"The SIMD question does not re-open" was about wide files, not the byte scan.** That finding
  said throughput is flat from 20 columns to 200. It is not a finding about how many bytes a
  scan step covers, and the two were confused once already.
- **An instruction profile picks candidates; it does not rank them.** `callgrind` put `rle_fill`
  at 18.5% of the columnar path's instructions. Deleting its work outright -- wrong answers, pure
  ceiling -- changed the wall clock by nothing: the phase is memory-bound and those instructions
  issue in the shadow of the loads they wait on. The same profile found the CSV scan, where the
  two did line up. Before writing an optimisation, build the ceiling: replace the function's work
  with something trivially wrong, keep the rest in step, and time it. Ten minutes, and it says
  whether there is anything there at all.
- **The scale ceiling is two questions, not one.** `.github/workflows/scale-ceiling.yml` climbs a
  ladder of sizes until a port fails (the ceiling) and separately lowers a cgroup limit until the
  run is killed (the floor). They rank differently, which is why both tables exist. One port per
  runner on purpose: the ladder is bigger than one runner's disk, and separate jobs mean a port
  that dies at 40m does not take the rest of the table with it.
- **Peak RSS is not the memory answer for anything that maps its input.** Mapped pages are
  reclaimable, so RSS is whatever the kernel allowed, not what the engine needed. Use
  `scripts/memory_floor.sh`, which takes the memory away until the run dies. It handles cgroup v1
  and v2 -- the runners are v2, most older images are v1, and a script that knew only one would
  report "no controller" on exactly the host worth measuring.
- **The ports build in parallel, and they cannot move to a job of their own.** Every workflow
  that needs more than one port calls `scripts/build_ports.sh` with target names; it runs them up
  to `nproc` at a time and exits non-zero naming each one that failed. Measured from clean on four
  cores, `cpp cpp-gen cpp-scanners zig zig-v32` is 149s in a row and 35s at once. The obvious next
  step -- build on one runner, upload the binaries, measure on another -- **does not work here**:
  both Makefiles compile with `-march=native` and the Zig scanner builds use `-Dcpu=native`, and
  the hosted fleet is not uniform. The workflows already branch on `grep avx512bw /proc/cpuinfo`
  and `cpp/Makefile` already carries a note about runners advertising AVX10.1, so a binary built
  on one runner and run on another is a `SIGILL` waiting for the wrong machine. Shortening the
  job is the only way to shorten the `benchmark-host` lock, because the lock is held for the whole
  job.
- **One benchmark at a time, repository-wide.** Two timing jobs running at once share a host and
  measure each other's contention, which spoils both — including the one already running that
  somebody is waiting on. Check for a run in progress before pushing to a path that triggers a
  benchmark or dispatching one by hand. The benchmark workflows name a single `benchmark-host`
  concurrency group so GitHub queues them; a per-ref group does not, because another branch is
  another group.
- **A Windows checkout can hand WSL scripts CRLF, even though every blob in the repository is
  LF.** Git for Windows ships `core.autocrlf=true` in its system-wide config, not just as a user
  opt-in, so a plain `git clone` on the Windows side rewrites every tracked `.sh` on checkout —
  `bash test.sh` then fails as `set: pipefail: invalid option name` or `$'.\r': No such file or
  directory`, naming neither Windows nor line endings. `git show HEAD:path` on the same file comes
  back clean, which is the tell: the corruption is in the checkout, not the commit. `.gitattributes`
  now pins `*.sh` to `eol=lf`, which overrides `core.autocrlf` for those paths regardless of which
  side cloned; a checkout from before it existed needs `git add --renormalize .` plus a re-checkout
  of the affected files to actually rewrite what is already on disk.
