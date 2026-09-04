package dev.csvdiff;

import java.util.Arrays;
import java.util.List;
import java.util.Locale;

/**
 * The available comparison backends.
 *
 * <p>{@link #AUTO} resolves to the first concrete engine that can run here, in declaration order.
 * Every concrete engine must return identical {@code counts} and {@code columns} for the same
 * input; the test suite and CI both assert that.
 */
public enum EngineName {
  /** Pick the first available concrete engine. */
  AUTO,
  /** DuckDB over JDBC: out-of-core, spills to disk, handles files larger than RAM. */
  DUCKDB,
  /** Mapped with the FFM API, scanned with SWAR, indexed on every core. Needs no extra modules. */
  TURBO,
  /** Mapped with the FFM API, scanned with SWAR, one thread. Needs no extra modules. */
  SWAR,
  /** Mapped with the FFM API, scanned with the Vector API, indexed on every core. */
  SHARD,
  /** Mapped with the FFM API, scanned with the Vector API, one thread. */
  MMAP,
  /** On the heap, scanned with the Vector API, one thread. */
  SIMD,
  /** Tablesaw: in-memory columnar dataframe, pure Java. */
  TABLESAW,
  /** External sort-merge join: bounded memory, spills to disk, size limited by disk not RAM. */
  SORTMERGE,
  /** This project: in-memory hash join over FastCSV, no dataframe layer. */
  NATIVE;

  /** Concrete engines in {@link #AUTO} preference order. */
  public static List<EngineName> concrete() {
    return Arrays.stream(values()).filter(e -> e != AUTO).toList();
  }

  /** The spelling used on the command line and in reports. */
  public String label() {
    return name().toLowerCase(Locale.ROOT);
  }

  /** Parses a command-line spelling, with a message that lists the valid ones. */
  public static EngineName parse(String value) {
    for (EngineName e : values()) {
      if (e.label().equals(value.toLowerCase(Locale.ROOT))) {
        return e;
      }
    }
    String valid = Arrays.stream(values()).map(EngineName::label).reduce((a, b) -> a + ", " + b).orElseThrow();
    throw new CsvDiffException("Unknown engine: " + value + ". Choose one of " + valid + ".");
  }
}
