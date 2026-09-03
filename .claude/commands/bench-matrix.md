---
description: Benchmark every installed engine over one or more scales and rank them
argument-hint: [scales, e.g. 10k,1m] [engines, default: every installed one]
allowed-tools: Bash(python scripts/*), Bash(pip install*), Bash(rm -rf data), Read
---

Run `python scripts/bench_matrix.py --scales ${1:-10k,1m} --repeat 3 --threads $(nproc)`,
adding `--engines $2` when a second argument is given.

Read `bench/matrix.md` and report:

- the ranking per scale, in-process time as well as wall time (at 10k almost all the wall time is
  interpreter start and imports, so ranking on it alone is misleading);
- peak RSS against the budget in `scripts/bench.py` — an engine that is fastest but needs several
  times the memory is the wrong default;
- any engine flagged `MISMATCH`. A disagreement with DuckDB disqualifies an engine no matter how
  fast it was; find the cause before reporting a winner.

If an engine failed with an out-of-memory kill or a timeout, say so plainly rather than dropping it
from the table — that is a result too. Update BENCHMARKS.md when the numbers move materially, and
say what changed rather than only pasting the new table.
