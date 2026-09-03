package dev.csvdiff;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.csvdiff.bench.GenData;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * The generator is part of CI, so its drift recipe is asserted like any other behaviour. Changing a
 * rate here means updating this test deliberately, not to make it pass.
 */
class GenDataTest {

  private static final List<String> KEY = List.of("account_id", "txn_id");

  @Test
  @DisplayName("row counts and drift rates match the documented recipe")
  void shapeAndDrift(@TempDir Path dir) throws Exception {
    Path a = dir.resolve("a.csv");
    Path b = dir.resolve("b.csv");
    GenData.generate(10_000, a, b, 7);

    var result =
        CsvDiff.compare(a, b, Options.builder().key(KEY).ignore(List.of("updated_at")).build());
    var c = result.counts();

    assertEquals(17, result.meta().compared().size(), "20 columns - 2 key - 1 ignored");
    assertEquals(10_000, c.aKeys());
    assertEquals(10_000, c.bKeys());
    assertEquals(10, c.added(), "0.1% of rows are only in B");
    assertEquals(10, c.removed(), "0.1% of rows are only in A");
    assertEquals(1, c.aDupKeys());
    assertEquals(1, c.bDupKeys());

    double ratio = (double) c.changed() / c.matched();
    assertTrue(ratio > 0.055 && ratio < 0.065, "about 6% of matched rows differ, got " + ratio);

    Map<String, Contract.ColumnStat> by =
        result.columns().stream().collect(Collectors.toMap(Contract.ColumnStat::name, s -> s));
    assertTrue(by.get("status").changed() > by.get("amount").changed());
    assertTrue(by.get("amount").changed() > 0);
    assertEquals(by.get("value_date").changed(), by.get("value_date").blanked());
    assertTrue(by.get("value_date").changed() > 0);
    assertEquals(0, by.get("note").changed(), "untouched columns stay untouched");
  }

  @Test
  @DisplayName("the same seed produces byte-identical files")
  void deterministic(@TempDir Path dir) throws Exception {
    Path a1 = dir.resolve("a1.csv");
    Path b1 = dir.resolve("b1.csv");
    Path a2 = dir.resolve("a2.csv");
    Path b2 = dir.resolve("b2.csv");
    GenData.generate(2_000, a1, b1, 7);
    GenData.generate(2_000, a2, b2, 7);

    assertArrayEquals(Files.readAllBytes(a1), Files.readAllBytes(a2));
    assertArrayEquals(Files.readAllBytes(b1), Files.readAllBytes(b2));
  }

  @Test
  @DisplayName("row-count shorthands parse")
  void parseRows() {
    assertEquals(10_000, GenData.parseRows("10k"));
    assertEquals(1_000_000, GenData.parseRows("1m"));
    assertEquals(2_500_000, GenData.parseRows("2.5M"));
    assertEquals(1_000, GenData.parseRows("1,000"));
    assertEquals(42, GenData.parseRows("42"));
    assertEquals(20, GenData.COLUMNS.size());
  }
}
