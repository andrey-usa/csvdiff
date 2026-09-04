package dev.csvdiff.engine.sortmerge;

import dev.csvdiff.Columns;
import dev.csvdiff.CsvDiffException;
import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.PriorityQueue;

/**
 * External sort: buffer rows until a byte budget is reached, sort that batch, spill it, then merge
 * the spilled runs back as one ordered stream.
 *
 * <p>This is the half of a sort-merge join that keeps memory flat. Nothing here holds more than one
 * batch plus one row per open run, so a file ten times larger costs ten times the disk and the same
 * heap. The batches are sorted with {@link Columns#keyOrder}, which is also the order the report
 * sections are written in, so the join downstream produces them already sorted and never has to
 * hold a section to sort it.
 *
 * <p>Ties keep file order. Batches are filled in file order and sorted stably, and the merge breaks
 * equal keys by run number, so the first row carrying a key is still the first row the join sees —
 * which is what makes first-occurrence-wins mean the same thing here as in every other engine.
 */
public final class Runs implements AutoCloseable {

  /**
   * How much heap a batch may occupy before it is spilled.
   *
   * <p>This is a budget for the batch's real cost, not for the bytes in the file. A row arrives as
   * a list of Strings, and on a 64-bit JVM each of those carries an object header, a length and a
   * byte array of its own, so a 180-byte CSV row occupies closer to a kilobyte once it is parsed.
   * Charging only the characters — which is what this class did first — puts five times more in the
   * batch than the number suggests and hands back an OutOfMemoryError on the heap the budget
   * claimed to fit.
   */
  private static final int DEFAULT_BATCH_BYTES = 32 * 1024 * 1024;

  /** Object header, length, hash and the reference to the value array, per String. */
  private static final int STRING_OVERHEAD = 48;

  /** The row's own list and its backing array. */
  private static final int ROW_OVERHEAD = 64;

  /**
   * Overrides {@link #DEFAULT_BATCH_BYTES}, so the spill and merge path can be exercised on a file
   * small enough to assert about. A comparison run below the default budget never spills at all,
   * which would leave the interesting half of this class untested.
   */
  public static final String BATCH_BYTES_PROPERTY = "csvdiff.sortmerge.batchBytes";

  /**
   * Read per instance rather than once per class, so the budget is whatever it is when a
   * comparison starts. As a static initialiser it would be frozen at whichever moment this class
   * happened to load, which in a long-lived process is a moment nothing controls.
   */
  private static int batchBytes() {
    String override = System.getProperty(BATCH_BYTES_PROPERTY);
    if (override == null) {
      return DEFAULT_BATCH_BYTES;
    }
    try {
      int value = Integer.parseInt(override.trim());
      if (value <= 0) {
        throw new NumberFormatException(override);
      }
      return value;
    } catch (NumberFormatException e) {
      throw new CsvDiffException(
          BATCH_BYTES_PROPERTY + " must be a positive number of bytes, got " + override);
    }
  }

  /** Marks an absent field, which is not the same as an empty one. */
  private static final int NULL_FIELD = -1;

  private final int batchBytes = batchBytes();
  private final Path dir;
  private final int width;
  private final Comparator<List<String>> order;
  private final List<Path> spilled = new ArrayList<>();

  private List<List<String>> batch = new ArrayList<>();
  private long held;
  private long rows;

  public Runs(Path dir, int width, int keySize) {
    this.dir = dir;
    this.width = width;
    this.order = Columns.keyOrder(keySize);
  }

  /**
   * Adds one normalised row, spilling the current batch first if it is full.
   *
   * @param row key columns then compared columns, exactly {@code width} of them
   */
  public void add(List<String> row) throws IOException {
    batch.add(row);
    rows++;
    held += ROW_OVERHEAD;
    for (String v : row) {
      // A null field costs only the reference already counted in the row's backing array.
      held += v == null ? 0 : STRING_OVERHEAD + v.length();
    }
    if (held >= batchBytes) {
      spill();
    }
  }

  /** Rows added so far, which is the file's row count once reading has finished. */
  public long rows() {
    return rows;
  }

  /** How many batches went to disk; zero means the sort stayed in memory. */
  public int spills() {
    return spilled.size();
  }

  /**
   * Finishes the sort and returns the rows in key order.
   *
   * <p>A sort that never spilled is returned straight from the heap, so the common case of a file
   * that fits pays nothing for the machinery that handles one that does not.
   */
  public Cursor sorted() throws IOException {
    if (spilled.isEmpty()) {
      batch.sort(order);
      var inMemory = batch;
      batch = new ArrayList<>();
      return new ListCursor(inMemory);
    }
    spill();
    return new MergeCursor(spilled, width, order);
  }

  private void spill() throws IOException {
    if (batch.isEmpty()) {
      return;
    }
    batch.sort(order);
    Path path = dir.resolve("run-" + spilled.size() + ".bin");
    try (OutputStream out = new BufferedOutputStream(Files.newOutputStream(path), 1 << 16)) {
      for (List<String> row : batch) {
        writeRow(out, row);
      }
    }
    spilled.add(path);
    batch = new ArrayList<>();
    held = 0;
  }

  @Override
  public void close() {
    for (Path p : spilled) {
      try {
        Files.deleteIfExists(p);
      } catch (IOException e) {
        // A leftover run in a temp directory is not worth failing a finished comparison over.
      }
    }
    spilled.clear();
  }

  /** One ordered pass over sorted rows. */
  public interface Cursor extends AutoCloseable {

    /** The row at the cursor, or {@code null} once the rows are exhausted. */
    List<String> peek();

    /** Advances past the current row. */
    void next() throws IOException;

    @Override
    void close();
  }

  private static final class ListCursor implements Cursor {
    private final List<List<String>> rows;
    private int at;

    ListCursor(List<List<String>> rows) {
      this.rows = rows;
    }

    @Override
    public List<String> peek() {
      return at < rows.size() ? rows.get(at) : null;
    }

    @Override
    public void next() {
      at++;
    }

    @Override
    public void close() {
      rows.clear();
    }
  }

  /** A k-way merge over the spilled runs, holding one row from each. */
  private static final class MergeCursor implements Cursor {
    private final List<InputStream> streams = new ArrayList<>();
    private final PriorityQueue<Head> queue;
    private final int width;

    private record Head(List<String> row, int run) {}

    MergeCursor(List<Path> paths, int width, Comparator<List<String>> order) throws IOException {
      this.width = width;
      this.queue =
          new PriorityQueue<>(
              Math.max(1, paths.size()),
              Comparator.<Head, List<String>>comparing(Head::row, order)
                  .thenComparingInt(Head::run));
      for (Path p : paths) {
        streams.add(new BufferedInputStream(Files.newInputStream(p), 1 << 16));
      }
      for (int run = 0; run < streams.size(); run++) {
        pull(run);
      }
    }

    private void pull(int run) throws IOException {
      List<String> row = readRow(streams.get(run), width);
      if (row != null) {
        queue.add(new Head(row, run));
      }
    }

    @Override
    public List<String> peek() {
      Head head = queue.peek();
      return head == null ? null : head.row();
    }

    @Override
    public void next() throws IOException {
      Head head = queue.poll();
      if (head != null) {
        pull(head.run());
      }
    }

    @Override
    public void close() {
      for (InputStream in : streams) {
        try {
          in.close();
        } catch (IOException e) {
          // Closing a finished read-only stream cannot lose data.
        }
      }
      streams.clear();
      queue.clear();
    }
  }

  // -------------------------------------------------------------------------
  // The spill format: a length-prefixed field per column, varint lengths.
  // -------------------------------------------------------------------------

  private static void writeRow(OutputStream out, List<String> row) throws IOException {
    for (String v : row) {
      if (v == null) {
        writeVarInt(out, 0);
        continue;
      }
      byte[] bytes = v.getBytes(StandardCharsets.UTF_8);
      writeVarInt(out, bytes.length + 1);
      out.write(bytes);
    }
  }

  private static List<String> readRow(InputStream in, int width) throws IOException {
    var row = new ArrayList<String>(width);
    for (int i = 0; i < width; i++) {
      int len = readVarInt(in, i == 0);
      if (len == NULL_FIELD) {
        return null;
      }
      if (len == 0) {
        row.add(null);
        continue;
      }
      row.add(new String(in.readNBytes(len - 1), StandardCharsets.UTF_8));
    }
    return row;
  }

  /** Zero is an absent field, so lengths are stored one higher and never collide with it. */
  private static void writeVarInt(OutputStream out, int value) throws IOException {
    int v = value;
    while ((v & ~0x7F) != 0) {
      out.write((v & 0x7F) | 0x80);
      v >>>= 7;
    }
    out.write(v);
  }

  private static int readVarInt(InputStream in, boolean eofAllowed) throws IOException {
    int result = 0;
    for (int shift = 0; shift < 35; shift += 7) {
      int b = in.read();
      if (b < 0) {
        if (shift == 0 && eofAllowed) {
          return NULL_FIELD;
        }
        throw new EOFException("Spilled run ended mid-row");
      }
      result |= (b & 0x7F) << shift;
      if ((b & 0x80) == 0) {
        return result;
      }
    }
    throw new CsvDiffException("Corrupt spilled run: field length is not a varint");
  }
}
