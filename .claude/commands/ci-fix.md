---
description: Diagnose the latest failing CI or benchmark run and fix it
allowed-tools: Bash(gh run*), Bash(gh workflow*), Bash(pytest*), Bash(python scripts/*), Read, Edit
---

Find the most recent failing run with `gh run list --status failure --limit 5`, pull its logs with
`gh run view <id> --log-failed`, and identify the actual failing step.

Reproduce locally before editing anything — most failures reproduce with `pytest` or with the same
`scripts/bench.py` invocation at a smaller scale. Then fix the cause, not the assertion. If the
failure is a benchmark budget rather than a bug, say so and propose either an optimisation or a
justified budget change; do not silently raise a budget.
