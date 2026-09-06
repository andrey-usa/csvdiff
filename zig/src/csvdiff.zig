//! A composite-key CSV comparison, byte-level, with the memory it may use
//! passed in rather than assumed.
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
//! measurement and becomes a property the type system enforces.

const std = @import("std");
const scan = @import("scan.zig");

pub const Error = error{
    KeyColumnMissing,
    ComparedColumnMissing,
    NoHeaderRow,
    FieldTooLong,
    CannotReadFile,
    NonAsciiCaseFold,
};

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

// ---------------------------------------------------------------------------
// A field packed into one word: offset, length, and whether it needs unescaping.
//
// 40 bits of offset addresses a terabyte and 23 bits of length a field of eight
// megabytes. An over-long field is reported rather than truncated through the
// mask.
// ---------------------------------------------------------------------------

const Field = u64;
const ABSENT: Field = std.math.maxInt(u64);
const TOO_LONG: Field = std.math.maxInt(u64) - 1;
const OFFSET_MASK: u64 = (1 << 40) - 1;
const LENGTH_SHIFT: u6 = 40;
const LENGTH_MASK: u64 = (1 << 23) - 1;
const ESCAPED: u64 = 1 << 63;
const MAX_FIELD_LEN: u64 = LENGTH_MASK;

fn pack(offset: usize, len: usize, escaped: bool) Field {
    if (len > MAX_FIELD_LEN) return TOO_LONG;
    return (@as(u64, offset) & OFFSET_MASK) |
        ((@as(u64, len) & LENGTH_MASK) << LENGTH_SHIFT) |
        (if (escaped) ESCAPED else 0);
}

fn offsetOf(f: Field) usize {
    return @intCast(f & OFFSET_MASK);
}

fn lenOf(f: Field) usize {
    return @intCast((f >> LENGTH_SHIFT) & LENGTH_MASK);
}

fn isEscaped(f: Field) bool {
    return (f & ESCAPED) != 0;
}

fn isReal(f: Field) bool {
    return f != ABSENT and f != TOO_LONG;
}

// ---------------------------------------------------------------------------
// The mapped file
// ---------------------------------------------------------------------------

pub const Slab = struct {
    data: []align(std.heap.page_size_min) const u8,

    pub fn open(io: std.Io, path: []const u8) !Slab {
        const file = std.Io.Dir.cwd().openFile(io, path, .{}) catch return Error.CannotReadFile;
        defer file.close(io);
        const size = (file.stat(io) catch return Error.CannotReadFile).size;
        if (size == 0) return Slab{ .data = &[_]u8{} };
        const mapped = std.posix.mmap(
            null,
            size,
            .{ .READ = true },
            .{ .TYPE = .PRIVATE },
            file.handle,
            0,
        ) catch return Error.CannotReadFile;
        return Slab{ .data = mapped };
    }

    pub fn close(self: *Slab) void {
        if (self.data.len > 0) std.posix.munmap(self.data);
    }

    /// The field's raw span, still holding any doubled quotes.
    fn raw(self: Slab, f: Field) []const u8 {
        if (!isReal(f)) return &[_]u8{};
        return self.data[offsetOf(f)..][0..lenOf(f)];
    }
};

/// A field's logical bytes: the raw span with the second quote of each doubled
/// pair dropped. Equality, hashing and decoding all read a field through this
/// one route, so they cannot disagree about its value — the property whose
/// absence produced two silently wrong answers in the Java port.
const Logical = struct {
    raw: []const u8,
    escaped: bool,
    at: usize = 0,

    fn next(self: *Logical) ?u8 {
        if (self.at >= self.raw.len) return null;
        const b = self.raw[self.at];
        self.at += 1;
        if (self.escaped and b == '"' and self.at < self.raw.len and self.raw[self.at] == '"') {
            self.at += 1;
        }
        return b;
    }
};

fn logical(slab: Slab, f: Field) Logical {
    return .{ .raw = slab.raw(f), .escaped = isReal(f) and isEscaped(f) };
}

fn sameBytes(a: Slab, x: Field, b: Slab, y: Field) bool {
    const ex = isReal(x) and isEscaped(x);
    const ey = isReal(y) and isEscaped(y);
    if (!ex and !ey) return std.mem.eql(u8, a.raw(x), b.raw(y));
    var lx = logical(a, x);
    var ly = logical(b, y);
    while (true) {
        const cx = lx.next();
        const cy = ly.next();
        if (cx == null and cy == null) return true;
        if (cx == null or cy == null) return false;
        if (cx.? != cy.?) return false;
    }
}

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
/// Only the normalising paths and the report call this; the default path never
/// builds anything for a cell at all.
fn normalised(slab: Slab, f: Field, o: Options, buf: []u8) Error!?[]const u8 {
    if (!isReal(f)) return null;
    var n: usize = 0;
    var it = logical(slab, f);
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
    if (!isReal(f) or lenOf(f) == 0) return true;
    if (!needsNormalising(o)) return false;
    return (try normalised(slab, f, o, buf)) == null;
}

fn same(a: Slab, x: Field, b: Slab, y: Field, o: Options, s: *Scratch) Error!bool {
    const xa = try isAbsent(a, x, o, &s.a);
    const yb = try isAbsent(b, y, o, &s.b);
    if (xa or yb) return xa and yb;
    if (!needsNormalising(o)) return sameBytes(a, x, b, y);
    const nx = (try normalised(a, x, o, &s.a)) orelse "";
    const ny = (try normalised(b, y, o, &s.b)) orelse "";
    return std.mem.eql(u8, nx, ny);
}

/// Deliberately stricter than a plain float parse: "inf" and "nan" are ordinary
/// text in a CSV, and treating them as numbers would make two unequal strings
/// compare equal under a tolerance.
fn asNumber(text: []const u8) ?f64 {
    var s = text;
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
        var it = logical(slab, f);
        while (it.next()) |b| {
            h = (h ^ b) *% PRIME;
            len += 1;
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
// Parsing
// ---------------------------------------------------------------------------

/// Splits rows into fields, projecting straight to the columns asked for. Once
/// the last needed column has been read the rest of the row is skipped to its
/// newline without its fields ever being delimited: on twenty columns keyed on
/// the first two, most of a row is never looked at.
const RowParser = struct {
    delimiter: u8,
    /// Where each projected column sits in the file, or null when absent.
    source: []const ?usize,
    last_needed: usize,

    fn init(delimiter: u8, source: []const ?usize) RowParser {
        var last: usize = 0;
        for (source) |s| if (s) |c| {
            if (c > last) last = c;
        };
        return .{ .delimiter = delimiter, .source = source, .last_needed = last };
    }

    /// Parses one row into `out`, returning the offset of the next row. A row
    /// shorter than the header leaves the missing fields absent, which is a
    /// difference to report rather than a file to refuse.
    fn parse(self: RowParser, d: []const u8, start: usize, end: usize, out: []Field) usize {
        @memset(out, ABSENT);
        var pos = start;
        var column: usize = 0;

        while (pos <= end) {
            var field: Field = undefined;
            var next: usize = undefined;
            if (pos < end and d[pos] == '"') {
                const close = scan.skipQuoted(d, pos + 1, end);
                const body_end = if (close > pos + 1) close - 1 else pos + 1;
                next = scan.nextOf2(d, close, end, self.delimiter, '\n');
                const escaped = scan.nextOf1(d, pos + 1, body_end, '"') < body_end;
                field = pack(pos + 1, body_end - (pos + 1), escaped);
            } else {
                next = scan.nextOf2(d, pos, end, self.delimiter, '\n');
                var stop = next;
                if (stop > pos and d[stop - 1] == '\r') stop -= 1; // CRLF behaves like LF
                field = pack(pos, stop - pos, false);
            }
            self.store(column, field, out);
            column += 1;

            if (next >= end) return end;
            if (d[next] == '\n') return next + 1;
            pos = next + 1;
            if (column > self.last_needed) {
                const eol = endOfRow(d, pos, end);
                return if (eol >= end) end else eol + 1;
            }
        }
        return end;
    }

    fn store(self: RowParser, column: usize, f: Field, out: []Field) void {
        if (column > self.last_needed) return;
        for (self.source, 0..) |s, i| {
            if (s != null and s.? == column) out[i] = f;
        }
    }
};

fn endOfRow(d: []const u8, pos: usize, end: usize) usize {
    var at = pos;
    while (at < end) {
        const next = scan.nextOf2(d, at, end, '\n', '"');
        if (next >= end) return end;
        if (d[next] == '"') {
            at = scan.skipQuoted(d, next + 1, end);
            continue;
        }
        return next;
    }
    return end;
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

const EMPTY: i32 = -1;

/// Open addressing over one file's rows. Everything is a primitive array taken
/// from the caller's allocator, so an index that would not fit in the budget
/// fails here rather than growing until the kernel intervenes.
const RowIndex = struct {
    gpa: std.mem.Allocator,
    slab: Slab,
    parser: RowParser,
    key_size: usize,
    opt: Options,
    row_start: std.ArrayList(u64),
    row_hash: std.ArrayList(u64),
    table: []i32,
    mask: usize,
    first_row: std.ArrayList(i32),
    occurrences: std.ArrayList(u32),
    probe: []Field,
    rows: i64 = 0,
    dup_keys: i64 = 0,
    dup_rows: i64 = 0,

    fn build(
        gpa: std.mem.Allocator,
        slab: Slab,
        parser: RowParser,
        from: usize,
        width: usize,
        key_size: usize,
        opt: Options,
    ) !RowIndex {
        const table = try gpa.alloc(i32, 1 << 12);
        @memset(table, EMPTY);
        var self = RowIndex{
            .gpa = gpa,
            .slab = slab,
            .parser = parser,
            .key_size = key_size,
            .opt = opt,
            .row_start = .empty,
            .row_hash = .empty,
            .table = table,
            .mask = table.len - 1,
            .first_row = .empty,
            .occurrences = .empty,
            .probe = try gpa.alloc(Field, width),
        };
        errdefer self.deinit();

        const d = slab.data;
        const end = d.len;
        const fields = try gpa.alloc(Field, width);
        defer gpa.free(fields);
        var s = Scratch{};

        var pos = from;
        while (pos < end) {
            // A line with nothing on it is not a row.
            if (d[pos] == '\n') {
                pos += 1;
                continue;
            }
            if (d[pos] == '\r' and pos + 1 < end and d[pos + 1] == '\n') {
                pos += 2;
                continue;
            }
            const next = parser.parse(d, pos, end, fields);
            for (fields) |f| if (f == TOO_LONG) return Error.FieldTooLong;
            try self.add(pos, fields, &s);
            if (next <= pos) break; // no progress: a malformed tail, not an endless loop
            pos = next;
        }
        return self;
    }

    fn deinit(self: *RowIndex) void {
        self.row_start.deinit(self.gpa);
        self.row_hash.deinit(self.gpa);
        self.first_row.deinit(self.gpa);
        self.occurrences.deinit(self.gpa);
        self.gpa.free(self.table);
        self.gpa.free(self.probe);
    }

    fn add(self: *RowIndex, start: usize, fields: []const Field, s: *Scratch) !void {
        self.rows += 1;
        const row: i32 = @intCast(self.row_start.items.len);
        const hash = try keyHash(self.slab, fields, self.key_size, self.opt, &s.a);
        try self.row_start.append(self.gpa, start);
        try self.row_hash.append(self.gpa, hash);

        var slot = self.slotOf(hash);
        while (true) {
            const at = self.table[slot];
            if (at == EMPTY) {
                self.table[slot] = @intCast(self.first_row.items.len);
                try self.first_row.append(self.gpa, row);
                try self.occurrences.append(self.gpa, 1);
                if (self.first_row.items.len * 2 > self.table.len) try self.rehash();
                return;
            }
            const candidate = self.first_row.items[@intCast(at)];
            if (self.row_hash.items[@intCast(candidate)] == hash and
                try self.sameKey(candidate, fields, s))
            {
                self.occurrences.items[@intCast(at)] += 1;
                if (self.occurrences.items[@intCast(at)] == 2) {
                    self.dup_keys += 1;
                    self.dup_rows += 1; // the first occurrence counts once the key repeats
                }
                self.dup_rows += 1;
                return;
            }
            slot = (slot + 1) & self.mask;
        }
    }

    fn sameKey(self: *RowIndex, candidate: i32, fields: []const Field, s: *Scratch) !bool {
        self.fieldsOf(candidate, self.probe);
        for (0..self.key_size) |i| {
            if (!(try same(self.slab, self.probe[i], self.slab, fields[i], self.opt, s))) return false;
        }
        return true;
    }

    /// Re-parses a row. The index stores where a row starts rather than its
    /// fields, because an offset is eight bytes where the fields would be twenty
    /// times that; re-parsing is cheap because the parser stops early.
    fn fieldsOf(self: RowIndex, row: i32, out: []Field) void {
        const start: usize = @intCast(self.row_start.items[@intCast(row)]);
        _ = self.parser.parse(self.slab.data, start, self.slab.data.len, out);
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
    /// live in, which is the opposite file when this is a join probe.
    fn lookup(self: *RowIndex, other: Slab, fields: []const Field, hash: u64, s: *Scratch) !?i32 {
        var slot = self.slotOf(hash);
        while (true) {
            const at = self.table[slot];
            if (at == EMPTY) return null;
            const candidate = self.first_row.items[@intCast(at)];
            if (self.row_hash.items[@intCast(candidate)] == hash) {
                self.fieldsOf(candidate, self.probe);
                var ok = true;
                for (0..self.key_size) |i| {
                    if (!(try same(self.slab, self.probe[i], other, fields[i], self.opt, s))) {
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
// Columns
// ---------------------------------------------------------------------------

fn detectDelimiter(header: []const u8) u8 {
    var best: u8 = ',';
    var best_count: isize = -1;
    for ([_]u8{ ',', ';', '\t', '|' }) |c| {
        var n: isize = 0;
        for (header) |b| {
            if (b == c) n += 1;
        }
        if (n > best_count) {
            best = c;
            best_count = n;
        }
    }
    return best;
}

fn contains(haystack: []const []const u8, needle: []const u8) bool {
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

/// The header row's names, and where the first data row starts.
fn readHeader(gpa: std.mem.Allocator, slab: Slab, delimiter: u8) !struct { names: [][]const u8, start: usize } {
    const d = slab.data;
    if (d.len == 0) return Error.NoHeaderRow;
    var names: std.ArrayList([]const u8) = .empty;
    errdefer names.deinit(gpa);
    var pos: usize = 0;
    while (true) {
        var field: Field = undefined;
        var next: usize = undefined;
        if (pos < d.len and d[pos] == '"') {
            const close = scan.skipQuoted(d, pos + 1, d.len);
            const body_end = if (close > pos + 1) close - 1 else pos + 1;
            next = scan.nextOf2(d, close, d.len, delimiter, '\n');
            field = pack(pos + 1, body_end - (pos + 1), scan.nextOf1(d, pos + 1, body_end, '"') < body_end);
        } else {
            next = scan.nextOf2(d, pos, d.len, delimiter, '\n');
            var stop = next;
            if (stop > pos and d[stop - 1] == '\r') stop -= 1;
            field = pack(pos, stop - pos, false);
        }
        // A header name is one of the few strings this engine does own; there is
        // one per column, not one per cell.
        var buf: std.ArrayList(u8) = .empty;
        var it = logical(slab, field);
        while (it.next()) |b| try buf.append(gpa, b);
        try names.append(gpa, try buf.toOwnedSlice(gpa));

        if (next >= d.len) return .{ .names = try names.toOwnedSlice(gpa), .start = d.len };
        if (d[next] == '\n') return .{ .names = try names.toOwnedSlice(gpa), .start = next + 1 };
        pos = next + 1;
    }
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

    /// The column names are owned copies, so the header they were read from can
    /// be released as soon as the comparison is done with it.
    pub fn deinit(self: *Result, gpa: std.mem.Allocator) void {
        for (self.columns) |c| gpa.free(c.name);
        gpa.free(self.columns);
    }
};

/// Releases a header read by `readHeader`.
fn freeHeader(gpa: std.mem.Allocator, names: [][]const u8) void {
    for (names) |n| gpa.free(n);
    gpa.free(names);
}

/// Compares two files, taking every byte it needs from `gpa`.
///
/// Hand it a `FixedBufferAllocator` and the comparison cannot exceed that
/// buffer: it returns `error.OutOfMemory` at the allocation that would have
/// crossed the line. That is the whole reason this port is in Zig.
pub fn compare(
    io: std.Io,
    gpa: std.mem.Allocator,
    a_path: []const u8,
    b_path: []const u8,
    opt: Options,
) !Result {
    var a = try Slab.open(io, a_path);
    defer a.close();
    var b = try Slab.open(io, b_path);
    defer b.close();

    const a_delim = opt.delimiter orelse detectDelimiter(a.data[0..scan.nextOf1(a.data, 0, a.data.len, '\n')]);
    const b_delim = opt.delimiter orelse detectDelimiter(b.data[0..scan.nextOf1(b.data, 0, b.data.len, '\n')]);

    const a_head = try readHeader(gpa, a, a_delim);
    defer freeHeader(gpa, a_head.names);
    const b_head = try readHeader(gpa, b, b_delim);
    defer freeHeader(gpa, b_head.names);

    for (opt.key) |k| {
        if (!contains(a_head.names, k) or !contains(b_head.names, k)) return Error.KeyColumnMissing;
    }

    var compared: std.ArrayList([]const u8) = .empty;
    defer compared.deinit(gpa);
    if (opt.compare.len > 0) {
        for (opt.compare) |c| {
            if (!contains(a_head.names, c) or !contains(b_head.names, c)) return Error.ComparedColumnMissing;
            try compared.append(gpa, c);
        }
    } else {
        for (a_head.names) |c| {
            if (contains(b_head.names, c) and !contains(opt.key, c) and !contains(opt.ignore, c))
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

    const a_src = try gpa.alloc(?usize, width);
    defer gpa.free(a_src);
    const b_src = try gpa.alloc(?usize, width);
    defer gpa.free(b_src);
    for (wanted.items, 0..) |n, i| {
        a_src[i] = indexOf(a_head.names, n);
        b_src[i] = indexOf(b_head.names, n);
    }

    const ap = RowParser.init(a_delim, a_src);
    const bp = RowParser.init(b_delim, b_src);

    var ai = try RowIndex.build(gpa, a, ap, a_head.start, width, key_size, opt);
    defer ai.deinit();
    var bi = try RowIndex.build(gpa, b, bp, b_head.start, width, key_size, opt);
    defer bi.deinit();

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

    const fa = try gpa.alloc(Field, width);
    defer gpa.free(fa);
    const fb = try gpa.alloc(Field, width);
    defer gpa.free(fb);
    var s = Scratch{};

    var counts = Counts{};
    // A's distinct keys, in first-appearance order, so a run is reproducible.
    for (ai.first_row.items) |row| {
        ai.fieldsOf(row, fa);
        const hash = try keyHash(a, fa, key_size, opt, &s.a);
        const mate = try bi.lookup(a, fa, hash, &s) orelse {
            counts.removed += 1;
            continue;
        };
        counts.matched += 1;
        bi.fieldsOf(mate, fb);
        var any = false;
        for (0..nc) |i| {
            const x = fa[key_size + i];
            const y = fb[key_size + i];
            if (try cellDiffers(a, x, b, y, opt, &s)) {
                any = true;
                columns[i].changed += 1;
                if (try isAbsent(b, y, opt, &s.b)) columns[i].blanked += 1;
                if (try isAbsent(a, x, opt, &s.a)) columns[i].filled += 1;
            }
        }
        if (any) counts.changed += 1;
    }
    for (bi.first_row.items) |row| {
        bi.fieldsOf(row, fb);
        const hash = try keyHash(b, fb, key_size, opt, &s.b);
        if (try ai.lookup(b, fb, hash, &s) == null) counts.added += 1;
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
