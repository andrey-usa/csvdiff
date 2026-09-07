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

exit $fail
