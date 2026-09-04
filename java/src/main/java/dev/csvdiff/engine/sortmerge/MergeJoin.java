package dev.csvdiff.engine.sortmerge;

import dev.csvdiff.Columns;
import dev.csvdiff.Contract.CellDiff;
import dev.csvdiff.Contract.ColumnStat;
import dev.csvdiff.Contract.Counts;
import dev.csvdiff.Contract.Section;
import dev.csvdiff.Options;
import dev.csvdiff.engine.RowStore;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.PriorityQueue;

/**
 * The join half of a sort-merge: one ordered pass down both files at once.
 *
 * <p>Every other engine in this project indexes a whole file so it can ask "is this key on the
 * other side?". This one never asks. Both sides arrive in key order, so the smaller key can only be
 * missing from the other file, equal keys are a match, and the answer falls out of walking the two
 * cursors forward. Nothing is held but the current row on each side and the capped report sections.
 *
 * <p>It produces exactly what {@link RowStore#join} produces, and the parity tests hold it to that.
 * Sections come out already sorted, because the input was.
 */
final class MergeJoin {

  private MergeJoin() {}

  /**
   * Walks two sorted cursors and builds the finished join.
   *
   * @param exporting whether {@code --export-dir} needs the uncapped rows; when it does not, each
   *     section stops growing past the report cap and memory stays flat
   */
  static RowStore.Joined join(
      Runs.Cursor a,
      Runs.Cursor b,
      Options opt,
      List<String> compared,
      long aRows,
      long bRows,
      Dups dupA,
      Dups dupB,
      boolean exporting)
      throws IOException {

    int keySize = opt.key().size();
    int nc = compared.size();
    Comparator<List<String>> order = Columns.keyOrder(keySize);

    long[] changedPer = new long[nc];
    long[] blankedPer = new long[nc];
    long[] filledPer = new long[nc];

    var changed = new Capped<List<Object>>(opt.maxRows(), exporting);
    var changedA = new Capped<List<String>>(opt.maxRows(), exporting);
    var changedB = new Capped<List<String>>(opt.maxRows(), exporting);
    var added = new Capped<List<Object>>(opt.maxRows(), exporting);
    var removed = new Capped<List<Object>>(opt.maxRows(), exporting);

    long matched = 0;
    long aKeys = 0;
    long bKeys = 0;

    List<String> ar = a.peek();
    List<String> br = b.peek();
    while (ar != null || br != null) {
      int cmp = ar == null ? 1 : br == null ? -1 : order.compare(ar, br);
      if (cmp < 0) {
        aKeys++;
        removed.add(new ArrayList<>(ar));
        ar = skipKey(a, ar, order, dupA);
      } else if (cmp > 0) {
        bKeys++;
        added.add(new ArrayList<>(br));
        br = skipKey(b, br, order, dupB);
      } else {
        aKeys++;
        bKeys++;
        matched++;
        List<CellDiff> cells = null;
        for (int i = 0; i < nc; i++) {
          String x = ar.get(keySize + i);
          String y = br.get(keySize + i);
          if (Columns.differs(x, y, opt)) {
            if (cells == null) {
              cells = new ArrayList<>();
            }
            cells.add(new CellDiff(i, x, y));
            changedPer[i]++;
            if (y == null) {
              blankedPer[i]++;
            }
            if (x == null) {
              filledPer[i]++;
            }
          }
        }
        if (cells != null) {
          List<Object> row = Columns.keyValues(ar, keySize);
          row.add(cells);
          changed.add(row);
          changedA.add(ar);
          changedB.add(br);
        }
        ar = skipKey(a, ar, order, dupA);
        br = skipKey(b, br, order, dupB);
      }
    }

    var columns = new ArrayList<ColumnStat>(nc);
    for (int i = 0; i < nc; i++) {
      columns.add(new ColumnStat(compared.get(i), changedPer[i], blankedPer[i], filledPer[i]));
    }

    var counts =
        new Counts(
            aRows, bRows, aKeys, bKeys,
            matched, matched - changed.total(), changed.total(), added.total(), removed.total(),
            dupA.keys(), dupA.rows(), dupB.keys(), dupB.rows());

    return new RowStore.Joined(
        counts, List.copyOf(columns),
        changed.held(), added.held(), removed.held(), changedA.held(), changedB.held());
  }

  /**
   * Advances past every row sharing the current key, counting the repeats as duplicates.
   *
   * <p>The first row of a run is the one the join used, which is first-occurrence-wins: the sort is
   * stable and the merge breaks ties by run, so file order survives it.
   *
   * @return the first row of the next key, or {@code null} at the end
   */
  private static List<String> skipKey(
      Runs.Cursor cursor, List<String> current, Comparator<List<String>> order, Dups dups)
      throws IOException {
    cursor.next();
    int repeats = 0;
    List<String> next = cursor.peek();
    while (next != null && order.compare(current, next) == 0) {
      repeats++;
      cursor.next();
      next = cursor.peek();
    }
    if (repeats > 0) {
      dups.record(current, repeats + 1);
    }
    return next;
  }

  /**
   * A row list that stops growing at the report cap but keeps counting.
   *
   * <p>The counts in the contract are always exact and the embedded rows are always capped, so
   * holding more than the cap only ever serves {@code --export-dir}. Not holding them is what lets
   * this engine compare a file far larger than the heap.
   */
  private static final class Capped<T> {
    private final List<T> held = new ArrayList<>();
    private final int cap;
    private final boolean unbounded;
    private long total;

    Capped(int cap, boolean unbounded) {
      this.cap = cap;
      this.unbounded = unbounded;
    }

    void add(T row) {
      total++;
      // One past the cap, so a section can still report that it was truncated.
      if (unbounded || held.size() <= cap) {
        held.add(row);
      }
    }

    long total() {
      return total;
    }

    List<T> held() {
      return held;
    }
  }

  /**
   * The duplicate-key report, kept to the top {@code --max-rows} by count.
   *
   * <p>A heap of the best few, rather than every duplicate, so a pathological file where every key
   * repeats does not undo the memory bound. Ordering matches every other engine: most duplicated
   * first, then by key.
   */
  static final class Dups {
    private final List<String> keyCols;
    private final int keySize;
    private final int cap;
    private final Comparator<Entry> weakestFirst;
    private final PriorityQueue<Entry> best;
    private long keys;
    private long rows;

    private record Entry(List<Object> key, long count, List<String> row) {}

    Dups(List<String> keyCols, int cap) {
      this.keyCols = keyCols;
      this.keySize = keyCols.size();
      this.cap = cap;
      Comparator<List<String>> byKey = Columns.keyOrder(keySize);
      this.weakestFirst =
          Comparator.<Entry>comparingLong(Entry::count)
              .thenComparing(Entry::row, byKey.reversed());
      this.best = new PriorityQueue<>(weakestFirst);
    }

    void record(List<String> row, int count) {
      keys++;
      rows += count;
      best.add(new Entry(Columns.keyValues(row, keySize), count, row));
      if (best.size() > cap + 1) {
        best.poll();
      }
    }

    long keys() {
      return keys;
    }

    long rows() {
      return rows;
    }

    Section section() {
      var all = new ArrayList<>(best);
      all.sort(weakestFirst.reversed());
      var rowsOut = new ArrayList<List<Object>>(Math.min(all.size(), cap));
      for (Entry e : all.subList(0, Math.min(all.size(), cap))) {
        var out = new ArrayList<>(e.key());
        out.add(e.count());
        rowsOut.add(out);
      }
      var cols = new ArrayList<>(keyCols);
      cols.add("count");
      return new Section(List.copyOf(cols), List.copyOf(rowsOut), keys > cap);
    }
  }
}
