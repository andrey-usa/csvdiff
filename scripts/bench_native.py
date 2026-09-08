#!/usr/bin/env python3
"""Benchmarks the byte-level engines against each other.

This is the harness behind the "byte-level ports" and "twenty million rows"
tables in the README. It is deliberately not `bench_external.py`: that one asks
whether a *tool* can do this job, and includes tools that only produce counts.
This one asks how fast one *design* is when several languages and toolchains
implement it, so every entry here computes which cell changed and the field is
limited to that.

Two things it is careful about, both of which produced wrong numbers first:

Peak RSS comes from `wait4`'s rusage for that exact child, which is the kernel's
own high-water mark rather than a poll that can miss a spike. Because these
engines map their inputs, resident pages include the two files -- so the column
that carries information is `above`, which subtracts them.

Both inputs are read once before anything is timed. A cold page cache costs more
than every difference this table is trying to show.

  python scripts/bench_native.py --rows 20m --repeats 2
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KEY = ["-k", "account_id,txn_id", "-i", "updated_at"]

# A build that is not there is skipped by name rather than silently dropped: it
# is not the same result as a build that is slow.
def builds(tmp: Path) -> list[tuple[str, list[str], list[str]]]:
    """(label, argv prefix, extra flags).

    The C, C++ and Zig ports carry the comparison and the JSON counts but not
    the HTML report, so they take no `-o`. The full ports are given `-o
    /dev/null` for the same reason: rendering is not what is being compared.
    """
    out: list[tuple[str, list[str], list[str]]] = []

    for label, path in [
        ("C++, clang 20", tmp / "cpp_clang20"),
        ("C++, clang 18", tmp / "cpp_clang18"),
        ("C++, g++ 14", tmp / "cpp_g14"),
        ("C++, g++ 13", tmp / "cpp_g13"),
        ("C, clang 20", tmp / "c_clang20"),
        ("C, gcc 14", tmp / "c_gcc14"),
        ("Zig 0.17-dev", tmp / "zig017"),
        ("Zig 0.16", tmp / "zig016"),
    ]:
        out.append((label, [str(path)], []))

    out.append(("Rust, turbo", [str(ROOT / "rust/target/release/csvdiff")],
                ["--engine", "turbo", "-o", "/dev/null"]))
    return out


def run(argv: list[str], out: Path, timeout: float) -> tuple[float, float, int]:
    """Times one child and returns seconds, peak RSS in MB, and its exit code.

    The output file is removed first: a compare exits 1 when it finds
    differences, so its status cannot say whether it ran, and a crash would
    otherwise be recorded as the previous iteration's answer read back as this
    one's. -1 as the code means it was killed for running past `timeout`.
    """
    if out.exists():
        out.unlink()
    started = time.monotonic()
    pid = os.fork()
    if pid == 0:
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, 1)
        os.dup2(devnull, 2)
        try:
            os.execv(argv[0], argv)
        except OSError:
            pass
        os._exit(127)
    deadline = started + timeout
    while True:
        done, status, usage = os.wait4(pid, os.WNOHANG)
        if done:
            return time.monotonic() - started, usage.ru_maxrss / 1024, os.waitstatus_to_exitcode(status)
        if time.monotonic() > deadline:
            os.kill(pid, 9)
            _, _, usage = os.wait4(pid, 0)
            return time.monotonic() - started, usage.ru_maxrss / 1024, -1
        time.sleep(0.05)


def warm(*paths: Path) -> None:
    for path in paths:
        with path.open("rb") as handle:
            while handle.read(1 << 22):
                pass


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rows", default="1m", help="10k, 1m, 10m, 20m")
    ap.add_argument("--data-dir", type=Path, default=ROOT / "bench/external/data")
    ap.add_argument("--tmp-dir", type=Path, default=Path("/tmp"),
                    help="where the alternative builds live")
    ap.add_argument("--repeats", type=int, default=2)
    ap.add_argument("--timeout", type=float, default=1800,
                    help="seconds before a build is recorded as not finishing")
    args = ap.parse_args()

    a = args.data_dir / f"{args.rows}_a.csv"
    b = args.data_dir / f"{args.rows}_b.csv"
    if not (a.exists() and b.exists()):
        print(f"generate the pair first: python scripts/gen_data.py --rows {args.rows} "
              f"--out-dir {args.data_dir} --prefix {args.rows}", file=sys.stderr)
        return 2

    plan = builds(args.tmp_dir)
    mapped = (a.stat().st_size + b.stat().st_size) / 1024**2
    warm(a, b)
    print(f"{args.rows}: inputs map {mapped:,.0f} MB, best of {args.repeats}\n")
    print(f"{'build':32} {'seconds':>9} {'peak RSS':>10} {'above':>9}  counts")

    truth = None
    for label, base, extra in plan:
        if not (os.path.exists(base[0]) and os.access(base[0], os.X_OK)):
            print(f"{label:32} {'not built':>9}")
            continue
        out = Path("/tmp") / ("bn_" + "".join(c if c.isalnum() else "_" for c in label) + ".json")
        best, counts = None, None
        for _ in range(args.repeats):
            secs, rss, code = run(base + ["compare", str(a), str(b)] + KEY
                                  + ["--json", str(out)] + extra, out, args.timeout)
            if code == -1:
                print(f"{label:32} {'✗':>9}  did not finish in {args.timeout:.0f}s")
                best = None
                break
            if not out.exists():
                print(f"{label:32} {'✗':>9}  produced no output (exit {code})")
                best = None
                break
            counts = json.loads(out.read_text())["counts"]
            if best is None or secs < best[0]:
                best = (secs, rss)
        if not best or counts is None:
            continue
        key = (counts["changed"], counts["added"], counts["removed"])
        if truth is None:
            truth, verdict = key, "reference"
        else:
            verdict = "agrees" if key == truth else f"DIFFERS {key}"
        print(f"{label:32} {best[0]:9.2f} {best[1]:9,.0f} {best[1] - mapped:9,.0f}  {verdict}")

    if truth:
        print(f"\nchanged {truth[0]:,} · added {truth[1]:,} · removed {truth[2]:,}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
