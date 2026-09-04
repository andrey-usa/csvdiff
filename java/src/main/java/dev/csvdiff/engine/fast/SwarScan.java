package dev.csvdiff.engine.fast;

import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.ByteOrder;

/**
 * SWAR — "SIMD within a register". Eight bytes at a time in an ordinary {@code long}.
 *
 * <p>This is the technique the fastest One Billion Row Challenge entries are built on. Where the
 * Vector API asks the CPU for a real SIMD compare, SWAR gets the same answer out of a plain 64-bit
 * ALU with one subtraction and two ands — the classic "has a zero byte" trick:
 *
 * <pre>{@code
 * long diff = word ^ broadcast(target);          // zero byte exactly where the target matched
 * long hits = (diff - 0x0101..) & ~diff & 0x8080..;  // high bit set in each matching byte
 * }</pre>
 *
 * <p>{@code diff - 0x0101...} borrows across a byte only when that byte was zero, {@code ~diff}
 * cancels the false positives that borrowing creates in non-zero bytes, and the {@code 0x8080...}
 * mask keeps one bit per byte. {@link Long#numberOfTrailingZeros} then names the first match, and
 * {@code >>> 3} turns a bit index into a byte index.
 *
 * <p>Two reasons it can beat the Vector API here even though it looks at a quarter as many bytes:
 * there is no vector register to fill or drain, and CSV fields are short — an eleven-character
 * transaction id is found in the second 8-byte word, well before a 32-byte vector would have paid
 * for itself. It also needs no incubator module.
 *
 * <p>Words are read little-endian explicitly rather than in native order, so byte 0 of the word is
 * always the lowest address and {@code numberOfTrailingZeros} always names the first match. On a
 * little-endian machine — every machine this runs on — that layout is free.
 */
public final class SwarScan implements Scan {

  private static final ValueLayout.OfLong LONG =
      ValueLayout.JAVA_LONG_UNALIGNED.withOrder(ByteOrder.LITTLE_ENDIAN);

  private static final long ONES = 0x0101_0101_0101_0101L;
  private static final long HIGH = 0x8080_8080_8080_8080L;

  @Override
  public String technique() {
    return "swar";
  }

  @Override
  public boolean available() {
    return true; // no module, no intrinsic, nothing to probe: it is arithmetic
  }

  /** One byte smeared across all eight lanes. */
  private static long broadcast(byte b) {
    return (b & 0xFFL) * ONES;
  }

  /** A high bit set in each byte of {@code word} that equals the broadcast byte. */
  private static long matchBits(long word, long broadcast) {
    long diff = word ^ broadcast;
    return (diff - ONES) & ~diff & HIGH;
  }

  /** The byte index, within the word, of the lowest set match bit. */
  private static int firstMatch(long bits) {
    return Long.numberOfTrailingZeros(bits) >>> 3;
  }

  @Override
  public long nextOf2(MemorySegment seg, long from, long end, byte a, byte b) {
    long pos = from;
    long ba = broadcast(a);
    long bb = broadcast(b);
    long limit = end - Long.BYTES;

    while (pos <= limit) {
      long word = seg.get(LONG, pos);
      long bits = matchBits(word, ba) | matchBits(word, bb);
      if (bits != 0) {
        return pos + firstMatch(bits);
      }
      pos += Long.BYTES;
    }
    for (; pos < end; pos++) {
      byte x = seg.get(BYTE, pos);
      if (x == a || x == b) {
        return pos;
      }
    }
    return end;
  }

  @Override
  public long nextOf1(MemorySegment seg, long from, long end, byte a) {
    long pos = from;
    long ba = broadcast(a);
    long limit = end - Long.BYTES;

    while (pos <= limit) {
      long bits = matchBits(seg.get(LONG, pos), ba);
      if (bits != 0) {
        return pos + firstMatch(bits);
      }
      pos += Long.BYTES;
    }
    for (; pos < end; pos++) {
      if (seg.get(BYTE, pos) == a) {
        return pos;
      }
    }
    return end;
  }
}
