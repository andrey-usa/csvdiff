//! The engine registry and the [`compare`] entry point.

pub mod native;
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

    // Only the byte-level engine reads JSON and Parquet, so an input in either
    // format decides `auto` on its own rather than being handed to a CSV
    // reader and reported as the parse error it makes of a binary footer.
    let requested = opt.engine_name()?;
    let engine = if requested == Engine::Auto
        && (turbo::only_this_engine_reads(a_path) || turbo::only_this_engine_reads(b_path))
    {
        Engine::Turbo
    } else {
        resolve_engine(requested)
    };
    let start = Instant::now();
    let result = run(engine, a_path, b_path, opt)?;
    let seconds = (start.elapsed().as_millis() as f64) / 1000.0;

    // A Parquet pair goes to the columnar path whichever engine was asked for,
    // so the report has to say `parquet` rather than repeat the request back.
    // A mixed pair does not: that ran on `turbo`, and saying `parquet` would
    // claim a path it did not take.
    let label = if pqdiff::is_parquet(a_path) && pqdiff::is_parquet(b_path) {
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
    // Parquet is not a text format and is not read as one. Two Parquet files go
    // to the columnar path, which never materialises a row: it joins on the key
    // columns and then compares whole columns as integers, whichever engine was
    // asked for -- none of the others can read Parquet at all.
    //
    // One Parquet file against a text one has no column to compare a byte
    // stream against, so that path is not available. `turbo` reads it anyway,
    // by decoding the pages into rows (`turbo/parquet.rs`); it costs what the
    // columnar path exists to avoid, and it answers the question rather than
    // refusing it.
    let (a_pq, b_pq) = (pqdiff::is_parquet(a), pqdiff::is_parquet(b));
    if a_pq && b_pq {
        return pqdiff::compare(a, b, opt)
            .map_err(|e| Error::new(format!("the parquet engine failed: {e}")));
    }
    if (a_pq || b_pq) && !matches!(engine, Engine::Turbo | Engine::Auto) {
        return Err(Error::new(format!(
            "the {engine} engine cannot read parquet; use --engine turbo, \
             or convert the other side to parquet too"
        )));
    }
    if a_pq || b_pq {
        return turbo::compare(a, b, opt)
            .map_err(|e| Error::new(format!("the turbo engine failed: {e}")));
    }

    let result = match engine {
        Engine::Turbo => turbo::compare(a, b, opt),
        Engine::SortMerge => sortmerge::compare(a, b, opt),
        Engine::Native => native::compare(a, b, opt),
        Engine::Auto => unreachable!("auto is resolved before this point"),
    };
    result.map_err(|e| Error::new(format!("the {engine} engine failed: {e}")))
}

/// Turns [`Engine::Auto`] into a concrete backend.
///
/// `turbo` is preferred: it is the byte-level engine every benchmark here
/// measures, and the only one that reads Parquet and newline-delimited JSON.
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
