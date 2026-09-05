#!/usr/bin/env python3
"""Benchmark this tool against the CSV-comparison tools people actually use.

Five implementations of one design tell you which language and which technique is
faster. They do not tell you whether the design is any good. This runs the same
comparison — 20 columns, composite key ``(account_id, txn_id)``, ``updated_at``
ignored — through the established tools and reports time, peak memory, and
whether the answer agrees with ours.

The answer matters more than the time. A tool that is fast because it cannot
express the task, or because it silently drops duplicate keys, is not a faster
way of doing this job; it is a different job.

Usage:
  python scripts/bench_external.py --rows 1m --out-dir bench/external
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import shutil
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

KEY = ["account_id", "txn_id"]
IGNORE = "updated_at"
HEADER = [
    "account_id", "txn_id", "posting_date", "value_date", "currency", "amount",
    "fee", "balance", "status", "channel", "region", "branch_code", "product_code",
    "counterparty", "quantity", "rate", "category", "risk_flag", "note", "updated_at",
]


@dataclass
class Result:
    """One tool's attempt at the comparison."""

    tool: str
    kind: str                      # library or approach it stands for
    seconds: float | None = None
    peak_mb: float | None = None
    counts: dict[str, int] | None = None
    failed: str | None = None
    note: str = ""
    expresses_task: bool = True
    extra: dict = field(default_factory=dict)


def peak_rss_mb(pid: int, stop: threading.Event) -> list[float]:
    """Polls /proc for a child's high-water mark while it runs.

    VmHWM is a high-water mark, so the last reading before exit would be enough
    if we could be sure of getting one. Polling keeps the largest we saw, which
    survives the process exiting between reads.
    """
    seen = [0.0]

    def high_water(of: int) -> float:
        with open(f"/proc/{of}/status") as fh:
            for line in fh:
                if line.startswith("VmHWM:"):
                    return int("".join(c for c in line if c.isdigit())) / 1024
        return 0.0

    def children(of: int) -> list[int]:
        """Direct children, from the main thread's list.

        A thread group has one of these files per thread, but reading all of them means an
        open per thread on every poll, and a JVM has dozens — enough overhead to show up in
        the timing this function exists to leave alone. Every tool here that forks does so
        from its main thread, whose task id is the process id.
        """
        try:
            with open(f"/proc/{of}/task/{of}/children") as fh:
                return [int(p) for p in fh.read().split()]
        except (OSError, ValueError):
            return []

    def tree(of: int) -> float:
        """The largest high-water mark anywhere in the process tree.

        A shell pipeline holds its memory in a grandchild and a tool that forks workers holds
        it in those, so watching only the process that was launched reports a number belonging
        to something that did no work.
        """
        peak = 0.0
        stack = [of]
        while stack:
            at = stack.pop()
            try:
                peak = max(peak, high_water(at))
            except OSError:
                continue
            stack.extend(children(at))
        return peak

    def watch() -> None:
        wait = POLL_MIN_SECONDS
        while not stop.is_set():
            found = tree(pid)
            if found == 0.0 and seen[0] > 0.0:
                return
            seen[0] = max(seen[0], found)
            stop.wait(wait)
            wait = min(wait * 2, POLL_MAX_SECONDS)

    threading.Thread(target=watch, daemon=True).start()
    return seen


# Address space each tool may claim, set by --mem-cap-gb. Without a cap, a tool that
# needs more memory than the machine has does not fail — the kernel kills whatever it
# feels like, benchmark harness included. With one, running out of memory is a result
# the table can print. Mapped file bytes count against it, which is why the cap has to
# leave room for the input on top of whatever the tool holds.
MEM_CAP_BYTES: int | None = None

# How often to read the process tree's high-water mark, starting fast and backing off.
#
# VmHWM is a high-water mark the kernel maintains rather than a sample, so the only thing the
# interval decides is whether the process is still alive to be read at all. A tool that
# finishes in forty milliseconds needs a fast poll or it is never seen — reading it at 10 Hz
# reported the Go tool at 2 MB when it actually peaks above 20. A tool that runs for minutes
# needs a slow one, because polling it hard is overhead charged to the thing being timed.
# Starting at 2 ms and doubling to 100 ms gives each what it needs.
POLL_MIN_SECONDS = 0.002
POLL_MAX_SECONDS = 0.1


def _apply_cap() -> None:
    """Runs in the child between fork and exec."""
    import resource

    if MEM_CAP_BYTES is not None:
        resource.setrlimit(resource.RLIMIT_AS, (MEM_CAP_BYTES, MEM_CAP_BYTES))


def run(cmd: list[str], timeout: float = 3600, env: dict | None = None) -> tuple[int, str, str, float, float]:
    """Runs a command, returning status, stdout, stderr, wall seconds and peak RSS."""
    started = time.perf_counter()
    proc = subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        env={**os.environ, **(env or {})},
        preexec_fn=_apply_cap if MEM_CAP_BYTES is not None else None,
    )
    stop = threading.Event()
    seen = peak_rss_mb(proc.pid, stop)
    try:
        out, err = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        out, err = proc.communicate()
        stop.set()
        return -1, out, err, time.perf_counter() - started, seen[0]
    stop.set()
    return proc.returncode, out, err, round(time.perf_counter() - started, 2), round(seen[0], 1)


# ---------------------------------------------------------------------------
# The tools
# ---------------------------------------------------------------------------

def jvm_heap_mb(cap: int) -> int:
    """The heap to give the JVM under an address-space cap.

    A JVM claims about three gigabytes of address space that is not heap — a gigabyte of
    compressed class space, the code cache, the collector's own tables, a stack per thread —
    and it reserves the whole maximum heap on top of that before running a line of the
    program. The engines here then map both input files, which counts too. So the heap gets
    half the cap: enough to be a real limit, with the other half left for everything the JVM
    and the mapping need. Anything greedier fails at startup or on the first mapping, which
    would read in the table as the tool being unable to do the work rather than the harness
    having mis-set the limit.
    """
    return max(512, int(cap / 1024**2) // 2)


def java_exe() -> Path:
    """The JDK the jar was built for, which is not necessarily the one on PATH."""
    named = os.environ.get("CSVDIFF_JAVA_HOME")
    if named:
        return Path(named, "bin", "java")
    newest = sorted(Path("/opt/jdks").glob("jdk-*/bin/java"), reverse=True)
    if newest:
        return newest[0]
    home = os.environ.get("JAVA_HOME")
    return Path(home, "bin", "java") if home else Path("java")


def fresh(path: Path) -> Path:
    """Removes a tool's output before it runs.

    A crashed tool writes nothing, and a leftover file from an earlier run of the same tool
    then reads as this run's answer — which is exactly how a JVM that died at startup came
    to be recorded as the fastest thing in the table.
    """
    path.unlink(missing_ok=True)
    return path


def ours(a: Path, b: Path, out: Path, engine: str, jar: Path) -> Result:
    """This project, as the reference every other answer is checked against."""
    summary = fresh(out / f"ours-{engine}.json")
    cmd = [str(java_exe()), "--add-modules", "jdk.incubator.vector"]
    if MEM_CAP_BYTES is not None:
        cmd.append(f"-Xmx{jvm_heap_mb(MEM_CAP_BYTES)}m")
    status, _, err, secs, mb = run([
        *cmd, "-cp", str(jar), "dev.csvdiff.Cli",
        "compare", str(a), str(b), "-k", ",".join(KEY), "-i", IGNORE,
        "--engine", engine, "-o", str(out / f"ours-{engine}.html"), "--json", str(summary),
    ])
    if status not in (0, 1) or not summary.exists():
        return Result(f"csvdiff (this project, {engine})", "bespoke", failed=classify(status, err))
    counts = json.loads(summary.read_text())["counts"]
    return Result(
        f"csvdiff (this project, {engine})", "bespoke", secs, mb,
        {"changed": counts["changed"], "added": counts["added"], "removed": counts["removed"]},
        note="cell-level diff, duplicate-key report, self-contained HTML",
        extra={"full": counts},
    )


def go_csvdiff(a: Path, b: Path, out: Path, exe: str) -> Result:
    """aswinkarthik/csvdiff — the fastest dedicated CSV diff tool in wide use.

    Hashes the key and the row with xxHash and keeps only the two hashes per row,
    which is why it is quick and why it can only say *that* a row changed, not
    which cell did.
    """
    marks = fresh(out / "go-csvdiff.csv")
    status, stdout, err, secs, mb = run([
        exe, str(a), str(b), "--primary-key", "0,1", "--ignore-columns", "19", "--format", "rowmark",
    ])
    if status != 0:
        return Result("csvdiff (Go, aswinkarthik)", "hash-only", failed=classify(status, err))
    marks.write_text(stdout)
    tally = {"changed": 0, "added": 0, "removed": 0}
    keys = {"changed": set(), "added": set(), "removed": set()}
    for line in stdout.splitlines():
        cells = line.rsplit(",", 1)
        if len(cells) != 2:
            continue
        bucket = {"MODIFIED": "changed", "ADDED": "added", "DELETED": "removed"}.get(cells[1])
        if bucket:
            tally[bucket] += 1
            keys[bucket].add(tuple(cells[0].split(",")[:2]))
    return Result(
        "csvdiff (Go, aswinkarthik)", "hash-only", secs, mb,
        {k: len(v) for k, v in keys.items()},
        note="row hash only: says a row changed, not which cell",
        extra={"rows_emitted": tally},
    )


def duckdb_sql(a: Path, b: Path, out: Path, exe: str) -> Result:
    """A full outer join written by hand in the DuckDB CLI — the SQL people reach for.

    This is the honest version of "just use SQL": it expresses the whole task,
    including the composite key and the ignored column, and it needs no code.
    What it does not give you is a cell-level diff or a duplicate-key report,
    and the join silently multiplies duplicate keys instead of flagging them.
    """
    compared = [c for c in HEADER if c != IGNORE and c not in KEY]
    on = " AND ".join(f"a.{k} IS NOT DISTINCT FROM b.{k}" for k in KEY)
    changed = " OR ".join(f"a.{c} IS DISTINCT FROM b.{c}" for c in compared)
    def load(name: str, path: Path) -> str:
        return (f"CREATE TABLE {name} AS SELECT * FROM read_csv('{path}', "
                "all_varchar = true, header = true, sample_size = -1);")
    sql = "\n".join([
        load("a", a), load("b", b),
        "SELECT",
        f"  count(*) FILTER (WHERE a.{KEY[0]} IS NOT NULL AND b.{KEY[0]} IS NOT NULL"
        f" AND ({changed})) AS changed,",
        f"  count(*) FILTER (WHERE a.{KEY[0]} IS NULL) AS added,",
        f"  count(*) FILTER (WHERE b.{KEY[0]} IS NULL) AS removed",
        f"FROM a FULL OUTER JOIN b ON {on};",
    ])
    status, stdout, err, secs, mb = run([exe, "-csv", "-c", sql])
    if status != 0:
        return Result("DuckDB CLI (hand-written SQL)", "SQL", failed=classify(status, err))
    rows = [r for r in stdout.strip().splitlines() if r and not r.startswith("changed")]
    values = [int(v) for v in rows[-1].split(",")] if rows else [0, 0, 0]
    (out / "duckdb-sql.txt").write_text(stdout)
    return Result(
        "DuckDB CLI (hand-written SQL)", "SQL", secs, mb,
        dict(zip(("changed", "added", "removed"), values)),
        note="counts only: no cell diff, no duplicate-key report, no report file",
    )


def daff_diff(a: Path, b: Path, out: Path, exe: str) -> Result:
    """daff — the tabular-diff library behind `git daff` and several CSV review tools.

    Alignment-based rather than key-based by default, but `--id` pins the key and
    `--ignore` drops a column, so it does express this task. Its output is a diff
    table, not a summary, so the counts here come from tallying the `@@` marks.
    """
    diff = fresh(out / "daff.csv")
    status, _, err, secs, mb = run([
        exe, "diff", "--unordered", "--output", str(diff), "--ignore", IGNORE,
        *[arg for k in KEY for arg in ("--id", k)], str(a), str(b),
    ])
    if status not in (0, 1) or not diff.exists():
        return Result("daff (JS)", "alignment diff", failed=classify(status, err))
    tally = {"changed": 0, "added": 0, "removed": 0}
    with diff.open() as fh:
        for line in fh:
            mark = line.split(",", 1)[0]
            if mark == "->":
                tally["changed"] += 1
            elif mark == "+++":
                tally["added"] += 1
            elif mark == "---":
                tally["removed"] += 1
    return Result(
        "daff (JS)", "alignment diff", secs, mb, tally,
        note="cell-level diff; no duplicate-key concept — a repeated key reads as an insert",
    )


def datacompy_compare(a: Path, b: Path, out: Path) -> Result:
    """datacompy (Capital One) — the reconciliation library, on pandas.

    Runs in a child process so its peak memory is measured the same way as
    everyone else's; the child is this same file under ``--child``.
    """
    summary = fresh(out / "datacompy.json")
    status, _, err, secs, mb = run([
        sys.executable, str(Path(__file__).resolve()), "--child", "datacompy",
        str(a), str(b), str(summary),
    ])
    if status != 0 or not summary.exists():
        return Result("datacompy (pandas)", "dataframe", failed=classify(status, err))
    return Result(
        "datacompy (pandas)", "dataframe", secs, mb, json.loads(summary.read_text()),
        note="cell-level diff and a per-column summary; whole frame in memory",
    )


def datacompy_polars(a: Path, b: Path, out: Path) -> Result:
    """datacompy again, this time over Polars rather than pandas."""
    summary = fresh(out / "datacompy-polars.json")
    status, _, err, secs, mb = run([
        sys.executable, str(Path(__file__).resolve()), "--child", "datacompy-polars",
        str(a), str(b), str(summary),
    ])
    if status != 0 or not summary.exists():
        return Result("datacompy (polars)", "dataframe", failed=classify(status, err))
    return Result(
        "datacompy (polars)", "dataframe", secs, mb, json.loads(summary.read_text()),
        note="same library, columnar backend",
    )


def csv_diff_tool(a: Path, b: Path, out: Path) -> Result:
    """csv-diff (Simon Willison) — the small, widely used Python one.

    It takes a single key column and has no way to ignore one, so it cannot be pointed at this
    task as it stands: keyed on account_id alone every row is ambiguous, and with updated_at
    still in the file every row is changed. The preprocessing that makes it usable — fuse the
    key into one column, drop the ignored one — is counted in its time, because a user reaching
    for this tool has to do it too.
    """
    summary = fresh(out / "csv-diff.json")
    status, _, err, secs, mb = run([
        sys.executable, str(Path(__file__).resolve()), "--child", "csv-diff",
        str(a), str(b), str(summary),
    ])
    if status != 0 or not summary.exists():
        return Result("csv-diff (Python)", "row dicts", failed=classify(status, err))
    return Result(
        "csv-diff (Python)", "row dicts", secs, mb, json.loads(summary.read_text()),
        note="single key column and no column-ignore, so the input has to be reshaped first",
        expresses_task=False,
    )


def unix_pipeline(a: Path, b: Path, out: Path) -> Result:
    """sort(1) and join(1) — the same algorithm as our sortmerge engine, in forty-year-old C.

    Included because it is the honest ceiling for an external sort-merge join, and because it
    is what a lot of people actually run. It gets three numbers, quickly, on data that happens
    to have no commas, quotes or newlines inside a field.
    """
    script = Path(__file__).resolve().parent / "unix_pipeline.sh"
    work = out / "unix-work"
    shutil.rmtree(work, ignore_errors=True)
    status, stdout, err, secs, mb = run(["bash", str(script), str(a), str(b), str(work)])
    shutil.rmtree(work, ignore_errors=True)
    if status != 0:
        return Result("sort(1) + join(1)", "shell pipeline", failed=classify(status, err))
    counts = dict(
        (k, int(v)) for k, v in
        (line.split("=", 1) for line in stdout.strip().splitlines() if "=" in line)
    )
    return Result(
        "sort(1) + join(1)", "shell pipeline", secs, mb, counts,
        note="counts only; no CSV quoting, no duplicate-key concept, no diff",
        expresses_task=False,
    )


def pandas_merge(a: Path, b: Path, out: Path) -> Result:
    """The hand-written pandas outer merge — what most people write before finding a library."""
    summary = fresh(out / "pandas.json")
    status, _, err, secs, mb = run([
        sys.executable, str(Path(__file__).resolve()), "--child", "pandas",
        str(a), str(b), str(summary),
    ])
    if status != 0 or not summary.exists():
        return Result("pandas (hand-written merge)", "dataframe", failed=classify(status, err))
    return Result(
        "pandas (hand-written merge)", "dataframe", secs, mb, json.loads(summary.read_text()),
        note="counts only unless you write more; duplicate keys multiply through the merge",
    )


def classify(status: int, err: str) -> str:
    """Turns a non-zero exit into the reason a reader of the table cares about."""
    if status == -1:
        return "timed out"
    noise = ("Picked up JAVA_TOOL_OPTIONS", "WARNING:", "SLF4J", "#")
    tail = [l for l in (err or "").strip().splitlines() if l and not l.startswith(noise)]
    last = tail[-1] if tail else f"exit {status}"
    for needle, reason in (
        ("Array buffer allocation failed", "out of memory"),
        ("JavaScript heap out of memory", "out of memory"),
        ("Cannot enlarge memory", "out of memory"),
        ("insufficient memory for the Java Runtime", "out of memory"),
        ("MemoryError", "out of memory"),
        ("Cannot allocate", "out of memory"),
        ("OutOfMemoryError", "out of memory"),
        ("Killed", "killed (out of memory)"),
        ("std::bad_alloc", "out of memory"),
    ):
        if needle in err:
            return reason
    if status == -9 or status == 137:
        return "killed (out of memory)"
    return last[:120]


# ---------------------------------------------------------------------------
# Child mode: one dataframe tool per run, so peak memory is its own
# ---------------------------------------------------------------------------

def child_datacompy(a: Path, b: Path, summary: Path) -> None:
    import datacompy
    import pandas as pd

    dtype = {c: "string" for c in HEADER}
    left, right = (pd.read_csv(p, dtype=dtype, keep_default_na=False).drop(columns=[IGNORE])
                   for p in (a, b))
    cmp = datacompy.PandasCompare(left, right, join_columns=KEY, df1_name="a", df2_name="b")
    _write_counts(summary, cmp)


def child_datacompy_polars(a: Path, b: Path, summary: Path) -> None:
    """The same library over Polars — the backend, not the design, is what changes."""
    import datacompy
    import polars as pl

    schema = {c: pl.String for c in HEADER}
    left, right = (pl.read_csv(p, schema=schema, has_header=True).drop(IGNORE)
                   for p in (a, b))
    cmp = datacompy.PolarsCompare(left, right, join_columns=KEY, df1_name="a", df2_name="b")
    _write_counts(summary, cmp)


def _write_counts(summary: Path, cmp: object) -> None:
    """Both comparators expose the same three numbers under the same names."""
    summary.write_text(json.dumps({
        "changed": int(len(cmp.all_mismatch())),
        "added": int(cmp.df2_unq_rows.shape[0]),
        "removed": int(cmp.df1_unq_rows.shape[0]),
    }))


def child_csv_diff(a: Path, b: Path, summary: Path) -> None:
    """Reshapes the input into what csv-diff can take, then runs it."""
    import csv_diff

    def reshape(path: Path) -> list[dict[str, str]]:
        rows = []
        with path.open(newline="", encoding="utf-8") as fh:
            for row in csv.DictReader(fh):
                row.pop(IGNORE, None)
                row["__key"] = "\x1e".join(row[k] for k in KEY)
                rows.append(row)
        return rows

    diff = csv_diff.compare(_keyed(reshape(a)), _keyed(reshape(b)))
    summary.write_text(json.dumps({
        "changed": len(diff["changed"]),
        "added": len(diff["added"]),
        "removed": len(diff["removed"]),
    }))


def _keyed(rows: list[dict[str, str]]) -> dict[str, dict[str, str]]:
    """The key-to-row mapping csv-diff's own loader builds, with our fused key."""
    return {row["__key"]: row for row in rows}


def child_pandas(a: Path, b: Path, summary: Path) -> None:
    import pandas as pd

    dtype = {c: "string" for c in HEADER}
    left, right = (pd.read_csv(p, dtype=dtype, keep_default_na=False).drop(columns=[IGNORE])
                   for p in (a, b))
    merged = left.merge(right, on=KEY, how="outer", indicator=True, suffixes=("_a", "_b"))
    compared = [c for c in HEADER if c != IGNORE and c not in KEY]
    both = merged["_merge"] == "both"
    differs = False
    for c in compared:
        differs = differs | (merged[f"{c}_a"].fillna("") != merged[f"{c}_b"].fillna(""))
    summary.write_text(json.dumps({
        "changed": int((both & differs).sum()),
        "added": int((merged["_merge"] == "right_only").sum()),
        "removed": int((merged["_merge"] == "left_only").sum()),
    }))


# ---------------------------------------------------------------------------
# Running the set
# ---------------------------------------------------------------------------

def warm(*paths: Path) -> None:
    """Reads the inputs once so nobody is timed reading them off disk.

    Every tool here reads the same two files, so whichever ran first would otherwise pay the
    page-cache miss for all of them — which is exactly how a four-second comparison came to be
    recorded as twenty-nine on a freshly booted machine.
    """
    for path in paths:
        with path.open("rb") as fh:
            while fh.read(1 << 22):
                pass


def repeat(make: "Callable[[], Result]", times: int) -> Result:
    """Runs a tool several times, keeping the median time and the largest memory seen.

    A single run on a shared four-core machine varies by a quarter, which is wider than most
    of the gaps in the table it feeds. The median of three is not a rigorous statistic but it
    is enough to stop one unlucky run from deciding a ranking.
    """
    runs = [make() for _ in range(max(1, times))]
    ok = [r for r in runs if not r.failed and r.seconds is not None]
    if not ok:
        return runs[-1]
    best = sorted(ok, key=lambda r: r.seconds)[len(ok) // 2]
    best.peak_mb = max(r.peak_mb for r in ok if r.peak_mb is not None)
    best.extra = {**best.extra, "seconds_all": [r.seconds for r in ok]}
    return best


def data_for(rows: str, data_dir: Path) -> tuple[Path, Path]:
    """Generates the pair once and reuses it, so every tool sees the same bytes."""
    a, b = data_dir / f"{rows}_a.csv", data_dir / f"{rows}_b.csv"
    if not (a.exists() and b.exists()):
        data_dir.mkdir(parents=True, exist_ok=True)
        subprocess.run([
            sys.executable, str(Path(__file__).resolve().parent / "gen_data.py"),
            "--rows", rows, "--out-dir", str(data_dir), "--prefix", rows,
        ], check=True)
    return a, b


def verdict(counts: dict[str, int], truth: Result) -> str:
    """Whether a tool agrees with us, and — when it does not — why.

    Every disagreement seen so far comes from one thing: a tool with no concept of
    a duplicate key reads the repeated row as an insert on one side and a delete on
    the other. Saying so is more useful than a bare "no", because it is a design
    difference rather than a wrong answer about which cells changed.
    """
    keys = ("changed", "added", "removed")
    delta = {k: counts.get(k, 0) - (truth.counts or {}).get(k, 0) for k in keys}
    if not any(delta.values()):
        return "yes"
    full = truth.extra.get("full", {})
    dup_a = full.get("a_dup_rows", 0) - full.get("a_dup_keys", 0)
    dup_b = full.get("b_dup_rows", 0) - full.get("b_dup_keys", 0)
    if delta["changed"] == 0 and delta["added"] == dup_b and delta["removed"] == dup_a:
        return "dup keys only"
    return "no (" + ", ".join(f"{k} {delta[k]:+d}" for k in keys if delta[k]) + ")"


def table(results: list[Result], truth: Result | None) -> str:
    """The comparison as a markdown table, agreement measured against our answer."""
    def cell(value: object) -> str:
        return "—" if value is None else str(value)

    lines = [
        "| Tool | Approach | Time | Peak RSS | changed / added / removed | Agrees | Notes |",
        "| --- | --- | ---: | ---: | --- | --- | --- |",
    ]
    for r in results:
        if r.failed:
            lines.append(f"| {r.tool} | {r.kind} | — | — | — | — | **{r.failed}** |")
            continue
        counts = r.counts or {}
        shown = " / ".join(f"{counts.get(k, 0):,}" for k in ("changed", "added", "removed"))
        if truth is None or truth.counts is None or r.counts is None:
            agrees = "—"
        elif r is truth:
            agrees = "reference"
        else:
            agrees = verdict(counts, truth)
        lines.append(
            f"| {r.tool} | {r.kind} | {r.seconds:.2f}s | {r.peak_mb:,.0f} MB | "
            f"{shown} | {agrees} | {r.note} |"
        )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rows", default="1m", help="10k, 200k, 1m, 10m")
    parser.add_argument("--data-dir", type=Path, default=Path("bench/external/data"))
    parser.add_argument("--out-dir", type=Path, default=Path("bench/external"))
    parser.add_argument("--jar", type=Path, default=Path("java/target/csvdiff.jar"))
    parser.add_argument("--tools-dir", type=Path, default=Path("bench/external/tools"))
    parser.add_argument("--engine", default="turbo", help="engine of ours to use as the reference")
    parser.add_argument("--skip", default="", help="comma-separated tool keys to skip")
    parser.add_argument("--repeats", type=int, default=3,
                        help="times to run each tool; the reported time is the median, the "
                             "reported memory the largest seen")
    parser.add_argument("--mem-cap-gb", type=float, default=None,
                        help="address space each tool may claim; without it the kernel "
                             "picks what to kill when a tool asks for more than the machine has")
    parser.add_argument("--child", nargs=4, metavar=("TOOL", "A", "B", "SUMMARY"),
                        help=argparse.SUPPRESS)
    args = parser.parse_args()

    if args.mem_cap_gb:
        global MEM_CAP_BYTES
        MEM_CAP_BYTES = int(args.mem_cap_gb * 1024**3)

    if args.child:
        tool, a, b, summary = args.child
        {"datacompy": child_datacompy, "datacompy-polars": child_datacompy_polars,
          "csv-diff": child_csv_diff, "pandas": child_pandas}[tool](
            Path(a), Path(b), Path(summary))
        return 0

    out = args.out_dir / args.rows
    out.mkdir(parents=True, exist_ok=True)
    a, b = data_for(args.rows, args.data_dir)
    warm(a, b)
    skip = {s.strip() for s in args.skip.split(",") if s.strip()}

    go_exe = shutil.which("csvdiff", path=str(args.tools_dir)) or shutil.which("csvdiff")
    duck_exe = shutil.which("duckdb", path=str(args.tools_dir)) or shutil.which("duckdb")
    daff_exe = (shutil.which("daff", path=str(args.tools_dir / "node_modules" / ".bin"))
                or shutil.which("daff"))

    plan: list[tuple[str, object]] = [
        ("ours", lambda: ours(a, b, out, args.engine, args.jar)),
        ("ours-sortmerge", lambda: ours(a, b, out, "sortmerge", args.jar)),
        ("go-csvdiff", (lambda: go_csvdiff(a, b, out, go_exe)) if go_exe else None),
        ("duckdb-sql", (lambda: duckdb_sql(a, b, out, duck_exe)) if duck_exe else None),
        ("daff", (lambda: daff_diff(a, b, out, daff_exe)) if daff_exe else None),
        ("datacompy", lambda: datacompy_compare(a, b, out)),
        ("datacompy-polars", lambda: datacompy_polars(a, b, out)),
        ("pandas", lambda: pandas_merge(a, b, out)),
        ("csv-diff", lambda: csv_diff_tool(a, b, out)),
        ("unix", lambda: unix_pipeline(a, b, out)),
    ]

    results: list[Result] = []
    for name, make in plan:
        if name in skip:
            continue
        if make is None:
            print(f"skipping {name}: not installed", file=sys.stderr)
            continue
        print(f"running {name} on {args.rows} ...", file=sys.stderr, flush=True)
        results.append(repeat(make, args.repeats))
        r = results[-1]
        print(f"  {r.failed or f'{r.seconds:.2f}s  {r.peak_mb:,.0f} MB  {r.counts}'}",
              file=sys.stderr, flush=True)

    truth = next((r for r in results if r.tool.startswith("csvdiff (this project")), None)
    rendered = table(results, truth)
    (out / "results.md").write_text(rendered + "\n")
    (out / "results.json").write_text(json.dumps(
        [r.__dict__ for r in results], indent=2, default=str) + "\n")
    print(f"\n### {args.rows}\n")
    print(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
