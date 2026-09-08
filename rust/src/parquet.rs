//! A Parquet reader shaped for comparing, not for querying.
//!
//! This is the Rust twin of `cpp/src/parquet.cpp`, and it makes the same
//! bargain: it reads exactly what this job needs and refuses the rest by name.
//! `BYTE_ARRAY` columns, PLAIN and dictionary encodings, uncompressed and
//! snappy, v1 data pages. That covers what DuckDB, polars and this project's
//! own generator write for string columns, and it is the shape the comparison
//! actually meets.
//!
//! Two things it does that a general reader would not.
//!
//! It hands back *offsets into the mapped file* rather than strings. A PLAIN
//! byte array is a four-byte length followed by its bytes, already contiguous
//! and already in the mapping, so a value can stay an offset and a length --
//! the same representation the CSV and JSON paths use, and the reason nothing
//! here builds a string per cell.
//!
//! And it keeps dictionary columns encoded. Where a column is dictionary
//! encoded the rows are indices into a small table of distinct values, so two
//! files can be compared by mapping one dictionary onto the other once and then
//! comparing integers -- which is what makes the per-column diff a vector
//! operation instead of a string compare per row.

use crate::error::{Error, Result};

fn fail<T>(what: &str, path: &str) -> Result<T> {
    Err(Error::new(format!("{what}: {path}")))
}

// parquet.thrift, the handful of values this reader meets.
const TYPE_BYTE_ARRAY: i64 = 6;
const ENC_PLAIN: i64 = 0;
const ENC_PLAIN_DICTIONARY: i64 = 2;
const ENC_RLE: i64 = 3;
const ENC_RLE_DICTIONARY: i64 = 8;
const CODEC_UNCOMPRESSED: i64 = 0;
const CODEC_SNAPPY: i64 = 1;
const PAGE_DATA: i64 = 0;
const PAGE_INDEX: i64 = 1;
const PAGE_DICTIONARY: i64 = 2;
const PAGE_DATA_V2: i64 = 3;

// Thrift compact protocol field types.
const T_STOP: u8 = 0;
const T_TRUE: u8 = 1;
const T_FALSE: u8 = 2;
const T_BYTE: u8 = 3;
const T_I16: u8 = 4;
const T_I32: u8 = 5;
const T_I64: u8 = 6;
const T_DOUBLE: u8 = 7;
const T_BINARY: u8 = 8;
const T_LIST: u8 = 9;
const T_SET: u8 = 10;
const T_MAP: u8 = 11;
const T_STRUCT: u8 = 12;

/// A value's bytes, as an offset and a length packed into one word: forty bits
/// of offset, twenty-three of length, and one to say the value is null.
///
/// That is the same layout the CSV engine packs a field into, for the same
/// reason -- eight bytes a value rather than sixteen is eighty megabytes less to
/// carry, and eighty megabytes less to stream past the CPU, on a ten-million-row
/// column. Forty bits of offset is a terabyte; twenty-three of length is eight
/// megabytes, and a longer value is refused by name rather than truncated.
///
/// The offset is into the mapped file where the page was stored uncompressed,
/// and into the reader's own buffer where it had to be decompressed --
/// [`Column::owned`] says which.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Slice(u64);

impl Slice {
    pub const MAX_LENGTH: u32 = (1 << 23) - 1;
    const OFFSET_MASK: u64 = (1 << 40) - 1;
    const LENGTH_SHIFT: u32 = 40;
    const NULL_BIT: u64 = 1 << 63;

    pub fn at(offset: u64, length: u32) -> Self {
        Slice((offset & Self::OFFSET_MASK) | ((length as u64) << Self::LENGTH_SHIFT))
    }
    pub fn null() -> Self {
        Slice(Self::NULL_BIT)
    }
    pub fn is_null(self) -> bool {
        self.0 & Self::NULL_BIT != 0
    }
    pub fn offset(self) -> usize {
        (self.0 & Self::OFFSET_MASK) as usize
    }
    pub fn length(self) -> usize {
        ((self.0 >> Self::LENGTH_SHIFT) & Self::MAX_LENGTH as u64) as usize
    }
}

impl Default for Slice {
    fn default() -> Self {
        Slice::null()
    }
}

/// One column of one file, decoded as far as it is useful to decode it.
///
/// A dictionary column keeps `dict` and `index`: `index[row]` selects a value,
/// or is [`Column::NULL`]. A plain column keeps `values`, one slice per row. The
/// comparison reads whichever is populated, and the dictionary form is the one
/// worth having.
#[derive(Default)]
pub struct Column {
    pub dictionary: bool,
    /// Dictionary form: the distinct values.
    pub dict: Vec<Slice>,
    /// Dictionary form: one per row.
    pub index: Vec<i32>,
    /// Plain form: one per row.
    pub values: Vec<Slice>,
    /// Decompressed pages, when the column was compressed. A column is either
    /// wholly compressed or wholly not -- the reader refuses anything else --
    /// so this one field says where every slice in the column points: into
    /// `owned` when it is non-empty, into the mapping when it is not.
    pub owned: Vec<u8>,
}

impl Column {
    pub const NULL: i32 = -1;

    pub fn rows(&self) -> usize {
        if self.dictionary {
            self.index.len()
        } else {
            self.values.len()
        }
    }
}

/// What a file says about itself, before any column is read.
#[derive(Default)]
pub struct Meta {
    /// Leaf columns, in file order.
    pub names: Vec<String>,
    pub rows: i64,
    pub row_groups: usize,
}

// ---------------------------------------------------------------------------
// Thrift compact protocol
// ---------------------------------------------------------------------------

struct Thrift<'a> {
    d: &'a [u8],
    at: usize,
    path: &'a str,
}

impl<'a> Thrift<'a> {
    fn new(d: &'a [u8], path: &'a str) -> Self {
        Thrift { d, at: 0, path }
    }

    fn byte(&mut self) -> Result<u8> {
        if self.at >= self.d.len() {
            return fail("parquet metadata ends early", self.path);
        }
        let b = self.d[self.at];
        self.at += 1;
        Ok(b)
    }

    fn varint(&mut self) -> Result<u64> {
        let mut out = 0u64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            out |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
            if shift > 63 {
                return fail("a varint in the metadata is malformed", self.path);
            }
        }
    }

    fn zigzag(&mut self) -> Result<i64> {
        let v = self.varint()?;
        Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
    }

    fn binary(&mut self) -> Result<&'a [u8]> {
        let len = self.varint()? as usize;
        if self.at + len > self.d.len() {
            return fail("a string in the metadata runs past the end", self.path);
        }
        let out = &self.d[self.at..self.at + len];
        self.at += len;
        Ok(out)
    }

    /// Reads a field header. Returns the type, or `T_STOP` at the end of a
    /// struct; `id` is set to the field number.
    fn field(&mut self, id: &mut i16, last: &mut i16) -> Result<u8> {
        let h = self.byte()?;
        if h == 0 {
            return Ok(T_STOP);
        }
        let ty = h & 0x0F;
        let delta = (h & 0xF0) >> 4;
        *id = if delta == 0 {
            self.zigzag()? as i16
        } else {
            *last + delta as i16
        };
        *last = *id;
        Ok(ty)
    }

    /// Reads a list header, returning the element type and the count.
    fn list(&mut self) -> Result<(u8, u32)> {
        let h = self.byte()?;
        let mut count = ((h & 0xF0) >> 4) as u32;
        if count == 15 {
            count = self.varint()? as u32;
        }
        Ok((h & 0x0F, count))
    }

    /// Steps over a value without interpreting it, so a struct can be read for
    /// the few fields that matter and the rest skipped.
    fn skip(&mut self, ty: u8) -> Result<()> {
        match ty {
            T_TRUE | T_FALSE => {}
            T_BYTE => {
                self.byte()?;
            }
            T_I16 | T_I32 | T_I64 => {
                self.zigzag()?;
            }
            T_DOUBLE => {
                for _ in 0..8 {
                    self.byte()?;
                }
            }
            T_BINARY => {
                self.binary()?;
            }
            T_LIST | T_SET => {
                let (elem, n) = self.list()?;
                for _ in 0..n {
                    self.skip(elem)?;
                }
            }
            T_MAP => {
                let n = self.varint()?;
                if n > 0 {
                    let kinds = self.byte()?;
                    for _ in 0..n {
                        self.skip(kinds >> 4)?;
                        self.skip(kinds & 0x0F)?;
                    }
                }
            }
            T_STRUCT => {
                let (mut id, mut last) = (0i16, 0i16);
                loop {
                    let t = self.field(&mut id, &mut last)?;
                    if t == T_STOP {
                        break;
                    }
                    self.skip(t)?;
                }
            }
            _ => return fail("unknown type in the parquet metadata", self.path),
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The footer
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct ChunkMeta {
    ty: i64,
    codec: i64,
    num_values: i64,
    data_page_offset: i64,
    dictionary_page_offset: i64,
    total_compressed_size: i64,
    total_uncompressed_size: i64,
}

#[derive(Default, Clone)]
struct RowGroupMeta {
    columns: Vec<ChunkMeta>,
}

#[derive(Default)]
struct FileMeta {
    names: Vec<String>,
    /// 1 where the column may be null.
    optional: Vec<bool>,
    groups: Vec<RowGroupMeta>,
    rows: i64,
}

fn read_column_meta(t: &mut Thrift, out: &mut ChunkMeta) -> Result<()> {
    let (mut id, mut last) = (0i16, 0i16);
    loop {
        let ty = t.field(&mut id, &mut last)?;
        if ty == T_STOP {
            return Ok(());
        }
        match id {
            1 => out.ty = t.zigzag()?,
            4 => out.codec = t.zigzag()?,
            5 => out.num_values = t.zigzag()?,
            6 => out.total_uncompressed_size = t.zigzag()?,
            7 => out.total_compressed_size = t.zigzag()?,
            9 => out.data_page_offset = t.zigzag()?,
            11 => out.dictionary_page_offset = t.zigzag()?,
            _ => t.skip(ty)?,
        }
    }
}

fn read_chunk(t: &mut Thrift, out: &mut ChunkMeta) -> Result<()> {
    let (mut id, mut last) = (0i16, 0i16);
    loop {
        let ty = t.field(&mut id, &mut last)?;
        if ty == T_STOP {
            return Ok(());
        }
        if id == 3 {
            read_column_meta(t, out)?;
        } else {
            t.skip(ty)?;
        }
    }
}

fn read_row_group(t: &mut Thrift, out: &mut RowGroupMeta) -> Result<()> {
    let (mut id, mut last) = (0i16, 0i16);
    loop {
        let ty = t.field(&mut id, &mut last)?;
        if ty == T_STOP {
            return Ok(());
        }
        if id == 1 {
            let (_, n) = t.list()?;
            out.columns = vec![ChunkMeta::default(); n as usize];
            for c in out.columns.iter_mut() {
                read_chunk(t, c)?;
            }
        } else {
            t.skip(ty)?;
        }
    }
}

fn read_file_meta(data: &[u8], path: &str) -> Result<FileMeta> {
    if data.len() < 12 || &data[..4] != b"PAR1" || &data[data.len() - 4..] != b"PAR1" {
        return fail("not a parquet file", path);
    }
    let meta_len =
        u32::from_le_bytes(data[data.len() - 8..data.len() - 4].try_into().unwrap()) as usize;
    if meta_len + 8 > data.len() {
        return fail("the parquet footer is longer than the file", path);
    }
    let start = data.len() - 8 - meta_len;
    let mut t = Thrift::new(&data[start..start + meta_len], path);

    let mut out = FileMeta::default();
    let (mut id, mut last) = (0i16, 0i16);
    loop {
        let ty = t.field(&mut id, &mut last)?;
        if ty == T_STOP {
            break;
        }
        match id {
            2 => {
                let (_, n) = t.list()?;
                for i in 0..n {
                    // SchemaElement: repetition_type is 3, name 4, num_children 5.
                    let (mut sid, mut slast) = (0i16, 0i16);
                    let mut name = String::new();
                    let (mut repetition, mut children) = (-1i64, 0i64);
                    loop {
                        let st = t.field(&mut sid, &mut slast)?;
                        if st == T_STOP {
                            break;
                        }
                        match sid {
                            3 => repetition = t.zigzag()?,
                            4 => name = String::from_utf8_lossy(t.binary()?).into_owned(),
                            5 => children = t.zigzag()?,
                            _ => t.skip(st)?,
                        }
                    }
                    // The first element is the root, and anything with children
                    // is a group rather than a column this reader can read.
                    if i == 0 {
                        continue;
                    }
                    if children > 0 {
                        return fail("nested parquet columns are not read here", path);
                    }
                    out.names.push(name);
                    out.optional.push(repetition == 1);
                }
            }
            3 => out.rows = t.zigzag()?,
            4 => {
                let (_, n) = t.list()?;
                out.groups = vec![RowGroupMeta::default(); n as usize];
                for g in out.groups.iter_mut() {
                    read_row_group(&mut t, g)?;
                }
            }
            _ => t.skip(ty)?,
        }
    }
    if out.names.is_empty() {
        return fail("the parquet schema has no columns", path);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PageHead {
    ty: i64,
    compressed: i64,
    num_values: i32,
    encoding: i64,
    def_encoding: i64,
    /// Offset of the page body.
    after: usize,
}

fn read_page_head(data: &[u8], at: usize, path: &str) -> Result<PageHead> {
    let mut t = Thrift::new(data, path);
    t.at = at;
    let mut out = PageHead::default();
    let (mut id, mut last) = (0i16, 0i16);
    loop {
        let ty = t.field(&mut id, &mut last)?;
        if ty == T_STOP {
            break;
        }
        match id {
            1 => out.ty = t.zigzag()?,
            3 => out.compressed = t.zigzag()?,
            5 | 7 => {
                let (mut hid, mut hlast) = (0i16, 0i16);
                loop {
                    let ht = t.field(&mut hid, &mut hlast)?;
                    if ht == T_STOP {
                        break;
                    }
                    match hid {
                        1 => out.num_values = t.zigzag()? as i32,
                        2 => out.encoding = t.zigzag()?,
                        3 if id == 5 => out.def_encoding = t.zigzag()?,
                        _ => t.skip(ht)?,
                    }
                }
            }
            8 => return fail("parquet data page v2 is not read here", path),
            _ => t.skip(ty)?,
        }
    }
    out.after = t.at;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Snappy
// ---------------------------------------------------------------------------

/// Snappy raw block format: a varint of the uncompressed length, then a stream
/// of literal and copy elements.
///
/// It appends to `out` and writes through indices rather than pushing a byte at
/// a time, and a back-reference is copied eight bytes at a time where the
/// distance allows it. Both matter more than they look: this is the one loop
/// that touches every byte of a compressed file.
fn snappy_append(input: &[u8], out: &mut Vec<u8>) -> bool {
    let mut at = 0usize;
    let mut want = 0u64;
    let mut shift = 0;
    loop {
        if at >= input.len() {
            return false;
        }
        let b = input[at];
        at += 1;
        want |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 63 {
            return false;
        }
    }

    let base = out.len();
    let want = want as usize;
    out.resize(base + want, 0);
    let mut dst = base;
    let end = base + want;

    while at < input.len() {
        let tag = input[at];
        at += 1;
        if tag & 0x03 == 0 {
            // literal
            let mut len = (tag >> 2) as usize + 1;
            if len > 60 {
                let extra = len - 60;
                if at + extra > input.len() {
                    return false;
                }
                let mut v = 0usize;
                for i in 0..extra {
                    v |= (input[at + i] as usize) << (8 * i);
                }
                at += extra;
                len = v + 1;
            }
            if at + len > input.len() || len > end - dst {
                return false;
            }
            out[dst..dst + len].copy_from_slice(&input[at..at + len]);
            dst += len;
            at += len;
            continue;
        }
        let (len, offset) = match tag & 0x03 {
            1 => {
                if at >= input.len() {
                    return false;
                }
                let o = (((tag >> 5) as usize) << 8) | input[at] as usize;
                at += 1;
                (((tag >> 2) & 0x07) as usize + 4, o)
            }
            2 => {
                if at + 2 > input.len() {
                    return false;
                }
                let o = input[at] as usize | ((input[at + 1] as usize) << 8);
                at += 2;
                ((tag >> 2) as usize + 1, o)
            }
            _ => {
                if at + 4 > input.len() {
                    return false;
                }
                let mut o = 0usize;
                for i in 0..4 {
                    o |= (input[at + i] as usize) << (8 * i);
                }
                at += 4;
                ((tag >> 2) as usize + 1, o)
            }
        };
        if offset == 0 || offset > dst - base || len > end - dst {
            return false;
        }
        let from = dst - offset;
        if offset >= 8 {
            // The source of the next eight bytes is entirely behind the
            // destination, so whole words can be moved.
            let mut i = 0;
            while i + 8 <= len {
                out.copy_within(from + i..from + i + 8, dst + i);
                i += 8;
            }
            while i < len {
                out[dst + i] = out[from + i];
                i += 1;
            }
        } else {
            // A short distance means the copy repeats a pattern it is still
            // writing, so it has to go a byte at a time.
            for i in 0..len {
                out[dst + i] = out[from + i];
            }
        }
        dst += len;
    }
    dst == end
}

// ---------------------------------------------------------------------------
// RLE / bit-packed hybrid
// ---------------------------------------------------------------------------

struct RleReader<'a> {
    d: &'a [u8],
    width: u32,
    mask: u64,
    at: usize,
    left: usize,
    bit: usize,
    packed: bool,
    value: i32,
}

impl<'a> RleReader<'a> {
    fn new(d: &'a [u8], width: u32) -> Self {
        RleReader {
            d,
            width,
            mask: if width >= 64 { !0 } else { (1u64 << width) - 1 },
            at: 0,
            left: 0,
            bit: 0,
            packed: false,
            value: 0,
        }
    }

    fn header(&mut self) -> bool {
        let mut h = 0u64;
        let mut shift = 0;
        loop {
            if self.at >= self.d.len() {
                return false;
            }
            let b = self.d[self.at];
            self.at += 1;
            h |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 63 {
                return false;
            }
        }
        if h & 1 == 0 {
            // RLE run: a count and one value.
            self.packed = false;
            self.left = (h >> 1) as usize;
            let bytes = ((self.width + 7) / 8) as usize;
            if self.at + bytes > self.d.len() {
                return false;
            }
            self.value = 0;
            for i in 0..bytes {
                self.value |= (self.d[self.at + i] as i32) << (8 * i);
            }
            self.at += bytes;
        } else {
            // Bit-packed run, in groups of eight. A group is exactly `width`
            // bytes, so the whole run's length is known and `at` can jump
            // straight to the next header while `bit` walks inside it.
            self.packed = true;
            let groups = (h >> 1) as usize;
            self.left = groups * 8;
            self.bit = self.at * 8;
            let run = groups * self.width as usize;
            self.at = (self.at + run).min(self.d.len());
        }
        self.left > 0
    }

    /// Fills `out`. Bulk rather than one at a time on purpose: an RLE run
    /// becomes a fill, and a bit-packed run becomes one 64-bit load, one shift
    /// and one mask per value.
    ///
    /// Values are packed end to end with no padding, so a value straddles bytes
    /// more often than not. Reading eight bytes around it and shifting picks
    /// any of them out without a loop -- the same "look at eight bytes at once"
    /// trick the CSV scanner uses to find a delimiter, here reading rather than
    /// searching. It is exact for every width Parquet allows: seven bits of
    /// misalignment plus thirty-two of value still fits in a word.
    fn fill(&mut self, out: &mut [i32]) -> bool {
        let want = out.len();
        let mut done = 0usize;
        while done < want {
            if self.left == 0 && !self.header() {
                return false;
            }
            let take = (want - done).min(self.left);
            if !self.packed {
                out[done..done + take].fill(self.value);
            } else if self.width == 0 {
                out[done..done + take].fill(0);
            } else {
                for i in 0..take {
                    let byte = self.bit >> 3;
                    let mut w = 0u64;
                    if byte + 8 <= self.d.len() {
                        w = u64::from_le_bytes(self.d[byte..byte + 8].try_into().unwrap());
                    } else {
                        for k in 0..8 {
                            if byte + k < self.d.len() {
                                w |= (self.d[byte + k] as u64) << (8 * k);
                            }
                        }
                    }
                    out[done + i] = ((w >> (self.bit & 7)) & self.mask) as i32;
                    self.bit += self.width as usize;
                }
            }
            self.left -= take;
            done += take;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// PLAIN byte arrays: a four-byte little-endian length, then that many bytes,
/// repeated. The slices point straight at `base`, so nothing is copied.
fn plain_slices(page: &[u8], base: u64, count: i32, out: &mut Vec<Slice>, path: &str) -> Result<()> {
    let mut at = 0usize;
    for _ in 0..count {
        if at + 4 > page.len() {
            return fail("a parquet page ends inside a value length", path);
        }
        let len = u32::from_le_bytes(page[at..at + 4].try_into().unwrap());
        at += 4;
        if at + len as usize > page.len() {
            return fail("a parquet value runs past its page", path);
        }
        if len > Slice::MAX_LENGTH {
            return fail("a parquet value is larger than eight megabytes", path);
        }
        out.push(Slice::at(base + at as u64, len));
        at += len as usize;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The two entry points
// ---------------------------------------------------------------------------

/// Reads the footer. Errors name the problem if the file is not Parquet or uses
/// something this reader does not implement.
pub fn read_meta(data: &[u8], path: &str) -> Result<Meta> {
    let fm = read_file_meta(data, path)?;
    Ok(Meta {
        names: fm.names,
        rows: fm.rows,
        row_groups: fm.groups.len(),
    })
}

/// Reads one column across every row group. `which` indexes into [`Meta::names`].
pub fn read_column(data: &[u8], which: usize, path: &str) -> Result<Column> {
    let fm = read_file_meta(data, path)?;
    if which >= fm.names.len() {
        return fail("no such column in the parquet file", path);
    }
    let optional = fm.optional[which];

    // The column's parts are held as locals rather than in a `Column`, because
    // a page is read *out of* `owned` while its values are pushed *into* `dict`
    // or `values`, and those are only disjoint borrows if they are separate
    // bindings. They become a `Column` at the end.
    let mut owned: Vec<u8> = Vec::new();
    let mut dict: Vec<Slice> = Vec::new();
    let mut index: Vec<i32> = Vec::new();
    let mut values: Vec<Slice> = Vec::new();

    let mut codec: i64 = -1;
    let mut uncompressed = 0i64;
    for rg in &fm.groups {
        if which >= rg.columns.len() {
            return fail("a row group is missing a column", path);
        }
        uncompressed += rg.columns[which].total_uncompressed_size;
    }

    // Reused across pages so the per-page work allocates nothing, and sized
    // once from the footer's row count so appending a page never has to move
    // eighty megabytes of what is already decoded.
    let mut defs: Vec<i32> = Vec::new();
    let mut idx: Vec<i32> = Vec::new();
    let mut got: Vec<Slice> = Vec::new();
    index.reserve(fm.rows as usize);

    // The column starts out held as dictionary indices and stays that way only
    // if every page cooperates. A writer that gives up on the dictionary
    // partway -- DuckDB does, once a column's distinct values outgrow its
    // budget, which at ten million rows is most of them -- forces the whole
    // column into the plain form. Expanding what has been read costs no bytes:
    // a dictionary entry is already a slice.
    let mut dictionary = true;

    for rg in &fm.groups {
        let c = &rg.columns[which];
        if c.ty != TYPE_BYTE_ARRAY {
            return fail("only BYTE_ARRAY parquet columns are read here", path);
        }
        if c.codec != CODEC_UNCOMPRESSED && c.codec != CODEC_SNAPPY {
            return fail("only uncompressed and snappy parquet are read here", path);
        }
        // Every chunk has to agree about compression, because a slice is an
        // offset with no room to say what it is an offset *into*. Uncompressed
        // pages are read in place and their offsets are into the mapping;
        // compressed ones are expanded into `owned` and their offsets are into
        // that. One column, one base.
        if codec < 0 {
            codec = c.codec;
            if codec == CODEC_SNAPPY {
                owned.reserve(uncompressed as usize);
            }
        } else if codec != c.codec {
            return fail("a parquet column changes compression between row groups", path);
        }

        let mut at = if c.dictionary_page_offset > 0 {
            c.dictionary_page_offset as usize
        } else {
            c.data_page_offset as usize
        };
        let stop = at + c.total_compressed_size as usize;
        let mut seen = 0i64;
        let dict_base = dict.len();

        while at < stop && at < data.len() && seen < c.num_values {
            let h = read_page_head(data, at, path)?;
            let body_len = h.compressed as usize;
            if h.after + body_len > data.len() {
                return fail("a parquet page runs past the file", path);
            }

            // Where the page's bytes live, and what a slice into them counts
            // from. `in_owned` is the whole of the distinction.
            let (page_from, page_len, page_base, in_owned) = if c.codec == CODEC_SNAPPY {
                let was = owned.len();
                if !snappy_append(&data[h.after..h.after + body_len], &mut owned) {
                    return fail("a snappy page in the parquet file will not decompress", path);
                }
                (was, owned.len() - was, was as u64, true)
            } else {
                (h.after, body_len, h.after as u64, false)
            };
            let body: &[u8] = if in_owned {
                &owned[page_from..page_from + page_len]
            } else {
                &data[page_from..page_from + page_len]
            };

            if h.ty == PAGE_DICTIONARY {
                if h.encoding != ENC_PLAIN && h.encoding != ENC_PLAIN_DICTIONARY {
                    return fail("only PLAIN parquet dictionaries are read here", path);
                }
                plain_slices(body, page_base, h.num_values, &mut dict, path)?;
            } else if h.ty == PAGE_DATA {
                let n_vals = h.num_values as usize;
                let mut vat = 0usize;
                let mut real = n_vals;
                if optional {
                    // Definition levels: RLE, four-byte length prefix in v1.
                    if h.def_encoding != ENC_RLE {
                        return fail("parquet definition levels are not RLE", path);
                    }
                    if page_len < 4 {
                        return fail("a parquet page has no definition levels", path);
                    }
                    let dl = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
                    if 4 + dl > page_len {
                        return fail("a parquet page ends inside its levels", path);
                    }
                    defs.clear();
                    defs.resize(n_vals, 0);
                    if !RleReader::new(&body[4..4 + dl], 1).fill(&mut defs) {
                        return fail("a parquet page ran out of definition levels", path);
                    }
                    real = defs.iter().filter(|&&d| d != 0).count();
                    vat = 4 + dl;
                }
                if vat > page_len {
                    return fail("a parquet page ends inside its levels", path);
                }
                let here = |i: usize| !optional || defs[i] != 0;

                if h.encoding == ENC_PLAIN_DICTIONARY || h.encoding == ENC_RLE_DICTIONARY {
                    if dict.is_empty() {
                        return fail("a parquet data page wants a dictionary there is none of", path);
                    }
                    if vat + 1 > page_len {
                        return fail("a parquet page has no bit width", path);
                    }
                    let width = body[vat] as u32;
                    if width > 32 {
                        return fail("a parquet dictionary index is wider than 32 bits", path);
                    }
                    idx.clear();
                    idx.resize(real, 0);
                    if !RleReader::new(&body[vat + 1..], width).fill(&mut idx) {
                        return fail("a parquet page ran out of dictionary indices", path);
                    }
                    for v in idx.iter_mut() {
                        let k = dict_base + *v as usize;
                        if k >= dict.len() {
                            return fail("a parquet dictionary index is out of range", path);
                        }
                        *v = k as i32;
                    }
                    let mut k = 0usize;
                    for i in 0..n_vals {
                        if !here(i) {
                            if dictionary {
                                index.push(Column::NULL);
                            } else {
                                values.push(Slice::null());
                            }
                            continue;
                        }
                        let v = idx[k];
                        k += 1;
                        if dictionary {
                            index.push(v);
                        } else {
                            values.push(dict[v as usize]);
                        }
                    }
                } else if h.encoding == ENC_PLAIN {
                    if dictionary {
                        // Fold what has been read into the plain form. This
                        // copies eight-byte handles, not values.
                        values.reserve(fm.rows as usize);
                        for &k in &index {
                            values.push(if k >= 0 { dict[k as usize] } else { Slice::null() });
                        }
                        index = Vec::new();
                        dictionary = false;
                    }
                    got.clear();
                    got.reserve(real);
                    plain_slices(
                        &body[vat..],
                        page_base + vat as u64,
                        real as i32,
                        &mut got,
                        path,
                    )?;
                    let mut k = 0usize;
                    for i in 0..n_vals {
                        values.push(if here(i) {
                            let v = got[k];
                            k += 1;
                            v
                        } else {
                            Slice::null()
                        });
                    }
                } else {
                    return fail("only PLAIN and dictionary parquet encodings are read here", path);
                }
                seen += h.num_values as i64;
            } else if h.ty == PAGE_DATA_V2 {
                return fail("parquet data page v2 is not read here", path);
            } else if h.ty != PAGE_INDEX {
                return fail("unknown parquet page type", path);
            }
            at = h.after + body_len;
        }
    }

    // A column that never met a page at all is plain and empty, not dictionary.
    if dictionary && index.is_empty() && dict.is_empty() {
        dictionary = false;
    }
    Ok(Column {
        dictionary,
        dict,
        index,
        values,
        owned,
    })
}

/// True when the file begins with Parquet's `PAR1` magic. Cheap: four bytes.
pub fn is_parquet(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    match std::fs::File::open(path) {
        Ok(mut f) => f.read_exact(&mut magic).is_ok() && &magic == b"PAR1",
        Err(_) => false,
    }
}
