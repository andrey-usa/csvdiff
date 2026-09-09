#!/usr/bin/env bash
# Holds this port to the answers the Rust port gives -- on every input format,
# at every thread count -- and checks that the memory budget is a bound rather
# than a suggestion. Run from zig/.
set -uo pipefail
cd "$(dirname "$0")"
ZIG=${ZIG:-zig}
"$ZIG" build --release=fast || exit 2
RUST=../rust/target/release/csvdiff
[ -x "$RUST" ] || { echo "build the Rust port first: (cd ../rust && cargo build --release)"; exit 2; }
BIN=zig-out/bin/csvdiff
FIX=../tests/fixtures
fail=0

# The two ports print the same facts in different shapes; this is the part both
# agree on, with the thousands separators, the engine label and the timing
# stripped -- the label is `parquet` on a Parquet pair and `turbo` otherwise,
# and only one of the two ports prints a time after it.
answer() { sed 's/ | \(turbo\|parquet\).*//; s/,//g' ; }

check() {
  local label=$1 a=$2 b=$3; shift 3
  local r z
  r=$("$RUST" compare "$a" "$b" "$@" --engine turbo -o /dev/null 2>&1 | head -1 | answer)
  z=$($BIN compare "$a" "$b" "$@" 2>&1 | head -1 | answer)
  if [ "$r" = "$z" ]; then printf '  ok    %s\n' "$label"
  else printf '  FAIL  %s\n    rust: %s\n    zig : %s\n' "$label" "$r" "$z"; fail=1; fi
}

echo "awkward fixture, zig against rust:"
check "defaults" "$FIX/awkward_a.csv" "$FIX/awkward_b.csv" -k k
check "--trim" "$FIX/awkward_a.csv" "$FIX/awkward_b.csv" -k k --trim
# --ignore-case is excluded on purpose: this port refuses non-ASCII folding,
# which the fixture contains. See README.md.

echo "every input format reads the same way:"
for a in a.csv a.ndjson a_dict_snappy.parquet a_plain_none.parquet a_dict_gzip.parquet \
         a_plain_zstd.parquet a_plain_lz4.parquet a_delta_v2.parquet a_dict_row_groups.parquet; do
  check "$a against b.csv" "$FIX/formats/$a" "$FIX/formats/b.csv" -k id
done
check "parquet against parquet" "$FIX/formats/a_dict_snappy.parquet" \
      "$FIX/formats/b_dict_snappy.parquet" -k id
check "json against parquet" "$FIX/formats/a.ndjson" "$FIX/formats/b_dict_snappy.parquet" -k id

echo "a typed parquet column renders as the csv of the same data:"
out=$($BIN compare "$FIX/formats/typed.parquet" "$FIX/formats/typed.csv" -k id 2>&1 | head -1)
case "$out" in
  *"matched 8 (changed 0)"*) echo "  ok    every column matches its stated text" ;;
  *) echo "  FAIL  expected no differences, got: $out"; fail=1 ;;
esac

echo "the thread count does not change the answer:"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
# Over the 4 MB threshold where the file is split into chunks, with quoted
# newlines and doubled quotes at the boundaries -- the case chunking can break.
python3 - "$tmp" <<'PY'
import sys
d = sys.argv[1]
with open(f"{d}/a.csv", "w") as a, open(f"{d}/b.csv", "w") as b:
    a.write("k,v,w\n"); b.write("k,v,w\n")
    for i in range(40000):
        quoted = f'"line {i}\nsecond, with ""quotes"" in it"'
        a.write(f"K{i},{quoted},{i}\n")
        b.write(f"K{i},{quoted},{i + 1 if i % 7 == 0 else i}\n")
PY
reference=""
for threads in 1 2 3 4 8; do
  got=$($BIN compare "$tmp/a.csv" "$tmp/b.csv" -k k --threads $threads 2>&1 | head -1 | answer)
  if [ -z "$reference" ]; then reference=$got
  elif [ "$got" != "$reference" ]; then
    printf '  FAIL  %s threads gave a different answer\n    %s\n' "$threads" "$got"; fail=1
  fi
done
[ $fail -eq 0 ] && echo "  ok    one answer at 1, 2, 3, 4 and 8 threads"
check "and it is the answer rust gives" "$tmp/a.csv" "$tmp/b.csv" -k k

echo "the memory budget is a bound, not a target:"
# Enough rows that an index cannot fit in a very small budget.
{ echo "k,v"; for i in $(seq 1 20000); do echo "$i,value-$i"; done; } > "$tmp/small_a.csv"
cp "$tmp/small_a.csv" "$tmp/small_b.csv"

out=$($BIN compare "$tmp/small_a.csv" "$tmp/small_b.csv" -k k --max-memory 1 2>&1 | head -1)
case "$out" in
  *"more than the 1 MB"*) echo "  ok    a budget too small is refused, naming the budget" ;;
  *) echo "  FAIL  expected a refusal, got: $out"; fail=1 ;;
esac

# A misspelled --ignore used to widen the comparison in silence: the column it
# meant to drop got compared and came back changed on every row. `--key` and
# `--compare` have always refused an unknown name; this is the third.
out=$($BIN compare "$tmp/small_a.csv" "$tmp/small_b.csv" -k k -i no_such_column 2>&1 | head -1)
case "$out" in
  *"present in neither file"*) echo "  ok    an unknown --ignore name is refused" ;;
  *) echo "  FAIL  expected a refusal, got: $out"; fail=1 ;;
esac

# The other half: a name that is really there still works, so the check refuses
# typos rather than refusing --ignore.
out=$($BIN compare "$tmp/small_a.csv" "$tmp/small_b.csv" -k k -i v 2>&1 | head -1)
case "$out" in
  *"matched 20000"*) echo "  ok    a real --ignore name is still accepted" ;;
  *) echo "  FAIL  a real --ignore name should be accepted, got: $out"; fail=1 ;;
esac

out=$($BIN compare "$tmp/small_a.csv" "$tmp/small_b.csv" -k k --max-memory 64 2>&1 | head -1)
case "$out" in
  *"matched 20000"*) echo "  ok    a budget that is enough finishes inside it" ;;
  *) echo "  FAIL  expected a comparison, got: $out"; fail=1 ;;
esac

echo "parquet, read natively and compared columnwise:"
# The claim the columnar path has to earn is that it is the *same* comparison:
# the same rows, in a format that stores them as columns of dictionary indices
# rather than as lines of text, must produce the same counts and the same
# per-column statistics. The files come from cpp/build/gen-data, which writes
# CSV and Parquet from one field-by-field recipe.
GEN=../cpp/build/gen-data
if [ ! -x "$GEN" ]; then
  echo "  skip  ../cpp/build/gen-data is not built: (cd ../cpp && make gen-data)"
else
  pq=$(mktemp -d)
  same() { # label, generator flags, extension, then compare flags
    local label=$1 flags=$2 ext=$3; shift 3
    "$GEN" --rows 20k --out-dir "$pq" --prefix t >/dev/null
    # shellcheck disable=SC2086
    "$GEN" --rows 20k --out-dir "$pq" --prefix t $flags >/dev/null
    $BIN compare "$pq/t_a.csv" "$pq/t_b.csv" "$@" --json "$pq/csv.json" >/dev/null 2>&1
    $BIN compare "$pq/t_a$ext" "$pq/t_b$ext" "$@" --json "$pq/pq.json" >/dev/null 2>&1
    if python3 - "$pq/csv.json" "$pq/pq.json" <<'PYEOF'
import json, sys
def load(path):
    d = json.load(open(path))
    return (d["counts"], d["columns"])
sys.exit(0 if load(sys.argv[1]) == load(sys.argv[2]) else 1)
PYEOF
    then
      printf '  ok    %s\n' "$label"
    else
      printf '  FAIL  %s: the parquet answer differs from the csv one\n' "$label"; fail=1
    fi
    rm -f "$pq"/t_*
  }
  K=(-k account_id,txn_id -i updated_at)
  same "snappy"                  "--format parquet --compression snappy" .parquet "${K[@]}"
  same "uncompressed"            "--format parquet --compression none"   .unc.parquet "${K[@]}"
  same "many small row groups"   "--format parquet --compression snappy --row-group-size 512" .parquet "${K[@]}"
  # A dictionary budget the data crosses partway makes a column that is
  # dictionary encoded in some row groups and plain in others -- the shape a
  # real writer produces at scale, and the one the reader has to fold together.
  same "the dictionary gives up" "--format parquet --compression none --dict-limit 175 --row-group-size 300" .unc.parquet "${K[@]}"
  same "every column plain"      "--format parquet --compression snappy --dict-limit 1" .parquet "${K[@]}"
  same "--trim"                  "--format parquet --compression snappy" .parquet "${K[@]}" --trim
  same "--empty-is-null"         "--format parquet --compression snappy" .parquet "${K[@]}" --empty-is-null
  # A tolerance makes equality non-transitive, so it cannot be given an id: the
  # column falls back to comparing bytes, and must still agree.
  same "--tolerance"             "--format parquet --compression snappy" .parquet "${K[@]}" --tolerance 0.01
  # A key column both sides store as a dictionary takes the shared-id path,
  # where the join never looks at a byte after the dictionaries are mapped.
  same "a dictionary key column" "--format parquet --compression snappy" .parquet -k currency,status

  "$GEN" --rows 1k --out-dir "$pq" --prefix m >/dev/null
  "$GEN" --rows 1k --out-dir "$pq" --prefix m --format parquet --compression snappy >/dev/null

  # A mixed pair has no column to compare a byte stream against, so the
  # columnar path cannot take it -- but the text engine can, by materialising
  # the Parquet side into rows (see pqread.zig). Slower, and the same answer:
  # that is what this checks, against csv-vs-csv on the same rows.
  $BIN compare "$pq/m_a.csv" "$pq/m_b.csv" -k account_id,txn_id \
       --json "$pq/text.json" >/dev/null 2>&1
  $BIN compare "$pq/m_a.parquet" "$pq/m_b.csv" -k account_id,txn_id \
       --json "$pq/mixed.json" >/dev/null 2>&1
  if python3 - "$pq/text.json" "$pq/mixed.json" <<'PYEOF'
import json, sys
def load(path):
    d = json.load(open(path))
    return (d["counts"], d["columns"])
sys.exit(0 if load(sys.argv[1]) == load(sys.argv[2]) else 1)
PYEOF
  then echo "  ok    a mixed parquet/text pair is read, not refused"
  else echo "  FAIL  the mixed pair disagrees with csv against csv"; fail=1; fi

  # The budget is what this port is for, and it has to hold on this path too.
  out=$($BIN compare "$pq/m_a.parquet" "$pq/m_b.parquet" -k account_id,txn_id --max-memory 1 2>&1 | head -1)
  case "$out" in
    *"more than the 1 MB"*) echo "  ok    --max-memory bounds the parquet path too" ;;
    *) echo "  FAIL  the budget was not enforced: $out"; fail=1 ;;
  esac
  rm -rf "$pq"
fi

# Parquet decodes into an arena taken from the same allocator, so the budget has
# to bound that path too rather than only the mapped one. It needs a file large
# enough that the arena is the thing that does not fit.
../rust/target/release/gen-data --rows 20k --out-dir "$tmp" --prefix pq --format parquet >/dev/null
out=$($BIN compare "$tmp/pq_a.parquet" "$tmp/pq_b.parquet" -k account_id,txn_id -i updated_at \
      --max-memory 4 2>&1 | head -1)
case "$out" in
  *"more than the 4 MB"*) echo "  ok    reading parquet stays inside the budget too" ;;
  *) echo "  FAIL  expected a refusal, got: $out"; fail=1 ;;
esac
out=$($BIN compare "$tmp/pq_a.parquet" "$tmp/pq_b.parquet" -k account_id,txn_id -i updated_at \
      --max-memory 128 2>&1 | head -1)
case "$out" in
  *"matched 19980"*) echo "  ok    and finishes when the budget is enough" ;;
  *) echo "  FAIL  expected a comparison, got: $out"; fail=1 ;;
esac

echo "unit tests:"
if "$ZIG" build test; then echo "  ok    scan, field, slab, text, thrift, codec, encoding, parquet"
else echo "  FAIL  zig build test"; fail=1; fi

# The columnar reader takes uncompressed and snappy; the reader beside it takes
# zstd, gzip and lz4. A capability refusal falls through to that one, so a file
# this binary carries a decoder for is read rather than turned away. Before the
# fall-through a zstd pair was refused -- and, on a large enough file, the
# refusal path freed an uninitialized column array and took the process down
# with SIGSEGV.
echo "parquet codecs, and which reader takes them:"
fx=../tests/fixtures/formats
for spec in "a_plain_none parquet" "a_dict_snappy parquet" \
            "a_plain_zstd turbo" "a_dict_gzip turbo" "a_plain_lz4 turbo"; do
  set -- $spec
  out=$($BIN compare "$fx/$1.parquet" "$fx/$1.parquet" -k id 2>&1 | head -1)
  case "$out" in
    *"matched 301"*"| $2")
      echo "  ok    $1 is read, by the $2 reader" ;;
    *"matched 301"*)
      echo "  FAIL  $1 was read but by the wrong reader: $out"; fail=1 ;;
    *)
      echo "  FAIL  $1 was not read: $out"; fail=1 ;;
  esac
done

# A file that is actually broken must still fail, rather than being handed to
# the second reader and failing there with a message about the wrong thing.
trunc=$(mktemp -d)
head -c 400 "$fx/a_plain_zstd.parquet" > "$trunc/cut.parquet"
out=$($BIN compare "$trunc/cut.parquet" "$trunc/cut.parquet" -k id 2>&1 | head -1)
case "$out" in
  *"matched"*) echo "  FAIL  a truncated parquet file was read: $out"; fail=1 ;;
  *) echo "  ok    a truncated parquet file is still refused" ;;
esac
rm -rf "$trunc"

exit $fail
