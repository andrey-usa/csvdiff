package dev.csvdiff.engine.fast;

import dev.csvdiff.CsvDiffException;
import dev.csvdiff.Options;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Splits rows into fields, projecting straight to the columns the comparison asked for.
 *
 * <p>Only the key and compared columns are kept, and once the last of them has been read the rest
 * of the row is skipped to its newline without its fields ever being delimited. On a twenty-column
 * file keyed on the first two, that is most of the row never looked at.
 *
 * <p>A parser holds a scratch buffer and is not safe to share between threads; the sharded engine
 * gives each thread its own.
 */
public final class RowParser {

  private static final ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;
  private static final byte QUOTE = '"';
  private static final byte LF = '\n';

  private final Slab slab;
  private final MemorySegment seg;
  private final byte delimiter;
  /** Where each projected column sits in the file, or -1 when the file does not have it. */
  private final int[] sourceColumn;
  /** The highest source column that has to be reached before the rest of a row can be skipped. */
  private final int lastNeeded;

  private byte[] scratch = new byte[256];

  public RowParser(Slab slab, byte delimiter, int[] sourceColumn) {
    this.slab = slab;
    this.seg = slab.main();
    this.delimiter = delimiter;
    this.sourceColumn = sourceColumn.clone();
    int last = -1;
    for (int c : sourceColumn) {
      last = Math.max(last, c);
    }
    this.lastNeeded = last;
  }

  /**
   * Parses one row into {@code out} and returns the offset of the next row.
   *
   * <p>A row shorter than the header leaves the missing fields {@link Bytes#ABSENT}, which compares
   * as absent — a difference to report rather than a file to refuse.
   */
  public long parseRow(long start, long end, long[] out) {
    Arrays.fill(out, Bytes.ABSENT);
    long pos = start;
    int column = 0;

    while (pos <= end) {
      long fieldStart;
      long fieldEnd;
      long next;

      if (pos < end && seg.get(BYTE, pos) == QUOTE) {
        long close = Scanner.skipQuoted(seg, pos + 1, end);
        fieldStart = pos + 1;
        fieldEnd = Math.max(fieldStart, close - 1);
        next = Scanner.nextOf2(seg, close, end, delimiter, LF);
        store(column, quotedField(fieldStart, fieldEnd), out);
      } else {
        next = Scanner.nextOf2(seg, pos, end, delimiter, LF);
        fieldStart = pos;
        fieldEnd = next;
        store(column, plainField(fieldStart, fieldEnd), out);
      }
      column++;

      if (next >= end) {
        return end;
      }
      if (seg.get(BYTE, next) == LF) {
        return next + 1;
      }
      pos = next + 1;
      if (column > lastNeeded) {
        // Everything this comparison needs is in hand; jump to the end of the row.
        long eol = endOfRow(pos, end);
        return eol >= end ? end : eol + 1;
      }
    }
    return end;
  }

  private void store(int column, long field, long[] out) {
    if (column > lastNeeded) {
      return;
    }
    for (int i = 0; i < sourceColumn.length; i++) {
      if (sourceColumn[i] == column) {
        out[i] = field;
      }
    }
  }

  /** An unquoted field, with a trailing carriage return stripped so CRLF behaves like LF. */
  private long plainField(long from, long to) {
    long stop = to;
    if (stop > from && seg.get(BYTE, stop - 1) == '\r') {
      stop--;
    }
    return packed(from, stop);
  }

  /**
   * A quoted field, unescaped into the slab's side-buffer when it holds a doubled quote.
   *
   * <p>{@code "a""b"} has the value {@code a"b}, which is not a slice of the file, so it is the one
   * case that has to be copied. A quote inside the content range can only be half of such a pair,
   * which is what makes the test a single scan.
   */
  private long quotedField(long from, long to) {
    if (Scanner.nextOf1(seg, from, to, QUOTE) >= to) {
      return packed(from, to);
    }
    int len = (int) (to - from);
    if (scratch.length < len) {
      scratch = new byte[Math.max(len, scratch.length * 2)];
    }
    int n = 0;
    for (long i = from; i < to; i++) {
      byte b = seg.get(BYTE, i);
      scratch[n++] = b;
      if (b == QUOTE && i + 1 < to && seg.get(BYTE, i + 1) == QUOTE) {
        i++; // the second quote of the pair is not part of the value
      }
    }
    return slab.intern(scratch, n);
  }

  private long packed(long from, long to) {
    long len = to - from;
    if (len > Bytes.MAX_FIELD_LENGTH) {
      throw new CsvDiffException(
          "A field of " + len + " bytes is larger than this engine handles; use --engine native.");
    }
    return Bytes.pack(from, (int) len);
  }

  /** The offset of the newline ending the row that starts at {@code pos}. */
  private long endOfRow(long pos, long end) {
    long at = pos;
    while (at < end) {
      long next = Scanner.nextOf2(seg, at, end, LF, QUOTE);
      if (next >= end) {
        return end;
      }
      if (seg.get(BYTE, next) == QUOTE) {
        at = Scanner.skipQuoted(seg, next + 1, end);
        continue;
      }
      return next;
    }
    return end;
  }

  /** A file's header row: the column names, and where the first data row starts. */
  public record Header(List<String> names, long dataStart) {}

  /** Reads the header row without needing a parser instance. */
  public static Header header(Slab slab, byte delimiter, Options opt) {
    MemorySegment seg = slab.main();
    long end = seg.byteSize();
    var names = new ArrayList<String>();
    long pos = 0;

    while (pos <= end) {
      long from;
      long to;
      long next;
      if (pos < end && seg.get(BYTE, pos) == QUOTE) {
        long close = Scanner.skipQuoted(seg, pos + 1, end);
        from = pos + 1;
        to = Math.max(from, close - 1);
        next = Scanner.nextOf2(seg, close, end, delimiter, LF);
      } else {
        next = Scanner.nextOf2(seg, pos, end, delimiter, LF);
        from = pos;
        to = next;
      }
      if (to > from && seg.get(BYTE, to - 1) == '\r') {
        to--;
      }
      names.add(Bytes.decode(seg, from, (int) (to - from), opt));

      if (next >= end) {
        return new Header(finish(names), end);
      }
      if (seg.get(BYTE, next) == LF) {
        return new Header(finish(names), next + 1);
      }
      pos = next + 1;
    }
    return new Header(finish(names), end);
  }

  private static List<String> finish(List<String> names) {
    if (names.isEmpty()) {
      throw new CsvDiffException("File has no header row.");
    }
    return List.copyOf(names);
  }
}
