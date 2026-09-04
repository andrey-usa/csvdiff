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
import java.util.function.Supplier;

/**
 * The shared body of the byte-level engines.
 *
 * <p>They are one implementation with three independent variables, which is what makes the
 * benchmark readable: each pair of rows in the results table isolates a single technique.
 *
 * <table>
 *   <caption>The five configurations</caption>
 *   <tr><th>Engine</th><th>Bytes</th><th>Scan</th><th>Threads</th></tr>
 *   <tr><td>{@code simd}</td><td>heap array</td><td>Vector API</td><td>one</td></tr>
 *   <tr><td>{@code mmap}</td><td>FFM mapping</td><td>Vector API</td><td>one</td></tr>
 *   <tr><td>{@code shard}</td><td>FFM mapping</td><td>Vector API</td><td>all cores</td></tr>
 *   <tr><td>{@code swar}</td><td>FFM mapping</td><td>SWAR</td><td>one</td></tr>
 *   <tr><td>{@code turbo}</td><td>FFM mapping</td><td>SWAR</td><td>all cores</td></tr>
 * </table>
 *
 * <p>So {@code simd} against {@code mmap} prices the heap copy, {@code mmap} against {@code shard}
 * prices parallelism, and {@code mmap} against {@code swar} prices real SIMD against the bit trick
 * that stands in for it.
 *
 * <p>What they all share is the thing that matters most: no {@link String} is built for a cell
 * unless that cell reaches the report.
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
  private final Supplier<Scan> scanner;
  private final boolean parallel;

  protected FastEngine(Source source, Supplier<Scan> scanner, boolean parallel) {
    this.source = source;
    this.scanner = scanner;
    this.parallel = parallel;
  }

  /** Whether this engine's scanning technique can run here. */
  public boolean canRun() {
    try {
      return scanner.get().available();
    } catch (LinkageError | RuntimeException e) {
      return false;
    }
  }

  /** Whether the incubating Vector API is present, which the three vector engines need. */
  public static boolean vectorAvailable() {
    try {
      return new VectorScan().available();
    } catch (LinkageError | RuntimeException e) {
      return false;
    }
  }

  @Override
  public EngineResult compare(Path aPath, Path bPath, Options opt) throws IOException {
    Scan scan;
    try {
      scan = scanner.get();
    } catch (LinkageError e) {
      throw new CsvDiffException(vectorMissing(), e);
    }
    if (!scan.available()) {
      throw new CsvDiffException(vectorMissing());
    }

    try (Slab aSlab = open(aPath, opt);
        Slab bSlab = open(bPath, opt)) {
      byte aDelim = Files2.delimiter(aSlab, scan, opt);
      byte bDelim = Files2.delimiter(bSlab, scan, opt);
      var aHeader = RowParser.header(aSlab, scan, aDelim, opt);
      var bHeader = RowParser.header(bSlab, scan, bDelim, opt);
      var resolved = Columns.resolve(aHeader.names(), bHeader.names(), opt);

      List<String> compared = resolved.compared();
      int width = opt.key().size() + compared.size();

      RowIndex a =
          index(aSlab, scan, opt, aDelim, projection(aHeader.names(), compared, opt), width,
              aHeader.dataStart());
      RowIndex b =
          index(bSlab, scan, opt, bDelim, projection(bHeader.names(), compared, opt), width,
              bHeader.dataStart());

      var dupA = FastJoin.duplicates(a, opt);
      var dupB = FastJoin.duplicates(b, opt);
      var joined = FastJoin.join(a, b, opt, compared);

      return Sections.assemble(
          resolved.toMeta(opt.key(), aHeader.names().size(), bHeader.names().size()),
          joined, dupA, dupB, opt, compared);
    }
  }

  private static String vectorMissing() {
    return "The Vector API is not on the module path. Add --add-modules jdk.incubator.vector, "
        + "or choose an engine that does not need it: swar, turbo, or native.";
  }

  private Slab open(Path path, Options opt) throws IOException {
    if (!Files.isRegularFile(path)) {
      throw new CsvDiffException("File not found: " + path);
    }
    return source == Source.HEAP ? Files2.load(path, opt) : Files2.map(path, opt);
  }

  private RowIndex index(
      Slab slab, Scan scan, Options opt, byte delimiter, int[] columns, int width, long dataStart) {
    // One row per 96 bytes is a deliberate under-estimate for a twenty-column extract: the arrays
    // double when it is wrong, and over-allocating at ten million rows costs more than a resize.
    long expected = Math.max(16, slab.size() / 96);
    var index = new RowIndex(slab, scan, opt, delimiter, columns, width, expected);
    if (parallel) {
      ShardedBuilder.build(index, slab, scan, opt, delimiter, columns, width, dataStart);
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
