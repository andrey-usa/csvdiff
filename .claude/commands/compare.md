---
description: Compare two files and summarise what changed
argument-hint: <file-a> <file-b> <key-columns> [extra flags]
allowed-tools: Bash(c/csvdiff*), Bash(c/gen-data*), Bash(rust/target/release/csvdiff*), Bash(rust/target/release/gen-data*), Read
---

Build what you need first: `scripts/build_ports.sh c rust`, or just `make -C c` and
`cargo build --release --manifest-path rust/Cargo.toml`.

Compare `$1` and `$2` on key `$3` with any extra flags in `$4`. The C port is the fastest:
`c/csvdiff compare $1 $2 -k $3 $4`. Reach for the Rust port when you want the HTML report or the
JSON summary: `rust/target/release/csvdiff compare $1 $2 -k $3 -i updated_at $4 -o out/report.html --json out/summary.json`.

Exit code 1 means differences were found — that is the normal result, not a failure; anything at or
above 2 is an error.

Then tell me: row and unique-key counts per file, duplicate keys, matched / changed / added /
removed, the columns with the most discrepancies, and any schema drift. Flag anything that looks
like a comparison-setup problem rather than real drift — a key that is not unique, a column that
changed in every row (usually a timestamp that wants `--ignore`), or numeric noise that wants
`--tolerance`.

If the files do not exist, generate a pair first: `c/gen-data --rows 10k --out-dir data --prefix p`.
