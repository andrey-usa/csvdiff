---
description: Diagnose the latest failing CI or benchmark run and fix it
allowed-tools: Bash(gh run*), Bash(gh workflow*), Bash(bash *), Bash(cargo *), Bash(zig *), Bash(python scripts/*), Read, Edit
---

Find the most recent failing run with `gh run list --status failure --limit 5`, pull its logs with
`gh run view <id> --log-failed`, and identify the actual failing step.

Reproduce locally before editing anything. The suites, by port: C `(cd c && bash test.sh --with-ports)`,
C++ `(cd cpp && bash test.sh)`, Rust `(cd rust && cargo test)`, Zig `(cd zig && zig build --release=fast && bash test.sh)`.
Generator and parity problems reproduce with `python3 scripts/generator_parity.py` and a smaller
`python3 scripts/bench_ports.py` run. Then fix the cause, not the assertion. If the failure is a
benchmark budget rather than a bug, say so and propose either an optimisation or a justified budget
change; do not silently raise a budget.
