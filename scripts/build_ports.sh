#!/usr/bin/env bash
#
# Build named port targets at the same time instead of one after another.
#
# Every workflow that needs more than one port was building them serially in a
# single `run:` block, which on a four-core runner leaves three cores idle
# through most of it: a single-file `clang++` compile is one core, and three of
# those fit beside a `cargo build` that is already using what it can. Measured
# on a four-core box, `cpp`, `cpp-scanners` and `zig` together took 118.5s in a
# row and 54.0s at once -- 2.19x, for work that was always independent.
#
# Concurrency defaults to the core count rather than "all of them at once".
# Seven concurrent C++ and Rust compiles is several gigabytes of resident
# compiler, and a runner that swaps would give the time back with interest.
#
#   scripts/build_ports.sh rust zig cpp cpp-gen
#   BUILD_JOBS=2 scripts/build_ports.sh c cpp
#
# `CXX`, `CC` and `RUSTFLAGS` are read from the environment, so a caller that
# wants clang or a target feature sets it the way it always did.
#
# Each target's output is kept and printed under its own heading when it
# finishes, because interleaved compiler errors from four builds at once are
# not readable. The exit status is non-zero if any target failed, and every
# target is run even when an earlier one fails -- one run should name all the
# broken builds, not just the first.

set -uo pipefail

command_for() {
    case "$1" in
        c)            echo 'make -C c' ;;
        c-gen)        echo 'make -C c gen-data' ;;
        cpp)          echo 'make -C cpp' ;;
        cpp-gen)      echo 'make -C cpp gen-data' ;;
        # Not the `scanners` target: that also builds `build/csvdiff-avx512`,
        # which does not start on a runner without AVX-512 -- the caller asks
        # for `cpp-avx512` separately when the host has the instructions.
        cpp-scanners) echo 'make -C cpp build/csvdiff-swar build/csvdiff-avx2' ;;
        cpp-avx512)   echo 'make -C cpp build/csvdiff-avx512' ;;
        rust)         echo 'cd rust && cargo build --release --no-default-features' ;;
        # With DuckDB and polars, for the workflows that still ask for them.
        rust-full)    echo 'cd rust && cargo build --release' ;;
        # A target feature is not a source switch, so this needs its own target
        # directory; cargo locks per directory, so it runs beside the first.
        rust-avx2)    echo 'cd rust && RUSTFLAGS="-C target-feature=+avx2" cargo build --release --no-default-features --target-dir target-avx2' ;;
        zig)          echo 'cd zig && zig build --release=fast' ;;
        zig-native)   echo 'cd zig && zig build --release=fast -Dcpu=native' ;;
        # `-Dcpu=native` is not optional on these two: at baseline x86-64 a
        # @Vector(32, u8) is lowered to scalar code and the row would be
        # measuring a slower SWAR rather than the instruction set it names.
        zig-v32)      echo 'cd zig && zig build --release=fast -Dscan=32 -Dcpu=native --prefix zig-out-v32' ;;
        zig-v64)      echo 'cd zig && zig build --release=fast -Dscan=64 -Dcpu=native --prefix zig-out-v64' ;;
        *)            return 1 ;;
    esac
}

if [ "$#" -eq 0 ]; then
    echo "usage: $0 TARGET [TARGET...]" >&2
    echo "targets: c c-gen cpp cpp-gen cpp-scanners cpp-avx512 rust rust-full" >&2
    echo "         rust-avx2 zig zig-native zig-v32 zig-v64" >&2
    exit 2
fi

# Refuse the whole run on a name that does not exist rather than building most
# of it and reporting success. A workflow that asks for a target this script
# has since renamed should fail loudly, not quietly measure a stale binary.
for target in "$@"; do
    if ! command_for "$target" >/dev/null; then
        echo "build_ports: no such target: $target" >&2
        exit 2
    fi
done

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
logs=$(mktemp -d)
trap 'rm -rf "$logs"' EXIT

jobs=${BUILD_JOBS:-$(nproc 2>/dev/null || echo 4)}
echo "building ${*} with up to $jobs at a time"

declare -a names pids started
for target in "$@"; do
    # Wait for a slot. `wait -n` returns when any one child finishes; its exit
    # status is collected properly in the join loop below, so it is ignored here.
    while [ "$(jobs -rp | wc -l)" -ge "$jobs" ]; do
        wait -n 2>/dev/null || true
    done
    ( cd "$root" && eval "$(command_for "$target")" ) >"$logs/$target.log" 2>&1 &
    names+=("$target")
    pids+=("$!")
    started+=("$SECONDS")
done

broken=0
for i in "${!pids[@]}"; do
    name=${names[$i]}
    if wait "${pids[$i]}"; then
        status="ok"
    else
        status="FAILED"
        broken=$(( broken + 1 ))
    fi
    echo "::group::$status  $name  ($(( SECONDS - started[i] ))s)"
    cat "$logs/$name.log"
    echo "::endgroup::"
    [ "$status" = "ok" ] || echo "build_ports: $name failed" >&2
done

echo "build_ports: $(( ${#names[@]} - broken )) of ${#names[@]} target sets built in ${SECONDS}s"
[ "$broken" -eq 0 ]
