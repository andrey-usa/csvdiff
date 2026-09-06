//! SWAR scanning: eight bytes per step, using arithmetic rather than comparison.
//!
//! A delimiter is found by broadcasting it across a word and subtracting ones:
//! the borrow crosses a byte only where that byte was zero, and `~diff` cancels
//! the false positives the borrow creates, so what survives marks exactly the
//! matching bytes.

const std = @import("std");

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

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
    const ba = broadcast(a);
    const bb = broadcast(b);
    var at = from;
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
    const bt = broadcast(target);
    var at = from;
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
