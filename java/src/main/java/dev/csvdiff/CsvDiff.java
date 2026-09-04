package dev.csvdiff;

import dev.csvdiff.Contract.CompareResult;
import dev.csvdiff.Contract.EngineMeta;
import dev.csvdiff.Contract.EngineResult;
import dev.csvdiff.Contract.FileMeta;
import dev.csvdiff.engine.DuckDbEngine;
import dev.csvdiff.engine.NativeEngine;
import dev.csvdiff.engine.TablesawEngine;
import dev.csvdiff.engine.sortmerge.SortMergeEngine;
import dev.csvdiff.engine.fast.FastEngine;
import dev.csvdiff.engine.fast.MmapEngine;
import dev.csvdiff.engine.fast.ShardEngine;
import dev.csvdiff.engine.fast.SwarEngine;
import dev.csvdiff.engine.fast.SimdEngine;
import dev.csvdiff.engine.fast.TurboEngine;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.OffsetDateTime;
import java.time.ZoneId;
import java.time.temporal.ChronoUnit;
import java.util.EnumMap;
import java.util.Map;
import java.util.function.Supplier;

/**
 * The entry point: pick an engine, run the comparison, attach the run's own facts.
 *
 * <pre>{@code
 * var opt = Options.builder().key(List.of("order_id", "line_no")).ignore(List.of("updated_at")).build();
 * var result = CsvDiff.compare(Path.of("july.csv"), Path.of("august.csv"), opt);
 * Files.writeString(Path.of("report.html"), ReportRenderer.render(result));
 * }</pre>
 */
public final class CsvDiff {

  private CsvDiff() {}

  /** Engines are constructed lazily, so a backend that cannot load only matters if you ask for it. */
  private static final Map<EngineName, Supplier<CompareEngine>> ENGINES = buildRegistry();

  private static Map<EngineName, Supplier<CompareEngine>> buildRegistry() {
    var m = new EnumMap<EngineName, Supplier<CompareEngine>>(EngineName.class);
    m.put(EngineName.DUCKDB, DuckDbEngine::new);
    m.put(EngineName.TURBO, TurboEngine::new);
    m.put(EngineName.SWAR, SwarEngine::new);
    m.put(EngineName.SHARD, ShardEngine::new);
    m.put(EngineName.MMAP, MmapEngine::new);
    m.put(EngineName.SIMD, SimdEngine::new);
    m.put(EngineName.TABLESAW, TablesawEngine::new);
    m.put(EngineName.SORTMERGE, SortMergeEngine::new);
    m.put(EngineName.NATIVE, NativeEngine::new);
    return Map.copyOf(m);
  }

  /**
   * Compares two CSV files and returns the full result.
   *
   * @throws CsvDiffException if a file is missing, the key is absent from either file, or the
   *     chosen engine cannot run
   */
  public static CompareResult compare(Path a, Path b, Options opt) {
    if (opt.key().isEmpty()) {
      throw new CsvDiffException("At least one key column is required.");
    }
    for (Path p : new Path[] {a, b}) {
      if (!Files.isRegularFile(p)) {
        throw new CsvDiffException("File not found: " + p);
      }
    }

    EngineName engine = resolveEngine(opt.engineName());
    long t0 = System.nanoTime();
    EngineResult result;
    try {
      result = ENGINES.get(engine).get().compare(a, b, opt);
    } catch (CsvDiffException e) {
      throw e;
    } catch (Exception e) {
      throw new CsvDiffException("The " + engine.label() + " engine failed: " + e.getMessage(), e);
    }
    double seconds = Math.round((System.nanoTime() - t0) / 1_000_000.0) / 1000.0;

    EngineMeta em = result.meta();
    var meta =
        new Contract.Meta(
            em.key(), em.compared(), em.onlyInA(), em.onlyInB(), em.aCols(), em.bCols(),
            fileMeta(a), fileMeta(b), engine.label(), seconds, generatedAt(), opt);
    return new CompareResult(
        meta, result.counts(), result.columns(),
        result.changed(), result.added(), result.removed(), result.dupA(), result.dupB());
  }

  /**
   * Resolves {@link EngineName#AUTO} to the first backend whose dependency is actually loadable,
   * so a stripped-down deployment still works.
   */
  public static EngineName resolveEngine(EngineName requested) {
    if (requested != EngineName.AUTO) {
      return requested;
    }
    for (EngineName candidate : EngineName.concrete()) {
      if (available(candidate)) {
        return candidate;
      }
    }
    return EngineName.NATIVE;
  }

  private static boolean available(EngineName engine) {
    String probe =
        switch (engine) {
          case DUCKDB -> "org.duckdb.DuckDBDriver";
          case TABLESAW -> "tech.tablesaw.api.Table";
          case NATIVE, SORTMERGE -> "de.siegmar.fastcsv.reader.CsvReader";
          // The vector engines need the incubating Vector API, which is only on the module path
          // when the launcher was told to put it there. The SWAR engines need nothing at all.
          case SHARD, MMAP, SIMD -> null;
          case TURBO, SWAR -> "";
          case AUTO -> throw new IllegalArgumentException("AUTO is not a concrete engine");
        };
    if (probe == null) {
      return FastEngine.vectorAvailable();
    }
    if (probe.isEmpty()) {
      return true;
    }
    try {
      Class.forName(probe, false, CsvDiff.class.getClassLoader());
      return true;
    } catch (ClassNotFoundException | LinkageError e) {
      return false;
    }
  }

  private static FileMeta fileMeta(Path p) {
    try {
      return new FileMeta(
          p.getFileName().toString(), p.toAbsolutePath().normalize().toString(), Files.size(p));
    } catch (IOException e) {
      throw new UncheckedIOException("Cannot stat " + p, e);
    }
  }

  /** Local time with a numeric offset, seconds precision, matching the other implementations. */
  private static String generatedAt() {
    return OffsetDateTime.now(ZoneId.systemDefault()).truncatedTo(ChronoUnit.SECONDS).toString();
  }
}
