#!/usr/bin/env node
/**
 * Benchmark one scale end to end and record the numbers.
 *
 *   node scripts/bench.ts --rows 1m --engine duckdb --out-dir bench
 *
 * Runs the comparison in a child process so peak RSS is measured honestly,
 * writes bench/<scale>-<engine>.json, appends a row to $GITHUB_STEP_SUMMARY
 * when running in Actions, and exits non-zero if a time or memory budget is
 * exceeded. Same budgets and output columns as scripts/bench.py.
 */
import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, realpathSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import os from "node:os";
import { join } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { genData, parseRows } from "./gen-data.ts";

// Budgets on a 4-vCPU / 16 GB GitHub-hosted runner. Generous by ~2x so the gate
// catches real regressions, not noise.
const BUDGETS: [rows: number, seconds: number, peakMb: number][] = [
  [10_000, 20, 1_500],
  [1_000_000, 120, 6_000],
  [10_000_000, 900, 12_000],
];
const KEY = "account_id,txn_id";
const IGNORE = "updated_at";

function budgetFor(rows: number): [number, number] {
  for (const [n, s, mb] of BUDGETS) if (rows <= n) return [s, mb];
  const [, s, mb] = BUDGETS[BUDGETS.length - 1]!;
  return [s, mb];
}

const round1 = (x: number) => Math.round(x * 10) / 10;
const round2 = (x: number) => Math.round(x * 100) / 100;
const f = (n: number) => n.toLocaleString("en-US");

/** Turn a dead child process into a short, honest reason for the results table. */
function classifyFailure(status: number | null, signal: string | null, stderr: string): string {
  if (signal === "SIGKILL" || status === 137) return "OOM killed";
  if (/JavaScript heap out of memory|Allocation failed|Reached heap limit/i.test(stderr)) return "JS heap OOM";
  // V8 caps a single string at 0x1fffffe8 characters (~512 MB). The in-heap JS
  // engines read the whole file into one string, so they hit this well before
  // memory runs out — it is a hard ceiling, not a tuning problem.
  if (/Cannot create a string longer than|Invalid string length/i.test(stderr)) return "file > V8 512MB string cap";
  if (/Array buffer allocation failed|Invalid array length/i.test(stderr)) return "exceeds V8 limit";
  if (/No space left on device|ENOSPC/i.test(stderr)) return "disk full";
  if (/Out of Memory Error|failed to allocate/i.test(stderr)) return "engine OOM";
  if (signal) return `killed by ${signal}`;
  return `exit ${status ?? "?"}`;
}

function appendSummary(line: string): void {
  const step = process.env.GITHUB_STEP_SUMMARY;
  if (!step) return;
  if (!existsSync(step) || statSync(step).size === 0) {
    appendFileSync(
      step,
      "| scale | engine | input | generate | compare | throughput | peak RSS | " +
        "report | changed | added | removed | budget |\n" +
        "|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
  }
  appendFileSync(step, line + "\n");
}

async function main(): Promise<number> {
  const { values: ns } = parseArgs({
    options: {
      rows: { type: "string", short: "n", default: "10k" },
      engine: { type: "string", default: "auto" },
      "out-dir": { type: "string", short: "o", default: "bench" },
      "data-dir": { type: "string", default: "data" },
      "keep-data": { type: "boolean", default: false },
      threads: { type: "string" },
      "memory-limit": { type: "string" },
      "no-budget": { type: "boolean", default: false },
      "heap-mb": { type: "string" },
      "allow-failure": { type: "boolean", default: false },
    },
  });

  const rows = parseRows(ns.rows);
  const label = ns.rows.toLowerCase();
  mkdirSync(ns["out-dir"], { recursive: true });
  mkdirSync(ns["data-dir"], { recursive: true });
  const a = join(ns["data-dir"], `${label}_a.csv`);
  const b = join(ns["data-dir"], `${label}_b.csv`);

  let genSeconds = 0;
  if (!(existsSync(a) && existsSync(b))) {
    const t0 = performance.now();
    genData(rows, a, b);
    genSeconds = round1((performance.now() - t0) / 1000);
    console.log(`typescript: generated ${f(rows)} rows x 20 columns in ${genSeconds}s`);
  }

  const report = join(ns["out-dir"], `${label}-${ns.engine}.html`);
  const summary = join(ns["out-dir"], `${label}-${ns.engine}-summary.json`);
  const here = fileURLToPath(import.meta.url);
  const cli = join(here, "..", "..", "src", here.endsWith(".ts") ? "cli.ts" : "cli.js");
  const nodeArgs = ns["heap-mb"] ? [`--max-old-space-size=${Number.parseInt(ns["heap-mb"], 10)}`] : [];
  const args = [...nodeArgs, cli, "compare", a, b, "-k", KEY, "-i", IGNORE, "--engine", ns.engine, "-o", report, "--json", summary];
  if (ns.threads) args.push("--threads", ns.threads);
  if (ns["memory-limit"]) args.push("--memory-limit", ns["memory-limit"]);

  const t0 = performance.now();
  const proc = spawnSync(process.execPath, args, {
    encoding: "utf-8",
    env: { ...process.env, CSVDIFF_PRINT_PEAK_RSS: "1" },
    maxBuffer: 1 << 28,
  });
  const wall = round2((performance.now() - t0) / 1000);
  process.stdout.write(proc.stdout ?? "");
  process.stderr.write(proc.stderr ?? "");

  if (proc.status !== 0 && proc.status !== 1) {
    // An engine that cannot handle this scale is a benchmark result, not a
    // harness error: record why it failed so the comparison table can show it.
    const why = classifyFailure(proc.status, proc.signal, proc.stderr ?? "");
    const rec = {
      rows, scale: label, engine: ns.engine, generate_seconds: genSeconds,
      compare_seconds: null, peak_rss_mb: null,
      input_mb: round1((statSync(a).size + statSync(b).size) / 1e6),
      report_mb: null, rows_per_second: null, counts: null,
      failed: why, exit_status: proc.status, signal: proc.signal ?? null,
      wall_before_failure: wall,
      runner: `${os.type()} ${os.arch()} node${process.versions.node} ${os.availableParallelism()} cpu`,
    };
    writeFileSync(join(ns["out-dir"], `${label}-${ns.engine}.json`), JSON.stringify(rec, null, 2));
    const failLine =
      `| ${label} | ${ns.engine} | ${f(rec.input_mb)} MB | ${genSeconds}s | **${why}** after ${wall}s | - | - | - | - | - | - | failed |`;
    appendSummary(failLine);
    console.log(failLine);
    console.error(`compare exited with ${proc.status ?? proc.signal}: ${why}`);
    if (!ns["keep-data"]) for (const p of [a, b]) if (existsSync(p)) unlinkSync(p);
    return ns["allow-failure"] ? 0 : 2;
  }

  // The child reports its own peak; process.resourceUsage().maxRSS is KB everywhere.
  const peakLine = (proc.stderr ?? "").split("\n").find((l) => l.startsWith("PEAK_RSS_KB"));
  const peakKb = peakLine ? Number.parseInt(peakLine.split(/\s+/)[1] ?? "0", 10) : 0;
  const peakMb = round1(peakKb / 1024);

  const counts = JSON.parse(readFileSync(summary, "utf-8")).counts as Record<string, number>;
  const rec = {
    rows,
    scale: label,
    engine: ns.engine,
    generate_seconds: genSeconds,
    compare_seconds: wall,
    peak_rss_mb: peakMb,
    input_mb: round1((statSync(a).size + statSync(b).size) / 1e6),
    report_mb: round2(statSync(report).size / 1e6),
    rows_per_second: wall ? Math.floor((counts.a_rows! + counts.b_rows!) / wall) : null,
    counts,
    runner: `${os.type()} ${os.arch()} node${process.versions.node} ${os.availableParallelism()} cpu`,
  };
  writeFileSync(join(ns["out-dir"], `${label}-${ns.engine}.json`), JSON.stringify(rec, null, 2));

  const [budgetS, budgetMb] = budgetFor(rows);
  const ok = wall <= budgetS && peakMb <= budgetMb;
  const line =
    `| ${label} | ${ns.engine} | ${f(rec.input_mb)} MB | ${genSeconds}s | **${wall}s** | ` +
    `${rec.rows_per_second === null ? "-" : f(rec.rows_per_second)}/s | ${f(peakMb)} MB | ${rec.report_mb} MB | ` +
    `${f(counts.changed!)} | ${f(counts.added!)} | ${f(counts.removed!)} | ` +
    `${ok ? "pass" : `over budget (${budgetS}s / ${budgetMb} MB)`} |`;
  appendSummary(line);
  console.log(line);

  if (!ns["keep-data"]) {
    for (const p of [a, b]) if (existsSync(p)) unlinkSync(p);
  }
  return ok || ns["no-budget"] ? 0 : 1;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === realpathSync(process.argv[1])) {
  process.exitCode = await main();
}
