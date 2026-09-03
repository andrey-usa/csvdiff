---
description: Run a benchmark scale and report the numbers
argument-hint: [10k|1m|10m] [duckdb|pandas]
allowed-tools: Bash(python scripts/*), Bash(rm -rf data), Read
---

Run `python scripts/bench.py --rows ${1:-10k} --engine ${2:-duckdb} --data-dir data --out-dir bench --threads $(nproc)`.

Then report compare time, throughput, peak RSS and report size, and say whether each is within the
budget in `scripts/bench.py`. If a budget is exceeded, profile before changing anything: identify
which phase (read, join, per-column stats, row extraction) dominates, and say so before proposing a fix.
Delete generated CSVs when finished unless I asked to keep them.
