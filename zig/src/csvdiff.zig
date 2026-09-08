//! A composite-key comparison, byte-level, with the memory it may use passed in
//! rather than assumed.
//!
//! The design is the one the Java, Rust and C++ ports share: the file is mapped,
//! a field is an offset and a length packed into one word, delimiters are found
//! eight bytes at a time, and nothing becomes a string unless it reaches the
//! report.
//!
//! What Zig adds is the reason this port exists. Every allocation goes through an
//! allocator the caller supplies, so `--max-memory` is not a target the engine
//! tries to respect — it is a `FixedBufferAllocator` that cannot hand out more
//! than it was given. A comparison that would exceed the budget fails with
//! `error.OutOfMemory` at the allocation that would have crossed it, rather than
//! growing until the kernel intervenes. "Bounded memory" stops being a
//! measurement and becomes a property the type system enforces. That still holds
//! for the two things added since: reading Parquet allocates its arena from the
//! same allocator, and every thread takes its chunk from it too.
//!
//! Three input formats — CSV, newline-delimited JSON and Parquet — all reduce to
//! the same field word, so the two sides of a comparison need not be in the same
//! format. See `text.zig` and `parquet.zig`.

const std = @import("std");
const scan = @import("scan.zig");
const fld = @import("field.zig");
const slab_mod = @import("slab.zig");
const text = @import("text.zig");
const parquet = @import("pqread.zig");
const codec = @import("codec.zig");
const encoding = @import("encoding.zig");

const Field = fld.Field;
const ABSENT = fld.ABSENT;
const TOO_LONG = fld.TOO_LONG;
// Re-exported: `pqdiff.zig` maps its two files the same way this engine does.
pub const Slab = slab_mod.Slab;
const Dialect = slab_mod.Dialect;

pub const Error = error{
    KeyColumnMissing,
    ComparedColumnMissing,
    NoHeaderRow,
    FieldTooLong,
    CannotReadFile,
    NonAsciiCaseFold,
};

/// Below this there is nothing to divide: finding the chunk boundaries would cost
/// more than the parsing it splits.
const CHUNKING_THRESHOLD: usize = 4 << 20;

/// How many keys make the join worth splitting.
const JOIN_THRESHOLD: usize = 1 << 14;

pub const Options = struct {
    key: []const []const u8,
    compare: []const []const u8 = &.{},
    ignore: []const []const u8 = &.{},
    trim: bool = false,
    ignore_case: bool = false,
    empty_is_null: bool = false,
    tolerance: f64 = 0,
    max_rows: usize = 50_000,
    delimiter: ?u8 = null,
    /// How many threads the whole comparison may use. Zero means as many as the
    /// machine has. Both files are read at once and each is split further, so
    /// this is the width of the run rather than of one file.
    threads: usize = 0,
};

pub const Counts = struct {
    a_rows: i64 = 0,
    b_rows: i64 = 0,
    a_keys: i64 = 0,
    b_keys: i64 = 0,
    matched: i64 = 0,
    unchanged: i64 = 0,
    changed: i64 = 0,
    added: i64 = 0,
    removed: i64 = 0,
    a_dup_keys: i64 = 0,
    a_dup_rows: i64 = 0,
    b_dup_keys: i64 = 0,
    b_dup_rows: i64 = 0,
};

pub const ColumnStat = struct {
    name: []const u8,
    changed: i64 = 0,
    blanked: i64 = 0,
    filled: i64 = 0,
};

/// What went wrong, in words, for any error this engine or its readers can
/// return. A switch here rather than in the command line, because the readers
/// each have their own error set and the caller should not have to know them.
pub fn message(err: anyerror) []const u8 {
    return switch (err) {
        error.OutOfMemory => "out of memory",
        Error.KeyColumnMissing => "key column(s) missing from one of the files",
        Error.ComparedColumnMissing => "compared column missing from one of the files",
        Error.NoHeaderRow => "file has no header row",
        Error.FieldTooLong => "a field is larger than this engine packs",
        Error.CannotReadFile => "cannot read one of the files",
        Error.NonAsciiCaseFold => "--ignore-case outside ASCII needs Unicode case folding, which " ++
            "this port does not carry; use another implementation for that data",
        text.Error.NoJsonObject => "the JSON file has no object to read the column names from",
        parquet.Error.NotParquet => "that file does not begin and end with a Parquet magic number",
        parquet.Error.BadFooter => "the Parquet footer is longer than the file",
        parquet.Error.TruncatedParquetFile => "that file begins with a Parquet magic number but " ++
            "does not end with one: it is truncated",
        parquet.Error.NestedSchema => "this Parquet file nests a group; a comparison needs a flat " ++
            "table of cells",
        parquet.Error.RepeatedColumn => "this Parquet file repeats a column; a comparison needs " ++
            "one value per row",
        parquet.Error.NoColumns => "this Parquet file has no columns",
        parquet.Error.RowCountMismatch => "this Parquet file's row groups do not hold the number " ++
            "of rows its footer claims",
        parquet.Error.UnsupportedEncoding => "this Parquet file uses an encoding this reader does " ++
            "not decode; rewrite it with PLAIN or dictionary encoding",
        parquet.Error.UnsupportedType => "this Parquet file has a column of a type this reader " ++
            "does not know",
        parquet.Error.ExternalColumn => "this Parquet file keeps a column in another file, which " ++
            "this reader does not follow",
        parquet.Error.TruncatedPage => "a Parquet page ends mid-value",
        parquet.Error.DictionaryMissing => "a Parquet page is dictionary-encoded but its column " ++
            "chunk has no dictionary",
        parquet.Error.DictionaryIndexOutOfRange => "a Parquet page indexes past the end of its " ++
            "dictionary",
        codec.Error.UnsupportedCodec => "this Parquet file is compressed with a codec this reader " ++
            "does not decode",
        codec.Error.CorruptPage => "a compressed Parquet page will not decode",
        codec.Error.PageSizeMismatch => "a Parquet page decompressed to a different size from the " ++
            "one its header states",
        encoding.Error.TruncatedValues => "a Parquet page holds fewer values than its levels promise",
        encoding.Error.BadBlockSize => "a delta-encoded Parquet page has a block size its " ++
            "miniblocks do not divide",
        encoding.Error.VarintTooLong => "a Parquet varint is longer than 64 bits",
        thrift_error.TruncatedMetadata => "the Parquet metadata ends mid-value",
        thrift_error.UnknownThriftType => "the Parquet metadata holds a Thrift type this reader " ++
            "does not know",
        else => "the comparison failed",
    };
}

const thrift_error = @import("thrift.zig").Error;

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

fn needsNormalising(o: Options) bool {
    return o.trim or o.ignore_case or o.empty_is_null or o.tolerance > 0;
}

fn isSpace(c: u8) bool {
    return c <= ' ';
}

/// The field's normalised bytes written into `buf`, or null when it is absent.
/// Only the normalising paths call this; the default path never builds anything
/// for a cell at all.
fn normalised(slab: Slab, f: Field, o: Options, buf: []u8) Error!?[]const u8 {
    if (!fld.isReal(f)) return null;
    var n: usize = 0;
    var it = slab.logical(f);
    while (it.next()) |b| {
        if (n >= buf.len) return Error.FieldTooLong;
        buf[n] = b;
        n += 1;
    }
    var s = buf[0..n];
    if (s.len == 0) return null;
    if (o.trim) {
        var start: usize = 0;
        var stop: usize = s.len;
        while (start < stop and isSpace(s[start])) start += 1;
        while (stop > start and isSpace(s[stop - 1])) stop -= 1;
        s = s[start..stop];
    }
    if (o.ignore_case) {
        // Folding case outside ASCII needs a Unicode table this port does not
        // carry, and folding it partially is worse than not folding it at all:
        // CAFE and cafe with an acute would compare equal in the ports that do
        // fold and unequal here, with nothing in the output to say why.
        for (s) |c| if (c >= 0x80) return Error.NonAsciiCaseFold;
        for (s, 0..) |c, i| s[i] = std.ascii.toLower(c);
    }
    if (o.empty_is_null and s.len == 0) return null;
    return s;
}

/// Scratch space for the two fields a comparison has in hand at once.
const Scratch = struct {
    a: [MAX_INLINE]u8 = undefined,
    b: [MAX_INLINE]u8 = undefined,
    const MAX_INLINE = 4096;
};

fn isAbsent(slab: Slab, f: Field, o: Options, buf: []u8) Error!bool {
    if (!fld.isReal(f) or fld.lenOf(f) == 0) return true;
    if (!needsNormalising(o)) return false;
    return (try normalised(slab, f, o, buf)) == null;
}

fn same(a: Slab, x: Field, b: Slab, y: Field, o: Options, s: *Scratch) Error!bool {
    const xa = try isAbsent(a, x, o, &s.a);
    const yb = try isAbsent(b, y, o, &s.b);
    if (xa or yb) return xa and yb;
    if (!needsNormalising(o)) return slab_mod.sameBytes(a, x, b, y);
    const nx = (try normalised(a, x, o, &s.a)) orelse "";
    const ny = (try normalised(b, y, o, &s.b)) orelse "";
    return std.mem.eql(u8, nx, ny);
}

/// Deliberately stricter than a plain float parse: "inf" and "nan" are ordinary
/// text in a CSV, and treating them as numbers would make two unequal strings
/// compare equal under a tolerance.
fn asNumber(value: []const u8) ?f64 {
    var s = value;
    while (s.len > 0 and isSpace(s[0])) s = s[1..];
    while (s.len > 0 and isSpace(s[s.len - 1])) s = s[0 .. s.len - 1];
    if (s.len == 0) return null;
    var body = s;
    if (body[0] == '+' or body[0] == '-') body = body[1..];
    if (body.len == 0) return null;
    if (!(std.ascii.isDigit(body[0]) or body[0] == '.')) return null;
    for (body) |c| {
        if (!(std.ascii.isDigit(c) or c == '.' or c == 'e' or c == 'E' or c == '+' or c == '-'))
            return null;
    }
    const v = std.fmt.parseFloat(f64, s) catch return null;
    return if (std.math.isFinite(v)) v else null;
}

/// Whether a field is absent, when nothing has to be normalised first.
///
/// The general `isAbsent` returns `Error!bool` because a normalising build can
/// refuse a non-ASCII byte under `--ignore-case`. With no normalising there is
/// no error to have, and the difference matters: the join asks this four times
/// per compared cell -- twice in `cellDiffers` and twice more inside the `same`
/// it calls -- so ten million rows over seventeen columns propagate an error
/// union nearly seven hundred million times for a question that is two bit
/// tests.
inline fn plainAbsent(f: Field) bool {
    return !fld.isReal(f) or fld.lenOf(f) == 0;
}

/// `cellDiffers` for the common case, computing each side's absence once and
/// returning a plain `bool`. Both absent is not a difference; exactly one is.
inline fn plainDiffers(a: Slab, x: Field, xa: bool, b: Slab, y: Field, yb: bool) bool {
    if (xa or yb) return xa != yb;
    return !slab_mod.sameBytes(a, x, b, y);
}

fn cellDiffers(a: Slab, x: Field, b: Slab, y: Field, o: Options, s: *Scratch) Error!bool {
    const xa = try isAbsent(a, x, o, &s.a);
    const yb = try isAbsent(b, y, o, &s.b);
    if (xa and yb) return false;
    if (o.tolerance > 0 and !xa and !yb) {
        const tx = (try normalised(a, x, o, &s.a)) orelse "";
        const nx = asNumber(tx);
        const ty = (try normalised(b, y, o, &s.b)) orelse "";
        const ny = asNumber(ty);
        if (nx != null and ny != null) return @abs(nx.? - ny.?) > o.tolerance;
    }
    return !(try same(a, x, b, y, o, s));
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
fn hashBytes(bytes: []const u8, seed: u64) u64 {
    const PRIME: u64 = 0x100_0000_01b3;
    var h = seed;
    var at: usize = 0;
    while (at + 8 <= bytes.len) : (at += 8) {
        h = (h ^ std.mem.readInt(u64, bytes[at..][0..8], .little)) *% PRIME;
        // The xor-shift is what spreads a whole word into the low bits, which is
        // where the table's slot comes from.
        h ^= h >> 29;
    }
    if (at < bytes.len) {
        var tail: [8]u8 = @splat(0);
        @memcpy(tail[0 .. bytes.len - at], bytes[at..]);
        h = (h ^ std.mem.readInt(u64, &tail, .little)) *% PRIME;
        h ^= h >> 29;
    }
    return h;
}

/// FNV-1a over exactly the bytes equality compares, by the same route.
fn hashField(slab: Slab, f: Field, o: Options, seed: u64, buf: []u8) Error!u64 {
    const PRIME: u64 = 0x100_0000_01b3;
    var h = seed;
    if (try isAbsent(slab, f, o, buf)) return (h ^ 0x9e37_79b9_7f4a_7c15) *% PRIME;
    var len: u64 = 0;
    if (needsNormalising(o)) {
        const v = (try normalised(slab, f, o, buf)) orelse "";
        for (v) |b| {
            h = (h ^ b) *% PRIME;
            len += 1;
        }
    } else {
        var it = slab.logical(f);
        if (it.isPlain()) {
            // Nothing to unescape, so the bytes are the value and eight of them
            // can be taken at a time. See `hashBytes`.
            const raw = slab.raw(f);
            h = hashBytes(raw, h);
            len = raw.len;
        } else {
            while (it.next()) |b| {
                h = (h ^ b) *% PRIME;
                len += 1;
            }
        }
    }
    return (h ^ len) *% PRIME;
}

fn keyHash(slab: Slab, fields: []const Field, key_size: usize, o: Options, buf: []u8) Error!u64 {
    var h: u64 = 0xcbf2_9ce4_8422_2325;
    for (fields[0..key_size]) |f| h = try hashField(slab, f, o, h, buf);
    return h;
}

// ---------------------------------------------------------------------------
// One file, read into the representation the join works on
// ---------------------------------------------------------------------------

/// How a file's rows are addressed.
const Rows = union(enum) {
    /// Text: a row is an offset into the mapping, re-parsed on demand. The index
    /// stores where a row starts rather than its fields, because an offset is
    /// eight bytes where the fields would be twenty times that, and re-parsing is
    /// cheap because the parser stops at the last needed column.
    text: struct { parser: text.RowParser, from: usize },
    /// Parquet: there is no row to re-read, so the fields are materialised once,
    /// row-major, and a row is an index into them.
    columnar: struct { fields: []Field, rows: usize },
};

const Side = struct {
    gpa: std.mem.Allocator,
    slab: Slab,
    rows: Rows,
    width: usize,
    /// Held so the JSON parser's key names outlive it.
    wanted_names: ?[]?[]const u8 = null,
    wanted_source: ?[]?usize = null,

    /// The fields of the row addressed by `at` — a byte offset for a text file,
    /// a row number for a columnar one.
    fn fieldsAt(self: Side, at: u64, out: []Field) void {
        switch (self.rows) {
            .text => |t| _ = t.parser.parse(self.slab.data, @intCast(at), self.slab.data.len, out),
            .columnar => |c| {
                const from = @as(usize, @intCast(at)) * self.width;
                @memcpy(out, c.fields[from..][0..self.width]);
            },
        }
    }

    fn deinit(self: *Side) void {
        switch (self.rows) {
            .text => |t| t.parser.deinit(self.gpa),
            .columnar => |c| self.gpa.free(c.fields),
        }
        if (self.wanted_names) |n| self.gpa.free(n);
        if (self.wanted_source) |s| self.gpa.free(s);
        self.slab.close();
    }
};

/// A file after its header has been read but before the comparison knows which
/// columns it wants. The two phases are separate because a Parquet file should
/// decode the columns being compared and no others, and that list is not known
/// until both headers have been resolved against each other.
const Input = union(enum) {
    text: struct { slab: Slab, delimiter: u8, from: usize, names: [][]const u8 },
    parquet: struct { reader: parquet.Reader, names: [][]const u8 },

    fn open(gpa: std.mem.Allocator, io: std.Io, path: []const u8, opt: Options) !Input {
        // The format is decided by what is in the file, not by its name: a file
        // called `.parquet` that is really a CSV is read as a CSV.
        var slab = try Slab.map(io, path);
        errdefer slab.close();
        if (parquet.looksLikeParquet(slab.data)) {
            slab.close();
            var reader = try parquet.Reader.open(gpa, io, path);
            errdefer reader.deinit();
            return .{ .parquet = .{ .reader = reader, .names = try reader.columnNames(gpa) } };
        }
        // A file that begins with the magic and does not end with it is a
        // truncated Parquet file. Reading it as a CSV would report a missing key
        // column, which sends the reader looking in the wrong place entirely.
        if (slab.data.len >= 4 and std.mem.eql(u8, slab.data[0..4], "PAR1"))
            return parquet.Error.TruncatedParquetFile;
        slab.dialect = text.sniffDialect(slab.data);
        if (slab.dialect == .json) {
            const head = try text.readJsonHeader(gpa, slab);
            return .{ .text = .{ .slab = slab, .delimiter = ',', .from = 0, .names = head.names } };
        }
        const delimiter = opt.delimiter orelse
            text.detectDelimiter(slab.data[0..scan.nextOf1(slab.data, 0, slab.data.len, '\n')]);
        const head = try text.readCsvHeader(gpa, slab, delimiter);
        return .{ .text = .{
            .slab = slab,
            .delimiter = delimiter,
            .from = head.start,
            .names = head.names,
        } };
    }

    fn names(self: Input) [][]const u8 {
        return switch (self) {
            .text => |t| t.names,
            .parquet => |p| p.names,
        };
    }

    fn freeNames(self: Input, gpa: std.mem.Allocator) void {
        for (self.names()) |n| gpa.free(n);
        gpa.free(self.names());
    }

    /// Reads the file into the join's representation, keeping only `wanted`.
    /// Takes ownership: on return the input is spent either way.
    fn project(
        self: Input,
        gpa: std.mem.Allocator,
        wanted: []const []const u8,
        threads: usize,
    ) !Side {
        const width = wanted.len;
        switch (self) {
            .text => |t| {
                var side = Side{ .gpa = gpa, .slab = t.slab, .width = width, .rows = undefined };
                errdefer side.slab.close();
                if (t.slab.dialect == .json) {
                    const keys = try gpa.alloc(?[]const u8, width);
                    errdefer gpa.free(keys);
                    for (wanted, 0..) |n, i| keys[i] = if (has(t.names, n)) n else null;
                    side.wanted_names = keys;
                    side.rows = .{ .text = .{
                        .parser = try text.RowParser.initJson(gpa, keys),
                        .from = t.from,
                    } };
                } else {
                    const source = try gpa.alloc(?usize, width);
                    errdefer gpa.free(source);
                    for (wanted, 0..) |n, i| source[i] = indexOf(t.names, n);
                    side.wanted_source = source;
                    side.rows = .{ .text = .{
                        .parser = try text.RowParser.initCsv(gpa, t.delimiter, source),
                        .from = t.from,
                    } };
                }
                return side;
            },
            .parquet => |p| {
                var reader = p.reader;
                defer reader.deinit(); // the mapping is finished with once decoded
                const keys = try gpa.alloc(?[]const u8, width);
                defer gpa.free(keys);
                for (wanted, 0..) |n, i| keys[i] = if (has(p.names, n)) n else null;
                const rows = reader.rows;
                const projected = try reader.project(gpa, keys, threads);
                return .{
                    .gpa = gpa,
                    .slab = Slab.owned(gpa, projected.arena, .raw),
                    .rows = .{ .columnar = .{ .fields = projected.fields, .rows = rows } },
                    .width = width,
                };
            },
        }
    }
};

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

const EMPTY: i32 = -1;

/// One chunk's rows, in the order they appear in it.
const Chunk = struct {
    at: std.ArrayList(u64) = .empty,
    hash: std.ArrayList(u64) = .empty,
    /// Written only by the thread that owns this chunk, read only once every
    /// thread has been joined -- which is why no lock is needed for it.
    failure: ?anyerror = null,

    fn deinit(self: *Chunk, gpa: std.mem.Allocator) void {
        self.at.deinit(gpa);
        self.hash.deinit(gpa);
    }
};

/// Open addressing over one file's rows. Everything is a primitive array taken
/// from the caller's allocator, so an index that would not fit in the budget
/// fails here rather than growing until the kernel intervenes.
const RowIndex = struct {
    gpa: std.mem.Allocator,
    side: *const Side,
    key_size: usize,
    opt: Options,
    row_at: std.ArrayList(u64),
    row_hash: std.ArrayList(u64),
    table: []i32,
    mask: usize,
    first_row: std.ArrayList(i32),
    occurrences: std.ArrayList(u32),
    probe: []Field,
    mine: []Field,
    rows: i64 = 0,
    dup_keys: i64 = 0,
    dup_rows: i64 = 0,

    /// Finds and hashes every row in parallel, then inserts them on one thread in
    /// file order.
    ///
    /// The split is safe because the two halves need different things: parsing a
    /// row depends on nothing but where it starts, while the table depends on the
    /// order rows arrive — first occurrence of a key wins, and the duplicate
    /// counts follow from that. Doing the second half in parallel would make the
    /// answer depend on thread scheduling.
    fn build(
        gpa: std.mem.Allocator,
        side: *const Side,
        key_size: usize,
        opt: Options,
        threads: usize,
    ) !RowIndex {
        const chunks = try sweep(gpa, side, key_size, opt, threads);
        defer {
            for (chunks) |*c| c.deinit(gpa);
            gpa.free(chunks);
        }

        const table = try gpa.alloc(i32, 1 << 12);
        @memset(table, EMPTY);
        var self = RowIndex{
            .gpa = gpa,
            .side = side,
            .key_size = key_size,
            .opt = opt,
            .row_at = .empty,
            .row_hash = .empty,
            .table = table,
            .mask = table.len - 1,
            .first_row = .empty,
            .occurrences = .empty,
            .probe = try gpa.alloc(Field, side.width),
            .mine = try gpa.alloc(Field, side.width),
        };
        errdefer self.deinit();

        var total: usize = 0;
        for (chunks) |c| total += c.at.items.len;
        try self.row_at.ensureTotalCapacity(gpa, total);
        try self.row_hash.ensureTotalCapacity(gpa, total);

        var s = Scratch{};
        // Each chunk is released as soon as it has been inserted. Holding all of
        // them to the end would keep two copies of every row's address and hash
        // alive at once, which is sixteen bytes a row of pure duplication.
        for (chunks) |*chunk| {
            for (chunk.at.items, chunk.hash.items) |at, hash| try self.insert(at, hash, &s);
            // Emptied rather than only released, because the caller frees the
            // chunks too and a list deinitialised twice frees a pointer it no
            // longer owns.
            chunk.deinit(gpa);
            chunk.* = .{};
        }
        return self;
    }

    fn deinit(self: *RowIndex) void {
        self.row_at.deinit(self.gpa);
        self.row_hash.deinit(self.gpa);
        self.first_row.deinit(self.gpa);
        self.occurrences.deinit(self.gpa);
        self.gpa.free(self.table);
        self.gpa.free(self.probe);
        self.gpa.free(self.mine);
    }

    fn insert(self: *RowIndex, at: u64, hash: u64, s: *Scratch) !void {
        self.rows += 1;
        const row: i32 = @intCast(self.row_at.items.len);
        try self.row_at.append(self.gpa, at);
        try self.row_hash.append(self.gpa, hash);

        var slot = self.slotOf(hash);
        var mine_parsed = false;
        while (true) {
            const key = self.table[slot];
            if (key == EMPTY) {
                self.table[slot] = @intCast(self.first_row.items.len);
                try self.first_row.append(self.gpa, row);
                try self.occurrences.append(self.gpa, 1);
                if (self.first_row.items.len * 2 > self.table.len) try self.rehash();
                return;
            }
            const candidate = self.first_row.items[@intCast(key)];
            if (self.row_hash.items[@intCast(candidate)] == hash) {
                // This row's fields are re-read rather than carried over from the
                // sweep because the sweep produced ten million of them and this
                // branch wants one.
                if (!mine_parsed) {
                    self.side.fieldsAt(at, self.mine);
                    mine_parsed = true;
                }
                self.fieldsOf(candidate, self.probe);
                var ok = true;
                for (0..self.key_size) |i| {
                    if (!(try same(self.side.slab, self.probe[i], self.side.slab, self.mine[i], self.opt, s))) {
                        ok = false;
                        break;
                    }
                }
                if (ok) {
                    self.occurrences.items[@intCast(key)] += 1;
                    if (self.occurrences.items[@intCast(key)] == 2) {
                        self.dup_keys += 1;
                        self.dup_rows += 1; // the first occurrence counts once the key repeats
                    }
                    self.dup_rows += 1;
                    return;
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }

    fn fieldsOf(self: RowIndex, row: i32, out: []Field) void {
        self.side.fieldsAt(self.row_at.items[@intCast(row)], out);
    }

    /// The high bits of an FNV hash are the well-mixed ones; fold them down.
    fn slotOf(self: RowIndex, hash: u64) usize {
        return @as(usize, @intCast((hash ^ (hash >> 32)) & 0xffff_ffff)) & self.mask;
    }

    fn rehash(self: *RowIndex) !void {
        const table = try self.gpa.alloc(i32, self.table.len * 2);
        @memset(table, EMPTY);
        self.gpa.free(self.table);
        self.table = table;
        self.mask = table.len - 1;
        for (self.first_row.items, 0..) |row, key| {
            var slot = self.slotOf(self.row_hash.items[@intCast(row)]);
            while (self.table[slot] != EMPTY) slot = (slot + 1) & self.mask;
            self.table[slot] = @intCast(key);
        }
    }

    /// The row carrying `fields`' key, or null. `other` is the slab those fields
    /// live in, which is the opposite file when this is a join probe. `probe` is
    /// scratch the caller owns: the join runs several ranges at once, and a buffer
    /// hanging off the index would be shared between them.
    fn lookup(
        self: *const RowIndex,
        other: Slab,
        fields: []const Field,
        hash: u64,
        s: *Scratch,
        probe: []Field,
    ) !?i32 {
        var slot = self.slotOf(hash);
        while (true) {
            const key = self.table[slot];
            if (key == EMPTY) return null;
            const candidate = self.first_row.items[@intCast(key)];
            if (self.row_hash.items[@intCast(candidate)] == hash) {
                self.fieldsOf(candidate, probe);
                var ok = true;
                for (0..self.key_size) |i| {
                    if (!(try same(self.side.slab, probe[i], other, fields[i], self.opt, s))) {
                        ok = false;
                        break;
                    }
                }
                if (ok) return candidate;
            }
            slot = (slot + 1) & self.mask;
        }
    }

    fn uniqueKeys(self: RowIndex) i64 {
        return @intCast(self.first_row.items.len);
    }
};

// ---------------------------------------------------------------------------
// The sweep: finding and hashing every row, in parallel
// ---------------------------------------------------------------------------

/// Where each chunk begins, as offsets of real row starts.
///
/// The nominal split is `size / threads`, walked forward to the next row. Walking
/// forward is the whole difficulty for CSV: a newline inside a quoted field is not
/// a row boundary, and a thread starting mid-file cannot tell whether it is inside
/// one. Parity settles it — every `"` toggles in-quote state, including both halves
/// of a doubled quote, which toggles twice and so correctly leaves the state alone,
/// so the count of quotes before a position says whether that position is inside a
/// field. Counting them is a scan for one byte, far cheaper than parsing.
///
/// JSON needs none of that: a raw newline inside a string is not valid JSON, so
/// every newline ends a record.
fn chunkBounds(
    gpa: std.mem.Allocator,
    data: []const u8,
    from: usize,
    threads: usize,
    dialect: Dialect,
) ![]usize {
    var bounds: std.ArrayList(usize) = .empty;
    errdefer bounds.deinit(gpa);
    const end = data.len;
    if (threads <= 1 or end - from < CHUNKING_THRESHOLD) {
        try bounds.appendSlice(gpa, &.{ from, end });
        return bounds.toOwnedSlice(gpa);
    }

    try bounds.append(gpa, from);
    for (1..threads) |i| {
        const nominal = from + (end - from) * i / threads;
        var in_quotes = dialect == .csv and
            scan.countByte(data, from, nominal, '"') % 2 == 1;
        var at = nominal;
        while (at < end) : (at += 1) {
            const c = data[at];
            if (c == '"' and dialect == .csv) {
                in_quotes = !in_quotes;
            } else if (c == '\n' and !in_quotes) {
                at += 1;
                break;
            }
        }
        if (at > bounds.items[bounds.items.len - 1] and at < end) try bounds.append(gpa, at);
    }
    try bounds.append(gpa, end);
    return bounds.toOwnedSlice(gpa);
}

/// The shared state of a parallel sweep: a range each, and the first failure.
const Sweep = struct {
    gpa: std.mem.Allocator,
    side: *const Side,
    key_size: usize,
    opt: Options,
    bounds: []const usize,
    /// Set for a columnar file, where a chunk is a range of row numbers rather
    /// than a range of bytes.
    columnar_rows: usize,
    chunks: []Chunk,
    next: std.atomic.Value(usize),

    fn run(self: *Sweep) void {
        while (true) {
            const i = self.next.fetchAdd(1, .monotonic);
            if (i >= self.chunks.len) return;
            self.one(i) catch |e| {
                self.chunks[i].failure = e;
                return;
            };
        }
    }

    fn one(self: *Sweep, i: usize) !void {
        const gpa = self.gpa;
        const fields = try gpa.alloc(Field, self.side.width);
        defer gpa.free(fields);
        var s = Scratch{};
        var chunk = &self.chunks[i];

        switch (self.side.rows) {
            .columnar => {
                const lo = self.columnar_rows * i / self.chunks.len;
                const hi = self.columnar_rows * (i + 1) / self.chunks.len;
                try chunk.at.ensureTotalCapacity(gpa, hi - lo);
                try chunk.hash.ensureTotalCapacity(gpa, hi - lo);
                for (lo..hi) |row| {
                    self.side.fieldsAt(row, fields);
                    chunk.at.appendAssumeCapacity(row);
                    chunk.hash.appendAssumeCapacity(
                        try keyHash(self.side.slab, fields, self.key_size, self.opt, &s.a),
                    );
                }
            },
            .text => |t| {
                const data = self.side.slab.data;
                var pos = self.bounds[i];
                const stop = self.bounds[i + 1];
                // Rows that *start* in this chunk belong to it; the last one is
                // finished past the boundary rather than cut in half.
                while (pos < stop) {
                    // A line with nothing on it is not a row.
                    if (data[pos] == '\n') {
                        pos += 1;
                        continue;
                    }
                    if (data[pos] == '\r' and pos + 1 < data.len and data[pos + 1] == '\n') {
                        pos += 2;
                        continue;
                    }
                    const next = t.parser.parse(data, pos, data.len, fields);
                    for (fields) |field| if (field == TOO_LONG) return Error.FieldTooLong;
                    try chunk.at.append(gpa, pos);
                    try chunk.hash.append(
                        gpa,
                        try keyHash(self.side.slab, fields, self.key_size, self.opt, &s.a),
                    );
                    if (next <= pos) break; // no progress: a malformed tail, not a loop
                    pos = next;
                }
            },
        }
    }
};

fn sweep(
    gpa: std.mem.Allocator,
    side: *const Side,
    key_size: usize,
    opt: Options,
    threads: usize,
) ![]Chunk {
    var bounds: []usize = &.{};
    var parts: usize = 1;
    switch (side.rows) {
        .text => |t| {
            const data = side.slab.data;
            if (t.from >= data.len) return gpa.alloc(Chunk, 0);
            bounds = try chunkBounds(gpa, data, t.from, threads, side.slab.dialect);
            parts = bounds.len - 1;
        },
        .columnar => |c| {
            parts = @max(1, @min(threads, (c.rows + (1 << 14) - 1) / (1 << 14)));
        },
    }
    defer if (bounds.len > 0) gpa.free(bounds);

    const chunks = try gpa.alloc(Chunk, parts);
    errdefer {
        for (chunks) |*c| c.deinit(gpa);
        gpa.free(chunks);
    }
    for (chunks) |*c| c.* = .{};

    var work = Sweep{
        .gpa = gpa,
        .side = side,
        .key_size = key_size,
        .opt = opt,
        .bounds = bounds,
        .columnar_rows = switch (side.rows) {
            .columnar => |c| c.rows,
            .text => 0,
        },
        .chunks = chunks,
        .next = std.atomic.Value(usize).init(0),
    };
    try runOnThreads(&work, Sweep.run, parts);
    for (chunks) |c| {
        if (c.failure) |e| return e;
    }
    return chunks;
}

/// Runs `entry` on `ways` threads, or on this one where a thread cannot be had:
/// a thread that will not spawn is not a reason to fail, it is the same work.
fn runOnThreads(state: anytype, comptime entry: anytype, ways: usize) !void {
    var workers: [64]?std.Thread = undefined;
    const spawned = @min(if (ways > 0) ways - 1 else 0, workers.len);
    for (0..spawned) |i| workers[i] = std.Thread.spawn(.{}, entry, .{state}) catch null;
    entry(state);
    for (0..spawned) |i| {
        if (workers[i]) |w| w.join() else entry(state);
    }
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

fn has(haystack: []const []const u8, needle: []const u8) bool {
    for (haystack) |h| {
        if (std.mem.eql(u8, h, needle)) return true;
    }
    return false;
}

fn indexOf(haystack: []const []const u8, needle: []const u8) ?usize {
    for (haystack, 0..) |h, i| {
        if (std.mem.eql(u8, h, needle)) return i;
    }
    return null;
}

// ---------------------------------------------------------------------------
// The comparison
// ---------------------------------------------------------------------------

pub const Result = struct {
    counts: Counts,
    columns: []ColumnStat,

    pub fn identical(self: Result) bool {
        return self.counts.changed == 0 and self.counts.added == 0 and self.counts.removed == 0;
    }

    /// The column names are owned copies, so the header they were read from can be
    /// released as soon as the comparison is done with it.
    pub fn deinit(self: *Result, gpa: std.mem.Allocator) void {
        for (self.columns) |c| gpa.free(c.name);
        gpa.free(self.columns);
    }
};

/// One range of A's keys, joined on its own thread. Each range keeps its own
/// counts and column stats; the merge is a sum, and the ranges are contiguous, so
/// the answer does not depend on how many there are.
const Part = struct {
    matched: i64 = 0,
    changed: i64 = 0,
    removed: i64 = 0,
    columns: []ColumnStat,
    /// Written only by the thread that owns this range, read only once every
    /// thread has been joined.
    failure: ?anyerror = null,
};

const Join = struct {
    a: *const Side,
    b: *const Side,
    ai: *const RowIndex,
    bi: *const RowIndex,
    opt: Options,
    key_size: usize,
    nc: usize,
    width: usize,
    parts: []Part,
    next: std.atomic.Value(usize),

    fn run(self: *Join) void {
        while (true) {
            const i = self.next.fetchAdd(1, .monotonic);
            if (i >= self.parts.len) return;
            self.range(i) catch |e| {
                self.parts[i].failure = e;
                return;
            };
        }
    }

    fn range(self: *Join, p: usize) !void {
        const gpa = self.a.gpa;
        const fa = try gpa.alloc(Field, self.width);
        defer gpa.free(fa);
        const fb = try gpa.alloc(Field, self.width);
        defer gpa.free(fb);
        const probe = try gpa.alloc(Field, self.width);
        defer gpa.free(probe);
        var s = Scratch{};
        var out = &self.parts[p];

        const keys = self.ai.first_row.items;
        const lo = keys.len * p / self.parts.len;
        const hi = keys.len * (p + 1) / self.parts.len;
        // Nothing to normalise and no tolerance: the cell comparison cannot
        // fail, so it need not be asked through an error union.
        const plain = !needsNormalising(self.opt);
        for (keys[lo..hi]) |row| {
            self.ai.fieldsOf(row, fa);
            // The hash is the one the sweep computed for this row: the same
            // bytes through the same function, so computing it again here would
            // be a second pass over every key in the file for the same number.
            const hash = self.ai.row_hash.items[@intCast(row)];
            const mate = (try self.bi.lookup(self.a.slab, fa, hash, &s, probe)) orelse {
                out.removed += 1;
                continue;
            };
            out.matched += 1;
            self.bi.fieldsOf(mate, fb);
            var any = false;
            // Two loops rather than one with a flag inside it: `plain` cannot
            // change between columns or between rows, and this is the innermost
            // loop of the whole comparison -- seventeen columns of ten million
            // matched rows.
            if (plain) {
                for (0..self.nc) |i| {
                    const x = fa[self.key_size + i];
                    const y = fb[self.key_size + i];
                    const xa = plainAbsent(x);
                    const yb = plainAbsent(y);
                    if (plainDiffers(self.a.slab, x, xa, self.b.slab, y, yb)) {
                        any = true;
                        out.columns[i].changed += 1;
                        // Absence is already known: the general path asks for it
                        // twice more, having thrown away the answer.
                        if (yb) out.columns[i].blanked += 1;
                        if (xa) out.columns[i].filled += 1;
                    }
                }
            } else {
                for (0..self.nc) |i| {
                    const x = fa[self.key_size + i];
                    const y = fb[self.key_size + i];
                    if (try cellDiffers(self.a.slab, x, self.b.slab, y, self.opt, &s)) {
                        any = true;
                        out.columns[i].changed += 1;
                        if (try isAbsent(self.b.slab, y, self.opt, &s.b)) out.columns[i].blanked += 1;
                        if (try isAbsent(self.a.slab, x, self.opt, &s.a)) out.columns[i].filled += 1;
                    }
                }
            }
            if (any) out.changed += 1;
        }
    }
};

/// B's side of the join: which of B's keys are not in A at all. It reads both
/// indexes and writes only this count, so it runs beside A's ranges.
const Added = struct {
    a: *const Side,
    b: *const Side,
    ai: *const RowIndex,
    bi: *const RowIndex,
    opt: Options,
    key_size: usize,
    width: usize,
    count: i64 = 0,
    failure: ?anyerror = null,

    fn run(self: *Added) void {
        self.go() catch |e| {
            self.failure = e;
        };
    }

    fn go(self: *Added) !void {
        const gpa = self.b.gpa;
        const fb = try gpa.alloc(Field, self.width);
        defer gpa.free(fb);
        const probe = try gpa.alloc(Field, self.width);
        defer gpa.free(probe);
        var s = Scratch{};
        var n: i64 = 0;
        for (self.bi.first_row.items) |row| {
            self.bi.fieldsOf(row, fb);
            const hash = self.bi.row_hash.items[@intCast(row)];
            if ((try self.ai.lookup(self.b.slab, fb, hash, &s, probe)) == null) n += 1;
        }
        self.count = n;
    }
};

/// One file's whole preparation — decode or map it, then index it — so that both
/// files can be done at once.
const Prepare = struct {
    gpa: std.mem.Allocator,
    input: Input,
    wanted: []const []const u8,
    key_size: usize,
    opt: Options,
    threads: usize,
    side: ?Side = null,
    index: ?RowIndex = null,
    failure: ?anyerror = null,

    fn run(self: *Prepare) void {
        self.go() catch |e| {
            self.failure = e;
        };
    }

    fn go(self: *Prepare) !void {
        self.side = try self.input.project(self.gpa, self.wanted, self.threads);
        self.index = try RowIndex.build(
            self.gpa,
            &self.side.?,
            self.key_size,
            self.opt,
            self.threads,
        );
    }

    fn deinit(self: *Prepare) void {
        if (self.index) |*i| i.deinit();
        if (self.side) |*s| s.deinit();
    }
};

/// Compares two files, taking every byte it needs from `gpa`.
///
/// Hand it a `FixedBufferAllocator` and the comparison cannot exceed that buffer:
/// it returns `error.OutOfMemory` at the allocation that would have crossed the
/// line. That is the whole reason this port is in Zig.
pub fn compare(
    io: std.Io,
    gpa: std.mem.Allocator,
    a_path: []const u8,
    b_path: []const u8,
    opt: Options,
) !Result {
    const total = if (opt.threads > 0) opt.threads else @max(1, std.Thread.getCpuCount() catch 1);

    const a_input = try Input.open(gpa, io, a_path, opt);
    defer a_input.freeNames(gpa);
    const b_input = try Input.open(gpa, io, b_path, opt);
    defer b_input.freeNames(gpa);
    const a_names = a_input.names();
    const b_names = b_input.names();

    for (opt.key) |k| {
        if (!has(a_names, k) or !has(b_names, k)) return Error.KeyColumnMissing;
    }

    var compared: std.ArrayList([]const u8) = .empty;
    defer compared.deinit(gpa);
    if (opt.compare.len > 0) {
        for (opt.compare) |c| {
            if (!has(a_names, c) or !has(b_names, c)) return Error.ComparedColumnMissing;
            try compared.append(gpa, c);
        }
    } else {
        for (a_names) |c| {
            if (has(b_names, c) and !has(opt.key, c) and !has(opt.ignore, c))
                try compared.append(gpa, c);
        }
    }

    const key_size = opt.key.len;
    const nc = compared.items.len;
    const width = key_size + nc;

    var wanted: std.ArrayList([]const u8) = .empty;
    defer wanted.deinit(gpa);
    for (opt.key) |k| try wanted.append(gpa, k);
    for (compared.items) |c| try wanted.append(gpa, c);

    // The two files share nothing until the join, so they are read at the same
    // time, and each is split further: two files across four cores is two chunks
    // each, so the whole machine is busy rather than half of it.
    //
    // `gpa` has to be thread-safe for this. Under --max-memory it is a
    // FixedBufferAllocator -- a bump pointer with no lock, which would hand two
    // threads the same bytes -- so main.zig passes its lock-taking variant. The
    // budget it enforces is unchanged.
    const per_file = @max(1, total / 2);
    var prepare_a = Prepare{
        .gpa = gpa,
        .input = a_input,
        .wanted = wanted.items,
        .key_size = key_size,
        .opt = opt,
        .threads = per_file,
    };
    var prepare_b = Prepare{
        .gpa = gpa,
        .input = b_input,
        .wanted = wanted.items,
        .key_size = key_size,
        .opt = opt,
        .threads = per_file,
    };
    const reader: ?std.Thread = std.Thread.spawn(.{}, Prepare.run, .{&prepare_b}) catch null;
    if (reader == null) prepare_b.run();
    prepare_a.run();
    if (reader) |w| w.join();
    defer prepare_a.deinit();
    defer prepare_b.deinit();
    if (prepare_a.failure) |e| return e;
    if (prepare_b.failure) |e| return e;

    const a = &prepare_a.side.?;
    const b = &prepare_b.side.?;
    const ai = &prepare_a.index.?;
    const bi = &prepare_b.index.?;

    // Owned copies: the names point into the header, which is released when this
    // function returns, and the result outlives it.
    const columns = try gpa.alloc(ColumnStat, nc);
    var made: usize = 0;
    errdefer {
        for (columns[0..made]) |c| gpa.free(c.name);
        gpa.free(columns);
    }
    for (compared.items, 0..) |name, i| {
        columns[i] = .{ .name = try gpa.dupe(u8, name) };
        made += 1;
    }

    // A's side is the long pole: every distinct key is looked up in B, both rows
    // are read, and every compared column is examined. B's side only asks whether
    // each of its keys exists in A. So A splits over ranges and B gets a thread of
    // its own; the two write different outputs and read both indexes without
    // writing either.
    var ways = if (total > 1) total - 1 else 1;
    if (ai.first_row.items.len < JOIN_THRESHOLD) ways = 1;
    const parts = try gpa.alloc(Part, ways);
    defer {
        for (parts) |p| gpa.free(p.columns);
        gpa.free(parts);
    }
    var built: usize = 0;
    errdefer for (parts[0..built]) |p| gpa.free(p.columns);
    for (parts) |*p| {
        const stats = try gpa.alloc(ColumnStat, nc);
        for (stats, compared.items) |*stat, name| stat.* = .{ .name = name };
        p.* = .{ .columns = stats };
        built += 1;
    }

    var join = Join{
        .a = a,
        .b = b,
        .ai = ai,
        .bi = bi,
        .opt = opt,
        .key_size = key_size,
        .nc = nc,
        .width = width,
        .parts = parts,
        .next = std.atomic.Value(usize).init(0),
    };
    var added = Added{
        .a = a,
        .b = b,
        .ai = ai,
        .bi = bi,
        .opt = opt,
        .key_size = key_size,
        .width = width,
    };
    const adder: ?std.Thread = std.Thread.spawn(.{}, Added.run, .{&added}) catch null;
    if (adder == null) added.run();
    try runOnThreads(&join, Join.run, ways);
    if (adder) |w| w.join();
    for (parts) |part| {
        if (part.failure) |e| return e;
    }
    if (added.failure) |e| return e;

    var counts = Counts{ .added = added.count };
    for (parts) |part| {
        counts.matched += part.matched;
        counts.changed += part.changed;
        counts.removed += part.removed;
        for (columns, part.columns) |*into, from| {
            into.changed += from.changed;
            into.blanked += from.blanked;
            into.filled += from.filled;
        }
    }

    counts.a_rows = ai.rows;
    counts.b_rows = bi.rows;
    counts.a_keys = ai.uniqueKeys();
    counts.b_keys = bi.uniqueKeys();
    counts.unchanged = counts.matched - counts.changed;
    counts.a_dup_keys = ai.dup_keys;
    counts.a_dup_rows = ai.dup_rows;
    counts.b_dup_keys = bi.dup_keys;
    counts.b_dup_rows = bi.dup_rows;

    return .{ .counts = counts, .columns = columns };
}
