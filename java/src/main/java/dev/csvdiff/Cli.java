package dev.csvdiff;

import com.fasterxml.jackson.databind.SerializationFeature;
import com.fasterxml.jackson.databind.json.JsonMapper;
import dev.csvdiff.Contract.CompareResult;
import java.io.IOException;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * The command line.
 *
 * <pre>{@code
 * csvdiff compare A.csv B.csv --key id,region [--compare c1,c2] [--out report.html]
 * csvdiff compare A.csv B.csv --profile orders
 * }</pre>
 *
 * <p>Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (only with
 * {@code --fail-on-dups}). That makes it a drop-in CI or pipeline gate.
 */
public final class Cli {

  private Cli() {}

  static final String USAGE =
      """
      usage: csvdiff <command> [options]

      commands:
        compare A B     Compare two CSV files on a composite key and write an HTML report

      compare options:
        -k, --key COLS          Comma-separated key column(s) (or from --profile)
        -c, --compare COLS      Columns to compare (default: all common non-key columns)
        -i, --ignore COLS       Columns to skip
        -p, --profile NAME      Profile name from csvdiff.toml
            --config PATH       Path to csvdiff.toml
            --trim              Strip whitespace before comparing
            --ignore-case
            --empty-is-null     Treat empty string and null as equal
            --tolerance N       Absolute numeric tolerance
            --max-rows N        Rows embedded per section (default 50000)
            --delimiter D       Force delimiter (default: auto)
            --encoding ENC
            --engine E          auto | duckdb | turbo | swar | shard | mmap | simd | tablesaw | sortmerge | native
            --threads N
            --memory-limit S    DuckDB memory limit, e.g. 4GB
            --export-dir DIR    Write full changed/added/removed CSVs here
        -o, --out PATH          Report path (default: <a>__vs__<b>.html)
            --json PATH         Also write a JSON summary (counts + column stats) here
            --no-compress       Embed plain JSON instead of gzip (older browsers)
            --fail-on-dups      Exit 3 when either file has duplicate keys

      Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (with --fail-on-dups).
      """;

  public static void main(String[] args) {
    int status = run(args, System.out, System.err);
    if (System.getenv("CSVDIFF_PRINT_PEAK_RSS") != null) {
      reportPeakRss(System.err);
    }
    System.exit(status);
  }

  /**
   * Prints this process's peak resident set size, for the benchmark harness.
   *
   * <p>The JVM has no portable API for it, so this reads {@code VmHWM} from {@code /proc}; on a
   * platform without it, nothing is printed and the harness records no figure rather than a wrong
   * one. The Python and TypeScript implementations print the same line from {@code getrusage} and
   * {@code process.resourceUsage()}.
   */
  private static void reportPeakRss(PrintStream err) {
    try {
      for (String line : Files.readAllLines(Path.of("/proc/self/status"))) {
        if (line.startsWith("VmHWM:")) {
          String kb = line.replaceAll("[^0-9]", "");
          if (!kb.isEmpty()) {
            err.println("PEAK_RSS_KB " + kb);
          }
          return;
        }
      }
    } catch (IOException | RuntimeException e) {
      // Peak RSS is a nicety; never fail a comparison over it.
    }
  }

  /** Runs one invocation. Split out from {@link #main} so tests can drive it without exiting. */
  public static int run(String[] args, PrintStream out, PrintStream err) {
    try {
      if (args.length == 0) {
        err.print(USAGE);
        return 2;
      }
      return switch (args[0]) {
        case "-h", "--help", "help" -> {
          out.print(USAGE);
          yield 0;
        }
        case "compare" -> compare(args, out);
        default -> throw new CsvDiffException("Unknown command: " + args[0] + "\n\n" + USAGE);
      };
    } catch (CsvDiffException e) {
      err.println("error: " + e.getMessage());
      return 2;
    } catch (Throwable t) {
      // Throwable, not Exception: OutOfMemoryError and StackOverflowError are Errors, and letting
      // one escape would end the JVM with status 1 — which in this contract means "differences
      // found". A failed comparison must never be mistaken for a successful one.
      err.println("error: " + t);
      return 2;
    }
  }

  private static int compare(String[] argv, PrintStream out) throws IOException {
    var args = new Args(argv);
    if (args.flag("help", "h")) {
      out.print(USAGE);
      return 0;
    }
    List<String> positional = args.positional();
    if (positional.size() < 2) {
      throw new CsvDiffException("compare needs two files: csvdiff compare A.csv B.csv --key ...");
    }
    Path a = Path.of(positional.get(0));
    Path b = Path.of(positional.get(1));

    var config = Profiles.load(args.value("config", null));
    var profile = Profiles.profile(config, args.value("profile", "p"));

    var builder = Options.builder();
    profile.ifPresent(p -> Profiles.apply(builder, p));

    builder
        .key(Profiles.parseList(args.value("key", "k")))
        .compare(Profiles.parseList(args.value("compare", "c")))
        .ignore(Profiles.parseList(args.value("ignore", "i")))
        .trim(args.flag("trim", null) ? Boolean.TRUE : null)
        .ignoreCase(args.flag("ignore-case", null) ? Boolean.TRUE : null)
        .emptyIsNull(args.flag("empty-is-null", null) ? Boolean.TRUE : null)
        .tolerance(args.doubleValue("tolerance"))
        .maxRows(args.intValue("max-rows"))
        .delimiter(args.charValue("delimiter"))
        .encoding(args.value("encoding", null))
        .engine(args.value("engine", null))
        .threads(args.intValue("threads"))
        .memoryLimit(args.value("memory-limit", null))
        .exportDir(args.value("export-dir", null));

    Options opt = builder.build();
    if (opt.key().isEmpty()) {
      throw new CsvDiffException("--key (or a profile with key) is required");
    }

    CompareResult result = CsvDiff.compare(a, b, opt);

    Path outPath = Path.of(args.value("out", "o") != null ? args.value("out", "o")
        : stem(a) + "__vs__" + stem(b) + ".html");
    Files.writeString(outPath, ReportRenderer.render(result, !args.flag("no-compress", null)),
        StandardCharsets.UTF_8);

    String jsonPath = args.value("json", null);
    if (jsonPath != null) {
      var mapper = JsonMapper.builder().enable(SerializationFeature.INDENT_OUTPUT).build();
      Map<String, Object> summary = result.summary();
      Files.writeString(Path.of(jsonPath), mapper.writeValueAsString(summary), StandardCharsets.UTF_8);
    }

    var c = result.counts();
    out.printf(
        Locale.US,
        "A %,d rows | B %,d rows | matched %,d (changed %,d) | added %,d | removed %,d | "
            + "dup keys A %,d B %,d | %s %ss%n",
        c.aRows(), c.bRows(), c.matched(), c.changed(), c.added(), c.removed(),
        c.aDupKeys(), c.bDupKeys(), result.meta().engine(), result.meta().seconds());
    out.printf(Locale.US, "Report: %s (%.0f KB)%n", outPath, Files.size(outPath) / 1024.0);

    if (args.flag("fail-on-dups", null) && (c.aDupKeys() > 0 || c.bDupKeys() > 0)) {
      return 3;
    }
    return result.identical() ? 0 : 1;
  }

  private static String stem(Path p) {
    String name = p.getFileName().toString();
    int dot = name.lastIndexOf('.');
    return dot > 0 ? name.substring(0, dot) : name;
  }

  /**
   * A small GNU-style argument reader: {@code --name value}, {@code --name=value}, {@code -n value}
   * and bare flags, with everything else treated as positional.
   */
  static final class Args {
    private final List<String> raw;

    Args(String[] argv) {
      // argv[0] is the command name.
      this.raw = List.of(argv).subList(1, argv.length);
    }

    private static boolean matches(String token, String longName, String shortName) {
      return token.equals("--" + longName)
          || token.startsWith("--" + longName + "=")
          || (shortName != null && token.equals("-" + shortName));
    }

    String value(String longName, String shortName) {
      for (int i = 0; i < raw.size(); i++) {
        String t = raw.get(i);
        if (t.startsWith("--" + longName + "=")) {
          return t.substring(longName.length() + 3);
        }
        if (matches(t, longName, shortName)) {
          if (i + 1 >= raw.size()) {
            throw new CsvDiffException("--" + longName + " needs a value");
          }
          return raw.get(i + 1);
        }
      }
      return null;
    }

    boolean flag(String longName, String shortName) {
      return raw.stream().anyMatch(t -> matches(t, longName, shortName));
    }

    Double doubleValue(String longName) {
      String v = value(longName, null);
      if (v == null) {
        return null;
      }
      try {
        return Double.valueOf(v);
      } catch (NumberFormatException e) {
        throw new CsvDiffException("--" + longName + " must be a number, got: " + v);
      }
    }

    Integer intValue(String longName) {
      String v = value(longName, null);
      if (v == null) {
        return null;
      }
      try {
        return Integer.valueOf(v);
      } catch (NumberFormatException e) {
        throw new CsvDiffException("--" + longName + " must be an integer, got: " + v);
      }
    }

    Character charValue(String longName) {
      String v = value(longName, null);
      return v == null || v.isEmpty() ? null : v.charAt(0);
    }

    /** Tokens that are not options and not an option's value. */
    List<String> positional() {
      var out = new ArrayList<String>();
      for (int i = 0; i < raw.size(); i++) {
        String t = raw.get(i);
        if (!t.startsWith("-")) {
          out.add(t);
          continue;
        }
        if (t.contains("=") || isBareFlag(t)) {
          continue;
        }
        i++; // this option consumes the next token
      }
      return List.copyOf(out);
    }

    private static boolean isBareFlag(String token) {
      return switch (token) {
        case "--trim", "--ignore-case", "--empty-is-null", "--no-compress", "--fail-on-dups",
             "--help", "-h" -> true;
        default -> false;
      };
    }
  }
}
