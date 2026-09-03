/**
 * Self-contained HTML report — the same template as the Python implementation
 * (see report-template.ts, generated from report.py). Only differing cells are
 * embedded; the payload is gzip+base64 and decoded natively by the browser.
 */
import { gzipSync } from "node:zlib";

import { TEMPLATE } from "./report-template.ts";
import type { CompareResult } from "./types.ts";

export function render(result: CompareResult, compress = true): string {
  const raw = JSON.stringify(result, (_k, v: unknown) => (typeof v === "bigint" ? Number(v) : v));
  let payload: string;
  let mode: string;
  if (compress) {
    payload = gzipSync(Buffer.from(raw, "utf-8"), { level: 9 }).toString("base64");
    mode = "gzip";
  } else {
    payload = raw.replaceAll("</", "<\\/");
    mode = "json";
  }
  const title = `${result.meta.a.name} vs ${result.meta.b.name}`;
  // Function replacers: a plain string replacement would interpret `$&` etc.
  return TEMPLATE.replace("__TITLE__", () => escapeHtml(title))
    .replace("__MODE__", () => mode)
    .replace("__PAYLOAD__", () => payload);
}

export function escapeHtml(s: string): string {
  return s
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#x27;");
}
