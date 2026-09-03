package dev.csvdiff;

import dev.csvdiff.Contract.EngineMeta;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.regex.Pattern;

/**
 * Column resolution and cell semantics shared by every engine.
 *
 * <p>These are the rules that make the engines interchangeable, so they live in one place: which
 * columns get compared, what normalisation means, when two cells differ, and how key values sort.
 */
public final class Columns {

  private Columns() {}

  /** Which columns take part, and which exist on only one side. */
  public record Resolved(List<String> compared, List<String> onlyInA, List<String> onlyInB) {

    public EngineMeta toMeta(List<String> key, int aCols, int bCols) {
      return new EngineMeta(key, compared, onlyInA, onlyInB, aCols, bCols);
    }
  }

  /**
   * Works out the compared columns from the two headers.
   *
   * @throws CsvDiffException if a key column is missing from either file, or an explicitly
   *     requested compare column is not present in both
   */
  public static Resolved resolve(List<String> aCols, List<String> bCols, Options opt) {
    Set<String> inA = new LinkedHashSet<>(aCols);
    Set<String> inB = new LinkedHashSet<>(bCols);

    List<String> missing = opt.key().stream().filter(k -> !inA.contains(k) || !inB.contains(k)).toList();
    if (!missing.isEmpty()) {
      throw new CsvDiffException("Key column(s) missing from one of the files: " + String.join(", ", missing));
    }

    List<String> common = aCols.stream().filter(inB::contains).toList();
    Set<String> keys = Set.copyOf(opt.key());
    List<String> compared;
    if (opt.compare() == null) {
      compared = common.stream().filter(c -> !keys.contains(c)).toList();
    } else {
      Set<String> commonSet = Set.copyOf(common);
      List<String> bad = opt.compare().stream().filter(c -> !commonSet.contains(c)).toList();
      if (!bad.isEmpty()) {
        throw new CsvDiffException("Compare column(s) not present in both files: " + String.join(", ", bad));
      }
      compared = opt.compare().stream().filter(c -> !keys.contains(c)).toList();
    }
    Set<String> ignored = Set.copyOf(opt.ignore());
    compared = compared.stream().filter(c -> !ignored.contains(c)).toList();

    return new Resolved(
        compared,
        aCols.stream().filter(c -> !inB.contains(c)).toList(),
        bCols.stream().filter(c -> !inA.contains(c)).toList());
  }

  /** Applies {@code --trim}, {@code --ignore-case} and {@code --empty-is-null} to one cell. */
  public static String normalise(String value, Options opt) {
    if (value == null) {
      return null;
    }
    String s = value;
    if (opt.trim()) {
      s = s.strip();
    }
    if (opt.ignoreCase()) {
      s = s.toLowerCase(Locale.ROOT);
    }
    if (opt.emptyIsNull() && s.isEmpty()) {
      return null;
    }
    return s;
  }

  private static final Pattern NUMBER = Pattern.compile("[+-]?(\\d+\\.?\\d*|\\.\\d+)([eE][+-]?\\d+)?");

  /** The numeric value of a cell, or {@code null} when it does not parse as one. */
  private static Double asNumber(String v) {
    if (v == null) {
      return null;
    }
    String s = v.strip();
    return NUMBER.matcher(s).matches() ? Double.valueOf(s) : null;
  }

  /**
   * SQL {@code IS DISTINCT FROM}, with the numeric tolerance applied where both sides parse as
   * numbers. Two absent values are equal; one absent value differs from any present one.
   */
  public static boolean differs(String a, String b, Options opt) {
    if (a == null && b == null) {
      return false;
    }
    if (opt.tolerance() > 0) {
      Double na = asNumber(a);
      Double nb = asNumber(b);
      if (na != null && nb != null) {
        return Math.abs(na - nb) > opt.tolerance();
      }
    }
    return a == null || b == null || !a.equals(b);
  }

  /** Orders key values the way DuckDB orders VARCHAR: ascending, absent values last. */
  public static Comparator<List<String>> keyOrder(int keySize) {
    return (x, y) -> {
      for (int i = 0; i < keySize; i++) {
        String a = x.get(i);
        String b = y.get(i);
        if (a == null && b == null) {
          continue;
        }
        if (a == null) {
          return 1;
        }
        if (b == null) {
          return -1;
        }
        int c = a.compareTo(b);
        if (c != 0) {
          return c;
        }
      }
      return 0;
    };
  }

  /**
   * A composite key flattened into one string for hashing. {@code \0} marks an absent value and
   * {@code \1} separates columns, neither of which can appear in a CSV field, so distinct keys
   * cannot collide.
   */
  public static String keyOf(List<String> row, int keySize) {
    var sb = new StringBuilder();
    for (int i = 0; i < keySize; i++) {
      String v = row.get(i);
      sb.append(v == null ? '\0' : v).append('\1');
    }
    return sb.toString();
  }

  /** The key columns of a row, as the report embeds them. */
  public static List<Object> keyValues(List<String> row, int keySize) {
    List<Object> out = new ArrayList<>(keySize);
    for (int i = 0; i < keySize; i++) {
      out.add(row.get(i));
    }
    return out;
  }
}
