//! Page decompression: the four codecs a Parquet file in the wild actually uses.
//!
//! Snappy and LZ4 are written out here rather than pulled in, because both are
//! byte-copy loops of about eighty lines and neither is in the standard library.
//! Gzip and zstd are not written out: their decoders are real programs, and Zig
//! ships both.
//!
//! Every one of these writes into a buffer sized from the page header, so a
//! corrupt length is a refusal rather than an allocation the size of the number
//! that happened to be in the file. The buffer comes from the caller's
//! allocator, so `--max-memory` still bounds a Parquet comparison.

const std = @import("std");

pub const Error = error{
    UnsupportedCodec,
    TruncatedPage,
    CorruptPage,
    PageSizeMismatch,
};

/// Parquet's compression codec ids, from its Thrift schema.
pub const Codec = enum {
    none,
    snappy,
    gzip,
    zstd,
    lz4_raw,

    pub fn fromId(id: i32) !Codec {
        return switch (id) {
            0 => .none,
            1 => .snappy,
            2 => .gzip,
            6 => .zstd,
            7 => .lz4_raw,
            else => Error.UnsupportedCodec, // LZO, Brotli, and the deprecated LZ4 framing
        };
    }
};

/// Decompresses one page into a buffer sized from the page header.
pub fn decompress(
    gpa: std.mem.Allocator,
    codec: Codec,
    input: []const u8,
    expected: usize,
) ![]u8 {
    const out = try gpa.alloc(u8, expected);
    errdefer gpa.free(out);
    switch (codec) {
        .none => {
            if (input.len < expected) return Error.PageSizeMismatch;
            @memcpy(out, input[0..expected]);
        },
        .snappy => try snappy(input, out),
        .lz4_raw => try lz4(input, out),
        .gzip => try inflate(gpa, input, out),
        .zstd => try unzstd(gpa, input, out),
    }
    return out;
}

/// A cursor that fills `out` and refuses to run past it, which is what turns a
/// corrupt length into an error rather than a wild write.
const Sink = struct {
    out: []u8,
    at: usize = 0,

    fn literal(self: *Sink, bytes: []const u8) !void {
        if (self.at + bytes.len > self.out.len) return Error.PageSizeMismatch;
        @memcpy(self.out[self.at..][0..bytes.len], bytes);
        self.at += bytes.len;
    }

    /// Copies `len` bytes from `distance` back, which may overlap what it is
    /// writing: a run of one byte is encoded as a one-byte match repeated.
    fn copy(self: *Sink, distance: usize, len: usize) !void {
        if (distance == 0 or distance > self.at) return Error.CorruptPage;
        if (self.at + len > self.out.len) return Error.PageSizeMismatch;
        var from = self.at - distance;
        for (0..len) |_| {
            self.out[self.at] = self.out[from];
            self.at += 1;
            from += 1;
        }
    }
};

/// Snappy's raw block format: a length, then literals and back-references.
fn snappy(input: []const u8, out: []u8) !void {
    var at: usize = 0;
    var declared: usize = 0;
    var shift: u6 = 0;
    while (true) {
        if (at >= input.len) return Error.TruncatedPage;
        const b = input[at];
        at += 1;
        declared |= @as(usize, b & 0x7f) << shift;
        if (b & 0x80 == 0) break;
        if (shift >= 28) return Error.CorruptPage;
        shift += 7;
    }
    if (declared != out.len) return Error.PageSizeMismatch;

    var sink = Sink{ .out = out };
    while (at < input.len) {
        const tag = input[at];
        at += 1;
        if (tag & 0x03 == 0) {
            // A literal: a short length in the tag, or one to four bytes of it.
            var len: usize = tag >> 2;
            if (len >= 60) {
                const extra = len - 59;
                if (at + extra > input.len) return Error.TruncatedPage;
                len = 0;
                for (input[at..][0..extra], 0..) |b, i| len |= @as(usize, b) << @intCast(8 * i);
                at += extra;
            }
            len += 1;
            if (at + len > input.len) return Error.TruncatedPage;
            try sink.literal(input[at..][0..len]);
            at += len;
            continue;
        }
        var len: usize = 0;
        var distance: usize = 0;
        switch (tag & 0x03) {
            1 => {
                if (at >= input.len) return Error.TruncatedPage;
                len = 4 + ((tag >> 2) & 0x07);
                distance = (@as(usize, tag >> 5) << 8) | input[at];
                at += 1;
            },
            2 => {
                if (at + 2 > input.len) return Error.TruncatedPage;
                len = @as(usize, tag >> 2) + 1;
                distance = std.mem.readInt(u16, input[at..][0..2], .little);
                at += 2;
            },
            else => {
                if (at + 4 > input.len) return Error.TruncatedPage;
                len = @as(usize, tag >> 2) + 1;
                distance = std.mem.readInt(u32, input[at..][0..4], .little);
                at += 4;
            },
        }
        try sink.copy(distance, len);
    }
    if (sink.at != out.len) return Error.PageSizeMismatch;
}

/// LZ4's block format: a token, literals, then a two-byte back-reference.
fn lz4(input: []const u8, out: []u8) !void {
    var sink = Sink{ .out = out };
    var at: usize = 0;
    while (at < input.len) {
        const token = input[at];
        at += 1;
        var literals: usize = token >> 4;
        if (literals == 15) {
            while (true) {
                if (at >= input.len) return Error.TruncatedPage;
                const b = input[at];
                at += 1;
                literals += b;
                if (b != 255) break;
            }
        }
        if (at + literals > input.len) return Error.TruncatedPage;
        try sink.literal(input[at..][0..literals]);
        at += literals;
        // The last sequence of a block is literals only, with no match after it.
        if (at >= input.len) break;
        if (at + 2 > input.len) return Error.TruncatedPage;
        const distance = std.mem.readInt(u16, input[at..][0..2], .little);
        at += 2;
        var len: usize = token & 0x0f;
        if (len == 15) {
            while (true) {
                if (at >= input.len) return Error.TruncatedPage;
                const b = input[at];
                at += 1;
                len += b;
                if (b != 255) break;
            }
        }
        try sink.copy(distance, len + 4);
    }
    if (sink.at != out.len) return Error.PageSizeMismatch;
}

fn inflate(gpa: std.mem.Allocator, input: []const u8, out: []u8) !void {
    const window = try gpa.alloc(u8, std.compress.flate.max_window_len);
    defer gpa.free(window);
    var in = std.Io.Reader.fixed(input);
    var d = std.compress.flate.Decompress.init(&in, .gzip, window);
    var w = std.Io.Writer.fixed(out);
    const n = d.reader.streamRemaining(&w) catch return Error.CorruptPage;
    if (n != out.len) return Error.PageSizeMismatch;
}

fn unzstd(gpa: std.mem.Allocator, input: []const u8, out: []u8) !void {
    const window = try gpa.alloc(
        u8,
        std.compress.zstd.default_window_len + std.compress.zstd.block_size_max,
    );
    defer gpa.free(window);
    var in = std.Io.Reader.fixed(input);
    var d = std.compress.zstd.Decompress.init(&in, window, .{ .verify_checksum = false });
    var w = std.Io.Writer.fixed(out);
    const n = d.reader.streamRemaining(&w) catch return Error.CorruptPage;
    if (n != out.len) return Error.PageSizeMismatch;
}

test "snappy reads literals and back-references" {
    // "abcabcabcab": a three-byte literal then a copy that overlaps itself.
    const block = [_]u8{ 0x0b, 0x08, 'a', 'b', 'c', 0x11, 0x03 };
    var out: [11]u8 = undefined;
    try snappy(&block, &out);
    try std.testing.expectEqualStrings("abcabcabcab", &out);
}

test "a page that decompresses to the wrong size is refused" {
    const block = [_]u8{ 0x0b, 0x08, 'a', 'b', 'c', 0x11, 0x03 };
    var out: [12]u8 = undefined;
    try std.testing.expectError(Error.PageSizeMismatch, snappy(&block, &out));
}

test "a back-reference before the page is refused rather than read" {
    const block = [_]u8{ 0x04, 0x01, 0x0a };
    var out: [4]u8 = undefined;
    try std.testing.expectError(Error.CorruptPage, snappy(&block, &out));
}

test "lz4 reads literals and matches" {
    const block = [_]u8{ 0x30, 'a', 'b', 'c', 0x03, 0x00 };
    var out: [7]u8 = undefined;
    try lz4(&block, &out);
    try std.testing.expectEqualStrings("abcabca", &out);
}
