//! The two text readers: CSV, and newline-delimited JSON.
//!
//! They are one module because they are one design. A JSON value is a contiguous
//! run of bytes in the file exactly as a CSV field is, so a field stays an offset
//! and a length for both and nothing downstream has to know which format it came
//! from. What differs is how a row is walked — by column number, or by key — and
//! how a value is escaped, which `slab.zig` settles.
//!
//! The two sides of a comparison need not agree: a CSV export compares against
//! the JSON the same pipeline emits, with the key order on each side free to
//! differ, because the JSON reader joins by name.

const std = @import("std");
const scan = @import("scan.zig");
const f = @import("field.zig");
const Field = f.Field;
const Slab = @import("slab.zig").Slab;
const Dialect = @import("slab.zig").Dialect;

pub const Error = error{ NoHeaderRow, NoJsonObject };

fn jsonSpace(c: u8) bool {
    return c == ' ' or c == '\t' or c == '\r' or c == '\n';
}

/// Newline-delimited JSON if the first thing that is not whitespace is a brace.
///
/// A CSV header can begin with anything else, and a `{` in the first column of a
/// CSV header is not something this project has ever had to read.
pub fn sniffDialect(data: []const u8) Dialect {
    for (data[0..@min(data.len, 64)]) |b| {
        if (jsonSpace(b)) continue;
        return if (b == '{') .json else .csv;
    }
    return .csv;
}

/// Guesses the delimiter from the header line, defaulting to a comma.
pub fn detectDelimiter(header: []const u8) u8 {
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

/// A quoted CSV field, flagged when it holds a doubled quote. A quote inside the
/// body can only be half of such a pair, which is what makes the test a single
/// scan and lets the unescaping wait until the bytes are actually read.
fn quotedField(d: []const u8, from: usize, to: usize) Field {
    return f.pack(from, to - from, scan.nextOf1(d, from, to, '"') < to);
}

/// An unquoted field, with a trailing carriage return stripped so CRLF behaves
/// like LF.
fn plainField(d: []const u8, from: usize, to: usize) Field {
    var stop = to;
    if (stop > from and d[stop - 1] == '\r') stop -= 1;
    return f.pack(from, stop - from, false);
}

/// The offset of the newline ending the row that starts at `pos`.
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

/// Skips one JSON string starting at its opening quote, returning the offset one
/// past the closing quote and whether the string holds a backslash.
fn skipJsonString(d: []const u8, from: usize, end: usize) struct { usize, bool } {
    var at = from + 1; // the opening quote
    var escaped = false;
    while (true) {
        const stop = scan.nextOf2(d, at, end, '"', '\\');
        if (stop >= end) return .{ end, escaped };
        if (d[stop] == '"') return .{ stop + 1, escaped };
        escaped = true;
        at = stop + 2; // the backslash and whatever it escapes
        if (at > end) return .{ end, escaped };
    }
}

/// Skips a nested object or array, which is not a cell value.
fn skipJsonNested(d: []const u8, from: usize, end: usize) usize {
    var pos = from;
    var depth: i32 = 0;
    while (pos < end) {
        switch (d[pos]) {
            '"' => {
                pos = skipJsonString(d, pos, end)[0];
                continue;
            },
            '{', '[' => depth += 1,
            '}', ']' => {
                depth -= 1;
                pos += 1;
                if (depth <= 0) return pos;
                continue;
            },
            else => {},
        }
        pos += 1;
    }
    return end;
}

/// Past the end of this object's line. Records are newline-delimited, so a
/// newline outside a string ends the row.
fn endOfJsonRow(d: []const u8, from: usize, end: usize) usize {
    var pos = from;
    while (pos < end) {
        const stop = scan.nextOf2(d, pos, end, '\n', '"');
        if (stop >= end) return end;
        if (d[stop] == '\n') return stop + 1;
        const next = skipJsonString(d, stop, end)[0];
        if (next <= stop) return end;
        pos = next;
    }
    return end;
}

fn nameHash(s: []const u8) u64 {
    var h: u64 = 0xcbf2_9ce4_8422_2325;
    for (s) |c| h = (h ^ c) *% 0x100_0000_01b3;
    return h ^ (h >> 32);
}

/// Splits rows into fields, projecting straight to the columns asked for.
///
/// The CSV form is addressed by column number: once the last needed column has
/// been read the rest of the row is skipped to its newline without its fields
/// ever being delimited, so on twenty columns keyed on the first two most of a
/// row is never looked at. The JSON form is addressed by key instead, so it walks
/// the whole object — but only once, and one hash per key rather than a search
/// per wanted column, which at twenty columns would be four hundred comparisons
/// a row.
/// Where the wanted part of a row ends, when two files agree closely enough for
/// one to be measured against the other in bytes.
///
/// The join's expensive question is whether a matched pair differs, and it
/// answers it by parsing both rows into fields. It does not have to. If the two
/// rows are byte-identical up to the end of the last column either file wants,
/// then every column in between is byte-identical too, and no parse can say
/// otherwise -- so the pair is unchanged and the mate never needs reading.
///
/// That holds only when both sides really are the same shape: the same delimiter
/// and the same projection, because two files whose headers are ordered
/// differently can carry identical bytes and mean different things. `source` is
/// that projection before it is inverted, so comparing it settles the question
/// outright. JSON has no such prefix -- its keys may come in any order -- so it
/// never qualifies.
pub const Tail = struct { slot: usize, delimiter: u8 };

pub fn sharedTail(a: RowParser, b: RowParser) ?Tail {
    const ca = switch (a) {
        .csv => |c| c,
        .json => return null,
    };
    const cb = switch (b) {
        .csv => |c| c,
        .json => return null,
    };
    if (ca.delimiter != cb.delimiter) return null;
    if (ca.source.len != cb.source.len) return null;
    for (ca.source, cb.source) |x, y| {
        if (x) |xc| {
            if (y) |yc| {
                if (xc != yc) return null;
            } else return null;
        } else if (y != null) return null;
    }
    const run = ca.slots[ca.starts[ca.last_needed]..ca.starts[ca.last_needed + 1]];
    if (run.len == 0) return null;
    return .{ .slot = run[0], .delimiter = ca.delimiter };
}

pub const RowParser = union(enum) {
    csv: Csv,
    json: Json,

    pub const Csv = struct {
        delimiter: u8,
        /// Where each projected column sits in the file, or null when absent.
        source: []const ?usize,
        last_needed: usize,
        /// The inverse: which slots each column of the file feeds, laid out as
        /// one flat array with a start per column, so storing a field is a
        /// lookup rather than a walk of every wanted column -- twenty columns
        /// against twenty slots is four hundred comparisons a row otherwise.
        /// A column can feed more than one slot, because `--compare` may name a
        /// key column, so the run is a range rather than a single entry.
        slots: []const u16,
        starts: []const u32,
        /// The one slot each column feeds, or a sentinel. Almost every column
        /// feeds exactly one, and the hot path stores through this rather than
        /// slicing `slots` -- which costs two dependent loads from `starts` and
        /// a loop setup, per column, per parse, and a row is parsed about two
        /// and a half times: once by the sweep, then again for each side of the
        /// join and once more when a probe compares keys.
        single: []const i32,

        /// Marks the slots of columns `from` onward absent.
        ///
        /// This replaces clearing every slot at the top of the row. `out` is
        /// reused across rows, so a column this row does not reach has to be
        /// blanked or it would show the previous row's value -- but that is
        /// only the columns after the row ran out, and a well-formed row runs
        /// out of nothing. The full clear was 315 million instructions of the
        /// 4.6 billion this port spent on a 200,000-row pair, seven per cent of
        /// the whole comparison, because it ran twenty slots per row per file
        /// whether or not a single one of them needed it.
        /// `single[column]` when the column feeds no slot, and when it feeds
        /// more than one -- `--compare` naming a key column is the only way to
        /// get there, so it is worth a branch rather than a second array.
        pub const NONE: i32 = -1;
        pub const MANY: i32 = -2;

        /// Stores one field into every slot its column feeds.
        inline fn store(self: Csv, out: []Field, column: usize, field: Field) void {
            const one = self.single[column];
            if (one >= 0) {
                out[@intCast(one)] = field;
            } else if (one == MANY) {
                for (self.slots[self.starts[column]..self.starts[column + 1]]) |slot| {
                    out[slot] = field;
                }
            }
        }

        inline fn blankFrom(self: Csv, out: []Field, from: usize) void {
            if (from > self.last_needed) return;
            for (self.slots[self.starts[from]..self.starts[self.last_needed + 1]]) |slot| {
                out[slot] = f.ABSENT;
            }
        }
    };

    pub const Json = struct {
        /// The key whose value belongs in each slot; null for a column this file
        /// does not have.
        wanted: []const ?[]const u8,
        /// Open-addressed name to slot, so a key costs one hash.
        slots: []i32,
        slot_mask: usize,

        fn slotFor(self: Json, key: []const u8) ?usize {
            var at = nameHash(key) & self.slot_mask;
            while (true) {
                const i = self.slots[at];
                if (i < 0) return null;
                const name = self.wanted[@intCast(i)];
                if (name != null and std.mem.eql(u8, name.?, key)) return @intCast(i);
                at = (at + 1) & self.slot_mask;
            }
        }
    };

    pub fn initCsv(gpa: std.mem.Allocator, delimiter: u8, source: []const ?usize) !RowParser {
        var last: usize = 0;
        for (source) |s| if (s) |c| {
            if (c > last) last = c;
        };
        // Counting sort into the flat inverse: how many slots each column feeds,
        // then where each column's run starts, then the slots themselves.
        const starts = try gpa.alloc(u32, last + 2);
        @memset(starts, 0);
        for (source) |s| if (s) |c| {
            starts[c + 1] += 1;
        };
        for (1..starts.len) |i| starts[i] += starts[i - 1];
        const slots = try gpa.alloc(u16, starts[starts.len - 1]);
        var filled = try gpa.alloc(u32, last + 1);
        defer gpa.free(filled);
        @memset(filled, 0);
        for (source, 0..) |s, slot| {
            if (s) |c| {
                slots[starts[c] + filled[c]] = @intCast(slot);
                filled[c] += 1;
            }
        }
        // The one-slot shortcut, derived from the same counting sort.
        const single = try gpa.alloc(i32, last + 1);
        for (single, 0..) |*one, c| {
            one.* = switch (starts[c + 1] - starts[c]) {
                0 => Csv.NONE,
                1 => @intCast(slots[starts[c]]),
                else => Csv.MANY,
            };
        }
        return .{ .csv = .{
            .delimiter = delimiter,
            .source = source,
            .last_needed = last,
            .slots = slots,
            .starts = starts,
            .single = single,
        } };
    }

    pub fn initJson(gpa: std.mem.Allocator, wanted: []const ?[]const u8) !RowParser {
        var n: usize = 16;
        while (n < wanted.len * 4) n <<= 1;
        const slots = try gpa.alloc(i32, n);
        @memset(slots, -1);
        const mask = n - 1;
        for (wanted, 0..) |name, i| {
            if (name == null) continue;
            var at = nameHash(name.?) & mask;
            while (slots[at] >= 0) at = (at + 1) & mask;
            slots[at] = @intCast(i);
        }
        return .{ .json = .{ .wanted = wanted, .slots = slots, .slot_mask = mask } };
    }

    pub fn deinit(self: RowParser, gpa: std.mem.Allocator) void {
        switch (self) {
            .json => |j| gpa.free(j.slots),
            .csv => |c| {
                gpa.free(c.slots);
                gpa.free(c.starts);
                gpa.free(c.single);
            },
        }
    }

    /// Parses one row into `out`, returning the offset of the next row. A row
    /// shorter than the header leaves the missing fields absent, which is a
    /// difference to report rather than a file to refuse.
    pub fn parse(self: RowParser, d: []const u8, start: usize, end: usize, out: []Field) usize {
        return switch (self) {
            .csv => |c| parseCsv(c, d, start, end, out),
            .json => |j| parseJson(j, d, start, end, out),
        };
    }

    fn parseCsv(self: Csv, d: []const u8, start: usize, end: usize, out: []Field) usize {
        var pos = start;
        var column: usize = 0;
        // One cursor for the row -- every byte scanned once, both delimiters
        // broadcast once -- but only where a scan step spans several fields.
        // On an eight-byte step it does not, and the plain scan is quicker.
        var delims: scan.Delims = if (scan.wide_scan)
            scan.Delims.init(d, start, end, self.delimiter, '\n')
        else
            undefined;

        while (pos <= end) {
            var field: Field = undefined;
            var next: usize = undefined;
            if (pos < end and d[pos] == '"') {
                const close = scan.skipQuoted(d, pos + 1, end);
                const body_end = if (close > pos + 1) close - 1 else pos + 1;
                next = if (scan.wide_scan)
                    delims.next(close)
                else
                    scan.nextOf2(d, close, end, self.delimiter, '\n');
                field = quotedField(d, pos + 1, body_end);
            } else {
                next = if (scan.wide_scan)
                    delims.next(pos)
                else
                    scan.nextOf2(d, pos, end, self.delimiter, '\n');
                field = plainField(d, pos, next);
            }
            if (column <= self.last_needed) self.store(out, column, field);
            column += 1;

            if (next >= end) {
                self.blankFrom(out, column);
                return end;
            }
            if (d[next] == '\n') {
                self.blankFrom(out, column);
                return next + 1;
            }
            pos = next + 1;
            // Every needed column is filled, so there is nothing to blank: the
            // rest of the row is skipped without being parsed.
            if (column > self.last_needed) {
                const eol = endOfRow(d, pos, end);
                return if (eol >= end) end else eol + 1;
            }
        }
        self.blankFrom(out, column);
        return end;
    }

    /// Walks one JSON object, storing the values of the keys we want.
    fn parseJson(self: Json, d: []const u8, start: usize, end: usize, out: []Field) usize {
        @memset(out, f.ABSENT);
        var pos = start;
        while (pos < end and jsonSpace(d[pos])) pos += 1;
        if (pos >= end) return end;
        if (d[pos] != '{') return endOfJsonRow(d, pos, end); // not an object: skip the line
        pos += 1;

        while (true) {
            while (pos < end and jsonSpace(d[pos])) pos += 1;
            if (pos >= end) break;
            if (d[pos] == '}') {
                pos += 1;
                break;
            }
            if (d[pos] == ',') {
                pos += 1;
                continue;
            }
            if (d[pos] != '"') break; // malformed: stop reading this object

            const key_from = pos + 1;
            const key_end = skipJsonString(d, pos, end)[0];
            if (key_end > end or key_end < 2) break;
            const key = d[key_from .. key_end - 1];
            pos = key_end;
            while (pos < end and jsonSpace(d[pos])) pos += 1;
            if (pos >= end or d[pos] != ':') break;
            pos += 1;
            while (pos < end and jsonSpace(d[pos])) pos += 1;
            if (pos >= end) break;

            var field: ?Field = null;
            if (d[pos] == '"') {
                const from = pos + 1;
                const close, const escaped = skipJsonString(d, pos, end);
                const to = if (close > from) close - 1 else from;
                pos = close;
                field = f.pack(from, to - from, escaped);
            } else if (d[pos] == '{' or d[pos] == '[') {
                // Not a cell value. Left absent rather than guessed at.
                pos = skipJsonNested(d, pos, end);
            } else {
                // A number, true, false or null: it runs to the next comma,
                // brace or space.
                const from = pos;
                while (pos < end and d[pos] != ',' and d[pos] != '}' and !jsonSpace(d[pos])) pos += 1;
                if (!std.mem.eql(u8, d[from..pos], "null")) field = f.pack(from, pos - from, false);
            }
            if (field) |value| {
                if (self.slotFor(key)) |slot| out[slot] = value;
            }
        }
        return endOfJsonRow(d, pos, end);
    }

};

/// A header name is one of the few strings this engine owns; there is one per
/// column, not one per cell.
fn ownField(gpa: std.mem.Allocator, slab: Slab, field: Field) ![]const u8 {
    var buf: std.ArrayList(u8) = .empty;
    errdefer buf.deinit(gpa);
    var it = slab.logical(field);
    while (it.next()) |b| try buf.append(gpa, b);
    return buf.toOwnedSlice(gpa);
}

pub const Header = struct { names: [][]const u8, start: usize };

/// The CSV header row's names, and where the first data row starts.
pub fn readCsvHeader(gpa: std.mem.Allocator, slab: Slab, delimiter: u8) !Header {
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
            field = quotedField(d, pos + 1, body_end);
        } else {
            next = scan.nextOf2(d, pos, d.len, delimiter, '\n');
            field = plainField(d, pos, next);
        }
        try names.append(gpa, try ownField(gpa, slab, field));

        if (next >= d.len) return .{ .names = try names.toOwnedSlice(gpa), .start = d.len };
        if (d[next] == '\n') return .{ .names = try names.toOwnedSlice(gpa), .start = next + 1 };
        pos = next + 1;
    }
}

/// A JSON file has no header row, so the column names are the keys of the first
/// object, in the order it lists them.
pub fn readJsonHeader(gpa: std.mem.Allocator, slab: Slab) !Header {
    const d = slab.data;
    var names: std.ArrayList([]const u8) = .empty;
    errdefer names.deinit(gpa);
    var pos: usize = 0;
    while (pos < d.len and jsonSpace(d[pos])) pos += 1;
    if (pos >= d.len or d[pos] != '{') return Error.NoJsonObject;
    pos += 1;
    while (true) {
        while (pos < d.len and jsonSpace(d[pos])) pos += 1;
        if (pos >= d.len or d[pos] == '}') break;
        if (d[pos] == ',') {
            pos += 1;
            continue;
        }
        if (d[pos] != '"') break;
        const from = pos + 1;
        const close, const escaped = skipJsonString(d, pos, d.len);
        if (close <= from) break;
        try names.append(gpa, try ownField(gpa, slab, f.pack(from, close - 1 - from, escaped)));
        pos = close;
        while (pos < d.len and jsonSpace(d[pos])) pos += 1;
        if (pos >= d.len or d[pos] != ':') break;
        pos += 1;
        while (pos < d.len and jsonSpace(d[pos])) pos += 1;
        if (pos >= d.len) break;
        if (d[pos] == '"') {
            pos = skipJsonString(d, pos, d.len)[0];
        } else if (d[pos] == '{' or d[pos] == '[') {
            pos = skipJsonNested(d, pos, d.len);
        } else {
            while (pos < d.len and d[pos] != ',' and d[pos] != '}' and !jsonSpace(d[pos])) pos += 1;
        }
    }
    if (names.items.len == 0) return Error.NoJsonObject;
    return .{ .names = try names.toOwnedSlice(gpa), .start = 0 };
}

test "a brace is json and anything else is csv" {
    try std.testing.expectEqual(Dialect.json, sniffDialect("  {\"a\":1}"));
    try std.testing.expectEqual(Dialect.csv, sniffDialect("a,b,c\n"));
    try std.testing.expectEqual(Dialect.csv, sniffDialect(""));
}

test "json values are read by key whatever order they come in" {
    const text = "{\"k\":\"1\",\"v\":\"a\"}\n{\"v\":\"b\",\"k\":\"2\"}\n";
    const slab = Slab{ .data = text, .dialect = .json };
    const gpa = std.testing.allocator;
    const wanted = [_]?[]const u8{ "k", "v" };
    const parser = try RowParser.initJson(gpa, &wanted);
    defer parser.deinit(gpa);
    var out: [2]Field = undefined;
    const next = parser.parse(text, 0, text.len, &out);
    try std.testing.expectEqualStrings("1", slab.raw(out[0]));
    try std.testing.expectEqualStrings("a", slab.raw(out[1]));
    _ = parser.parse(text, next, text.len, &out);
    try std.testing.expectEqualStrings("2", slab.raw(out[0]));
    try std.testing.expectEqualStrings("b", slab.raw(out[1]));
}

test "null, a nested value and a missing key are all absent" {
    const text = "{\"k\":\"1\",\"v\":null,\"w\":{\"deep\":1},\"x\":[1,2]}\n";
    const gpa = std.testing.allocator;
    const wanted = [_]?[]const u8{ "k", "v", "w", "x", "missing" };
    const parser = try RowParser.initJson(gpa, &wanted);
    defer parser.deinit(gpa);
    var out: [5]Field = undefined;
    const next = parser.parse(text, 0, text.len, &out);
    const slab = Slab{ .data = text, .dialect = .json };
    try std.testing.expectEqualStrings("1", slab.raw(out[0]));
    for (out[1..]) |field| try std.testing.expectEqual(f.ABSENT, field);
    try std.testing.expectEqual(text.len, next);
}

test "the json header is the first object's keys in order" {
    const text = "{\"b\":1,\"a\":\"x\",\"n\":null}\n";
    const gpa = std.testing.allocator;
    const head = try readJsonHeader(gpa, .{ .data = text, .dialect = .json });
    defer {
        for (head.names) |n| gpa.free(n);
        gpa.free(head.names);
    }
    try std.testing.expectEqual(@as(usize, 3), head.names.len);
    try std.testing.expectEqualStrings("b", head.names[0]);
    try std.testing.expectEqualStrings("a", head.names[1]);
    try std.testing.expectEqualStrings("n", head.names[2]);
}
