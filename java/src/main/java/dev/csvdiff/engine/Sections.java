package dev.csvdiff.engine;

import dev.csvdiff.Contract.EngineMeta;
import dev.csvdiff.Contract.EngineResult;
import dev.csvdiff.Contract.Section;
import dev.csvdiff.Options;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.io.Writer;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/** Turns a completed join into the capped report sections, and writes the uncapped CSV exports. */
public final class Sections {

  private Sections() {}

  /** Builds the {@link EngineResult}, capping each row list and writing {@code --export-dir}. */
  public static EngineResult assemble(
      EngineMeta meta,
      RowStore.Joined joined,
      Section dupA,
      Section dupB,
      Options opt,
      List<String> compared) {

    List<String> key = opt.key();
    var wanted = new ArrayList<>(key);
    wanted.addAll(compared);
    List<String> cols = List.copyOf(wanted);

    if (opt.exportDir() != null) {
      export(joined, opt, key, compared, cols);
    }

    return new EngineResult(
        meta,
        joined.counts(),
        joined.columns(),
        RowStore.section(key, joined.changed(), opt.maxRows()),
        RowStore.section(cols, joined.added(), opt.maxRows()),
        RowStore.section(cols, joined.removed(), opt.maxRows()),
        dupA,
        dupB);
  }

  private static void export(
      RowStore.Joined joined, Options opt, List<String> key, List<String> compared, List<String> cols) {
    Path dir = Path.of(opt.exportDir());
    try {
      Files.createDirectories(dir);
      writeCsv(dir.resolve("added.csv"), cols, joined.added());
      writeCsv(dir.resolve("removed.csv"), cols, joined.removed());

      var both = new ArrayList<>(key);
      for (String c : compared) {
        both.add(c + " (A)");
        both.add(c + " (B)");
      }
      var rows = new ArrayList<List<Object>>(joined.changedA().size());
      int keySize = key.size();
      for (int r = 0; r < joined.changedA().size(); r++) {
        List<String> ar = joined.changedA().get(r);
        List<String> br = joined.changedB().get(r);
        var row = new ArrayList<Object>(both.size());
        for (int i = 0; i < keySize; i++) {
          row.add(ar.get(i));
        }
        for (int i = 0; i < compared.size(); i++) {
          row.add(ar.get(keySize + i));
          row.add(br.get(keySize + i));
        }
        rows.add(row);
      }
      writeCsv(dir.resolve("changed.csv"), both, rows);
    } catch (IOException e) {
      throw new UncheckedIOException("Cannot write export-dir " + dir, e);
    }
  }

  private static void writeCsv(Path path, List<String> header, List<List<Object>> rows) throws IOException {
    try (Writer w = Files.newBufferedWriter(path)) {
      writeRow(w, header);
      for (List<Object> row : rows) {
        writeRow(w, row);
      }
    }
  }

  private static void writeRow(Writer w, List<?> values) throws IOException {
    for (int i = 0; i < values.size(); i++) {
      if (i > 0) {
        w.write(',');
      }
      Object v = values.get(i);
      if (v != null) {
        w.write(quote(v.toString()));
      }
    }
    w.write('\n');
  }

  private static String quote(String v) {
    if (v.indexOf('"') < 0 && v.indexOf(',') < 0 && v.indexOf('\n') < 0 && v.indexOf('\r') < 0) {
      return v;
    }
    return '"' + v.replace("\"", "\"\"") + '"';
  }
}
