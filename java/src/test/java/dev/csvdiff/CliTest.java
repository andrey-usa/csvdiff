package dev.csvdiff;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/** The CLI's exit codes are a public contract: pipelines gate on them. */
class CliTest {

  private static final Path EXAMPLES = Path.of("..", "examples");
  private static final String A = EXAMPLES.resolve("orders_2026-08.csv").toString();
  private static final String B = EXAMPLES.resolve("orders_2026-09.csv").toString();

  private record Run(int status, String out, String err) {}

  private static Run run(String... args) {
    var out = new ByteArrayOutputStream();
    var err = new ByteArrayOutputStream();
    int status;
    try (var o = new PrintStream(out, true, StandardCharsets.UTF_8);
        var e = new PrintStream(err, true, StandardCharsets.UTF_8)) {
      status = Cli.run(args, o, e);
    }
    return new Run(status, out.toString(StandardCharsets.UTF_8), err.toString(StandardCharsets.UTF_8));
  }

  @Test
  @DisplayName("identical files exit 0, differences exit 1")
  void exitCodes(@TempDir Path dir) {
    var same = run("compare", A, A, "-k", "order_id,line_no", "-o", dir.resolve("same.html").toString());
    assertEquals(0, same.status(), same.err());

    var diff =
        run("compare", A, B, "-k", "order_id,line_no", "-i", "updated_at",
            "-o", dir.resolve("diff.html").toString());
    assertEquals(1, diff.status(), diff.err());
    assertTrue(diff.out().contains("matched 787 (changed 102)"), diff.out());
  }

  @Test
  @DisplayName("a bad key exits 2 with a one-line message")
  void missingKey(@TempDir Path dir) {
    var r = run("compare", A, B, "-k", "missing", "-o", dir.resolve("r.html").toString());
    assertEquals(2, r.status());
    assertTrue(r.err().contains("Key column(s) missing"), r.err());
  }

  @Test
  @DisplayName("--fail-on-dups exits 3 when either file has duplicate keys")
  void failOnDups(@TempDir Path dir) {
    var r =
        run("compare", A, B, "-k", "order_id,line_no", "--fail-on-dups",
            "-o", dir.resolve("r.html").toString());
    assertEquals(3, r.status(), r.err());
  }

  @Test
  @DisplayName("--json writes the summary without the embedded rows")
  void jsonSummary(@TempDir Path dir) throws Exception {
    Path json = dir.resolve("s.json");
    var r =
        run("compare", A, B, "-k", "order_id,line_no", "-i", "updated_at",
            "-o", dir.resolve("r.html").toString(), "--json", json.toString());
    assertEquals(1, r.status(), r.err());

    String text = Files.readString(json);
    assertTrue(text.contains("\"changed\" : 102"), text.substring(0, Math.min(400, text.length())));
    assertTrue(text.contains("\"counts\""));
    assertTrue(text.contains("\"columns\""));
  }

  @Test
  @DisplayName("an unknown engine is rejected by name")
  void unknownEngine(@TempDir Path dir) {
    var r =
        run("compare", A, B, "-k", "order_id,line_no", "--engine", "nope",
            "-o", dir.resolve("r.html").toString());
    assertEquals(2, r.status());
    assertTrue(r.err().contains("Unknown engine: nope"), r.err());
  }

  @Test
  @DisplayName("help and no arguments both explain themselves")
  void usage() {
    var help = run("--help");
    assertEquals(0, help.status());
    assertTrue(help.out().contains("usage: csvdiff <command>"), help.out());
    assertTrue(help.out().contains("--engine E"), help.out());
    assertEquals(2, run().status(), "no arguments is a usage error");
  }
}
