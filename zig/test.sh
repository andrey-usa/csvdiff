#!/usr/bin/env bash
# Holds this port to the answers the Rust port gives, and checks that the memory
# budget is a bound rather than a suggestion. Run from zig/.
set -uo pipefail
cd "$(dirname "$0")"
ZIG=${ZIG:-/opt/zig/zig}
"$ZIG" build --release=fast || exit 2
RUST=../rust/target/release/csvdiff
[ -x "$RUST" ] || { echo "build the Rust port first: (cd ../rust && cargo build --release)"; exit 2; }
BIN=zig-out/bin/csvdiff
fail=0

check() {
  local label=$1; shift
  local a=../tests/fixtures/awkward_a.csv b=../tests/fixtures/awkward_b.csv r z
  r=$("$RUST" compare "$a" "$b" -k k "$@" --engine turbo -o /dev/null 2>&1 | head -1 | sed 's/ | turbo.*//')
  z=$($BIN compare "$a" "$b" -k k "$@" 2>&1 | head -1 | sed 's/ | turbo.*//')
  if [ "$r" = "$z" ]; then printf '  ok    %s\n' "$label"
  else printf '  FAIL  %s\n    rust: %s\n    zig : %s\n' "$label" "$r" "$z"; fail=1; fi
}

echo "awkward fixture, zig against rust:"
check "defaults"
check "--trim" --trim
# --ignore-case is excluded on purpose: this port refuses non-ASCII folding,
# which the fixture contains. See README.md.

echo "the memory budget is a bound, not a target:"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
# Enough rows that an index cannot fit in a very small budget.
{ echo "k,v"; for i in $(seq 1 20000); do echo "$i,value-$i"; done; } > "$tmp/a.csv"
{ echo "k,v"; for i in $(seq 1 20000); do echo "$i,value-$i"; done; } > "$tmp/b.csv"

out=$($BIN compare "$tmp/a.csv" "$tmp/b.csv" -k k --max-memory 1 2>&1 | head -1)
case "$out" in
  *"more than the 1 MB"*) echo "  ok    a budget too small is refused, naming the budget" ;;
  *) echo "  FAIL  expected a refusal, got: $out"; fail=1 ;;
esac

out=$($BIN compare "$tmp/a.csv" "$tmp/b.csv" -k k --max-memory 64 2>&1 | head -1)
case "$out" in
  *"matched 20000"*) echo "  ok    a sufficient budget completes with the right answer" ;;
  *) echo "  FAIL  expected a result, got: $out"; fail=1 ;;
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

  out=$($BIN compare "$pq/m_a.parquet" "$pq/m_b.csv" -k account_id,txn_id 2>&1 | head -1)
  case "$out" in
    *"one file is parquet"*) echo "  ok    a mixed parquet/text pair is refused" ;;
    *) echo "  FAIL  mixed pair: $out"; fail=1 ;;
  esac

  # The budget is what this port is for, and it has to hold on this path too.
  out=$($BIN compare "$pq/m_a.parquet" "$pq/m_b.parquet" -k account_id,txn_id --max-memory 1 2>&1 | head -1)
  case "$out" in
    *"more than the 1 MB"*) echo "  ok    --max-memory bounds the parquet path too" ;;
    *) echo "  FAIL  the budget was not enforced: $out"; fail=1 ;;
  esac
  rm -rf "$pq"
fi

exit $fail
