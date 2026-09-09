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

const builtin = @import("builtin");
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

/// Phase timings, on stderr, when `CSVDIFF_PHASES` is set — the same switch and
/// the same output shape the other two ports use.
///
/// A comparison has costs that move independently: sweeping and hashing every
/// row, inserting them into the index, and joining. Knowing which one grew is
/// the difference between tuning and guessing, and the cores-busy column says
/// only *that* something is serial, never which thing.
/// Set once from `CSVDIFF_PHASES` before the comparison starts; read by every
/// thread and written by none of them. `main.zig` owns the environment, so it
/// does the reading.
pub var phases_on: bool = false;

pub const Phases = struct {
    on: bool,
    tag: []const u8,
    last: i128,

    /// `tag` prefixes every line, because the two files are read on two threads
    /// and their phases would otherwise interleave unattributed.
    ///
    /// The monotonic clock straight from the platform: std's timing now wants an
    /// `Io`, and threading one through the engine for a diagnostic that is off by
    /// default would be a worse trade than this switch.
    ///
    /// The switch is the point. This was one unconditional call to
    /// `std.os.linux.clock_gettime`, which *compiles* on macOS -- `std.os.linux`
    /// is a namespace, not a target check -- and then emits Linux syscall numbers
    /// on a kernel that does not use them. A build error would have been the
    /// kinder failure. `Phases.start` calls this whether or not `CSVDIFF_PHASES`
    /// is set, so every macOS run went through it.
    fn now() i128 {
        return switch (builtin.os.tag) {
            // Linux by way of the kernel, so this port keeps linking no libc.
            .linux => blk: {
                var ts: std.os.linux.timespec = undefined;
                _ = std.os.linux.clock_gettime(.MONOTONIC, &ts);
                break :blk @as(i128, ts.sec) * std.time.ns_per_s + ts.nsec;
            },
            // Darwin always links libSystem, so the C entry point is the one.
            .macos, .ios, .tvos, .watchos, .visionos => blk: {
                var ts: std.c.timespec = undefined;
                _ = std.c.clock_gettime(.MONOTONIC, &ts);
                break :blk @as(i128, ts.sec) * std.time.ns_per_s + ts.nsec;
            },
            .windows => blk: {
                var counter: std.os.windows.LARGE_INTEGER = undefined;
                var frequency: std.os.windows.LARGE_INTEGER = undefined;
                _ = std.os.windows.ntdll.RtlQueryPerformanceCounter(&counter);
                _ = std.os.windows.ntdll.RtlQueryPerformanceFrequency(&frequency);
                if (frequency == 0) break :blk 0;
                break :blk @divTrunc(@as(i128, counter) * std.time.ns_per_s, @as(i128, frequency));
            },
            // Deliberately a build error rather than a plausible-looking number:
            // a phase table is only worth reading if its clock is real.
            else => @compileError(
                "no monotonic clock for this target; add one to Phases.now()",
            ),
        };
    }

    pub fn start(tag: []const u8) Phases {
        return .{
            .on = phases_on,
            .tag = tag,
            .last = now(),
        };
    }

    pub fn mark(self: *Phases, what: []const u8) void {
        const at = now();
        if (self.on) {
            const seconds = @as(f64, @floatFromInt(at - self.last)) / 1e9;
            var buf: [64]u8 = undefined;
            const name = std.fmt.bufPrint(&buf, "{s}{s}", .{ self.tag, what }) catch what;
            std.debug.print("  {s: <26} {d: >7.3}s\n", .{ name, seconds });
        }
        self.last = at;
    }
};

/// How many rows ahead a table probe is started. Enough misses in flight to
/// cover the latency of one, and not so many that the lines are evicted before
/// the loop reaches them.
pub const PREFETCH_AHEAD: usize = 32;

/// Keys per join chunk. Sized in rows rather than in threads so that a chunk is
/// small enough that no single one of them is the last thing four cores are
/// waiting on: at ten million keys this is a couple of hundred chunks of a few
/// hundredths of a second each.
const JOIN_CHUNK: usize = 1 << 16;

/// How many chunks `keys` divides into. One below the threshold, where finding
/// the boundaries would cost more than the join it splits.
fn waysFor(keys: usize) usize {
    if (keys < JOIN_THRESHOLD) return 1;
    return @max(1, std.math.divCeil(usize, keys, JOIN_CHUNK) catch 1);
}

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
        if (!fld.isEscaped(f)) {
            // Nothing to unescape, so the bytes are the value and eight of them
            // can be taken at a time. See `hashBytes`.
            //
            // The field word is asked directly rather than through `logical()`,
            // which answers the same question by building an iterator over the
            // slab -- the cost `sameBytes` used to pay, in the one function that
            // runs for every key column of every row of both files.
            const raw = slab.raw(f);
            h = hashBytes(raw, h);
            len = raw.len;
        } else {
            var it = slab.logical(f);
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
    text: struct {
        parser: text.RowParser,
        /// The same parser configured with the key columns alone, for the sweep.
        ///
        /// The sweep reads every row of the file and wants two things from each:
        /// its hash, which comes from the key columns, and where the next row
        /// starts. It has never wanted the other seventeen, and packing them was
        /// most of what it cost -- a parser stops at the last column it was asked
        /// for, so asking for less makes the rest of the row a plain scan for the
        /// newline rather than a field-by-field walk.
        keys: text.RowParser,
        from: usize,
    },
    /// Parquet: there is no row to re-read, so the fields are materialised once,
    /// row-major, and a row is an index into them.
    columnar: struct { fields: []Field, rows: usize },
};

/// How much of a candidate row a lookup has to parse.
///
/// The two directions of the join want different things from the row they land
/// on. A's direction compares every column against its mate, so it needs the
/// whole row and keeps it. B's direction is asking one question -- is this key in
/// A? -- and discards the row it matched against; parsing the eighteen columns it
/// will not look at is the largest thing that side was doing.
const Want = enum { whole, keys };

/// A run of one row's bytes, and the delimiter that has to end it in the other
/// file for that run to be a whole number of columns.
const Span = struct { bytes: []const u8, delimiter: u8 };

/// What a probe found: the row, and whether the bytes settled it outright.
const Hit = struct { row: i32, same_bytes: bool };

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

    /// The row parser, when rows are text and there is one.
    fn parser(self: Side) ?text.RowParser {
        return switch (self.rows) {
            .text => |t| t.parser,
            .columnar => null,
        };
    }

    /// The key columns of that row, and nothing else.
    ///
    /// The whole row is eighteen more fields than a key comparison reads, and a
    /// parser stops at the last column it was asked for -- so asking for the two
    /// key columns turns the rest of the row into one scan for the newline
    /// instead of eighteen field boundaries packed into words nobody looks at.
    /// This is the saving the sweep already takes, in the half of the engine
    /// that only ever wanted a key.
    ///
    /// Fills `out[0..key_size]` and leaves the rest of the buffer as it was:
    /// every caller reads the key columns alone.
    fn keysAt(self: Side, at: u64, key_size: usize, out: []Field) void {
        switch (self.rows) {
            .text => |t| _ = t.keys.parse(self.slab.data, @intCast(at), self.slab.data.len, out),
            .columnar => |c| {
                const from = @as(usize, @intCast(at)) * self.width;
                @memcpy(out[0..key_size], c.fields[from..][0..key_size]);
            },
        }
    }

    fn deinit(self: *Side) void {
        switch (self.rows) {
            .text => |t| {
                t.parser.deinit(self.gpa);
                t.keys.deinit(self.gpa);
            },
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
        key_size: usize,
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
                        .keys = try text.RowParser.initJson(gpa, keys[0..@min(key_size, width)]),
                        .from = t.from,
                    } };
                } else {
                    const source = try gpa.alloc(?usize, width);
                    errdefer gpa.free(source);
                    for (wanted, 0..) |n, i| source[i] = indexOf(t.names, n);
                    side.wanted_source = source;
                    side.rows = .{ .text = .{
                        .parser = try text.RowParser.initCsv(gpa, t.delimiter, source),
                        .keys = try text.RowParser.initCsv(
                            gpa,
                            t.delimiter,
                            source[0..@min(key_size, width)],
                        ),
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

/// A slot holds the top bits of its key's hash and the position in `first_row`
/// plus one, so zero means empty.
///
/// Carrying the tag is what makes a failed probe cheap: the word already loaded
/// settles it. A table of bare positions has to follow each one into `first_row`
/// and then into `row_hash` — two dependent random loads, over arrays far too big
/// to cache at ten million keys — only to reject it.
const POS_MASK: u64 = (1 << 40) - 1;
const TAG_MASK: u64 = ~POS_MASK;
const EMPTY_SLOT: u64 = 0;

fn slotFor(hash: u64, pos: usize) u64 {
    return (hash & TAG_MASK) | (@as(u64, pos) + 1);
}

fn tagIs(slot: u64, hash: u64) bool {
    return (slot ^ hash) & TAG_MASK == 0;
}

fn posOf(slot: u64) usize {
    return @intCast((slot & POS_MASK) - 1);
}

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
    table: []u64,
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
        tag: []const u8,
    ) !RowIndex {
        var phases = Phases.start(tag);
        const chunks = try sweep(gpa, side, key_size, opt, threads);
        phases.mark("sweep (parallel)");
        defer {
            for (chunks) |*c| c.deinit(gpa);
            gpa.free(chunks);
        }

        var total: usize = 0;
        for (chunks) |c| total += c.at.items.len;
        // Sized once for the rows about to be inserted, at about a two-thirds
        // load. Starting at four thousand and doubling meant twelve rehashes at
        // ten million rows, each one a full random-access pass over a table
        // already too big to cache — work that grows with the file and is
        // entirely avoidable, since the row count is known before the first
        // insert.
        //
        // Two thirds and not a half: sizing for a half load also removes the
        // rehash, but the table it leaves is twice as big, and at ten million
        // keys the misses that costs are worth more than the probes it saves.
        var cap: usize = 1 << 12;
        while (cap * 2 < total * 3 + 16) cap <<= 1;
        const table = try gpa.alloc(u64, cap);
        @memset(table, EMPTY_SLOT);
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

        try self.row_at.ensureTotalCapacity(gpa, total);
        try self.row_hash.ensureTotalCapacity(gpa, total);
        // One entry per distinct key, and every key is distinct until proven
        // otherwise.
        try self.first_row.ensureTotalCapacity(gpa, total);
        try self.occurrences.ensureTotalCapacity(gpa, total);

        var s = Scratch{};
        // Each chunk is released as soon as it has been inserted. Holding all of
        // them to the end would keep two copies of every row's address and hash
        // alive at once, which is sixteen bytes a row of pure duplication.
        for (chunks) |*chunk| {
            const hashes = chunk.hash.items;
            for (chunk.at.items, hashes, 0..) |at, hash, i| {
                if (i + PREFETCH_AHEAD < hashes.len) self.prefetch(hashes[i + PREFETCH_AHEAD]);
                try self.insert(at, hash, &s);
            }
            // Emptied rather than only released, because the caller frees the
            // chunks too and a list deinitialised twice frees a pointer it no
            // longer owns.
            chunk.deinit(gpa);
            chunk.* = .{};
        }
        phases.mark("index insert (serial)");
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
            const word = self.table[slot];
            if (word == EMPTY_SLOT) {
                self.table[slot] = slotFor(hash, self.first_row.items.len);
                try self.first_row.append(self.gpa, row);
                try self.occurrences.append(self.gpa, 1);
                // Two thirds, which is what `build` sizes the table for.
                if (self.first_row.items.len * 3 > self.table.len * 2) try self.rehash();
                return;
            }
            // The tag rejects almost every collision without leaving this word.
            if (tagIs(word, hash)) {
                const key = posOf(word);
                const candidate = self.first_row.items[key];
                if (self.row_hash.items[@intCast(candidate)] == hash) {
                    // This row's fields are re-read rather than carried over from
                    // the sweep because the sweep produced ten million of them and
                    // this branch wants one.
                    if (!mine_parsed) {
                        self.side.keysAt(at, self.key_size, self.mine);
                        mine_parsed = true;
                    }
                    self.keysOf(candidate, self.probe);
                    var ok = true;
                    for (0..self.key_size) |i| {
                        if (!(try same(self.side.slab, self.probe[i], self.side.slab, self.mine[i], self.opt, s))) {
                            ok = false;
                            break;
                        }
                    }
                    if (ok) {
                        self.occurrences.items[key] += 1;
                        if (self.occurrences.items[key] == 2) {
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

    fn fieldsOf(self: RowIndex, row: i32, out: []Field) void {
        self.side.fieldsAt(self.row_at.items[@intCast(row)], out);
    }

    fn keysOf(self: RowIndex, row: i32, out: []Field) void {
        self.side.keysAt(self.row_at.items[@intCast(row)], self.key_size, out);
    }

    /// The high bits of an FNV hash are the well-mixed ones; fold them down.
    fn slotOf(self: RowIndex, hash: u64) usize {
        return @as(usize, @intCast((hash ^ (hash >> 32)) & 0xffff_ffff)) & self.mask;
    }

    /// Starts the fetch of the slot `hash` will land in, without waiting for it.
    ///
    /// Every probe of this table is a random access into tens of megabytes, so it
    /// misses to memory, and the row after it needs a different line: the loop
    /// spends most of its time waiting on a load whose address was known long
    /// before it was issued. Asking for the line `PREFETCH_AHEAD` rows early turns
    /// that serial chain of misses into overlapping ones.
    fn prefetch(self: *const RowIndex, hash: u64) void {
        @prefetch(&self.table[self.slotOf(hash)], .{ .rw = .read, .locality = 3, .cache = .data });
    }

    /// Only reached if the row count was underestimated: `build` sizes the table
    /// for the rows it is about to insert, so the common path never grows it.
    fn rehash(self: *RowIndex) !void {
        const table = try self.gpa.alloc(u64, self.table.len * 2);
        @memset(table, EMPTY_SLOT);
        self.gpa.free(self.table);
        self.table = table;
        self.mask = table.len - 1;
        for (self.first_row.items, 0..) |row, key| {
            const hash = self.row_hash.items[@intCast(row)];
            var slot = self.slotOf(hash);
            while (self.table[slot] != EMPTY_SLOT) slot = (slot + 1) & self.mask;
            self.table[slot] = slotFor(hash, key);
        }
    }

    /// The row carrying `fields`' key, or null. `other` is the slab those fields
    /// live in, which is the opposite file when this is a join probe. `probe` is
    /// scratch the caller owns: the join runs several chunks at once, and a buffer
    /// hanging off the index would be shared between them.
    ///
    /// On a hit, `probe` holds as much of that row as `want` asked for — it is
    /// what the key columns were compared against. The join used to read the row
    /// again on the line after this one returned, which is a second parse of
    /// every matched row in the file.
    ///
    /// `want` is `.whole` for the side that goes on to compare the columns, and
    /// `.keys` for the side that only asks whether the key exists at all and
    /// throws the answer away.
    fn lookup(
        self: *const RowIndex,
        other: Slab,
        fields: []const Field,
        hash: u64,
        want: Want,
        span: ?Span,
        s: *Scratch,
        probe: []Field,
    ) !?Hit {
        var slot = self.slotOf(hash);
        while (true) {
            const word = self.table[slot];
            if (word == EMPTY_SLOT) return null;
            // The tag rejects almost every collision without leaving this word.
            if (tagIs(word, hash)) {
                const candidate = self.first_row.items[posOf(word)];
                if (self.row_hash.items[@intCast(candidate)] == hash) {
                    // The bytes first, where the caller offered them. A candidate
                    // whose row opens with the same bytes as far as either file
                    // reads has the same keys and the same columns, and neither
                    // row needs parsing to say so. A candidate the tag let
                    // through with a different key fails this on its first few
                    // bytes, so the cost of being wrong is a handful of them.
                    if (span) |sp| {
                        if (self.rowMatches(candidate, sp)) {
                            return .{ .row = candidate, .same_bytes = true };
                        }
                    }
                    switch (want) {
                        .whole => self.fieldsOf(candidate, probe),
                        .keys => self.keysOf(candidate, probe),
                    }
                    var ok = true;
                    for (0..self.key_size) |i| {
                        if (!(try same(self.side.slab, probe[i], other, fields[i], self.opt, s))) {
                            ok = false;
                            break;
                        }
                    }
                    if (ok) return .{ .row = candidate, .same_bytes = false };
                }
            }
            slot = (slot + 1) & self.mask;
        }
    }

    /// Whether `candidate`'s row opens with exactly `sp.bytes` and ends that run
    /// on a field boundary.
    ///
    /// The boundary is what makes the run a whole number of columns rather than
    /// a truncation of one: without it `12,3` would match a row opening `12,34`.
    fn rowMatches(self: *const RowIndex, candidate: i32, sp: Span) bool {
        const data = self.side.slab.data;
        const from: usize = @intCast(self.row_at.items[@intCast(candidate)]);
        // Both tests, and in this order: `data.len - sp.bytes.len` underflows for
        // a run longer than this whole file, which a release build would wrap
        // rather than trap. A's rows and B's bytes come from different files and
        // nothing bounds one by the other.
        if (sp.bytes.len > data.len or from > data.len - sp.bytes.len) return false;
        const to = from + sp.bytes.len;
        if (to < data.len and data[to] != sp.delimiter and data[to] != '\n') return false;
        return std.mem.eql(u8, data[from..to], sp.bytes);
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
        // The sweep parses with the key-only parser, so it needs room for the
        // key columns; the full-width buffer above is what a row over the field
        // cap is re-read into, and what the columnar branch fills.
        const keys = try gpa.alloc(Field, @max(1, self.key_size));
        defer gpa.free(keys);
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
                    const next = t.keys.parse(data, pos, data.len, keys);
                    // A field cannot be longer than the row that holds it, so
                    // only a row over the cap can hide an over-long column the
                    // key parser never looked at. That row, and only that row, is
                    // re-read in full to find it. At a hundred and eighty bytes a
                    // row this is one comparison and never taken; the check it
                    // replaces walked twenty fields of every row of the file.
                    for (keys) |field| if (field == TOO_LONG) return Error.FieldTooLong;
                    if (next -| pos > fld.MAX_FIELD_LEN) {
                        _ = t.parser.parse(data, pos, data.len, fields);
                        for (fields) |field| if (field == TOO_LONG) return Error.FieldTooLong;
                    }
                    try chunk.at.append(gpa, pos);
                    try chunk.hash.append(
                        gpa,
                        try keyHash(self.side.slab, keys, self.key_size, self.opt, &s.a),
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
pub fn runOnThreads(state: anytype, comptime entry: anytype, ways: usize) !void {
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

/// One chunk of A's keys: the matched, changed and removed counts and the column
/// stats. The merge is a sum, so the answer does not depend on how many chunks
/// there are or on which thread took which.
///
/// There are no chunks of B. `added` is not counted by a pass any more; see the
/// join's caller.
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
    /// Where a byte comparison may stand in for a parse, when the two files are
    /// the same shape; see `text.sharedTail`. Null compares every pair column by
    /// column, which is what a mixed pair, a columnar side or JSON gets.
    span_tail: ?text.Tail,
    next: std.atomic.Value(usize),

    fn run(self: *Join) void {
        while (true) {
            const i = self.next.fetchAdd(1, .monotonic);
            if (i >= self.parts.len) return;
            const work = self.range(i);
            work catch |e| {
                self.parts[i].failure = e;
                return;
            };
        }
    }

    fn range(self: *Join, p: usize) !void {
        const gpa = self.a.gpa;
        const fa = try gpa.alloc(Field, self.width);
        defer gpa.free(fa);
        // The lookup's scratch, and on a hit it already holds the mate's fields:
        // that is what the key columns were matched against.
        const fb = try gpa.alloc(Field, self.width);
        defer gpa.free(fb);
        var s = Scratch{};
        var out = &self.parts[p];

        const keys = self.ai.first_row.items;
        const lo = keys.len * p / self.parts.len;
        const hi = keys.len * (p + 1) / self.parts.len;
        // Nothing to normalise and no tolerance: the cell comparison cannot
        // fail, so it need not be asked through an error union.
        const plain = !needsNormalising(self.opt);
        const mine = keys[lo..hi];
        for (mine, 0..) |row, at| {
            if (at + PREFETCH_AHEAD < mine.len) {
                self.bi.prefetch(self.ai.row_hash.items[@intCast(mine[at + PREFETCH_AHEAD])]);
            }
            self.ai.fieldsOf(row, fa);
            // The hash is the one the sweep computed for this row: the same
            // bytes through the same function, so computing it again here would
            // be a second pass over every key in the file for the same number.
            const hash = self.ai.row_hash.items[@intCast(row)];
            // A's row up to the end of the last column either file wants. The
            // end has to be a boundary in A as well: a quoted field ends on its
            // closing quote, and what follows is not part of the run.
            const span: ?Span = blk: {
                const tail = self.span_tail orelse break :blk null;
                const f = fa[tail.slot];
                if (!fld.isReal(f)) break :blk null;
                const data = self.a.slab.data;
                const from: usize = @intCast(self.ai.row_at.items[@intCast(row)]);
                const to = fld.offsetOf(f) + fld.lenOf(f);
                if (to < from or to > data.len) break :blk null;
                if (to < data.len and data[to] != tail.delimiter and data[to] != '\n') {
                    break :blk null;
                }
                break :blk .{ .bytes = data[from..to], .delimiter = tail.delimiter };
            };
            const hit = (try self.bi.lookup(self.a.slab, fa, hash, .whole, span, &s, fb)) orelse {
                out.removed += 1;
                continue;
            };
            out.matched += 1;
            // The two rows carry the same bytes across every column either file
            // wants, so no column differs and the mate was never read.
            if (hit.same_bytes) continue;
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

/// One file's whole preparation — decode or map it, then index it — so that both
/// files can be done at once.
const Prepare = struct {
    gpa: std.mem.Allocator,
    input: Input,
    wanted: []const []const u8,
    key_size: usize,
    opt: Options,
    threads: usize,
    tag: []const u8,
    side: ?Side = null,
    index: ?RowIndex = null,
    failure: ?anyerror = null,

    fn run(self: *Prepare) void {
        self.go() catch |e| {
            self.failure = e;
        };
    }

    fn go(self: *Prepare) !void {
        self.side = try self.input.project(self.gpa, self.wanted, self.key_size, self.threads);
        self.index = try RowIndex.build(
            self.gpa,
            &self.side.?,
            self.key_size,
            self.opt,
            self.threads,
            self.tag,
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
        .tag = "A ",
    };
    var prepare_b = Prepare{
        .gpa = gpa,
        .input = b_input,
        .wanted = wanted.items,
        .key_size = key_size,
        .opt = opt,
        .threads = per_file,
        .tag = "B ",
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

    // A's keys, chunked into one queue every thread pulls from. B has no pass of
    // its own any more: see `counts.added` below.
    const a_ways = waysFor(ai.first_row.items.len);
    const parts = try gpa.alloc(Part, a_ways);
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
        .span_tail = if (a.parser()) |pa| (if (b.parser()) |pb| text.sharedTail(pa, pb) else null) else null,
        .next = std.atomic.Value(usize).init(0),
    };
    var phases = Phases.start("");
    try runOnThreads(&join, Join.run, total);
    phases.mark("join chunks (par)");
    for (parts) |part| {
        if (part.failure) |e| return e;
    }

    var counts = Counts{};
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
    // `added` is arithmetic, not a pass.
    //
    // B's half of the join used to ask "is this key in A?" for every key in B --
    // ten million probes to find the ten thousand rows A never had -- while A's
    // half had already asked the same question of the same pairs from the other
    // end. It does not need asking twice: every distinct key of A finds at most
    // one distinct key of B, two distinct keys of A cannot find the same key of
    // B, and the key comparison is symmetric, so the keys of B that nothing
    // matched are simply the ones the A pass did not account for.
    //
    // This port can stop there because it reports counts and column stats and
    // never names an added row. The Rust port renders those rows, so it keeps a
    // bitmap of B's matched rows instead and walks that for the sample; the C++
    // port takes this same subtraction and runs a pass only for `--json`.
    counts.added = counts.b_keys - counts.matched;
    counts.unchanged = counts.matched - counts.changed;
    counts.a_dup_keys = ai.dup_keys;
    counts.a_dup_rows = ai.dup_rows;
    counts.b_dup_keys = bi.dup_keys;
    counts.b_dup_rows = bi.dup_rows;

    return .{ .counts = counts, .columns = columns };
}
