/** The generator is part of CI, so its drift rates are asserted like any other behaviour. */
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { compare } from "../src/engine.ts";
import { makeOptions } from "../src/types.ts";
import { COLUMNS, genData, parseRows } from "../scripts/gen-data.ts";

const KEY = ["account_id", "txn_id"];

function gen(rows = 10_000) {
  const dir = mkdtempSync(join(tmpdir(), "csvdiff-gen-"));
  const a = join(dir, "a.csv");
  const b = join(dir, "b.csv");
  genData(rows, a, b);
  return { a, b };
}

test("parseRows", () => {
  assert.equal(parseRows("10k"), 10_000);
  assert.equal(parseRows("1m"), 1_000_000);
  assert.equal(parseRows("2.5M"), 2_500_000);
  assert.equal(parseRows("1,000"), 1000);
  assert.equal(parseRows("42"), 42);
  assert.equal(COLUMNS.length, 20);
});

test("shape and drift", async () => {
  const { a, b } = gen();
  const r = await compare(a, b, makeOptions({ key: KEY, ignore: ["updated_at"] }));
  const c = r.counts;
  assert.equal(r.meta.compared.length, 17); // 20 columns - 2 key - 1 ignored
  assert.equal(c.a_keys, 10_000);
  assert.equal(c.b_keys, 10_000);
  assert.equal(c.added, 10); // 0.1% each
  assert.equal(c.removed, 10);
  assert.equal(c.a_dup_keys, 1);
  assert.equal(c.b_dup_keys, 1);
  const ratio = c.changed / c.matched;
  assert.ok(ratio > 0.055 && ratio < 0.065, `~6% of matched rows differ, got ${ratio}`);
  const by = Object.fromEntries(r.columns.map((x) => [x.name, x]));
  assert.ok(by["status"]!.changed > by["amount"]!.changed && by["amount"]!.changed > 0);
  assert.equal(by["value_date"]!.blanked, by["value_date"]!.changed);
  assert.ok(by["value_date"]!.changed > 0);
  assert.equal(by["note"]!.changed, 0); // untouched columns stay untouched
});

test("deterministic", () => {
  const one = gen(2_000);
  const two = gen(2_000);
  assert.ok(readFileSync(one.a).equals(readFileSync(two.a)));
  assert.ok(readFileSync(one.b).equals(readFileSync(two.b)));
});

test("money never passes through a float", () => {
  // Two-decimal columns come from integer cents, so every implementation of the
  // generator writes the same digits whatever its rounding rule.
  const { a, b } = gen(2_000);
  for (const path of [a, b]) {
    const lines = readFileSync(path, "utf-8").split("\n").slice(1, 500).filter(Boolean);
    const at = (line: string, name: string) => line.split(",")[COLUMNS.indexOf(name)]!;
    for (const line of lines) {
      for (const column of ["amount", "fee", "balance"]) {
        assert.match(at(line, column), /^-?\d+\.\d{2}$/, `${column} is not 2dp in ${line}`);
      }
      assert.match(at(line, "rate"), /^0\.\d{4}$/);
    }
  }
});
