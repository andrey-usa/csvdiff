//! The DuckDB engine. The default, and the only one here not bounded by RAM.
//!
//! Both files are read as text, normalised, de-duplicated on the key so the
//! first occurrence joins, then full-outer-joined; only the differing cells come
//! back into Rust. DuckDB hash-joins in parallel and spills to disk, so files
//! larger than memory are fine.
//!
//! SQL is assembled by string interpolation, so identifiers go through [`q`] and
//! string literals and paths through [`lit`]. Nothing else is interpolated.

use std::fs;
use std::path::Path;

use duckdb::{Connection, Row, params};

use crate::columns::resolve;
use crate::contract::{Cell, CellDiff, ColumnStat, Counts, EngineResult, Section, Val};
use crate::error::{Error, Result};
use crate::options::Options;

pub fn compare(a_path: &Path, b_path: &Path, opt: &Options) -> Result<EngineResult> {
    let db = Connection::open_in_memory()?;

    if let Some(threads) = opt.threads {
        db.execute_batch(&format!("SET threads = {threads}"))?;
    }
    if let Some(limit) = &opt.memory_limit {
        db.execute_batch(&format!("SET memory_limit = {}", lit(limit)))?;
    }
    db.execute_batch("SET preserve_insertion_order = true")?;

    let mut read_opts = String::from("all_varchar=true, header=true, sample_size=-1");
    if let Some(d) = opt.delimiter {
        read_opts.push_str(&format!(", delim={}", lit(&d.to_string())));
    }
    let encoding = opt.encoding.to_ascii_lowercase();
    if !encoding.is_empty() && encoding != "utf-8" && encoding != "utf8" {
        read_opts.push_str(&format!(", encoding={}", lit(&opt.encoding)));
    }

    let a_cols = load(&db, "a", a_path, &read_opts)?;
    let b_cols = load(&db, "b", b_path, &read_opts)?;
    let resolved = resolve(&a_cols, &b_cols, opt)?;
    let key = &opt.key;
    let compared = &resolved.compared;
    let key_size = key.len();

    // Normalised projection plus a stable row number, so "first occurrence wins"
    // is well defined.
    for table in ["a", "b"] {
        let projection: Vec<String> = key
            .iter()
            .chain(compared)
            .map(|c| format!("{} AS {}", norm(&q(c), opt), q(c)))
            .collect();
        db.execute_batch(&format!(
            "CREATE TABLE {table} AS SELECT {}, row_number() OVER () AS _rn FROM {table}_raw;
             DROP TABLE {table}_raw;",
            projection.join(", ")
        ))?;
    }

    let kq = quoted_list(key);
    let mut counts = Counts {
        a_rows: scalar(&db, "SELECT count(*) FROM a")?,
        b_rows: scalar(&db, "SELECT count(*) FROM b")?,
        ..Counts::default()
    };

    let mut dups: Vec<Section> = Vec::with_capacity(2);
    let totals = [counts.a_rows, counts.b_rows];
    let mut dup_keys = [0i64; 2];
    let mut dup_rows = [0i64; 2];
    for (i, table) in ["a", "b"].into_iter().enumerate() {
        db.execute_batch(&format!(
            "CREATE TABLE {table}_dup AS
             SELECT {kq}, count(*) AS n FROM {table} GROUP BY ALL HAVING count(*) > 1"
        ))?;
        dup_keys[i] = scalar(&db, &format!("SELECT count(*) FROM {table}_dup"))?;
        dup_rows[i] = scalar(&db, &format!("SELECT coalesce(sum(n), 0) FROM {table}_dup"))?;

        let rows = query_rows(
            &db,
            &format!(
                "SELECT * FROM {table}_dup ORDER BY n DESC, {kq} LIMIT {}",
                opt.max_rows
            ),
            key_size + 1,
            key_size,
        )?;
        let mut cols = key.clone();
        cols.push("count".to_string());
        dups.push(Section {
            cols,
            rows,
            truncated: dup_keys[i] > opt.max_rows as i64,
        });

        // First occurrence per key joins; the rest are reported separately.
        db.execute_batch(&format!(
            "CREATE TABLE {table}1 AS SELECT * EXCLUDE(_k) FROM
             (SELECT *, row_number() OVER (PARTITION BY {kq} ORDER BY _rn) AS _k FROM {table})
             WHERE _k = 1"
        ))?;
    }
    counts.a_dup_keys = dup_keys[0];
    counts.a_dup_rows = dup_rows[0];
    counts.b_dup_keys = dup_keys[1];
    counts.b_dup_rows = dup_rows[1];
    counts.a_keys = totals[0] - dup_rows[0] + dup_keys[0];
    counts.b_keys = totals[1] - dup_rows[1] + dup_keys[1];

    build_join(&db, key, compared, opt)?;
    tally_statuses(&db, &mut counts)?;

    let columns = column_stats(&db, compared)?;
    let changed = changed_rows(&db, key, compared, opt, &counts)?;
    let added = side_rows(&db, "added", "b_", key, compared, opt, counts.added)?;
    let removed = side_rows(&db, "removed", "a_", key, compared, opt, counts.removed)?;

    if let Some(dir) = &opt.export_dir {
        export(&db, Path::new(dir), key, compared)?;
    }

    Ok(EngineResult {
        meta: resolved.meta(key, a_cols.len(), b_cols.len()),
        counts,
        columns,
        changed,
        added,
        removed,
        dup_b: dups.pop().expect("two duplicate sections"),
        dup_a: dups.pop().expect("two duplicate sections"),
    })
}

/// Reads one file into `<table>_raw` and returns its column names.
fn load(db: &Connection, table: &str, path: &Path, read_opts: &str) -> Result<Vec<String>> {
    let abs = fs::canonicalize(path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    db.execute_batch(&format!(
        "CREATE TABLE {table}_raw AS SELECT * FROM read_csv({}, {read_opts})",
        lit(&abs.to_string_lossy())
    ))?;

    let mut stmt = db.prepare(&format!("DESCRIBE {table}_raw"))?;
    let names = stmt
        .query_map(params![], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<String>, duckdb::Error>>()?;
    Ok(names)
}

/// Builds table `j`: the full outer join, with each compared column carried as
/// its A value, its B value and a "these differ" flag.
fn build_join(db: &Connection, key: &[String], compared: &[String], opt: &Options) -> Result<()> {
    let key_select: Vec<String> = key
        .iter()
        .map(|k| format!("coalesce(a.{0}, b.{0}) AS {0}", q(k)))
        .collect();
    let on: Vec<String> = key
        .iter()
        .map(|k| format!("a.{0} IS NOT DISTINCT FROM b.{0}", q(k)))
        .collect();

    let mut col_select: Vec<String> = Vec::with_capacity(3 * compared.len());
    let mut flags: Vec<String> = Vec::with_capacity(compared.len());
    for (i, c) in compared.iter().enumerate() {
        let (a, b) = (format!("a.{}", q(c)), format!("b.{}", q(c)));
        let d = diff_expr(&a, &b, opt);
        col_select.push(format!("{a} AS {}", q(&format!("a_{i}"))));
        col_select.push(format!("{b} AS {}", q(&format!("b_{i}"))));
        col_select.push(format!("{d} AS {}", q(&format!("d_{i}"))));
        flags.push(d);
    }
    let changed_expr = if flags.is_empty() {
        "false".to_string()
    } else {
        flags.join(" OR ")
    };
    let cols = if col_select.is_empty() {
        String::new()
    } else {
        format!("{}, ", col_select.join(", "))
    };

    db.execute_batch(&format!(
        "CREATE TABLE j AS SELECT {},
         CASE WHEN a._rn IS NULL THEN 'added' WHEN b._rn IS NULL THEN 'removed' ELSE 'matched' END AS _status,
         {cols}({changed_expr}) AS _changed
         FROM a1 a FULL OUTER JOIN b1 b ON {}",
        key_select.join(", "),
        on.join(" AND ")
    ))?;
    Ok(())
}

fn tally_statuses(db: &Connection, counts: &mut Counts) -> Result<()> {
    let mut stmt = db.prepare("SELECT _status, _changed, count(*) FROM j GROUP BY ALL")?;
    let rows = stmt.query_map(params![], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<bool>>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (status, is_changed, n) = row?;
        match status.as_str() {
            "matched" if is_changed.unwrap_or(false) => counts.changed += n,
            "matched" => counts.unchanged += n,
            "added" => counts.added += n,
            "removed" => counts.removed += n,
            _ => {}
        }
    }
    counts.matched = counts.unchanged + counts.changed;
    Ok(())
}

fn column_stats(db: &Connection, compared: &[String]) -> Result<Vec<ColumnStat>> {
    if compared.is_empty() {
        return Ok(Vec::new());
    }
    let mut agg: Vec<String> = Vec::with_capacity(3 * compared.len());
    for i in 0..compared.len() {
        let (a, b, d) = (
            q(&format!("a_{i}")),
            q(&format!("b_{i}")),
            q(&format!("d_{i}")),
        );
        agg.push(format!("coalesce(sum({d}::INT), 0)"));
        agg.push(format!(
            "coalesce(sum(({a} IS NOT NULL AND {b} IS NULL)::INT), 0)"
        ));
        agg.push(format!(
            "coalesce(sum(({a} IS NULL AND {b} IS NOT NULL)::INT), 0)"
        ));
    }
    let mut stmt = db.prepare(&format!(
        "SELECT {} FROM j WHERE _status = 'matched'",
        agg.join(", ")
    ))?;
    let values: Vec<i64> = stmt.query_row(params![], |row| {
        (0..3 * compared.len())
            .map(|i| row.get::<_, i64>(i))
            .collect()
    })?;

    Ok(compared
        .iter()
        .enumerate()
        .map(|(i, c)| ColumnStat {
            name: c.clone(),
            changed: values[3 * i],
            blanked: values[3 * i + 1],
            filled: values[3 * i + 2],
        })
        .collect())
}

fn changed_rows(
    db: &Connection,
    key: &[String],
    compared: &[String],
    opt: &Options,
    counts: &Counts,
) -> Result<Section> {
    let truncated = counts.changed > opt.max_rows as i64;
    if compared.is_empty() {
        return Ok(Section {
            cols: key.to_vec(),
            rows: Vec::new(),
            truncated,
        });
    }
    let key_size = key.len();
    let kq = quoted_list(key);

    let mut select = vec![kq.clone()];
    for i in 0..compared.len() {
        select.push(format!(
            "{}, {}, {}",
            q(&format!("a_{i}")),
            q(&format!("b_{i}")),
            q(&format!("d_{i}"))
        ));
    }
    let mut stmt = db.prepare(&format!(
        "SELECT {} FROM j WHERE _status = 'matched' AND _changed ORDER BY {kq} LIMIT {}",
        select.join(", "),
        opt.max_rows
    ))?;

    let n = compared.len();
    let rows = stmt.query_map(params![], move |row| {
        let mut out: Vec<Cell> = Vec::with_capacity(key_size + 1);
        for i in 0..key_size {
            out.push(Cell::Value(row.get::<_, Val>(i)?));
        }
        let mut diffs = Vec::new();
        for i in 0..n {
            let at = key_size + 3 * i;
            if row.get::<_, Option<bool>>(at + 2)?.unwrap_or(false) {
                diffs.push(CellDiff {
                    column: i,
                    a: row.get::<_, Val>(at)?,
                    b: row.get::<_, Val>(at + 1)?,
                });
            }
        }
        out.push(Cell::Diffs(diffs));
        Ok(out)
    })?;

    Ok(Section {
        cols: key.to_vec(),
        rows: rows.collect::<std::result::Result<Vec<_>, duckdb::Error>>()?,
        truncated,
    })
}

fn side_rows(
    db: &Connection,
    status: &str,
    prefix: &str,
    key: &[String],
    compared: &[String],
    opt: &Options,
    total: i64,
) -> Result<Section> {
    let kq = quoted_list(key);
    let mut select = vec![kq.clone()];
    for i in 0..compared.len() {
        select.push(q(&format!("{prefix}{i}")));
    }
    let width = key.len() + compared.len();
    let rows = query_rows(
        db,
        &format!(
            "SELECT {} FROM j WHERE _status = {} ORDER BY {kq} LIMIT {}",
            select.join(", "),
            lit(status),
            opt.max_rows
        ),
        width,
        width,
    )?;

    let mut cols = key.to_vec();
    cols.extend_from_slice(compared);
    Ok(Section {
        cols,
        rows,
        truncated: total > opt.max_rows as i64,
    })
}

/// The uncapped exports, written by DuckDB itself rather than pulled through Rust.
fn export(db: &Connection, dir: &Path, key: &[String], compared: &[String]) -> Result<()> {
    fs::create_dir_all(dir)
        .map_err(|e| Error::new(format!("cannot create export-dir {}: {e}", dir.display())))?;
    let kq = quoted_list(key);
    let aliases = |prefix: &str| -> String {
        compared
            .iter()
            .enumerate()
            .map(|(i, c)| format!(", {} AS {}", q(&format!("{prefix}{i}")), q(c)))
            .collect()
    };
    let path = |name: &str| lit(&dir.join(name).to_string_lossy());

    db.execute_batch(&format!(
        "COPY (SELECT {kq}{} FROM j WHERE _status='added' ORDER BY {kq}) TO {} (HEADER)",
        aliases("b_"),
        path("added.csv")
    ))?;
    db.execute_batch(&format!(
        "COPY (SELECT {kq}{} FROM j WHERE _status='removed' ORDER BY {kq}) TO {} (HEADER)",
        aliases("a_"),
        path("removed.csv")
    ))?;
    if !compared.is_empty() {
        let both: String = compared
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    ", {} AS {}, {} AS {}",
                    q(&format!("a_{i}")),
                    q(&format!("{c} (A)")),
                    q(&format!("b_{i}")),
                    q(&format!("{c} (B)"))
                )
            })
            .collect();
        db.execute_batch(&format!(
            "COPY (SELECT {kq}{both} FROM j WHERE _status='matched' AND _changed ORDER BY {kq}) TO {} (HEADER)",
            path("changed.csv")
        ))?;
    }
    Ok(())
}

/// Reads `width` columns, treating the first `string_cols` as text and anything
/// after that as an integer (the duplicate-key count).
fn query_rows(
    db: &Connection,
    query: &str,
    width: usize,
    string_cols: usize,
) -> Result<Vec<Vec<Cell>>> {
    let mut stmt = db.prepare(query)?;
    let rows = stmt.query_map(params![], move |row: &Row<'_>| {
        let mut out = Vec::with_capacity(width);
        for i in 0..string_cols {
            out.push(Cell::Value(row.get::<_, Val>(i)?));
        }
        for i in string_cols..width {
            out.push(Cell::Count(row.get::<_, i64>(i)?));
        }
        Ok(out)
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, duckdb::Error>>()?)
}

fn scalar(db: &Connection, query: &str) -> Result<i64> {
    let mut stmt = db.prepare(query)?;
    Ok(stmt
        .query_row(params![], |row| row.get::<_, Option<i64>>(0))?
        .unwrap_or(0))
}

/// Wraps a column expression in whatever `--trim`, `--ignore-case` and
/// `--empty-is-null` ask for.
fn norm(expr: &str, opt: &Options) -> String {
    let mut out = expr.to_string();
    if opt.trim {
        out = format!("trim({out})");
    }
    if opt.ignore_case {
        out = format!("lower({out})");
    }
    if opt.empty_is_null {
        out = format!("nullif({out}, '')");
    }
    out
}

/// `IS DISTINCT FROM`, or a tolerance comparison where both sides are numbers.
fn diff_expr(a: &str, b: &str, opt: &Options) -> String {
    if opt.tolerance > 0.0 {
        format!(
            "(CASE WHEN try_cast({a} AS DOUBLE) IS NOT NULL AND try_cast({b} AS DOUBLE) IS NOT NULL \
             THEN abs(try_cast({a} AS DOUBLE) - try_cast({b} AS DOUBLE)) > {} \
             ELSE ({a} IS DISTINCT FROM {b}) END)",
            opt.tolerance
        )
    } else {
        format!("({a} IS DISTINCT FROM {b})")
    }
}

fn quoted_list(names: &[String]) -> String {
    names.iter().map(|n| q(n)).collect::<Vec<_>>().join(", ")
}

/// Quotes a SQL identifier.
fn q(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Quotes a SQL string literal.
fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Whether DuckDB can actually open a database here.
pub fn available() -> bool {
    Connection::open_in_memory().is_ok()
}
