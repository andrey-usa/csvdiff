"""csvdiff command line.

  csvdiff compare A.csv B.csv --key id,region [--compare c1,c2] [--out report.html]
  csvdiff compare A.csv B.csv --profile orders
  csvdiff serve [--port 8765]          # drag-and-drop page
  csvdiff mail  [--once]               # mailbox watcher

Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (only with --fail-on-dups).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import webbrowser

from .config import load_config, options_from, parse_list
from .engine import CompareError, compare, is_identical
from .report import render


def _add_compare_options(p: argparse.ArgumentParser):
    p.add_argument("--key", "-k", help="Comma-separated key column(s)")
    p.add_argument("--compare", "-c", help="Comma-separated columns to compare (default: all common non-key columns)")
    p.add_argument("--ignore", "-i", help="Comma-separated columns to skip")
    p.add_argument("--profile", "-p", help="Profile name from csvdiff.toml")
    p.add_argument("--config", help="Path to csvdiff.toml")
    p.add_argument("--trim", action="store_true", default=None, help="Strip whitespace before comparing")
    p.add_argument("--ignore-case", action="store_true", default=None)
    p.add_argument("--empty-is-null", action="store_true", default=None, help="Treat empty string and null as equal")
    p.add_argument("--tolerance", type=float, help="Absolute numeric tolerance")
    p.add_argument("--max-rows", type=int, help="Rows embedded per section (default 50000)")
    p.add_argument("--delimiter", help="Force delimiter (default: auto)")
    p.add_argument("--encoding")
    p.add_argument("--engine", choices=["auto", "duckdb", "pandas"])
    p.add_argument("--threads", type=int)
    p.add_argument("--memory-limit", help="DuckDB memory limit, e.g. 4GB")
    p.add_argument("--export-dir", help="Write full changed/added/removed CSVs here")


def build_options(ns: argparse.Namespace):
    cfg = load_config(ns.config)
    profile = cfg.get("profiles", {}).get(ns.profile) if ns.profile else None
    if ns.profile and profile is None:
        raise CompareError(f"Profile not found: {ns.profile}")
    overrides = {
        "key": parse_list(ns.key), "compare": parse_list(ns.compare), "ignore": parse_list(ns.ignore),
        "trim": ns.trim, "ignore_case": ns.ignore_case, "empty_is_null": ns.empty_is_null,
        "tolerance": ns.tolerance, "max_rows": ns.max_rows, "delimiter": ns.delimiter,
        "encoding": ns.encoding, "engine": ns.engine, "threads": ns.threads,
        "memory_limit": ns.memory_limit, "export_dir": ns.export_dir,
    }
    if not (profile and profile.get("key")) and not overrides["key"]:
        raise CompareError("--key (or a profile with key) is required")
    return options_from(profile, overrides)


def cmd_compare(ns: argparse.Namespace) -> int:
    opt = build_options(ns)
    result = compare(ns.a, ns.b, opt)
    out = ns.out or f"{os.path.splitext(os.path.basename(ns.a))[0]}__vs__{os.path.splitext(os.path.basename(ns.b))[0]}.html"
    with open(out, "w", encoding="utf-8") as f:
        f.write(render(result, compress=not ns.no_compress))
    if ns.json:
        with open(ns.json, "w", encoding="utf-8") as f:
            json.dump({k: result[k] for k in ("meta", "counts", "columns")}, f, indent=2, default=str)
    c = result["counts"]
    print(f"A {c['a_rows']:,} rows | B {c['b_rows']:,} rows | matched {c['matched']:,} "
          f"(changed {c['changed']:,}) | added {c['added']:,} | removed {c['removed']:,} | "
          f"dup keys A {c['a_dup_keys']:,} B {c['b_dup_keys']:,} | {result['meta']['engine']} {result['meta']['seconds']}s")
    print(f"Report: {out} ({os.path.getsize(out) / 1024:.0f} KB)")
    if ns.open:
        webbrowser.open("file://" + os.path.abspath(out))
    if ns.fail_on_dups and (c["a_dup_keys"] or c["b_dup_keys"]):
        return 3
    return 0 if is_identical(result) else 1


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="csvdiff", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("compare", help="Compare two CSV files")
    p.add_argument("a"), p.add_argument("b")
    _add_compare_options(p)
    p.add_argument("--out", "-o", help="Report path (default: <a>__vs__<b>.html)")
    p.add_argument("--json", help="Also write a JSON summary (counts + column stats) here")
    p.add_argument("--no-compress", action="store_true", help="Embed plain JSON instead of gzip (older browsers)")
    p.add_argument("--open", action="store_true", help="Open the report in the default browser")
    p.add_argument("--fail-on-dups", action="store_true", help="Exit 3 when either file has duplicate keys")
    p.set_defaults(fn=cmd_compare)

    p = sub.add_parser("serve", help="Run the drag-and-drop web page")
    p.add_argument("--host", default="127.0.0.1"), p.add_argument("--port", type=int, default=8765)
    p.add_argument("--config", help="Path to csvdiff.toml (profiles offered in the page)")
    p.set_defaults(fn=lambda ns: __import__("csvdiff.server", fromlist=["serve"]).serve(ns.host, ns.port, ns.config))

    p = sub.add_parser("mail", help="Watch a mailbox and reply with reports")
    p.add_argument("--config", help="Path to csvdiff.toml ([mail] section + profiles)")
    p.add_argument("--once", action="store_true", help="Process the inbox once and exit")
    p.set_defaults(fn=lambda ns: __import__("csvdiff.mailbot", fromlist=["run"]).run(ns.config, ns.once))

    ns = ap.parse_args(argv)
    try:
        return ns.fn(ns) or 0
    except CompareError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
