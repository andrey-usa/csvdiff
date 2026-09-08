//! Writes a deterministic pair of files for benchmarking and CI.
//!
//! The recipe lives in [`csvdiff::gendata`]; this is only the command line. The
//! same rows go out as CSV, as newline-delimited JSON or as Parquet, so a format
//! benchmark compares readers rather than converters.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use csvdiff::gendata::{COLUMNS, Compression, Format, generate_as, parse_rows};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> csvdiff::Result<()> {
    let mut rows_arg = "10k".to_string();
    let mut out_dir = "data".to_string();
    let mut prefix: Option<String> = None;
    let mut seed: i64 = 7;
    let mut format = Format::Csv;
    let mut compression = Compression::None;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i + 1 < argv.len() {
        let value = argv[i + 1].clone();
        match argv[i].as_str() {
            "--rows" | "-n" => rows_arg = value,
            "--out-dir" | "-o" => out_dir = value,
            "--prefix" => prefix = Some(value),
            "--seed" => seed = value.parse().unwrap_or(7),
            "--format" | "-f" => format = Format::parse(&value)?,
            "--compression" => {
                compression = match value.trim().to_ascii_lowercase().as_str() {
                    "none" | "uncompressed" => Compression::None,
                    "zstd" => Compression::Zstd,
                    other => {
                        return Err(csvdiff::Error::new(format!(
                            "unknown compression: {other}. Choose none or zstd"
                        )));
                    }
                }
            }
            _ => {
                i += 1;
                continue; // keeps the harness forgiving about unknown flags
            }
        }
        i += 2;
    }

    let rows = parse_rows(&rows_arg)?;
    let dir = PathBuf::from(&out_dir);
    std::fs::create_dir_all(&dir)?;
    let label = prefix.unwrap_or_else(|| rows_arg.to_lowercase());
    let a = dir.join(format!("{label}_a.{}", format.extension()));
    let b = dir.join(format!("{label}_b.{}", format.extension()));

    let start = Instant::now();
    generate_as(rows, &a, &b, seed, format, compression)?;
    println!(
        "rust: {rows} rows x {} columns in {:.1}s",
        COLUMNS.len(),
        start.elapsed().as_secs_f64()
    );
    for path in [&a, &b] {
        if let Ok(meta) = std::fs::metadata(path) {
            println!("  {}  {:.1} MB", path.display(), meta.len() as f64 / 1e6);
        }
    }
    Ok(())
}
