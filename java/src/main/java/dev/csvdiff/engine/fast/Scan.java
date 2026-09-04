package dev.csvdiff.engine.fast;

import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;

/**
 * How the parser finds the bytes that matter.
 *
 * <p>Parsing CSV is mostly one question asked over and over: where is the next delimiter, newline
 * or quote? A scalar loop asks it once per byte. There are two well-known ways to ask it for many
 * bytes at once, and this interface is the seam that lets the benchmark put them side by side:
 *
 * <ul>
 *   <li>{@link VectorScan} — the Vector API. One SIMD register, 32 or 64 bytes, one compare.
 *   <li>{@link SwarScan} — SWAR, "SIMD within a register". Eight bytes packed in an ordinary
 *       {@code long}, and a bit trick instead of a comparison. No incubator module, no vector
 *       registers, and on short fields often faster than the real thing because there is nothing
 *       to set up.
 * </ul>
 *
 * <p>An engine holds exactly one implementation for its lifetime, so the call sites are monomorphic
 * and the JIT inlines through them.
 */
public interface Scan {

  ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;

  /** The name this technique goes by in the report. */
  String technique();

  /** Whether this technique can run on this JVM and this hardware. */
  boolean available();

  /** The offset of the next byte equal to {@code a} or {@code b}, or {@code end} if there is none. */
  long nextOf2(MemorySegment seg, long from, long end, byte a, byte b);

  /** The offset of the next byte equal to {@code a}, or {@code end} if there is none. */
  long nextOf1(MemorySegment seg, long from, long end, byte a);

  /** The offset just past the closing quote of a quoted field that starts at {@code from}. */
  default long skipQuoted(MemorySegment seg, long from, long end) {
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

  /**
   * The start of the row after {@code from}, skipping over quoted regions.
   *
   * <p>Used to split a file into chunks that each begin on a row boundary, so several threads can
   * parse at once.
   */
  default long nextRowStart(MemorySegment seg, long from, long end, byte delimiter) {
    long pos = from;
    while (pos < end) {
      long at = nextOf2(seg, pos, end, (byte) '\n', (byte) '"');
      if (at >= end) {
        return end;
      }
      if (seg.get(BYTE, at) == '\n') {
        return at + 1;
      }
      pos = skipQuoted(seg, at + 1, end);
    }
    return end;
  }
}
