#!/usr/bin/env bash
#
# Two builds of one engine, on one pair of files, interleaved.
#
# This is the inner loop of working on a port: you changed something, and you
# want to know whether it paid. `scripts/bench_ports.py` answers the other
# question -- every port against every other, with peak RSS and the counts gate
# -- and is what the published tables come from.
#
# Three things it is careful about, each of which produced a wrong answer first.
#
# **Interleaved, not one build at a time.** Running all of A and then all of B
# is how hyperfine and most harnesses work, and on a shared 4-vCPU runner it
# roughly doubles the error: two copies of the *same* binary measured that way
# came out 15.8% apart at worst, against 8.3% interleaved. Every build runs once
# per round, and the rounds are what repeat.
#
# **CPU, not wall.** Wall time mixes work with how many cores a design manages
# to use, and it is the noisier of the two by a wide margin -- see `--self-test`,
# which measures both by running one build against itself.
#
# **Paired, and a verdict rather than a bare ratio.** Comparing the two builds'
# separately-taken bests puts the machine's drift into the answer: two copies of
# one binary came out 9.1% apart that way, and fifteen rounds instead of five
# moved that to 9.4% -- because drift is not jitter and averaging does not touch
# it. Comparing a_r against b_r *within* each round cancels it: the same A/A
# test then reports 1.01x. So the number to read is the median of the per-round
# ratios, and the middle half of them is printed beside it. If that half
# straddles 1.00x the two builds were not told apart, and the script says so
# instead of leaving a ratio to be argued about. A day went into a 5%
# "regression" here that this would have called immediately.
#
#   scripts/bench_ab.sh old/csvdiff new/csvdiff -- compare a.csv b.csv -k id
#   scripts/bench_ab.sh --self-test c/csvdiff   -- compare a.csv b.csv -k id
#
set -uo pipefail

rounds=7
warmup=1
self_test=0
labels=()
bins=()

while [ $# -gt 0 ]; do
  case "$1" in
    --rounds)    rounds=$2; shift 2 ;;
    --warmup)    warmup=$2; shift 2 ;;
    --self-test) self_test=1; shift ;;
    --)          shift; break ;;
    -*)          echo "unknown option $1" >&2; exit 2 ;;
    *)           bins+=("$1"); shift ;;
  esac
done
args=("$@")

if [ "$self_test" = 1 ]; then
  [ "${#bins[@]}" = 1 ] || { echo "--self-test takes one build" >&2; exit 2; }
  # The same file under two names: whatever difference is reported is the
  # harness measuring the machine rather than the code.
  bins=("${bins[0]}" "${bins[0]}")
  labels=("copy A" "copy B")
else
  [ "${#bins[@]}" = 2 ] || { echo "usage: bench_ab.sh OLD NEW -- <arguments>" >&2; exit 2; }
  labels=("${bins[0]}" "${bins[1]}")
fi
[ "${#args[@]}" -gt 0 ] || { echo "no arguments to measure; put them after --" >&2; exit 2; }

for b in "${bins[@]}"; do
  [ -x "$b" ] || { echo "not executable: $b" >&2; exit 2; }
done

TIMEFORMAT='%R %U %S'

# Wall, then CPU, for one run. The shell's own `time` reports both, so nothing
# outside the shell is needed -- peak RSS is the one number it cannot give, and
# the reason bench_ports.py reaches for wait4.
one() {
  local out
  out=$( { time "$@" >/dev/null 2>&1; } 2>&1 ) || true
  awk '{printf "%.4f %.4f\n", $1, $2 + $3}' <<<"$out"
}

# Warm the page cache and let the first run pay for whatever a first run pays
# for; these are thrown away.
for ((w = 0; w < warmup; w++)); do
  for i in 0 1; do one "${bins[$i]}" "${args[@]}" >/dev/null; done
done

# Both builds inside one round, and the round is what is compared. The noise
# here is drift, not jitter: three times the rounds moved the same-build-twice
# error from 9.1% to 9.4%, so averaging more of it does nothing. What does work
# is pairing -- a_r and b_r ran seconds apart under one machine state, so their
# ratio carries far less of the drift than a ratio of two separately-taken
# bests. The median of the per-round ratios is the number to read.
wall=("" ""); cpu=("" ""); pair_w=""; pair_c=""
for ((r = 0; r < rounds; r++)); do
  aw=(); ac=()
  for i in 0 1; do
    read -r a b < <(one "${bins[$i]}" "${args[@]}")
    wall[$i]+="$a"$'\n'; cpu[$i]+="$b"$'\n'
    aw+=("$a"); ac+=("$b")
  done
  pair_w+="$(awk -v x="${aw[0]}" -v y="${aw[1]}" 'BEGIN{printf "%.5f", x/y}')"$'\n'
  pair_c+="$(awk -v x="${ac[0]}" -v y="${ac[1]}" 'BEGIN{printf "%.5f", x/y}')"$'\n'
done

stats() { sort -g | awk '{v[n++]=$1} END{printf "%.3f %.3f", v[0], v[int(n/2)]}'; }
# Median of the paired ratios, and the range the middle half of them sits in.
# If that range straddles 1.00 the two builds were not told apart.
paired() { sort -g | awk '{v[n++]=$1}
  END{printf "%.3f %.3f %.3f", v[int(n/2)], v[int(n/4)], v[int(3*n/4)]}'; }

read -r w0b w0m < <(printf '%s' "${wall[0]}" | stats)
read -r w1b w1m < <(printf '%s' "${wall[1]}" | stats)
read -r c0b c0m < <(printf '%s' "${cpu[0]}"  | stats)
read -r c1b c1m < <(printf '%s' "${cpu[1]}"  | stats)

printf '\n%-34s %9s %9s   %9s %9s\n' "" "wall best" "median" "cpu best" "median"
printf '%-34s %8.3fs %8.3fs   %8.2fs %8.2fs\n' "${labels[0]}" "$w0b" "$w0m" "$c0b" "$c0m"
printf '%-34s %8.3fs %8.3fs   %8.2fs %8.2fs\n' "${labels[1]}" "$w1b" "$w1m" "$c1b" "$c1m"

read -r rw rwlo rwhi < <(printf '%s' "$pair_w" | paired)
read -r rc rclo rchi < <(printf '%s' "$pair_c" | paired)

printf '\n%-34s %9s %9s   %9s %9s\n' "paired ratio (old / new)" "wall" "[mid half]" "cpu" "[mid half]"
printf '%-34s %8.2fx %4.2f-%.2f   %8.2fx %4.2f-%.2f\n' "" "$rw" "$rwlo" "$rwhi" "$rc" "$rclo" "$rchi"

awk -v rc="$rc" -v lo="$rclo" -v hi="$rchi" -v self="$self_test" -v n="$rounds" 'BEGIN{
  told_apart = (lo > 1.0 || hi < 1.0)
  if (self) {
    printf "\nthe same build twice, %d interleaved rounds. A harness that works says 1.00x\n", n
    printf "and a middle half straddling it; anything else is this machine, not code.\n"
    exit
  }
  if (!told_apart) {
    printf "\nno result: the middle half of the rounds straddles 1.00x. The two builds\n"
    printf "were not told apart -- run more rounds, or the difference is not there.\n"
  } else if (rc > 1)
    printf "\nthe new build does %.0f%% less work, in every quarter of the rounds\n", 100*(1 - 1/rc)
  else
    printf "\nthe new build does %.0f%% MORE work, in every quarter of the rounds\n", 100*(1/rc - 1)
}'
