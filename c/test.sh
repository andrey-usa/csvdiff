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
GEN=./gen-data
CPP=../cpp/build/csvdiff

# Two sets of checks, because they cost very different amounts to run.
#
# By default only the ones this port can answer on its own: it generates its own
# fixtures now, so nothing else has to be built first and the whole suite is a
# few seconds. `--with-ports` adds the cross-port checks -- the Rust port as the
# oracle on CSV, the C++ port on ndjson, and the two generators' bytes against
# each other -- which need those toolchains and are worth a job of their own.
with_ports=0
[ "${1:-}" = "--with-ports" ] && with_ports=1
if [ "$with_ports" = 1 ]; then
  [ -x "$RUST" ] || { echo "--with-ports needs the Rust port: (cd ../rust && cargo build --release)"; exit 2; }
  [ -x "$CPP" ] || { echo "--with-ports needs the C++ port: (cd ../cpp && make && make gen-data)"; exit 2; }
fi

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

if [ "$with_ports" = 1 ]; then
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
fi   # with_ports

agree() { python3 -c '
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
shape = lambda r: (r["counts"], sorted(tuple(sorted(c.items())) for c in r["columns"]))
sys.exit(0 if shape(a) == shape(b) else 1)
' "$1" "$2"; }

echo "the threaded csv sweep, over quoted newlines and chunk boundaries:"
# --- the threaded CSV sweep -------------------------------------------------
#
# Rows are found on every core, which means a thread starts in the middle of the
# file and has to work out whether it landed inside a quoted field. Nothing in
# the fixtures above reaches the size where that splitting turns on, so this
# builds a file that does, out of the shapes that make the boundary hard:
# newlines inside quotes, doubled quotes, delimiters inside quotes, CRLF, blank
# lines between rows, duplicate keys, and rows present on one side only.
#
# The check is that the thread count cannot change the answer -- against itself
# at one thread, and against the Rust port.
thr_dir=$(mktemp -d)
python3 - "$thr_dir" <<'PY'
import random, sys
out_dir = sys.argv[1]
random.seed(3)
FIELDS = ["plain", '"has,comma"', '"has\nnewline\nand more"',
          '"doubled ""quote"" inside"', '"comma, and\nnewline together"', '""']
def rows(side):
    out = ["k,a,b,c\n"]
    for i in range(130000):
        if side == "b" and i % 5000 == 0:
            continue                                  # removed from B
        a = FIELDS[(i * 7 + (3 if side == "b" and i % 41 == 0 else 0)) % len(FIELDS)]
        b = str(i * 3 + (1 if side == "b" and i % 23 == 0 else 0))
        c = '"tail\nwith newline"' if i % 11 == 0 else "tail"
        sep = "\r\n" if i % 13 == 0 else "\n"
        out.append(f"K{i:07d},{a},{b},{c}{sep}")
        if i % 997 == 0:
            out.append(f"K{i:07d},{a},{b},{c}{sep}")   # a duplicate key
        if i % 1499 == 0:
            out.append("\n")                           # a blank line is not a row
    if side == "b":
        for i in range(40):
            out.append(f"NEW{i:05d},plain,0,tail\n")   # added in B
    return "".join(out)
for side in ("a", "b"):
    open(f"{out_dir}/t_{side}.csv", "w").write(rows(side))
PY
bytes=$(wc -c < "$thr_dir/t_a.csv")
if [ "$bytes" -lt 4194304 ]; then
  echo "  FAIL  the threading fixture is $bytes bytes, under the 4 MB split threshold"
  fail=1
else
  ./csvdiff compare "$thr_dir/t_a.csv" "$thr_dir/t_b.csv" -k k --threads 1 \
      --json "$thr_dir/one.json" >/dev/null 2>&1 || true
  for t in 2 3 4 7; do
    ./csvdiff compare "$thr_dir/t_a.csv" "$thr_dir/t_b.csv" -k k --threads $t \
        --json "$thr_dir/many.json" >/dev/null 2>&1 || true
    if agree "$thr_dir/one.json" "$thr_dir/many.json"; then
      printf '  ok    %s threads finds what 1 thread finds\n' "$t"
    else
      printf '  FAIL  %s threads disagrees with 1 thread\n' "$t"; fail=1
    fi
  done
  if [ "$with_ports" = 1 ]; then
    "$RUST" compare "$thr_dir/t_a.csv" "$thr_dir/t_b.csv" -k k --engine turbo -o /dev/null \
        --json "$thr_dir/rust.json" >/dev/null 2>&1 || true
    if agree "$thr_dir/one.json" "$thr_dir/rust.json"; then
      echo "  ok    and what the rust port finds"
    else
      echo "  FAIL  the threaded sweep disagrees with the rust port"; fail=1
    fi
  fi
fi
rm -rf "$thr_dir"
echo "quoting, ragged rows and keys near the end of the file:"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
printf 'a,k,c\nx,K1,c1\ny,K2,cc\n'        > "$tmp/a.csv"
printf 'a,k,c\nx,K1,c1\ny,K2,cccccccc\n'  > "$tmp/b.csv"
r=$("$RUST" compare "$tmp/a.csv" "$tmp/b.csv" -k k --engine turbo -o /dev/null 2>&1 | summary) || true
c=$(./csvdiff compare "$tmp/a.csv" "$tmp/b.csv" -k k 2>&1 | summary) || true
[ "$r" = "$c" ] && echo "  ok    key in the last bytes of the file" || { echo "  FAIL  key near end: rust=$r c=$c"; fail=1; }



# --- newline-delimited JSON -------------------------------------------------
#
# The same rows in both shapes have to give the same answer, which is the whole
# claim: a JSON reader that disagreed with the CSV one would be a second engine,
# not a second parser.
echo "newline-delimited json:"
if [ -x "$GEN" ]; then
  js_dir=$(mktemp -d)
  "$GEN" --rows 20k --out-dir "$js_dir" --prefix j >/dev/null 2>&1
  "$GEN" --rows 20k --out-dir "$js_dir" --prefix j --format json >/dev/null 2>&1
  ./csvdiff compare "$js_dir/j_a.csv" "$js_dir/j_b.csv" -k account_id,txn_id -i updated_at \
      --json "$js_dir/csv.json" >/dev/null 2>&1 || true
  ./csvdiff compare "$js_dir/j_a.ndjson" "$js_dir/j_b.ndjson" -k account_id,txn_id -i updated_at \
      --json "$js_dir/js.json" >/dev/null 2>&1 || true
  if agree "$js_dir/csv.json" "$js_dir/js.json"; then
    echo "  ok    ndjson finds what the same rows as csv find"
  else
    echo "  FAIL  ndjson disagrees with csv on the same rows"; fail=1
  fi
  # Against C++ rather than Rust here: it is the only other port that reads
  # ndjson at all, and a check against a port that refuses the file would be a
  # check that always passes.
  if [ "$with_ports" = 1 ]; then
    "$CPP" compare "$js_dir/j_a.ndjson" "$js_dir/j_b.ndjson" -k account_id,txn_id \
        -i updated_at --json "$js_dir/cpp.json" >/dev/null 2>&1 || true
    if agree "$js_dir/js.json" "$js_dir/cpp.json"; then
      echo "  ok    and what the c++ port finds on the same ndjson"
    else
      echo "  FAIL  ndjson disagrees with the c++ port"; fail=1
    fi
  fi
  rm -rf "$js_dir"
else
  echo "  skip  ndjson against csv needs $GEN"; fail=1
fi

# A JSON writer may escape a character or write it literally, and the two spell
# the same value -- so they have to compare equal. This is the one place the two
# dialects genuinely differ: CSV doubles a quote, JSON puts a backslash in front,
# and \uXXXX has to become UTF-8 before anything is compared.
esc_dir=$(mktemp -d)
python3 - "$esc_dir" <<'PY'
import json, sys
out = sys.argv[1]
BS = chr(92)                      # kept out of the literals below
pairs = [
    ("k1", BS + "u00e9" + BS + "u00e8", "éè"),   # two-byte utf-8
    ("k2", BS + "ud83d" + BS + "ude00", "\U0001F600"),      # a surrogate pair is one code point
    ("k3", "a" + BS + "/b",             "a/b"),             # the optional solidus escape
    ("k4", "tab" + BS + "there",        "tab\there"),
    ("k5", BS + "u0041BC",              "ABC"),             # ascii written as an escape
    ("k6", "quote" + BS + '"in',        'quote"in'),
    ("k7", "back" + BS + BS + "slash",  "back" + BS + "slash"),
    ("k8", BS + "u4e2d" + BS + "u6587", "中文"),    # three-byte utf-8
    ("k9", "line" + BS + "nbreak",      "line\nbreak"),
]
with open(f"{out}/a.ndjson", "w") as fa, open(f"{out}/b.ndjson", "w") as fb:
    for k, escaped, literal in pairs:
        fa.write('{"k":"%s","v":"%s"}\n' % (k, escaped))
        fb.write(json.dumps({"k": k, "v": literal}, ensure_ascii=False) + "\n")
# And a control: one value that really does differ, so the check above is known
# to be capable of failing.
open(f"{out}/c.ndjson", "w").write(
    open(f"{out}/b.ndjson").read().replace('"éè"', '"éé"'))
PY
# `set -e` is on and a compare exits 1 when it finds differences, so the status
# has to be caught rather than left to reach the shell -- which would end the
# run here and call the negative control below a crash.
code=0; ./csvdiff compare "$esc_dir/a.ndjson" "$esc_dir/b.ndjson" -k k >/dev/null 2>&1 || code=$?
if [ "$code" -eq 0 ]; then
  echo "  ok    an escaped value equals the same value written literally"
else
  echo "  FAIL  escaped and literal spellings of one value compare different"; fail=1
fi
code=0; ./csvdiff compare "$esc_dir/a.ndjson" "$esc_dir/c.ndjson" -k k >/dev/null 2>&1 || code=$?
if [ "$code" -eq 1 ]; then
  echo "  ok    and a value that really differs is still found"
else
  echo "  FAIL  the escape check cannot fail, so it proves nothing"; fail=1
fi
rm -rf "$esc_dir"
# --- the Parquet path -------------------------------------------------------
#
# The claim the columnar path has to earn is that it is the *same* comparison:
# the same rows, in a format that stores them as columns of dictionary indices
# rather than as lines of text, must produce the same counts and the same
# per-column changed/blanked/filled. So each case generates one pair in both
# formats and requires the two reports to agree in full.
#
# The fixtures come from this port's own generator, which writes CSV, ndjson and
# Parquet from one field-by-field recipe -- byte for byte the same files the C++
# generator writes, which is checked below. Where it is not built these skip by
# name rather than silently passing.
pq_dir=$(mktemp -d); trap 'rm -rf "$tmp" "$pq_dir"' EXIT

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
  # A column store and a byte stream have no common ground to be compared on.
  if ./csvdiff compare "$pq_dir/r_a.unc.parquet" "$pq_dir/r_b.csv" -k account_id 2>&1 \
       | grep -q "one file is parquet"; then
    echo "  ok    a mixed parquet/text pair is refused"
  else
    echo "  FAIL  mixed parquet/text pair"; fail=1
  fi
  # This port carries no decompressor on purpose; saying so is better than
  # producing a wrong answer out of bytes it did not understand.
  # This port writes no snappy, so the file that proves it refuses one is
  # checked in rather than generated.
  if ./csvdiff compare ../tests/fixtures/snappy.parquet ../tests/fixtures/snappy.parquet \
       -k account_id 2>&1 | grep -q "uncompressed parquet only"; then
    echo "  ok    snappy is refused by name"
  else
    echo "  FAIL  snappy should be refused by name"; fail=1
  fi
else
  echo "skip: $GEN is not built, so the parquet checks did not run"
  echo "      it is built by 'make' in this directory"
  fail=1
fi

# --- the generator against the C++ one --------------------------------------
#
# The fast half of this suite makes its own fixtures, which is only safe because
# the bytes are the same bytes. A generator that drifted would not fail any
# check above -- both sides of every comparison would drift together -- so the
# drift has to be caught here, against the generator this one replaced.
if [ "$with_ports" = 1 ]; then
  echo "generator bytes, c against c++:"
  gen_dir=$(mktemp -d)
  gen_case() { # label, then the flags both generators are given
    local label=$1; shift
    rm -rf "$gen_dir/c" "$gen_dir/x"; mkdir -p "$gen_dir/c" "$gen_dir/x"
    "$GEN" --out-dir "$gen_dir/c" --prefix g "$@" >/dev/null 2>&1
    ../cpp/build/gen-data --out-dir "$gen_dir/x" --prefix g "$@" >/dev/null 2>&1
    local same=1 any=0
    for f in $(cd "$gen_dir/x" && ls); do
      any=1
      cmp -s "$gen_dir/c/$f" "$gen_dir/x/$f" || same=0
    done
    if [ "$any" = 1 ] && [ "$same" = 1 ]; then
      printf '  ok    %s\n' "$label"
    else
      printf '  FAIL  %s\n' "$label"; fail=1
    fi
  }
  gen_case "csv"                    --rows 5k
  gen_case "ndjson"                 --rows 5k --format json
  gen_case "parquet"                --rows 5k --format parquet --compression none
  gen_case "parquet, small groups"  --rows 5k --format parquet --compression none --row-group-size 512
  gen_case "parquet, dict gives up" --rows 5k --format parquet --compression none --dict-limit 175 --row-group-size 300
  gen_case "parquet, all plain"     --rows 5k --format parquet --compression none --dict-limit 1
  gen_case "a different seed"       --rows 2k --seed 42
  rm -rf "$gen_dir"
fi

exit $fail
