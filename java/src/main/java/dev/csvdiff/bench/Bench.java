package dev.csvdiff.bench;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.SerializationFeature;
import com.fasterxml.jackson.databind.json.JsonMapper;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.TimeUnit;

/**
 * Benchmarks one scale end to end and records the numbers.
 *
 * <pre>{@code
 * java -cp target/csvdiff.jar dev.csvdiff.bench.Bench --rows 1m --engine duckdb --out-dir bench
 * }</pre>
 *
 * <p>The comparison runs in a child JVM so peak RSS is measured honestly rather than including this
 * process's own heap. Writes {@code bench/<scale>-<engine>.json}, appends a row to
 * {@code $GITHUB_STEP_SUMMARY} under Actions, and exits non-zero when a budget is exceeded. An
 * engine that cannot handle a scale is recorded as a failed row with the reason rather than
 * aborting the run, because where an engine stops is a result worth having.
 */
public final class Bench {

  private Bench() {}

  /** Budgets on a 4-vCPU / 16 GB GitHub-hosted runner, generous by about 2x. */
  private static final long[][] BUDGETS = {
    {10_000L, 20L, 1_500L},
    {1_000_000L, 120L, 6_000L},
    {10_000_000L, 900L, 12_000L},
  };

  private static final String KEY = "account_id,txn_id";
  private static final String IGNORE = "updated_at";

  private record Budget(long seconds, long peakMb) {}

  private static Budget budgetFor(long rows) {
    for (long[] b : BUDGETS) {
      if (rows <= b[0]) {
        return new Budget(b[1], b[2]);
      }
    }
    long[] last = BUDGETS[BUDGETS.length - 1];
    return new Budget(last[1], last[2]);
  }

  @SuppressWarnings("PMD")
  public static void main(String[] args) throws IOException, InterruptedException {
    var cfg = Config.parse(args);
    long rows = GenData.parseRows(cfg.rowsArg);

    Files.createDirectories(cfg.outDir);
    Files.createDirectories(cfg.dataDir);
    Path a = cfg.dataDir.resolve(cfg.label + "_a.csv");
    Path b = cfg.dataDir.resolve(cfg.label + "_b.csv");

    double genSeconds = 0;
    if (!Files.exists(a) || !Files.exists(b)) {
      long t0 = System.nanoTime();
      GenData.generate(rows, a, b, 7);
      genSeconds = round((System.nanoTime() - t0) / 1e9, 1);
      System.out.printf(Locale.US, "java: generated %,d rows x 20 columns in %.1fs%n", rows, genSeconds);
    }

    Path report = cfg.outDir.resolve(cfg.label + "-" + cfg.engine + ".html");
    Path summary = cfg.outDir.resolve(cfg.label + "-" + cfg.engine + "-summary.json");

    var command = new ArrayList<String>();
    command.add(Path.of(System.getProperty("java.home"), "bin", "java").toString());
    if (cfg.heapMb != null) {
      command.add("-Xmx" + cfg.heapMb + "m");
    }
    // The Vector API is an incubator module, so the simd, mmap and shard engines are only on the
    // menu when the launcher is told to load it. Harmless for the engines that do not use it.
    command.add("--add-modules");
    command.add("jdk.incubator.vector");
    command.add("-cp");
    command.add(System.getProperty("java.class.path"));
    command.add("dev.csvdiff.Cli");
    command.addAll(List.of("compare", a.toString(), b.toString(), "-k", KEY, "-i", IGNORE,
        "--engine", cfg.engine, "-o", report.toString(), "--json", summary.toString()));
    if (cfg.threads != null) {
      command.addAll(List.of("--threads", String.valueOf(cfg.threads)));
    }
    if (cfg.memoryLimit != null) {
      command.addAll(List.of("--memory-limit", cfg.memoryLimit));
    }

    // The child reports its own peak RSS on stderr, so the figure excludes this JVM's heap.
    var builder =
        new ProcessBuilder(command)
            .redirectOutput(ProcessBuilder.Redirect.INHERIT)
            .redirectErrorStream(false);
    builder.environment().put("CSVDIFF_PRINT_PEAK_RSS", "1");

    long t0 = System.nanoTime();
    Process proc = builder.start();
    var stderr = new StringBuilder();
    var drain =
        Thread.ofVirtual()
            .start(
                () -> {
                  try (var in = proc.getErrorStream()) {
                    String text = new String(in.readAllBytes(), StandardCharsets.UTF_8);
                    stderr.append(text);
                    System.err.print(text);
                  } catch (IOException e) {
                    // the child died; the exit status already tells the story
                  }
                });
    boolean finished = proc.waitFor(6, TimeUnit.HOURS);
    if (!finished) {
      proc.destroyForcibly();
    }
    drain.join();
    int status = finished ? proc.exitValue() : -1;
    double wall = round((System.nanoTime() - t0) / 1e9, 2);

    long peakKb = stderr.toString().lines()
        .filter(l -> l.startsWith("PEAK_RSS_KB"))
        .mapToLong(l -> Long.parseLong(l.replaceAll("[^0-9]", "")))
        .max()
        .orElse(0L);
    double peakMb = round(peakKb / 1024.0, 1);
    double inputMb = round((Files.size(a) + Files.size(b)) / 1e6, 1);

    var mapper = JsonMapper.builder().enable(SerializationFeature.INDENT_OUTPUT).build();
    String runner = "%s %s java%s %d cpu"
        .formatted(System.getProperty("os.name"), System.getProperty("os.arch"),
            Runtime.version().feature(), Runtime.getRuntime().availableProcessors());

    if (status != 0 && status != 1) {
      String why = classify(status, stderr.toString());
      recordFailure(mapper, cfg, rows, genSeconds, inputMb, peakMb, wall, status, why, runner);
      System.err.printf("compare exited with %d: %s%n", status, why);
      cleanUp(cfg, a, b);
      System.exit(cfg.allowFailure ? 0 : 2);
    }

    if (!Files.exists(summary)) {
      // The child reported success but wrote nothing. Record it as a failure rather than
      // crashing the harness on a missing file, and never present it as a result.
      String why = "no summary written";
      recordFailure(mapper, cfg, rows, genSeconds, inputMb, peakMb, wall, status, why, runner);
      System.err.printf("compare exited %d but wrote no summary; recorded as failed%n", status);
      cleanUp(cfg, a, b);
      System.exit(cfg.allowFailure ? 0 : 2);
    }
    JsonNode counts = mapper.readTree(Files.readString(summary)).get("counts");
    long aRows = counts.get("a_rows").asLong();
    long bRows = counts.get("b_rows").asLong();
    double reportMb = round(Files.size(report) / 1e6, 2);
    Long perSecond = wall > 0 ? Math.round((aRows + bRows) / wall) : null;

    var rec = new LinkedHashMap<String, Object>();
    rec.put("rows", rows);
    rec.put("scale", cfg.label);
    rec.put("engine", cfg.engine);
    rec.put("generate_seconds", genSeconds);
    rec.put("compare_seconds", wall);
    rec.put("peak_rss_mb", peakMb);
    rec.put("input_mb", inputMb);
    rec.put("report_mb", reportMb);
    rec.put("rows_per_second", perSecond);
    rec.put("counts", mapper.convertValue(counts, Map.class));
    rec.put("runner", runner);
    write(mapper, cfg.outDir.resolve(cfg.label + "-" + cfg.engine + ".json"), rec);

    Budget budget = budgetFor(rows);
    boolean ok = wall <= budget.seconds() && peakMb <= budget.peakMb();
    String line =
        "| %s | %s | %s MB | %ss | **%ss** | %s/s | %s MB | %s MB | %s | %s | %s | %s |"
            .formatted(cfg.label, cfg.engine, fmt(inputMb), genSeconds, wall,
                perSecond == null ? "-" : fmt(perSecond), fmt(peakMb), reportMb,
                fmt(counts.get("changed").asLong()), fmt(counts.get("added").asLong()),
                fmt(counts.get("removed").asLong()),
                ok ? "pass" : "over budget (%ds / %d MB)".formatted(budget.seconds(), budget.peakMb()));
    appendSummary(line);
    System.out.println(line);

    cleanUp(cfg, a, b);
    System.exit(ok || cfg.noBudget ? 0 : 1);
  }

  /** Records a scale an engine could not handle: a JSON row plus a line in the results table. */
  private static void recordFailure(
      ObjectMapper mapper, Config cfg, long rows, double genSeconds, double inputMb,
      double peakMb, double wall, int status, String why, String runner) throws IOException {
    var rec = new LinkedHashMap<String, Object>();
    rec.put("rows", rows);
    rec.put("scale", cfg.label);
    rec.put("engine", cfg.engine);
    rec.put("generate_seconds", genSeconds);
    rec.put("compare_seconds", null);
    rec.put("peak_rss_mb", peakMb > 0 ? peakMb : null);
    rec.put("input_mb", inputMb);
    rec.put("report_mb", null);
    rec.put("rows_per_second", null);
    rec.put("counts", null);
    rec.put("failed", why);
    rec.put("exit_status", status);
    rec.put("wall_before_failure", wall);
    rec.put("runner", runner);
    write(mapper, cfg.outDir.resolve(cfg.label + "-" + cfg.engine + ".json"), rec);

    String line = "| %s | %s | %s MB | %ss | **%s** after %ss | - | %s | - | - | - | - | failed |"
        .formatted(cfg.label, cfg.engine, fmt(inputMb), genSeconds, why, wall,
            peakMb > 0 ? fmt(peakMb) + " MB" : "-");
    appendSummary(line);
    System.out.println(line);
  }

  private static void cleanUp(Config cfg, Path a, Path b) throws IOException {
    if (!cfg.keepData) {
      Files.deleteIfExists(a);
      Files.deleteIfExists(b);
    }
  }

  /**
   * A dead child process, turned into a short honest reason for the results table.
   *
   * <p>An in-heap engine on a 16 GB runner hits the JVM's default max heap (a quarter of RAM,
   * about 4 GB) long before the machine runs out, so say so: that ceiling is raised with -Xmx,
   * whereas being killed by the OS is not.
   */
  private static String classify(int status, String stderr) {
    if (stderr.contains("OutOfMemoryError")) {
      return stderr.contains("GC overhead limit") ? "Java heap OOM (GC thrash)" : "Java heap OOM";
    }
    if (stderr.contains("No space left on device")) {
      return "disk full";
    }
    return switch (status) {
      case 137 -> "OOM killed";
      case 139 -> "segfault";
      case -1 -> "timed out";
      default -> "exit " + status;
    };
  }

  private static void write(ObjectMapper mapper, Path path, Map<String, Object> rec) throws IOException {
    Files.writeString(path, mapper.writeValueAsString(rec), StandardCharsets.UTF_8);
  }

  private static void appendSummary(String line) throws IOException {
    String step = System.getenv("GITHUB_STEP_SUMMARY");
    if (step == null) {
      return;
    }
    Path p = Path.of(step);
    if (!Files.exists(p) || Files.size(p) == 0) {
      Files.writeString(p,
          "| scale | engine | input | generate | compare | throughput | peak RSS | report "
              + "| changed | added | removed | budget |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
          StandardCharsets.UTF_8, StandardOpenOption.CREATE, StandardOpenOption.APPEND);
    }
    Files.writeString(p, line + "\n", StandardCharsets.UTF_8, StandardOpenOption.CREATE, StandardOpenOption.APPEND);
  }

  private static double round(double v, int decimals) {
    double f = Math.pow(10, decimals);
    return Math.round(v * f) / f;
  }

  private static String fmt(double v) {
    return v == Math.rint(v) ? String.format(Locale.US, "%,d", (long) v) : String.format(Locale.US, "%,.1f", v);
  }

  private static String fmt(long v) {
    return String.format(Locale.US, "%,d", v);
  }

  /** Command-line configuration for one benchmark run. */
  private record Config(
      String rowsArg, String label, String engine, Path outDir, Path dataDir,
      boolean keepData, Integer threads, String memoryLimit, boolean noBudget,
      Integer heapMb, boolean allowFailure) {

    static Config parse(String[] args) {
      String rows = "10k";
      String engine = "auto";
      String outDir = "bench";
      String dataDir = "data";
      boolean keepData = false;
      Integer threads = null;
      String memoryLimit = null;
      boolean noBudget = false;
      Integer heapMb = null;
      boolean allowFailure = false;

      for (int i = 0; i < args.length; i++) {
        switch (args[i]) {
          case "--rows", "-n" -> rows = args[++i];
          case "--engine" -> engine = args[++i];
          case "--out-dir", "-o" -> outDir = args[++i];
          case "--data-dir" -> dataDir = args[++i];
          case "--threads" -> threads = Integer.valueOf(args[++i]);
          case "--memory-limit" -> memoryLimit = args[++i];
          case "--heap-mb" -> heapMb = Integer.valueOf(args[++i]);
          case "--keep-data" -> keepData = true;
          case "--no-budget" -> noBudget = true;
          case "--allow-failure" -> allowFailure = true;
          default -> { /* ignored */ }
        }
      }
      return new Config(rows, rows.toLowerCase(Locale.ROOT), engine, Path.of(outDir), Path.of(dataDir),
          keepData, threads, memoryLimit, noBudget, heapMb, allowFailure);
    }
  }
}
