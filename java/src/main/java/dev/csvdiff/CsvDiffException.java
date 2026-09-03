package dev.csvdiff;

import java.io.Serial;

/**
 * A comparison that cannot be run as asked: a missing file, a key column that is not in both
 * files, an unknown engine or profile. Distinct from an unexpected failure, because the CLI maps
 * it to exit code 2 with a one-line message rather than a stack trace.
 */
public final class CsvDiffException extends RuntimeException {

  @Serial private static final long serialVersionUID = 1L;

  public CsvDiffException(String message) {
    super(message);
  }

  public CsvDiffException(String message, Throwable cause) {
    super(message, cause);
  }
}
