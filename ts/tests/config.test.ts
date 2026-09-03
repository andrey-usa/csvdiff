import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { loadConfig, optionsFrom, parseList } from "../src/config.ts";
import { detectDelimiter, parseCsv } from "../src/csv.ts";
import { DEFAULT_OPTIONS } from "../src/types.ts";

test("parseList", () => {
  assert.deepEqual(parseList("a, b ,c"), ["a", "b", "c"]);
  assert.deepEqual(parseList(""), []);
  assert.deepEqual(parseList(["x", "y"]), ["x", "y"]);
  assert.equal(parseList(null), null);
  assert.equal(parseList(undefined), null);
});

test("optionsFrom merges profile and overrides, overrides win when set", () => {
  const profile = { key: ["id"], trim: true, tolerance: 0.005, compare: ["qty"], unknown: 1 } as Record<string, unknown>;
  const opt = optionsFrom(profile, { tolerance: null, ignore: ["updated_at"], engine: "native", key: undefined });
  assert.deepEqual(opt.key, ["id"]);
  assert.equal(opt.trim, true);
  assert.equal(opt.tolerance, 0.005);
  assert.deepEqual(opt.compare, ["qty"]);
  assert.deepEqual(opt.ignore, ["updated_at"]);
  assert.equal(opt.engine, "native");
  assert.equal(opt.max_rows, DEFAULT_OPTIONS.max_rows);
  assert.ok(!("unknown" in opt));
  assert.deepEqual(optionsFrom(null, {}).key, []);
});

test("loadConfig reads TOML profiles", () => {
  const dir = mkdtempSync(join(tmpdir(), "csvdiff-cfg-"));
  const p = join(dir, "csvdiff.toml");
  writeFileSync(
    p,
    `[profiles.orders]\nkey = ["order_id", "line_no"]\nignore = ["updated_at"]\ntrim = true\ntolerance = 0.005\n`,
  );
  const cfg = loadConfig(p);
  assert.deepEqual(cfg.profiles?.["orders"]?.key, ["order_id", "line_no"]);
  assert.equal(cfg.profiles?.["orders"]?.tolerance, 0.005);
  assert.deepEqual(loadConfig(join(dir, "missing.toml")), {});
});

test("csv parser: quotes, empties, CRLF, delimiter detection", () => {
  assert.equal(detectDelimiter("a;b;c\n1;2;3"), ";");
  assert.equal(detectDelimiter("a\tb\n"), "\t");
  assert.equal(detectDelimiter("single\n"), ",");
  const rows = parseCsv('id,name,note\r\n1,"Smith, J","say ""hi"""\r\n2,,""\r\n3,x\n', ",");
  assert.deepEqual(rows, [
    ["id", "name", "note"],
    ["1", "Smith, J", 'say "hi"'],
    ["2", null, null],
    ["3", "x"],
  ]);
  assert.deepEqual(parseCsv("a,b\n1,\n", ","), [["a", "b"], ["1", null]]);
  assert.deepEqual(parseCsv('a\n"multi\nline"\n', ","), [["a"], ["multi\nline"]]);
});
