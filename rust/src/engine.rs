//! The engine registry and the [`compare`] entry point.

pub mod duckdb;
pub mod native;
pub mod polars;
pub mod pqdiff;
pub mod sortmerge;
pub mod turbo;

use std::fs;
use std::path::Path;
use std::time::Instant;

use chrono::{Local, SecondsFormat};

use crate::contract::{CompareResult, EngineResult, FileMeta, Meta};
use crate::error::{Error, Result};
use crate::options::{Engine, Options};

/// Runs the comparison and attaches the run's own facts to the result.
///
/// ```no_run
/// use csvdiff::{Options, compare};
/// use std::path::Path;
///
/// let mut opt = Options::with_key(["order_id", "line_no"]);
/// opt.ignore = vec!["updated_at".to_string()];
/// let result = compare(Path::new("july.csv"), Path::new("august.csv"), &mut opt)?;
/// println!("{} rows changed", result.counts.changed);
/// # Ok::<(), csvdiff::Error>(())
/// ```
pub fn compare(a_path: &Path, b_path: &Path, opt: &mut Options) -> Result<CompareResult> {
    opt.validate()?;
    for path in [a_path, b_path] {
        if !path.is_file() {
            return Err(Error::new(format!("file not found: {}", path.display())));
        }
    }

    let engine = resolve_engine(opt.engine_name()?);
    let start = Instant::now();
    let result = run(engine, a_path, b_path, opt)?;
    let seconds = (start.elapsed().as_millis() as f64) / 1000.0;

    // A Parquet pair goes to the columnar path whichever engine was asked for,
    // so the report has to say `parquet` rather than repeat the request back.
    let label = if pqdiff::is_parquet(a_path) {
        "parquet".to_string()
    } else {
        engine.label().to_string()
    };

    let meta = Meta {
        engine_meta: result.meta,
        a: file_meta(a_path),
        b: file_meta(b_path),
        engine: label,
        seconds,
        generated: Local::now().to_rfc3339_opts(SecondsFormat::Secs, false),
        options: opt.clone(),
    };
    Ok(CompareResult {
        meta,
        counts: result.counts,
        columns: result.columns,
        changed: result.changed,
        added: result.added,
        removed: result.removed,
        dup_a: result.dup_a,
        dup_b: result.dup_b,
    })
}

fn run(engine: Engine, a: &Path, b: &Path, opt: &Options) -> Result<EngineResult> {
    // Parquet is not a text format and is not read as one: it goes to the
    // columnar path, which never materialises a row, whichever engine was
    // asked for -- none of the others can read it at all. Both sides have to
    // be Parquet, because comparing a column store against a byte stream would
    // mean building rows out of one of them, and that is the cost the columnar
    // path exists to avoid.
    let (a_pq, b_pq) = (pqdiff::is_parquet(a), pqdiff::is_parquet(b));
    if a_pq != b_pq {
        return Err(Error::new(
            "one file is parquet and the other is not; convert one of them first",
        ));
    }
    if a_pq {
        return pqdiff::compare(a, b, opt)
            .map_err(|e| Error::new(format!("the parquet engine failed: {e}")));
    }

    let result = match engine {
        Engine::DuckDb => duckdb::compare(a, b, opt),
        Engine::Polars => polars::compare(a, b, opt),
        Engine::Turbo => turbo::compare(a, b, opt),
        Engine::SortMerge => sortmerge::compare(a, b, opt),
        Engine::Native => native::compare(a, b, opt),
        Engine::Auto => unreachable!("auto is resolved before this point"),
    };
    result.map_err(|e| Error::new(format!("the {engine} engine failed: {e}")))
}

/// Turns [`Engine::Auto`] into a concrete backend.
///
/// DuckDB is preferred because it streams from disk rather than holding both
/// files in memory, but only if it can actually load here.
pub fn resolve_engine(requested: Engine) -> Engine {
    if requested != Engine::Auto {
        return requested;
    }
    Engine::CONCRETE
        .into_iter()
        .find(|&candidate| available(candidate))
        .unwrap_or(Engine::Native)
}

/// Whether a backend can run in this build.
pub fn available(engine: Engine) -> bool {
    match engine {
        Engine::DuckDb => duckdb::available(),
        Engine::Polars => polars::available(),
        Engine::Turbo => turbo::available(),
        Engine::SortMerge => sortmerge::available(),
        Engine::Native => native::available(),
        Engine::Auto => false,
    }
}

fn file_meta(path: &Path) -> FileMeta {
    FileMeta {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .into_owned(),
        size: fs::metadata(path).map(|m| m.len()).unwrap_or(0),
    }
}
