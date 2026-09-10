#!/usr/bin/env python3
"""The same comparison from CSV, JSON and Parquet, in every native port.

Two questions at once, which is why the table has two axes:

  * how the three ports compare on one input -- the question "can Rust and Zig
    match the C++ port" is asked here, on one host, in one sitting;
  * what the input format costs -- the same rows, the same key, the same
    per-cell diff, with only the container changing.

Every payload is written by this project's own generator (`rust/gen-data`), so
the three formats hold the same values spelled the same way; a conversion step
would otherwise be measuring the converter. Each format is generated, measured
and deleted in turn, because all three at ten million rows is about 15 GB.

Peak RSS and CPU time both come from `wait4`'s rusage for that exact child --
the kernel's own high-water mark rather than a poll that can miss a spike. These
engines map their inputs, so resident pages include the file; the `above` column
subtracts it, and for Parquet that number is the whole point, since the pages are
decoded into an arena and the mapping is given back.

CPU seconds over wall seconds is the `cores` column: how many of this machine's
cores were actually busy. It is the column that separates "slow" from "idle" --
two ports at the same wall time and 1.2x against 3.6x cores are not the same
result, and the first one has headroom the second has already spent.

Both inputs are read once before anything is timed: a cold page cache costs more
than every difference this table is trying to show.

  python3 scripts/bench_formats_ports.py --rows 1m --repeats 2
  python3 scripts/bench_formats_ports.py --rows 10m --formats csv,parquet
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

from mdtable import render

ROOT = Path(__file__).resolve().parent.parent
KEY = ["-k", "account_id,txn_id", "-i", "updated_at"]

C = ROOT / "c/csvdiff"
CPP = ROOT / "cpp/build/csvdiff"
RUST = ROOT / "rust/target/release/csvdiff"
ZIG = ROOT / "zig/zig-out/bin/csvdiff"
GEN = ROOT / "rust/target/release/gen-data"


TEXT = {"csv", "ndjson"}
ALL = {"csv", "ndjson", "parquet"}


def ports(threads: int | None, matrix: bool) -> list[tuple[str, list[str], list[str], set[str]]]:
    """(label, argv prefix, extra flags, the formats it can read).

    All four ports read all three formats. A Parquet pair is the one input none
    of them scans: it goes to the columnar path instead, which is why the Parquet
    rows below are not measuring the scanner the CSV rows are.

    The C port is here rather than only in `bench_ports.py` so that one table can
    answer "did this change make a port slower" for every port at once. It was
    the one port this script did not build, which meant the per-pull-request
    benchmark and the leading port were in two different workflows.

    `matrix` adds the scanner builds -- SWAR against a vector register, one
    binary each so nothing is measuring a branch -- and the Rust engine without
    its report, which is the only row here that is not comparing like with like:
    the Rust port renders the HTML the other two do not produce at all.
    """
    thread_flag = ["--threads", str(threads)] if threads else []
    report = ["--engine", "turbo", "-o", "/dev/null"]
    rows: list[tuple[str, list[str], list[str], set[str]]] = [
        ("C", [str(C)], thread_flag, ALL),
        ("C++", [str(CPP)], thread_flag, ALL),
        ("Rust", [str(RUST)], report + thread_flag, ALL),
        ("Zig", [str(ZIG)], thread_flag, ALL),
    ]
    if not matrix:
        return rows
    rows.append(("Rust engine", [str(RUST)], report + ["--max-rows", "1"] + thread_flag, ALL))
    for label, path, flags, formats in [
        # The scanner builds differ only in how they find a delimiter, so they
        # are asked only about the formats that have delimiters to find.
        ("C++ swar", CPP.with_name("csvdiff-swar"), thread_flag, TEXT),
        ("C++ avx2", CPP.with_name("csvdiff-avx2"), thread_flag, TEXT),
        ("C++ avx512", CPP.with_name("csvdiff-avx512"), thread_flag, TEXT),
        ("Rust avx2", ROOT / "rust/target-avx2/release/csvdiff", report + thread_flag, ALL),
        ("Zig v32", ROOT / "zig/zig-out-v32/bin/csvdiff", thread_flag, ALL),
        ("Zig v64", ROOT / "zig/zig-out-v64/bin/csvdiff", thread_flag, ALL),
    ]:
        # A variant that was not built is left out by name rather than reported
        # as slow, and an AVX-512 binary on a runner without AVX-512 will not
        # start at all -- which the run records as a failure rather than a time.
        if path.exists():
            rows.append((label, [str(path)], flags, formats))
    return rows


def run(argv: list[str], timeout: float) -> tuple[float, float, float, int]:
    """Times one child: wall seconds, peak RSS in MB, CPU seconds, exit code.

    CPU is user plus system for that exact child, from the same `wait4` rusage
    the RSS comes from. Wall time says how long you waited; CPU divided by wall
    says how many cores were busy while you waited, which is the difference
    between an engine that is slow and an engine that is idle. A port that
    finishes in the same wall time on half the CPU has the headroom the other
    one has already spent.
    """
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
            return (time.monotonic() - started, usage.ru_maxrss / 1024,
                    usage.ru_utime + usage.ru_stime, os.waitstatus_to_exitcode(status))
        if time.monotonic() > deadline:
            os.kill(pid, 9)
            _, _, usage = os.wait4(pid, 0)
            return (time.monotonic() - started, 0.0,
                    usage.ru_utime + usage.ru_stime, -1)
        time.sleep(0.05)


def warm(*paths: Path) -> None:
    for path in paths:
        with path.open("rb") as handle:
            while handle.read(1 << 22):
                pass


def counts(path: Path) -> dict:
    with path.open() as handle:
        return json.load(handle)["counts"]


def generate(rows: str, fmt: str, data: Path) -> tuple[Path, Path, float]:
    started = time.monotonic()
    argv = [str(GEN), "--rows", rows, "--out-dir", str(data), "--prefix", "bench",
            "--format", fmt]
    done = subprocess.run(argv, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(f"the generator failed: {done.stderr.strip()[:400]}")
    extension = {"csv": "csv", "ndjson": "ndjson", "parquet": "parquet"}[fmt]
    return (data / f"bench_a.{extension}", data / f"bench_b.{extension}",
            time.monotonic() - started)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rows", default="1m")
    parser.add_argument("--formats", default="csv,ndjson,parquet")
    parser.add_argument("--json-out", type=Path, default=None,
                        help="also write the rows here, unrounded, for a harness "
                             "that has to compare two runs of this script")
    parser.add_argument("--md-out", type=Path, default=None,
                        help="write just the results table here, as markdown")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--threads", type=int, default=None,
                        help="threads per run; the default is whatever each port picks")
    parser.add_argument("--timeout", type=float, default=1800)
    parser.add_argument("--data-dir", default=str(ROOT / "data/bench"))
    parser.add_argument("--keep", action="store_true", help="do not delete the payloads")
    parser.add_argument("--matrix", action="store_true",
                        help="also run the scanner builds and the Rust engine without its report")
    args = parser.parse_args(argv)

    for tool in (RUST, ZIG, GEN):
        if not tool.exists():
            raise SystemExit(f"build it first: {tool} is missing")

    data = Path(args.data_dir)
    data.mkdir(parents=True, exist_ok=True)
    summary = Path("/tmp/bench_ports_summary.json")
    results: list[dict] = []
    answers: dict[str, dict] = {}

    for fmt in args.formats.split(","):
        fmt = fmt.strip()
        a, b, generated = generate(args.rows, fmt, data)
        size = (a.stat().st_size + b.stat().st_size) / (1 << 20)
        print(f"\n{fmt}: {size:,.0f} MB, generated in {generated:.1f}s", flush=True)
        warm(a, b)

        for label, prefix, flags, can_read in ports(args.threads, args.matrix):
            if fmt not in can_read:
                print(f"  {label:5s} -- does not read {fmt}", flush=True)
                results.append({"format": fmt, "port": label, "seconds": None})
                continue
            if not Path(prefix[0]).exists():
                print(f"  {label:5s} -- not built", flush=True)
                continue
            best = None
            for _ in range(args.repeats):
                argv_run = prefix + ["compare", str(a), str(b)] + KEY + flags + \
                    ["--json", str(summary)]
                seconds, rss, cpu, code = run(argv_run, args.timeout)
                if code not in (0, 1):
                    print(f"  {label:5s} FAILED (exit {code})", flush=True)
                    best = None
                    break
                got = counts(summary)
                answers.setdefault(f"{label}/{fmt}", got)
                # The best run is the fastest one, and its CPU travels with it:
                # pairing the fastest wall time with another run's CPU would
                # make the utilisation a ratio of two different runs.
                if best is None or seconds < best[0]:
                    best = (seconds, rss, cpu)
            if best is None:
                results.append({"format": fmt, "port": label, "seconds": None})
                continue
            seconds, rss, cpu = best
            results.append({
                "format": fmt, "port": label, "seconds": seconds, "rss": rss,
                "cpu": cpu, "cores": cpu / seconds if seconds > 0 else 0.0,
                "input": size, "above": rss - size, "rows": got["a_rows"],
            })
            print(f"  {label:5s} {seconds:8.2f}s  {cpu:8.1f}s cpu  "
                  f"{cpu / seconds if seconds else 0:5.2f}x cores  "
                  f"{rss:9,.0f} MB peak  {rss - size:8,.0f} MB above the input",
                  flush=True)

        if not args.keep:
            for path in (a, b):
                path.unlink(missing_ok=True)

    # Every port and every format has to return the same counts. A faster answer
    # that is not the same answer is not a result.
    distinct = {json.dumps(v, sort_keys=True) for v in answers.values()}
    print("\ncounts:", "identical everywhere" if len(distinct) == 1 else "DISAGREE")
    if len(distinct) != 1:
        for name, value in answers.items():
            print(f"  {name}: {value}")
        return 1
    print(f"  {json.dumps(json.loads(distinct.pop()))}")

    cores = os.cpu_count() or 1
    print(f"\n{cores} cores; \"cores\" is CPU seconds over wall seconds -- how many "
          f"were busy, out of {cores}.")
    grid = []
    for row in results:
        if row.get("seconds") is None:
            grid.append([row["port"], row["format"], "-", "-", "-", "-", "-", "-"])
            continue
        rate = row["rows"] / row["seconds"]
        grid.append([row["port"], row["format"], f"{row['seconds']:.2f}s", f"{rate:,.0f}",
                     f"{row['cpu']:.1f}s", f"{row['cores']:.2f}x",
                     f"{row['rss']:,.0f} MB", f"{row['above']:,.0f} MB"])
    md = render(["Build", "Format", "Compare", "Rows/s", "CPU", "Cores", "Peak RSS",
                 "Above the input"],
                ["l", "l", "r", "r", "r", "r", "r", "r"], grid)
    print("\n" + md, end="")

    # The table on its own, for a caller that wants to publish it as a table
    # rather than as part of this log. Everything above is narrative -- what was
    # generated, what each port did, whether the counts agreed -- and belongs in
    # a code block; the table does not, and inside one it can never render.
    if args.md_out:
        args.md_out.parent.mkdir(parents=True, exist_ok=True)
        args.md_out.write_text(md)

    # The table rounds seconds to two decimals, which is a couple of per cent at
    # the sizes this runs at -- fine to read, too coarse to compare two runs
    # with. The JSON keeps what was measured.
    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(json.dumps(
            {"rows_arg": args.rows, "cores": cores, "results": results},
            indent=2, sort_keys=True))
        print(f"\nwrote {args.json_out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
