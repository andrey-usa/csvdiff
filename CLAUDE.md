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

c/csvdiff compare data/p_a.csv data/p_b.csv -k account_id,txn_id -i updated_at
python scripts/bench_ports.py data/p_a.csv data/p_b.csv --repeats 5    # every port, one table
scripts/bench_ab.sh old/csvdiff new/csvdiff -- compare A.csv B.csv -k id   # two builds
scripts/bench_ab.sh --self-test c/csvdiff -- compare A.csv B.csv -k id    # the harness itself
gh workflow run "Benchmark (native)" -f rows=10m -f all_ports=true
```

Linux is the only tested platform. C, C++ and Zig need POSIX or better — Zig
calls `std.os.linux.clock_gettime`, so it is Linux and not merely POSIX — and
only the Rust port builds on Windows without WSL. See the table in README.md.

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
- **Never compare across tables.** Two numbers from two sittings compare machine
  states. This runner's ndjson figures moved 20% between two runs one morning
  with no code change in any port.

## Gotchas

- `resource.ru_maxrss` is KB on Linux, bytes on macOS — the harnesses in `scripts/` handle both.
- `--ignore` with a name no column matches is accepted in silence by all four ports, where `--key`
  with one is an error. It has already cost a benchmark run that reported every row as changed.
  Check the counts a run produces before believing its timings.
- Duplicate keys: the first occurrence of each key joins, the rest are reported separately.
  Changing that changes the matched/added/removed counts, so it is a behaviour change, not a fix.
- The report decodes its gzip payload with `DecompressionStream`, which needs a 2023+ browser.
  `--no-compress` is the escape hatch.
- **One benchmark at a time, repository-wide.** Two timing jobs running at once share a host and
  measure each other's contention, which spoils both — including the one already running that
  somebody is waiting on. Check for a run in progress before pushing to a path that triggers a
  benchmark or dispatching one by hand. The benchmark workflows name a single `benchmark-host`
  concurrency group so GitHub queues them; a per-ref group does not, because another branch is
  another group.
