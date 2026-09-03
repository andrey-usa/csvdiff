package dev.csvdiff;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.json.JsonMapper;
import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.Base64;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.zip.GZIPInputStream;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/** The report must be one file with no external references, and its payload must round-trip. */
class ReportRendererTest {

  private static final Path EXAMPLES = Path.of("..", "examples");

  private static Contract.CompareResult sample() {
    return CsvDiff.compare(
        EXAMPLES.resolve("orders_2026-08.csv"),
        EXAMPLES.resolve("orders_2026-09.csv"),
        Options.builder().key(List.of("order_id", "line_no")).ignore(List.of("updated_at")).build());
  }

  @Test
  @DisplayName("nothing in the report points outside the document")
  void selfContained() {
    String html = ReportRenderer.render(sample());
    Matcher m = Pattern.compile("(?:src|href)=\"(?!#)([^\"]+)\"").matcher(html);
    assertFalse(m.find(), () -> "report references an external resource: " + m.group(1));
    assertTrue(html.length() < 8_000_000, "report unexpectedly large: " + html.length());
    assertTrue(html.startsWith("<!doctype html>"));
  }

  @Test
  @DisplayName("the gzip payload decodes back to the result")
  void payloadRoundTrips() throws Exception {
    var result = sample();
    String html = ReportRenderer.render(result);

    Matcher m =
        Pattern.compile("<script id=\"payload\" type=\"application/gzip\">([^<]+)</script>").matcher(html);
    assertTrue(m.find(), "the report should carry a gzip payload");

    byte[] gz = Base64.getDecoder().decode(m.group(1).strip());
    String json;
    try (var in = new GZIPInputStream(new ByteArrayInputStream(gz))) {
      json = new String(in.readAllBytes(), StandardCharsets.UTF_8);
    }
    JsonNode decoded = JsonMapper.builder().build().readTree(json);

    assertEquals(result.counts().changed(), decoded.get("counts").get("changed").asLong());
    assertEquals(result.counts().added(), decoded.get("counts").get("added").asLong());
    assertEquals(result.counts().changed(), decoded.get("changed").get("rows").size());
    // Changed rows are sparse: key values, then [columnIndex, a, b] triples.
    JsonNode firstRow = decoded.get("changed").get("rows").get(0);
    JsonNode cells = firstRow.get(firstRow.size() - 1);
    assertTrue(cells.isArray() && cells.get(0).isArray() && cells.get(0).size() == 3, cells.toString());
  }

  @Test
  @DisplayName("--no-compress embeds plain JSON and cannot close the script early")
  void plainPayload() {
    String html = ReportRenderer.render(sample(), false);
    assertTrue(html.contains("type=\"application/json\""));
    assertFalse(html.contains("</script>x"));
  }

  @Test
  @DisplayName("the title is HTML-escaped")
  void titleEscaped() {
    assertEquals("&lt;b&gt; &amp; &quot;q&quot;", ReportRenderer.escapeHtml("<b> & \"q\""));
  }
}
