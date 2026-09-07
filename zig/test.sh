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
# agree on, with the thousands separators and the timing stripped.
answer() { sed 's/ | turbo.*//; s/,//g' ; }

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

out=$($BIN compare "$tmp/small_a.csv" "$tmp/small_b.csv" -k k --max-memory 64 2>&1 | head -1)
case "$out" in
  *"matched 20000"*) echo "  ok    a budget that is enough finishes inside it" ;;
  *) echo "  FAIL  expected a comparison, got: $out"; fail=1 ;;
esac

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

exit $fail
