package dev.csvdiff;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.csvdiff.Contract.CompareResult;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.EnumSource;

/**
 * The parts of the byte-level engines that the shared parity suite does not reach.
 *
 * <p>{@link EngineParityTest} runs every engine over the example files and holds them to the same
 * numbers, which covers the ordinary path. What it does not cover is the CSV shapes the fast
 * engines had to reimplement from scratch — quoting, escapes, ragged rows, CRLF — so those are
 * pinned here against the engine that uses a real CSV parser.
 */
class FastEngineTest {

  /** The engines that parse bytes themselves rather than delegating to a library or to DuckDB. */
  private enum Fast {
    SIMD,
    MMAP,
    SHARD,
    SWAR,
    TURBO;

    EngineName engine() {
      return EngineName.valueOf(name());
    }
  }

  private static Path write(Path dir, String name, String body) throws IOException {
    Path path = dir.resolve(name);
    Files.writeString(path, body, StandardCharsets.UTF_8);
    return path;
  }

  /** Runs one comparison twice: once on the fast engine, once on the reference, and compares. */
  private static void agrees(Path a, Path b, Options.Builder builder, Fast fast) {
    CompareResult reference = CsvDiff.compare(a, b, builder.engine("native").build());
    CompareResult actual = CsvDiff.compare(a, b, builder.engine(fast.engine().label()).build());

    assertEquals(reference.counts(), actual.counts(), "counts");
    assertEquals(reference.columns(), actual.columns(), "column stats");
    assertEquals(reference.changed(), actual.changed(), "changed rows");
    assertEquals(reference.added(), actual.added(), "added rows");
    assertEquals(reference.removed(), actual.removed(), "removed rows");
    assertEquals(reference.dupA(), actual.dupA(), "duplicate keys in A");
    assertEquals(reference.dupB(), actual.dupB(), "duplicate keys in B");
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("--ignore-case folds a non-ASCII key the same way the comparison does")
  void ignoreCaseFoldsNonAsciiKeysConsistently(Fast fast, @TempDir Path dir) throws IOException {
    // These engines decide equality one way and compute the hash another, and under --ignore-case
    // the two had drifted apart: equality handed anything non-ASCII to String.toLowerCase, while
    // the hash lower-cased ASCII bytes and left the rest alone. CAFE-acute therefore compared equal
    // to cafe-acute and hashed apart, so the join never looked at the pair and reported the key as
    // added on one side and removed on the other.
    Path a = write(dir, "a.csv", "k,v\nCAF\u00c9,x\nplain,y\n");
    Path b = write(dir, "b.csv", "k,v\ncaf\u00e9,z\nplain,y\n");
    agrees(a, b, Options.builder().key(List.of("k")).ignoreCase(true), fast);
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("--ignore-case matches a key whose fold changes its length")
  void ignoreCaseFoldsAcrossTheAsciiBoundary(Fast fast, @TempDir Path dir) throws IOException {
    // U+212A KELVIN SIGN is three bytes and lower-cases to the one byte "k". Folding the bytes
    // consistently was not enough on its own: the hash also mixed in the field's length to keep a
    // field and its prefix apart, and mixing the raw three where the fold produced one put the two
    // spellings in different buckets again. The length that gets mixed has to be the folded one.
    Path a = write(dir, "a.csv", "k,v\n\u212a,x\nplain,y\n");
    Path b = write(dir, "b.csv", "k,v\nk,z\nplain,y\n");
    agrees(a, b, Options.builder().key(List.of("k")).ignoreCase(true), fast);
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("--ignore-case on plain ASCII keys still agrees with the reference")
  void ignoreCaseOnAsciiKeys(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k,v\nABC,x\nDeF,y\n");
    Path b = write(dir, "b.csv", "k,v\nabc,x\ndef,z\n");
    agrees(a, b, Options.builder().key(List.of("k")).ignoreCase(true), fast);
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("a key near the end of the file hashes the same as one anywhere else")
  void keyNearTheEndOfTheFileStillMatches(Fast fast, @TempDir Path dir) throws IOException {
    // These engines hash eight bytes at a time, and a field in the last eight bytes of the file
    // cannot be read that way — the read would run off the segment. The bytes therefore have to
    // reach the hash by a second route, and for a while that route produced a different number for
    // the same bytes. The effect was silent and severe: a key hashed one way in the middle of a
    // file and another way at the end of it, so a join missed the last row and reported it as
    // removed from one side and added to the other.
    //
    // Both files below are well formed and hold exactly the keys K1 and K2. They differ only in
    // how many bytes trail the final row's key, which is what decides the route.
    Path a = write(dir, "a.csv", "a,k,c\nx,K1,c1\ny,K2,cc\n");
    Path b = write(dir, "b.csv", "a,k,c\nx,K1,c1\ny,K2,cccccccc\n");

    var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k"))
        .engine(fast.engine().label()).build());

    assertEquals(2, result.counts().matched(), "both keys are in both files");
    assertEquals(0, result.counts().added(), "nothing was added");
    assertEquals(0, result.counts().removed(), "nothing was removed");
    assertEquals(1, result.counts().changed(), "only column a of the last row differs");
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("the key's distance from the end of the file never changes the answer")
  void hashDoesNotDependOnPositionInFile(Fast fast, @TempDir Path dir) throws IOException {
    // The same comparison at every trailing length that straddles the eight-byte boundary, so the
    // fix cannot be right for one padding and wrong for the next.
    for (int trailing = 0; trailing <= 12; trailing++) {
      String tail = "c".repeat(trailing);
      Path a = write(dir, "a" + trailing + ".csv", "a,k,c\nx,K1,c1\ny,K2,z\n");
      Path b = write(dir, "b" + trailing + ".csv", "a,k,c\nx,K1,c1\ny,K2,z" + tail + "\n");

      var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k"))
          .engine(fast.engine().label()).build());

      assertEquals(2, result.counts().matched(), "trailing bytes: " + trailing);
      assertEquals(0, result.counts().added(), "trailing bytes: " + trailing);
      assertEquals(0, result.counts().removed(), "trailing bytes: " + trailing);
    }
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("a quoted field holding a doubled quote keeps its real value")
  void escapedQuotes(Fast fast, @TempDir Path dir) throws IOException {
    // "a""b" is the three-character value a"b, which is not any slice of the file, so it is the
    // one field shape the fast engines have to copy rather than point at.
    Path a = write(dir, "a.csv", "k,v\n1,\"a\"\"b\"\n2,\"plain\"\n3,\"has,comma\"\n");
    Path b = write(dir, "b.csv", "k,v\n1,\"a\"\"c\"\n2,\"plain\"\n3,\"has,comma\"\n");
    agrees(a, b, Options.builder().key(List.of("k")), fast);

    var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k")).engine(fast.engine().label()).build());
    assertEquals(1, result.counts().changed());
    List<Object> row = result.changed().rows().getFirst();
    var cells = (List<?>) row.getLast();
    var diff = (Contract.CellDiff) cells.getFirst();
    assertEquals("a\"b", diff.a());
    assertEquals("a\"c", diff.b());
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("a quoted empty field is absent, like every other engine")
  void quotedEmptyIsAbsent(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k,v\n1,\n2,\"\"\n3,keep\n");
    Path b = write(dir, "b.csv", "k,v\n1,\"\"\n2,\n3,keep\n");
    agrees(a, b, Options.builder().key(List.of("k")), fast);
    var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k")).engine(fast.engine().label()).build());
    assertEquals(0, result.counts().changed());
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("a delimiter or newline inside quotes does not split the row")
  void quotedSeparators(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k,v,w\n1,\"x,y\",end\n2,\"line\nbreak\",end\n");
    Path b = write(dir, "b.csv", "k,v,w\n1,\"x,z\",end\n2,\"line\nbreak\",end\n");
    agrees(a, b, Options.builder().key(List.of("k")), fast);
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("CRLF line endings")
  void crlfLineEndings(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k,v,w\r\n1,x,y\r\n2,p,q\r\n3,a,b\r\n");
    Path b = write(dir, "b.csv", "k,v,w\r\n1,x,z\r\n2,p,q\r\n3,a,b\r\n");
    agrees(a, b, Options.builder().key(List.of("k")), fast);
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("a short row is read, with its missing cells absent")
  void raggedRowsArePadded(Fast fast, @TempDir Path dir) throws IOException {
    // A short row is a difference to report, not a file to refuse. That was once true only of
    // these engines: FastCSV and Tablesaw rejected the file and DuckDB quietly read it as a single
    // column, so the same input got four different answers depending on --engine. The readers that
    // can be told to allow it now are, and EngineParityTest holds them to the same numbers; only
    // Tablesaw still refuses, because its reader has no option for it, and it now says so.
    // Row 1 differs in v, so a non-zero count for w could only come from row 2's missing cell.
    Path a = write(dir, "a.csv", "k,v,w\n1,x,y\n2,short\n");
    Path b = write(dir, "b.csv", "k,v,w\n1,z,y\n2,short\n");
    var result =
        CsvDiff.compare(a, b, Options.builder().key(List.of("k")).engine(fast.engine().label()).build());
    assertEquals(2, result.counts().aRows());
    assertEquals(2, result.counts().matched());
    assertEquals(1, result.counts().changed());
    // Row 2's missing "w" is absent on both sides, so it is not a difference.
    var byName = result.columns().stream().collect(
        java.util.stream.Collectors.toMap(Contract.ColumnStat::name, c -> c));
    assertEquals(0, byName.get("w").changed());
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("a file with no trailing newline loses no row")
  void noTrailingNewline(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k,v\n1,x\n2,y");
    Path b = write(dir, "b.csv", "k,v\n1,x\n2,z");
    agrees(a, b, Options.builder().key(List.of("k")), fast);
    var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k")).engine(fast.engine().label()).build());
    assertEquals(2, result.counts().aRows());
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("blank lines are not rows")
  void blankLines(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k,v\n1,x\n\n2,y\n");
    Path b = write(dir, "b.csv", "k,v\n1,x\n2,z\n");
    agrees(a, b, Options.builder().key(List.of("k")), fast);
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("a semicolon delimiter is sniffed from the header")
  void semicolonDelimiter(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k;v\n1;x\n");
    Path b = write(dir, "b.csv", "k;v\n1;y\n");
    agrees(a, b, Options.builder().key(List.of("k")), fast);
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("normalisation matches the reference on non-ASCII text")
  void nonAsciiNormalisation(Fast fast, @TempDir Path dir) throws IOException {
    // Case folding outside ASCII cannot be done byte by byte, so the fast engines fall back to
    // String for these fields. This pins that they still agree.
    Path a = write(dir, "a.csv", "k,v\n1, Straße \n2,ÉCOLE\n3,ascii\n");
    Path b = write(dir, "b.csv", "k,v\n1,straße\n2,école\n3,ASCII\n");
    agrees(a, b, Options.builder().key(List.of("k")).trim(true).ignoreCase(true), fast);
    var opt = Options.builder().key(List.of("k")).trim(true).ignoreCase(true)
        .engine(fast.engine().label()).build();
    assertEquals(0, CsvDiff.compare(a, b, opt).counts().changed());
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("duplicate keys are counted and the first occurrence joins")
  void duplicateKeys(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k,v\n1,first\n1,second\n1,third\n2,x\n");
    Path b = write(dir, "b.csv", "k,v\n1,first\n2,y\n");
    agrees(a, b, Options.builder().key(List.of("k")), fast);
    var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k")).engine(fast.engine().label()).build());
    assertEquals(1, result.counts().aDupKeys());
    assertEquals(3, result.counts().aDupRows());
    assertEquals(1, result.counts().changed()); // only key 2; key 1's first row matched
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("a composite key with an absent column still joins correctly")
  void absentKeyColumn(Fast fast, @TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k1,k2,v\n1,,x\n1,b,y\n,b,z\n");
    Path b = write(dir, "b.csv", "k1,k2,v\n1,,X\n1,b,y\n,b,Z\n");
    agrees(a, b, Options.builder().key(List.of("k1", "k2")), fast);
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("the truncated flag survives a section larger than --max-rows")
  void truncationIsReported(Fast fast, @TempDir Path dir) throws IOException {
    var a = new StringBuilder("k,v\n");
    var b = new StringBuilder("k,v\n");
    for (int i = 0; i < 40; i++) {
      a.append(i).append(",x\n");
      b.append(i).append(",y\n");
    }
    Path pa = write(dir, "a.csv", a.toString());
    Path pb = write(dir, "b.csv", b.toString());
    var builder = Options.builder().key(List.of("k")).maxRows(10);
    agrees(pa, pb, builder, fast);

    var result = CsvDiff.compare(pa, pb, builder.engine(fast.engine().label()).build());
    assertEquals(40, result.counts().changed(), "the count stays exact when the section is capped");
    assertEquals(10, result.changed().rows().size());
    assertTrue(result.changed().truncated());
  }

  @ParameterizedTest
  @EnumSource(Fast.class)
  @DisplayName("--export-dir writes the uncapped rows")
  void exportIsUncapped(Fast fast, @TempDir Path dir) throws IOException {
    var a = new StringBuilder("k,v\n");
    var b = new StringBuilder("k,v\n");
    for (int i = 0; i < 30; i++) {
      a.append(i).append(",x\n");
      b.append(i).append(",y\n");
    }
    Path pa = write(dir, "a.csv", a.toString());
    Path pb = write(dir, "b.csv", b.toString());
    Path out = dir.resolve("export");

    var opt = Options.builder().key(List.of("k")).maxRows(5)
        .exportDir(out.toString()).engine(fast.engine().label()).build();
    var result = CsvDiff.compare(pa, pb, opt);
    assertEquals(5, result.changed().rows().size(), "the report is still capped");

    var lines = new ArrayList<>(Files.readAllLines(out.resolve("changed.csv")));
    assertEquals(31, lines.size(), "the export holds a header and every changed row");
    assertFalse(lines.get(1).isBlank());
  }
}
