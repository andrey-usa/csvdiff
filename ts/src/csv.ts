/**
 * Minimal RFC 4180 reader for the native engine. Whole-file, in memory (the
 * native engine is in-memory anyway). Values are text; empty fields (quoted or
 * not) become null, matching DuckDB's read_csv defaults so both engines see
 * the same cells.
 */
import { readFileSync } from "node:fs";

import type { Val } from "./types.ts";

export interface CsvTable {
  header: string[];
  rows: Val[][];
}

const CANDIDATES = [",", ";", "\t", "|"];

export function detectDelimiter(text: string): string {
  const nl = text.indexOf("\n");
  const line = nl === -1 ? text : text.slice(0, nl);
  let best = ",";
  let bestCount = -1;
  for (const d of CANDIDATES) {
    const n = line.split(d).length - 1;
    if (n > bestCount) {
      best = d;
      bestCount = n;
    }
  }
  return best;
}

export function parseCsv(text: string, delimiter: string): Val[][] {
  const rows: Val[][] = [];
  let row: Val[] = [];
  const n = text.length;
  let i = 0;
  while (i < n) {
    let value = "";
    if (text[i] === '"') {
      i++;
      for (;;) {
        const qi = text.indexOf('"', i);
        if (qi === -1) {
          value += text.slice(i);
          i = n;
          break;
        }
        value += text.slice(i, qi);
        if (text[qi + 1] === '"') {
          value += '"';
          i = qi + 2;
          continue;
        }
        i = qi + 1;
        break;
      }
      // tolerate stray characters between the closing quote and the delimiter
      let j = i;
      while (j < n && text[j] !== delimiter && text[j] !== "\n" && text[j] !== "\r") j++;
      value += text.slice(i, j);
      i = j;
    } else {
      let j = i;
      while (j < n) {
        const c = text[j];
        if (c === delimiter || c === "\n" || c === "\r") break;
        j++;
      }
      value = text.slice(i, j);
      i = j;
    }
    row.push(value === "" ? null : value);
    if (i >= n) break;
    const c = text[i];
    if (c === delimiter) {
      i++;
      if (i >= n) row.push(null); // trailing delimiter at EOF -> empty last field
      continue;
    }
    if (c === "\r" && text[i + 1] === "\n") i++;
    i++;
    rows.push(row);
    row = [];
  }
  if (row.length) rows.push(row);
  return rows;
}

export function readCsv(path: string, opts: { delimiter?: string | null; encoding?: string }): CsvTable {
  const buf = readFileSync(path);
  const text = new TextDecoder(opts.encoding || "utf-8").decode(buf);
  const delimiter = opts.delimiter || detectDelimiter(text);
  const all = parseCsv(text, delimiter);
  if (all.length === 0) return { header: [], rows: [] };
  const header = all[0]!.map((v) => v ?? "");
  const width = header.length;
  const rows: Val[][] = [];
  for (let r = 1; r < all.length; r++) {
    const row = all[r]!;
    if (row.length === 1 && row[0] === null && width > 1) continue; // blank line
    if (row.length < width) {
      while (row.length < width) row.push(null);
    } else if (row.length > width) {
      row.length = width;
    }
    rows.push(row);
  }
  return { header, rows };
}
