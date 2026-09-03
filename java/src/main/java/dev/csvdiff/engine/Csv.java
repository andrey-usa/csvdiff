package dev.csvdiff.engine;

/** Small CSV conventions that every in-heap engine has to agree on. */
public final class Csv {

  private Csv() {}

  private static final char[] CANDIDATES = {',', ';', '\t', '|'};

  /**
   * Guesses the delimiter from the header line by picking whichever candidate appears most often,
   * defaulting to a comma. Matches what DuckDB's sniffer settles on for the files this tool sees.
   */
  public static char detectDelimiter(String headerLine) {
    char best = ',';
    int bestCount = -1;
    for (char c : CANDIDATES) {
      int n = 0;
      for (int i = 0; i < headerLine.length(); i++) {
        if (headerLine.charAt(i) == c) {
          n++;
        }
      }
      if (n > bestCount) {
        best = c;
        bestCount = n;
      }
    }
    return best;
  }

  /**
   * An empty field is an absent value, quoted or not.
   *
   * <p>This is what DuckDB's reader does, and the Python and TypeScript implementations follow it,
   * so the Java engines must too or a count would change with the engine.
   */
  public static String emptyToNull(String field) {
    return field == null || field.isEmpty() ? null : field;
  }
}
