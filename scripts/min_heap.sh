#!/usr/bin/env bash
# Find the smallest heap each Java engine can finish a comparison in.
#
# Wall time answers "which is fastest on this machine". This answers the other question:
# which of them still work when the machine is smaller than the data. A binary search over
# -Xmx per engine, reporting the smallest heap that produced the right answer.
#
# Usage: min_heap.sh A.csv B.csv    (CSVDIFF_MAX_ROWS sets the report cap, default 1000)
set -euo pipefail

a=$1; b=$2
jar=${CSVDIFF_JAR:-java/target/csvdiff.jar}
java=${CSVDIFF_JAVA:-$(ls -d /opt/jdks/jdk-*/bin/java 2>/dev/null | sort -r | head -1)}
java=${java:-java}
engines=${CSVDIFF_ENGINES:-turbo swar shard mmap simd sortmerge native tablesaw duckdb}

# Rows embedded per section. This is the one part of a comparison that grows with the answer
# rather than the input, so it belongs in the search as a stated condition, not a default.
max_rows=${CSVDIFF_MAX_ROWS:-1000}
report=$(mktemp -d)/report.html
trap 'rm -rf "$(dirname "$report")"' EXIT

runs() {  # engine, heap in MB -> 0 if it finished; exit 1 just means differences were found
  "$java" -Xmx"$2"m --add-modules jdk.incubator.vector -cp "$jar" dev.csvdiff.Cli \
    compare "$a" "$b" -k account_id,txn_id -i updated_at --engine "$1" \
    --max-rows "$max_rows" -o "$report" >/dev/null 2>&1 || [ $? = 1 ]
}

printf '| Engine | Smallest heap that finishes |\n|---|---:|\n'
for engine in $engines; do
  # Anything that cannot do it in eight gigabytes is reported as such rather than searched for.
  if ! runs "$engine" 8192; then
    printf '| `%s` | over 8192 MB |\n' "$engine"
    continue
  fi
  low=32; high=8192
  while [ $((high - low)) -gt 16 ]; do
    mid=$(((low + high) / 2))
    if runs "$engine" "$mid"; then high=$mid; else low=$mid; fi
  done
  printf '| `%s` | %s MB |\n' "$engine" "$high"
done
