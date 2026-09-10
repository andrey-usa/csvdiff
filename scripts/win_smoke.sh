#!/usr/bin/env bash
# Does a Windows build give the same answer as the Rust port?
#
# The three POSIX ports each have a suite (c/test.sh, cpp/test.sh, zig/test.sh)
# and those are the real coverage. They are written for a POSIX shell and reach
# for binaries by their extensionless names, so they do not run on Windows yet.
# This is the part that can: the same oracle those suites use -- the Rust port,
# which has built and been tested on Windows for some time -- asked the same
# questions about the same files.
#
# It checks the two things a Windows build gets wrong that a Linux one cannot:
#
#   the generator's bytes, because the Microsoft runtime turns a lone \n into
#   \r\n on a text-mode handle, and a generator that does that has quietly
#   stopped writing the same files as the other five;
#
#   a comparison over CSV, ndjson and Parquet, because reading is done through a
#   file mapping, and on Windows that is a section object and a view of it
#   rather than mmap -- a different call with a different failure mode.
#
# Run from anywhere:  scripts/win_smoke.sh c|cpp|zig
# EXE is the binary suffix, `.exe` here and empty if you want to try it on Linux.
set -uo pipefail

port=${1:?usage: win_smoke.sh c|cpp|zig}
cd "$(dirname "$0")/.."
EXE=${EXE-.exe}

RUST=rust/target/release/csvdiff$EXE
RGEN=rust/target/release/gen-data$EXE
case "$port" in
  c)   BIN=c/csvdiff$EXE;               GEN=c/gen-data$EXE ;;
  cpp) BIN=cpp/build/csvdiff$EXE;       GEN=cpp/build/gen-data$EXE ;;
  # The Zig port ships no generator, so it is checked against files the Rust one
  # wrote -- which also means there are no bytes of its own to compare.
  zig) BIN=zig/zig-out/bin/csvdiff$EXE; GEN=$RGEN ;;
  *)   echo "unknown port: $port"; exit 2 ;;
esac

for f in "$BIN" "$GEN" "$RUST" "$RGEN"; do
  [ -x "$f" ] || { echo "not built: $f"; exit 2; }
done

fail=0
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The engine label and the time after it differ by port and by run, and the Rust
# port groups its digits where the others do not. Same normalisation the three
# suites use.
summary() { head -1 | sed 's/ | \(turbo\|parquet\).*//; s/,//g'; }

echo "generator bytes, $port against rust:"
if [ "$GEN" = "$RGEN" ]; then
  echo "  skip  this port has no generator of its own"
else
  # CSV only, and the two exclusions are worth writing down.
  #
  # Parquet, because the ports spell its compression defaults differently and so
  # write differently-named files: a byte comparison there is a question about
  # flags rather than about the platform. Nothing is lost -- a Parquet file with
  # \r\n injected into it cannot be read back, so the answers below catch it.
  #
  # ndjson, because the C and C++ generators already disagree with the Rust one
  # about it, on Linux, today: where a row has no `value_date` they write JSON
  # `null` and Rust writes `""`. The readers treat both as absent so every answer
  # matches, which is why nothing has caught it -- the only cross-generator byte
  # check in the repository is c/test.sh's, and that compares C against C++,
  # which agree. It is a real divergence from the byte-identical claim and it is
  # not this script's to settle; including it here would only report the same
  # known difference on every Windows run.
  #
  # CSV is where the hazard actually lives anyway: \r\n translation happens on a
  # text-mode handle, and that is the file written through one.
  for fmt in csv; do
    mine=$tmp/mine-$fmt theirs=$tmp/theirs-$fmt
    mkdir -p "$mine" "$theirs"
    "$GEN"  --rows 20k --out-dir "$mine"   --prefix g --format "$fmt" >/dev/null 2>&1
    "$RGEN" --rows 20k --out-dir "$theirs" --prefix g --format "$fmt" >/dev/null 2>&1
    same=1 any=0
    for name in $(cd "$theirs" && ls); do
      any=1
      cmp -s "$mine/$name" "$theirs/$name" || same=0
    done
    if [ "$any" = 1 ] && [ "$same" = 1 ]; then
      echo "  ok    $fmt"
    else
      # A \r\n translation shows up as a length difference and nothing else, so
      # say the sizes: it is the whole diagnosis.
      echo "  FAIL  $fmt is not byte-identical"
      for name in $(cd "$theirs" && ls); do
        printf '        %-24s mine %s  rust %s\n' "$name" \
               "$(wc -c <"$mine/$name" 2>/dev/null)" "$(wc -c <"$theirs/$name")"
      done
      fail=1
    fi
  done
fi

echo "answers, $port against rust:"
data=$tmp/data
mkdir -p "$data"
for fmt in csv json parquet; do
  "$RGEN" --rows 20k --out-dir "$data" --prefix "$fmt" --format "$fmt" >/dev/null 2>&1
done
compare() { # label, then the two files and the flags both ports are given
  local label=$1; shift
  local r m
  r=$("$RUST" compare "$@" --engine turbo -o /dev/null 2>&1 | summary) || true
  m=$("$BIN"  compare "$@"                                 2>&1 | summary) || true
  if [ -n "$r" ] && [ "$r" = "$m" ]; then
    printf '  ok    %s: %s\n' "$label" "$m"
  else
    printf '  FAIL  %s\n    rust: %s\n    %-4s: %s\n' "$label" "$r" "$port" "$m"; fail=1
  fi
}
compare csv    "$data/csv_a.csv"        "$data/csv_b.csv"        -k account_id,txn_id -i updated_at
compare ndjson "$data/json_a.ndjson"    "$data/json_b.ndjson"    -k account_id,txn_id -i updated_at
compare parquet "$data/parquet_a.parquet" "$data/parquet_b.parquet" -k account_id,txn_id -i updated_at

# Threads are the other thing that is a different call on Windows -- CreateThread
# in the C port, std::thread in the C++ one -- and a comparison that splits and
# rejoins wrongly gives a wrong count rather than a crash.
compare "csv on one thread"   "$data/csv_a.csv" "$data/csv_b.csv" -k account_id,txn_id -i updated_at --threads 1
compare "csv on four threads" "$data/csv_a.csv" "$data/csv_b.csv" -k account_id,txn_id -i updated_at --threads 4

[ "$fail" = 0 ] && echo "all good" || echo "there were failures"
exit "$fail"
