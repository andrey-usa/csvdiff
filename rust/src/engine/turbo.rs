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
//! That is the same design as the Java `turbo` engine and the C, C++ and Zig
//! ports, and it exists here for a specific reason: Java `turbo` beats Rust
//! `native` by nearly six times on time and four on memory, which reads as a
//! language result and is not one. With both designs in one language the
//! comparison is honest.
//!
//! Three things are layered on that base, and each is worth a sentence:
//!
//! - **Three input formats.** CSV, newline-delimited JSON and Parquet all reduce
//!   to the same field word, so the two sides of a comparison need not be in the
//!   same format: a CSV export compares against the Parquet a warehouse emits.
//!   See [`text`] and [`parquet`].
//! - **Every core.** Each file is split at row boundaries, parsed and hashed in
//!   parallel, and inserted into its index in file order; the join then splits
//!   over contiguous ranges of A's keys. Ordering is preserved where it is
//!   load-bearing, so the answer does not depend on the thread count.
//! - **Normalisation is delegated rather than reimplemented.** With `--trim`,
//!   `--ignore-case`, `--empty-is-null` or a tolerance in play a field is decoded
//!   and handed to the same [`crate::columns`] functions every other engine uses,
//!   so the answer cannot drift; without them the raw bytes are compared and
//!   hashed directly, which is the path the benchmarks take.
//!
//! Hashing and equality always read the same bytes by the same route — the
//! property whose absence produced two silently wrong answers in the Java port.

mod codec;
mod encoding;
mod field;
mod parquet;
mod slab;
mod text;
mod thrift;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use field::{ABSENT, Field, MAX_FIELD_LEN, TOO_LONG, count_byte, next_of1};
use slab::{Dialect, Slab, same_bytes, text_of};
use text::{RowParser, csv_header, detect_delimiter, json_header, sniff_dialect};

use crate::columns::{compare_keys, differs, empty_to_null, normalise, resolve};
use crate::contract::{Cell, CellDiff, ColumnStat, Counts, EngineResult, Section, Val};
use crate::error::{Error, Result};
use crate::options::Options;
use crate::rowstore::Joined;
use crate::sections::assemble;

/// Below this there is nothing to divide: finding the chunk boundaries would
/// cost more than the parsing it splits.
const CHUNKING_THRESHOLD: usize = 4 << 20;

/// How many keys make the join worth splitting.
const JOIN_THRESHOLD: usize = 1 << 14;

/// How many rows ahead a table probe is started. Enough misses in flight to
/// cover the latency of one, and not so many that the lines are evicted before
/// the loop reaches them.
const PREFETCH_AHEAD: usize = 32;

/// Keys per join chunk.
///
/// Sized in rows rather than in threads so that the chunk boundaries -- and so
/// which rows a capped section keeps -- are the same at any `--threads`. Small
/// enough that no single chunk can be the last thing four cores are waiting on:
/// at ten million keys this is a couple of hundred chunks of a few hundredths
/// of a second each.
const JOIN_CHUNK: usize = 1 << 16;

/// How many threads this comparison may use in total. Both files are read at
/// once and each is split further, so this is the width of the whole run rather
/// than of one file.
fn budget(opt: &Options) -> usize {
    opt.threads.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    })
}

// ---------------------------------------------------------------------------
// One file, read into the representation the join works on
// ---------------------------------------------------------------------------

/// How a file's rows are addressed.
enum Rows {
    /// Text: a row is an offset into the mapping, re-parsed on demand. The index
    /// stores where a row starts rather than its fields, because an offset is
    /// eight bytes where the fields would be twenty times that, and re-parsing is
    /// cheap because the parser stops at the last needed column.
    Text { parser: RowParser, from: usize },
    /// Parquet: there is no row to re-read, so the fields are materialised once,
    /// row-major, and a row is an index into them.
    Columnar { fields: Vec<Field>, rows: usize },
}

/// One file: the bytes its fields point into, and how to get a row's fields.
struct Side {
    slab: Slab,
    rows: Rows,
    width: usize,
}

impl Side {
    /// The fields of the row addressed by `at` — a byte offset for a text file,
    /// a row number for a columnar one.
    fn fields_at(&self, at: u64, out: &mut [Field]) {
        match &self.rows {
            Rows::Text { parser, .. } => {
                let data = self.slab.data();
                parser.parse(data, at as usize, data.len(), out);
            }
            Rows::Columnar { fields, .. } => {
                let from = at as usize * self.width;
                out.copy_from_slice(&fields[from..from + self.width]);
            }
        }
    }
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

fn is_absent(slab: &Slab, f: Field, opt: &Options) -> bool {
    if f == ABSENT || field::len_of(f) == 0 || f == TOO_LONG {
        return true;
    }
    if !needs_normalising(opt) {
        return false;
    }
    value(slab, f, opt).is_none()
}

fn same(a: &Slab, x: Field, b: &Slab, y: Field, opt: &Options) -> bool {
    if !needs_normalising(opt) {
        let (xa, yb) = (is_absent(a, x, opt), is_absent(b, y, opt));
        if xa || yb {
            return xa && yb;
        }
        return same_bytes(a, x, b, y);
    }
    value(a, x, opt) == value(b, y, opt)
}

/// Whether a field is absent, when nothing has to be normalised first.
///
/// The general `is_absent` may decode the value; with no normalising it is two
/// bit tests on the field word, and the join asks it three times per compared
/// cell -- once inside `same`, then twice more for the blanked and filled
/// counters, having thrown the first answer away.
#[inline]
fn plain_absent(f: Field) -> bool {
    !field::is_real(f) || field::len_of(f) == 0
}

/// `cell_differs` for the common case, with each side's absence already known.
/// Both absent is not a difference; exactly one is.
#[inline]
fn plain_differs(a: &Slab, x: Field, xa: bool, b: &Slab, y: Field, yb: bool) -> bool {
    if xa || yb {
        return xa != yb;
    }
    !same_bytes(a, x, b, y)
}

fn cell_differs(a: &Slab, x: Field, b: &Slab, y: Field, opt: &Options) -> bool {
    if !needs_normalising(opt) {
        return !same(a, x, b, y, opt);
    }
    differs(&value(a, x, opt), &value(b, y, opt), opt)
}

/// FNV-1a over eight bytes at a time.
///
/// A hash is internal — nothing outside this engine can see one — so the only
/// property it owes anyone is that the index build and the join probe compute
/// the same number for the same bytes. That is what lets the common path read a
/// word at a time: a key of twenty-six bytes costs four multiplies instead of
/// twenty-six, and the key hash is computed four times over at every row (both
/// files, indexed then probed), which made byte-at-a-time FNV about a billion
/// dependent multiply-xor steps of a ten-million-row run.
fn hash_bytes(bytes: &[u8], seed: u64) -> u64 {
    const PRIME: u64 = 0x100_0000_01b3;
    let mut h = seed;
    let mut chunks = bytes.as_chunks::<8>().0.iter();
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(*chunk);
        h = (h ^ word).wrapping_mul(PRIME);
        // The xor-shift is what spreads a whole word into the low bits, which
        // is where the table's slot comes from.
        h ^= h >> 29;
    }
    let tail = &bytes[bytes.len() - bytes.len() % 8..];
    if !tail.is_empty() {
        let mut word = [0u8; 8];
        word[..tail.len()].copy_from_slice(tail);
        h = (h ^ u64::from_le_bytes(word)).wrapping_mul(PRIME);
        h ^= h >> 29;
    }
    h
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
    } else if !field::is_escaped(f) {
        // Nothing to unescape, so the bytes are the value and eight of them can
        // be taken at a time. See `hash_bytes`.
        //
        // The field word is asked directly rather than through `logical()`,
        // which answers the same question by building an iterator over the slab
        // -- the cost `same_bytes` used to pay, in the one function that runs
        // for every key column of every row of both files.
        let raw = slab.raw(f);
        h = hash_bytes(raw, h);
        len = raw.len() as u64;
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

/// Phase timings, on stderr, when `CSVDIFF_PHASES` is set — the same switch and
/// the same output shape the C++ columnar path uses.
///
/// A comparison has costs that move independently: sweeping and hashing every
/// row, inserting them into the index, joining, and rendering. Knowing which one
/// grew is the difference between tuning and guessing, and the cores-busy column
/// says only *that* something is serial, never which thing.
struct Phases {
    on: bool,
    tag: &'static str,
    last: Instant,
}

impl Phases {
    /// `tag` prefixes every line, because the two files are read on two threads
    /// and their phases would otherwise interleave unattributed.
    fn new(tag: &'static str) -> Self {
        Phases {
            on: std::env::var_os("CSVDIFF_PHASES").is_some(),
            tag,
            last: Instant::now(),
        }
    }

    fn mark(&mut self, what: &str) {
        let now = Instant::now();
        if self.on {
            let name = format!("{}{}", self.tag, what);
            eprintln!("  {:<26} {:7.3}s", name, (now - self.last).as_secs_f64());
        }
        self.last = now;
    }
}

/// An empty table of `cap` slots, with the zeroes written rather than taken from
/// the kernel.
///
/// `vec![EMPTY_SLOT; cap]` compiles to a calloc, and a fresh anonymous mapping is
/// one shared page of zeroes until something writes to it. Nothing reads this
/// table before the inserts start, so each of its thirty-odd thousand pages would
/// be touched first by a random probe -- a page fault landing in the middle of
/// the dependent load chain the prefetch is there to hide. Faulting them in order
/// instead is work the kernel is far better at: at ten million rows it halves the
/// insert, 1.55s to 0.74s.
fn empty_table(cap: usize) -> Vec<u64> {
    let mut table = Vec::with_capacity(cap);
    table.resize(cap, EMPTY_SLOT);
    table
}

/// A slot holds the top bits of its key's hash and the position in `first_row`
/// plus one, so zero means empty.
///
/// Carrying the tag is what makes a failed probe cheap: the word already loaded
/// settles it. A table of bare positions has to follow each one into
/// `first_row` and then into `row_hash` -- two dependent random loads, over
/// arrays far too big to cache at ten million keys -- only to reject it. The
/// Zig and C++ ports both do this; this port did not, and it cost more the
/// larger the file got.
const POS_MASK: u64 = (1 << 40) - 1;
const EMPTY_SLOT: u64 = 0;

fn slot_for(hash: u64, pos: usize) -> u64 {
    (hash & !POS_MASK) | (pos as u64 + 1)
}

fn tag_is(slot: u64, hash: u64) -> bool {
    (slot ^ hash) & !POS_MASK == 0
}

fn pos_of(slot: u64) -> usize {
    ((slot & POS_MASK) - 1) as usize
}

/// One chunk's rows, in the order they appear in it.
struct Chunk {
    at: Vec<u64>,
    hash: Vec<u64>,
}

/// An open-addressing index over one file's rows, keyed on the composite key.
///
/// Everything is a primitive array: where each row is, its key hash, and a table
/// of key numbers masked into a power-of-two slot count. Collisions are resolved
/// by comparing the key bytes, so the hash only has to be fast and spread.
struct RowIndex {
    row_at: Vec<u64>,
    row_hash: Vec<u64>,
    table: Vec<u64>,
    mask: usize,
    /// The row that first carried each distinct key, in first-appearance order.
    first_row: Vec<i32>,
    occurrences: Vec<u32>,
    rows: i64,
    dup_keys: i64,
    dup_rows: i64,
}

impl RowIndex {
    /// Finds and hashes every row in parallel, then inserts them on one thread in
    /// file order.
    ///
    /// The split is safe because the two halves need different things: parsing a
    /// row depends on nothing but where it starts, while the table depends on the
    /// order rows arrive — first occurrence of a key wins, and the duplicate
    /// counts follow from that. Doing the second half in parallel would make the
    /// answer depend on thread scheduling.
    fn build(
        side: &Side,
        key_size: usize,
        opt: &Options,
        threads: usize,
        tag: &'static str,
    ) -> Result<Self> {
        let mut phases = Phases::new(tag);
        let mut chunks = sweep(side, key_size, opt, threads)?;
        phases.mark("sweep (parallel)");

        let total: usize = chunks.iter().map(|c| c.at.len()).sum();
        // Sized once for the rows about to be inserted, at about a two-thirds
        // load. Starting at four thousand and doubling meant twelve rehashes at
        // ten million rows, each one a full random-access pass over a table
        // already too big to cache -- work that grows with the file and is
        // entirely avoidable, since the row count is known before the first
        // insert.
        //
        // Two thirds and not a half: sizing for a half load doubles the table,
        // and at ten million keys the 268 MB that costs per side is worth more
        // than the probes it saves. Measured both ways, the denser table wins the
        // insert by 27% and the join by 6%. `first_row` and `occurrences` are sized the same way: they end
        // up one entry per distinct key, and every key is distinct until proven
        // otherwise.
        let mut cap: usize = 1 << 12;
        while cap * 2 < total * 3 + 16 {
            cap <<= 1;
        }
        let mut idx = RowIndex {
            row_at: Vec::with_capacity(total),
            row_hash: Vec::with_capacity(total),
            table: empty_table(cap),
            mask: cap - 1,
            first_row: Vec::with_capacity(total),
            occurrences: Vec::with_capacity(total),
            rows: 0,
            dup_keys: 0,
            dup_rows: 0,
        };
        let mut probe = vec![ABSENT; side.width];
        let mut mine = vec![ABSENT; side.width];
        // Each chunk is released as soon as it has been inserted. Holding all of
        // them to the end would keep two copies of every row's address and hash
        // alive at once, which is sixteen bytes a row of pure duplication —
        // 320 MB at ten million rows across both files.
        for chunk in &mut chunks {
            for i in 0..chunk.at.len() {
                if let Some(&soon) = chunk.hash.get(i + PREFETCH_AHEAD) {
                    idx.prefetch(soon);
                }
                idx.insert(
                    side,
                    chunk.at[i],
                    chunk.hash[i],
                    key_size,
                    opt,
                    &mut probe,
                    &mut mine,
                );
            }
            chunk.at = Vec::new();
            chunk.hash = Vec::new();
        }
        phases.mark("index insert (serial)");
        Ok(idx)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        &mut self,
        side: &Side,
        at: u64,
        hash: u64,
        key_size: usize,
        opt: &Options,
        probe: &mut [Field],
        mine: &mut [Field],
    ) {
        self.rows += 1;
        let row = self.row_at.len() as i32;
        self.row_at.push(at);
        self.row_hash.push(hash);

        let mut slot = self.slot(hash);
        let mut mine_parsed = false;
        loop {
            let word = self.table[slot];
            if word == EMPTY_SLOT {
                self.table[slot] = slot_for(hash, self.first_row.len());
                self.first_row.push(row);
                self.occurrences.push(1);
                // Two thirds, which is what `build` sizes the table for. Half was
                // the wrong number in the wrong place: a file whose keys are
                // nearly all distinct crossed it and doubled a table that had
                // been sized precisely so it would not have to.
                if self.first_row.len() * 3 > self.table.len() * 2 {
                    self.rehash();
                }
                return;
            }
            // The tag rejects almost every collision without leaving this word.
            if tag_is(word, hash) {
                let key = pos_of(word);
                let candidate = self.first_row[key];
                if self.row_hash[candidate as usize] == hash {
                    // This row's fields are re-parsed rather than carried over from
                    // the sweep because the sweep produced ten million of them and
                    // this branch wants one.
                    if !mine_parsed {
                        side.fields_at(at, mine);
                        mine_parsed = true;
                    }
                    self.fields_of(side, candidate, probe);
                    if (0..key_size).all(|i| same(&side.slab, probe[i], &side.slab, mine[i], opt)) {
                        self.occurrences[key] += 1;
                        if self.occurrences[key] == 2 {
                            self.dup_keys += 1;
                            self.dup_rows += 1; // the first occurrence counts once the key repeats
                        }
                        self.dup_rows += 1;
                        return;
                    }
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }

    fn fields_of(&self, side: &Side, row: i32, out: &mut [Field]) {
        side.fields_at(self.row_at[row as usize], out);
    }

    fn slot(&self, hash: u64) -> usize {
        // The high bits of an FNV hash are the well-mixed ones; fold them down.
        ((hash ^ (hash >> 32)) as usize) & self.mask
    }

    /// Starts the fetch of the slot `hash` will land in, without waiting for it.
    ///
    /// Every probe of this table is a random access into tens of megabytes, so
    /// it misses to memory, and the row after it needs a different line: the loop
    /// spends most of its time waiting on a load whose address was known long
    /// before it was issued. Asking for the line `PREFETCH_AHEAD` rows early
    /// turns that serial chain of misses into overlapping ones.
    #[inline]
    fn prefetch(&self, hash: u64) {
        #[cfg(target_arch = "x86_64")]
        // Safety: `slot` masks into the table's length, so the pointer is in
        // bounds, and a prefetch has no architectural effect in any case.
        unsafe {
            use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
            _mm_prefetch::<_MM_HINT_T0>(self.table.as_ptr().add(self.slot(hash)) as *const i8);
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = hash;
    }

    /// Only reached if the row count was underestimated: `build` sizes the table
    /// for the rows it is about to insert, so the common path never grows it.
    fn rehash(&mut self) {
        let size = self.table.len() * 2;
        self.table = empty_table(size);
        self.mask = size - 1;
        for key in 0..self.first_row.len() {
            let row = self.first_row[key];
            let hash = self.row_hash[row as usize];
            let mut slot = self.slot(hash);
            while self.table[slot] != EMPTY_SLOT {
                slot = (slot + 1) & self.mask;
            }
            self.table[slot] = slot_for(hash, key);
        }
    }

    /// The row carrying `fields`' key in this index, or `None`. `other` is the
    /// side those fields live in, which is the opposite file when this is a join
    /// probe. `probe` is scratch the caller owns: the join runs several ranges
    /// at once, and a buffer hanging off the index would be shared between them.
    ///
    /// On `Some`, `probe` holds that row's fields — it is what the key columns
    /// were compared against. The join used to parse the row again on the line
    /// after this one returned, which is a second parse of every matched row in
    /// the file.
    #[allow(clippy::too_many_arguments)]
    fn lookup(
        &self,
        side: &Side,
        other: &Slab,
        fields: &[Field],
        hash: u64,
        key_size: usize,
        opt: &Options,
        probe: &mut [Field],
    ) -> Option<i32> {
        let mut slot = self.slot(hash);
        loop {
            let word = self.table[slot];
            if word == EMPTY_SLOT {
                return None;
            }
            if tag_is(word, hash) {
                let candidate = self.first_row[pos_of(word)];
                if self.row_hash[candidate as usize] == hash {
                    self.fields_of(side, candidate, probe);
                    if (0..key_size).all(|i| same(&side.slab, probe[i], other, fields[i], opt)) {
                        return Some(candidate);
                    }
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
// The sweep: finding and hashing every row, in parallel
// ---------------------------------------------------------------------------

fn sweep(side: &Side, key_size: usize, opt: &Options, threads: usize) -> Result<Vec<Chunk>> {
    match &side.rows {
        Rows::Text { parser, from } => sweep_text(side, parser, *from, key_size, opt, threads),
        Rows::Columnar { rows, .. } => sweep_columnar(side, *rows, key_size, opt, threads),
    }
}

/// Runs `each` over `0..parts` on that many threads, collecting what they return
/// in order. A thread that cannot be spawned is not a failure: it is the same
/// work, done here.
fn in_parallel<T, F>(parts: usize, each: F) -> Vec<Result<T>>
where
    T: Send,
    F: Fn(usize) -> Result<T> + Sync,
{
    if parts <= 1 {
        return vec![each(0)];
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = (1..parts)
            .map(|i| {
                let each = &each;
                scope.spawn(move || each(i))
            })
            .collect();
        let mut out = vec![each(0)];
        for handle in handles {
            out.push(
                handle
                    .join()
                    .unwrap_or_else(|_| Err(Error::new("a comparison worker panicked"))),
            );
        }
        out
    })
}

/// Maps `items` into an owned `Vec` across `threads` threads, in order.
///
/// The report's rows are the one place this engine turns cells into `String`s,
/// and it does it up to two hundred thousand times — fifty thousand changed
/// rows on both sides, plus the added and removed lists — over twenty columns
/// each. Every row is independent of every other, so the only reason it was
/// serial is that nobody had split it, and it is the whole of the gap between
/// the Rust port's cores-busy ratio and the C++ port's.
///
/// Chunked rather than one thread per list: the four lists are very uneven —
/// fifty thousand against ten — so dealing them out whole would leave two
/// threads idle while the other two did all of it.
fn map_rows<T, U, F>(items: &[T], threads: usize, each: F) -> Vec<U>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> U + Sync,
{
    let parts = threads.clamp(1, items.len().div_ceil(1 << 12).max(1));
    if parts <= 1 {
        return items.iter().map(&each).collect();
    }
    let chunk = items.len().div_ceil(parts);
    let mut out: Vec<U> = Vec::with_capacity(items.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .skip(1)
            .map(|part| {
                let each = &each;
                scope.spawn(move || part.iter().map(each).collect::<Vec<U>>())
            })
            .collect();
        out.extend(items.chunks(chunk).next().into_iter().flatten().map(&each));
        for handle in handles {
            out.extend(handle.join().expect("a report row worker"));
        }
    });
    out
}

/// Parses and hashes every row of a mapped text file, in `threads` chunks.
fn sweep_text(
    side: &Side,
    parser: &RowParser,
    from: usize,
    key_size: usize,
    opt: &Options,
    threads: usize,
) -> Result<Vec<Chunk>> {
    let data = side.slab.data();
    if from >= data.len() {
        return Ok(Vec::new());
    }
    let bounds = chunk_bounds(data, from, threads, side.slab.dialect());
    let parts = bounds.len() - 1;

    let results = in_parallel(parts, |i| {
        let (begin, stop) = (bounds[i], bounds[i + 1]);
        let mut chunk = Chunk {
            at: Vec::new(),
            hash: Vec::new(),
        };
        let mut fields = vec![ABSENT; side.width];
        let mut pos = begin;
        // Rows that *start* in this chunk belong to it; the last one is finished
        // past the boundary rather than cut in half.
        while pos < stop {
            match data[pos] {
                // A line with nothing on it is not a row.
                b'\n' => {
                    pos += 1;
                    continue;
                }
                b'\r' if pos + 1 < data.len() && data[pos + 1] == b'\n' => {
                    pos += 2;
                    continue;
                }
                _ => {}
            }
            let next = parser.parse(data, pos, data.len(), &mut fields);
            if fields.contains(&TOO_LONG) {
                return Err(Error::new(format!(
                    "a field larger than {MAX_FIELD_LEN} bytes is more than this engine packs; \
                     use --engine native"
                )));
            }
            chunk.at.push(pos as u64);
            chunk
                .hash
                .push(key_hash(&side.slab, &fields, key_size, opt));
            if next <= pos {
                break; // no progress: a malformed tail rather than an endless loop
            }
            pos = next;
        }
        Ok(chunk)
    });
    results.into_iter().collect()
}

/// Hashes every row of an already-materialised columnar file. There is nothing
/// to find — row `n` is row `n` — so this splits into equal ranges.
fn sweep_columnar(
    side: &Side,
    rows: usize,
    key_size: usize,
    opt: &Options,
    threads: usize,
) -> Result<Vec<Chunk>> {
    let parts = threads.clamp(1, rows.div_ceil(1 << 14).max(1));
    let results = in_parallel(parts, |i| {
        let lo = rows * i / parts;
        let hi = rows * (i + 1) / parts;
        let mut chunk = Chunk {
            at: Vec::with_capacity(hi - lo),
            hash: Vec::with_capacity(hi - lo),
        };
        let mut fields = vec![ABSENT; side.width];
        for row in lo..hi {
            side.fields_at(row as u64, &mut fields);
            chunk.at.push(row as u64);
            chunk
                .hash
                .push(key_hash(&side.slab, &fields, key_size, opt));
        }
        Ok(chunk)
    });
    results.into_iter().collect()
}

/// Where each chunk begins, as offsets of real row starts.
///
/// The nominal split is `size / threads`, walked forward to the next row. Walking
/// forward is the whole difficulty for CSV: a newline inside a quoted field is
/// not a row boundary, and a thread starting mid-file cannot tell whether it is
/// inside one. Parity settles it — every `"` toggles in-quote state, including
/// both halves of a doubled quote, which toggles twice and so correctly leaves
/// the state alone, so the count of quotes before a position says whether that
/// position is inside a field. Counting them is a scan for one byte, far cheaper
/// than parsing, and it splits across the same threads.
///
/// JSON needs none of that: a raw newline inside a string is not valid JSON, so
/// every newline ends a record.
fn chunk_bounds(data: &[u8], from: usize, threads: usize, dialect: Dialect) -> Vec<usize> {
    let end = data.len();
    if threads <= 1 || end - from < CHUNKING_THRESHOLD {
        return vec![from, end];
    }
    let nominal: Vec<usize> = (1..threads)
        .map(|i| from + (end - from) * i / threads)
        .collect();

    let quotes: Vec<usize> = if dialect == Dialect::Json {
        vec![0; nominal.len()]
    } else {
        in_parallel(nominal.len(), |i| {
            Ok(count_byte(data, from, nominal[i], b'"'))
        })
        .into_iter()
        .map(|r| r.unwrap_or(0))
        .collect()
    };

    let mut bounds = vec![from];
    for (i, &start) in nominal.iter().enumerate() {
        let mut in_quotes = quotes[i] % 2 == 1;
        let mut at = start;
        while at < end {
            match data[at] {
                b'"' if dialect == Dialect::Csv => in_quotes = !in_quotes,
                b'\n' if !in_quotes => {
                    at += 1;
                    break;
                }
                _ => {}
            }
            at += 1;
        }
        if at > *bounds.last().expect("a first bound") && at < end {
            bounds.push(at);
        }
    }
    bounds.push(end);
    bounds
}

// ---------------------------------------------------------------------------
// The join
// ---------------------------------------------------------------------------

/// A row chosen for a report section, with the row it matched on the other side.
#[derive(Clone, Copy)]
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

/// What one chunk of the queue produced: A's chunks carry counts and picks,
/// B's carry only the rows A never had.
enum Piece {
    A(Part),
    B(Capped),
}

/// One chunk of A's keys.
///
/// Each chunk keeps its own counts, column stats and capped lists; because the
/// chunks are contiguous and merged in chunk order, the result is identical to
/// one thread's, including which rows survive the cap.
struct Part {
    matched: i64,
    changed_per: Vec<i64>,
    blanked_per: Vec<i64>,
    filled_per: Vec<i64>,
    changed: Vec<Pick>,
    removed: Vec<Pick>,
    changed_total: i64,
    removed_total: i64,
}

/// Decodes one row's key and compared columns into the owned values the report
/// holds. This is the only place a cell becomes a `String`, and it runs at most
/// `--max-rows` times per section rather than once per row in the file.
fn row_values(side: &Side, idx: &RowIndex, row: i32, opt: &Options) -> Vec<Val> {
    let mut fields = vec![ABSENT; side.width];
    idx.fields_of(side, row, &mut fields);
    fields.iter().map(|f| value(&side.slab, *f, opt)).collect()
}

/// How many chunks `keys` divides into. One below the threshold, where finding
/// the boundaries would cost more than the join it splits.
fn ways_for(keys: usize) -> usize {
    if keys < JOIN_THRESHOLD {
        1
    } else {
        keys.div_ceil(JOIN_CHUNK).max(1)
    }
}

fn to_cells(values: &[Val]) -> Vec<Cell> {
    values.iter().map(|v| Cell::Value(v.clone())).collect()
}

#[allow(clippy::too_many_arguments)]
fn join(
    a: &Side,
    ai: &RowIndex,
    b: &Side,
    bi: &RowIndex,
    opt: &Options,
    compared: &[String],
    exporting: bool,
    threads: usize,
) -> Joined {
    let key_size = opt.key.len();
    let nc = compared.len();
    let width = key_size + nc;
    let cap = opt.max_rows;
    // Nothing to normalise: the cell comparison is two bit tests and a memcmp,
    // and neither the absence checks nor the value decoding are reachable.
    let plain = !needs_normalising(opt);

    // Both sides are chunked into one queue and every thread pulls from it.
    //
    // Giving B a thread of its own looked right -- A reads both rows and every
    // compared column where B only asks whether each of its keys exists in A --
    // but measuring it says otherwise: B's four million probes on one thread took
    // 3.1s while A's three ranges took 1.6s each, so the join was waiting on B
    // with three cores idle. A key costs B about two thirds of what it costs A,
    // and B has just as many of them; nothing about that ratio makes a thread the
    // right unit. Chunks do not care which side they came from.
    let a_keys = ai.first_row.len();
    let b_keys = bi.first_row.len();
    let a_ways = ways_for(a_keys);
    let b_ways = ways_for(b_keys);

    let a_range = |p: usize| -> Part {
        let mut out = Part {
            matched: 0,
            changed_per: vec![0; nc],
            blanked_per: vec![0; nc],
            filled_per: vec![0; nc],
            changed: Vec::new(),
            removed: Vec::new(),
            changed_total: 0,
            removed_total: 0,
        };
        let (mut fa, mut fb) = (vec![ABSENT; width], vec![ABSENT; width]);
        let lo = a_keys * p / a_ways;
        let hi = a_keys * (p + 1) / a_ways;
        let keys = &ai.first_row[lo..hi];
        for (i, &row) in keys.iter().enumerate() {
            if let Some(&soon) = keys.get(i + PREFETCH_AHEAD) {
                bi.prefetch(ai.row_hash[soon as usize]);
            }
            ai.fields_of(a, row, &mut fa);
            // The hash is the one the sweep computed for this row: the same
            // bytes through the same function, so computing it again here would
            // be a second pass over every key in the file for the same number.
            let hash = ai.row_hash[row as usize];
            // `fb` is the lookup's scratch, and on a hit it already holds the
            // mate's fields: that is what the key columns were matched against.
            let Some(mate) = bi.lookup(b, &a.slab, &fa, hash, key_size, opt, &mut fb) else {
                out.removed_total += 1;
                if exporting || out.removed.len() <= cap {
                    out.removed.push(Pick { row, mate: -1 });
                }
                continue;
            };
            out.matched += 1;

            let mut any = false;
            // Two loops rather than one with a flag inside it: `plain` cannot
            // change between columns or between rows, and this is the innermost
            // loop of the whole comparison.
            if plain {
                for i in 0..nc {
                    let (x, y) = (fa[key_size + i], fb[key_size + i]);
                    let (xa, yb) = (plain_absent(x), plain_absent(y));
                    if plain_differs(&a.slab, x, xa, &b.slab, y, yb) {
                        any = true;
                        out.changed_per[i] += 1;
                        // Absence is already known rather than asked for again.
                        if yb {
                            out.blanked_per[i] += 1;
                        }
                        if xa {
                            out.filled_per[i] += 1;
                        }
                    }
                }
            } else {
                for i in 0..nc {
                    let (x, y) = (fa[key_size + i], fb[key_size + i]);
                    if cell_differs(&a.slab, x, &b.slab, y, opt) {
                        any = true;
                        out.changed_per[i] += 1;
                        if is_absent(&b.slab, y, opt) {
                            out.blanked_per[i] += 1;
                        }
                        if is_absent(&a.slab, x, opt) {
                            out.filled_per[i] += 1;
                        }
                    }
                }
            }
            if any {
                out.changed_total += 1;
                if exporting || out.changed.len() <= cap {
                    out.changed.push(Pick { row, mate });
                }
            }
        }
        out
    };

    let b_range = |p: usize| -> Capped {
        let mut added = Capped::new(cap, exporting);
        let (mut fb, mut probe) = (vec![ABSENT; width], vec![ABSENT; width]);
        let lo = b_keys * p / b_ways;
        let hi = b_keys * (p + 1) / b_ways;
        let keys = &bi.first_row[lo..hi];
        for (i, &row) in keys.iter().enumerate() {
            if let Some(&soon) = keys.get(i + PREFETCH_AHEAD) {
                ai.prefetch(bi.row_hash[soon as usize]);
            }
            bi.fields_of(b, row, &mut fb);
            let hash = bi.row_hash[row as usize];
            if ai
                .lookup(a, &b.slab, &fb, hash, key_size, opt, &mut probe)
                .is_none()
            {
                added.push(Pick { row, mate: -1 });
            }
        }
        added
    };

    // Chunk `t` is A's chunk `t` while `t` is below `a_ways` and B's after that.
    // Threads take them in that order, so the merge below only has to sort the
    // pieces back into it.
    let next = AtomicUsize::new(0);
    let take_chunks = || -> Vec<(usize, Piece)> {
        let mut mine = Vec::new();
        loop {
            let t = next.fetch_add(1, Ordering::Relaxed);
            if t >= a_ways + b_ways {
                return mine;
            }
            mine.push(match t.checked_sub(a_ways) {
                None => (t, Piece::A(a_range(t))),
                Some(p) => (t, Piece::B(b_range(p))),
            });
        }
    };

    let mut phases = Phases::new("");
    let mut pieces = std::thread::scope(|scope| {
        let handles: Vec<_> = (1..threads.max(1))
            .map(|_| {
                let take_chunks = &take_chunks;
                scope.spawn(take_chunks)
            })
            .collect();
        let mut all = take_chunks();
        for handle in handles {
            all.extend(handle.join().expect("a join worker"));
        }
        all
    });
    pieces.sort_by_key(|(t, _)| *t);

    let mut parts: Vec<Part> = Vec::with_capacity(a_ways);
    let mut added = Capped::new(cap, exporting);
    for (_, piece) in pieces {
        match piece {
            Piece::A(part) => parts.push(part),
            Piece::B(chunk) => {
                for pick in &chunk.held {
                    added.push(*pick);
                }
                added.total += chunk.total - chunk.held.len() as i64;
            }
        }
    }

    phases.mark("  join chunks (par)");
    // Merged in chunk order, so the rows kept under the cap are the same rows
    // one thread would have kept.
    let mut changed = Capped::new(cap, exporting);
    let mut removed = Capped::new(cap, exporting);
    let mut matched = 0i64;
    let mut changed_per = vec![0i64; nc];
    let mut blanked_per = vec![0i64; nc];
    let mut filled_per = vec![0i64; nc];
    for part in &parts {
        matched += part.matched;
        for i in 0..nc {
            changed_per[i] += part.changed_per[i];
            blanked_per[i] += part.blanked_per[i];
            filled_per[i] += part.filled_per[i];
        }
        for pick in &part.changed {
            changed.push(*pick);
        }
        changed.total += part.changed_total - part.changed.len() as i64;
        for pick in &part.removed {
            removed.push(*pick);
        }
        removed.total += part.removed_total - part.removed.len() as i64;
    }

    let columns: Vec<ColumnStat> = (0..nc)
        .map(|i| ColumnStat {
            name: compared[i].clone(),
            changed: changed_per[i],
            blanked: blanked_per[i],
            filled: filled_per[i],
        })
        .collect();

    // Only now does anything become a String, and only for the rows kept -- but
    // "only" is up to two hundred thousand rows over twenty columns, which was
    // the last serial second of every run that renders a report.
    phases.mark("  merge parts (serial)");
    let mut removed_rows = map_rows(&removed.held, threads, |p| row_values(a, ai, p.row, opt));
    let mut added_rows = map_rows(&added.held, threads, |p| row_values(b, bi, p.row, opt));
    let mut changed_a = map_rows(&changed.held, threads, |p| row_values(a, ai, p.row, opt));
    let mut changed_b = map_rows(&changed.held, threads, |p| row_values(b, bi, p.mate, opt));

    phases.mark("  row values (par)");
    sort_rows(&mut removed_rows, key_size);
    sort_rows(&mut added_rows, key_size);
    sort_changed_together(&mut changed_a, &mut changed_b, key_size);

    // Zipped by index rather than by iterator so the two sides can be chunked
    // together; they are the same length and in the same order by construction.
    phases.mark("  sorts (serial)");
    let rows: Vec<usize> = (0..changed_a.len()).collect();
    let changed_cells: Vec<Vec<Cell>> = map_rows(&rows, threads, |&r| {
        let (ar, br) = (&changed_a[r], &changed_b[r]);
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
    });

    phases.mark("  changed cells (par)");
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
        added: map_rows(&added_rows, threads, |r| to_cells(r)),
        removed: map_rows(&removed_rows, threads, |r| to_cells(r)),
        changed_a,
        changed_b,
    }
}

fn sort_rows(rows: &mut [Vec<Val>], key_size: usize) {
    rows.sort_by(|x, y| compare_keys(x, y, key_size));
}

/// Sorts the changed rows by key while keeping the parallel A and B lists in step.
///
/// The permutation moves rows rather than copying them. Cloning instead meant
/// deep-copying every `String` in both lists -- fifty thousand rows over twenty
/// columns, twice -- which was almost the whole of this engine's serial sort
/// time, and none of it was the comparisons.
fn sort_changed_together(a: &mut Vec<Vec<Val>>, b: &mut Vec<Vec<Val>>, key_size: usize) {
    let mut order: Vec<usize> = (0..a.len()).collect();
    order.sort_by(|&p, &q| compare_keys(&a[p], &a[q], key_size));
    permute(a, &order);
    permute(b, &order);
}

/// Reorders `v` so that `v[i]` becomes what `v[order[i]]` was, moving each
/// element exactly once. `order` must be a permutation of `0..v.len()`.
fn permute<T: Default>(v: &mut Vec<T>, order: &[usize]) {
    let mut src = std::mem::take(v);
    *v = order.iter().map(|&i| std::mem::take(&mut src[i])).collect();
}

/// The duplicate-key section: most duplicated first, then by key.
fn duplicate_section(side: &Side, idx: &RowIndex, opt: &Options) -> Section {
    let key_size = opt.key.len();
    let mut entries: Vec<(Vec<Val>, i64)> = idx
        .first_row
        .iter()
        .zip(&idx.occurrences)
        .filter(|(_, n)| **n > 1)
        .map(|(row, n)| {
            let values = row_values(side, idx, *row, opt);
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
// Opening a file, whatever format it is in
// ---------------------------------------------------------------------------

/// A file after its header has been read but before the comparison knows which
/// columns it wants. The two phases are separate because a Parquet file should
/// decode the columns being compared and no others, and that list is not known
/// until both headers have been resolved against each other.
enum Input {
    Text {
        slab: Slab,
        delimiter: u8,
        from: usize,
        header: Vec<String>,
    },
    Parquet {
        reader: parquet::Reader,
        header: Vec<String>,
    },
}

impl Input {
    fn open(path: &Path, opt: &Options) -> Result<Input> {
        // The format is decided by what is in the file, not by its name: a file
        // called `.parquet` that is really a CSV is read as a CSV.
        let probe = Slab::map(path)?;
        if parquet::looks_like_parquet(probe.data()) {
            drop(probe);
            let reader = parquet::Reader::open(path)?;
            let header = reader.column_names();
            return Ok(Input::Parquet { reader, header });
        }
        // A file that begins with the magic and does not end with it is a
        // truncated Parquet file. Reading it as a CSV would report a missing key
        // column, which sends the reader looking in the wrong place entirely.
        if probe.data().starts_with(b"PAR1") {
            return Err(Error::new(format!(
                "{} begins with a Parquet magic number but does not end with one: \
                 the file is truncated",
                path.display()
            )));
        }
        let mut slab = probe;
        let dialect = sniff_dialect(slab.data());
        slab.set_dialect(dialect);
        if dialect == Dialect::Json {
            let header = json_header(&slab, path)?;
            return Ok(Input::Text {
                slab,
                delimiter: b',',
                from: 0,
                header,
            });
        }
        let delimiter = match opt.delimiter_byte()? {
            Some(d) => d,
            None => {
                let data = slab.data();
                detect_delimiter(&data[..next_of1(data, 0, data.len(), b'\n')])
            }
        };
        let (header, from) = csv_header(&slab, delimiter, path)?;
        Ok(Input::Text {
            slab,
            delimiter,
            from,
            header,
        })
    }

    fn header(&self) -> &[String] {
        match self {
            Input::Text { header, .. } | Input::Parquet { header, .. } => header,
        }
    }

    /// Reads the file into the join's representation, keeping only `wanted`.
    fn project(self, wanted: &[&String], threads: usize) -> Result<Side> {
        let width = wanted.len();
        match self {
            Input::Text {
                slab,
                delimiter,
                from,
                header,
            } => {
                let has = |n: &String| header.iter().any(|c| c == n);
                let parser = if slab.dialect() == Dialect::Json {
                    RowParser::json(
                        wanted
                            .iter()
                            .map(|n| has(n).then(|| (*n).clone()))
                            .collect(),
                    )
                } else {
                    RowParser::csv(
                        delimiter,
                        wanted
                            .iter()
                            .map(|n| header.iter().position(|c| &c == n))
                            .collect(),
                    )
                };
                Ok(Side {
                    slab,
                    rows: Rows::Text { parser, from },
                    width,
                })
            }
            Input::Parquet { reader, header } => {
                let names: Vec<Option<&str>> = wanted
                    .iter()
                    .map(|n| header.iter().any(|c| c == *n).then_some(n.as_str()))
                    .collect();
                let rows = reader.rows();
                let (fields, arena) = reader.project(&names, threads)?;
                // The mapping is finished with: everything the join reads now
                // lives in the arena, so the file's pages can be given back.
                drop(reader);
                Ok(Side {
                    slab: Slab::owned(arena, Dialect::Raw),
                    rows: Rows::Columnar { fields, rows },
                    width,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn compare(a_path: &Path, b_path: &Path, opt: &Options) -> Result<EngineResult> {
    let total = budget(opt);
    let a_input = Input::open(a_path, opt)?;
    let b_input = Input::open(b_path, opt)?;
    let resolved = resolve(a_input.header(), b_input.header(), opt)?;

    let key_size = opt.key.len();
    let a_cols = a_input.header().len();
    let b_cols = b_input.header().len();
    let wanted: Vec<&String> = opt.key.iter().chain(&resolved.compared).collect();

    // The two files share nothing until the join, so they are read at the same
    // time, and each is split further: two files across four cores is two chunks
    // each, so the whole machine is busy rather than half of it.
    let per_file = (total / 2).max(1);
    let prepare = |input: Input, tag: &'static str| -> Result<(Side, RowIndex)> {
        let side = input.project(&wanted, per_file)?;
        let index = RowIndex::build(&side, key_size, opt, per_file, tag)?;
        Ok((side, index))
    };
    let (from_a, from_b) = std::thread::scope(|scope| {
        let worker = scope.spawn(|| prepare(b_input, "B "));
        let mine = prepare(a_input, "A ");
        let theirs = worker
            .join()
            .unwrap_or_else(|_| Err(Error::new("a file reader panicked")));
        (mine, theirs)
    });
    let (a, ai) = from_a?;
    let (b, bi) = from_b?;
    let mut phases = Phases::new("");

    let dup_a = duplicate_section(&a, &ai, opt);
    let dup_b = duplicate_section(&b, &bi, opt);
    phases.mark("duplicate sections");
    let joined = join(
        &a,
        &ai,
        &b,
        &bi,
        opt,
        &resolved.compared,
        opt.export_dir.is_some(),
        total,
    );

    phases.mark("join");
    let meta = resolved.meta(&opt.key, a_cols, b_cols);
    let out = assemble(meta, joined, dup_a, dup_b, opt, &resolved.compared);
    phases.mark("assemble");
    out
}

/// The turbo engine has no optional dependency.
pub fn available() -> bool {
    true
}

/// Whether this file is in a format only this engine reads.
///
/// The other engines are CSV readers, so a Parquet or JSON input has exactly one
/// backend that can answer for it — and `--engine auto` should choose that one
/// rather than hand the file to DuckDB's CSV reader and report the parse error
/// it makes of a binary footer.
pub fn only_this_engine_reads(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::{Read, Seek, SeekFrom};
    let mut head = [0u8; 64];
    let read = file.read(&mut head).unwrap_or(0);
    if read >= 4 && &head[..4] == b"PAR1" {
        // The magic is at both ends, so a CSV that happens to start with it is
        // not mistaken for Parquet.
        let mut tail = [0u8; 4];
        if file.seek(SeekFrom::End(-4)).is_ok() && file.read_exact(&mut tail).is_ok() {
            return &tail == b"PAR1";
        }
    }
    sniff_dialect(&head[..read]) == Dialect::Json
}
