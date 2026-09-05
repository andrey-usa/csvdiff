package dev.csvdiff;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertLinesMatch;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.csvdiff.Contract.CompareResult;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.EnumSource;

/**
 * The engines must be interchangeable. These tests pin the numbers the Python and TypeScript
 * implementations produce for the same files, and then assert every engine reproduces them.
 */
class EngineParityTest {

  private static final Path EXAMPLES = Path.of("..", "examples");
  private static final Path A = EXAMPLES.resolve("orders_2026-08.csv");
  private static final Path B = EXAMPLES.resolve("orders_2026-09.csv");
  private static final List<String> KEY = List.of("order_id", "line_no");

  private static Options.Builder base() {
    return Options.builder().key(KEY).ignore(List.of("updated_at"));
  }

  @ParameterizedTest
  @EnumSource(value = EngineName.class, names = "AUTO", mode = EnumSource.Mode.EXCLUDE)
  @DisplayName("counts and tolerance match the reference implementations")
  void countsAndTolerance(EngineName engine) {
    var result = CsvDiff.compare(A, B, base().tolerance(0.005).engine(engine.label()).build());
    var c = result.counts();

    assertEquals(engine.label(), result.meta().engine());
    assertEquals(800, c.aRows());
    assertEquals(799, c.bRows());
    assertEquals(10, c.added());
    assertEquals(12, c.removed());
    assertEquals(87, c.changed());
    assertEquals(1, c.aDupKeys());
    assertEquals(1, c.bDupKeys());

    Map<String, Long> changedByColumn =
        result.columns().stream()
            .collect(Collectors.toMap(Contract.ColumnStat::name, Contract.ColumnStat::changed));
    assertEquals(0L, changedByColumn.get("unit_price"), "unit_price differences are inside the tolerance");
    assertEquals(List.of("carrier"), result.meta().onlyInB());
  }

  @ParameterizedTest
  @EnumSource(value = EngineName.class, names = {"AUTO", "TABLESAW"}, mode = EnumSource.Mode.EXCLUDE)
  @DisplayName("a ragged row gets the same answer from every engine that can read one")
  void raggedRowsAgree(EngineName engine, @TempDir Path dir) throws java.io.IOException {
    // One short row and one long one. This used to be the input that split the engines four ways:
    // the byte-level ones padded, FastCSV and Tablesaw refused, and DuckDB abandoned the split and
    // returned the file as a single column, which surfaced as "key column missing" — a wrong
    // diagnosis of a readable file. Everything except Tablesaw now agrees, and Tablesaw's reader
    // has no option to allow it, so it is excluded here and says why at runtime.
    Path a = write(dir, "a.csv", "k,v,w\n1,x,y\n2,short\n3,p,q,EXTRA\n");
    Path b = write(dir, "b.csv", "k,v,w\n1,x,CHANGED\n2,short\n3,p,q,EXTRA\n");

    var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k"))
        .engine(engine.label()).build());

    assertEquals(3, result.counts().matched(), "every key is in both files");
    assertEquals(1, result.counts().changed(), "only row 1 differs");
    assertEquals(0, result.counts().added());
    assertEquals(0, result.counts().removed());
  }

  private static Path write(Path dir, String name, String body) throws java.io.IOException {
    Path path = dir.resolve(name);
    Files.writeString(path, body, java.nio.charset.StandardCharsets.UTF_8);
    return path;
  }

  @Test
  @DisplayName("every engine agrees on an input built from the shapes that have broken one")
  void awkwardInputAgrees(@TempDir Path dir) throws java.io.IOException {
    // The sweep above varies the options over clean ASCII data. Both of the wrong answers this
    // project has shipped needed neither an unusual option nor a malformed file: one wanted a key
    // in the last eight bytes, the other a key outside ASCII. So this varies the input instead, and
    // puts every shape that has broken an engine, or plausibly could, into one pair of files.
    //
    // What it asserts is not a particular answer but that the engines cannot disagree about one.
    // Under the default options CAFE-acute and cafe-acute are different keys and under
    // --ignore-case they are the same; either is fine, and every engine has to say the same thing.
    String header = "k,v,w\n";
    Path a = write(dir, "a.csv", header
        + "CAF\u00c9,alpha,1\n"           // non-ASCII key, upper case
        + "  padded  ,beta,2\n"            // whitespace around the key
        + "\u212a,gamma,3\n"               // KELVIN SIGN: folds across the ASCII boundary
        + "\"has,comma\",delta,4\n"        // quoted key holding the delimiter
        + "\"a\"\"b\",epsilon,5\n"         // quoted key holding a doubled quote
        + "\"two\nlines\",zeta,6\n"       // quoted key holding a newline
        + "dup,eta,7\n"
        + "dup,eta,8\n"                    // duplicate key
        + "blank,,9\n"                     // empty value
        + "gone,theta,10\n"                // only in A
        + "z,omega,11\n");                 // short key in the last row, near end of file
    Path b = write(dir, "b.csv", header
        + "caf\u00e9,alpha,1\n"            // same key folded, differing only in case
        + "padded,beta,CHANGED\n"          // same key untrimmed, value differs
        + "k,gamma,3\n"                    // the Kelvin sign's fold
        + "\"has,comma\",delta,4\n"
        + "\"a\"\"b\",epsilon,CHANGED\n"
        + "\"two\nlines\",zeta,6\n"
        + "dup,eta,7\n"
        + "dup,eta,8\n"
        + "blank,,9\n"
        + "extra,iota,12\n"                // only in B
        + "z,omega,11\n");

    List<Options.Builder> variants =
        List.of(
            Options.builder().key(List.of("k")),
            Options.builder().key(List.of("k")).trim(true),
            Options.builder().key(List.of("k")).ignoreCase(true),
            Options.builder().key(List.of("k")).trim(true).ignoreCase(true),
            Options.builder().key(List.of("k")).trim(true).ignoreCase(true).emptyIsNull(true));

    for (Options.Builder variant : variants) {
      // Tablesaw's reader strips the whitespace around every field and offers no way to stop it,
      // so that engine cannot tell "  padded  " from "padded" and answers every run as though
      // --trim were set. Where it is set there is nothing to disagree about; where it is not, this
      // sweep would only re-discover a limitation the engine documents. See TablesawEngine.
      boolean trimmed = variant.build().trim();
      CompareResult reference = null;
      for (EngineName engine : EngineName.concrete()) {
        if (engine == EngineName.TABLESAW && !trimmed) {
          continue;
        }
        var result = CsvDiff.compare(a, b, variant.engine(engine.label()).build());
        if (reference == null) {
          reference = result;
          continue;
        }
        String where = engine.label() + " vs " + reference.meta().engine();
        assertEquals(reference.counts(), result.counts(), where);
        assertEquals(reference.columns(), result.columns(), where);
        assertEquals(reference.changed(), result.changed(), where);
        assertEquals(reference.added(), result.added(), where);
        assertEquals(reference.removed(), result.removed(), where);
        assertEquals(reference.dupA(), result.dupA(), where);
        assertEquals(reference.dupB(), result.dupB(), where);
      }
    }
  }

  @Test
  @DisplayName("every engine agrees, under several option sets")
  void enginesAgree() {
    List<Options.Builder> variants =
        List.of(
            base(),
            base().tolerance(0.005),
            base().trim(true).ignoreCase(true).emptyIsNull(true));

    for (Options.Builder variant : variants) {
      CompareResult reference = null;
      for (EngineName engine : EngineName.concrete()) {
        var result = CsvDiff.compare(A, B, variant.engine(engine.label()).build());
        if (reference == null) {
          reference = result;
          continue;
        }
        String where = engine.label() + " vs " + reference.meta().engine();
        assertEquals(reference.counts(), result.counts(), where);
        assertEquals(reference.columns(), result.columns(), where);
        assertEquals(reference.changed(), result.changed(), where);
        assertEquals(reference.added(), result.added(), where);
        assertEquals(reference.removed(), result.removed(), where);
        assertEquals(reference.dupA(), result.dupA(), where);
        assertEquals(reference.dupB(), result.dupB(), where);
        assertEquals(reference.meta().compared(), result.meta().compared(), where);
      }
    }
  }

  @ParameterizedTest
  @EnumSource(value = EngineName.class, names = "AUTO", mode = EnumSource.Mode.EXCLUDE)
  @DisplayName("a quoted empty field is absent, like DuckDB reads it")
  void quotedEmptyIsAbsent(EngineName engine, @TempDir Path dir) throws Exception {
    Path a = dir.resolve("a.csv");
    Path b = dir.resolve("b.csv");
    Files.writeString(a, "k,v\n1,\n2,\"\"\n3,keep\n");
    Files.writeString(b, "k,v\n1,\"\"\n2,\n3,keep\n");

    var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k")).engine(engine.label()).build());
    assertEquals(0, result.counts().changed(), "quoted and unquoted empties must compare equal");
    assertTrue(result.identical());
  }

  @Test
  @DisplayName("comparing a file with itself finds nothing")
  void identicalFiles() {
    assertTrue(CsvDiff.compare(A, A, Options.builder().key(KEY).build()).identical());
    assertFalse(CsvDiff.compare(A, B, Options.builder().key(KEY).build()).identical());
  }

  @Test
  @DisplayName("auto resolves to a concrete engine that is actually present")
  void autoResolves() {
    assertEquals(EngineName.DUCKDB, CsvDiff.resolveEngine(EngineName.AUTO));
    for (EngineName e : EngineName.concrete()) {
      assertEquals(e, CsvDiff.resolveEngine(e));
    }
  }

  @Test
  @DisplayName("the export directory holds the uncapped rows")
  void exportDir(@TempDir Path dir) throws Exception {
    Path out = dir.resolve("rows");
    CsvDiff.compare(A, B, base().exportDir(out.toString()).build());

    for (String name : List.of("added.csv", "removed.csv", "changed.csv")) {
      assertTrue(Files.isRegularFile(out.resolve(name)), name + " should exist");
    }
    // 10 added rows plus the header, and the export is not capped by --max-rows.
    assertEquals(11, Files.readAllLines(out.resolve("added.csv")).size());
    assertEquals(13, Files.readAllLines(out.resolve("removed.csv")).size());
    assertLinesMatch(
        List.of("order_id,line_no,sku,qty,unit_price,status,ship_date,region"),
        Files.readAllLines(out.resolve("added.csv")).subList(0, 1));
  }
}
