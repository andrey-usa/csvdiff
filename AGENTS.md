# csvdiff — for AI coding agents

This is the entry point for AI coding tools that read a repository's docs: Cline, Codex, Copilot,
Cursor, Aider and the rest. Read it before editing anything, and then the README of the port you
touch. Claude Code additionally reads `CLAUDE.md`, which is the thin Claude-specific layer over
this file.

## What this repository is

`csvdiff` compares two tables on a composite key and reports the counts, the per-column statistics
and — from the Rust port — a self-contained HTML report. Key columns, compared columns and
normalisation rules are parameters; nothing about a specific dataset belongs in the code.

It is **four byte-level ports to one result contract**: C (`c/`), C++ (`cpp/`), Rust (`rust/`) and
Zig (`zig/`). They read the same files, return the same counts and the same exit codes — `0`
identical, `1` differences found (the normal result, not a failure), `2` error, `3` duplicate keys
(Rust, with `--fail-on-dups`).

The Python, Java, TypeScript and Go implementations and the DuckDB/polars dataframe engines are
gone. Do not reintroduce them; their measured verdicts are the point of `ARCHIVE.md`.

## Layout

| Path | What it is |
|---|---|
| `README.md` | the full user guide. Every command runs from the repository root, as written, on a fresh clone — keep it that way |
| `AGENTS.md` | this file |
| `CLAUDE.md` | the Claude Code layer: slash commands, the report skill, settings |
| `BENCHMARKS.md` | every kept benchmark run, newest first. Compare rows within a table, never across tables |
| `ARCHIVE.md` | what was tried, what it was worth, what was removed |
| `c/` | the leading port on every format measured — `csvdiff.c` (CSV/ndjson), `parquet.c` + `pqdiff.c` (columnar), `parallel.c`, `gen-data.c` + `pqwrite.c`; own suite `test.sh` |
| `cpp/` | the C++ port (SWAR text + columnar + snappy); own suite `test.sh`; generator included |
| `rust/` | the port that carries the HTML report; engines `turbo` (default), `sortmerge`, `native`; `cargo test` |
| `zig/` | the port with the *enforced* `--max-memory` budget; `zig build --release=fast` + `test.sh` |
| `scripts/` | the measurement harnesses in Python and bash — see the commands below |
| `.github/workflows/` | the CI and benchmark workflows, listed in `README.md`; `parity.yml` and `formats.yml` are the repository-wide gates |
| `tests/fixtures/` | the awkward-input and multi-format fixtures every suite is held to |
| `.claude/` | Claude Code only: slash commands, a report skill, `settings.json` |

## Building, testing and running

Run every command from the repository root.

```bash
(cd c && make)                       # csvdiff + gen-data; CC ?= cc
(cd c && bash test.sh)               # 42 checks, no other toolchain built
(cd c && bash test.sh --with-ports)  # + cross-port and generator oracles

(cd cpp && make && make gen-data && bash test.sh)     # g++ by default
(cd rust && cargo build --release && cargo test)      # also builds gen-data
(cd rust && cargo fmt --check && cargo clippy --all-targets -- -D warnings)
(cd zig && zig build --release=fast && bash test.sh)  # a plain `zig build` is 4x slower

scripts/build_ports.sh c cpp rust zig                  # all at once, up to nproc builds
```

Linux is the only platform anything is *measured* on — the `${port}/test.sh` suites are POSIX
shell. Rust builds and tests on Windows without help; C and C++ build on Windows under
MSYS2/MinGW (`platforms.yml`, cross-checked by `scripts/win_smoke.sh`); Zig does not build for
Windows yet.

Make a pair:

```bash
c/gen-data --rows 10k --out-dir data --prefix p [--format json|parquet]   # fast, threaded
rust/target/release/gen-data --rows 10k --out-dir data --prefix p         # same bytes, Windows-safe
```

Both generators write byte-identical files from the same seed; `scripts/generator_parity.py` gates
it. Compare:

```bash
c/csvdiff compare data/p_a.csv data/p_b.csv -k account_id,txn_id -i updated_at
rust/target/release/csvdiff compare data/p_a.csv data/p_b.csv -k account_id,txn_id -o report.html
rust/target/release/csvdiff columns data/p_a.parquet     # names only
rust/target/release/csvdiff head data/p_a.parquet -n 10
```
## Invariants

- **The result contract is the API.** `rust/src/contract.rs` defines the result shape; the report
  renders that and nothing else. Every port must return identical `counts` and `columns` on the
  same input — `parity.yml` gates it. A build that disagrees about row counts is a bug: count
  changes are bugs, not tweaks.
- **Duplicate keys are first-class.** The *first occurrence* of each key joins; the rest are
  counted and listed separately. Changing that changes matched/added/removed and is a behaviour
  change, not a fix.
- **Bad options are refused, not ignored.** `--key`, `--compare` and `--ignore` with a column name
  *neither* file has is an error in all four ports. `--ignore` is subtractive, so a name only one
  side carries is real — the rule is "neither file", not "both".
- **The packed field is the representation.** A field is an offset and a length in a 64-bit word;
  nothing becomes a heap string until the report. Memory is this project's argument.
- **`--ignore-case` is ASCII-only in C, C++ and Zig.** A non-ASCII byte in a folded field is
  refused by name: folding it partially would be worse than not folding.
- **Parquet capability refusals route; real errors fail.** The fast columnar path (uncompressed +
  snappy) falls through to `turbo` on capability only. A corrupt file or a missing key column must
  keep failing, as its own error. Check the report's engine field before reading any codec number.
- **Zig's `--max-memory` is *enforced*; C's *bounds what scales*.** Zig uses a
  `FixedBufferAllocator` that cannot hand out more than it was given. C checks `malloc` for the
  allocations that grow with the input; the mapped files are not in it. Say "bounds", never
  "enforced", about C.
- **The HTML report is file://-safe.** No external references (CI greps `src=`/`href=`), no
  browser storage, sparse payload, gzip+base64 (`DecompressionStream`) or `--no-compress`.
- **No new dependency carries a comparison engine.** No dataframe library, no DuckDB wiring, no
  web or templating framework, no JS bundler.

## Working in this repository

- **Branch, then a pull request to `main`.** Commit messages say what was measured, not only what
  changed.
- **Check the other ports before you start.** The same finding usually applies to all four, and the
  right answer can differ per port — `added` is derived arithmetically in C++ and Zig, from a
  bitmap in Rust, because only Rust has to name the rows.
- **Your change may already exist.** `git log --oneline -20` before starting a round.
- **A docs change is not done until the README commands still run.** The root README's examples
  generate their own files, and every path is written from the repository root — the
  `windows-latest` job in `ci-rust.yml` re-runs the PowerShell ones.
- **CI is the gate.** Wait for it, and for the review. Fix legitimate findings on the branch;
  answer the ones you disagree with on the PR. Do not merge past an unanswered review.

## Measuring

Most wrong turns here have been measurement, not code. The full reasoning is in `BENCHMARKS.md`
("How to read these"); the rules in short:

- **`scripts/bench_ab.sh` for "did that change pay?"** It interleaves the two builds and reports
  the median of the per-round paired ratios with the middle half; where that half straddles 1.00x
  it says *no result*. `--self-test` rechecks the claim on whatever machine you are on.
- **Never compare across tables.** Two numbers from two sittings are two machine states — this
  runner's ndjson figures moved 20% in one morning with no code change.
- **Not `hyperfine`.** Its model (one build's runs, then the other's) reads two copies of the same
  binary as 9-16% apart on a shared runner, and fifteen rounds instead of five makes it worse. The
  machine drifts; pairing cancels drift, averaging does not.
- **Bound the prize before building anything.** A deliberately unsound build that removes the cost
  names the upper bound — ten minutes that has repeatedly replaced a day of implementation.
- **Check a probe against the counts it still produces, not just the clock.** A probe that skipped
  the index insert measured 2.08x and meant nothing: `matched 0` said why.
- **Rounds scale with how short the run is.** *No result* at nine rounds has become 1.29x at
  twenty-five.
- **Peak RSS is not the memory answer for anything that maps its input.** `scripts/memory_floor.sh`
  takes memory away until the run dies; that is the honest floor.

## Gotchas

- `resource.ru_maxrss` is **KB on Linux, bytes on macOS** — the harnesses in `scripts/` handle both.
- The Rust port has **two Parquet readers**. Route on the *requested* engine, not the resolved one
  (`auto` is already `turbo` for Parquet input), and mark only capability refusals.
- Neither Rust nor Zig reads brotli or LZO — both refuse by name. C reads snappy and LZ4 and
  refuses gzip and zstd by design.
- In Zig, an allocation from the fixed buffer is **spent for good** even when freed. Every `alloc`
  counts against `--max-memory` for the whole run.
- In C, `--compare`/`--key`/`--ignore` live in **two resolvers** — `csvdiff.c` for text and
  `pqdiff.c` for the columnar path. Change both, or the flag works on CSV and is ignored on Parquet.
- A compressed Parquet column in C means `owned`, and `pq_base()` is the only correct base — never
  take `mapping->data` directly.
- `drop_caches` does not give a cold disk on hosted runners (the hypervisor keeps the blocks). Only
  a container restart gives a genuine cold read.
- `.gitattributes` pins `*.sh` to `eol=lf`; a Windows checkout from before it existed needs
  `git add --renormalize .`.

## Where to look next

- `README.md` — the whole contract and every command, the per-port matrix, the PowerShell rules.
- `c/README.md`, `cpp/README.md`, `rust/README.md`, `zig/README.md` — per-port design and layout.
- `BENCHMARKS.md` — what this machine can and cannot resolve.
- `ARCHIVE.md` — everything removed, with the number that earned the verdict.
- `CLAUDE.md` — Claude Code specifics.