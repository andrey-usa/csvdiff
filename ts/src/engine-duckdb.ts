/**
 * DuckDB engine. Same SQL as the Python implementation: both files are read as
 * text (all_varchar), normalised, de-duplicated on the key (first occurrence
 * joins) and full-outer-joined; only differing cells are extracted.
 *
 * SQL is built by string interpolation: identifiers go through q(), string
 * values and paths through lit(). Nothing else is interpolated raw.
 */
import { mkdirSync } from "node:fs";
import { join } from "node:path";

import { DuckDBInstance, type DuckDBConnection, type DuckDBValue } from "@duckdb/node-api";

import { resolveColumns } from "./engine.ts";
import type { CellDiff, ChangedRow, ColumnStat, Counts, EngineResult, Options, Section, Val } from "./types.ts";

export const q = (name: string): string => '"' + name.replaceAll('"', '""') + '"';
export const lit = (s: string): string => "'" + s.replaceAll("'", "''") + "'";

function normSql(expr: string, opt: Options): string {
  if (opt.trim) expr = `trim(${expr})`;
  if (opt.ignore_case) expr = `lower(${expr})`;
  if (opt.empty_is_null) expr = `nullif(${expr}, '')`;
  return expr;
}

function diffSql(a: string, b: string, opt: Options): string {
  if (opt.tolerance > 0) {
    const tol = String(opt.tolerance);
    return (
      `(CASE WHEN try_cast(${a} AS DOUBLE) IS NOT NULL AND try_cast(${b} AS DOUBLE) IS NOT NULL ` +
      `THEN abs(try_cast(${a} AS DOUBLE) - try_cast(${b} AS DOUBLE)) > ${tol} ` +
      `ELSE (${a} IS DISTINCT FROM ${b}) END)`
    );
  }
  return `(${a} IS DISTINCT FROM ${b})`;
}

async function rows(con: DuckDBConnection, sql: string): Promise<DuckDBValue[][]> {
  const reader = await con.runAndReadAll(sql);
  return reader.getRows();
}

async function one(con: DuckDBConnection, sql: string): Promise<DuckDBValue[]> {
  const r = await rows(con, sql);
  return r[0] ?? [];
}

const num = (v: DuckDBValue): number =>
  typeof v === "bigint" ? Number(v) : typeof v === "number" ? v : v == null ? 0 : Number(String(v));
const str = (v: DuckDBValue): Val => (v == null ? null : String(v));

export async function compareDuckdb(aPath: string, bPath: string, opt: Options): Promise<EngineResult> {
  const instance = await DuckDBInstance.create(":memory:");
  const con = await instance.connect();
  try {
    if (opt.threads) await con.run(`SET threads = ${Math.trunc(opt.threads)}`);
    if (opt.memory_limit) await con.run(`SET memory_limit = ${lit(opt.memory_limit)}`);
    await con.run("SET preserve_insertion_order = true");

    let readOpts = "all_varchar=true, header=true, sample_size=-1";
    if (opt.delimiter) readOpts += `, delim=${lit(opt.delimiter)}`;
    if (opt.encoding && !["utf-8", "utf8"].includes(opt.encoding.toLowerCase())) {
      readOpts += `, encoding=${lit(opt.encoding)}`;
    }

    const load = async (tbl: string, path: string): Promise<string[]> => {
      await con.run(`CREATE TABLE ${tbl}_raw AS SELECT * FROM read_csv(${lit(path)}, ${readOpts})`);
      return (await rows(con, `DESCRIBE ${tbl}_raw`)).map((r) => String(r[0]));
    };

    const aCols = await load("a", aPath);
    const bCols = await load("b", bPath);
    const { compared, onlyA, onlyB } = resolveColumns(aCols, bCols, opt);
    const key = opt.key;

    // Normalised projection + stable row number
    for (const tbl of ["a", "b"]) {
      const proj = [...key, ...compared].map((c) => `${normSql(q(c), opt)} AS ${q(c)}`).join(", ");
      await con.run(`CREATE TABLE ${tbl} AS SELECT ${proj}, row_number() OVER () AS _rn FROM ${tbl}_raw`);
      await con.run(`DROP TABLE ${tbl}_raw`);
    }

    const kq = key.map(q).join(", ");
    const counts: Counts = {
      a_rows: 0, b_rows: 0, a_keys: 0, b_keys: 0, matched: 0, unchanged: 0, changed: 0, added: 0, removed: 0,
      a_dup_keys: 0, a_dup_rows: 0, b_dup_keys: 0, b_dup_rows: 0,
    };
    counts.a_rows = num((await one(con, "SELECT count(*) FROM a"))[0]);
    counts.b_rows = num((await one(con, "SELECT count(*) FROM b"))[0]);

    // Duplicate keys
    const dups: Record<"a" | "b", Section<(Val | number)[]>> = { a: emptySection(key), b: emptySection(key) };
    for (const tbl of ["a", "b"] as const) {
      await con.run(
        `CREATE TABLE ${tbl}_dup AS SELECT ${kq}, count(*) AS n FROM ${tbl} GROUP BY ALL HAVING count(*) > 1`,
      );
      const [nk, nr] = await one(con, `SELECT count(*)::BIGINT, coalesce(sum(n), 0)::BIGINT FROM ${tbl}_dup`);
      counts[`${tbl}_dup_keys`] = num(nk);
      counts[`${tbl}_dup_rows`] = num(nr);
      counts[`${tbl}_keys`] = counts[`${tbl}_rows`] - counts[`${tbl}_dup_rows`] + counts[`${tbl}_dup_keys`];
      const dr = await rows(con, `SELECT * FROM ${tbl}_dup ORDER BY n DESC, ${kq} LIMIT ${opt.max_rows}`);
      dups[tbl] = {
        cols: [...key, "count"],
        rows: dr.map((r) => [...r.slice(0, key.length).map(str), num(r[key.length])]),
        truncated: counts[`${tbl}_dup_keys`] > opt.max_rows,
      };
      // First occurrence per key participates in the join
      await con.run(
        `CREATE TABLE ${tbl}1 AS SELECT * EXCLUDE(_k) FROM ` +
          `(SELECT *, row_number() OVER (PARTITION BY ${kq} ORDER BY _rn) AS _k FROM ${tbl}) WHERE _k = 1`,
      );
    }

    // Full outer join
    const keySel = key.map((k) => `coalesce(a.${q(k)}, b.${q(k)}) AS ${q(k)}`).join(", ");
    const on = key.map((k) => `a.${q(k)} IS NOT DISTINCT FROM b.${q(k)}`).join(" AND ");
    const colSel: string[] = [];
    const diffFlags: string[] = [];
    compared.forEach((c, i) => {
      const a = `a.${q(c)}`;
      const b = `b.${q(c)}`;
      const d = diffSql(a, b, opt);
      colSel.push(`${a} AS ${q("a_" + i)}`, `${b} AS ${q("b_" + i)}`, `${d} AS ${q("d_" + i)}`);
      diffFlags.push(d);
    });
    const changedExpr = diffFlags.length ? diffFlags.join(" OR ") : "false";
    await con.run(`
        CREATE TABLE j AS
        SELECT ${keySel},
               CASE WHEN a._rn IS NULL THEN 'added' WHEN b._rn IS NULL THEN 'removed' ELSE 'matched' END AS _status,
               ${colSel.length ? colSel.join(", ") + "," : ""}
               (${changedExpr}) AS _changed
        FROM a1 a FULL OUTER JOIN b1 b ON ${on}
    `);

    for (const [status, chg, n] of await rows(con, "SELECT _status, _changed, count(*) FROM j GROUP BY ALL")) {
      const count = num(n);
      if (status === "matched") {
        if (chg) counts.changed += count;
        else counts.unchanged += count;
      } else if (status === "added") counts.added += count;
      else if (status === "removed") counts.removed += count;
    }
    counts.matched = counts.unchanged + counts.changed;

    // Per-column stats
    const columns: ColumnStat[] = [];
    if (compared.length) {
      const agg = compared
        .map(
          (_, i) =>
            `sum(${q("d_" + i)}::INT)::BIGINT, ` +
            `sum((${q("a_" + i)} IS NOT NULL AND ${q("b_" + i)} IS NULL)::INT)::BIGINT, ` +
            `sum((${q("a_" + i)} IS NULL AND ${q("b_" + i)} IS NOT NULL)::INT)::BIGINT`,
        )
        .join(", ");
      const stats = await one(con, `SELECT ${agg} FROM j WHERE _status = 'matched'`);
      compared.forEach((c, i) => {
        columns.push({
          name: c,
          changed: num(stats[3 * i]),
          blanked: num(stats[3 * i + 1]),
          filled: num(stats[3 * i + 2]),
        });
      });
    }

    // Changed rows -> sparse cell diffs
    const nk = key.length;
    const changedRows: ChangedRow[] = [];
    if (compared.length) {
      const cols =
        kq + ", " + compared.map((_, i) => `${q("a_" + i)}, ${q("b_" + i)}, ${q("d_" + i)}`).join(", ");
      const cr = await rows(
        con,
        `SELECT ${cols} FROM j WHERE _status = 'matched' AND _changed ORDER BY ${kq} LIMIT ${opt.max_rows}`,
      );
      for (const r of cr) {
        const cells: CellDiff[] = [];
        for (let i = 0; i < compared.length; i++) {
          if (r[nk + 3 * i + 2]) cells.push([i, str(r[nk + 3 * i]), str(r[nk + 3 * i + 1])]);
        }
        changedRows.push([...r.slice(0, nk).map(str), cells]);
      }
    }

    const sideRows = async (status: "added" | "removed", prefix: string): Promise<Section> => {
      const cols = kq + compared.map((_, i) => `, ${q(prefix + i)}`).join("");
      const sr = await rows(
        con,
        `SELECT ${cols} FROM j WHERE _status = ${lit(status)} ORDER BY ${kq} LIMIT ${opt.max_rows}`,
      );
      return { cols: [...key, ...compared], rows: sr.map((r) => r.map(str)), truncated: counts[status] > opt.max_rows };
    };

    const added = await sideRows("added", "b_");
    const removed = await sideRows("removed", "a_");

    if (opt.export_dir) {
      mkdirSync(opt.export_dir, { recursive: true });
      const aliases = (p: string) => compared.map((c, i) => `, ${q(p + i)} AS ${q(c)}`).join("");
      await con.run(
        `COPY (SELECT ${kq}${aliases("b_")} FROM j WHERE _status='added' ORDER BY ${kq}) TO ` +
          `${lit(join(opt.export_dir, "added.csv"))} (HEADER)`,
      );
      await con.run(
        `COPY (SELECT ${kq}${aliases("a_")} FROM j WHERE _status='removed' ORDER BY ${kq}) TO ` +
          `${lit(join(opt.export_dir, "removed.csv"))} (HEADER)`,
      );
      if (compared.length) {
        const both = compared
          .map((c, i) => `, ${q("a_" + i)} AS ${q(c + " (A)")}, ${q("b_" + i)} AS ${q(c + " (B)")}`)
          .join("");
        await con.run(
          `COPY (SELECT ${kq}${both} FROM j WHERE _status='matched' AND _changed ORDER BY ${kq}) TO ` +
            `${lit(join(opt.export_dir, "changed.csv"))} (HEADER)`,
        );
      }
    }

    return {
      meta: { key, compared, only_in_a: onlyA, only_in_b: onlyB, a_cols: aCols.length, b_cols: bCols.length },
      counts,
      columns,
      changed: { cols: key, rows: changedRows, truncated: counts.changed > opt.max_rows },
      added,
      removed,
      dup_a: dups.a,
      dup_b: dups.b,
    };
  } finally {
    con.closeSync();
    instance.closeSync();
  }
}

function emptySection(key: string[]): Section<(Val | number)[]> {
  return { cols: [...key, "count"], rows: [], truncated: false };
}
