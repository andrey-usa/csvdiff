package dev.csvdiff.engine.fast;

import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.ByteOrder;
import jdk.incubator.vector.ByteVector;
import jdk.incubator.vector.VectorMask;
import jdk.incubator.vector.VectorOperators;
import jdk.incubator.vector.VectorSpecies;

/**
 * Finds the bytes that matter, a vector register at a time.
 *
 * <p>Parsing CSV is mostly the search for three bytes: the delimiter, the newline and the quote.
 * A scalar loop asks that question once per byte. {@link ByteVector} asks it for a whole register
 * at once — 32 or 64 bytes on the machines this runs on — and turns the answer into a bitmask whose
 * lowest set bit is the next interesting position. On a 184-byte row that is five or six vector
 * comparisons instead of a hundred and eighty branches.
 *
 * <p>The Vector API is still an incubator module, so this class is the only place that depends on
 * it: {@link #available()} says whether it can be used, and the engines that need it check first.
 *
 * <p>Vectors read straight from a {@link MemorySegment}, so the same code scans a heap array and a
 * mapped file without a copy or a second implementation.
 */
public final class Scanner {

  private Scanner() {}

  private static final VectorSpecies<Byte> SPECIES = ByteVector.SPECIES_PREFERRED;
  private static final int LANES = SPECIES.length();
  private static final ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;
  private static final ByteOrder ORDER = ByteOrder.nativeOrder();

  /** How wide the vectors are here, for the run metadata. */
  public static int lanes() {
    return LANES;
  }

  /** Whether the incubating Vector API loaded, and so whether the SIMD engines can run. */
  public static boolean available() {
    try {
      return SPECIES.length() > 0;
    } catch (LinkageError e) {
      return false;
    }
  }

  /**
   * The offset of the next byte equal to {@code a}, {@code b} or {@code c}, or {@code end} if there
   * is none.
   */
  public static long nextOf3(MemorySegment seg, long from, long end, byte a, byte b, byte c) {
    long pos = from;
    ByteVector va = ByteVector.broadcast(SPECIES, a);
    ByteVector vb = ByteVector.broadcast(SPECIES, b);
    ByteVector vc = ByteVector.broadcast(SPECIES, c);

    while (pos + LANES <= end) {
      ByteVector v = ByteVector.fromMemorySegment(SPECIES, seg, pos, ORDER);
      VectorMask<Byte> hit =
          v.compare(VectorOperators.EQ, va)
              .or(v.compare(VectorOperators.EQ, vb))
              .or(v.compare(VectorOperators.EQ, vc));
      if (hit.anyTrue()) {
        return pos + hit.firstTrue();
      }
      pos += LANES;
    }
    for (; pos < end; pos++) {
      byte x = seg.get(BYTE, pos);
      if (x == a || x == b || x == c) {
        return pos;
      }
    }
    return end;
  }

  /** The offset of the next byte equal to {@code a} or {@code b}, or {@code end} if there is none. */
  public static long nextOf2(MemorySegment seg, long from, long end, byte a, byte b) {
    long pos = from;
    ByteVector va = ByteVector.broadcast(SPECIES, a);
    ByteVector vb = ByteVector.broadcast(SPECIES, b);
    while (pos + LANES <= end) {
      ByteVector v = ByteVector.fromMemorySegment(SPECIES, seg, pos, ORDER);
      VectorMask<Byte> hit =
          v.compare(VectorOperators.EQ, va).or(v.compare(VectorOperators.EQ, vb));
      if (hit.anyTrue()) {
        return pos + hit.firstTrue();
      }
      pos += LANES;
    }
    for (; pos < end; pos++) {
      byte x = seg.get(BYTE, pos);
      if (x == a || x == b) {
        return pos;
      }
    }
    return end;
  }

  /** The offset of the next byte equal to {@code a}, or {@code end} if there is none. */
  public static long nextOf1(MemorySegment seg, long from, long end, byte a) {
    long pos = from;
    ByteVector va = ByteVector.broadcast(SPECIES, a);
    while (pos + LANES <= end) {
      ByteVector v = ByteVector.fromMemorySegment(SPECIES, seg, pos, ORDER);
      VectorMask<Byte> hit = v.compare(VectorOperators.EQ, va);
      if (hit.anyTrue()) {
        return pos + hit.firstTrue();
      }
      pos += LANES;
    }
    for (; pos < end; pos++) {
      if (seg.get(BYTE, pos) == a) {
        return pos;
      }
    }
    return end;
  }

  /**
   * Counts the newlines in a range.
   *
   * <p>Used to size the row arrays exactly before parsing, which turns a growing array into a
   * single allocation. It over-counts nothing: a trailing row without a newline is added by the
   * caller.
   */
  public static long countLines(MemorySegment seg, long from, long end) {
    long pos = from;
    long count = 0;
    ByteVector nl = ByteVector.broadcast(SPECIES, (byte) '\n');
    while (pos + LANES <= end) {
      ByteVector v = ByteVector.fromMemorySegment(SPECIES, seg, pos, ORDER);
      count += v.compare(VectorOperators.EQ, nl).trueCount();
      pos += LANES;
    }
    for (; pos < end; pos++) {
      if (seg.get(BYTE, pos) == '\n') {
        count++;
      }
    }
    return count;
  }

  /**
   * The start of the row after {@code from}, skipping over quoted regions.
   *
   * <p>Used to split a file into chunks that each begin on a row boundary, so several threads can
   * parse at once.
   */
  public static long nextRowStart(MemorySegment seg, long from, long end, byte delimiter) {
    long pos = from;
    while (pos < end) {
      long at = nextOf3(seg, pos, end, delimiter, (byte) '\n', (byte) '"');
      if (at >= end) {
        return end;
      }
      byte b = seg.get(BYTE, at);
      if (b == '\n') {
        return at + 1;
      }
      if (b == '"') {
        pos = skipQuoted(seg, at + 1, end);
      } else {
        pos = at + 1;
      }
    }
    return end;
  }

  /** The offset just past the closing quote of a quoted field that starts at {@code from}. */
  public static long skipQuoted(MemorySegment seg, long from, long end) {
    long pos = from;
    while (pos < end) {
      long at = nextOf1(seg, pos, end, (byte) '"');
      if (at >= end) {
        return end;
      }
      if (at + 1 < end && seg.get(BYTE, at + 1) == '"') {
        pos = at + 2; // a doubled quote is a literal one, not the end of the field
        continue;
      }
      return at + 1;
    }
    return end;
  }
}
