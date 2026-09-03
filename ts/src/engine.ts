/**
 * Composite-key CSV comparison.
 *
 * Engine: DuckDB (default, via @duckdb/node-api). It streams both files from
 * disk, infers delimiters, hash-joins on the composite key and spills to disk
 * when data exceeds RAM, so multi-GB files are fine. The `native` engine is a
 * dependency-free in-memory fallback with the same result contract.
 *
 * The result contract is documented in `types.ts`; both engines must return
 * identical `counts` and `columns` for the same input (CI asserts this).
 */
import { statSync } from "node:fs";
import { basename, resolve } from "node:path";
import { performance } from "node:perf_hooks";

import type { CompareResult, EngineName, EngineResult, Options, Val } from "./types.ts";

export class CompareError extends Error {
  override name = "CompareError";
}

export async function compare(aPath: string, bPath: string, opt: Options): Promise<CompareResult> {
  if (!opt.key || opt.key.length === 0) {
    throw new CompareError("At least one key column is required.");
  }
  for (const p of [aPath, bPath]) {
    if (!isFile(p)) throw new CompareError(`File not found: ${p}`);
  }

  const t0 = performance.now();
  const engine = await resolveEngine(opt.engine);
  let result: EngineResult;
  if (engine === "duckdb") {
    const { compareDuckdb } = await import("./engine-duckdb.ts");
    result = await compareDuckdb(aPath, bPath, opt);
  } else {
    const { compareNative } = await import("./engine-native.ts");
    result = await compareNative(aPath, bPath, opt);
  }

  const { key: _k, compare: _c, ignore: _i, ...options } = opt;
  return {
    ...result,
    meta: {
      ...result.meta,
      a: fileMeta(aPath),
      b: fileMeta(bPath),
      engine,
      seconds: Math.round(performance.now() - t0) / 1000,
      generated: isoWithOffset(new Date()),
      options,
    },
  };
}

export function isIdentical(result: CompareResult): boolean {
  const c = result.counts;
  return c.changed === 0 && c.added === 0 && c.removed === 0;
}

export async function resolveEngine(engine: EngineName): Promise<"duckdb" | "native"> {
  if (engine === "duckdb" || engine === "native") return engine;
  if (engine !== "auto") throw new CompareError(`Unknown engine: ${engine as string}`);
  try {
    await import("@duckdb/node-api");
    return "duckdb";
  } catch {
    return "native";
  }
}

// ----------------------------------------------------------------------------
// Shared helpers (used by both engines)
// ----------------------------------------------------------------------------

export interface ResolvedColumns {
  compared: string[];
  onlyA: string[];
  onlyB: string[];
}

export function resolveColumns(aCols: string[], bCols: string[], opt: Options): ResolvedColumns {
  const aSet = new Set(aCols);
  const bSet = new Set(bCols);
  const missing = opt.key.filter((k) => !aSet.has(k) || !bSet.has(k));
  if (missing.length) {
    throw new CompareError(`Key column(s) missing from one of the files: ${missing.join(", ")}`);
  }
  const common = aCols.filter((c) => bSet.has(c));
  const commonSet = new Set(common);
  const keySet = new Set(opt.key);
  let compared: string[];
  if (opt.compare === null) {
    compared = common.filter((c) => !keySet.has(c));
  } else {
    const bad = opt.compare.filter((c) => !commonSet.has(c));
    if (bad.length) {
      throw new CompareError(`Compare column(s) not present in both files: ${bad.join(", ")}`);
    }
    compared = opt.compare.filter((c) => !keySet.has(c));
  }
  const ignore = new Set(opt.ignore);
  compared = compared.filter((c) => !ignore.has(c));
  return {
    compared,
    onlyA: aCols.filter((c) => !bSet.has(c)),
    onlyB: bCols.filter((c) => !aSet.has(c)),
  };
}

/** Apply --trim / --ignore-case / --empty-is-null to one value (native engine). */
export function normValue(v: Val, opt: Options): Val {
  if (v === null) return null;
  let s = v;
  if (opt.trim) s = s.trim();
  if (opt.ignore_case) s = s.toLowerCase();
  if (opt.empty_is_null && s === "") return null;
  return s;
}

const NUMBER = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;

function asNumber(v: Val): number | null {
  if (v === null) return null;
  const s = v.trim();
  return NUMBER.test(s) ? Number(s) : null;
}

/** Cell inequality with optional numeric tolerance (native engine). */
export function differs(x: Val, y: Val, opt: Options): boolean {
  if (x === null && y === null) return false;
  if (opt.tolerance > 0) {
    const nx = asNumber(x);
    const ny = asNumber(y);
    if (nx !== null && ny !== null) return Math.abs(nx - ny) > opt.tolerance;
  }
  return x !== y;
}

/** Sort key values the way DuckDB orders VARCHAR: ascending, NULLs last. */
export function compareKeys(a: Val[], b: Val[], nk: number): number {
  for (let i = 0; i < nk; i++) {
    const x = a[i] ?? null;
    const y = b[i] ?? null;
    if (x === y) continue;
    if (x === null) return 1;
    if (y === null) return -1;
    return x < y ? -1 : 1;
  }
  return 0;
}

function isFile(p: string): boolean {
  try {
    return statSync(p).isFile();
  } catch {
    return false;
  }
}

function fileMeta(p: string) {
  return { name: basename(p), path: resolve(p), size: statSync(p).size };
}

/** Local time with numeric offset, seconds precision: 2026-09-03T18:56:28+00:00 */
export function isoWithOffset(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const off = -d.getTimezoneOffset();
  const sign = off >= 0 ? "+" : "-";
  const abs = Math.abs(off);
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}` +
    `${sign}${pad(Math.floor(abs / 60))}:${pad(abs % 60)}`
  );
}
