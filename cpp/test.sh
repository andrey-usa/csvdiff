#!/usr/bin/env bash
# Holds this port to the answers the Rust port gives, on the fixture built from
# every shape that has broken an engine in this project. Run from cpp/.
# No pipefail: a compare exits 1 when it finds differences, which is the
# expected outcome of almost every check here.
set -uo pipefail
set +o pipefail
set -e
cd "$(dirname "$0")"
make >/dev/null
RUST=../rust/target/release/csvdiff
[ -x "$RUST" ] || { echo "build the Rust port first: (cd ../rust && cargo build --release)"; exit 2; }

fail=0
check() { # label, then the flags both are given
  local label=$1; shift
  local a=../tests/fixtures/awkward_a.csv b=../tests/fixtures/awkward_b.csv
  local r c
  r=$("$RUST" compare "$a" "$b" -k k "$@" --engine turbo -o /dev/null 2>&1 | head -1 | sed 's/ | turbo.*//') || true
  c=$(build/csvdiff compare "$a" "$b" -k k "$@" 2>&1 | head -1 | sed 's/ | turbo.*//') || true
  if [ "$r" = "$c" ]; then
    printf '  ok    %s\n' "$label"
  else
    printf '  FAIL  %s\n    rust: %s\n    c++ : %s\n' "$label" "$r" "$c"; fail=1
  fi
}

echo "awkward fixture, c++ against rust:"
check "defaults"
check "--trim" --trim
# --ignore-case is deliberately excluded: this port refuses non-ASCII folding,
# which the fixture contains on purpose. See README.md.

echo "newline-delimited JSON:"
tmpj=$(mktemp -d)
# The same rows twice: once with the keys in one order and the escapes spelled
# with \uXXXX, once in another order with the characters written literally. A
# JSON writer is free to do either, so these must compare equal.
printf '{"k":"1","v":"caf\\u00e9","w":"x"}\n{"k":"2","v":"has \\"quotes\\"","w":"y"}\n{"k":"3","v":"line\\nbreak","w":"z"}\n{"k":"4","v":null,"w":"w"}\n{"k":"5","v":"emoji \\ud83d\\ude00","w":"v"}\n' > "$tmpj/a.ndjson"
printf '{"w":"x","k":"1","v":"caf\xc3\xa9"}\n{"w":"y","k":"2","v":"has \\"quotes\\""}\n{"w":"z","k":"3","v":"line\\nbreak"}\n{"w":"w","k":"4","v":null}\n{"w":"v","k":"5","v":"emoji \xf0\x9f\x98\x80"}\n' > "$tmpj/b.ndjson"
out=$(./build/csvdiff compare "$tmpj/a.ndjson" "$tmpj/b.ndjson" -k k 2>&1 | head -1 | sed "s/ | turbo.*//") || true
case "$out" in
  *"matched 5 (changed 0)"*) echo "  ok    key order, \\uXXXX against literal UTF-8, null, surrogate pair" ;;
  *) echo "  FAIL  json escapes: $out"; fail=1 ;;
esac

# A CSV file against the JSON of the same rows: the formats meet at the join.
printf 'k,v,w\n1,alpha,x\n2,beta,y\n3,gamma,z\n' > "$tmpj/c.csv"
printf '{"k":"1","v":"alpha","w":"x"}\n{"k":"2","v":"beta","w":"y"}\n{"k":"3","v":"CHANGED","w":"z"}\n' > "$tmpj/c.ndjson"
out=$(./build/csvdiff compare "$tmpj/c.csv" "$tmpj/c.ndjson" -k k 2>&1 | head -1 | sed "s/ | turbo.*//") || true
case "$out" in
  *"matched 3 (changed 1)"*) echo "  ok    a CSV file compared against a JSON file" ;;
  *) echo "  FAIL  mixed csv/json: $out"; fail=1 ;;
esac
rm -rf "$tmpj"

echo "quoting, ragged rows and keys near the end of the file:"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
printf 'a,k,c\nx,K1,c1\ny,K2,cc\n'        > "$tmp/a.csv"
printf 'a,k,c\nx,K1,c1\ny,K2,cccccccc\n'  > "$tmp/b.csv"
r=$("$RUST" compare "$tmp/a.csv" "$tmp/b.csv" -k k --engine turbo -o /dev/null 2>&1 | head -1 | sed 's/ | turbo.*//') || true
c=$(build/csvdiff compare "$tmp/a.csv" "$tmp/b.csv" -k k 2>&1 | head -1 | sed 's/ | turbo.*//') || true
[ "$r" = "$c" ] && echo "  ok    key in the last bytes of the file" || { echo "  FAIL  key near end: rust=$r cpp=$c"; fail=1; }

echo "parquet, read natively and compared columnwise:"
# The claim the columnar path has to earn is that it is the *same* comparison:
# the same data, in a format that stores it as columns of dictionary indices
# rather than as lines of text, must produce a byte-identical report. So the
# fixture is converted and the two JSON documents are diffed in full -- counts,
# per-column statistics, every changed cell, every added and removed row, the
# duplicate sections -- with only the file names and the timing removed.
DUCK=""
for cand in ../bench/external/tools/duckdb "$(command -v duckdb || true)"; do
  [ -n "$cand" ] && [ -x "$cand" ] && { DUCK=$cand; break; }
done
if [ -z "$DUCK" ]; then
  echo "  skip  no duckdb to write parquet with"
else
  tmpp=$(mktemp -d)
  for side in a b; do
    for spec in "snappy:$tmpp/$side.parquet:122880" "uncompressed:$tmpp/$side.unc.parquet:8"; do
      IFS=: read -r codec dest rg <<< "$spec"
      "$DUCK" -c "COPY (SELECT * FROM read_csv('../tests/fixtures/awkward_$side.csv',
                   all_varchar = true, header = true, sample_size = -1, null_padding = true))
                 TO '$dest' (FORMAT PARQUET, COMPRESSION $codec, ROW_GROUP_SIZE $rg);" >/dev/null
    done
  done
  # A small row group size on the uncompressed side is not decoration: it makes
  # the writer start a fresh dictionary per group, and at some point give up on
  # the dictionary and write plain pages instead, which is the mixed-encoding
  # column the reader has to fold together.
  pqcheck() { # label, then the flags
    local label=$1; shift
    build/csvdiff compare ../tests/fixtures/awkward_a.csv ../tests/fixtures/awkward_b.csv \
      -k k "$@" --json "$tmpp/csv.json" >/dev/null 2>&1 || true
    local bad=0
    for kind in "" ".unc"; do
      build/csvdiff compare "$tmpp/a$kind.parquet" "$tmpp/b$kind.parquet" \
        -k k "$@" --json "$tmpp/pq.json" >/dev/null 2>&1 || true
      python3 - "$tmpp/csv.json" "$tmpp/pq.json" <<'PYEOF' || bad=1
import json, sys
def load(path):
    d = json.load(open(path))
    for gone in ("a", "b", "seconds"):
        d["meta"].pop(gone, None)
    return d
sys.exit(0 if load(sys.argv[1]) == load(sys.argv[2]) else 1)
PYEOF
    done
    if [ "$bad" = 0 ]; then printf '  ok    %s\n' "$label"
    else printf '  FAIL  %s: the parquet report differs from the csv one\n' "$label"; fail=1; fi
  }
  pqcheck "same report as the csv it was written from"
  pqcheck "--trim" --trim
  pqcheck "--empty-is-null" --empty-is-null
  pqcheck "--tolerance falls back to the byte path" --tolerance 0.5
  pqcheck "--max-rows caps the same rows" --max-rows 2

  # A key column both sides store as a dictionary takes the shared-id path,
  # where the join never looks at a byte after the dictionaries are mapped.
  pqcheck "a dictionary key column" -k v

  # Parquet has real nulls; CSV has only empty fields. Both must read as absent.
  "$DUCK" -c "COPY (SELECT * FROM (VALUES ('k1','a',NULL),('k2',NULL,''),('k3','c','z'))
              v(k, p, q)) TO '$tmpp/n_a.parquet' (FORMAT PARQUET);
              COPY (SELECT * FROM (VALUES ('k1','a',''),('k2','',NULL),('k3','c','y'))
              v(k, p, q)) TO '$tmpp/n_b.parquet' (FORMAT PARQUET);" >/dev/null
  out=$(build/csvdiff compare "$tmpp/n_a.parquet" "$tmpp/n_b.parquet" -k k 2>&1 | head -1 |
        sed "s/ | turbo.*//") || true
  case "$out" in
    *"matched 3 (changed 1)"*) echo "  ok    a null and an empty string are both absent" ;;
    *) echo "  FAIL  parquet nulls: $out"; fail=1 ;;
  esac

  # A column a writer starts as a dictionary and gives up on partway is the
  # shape DuckDB produces on any high-cardinality string at scale, and the one
  # the reader has to fold into a single form. Built here on purpose: `v` is
  # five distinct values for the first two row groups and then all distinct,
  # `w` carries nulls, and the two sides are written with different codecs so
  # the compressed and uncompressed slice bases both get exercised in one
  # comparison.
  "$DUCK" -c "
    CREATE TABLE t AS SELECT 'k'||lpad(i::VARCHAR, 6, '0') AS k,
      CASE WHEN i < 8192 THEN 'lo'||(i%5) ELSE 'hi'||i END AS v,
      CASE WHEN i%3 = 0 THEN NULL ELSE 'w'||(i%7) END AS w FROM range(20000) t(i);
    CREATE TABLE u AS SELECT k, CASE WHEN i%1000 = 0 THEN 'X' ELSE v END AS v, w
      FROM (SELECT *, row_number() OVER () - 1 AS i FROM t);
    COPY (SELECT * FROM t) TO '$tmpp/m_a.parquet'
      (FORMAT PARQUET, COMPRESSION uncompressed, ROW_GROUP_SIZE 4096);
    COPY (SELECT * FROM u) TO '$tmpp/m_b.parquet'
      (FORMAT PARQUET, COMPRESSION snappy, ROW_GROUP_SIZE 4096);
    COPY (SELECT * FROM t) TO '$tmpp/m_a.csv' (HEADER, FORMAT CSV);
    COPY (SELECT * FROM u) TO '$tmpp/m_b.csv' (HEADER, FORMAT CSV);" >/dev/null
  build/csvdiff compare "$tmpp/m_a.csv" "$tmpp/m_b.csv" -k k --json "$tmpp/m1.json" \
    >/dev/null 2>&1 || true
  build/csvdiff compare "$tmpp/m_a.parquet" "$tmpp/m_b.parquet" -k k --json "$tmpp/m2.json" \
    >/dev/null 2>&1 || true
  if python3 - "$tmpp/m1.json" "$tmpp/m2.json" <<'PYEOF'
import json, sys
def load(path):
    d = json.load(open(path))
    for gone in ("a", "b", "seconds"):
        d["meta"].pop(gone, None)
    return d
sys.exit(0 if load(sys.argv[1]) == load(sys.argv[2]) else 1)
PYEOF
  then echo "  ok    a column that starts as a dictionary and gives up partway"
  else echo "  FAIL  mixed dictionary/plain column"; fail=1; fi

  # One side parquet and one side text is refused rather than half-answered.
  out=$(build/csvdiff compare "$tmpp/a.parquet" ../tests/fixtures/awkward_b.csv -k k 2>&1) || true
  case "$out" in
    *"one file is parquet and the other is not"*) echo "  ok    a mixed parquet/text pair is refused" ;;
    *) echo "  FAIL  mixed parquet/text: $out"; fail=1 ;;
  esac
  rm -rf "$tmpp"
fi

exit $fail
