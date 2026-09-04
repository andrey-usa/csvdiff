//! The Polars engine: columnar, multi-threaded, in memory.
//!
//! Both files are materialised as all-String frames, the key is de-duplicated
//! keeping the first occurrence, the two frames are full-joined and the
//! per-column difference flags are computed as expressions. This is the same
//! pipeline the TypeScript port runs through the Node binding — the difference
//! is only which language holds the handle.
//!
//! Parity note: Polars reads an unquoted empty field as null but keeps a
//! *quoted* empty (`""`) as an empty string, where DuckDB and the native reader
//! both yield null. [`empty_to_null_expr`] restores that agreement before any
//! user-supplied normalisation runs, so an engine swap never changes counts.

use std::fs;
use std::path::Path;

use polars::prelude::*;

use crate::columns::{detect_delimiter, resolve};
use crate::contract::{Cell, CellDiff, ColumnStat, Counts, EngineResult, Section, Val};
use crate::error::{Error, Result};
use crate::options::Options;

const SUFFIX: &str = "__csvdiff_b";
const STATUS: &str = "_status";
const CHANGED: &str = "_changed";
const IN_A: &str = "_ina";
const IN_B: &str = "_inb";

pub fn compare(a_path: &Path, b_path: &Path, opt: &Options) -> Result<EngineResult> {
    let raw_a = read_csv(a_path, opt)?;
    let raw_b = read_csv(b_path, opt)?;
    let a_cols = names(&raw_a);
    let b_cols = names(&raw_b);
    let resolved = resolve(&a_cols, &b_cols, opt)?;

    let key = &opt.key;
    let compared = &resolved.compared;
    let nk = key.len();
    let wanted: Vec<String> = key.iter().chain(compared).cloned().collect();

    let projection: Vec<Expr> = wanted.iter().map(|c| norm_expr(c, opt)).collect();
    let a = raw_a.lazy().select(&projection).collect()?;
    let b = raw_b.lazy().select(&projection).collect()?;

    let mut counts = Counts {
        a_rows: a.height() as i64,
        b_rows: b.height() as i64,
        ..Counts::default()
    };

    let (dup_a, a_dup_keys, a_dup_rows) = duplicate_section(&a, key, opt)?;
    let (dup_b, b_dup_keys, b_dup_rows) = duplicate_section(&b, key, opt)?;
    counts.a_dup_keys = a_dup_keys;
    counts.a_dup_rows = a_dup_rows;
    counts.b_dup_keys = b_dup_keys;
    counts.b_dup_rows = b_dup_rows;
    counts.a_keys = counts.a_rows - a_dup_rows + a_dup_keys;
    counts.b_keys = counts.b_rows - b_dup_rows + b_dup_keys;

    // First occurrence of each key takes part in the join.
    let key_exprs: Vec<Expr> = key.iter().map(|k| col(k.as_str())).collect();
    let unique = |df: DataFrame, marker: &str| -> LazyFrame {
        df.lazy()
            .unique_stable(
                Some(by_name(key.iter().map(String::as_str), true, false)),
                UniqueKeepStrategy::First,
            )
            .with_column(lit(1i32).alias(marker))
    };
    let a1 = unique(a, IN_A);
    let b1 = unique(b, IN_B);

    let joined = a1.join(
        b1,
        &key_exprs,
        &key_exprs,
        JoinArgs::new(JoinType::Full).with_suffix(Some(SUFFIX.into())),
    );

    // A full join keeps both key columns; rebuild one key per row plus the status.
    let b_name = |c: &str| {
        if wanted.iter().any(|w| w == c) {
            format!("{c}{SUFFIX}")
        } else {
            c.to_string()
        }
    };
    let mut selection: Vec<Expr> = key
        .iter()
        .map(|k| {
            when(col(k.as_str()).is_null())
                .then(col(b_name(k).as_str()))
                .otherwise(col(k.as_str()))
                .alias(k.as_str())
        })
        .collect();
    selection.push(
        when(col(IN_A).is_null())
            .then(lit("added"))
            .when(col(IN_B).is_null())
            .then(lit("removed"))
            .otherwise(lit("matched"))
            .alias(STATUS),
    );
    for (i, c) in compared.iter().enumerate() {
        let (x, y) = (col(c.as_str()), col(b_name(c).as_str()));
        selection.push(x.clone().alias(a_col(i)));
        selection.push(y.clone().alias(b_col(i)));
        selection.push(diff_expr(x, y, opt).alias(d_col(i)));
    }

    let changed_flag = (0..compared.len())
        .map(|i| col(d_col(i)))
        .reduce(|acc, e| acc.or(e))
        .unwrap_or_else(|| lit(false));
    let flat = joined
        .select(&selection)
        .with_column(changed_flag.alias(CHANGED))
        .collect()?;

    let matched = filter_status(&flat, "matched")?;
    let changed_only = matched
        .clone()
        .lazy()
        .filter(col(CHANGED).eq(lit(true)))
        .collect()?;
    counts.matched = matched.height() as i64;
    counts.changed = changed_only.height() as i64;
    counts.unchanged = counts.matched - counts.changed;
    let added_rows = filter_status(&flat, "added")?;
    let removed_rows = filter_status(&flat, "removed")?;
    counts.added = added_rows.height() as i64;
    counts.removed = removed_rows.height() as i64;

    let columns = column_stats(&matched, compared)?;

    let sorted_changed = sort_by_key(changed_only, key)?;
    let changed = changed_section(&sorted_changed, nk, compared, opt, counts.changed)?;
    let sorted_added = sort_by_key(added_rows, key)?;
    let sorted_removed = sort_by_key(removed_rows, key)?;
    let added = side_section(&sorted_added, key, compared, B_PREFIX, opt, counts.added)?;
    let removed = side_section(
        &sorted_removed,
        key,
        compared,
        A_PREFIX,
        opt,
        counts.removed,
    )?;

    if let Some(dir) = &opt.export_dir {
        export(
            Path::new(dir),
            &sorted_changed,
            &sorted_added,
            &sorted_removed,
            key,
            compared,
        )?;
    }

    Ok(EngineResult {
        meta: resolved.meta(key, a_cols.len(), b_cols.len()),
        counts,
        columns,
        changed,
        added,
        removed,
        dup_a,
        dup_b,
    })
}

const A_PREFIX: &str = "a_";
const B_PREFIX: &str = "b_";

fn a_col(i: usize) -> PlSmallStr {
    format!("{A_PREFIX}{i}").into()
}

fn b_col(i: usize) -> PlSmallStr {
    format!("{B_PREFIX}{i}").into()
}

fn d_col(i: usize) -> PlSmallStr {
    format!("d_{i}").into()
}

fn read_csv(path: &Path, opt: &Options) -> Result<DataFrame> {
    // Polars has no delimiter auto-detection, and the other engines do, so the
    // header line is sniffed here rather than letting a semicolon-separated file
    // parse as one wide column.
    let separator = match opt.delimiter_byte()? {
        Some(d) => d,
        None => detect_delimiter(&first_line(path)?),
    };
    // infer_schema_length(0) keeps every column a string: values are compared as
    // text, so no type inference may happen.
    let df = LazyCsvReader::new(PlRefPath::new(path.to_string_lossy().as_ref()))
        .with_has_header(true)
        .with_infer_schema_length(Some(0))
        .map_parse_options(move |mut parse| {
            parse.separator = separator;
            parse
        })
        .finish()
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?
        .collect()
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    if df.width() == 0 {
        return Err(Error::new(format!(
            "file has no header row: {}",
            path.display()
        )));
    }
    Ok(df)
}

/// The first line of a file, for delimiter sniffing.
fn first_line(path: &Path) -> Result<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    let mut line = String::new();
    std::io::BufReader::new(file)
        .read_line(&mut line)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    Ok(line)
}

fn names(df: &DataFrame) -> Vec<String> {
    df.get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Quoted-empty parity with DuckDB: `""` is an absent value, not a zero-length string.
fn empty_to_null_expr(e: Expr) -> Expr {
    when(e.clone().eq(lit(""))).then(lit(NULL)).otherwise(e)
}

fn norm_expr(name: &str, opt: &Options) -> Expr {
    let mut e = empty_to_null_expr(col(name));
    if opt.trim {
        e = e.str().strip_chars(lit(NULL));
    }
    if opt.ignore_case {
        e = e.str().to_lowercase();
    }
    if opt.empty_is_null {
        e = empty_to_null_expr(e);
    }
    e.alias(name)
}

/// SQL `a IS DISTINCT FROM b`, with the numeric tolerance applied where both
/// sides parse as numbers.
fn diff_expr(a: Expr, b: Expr, opt: &Options) -> Expr {
    let both_null = a.clone().is_null().and(b.clone().is_null());
    let one_null = a.clone().is_null().or(b.clone().is_null());
    let unequal = if opt.tolerance > 0.0 {
        let na = a.clone().cast(DataType::Float64);
        let nb = b.clone().cast(DataType::Float64);
        when(na.clone().is_not_null().and(nb.clone().is_not_null()))
            .then((na - nb).abs().gt(lit(opt.tolerance)))
            .otherwise(a.clone().neq(b.clone()))
    } else {
        a.clone().neq(b.clone())
    };
    when(both_null)
        .then(lit(false))
        .when(one_null)
        .then(lit(true))
        .otherwise(unequal)
}

fn filter_status(df: &DataFrame, status: &str) -> Result<DataFrame> {
    Ok(df
        .clone()
        .lazy()
        .filter(col(STATUS).eq(lit(status)))
        .collect()?)
}

fn sort_by_key(df: DataFrame, key: &[String]) -> Result<DataFrame> {
    let options = SortMultipleOptions::default()
        .with_order_descending(false)
        .with_nulls_last(true)
        .with_maintain_order(true);
    let by: Vec<String> = key.to_vec();
    Ok(df.sort(by, options)?)
}

/// The duplicate-key list: most duplicated first, then by key.
fn duplicate_section(df: &DataFrame, key: &[String], opt: &Options) -> Result<(Section, i64, i64)> {
    let key_exprs: Vec<Expr> = key.iter().map(|k| col(k.as_str())).collect();
    let grouped = df
        .clone()
        .lazy()
        .group_by(&key_exprs)
        .agg([len().alias("n")])
        .filter(col("n").gt(lit(1u32)))
        .collect()?;

    let dup_keys = grouped.height() as i64;
    let dup_rows = if dup_keys == 0 {
        0
    } else {
        grouped
            .column("n")?
            .cast(&DataType::Int64)?
            .i64()?
            .sum()
            .unwrap_or(0)
    };

    let mut by: Vec<String> = vec!["n".to_string()];
    by.extend(key.iter().cloned());
    let mut descending = vec![true];
    descending.extend(key.iter().map(|_| false));
    let sorted = grouped.sort(
        by,
        SortMultipleOptions::default()
            .with_order_descending_multi(descending)
            .with_nulls_last(true)
            .with_maintain_order(true),
    )?;

    let limit = opt.max_rows.min(sorted.height());
    let counts_column = sorted.column("n")?.cast(&DataType::Int64)?;
    let counts_column = counts_column.i64()?;
    let key_columns = string_columns(&sorted, key)?;

    let rows = (0..limit)
        .map(|i| {
            let mut row: Vec<Cell> = key_columns
                .iter()
                .map(|c| Cell::Value(c.get(i).map(str::to_string)))
                .collect();
            row.push(Cell::Count(counts_column.get(i).unwrap_or(0)));
            row
        })
        .collect();

    let mut cols = key.to_vec();
    cols.push("count".to_string());
    Ok((
        Section {
            cols,
            rows,
            truncated: dup_keys > opt.max_rows as i64,
        },
        dup_keys,
        dup_rows,
    ))
}

/// Per-column stats over matched rows only.
fn column_stats(matched: &DataFrame, compared: &[String]) -> Result<Vec<ColumnStat>> {
    if compared.is_empty() {
        return Ok(Vec::new());
    }
    if matched.height() == 0 {
        return Ok(compared
            .iter()
            .map(|c| ColumnStat {
                name: c.clone(),
                changed: 0,
                blanked: 0,
                filled: 0,
            })
            .collect());
    }

    let mut aggs: Vec<Expr> = Vec::with_capacity(3 * compared.len());
    for i in 0..compared.len() {
        aggs.push(
            col(d_col(i))
                .cast(DataType::Int64)
                .sum()
                .alias(format!("c{i}")),
        );
        aggs.push(
            col(a_col(i))
                .is_not_null()
                .and(col(b_col(i)).is_null())
                .cast(DataType::Int64)
                .sum()
                .alias(format!("bl{i}")),
        );
        aggs.push(
            col(a_col(i))
                .is_null()
                .and(col(b_col(i)).is_not_null())
                .cast(DataType::Int64)
                .sum()
                .alias(format!("fi{i}")),
        );
    }
    let stats = matched.clone().lazy().select(&aggs).collect()?;
    let scalar = |name: String| -> Result<i64> {
        Ok(stats.column(name.as_str())?.i64()?.get(0).unwrap_or(0))
    };

    compared
        .iter()
        .enumerate()
        .map(|(i, c)| {
            Ok(ColumnStat {
                name: c.clone(),
                changed: scalar(format!("c{i}"))?,
                blanked: scalar(format!("bl{i}"))?,
                filled: scalar(format!("fi{i}"))?,
            })
        })
        .collect()
}

/// Changed rows, as the sparse cell diffs the report embeds.
fn changed_section(
    sorted: &DataFrame,
    nk: usize,
    compared: &[String],
    opt: &Options,
    total: i64,
) -> Result<Section> {
    let truncated = total > opt.max_rows as i64;
    let key_cols: Vec<String> = sorted
        .get_column_names()
        .iter()
        .take(nk)
        .map(|s| s.to_string())
        .collect();
    if compared.is_empty() || sorted.height() == 0 {
        return Ok(Section {
            cols: key_cols,
            rows: Vec::new(),
            truncated,
        });
    }

    let keys = string_columns_by_index(sorted, 0..nk)?;
    let mut a_side = Vec::with_capacity(compared.len());
    let mut b_side = Vec::with_capacity(compared.len());
    let mut flags = Vec::with_capacity(compared.len());
    for i in 0..compared.len() {
        a_side.push(sorted.column(&a_col(i))?.str()?.clone());
        b_side.push(sorted.column(&b_col(i))?.str()?.clone());
        flags.push(sorted.column(&d_col(i))?.bool()?.clone());
    }

    let limit = opt.max_rows.min(sorted.height());
    let rows = (0..limit)
        .map(|r| {
            let mut row: Vec<Cell> = keys
                .iter()
                .map(|c| Cell::Value(c.get(r).map(str::to_string)))
                .collect();
            let diffs = (0..compared.len())
                .filter(|&i| flags[i].get(r).unwrap_or(false))
                .map(|i| CellDiff {
                    column: i,
                    a: a_side[i].get(r).map(str::to_string),
                    b: b_side[i].get(r).map(str::to_string),
                })
                .collect();
            row.push(Cell::Diffs(diffs));
            row
        })
        .collect();

    Ok(Section {
        cols: key_cols,
        rows,
        truncated,
    })
}

/// The added or removed rows, taken from whichever side of the join they came from.
fn side_section(
    sorted: &DataFrame,
    key: &[String],
    compared: &[String],
    prefix: &str,
    opt: &Options,
    total: i64,
) -> Result<Section> {
    let mut cols = key.to_vec();
    cols.extend_from_slice(compared);
    let rows = side_values(sorted, key, compared, prefix, Some(opt.max_rows))?
        .into_iter()
        .map(|row| row.into_iter().map(Cell::Value).collect())
        .collect();
    Ok(Section {
        cols,
        rows,
        truncated: total > opt.max_rows as i64,
    })
}

/// The raw values behind [`side_section`], uncapped when `limit` is `None`.
fn side_values(
    sorted: &DataFrame,
    key: &[String],
    compared: &[String],
    prefix: &str,
    limit: Option<usize>,
) -> Result<Vec<Vec<Val>>> {
    let mut columns = string_columns(sorted, key)?;
    for i in 0..compared.len() {
        columns.push(sorted.column(&format!("{prefix}{i}"))?.str()?.clone());
    }
    let rows = limit.unwrap_or(sorted.height()).min(sorted.height());
    Ok((0..rows)
        .map(|r| {
            columns
                .iter()
                .map(|c| c.get(r).map(str::to_string))
                .collect()
        })
        .collect())
}

fn export(
    dir: &Path,
    changed: &DataFrame,
    added: &DataFrame,
    removed: &DataFrame,
    key: &[String],
    compared: &[String],
) -> Result<()> {
    fs::create_dir_all(dir)
        .map_err(|e| Error::new(format!("cannot create export-dir {}: {e}", dir.display())))?;
    let mut cols = key.to_vec();
    cols.extend_from_slice(compared);

    crate::sections::write_rows(
        &dir.join("added.csv"),
        &cols,
        &side_values(added, key, compared, B_PREFIX, None)?,
    )?;
    crate::sections::write_rows(
        &dir.join("removed.csv"),
        &cols,
        &side_values(removed, key, compared, A_PREFIX, None)?,
    )?;

    let mut both = key.to_vec();
    for c in compared {
        both.push(format!("{c} (A)"));
        both.push(format!("{c} (B)"));
    }
    let mut columns = string_columns(changed, key)?;
    for i in 0..compared.len() {
        columns.push(changed.column(&a_col(i))?.str()?.clone());
        columns.push(changed.column(&b_col(i))?.str()?.clone());
    }
    let rows: Vec<Vec<Val>> = (0..changed.height())
        .map(|r| {
            columns
                .iter()
                .map(|c| c.get(r).map(str::to_string))
                .collect()
        })
        .collect();
    crate::sections::write_rows(&dir.join("changed.csv"), &both, &rows)
}

fn string_columns(df: &DataFrame, names: &[String]) -> Result<Vec<StringChunked>> {
    names
        .iter()
        .map(|n| Ok(df.column(n.as_str())?.str()?.clone()))
        .collect()
}

fn string_columns_by_index(
    df: &DataFrame,
    range: std::ops::Range<usize>,
) -> Result<Vec<StringChunked>> {
    range.map(|i| Ok(df.columns()[i].str()?.clone())).collect()
}

/// Polars is compiled in, so it is always available.
pub fn available() -> bool {
    true
}
