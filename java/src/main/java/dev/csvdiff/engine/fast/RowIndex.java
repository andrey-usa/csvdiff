package dev.csvdiff.engine.fast;

import dev.csvdiff.Options;
import java.util.Arrays;

/**
 * One file, indexed by composite key, in primitive arrays.
 *
 * <p>The in-heap engines build a {@code LinkedHashMap<String, List<String>>}: at ten million rows
 * that is a hash node, a String key, a list and twenty Strings per row. Here the same information
 * is four {@code long[]} and an open-addressed table — no objects, no boxing, and the row bytes
 * stay in the file.
 *
 * <p>What it holds per row is the row's start offset and its key hash. Fields are re-split from the
 * offset when they are actually needed, which is cheaper than storing a field table for every
 * column of every row: a row is a couple of hundred bytes and splitting it is a handful of vector
 * comparisons, against 152 bytes of index per row for a nineteen-column projection.
 *
 * <p>The table maps a key to the <em>first</em> row that carries it, because first occurrence wins
 * the join; later rows with the same key are counted for the duplicate report and otherwise
 * ignored.
 */
public final class RowIndex {

  /** Load factor: 0.5, so probe chains stay short at the cost of a bigger table. */
  private static final int EMPTY = -1;

  private final Slab slab;
  private final Scan scan;
  private final Options opt;
  private final int keySize;
  private final int width;
  private final byte delimiter;
  private final int[] sourceColumn;

  private long[] rowStart;
  private long[] rowHash;
  private int rowCount;

  /** Open-addressed, power-of-two sized, holding row indexes and {@link #EMPTY}. */
  private int[] table;
  private int mask;

  /** How many rows carry each distinct key, parallel to the first-occurrence row list. */
  private int[] occurrences;
  private int[] firstRow;
  private int keyCount;

  private long duplicateKeys;
  private long duplicateRows;

  /** Scratch for the probe path, hoisted so a ten-million-row build allocates nothing per row. */
  private RowParser probeParser;
  private long[] probeFields;
  private RowParser mergeParser;
  private long[] mergeFields;
  private RowParser lookupParser;
  private long[] lookupFields;

  public RowIndex(
      Slab slab, Scan scan, Options opt, byte delimiter, int[] sourceColumn, int width,
      long expectedRows) {
    this.slab = slab;
    this.scan = scan;
    this.opt = opt;
    this.keySize = opt.key().size();
    this.width = width;
    this.delimiter = delimiter;
    this.sourceColumn = sourceColumn;

    int initial = (int) Math.min(Integer.MAX_VALUE - 8, Math.max(16, expectedRows + 16));
    this.rowStart = new long[initial];
    this.rowHash = new long[initial];
    this.firstRow = new int[initial];
    this.occurrences = new int[initial];

    int capacity = Integer.highestOneBit(Math.max(16, (int) Math.min(1 << 30, expectedRows * 2))) * 2;
    this.table = new int[capacity];
    Arrays.fill(this.table, EMPTY);
    this.mask = capacity - 1;
  }

  public Slab slab() {
    return slab;
  }

  public int rows() {
    return rowCount;
  }

  /** How many compared columns this index was projected for. */
  public int comparedCount() {
    return width - keySize;
  }

  public long uniqueKeys() {
    return keyCount;
  }

  public long duplicateKeys() {
    return duplicateKeys;
  }

  public long duplicateRows() {
    return duplicateRows;
  }

  public long rowStart(int row) {
    return rowStart[row];
  }

  /** A fresh parser over this index's file, for a thread that needs one. */
  public RowParser parser() {
    return new RowParser(slab, scan, delimiter, sourceColumn);
  }

  /** Splits row {@code row} into {@code out}. */
  public void fields(RowParser parser, int row, long[] out) {
    parser.parseRow(rowStart[row], slab.size(), out);
  }

  /**
   * Reads the whole file in, filling the row arrays and the key table.
   *
   * <p>One pass: split the key columns, hash them, probe, and record. Non-key columns are not even
   * delimited here — they are split later, and only for the rows that turn out to matter.
   */
  public void build(long from) {
    RowParser parser = parser();
    probeParser = parser();
    probeFields = new long[width];
    long[] fields = new long[width];
    var seg = slab.main();
    long end = slab.size();
    long pos = from;

    while (pos < end) {
      // A line with nothing on it is not a row, which is what every other reader here does.
      byte b = seg.get(java.lang.foreign.ValueLayout.JAVA_BYTE, pos);
      if (b == '\n') {
        pos++;
        continue;
      }
      if (b == '\r' && pos + 1 < end
          && seg.get(java.lang.foreign.ValueLayout.JAVA_BYTE, pos + 1) == '\n') {
        pos += 2;
        continue;
      }
      long next = parser.parseRow(pos, end, fields);
      add(pos, fields);
      if (next <= pos) {
        break; // no progress; a malformed tail rather than an infinite loop
      }
      pos = next;
    }
    compact();
  }

  private void add(long start, long[] fields) {
    if (rowCount == rowStart.length) {
      grow();
    }
    long hash = keyHash(fields);
    rowStart[rowCount] = start;
    rowHash[rowCount] = hash;
    probeAndRecord(rowCount, hash, fields);
    rowCount++;
  }

  /** The composite key's hash: each column folded into the running value in order. */
  private long keyHash(long[] fields) {
    long h = Bytes.seed();
    for (int i = 0; i < keySize; i++) {
      h = Bytes.hash(slab, fields[i], opt, h);
    }
    return h;
  }

  /** Gets the scratch parsers ready for a merge that did not go through {@link #build}. */
  void prepareMerge() {
    probeParser = parser();
    probeFields = new long[width];
    mergeParser = parser();
    mergeFields = new long[width];
  }

  /**
   * Appends one shard's rows, in file order.
   *
   * <p>The hashes were computed by the shard's own thread; all this does is place them, which is
   * why the serial part of a parallel build is small. A row is only re-parsed when its hash lands
   * on an occupied slot, which for a 64-bit hash means a genuine duplicate key.
   */
  void appendShard(long[] starts, long[] hashes, int count) {
    for (int i = 0; i < count; i++) {
      if (rowCount == rowStart.length) {
        grow();
      }
      rowStart[rowCount] = starts[i];
      rowHash[rowCount] = hashes[i];
      probeAndRecordMerged(rowCount, hashes[i]);
      rowCount++;
    }
  }

  /** Trims the arrays once every shard is in. */
  void finishMerge() {
    compact();
  }

  /** The insert path for a merged row, whose fields are not to hand. */
  private void probeAndRecordMerged(int row, long hash) {
    int slot = slot(hash);
    while (true) {
      int at = table[slot];
      if (at == EMPTY) {
        insert(slot, row);
        return;
      }
      int candidate = firstRow[at];
      if (rowHash[candidate] == hash && sameKeyRows(candidate, row)) {
        countDuplicate(at);
        return;
      }
      slot = (slot + 1) & mask;
    }
  }

  /** Whether two rows of this file carry the same composite key. */
  private boolean sameKeyRows(int left, int right) {
    probeParser.parseRow(rowStart[left], slab.size(), probeFields);
    mergeParser.parseRow(rowStart[right], slab.size(), mergeFields);
    for (int i = 0; i < keySize; i++) {
      if (!Bytes.equal(slab, probeFields[i], slab, mergeFields[i], opt)) {
        return false;
      }
    }
    return true;
  }

  private void probeAndRecord(int row, long hash, long[] fields) {
    int slot = slot(hash);
    while (true) {
      int at = table[slot];
      if (at == EMPTY) {
        insert(slot, row);
        return;
      }
      int candidate = firstRow[at];
      if (rowHash[candidate] == hash && sameKey(candidate, fields)) {
        countDuplicate(at);
        return;
      }
      slot = (slot + 1) & mask;
    }
  }

  /** Claims an empty slot for a key seen for the first time. */
  private void insert(int slot, int row) {
    table[slot] = keyCount;
    firstRow[keyCount] = row;
    occurrences[keyCount] = 1;
    keyCount++;
    if (keyCount == firstRow.length) {
      firstRow = Arrays.copyOf(firstRow, firstRow.length * 2);
      occurrences = Arrays.copyOf(occurrences, occurrences.length * 2);
    }
    if (keyCount * 2 > table.length) {
      rehash();
    }
  }

  /** Counts another row on a key already seen. */
  private void countDuplicate(int key) {
    occurrences[key]++;
    if (occurrences[key] == 2) {
      duplicateKeys++;
      duplicateRows++; // the first occurrence counts too, once the key is known to repeat
    }
    duplicateRows++;
  }

  private boolean sameKey(int candidateRow, long[] fields) {
    probeParser.parseRow(rowStart[candidateRow], slab.size(), probeFields);
    for (int i = 0; i < keySize; i++) {
      if (!Bytes.equal(slab, probeFields[i], slab, fields[i], opt)) {
        return false;
      }
    }
    return true;
  }

  private int slot(long hash) {
    // The high bits of an FNV hash are the well-mixed ones; fold them down before masking.
    return (int) ((hash ^ (hash >>> 32)) & mask);
  }

  private void grow() {
    int size = rowStart.length * 2;
    rowStart = Arrays.copyOf(rowStart, size);
    rowHash = Arrays.copyOf(rowHash, size);
  }

  private void rehash() {
    int[] bigger = new int[table.length * 2];
    Arrays.fill(bigger, EMPTY);
    int newMask = bigger.length - 1;
    for (int key = 0; key < keyCount; key++) {
      long hash = rowHash[firstRow[key]];
      int slot = (int) ((hash ^ (hash >>> 32)) & newMask);
      while (bigger[slot] != EMPTY) {
        slot = (slot + 1) & newMask;
      }
      bigger[slot] = key;
    }
    table = bigger;
    mask = newMask;
  }

  private void compact() {
    if (rowStart.length > rowCount) {
      rowStart = Arrays.copyOf(rowStart, rowCount);
      rowHash = Arrays.copyOf(rowHash, rowCount);
    }
  }

  /**
   * The row that owns a key, or -1 when this file does not have it.
   *
   * <p>The candidate row is re-split with this index's own parser: a parser is bound to the slab it
   * was made for, and the key being looked up comes from the other file.
   */
  public int lookup(Slab otherSlab, long[] keyFields, long hash) {
    if (lookupParser == null) {
      lookupParser = parser();
      lookupFields = new long[width];
    }
    int slot = slot(hash);
    while (true) {
      int at = table[slot];
      if (at == EMPTY) {
        return -1;
      }
      int candidate = firstRow[at];
      if (rowHash[candidate] == hash) {
        lookupParser.parseRow(rowStart[candidate], slab.size(), lookupFields);
        boolean same = true;
        for (int i = 0; i < keySize && same; i++) {
          same = Bytes.equal(slab, lookupFields[i], otherSlab, keyFields[i], opt);
        }
        if (same) {
          return candidate;
        }
      }
      slot = (slot + 1) & mask;
    }
  }

  /** The hash of a key already split into {@code fields} of another file. */
  public long hashOf(Slab otherSlab, long[] fields) {
    long h = Bytes.seed();
    for (int i = 0; i < keySize; i++) {
      h = Bytes.hash(otherSlab, fields[i], opt, h);
    }
    return h;
  }

  /** Distinct keys in first-appearance order, as the rows that own them. */
  public int[] firstRows() {
    return Arrays.copyOf(firstRow, keyCount);
  }

  /** How many rows carry each distinct key, parallel to {@link #firstRows()}. */
  public int[] counts() {
    return Arrays.copyOf(occurrences, keyCount);
  }
}
