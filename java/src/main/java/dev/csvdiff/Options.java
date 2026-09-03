package dev.csvdiff;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.nio.charset.Charset;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Objects;

/**
 * Everything that varies per comparison. Nothing about a specific dataset belongs in the code, so
 * key columns, compared columns and normalisation are all runtime parameters.
 *
 * <p>Instances are immutable; build one with {@link #builder()}. The JSON names are snake_case to
 * match the Python and TypeScript implementations, which read and write the same contract.
 *
 * @param key composite key columns; at least one is required
 * @param compare columns to diff, or {@code null} for every common non-key column
 * @param ignore columns to skip entirely
 * @param trim strip surrounding whitespace before comparing
 * @param ignoreCase lower-case before comparing
 * @param emptyIsNull treat an empty string and an absent value as equal
 * @param tolerance absolute numeric tolerance where both sides parse as numbers; 0 disables it
 * @param maxRows rows embedded per report section; counts are always exact
 * @param delimiter forced delimiter, or {@code null} to auto-detect
 * @param encoding character set of both files
 * @param engine which backend to run
 * @param threads DuckDB thread limit, or {@code null} for its default
 * @param memoryLimit DuckDB memory limit such as {@code "8GB"}, or {@code null}
 * @param exportDir directory for full, uncapped changed/added/removed CSVs, or {@code null}
 */
public record Options(
    @JsonIgnore List<String> key,
    @JsonIgnore List<String> compare,
    @JsonIgnore List<String> ignore,
    @JsonProperty("trim") boolean trim,
    @JsonProperty("ignore_case") boolean ignoreCase,
    @JsonProperty("empty_is_null") boolean emptyIsNull,
    @JsonProperty("tolerance") double tolerance,
    @JsonProperty("max_rows") int maxRows,
    @JsonProperty("delimiter") Character delimiter,
    @JsonProperty("encoding") String encoding,
    @JsonProperty("engine") String engine,
    @JsonProperty("threads") Integer threads,
    @JsonProperty("memory_limit") String memoryLimit,
    @JsonProperty("export_dir") String exportDir) {

  public static final int DEFAULT_MAX_ROWS = 50_000;

  /** Defensive copies and validation, so a constructed instance is always usable. */
  public Options {
    key = List.copyOf(Objects.requireNonNull(key, "key"));
    compare = compare == null ? null : List.copyOf(compare);
    ignore = List.copyOf(Objects.requireNonNullElse(ignore, List.of()));
    encoding = Objects.requireNonNullElse(encoding, "utf-8");
    engine = Objects.requireNonNullElse(engine, EngineName.AUTO.label());
    if (tolerance < 0) {
      throw new CsvDiffException("--tolerance must not be negative, got " + tolerance);
    }
    if (maxRows <= 0) {
      throw new CsvDiffException("--max-rows must be positive, got " + maxRows);
    }
    if (threads != null && threads <= 0) {
      throw new CsvDiffException("--threads must be positive, got " + threads);
    }
  }

  @JsonIgnore
  public EngineName engineName() {
    return EngineName.parse(engine);
  }

  @JsonIgnore
  public Charset charset() {
    return "utf-8".equalsIgnoreCase(encoding) || "utf8".equalsIgnoreCase(encoding)
        ? StandardCharsets.UTF_8
        : Charset.forName(encoding);
  }

  public static Builder builder() {
    return new Builder();
  }

  /** Turns this instance back into a builder, for the "profile then overrides" merge. */
  public Builder toBuilder() {
    return new Builder()
        .key(key)
        .compare(compare)
        .ignore(ignore)
        .trim(trim)
        .ignoreCase(ignoreCase)
        .emptyIsNull(emptyIsNull)
        .tolerance(tolerance)
        .maxRows(maxRows)
        .delimiter(delimiter)
        .encoding(encoding)
        .engine(engine)
        .threads(threads)
        .memoryLimit(memoryLimit)
        .exportDir(exportDir);
  }

  /**
   * Mutable builder for {@link Options}. Every setter ignores a {@code null} argument, so a profile
   * can be layered under command-line overrides without either side needing to know which values
   * the other supplied.
   */
  public static final class Builder {
    private List<String> key = List.of();
    private List<String> compare;
    private List<String> ignore = List.of();
    private boolean trim;
    private boolean ignoreCase;
    private boolean emptyIsNull;
    private double tolerance;
    private int maxRows = DEFAULT_MAX_ROWS;
    private Character delimiter;
    private String encoding = "utf-8";
    private String engine = EngineName.AUTO.label();
    private Integer threads;
    private String memoryLimit;
    private String exportDir;

    private Builder() {}

    public Builder key(List<String> v) {
      if (v != null) {
        this.key = v;
      }
      return this;
    }

    public Builder compare(List<String> v) {
      if (v != null) {
        this.compare = v;
      }
      return this;
    }

    public Builder ignore(List<String> v) {
      if (v != null) {
        this.ignore = v;
      }
      return this;
    }

    public Builder trim(Boolean v) {
      if (v != null) {
        this.trim = v;
      }
      return this;
    }

    public Builder ignoreCase(Boolean v) {
      if (v != null) {
        this.ignoreCase = v;
      }
      return this;
    }

    public Builder emptyIsNull(Boolean v) {
      if (v != null) {
        this.emptyIsNull = v;
      }
      return this;
    }

    public Builder tolerance(Double v) {
      if (v != null) {
        this.tolerance = v;
      }
      return this;
    }

    public Builder maxRows(Integer v) {
      if (v != null) {
        this.maxRows = v;
      }
      return this;
    }

    public Builder delimiter(Character v) {
      if (v != null) {
        this.delimiter = v;
      }
      return this;
    }

    public Builder encoding(String v) {
      if (v != null) {
        this.encoding = v;
      }
      return this;
    }

    public Builder engine(String v) {
      if (v != null) {
        this.engine = v;
      }
      return this;
    }

    public Builder threads(Integer v) {
      if (v != null) {
        this.threads = v;
      }
      return this;
    }

    public Builder memoryLimit(String v) {
      if (v != null) {
        this.memoryLimit = v;
      }
      return this;
    }

    public Builder exportDir(String v) {
      if (v != null) {
        this.exportDir = v;
      }
      return this;
    }

    public Options build() {
      return new Options(
          key, compare, ignore, trim, ignoreCase, emptyIsNull, tolerance, maxRows,
          delimiter, encoding, engine, threads, memoryLimit, exportDir);
    }
  }
}
