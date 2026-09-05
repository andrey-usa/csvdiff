package dev.csvdiff.engine;

import de.siegmar.fastcsv.reader.CsvReader;
import de.siegmar.fastcsv.reader.CsvRecord;
import de.siegmar.fastcsv.reader.FieldMismatchStrategy;
import dev.csvdiff.Columns;
import dev.csvdiff.CompareEngine;
import dev.csvdiff.Contract.EngineResult;
import dev.csvdiff.CsvDiffException;
import dev.csvdiff.Options;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * The dependency-light engine: FastCSV parses, {@link RowStore} joins.
 *
 * <p>Both files are held in the heap, so this is for data that fits comfortably in memory. It is
 * the fallback when no other backend can load, and the baseline the others are measured against.
 */
public final class NativeEngine implements CompareEngine {

  @Override
  public EngineResult compare(Path aPath, Path bPath, Options opt) throws IOException {
    List<String> aHeader = header(aPath, opt);
    List<String> bHeader = header(bPath, opt);
    var resolved = Columns.resolve(aHeader, bHeader, opt);

    var a = read(aPath, aHeader, resolved.compared(), opt);
    var b = read(bPath, bHeader, resolved.compared(), opt);

    var dupA = a.duplicateSection();
    var dupB = b.duplicateSection();
    var joined = RowStore.join(a, b, opt);

    return Sections.assemble(
        resolved.toMeta(opt.key(), aHeader.size(), bHeader.size()), joined, dupA, dupB, opt, resolved.compared());
  }

  private static char delimiter(Path path, Options opt) throws IOException {
    if (opt.delimiter() != null) {
      return opt.delimiter();
    }
    try (var lines = Files.lines(path, opt.charset())) {
      String first = lines.findFirst().orElse("");
      return Csv.detectDelimiter(first);
    }
  }

  private static CsvReader<CsvRecord> reader(Path path, Options opt) throws IOException {
    return CsvReader.builder()
        .fieldSeparator(delimiter(path, opt))
        // A row with more or fewer fields than the header is a difference to report, not a file to
        // refuse — see FastEngineTest.raggedRowsArePadded. FastCSV is STRICT by default, which made
        // this engine reject input the byte-level engines happily compared. IGNORE keeps the extra
        // fields out of the way and leaves the missing ones absent, which is what they are.
        .extraFieldStrategy(FieldMismatchStrategy.IGNORE)
        .missingFieldStrategy(FieldMismatchStrategy.IGNORE)
        .ofCsvRecord(Files.newBufferedReader(path, opt.charset()));
  }

  private static List<String> header(Path path, Options opt) throws IOException {
    try (var csv = reader(path, opt)) {
      for (CsvRecord rec : csv) {
        return List.copyOf(rec.getFields());
      }
    }
    throw new CsvDiffException("File has no header row: " + path);
  }

  /** Reads a file straight into a {@link RowStore}, projecting and normalising as it goes. */
  private static RowStore read(Path path, List<String> header, List<String> compared, Options opt)
      throws IOException {
    var store = new RowStore(opt, compared);
    int keySize = opt.key().size();
    int width = keySize + compared.size();

    // Column positions of the key and compared columns, in the order the store expects them.
    int[] index = new int[width];
    for (int i = 0; i < keySize; i++) {
      index[i] = header.indexOf(opt.key().get(i));
    }
    for (int i = 0; i < compared.size(); i++) {
      index[keySize + i] = header.indexOf(compared.get(i));
    }

    try (var csv = reader(path, opt)) {
      boolean first = true;
      for (CsvRecord rec : csv) {
        if (first) {
          first = false;
          continue;
        }
        var row = new ArrayList<String>(width);
        for (int i = 0; i < width; i++) {
          int at = index[i];
          String raw = at >= 0 && at < rec.getFieldCount() ? rec.getField(at) : null;
          row.add(Columns.normalise(Csv.emptyToNull(raw), opt));
        }
        store.add(row);
      }
    } catch (UncheckedIOException e) {
      throw new CsvDiffException("Cannot read " + path + ": " + e.getMessage(), e);
    }
    return store;
  }
}
