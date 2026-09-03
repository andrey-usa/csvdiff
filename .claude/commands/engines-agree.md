---
description: Verify every installed engine still returns identical results
allowed-tools: Bash(pytest*), Bash(python scripts/*), Bash(csvdiff *), Read
---

Two levels, cheapest first:

1. `pytest tests/test_engines.py -q` — every installed engine against DuckDB on the examples,
   including the NULL-handling fixture and the `--export-dir` byte comparison.
2. Generate 200k rows and run `csvdiff compare ... --engine <name> --json /tmp/<name>.json` once per
   engine from `csvdiff.engines.available(contract_only=True)`, then diff the `counts` and `columns`
   blocks against DuckDB's.

Report any field that differs and explain which engine is wrong and why. This is the invariant CI
enforces, so treat a mismatch as a blocking bug — never loosen the comparison to make it pass.
`koala` is excluded on purpose: it is `contract=False` in the registry.
