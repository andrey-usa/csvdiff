---
description: Run a benchmark scale and report the numbers
argument-hint: [10k|1m|10m]
allowed-tools: Bash(python scripts/*), Bash(rm -rf data), Read
---

Build the ports if you have not: `scripts/build_ports.sh c cpp rust zig`.

Run the interleaved every-port harness on a generated pair —
`python3 scripts/bench_ports.py data/p_a.csv data/p_b.csv --repeats 5` — or the formats matrix,
`python3 scripts/bench_formats_ports.py --rows ${1:-10k} --repeats 3`, which generates its own data.

Report compare time, throughput, peak RSS above the mapped input, and whether the counts gate
passed. Then say whether the numbers fit the budgets the workflows expect (10k: 20s / 1.5 GB, 1M:
120s / 6 GB, 10M: 900s / 12 GB — the Rust `bench` binary in `rust/src/bin/bench.rs` enforces
these). If a budget is exceeded, profile before changing anything: identify which phase (read,
join, per-column stats, row extraction) dominates, and say so before proposing a fix.

Delete generated data when finished unless I asked to keep it.
