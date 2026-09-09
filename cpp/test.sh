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

echo "the threaded csv sweep, over quoted newlines and chunk boundaries:"
# --- the threaded CSV sweep -------------------------------------------------
#
# Rows are found on every core, so a thread starts mid-file and has to work out
# whether it landed inside a quoted field. Nothing else in this suite reaches
# the 4 MB size where that splitting turns on, so this builds a file that does,
# out of the shapes that make a boundary hard: newlines inside quotes, doubled
# quotes, delimiters inside quotes, CRLF, blank lines, duplicate keys and rows
# present on one side only.
#
# Getting the in-quote state wrong at a boundary does not fail loudly -- it cuts
# a row in half, and the counts move. The check is that the thread count cannot
# change the answer. Modelled on the same section in `c/test.sh`.
thr=$(mktemp -d)
python3 - "$thr" <<'PYEOF'
import sys
out_dir = sys.argv[1]
FIELDS = ["plain", '"has,comma"', '"has\nnewline\nand more"',
          '"doubled ""quote"" inside"', '"comma, and\nnewline together"', '""']
def rows(side):
    out = ["k,a,b,c\n"]
    for i in range(130000):
        if side == "b" and i % 5000 == 0:
            continue                                   # removed from B
        a = FIELDS[(i * 7 + (3 if side == "b" and i % 41 == 0 else 0)) % len(FIELDS)]
        b = str(i * 3 + (1 if side == "b" and i % 23 == 0 else 0))
        c = '"tail\nwith newline"' if i % 11 == 0 else "tail"
        sep = "\r\n" if i % 13 == 0 else "\n"
        out.append(f"K{i:07d},{a},{b},{c}{sep}")
        if i % 997 == 0:
            out.append(f"K{i:07d},{a},{b},{c}{sep}")    # a duplicate key
        if i % 1499 == 0:
            out.append("\n")                           # a blank line is not a row
    if side == "b":
        for i in range(40):
            out.append(f"NEW{i:05d},plain,0,tail\n")    # added in B
    return "".join(out)
for side in ("a", "b"):
    open(f"{out_dir}/t_{side}.csv", "w").write(rows(side))
PYEOF
bytes=$(wc -c < "$thr/t_a.csv")
if [ "$bytes" -lt 4194304 ]; then
  echo "  FAIL  the threading fixture is $bytes bytes, under the 4 MB split threshold"; fail=1
else
  # The Rust port groups its digits and this one does not, which no other check
  # here notices because their fixtures hold fewer than a thousand rows.
  plain() { head -1 | sed 's/ | turbo.*//' | tr -d ','; }
  one=$(build/csvdiff compare "$thr/t_a.csv" "$thr/t_b.csv" -k k --threads 1 2>&1 | plain) || true
  for t in 2 3 4 7 16; do
    many=$(build/csvdiff compare "$thr/t_a.csv" "$thr/t_b.csv" -k k --threads $t 2>&1 | plain) || true
    if [ "$one" = "$many" ]; then
      printf '  ok    %s threads finds what 1 thread finds\n' "$t"
    else
      printf '  FAIL  %s threads disagrees with 1 thread\n    1 : %s\n    %s : %s\n' \
             "$t" "$one" "$t" "$many"; fail=1
    fi
  done
  r=$("$RUST" compare "$thr/t_a.csv" "$thr/t_b.csv" -k k --engine turbo -o /dev/null 2>&1 | plain) || true
  [ "$one" = "$r" ] && echo "  ok    and what the rust port finds" \
    || { echo "  FAIL  the threaded sweep disagrees with the rust port"; echo "    rust: $r"; fail=1; }
fi
rm -rf "$thr"

echo "quoting, ragged rows and keys near the end of the file:"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
printf 'a,k,c\nx,K1,c1\ny,K2,cc\n'        > "$tmp/a.csv"
printf 'a,k,c\nx,K1,c1\ny,K2,cccccccc\n'  > "$tmp/b.csv"
r=$("$RUST" compare "$tmp/a.csv" "$tmp/b.csv" -k k --engine turbo -o /dev/null 2>&1 | head -1 | sed 's/ | turbo.*//') || true
c=$(build/csvdiff compare "$tmp/a.csv" "$tmp/b.csv" -k k 2>&1 | head -1 | sed 's/ | turbo.*//') || true
[ "$r" = "$c" ] && echo "  ok    key in the last bytes of the file" || { echo "  FAIL  key near end: rust=$r cpp=$c"; fail=1; }

echo "written natively and read back:"
# The generator writes Parquet from the same field-by-field recipe it writes CSV
# from, so the two can be held against each other without a third tool in the
# way -- and these checks run wherever the port builds, rather than only where
# DuckDB happens to be installed.
make gen-data >/dev/null
tmpg=$(mktemp -d)
gen() { build/gen-data --rows "$1" --out-dir "$tmpg" --prefix "$2" "${@:3}" >/dev/null; }
gencheck() { # label, rows, prefix, then generator flags
  local label=$1 rows=$2 prefix=$3; shift 3
  gen "$rows" "$prefix"
  gen "$rows" "$prefix" "$@"
  local ext=.parquet
  case " $* " in
    *" json "*) ext=.ndjson ;;
    *" none "*) ext=.unc.parquet ;;
  esac
  build/csvdiff compare "$tmpg/${prefix}_a.csv" "$tmpg/${prefix}_b.csv" \
    -k account_id,txn_id -i updated_at --json "$tmpg/csv.json" >/dev/null 2>&1 || true
  build/csvdiff compare "$tmpg/${prefix}_a$ext" "$tmpg/${prefix}_b$ext" \
    -k account_id,txn_id -i updated_at --json "$tmpg/pq.json" >/dev/null 2>&1 || true
  if python3 - "$tmpg/csv.json" "$tmpg/pq.json" <<'PYEOF'
import json, sys
def load(path):
    d = json.load(open(path))
    for gone in ("a", "b", "seconds"):
        d["meta"].pop(gone, None)
    return d
sys.exit(0 if load(sys.argv[1]) == load(sys.argv[2]) else 1)
PYEOF
  then printf '  ok    %s\n' "$label"
  else printf '  FAIL  %s: the parquet report differs from the csv one\n' "$label"; fail=1; fi
  rm -f "$tmpg/${prefix}"_*
}
gencheck "newline-delimited json" 20k s0 --format json
gencheck "snappy, one row group" 20k s1 --format parquet --compression snappy
gencheck "uncompressed" 20k s2 --format parquet --compression none
gencheck "many small row groups" 20k s3 --format parquet --compression snappy --row-group-size 512
# A dictionary budget the data crosses partway makes a column that is dictionary
# encoded in some row groups and plain in others -- the shape a real writer
# produces on a high-cardinality string, and the one the reader has to fold into
# a single form. With these numbers three of the twenty columns come out mixed.
gencheck "a column the dictionary gives up on" 20k s4 \
  --format parquet --compression none --dict-limit 175 --row-group-size 300
gencheck "every column plain" 20k s5 --format parquet --compression snappy --dict-limit 1
gencheck "rows that do not fill a group" 1k s6 --format parquet --compression none

# Round-tripping the writer through the reader only proves they agree with each
# other. Reading the file back with a tool that had no part in writing it is
# what says it is Parquet rather than something that merely looks like it.
DUCK=""
for cand in ../bench/external/tools/duckdb "$(command -v duckdb || true)"; do
  [ -n "$cand" ] && [ -x "$cand" ] && { DUCK=$cand; break; }
done
if [ -n "$DUCK" ]; then
  gen 20k iop
  gen 20k iop --format parquet --compression snappy
  ours=$("$DUCK" -noheader -csv -c "
    SELECT md5(string_agg(account_id||'|'||txn_id||'|'||coalesce(value_date,'')||'|'||
                          currency||'|'||amount||'|'||note, ':'))
    FROM read_parquet('$tmpg/iop_b.parquet');" 2>/dev/null)
  csv=$("$DUCK" -noheader -csv -c "
    SELECT md5(string_agg(account_id||'|'||txn_id||'|'||coalesce(value_date,'')||'|'||
                          currency||'|'||amount||'|'||note, ':'))
    FROM read_csv('$tmpg/iop_b.csv', all_varchar = true, header = true, sample_size = -1);" 2>/dev/null)
  if [ -n "$ours" ] && [ "$ours" = "$csv" ]; then
    echo "  ok    duckdb reads what we wrote, to the same digest as the csv"
  else
    echo "  FAIL  duckdb read our parquet differently from the csv"; fail=1
  fi
  # And the other way: our file against DuckDB's file of the same rows.
  "$DUCK" -c "COPY (SELECT * FROM read_csv('$tmpg/iop_b.csv', all_varchar = true,
              header = true, sample_size = -1)) TO '$tmpg/duck.parquet'
              (FORMAT PARQUET, COMPRESSION snappy);" >/dev/null 2>&1
  out=$(build/csvdiff compare "$tmpg/iop_b.parquet" "$tmpg/duck.parquet" \
        -k account_id,txn_id 2>&1 | head -1 | sed "s/ | turbo.*//") || true
  case "$out" in
    *"(changed 0)"*"added 0 | removed 0"*)
      echo "  ok    our parquet and duckdb's of the same rows are identical" ;;
    *) echo "  FAIL  ours against duckdb's: $out"; fail=1 ;;
  esac
else
  echo "  skip  no duckdb, so the interop checks did not run"
fi
rm -rf "$tmpg"

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

# A misspelled --ignore used to widen the comparison in silence: the column it
# meant to drop got compared and came back changed on every row. `--key` and
# `--compare` have always refused an unknown name; this is the third.
igdir=$(mktemp -d)
printf 'id,a,b\nk1,p,q\n' > "$igdir/a.csv"
printf 'id,a,b\nk1,p,X\n' > "$igdir/b.csv"
out=$(build/csvdiff compare "$igdir/a.csv" "$igdir/b.csv" -k id -i no_such_column 2>&1) || true
case "$out" in
  *"present in neither file"*) echo "  ok    an unknown --ignore name is refused" ;;
  *) echo "  FAIL  expected a refusal, got: $out"; fail=1 ;;
esac
# The other half: a real name still works, so the check refuses typos rather
# than refusing --ignore.
out=$(build/csvdiff compare "$igdir/a.csv" "$igdir/b.csv" -k id -i b 2>&1) || true
case "$out" in
  *"changed 0"*) echo "  ok    a real --ignore name is still accepted" ;;
  *) echo "  FAIL  a real --ignore name should be accepted, got: $out"; fail=1 ;;
esac
rm -rf "$igdir"

exit $fail
