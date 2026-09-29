# Contributing to csvdiff

Thanks for looking at the code. This project is four byte-level ports of one
table-comparison engine — C, C++, Rust and Zig — held to a single result
contract. Contributions are welcome, whether that's a bug fix, a faster probe,
a clearer doc, or a new awkward fixture.

## What to know before you start

**Read `AGENTS.md` first.** It's the canonical guide for working in this
repository: the layout, the invariants (the result contract is the API),
the measuring rules, and the gotchas that have bitten before. The per-port
READMEs (`c/`, `cpp/`, `rust/`, `zig/`) cover each implementation's design.

The short version of the rules that matter most:

- **The result contract is the API.** Every port must return identical
  `counts` and `columns` on the same input. A build that disagrees about row
  counts is a bug. `parity.yml` gates this in CI.
- **Measure before claiming.** `scripts/bench_ab.sh` is how "did that change
  pay?" is answered here — interleaved, paired ratios, `--self-test` on your
  machine. Never compare numbers across benchmark sittings. `BENCHMARKS.md`
  ("How to read these") has the full reasoning.
- **Check the other ports.** The same finding usually applies to all four,
  though the right fix can differ per port.

## How to contribute

1. **Branch from `main`, then open a pull request to `main`.** One PR per
   change; one port per PR when the change touches ports.
2. **Commit messages say what was measured, not only what changed.**
   `Rust: adaptive 32-bit hash-table slots — 1.04x CPU, −119 MB RSS at 10M`
   beats `optimize hash table`.
3. **Wait for CI and the review.** Fix legitimate findings on the branch;
   where you disagree with a finding, say so on the PR and why. Do not merge
   past an unanswered review.

## Building and testing

Every command runs from the repository root:

```bash
(cd c && make)                       # csvdiff + gen-data
(cd c && bash test.sh)               # 42 checks

(cd cpp && make && make gen-data && bash test.sh)

(cd rust && cargo build --release && cargo test)
(cd rust && cargo fmt --check && cargo clippy --all-targets -- -D warnings)

(cd zig && zig build --release=fast && bash test.sh)
# A plain `zig build` is a Debug build, about 4x slower on the Parquet path.

scripts/build_ports.sh c cpp rust zig  # all four at once
```

Linux is the only platform anything is *measured* on. Rust also builds and
tests on Windows; C and C++ build on Windows under MSYS2/MinGW.

Make a test pair and compare:

```bash
c/gen-data --rows 10k --out-dir data --prefix p
c/csvdiff compare data/p_a.csv data/p_b.csv -k account_id,txn_id -i updated_at
```

## Reporting issues

Use the issue templates. A good bug report names the port, the input shape
(rows × columns, format), the exact command, and what you expected versus
what you got. If the counts disagree between ports, that's a bug — say which
two and attach the smallest input that reproduces it.

## Code of conduct

Be kind and precise. The full text is in `CODE_OF_CONDUCT.md`.

## Releasing

Nothing is released automatically, and nothing is published without a person.
Pushing a tag `vX.Y.Z` runs `release.yml`, which builds all four ports for the
baseline of each architecture (never `-march=native`; see
`scripts/package_release.sh`), packs them, writes `SHA256SUMS`, and attaches
everything to a **draft** release. Read the draft, then publish it.

```bash
git tag v1.2.3 && git push origin v1.2.3
```

To try the packaging without a tag, run the workflow from the Actions tab (it
uploads the archives as artifacts and stops), or run
`scripts/package_release.sh PORT VERSION` for the host you are on.

The archives are the baseline builds. They leave out the wide scanners the ports
choose at compile time (AVX2 and up), so they are slower than a build made for
your machine; build from source with the port's README for that.
