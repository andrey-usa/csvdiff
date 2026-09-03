/**
 * Named comparison profiles from csvdiff.toml (searched in CWD, then
 * ~/.config/csvdiff/). Same file format as the Python implementation:
 *
 * [profiles.orders]
 * key      = ["order_id", "line_no"]
 * compare  = ["qty", "price", "status"]   # omit for all common non-key columns
 * ignore   = ["updated_at"]
 * trim     = true
 * tolerance = 0.005
 */
import { readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

import { parse } from "smol-toml";

import { DEFAULT_OPTIONS, OPTION_KEYS, type Options } from "./types.ts";

export type Profile = Partial<Options>;

export interface Config {
  profiles?: Record<string, Profile>;
  [section: string]: unknown;
}

export const SEARCH = ["csvdiff.toml", join(homedir(), ".config", "csvdiff", "csvdiff.toml")];

export function loadConfig(path?: string | null): Config {
  for (const p of path ? [path] : SEARCH) {
    if (p && isFile(p)) {
      return parse(readFileSync(p, "utf-8")) as Config;
    }
  }
  return {};
}

/** Merge a profile (may be null) with CLI/form overrides. Overrides win when not null/undefined. */
export function optionsFrom(
  profile: Profile | null | undefined,
  overrides: Partial<Record<keyof Options, unknown>>,
): Options {
  const merged: Record<string, unknown> = { ...(profile ?? {}) };
  for (const [k, v] of Object.entries(overrides)) {
    if (v !== null && v !== undefined) merged[k] = v;
  }
  const out: Record<string, unknown> = { ...DEFAULT_OPTIONS, key: [] };
  for (const k of OPTION_KEYS) {
    if (k in merged) out[k] = merged[k];
  }
  return out as unknown as Options;
}

/** 'a, b ,c' -> ['a','b','c']; array passthrough; null/undefined -> null. */
export function parseList(s: unknown): string[] | null {
  if (s === null || s === undefined) return null;
  if (Array.isArray(s)) return s.map(String);
  return String(s)
    .split(",")
    .map((x) => x.trim())
    .filter((x) => x.length > 0);
}

function isFile(p: string): boolean {
  try {
    return statSync(p).isFile();
  } catch {
    return false;
  }
}
