#!/usr/bin/env bash
#
# Build one port for release and pack it into a tarball.
#
#   scripts/package_release.sh PORT VERSION [OUTDIR]     PORT is c, cpp, rust or zig
#
# Writes OUTDIR/csvdiff-PORT-VERSION-OS-ARCH.tar.gz (OUTDIR defaults to dist/) and
# prints its SHA-256. The archive holds the binary, the LICENSE, the port's
# README and a BUILDINFO naming the commit, the compiler and the flags.
#
# Every port is built for the *baseline* of its architecture, never for the
# machine that builds it. `-march=native` (which the C and C++ Makefiles use by
# default, because they are made for benchmarking) bakes the runner's CPU into the
# binary; a release built that way dies with SIGILL on an older machine, and the
# project has already shipped a committed binary that did exactly that. The
# price is the wide scanners the ports pick at compile time (AVX2 and up), which
# a baseline x86-64 build leaves out -- see README.md on the per-port scan step.
# A tuned x86-64-v3 build is a separate artifact to add, not a default to sneak in.
#
# It runs the binary before packing it: `--help` must work and exit 0 or 1, which
# is the difference between a build and a build that starts.

set -euo pipefail

port=${1:?usage: package_release.sh PORT VERSION [OUTDIR]}
version=${2:?usage: package_release.sh PORT VERSION [OUTDIR]}
out=${3:-dist}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) os=windows exe=.exe ;;
  Darwin)               os=macos   exe= ;;
  *)                    os=linux   exe= ;;
esac
arch=$(uname -m)
[ "$arch" = arm64 ] && arch=aarch64
[ "$arch" = amd64 ] && arch=x86_64
name="csvdiff-$port-$version-$os-$arch"

case "$port" in
  c)
    # No -march: the Makefile only adds -march=native by default; CFLAGS replaces it.
    flags="-std=c11 -O2 -Wall -Wextra -Werror"
    [ -z "$exe" ] && flags="$flags -pthread"
    make -C c clean >/dev/null
    make -C c CFLAGS="$flags" "csvdiff$exe"
    built=c/csvdiff$exe; readme=c/README.md
    tool="$(${CC:-cc} --version | head -1)"
    ;;
  cpp)
    flags="-std=c++20 -O2 -Wall -Wextra -Wpedantic -Werror -pthread"
    make -C cpp clean >/dev/null
    make -C cpp CXXFLAGS="$flags" "build/csvdiff$exe"
    built=cpp/build/csvdiff$exe; readme=cpp/README.md
    tool="$(${CXX:-g++} --version | head -1)"
    ;;
  rust)
    # Cargo's default target has no target-cpu: baseline.
    (cd rust && cargo build --release --locked --bin csvdiff)
    built=rust/target/release/csvdiff$exe; readme=rust/README.md
    flags="cargo build --release --locked"
    tool="$(rustc --version)"
    ;;
  zig)
    (cd zig && zig build --release=fast -Dcpu=baseline --prefix zig-out-release)
    built=zig/zig-out-release/bin/csvdiff$exe; readme=zig/README.md
    flags="zig build --release=fast -Dcpu=baseline"
    tool="zig $(zig version)"
    ;;
  *) echo "unknown port: $port (c, cpp, rust, zig)" >&2; exit 2 ;;
esac

[ -x "$built" ] || { echo "::error::$port did not produce $built" >&2; exit 1; }

# It has to start. --help exits 0, or 1 in some ports; 2 is a usage error and
# a signal is a crash, which is the case this is here to catch.
code=0
"$built" --help >/dev/null 2>&1 || code=$?
if [ "$code" -gt 1 ]; then
  echo "::error::$built --help exited $code" >&2
  exit 1
fi

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name" "$out"
cp "$built" "$stage/$name/"
cp LICENSE "$stage/$name/"
cp "$readme" "$stage/$name/README.md"
cat > "$stage/$name/BUILDINFO" <<EOF
port:     $port
version:  $version
commit:   $(git rev-parse HEAD 2>/dev/null || echo unknown)
platform: $os-$arch
compiler: $tool
flags:    $flags
EOF

tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
if command -v sha256sum >/dev/null; then sum=$(sha256sum "$out/$name.tar.gz" | cut -d' ' -f1)
else sum=$(shasum -a 256 "$out/$name.tar.gz" | cut -d' ' -f1); fi
echo "$sum  $name.tar.gz"
