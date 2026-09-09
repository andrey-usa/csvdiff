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
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
if [ "$with_ports" = 1 ]; then
  echo "quoting, ragged rows and keys near the end of the file:"
  printf 'a,k,c\nx,K1,c1\ny,K2,cc\n'        > "$tmp/a.csv"
  printf 'a,k,c\nx,K1,c1\ny,K2,cccccccc\n'  > "$tmp/b.csv"
  r=$("$RUST" compare "$tmp/a.csv" "$tmp/b.csv" -k k --engine turbo -o /dev/null 2>&1 | summary) || true
  c=$(./csvdiff compare "$tmp/a.csv" "$tmp/b.csv" -k k 2>&1 | summary) || true
  [ "$r" = "$c" ] && echo "  ok    key in the last bytes of the file" \
                  || { echo "  FAIL  key near end: rust=$r c=$c"; fail=1; }
fi



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

# Two rules the key-only JSON parse rests on, neither of which the generated
# fixture can exercise because it never produces either shape.
# `added` is derived from the A pass rather than counted by a pass of its own,
# on the argument that the join is symmetric. CSVDIFF_VERIFY_ADDED=1 runs the
# pass that was removed and refuses the run if the two disagree, so the argument
# is checked here on every shape that has ever broken an engine, not assumed.
echo "the derivation of added, checked against the pass it replaced:"
vcheck() { # label, then the compare arguments
  local label=$1; shift
  local out code=0
  # A compare exits 1 when it finds differences, which every case here does, so
  # the status has to be caught rather than reach `set -e`. Declaring and
  # assigning on one line would also hide it: the status would be `local`'s.
  out=$(CSVDIFF_VERIFY_ADDED=1 ./csvdiff compare "$@" 2>&1 >/dev/null) || code=$?
  if [ "$code" -le 1 ] && [ -z "$out" ]; then
    printf '  ok    %s\n' "$label"
  else
    printf '  FAIL  %s\n    %s\n' "$label" "${out:-exit $code}"; fail=1
  fi
}
vcheck "the awkward fixture" ../tests/fixtures/awkward_a.csv ../tests/fixtures/awkward_b.csv -k k
if [ -x "$GEN" ]; then
  vd=$(mktemp -d)
  "$GEN" --rows 20k --out-dir "$vd" --prefix v >/dev/null 2>&1
  "$GEN" --rows 20k --out-dir "$vd" --prefix v --format json >/dev/null 2>&1
  "$GEN" --rows 20k --out-dir "$vd" --prefix v --format parquet >/dev/null 2>&1
  vcheck "generated csv"     "$vd/v_a.csv"          "$vd/v_b.csv"          -k account_id,txn_id -i updated_at
  vcheck "generated ndjson"  "$vd/v_a.ndjson"       "$vd/v_b.ndjson"       -k account_id,txn_id -i updated_at
  vcheck "generated parquet" "$vd/v_a.unc.parquet"  "$vd/v_b.unc.parquet"  -k account_id,txn_id -i updated_at
  rm -rf "$vd"
else
  echo "  skip  the derivation checks need $GEN"; fail=1
fi

echo "the ndjson rules the fast path depends on:"
jd=$(mktemp -d)
BS=$(printf '\\')
# A row ends at the next newline byte, full stop -- valid JSON cannot carry a
# raw one inside a string, so an *escaped* newline must not split the row.
printf '{"k":"1","v":"a%snb"}\n{"k":"2","v":"plain"}\n' "$BS" > "$jd/a.ndjson"
printf '{"k":"1","v":"a%snb"}\n{"k":"2","v":"other"}\n' "$BS" > "$jd/b.ndjson"
jrun() { # file pair, expected "rows changed"
  rm -f "$jd/o.json"
  ./csvdiff compare "$jd/$1" "$jd/$2" -k k --json "$jd/o.json" >/dev/null 2>&1 || true
  python3 -c 'import json,sys; c=json.load(open(sys.argv[1]))["counts"]; print(c["a_rows"], c["changed"])' "$jd/o.json" 2>/dev/null || echo "(no report)"
}
got=$(jrun a.ndjson b.ndjson)
if [ "$got" = "2 1" ]; then
  echo "  ok    an escaped newline inside a string does not end the row"
else
  echo "  FAIL  an escaped newline inside a string does not end the row"
  echo "    want: 2 1"; echo "    got : $got"; fail=1
fi

# A repeated key column takes its *first* value, so that the key-only parse --
# which stops as soon as it has the keys -- and the full parse agree on what a
# row's key is. Both rows below key on 1 under that rule and on 2 under the
# other, so the counts say which rule ran.
printf '{"k":"1","k":"2","v":"x"}\n' > "$jd/dup_a.ndjson"
printf '{"k":"1","v":"y"}\n'         > "$jd/dup_b.ndjson"
got=$(jrun dup_a.ndjson dup_b.ndjson)
if [ "$got" = "1 1" ]; then
  echo "  ok    a repeated key column keeps its first value"
else
  echo "  FAIL  a repeated key column keeps its first value"
  echo "    want: 1 1 (matched on k=1, v differs)"; echo "    got : $got"; fail=1
fi
rm -rf "$jd"

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
# The sweep and the probes read only the key columns now, and the projection
# gets from a file column to its slot through a precomputed chain. Both make
# assumptions about where the key sits and which slots a column feeds, and
# neither is exercised by a fixture whose key is column zero.
echo "the key-only parse, where the key is not the first column:"
kdir=$(mktemp -d)
{
  echo 'x,y,id,z2'
  echo 'a1,b1,k1,z1'
  echo 'a2,b2,k2,z2'
  echo 'a3,b3,k3,z3'
} > "$kdir/a.csv"
{
  echo 'x,y,id,z2'
  echo 'a1,b1,k1,z1'
  echo 'a2,CHANGED,k2,z2'
  echo 'a9,b9,k9,z9'
} > "$kdir/b.csv"
kcase() { # label, expected "changed added removed", then the flags
  local label=$1 want=$2; shift 2
  # Removed first, and a missing one is a failure: the first version of this
  # helper read the previous case's report when a run refused its flags, and
  # reported the previous case's answer as this one's.
  rm -f "$kdir/o.json"
  ./csvdiff compare "$kdir/a.csv" "$kdir/b.csv" "$@" --json "$kdir/o.json" >/dev/null 2>&1 || true
  local got
  got=$(python3 -c 'import json,sys; c=json.load(open(sys.argv[1]))["counts"]; print(c["changed"], c["added"], c["removed"])' "$kdir/o.json" 2>/dev/null) || got="(no report)"
  if [ "$got" = "$want" ]; then
    printf '  ok    %s\n' "$label"
  else
    printf '  FAIL  %s\n    want: %s\n    got : %s\n' "$label" "$want" "$got"; fail=1
  fi
}
kcase "key in the third of four columns"      "1 1 1" -k id
kcase "with everything after the key ignored" "0 1 1" -k id -i x,y,z
kcase "key in the last column"                "1 1 1" -k z2
kcase "two key columns, first and last"       "1 1 1" -k x,z2
rm -rf "$kdir"

# The join proves a row unchanged from the two rows' raw bytes when they agree
# far enough in, and only parses the mate when they do not. These are the shapes
# where that proof has to hold or refuse: a change in the column it checks, a
# change hidden behind an ignored one, and two files that spell the same columns
# in a different order -- where equal bytes mean different columns.
echo "proving a row unchanged from the bytes:"
pdir=$(mktemp -d)
pcase() { # label, expected "changed added removed", a-file, b-file, then flags
  local label=$1 want=$2 af=$3 bf=$4; shift 4
  rm -f "$pdir/o.json"
  ./csvdiff compare "$pdir/$af" "$pdir/$bf" "$@" --json "$pdir/o.json" >/dev/null 2>&1 || true
  local got
  got=$(python3 -c 'import json,sys; c=json.load(open(sys.argv[1]))["counts"]; print(c["changed"], c["added"], c["removed"])' "$pdir/o.json" 2>/dev/null) || got="(no report)"
  if [ "$got" = "$want" ]; then
    printf '  ok    %s\n' "$label"
  else
    printf '  FAIL  %s\n    want: %s\n    got : %s\n' "$label" "$want" "$got"; fail=1
  fi
}

# One ignored column at the end that always differs -- the case the proof is
# for -- and a change in the last compared column, which is the one it checks.
{ echo 'id,a,b,ts'; echo 'k1,p,q,T1'; echo 'k2,p,q,T1'; } > "$pdir/tail_a.csv"
{ echo 'id,a,b,ts'; echo 'k1,p,q,T2'; echo 'k2,p,zz,T2'; } > "$pdir/tail_b.csv"
pcase "an ignored last column that always differs" "1 0 0" tail_a.csv tail_b.csv -k id -i ts
pcase "and no proof to be had once it is compared" "2 0 0" tail_a.csv tail_b.csv -k id

# A change in the first compared column, with everything after it identical.
{ echo 'id,a,b,ts'; echo 'k1,p,q,T1'; } > "$pdir/head_a.csv"
{ echo 'id,a,b,ts'; echo 'k1,X,q,T2'; } > "$pdir/head_b.csv"
pcase "a change in the first compared column" "1 0 0" head_a.csv head_b.csv -k id -i ts

# An ignored column in the middle that differs, and a real change after it: the
# bytes diverge early, so the proof must refuse and the change must be found.
{ echo 'id,skip,a,ts'; echo 'k1,S1,p,T1'; } > "$pdir/mid_a.csv"
{ echo 'id,skip,a,ts'; echo 'k1,S2,X,T2'; } > "$pdir/mid_b.csv"
pcase "a change behind an ignored middle column" "1 0 0" mid_a.csv mid_b.csv -k id -i skip,ts

# Identical bytes, different columns. The proof reads bytes, so this is the
# shape that breaks it if it is applied where the two headers disagree.
{ echo 'id,p,q'; echo 'k1,X,Y'; } > "$pdir/ord_a.csv"
{ echo 'id,q,p'; echo 'k1,X,Y'; } > "$pdir/ord_b.csv"
pcase "the same bytes under a different column order" "1 0 0" ord_a.csv ord_b.csv -k id

# Same value, spelled two ways: quoted on one side, bare on the other, and CRLF
# against LF. The bytes diverge, the values do not.
printf 'id,a,ts\nk1,"p",T1\n' > "$pdir/q_a.csv"
printf 'id,a,ts\r\nk1,p,T2\r\n' > "$pdir/q_b.csv"
pcase "quoted against bare, and CRLF against LF" "0 0 0" q_a.csv q_b.csv -k id -i ts

# A row that stops before the column the proof checks.
{ echo 'id,a,b,ts'; echo 'k1,p,q,T1'; } > "$pdir/rag_a.csv"
{ echo 'id,a,b,ts'; echo 'k1,p'; } > "$pdir/rag_b.csv"
pcase "a mate that stops before the checked column" "1 0 0" rag_a.csv rag_b.csv -k id -i ts

# The mate's value carries on where this one stopped. `cc` is a prefix of
# `cccccccc`, so a proof that reads only as far as this row's last byte calls
# them equal; the cross-port oracle caught exactly this.
{ echo 'a,k,c'; echo 'x,K1,c1'; echo 'y,K2,cc'; } > "$pdir/pre_a.csv"
{ echo 'a,k,c'; echo 'x,K1,c1'; echo 'y,K2,cccccccc'; } > "$pdir/pre_b.csv"
pcase "a mate whose last column carries on" "1 0 0" pre_a.csv pre_b.csv -k k

# The same trap behind quotes: the mate closes its field two bytes later,
# because what looks like the closing quote is a doubled one.
printf 'id,a,ts\nk1,"ab",T1\n'      > "$pdir/dq_a.csv"
printf 'id,a,ts\nk1,"ab""x",T2\n'   > "$pdir/dq_b.csv"
pcase "a mate that reopens on a doubled quote" "1 0 0" dq_a.csv dq_b.csv -k id -i ts

# The same proof on objects, where a value is found by name rather than by
# position -- and a name repeated in one object takes its *last* value for a
# compared column. A duplicate past the diverging byte is the shape that breaks
# it, so the mate's tail is checked for anything this run tracks.
echo "proving an object unchanged from the bytes:"
printf '{"id":"k1","a":"p","ts":"T1"}\n' > "$pdir/j_a.ndjson"
printf '{"id":"k1","a":"p","ts":"T2"}\n' > "$pdir/j_b.ndjson"
pcase "an ignored last field that always differs" "0 0 0" j_a.ndjson j_b.ndjson -k id -i ts

# The mate repeats the compared name after the bytes stop agreeing, and last
# wins -- so the value the prefix proved is not the value the row has.
printf '{"id":"k1","a":"p","ts":"T1"}\n'              > "$pdir/jd_a.ndjson"
printf '{"id":"k1","a":"p","ts":"T2","a":"LATER"}\n'  > "$pdir/jd_b.ndjson"
pcase "a mate that repeats the column past the prefix" "1 0 0" jd_a.ndjson jd_b.ndjson -k id -i ts

# The same duplicate, but inside the agreeing prefix: both objects read it the
# same way, so there is nothing to catch and the proof may stand.
printf '{"id":"k1","a":"p","a":"q","ts":"T1"}\n' > "$pdir/jp_a.ndjson"
printf '{"id":"k1","a":"p","a":"q","ts":"T2"}\n' > "$pdir/jp_b.ndjson"
pcase "a duplicate inside the prefix, agreeing"  "0 0 0" jp_a.ndjson jp_b.ndjson -k id -i ts

# Objects need not list their names in the same order. The bytes diverge at the
# first name, so the proof refuses and the values are compared properly.
printf '{"id":"k1","a":"p","b":"q","ts":"T1"}\n' > "$pdir/jo_a.ndjson"
printf '{"id":"k1","b":"q","a":"p","ts":"T2"}\n' > "$pdir/jo_b.ndjson"
pcase "the same values under a different name order" "0 0 0" jo_a.ndjson jo_b.ndjson -k id -i ts

# A change in the compared value itself, with the ignored field moving too.
printf '{"id":"k1","a":"p","ts":"T1"}\n' > "$pdir/jc_a.ndjson"
printf '{"id":"k1","a":"X","ts":"T2"}\n' > "$pdir/jc_b.ndjson"
pcase "a change in the compared value"          "1 0 0" jc_a.ndjson jc_b.ndjson -k id -i ts

# A repeated *key* name. First occurrence wins, so this row's key is `k1` and it
# matches. Under last-wins the key would be `OTHER`, and the row would come out
# as one added and one removed instead -- which is what the C++ port did until
# its key-only parse landed, on an input no fixture covered. Both ports are
# pinned to it now.
printf '{"id":"k1","a":"p"}\n'              > "$pdir/jk_a.ndjson"
printf '{"id":"k1","id":"OTHER","a":"p"}\n' > "$pdir/jk_b.ndjson"
pcase "a repeated key name keeps the first"     "0 0 0" jk_a.ndjson jk_b.ndjson -k id
rm -rf "$pdir"

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
  # The generator formats rows on every core, in waves, so these are the shapes
  # where a threaded writer differs from a serial one: a thread count that does
  # not divide the wave, a row count that ends inside one, and one row.
  gen_case "csv, one thread"        --rows 20k --threads 1
  gen_case "csv, three threads"     --rows 20k --threads 3
  gen_case "csv, seven threads"     --rows 20k --threads 7
  gen_case "csv, ending mid-wave"   --rows 8193
  gen_case "csv, a single row"      --rows 1
  gen_case "ndjson, three threads"  --rows 20k --format json --threads 3
  gen_case "parquet"                --rows 5k --format parquet --compression none
  gen_case "parquet, small groups"  --rows 5k --format parquet --compression none --row-group-size 512
  gen_case "parquet, dict gives up" --rows 5k --format parquet --compression none --dict-limit 175 --row-group-size 300
  gen_case "parquet, all plain"     --rows 5k --format parquet --compression none --dict-limit 1
  gen_case "a different seed"       --rows 2k --seed 42
  rm -rf "$gen_dir"
fi

exit $fail
