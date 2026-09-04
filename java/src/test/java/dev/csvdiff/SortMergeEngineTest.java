package dev.csvdiff;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.csvdiff.Contract.CompareResult;
import dev.csvdiff.engine.sortmerge.Runs;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.Stream;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * The spill-and-merge half of the sort-merge engine.
 *
 * <p>{@link EngineParityTest} already runs this engine over the example files alongside every
 * other one, but those files fit in a single batch, so the interesting path — sorted runs written
 * to disk and merged back — never executes. Shrinking the batch budget forces it, and the answer
 * has to come out the same as the in-memory run and the same as every other engine.
 */
class SortMergeEngineTest {

  private static final Path EXAMPLES = Path.of("..", "examples");
  private static final Path A = EXAMPLES.resolve("orders_2026-08.csv");
  private static final Path B = EXAMPLES.resolve("orders_2026-09.csv");

  private static Options.Builder base() {
    return Options.builder().key(List.of("order_id", "line_no")).ignore(List.of("updated_at"));
  }

  @AfterEach
  void clearBudget() {
    System.clearProperty(Runs.BATCH_BYTES_PROPERTY);
  }

  /** Small enough that the example files sort in many batches rather than one. */
  private static void forceSpills() {
    System.setProperty(Runs.BATCH_BYTES_PROPERTY, "2048");
  }

  @Test
  @DisplayName("spilling to disk and merging back gives the same answer as sorting in memory")
  void spilledRunsMatchTheInMemorySort() {
    CompareResult inMemory = CsvDiff.compare(A, B, base().engine("sortmerge").build());
    forceSpills();
    CompareResult spilled = CsvDiff.compare(A, B, base().engine("sortmerge").build());

    assertEquals(inMemory.counts(), spilled.counts(), "counts");
    assertEquals(inMemory.columns(), spilled.columns(), "column stats");
    assertEquals(inMemory.changed(), spilled.changed(), "changed rows");
    assertEquals(inMemory.added(), spilled.added(), "added rows");
    assertEquals(inMemory.removed(), spilled.removed(), "removed rows");
    assertEquals(inMemory.dupA(), spilled.dupA(), "duplicate keys in A");
    assertEquals(inMemory.dupB(), spilled.dupB(), "duplicate keys in B");
  }

  @Test
  @DisplayName("a spilled run still reports the first row of a duplicated key, not an arbitrary one")
  void firstOccurrenceSurvivesTheSpill(@TempDir Path dir) throws IOException {
    // The same key three times, each with a different value, and the file ordered so that the row
    // the answer depends on is neither the smallest nor the largest by value. A sort that is not
    // stable, or a merge that does not break ties by run, picks one of the other two.
    Path a = write(dir, "a.csv", """
        k,v
        3,c
        1,first
        2,b
        1,second
        1,third
        """);
    Path b = write(dir, "b.csv", """
        k,v
        1,first
        2,b
        3,c
        """);

    forceSpills();
    var result = CsvDiff.compare(a, b, Options.builder().key(List.of("k")).engine("sortmerge").build());

    assertEquals(0, result.counts().changed(), "the first row for key 1 is unchanged");
    assertEquals(1, result.counts().aDupKeys());
    assertEquals(3, result.counts().aDupRows());
    assertEquals(List.of(List.<Object>of("1", 3L)), result.dupA().rows());
  }

  @Test
  @DisplayName("the engine leaves no spilled runs behind")
  void temporaryFilesAreCleanedUp() throws IOException {
    forceSpills();
    long before = workingDirectories();
    CsvDiff.compare(A, B, base().engine("sortmerge").build());
    assertEquals(before, workingDirectories(), "a comparison left its working directory behind");
  }

  @Test
  @DisplayName("a batch budget that is not a positive number of bytes is rejected by name")
  void batchBudgetIsValidated() {
    System.setProperty(Runs.BATCH_BYTES_PROPERTY, "plenty");
    var opt = base().engine("sortmerge").build();
    var thrown = org.junit.jupiter.api.Assertions.assertThrows(
        CsvDiffException.class, () -> CsvDiff.compare(A, B, opt));
    assertTrue(thrown.getMessage().contains(Runs.BATCH_BYTES_PROPERTY), thrown.getMessage());
  }

  @Test
  @DisplayName("the spilled format round-trips quotes, commas, newlines and absent fields")
  void awkwardFieldsSurviveTheSpillFormat(@TempDir Path dir) throws IOException {
    Path a = write(dir, "a.csv", "k,v,w\n1,\"a\"\"b\",\n2,\"has,comma\",x\n3,\"two\nlines\",y\n");
    Path b = write(dir, "b.csv", "k,v,w\n1,\"a\"\"b\",z\n2,\"has,comma\",x\n3,\"two\nlines\",y\n");

    var options = Options.builder().key(List.of("k"));
    CompareResult reference = CsvDiff.compare(a, b, options.engine("native").build());
    forceSpills();
    CompareResult spilled = CsvDiff.compare(a, b, options.engine("sortmerge").build());

    assertEquals(reference.counts(), spilled.counts());
    assertEquals(reference.changed(), spilled.changed());
    assertNotEquals(0, reference.counts().changed(), "the fixture is meant to find a difference");
  }

  private static Path write(Path dir, String name, String body) throws IOException {
    Path path = dir.resolve(name);
    Files.writeString(path, body, StandardCharsets.UTF_8);
    return path;
  }

  /** Working directories the engine may have left in the system temp directory. */
  private static long workingDirectories() throws IOException {
    Path tmp = Path.of(System.getProperty("java.io.tmpdir"));
    try (Stream<Path> entries = Files.list(tmp)) {
      return entries.filter(p -> p.getFileName().toString().startsWith("csvdiff-sortmerge-")).count();
    }
  }
}
