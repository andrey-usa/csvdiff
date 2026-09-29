#!/usr/bin/env python3
"""Every port, every format, the same answer to every option combination.

    scripts/option_matrix.py                 # c,cpp,rust,zig on csv, ndjson and parquet
    scripts/option_matrix.py --ports c,zig   # a subset; the first is the reference

`scripts/fuzz_diff.py` varies the *input* and keeps the options simple. This
varies the *options* and keeps the input simple: one generated pair per format
(from c/gen-data, so every format holds the same rows), and a fixed table of
invocations covering what the fuzzer never passes -- --compare, --ignore and
--key together, overlapping, and misspelled.

That is where the ports had drifted apart. Before it existed, on the same pair:
  * `-c amount,updated_at -i updated_at` gave 317 changed rows in C and Rust and
    19,980 in C++ and Zig, which compared the column they were told to ignore;
  * a misspelled `-i` on Parquet was refused by C and Rust and accepted by C++
    and Zig, which then compared the column it meant to drop.

Per invocation and format, every port must give the same exit status, and where
that status is 0 or 1 the same `counts` and `columns`, and a document that
passes scripts/contract.py. A refusal (exit 2) must also say which column or
file it is about, the one thing a user needs from it.

Exit status: 0 all agree, 1 at least one disagreement, 2 usage.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import contract  # noqa: E402
from fuzz_diff import PORTS  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
GEN = ROOT / "c" / ("gen-data.exe" if os.name == "nt" else "gen-data")
KEY = "account_id,txn_id"

# (label, flags, the name a refusal must mention -- or None where it succeeds)
CASES: list[tuple[str, list[str], str | None]] = [
    ("defaults", ["-k", KEY], None),
    ("ignore one", ["-k", KEY, "-i", "updated_at"], None),
    ("ignore several", ["-k", KEY, "-i", "updated_at,fee,note"], None),
    ("compare explicit", ["-k", KEY, "-c", "amount,status"], None),
    ("compare repeats a name", ["-k", KEY, "-c", "amount,amount"], None),
    ("compare names a key", ["-k", KEY, "-c", "account_id,amount"], None),
    ("compare names an ignored column", ["-k", KEY, "-c", "amount,updated_at", "-i", "updated_at"], None),
    ("compare only ignored", ["-k", KEY, "-c", "updated_at", "-i", "updated_at"], None),
    ("ignore a key column", ["-k", KEY, "-i", "account_id"], None),
    ("single-column key", ["-k", "txn_id"], None),
    ("key order reversed", ["-k", "txn_id,account_id"], None),
    ("unknown key", ["-k", "no_such_key"], "no_such_key"),
    ("unknown key in a list", ["-k", "account_id,no_such_key"], "no_such_key"),
    ("unknown ignore", ["-k", KEY, "-i", "no_such_ignore"], "no_such_ignore"),
    ("unknown compare", ["-k", KEY, "-c", "amount,no_such_compare"], "no_such_compare"),
]


def run(port: str, a: Path, b: Path, flags: list[str], work: Path):
    """(exit status, contract slice, stderr, contract problems)."""
    doc = work / f"{port}.json"
    doc.unlink(missing_ok=True)
    extra = ["-o", os.devnull] if port == "rust" else []  # Rust alone writes a report by default
    proc = subprocess.run([str(PORTS[port]), "compare", str(a), str(b), *flags, "--json", str(doc), *extra],
                          capture_output=True, timeout=120)
    got, problems = None, []
    if proc.returncode <= 1:
        try:
            d = json.loads(doc.read_text())
            got = {k: d.get(k) for k in ("counts", "columns")}
            problems = contract.check(d)
        except (OSError, ValueError) as err:
            problems = [f"no readable --json document: {err}"]
    return proc.returncode, got, proc.stderr.decode(errors="replace").strip(), problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--ports", default="c,cpp,rust,zig")
    ap.add_argument("--rows", default="2k")
    args = ap.parse_args()
    ports = [p for p in args.ports.split(",") if p]
    missing = [p for p in ports if p not in PORTS or not PORTS[p].exists()]
    if missing or len(ports) < 2 or not GEN.exists():
        print(f"need two or more built ports and c/gen-data; missing: {missing or 'gen-data'}", file=sys.stderr)
        return 2

    bad = 0
    total = 0
    with tempfile.TemporaryDirectory(prefix="option-matrix-") as tmp:
        work = Path(tmp)
        pairs = {}
        for fmt, ext in (("csv", "csv"), ("ndjson", "ndjson"), ("parquet", "unc.parquet")):
            subprocess.run([str(GEN), "--rows", args.rows, "--out-dir", str(work), "--prefix", "m",
                            "--format", fmt], check=True, capture_output=True)
            pairs[fmt] = (work / f"m_a.{ext}", work / f"m_b.{ext}")

        for fmt, (a, b) in pairs.items():
            for label, flags, must_name in CASES:
                total += 1
                results = {p: run(p, a, b, flags, work) for p in ports}
                ref_port = ports[0]
                ref = results[ref_port]
                problems = []
                for p, (code, got, err, contract_problems) in results.items():
                    problems += [f"{p}: {c}" for c in contract_problems]
                    if code != ref[0]:
                        problems.append(f"{p} exits {code}, {ref_port} exits {ref[0]}"
                                        + (f" ({err.splitlines()[0] if err else 'no message'})" if code > 1 else ""))
                    elif code <= 1 and got != ref[1]:
                        problems.append(f"{p} counts/columns differ from {ref_port}: {got} vs {ref[1]}")
                    if must_name and code == 2 and must_name not in err:
                        problems.append(f"{p} refuses without naming {must_name!r}: {err.splitlines()[0] if err else '(nothing)'}")
                    if must_name and code != 2:
                        problems.append(f"{p} accepts {' '.join(flags)} (exit {code}); it must refuse")
                status = "ok  " if not problems else "FAIL"
                print(f"{status} {fmt:8} {label}")
                for line in problems:
                    print(f"       {line}")
                    if os.environ.get("GITHUB_ACTIONS"):
                        print(f"::error::{fmt} / {label}: {line}")
                bad += bool(problems)
    print(f"{total - bad}/{total} agree across {', '.join(ports)}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
