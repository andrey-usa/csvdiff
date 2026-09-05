package dev.csvdiff.engine;

import dev.csvdiff.Columns;
import dev.csvdiff.CompareEngine;
import dev.csvdiff.CsvDiffException;
import dev.csvdiff.Contract.EngineResult;
import dev.csvdiff.Options;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import tech.tablesaw.api.ColumnType;
import tech.tablesaw.api.Table;
import tech.tablesaw.columns.Column;
import tech.tablesaw.io.csv.CsvReadOptions;

/**
 * Tablesaw engine: a columnar dataframe, in pure Java.
 *
 * <p>Tablesaw owns the parse and the column storage; every column is forced to {@code STRING} so
 * nothing is type-inferred and {@code 1.0} stays different from {@code 1}. The join is then done by
 * {@link RowStore}, because Tablesaw's joiner materialises a cartesian-ish intermediate table for a
 * full outer join, which is exactly what this workload cannot afford at scale.
 *
 * <p>In-memory only, so it is bounded by the heap.
 */
public final class TablesawEngine implements CompareEngine {

  @Override
  public EngineResult compare(Path aPath, Path bPath, Options opt) throws IOException {
    Table a = read(aPath, opt);
    Table b = read(bPath, opt);

    List<String> aHeader = a.columnNames();
    List<String> bHeader = b.columnNames();
    var resolved = Columns.resolve(aHeader, bHeader, opt);

    var storeA = toStore(a, resolved.compared(), opt);
    var storeB = toStore(b, resolved.compared(), opt);

    var dupA = storeA.duplicateSection();
    var dupB = storeB.duplicateSection();
    var joined = RowStore.join(storeA, storeB, opt);

    return Sections.assemble(
        resolved.toMeta(opt.key(), aHeader.size(), bHeader.size()),
        joined, dupA, dupB, opt, resolved.compared());
  }

  private static Table read(Path path, Options opt) throws IOException {
    // The charset is applied by handing Tablesaw a Reader; its builder has no charset setter.
    // Every column is forced to STRING so nothing is type-inferred.
    try (var reader = Files.newBufferedReader(path, opt.charset())) {
      var builder =
          CsvReadOptions.builder(reader)
              .header(true)
              .columnTypes((String name) -> ColumnType.STRING)
              .missingValueIndicator("");
      if (opt.delimiter() != null) {
        builder.separator(opt.delimiter());
      }
      try {
        return Table.read().usingOptions(builder.build());
      } catch (RuntimeException e) {
        throw raggedOrRethrow(path, e);
      }
    }
  }

  /**
   * Explains a rejected file instead of passing Tablesaw's wording through.
   *
   * <p>Every other engine here treats a row with more or fewer fields than the header as a
   * difference to report rather than a file to refuse. Tablesaw's reader has no option for it, so
   * this engine is the one that cannot, and a user who hits it deserves to be told which engines
   * can rather than shown a message about cells and row numbers from inside a library.
   */
  private static RuntimeException raggedOrRethrow(Path path, RuntimeException e) {
    String text = String.valueOf(e.getMessage());
    boolean ragged = text.contains("contains") && text.contains("column")
        || text.contains("Error while adding cell");
    if (!ragged) {
      return e;
    }
    return new CsvDiffException(
        "Tablesaw cannot read " + path + ": a row has a different number of fields than the "
            + "header, and its reader has no option to allow that. Every other engine compares "
            + "such a file; try --engine turbo (or native, sortmerge, duckdb).", e);
  }

  /** Pulls the key and compared columns out of the dataframe, normalised, into a joinable store. */
  private static RowStore toStore(Table t, List<String> compared, Options opt) {
    var store = new RowStore(opt, compared);
    int keySize = opt.key().size();
    int width = keySize + compared.size();

    var columns = new ArrayList<Column<?>>(width);
    for (String name : opt.key()) {
      columns.add(t.column(name));
    }
    for (String name : compared) {
      columns.add(t.column(name));
    }

    int rows = t.rowCount();
    for (int r = 0; r < rows; r++) {
      var row = new ArrayList<String>(width);
      for (Column<?> col : columns) {
        Object v = col.get(r);
        // Tablesaw represents a missing string as "" once the missing-value indicator is applied.
        String s = v == null ? null : v.toString();
        row.add(Columns.normalise(Csv.emptyToNull(s), opt));
      }
      store.add(row);
    }
    return store;
  }
}
