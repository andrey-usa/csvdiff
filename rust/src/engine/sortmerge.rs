//! The out-of-core engine: sort both files by key, then walk them together.
//!
//! The other engines hold at least one whole file in memory, so the largest
//! comparison is bounded by the machine. This one is bounded by disk instead.
//! Batches of rows are sorted and spilled, the spilled runs are merged back as
//! one ordered stream, and the join is a single ordered pass down both sides at
//! once — so memory holds a batch, one row per run, and the capped report, and
//! nothing else grows with the file.
//!
//! That is the classical answer to comparing data larger than memory, and it is
//! what reconciliation systems have done since the files lived on tape. It is
//! not the fastest engine here and is not meant to be: sorting is `O(n log n)`
//! where a hash join is linear, and the spill writes the data twice more. What
//! it buys is that the answer does not depend on how much RAM you have.
//!
//! Duplicate keys are nearly free, because sorting puts the repeats next to each
//! other, and the sections come out in key order for the same reason, so nothing
//! is sorted a second time at the end.

use std::cmp::Ordering;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::columns::{compare_keys, differs, empty_to_null, normalise, resolve};
use crate::contract::{Cell, CellDiff, ColumnStat, Counts, EngineResult, Section, Val};
use crate::error::{Error, Result};
use crate::options::Options;
use crate::rowstore::Joined;
use crate::sections::assemble;

/// How much memory a batch may occupy before it is spilled.
///
/// This is a budget for the batch's real cost, not for the bytes in the file: a
/// row arrives as `String`s, each with its own allocation and header, so a
/// 180-byte CSV row occupies rather more than that once parsed. Charging only
/// the characters puts several times more in the batch than the number suggests.
const DEFAULT_BATCH_BYTES: usize = 32 << 20;

/// Per-value and per-row overhead beyond the characters themselves.
const STRING_OVERHEAD: usize = 32;
const ROW_OVERHEAD: usize = 48;

/// Overrides the batch budget so the spill and merge path can be exercised on a
/// file small enough to assert about. A comparison below the default never
/// spills at all, which would leave the interesting half of this module untested.
pub const BATCH_BYTES_ENV: &str = "CSVDIFF_SORTMERGE_BATCH_BYTES";

fn batch_bytes() -> usize {
    std::env::var(BATCH_BYTES_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_BATCH_BYTES)
}

pub fn compare(a_path: &Path, b_path: &Path, opt: &Options) -> Result<EngineResult> {
    let a_header = read_header(a_path, opt)?;
    let b_header = read_header(b_path, opt)?;
    let resolved = resolve(&a_header, &b_header, opt)?;

    let key_size = opt.key.len();
    let width = key_size + resolved.compared.len();
    let work = temp_dir()?;
    let guard = DirGuard(work.clone());

    let a_runs = stream_into(
        &work.join("a"),
        a_path,
        &a_header,
        &resolved.compared,
        opt,
        width,
    )?;
    let b_runs = stream_into(
        &work.join("b"),
        b_path,
        &b_header,
        &resolved.compared,
        opt,
        width,
    )?;
    let (a_rows, b_rows) = (a_runs.rows, b_runs.rows);

    let mut ca = a_runs.sorted()?;
    let mut cb = b_runs.sorted()?;

    let mut dup_a = Dups::new(&opt.key, opt.max_rows);
    let mut dup_b = Dups::new(&opt.key, opt.max_rows);
    let joined = merge_join(
        &mut ca,
        &mut cb,
        opt,
        &resolved.compared,
        a_rows,
        b_rows,
        &mut dup_a,
        &mut dup_b,
        opt.export_dir.is_some(),
    )?;

    let meta = resolved.meta(&opt.key, a_header.len(), b_header.len());
    let out = assemble(
        meta,
        joined,
        dup_a.section(),
        dup_b.section(),
        opt,
        &resolved.compared,
    );
    drop(guard);
    out
}

/// Removes the working directory, spilled runs and all, however the run ended.
struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> Result<PathBuf> {
    let base = std::env::temp_dir().join(format!(
        "csvdiff-sortmerge-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

fn reader(path: &Path, opt: &Options) -> Result<csv::Reader<File>> {
    use std::io::{Seek, SeekFrom};

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
            crate::columns::detect_delimiter(&line)
        }
    };
    Ok(csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
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

/// Streams a file into the sorter, projecting and normalising a row at a time.
fn stream_into(
    dir: &Path,
    path: &Path,
    header: &[String],
    compared: &[String],
    opt: &Options,
    width: usize,
) -> Result<Runs> {
    std::fs::create_dir_all(dir)?;
    let index: Vec<Option<usize>> = opt
        .key
        .iter()
        .chain(compared)
        .map(|name| header.iter().position(|c| c == name))
        .collect();

    let mut runs = Runs::new(dir.to_path_buf(), width, opt.key.len());
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
        runs.add(row)?;
    }
    Ok(runs)
}

// ---------------------------------------------------------------------------
// External sort
// ---------------------------------------------------------------------------

/// Buffers rows, spilling sorted batches to disk when the budget is reached.
///
/// Ties keep file order: batches are filled in file order and sorted stably, and
/// the merge breaks equal keys by run number, so the first row carrying a key is
/// still the first row the join sees — which is what makes first-occurrence-wins
/// mean the same thing here as in every other engine.
struct Runs {
    dir: PathBuf,
    width: usize,
    key_size: usize,
    budget: usize,
    spilled: Vec<PathBuf>,
    batch: Vec<Vec<Val>>,
    held: usize,
    rows: i64,
}

impl Runs {
    fn new(dir: PathBuf, width: usize, key_size: usize) -> Self {
        Self {
            dir,
            width,
            key_size,
            budget: batch_bytes(),
            spilled: Vec::new(),
            batch: Vec::new(),
            held: 0,
            rows: 0,
        }
    }

    fn add(&mut self, row: Vec<Val>) -> Result<()> {
        self.rows += 1;
        self.held += ROW_OVERHEAD;
        for s in row.iter().flatten() {
            self.held += STRING_OVERHEAD + s.len();
        }
        self.batch.push(row);
        if self.held >= self.budget {
            self.spill()?;
        }
        Ok(())
    }

    fn sort_batch(&mut self) {
        let key_size = self.key_size;
        self.batch.sort_by(|x, y| compare_keys(x, y, key_size));
    }

    fn spill(&mut self) -> Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        self.sort_batch();
        let path = self.dir.join(format!("run-{}.bin", self.spilled.len()));
        let mut w = BufWriter::with_capacity(1 << 16, File::create(&path)?);
        for row in &self.batch {
            write_row(&mut w, row)?;
        }
        w.flush()?;
        self.spilled.push(path);
        self.batch = Vec::new();
        self.held = 0;
        Ok(())
    }

    /// Finishes the sort and returns the rows in key order. A sort that never
    /// spilled is returned straight from memory, so the common case of a file
    /// that fits pays nothing for the machinery that handles one that does not.
    fn sorted(mut self) -> Result<Box<dyn Cursor>> {
        if self.spilled.is_empty() {
            self.sort_batch();
            return Ok(Box::new(VecCursor {
                rows: std::mem::take(&mut self.batch),
                at: 0,
            }));
        }
        self.spill()?;
        Ok(Box::new(MergeCursor::open(
            &self.spilled,
            self.width,
            self.key_size,
        )?))
    }
}

/// One ordered pass over sorted rows.
trait Cursor {
    /// The row at the cursor, or `None` once the rows are exhausted.
    fn peek(&mut self) -> Option<&[Val]>;
    /// Advances past the current row.
    fn next(&mut self) -> Result<()>;
}

struct VecCursor {
    rows: Vec<Vec<Val>>,
    at: usize,
}

impl Cursor for VecCursor {
    fn peek(&mut self) -> Option<&[Val]> {
        self.rows.get(self.at).map(|r| r.as_slice())
    }

    fn next(&mut self) -> Result<()> {
        self.at += 1;
        Ok(())
    }
}

/// A k-way merge over the spilled runs, holding one row from each.
struct MergeCursor {
    readers: Vec<BufReader<File>>,
    heads: Vec<(Vec<Val>, usize)>,
    width: usize,
    key_size: usize,
}

impl MergeCursor {
    fn open(paths: &[PathBuf], width: usize, key_size: usize) -> Result<Self> {
        let mut c = Self {
            readers: Vec::new(),
            heads: Vec::new(),
            width,
            key_size,
        };
        for p in paths {
            c.readers
                .push(BufReader::with_capacity(1 << 16, File::open(p)?));
        }
        for run in 0..c.readers.len() {
            c.pull(run)?;
        }
        Ok(c)
    }

    fn pull(&mut self, run: usize) -> Result<()> {
        if let Some(row) = read_row(&mut self.readers[run], self.width)? {
            self.heads.push((row, run));
        }
        Ok(())
    }

    /// The smallest head, breaking equal keys by run number so that file order
    /// survives the merge. A linear scan over one row per run beats a heap at
    /// the handful of runs a real file produces, and keeps the tie-break plain.
    fn best(&self) -> Option<usize> {
        let mut best: Option<usize> = None;
        for i in 0..self.heads.len() {
            best = match best {
                None => Some(i),
                Some(b) => {
                    let d = compare_keys(&self.heads[i].0, &self.heads[b].0, self.key_size);
                    if d == Ordering::Less
                        || (d == Ordering::Equal && self.heads[i].1 < self.heads[b].1)
                    {
                        Some(i)
                    } else {
                        Some(b)
                    }
                }
            };
        }
        best
    }
}

impl Cursor for MergeCursor {
    fn peek(&mut self) -> Option<&[Val]> {
        let best = self.best()?;
        self.heads.swap(0, best);
        Some(self.heads[0].0.as_slice())
    }

    fn next(&mut self) -> Result<()> {
        if self.heads.is_empty() {
            return Ok(());
        }
        let (_, run) = self.heads.remove(0);
        self.pull(run)
    }
}

// ---------------------------------------------------------------------------
// The spill format: a length-prefixed field per column, varint lengths, where
// zero is an absent field and a real length is stored one higher.
// ---------------------------------------------------------------------------

fn write_row<W: Write>(w: &mut W, row: &[Val]) -> Result<()> {
    for v in row {
        match v {
            None => write_varint(w, 0)?,
            Some(s) => {
                write_varint(w, s.len() as u64 + 1)?;
                w.write_all(s.as_bytes())?;
            }
        }
    }
    Ok(())
}

fn read_row<R: Read>(r: &mut R, width: usize) -> Result<Option<Vec<Val>>> {
    let mut row = Vec::with_capacity(width);
    for i in 0..width {
        match read_varint(r)? {
            None if i == 0 => return Ok(None),
            None => return Err(Error::new("spilled run ended mid-row".to_string())),
            Some(0) => row.push(None),
            Some(n) => {
                let mut buf = vec![0u8; (n - 1) as usize];
                r.read_exact(&mut buf)
                    .map_err(|_| Error::new("spilled run ended mid-row".to_string()))?;
                row.push(Some(String::from_utf8(buf).map_err(|e| {
                    Error::new(format!("spilled run holds invalid UTF-8: {e}"))
                })?));
            }
        }
    }
    Ok(Some(row))
}

fn write_varint<W: Write>(w: &mut W, value: u64) -> Result<()> {
    let mut v = value;
    while v >= 0x80 {
        w.write_all(&[(v as u8) | 0x80])?;
        v >>= 7;
    }
    w.write_all(&[v as u8])?;
    Ok(())
}

fn read_varint<R: Read>(r: &mut R) -> Result<Option<u64>> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        let mut byte = [0u8; 1];
        match r.read(&mut byte)? {
            0 if shift == 0 => return Ok(None),
            0 => return Err(Error::new("spilled run ended mid-row".to_string())),
            _ => {}
        }
        result |= ((byte[0] & 0x7F) as u64) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(Some(result));
        }
        shift += 7;
        if shift > 63 {
            return Err(Error::new(
                "corrupt spilled run: length is not a varint".to_string(),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// The join
// ---------------------------------------------------------------------------

/// Walks two sorted cursors and builds the finished join.
///
/// Every other engine indexes a whole file so it can ask "is this key on the
/// other side?". This one never asks. Both sides arrive in key order, so the
/// smaller key can only be missing from the other file, equal keys are a match,
/// and the answer falls out of walking the two cursors forward.
#[allow(clippy::too_many_arguments)]
fn merge_join(
    a: &mut Box<dyn Cursor>,
    b: &mut Box<dyn Cursor>,
    opt: &Options,
    compared: &[String],
    a_rows: i64,
    b_rows: i64,
    dup_a: &mut Dups,
    dup_b: &mut Dups,
    exporting: bool,
) -> Result<Joined> {
    let key_size = opt.key.len();
    let nc = compared.len();

    let mut changed_per = vec![0i64; nc];
    let mut blanked_per = vec![0i64; nc];
    let mut filled_per = vec![0i64; nc];

    let mut changed = Capped::new(opt.max_rows, exporting);
    let mut added = Capped::new(opt.max_rows, exporting);
    let mut removed = Capped::new(opt.max_rows, exporting);
    let mut changed_a: Vec<Vec<Val>> = Vec::new();
    let mut changed_b: Vec<Vec<Val>> = Vec::new();

    let (mut matched, mut a_keys, mut b_keys) = (0i64, 0i64, 0i64);
    let mut ar = a.peek().map(<[Val]>::to_vec);
    let mut br = b.peek().map(<[Val]>::to_vec);

    loop {
        let order = match (&ar, &br) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(x), Some(y)) => compare_keys(x, y, key_size),
        };

        match order {
            Ordering::Less => {
                let row = ar.take().expect("a row is present on the less branch");
                a_keys += 1;
                removed.push(to_cells(&row));
                ar = skip_key(a, &row, key_size, dup_a)?;
            }
            Ordering::Greater => {
                let row = br.take().expect("a row is present on the greater branch");
                b_keys += 1;
                added.push(to_cells(&row));
                br = skip_key(b, &row, key_size, dup_b)?;
            }
            Ordering::Equal => {
                let arow = ar.take().expect("a row is present on the equal branch");
                let brow = br.take().expect("b row is present on the equal branch");
                a_keys += 1;
                b_keys += 1;
                matched += 1;

                let mut cells: Vec<CellDiff> = Vec::new();
                for i in 0..nc {
                    let (x, y) = (&arow[key_size + i], &brow[key_size + i]);
                    if differs(x, y, opt) {
                        cells.push(CellDiff {
                            column: i,
                            a: x.clone(),
                            b: y.clone(),
                        });
                        changed_per[i] += 1;
                        if y.is_none() {
                            blanked_per[i] += 1;
                        }
                        if x.is_none() {
                            filled_per[i] += 1;
                        }
                    }
                }
                if !cells.is_empty() {
                    let mut row: Vec<Cell> = arow[..key_size]
                        .iter()
                        .map(|v| Cell::Value(v.clone()))
                        .collect();
                    row.push(Cell::Diffs(cells));
                    changed.push(row);
                    if exporting || changed_a.len() <= opt.max_rows {
                        changed_a.push(arow.clone());
                        changed_b.push(brow.clone());
                    }
                }
                ar = skip_key(a, &arow, key_size, dup_a)?;
                br = skip_key(b, &brow, key_size, dup_b)?;
            }
        }
    }

    let columns: Vec<ColumnStat> = (0..nc)
        .map(|i| ColumnStat {
            name: compared[i].clone(),
            changed: changed_per[i],
            blanked: blanked_per[i],
            filled: filled_per[i],
        })
        .collect();

    let counts = Counts {
        a_rows,
        b_rows,
        a_keys,
        b_keys,
        matched,
        unchanged: matched - changed.total,
        changed: changed.total,
        added: added.total,
        removed: removed.total,
        a_dup_keys: dup_a.keys,
        a_dup_rows: dup_a.rows,
        b_dup_keys: dup_b.keys,
        b_dup_rows: dup_b.rows,
    };

    Ok(Joined {
        counts,
        columns,
        changed: changed.held,
        added: added.held,
        removed: removed.held,
        changed_a,
        changed_b,
    })
}

/// Advances past every row sharing the current key, counting the repeats as
/// duplicates. The first row of a run is the one the join used, which is
/// first-occurrence-wins: the sort is stable and the merge breaks ties by run.
fn skip_key(
    c: &mut Box<dyn Cursor>,
    current: &[Val],
    key_size: usize,
    dups: &mut Dups,
) -> Result<Option<Vec<Val>>> {
    c.next()?;
    let mut repeats = 0i64;
    loop {
        let same = match c.peek() {
            Some(next) => compare_keys(current, next, key_size) == Ordering::Equal,
            None => false,
        };
        if !same {
            break;
        }
        repeats += 1;
        c.next()?;
    }
    if repeats > 0 {
        dups.record(current, repeats + 1);
    }
    Ok(c.peek().map(<[Val]>::to_vec))
}

fn to_cells(row: &[Val]) -> Vec<Cell> {
    row.iter().map(|v| Cell::Value(v.clone())).collect()
}

/// A row list that stops growing at the report cap but keeps counting.
///
/// The counts in the contract are always exact and the embedded rows are always
/// capped, so holding more than the cap only ever serves `--export-dir`. Not
/// holding them is what lets this engine compare a file far larger than memory.
struct Capped {
    held: Vec<Vec<Cell>>,
    cap: usize,
    unbounded: bool,
    total: i64,
}

impl Capped {
    fn new(cap: usize, unbounded: bool) -> Self {
        Self {
            held: Vec::new(),
            cap,
            unbounded,
            total: 0,
        }
    }

    fn push(&mut self, row: Vec<Cell>) {
        self.total += 1;
        // One past the cap, so a section can still report that it was truncated.
        if self.unbounded || self.held.len() <= self.cap {
            self.held.push(row);
        }
    }
}

/// The duplicate-key report, kept to the top `--max-rows` by count.
///
/// A bounded list of the worst offenders, rather than every duplicate, so a
/// pathological file where every key repeats does not undo the memory bound.
/// Ordering matches every other engine: most duplicated first, then by key.
struct Dups {
    key_cols: Vec<String>,
    key_size: usize,
    cap: usize,
    best: Vec<(Vec<Val>, i64)>,
    keys: i64,
    rows: i64,
}

impl Dups {
    fn new(key_cols: &[String], cap: usize) -> Self {
        Self {
            key_cols: key_cols.to_vec(),
            key_size: key_cols.len(),
            cap,
            best: Vec::new(),
            keys: 0,
            rows: 0,
        }
    }

    fn record(&mut self, row: &[Val], count: i64) {
        self.keys += 1;
        self.rows += count;
        self.best.push((row[..self.key_size].to_vec(), count));
        if self.best.len() > self.cap + 1 {
            self.sort_best();
            self.best.truncate(self.cap + 1);
        }
    }

    fn sort_best(&mut self) {
        let key_size = self.key_size;
        self.best.sort_by(|x, y| {
            y.1.cmp(&x.1)
                .then_with(|| compare_keys(&x.0, &y.0, key_size))
        });
    }

    fn section(&mut self) -> Section {
        self.sort_best();
        let rows: Vec<Vec<Cell>> = self
            .best
            .iter()
            .take(self.cap)
            .map(|(key, count)| {
                let mut row: Vec<Cell> = key.iter().map(|v| Cell::Value(v.clone())).collect();
                row.push(Cell::Count(*count));
                row
            })
            .collect();
        let mut cols = self.key_cols.clone();
        cols.push("count".to_string());
        Section {
            cols,
            rows,
            truncated: self.keys > self.cap as i64,
        }
    }
}

/// The sort-merge engine has no optional dependency.
pub fn available() -> bool {
    true
}
