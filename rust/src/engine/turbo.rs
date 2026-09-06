//! The byte-level engine: map the file, index it in place, build no string for a
//! cell unless that cell reaches the report.
//!
//! Every other in-memory engine here reads a row into owned `String`s and hashes
//! one of them into a map. On twenty columns and a million rows that is twenty
//! million allocations, and all but the few thousand the report embeds are
//! thrown away. This engine keeps a field as an offset and a length into the
//! mapped bytes; hashing and comparison read those bytes in place, and only the
//! capped report sections are ever decoded.
//!
//! That is the same design as the Java `turbo` engine, and it exists here for a
//! specific reason: Java `turbo` beats Rust `native` by nearly six times on time
//! and four on memory, which reads as a language result and is not one. With
//! both designs in one language the comparison is honest.
//!
//! Normalisation is delegated rather than reimplemented. With `--trim`,
//! `--ignore-case`, `--empty-is-null` or a tolerance in play a field is decoded
//! and handed to the same [`crate::columns`] functions every other engine uses,
//! so the answer cannot drift; without them the raw bytes are compared and
//! hashed directly, which is the path the benchmarks take. Hashing and equality
//! therefore always read the same bytes by the same route — the property whose
//! absence produced two silently wrong answers in the Java port.

use std::fs::File;
use std::path::Path;

use memmap2::Mmap;

use crate::columns::{compare_keys, detect_delimiter, differs, empty_to_null, normalise, resolve};
use crate::contract::{Cell, CellDiff, ColumnStat, Counts, EngineResult, Section, Val};
use crate::error::{Error, Result};
use crate::options::Options;
use crate::rowstore::Joined;
use crate::sections::assemble;

/// A field packed into one word: offset, length, and whether it needs unescaping.
///
/// 40 bits of offset addresses a terabyte and 23 bits of length a field of eight
/// megabytes, which is more than a CSV cell has any business being.
type Field = u64;

const ABSENT: Field = u64::MAX;
/// A field too long for the packed length. Reported rather than truncated: the
/// length is masked into 23 bits, so silently packing an over-long field would
/// corrupt its value instead of failing.
const TOO_LONG: Field = u64::MAX - 1;
const OFFSET_MASK: u64 = (1 << 40) - 1;
const LENGTH_SHIFT: u32 = 40;
const LENGTH_MASK: u64 = (1 << 23) - 1;
const ESCAPED: u64 = 1 << 63;
const MAX_FIELD_LEN: u64 = LENGTH_MASK;

fn pack(offset: u64, len: u64, escaped: bool) -> Field {
    if len > MAX_FIELD_LEN {
        return TOO_LONG;
    }
    (offset & OFFSET_MASK)
        | ((len & LENGTH_MASK) << LENGTH_SHIFT)
        | if escaped { ESCAPED } else { 0 }
}

fn offset_of(f: Field) -> usize {
    (f & OFFSET_MASK) as usize
}

fn len_of(f: Field) -> usize {
    ((f >> LENGTH_SHIFT) & LENGTH_MASK) as usize
}

fn is_escaped(f: Field) -> bool {
    f & ESCAPED != 0
}

// ---------------------------------------------------------------------------
// SWAR scanning
// ---------------------------------------------------------------------------

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

fn broadcast(b: u8) -> u64 {
    (b as u64) * ONES
}

/// Sets the high bit of every byte of `word` that equals `target`.
///
/// `diff - ONES` borrows across a byte only where that byte was zero, and
/// `!diff` cancels the false positives the borrow creates, so what survives
/// marks exactly the matching bytes.
fn match_bits(word: u64, target: u64) -> u64 {
    let diff = word ^ target;
    (diff.wrapping_sub(ONES)) & !diff & HIGH
}

/// The offset of the first byte at or after `from` that is `a` or `b`, or `end`.
fn next_of2(data: &[u8], from: usize, end: usize, a: u8, b: u8) -> usize {
    let (ba, bb) = (broadcast(a), broadcast(b));
    let mut at = from;
    while at + 8 <= end {
        let word = u64::from_le_bytes(data[at..at + 8].try_into().expect("eight bytes"));
        let hits = match_bits(word, ba) | match_bits(word, bb);
        if hits != 0 {
            return at + (hits.trailing_zeros() >> 3) as usize;
        }
        at += 8;
    }
    while at < end {
        if data[at] == a || data[at] == b {
            return at;
        }
        at += 1;
    }
    end
}

/// The offset of the first `target` at or after `from`, or `end`.
fn next_of1(data: &[u8], from: usize, end: usize, target: u8) -> usize {
    let bt = broadcast(target);
    let mut at = from;
    while at + 8 <= end {
        let word = u64::from_le_bytes(data[at..at + 8].try_into().expect("eight bytes"));
        let hits = match_bits(word, bt);
        if hits != 0 {
            return at + (hits.trailing_zeros() >> 3) as usize;
        }
        at += 8;
    }
    while at < end {
        if data[at] == target {
            return at;
        }
        at += 1;
    }
    end
}

/// Walks past a quoted field's body, returning the offset after its closing quote.
/// A doubled quote inside is content, not the end.
fn skip_quoted(data: &[u8], from: usize, end: usize) -> usize {
    let mut at = from;
    loop {
        let q = next_of1(data, at, end, b'"');
        if q >= end {
            return end;
        }
        if q + 1 < end && data[q + 1] == b'"' {
            at = q + 2;
            continue;
        }
        return q + 1;
    }
}

// ---------------------------------------------------------------------------
// The mapped file
// ---------------------------------------------------------------------------

/// The file's bytes.
///
/// A quoted field holding a doubled quote is the one value that is not a slice
/// of the file. Rather than copy it into a side buffer, such a field is flagged
/// and the second quote of each pair is skipped when the bytes are read — so
/// parsing needs no buffer, no allocation and no mutation, and a parser can be
/// shared and re-run freely.
struct Slab {
    _file: File,
    map: Mmap,
}

impl Slab {
    fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
        // Safety: the file is opened read-only and not modified while mapped.
        // A concurrent truncation would be a torn read, which is the documented
        // hazard of every mapping engine here and of DuckDB's reader too.
        let map = unsafe { Mmap::map(&file) }
            .map_err(|e| Error::new(format!("cannot map {}: {e}", path.display())))?;
        Ok(Slab { _file: file, map })
    }

    fn bytes(&self) -> &[u8] {
        &self.map
    }

    /// The field's raw span, still holding any doubled quotes.
    fn raw(&self, f: Field) -> &[u8] {
        if f == ABSENT || f == TOO_LONG {
            return &[];
        }
        let (at, len) = (offset_of(f), len_of(f));
        &self.map[at..at + len]
    }

    /// The field's logical bytes: the raw span with the second quote of each
    /// doubled pair dropped. Equality, hashing and decoding all read a field
    /// through this one route, so they cannot disagree about its value.
    fn logical(&self, f: Field) -> LogicalBytes<'_> {
        let real = f != ABSENT && f != TOO_LONG;
        LogicalBytes {
            raw: self.raw(f),
            escaped: real && is_escaped(f),
            at: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Splits rows into fields, projecting straight to the columns asked for.
///
/// Once the last needed column has been read the rest of the row is skipped to
/// its newline without its fields ever being delimited. On twenty columns keyed
/// on the first two that is most of the row never looked at.
struct RowParser {
    delimiter: u8,
    /// Where each projected column sits in the file, or `None` when absent.
    source: Vec<Option<usize>>,
    last_needed: usize,
}

impl RowParser {
    fn new(delimiter: u8, source: Vec<Option<usize>>) -> Self {
        let last_needed = source.iter().flatten().copied().max().unwrap_or(0);
        RowParser {
            delimiter,
            source,
            last_needed,
        }
    }

    /// Parses one row into `out`, returning the offset of the next row.
    ///
    /// A row shorter than the header leaves the missing fields [`ABSENT`], which
    /// compares as absent — a difference to report, not a file to refuse.
    fn parse(&self, data: &[u8], start: usize, end: usize, out: &mut [Field]) -> usize {
        out.fill(ABSENT);
        let mut pos = start;
        let mut column = 0usize;

        while pos <= end {
            let (field, next) = if pos < end && data[pos] == b'"' {
                let close = skip_quoted(data, pos + 1, end);
                let body_end = close.saturating_sub(1).max(pos + 1);
                let next = next_of2(data, close, end, self.delimiter, b'\n');
                (quoted_field(data, pos + 1, body_end), next)
            } else {
                let next = next_of2(data, pos, end, self.delimiter, b'\n');
                (plain_field(data, pos, next), next)
            };
            self.store(column, field, out);
            column += 1;

            if next >= end {
                return end;
            }
            if data[next] == b'\n' {
                return next + 1;
            }
            pos = next + 1;
            if column > self.last_needed {
                let eol = end_of_row(data, pos, end);
                return if eol >= end { end } else { eol + 1 };
            }
        }
        end
    }

    fn store(&self, column: usize, field: Field, out: &mut [Field]) {
        if column > self.last_needed {
            return;
        }
        for (i, at) in self.source.iter().enumerate() {
            if *at == Some(column) {
                out[i] = field;
            }
        }
    }
}

/// A quoted field, flagged when it holds a doubled quote. A quote inside the
/// body can only be half of such a pair, which is what makes the test a single
/// scan and lets the unescaping wait until the bytes are actually read.
fn quoted_field(data: &[u8], from: usize, to: usize) -> Field {
    let escaped = next_of1(data, from, to, b'"') < to;
    pack(from as u64, (to - from) as u64, escaped)
}

/// A field's logical bytes, dropping the second quote of each doubled pair.
struct LogicalBytes<'a> {
    raw: &'a [u8],
    escaped: bool,
    at: usize,
}

impl Iterator for LogicalBytes<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        let b = *self.raw.get(self.at)?;
        self.at += 1;
        if self.escaped && b == b'"' && self.raw.get(self.at) == Some(&b'"') {
            self.at += 1;
        }
        Some(b)
    }
}

impl LogicalBytes<'_> {
    /// True when no unescaping is needed, so callers can take the slice whole.
    fn is_plain(&self) -> bool {
        !self.escaped
    }
}

/// An unquoted field, with a trailing carriage return stripped so CRLF behaves like LF.
fn plain_field(data: &[u8], from: usize, to: usize) -> Field {
    let mut stop = to;
    if stop > from && data[stop - 1] == b'\r' {
        stop -= 1;
    }
    pack(from as u64, (stop - from) as u64, false)
}

/// The offset of the newline ending the row that starts at `pos`.
fn end_of_row(data: &[u8], pos: usize, end: usize) -> usize {
    let mut at = pos;
    while at < end {
        let next = next_of2(data, at, end, b'\n', b'"');
        if next >= end {
            return end;
        }
        if data[next] == b'"' {
            at = skip_quoted(data, next + 1, end);
            continue;
        }
        return next;
    }
    end
}

// ---------------------------------------------------------------------------
// Values: raw bytes on the fast path, the shared functions on the slow one
// ---------------------------------------------------------------------------

/// Whether the options ask for anything the raw bytes cannot answer.
fn needs_normalising(opt: &Options) -> bool {
    opt.trim || opt.ignore_case || opt.empty_is_null || opt.tolerance > 0.0
}

/// The field as the `Val` every other engine would have built for it.
fn value(slab: &Slab, f: Field, opt: &Options) -> Val {
    if f == ABSENT {
        return None;
    }
    normalise(empty_to_null(&text_of(slab, f)), opt)
}

/// The field decoded. Only the report sections and the normalising paths call
/// this; the fast path never builds a string for a cell at all.
fn text_of(slab: &Slab, f: Field) -> String {
    let logical = slab.logical(f);
    if logical.is_plain() {
        return String::from_utf8_lossy(slab.raw(f)).into_owned();
    }
    let bytes: Vec<u8> = logical.collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Whether two fields hold the same logical bytes, without decoding either.
fn same_bytes(a: &Slab, x: Field, b: &Slab, y: Field) -> bool {
    let (lx, ly) = (a.logical(x), b.logical(y));
    if lx.is_plain() && ly.is_plain() {
        return a.raw(x) == b.raw(y);
    }
    lx.eq(ly)
}

fn is_absent(slab: &Slab, f: Field, opt: &Options) -> bool {
    if f == ABSENT || len_of(f) == 0 {
        return true;
    }
    if !needs_normalising(opt) {
        return false;
    }
    value(slab, f, opt).is_none()
}

fn same(a: &Slab, x: Field, b: &Slab, y: Field, opt: &Options) -> bool {
    if !needs_normalising(opt) {
        let (xa, xb) = (is_absent(a, x, opt), is_absent(b, y, opt));
        if xa || xb {
            return xa && xb;
        }
        return same_bytes(a, x, b, y);
    }
    value(a, x, opt) == value(b, y, opt)
}

fn cell_differs(a: &Slab, x: Field, b: &Slab, y: Field, opt: &Options) -> bool {
    if !needs_normalising(opt) {
        return !same(a, x, b, y, opt);
    }
    differs(&value(a, x, opt), &value(b, y, opt), opt)
}

/// FNV-1a over the bytes equality would compare, so the two cannot disagree.
fn hash_field(slab: &Slab, f: Field, opt: &Options, seed: u64) -> u64 {
    const PRIME: u64 = 0x100_0000_01b3;
    let mut h = seed;
    if is_absent(slab, f, opt) {
        return (h ^ 0x9e37_79b9_7f4a_7c15).wrapping_mul(PRIME);
    }
    // Hash exactly the bytes equality compares, by the same route, so the two
    // cannot disagree: the Java port shipped two silently wrong answers when a
    // field reached the hash by one path and the comparison by another.
    let mut len = 0u64;
    if needs_normalising(opt) {
        let owned = value(slab, f, opt).unwrap_or_default();
        for b in owned.as_bytes() {
            h = (h ^ (*b as u64)).wrapping_mul(PRIME);
            len += 1;
        }
    } else {
        for b in slab.logical(f) {
            h = (h ^ (b as u64)).wrapping_mul(PRIME);
            len += 1;
        }
    }
    (h ^ len).wrapping_mul(PRIME)
}

fn key_hash(slab: &Slab, fields: &[Field], key_size: usize, opt: &Options) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325;
    for f in &fields[..key_size] {
        h = hash_field(slab, *f, opt, h);
    }
    h
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/// An open-addressing index over one file's rows, keyed on the composite key.
///
/// Everything is a primitive array: row starts, row hashes, and a table of row
/// numbers masked into a power-of-two slot count. Collisions are resolved by
/// comparing the key bytes, so the hash only has to be fast and spread.
struct RowIndex {
    row_start: Vec<u64>,
    row_hash: Vec<u64>,
    table: Vec<i32>,
    mask: usize,
    /// The row that first carried each distinct key, in first-appearance order.
    first_row: Vec<i32>,
    occurrences: Vec<u32>,
    rows: i64,
    dup_keys: i64,
    dup_rows: i64,
}

const EMPTY: i32 = -1;

impl RowIndex {
    fn build(
        slab: &Slab,
        parser: &RowParser,
        from: usize,
        width: usize,
        key_size: usize,
        opt: &Options,
    ) -> Result<Self> {
        let end = slab.bytes().len();
        let mut idx = RowIndex {
            row_start: Vec::new(),
            row_hash: Vec::new(),
            table: vec![EMPTY; 1 << 12],
            mask: (1 << 12) - 1,
            first_row: Vec::new(),
            occurrences: Vec::new(),
            rows: 0,
            dup_keys: 0,
            dup_rows: 0,
        };
        let mut fields = vec![ABSENT; width];
        let mut scratch = vec![ABSENT; width];
        let mut pos = from;
        while pos < end {
            // A line with nothing on it is not a row, which is what every other
            // reader here does.
            match slab.bytes()[pos] {
                b'\n' => {
                    pos += 1;
                    continue;
                }
                b'\r' if pos + 1 < end && slab.bytes()[pos + 1] == b'\n' => {
                    pos += 2;
                    continue;
                }
                _ => {}
            }
            let next = parser.parse(slab.bytes(), pos, end, &mut fields);
            if fields.contains(&TOO_LONG) {
                return Err(Error::new(format!(
                    "a field larger than {MAX_FIELD_LEN} bytes is more than this engine packs; \
                     use --engine native"
                )));
            }
            idx.add(slab, pos, &fields, key_size, opt, parser, &mut scratch);
            if next <= pos {
                break; // no progress: a malformed tail rather than an endless loop
            }
            pos = next;
        }
        Ok(idx)
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        slab: &Slab,
        start: usize,
        fields: &[Field],
        key_size: usize,
        opt: &Options,
        parser: &RowParser,
        scratch: &mut [Field],
    ) {
        self.rows += 1;
        let row = self.row_start.len() as i32;
        let hash = key_hash(slab, fields, key_size, opt);
        self.row_start.push(start as u64);
        self.row_hash.push(hash);

        let mut slot = self.slot(hash);
        loop {
            let at = self.table[slot];
            if at == EMPTY {
                self.table[slot] = self.first_row.len() as i32;
                self.first_row.push(row);
                self.occurrences.push(1);
                if self.first_row.len() * 2 > self.table.len() {
                    self.rehash();
                }
                return;
            }
            let candidate = self.first_row[at as usize];
            if self.row_hash[candidate as usize] == hash
                && self.same_key(slab, candidate, fields, key_size, opt, parser, scratch)
            {
                self.occurrences[at as usize] += 1;
                if self.occurrences[at as usize] == 2 {
                    self.dup_keys += 1;
                    self.dup_rows += 1; // the first occurrence counts once the key is known to repeat
                }
                self.dup_rows += 1;
                return;
            }
            slot = (slot + 1) & self.mask;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn same_key(
        &self,
        slab: &Slab,
        candidate: i32,
        fields: &[Field],
        key_size: usize,
        opt: &Options,
        parser: &RowParser,
        probe: &mut [Field],
    ) -> bool {
        self.fields_of(slab, parser, candidate, probe);
        (0..key_size).all(|i| same(slab, probe[i], slab, fields[i], opt))
    }

    /// Re-parses a row. The index stores where each row starts, not its fields,
    /// because a row number and an offset are sixteen bytes where the fields
    /// would be twenty times that. Re-parsing is cheap because the parser stops
    /// at the last needed column, and it needs no mutable state.
    fn fields_of(&self, slab: &Slab, parser: &RowParser, row: i32, out: &mut [Field]) {
        let start = self.row_start[row as usize] as usize;
        parser.parse(slab.bytes(), start, slab.bytes().len(), out);
    }

    fn slot(&self, hash: u64) -> usize {
        // The high bits of an FNV hash are the well-mixed ones; fold them down.
        ((hash ^ (hash >> 32)) as usize) & self.mask
    }

    fn rehash(&mut self) {
        let size = self.table.len() * 2;
        self.table = vec![EMPTY; size];
        self.mask = size - 1;
        for key in 0..self.first_row.len() {
            let row = self.first_row[key];
            let hash = self.row_hash[row as usize];
            let mut slot = self.slot(hash);
            while self.table[slot] != EMPTY {
                slot = (slot + 1) & self.mask;
            }
            self.table[slot] = key as i32;
        }
    }

    /// The row carrying `fields`' key in this index, or `None`.
    #[allow(clippy::too_many_arguments)]
    fn lookup(
        &self,
        slab: &Slab,
        other: &Slab,
        fields: &[Field],
        hash: u64,
        key_size: usize,
        opt: &Options,
        parser: &RowParser,
        probe: &mut [Field],
    ) -> Option<i32> {
        let mut slot = self.slot(hash);
        loop {
            let at = self.table[slot];
            if at == EMPTY {
                return None;
            }
            let candidate = self.first_row[at as usize];
            if self.row_hash[candidate as usize] == hash {
                self.fields_of(slab, parser, candidate, probe);
                if (0..key_size).all(|i| same(slab, probe[i], other, fields[i], opt)) {
                    return Some(candidate);
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }

    fn unique_keys(&self) -> i64 {
        self.first_row.len() as i64
    }
}

// ---------------------------------------------------------------------------
// The join
// ---------------------------------------------------------------------------

/// A row chosen for a report section, with the row it matched on the other side.
struct Pick {
    row: i32,
    mate: i32,
}

/// A row list that stops growing at the report cap but keeps counting.
struct Capped {
    held: Vec<Pick>,
    cap: usize,
    unbounded: bool,
    total: i64,
}

impl Capped {
    fn new(cap: usize, unbounded: bool) -> Self {
        Capped {
            held: Vec::new(),
            cap,
            unbounded,
            total: 0,
        }
    }

    fn push(&mut self, pick: Pick) {
        self.total += 1;
        // One past the cap, so a section can still report that it was truncated.
        if self.unbounded || self.held.len() <= self.cap {
            self.held.push(pick);
        }
    }
}

/// Decodes one row's key and compared columns into the owned values the report
/// holds. This is the only place a cell becomes a `String`, and it runs at most
/// `--max-rows` times per section rather than once per row in the file.
fn row_values(
    slab: &Slab,
    parser: &RowParser,
    idx: &RowIndex,
    row: i32,
    width: usize,
    opt: &Options,
) -> Vec<Val> {
    let mut fields = vec![ABSENT; width];
    idx.fields_of(slab, parser, row, &mut fields);
    fields.iter().map(|f| value(slab, *f, opt)).collect()
}

fn to_cells(values: &[Val]) -> Vec<Cell> {
    values.iter().map(|v| Cell::Value(v.clone())).collect()
}

#[allow(clippy::too_many_arguments)]
fn join(
    a: &Slab,
    ai: &RowIndex,
    ap: &RowParser,
    b: &Slab,
    bi: &RowIndex,
    bp: &RowParser,
    opt: &Options,
    compared: &[String],
    exporting: bool,
) -> Joined {
    let key_size = opt.key.len();
    let nc = compared.len();
    let width = key_size + nc;

    let mut changed_per = vec![0i64; nc];
    let mut blanked_per = vec![0i64; nc];
    let mut filled_per = vec![0i64; nc];

    let mut changed = Capped::new(opt.max_rows, exporting);
    let mut added = Capped::new(opt.max_rows, exporting);
    let mut removed = Capped::new(opt.max_rows, exporting);
    let mut matched = 0i64;

    let mut fa = vec![ABSENT; width];
    let mut fb = vec![ABSENT; width];
    let mut probe = vec![ABSENT; width];

    // A's distinct keys, in first-appearance order, so a run is reproducible.
    for &row in &ai.first_row {
        ai.fields_of(a, ap, row, &mut fa);
        let hash = key_hash(a, &fa, key_size, opt);
        let Some(mate) = bi.lookup(b, a, &fa, hash, key_size, opt, bp, &mut probe) else {
            removed.push(Pick { row, mate: -1 });
            continue;
        };
        matched += 1;
        bi.fields_of(b, bp, mate, &mut fb);

        let mut any = false;
        for i in 0..nc {
            let (x, y) = (fa[key_size + i], fb[key_size + i]);
            if cell_differs(a, x, b, y, opt) {
                any = true;
                changed_per[i] += 1;
                if is_absent(b, y, opt) {
                    blanked_per[i] += 1;
                }
                if is_absent(a, x, opt) {
                    filled_per[i] += 1;
                }
            }
        }
        if any {
            changed.push(Pick { row, mate });
        }
    }
    for &row in &bi.first_row {
        bi.fields_of(b, bp, row, &mut fb);
        let hash = key_hash(b, &fb, key_size, opt);
        if ai
            .lookup(a, b, &fb, hash, key_size, opt, ap, &mut probe)
            .is_none()
        {
            added.push(Pick { row, mate: -1 });
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

    // Only now does anything become a String, and only for the rows kept.
    let mut removed_rows: Vec<Vec<Val>> = removed
        .held
        .iter()
        .map(|p| row_values(a, ap, ai, p.row, width, opt))
        .collect();
    let mut added_rows: Vec<Vec<Val>> = added
        .held
        .iter()
        .map(|p| row_values(b, bp, bi, p.row, width, opt))
        .collect();
    let mut changed_a: Vec<Vec<Val>> = changed
        .held
        .iter()
        .map(|p| row_values(a, ap, ai, p.row, width, opt))
        .collect();
    let mut changed_b: Vec<Vec<Val>> = changed
        .held
        .iter()
        .map(|p| row_values(b, bp, bi, p.mate, width, opt))
        .collect();

    sort_rows(&mut removed_rows, key_size);
    sort_rows(&mut added_rows, key_size);
    sort_changed_together(&mut changed_a, &mut changed_b, key_size);

    let changed_cells: Vec<Vec<Cell>> = changed_a
        .iter()
        .zip(&changed_b)
        .map(|(ar, br)| {
            let mut cells: Vec<CellDiff> = Vec::new();
            for i in 0..nc {
                let (x, y) = (&ar[key_size + i], &br[key_size + i]);
                if differs(x, y, opt) {
                    cells.push(CellDiff {
                        column: i,
                        a: x.clone(),
                        b: y.clone(),
                    });
                }
            }
            let mut row: Vec<Cell> = ar[..key_size]
                .iter()
                .map(|v| Cell::Value(v.clone()))
                .collect();
            row.push(Cell::Diffs(cells));
            row
        })
        .collect();

    let counts = Counts {
        a_rows: ai.rows,
        b_rows: bi.rows,
        a_keys: ai.unique_keys(),
        b_keys: bi.unique_keys(),
        matched,
        unchanged: matched - changed.total,
        changed: changed.total,
        added: added.total,
        removed: removed.total,
        a_dup_keys: ai.dup_keys,
        a_dup_rows: ai.dup_rows,
        b_dup_keys: bi.dup_keys,
        b_dup_rows: bi.dup_rows,
    };

    Joined {
        counts,
        columns,
        changed: changed_cells,
        added: added_rows.iter().map(|r| to_cells(r)).collect(),
        removed: removed_rows.iter().map(|r| to_cells(r)).collect(),
        changed_a,
        changed_b,
    }
}

fn sort_rows(rows: &mut [Vec<Val>], key_size: usize) {
    rows.sort_by(|x, y| compare_keys(x, y, key_size));
}

/// Sorts the changed rows by key while keeping the parallel A and B lists in step.
fn sort_changed_together(a: &mut Vec<Vec<Val>>, b: &mut Vec<Vec<Val>>, key_size: usize) {
    let mut order: Vec<usize> = (0..a.len()).collect();
    order.sort_by(|&p, &q| compare_keys(&a[p], &a[q], key_size));
    *a = order.iter().map(|&i| a[i].clone()).collect();
    *b = order.iter().map(|&i| b[i].clone()).collect();
}

/// The duplicate-key section: most duplicated first, then by key.
fn duplicate_section(
    slab: &Slab,
    idx: &RowIndex,
    parser: &RowParser,
    width: usize,
    opt: &Options,
) -> Section {
    let key_size = opt.key.len();
    let mut entries: Vec<(Vec<Val>, i64)> = idx
        .first_row
        .iter()
        .zip(&idx.occurrences)
        .filter(|(_, n)| **n > 1)
        .map(|(row, n)| {
            let values = row_values(slab, parser, idx, *row, width, opt);
            (values[..key_size].to_vec(), *n as i64)
        })
        .collect();
    entries.sort_by(|x, y| {
        y.1.cmp(&x.1)
            .then_with(|| compare_keys(&x.0, &y.0, key_size))
    });

    let total = entries.len();
    let rows: Vec<Vec<Cell>> = entries
        .into_iter()
        .take(opt.max_rows)
        .map(|(key, count)| {
            let mut row: Vec<Cell> = key.iter().map(|v| Cell::Value(v.clone())).collect();
            row.push(Cell::Count(count));
            row
        })
        .collect();
    let mut cols = opt.key.clone();
    cols.push("count".to_string());
    Section {
        cols,
        rows,
        truncated: total > opt.max_rows,
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn compare(a_path: &Path, b_path: &Path, opt: &Options) -> Result<EngineResult> {
    let a = Slab::open(a_path)?;
    let b = Slab::open(b_path)?;

    let a_delim = delimiter(&a, opt)?;
    let b_delim = delimiter(&b, opt)?;
    let (a_header, a_start) = header(&a, a_delim, a_path)?;
    let (b_header, b_start) = header(&b, b_delim, b_path)?;
    let resolved = resolve(&a_header, &b_header, opt)?;

    let key_size = opt.key.len();
    let width = key_size + resolved.compared.len();
    let wanted: Vec<&String> = opt.key.iter().chain(&resolved.compared).collect();

    let ap = RowParser::new(
        a_delim,
        wanted
            .iter()
            .map(|n| a_header.iter().position(|c| &c == n))
            .collect(),
    );
    let bp = RowParser::new(
        b_delim,
        wanted
            .iter()
            .map(|n| b_header.iter().position(|c| &c == n))
            .collect(),
    );

    let ai = RowIndex::build(&a, &ap, a_start, width, key_size, opt)?;
    let bi = RowIndex::build(&b, &bp, b_start, width, key_size, opt)?;

    let dup_a = duplicate_section(&a, &ai, &ap, width, opt);
    let dup_b = duplicate_section(&b, &bi, &bp, width, opt);
    let joined = join(
        &a,
        &ai,
        &ap,
        &b,
        &bi,
        &bp,
        opt,
        &resolved.compared,
        opt.export_dir.is_some(),
    );

    let meta = resolved.meta(&opt.key, a_header.len(), b_header.len());
    assemble(meta, joined, dup_a, dup_b, opt, &resolved.compared)
}

fn delimiter(slab: &Slab, opt: &Options) -> Result<u8> {
    if let Some(d) = opt.delimiter_byte()? {
        return Ok(d);
    }
    let data = slab.bytes();
    let line_end = next_of1(data, 0, data.len(), b'\n');
    Ok(detect_delimiter(&String::from_utf8_lossy(
        &data[..line_end],
    )))
}

/// The header row's names, and where the first data row starts.
fn header(slab: &Slab, delimiter: u8, path: &Path) -> Result<(Vec<String>, usize)> {
    let data = slab.bytes();
    let end = data.len();
    if end == 0 {
        return Err(Error::new(format!(
            "file has no header row: {}",
            path.display()
        )));
    }
    let mut names = Vec::new();
    let mut pos = 0usize;
    loop {
        let (field, next) = if pos < end && data[pos] == b'"' {
            let close = skip_quoted(data, pos + 1, end);
            let body_end = close.saturating_sub(1).max(pos + 1);
            (
                quoted_field(data, pos + 1, body_end),
                next_of2(data, close, end, delimiter, b'\n'),
            )
        } else {
            let next = next_of2(data, pos, end, delimiter, b'\n');
            (plain_field(data, pos, next), next)
        };
        names.push(text_of(slab, field));
        if next >= end {
            return Ok((names, end));
        }
        if data[next] == b'\n' {
            return Ok((names, next + 1));
        }
        pos = next + 1;
    }
}

/// The turbo engine has no optional dependency.
pub fn available() -> bool {
    true
}
