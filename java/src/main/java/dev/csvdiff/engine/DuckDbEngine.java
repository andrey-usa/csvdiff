package dev.csvdiff.engine;

import dev.csvdiff.Columns;
import dev.csvdiff.CompareEngine;
import dev.csvdiff.Contract.CellDiff;
import dev.csvdiff.Contract.ColumnStat;
import dev.csvdiff.Contract.Counts;
import dev.csvdiff.Contract.EngineResult;
import dev.csvdiff.Contract.Section;
import dev.csvdiff.Options;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;
import java.util.StringJoiner;

/**
 * DuckDB engine, over JDBC. The default, and the only one here that is not bounded by the heap.
 *
 * <p>Both files are read as text, normalised, de-duplicated on the key so the first occurrence
 * joins, then full-outer-joined; only the differing cells are pulled back into Java. DuckDB
 * hash-joins in parallel and spills to disk, so files larger than memory are fine.
 *
 * <p>SQL is assembled by string interpolation, so identifiers go through {@link #q} and string
 * literals and paths through {@link #lit}. Nothing else is interpolated.
 */
public final class DuckDbEngine implements CompareEngine {

  private static final String STATUS_MATCHED = "matched";

  @Override
  public EngineResult compare(Path aPath, Path bPath, Options opt) throws SQLException {
    try (Connection con = DriverManager.getConnection("jdbc:duckdb:");
        Statement st = con.createStatement()) {

      if (opt.threads() != null) {
        st.execute("SET threads = " + opt.threads());
      }
      if (opt.memoryLimit() != null) {
        st.execute("SET memory_limit = " + lit(opt.memoryLimit()));
      }
      st.execute("SET preserve_insertion_order = true");

      String readOpts = readOptions(opt);
      List<String> aCols = load(st, "a", aPath, readOpts);
      List<String> bCols = load(st, "b", bPath, readOpts);
      var resolved = Columns.resolve(aCols, bCols, opt);
      List<String> key = opt.key();
      List<String> compared = resolved.compared();
      int keySize = key.size();

      // Normalised projection plus a stable row number, so "first occurrence wins" is well defined.
      for (String tbl : List.of("a", "b")) {
        var proj = new StringJoiner(", ");
        for (String c : key) {
          proj.add(norm(q(c), opt) + " AS " + q(c));
        }
        for (String c : compared) {
          proj.add(norm(q(c), opt) + " AS " + q(c));
        }
        st.execute(
            "CREATE TABLE " + tbl + " AS SELECT " + proj + ", row_number() OVER () AS _rn FROM " + tbl + "_raw");
        st.execute("DROP TABLE " + tbl + "_raw");
      }

      String kq = quotedList(key);
      long aRows = scalar(st, "SELECT count(*) FROM a");
      long bRows = scalar(st, "SELECT count(*) FROM b");

      var dups = new ArrayList<Section>(2);
      long[] dupKeys = new long[2];
      long[] dupRows = new long[2];
      long[] uniqueKeys = new long[2];
      long[] totals = {aRows, bRows};

      for (int i = 0; i < 2; i++) {
        String tbl = i == 0 ? "a" : "b";
        st.execute(
            "CREATE TABLE " + tbl + "_dup AS SELECT " + kq + ", count(*) AS n FROM " + tbl
                + " GROUP BY ALL HAVING count(*) > 1");
        dupKeys[i] = scalar(st, "SELECT count(*) FROM " + tbl + "_dup");
        dupRows[i] = scalar(st, "SELECT coalesce(sum(n), 0) FROM " + tbl + "_dup");
        uniqueKeys[i] = totals[i] - dupRows[i] + dupKeys[i];

        var rows = new ArrayList<List<Object>>();
        try (ResultSet rs =
            st.executeQuery(
                "SELECT * FROM " + tbl + "_dup ORDER BY n DESC, " + kq + " LIMIT " + opt.maxRows())) {
          while (rs.next()) {
            var row = new ArrayList<Object>(keySize + 1);
            for (int c = 0; c < keySize; c++) {
              row.add(rs.getString(c + 1));
            }
            row.add(rs.getLong(keySize + 1));
            rows.add(row);
          }
        }
        var cols = new ArrayList<>(key);
        cols.add("count");
        dups.add(new Section(List.copyOf(cols), List.copyOf(rows), dupKeys[i] > opt.maxRows()));

        st.execute(
            "CREATE TABLE " + tbl + "1 AS SELECT * EXCLUDE(_k) FROM (SELECT *, row_number() OVER "
                + "(PARTITION BY " + kq + " ORDER BY _rn) AS _k FROM " + tbl + ") WHERE _k = 1");
      }

      buildJoin(st, key, compared, opt);

      long changed = 0;
      long unchanged = 0;
      long added = 0;
      long removed = 0;
      try (ResultSet rs = st.executeQuery("SELECT _status, _changed, count(*) FROM j GROUP BY ALL")) {
        while (rs.next()) {
          String status = rs.getString(1);
          boolean isChanged = rs.getBoolean(2);
          long n = rs.getLong(3);
          switch (status) {
            case STATUS_MATCHED -> {
              if (isChanged) {
                changed += n;
              } else {
                unchanged += n;
              }
            }
            case "added" -> added += n;
            case "removed" -> removed += n;
            default -> throw new IllegalStateException("Unexpected row status: " + status);
          }
        }
      }

      var counts =
          new Counts(
              aRows, bRows, uniqueKeys[0], uniqueKeys[1],
              unchanged + changed, unchanged, changed, added, removed,
              dupKeys[0], dupRows[0], dupKeys[1], dupRows[1]);

      List<ColumnStat> columns = columnStats(st, compared);
      Section changedSection = changedRows(st, key, compared, opt, counts);
      Section addedSection = sideRows(st, "added", "b_", key, compared, opt, counts);
      Section removedSection = sideRows(st, "removed", "a_", key, compared, opt, counts);

      if (opt.exportDir() != null) {
        exportAll(st, key, compared, opt);
      }

      return new EngineResult(
          resolved.toMeta(key, aCols.size(), bCols.size()),
          counts, columns, changedSection, addedSection, removedSection, dups.get(0), dups.get(1));
    }
  }

  private static String readOptions(Options opt) {
    var sb = new StringBuilder("all_varchar=true, header=true, sample_size=-1");
    if (opt.delimiter() != null) {
      sb.append(", delim=").append(lit(String.valueOf(opt.delimiter())));
    }
    String enc = opt.encoding();
    if (enc != null && !enc.equalsIgnoreCase("utf-8") && !enc.equalsIgnoreCase("utf8")) {
      sb.append(", encoding=").append(lit(enc));
    }
    return sb.toString();
  }

  private static List<String> load(Statement st, String tbl, Path path, String readOpts) throws SQLException {
    st.execute(
        "CREATE TABLE " + tbl + "_raw AS SELECT * FROM read_csv("
            + lit(path.toAbsolutePath().toString()) + ", " + readOpts + ")");
    var cols = new ArrayList<String>();
    try (ResultSet rs = st.executeQuery("DESCRIBE " + tbl + "_raw")) {
      while (rs.next()) {
        cols.add(rs.getString(1));
      }
    }
    return List.copyOf(cols);
  }

  private static void buildJoin(Statement st, List<String> key, List<String> compared, Options opt)
      throws SQLException {
    var keySel = new StringJoiner(", ");
    var on = new StringJoiner(" AND ");
    for (String k : key) {
      keySel.add("coalesce(a." + q(k) + ", b." + q(k) + ") AS " + q(k));
      on.add("a." + q(k) + " IS NOT DISTINCT FROM b." + q(k));
    }
    var colSel = new StringJoiner(", ");
    var flags = new StringJoiner(" OR ");
    for (int i = 0; i < compared.size(); i++) {
      String a = "a." + q(compared.get(i));
      String b = "b." + q(compared.get(i));
      String d = diff(a, b, opt);
      colSel.add(a + " AS " + q("a_" + i));
      colSel.add(b + " AS " + q("b_" + i));
      colSel.add(d + " AS " + q("d_" + i));
      flags.add(d);
    }
    String changedExpr = compared.isEmpty() ? "false" : flags.toString();
    st.execute(
        "CREATE TABLE j AS SELECT " + keySel + ", "
            + "CASE WHEN a._rn IS NULL THEN 'added' WHEN b._rn IS NULL THEN 'removed' ELSE 'matched' END AS _status, "
            + (compared.isEmpty() ? "" : colSel + ", ")
            + "(" + changedExpr + ") AS _changed "
            + "FROM a1 a FULL OUTER JOIN b1 b ON " + on);
  }

  private static List<ColumnStat> columnStats(Statement st, List<String> compared) throws SQLException {
    if (compared.isEmpty()) {
      return List.of();
    }
    var agg = new StringJoiner(", ");
    for (int i = 0; i < compared.size(); i++) {
      agg.add("sum(" + q("d_" + i) + "::INT)");
      agg.add("sum((" + q("a_" + i) + " IS NOT NULL AND " + q("b_" + i) + " IS NULL)::INT)");
      agg.add("sum((" + q("a_" + i) + " IS NULL AND " + q("b_" + i) + " IS NOT NULL)::INT)");
    }
    var out = new ArrayList<ColumnStat>(compared.size());
    try (ResultSet rs = st.executeQuery("SELECT " + agg + " FROM j WHERE _status = 'matched'")) {
      if (rs.next()) {
        for (int i = 0; i < compared.size(); i++) {
          out.add(new ColumnStat(compared.get(i), rs.getLong(3 * i + 1), rs.getLong(3 * i + 2), rs.getLong(3 * i + 3)));
        }
      }
    }
    return List.copyOf(out);
  }

  private static Section changedRows(
      Statement st, List<String> key, List<String> compared, Options opt, Counts counts) throws SQLException {
    int keySize = key.size();
    var rows = new ArrayList<List<Object>>();
    if (!compared.isEmpty()) {
      var cols = new StringJoiner(", ");
      cols.add(quotedList(key));
      for (int i = 0; i < compared.size(); i++) {
        cols.add(q("a_" + i) + ", " + q("b_" + i) + ", " + q("d_" + i));
      }
      String sql =
          "SELECT " + cols + " FROM j WHERE _status = 'matched' AND _changed ORDER BY "
              + quotedList(key) + " LIMIT " + opt.maxRows();
      try (ResultSet rs = st.executeQuery(sql)) {
        while (rs.next()) {
          var row = new ArrayList<Object>(keySize + 1);
          for (int c = 0; c < keySize; c++) {
            row.add(rs.getString(c + 1));
          }
          var cells = new ArrayList<CellDiff>();
          for (int i = 0; i < compared.size(); i++) {
            int base = keySize + 3 * i;
            if (rs.getBoolean(base + 3)) {
              cells.add(new CellDiff(i, rs.getString(base + 1), rs.getString(base + 2)));
            }
          }
          row.add(cells);
          rows.add(row);
        }
      }
    }
    return new Section(key, List.copyOf(rows), counts.changed() > opt.maxRows());
  }

  private static Section sideRows(
      Statement st, String status, String prefix, List<String> key, List<String> compared,
      Options opt, Counts counts) throws SQLException {
    var cols = new StringJoiner(", ");
    cols.add(quotedList(key));
    for (int i = 0; i < compared.size(); i++) {
      cols.add(q(prefix + i));
    }
    var rows = new ArrayList<List<Object>>();
    String sql =
        "SELECT " + cols + " FROM j WHERE _status = " + lit(status) + " ORDER BY "
            + quotedList(key) + " LIMIT " + opt.maxRows();
    int width = key.size() + compared.size();
    try (ResultSet rs = st.executeQuery(sql)) {
      while (rs.next()) {
        var row = new ArrayList<Object>(width);
        for (int c = 0; c < width; c++) {
          row.add(rs.getString(c + 1));
        }
        rows.add(row);
      }
    }
    var all = new ArrayList<>(key);
    all.addAll(compared);
    return new Section(List.copyOf(all), List.copyOf(rows), counts.forStatus(status) > opt.maxRows());
  }

  private static void exportAll(Statement st, List<String> key, List<String> compared, Options opt)
      throws SQLException {
    Path dir = Path.of(opt.exportDir());
    try {
      // DuckDB's COPY will not create the directory for us.
      Files.createDirectories(dir);
    } catch (IOException e) {
      throw new UncheckedIOException("Cannot create export-dir " + dir, e);
    }
    String kq = quotedList(key);
    st.execute("COPY (SELECT " + kq + aliases("b_", compared) + " FROM j WHERE _status='added' ORDER BY " + kq
        + ") TO " + lit(dir.resolve("added.csv").toString()) + " (HEADER)");
    st.execute("COPY (SELECT " + kq + aliases("a_", compared) + " FROM j WHERE _status='removed' ORDER BY " + kq
        + ") TO " + lit(dir.resolve("removed.csv").toString()) + " (HEADER)");
    if (!compared.isEmpty()) {
      var both = new StringBuilder();
      for (int i = 0; i < compared.size(); i++) {
        both.append(", ").append(q("a_" + i)).append(" AS ").append(q(compared.get(i) + " (A)"));
        both.append(", ").append(q("b_" + i)).append(" AS ").append(q(compared.get(i) + " (B)"));
      }
      st.execute("COPY (SELECT " + kq + both + " FROM j WHERE _status='matched' AND _changed ORDER BY " + kq
          + ") TO " + lit(dir.resolve("changed.csv").toString()) + " (HEADER)");
    }
  }

  private static String aliases(String prefix, List<String> compared) {
    var sb = new StringBuilder();
    for (int i = 0; i < compared.size(); i++) {
      sb.append(", ").append(q(prefix + i)).append(" AS ").append(q(compared.get(i)));
    }
    return sb.toString();
  }

  private static long scalar(Statement st, String sql) throws SQLException {
    try (ResultSet rs = st.executeQuery(sql)) {
      return rs.next() ? rs.getLong(1) : 0L;
    }
  }

  private static String norm(String expr, Options opt) {
    String e = expr;
    if (opt.trim()) {
      e = "trim(" + e + ")";
    }
    if (opt.ignoreCase()) {
      e = "lower(" + e + ")";
    }
    if (opt.emptyIsNull()) {
      e = "nullif(" + e + ", '')";
    }
    return e;
  }

  private static String diff(String a, String b, Options opt) {
    if (opt.tolerance() > 0) {
      return "(CASE WHEN try_cast(" + a + " AS DOUBLE) IS NOT NULL AND try_cast(" + b + " AS DOUBLE) IS NOT NULL "
          + "THEN abs(try_cast(" + a + " AS DOUBLE) - try_cast(" + b + " AS DOUBLE)) > " + opt.tolerance() + " "
          + "ELSE (" + a + " IS DISTINCT FROM " + b + ") END)";
    }
    return "(" + a + " IS DISTINCT FROM " + b + ")";
  }

  private static String quotedList(List<String> names) {
    var j = new StringJoiner(", ");
    names.forEach(n -> j.add(q(n)));
    return j.toString();
  }

  /** Quotes a SQL identifier. */
  private static String q(String name) {
    return '"' + name.replace("\"", "\"\"") + '"';
  }

  /** Quotes a SQL string literal. */
  private static String lit(String s) {
    return "'" + s.replace("'", "''") + "'";
  }
}
