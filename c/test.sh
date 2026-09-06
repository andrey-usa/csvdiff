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

exit $fail
