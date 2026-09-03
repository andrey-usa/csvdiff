/**
 * Polars engine (nodejs-polars, Rust core, multithreaded).
 *
 * Columnar and eager: both files are materialised as all-String frames, the
 * key is de-duplicated keeping the first occurrence, the two frames are
 * full-joined and the per-column difference flags are computed as expressions.
 *
 * Parity note — Polars reads an unquoted empty field as null but keeps a
 * *quoted* empty ("") as an empty string, where DuckDB, Arquero and the native
 * reader all yield null. `emptyToNull` restores that agreement before any
 * user-supplied normalisation runs, so an engine swap never changes counts.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join as pathJoin } from "node:path";

import pl from "nodejs-polars";
import type { DataFrame, Expr } from "nodejs-polars";

import { resolveColumns } from "./engine.ts";
import { toCsv } from "./engine-native.ts";
import type { CellDiff, ChangedRow, ColumnStat, Counts, EngineResult, Options, Section, Val } from "./types.ts";

const A_ = "a_";
const B_ = "b_";
const D_ = "d_";
const SUFFIX = "__csvdiff_b";
const STATUS = "_status";

const str = (v: unknown): Val => (v === null || v === undefined ? null : String(v));
const int = (v: unknown): number => (v === null || v === undefined ? 0 : Number(v));

/** Quoted-empty parity with DuckDB: "" is an absent value, not a zero-length string. */
const emptyToNull = (e: Expr): Expr => pl.when(e.eq(pl.lit(""))).then(pl.lit(null)).otherwise(e);

function normExpr(name: string, opt: Options): Expr {
  let e = emptyToNull(pl.col(name));
  if (opt.trim) e = e.str.strip();
  if (opt.ignore_case) e = e.str.toLowerCase();
  if (opt.empty_is_null) e = pl.when(e.eq(pl.lit(""))).then(pl.lit(null)).otherwise(e);
  return e.alias(name);
}

/** SQL `a IS DISTINCT FROM b`, with the numeric tolerance applied where both sides parse. */
function diffExpr(a: Expr, b: Expr, opt: Options): Expr {
  const bothNull = a.isNull().and(b.isNull());
  const oneNull = a.isNull().or(b.isNull());
  let unequal: Expr = a.neq(b);
  if (opt.tolerance > 0) {
    const na = a.cast(pl.Float64, false);
    const nb = b.cast(pl.Float64, false);
    unequal = pl
      .when(na.isNotNull().and(nb.isNotNull()))
      .then(na.sub(nb).abs().gt(opt.tolerance))
      .otherwise(a.neq(b));
  }
  return pl.when(bothNull).then(pl.lit(false)).when(oneNull).then(pl.lit(true)).otherwise(unequal);
}

export async function comparePolars(aPath: string, bPath: string, opt: Options): Promise<EngineResult> {
  const readOpts: Record<string, unknown> = { inferSchemaLength: 0 };
  if (opt.delimiter) readOpts["sep"] = opt.delimiter;

  const rawA = pl.readCSV(aPath, readOpts);
  const rawB = pl.readCSV(bPath, readOpts);
  const aCols = rawA.columns;
  const bCols = rawB.columns;
  const { compared, onlyA, onlyB } = resolveColumns(aCols, bCols, opt);
  const key = opt.key;
  const nk = key.length;
  const wanted = [...key, ...compared];

  const prep = (df: DataFrame): DataFrame => df.select(...wanted.map((c) => normExpr(c, opt)));
  const A = prep(rawA);
  const B = prep(rawB);

  const counts: Counts = {
    a_rows: A.height, b_rows: B.height, a_keys: 0, b_keys: 0,
    matched: 0, unchanged: 0, changed: 0, added: 0, removed: 0,
    a_dup_keys: 0, a_dup_rows: 0, b_dup_keys: 0, b_dup_rows: 0,
  };

  // Duplicate keys, ordered like the other engines: most duplicated first, then by key.
  const dupSection = (df: DataFrame, side: "a" | "b"): Section<(Val | number)[]> => {
    const grouped = df.groupBy(key).agg(pl.len().alias("n")).filter(pl.col("n").gt(1));
    counts[`${side}_dup_keys`] = grouped.height;
    counts[`${side}_dup_rows`] = grouped.height
      ? int(grouped.select(pl.col("n").sum().alias("s")).toRecords()[0]?.["s"])
      : 0;
    counts[`${side}_keys`] = counts[`${side}_rows`] - counts[`${side}_dup_rows`] + counts[`${side}_dup_keys`];
    const top = grouped
      .sort({ by: ["n", ...key], descending: [true, ...key.map(() => false)], nullsLast: true })
      .head(opt.max_rows);
    return {
      cols: [...key, "count"],
      rows: top.rows().map((r) => [...r.slice(0, nk).map(str), int(r[nk])]),
      truncated: grouped.height > opt.max_rows,
    };
  };
  const dupA = dupSection(A, "a");
  const dupB = dupSection(B, "b");

  // First occurrence of each key takes part in the join.
  const uniq = (df: DataFrame) => df.unique({ subset: key, keep: "first", maintainOrder: true });
  const A1 = uniq(A).withColumn(pl.lit(1).alias("_ina"));
  const B1 = uniq(B).withColumn(pl.lit(1).alias("_inb"));

  const J = A1.join(B1, { on: key, how: "full", suffix: SUFFIX });
  const bName = (c: string) => (wanted.includes(c) ? c + SUFFIX : c);

  // A full join keeps both key columns; rebuild one key per row plus the row status.
  const keySel = key.map((k) => pl.when(pl.col(k).isNull()).then(pl.col(bName(k))).otherwise(pl.col(k)).alias(k));
  const statusSel = pl
    .when(pl.col("_ina").isNull()).then(pl.lit("added"))
    .when(pl.col("_inb").isNull()).then(pl.lit("removed"))
    .otherwise(pl.lit("matched"))
    .alias(STATUS);
  const cellSel: Expr[] = [];
  compared.forEach((c, i) => {
    const a = pl.col(c);
    const b = pl.col(bName(c));
    cellSel.push(a.alias(A_ + i), b.alias(B_ + i), diffExpr(a, b, opt).alias(D_ + i));
  });
  const flat = J.select(...keySel, statusSel, ...cellSel);

  const changedFlag = compared.length
    ? compared.map((_, i) => pl.col(D_ + i)).reduce((acc, e) => acc.or(e))
    : pl.lit(false);
  const withChanged = flat.withColumn(changedFlag.alias("_changed"));

  const matchedRows = withChanged.filter(pl.col(STATUS).eq(pl.lit("matched")));
  counts.added = withChanged.filter(pl.col(STATUS).eq(pl.lit("added"))).height;
  counts.removed = withChanged.filter(pl.col(STATUS).eq(pl.lit("removed"))).height;
  counts.matched = matchedRows.height;
  const changedOnly = matchedRows.filter(pl.col("_changed"));
  counts.changed = changedOnly.height;
  counts.unchanged = counts.matched - counts.changed;

  // Per-column stats over matched rows only.
  const columns: ColumnStat[] = [];
  if (compared.length) {
    const aggs: Expr[] = [];
    compared.forEach((_, i) => {
      aggs.push(
        pl.col(D_ + i).cast(pl.Int32).sum().alias(`c${i}`),
        pl.col(A_ + i).isNotNull().and(pl.col(B_ + i).isNull()).cast(pl.Int32).sum().alias(`bl${i}`),
        pl.col(A_ + i).isNull().and(pl.col(B_ + i).isNotNull()).cast(pl.Int32).sum().alias(`fi${i}`),
      );
    });
    const stats = counts.matched ? (matchedRows.select(...aggs).toRecords()[0] ?? {}) : {};
    compared.forEach((c, i) => {
      columns.push({
        name: c,
        changed: int(stats[`c${i}`]),
        blanked: int(stats[`bl${i}`]),
        filled: int(stats[`fi${i}`]),
      });
    });
  }

  const sortByKey = (df: DataFrame) =>
    df.sort({ by: key, descending: key.map(() => false), nullsLast: true });

  // Changed rows -> sparse cell diffs.
  const changedRows: ChangedRow[] = [];
  if (compared.length && counts.changed) {
    const cols = [...key, ...compared.flatMap((_, i) => [A_ + i, B_ + i, D_ + i])];
    for (const r of sortByKey(changedOnly).head(opt.max_rows).select(...cols.map((c) => pl.col(c))).rows()) {
      const cells: CellDiff[] = [];
      for (let i = 0; i < compared.length; i++) {
        if (r[nk + 3 * i + 2]) cells.push([i, str(r[nk + 3 * i]), str(r[nk + 3 * i + 1])]);
      }
      changedRows.push([...r.slice(0, nk).map(str), cells]);
    }
  }

  const sideRows = (status: "added" | "removed", prefix: string): Section => {
    const sub = withChanged.filter(pl.col(STATUS).eq(pl.lit(status)));
    const selected = sub.height
      ? sortByKey(sub)
          .head(opt.max_rows)
          .select(...key.map((k) => pl.col(k)), ...compared.map((_, i) => pl.col(prefix + i)))
          .rows()
          .map((r) => r.map(str))
      : [];
    return { cols: wanted, rows: selected, truncated: counts[status] > opt.max_rows };
  };
  const added = sideRows("added", B_);
  const removed = sideRows("removed", A_);

  if (opt.export_dir) {
    mkdirSync(opt.export_dir, { recursive: true });
    const dump = (status: "added" | "removed", prefix: string): Val[][] => {
      const sub = withChanged.filter(pl.col(STATUS).eq(pl.lit(status)));
      if (!sub.height) return [];
      return sortByKey(sub)
        .select(...key.map((k) => pl.col(k)), ...compared.map((_, i) => pl.col(prefix + i)))
        .rows()
        .map((r) => r.map(str));
    };
    writeFileSync(pathJoin(opt.export_dir, "added.csv"), toCsv(wanted, dump("added", B_)));
    writeFileSync(pathJoin(opt.export_dir, "removed.csv"), toCsv(wanted, dump("removed", A_)));
    const both = [...key, ...compared.flatMap((c) => [`${c} (A)`, `${c} (B)`])];
    const changedFull = counts.changed
      ? sortByKey(changedOnly)
          .select(...key.map((k) => pl.col(k)), ...compared.flatMap((_, i) => [pl.col(A_ + i), pl.col(B_ + i)]))
          .rows()
          .map((r) => r.map(str))
      : [];
    writeFileSync(pathJoin(opt.export_dir, "changed.csv"), toCsv(both, changedFull));
  }

  return {
    meta: { key, compared, only_in_a: onlyA, only_in_b: onlyB, a_cols: aCols.length, b_cols: bCols.length },
    counts,
    columns,
    changed: { cols: key, rows: changedRows, truncated: counts.changed > opt.max_rows },
    added,
    removed,
    dup_a: dupA,
    dup_b: dupB,
  };
}
