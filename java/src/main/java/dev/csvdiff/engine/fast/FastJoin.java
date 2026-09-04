package dev.csvdiff.engine.fast;

import dev.csvdiff.Columns;
import dev.csvdiff.Contract.CellDiff;
import dev.csvdiff.Contract.ColumnStat;
import dev.csvdiff.Contract.Counts;
import dev.csvdiff.Contract.Section;
import dev.csvdiff.Options;
import dev.csvdiff.engine.RowStore;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;

/**
 * The full outer join, over byte fields rather than Strings.
 *
 * <p>It produces exactly what {@link RowStore#join} produces — the same counts, the same column
 * stats, the same rows in the same order — and the parity tests hold it to that. What differs is
 * the cost: nothing becomes a String until a row is known to be going into the report, and the
 * report holds at most {@code --max-rows} of them.
 *
 * <p>That is the whole design. A ten-million-row comparison finds six hundred thousand changed rows
 * and embeds fifty thousand, so it needs a tiny fraction of the objects a row-at-a-time engine
 * builds — and the ones it does build are bounded by the report size rather than by the file.
 */
public final class FastJoin {

  private FastJoin() {}

  /** A row chosen for a report section, with the row it matched on the other side. */
  private record Pick(int row, int mate, List<String> key) {}

  /** A duplicated key: the row that first carried it, and how many rows do. */
  private record Dup(int row, int count, List<String> key) {}

  /** Joins two indexed files. */
  public static RowStore.Joined join(RowIndex a, RowIndex b, Options opt, List<String> compared) {
    int keySize = opt.key().size();
    int nc = compared.size();
    int width = keySize + nc;

    RowParser pa = a.parser();
    RowParser pb = b.parser();
    long[] fa = new long[width];
    long[] fb = new long[width];

    long[] changedPer = new long[nc];
    long[] blankedPer = new long[nc];
    long[] filledPer = new long[nc];

    var changed = new ArrayList<Pick>();
    var removed = new ArrayList<Pick>();
    var added = new ArrayList<Pick>();
    long matched = 0;

    // A's distinct keys, in first-appearance order, so a run is reproducible.
    for (int row : a.firstRows()) {
      a.fields(pa, row, fa);
      long hash = b.hashOf(a.slab(), fa);
      int match = b.lookup(a.slab(), fa, hash);
      if (match < 0) {
        removed.add(new Pick(row, -1, key(a, fa, keySize, opt)));
        continue;
      }
      matched++;
      b.fields(pb, match, fb);

      boolean any = false;
      for (int i = 0; i < nc; i++) {
        long x = fa[keySize + i];
        long y = fb[keySize + i];
        if (Bytes.differs(a.slab(), x, b.slab(), y, opt)) {
          any = true;
          changedPer[i]++;
          if (Bytes.isNull(b.slab(), y, opt)) {
            blankedPer[i]++;
          }
          if (Bytes.isNull(a.slab(), x, opt)) {
            filledPer[i]++;
          }
        }
      }
      if (any) {
        changed.add(new Pick(row, match, key(a, fa, keySize, opt)));
      }
    }

    // B's distinct keys that A does not have.
    for (int row : b.firstRows()) {
      b.fields(pb, row, fb);
      long hash = a.hashOf(b.slab(), fb);
      if (a.lookup(b.slab(), fb, hash) < 0) {
        added.add(new Pick(row, -1, key(b, fb, keySize, opt)));
      }
    }

    var columns = new ArrayList<ColumnStat>(nc);
    for (int i = 0; i < nc; i++) {
      columns.add(new ColumnStat(compared.get(i), changedPer[i], blankedPer[i], filledPer[i]));
    }

    // Ordering is part of the contract, so it uses the same comparator as every other engine.
    Comparator<Pick> byKey = Comparator.comparing(Pick::key, Columns.keyOrder(keySize));
    changed.sort(byKey);
    removed.sort(byKey);
    added.sort(byKey);

    boolean exporting = opt.exportDir() != null;
    int cap = opt.maxRows();

    var changedRows = new ArrayList<List<Object>>();
    var changedA = new ArrayList<List<String>>();
    var changedB = new ArrayList<List<String>>();
    int changedLimit = materialise(changed.size(), cap, exporting);
    for (int i = 0; i < changedLimit; i++) {
      Pick pick = changed.get(i);
      a.fields(pa, pick.row(), fa);
      b.fields(pb, pick.mate(), fb);
      var row = new ArrayList<Object>(pick.key());
      row.add(cells(a, b, fa, fb, keySize, nc, opt));
      changedRows.add(frozen(row));
      if (exporting) {
        changedA.add(values(a, fa, width, opt));
        changedB.add(values(b, fb, width, opt));
      }
    }

    var counts =
        new Counts(
            a.rows(), b.rows(), a.uniqueKeys(), b.uniqueKeys(),
            matched, matched - changed.size(), changed.size(), added.size(), removed.size(),
            a.duplicateKeys(), a.duplicateRows(), b.duplicateKeys(), b.duplicateRows());

    return new RowStore.Joined(
        counts,
        List.copyOf(columns),
        frozen(changedRows),
        sideRows(added, b, pb, fb, width, cap, exporting, opt),
        sideRows(removed, a, pa, fa, width, cap, exporting, opt),
        frozen(changedA),
        frozen(changedB));
  }

  /**
   * The duplicate-key list for one side: most duplicated first, then by key.
   *
   * <p>Same ordering rule as {@link RowStore#duplicateSection()}, including the stable tie-break on
   * first appearance, because the index keeps its keys in that order.
   */
  public static Section duplicates(RowIndex side, Options opt) {
    int keySize = opt.key().size();
    int width = keySize + side.comparedCount();
    RowParser parser = side.parser();
    long[] fields = new long[width];

    int[] rows = side.firstRows();
    int[] counts = side.counts();
    var entries = new ArrayList<Dup>();
    for (int i = 0; i < rows.length; i++) {
      if (counts[i] > 1) {
        side.fields(parser, rows[i], fields);
        entries.add(new Dup(rows[i], counts[i], key(side, fields, keySize, opt)));
      }
    }
    entries.sort(
        Comparator.comparingInt(Dup::count).reversed()
            .thenComparing(Dup::key, Columns.keyOrder(keySize)));

    int limit = Math.min(entries.size(), opt.maxRows());
    var out = new ArrayList<List<Object>>(limit);
    for (Dup entry : entries.subList(0, limit)) {
      var row = new ArrayList<Object>(entry.key());
      row.add((long) entry.count());
      out.add(frozen(row));
    }
    var cols = new ArrayList<>(opt.key());
    cols.add("count");
    return new Section(List.copyOf(cols), frozen(out), entries.size() > opt.maxRows());
  }

  /**
   * How many rows of a section to build.
   *
   * <p>One more than the cap, not the cap itself: the caller trims the list and decides from its
   * length whether the section was truncated, so handing it exactly {@code --max-rows} rows would
   * lose the fact that there were more. An export needs all of them.
   */
  private static int materialise(int total, int cap, boolean exporting) {
    return exporting ? total : Math.min(total, cap + 1);
  }

  /**
   * A row list made unmodifiable without rejecting nulls.
   *
   * <p>{@code List.copyOf} refuses null elements, and an absent cell is exactly that — a blanked
   * {@code value_date} is a null in the middle of a row — so the rows the report embeds cannot go
   * through it.
   */
  private static <T> List<T> frozen(List<T> list) {
    return java.util.Collections.unmodifiableList(list);
  }

  /** The key columns of a parsed row, as the report embeds them. */
  private static List<String> key(RowIndex side, long[] fields, int keySize, Options opt) {
    var out = new ArrayList<String>(keySize);
    for (int i = 0; i < keySize; i++) {
      out.add(Bytes.value(side.slab(), fields[i], opt));
    }
    return frozen(out);
  }

  /** The sparse cell diffs of one changed row. */
  private static List<CellDiff> cells(
      RowIndex a, RowIndex b, long[] fa, long[] fb, int keySize, int nc, Options opt) {
    var out = new ArrayList<CellDiff>();
    for (int i = 0; i < nc; i++) {
      long x = fa[keySize + i];
      long y = fb[keySize + i];
      if (Bytes.differs(a.slab(), x, b.slab(), y, opt)) {
        out.add(new CellDiff(i, Bytes.value(a.slab(), x, opt), Bytes.value(b.slab(), y, opt)));
      }
    }
    return List.copyOf(out);
  }

  /** A whole row as Strings, for {@code --export-dir}. */
  private static List<String> values(RowIndex side, long[] fields, int width, Options opt) {
    var out = new ArrayList<String>(width);
    for (int i = 0; i < width; i++) {
      out.add(Bytes.value(side.slab(), fields[i], opt));
    }
    return out;
  }

  /**
   * The added or removed rows.
   *
   * <p>Capped at {@code --max-rows} for the report, but {@code --export-dir} writes the uncapped
   * list, so with an export the whole thing is materialised.
   */
  private static List<List<Object>> sideRows(
      List<Pick> picks, RowIndex side, RowParser parser, long[] fields,
      int width, int cap, boolean exporting, Options opt) {
    int limit = materialise(picks.size(), cap, exporting);
    var out = new ArrayList<List<Object>>(limit);
    for (int i = 0; i < limit; i++) {
      side.fields(parser, picks.get(i).row(), fields);
      out.add(frozen(new ArrayList<Object>(values(side, fields, width, opt))));
    }
    return frozen(out);
  }
}
