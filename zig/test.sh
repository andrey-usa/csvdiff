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

exit $fail
