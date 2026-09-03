package dev.csvdiff.engine;

import dev.csvdiff.Columns;
import dev.csvdiff.Contract.CellDiff;
import dev.csvdiff.Contract.ColumnStat;
import dev.csvdiff.Contract.Counts;
import dev.csvdiff.Contract.Section;
import dev.csvdiff.Options;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * The in-memory half of a comparison, shared by the engines that hold both files in the heap.
 *
 * <p>It owns the parts that must not drift between engines — first-occurrence-wins de-duplication,
 * the duplicate-key report, the full outer join and the sparse cell diffs — so an engine only has
 * to supply normalised rows. The DuckDB engine does all of this in SQL instead and does not use
 * this class.
 */
public final class RowStore {

  private final Options opt;
  private final List<String> key;
  private final List<String> compared;
  private final int keySize;
  private final int comparedSize;

  /** First occurrence per key; later rows with the same key are counted but do not join. */
  private final Map<String, List<String>> first = new LinkedHashMap<>();

  private final Map<String, Integer> occurrences = new LinkedHashMap<>();
  private long rowCount;

  public RowStore(Options opt, List<String> compared) {
    this.opt = opt;
    this.key = opt.key();
    this.compared = compared;
    this.keySize = key.size();
    this.comparedSize = compared.size();
  }

  /** Adds one already-normalised row: key columns first, then the compared columns. */
  public void add(List<String> row) {
    rowCount++;
    String k = Columns.keyOf(row, keySize);
    occurrences.merge(k, 1, Integer::sum);
    first.putIfAbsent(k, row);
  }

  public long rows() {
    return rowCount;
  }

  public long uniqueKeys() {
    return occurrences.size();
  }

  public long duplicateKeys() {
    return occurrences.values().stream().filter(n -> n > 1).count();
  }

  public long duplicateRows() {
    return occurrences.values().stream().filter(n -> n > 1).mapToLong(Integer::longValue).sum();
  }

  /** The duplicate-key list: most duplicated first, then by key. */
  public Section duplicateSection() {
    var entries = new ArrayList<Map.Entry<String, Integer>>();
    for (var e : occurrences.entrySet()) {
      if (e.getValue() > 1) {
        entries.add(e);
      }
    }
    Comparator<List<String>> byKey = Columns.keyOrder(keySize);
    entries.sort(
        Comparator.<Map.Entry<String, Integer>>comparingInt(Map.Entry::getValue)
            .reversed()
            .thenComparing(e -> first.get(e.getKey()), byKey));

    var rows = new ArrayList<List<Object>>(Math.min(entries.size(), opt.maxRows()));
    for (var e : entries.subList(0, Math.min(entries.size(), opt.maxRows()))) {
      List<Object> row = Columns.keyValues(first.get(e.getKey()), keySize);
      row.add(e.getValue().longValue());
      rows.add(row);
    }
    var cols = new ArrayList<>(key);
    cols.add("count");
    return new Section(List.copyOf(cols), List.copyOf(rows), entries.size() > opt.maxRows());
  }

  /** Everything the join produces, before the sections are capped. */
  public record Joined(
      Counts counts,
      List<ColumnStat> columns,
      List<List<Object>> changed,
      List<List<Object>> added,
      List<List<Object>> removed,
      List<List<String>> changedA,
      List<List<String>> changedB) {}

  /** Full outer join of two stores on the composite key. */
  public static Joined join(RowStore a, RowStore b, Options opt) {
    int keySize = opt.key().size();
    int nc = a.comparedSize;

    var columns = new ArrayList<ColumnStat>(nc);
    long[] changedPer = new long[nc];
    long[] blankedPer = new long[nc];
    long[] filledPer = new long[nc];

    var changed = new ArrayList<List<Object>>();
    var changedA = new ArrayList<List<String>>();
    var changedB = new ArrayList<List<String>>();
    var added = new ArrayList<List<Object>>();
    var removed = new ArrayList<List<Object>>();
    long matched = 0;

    for (var e : a.first.entrySet()) {
      List<String> br = b.first.get(e.getKey());
      List<String> ar = e.getValue();
      if (br == null) {
        removed.add(new ArrayList<>(ar));
        continue;
      }
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
    }
    for (var e : b.first.entrySet()) {
      if (!a.first.containsKey(e.getKey())) {
        added.add(new ArrayList<>(e.getValue()));
      }
    }

    for (int i = 0; i < nc; i++) {
      columns.add(new ColumnStat(a.compared.get(i), changedPer[i], blankedPer[i], filledPer[i]));
    }

    // Sort every section by key, and keep the changed A/B row views aligned with it.
    Comparator<List<String>> byKey = Columns.keyOrder(keySize);
    sortRowsByKey(added, keySize);
    sortRowsByKey(removed, keySize);
    sortChangedTogether(changed, changedA, changedB, byKey, keySize);

    var counts =
        new Counts(
            a.rows(), b.rows(), a.uniqueKeys(), b.uniqueKeys(),
            matched, matched - changed.size(), changed.size(), added.size(), removed.size(),
            a.duplicateKeys(), a.duplicateRows(), b.duplicateKeys(), b.duplicateRows());

    return new Joined(counts, List.copyOf(columns), changed, added, removed, changedA, changedB);
  }

  private static void sortRowsByKey(List<List<Object>> rows, int keySize) {
    rows.sort(
        (x, y) -> {
          for (int i = 0; i < keySize; i++) {
            String a = (String) x.get(i);
            String b = (String) y.get(i);
            if (a == null && b == null) {
              continue;
            }
            if (a == null) {
              return 1;
            }
            if (b == null) {
              return -1;
            }
            int c = a.compareTo(b);
            if (c != 0) {
              return c;
            }
          }
          return 0;
        });
  }

  /** Sorts the changed rows by key while keeping the parallel A and B row lists in step. */
  private static void sortChangedTogether(
      List<List<Object>> changed,
      List<List<String>> changedA,
      List<List<String>> changedB,
      Comparator<List<String>> byKey,
      int keySize) {
    Integer[] order = new Integer[changed.size()];
    for (int i = 0; i < order.length; i++) {
      order[i] = i;
    }
    java.util.Arrays.sort(order, (p, q) -> byKey.compare(changedA.get(p), changedA.get(q)));

    var c = new ArrayList<List<Object>>(changed.size());
    var ca = new ArrayList<List<String>>(changed.size());
    var cb = new ArrayList<List<String>>(changed.size());
    for (int i : order) {
      c.add(changed.get(i));
      ca.add(changedA.get(i));
      cb.add(changedB.get(i));
    }
    changed.clear();
    changed.addAll(c);
    changedA.clear();
    changedA.addAll(ca);
    changedB.clear();
    changedB.addAll(cb);
  }

  /** Caps a row list at {@code --max-rows} and records whether anything was cut. */
  public static Section section(List<String> cols, List<List<Object>> rows, int maxRows) {
    boolean truncated = rows.size() > maxRows;
    return new Section(cols, List.copyOf(rows.subList(0, Math.min(rows.size(), maxRows))), truncated);
  }
}
