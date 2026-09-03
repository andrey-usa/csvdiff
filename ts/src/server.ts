/**
 * Drag-and-drop launcher: `csvdiff-ts serve` then open http://127.0.0.1:8765.
 * Drop two CSVs, type the key (or pick a profile), get the report in a new tab.
 * node:http only — no framework.
 */
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { Readable } from "node:stream";

import { loadConfig, optionsFrom, parseList, type Config } from "./config.ts";
import { compare, CompareError } from "./engine.ts";
import { escapeHtml, render } from "./report.ts";
import { PAGE } from "./server-page.ts";

export function serve(host = "127.0.0.1", port = 8765, configPath?: string | null): Server {
  const config = loadConfig(configPath);
  const server = createServer((req, res) => {
    handle(req, res, config).catch((err: unknown) => {
      send(res, 500, String(err instanceof Error ? err.message : err), "text/plain; charset=utf-8");
    });
  });
  server.listen(port, host, () => {
    console.log(`csvdiff-ts drop page: http://${host}:${port}   (Ctrl+C to stop)`);
  });
  return server;
}

function send(res: ServerResponse, code: number, body: string | Buffer, ctype = "text/html; charset=utf-8", headers: Record<string, string> = {}): void {
  const buf = typeof body === "string" ? Buffer.from(body, "utf-8") : body;
  res.writeHead(code, { "Content-Type": ctype, "Content-Length": String(buf.length), ...headers });
  res.end(buf);
}

const field = (form: FormData, name: string): string => {
  const v = form.get(name);
  return typeof v === "string" ? v : "";
};

async function handle(req: IncomingMessage, res: ServerResponse, config: Config): Promise<void> {
  if (req.method === "GET") {
    const profiles = config.profiles ?? {};
    const opts = Object.entries(profiles)
      .map(([n, p]) => `<option value="${escapeHtml(n)}">${escapeHtml(n)}: key ${escapeHtml((p.key ?? []).join(", "))}</option>`)
      .join("");
    send(res, 200, PAGE.replace("__PROFILES__", () => opts));
    return;
  }
  if (req.method !== "POST" || req.url !== "/compare") {
    send(res, 404, "not found", "text/plain");
    return;
  }
  const ct = req.headers["content-type"] ?? "";
  const form = await new Response(Readable.toWeb(req) as unknown as ReadableStream, {
    headers: { "content-type": ct },
  }).formData();
  const fa = form.get("a");
  const fb = form.get("b");
  if (!(fa instanceof File) || !(fb instanceof File)) {
    send(res, 400, "Two files are required.", "text/plain");
    return;
  }
  const td = mkdtempSync(join(tmpdir(), "csvdiff-"));
  try {
    const pa = join(td, "a_" + basename(fa.name));
    const pb = join(td, "b_" + basename(fb.name));
    writeFileSync(pa, Buffer.from(await fa.arrayBuffer()));
    writeFileSync(pb, Buffer.from(await fb.arrayBuffer()));
    const profile = (config.profiles ?? {})[field(form, "profile")];
    const tol = Number(field(form, "tolerance") || 0);
    const opt = optionsFrom(profile, {
      key: parseList(field(form, "key")) || null,
      compare: parseList(field(form, "compare")) || null,
      ignore: parseList(field(form, "ignore")) || null,
      trim: field(form, "trim") === "true" || null,
      ignore_case: field(form, "ignore_case") === "true" || null,
      empty_is_null: field(form, "empty_is_null") === "true" || null,
      tolerance: tol || null,
    });
    if (!opt.key.length) throw new CompareError("Key columns are required.");
    const result = await compare(pa, pb, opt);
    const c = result.counts;
    const f = (n: number) => n.toLocaleString("en-US");
    const summary =
      `Matched ${f(c.matched)} (changed ${f(c.changed)}), added ${f(c.added)}, ` +
      `removed ${f(c.removed)}, ${result.meta.seconds}s.`;
    console.log(`POST /compare ${fa.name} vs ${fb.name}: ${summary}`);
    send(res, 200, render(result), "text/html; charset=utf-8", { "X-Summary": summary });
  } catch (e) {
    if (e instanceof Error) {
      send(res, 400, e.message, "text/plain; charset=utf-8");
      return;
    }
    throw e;
  } finally {
    rmSync(td, { recursive: true, force: true });
  }
}
