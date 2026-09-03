package dev.csvdiff;

import dev.csvdiff.Contract.EngineResult;
import java.nio.file.Path;

/**
 * One comparison backend.
 *
 * <p>Implementations differ only in how they get the work done; they must not differ in the answer.
 * Given the same inputs and options, every engine returns identical {@code counts}, {@code columns}
 * and embedded rows.
 */
@FunctionalInterface
public interface CompareEngine {

  /**
   * Compares two CSV files on the composite key in {@code opt}.
   *
   * @throws CsvDiffException if the files cannot be compared as asked
   */
  EngineResult compare(Path a, Path b, Options opt) throws Exception;
}
