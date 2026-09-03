---
description: Compare two CSVs and summarise what changed
argument-hint: <file-a> <file-b> <key-columns> [extra flags]
allowed-tools: Bash(csvdiff *), Bash(python -m csvdiff*), Read
---

Compare `$1` and `$2` on key `$3` with any extra flags in `$4`, writing the report to
`out/report.html` and the summary to `out/summary.json`.

Read the summary and tell me: row and unique-key counts per file, duplicate keys, matched /
changed / added / removed, the columns with the most discrepancies, and any schema drift.
Flag anything that looks like a comparison setup problem rather than real data drift — a key that
is not unique, a column that changed in every row (usually a timestamp that wants `--ignore`), or
numeric noise that wants `--tolerance`.
