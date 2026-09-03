package dev.csvdiff.bench;

import java.io.BufferedWriter;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.LocalDate;
import java.util.List;
import java.util.Locale;

/**
 * Generates a deterministic pair of CSV files for benchmarking and CI.
 *
 * <p>Both files have 20 columns and share the composite key {@code (account_id, txn_id)}. File B is
 * file A with a controlled amount of drift, so every run has a known answer:
 *
 * <table>
 *   <caption>Drift recipe</caption>
 *   <tr><th>Change</th><th>Share of rows</th></tr>
 *   <tr><td>{@code status} changed</td><td>3.0%</td></tr>
 *   <tr><td>{@code amount} changed</td><td>1.5%</td></tr>
 *   <tr><td>{@code balance} changed</td><td>1.5%</td></tr>
 *   <tr><td>{@code value_date} blanked</td><td>0.3%</td></tr>
 *   <tr><td>{@code updated_at} changed</td><td>100% (excluded with {@code --ignore})</td></tr>
 *   <tr><td>rows only in B</td><td>0.10%</td></tr>
 *   <tr><td>rows only in A</td><td>0.10%</td></tr>
 *   <tr><td>duplicate keys</td><td>0.01% per file</td></tr>
 * </table>
 *
 * <p>The recipe and the hash are the same as the Python and TypeScript generators, so a benchmark
 * number from any of the three is directly comparable.
 */
public final class GenData {

  private GenData() {}

  public static final List<String> COLUMNS =
      List.of(
          "account_id", "txn_id", "posting_date", "value_date", "currency", "amount", "fee",
          "balance", "status", "channel", "region", "branch_code", "product_code", "counterparty",
          "quantity", "rate", "category", "risk_flag", "note", "updated_at");

  private static final String[] STATUS = {"posted", "pending", "settled", "reversed"};
  private static final String[] CHANNEL = {"branch", "online", "mobile", "atm", "wire"};
  private static final String[] REGION = {"EMEA", "NA", "APAC", "LATAM"};
  private static final String[] CURRENCY = {"USD", "EUR", "GBP", "JPY"};
  private static final String[] CATEGORY = {"retail", "corporate", "treasury", "cards", "loans"};

  // Drift buckets, against a 0..9999 hash bucket per row.
  private static final int CHG_STATUS = 300;
  private static final int CHG_AMOUNT = 150;
  private static final int CHG_BALANCE = 150;
  private static final int CHG_VALUE_DATE = 30;
  private static final int REMOVED_MOD = 1000;
  private static final int ADDED_RATIO = 1000;
  private static final int DUP_MOD = 10_000;

  private static final String[] DAYS = days();

  private static String[] days() {
    var out = new String[240];
    LocalDate d0 = LocalDate.of(2026, 1, 1);
    for (int i = 0; i < out.length; i++) {
      out[i] = d0.plusDays(i).toString();
    }
    return out;
  }

  /** splitmix-style hash, matching the other implementations bit for bit. */
  private static long hash(long i, long salt, long seed) {
    long x = i * 31 + salt + seed;
    x = (x ^ (x >>> 30)) * 0xBF58476D1CE4E5B9L;
    x = (x ^ (x >>> 27)) * 0x94D049BB133111EBL;
    return x ^ (x >>> 31);
  }

  private static int mod(long i, long salt, long seed, int m) {
    return (int) Long.remainderUnsigned(hash(i, salt, seed), m);
  }

  /** Builds one row of one side. */
  static String row(long i, boolean b, long seed) {
    int bucket = mod(i, 0, seed, 10_000);
    double amount = mod(i, 21, seed, 900_000_000) / 100.0 - 1_000_000;
    double balance = mod(i, 31, seed, 2_000_000_000) / 100.0;
    String status = STATUS[mod(i, 11, seed, STATUS.length)];
    String valueDate = DAYS[mod(i, 41, seed, 240)];

    if (b) {
      if (bucket < CHG_STATUS) {
        status = STATUS[(mod(i, 11, seed, STATUS.length) + 1) % STATUS.length];
      } else if (bucket < CHG_STATUS + CHG_AMOUNT) {
        amount = amount + 12.34;
      } else if (bucket < CHG_STATUS + CHG_AMOUNT + CHG_BALANCE) {
        balance = balance * 1.01;
      }
      if (bucket < CHG_VALUE_DATE) {
        valueDate = "";
      }
    }

    var sb = new StringBuilder(220);
    sb.append("ACC-").append(pad((i * 7919) % 250_000, 8)).append(',');
    sb.append("TXN-").append(pad(i, 11)).append(',');
    sb.append(DAYS[mod(i, 1, seed, 240)]).append(',');
    sb.append(valueDate).append(',');
    sb.append(CURRENCY[mod(i, 51, seed, 4)]).append(',');
    sb.append(fixed(amount, 2)).append(',');
    sb.append(fixed(mod(i, 61, seed, 5000) / 100.0, 2)).append(',');
    sb.append(fixed(balance, 2)).append(',');
    sb.append(status).append(',');
    sb.append(CHANNEL[mod(i, 71, seed, 5)]).append(',');
    sb.append(REGION[mod(i, 81, seed, 4)]).append(',');
    sb.append("BR").append(pad(mod(i, 91, seed, 900) + 100L, 4)).append(',');
    sb.append('P').append(pad(mod(i, 101, seed, 5000), 5)).append(',');
    sb.append("CP-").append(pad(mod(i, 111, seed, 90_000), 6)).append(',');
    sb.append(mod(i, 121, seed, 500) + 1).append(',');
    sb.append(fixed(mod(i, 131, seed, 1200) / 10_000.0, 4)).append(',');
    sb.append(CATEGORY[mod(i, 141, seed, 5)]).append(',');
    sb.append(mod(i, 151, seed, 20) == 0 ? 'Y' : 'N').append(',');
    sb.append("batch ").append(i % 997 + 1).append(" line ").append(i % 53 + 1).append(',');
    sb.append(b ? "2026-09-01 02:15:00" : "2026-08-01 02:15:00");
    return sb.toString();
  }

  private static String pad(long v, int width) {
    String s = Long.toString(v);
    return s.length() >= width ? s : "0".repeat(width - s.length()) + s;
  }

  private static String fixed(double v, int decimals) {
    return String.format(Locale.ROOT, "%." + decimals + "f", v);
  }

  /** Writes both files in one pass. */
  public static void generate(long rows, Path aPath, Path bPath, long seed) throws IOException {
    String header = String.join(",", COLUMNS);
    long dupExtra = Math.max(1, rows / DUP_MOD);
    long added = Math.max(1, rows / ADDED_RATIO);

    try (BufferedWriter fa = Files.newBufferedWriter(aPath, StandardCharsets.UTF_8);
        BufferedWriter fb = Files.newBufferedWriter(bPath, StandardCharsets.UTF_8)) {
      fa.write(header);
      fa.write('\n');
      fb.write(header);
      fb.write('\n');

      for (long i = 0; i < rows; i++) {
        fa.write(row(i, false, seed));
        fa.write('\n');
        if (i % REMOVED_MOD != 7) {
          fb.write(row(i, true, seed));
          fb.write('\n');
        }
        if (i % DUP_MOD == 3 && i < rows / 2) {
          fb.write(row(i, true, seed));
          fb.write('\n');
        }
      }
      for (long i = 0; i < dupExtra; i++) {
        fa.write(row(i, false, seed));
        fa.write('\n');
      }
      for (long i = rows; i < rows + added; i++) {
        fb.write(row(i, true, seed));
        fb.write('\n');
      }
    }
  }

  /** Parses {@code 10000}, {@code 10k}, {@code 1m}, {@code 2.5M}. */
  public static long parseRows(String s) {
    String t = s.strip().toLowerCase(Locale.ROOT).replace("_", "").replace(",", "");
    char last = t.charAt(t.length() - 1);
    long mult =
        switch (last) {
          case 'k' -> 1_000L;
          case 'm' -> 1_000_000L;
          case 'g' -> 1_000_000_000L;
          default -> 0L;
        };
    return mult == 0 ? Long.parseLong(t) : Math.round(Double.parseDouble(t.substring(0, t.length() - 1)) * mult);
  }

  public static void main(String[] args) throws IOException {
    String rowsArg = "10k";
    String outDir = "data";
    String prefix = null;
    long seed = 7;
    for (int i = 0; i < args.length - 1; i++) {
      switch (args[i]) {
        case "--rows", "-n" -> rowsArg = args[++i];
        case "--out-dir", "-o" -> outDir = args[++i];
        case "--prefix" -> prefix = args[++i];
        case "--seed" -> seed = Long.parseLong(args[++i]);
        default -> { /* ignored, keeps the harness forgiving */ }
      }
    }
    long rows = parseRows(rowsArg);
    Path dir = Path.of(outDir);
    Files.createDirectories(dir);
    String p = prefix != null ? prefix : rowsArg.toLowerCase(Locale.ROOT);
    Path a = dir.resolve(p + "_a.csv");
    Path b = dir.resolve(p + "_b.csv");

    long t0 = System.nanoTime();
    generate(rows, a, b, seed);
    double dt = (System.nanoTime() - t0) / 1e9;
    System.out.printf(Locale.US, "java: %,d rows x %d columns in %.1fs%n", rows, COLUMNS.size(), dt);
    for (Path path : List.of(a, b)) {
      System.out.printf(Locale.US, "  %s  %.1f MB%n", path, Files.size(path) / 1e6);
    }
  }
}
