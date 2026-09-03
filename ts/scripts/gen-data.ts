#!/usr/bin/env node
/**
 * Generate a deterministic pair of CSV files for benchmarking and CI.
 *
 * Both files have 20 columns and share the composite key (account_id, txn_id).
 * File B is file A with a controlled amount of drift applied:
 *
 *   status        changes on ~3.0% of rows
 *   amount        changes on ~1.5% of rows
 *   balance       changes on ~1.5% of rows
 *   value_date    blanked  on ~0.3% of rows
 *   updated_at    changes on every row (so `--ignore updated_at` has something to do)
 *   removed       ~0.10% of rows are absent from B
 *   added         ~0.10% extra rows only in B
 *   duplicate key ~0.01% of rows are emitted twice in B, and a few in A
 *
 * Usage:
 *   node scripts/gen-data.ts --rows 1m --out-dir data
 *   node scripts/gen-data.ts --rows 10m --out-dir data --engine duckdb
 *
 * The DuckDB path is the same SQL as the Python generator, so its files are
 * byte-identical to `scripts/gen_data.py` on the same DuckDB version. The
 * pure-TS path streams both files in one pass and needs no dependencies.
 */
import { closeSync, mkdirSync, openSync, realpathSync, statSync, writeSync } from "node:fs";
import { join } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

export const COLUMNS = [
  "account_id", "txn_id", "posting_date", "value_date", "currency", "amount", "fee",
  "balance", "status", "channel", "region", "branch_code", "product_code", "counterparty",
  "quantity", "rate", "category", "risk_flag", "note", "updated_at",
];

const STATUS = ["posted", "pending", "settled", "reversed"];
const CHANNEL = ["branch", "online", "mobile", "atm", "wire"];
const REGION = ["EMEA", "NA", "APAC", "LATAM"];
const CURRENCY = ["USD", "EUR", "GBP", "JPY"];
const CATEGORY = ["retail", "corporate", "treasury", "cards", "loans"];

// Drift buckets, expressed against a 0..9999 hash bucket per row.
const CHG_STATUS = 300, CHG_AMOUNT = 150, CHG_BALANCE = 150, CHG_VALUE_DATE = 30; // 3%, 1.5%, 1.5%, 0.3%
const REMOVED_MOD = 1000, ADDED_RATIO = 1000, DUP_MOD = 10000;

export type GenEngine = "auto" | "duckdb" | "native";

// ---------------------------------------------------------------------------
// DuckDB path
// ---------------------------------------------------------------------------

function sqlExpressions(side: "a" | "b"): string {
  // hash() returns UBIGINT; list indexing needs BIGINT, hence the cast.
  const pick = (lst: string[], salt: number) =>
    `([${lst.map((v) => `'${v}'`).join(", ")}])[((hash(i * 31 + ${salt}) % ${lst.length}) + 1)::BIGINT]`;

  const b = side === "b";
  const status = pick(STATUS, 11);
  const statusB =
    `([${STATUS.map((v) => `'${v}'`).join(", ")}])` +
    `[(((hash(i * 31 + 11) % ${STATUS.length}) + 1) % ${STATUS.length} + 1)::BIGINT]`;
  const amount = "round(((hash(i * 31 + 21) % 900000000) / 100.0) - 1000000, 2)";
  const balance = "round(((hash(i * 31 + 31) % 2000000000) / 100.0), 2)";
  const valueDate = "strftime(DATE '2026-01-01' + to_days((hash(i * 31 + 41) % 240)::INT), '%Y-%m-%d')";
  return `
      'ACC-' || lpad(((i * 7919) % 250000)::VARCHAR, 8, '0')                        AS account_id,
      'TXN-' || lpad(i::VARCHAR, 11, '0')                                           AS txn_id,
      strftime(DATE '2026-01-01' + to_days((hash(i * 31 + 1) % 240)::INT), '%Y-%m-%d') AS posting_date,
      ${b ? `CASE WHEN bucket < ${CHG_VALUE_DATE} THEN '' ELSE ${valueDate} END` : valueDate} AS value_date,
      ${pick(CURRENCY, 51)}                                                          AS currency,
      ${b ? `CASE WHEN bucket >= ${CHG_STATUS} AND bucket < ${CHG_STATUS + CHG_AMOUNT} THEN round(${amount} + 12.34, 2) ELSE ${amount} END` : amount} AS amount,
      round(((hash(i * 31 + 61) % 5000) / 100.0), 2)                                AS fee,
      ${b ? `CASE WHEN bucket >= ${CHG_STATUS + CHG_AMOUNT} AND bucket < ${CHG_STATUS + CHG_AMOUNT + CHG_BALANCE} THEN round(${balance} * 1.01, 2) ELSE ${balance} END` : balance} AS balance,
      ${b ? `CASE WHEN bucket < ${CHG_STATUS} THEN ${statusB} ELSE ${status} END` : status} AS status,
      ${pick(CHANNEL, 71)}                                                           AS channel,
      ${pick(REGION, 81)}                                                            AS region,
      'BR' || lpad(((hash(i * 31 + 91) % 900) + 100)::VARCHAR, 4, '0')              AS branch_code,
      'P' || lpad((hash(i * 31 + 101) % 5000)::VARCHAR, 5, '0')                     AS product_code,
      'CP-' || lpad((hash(i * 31 + 111) % 90000)::VARCHAR, 6, '0')                  AS counterparty,
      (hash(i * 31 + 121) % 500) + 1                                                AS quantity,
      round(((hash(i * 31 + 131) % 1200) / 10000.0), 4)                             AS rate,
      ${pick(CATEGORY, 141)}                                                         AS category,
      CASE WHEN hash(i * 31 + 151) % 20 = 0 THEN 'Y' ELSE 'N' END                   AS risk_flag,
      'batch ' || ((i % 997) + 1)::VARCHAR || ' line ' || ((i % 53) + 1)::VARCHAR   AS note,
      '${b ? "2026-09-01" : "2026-08-01"} 02:15:00'                              AS updated_at
    `;
}

export async function genDuckdb(rows: number, aPath: string, bPath: string, seed: number): Promise<void> {
  const { DuckDBInstance } = await import("@duckdb/node-api");
  const lit = (p: string) => "'" + p.replaceAll("'", "''") + "'";
  const instance = await DuckDBInstance.create(":memory:");
  const con = await instance.connect();
  try {
    await con.run("SET preserve_insertion_order = false");
    const base = `SELECT i, (hash(i * 31 + ${seed}) % 10000) AS bucket FROM range(0, ${rows}) t(i)`;
    const dupExtra = Math.max(1, Math.floor(rows / DUP_MOD));
    await con.run(
      `COPY (SELECT ${sqlExpressions("a")} FROM (${base}) ` +
        `UNION ALL SELECT ${sqlExpressions("a")} FROM (${base}) WHERE i < ${dupExtra} ` +
        `) TO ${lit(aPath)} (HEADER, FORMAT CSV)`,
    );
    const added = Math.max(1, Math.floor(rows / ADDED_RATIO));
    const addedBase = `SELECT i, (hash(i * 31 + ${seed}) % 10000) AS bucket FROM range(${rows}, ${rows + added}) t(i)`;
    await con.run(
      `COPY (SELECT ${sqlExpressions("b")} FROM (${base}) WHERE i % ${REMOVED_MOD} <> 7 ` +
        `UNION ALL SELECT ${sqlExpressions("b")} FROM (${addedBase}) ` +
        `UNION ALL SELECT ${sqlExpressions("b")} FROM (${base}) WHERE i % ${DUP_MOD} = 3 AND i < ${Math.floor(rows / 2)} ` +
        `) TO ${lit(bPath)} (HEADER, FORMAT CSV)`,
    );
  } finally {
    con.closeSync();
    instance.closeSync();
  }
}

// ---------------------------------------------------------------------------
// Pure-TS path (single pass, writes both files together)
// ---------------------------------------------------------------------------

const MASK = (1n << 64n) - 1n;

export function genNative(rows: number, aPath: string, bPath: string, seed: number): void {
  const h = (i: number, salt: number): bigint => {
    // splitmix-style, deterministic
    let x = (BigInt(i) * 31n + BigInt(salt) + BigInt(seed)) & MASK;
    x = ((x ^ (x >> 30n)) * 0xbf58476d1ce4e5b9n) & MASK;
    x = ((x ^ (x >> 27n)) * 0x94d049bb133111ebn) & MASK;
    return x ^ (x >> 31n);
  };
  const hm = (i: number, salt: number, mod: number): number => Number(h(i, salt) % BigInt(mod));

  const d0 = Date.UTC(2026, 0, 1);
  const day = Array.from({ length: 240 }, (_, n) => new Date(d0 + n * 86_400_000).toISOString().slice(0, 10));
  const pad = (n: number, w: number) => String(n).padStart(w, "0");

  const row = (i: number, b: boolean): string => {
    const bucket = hm(i, 0, 10000);
    let amount = hm(i, 21, 900000000) / 100 - 1000000;
    let balance = hm(i, 31, 2000000000) / 100;
    let status = STATUS[hm(i, 11, STATUS.length)]!;
    let valueDate = day[hm(i, 41, 240)]!;
    if (b) {
      if (bucket < CHG_STATUS) status = STATUS[(hm(i, 11, STATUS.length) + 1) % STATUS.length]!;
      else if (bucket < CHG_STATUS + CHG_AMOUNT) amount = amount + 12.34;
      else if (bucket < CHG_STATUS + CHG_AMOUNT + CHG_BALANCE) balance = balance * 1.01;
      if (bucket < CHG_VALUE_DATE) valueDate = "";
    }
    return [
      `ACC-${pad((i * 7919) % 250000, 8)}`,
      `TXN-${pad(i, 11)}`,
      day[hm(i, 1, 240)]!,
      valueDate,
      CURRENCY[hm(i, 51, 4)]!,
      amount.toFixed(2),
      (hm(i, 61, 5000) / 100).toFixed(2),
      balance.toFixed(2),
      status,
      CHANNEL[hm(i, 71, 5)]!,
      REGION[hm(i, 81, 4)]!,
      `BR${pad(hm(i, 91, 900) + 100, 4)}`,
      `P${pad(hm(i, 101, 5000), 5)}`,
      `CP-${pad(hm(i, 111, 90000), 6)}`,
      String(hm(i, 121, 500) + 1),
      (hm(i, 131, 1200) / 10000).toFixed(4),
      CATEGORY[hm(i, 141, 5)]!,
      hm(i, 151, 20) === 0 ? "Y" : "N",
      `batch ${(i % 997) + 1} line ${(i % 53) + 1}`,
      b ? "2026-09-01 02:15:00" : "2026-08-01 02:15:00",
    ].join(",");
  };

  const header = COLUMNS.join(",") + "\n";
  const dupExtra = Math.max(1, Math.floor(rows / DUP_MOD));
  const added = Math.max(1, Math.floor(rows / ADDED_RATIO));
  const fa = openSync(aPath, "w");
  const fb = openSync(bPath, "w");
  try {
    writeSync(fa, header);
    writeSync(fb, header);
    let ba: string[] = [];
    let bb: string[] = [];
    const flush = () => {
      writeSync(fa, ba.join("\n") + "\n");
      writeSync(fb, bb.join("\n") + "\n");
      ba = [];
      bb = [];
    };
    for (let i = 0; i < rows; i++) {
      ba.push(row(i, false));
      if (i % REMOVED_MOD !== 7) bb.push(row(i, true));
      if (i % DUP_MOD === 3 && i < Math.floor(rows / 2)) bb.push(row(i, true));
      if (ba.length >= 20000) flush();
    }
    for (let i = 0; i < dupExtra; i++) ba.push(row(i, false));
    for (let i = rows; i < rows + added; i++) bb.push(row(i, true));
    flush();
  } finally {
    closeSync(fa);
    closeSync(fb);
  }
}

export function parseRows(s: string): number {
  const t = s.trim().toLowerCase().replaceAll("_", "").replaceAll(",", "");
  const mult: Record<string, number> = { k: 1_000, m: 1_000_000, g: 1_000_000_000 };
  const last = t[t.length - 1] ?? "";
  return last in mult ? Math.round(Number(t.slice(0, -1)) * mult[last]!) : Number.parseInt(t, 10);
}

export async function genData(
  rows: number,
  aPath: string,
  bPath: string,
  { seed = 7, engine = "auto" as GenEngine } = {},
): Promise<"duckdb" | "native"> {
  let chosen: "duckdb" | "native" = engine === "auto" ? "native" : engine;
  if (engine === "auto") {
    try {
      await import("@duckdb/node-api");
      chosen = "duckdb";
    } catch {
      chosen = "native";
    }
  }
  if (chosen === "duckdb") await genDuckdb(rows, aPath, bPath, seed);
  else genNative(rows, aPath, bPath, seed);
  return chosen;
}

async function main(): Promise<number> {
  const { values } = parseArgs({
    options: {
      rows: { type: "string", short: "n", default: "10k" },
      "out-dir": { type: "string", short: "o", default: "data" },
      prefix: { type: "string" },
      engine: { type: "string", default: "auto" },
      seed: { type: "string", default: "7" },
    },
  });
  const rows = parseRows(values.rows);
  mkdirSync(values["out-dir"], { recursive: true });
  const prefix = values.prefix ?? values.rows.toLowerCase();
  const a = join(values["out-dir"], `${prefix}_a.csv`);
  const b = join(values["out-dir"], `${prefix}_b.csv`);
  const t0 = performance.now();
  const engine = await genData(rows, a, b, { seed: Number(values.seed), engine: values.engine as GenEngine });
  const dt = (performance.now() - t0) / 1000;
  console.log(`${engine}: ${rows.toLocaleString("en-US")} rows x ${COLUMNS.length} columns in ${dt.toFixed(1)}s`);
  for (const p of [a, b]) console.log(`  ${p}  ${(statSync(p).size / 1e6).toFixed(1)} MB`);
  return 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === realpathSync(process.argv[1])) {
  process.exitCode = await main();
}
