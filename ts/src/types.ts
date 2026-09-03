/**
 * Result contract and options. Field names are deliberately snake_case so the
 * JSON produced here is byte-compatible with the Python implementation
 * (`csvdiff/engine.py`): the same report template, CI summaries and
 * downstream sinks read both.
 */

/** A CSV cell. Empty fields are read as null (no type inference, ever). */
export type Val = string | null;

/** One differing cell in a changed row: [compared-column index, A value, B value]. */
export type CellDiff = [number, Val, Val];

/** k1..kn key values followed by the list of differing cells. */
export type ChangedRow = (Val | CellDiff[])[];

export interface Section<R = Val[]> {
  cols: string[];
  rows: R[];
  truncated: boolean;
}

export interface FileMeta {
  name: string;
  path: string;
  size: number;
}

export interface Counts {
  a_rows: number;
  b_rows: number;
  a_keys: number;
  b_keys: number;
  matched: number;
  unchanged: number;
  changed: number;
  added: number;
  removed: number;
  a_dup_keys: number;
  a_dup_rows: number;
  b_dup_keys: number;
  b_dup_rows: number;
}

export interface ColumnStat {
  name: string;
  changed: number;
  blanked: number;
  filled: number;
}

export interface EngineMeta {
  key: string[];
  compared: string[];
  only_in_a: string[];
  only_in_b: string[];
  a_cols: number;
  b_cols: number;
}

export interface Meta extends EngineMeta {
  a: FileMeta;
  b: FileMeta;
  engine: string;
  seconds: number;
  generated: string;
  options: Record<string, unknown>;
}

/** What an engine returns; `compare()` completes `meta`. */
export interface EngineResult {
  meta: EngineMeta;
  counts: Counts;
  columns: ColumnStat[];
  changed: Section<ChangedRow>;
  added: Section;
  removed: Section;
  dup_a: Section<(Val | number)[]>;
  dup_b: Section<(Val | number)[]>;
}

export interface CompareResult extends Omit<EngineResult, "meta"> {
  meta: Meta;
}

/**
 * Comparison backends. `auto` picks the first available in preference order
 * (duckdb, polars, arquero, native); every engine must return identical
 * `counts` and `columns` for the same input.
 */
export type EngineName = "auto" | "duckdb" | "polars" | "arquero" | "native";

/** Concrete engines, in `auto` preference order. */
export const ENGINES = ["duckdb", "polars", "arquero", "native"] as const;
export type ConcreteEngine = (typeof ENGINES)[number];

export interface Options {
  key: string[];
  /** null -> every common non-key column */
  compare: string[] | null;
  ignore: string[];
  /** strip surrounding whitespace before comparing */
  trim: boolean;
  ignore_case: boolean;
  /** treat '' and NULL as equal */
  empty_is_null: boolean;
  /** absolute numeric tolerance when both sides parse as numbers */
  tolerance: number;
  /** rows embedded per section in the report */
  max_rows: number;
  /** null -> auto-detect */
  delimiter: string | null;
  encoding: string;
  engine: EngineName;
  threads: number | null;
  /** e.g. "4GB" (DuckDB only) */
  memory_limit: string | null;
  /** write full, uncapped changed/added/removed CSVs here */
  export_dir: string | null;
}

export const DEFAULT_OPTIONS: Omit<Options, "key"> = {
  compare: null,
  ignore: [],
  trim: false,
  ignore_case: false,
  empty_is_null: false,
  tolerance: 0,
  max_rows: 50_000,
  delimiter: null,
  encoding: "utf-8",
  engine: "auto",
  threads: null,
  memory_limit: null,
  export_dir: null,
};

export const OPTION_KEYS = ["key", ...Object.keys(DEFAULT_OPTIONS)] as (keyof Options)[];

export function makeOptions(partial: Partial<Options> & { key: string[] }): Options {
  return { ...DEFAULT_OPTIONS, ...partial };
}
