#!/usr/bin/env node
/**
 * csvdiff-ts command line.
 *
 *   csvdiff-ts compare A.csv B.csv --key id,region [--compare c1,c2] [--out report.html]
 *   csvdiff-ts compare A.csv B.csv --profile orders
 *   csvdiff-ts serve [--port 8765]          # drag-and-drop page
 *
 * Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (only with --fail-on-dups).
 */
import { spawn } from "node:child_process";
import { once } from "node:events";
import { realpathSync, writeFileSync } from "node:fs";
import { basename, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs, type ParseArgsOptionsConfig } from "node:util";

import { loadConfig, optionsFrom, parseList } from "./config.ts";
import { compare, CompareError, isIdentical } from "./engine.ts";
import { render } from "./report.ts";
import type { EngineName } from "./types.ts";

export const USAGE = `usage: csvdiff-ts <command> [options]

commands:
  compare A B     Compare two CSV files on a composite key and write an HTML report
  serve           Run the drag-and-drop web page (default http://127.0.0.1:8765)

compare options:
  -k, --key COLS          Comma-separated key column(s) (or from --profile)
  -c, --compare COLS      Columns to compare (default: all common non-key columns)
  -i, --ignore COLS       Columns to skip
  -p, --profile NAME      Profile name from csvdiff.toml
      --config PATH       Path to csvdiff.toml
      --trim              Strip whitespace before comparing
      --ignore-case
      --empty-is-null     Treat empty string and null as equal
      --tolerance N       Absolute numeric tolerance
      --max-rows N        Rows embedded per section (default 50000)
      --delimiter D       Force delimiter (default: auto)
      --encoding ENC
      --engine E          auto | duckdb | native
      --threads N
      --memory-limit S    DuckDB memory limit, e.g. 4GB
      --export-dir DIR    Write full changed/added/removed CSVs here
  -o, --out PATH          Report path (default: <a>__vs__<b>.html)
      --json PATH         Also write a JSON summary (counts + column stats) here
      --no-compress       Embed plain JSON instead of gzip (older browsers)
      --open              Open the report in the default browser
      --fail-on-dups      Exit 3 when either file has duplicate keys

serve options:
      --host HOST         default 127.0.0.1
      --port PORT         default 8765
      --config PATH       csvdiff.toml (profiles offered in the page)

Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (with --fail-on-dups).
`;

const COMPARE_OPTIONS = {
  key: { type: "string", short: "k" },
  compare: { type: "string", short: "c" },
  ignore: { type: "string", short: "i" },
  profile: { type: "string", short: "p" },
  config: { type: "string" },
  trim: { type: "boolean" },
  "ignore-case": { type: "boolean" },
  "empty-is-null": { type: "boolean" },
  tolerance: { type: "string" },
  "max-rows": { type: "string" },
  delimiter: { type: "string" },
  encoding: { type: "string" },
  engine: { type: "string" },
  threads: { type: "string" },
  "memory-limit": { type: "string" },
  "export-dir": { type: "string" },
  out: { type: "string", short: "o" },
  json: { type: "string" },
  "no-compress": { type: "boolean" },
  open: { type: "boolean" },
  "fail-on-dups": { type: "boolean" },
  help: { type: "boolean", short: "h" },
} satisfies ParseArgsOptionsConfig;

const SERVE_OPTIONS = {
  host: { type: "string", default: "127.0.0.1" },
  port: { type: "string", default: "8765" },
  config: { type: "string" },
  help: { type: "boolean", short: "h" },
} satisfies ParseArgsOptionsConfig;

const ENGINES: readonly EngineName[] = ["auto", "duckdb", "native"];

function numOpt(v: string | undefined, name: string, integer = false): number | null {
  if (v === undefined) return null;
  const n = integer ? Number.parseInt(v, 10) : Number(v);
  if (Number.isNaN(n)) throw new CompareError(`--${name} must be a number, got: ${v}`);
  return n;
}

async function cmdCompare(args: string[]): Promise<number> {
  const { values, positionals } = parseArgs({ args, options: COMPARE_OPTIONS, allowPositionals: true });
  if (values.help) {
    console.log(USAGE);
    return 0;
  }
  const [a, b] = positionals;
  if (!a || !b) throw new CompareError("compare needs two files: csvdiff-ts compare A.csv B.csv --key ...");

  const cfg = loadConfig(values.config);
  const profile = values.profile ? (cfg.profiles ?? {})[values.profile] : undefined;
  if (values.profile && profile === undefined) throw new CompareError(`Profile not found: ${values.profile}`);
  if (values.engine !== undefined && !ENGINES.includes(values.engine as EngineName)) {
    throw new CompareError(`--engine must be one of ${ENGINES.join(", ")}`);
  }
  const overrides = {
    key: parseList(values.key),
    compare: parseList(values.compare),
    ignore: parseList(values.ignore),
    trim: values.trim ?? null,
    ignore_case: values["ignore-case"] ?? null,
    empty_is_null: values["empty-is-null"] ?? null,
    tolerance: numOpt(values.tolerance, "tolerance"),
    max_rows: numOpt(values["max-rows"], "max-rows", true),
    delimiter: values.delimiter ?? null,
    encoding: values.encoding ?? null,
    engine: (values.engine as EngineName | undefined) ?? null,
    threads: numOpt(values.threads, "threads", true),
    memory_limit: values["memory-limit"] ?? null,
    export_dir: values["export-dir"] ?? null,
  };
  if (!(profile && profile.key && profile.key.length) && !overrides.key) {
    throw new CompareError("--key (or a profile with key) is required");
  }
  const opt = optionsFrom(profile, overrides);

  const result = await compare(a, b, opt);
  const stem = (p: string) => basename(p, extname(p));
  const out = values.out ?? `${stem(a)}__vs__${stem(b)}.html`;
  writeFileSync(out, render(result, !values["no-compress"]), "utf-8");
  if (values.json) {
    const { meta, counts, columns } = result;
    writeFileSync(values.json, JSON.stringify({ meta, counts, columns }, null, 2), "utf-8");
  }
  const c = result.counts;
  const f = (n: number) => n.toLocaleString("en-US");
  console.log(
    `A ${f(c.a_rows)} rows | B ${f(c.b_rows)} rows | matched ${f(c.matched)} ` +
      `(changed ${f(c.changed)}) | added ${f(c.added)} | removed ${f(c.removed)} | ` +
      `dup keys A ${f(c.a_dup_keys)} B ${f(c.b_dup_keys)} | ${result.meta.engine} ${result.meta.seconds}s`,
  );
  const { statSync } = await import("node:fs");
  console.log(`Report: ${out} (${Math.round(statSync(out).size / 1024)} KB)`);
  if (values.open) openInBrowser(resolve(out));
  if (values["fail-on-dups"] && (c.a_dup_keys || c.b_dup_keys)) return 3;
  return isIdentical(result) ? 0 : 1;
}

async function cmdServe(args: string[]): Promise<number> {
  const { values } = parseArgs({ args, options: SERVE_OPTIONS });
  if (values.help) {
    console.log(USAGE);
    return 0;
  }
  const port = numOpt(values.port, "port", true) ?? 8765;
  const { serve } = await import("./server.ts");
  const server = serve(values.host, port, values.config);
  await once(server, "close");
  return 0;
}

function openInBrowser(path: string): void {
  const [cmd, args] =
    process.platform === "darwin"
      ? ["open", [path]]
      : process.platform === "win32"
        ? ["cmd", ["/c", "start", "", path]]
        : ["xdg-open", [path]];
  try {
    spawn(cmd, args, { stdio: "ignore", detached: true }).on("error", () => {}).unref();
  } catch {
    /* best effort */
  }
}

export async function main(argv: string[] = process.argv.slice(2)): Promise<number> {
  const cmd = argv[0];
  try {
    if (!cmd || cmd === "-h" || cmd === "--help") {
      console.log(USAGE);
      return cmd ? 0 : 2;
    }
    if (cmd === "compare") return await cmdCompare(argv.slice(1));
    if (cmd === "serve") return await cmdServe(argv.slice(1));
    if (cmd === "mail") {
      throw new CompareError(
        "the mail launcher is not part of the TypeScript port; use the Python implementation (`csvdiff mail`).",
      );
    }
    throw new CompareError(`Unknown command: ${cmd}\n\n${USAGE}`);
  } catch (e) {
    if (e instanceof Error) {
      console.error(`error: ${e.message}`);
      return 2;
    }
    throw e;
  } finally {
    if (process.env.CSVDIFF_PRINT_PEAK_RSS) {
      // maxRSS is reported in kilobytes on every platform (unlike getrusage on macOS)
      console.error(`PEAK_RSS_KB ${process.resourceUsage().maxRSS}`);
    }
  }
}

function realpathSafe(p: string): string {
  try {
    return realpathSync(p);
  } catch {
    return p;
  }
}

if (process.argv[1] && realpathSafe(process.argv[1]) === realpathSafe(fileURLToPath(import.meta.url))) {
  main().then((rc) => {
    process.exitCode = rc;
  });
}
