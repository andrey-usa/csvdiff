//! The `csv`-crate engine: parse both files, join in a hash map.
//!
//! Both files are held in memory, so this is for data that fits comfortably in
//! RAM. It is the dependency-light baseline the other engines are measured
//! against.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use crate::columns::{detect_delimiter, empty_to_null, normalise, resolve};
use crate::contract::{EngineResult, Val};
use crate::error::{Error, Result};
use crate::options::Options;
use crate::rowstore::{RowStore, join};
use crate::sections::assemble;

pub fn compare(a_path: &Path, b_path: &Path, opt: &Options) -> Result<EngineResult> {
    let a_header = read_header(a_path, opt)?;
    let b_header = read_header(b_path, opt)?;
    let resolved = resolve(&a_header, &b_header, opt)?;

    let a = read_store(a_path, &a_header, &resolved.compared, opt)?;
    let b = read_store(b_path, &b_header, &resolved.compared, opt)?;

    let dup_a = a.duplicate_section();
    let dup_b = b.duplicate_section();
    let joined = join(&a, &b, opt);
    let meta = resolved.meta(&opt.key, a_header.len(), b_header.len());
    assemble(meta, joined, dup_a, dup_b, opt, &resolved.compared)
}

/// Opens a CSV reader with the delimiter forced or sniffed from the header.
fn reader(path: &Path, opt: &Options) -> Result<csv::Reader<File>> {
    let mut file =
        File::open(path).map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;

    let delimiter = match opt.delimiter_byte()? {
        Some(d) => d,
        None => {
            let mut line = String::new();
            BufReader::new(&mut file)
                .read_line(&mut line)
                .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
            file.seek(SeekFrom::Start(0))?;
            detect_delimiter(&line)
        }
    };

    Ok(csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
        // Ragged rows are padded rather than rejected: a short row means absent
        // values, which is a difference to report, not a file to refuse.
        .flexible(true)
        .from_reader(file))
}

fn read_header(path: &Path, opt: &Options) -> Result<Vec<String>> {
    let mut rdr = reader(path, opt)?;
    let headers = rdr
        .headers()
        .map_err(|_| Error::new(format!("file has no header row: {}", path.display())))?;
    if headers.is_empty() {
        return Err(Error::new(format!(
            "file has no header row: {}",
            path.display()
        )));
    }
    Ok(headers.iter().map(str::to_string).collect())
}

/// Reads a file straight into a [`RowStore`], projecting and normalising as it goes.
fn read_store(
    path: &Path,
    header: &[String],
    compared: &[String],
    opt: &Options,
) -> Result<RowStore> {
    let key_size = opt.key.len();
    let mut index: Vec<Option<usize>> = Vec::with_capacity(key_size + compared.len());
    for name in opt.key.iter().chain(compared) {
        index.push(header.iter().position(|c| c == name));
    }

    let mut store = RowStore::new(opt, compared);
    let mut rdr = reader(path, opt)?;
    let mut record = csv::StringRecord::new();
    while rdr
        .read_record(&mut record)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?
    {
        let row: Vec<Val> = index
            .iter()
            .map(|at| match at {
                Some(i) => normalise(record.get(*i).and_then(empty_to_null), opt),
                None => None,
            })
            .collect();
        store.add(row);
    }
    Ok(store)
}

/// The native engine is always available; it has no optional dependency.
pub fn available() -> bool {
    true
}
