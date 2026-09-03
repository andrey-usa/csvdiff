import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

import { compare, isIdentical } from "../src/engine.ts";
import { render } from "../src/report.ts";
import { makeOptions } from "../src/types.ts";

const EX = fileURLToPath(new URL("../../examples/", import.meta.url));
const A = join(EX, "orders_2026-08.csv");
const B = join(EX, "orders_2026-09.csv");
const CLI = fileURLToPath(new URL("../src/cli.ts", import.meta.url));
const KEY = ["order_id", "line_no"];

for (const engine of ["duckdb", "native"] as const) {
  test(`counts and tolerance (${engine})`, async () => {
    const r = await compare(A, B, makeOptions({ key: KEY, ignore: ["updated_at"], tolerance: 0.005, engine }));
    const c = r.counts;
    assert.equal(r.meta.engine, engine);
    assert.equal(c.a_rows, 800);
    assert.equal(c.b_rows, 799);
    assert.equal(c.added, 10);
    assert.equal(c.removed, 12);
    assert.equal(c.changed, 87);
    assert.equal(c.a_dup_keys, 1);
    assert.equal(c.b_dup_keys, 1);
    const by = Object.fromEntries(r.columns.map((x) => [x.name, x.changed]));
    assert.equal(by["unit_price"], 0); // inside tolerance
    assert.deepEqual(r.meta.only_in_b, ["carrier"]);
    assert.match(render(r), /<html/);
  });
}

test("engines agree on counts, columns and embedded rows", async () => {
  for (const extra of [{}, { tolerance: 0.005 }, { trim: true, ignore_case: true, empty_is_null: true }]) {
    const opts = { key: KEY, ignore: ["updated_at"], ...extra };
    const d = await compare(A, B, makeOptions({ ...opts, engine: "duckdb" }));
    const n = await compare(A, B, makeOptions({ ...opts, engine: "native" }));
    assert.deepEqual(n.counts, d.counts);
    assert.deepEqual(n.columns, d.columns);
    assert.deepEqual(n.changed, d.changed);
    assert.deepEqual(n.added, d.added);
    assert.deepEqual(n.removed, d.removed);
    assert.deepEqual(n.dup_a, d.dup_a);
    assert.deepEqual(n.dup_b, d.dup_b);
    assert.deepEqual(
      { ...n.meta, engine: 0, seconds: 0, generated: 0, options: 0 },
      { ...d.meta, engine: 0, seconds: 0, generated: 0, options: 0 },
    );
  }
});

test("identical files and CLI exit codes", async () => {
  assert.ok(isIdentical(await compare(A, A, makeOptions({ key: KEY }))));
  const tmp = mkdtempSync(join(tmpdir(), "csvdiff-test-"));
  const run = (...args: string[]) => spawnSync(process.execPath, [CLI, "compare", ...args], { encoding: "utf-8" });

  const same = run(A, A, "-k", "order_id,line_no", "-o", join(tmp, "same.html"));
  assert.equal(same.status, 0, same.stderr);

  const missing = run(A, B, "-k", "missing", "-o", join(tmp, "missing.html"));
  assert.equal(missing.status, 2);
  assert.match(missing.stderr, /Key column\(s\) missing/);

  const diff = run(A, B, "-k", "order_id,line_no", "-i", "updated_at", "-o", join(tmp, "diff.html"), "--json", join(tmp, "s.json"));
  assert.equal(diff.status, 1, diff.stderr);
  assert.match(diff.stdout, /A 800 rows \| B 799 rows \| matched 787 \(changed 102\)/);
  const summary = JSON.parse(readFileSync(join(tmp, "s.json"), "utf-8"));
  assert.equal(summary.counts.changed, 102);

  const dups = run(A, B, "-k", "order_id,line_no", "--fail-on-dups", "-o", join(tmp, "dups.html"));
  assert.equal(dups.status, 3);
});

test("report is one self-contained file whose payload round-trips", async () => {
  const r = await compare(A, B, makeOptions({ key: KEY, ignore: ["updated_at"] }));
  const html = render(r);
  const external = [...html.matchAll(/(?:src|href)="(?!#)([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(external, []);
  const m = html.match(/<script id="payload" type="application\/gzip">([^<]+)<\/script>/);
  assert.ok(m);
  const decoded = JSON.parse(gunzipSync(Buffer.from(m![1]!, "base64")).toString("utf-8"));
  assert.deepEqual(decoded.counts, r.counts);
  assert.equal(decoded.changed.rows.length, r.counts.changed);

  const plain = render(r, false);
  assert.match(plain, /type="application\/json"/);
  assert.ok(!plain.includes("</script>x"));
});
