//! SWAR scanning: eight bytes per step, using arithmetic rather than comparison.
//!
//! A delimiter is found by broadcasting it across a word and subtracting ones:
//! the borrow crosses a byte only where that byte was zero, and `~diff` cancels
//! the false positives the borrow creates, so what survives marks exactly the
//! matching bytes.

const std = @import("std");
const build_options = @import("build_options");

/// How many bytes a scan step takes. Eight is SWAR -- ordinary 64-bit
/// arithmetic, no CPU feature at all -- and 32 or 64 puts the same question to a
/// vector register, which on x86 is AVX2 or AVX-512 as long as the build targets
/// a CPU that has them (`zig build -Dscan=32 -Dcpu=native`).
///
/// It is a build option rather than a runtime switch because a benchmark of an
/// instruction set should not be measuring a function pointer, and because each
/// binary is then what you would actually ship for that target.
const width = build_options.scan_width;
const Vector = @Vector(width, u8);
const Mask = std.meta.Int(.unsigned, width);

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

/// The bytes of `chunk` equal to `target`, as one bit each.
inline fn matches(chunk: Vector, target: Vector) Mask {
    const equal: @Vector(width, bool) = chunk == target;
    return @bitCast(equal);
}

inline fn loadVector(data: []const u8, at: usize) Vector {
    return @bitCast(data[at..][0..width].*);
}

inline fn broadcast(b: u8) u64 {
    return @as(u64, b) *% ONES;
}

inline fn matchBits(word: u64, target: u64) u64 {
    const diff = word ^ target;
    return (diff -% ONES) & ~diff & HIGH;
}

inline fn load64(data: []const u8, at: usize) u64 {
    return std.mem.readInt(u64, data[at..][0..8], .little);
}

/// The offset of the first byte at or after `from` that is `a` or `b`, or `end`.
pub fn nextOf2(data: []const u8, from: usize, end: usize, a: u8, b: u8) usize {
    var at = from;
    if (width > 8) {
        const va: Vector = @splat(a);
        const vb: Vector = @splat(b);
        while (at + width <= end) : (at += width) {
            const chunk = loadVector(data, at);
            const hits = matches(chunk, va) | matches(chunk, vb);
            if (hits != 0) return at + @ctz(hits);
        }
    }
    const ba = broadcast(a);
    const bb = broadcast(b);
    while (at + 8 <= end) : (at += 8) {
        const word = load64(data, at);
        const hits = matchBits(word, ba) | matchBits(word, bb);
        if (hits != 0) return at + (@ctz(hits) >> 3);
    }
    while (at < end) : (at += 1) {
        if (data[at] == a or data[at] == b) return at;
    }
    return end;
}

/// The offset of the first `target` at or after `from`, or `end`.
pub fn nextOf1(data: []const u8, from: usize, end: usize, target: u8) usize {
    var at = from;
    if (width > 8) {
        const vt: Vector = @splat(target);
        while (at + width <= end) : (at += width) {
            const hits = matches(loadVector(data, at), vt);
            if (hits != 0) return at + @ctz(hits);
        }
    }
    const bt = broadcast(target);
    while (at + 8 <= end) : (at += 8) {
        const word = load64(data, at);
        const hits = matchBits(word, bt);
        if (hits != 0) return at + (@ctz(hits) >> 3);
    }
    while (at < end) : (at += 1) {
        if (data[at] == target) return at;
    }
    return end;
}

/// Walks past a quoted field's body. A doubled quote inside it is content.
pub fn skipQuoted(data: []const u8, from: usize, end: usize) usize {
    var at = from;
    while (true) {
        const q = nextOf1(data, at, end, '"');
        if (q >= end) return end;
        if (q + 1 < end and data[q + 1] == '"') {
            at = q + 2;
            continue;
        }
        return q + 1;
    }
}

test "nextOf2 finds either byte, across the word boundary" {
    const d = "aaaaaaaaaaaa,x\nbb";
    try std.testing.expectEqual(@as(usize, 12), nextOf2(d, 0, d.len, ',', '\n'));
    try std.testing.expectEqual(@as(usize, 14), nextOf2(d, 13, d.len, ',', '\n'));
    try std.testing.expectEqual(d.len, nextOf2(d, 15, d.len, ',', '\n'));
}

test "skipQuoted treats a doubled quote as content" {
    const d = "\"a\"\"b\",rest";
    try std.testing.expectEqual(@as(usize, 6), skipQuoted(d, 1, d.len));
}

/// How many `target` bytes there are in `data[from..end]`.
///
/// Only the chunked index build needs this, and only for the quote character:
/// the number of quotes before a position is what says whether that position is
/// inside a quoted field. Counting whole words at a time keeps it far cheaper
/// than the parsing it makes parallel.
pub fn countByte(data: []const u8, from: usize, end: usize, target: u8) usize {
    var n: usize = 0;
    var at = from;
    if (width > 8) {
        const vt: Vector = @splat(target);
        while (at + width <= end) : (at += width) {
            n += @popCount(matches(loadVector(data, at), vt));
        }
    }
    const bt = broadcast(target);
    while (at + 8 <= end) : (at += 8) {
        n += @popCount(matchBits(load64(data, at), bt));
    }
    while (at < end) : (at += 1) {
        if (data[at] == target) n += 1;
    }
    return n;
}

test "countByte counts the same as one byte at a time" {
    const d = "a\"b\"\"c\"d\"e\"\"\"f";
    var from: usize = 0;
    while (from < d.len) : (from += 1) {
        var want: usize = 0;
        for (d[from..]) |b| {
            if (b == '"') want += 1;
        }
        try std.testing.expectEqual(want, countByte(d, from, d.len, '"'));
    }
}
