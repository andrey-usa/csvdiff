#!/usr/bin/env bash
# Binary-searches the smallest memory limit a comparison finishes in.
#
# Peak RSS says what a process used with room to spare, which for an engine that
# maps its input says almost nothing: mapped pages are reclaimable, so the
# figure is whatever the kernel let it keep, not what it needed. This runs the
# comparison inside a memory cgroup and lowers the limit until it is killed.
#
#   scripts/memory_floor.sh cpp/build/csvdiff compare a.csv b.csv -k id
#
# Takes the whole command, so it works on any of the ports.
#
# Needs root and cgroup v1 with the memory controller (/sys/fs/cgroup/memory).
# Each probe gets a fresh cgroup: reusing one silently fails to lower the limit
# once pages are charged to it, which reads as a pass when nothing was limited.
set -o pipefail
set +m          # the OOM kill is the measurement, not an incident to announce

ROOT=/sys/fs/cgroup/memory
[ -d "$ROOT" ] || { echo "no cgroup v1 memory controller at $ROOT"; exit 2; }
[ $# -ge 3 ] || { echo "usage: $0 COMMAND [args...]   (the full compare command)"; exit 2; }

HI=${CSVDIFF_FLOOR_HI:-8192}   # a limit that is expected to pass
LO=${CSVDIFF_FLOOR_LO:-64}     # a limit that is expected to fail

CMD=("$@")

probe() { # $1 = limit in MB; runs CMD inside a fresh cgroup at that limit
  local mb=$1
  local cg="$ROOT/floor_$$_$mb"
  local rc
  mkdir -p "$cg" 2>/dev/null || return 2
  if ! echo $((mb * 1024 * 1024)) > "$cg/memory.limit_in_bytes" 2>/dev/null; then
    rmdir "$cg" 2>/dev/null
    return 2
  fi
  echo $((mb * 1024 * 1024)) > "$cg/memory.memsw.limit_in_bytes" 2>/dev/null
  ( echo $BASHPID > "$cg/cgroup.procs"; exec "${CMD[@]}" ) >/dev/null 2>&1
  rc=$?
  rmdir "$cg" 2>/dev/null
  # 0 identical, 1 differences: both mean it ran. 137 is the OOM kill.
  [ "$rc" = 0 ] || [ "$rc" = 1 ]
}

printf 'searching between %s and %s MB\n' "$LO" "$HI"
if ! probe "$HI"; then
  echo "did not finish even at ${HI} MB; raise CSVDIFF_FLOOR_HI"; exit 1
fi
while [ $((HI - LO)) -gt 32 ]; do
  MID=$(((HI + LO) / 2))
  if probe "$MID"; then
    printf '  %5s MB  completed\n' "$MID"; HI=$MID
  else
    printf '  %5s MB  killed\n' "$MID"; LO=$MID
  fi
done
printf '\nsmallest limit that finishes: about %s MB\n' "$HI"
