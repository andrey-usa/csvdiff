#!/usr/bin/env bash
# Holds this port to the answers the Rust port gives, on the fixture built from
# every shape that has broken an engine in this project. Run from c/.
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
summary() { head -1 | sed 's/ | turbo.*//'; }

check() { # label, then the flags both are given
  local label=$1; shift
  local a=../tests/fixtures/awkward_a.csv b=../tests/fixtures/awkward_b.csv
  local r c
  r=$("$RUST" compare "$a" "$b" -k k "$@" --engine turbo -o /dev/null 2>&1 | summary) || true
  c=$(./csvdiff compare "$a" "$b" -k k "$@" 2>&1 | summary) || true
  if [ "$r" = "$c" ]; then
    printf '  ok    %s\n' "$label"
  else
    printf '  FAIL  %s\n    rust: %s\n    c   : %s\n' "$label" "$r" "$c"; fail=1
  fi
}

echo "awkward fixture, c against rust:"
check "defaults"
# --trim and --ignore-case are not implemented here on purpose; see README.md.

echo "per-column counts, c against rust:"
a=../tests/fixtures/awkward_a.csv b=../tests/fixtures/awkward_b.csv
"$RUST" compare "$a" "$b" -k k --engine turbo -o /dev/null --json /tmp/c_test_rust.json >/dev/null 2>&1 || true
./csvdiff compare "$a" "$b" -k k --json /tmp/c_test_c.json >/dev/null 2>&1 || true
if python3 - <<'PY'
import json, sys
r = json.load(open("/tmp/c_test_rust.json"))
c = json.load(open("/tmp/c_test_c.json"))
sys.exit(0 if (r["counts"], r["columns"]) == (c["counts"], c["columns"]) else 1)
PY
then echo "  ok    counts and per-column changed/blanked/filled"
else echo "  FAIL  counts or per-column stats differ"; fail=1
fi

echo "quoting, ragged rows and keys near the end of the file:"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
printf 'a,k,c\nx,K1,c1\ny,K2,cc\n'        > "$tmp/a.csv"
printf 'a,k,c\nx,K1,c1\ny,K2,cccccccc\n'  > "$tmp/b.csv"
r=$("$RUST" compare "$tmp/a.csv" "$tmp/b.csv" -k k --engine turbo -o /dev/null 2>&1 | summary) || true
c=$(./csvdiff compare "$tmp/a.csv" "$tmp/b.csv" -k k 2>&1 | summary) || true
[ "$r" = "$c" ] && echo "  ok    key in the last bytes of the file" || { echo "  FAIL  key near end: rust=$r c=$c"; fail=1; }


# --- the Parquet path -------------------------------------------------------
#
# The claim the columnar path has to earn is that it is the *same* comparison:
# the same rows, in a format that stores them as columns of dictionary indices
# rather than as lines of text, must produce the same counts and the same
# per-column changed/blanked/filled. So each case generates one pair in both
# formats and requires the two reports to agree in full.
#
# The fixtures come from cpp/build/gen-data, which writes CSV and Parquet from
# one field-by-field recipe. Where it is not built these skip by name rather
# than silently passing: (cd ../cpp && make gen-data).
GEN=../cpp/build/gen-data
pq_dir=$(mktemp -d); trap 'rm -rf "$tmp" "$pq_dir"' EXIT

agree() { python3 -c '
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
shape = lambda r: (r["counts"], sorted(tuple(sorted(c.items())) for c in r["columns"]))
sys.exit(0 if shape(a) == shape(b) else 1)
' "$1" "$2"; }

same_report() { # label, key, then the generator flags for the parquet side
  local label=$1 key=$2; shift 2
  rm -f "$pq_dir"/p_*
  "$GEN" --rows 20k --out-dir "$pq_dir" --prefix p >/dev/null 2>&1
  "$GEN" --rows 20k --out-dir "$pq_dir" --prefix p "$@" >/dev/null 2>&1
  ./csvdiff compare "$pq_dir/p_a.csv" "$pq_dir/p_b.csv" -k "$key" -i updated_at \
      --json "$pq_dir/csv.json" >/dev/null 2>&1 || true
  ./csvdiff compare "$pq_dir/p_a.unc.parquet" "$pq_dir/p_b.unc.parquet" -k "$key" -i updated_at \
      --json "$pq_dir/pq.json" >/dev/null 2>&1 || true
  if agree "$pq_dir/csv.json" "$pq_dir/pq.json"; then
    printf '  ok    %s\n' "$label"
  else
    printf '  FAIL  %s\n' "$label"; fail=1
  fi
}

if [ -x "$GEN" ]; then
  echo "uncompressed parquet against the same rows as csv:"
  same_report "defaults"                    account_id,txn_id --format parquet --compression none
  same_report "many small row groups"       account_id,txn_id --format parquet --compression none --row-group-size 512
  # A dictionary budget the data crosses partway makes a column that is
  # dictionary encoded in some row groups and plain in others -- the shape a
  # real writer produces on a high-cardinality string, and the one the reader
  # has to fold into a single form.
  same_report "dictionary gives up partway" account_id,txn_id --format parquet --compression none --dict-limit 175 --row-group-size 300
  same_report "every column plain"          account_id,txn_id --format parquet --compression none --dict-limit 1
  # A key column both sides store as a dictionary takes the shared-id path,
  # where the join never looks at a byte after the dictionaries are mapped.
  same_report "dictionary key columns"      currency,status   --format parquet --compression none

  echo "refusals:"
  rm -f "$pq_dir"/r_*
  "$GEN" --rows 1k --out-dir "$pq_dir" --prefix r >/dev/null 2>&1
  "$GEN" --rows 1k --out-dir "$pq_dir" --prefix r --format parquet --compression none >/dev/null 2>&1
  "$GEN" --rows 1k --out-dir "$pq_dir" --prefix r --format parquet --compression snappy >/dev/null 2>&1
  # A column store and a byte stream have no common ground to be compared on.
  if ./csvdiff compare "$pq_dir/r_a.unc.parquet" "$pq_dir/r_b.csv" -k account_id 2>&1 \
       | grep -q "one file is parquet"; then
    echo "  ok    a mixed parquet/text pair is refused"
  else
    echo "  FAIL  mixed parquet/text pair"; fail=1
  fi
  # This port carries no decompressor on purpose; saying so is better than
  # producing a wrong answer out of bytes it did not understand.
  if ./csvdiff compare "$pq_dir/r_a.parquet" "$pq_dir/r_b.parquet" -k account_id 2>&1 \
       | grep -q "uncompressed parquet only"; then
    echo "  ok    snappy is refused by name"
  else
    echo "  FAIL  snappy should be refused by name"; fail=1
  fi
else
  echo "skip: $GEN is not built, so the parquet checks did not run"
  echo "      build it with: (cd ../cpp && make gen-data)"
  fail=1
fi

exit $fail
