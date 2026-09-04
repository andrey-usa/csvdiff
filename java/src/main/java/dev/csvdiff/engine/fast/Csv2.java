package dev.csvdiff.engine.fast;

import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;

/** Delimiter sniffing, done on bytes rather than on a decoded header line. */
final class Csv2 {

  private Csv2() {}

  private static final byte[] CANDIDATES = {',', ';', '\t', '|'};

  /**
   * Picks whichever candidate appears most often in the header, defaulting to a comma.
   *
   * <p>The same rule as {@link dev.csvdiff.engine.Csv#detectDelimiter}, including the tie-break to
   * the earliest candidate, so an engine swap cannot change how a file is split.
   */
  static byte detect(MemorySegment seg, long headerEnd) {
    byte best = ',';
    int bestCount = -1;
    for (byte candidate : CANDIDATES) {
      int n = 0;
      for (long i = 0; i < headerEnd; i++) {
        if (seg.get(ValueLayout.JAVA_BYTE, i) == candidate) {
          n++;
        }
      }
      if (n > bestCount) {
        best = candidate;
        bestCount = n;
      }
    }
    return best;
  }
}
