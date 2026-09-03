---
description: Verify the DuckDB and pandas engines still return identical results
allowed-tools: Bash(python scripts/*), Bash(csvdiff *), Read
---

Generate 200k rows, run the comparison once per engine, and diff the `counts` and `columns` blocks
of the two JSON summaries. Report any field that differs and explain which engine is wrong and why.
This is the invariant CI enforces, so treat a mismatch as a blocking bug.
