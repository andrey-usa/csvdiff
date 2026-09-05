package dev.csvdiff.engine.sortmerge;

import de.siegmar.fastcsv.reader.CsvReader;
import de.siegmar.fastcsv.reader.CsvRecord;
import de.siegmar.fastcsv.reader.FieldMismatchStrategy;
import dev.csvdiff.Columns;
import dev.csvdiff.CompareEngine;
import dev.csvdiff.Contract.EngineResult;
import dev.csvdiff.CsvDiffException;
import dev.csvdiff.Options;
import dev.csvdiff.engine.Csv;
import dev.csvdiff.engine.Sections;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.stream.Stream;

/**
 * The out-of-core engine: sort both files by key, then walk them together.
 *
 * <p>Every other engine here holds at least one whole file in memory, whether as a hash index, a
 * dataframe or a mapped slab, so the largest comparison is bounded by the machine. This one is
 * bounded by disk instead. Batches of rows are sorted and spilled, the spilled runs are merged back
 * as one ordered stream, and the join is a single ordered pass down both sides at once — so the
 * heap holds a batch, one row per run, and the capped report, and nothing else grows with the file.
 *
 * <p>That is the classical answer to comparing data larger than memory, and it is what reconciliation
 * systems have done since the files lived on tape. It is not the fastest engine here and is not
 * meant to be: sorting is {@code O(n log n)} where a hash join is linear, and the spill writes the
 * data twice more. What it buys is that the answer does not depend on how much RAM you have.
 *
 * <p>Duplicate keys are nearly free, because sorting puts the repeats next to each other. Sections
 * come out in key order for the same reason, so nothing is sorted a second time at the end.
 */
public final class SortMergeEngine implements CompareEngine {

  @Override
  public EngineResult compare(Path aPath, Path bPath, Options opt) throws IOException {
    List<String> aHeader = header(aPath, opt);
    List<String> bHeader = header(bPath, opt);
    var resolved = Columns.resolve(aHeader, bHeader, opt);
    List<String> compared = resolved.compared();

    int keySize = opt.key().size();
    int width = keySize + compared.size();
    boolean exporting = opt.exportDir() != null;

    Path work = Files.createTempDirectory("csvdiff-sortmerge-");
    try (var a = new Runs(subdir(work, "a"), width, keySize);
        var b = new Runs(subdir(work, "b"), width, keySize)) {

      read(aPath, aHeader, compared, opt, a);
      read(bPath, bHeader, compared, opt, b);

      var dupA = new MergeJoin.Dups(opt.key(), opt.maxRows());
      var dupB = new MergeJoin.Dups(opt.key(), opt.maxRows());

      try (var ca = a.sorted();
          var cb = b.sorted()) {
        var joined =
            MergeJoin.join(ca, cb, opt, compared, a.rows(), b.rows(), dupA, dupB, exporting);
        return Sections.assemble(
            resolved.toMeta(opt.key(), aHeader.size(), bHeader.size()),
            joined, dupA.section(), dupB.section(), opt, compared);
      }
    } finally {
      deleteTree(work);
    }
  }

  private static Path subdir(Path work, String name) throws IOException {
    return Files.createDirectories(work.resolve(name));
  }

  /** Removes the working directory, spilled runs and all, however the comparison ended. */
  private static void deleteTree(Path root) {
    try (Stream<Path> walk = Files.walk(root)) {
      for (Path p : walk.sorted(Comparator.reverseOrder()).toList()) {
        Files.deleteIfExists(p);
      }
    } catch (IOException e) {
      // The temp directory is the operating system's to reclaim if this does not manage it.
    }
  }

  private static char delimiter(Path path, Options opt) throws IOException {
    if (opt.delimiter() != null) {
      return opt.delimiter();
    }
    try (var lines = Files.lines(path, opt.charset())) {
      return Csv.detectDelimiter(lines.findFirst().orElse(""));
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

  /** Streams a file into the sorter, projecting and normalising a row at a time. */
  private static void read(
      Path path, List<String> header, List<String> compared, Options opt, Runs runs)
      throws IOException {
    int keySize = opt.key().size();
    int width = keySize + compared.size();

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
        runs.add(row);
      }
    } catch (UncheckedIOException e) {
      throw new CsvDiffException("Cannot read " + path + ": " + e.getMessage(), e);
    }
  }
}
