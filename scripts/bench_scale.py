#!/usr/bin/env python3
"""Runs one engine across a ladder of sizes, generating and deleting as it goes.

The point is the shape of the curve, not a comparison: one build, one thread
count, every size. Sizes are done one at a time and the pair is deleted after
its runs, because the ladder as a whole is larger than the disk -- fifty million
rows alone is 18.4 GB of input.

  python scripts/bench_scale.py --sizes 10k,1m,10m,20m,50m --threads 4

Peak RSS comes from wait4's rusage for that exact child, which is the kernel's
own high-water mark. It includes the mapped input files, so `above` subtracts
them: that column is what the engine allocates to do the work.

Once the pair no longer fits in RAM the subtraction stops meaning that, because
the kernel starts evicting mapped pages and resident memory falls below the
input size. That is the design working rather than a measurement fault, so the
column says `evicted` instead of going negative.
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

ROOT = Path(__file__).resolve().parent.parent
KEY = ["-k", "account_id,txn_id", "-i", "updated_at"]


def rows_in(label: str) -> int:
    mult = {"k": 1_000, "m": 1_000_000}
    return int(float(label[:-1]) * mult[label[-1]]) if label[-1] in mult else int(label)


def generator() -> list[str]:
    """The fastest generator on hand.

    All five ports emit byte-identical files -- parity.yml enforces it on every
    change -- so this is free to pick on speed alone. Go builds in seconds and
    writes a million rows in 4.6s against the Python generator's 30.5s, which at
    fifty million rows is four minutes instead of twenty-five. The Python one
    stays as the fallback, since it needs no toolchain.
    """
    built = Path("/tmp/csvdiff-gendata")
    go_src = ROOT / "go/cmd/gen-data"
    if not built.exists() and go_src.exists() and shutil.which("go"):
        done = subprocess.run(["go", "build", "-o", str(built), "./cmd/gen-data"],
                              cwd=ROOT / "go", capture_output=True)
        if done.returncode != 0:
            print(f"  (go generator would not build, using python)", file=sys.stderr)
    if built.exists():
        return [str(built)]
    return [sys.executable, str(ROOT / "scripts/gen_data.py")]


def generate(label: str, data_dir: Path) -> tuple[Path, Path]:
    a, b = data_dir / f"{label}_a.csv", data_dir / f"{label}_b.csv"
    if a.exists() and b.exists():
        return a, b
    data_dir.mkdir(parents=True, exist_ok=True)
    free = shutil.disk_usage(data_dir).free
    need = rows_in(label) * 368  # ~184 bytes a row, two files
    if free < need * 1.05:
        raise SystemExit(f"{label} needs about {need/2**30:.1f} GB and "
                         f"{free/2**30:.1f} GB is free")
    started = time.monotonic()
    subprocess.run(generator() + ["--rows", label, "--out-dir", str(data_dir),
                                  "--prefix", label], check=True,
                   stdout=subprocess.DEVNULL)
    print(f"  generated {label} in {time.monotonic()-started:.0f}s", file=sys.stderr, flush=True)
    return a, b


def warm(*paths: Path) -> bool:
    """Reads both files so no run is charged for a cold cache.

    Returns whether they actually fit: past the point where the pair is larger
    than RAM this cannot do its job, and the runs that follow are paying for
    disk reads the smaller ones did not.
    """
    total = sum(p.stat().st_size for p in paths)
    for p in paths:
        with p.open("rb") as fh:
            while fh.read(1 << 22):
                pass
    try:
        with open("/proc/meminfo") as fh:
            for line in fh:
                if line.startswith("MemTotal:"):
                    return total <= int(line.split()[1]) * 1024 * 0.85
    except OSError:
        pass
    return True


def run(argv: list[str], out: Path) -> tuple[float, float, float, int]:
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
    _, status, usage = os.wait4(pid, 0)
    return (time.monotonic() - started, usage.ru_utime + usage.ru_stime,
            usage.ru_maxrss / 1024, os.waitstatus_to_exitcode(status))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sizes", default="10k,1m,10m,20m,50m")
    ap.add_argument("--binary", default=str(ROOT / "cpp/build/csvdiff"))
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--repeats", type=int, default=2)
    ap.add_argument("--data-dir", type=Path, default=ROOT / "bench/external/data")
    ap.add_argument("--keep", action="store_true", help="do not delete each pair after its runs")
    args = ap.parse_args()

    if not os.access(args.binary, os.X_OK):
        raise SystemExit(f"not executable: {args.binary}")

    print(f"{Path(args.binary).name}, --threads {args.threads}, best of {args.repeats}\n")
    print(f"{'rows':>6} {'input':>9} {'wall':>9} {'rows/s':>12} {'cpu/wall':>9} "
          f"{'RSS':>9} {'above':>9}  result")

    for label in [s.strip() for s in args.sizes.split(",") if s.strip()]:
        a, b = generate(label, args.data_dir)
        fits = warm(a, b)
        mapped = (a.stat().st_size + b.stat().st_size) / 2**20
        out = Path(f"/tmp/scale_{label}.json")
        best = None
        counts = None
        for _ in range(args.repeats):
            secs, cpu, rss, code = run([args.binary, "compare", str(a), str(b)] + KEY
                                       + ["--threads", str(args.threads), "--json", str(out)], out)
            if not out.exists():
                print(f"{label:>6} {mapped:8,.0f}M  failed (exit {code})")
                best = None
                break
            counts = json.loads(out.read_text())["counts"]
            if best is None or secs < best[0]:
                best = (secs, cpu, rss)
        if best and counts:
            secs, cpu, rss = best
            note = "ok" if fits else "input exceeds RAM"
            above = f"{rss-mapped:8,.0f}" if rss >= mapped else " evicted"
            print(f"{label:>6} {mapped:8,.0f}M {secs:8.2f}s {counts['a_rows']/secs:11,.0f} "
                  f"{cpu/secs:8.2f}x {rss:8,.0f} {above}  {note}")
            print(f"       changed {counts['changed']:,} · added {counts['added']:,} · "
                  f"removed {counts['removed']:,} · dup keys "
                  f"{counts['a_dup_keys']:,}/{counts['b_dup_keys']:,}", flush=True)
        if not args.keep and label not in ("10k", "1m"):
            a.unlink(missing_ok=True)
            b.unlink(missing_ok=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
