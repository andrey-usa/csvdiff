/**
 * Native engine: dependency-free, in-memory, same result contract as DuckDB.
 * The analogue of the Python implementation's pandas fallback. Use it for
 * files that fit comfortably in RAM; DuckDB for anything large.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { readCsv } from "./csv.ts";
import { compareKeys, differs, normValue, resolveColumns } from "./engine.ts";
import type { CellDiff, ChangedRow, ColumnStat, Counts, EngineResult, Options, Section, Val } from "./types.ts";

interface Side {
  rows: number;
  keys: number;
  dupKeys: number;
  dupRows: number;
  dups: Section<(Val | number)[]>;
  /** first occurrence per key: key values followed by compared values */
  first: Map<string, Val[]>;
}

const keyOf = (row: Val[], nk: number): string => {
  let s = "";
  for (let i = 0; i < nk; i++) {
    const v = row[i];
    s += (v === null || v === undefined ? "\u0000" : v) + "\u0001";
  }
  return s;
};

export async function compareNative(aPath: string, bPath: string, opt: Options): Promise<EngineResult> {
  const A = readCsv(aPath, opt);
  const B = readCsv(bPath, opt);
  const { compared, onlyA, onlyB } = resolveColumns(A.header, B.header, opt);
  const key = opt.key;
  const nk = key.length;
  const nc = compared.length;
  const wanted = [...key, ...compared];

  const project = (header: string[], rows: Val[][]): Val[][] => {
    const idx = wanted.map((c) => header.indexOf(c));
    return rows.map((r) => idx.map((i) => normValue(r[i] ?? null, opt)));
  };

  const side = (rows: Val[][]): Side => {
    const groups = new Map<string, number>();
    const first = new Map<string, Val[]>();
    for (const r of rows) {
      const k = keyOf(r, nk);
      groups.set(k, (groups.get(k) ?? 0) + 1);
      if (!first.has(k)) first.set(k, r);
    }
    const dupEntries: [Val[], number][] = [];
    let dupRows = 0;
    for (const [k, n] of groups) {
      if (n > 1) {
        dupEntries.push([first.get(k)!, n]);
        dupRows += n;
      }
    }
    dupEntries.sort((x, y) => y[1] - x[1] || compareKeys(x[0], y[0], nk));
    return {
      rows: rows.length,
      keys: groups.size,
      dupKeys: dupEntries.length,
      dupRows,
      dups: {
        cols: [...key, "count"],
        rows: dupEntries.slice(0, opt.max_rows).map(([r, n]) => [...r.slice(0, nk), n]),
        truncated: dupEntries.length > opt.max_rows,
      },
      first,
    };
  };

  const a = side(project(A.header, A.rows));
  const b = side(project(B.header, B.rows));

  const columns: ColumnStat[] = compared.map((c) => ({ name: c, changed: 0, blanked: 0, filled: 0 }));
  const changedAll: ChangedRow[] = [];
  const removedAll: Val[][] = [];
  const addedAll: Val[][] = [];
  let matched = 0;

  for (const [k, ar] of a.first) {
    const br = b.first.get(k);
    if (br === undefined) {
      removedAll.push(ar);
      continue;
    }
    matched++;
    const cells: CellDiff[] = [];
    for (let i = 0; i < nc; i++) {
      const x = ar[nk + i] ?? null;
      const y = br[nk + i] ?? null;
      if (differs(x, y, opt)) {
        cells.push([i, x, y]);
        const col = columns[i]!;
        col.changed++;
        if (y === null) col.blanked++;
        if (x === null) col.filled++;
      }
    }
    if (cells.length) changedAll.push([...ar.slice(0, nk), cells]);
  }
  for (const [k, br] of b.first) {
    if (!a.first.has(k)) addedAll.push(br);
  }

  const byKey = (x: Val[], y: Val[]) => compareKeys(x, y, nk);
  changedAll.sort((x, y) => compareKeys(x as Val[], y as Val[], nk));
  addedAll.sort(byKey);
  removedAll.sort(byKey);

  const counts: Counts = {
    a_rows: a.rows,
    b_rows: b.rows,
    a_keys: a.keys,
    b_keys: b.keys,
    matched,
    unchanged: matched - changedAll.length,
    changed: changedAll.length,
    added: addedAll.length,
    removed: removedAll.length,
    a_dup_keys: a.dupKeys,
    a_dup_rows: a.dupRows,
    b_dup_keys: b.dupKeys,
    b_dup_rows: b.dupRows,
  };

  const section = (rows: Val[][]): Section => ({
    cols: wanted,
    rows: rows.slice(0, opt.max_rows),
    truncated: rows.length > opt.max_rows,
  });

  if (opt.export_dir) {
    mkdirSync(opt.export_dir, { recursive: true });
    writeFileSync(join(opt.export_dir, "added.csv"), toCsv(wanted, addedAll));
    writeFileSync(join(opt.export_dir, "removed.csv"), toCsv(wanted, removedAll));
    const both = [...key, ...compared.flatMap((c) => [`${c} (A)`, `${c} (B)`])];
    const changedFull: Val[][] = [];
    for (const r of changedAll) {
      const keys = r.slice(0, nk) as Val[];
      const ar = a.first.get(keyOf(keys, nk))!;
      const br = b.first.get(keyOf(keys, nk))!;
      const out: Val[] = [...keys];
      for (let i = 0; i < nc; i++) out.push(ar[nk + i] ?? null, br[nk + i] ?? null);
      changedFull.push(out);
    }
    writeFileSync(join(opt.export_dir, "changed.csv"), toCsv(both, changedFull));
  }

  return {
    meta: { key, compared, only_in_a: onlyA, only_in_b: onlyB, a_cols: A.header.length, b_cols: B.header.length },
    counts,
    columns,
    changed: { cols: key, rows: changedAll.slice(0, opt.max_rows), truncated: changedAll.length > opt.max_rows },
    added: section(addedAll),
    removed: section(removedAll),
    dup_a: a.dups,
    dup_b: b.dups,
  };
}

function csvCell(v: Val): string {
  if (v === null) return "";
  return /[",\n\r]/.test(v) ? '"' + v.replaceAll('"', '""') + '"' : v;
}

export function toCsv(header: string[], rows: Val[][]): string {
  const lines = [header.map(csvCell).join(",")];
  for (const r of rows) lines.push(r.map(csvCell).join(","));
  return lines.join("\n") + "\n";
}
