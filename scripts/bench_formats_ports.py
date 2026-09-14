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

The `Budget` column is a different quantity and is the one `--memory-cap` acts
on: peak `VmData`, the virtual size of the private writable mappings, which is
what `RLIMIT_DATA` is checked against. A port that holds an old buffer while it
fills a new one is charged for both there and for neither in peak RSS, so the two
columns can disagree by a lot -- on a 6M CSV pair the port with the second
smallest `above` needs the largest budget. Without this column the table could
not explain its own refusals. It is polled rather than taken from rusage, which
has no equivalent, so it is a floor on the true peak.

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
import resource
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
ALIAS = {"json": "ndjson"}


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

    The Rust rows do **not** pass `--engine turbo`, and used to. The Rust port
    carries two Parquet readers, and `engine.rs` says in as many words that an
    explicit `--engine turbo` is the one thing that retires the columnar one:
    *"Only an explicit --engine turbo should do that."* So this table spent its
    life measuring the reader a user never gets. On a 500k pair the two are
    0.15s against 0.43s, and 91 MB against 263 MB above the input, for identical
    counts -- which is the whole of the Rust Parquet column in every table this
    project has published, and most of why the port could not do 50M. `auto`
    picks the columnar reader and falls through to `turbo` for a codec or a page
    version it cannot read, so dropping the flag loses no coverage.
    """
    thread_flag = ["--threads", str(threads)] if threads else []
    # Only the report suppression. See the note above about `--engine`.
    report = ["-o", "/dev/null"]
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


def run(argv: list[str], timeout: float,
        memory_cap_mb: int | None = None,
        errors: Path | None = None) -> tuple[float, float, float, int, str, float]:
    """Times one child: wall, peak RSS in MB, CPU seconds, exit code, why, peak data.

    CPU is user plus system for that exact child, from the same `wait4` rusage
    the RSS comes from. Wall time says how long you waited; CPU divided by wall
    says how many cores were busy while you waited, which is the difference
    between an engine that is slow and an engine that is idle. A port that
    finishes in the same wall time on half the CPU has the headroom the other
    one has already spent.

    `why` is the child's last line of stderr, which is empty on a run that
    worked and is the whole diagnosis on one that did not. It used to go to
    /dev/null with stdout, so a rung that ran out of memory reported `FAILED
    (exit 2)` and nothing else -- and the ports name what did not fit.

    The last number is peak `VmData`, and it is here because **it is the quantity
    the cap below actually bounds** and peak RSS is not. `RLIMIT_DATA` is checked
    against `mm->data_vm`, the *virtual* size of the private writable mappings --
    heap, anonymous mmap, thread stacks -- so a port that holds an old buffer
    while it fills a new one is charged for both even though only one is ever
    touched. Peak RSS cannot see that: it counts resident pages, and it counts
    the mapped input among them.

    Which made the table unable to explain its own refusals. Measured on a 6M CSV
    pair, the four ports want 448, 747, 748 and 955 MB of `VmData` while holding
    405, 511, 697 and 592 MB above the input -- so the port with the *second
    smallest* footprint is the one that needs the largest budget, and it is the
    one the 100M rung refused. Sampled from /proc rather than taken from `wait4`,
    because rusage has no equivalent: `ru_maxrss` is the only memory figure there.
    """
    started = time.monotonic()
    pid = os.fork()
    if pid == 0:
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, 1)
        if errors is None:
            os.dup2(devnull, 2)
        else:
            fd = os.open(errors, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            os.dup2(fd, 2)
        if memory_cap_mb:
            # RLIMIT_DATA, not RLIMIT_AS. Every port maps its input, and since
            # Linux 4.7 this limit covers the heap and private anonymous
            # mappings but *not* a file-backed one -- so it bounds what scales
            # with the row count and leaves the mapping alone, which is the
            # distinction the ports' own --max-memory draws. Measured: a 702 MB
            # pair compares fine under a 256 MB cap, and is refused under 96 MB.
            #
            # This is what stops a rung too large for its runner from taking the
            # whole runner with it. Without it the kernel reclaims the VM, the
            # step dies with exit 143 and "the runner has received a shutdown
            # signal", and the rung reports nothing at all -- not even that it
            # ran out of memory. All four ports fail legibly under the cap
            # instead, and all four exit 2: C and Zig say "out of memory", C++
            # "std::bad_alloc", and Rust names the structure that did not fit.
            limit = memory_cap_mb * 1024 * 1024
            resource.setrlimit(resource.RLIMIT_DATA, (limit, limit))
        try:
            os.execv(argv[0], argv)
        except OSError:
            pass
        os._exit(127)
    deadline = started + timeout
    peak_data = 0.0
    status_path = f"/proc/{pid}/status"
    while True:
        # Before the wait, so the last sample is as late as the poll allows.
        peak_data = max(peak_data, vm_data_mb(status_path))
        done, status, usage = os.wait4(pid, os.WNOHANG)
        if done:
            return (time.monotonic() - started, usage.ru_maxrss / 1024,
                    usage.ru_utime + usage.ru_stime,
                    os.waitstatus_to_exitcode(status), last_line(errors), peak_data)
        if time.monotonic() > deadline:
            os.kill(pid, 9)
            _, _, usage = os.wait4(pid, 0)
            return (time.monotonic() - started, 0.0,
                    usage.ru_utime + usage.ru_stime, -1, last_line(errors), peak_data)
        time.sleep(0.05)


def vm_data_mb(status_path: str) -> float:
    """This process's current `VmData`, in MB, or 0 where it cannot be read.

    Linux only -- /proc/<pid>/status. On anything else this returns 0 and the
    column reads as a dash, which is honest: the cap is not applied there either,
    since `RLIMIT_DATA` bounding anonymous mappings is a Linux behaviour.

    Polled, so it is a floor on the true peak and not the peak itself: a spike
    shorter than the 50ms between samples is invisible here. Measured against a
    5ms poll of the same run, the Zig port's 955 MB peak reads 955, 888 and 888
    on three 50ms passes -- an undershoot of up to 7% on a run lasting 1.6s. The
    rungs this column exists for run for minutes, where a phase boundary lasts
    long enough to be sampled many times, so treat it as exact at scale and as
    approximate on a small pair.

    It rides the poll the timeout already runs on, so the column costs one open
    and one read per fifty milliseconds and no extra wakeups.
    """
    try:
        with open(status_path) as handle:
            for line in handle:
                if line.startswith("VmData:"):
                    return int(line.split()[1]) / 1024
    except (OSError, ValueError, IndexError):
        pass
    return 0.0


def last_line(path: Path | None) -> str:
    """The child's final line of stderr, or "" if it wrote none.

    The last one and not the first: a port that fails on several threads at once
    writes several lines, and the ones behind it are the same refusal again.
    """
    if path is None:
        return ""
    try:
        text = path.read_text(errors="replace").strip()
    except OSError:
        return ""
    if not text:
        return ""
    return text.splitlines()[-1].strip()[:200]


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


def table_of(results: list[dict]) -> str:
    """The results table as markdown, from whatever has been measured so far."""
    grid = []
    for row in results:
        if row.get("seconds") is None:
            grid.append([row["port"], row["format"], "-", "-", "-", "-", "-", "-", "-"])
            continue
        rate = row["rows"] / row["seconds"] if row["seconds"] else 0
        data = row.get("data") or 0
        grid.append([row["port"], row["format"], f"{row['seconds']:.2f}s", f"{rate:,.0f}",
                     f"{row['cpu']:.1f}s", f"{row['cores']:.2f}x",
                     f"{row['rss']:,.0f} MB", f"{row['above']:,.0f} MB",
                     f"{data:,.0f} MB" if data else "-"])
    # "Budget" and not a second memory column with a similar name: it is what
    # `--memory-cap` has to be at least, which is a different question from what
    # the run holds. See `run()` for why the two differ and by how much.
    return render(["Build", "Format", "Compare", "Rows/s", "CPU", "Cores", "Peak RSS",
                   "Above the input", "Budget"],
                  ["l", "l", "r", "r", "r", "r", "r", "r", "r"], grid)


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
    parser.add_argument("--memory-cap", type=int, default=None, metavar="MB",
                        help="cap each port's heap and anonymous memory (not its "
                             "mapped input) so a size too large for this host is "
                             "refused by the port rather than killing the host")
    parser.add_argument("--data-dir", default=str(ROOT / "data/bench"))
    parser.add_argument("--keep", action="store_true", help="do not delete the payloads")
    parser.add_argument("--matrix", action="store_true",
                        help="also run the scanner builds and the Rust engine without its report")
    parser.add_argument("--first", default=None, metavar="LABEL",
                        help="run the rows whose label starts with LABEL before all the "
                             "others; the table keeps its declared order either way")
    args = parser.parse_args(argv)

    for tool in (RUST, ZIG, GEN):
        if not tool.exists():
            raise SystemExit(f"build it first: {tool} is missing")

    data = Path(args.data_dir)
    data.mkdir(parents=True, exist_ok=True)
    summary = Path("/tmp/bench_ports_summary.json")
    errors = Path("/tmp/bench_ports_stderr.txt")
    results: list[dict] = []
    answers: dict[str, dict] = {}

    # Resolved and checked before anything is generated. `json` is what the
    # generators' own --format flag calls this format, so it is accepted here
    # too; a name that is neither is a mistake worth hearing about now rather
    # than after the first format has been measured.
    wanted = []
    for name in args.formats.split(","):
        fmt = ALIAS.get(name.strip(), name.strip())
        if fmt not in ALL:
            raise SystemExit(f"unknown format {name.strip()!r}: "
                             f"pick from {', '.join(sorted(ALL))}")
        wanted.append(fmt)

    for fmt in wanted:
        a, b, generated = generate(args.rows, fmt, data)
        size = (a.stat().st_size + b.stat().st_size) / (1 << 20)
        print(f"\n{fmt}: {size:,.0f} MB, generated in {generated:.1f}s", flush=True)
        warm(a, b)

        # Ports keep their declared order in the table no matter what
        # happens to them at run time, so two runs of this script can be read
        # side by side.
        plan = []
        for label, prefix, flags, can_read in ports(args.threads, args.matrix):
            if fmt not in can_read:
                print(f"  {label:5s} -- does not read {fmt}", flush=True)
                plan.append({"label": label, "state": "unreadable"})
            elif not Path(prefix[0]).exists():
                print(f"  {label:5s} -- not built", flush=True)
                plan.append({"label": label, "state": "unbuilt"})
            else:
                plan.append({"label": label, "prefix": prefix, "flags": flags,
                             "state": "ok"})
        runnable = [e for e in plan if e["state"] == "ok"]
        # `--first` reorders who runs, not what the table shows. A rung that dies
        # partway keeps whatever ran before it died, and declared order decided
        # who that was: a 200m measurement had the runner taken away after C and
        # C++ and before Rust and Zig, so the cells at the back of a truncated
        # ladder were blank for a reason that had nothing to do with them -- the
        # exact failure this rotation comment exists to name. Naming the port
        # that matters most puts its number on disk first. Rows that share the
        # label's prefix (the matrix Rust rows) come along with it.
        if args.first:
            first = [e for e in runnable if e["label"].startswith(args.first)]
            rest = [e for e in runnable if not e["label"].startswith(args.first)]
            runnable = first + rest
        best: dict[str, tuple[float, float, float]] = {}
        failed: set[str] = set()
        rows_seen: dict[str, int] = {}

        # One run of every port per round, and the rounds repeat -- which is
        # what BENCHMARKS.md has always said this does and what it did not do.
        # It used to run every repeat of one port before starting the next, so
        # a machine drifting under the run charged that drift to whichever port
        # happened to be in front of it.
        #
        # The starting port rotates. Fixed order makes position part of a
        # port's number -- someone is always first into a cold page cache and
        # someone always last -- and it decides who survives a truncated run.
        # A 40m Parquet cell has died three times on a 16 GB runner, always
        # after C and C++ and always before Rust and Zig, so the two ports at
        # the back of the list have never produced a number at that size. That
        # is not evidence they are slower. It is evidence they run last.
        for rnd in range(args.repeats):
            turn = rnd % len(runnable) if runnable else 0
            for entry in runnable[turn:] + runnable[:turn]:
                label = entry["label"]
                argv_run = entry["prefix"] + ["compare", str(a), str(b)] + KEY + \
                    entry["flags"] + ["--json", str(summary)]
                seconds, rss, cpu, code, why, vm_data = run(argv_run, args.timeout,
                                                            args.memory_cap, errors)
                if code not in (0, 1):
                    # A port that failed once has failed. A later round that
                    # happens to succeed does not withdraw the failure.
                    said = f": {why}" if why else ""
                    print(f"  {label:5s} FAILED (exit {code}){said}", flush=True)
                    failed.add(label)
                    continue
                got = counts(summary)
                answers.setdefault(f"{label}/{fmt}", got)
                rows_seen[label] = got["a_rows"]
                # The best run is the fastest one, and its CPU travels with it:
                # pairing the fastest wall time with another run's CPU would
                # make the utilisation a ratio of two different runs.
                if label not in best or seconds < best[label][0]:
                    best[label] = (seconds, rss, cpu, vm_data)

        for entry in plan:
            label, state = entry["label"], entry["state"]
            if state == "unbuilt":
                continue
            if state == "unreadable" or label in failed or label not in best:
                results.append({"format": fmt, "port": label, "seconds": None})
                continue
            seconds, rss, cpu, vm_data = best[label]
            results.append({
                "format": fmt, "port": label, "seconds": seconds, "rss": rss,
                "cpu": cpu, "cores": cpu / seconds if seconds > 0 else 0.0,
                "input": size, "above": rss - size, "rows": rows_seen[label],
                "data": vm_data,
            })
            budget = f"{vm_data:8,.0f} MB budget" if vm_data else "   (no budget)"
            print(f"  {label:5s} {seconds:8.2f}s  {cpu:8.1f}s cpu  "
                  f"{cpu / seconds if seconds else 0:5.2f}x cores  "
                  f"{rss:9,.0f} MB peak  {rss - size:8,.0f} MB above the input  "
                  f"{budget}",
                  flush=True)

        if not args.keep:
            for path in (a, b):
                path.unlink(missing_ok=True)

        # Written now rather than at the end. A run that is killed partway --
        # which is how the 40m Parquet cell has ended three times -- used to
        # leave nothing behind at all, because the table was assembled after
        # every format had finished. What has been measured is now on disk as
        # soon as it has been measured.
        if args.md_out:
            args.md_out.parent.mkdir(parents=True, exist_ok=True)
            args.md_out.write_text(table_of(results))

    # Every port and every format has to return the same counts. A faster answer
    # that is not the same answer is not a result.
    distinct = {json.dumps(v, sort_keys=True) for v in answers.values()}
    # Three outcomes, not two. No port producing counts is an absence, not a
    # disagreement, and calling it "DISAGREE" sent a reader looking for a
    # discrepancy between ports that had each failed before answering -- which is
    # what every port does when the run is capped below what the size needs.
    if not distinct:
        print("\ncounts: none -- no port got far enough to answer")
        return 1
    print("\ncounts:", "identical everywhere" if len(distinct) == 1 else "DISAGREE")
    if len(distinct) != 1:
        for name, value in answers.items():
            print(f"  {name}: {value}")
        return 1
    print(f"  {json.dumps(json.loads(distinct.pop()))}")

    cores = os.cpu_count() or 1
    print(f"\n{cores} cores; \"cores\" is CPU seconds over wall seconds -- how many "
          f"were busy, out of {cores}.")
    md = table_of(results)
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
