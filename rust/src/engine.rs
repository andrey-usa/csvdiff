//! The engine registry and the [`compare`] entry point.

pub mod duckdb;
pub mod native;
pub mod polars;
pub mod sortmerge;

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

    let meta = Meta {
        engine_meta: result.meta,
        a: file_meta(a_path),
        b: file_meta(b_path),
        engine: engine.label().to_string(),
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
    let result = match engine {
        Engine::DuckDb => duckdb::compare(a, b, opt),
        Engine::Polars => polars::compare(a, b, opt),
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
