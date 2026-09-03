---
name: csvdiff-report
description: Rules and workflow for changing the csvdiff HTML report — the template in csvdiff/report.py, its embedded CSS/JS, the gzip payload, and the virtualised grid. Use when adding a tab, column, filter, keyboard shortcut, or any change to what the report displays or how large it is.
---

# Changing the csvdiff report

The whole report is one Python string, `_TEMPLATE`, in `csvdiff/report.py`. There is no build
step, no bundler and no dependency. `render()` serialises the result dict to JSON, gzips it,
base64s it into a `<script type="application/gzip">` block, and the page decodes it on load with
`DecompressionStream`.

## Before editing

Read the result contract at the top of `csvdiff/engine.py`. The report renders that dict and
nothing else. If the report needs data it does not have, add a field to the contract in **both**
engines and assert it in `tests/test_engine.py` — never compute it in JavaScript from partial data.

## Rules

- No external references. No CDN, no font import, no image URL. CI greps for `src=` and `href=`
  pointing outside the document and fails the build.
- No browser storage. The report is opened from `file://`, from an email attachment, and from an
  artifact download; `localStorage` and `fetch` are unavailable or blocked in those contexts.
- Keep the payload sparse. Changed rows are `[key..., [[colIndex, old, new], ...]]`. Do not embed
  whole rows, do not repeat column names per cell, do not add a field that duplicates something
  derivable in the page.
- Keep the grid virtualised. `paint()` renders only the rows in the viewport plus a small margin.
  Any new tab must go through `build()` and `paint()`, not a full `innerHTML` table dump — the
  Columns tab is the one exception because it has one row per column, bounded by schema width.
- Escape everything. Values pass through `esc()` or `cellv()`. Column names are values too.
- Keyboard and focus behaviour is part of the feature: `/` focuses the filter, `Esc` clears or
  closes, `1`-`5` switch tabs, arrows move the row selection. A new tab gets a number; a new
  control gets a visible focus ring.

## After editing

```bash
pytest
python scripts/bench.py --rows 10k --engine duckdb   # or pandas locally
```

Then check the generated `bench/10k-*.html`:

1. Open it and exercise the changed path — filter, sort, switch tabs, open the drawer.
2. Confirm the file size did not jump. A 10k run with ~600 changed rows lands around 33 KB; a 1M
   run with 50k embedded changed rows lands near 1.1 MB. A large increase means the payload
   stopped being sparse.
3. Extract the inline JS and syntax-check it, since nothing else will:

```bash
python - <<'PY'
import re, subprocess
js = re.findall(r"<script>(.*?)</script>", open("bench/10k-duckdb.html").read(), re.S)[0]
open("/tmp/report.js", "w").write(js)
print(subprocess.run(["node", "--check", "/tmp/report.js"], capture_output=True, text=True))
PY
```
