//! Comparing two Parquet files without turning them back into rows.
//!
//! The Rust twin of `cpp/src/pqdiff.cpp`, and the same design. The `turbo`
//! engine beside this one has one shape: map the file, find every row, reduce
//! each row to a handful of (offset, length) fields. That shape is right for a
//! text format, where a value's boundaries are only known by scanning for them.
//!
//! Parquet is not that. A value's boundaries are written down, values of one
//! column are contiguous, and -- this is the part worth exploiting -- a
//! low-cardinality column is stored as small integers indexing a dictionary of
//! its distinct values. Reading such a file back into rows in order to compare
//! them row by row throws away the one thing the format gives you.
//!
//! So this path is columnar end to end. It reads the key columns, joins on them
//! once to produce a list of matched `(a_row, b_row)` pairs, and then walks the
//! compared columns one at a time, releasing each before reading the next. Where
//! both sides of a column are dictionary encoded, the two dictionaries are
//! mapped onto one shared id space once -- a few thousand string comparisons --
//! after which "did this cell change" is `i32 != i32`, which the compiler
//! vectorises, and the resulting mismatch mask is scanned eight bytes at a time
//! with the same SWAR trick the CSV scanner uses to find a delimiter.
//!
//! The result is the same `EngineResult`, and therefore the same JSON, as
//! comparing the same data as CSV.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use memmap2::Mmap;

use crate::columns::{compare_keys, resolve};
use crate::contract::{Cell, CellDiff, ColumnStat, Counts, EngineResult, Section, Val};
use crate::error::{Error, Result};
use crate::options::Options;
use crate::rowstore::Joined;
use crate::sections::assemble;
use crate::parquet;

/// True when both paths are Parquet, so the caller knows to come here.
pub fn is_parquet(path: &Path) -> bool {
    parquet::is_parquet(path)
}

// ---------------------------------------------------------------------------
// Values: the same rules as the CSV path, on a plain byte span
// ---------------------------------------------------------------------------

/// One cell. Parquet values carry no escaping -- a byte array is its own bytes
/// -- so where the CSV path has to read a field through its escape decoder,
/// here a cell is already a span.
#[derive(Clone, Copy)]
struct Look<'a> {
    bytes: &'a [u8],
    null: bool,
}

impl<'a> Look<'a> {
    const NONE: Look<'static> = Look {
        bytes: &[],
        null: true,
    };
}

fn needs_normalising(opt: &Options) -> bool {
    opt.trim || opt.ignore_case || opt.empty_is_null || opt.tolerance > 0.0
}

fn value_of(c: Look<'_>, opt: &Options) -> Val {
    if c.null || c.bytes.is_empty() {
        return None;
    }
    let mut text = String::from_utf8_lossy(c.bytes).into_owned();
    if opt.trim {
        text = text.trim().to_string();
    }
    if opt.ignore_case {
        text = text.to_lowercase();
    }
    if text.is_empty() && opt.empty_is_null {
        return None;
    }
    if text.is_empty() { None } else { Some(text) }
}

fn absent(c: Look<'_>, opt: &Options) -> bool {
    if c.null || c.bytes.is_empty() {
        return true;
    }
    if !needs_normalising(opt) {
        return false;
    }
    value_of(c, opt).is_none()
}

fn same(x: Look<'_>, y: Look<'_>, opt: &Options) -> bool {
    let (xa, ya) = (absent(x, opt), absent(y, opt));
    if xa || ya {
        return xa && ya;
    }
    if !needs_normalising(opt) {
        return x.bytes == y.bytes;
    }
    value_of(x, opt) == value_of(y, opt)
}

/// Deliberately stricter than a bare parse: "inf" and "nan" are ordinary text in
/// a table, and treating them as numbers would make two unequal strings compare
/// equal.
fn as_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let first = body.as_bytes().first()?;
    if !(first.is_ascii_digit() || *first == b'.') {
        return None;
    }
    if !body
        .bytes()
        .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// SQL's IS DISTINCT FROM, with the tolerance applied where both sides parse.
fn cell_differs(x: Look<'_>, y: Look<'_>, opt: &Options) -> bool {
    let (xa, ya) = (absent(x, opt), absent(y, opt));
    if xa && ya {
        return false;
    }
    if opt.tolerance > 0.0 && !xa && !ya {
        let nx = as_number(&String::from_utf8_lossy(x.bytes));
        let ny = as_number(&String::from_utf8_lossy(y.bytes));
        if let (Some(a), Some(b)) = (nx, ny) {
            return (a - b).abs() > opt.tolerance;
        }
    }
    !same(x, y, opt)
}

const PRIME: u64 = 0x100000001b3;
const SEED: u64 = 0xcbf29ce484222325;

fn fold_bytes(mut h: u64, v: &[u8]) -> u64 {
    for &b in v {
        h = (h ^ b as u64).wrapping_mul(PRIME);
    }
    (h ^ v.len() as u64).wrapping_mul(PRIME)
}
fn fold_absent(h: u64) -> u64 {
    (h ^ 0x9e3779b97f4a7c15).wrapping_mul(PRIME)
}
fn fold_id(h: u64, id: i32) -> u64 {
    (h ^ (id as u32) as u64).wrapping_mul(PRIME)
}

// ---------------------------------------------------------------------------
// A column, as the comparison sees it
// ---------------------------------------------------------------------------

/// `parquet::Column` hands back offsets; this resolves them against whichever
/// buffer they belong to and answers "what is in row r".
struct Col<'m> {
    c: parquet::Column,
    map: &'m [u8],
}

impl<'m> Col<'m> {
    fn new(c: parquet::Column, map: &'m [u8]) -> Self {
        Col { c, map }
    }
    fn base(&self) -> &[u8] {
        if self.c.owned.is_empty() {
            self.map
        } else {
            &self.c.owned
        }
    }
    fn at(&self, row: usize) -> Look<'_> {
        look(self.base(), self.slice_at(row))
    }
    fn dict_at(&self, k: usize) -> Look<'_> {
        look(self.base(), self.c.dict[k])
    }
    /// The row's slice, without resolving it. Hot loops take this and the base
    /// separately, so neither the "which buffer" branch nor its bounds check
    /// is paid per cell.
    #[inline]
    fn slice_at(&self, row: usize) -> parquet::Slice {
        if self.c.dictionary {
            let k = self.c.index[row];
            if k < 0 {
                return parquet::Slice::null();
            }
            return self.c.dict[k as usize];
        }
        self.c.values[row]
    }
    fn rows(&self) -> usize {
        self.c.rows()
    }
}

/// Resolves a slice against the buffer it belongs to.
#[inline]
fn look(base: &[u8], s: parquet::Slice) -> Look<'_> {
    if s.is_null() {
        return Look::NONE;
    }
    let at = s.offset();
    Look {
        bytes: &base[at..at + s.length()],
        null: false,
    }
}

// ---------------------------------------------------------------------------
// One id space for two dictionaries
// ---------------------------------------------------------------------------

/// The trick the whole columnar path turns on. Two files' dictionaries are
/// interned into one dense id space, which costs one hash per *distinct* value
/// rather than one per row; after that, two cells are equal exactly when their
/// ids are, and a column diff is a comparison of two `i32` arrays.
#[derive(Default)]
struct Ids {
    table: HashMap<Vec<u8>, i32>,
}

impl Ids {
    fn of(&mut self, v: &[u8]) -> i32 {
        let next = self.table.len() as i32;
        *self.table.entry(v.to_vec()).or_insert(next)
    }
}

/// Codes one column's dictionary into `ids`, with -1 for an absent value.
fn code(col: &Col<'_>, ids: &mut Ids, opt: &Options) -> Vec<i32> {
    let mut out = Vec::with_capacity(col.c.dict.len());
    for k in 0..col.c.dict.len() {
        let cell = col.dict_at(k);
        if absent(cell, opt) {
            out.push(-1);
            continue;
        }
        if needs_normalising(opt) {
            let v = value_of(cell, opt).unwrap_or_default();
            out.push(ids.of(v.as_bytes()));
        } else {
            out.push(ids.of(cell.bytes));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// One file's key columns. A key column both sides store as a dictionary is
/// reduced to a shared id per row, and from there hashing and equality are
/// integer work; anything else stays bytes and is compared as bytes.
struct KeySide<'m> {
    col: Vec<Col<'m>>,
    /// Filled where the column is id-coded on both sides.
    id: Vec<Vec<i32>>,
    rows: usize,
}

fn row_hash(as_id: &[bool], s: &KeySide<'_>, row: usize, opt: &Options) -> u64 {
    let mut h = SEED;
    for (j, &coded) in as_id.iter().enumerate() {
        if coded {
            h = fold_id(h, s.id[j][row]);
            continue;
        }
        let c = s.col[j].at(row);
        if absent(c, opt) {
            h = fold_absent(h);
        } else if !needs_normalising(opt) {
            h = fold_bytes(h, c.bytes);
        } else {
            h = fold_bytes(h, value_of(c, opt).unwrap_or_default().as_bytes());
        }
    }
    h
}

fn row_eq(
    as_id: &[bool],
    x: &KeySide<'_>,
    rx: usize,
    y: &KeySide<'_>,
    ry: usize,
    opt: &Options,
) -> bool {
    for (j, &coded) in as_id.iter().enumerate() {
        if coded {
            if x.id[j][rx] != y.id[j][ry] {
                return false;
            }
        } else if !same(x.col[j].at(rx), y.col[j].at(ry), opt) {
            return false;
        }
    }
    true
}

/// An open-addressed table over one file's distinct keys, first occurrence wins.
///
/// A slot is one word: the top twenty-four bits of the key's hash, and the
/// position in `firsts` plus one, with zero meaning empty. Carrying the hash
/// *inside* the slot is the point -- a probe that misses is settled by the word
/// it already loaded, where a table of bare positions would have to follow each
/// one into a separate array of hashes and take a second cache miss to reject
/// it. At ten million keys those second misses were the join.
#[derive(Default)]
struct Index {
    slots: Vec<u64>,
    mask: u64,
    firsts: Vec<i32>,
    counts: Vec<u32>,
    /// Per distinct key, for probing the other side.
    hashes: Vec<u64>,
    rows: i64,
    dup_keys: i64,
    dup_rows: i64,
}

impl Index {
    const POS_MASK: u64 = (1 << 40) - 1;
    fn slot_for(h: u64, pos: usize) -> u64 {
        (h & !Self::POS_MASK) | (pos as u64 + 1)
    }
    fn tag_is(slot: u64, h: u64) -> bool {
        (slot ^ h) & !Self::POS_MASK == 0
    }
    fn pos_of(slot: u64) -> usize {
        (slot & Self::POS_MASK) as usize - 1
    }
    fn unique(&self) -> i64 {
        self.firsts.len() as i64
    }
}

/// Hashes are computed in parallel because a row's hash depends on nothing but
/// that row; insertion is serial because first-occurrence-wins depends on the
/// order rows arrive, and threading it would make the answer depend on the
/// scheduler. Same split as the CSV path.
fn build_index(as_id: &[bool], s: &KeySide<'_>, opt: &Options, threads: usize) -> Index {
    let n = s.rows;
    let mut hs = vec![0u64; n];
    let ways = if n < (1 << 15) { 1 } else { threads.max(1) };
    if ways == 1 {
        for (r, h) in hs.iter_mut().enumerate() {
            *h = row_hash(as_id, s, r, opt);
        }
    } else {
        let per = n.div_ceil(ways);
        std::thread::scope(|scope| {
            for (p, chunk) in hs.chunks_mut(per).enumerate() {
                scope.spawn(move || {
                    let from = p * per;
                    for (i, h) in chunk.iter_mut().enumerate() {
                        *h = row_hash(as_id, s, from + i, opt);
                    }
                });
            }
        });
    }

    let mut ix = Index {
        rows: n as i64,
        ..Index::default()
    };
    // Sized to about a two-thirds load: linear probing is still short there,
    // and a smaller table is a smaller working set, which is what this phase is
    // actually limited by.
    let mut cap = 1usize << 12;
    while cap * 2 < n * 3 + 16 {
        cap <<= 1;
    }
    ix.slots = vec![0u64; cap];
    ix.mask = cap as u64 - 1;
    ix.firsts.reserve(n);
    ix.counts.reserve(n);
    ix.hashes.reserve(n);

    for (r, &h) in hs.iter().enumerate() {
        let mut at = (h & ix.mask) as usize;
        loop {
            let slot = ix.slots[at];
            if slot == 0 {
                ix.slots[at] = Index::slot_for(h, ix.firsts.len());
                ix.firsts.push(r as i32);
                ix.counts.push(1);
                ix.hashes.push(h);
                break;
            }
            if Index::tag_is(slot, h) {
                let pos = Index::pos_of(slot);
                if row_eq(as_id, s, ix.firsts[pos] as usize, s, r, opt) {
                    ix.counts[pos] += 1;
                    if ix.counts[pos] == 2 {
                        ix.dup_keys += 1;
                        ix.dup_rows += 1; // the first occurrence counts once the key repeats
                    }
                    ix.dup_rows += 1;
                    break;
                }
            }
            at = (at + 1) & ix.mask as usize;
        }
    }
    ix
}

/// Looks one side's row up in the other side's table.
fn lookup(
    as_id: &[bool],
    into: &Index,
    there: &KeySide<'_>,
    here: &KeySide<'_>,
    row: usize,
    h: u64,
    opt: &Options,
) -> i32 {
    let mut at = (h & into.mask) as usize;
    loop {
        let slot = into.slots[at];
        if slot == 0 {
            return -1;
        }
        if Index::tag_is(slot, h) {
            let first = into.firsts[Index::pos_of(slot)];
            if row_eq(as_id, there, first as usize, here, row, opt) {
                return first;
            }
        }
        at = (at + 1) & into.mask as usize;
    }
}

// ---------------------------------------------------------------------------
// Row lists, capped but exactly counted
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Capped {
    held: Vec<i32>,
    total: i64,
}

impl Capped {
    fn push(&mut self, row: i32, cap: usize) {
        self.total += 1;
        // One past the cap, so a section can still report that it was truncated.
        if self.held.len() <= cap {
            self.held.push(row);
        }
    }
}

// ---------------------------------------------------------------------------
// What one compared column produced
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ColOut {
    changed: i64,
    blanked: i64,
    filled: i64,
    /// One bit per matched pair.
    bits: Vec<u64>,
    /// The first cap+1 pairs where this column differs, with their two values.
    /// Every pair that reaches the report is among these -- a row only reaches
    /// the report by being one of the first cap+1 *changed* pairs, and the
    /// pairs where this column differs are a subset of those, in the same order.
    held: Vec<(usize, Val, Val)>,
    added_vals: Vec<Val>,
    removed_vals: Vec<Val>,
}

const BLOCK: usize = 4096;

/// The mismatch mask, read eight bytes at a time -- the same SWAR idiom the CSV
/// scanner uses to find a delimiter, applied to finding a changed cell. About
/// 0.7% of cells differ per column, so seven bytes in eight are zero and
/// skipping them wholesale is most of the loop.
fn scan_mask(neq: &[u8], base: usize, mut hit: impl FnMut(usize)) {
    let m = neq.len();
    let mut i = 0;
    while i + 8 <= m {
        let mut w = u64::from_le_bytes(neq[i..i + 8].try_into().unwrap());
        while w != 0 {
            let byte = (w.trailing_zeros() >> 3) as usize;
            hit(base + i + byte);
            w &= !(0xFFu64 << (byte * 8));
        }
        i += 8;
    }
    while i < m {
        if neq[i] != 0 {
            hit(base + i);
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn map(path: &Path) -> Result<Mmap> {
    let file = File::open(path).map_err(|e| Error::new(format!("cannot read {path:?}: {e}")))?;
    // SAFETY: the file is opened read-only and not modified while mapped, which
    // is the same contract the `turbo` engine's Slab makes.
    unsafe { Mmap::map(&file) }.map_err(|e| Error::new(format!("cannot map {path:?}: {e}")))
}

pub fn compare(a_path: &Path, b_path: &Path, opt: &Options) -> Result<EngineResult> {
    if opt.export_dir.is_some() {
        return Err(Error::new(
            "--export-dir is not supported on the parquet path: it needs every compared value of \
             every changed row, and this path holds one column at a time on purpose",
        ));
    }

    let a_map = map(a_path)?;
    let b_map = map(b_path)?;
    let a_name = a_path.display().to_string();
    let b_name = b_path.display().to_string();
    let a_meta = parquet::read_meta(&a_map, &a_name)?;
    let b_meta = parquet::read_meta(&b_map, &b_name)?;

    let resolved = resolve(&a_meta.names, &b_meta.names, opt)?;
    let key_size = opt.key.len();
    let nc = resolved.compared.len();
    let cap = opt.max_rows;

    let slot_of = |names: &[String], n: &str| names.iter().position(|c| c == n).unwrap();

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    // --- keys -------------------------------------------------------------
    //
    // Both files' key columns are read at once, then any column both sides
    // store as a dictionary is reduced to one shared id per row.
    let (a_key_cols, b_key_cols) = std::thread::scope(|scope| {
        let bh = scope.spawn(|| -> Result<Vec<parquet::Column>> {
            opt.key
                .iter()
                .map(|k| parquet::read_column(&b_map, slot_of(&b_meta.names, k), &b_name))
                .collect()
        });
        let a: Result<Vec<parquet::Column>> = opt
            .key
            .iter()
            .map(|k| parquet::read_column(&a_map, slot_of(&a_meta.names, k), &a_name))
            .collect();
        (a, bh.join().unwrap())
    });

    let mut a_keys = KeySide {
        col: a_key_cols?.into_iter().map(|c| Col::new(c, &a_map)).collect(),
        id: vec![Vec::new(); key_size],
        rows: 0,
    };
    let mut b_keys = KeySide {
        col: b_key_cols?.into_iter().map(|c| Col::new(c, &b_map)).collect(),
        id: vec![Vec::new(); key_size],
        rows: 0,
    };
    a_keys.rows = a_keys.col.first().map(|c| c.rows()).unwrap_or(0);
    b_keys.rows = b_keys.col.first().map(|c| c.rows()).unwrap_or(0);
    for j in 0..key_size {
        if a_keys.col[j].rows() != a_keys.rows || b_keys.col[j].rows() != b_keys.rows {
            return Err(Error::new(
                "parquet columns disagree about how many rows the file has",
            ));
        }
    }

    let mut as_id = vec![false; key_size];
    for j in 0..key_size {
        if !a_keys.col[j].c.dictionary || !b_keys.col[j].c.dictionary {
            continue;
        }
        as_id[j] = true;
        let mut ids = Ids::default();
        let a_map_j = code(&a_keys.col[j], &mut ids, opt);
        let b_map_j = code(&b_keys.col[j], &mut ids, opt);
        let apply = |col: &Col<'_>, m: &[i32]| -> Vec<i32> {
            col.c
                .index
                .iter()
                .map(|&k| if k < 0 { -1 } else { m[k as usize] })
                .collect()
        };
        a_keys.id[j] = apply(&a_keys.col[j], &a_map_j);
        b_keys.id[j] = apply(&b_keys.col[j], &b_map_j);
    }

    // --- the join ---------------------------------------------------------
    let per_side = (threads / 2).max(1);
    let (ai, bi) = std::thread::scope(|scope| {
        let bh = scope.spawn(|| build_index(&as_id, &b_keys, opt, per_side));
        let a = build_index(&as_id, &a_keys, opt, per_side);
        (a, bh.join().unwrap())
    });

    // Both directions split over contiguous ranges of one side's distinct keys.
    // Each range accumulates into its own lists, and the ranges merge in order,
    // so which rows survive the report cap is the same as one thread would have
    // kept.
    #[derive(Default)]
    struct Part {
        pa: Vec<i32>,
        pb: Vec<i32>,
        held: Vec<i32>,
        total: i64,
    }
    let split = |keys_here: usize| if keys_here < (1 << 14) { 1 } else { threads.max(1) };
    let a_ways = split(ai.firsts.len());
    let b_ways = split(bi.firsts.len());

    let a_range = |p: usize| -> Part {
        let mut out = Part::default();
        let lo = ai.firsts.len() * p / a_ways;
        let hi = ai.firsts.len() * (p + 1) / a_ways;
        out.pa.reserve(hi - lo);
        out.pb.reserve(hi - lo);
        for at in lo..hi {
            let row = ai.firsts[at];
            let mate = lookup(&as_id, &bi, &b_keys, &a_keys, row as usize, ai.hashes[at], opt);
            if mate < 0 {
                out.total += 1;
                if out.held.len() <= cap {
                    out.held.push(row);
                }
                continue;
            }
            out.pa.push(row);
            out.pb.push(mate);
        }
        out
    };
    let b_range = |p: usize| -> Part {
        let mut out = Part::default();
        let lo = bi.firsts.len() * p / b_ways;
        let hi = bi.firsts.len() * (p + 1) / b_ways;
        for at in lo..hi {
            let row = bi.firsts[at];
            if lookup(&as_id, &ai, &a_keys, &b_keys, row as usize, bi.hashes[at], opt) >= 0 {
                continue;
            }
            out.total += 1;
            if out.held.len() <= cap {
                out.held.push(row);
            }
        }
        out
    };

    let (a_parts, b_parts) = std::thread::scope(|scope| {
        let ah: Vec<_> = (0..a_ways).map(|p| scope.spawn(move || a_range(p))).collect();
        let bh: Vec<_> = (0..b_ways).map(|p| scope.spawn(move || b_range(p))).collect();
        (
            ah.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>(),
            bh.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>(),
        )
    });

    let mut added = Capped::default();
    for p in &b_parts {
        for &row in &p.held {
            added.push(row, cap);
        }
        added.total += p.total - p.held.len() as i64;
    }

    let mut pair_a: Vec<i32> = Vec::new();
    let mut pair_b: Vec<i32> = Vec::new();
    let mut removed = Capped::default();
    {
        let total: usize = a_parts.iter().map(|p| p.pa.len()).sum();
        pair_a.reserve(total);
        pair_b.reserve(total);
        for p in &a_parts {
            pair_a.extend_from_slice(&p.pa);
            pair_b.extend_from_slice(&p.pb);
            for &row in &p.held {
                removed.push(row, cap);
            }
            removed.total += p.total - p.held.len() as i64;
        }
    }
    drop(a_parts);
    drop(b_parts);

    let npairs = pair_a.len();
    let words = npairs.div_ceil(64);

    // --- the compared columns, one at a time ------------------------------
    //
    // Each worker owns whole columns, so nothing is shared but the pair arrays
    // and the two mappings, which are read-only from here on. How many are in
    // flight is a memory choice, not a parallelism one: a column of ten million
    // values costs a couple of hundred megabytes on each side while it is being
    // read, and it is released before the next is asked for.
    let lanes = threads.clamp(1, nc.max(1));
    let next = AtomicUsize::new(0);
    let compared = &resolved.compared;
    let (pa, pb) = (&pair_a, &pair_b);
    let (added_held, removed_held) = (&added.held, &removed.held);

    let work = || -> Result<Vec<(usize, ColOut)>> {
        let mut mine = Vec::new();
        loop {
            let c = next.fetch_add(1, AtomicOrdering::Relaxed);
            if c >= nc {
                return Ok(mine);
            }
            let mut out = ColOut {
                bits: vec![0u64; words],
                ..ColOut::default()
            };
            let a_col = Col::new(
                parquet::read_column(&a_map, slot_of(&a_meta.names, &compared[c]), &a_name)?,
                &a_map,
            );
            let b_col = Col::new(
                parquet::read_column(&b_map, slot_of(&b_meta.names, &compared[c]), &b_name)?,
                &b_map,
            );
            if a_col.rows() != a_keys.rows || b_col.rows() != b_keys.rows {
                return Err(Error::new(
                    "parquet columns disagree about how many rows the file has",
                ));
            }

            let mut hits: Vec<usize> = Vec::new();
            // The fast path: both sides dictionary encoded, so the two
            // dictionaries go into one id space and the per-row work is two
            // gathers and an integer compare.
            let coded = a_col.c.dictionary && b_col.c.dictionary && opt.tolerance == 0.0;
            if coded {
                let mut ids = Ids::default();
                let amap = code(&a_col, &mut ids, opt);
                let bmap = code(&b_col, &mut ids, opt);
                let (aix, bix) = (&a_col.c.index, &b_col.c.index);
                let mut xa = vec![0i32; BLOCK];
                let mut xb = vec![0i32; BLOCK];
                let mut neq = vec![0u8; BLOCK];
                let mut base = 0usize;
                while base < npairs {
                    let m = BLOCK.min(npairs - base);
                    for i in 0..m {
                        let k = aix[pa[base + i] as usize];
                        xa[i] = if k < 0 { -1 } else { amap[k as usize] };
                    }
                    for i in 0..m {
                        let k = bix[pb[base + i] as usize];
                        xb[i] = if k < 0 { -1 } else { bmap[k as usize] };
                    }
                    for i in 0..m {
                        neq[i] = (xa[i] != xb[i]) as u8;
                    }
                    scan_mask(&neq[..m], base, |p| hits.push(p));
                    base += BLOCK;
                }
            } else {
                // The two buffers are found once rather than per cell: which
                // one a slice belongs to is a property of the column, and
                // asking per access costs a branch and a bounds check on every
                // one of ten million rows.
                let (ab, bb) = (a_col.base(), b_col.base());
                let mut neq = vec![0u8; BLOCK];
                let mut base = 0usize;
                while base < npairs {
                    let m = BLOCK.min(npairs - base);
                    for i in 0..m {
                        neq[i] = cell_differs(
                            look(ab, a_col.slice_at(pa[base + i] as usize)),
                            look(bb, b_col.slice_at(pb[base + i] as usize)),
                            opt,
                        ) as u8;
                    }
                    scan_mask(&neq[..m], base, |p| hits.push(p));
                    base += BLOCK;
                }
            }

            for p in hits {
                let x = a_col.at(pa[p] as usize);
                let y = b_col.at(pb[p] as usize);
                out.changed += 1;
                if absent(y, opt) {
                    out.blanked += 1;
                }
                if absent(x, opt) {
                    out.filled += 1;
                }
                out.bits[p >> 6] |= 1u64 << (p & 63);
                if out.held.len() <= cap {
                    out.held.push((p, value_of(x, opt), value_of(y, opt)));
                }
            }

            // The rows that reach the report as added or removed need every
            // column's value, and this is the only time this column is in
            // memory.
            out.added_vals = added_held
                .iter()
                .map(|&r| value_of(b_col.at(r as usize), opt))
                .collect();
            out.removed_vals = removed_held
                .iter()
                .map(|&r| value_of(a_col.at(r as usize), opt))
                .collect();
            mine.push((c, out));
        }
    };

    let mut cols: Vec<ColOut> = (0..nc).map(|_| ColOut::default()).collect();
    {
        let gathered: Vec<Result<Vec<(usize, ColOut)>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..lanes).map(|_| scope.spawn(&work)).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for got in gathered {
            for (c, out) in got? {
                cols[c] = out;
            }
        }
    }

    let columns: Vec<ColumnStat> = compared
        .iter()
        .zip(&cols)
        .map(|(name, c)| ColumnStat {
            name: name.clone(),
            changed: c.changed,
            blanked: c.blanked,
            filled: c.filled,
        })
        .collect();

    // --- which pairs changed ---------------------------------------------
    let mut any = vec![0u64; words];
    for c in &cols {
        for (w, v) in any.iter_mut().zip(&c.bits) {
            *w |= *v;
        }
    }
    let mut changed_total = 0i64;
    let mut keep: Vec<usize> = Vec::new(); // the first cap+1 changed pairs, in pair order
    for (w, &word) in any.iter().enumerate() {
        changed_total += word.count_ones() as i64;
        let mut bits = word;
        while bits != 0 && keep.len() <= cap {
            let bit = bits.trailing_zeros() as usize;
            keep.push(w * 64 + bit);
            bits &= bits - 1;
        }
    }

    // --- the report -------------------------------------------------------
    let key_values = |s: &KeySide<'_>, row: i32| -> Vec<Val> {
        (0..key_size)
            .map(|j| value_of(s.col[j].at(row as usize), opt))
            .collect()
    };

    let mut rows: Vec<(Vec<Val>, Vec<CellDiff>)> = keep
        .iter()
        .map(|&p| (key_values(&a_keys, pair_a[p]), Vec::new()))
        .collect();
    for (c, out) in cols.iter_mut().enumerate() {
        for (p, x, y) in std::mem::take(&mut out.held) {
            if let Ok(i) = keep.binary_search(&p) {
                rows[i].1.push(CellDiff {
                    column: c,
                    a: x,
                    b: y,
                });
            }
        }
    }
    rows.sort_by(|x, y| compare_keys(&x.0, &y.0, key_size));
    // Not truncated here: `Section::capped` takes the cap and reads the one
    // extra row as the signal that there were more.
    let changed_rows: Vec<Vec<Cell>> = rows
        .into_iter()
        .map(|(key, diffs)| {
            let mut row: Vec<Cell> = key.into_iter().map(Cell::Value).collect();
            row.push(Cell::Diffs(diffs));
            row
        })
        .collect();

    let full_row = |s: &KeySide<'_>, row: i32, at: usize, from_b: bool| -> Vec<Val> {
        let mut out = key_values(s, row);
        out.reserve(nc);
        for c in &cols {
            out.push(if from_b {
                c.added_vals[at].clone()
            } else {
                c.removed_vals[at].clone()
            });
        }
        out
    };
    let mut added_rows: Vec<Vec<Val>> = added
        .held
        .iter()
        .enumerate()
        .map(|(i, &r)| full_row(&b_keys, r, i, true))
        .collect();
    let mut removed_rows: Vec<Vec<Val>> = removed
        .held
        .iter()
        .enumerate()
        .map(|(i, &r)| full_row(&a_keys, r, i, false))
        .collect();
    added_rows.sort_by(|x, y| compare_keys(x, y, key_size));
    removed_rows.sort_by(|x, y| compare_keys(x, y, key_size));

    let to_cells = |r: Vec<Val>| -> Vec<Cell> { r.into_iter().map(Cell::Value).collect() };

    let dup_section = |s: &KeySide<'_>, ix: &Index| -> Section {
        let mut all: Vec<(Vec<Val>, i64)> = ix
            .firsts
            .iter()
            .zip(&ix.counts)
            .filter(|(_, n)| **n >= 2)
            .map(|(&r, &n)| (key_values(s, r), n as i64))
            .collect();
        all.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| compare_keys(&x.0, &y.0, key_size)));
        let total = all.len();
        all.truncate(cap.min(total));
        let mut cols_out = opt.key.clone();
        cols_out.push("count".to_string());
        Section {
            cols: cols_out,
            rows: all
                .into_iter()
                .map(|(k, n)| {
                    let mut row: Vec<Cell> = k.into_iter().map(Cell::Value).collect();
                    row.push(Cell::Count(n));
                    row
                })
                .collect(),
            truncated: total > cap,
        }
    };
    let dup_a = dup_section(&a_keys, &ai);
    let dup_b = dup_section(&b_keys, &bi);

    let matched = npairs as i64;
    let counts = Counts {
        a_rows: ai.rows,
        b_rows: bi.rows,
        a_keys: ai.unique(),
        b_keys: bi.unique(),
        matched,
        unchanged: matched - changed_total,
        changed: changed_total,
        added: added.total,
        removed: removed.total,
        a_dup_keys: ai.dup_keys,
        a_dup_rows: ai.dup_rows,
        b_dup_keys: bi.dup_keys,
        b_dup_rows: bi.dup_rows,
    };

    let joined = Joined {
        counts,
        columns,
        changed: changed_rows,
        added: added_rows.into_iter().map(to_cells).collect(),
        removed: removed_rows.into_iter().map(to_cells).collect(),
        changed_a: Vec::new(),
        changed_b: Vec::new(),
    };
    let meta = resolved.meta(&opt.key, a_meta.names.len(), b_meta.names.len());
    assemble(meta, joined, dup_a, dup_b, opt, &resolved.compared)
}
