package dev.csvdiff.engine.fast;

import dev.csvdiff.Columns;
import dev.csvdiff.CompareEngine;
import dev.csvdiff.Contract.EngineResult;
import dev.csvdiff.CsvDiffException;
import dev.csvdiff.Options;
import dev.csvdiff.engine.Sections;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

/**
 * The shared body of the three high-performance engines.
 *
 * <p>They are the same algorithm with one thing changed at a time, which is what makes the
 * benchmark readable: put them side by side and each row of the table isolates one technique.
 *
 * <ul>
 *   <li><b>simd</b> — the bytes on the heap, scanned with the Vector API, joined on one thread.
 *       Against {@code native} it measures what vectorised scanning and a String-free join are
 *       worth on their own.
 *   <li><b>mmap</b> — the same, but the file is mapped rather than read, so its bytes never enter
 *       the heap. Against {@code simd} it measures what the copy and the garbage collector cost.
 *   <li><b>shard</b> — the same as {@code mmap}, with the index built by every core at once.
 *       Against {@code mmap} it measures the parallel speed-up on the parse, which is where a
 *       comparison of this shape spends most of its time.
 * </ul>
 */
public abstract class FastEngine implements CompareEngine {

  /** How a file's bytes are obtained. */
  public enum Source {
    /** Read onto the heap: bounded by array length and by {@code -Xmx}. */
    HEAP,
    /** Mapped into the address space: bounded by neither. */
    MAPPED
  }

  private final Source source;
  private final boolean parallel;

  protected FastEngine(Source source, boolean parallel) {
    this.source = source;
    this.parallel = parallel;
  }

  /** Whether the incubating Vector API is present, without which none of these can run. */
  public static boolean available() {
    try {
      return Scanner.available();
    } catch (LinkageError | RuntimeException e) {
      return false;
    }
  }

  @Override
  public EngineResult compare(Path aPath, Path bPath, Options opt) throws IOException {
    if (!available()) {
      throw new CsvDiffException(
          "The Vector API is not on the module path. Add --add-modules jdk.incubator.vector, "
              + "or choose --engine native.");
    }
    try (Slab aSlab = open(aPath, opt);
        Slab bSlab = open(bPath, opt)) {
      byte delimiter = Files2.delimiter(aSlab, opt);
      var aHeader = RowParser.header(aSlab, delimiter, opt);
      var bHeader = RowParser.header(bSlab, Files2.delimiter(bSlab, opt), opt);
      var resolved = Columns.resolve(aHeader.names(), bHeader.names(), opt);

      List<String> compared = resolved.compared();
      int[] aColumns = projection(aHeader.names(), compared, opt);
      int[] bColumns = projection(bHeader.names(), compared, opt);
      int width = opt.key().size() + compared.size();

      RowIndex a = index(aSlab, aPath, opt, delimiter, aColumns, width, aHeader.dataStart());
      RowIndex b =
          index(bSlab, bPath, opt, Files2.delimiter(bSlab, opt), bColumns, width, bHeader.dataStart());

      var dupA = FastJoin.duplicates(a, opt);
      var dupB = FastJoin.duplicates(b, opt);
      var joined = FastJoin.join(a, b, opt, compared);

      return Sections.assemble(
          resolved.toMeta(opt.key(), aHeader.names().size(), bHeader.names().size()),
          joined, dupA, dupB, opt, compared);
    }
  }

  private Slab open(Path path, Options opt) throws IOException {
    if (!Files.isRegularFile(path)) {
      throw new CsvDiffException("File not found: " + path);
    }
    return source == Source.HEAP ? Files2.load(path, opt) : Files2.map(path, opt);
  }

  private RowIndex index(
      Slab slab, Path path, Options opt, byte delimiter, int[] columns, int width, long dataStart)
      throws IOException {
    // One row per 96 bytes is a deliberate under-estimate for a twenty-column extract: the arrays
    // double when it is wrong, and over-allocating at ten million rows costs more than a resize.
    long expected = Math.max(16, slab.size() / 96);
    var index = new RowIndex(slab, opt, delimiter, columns, width, expected);
    if (parallel) {
      ShardedBuilder.build(index, slab, opt, delimiter, columns, width, dataStart);
    } else {
      index.build(dataStart);
    }
    return index;
  }

  /** Where each projected column sits in this file, or -1 when the file does not have it. */
  private static int[] projection(List<String> header, List<String> compared, Options opt) {
    int keySize = opt.key().size();
    var out = new int[keySize + compared.size()];
    for (int i = 0; i < keySize; i++) {
      out[i] = header.indexOf(opt.key().get(i));
    }
    for (int i = 0; i < compared.size(); i++) {
      out[keySize + i] = header.indexOf(compared.get(i));
    }
    return out;
  }
}
