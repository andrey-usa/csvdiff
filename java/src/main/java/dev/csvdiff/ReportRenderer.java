package dev.csvdiff;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.json.JsonMapper;
import dev.csvdiff.Contract.CompareResult;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.util.Base64;
import java.util.zip.Deflater;
import java.util.zip.GZIPOutputStream;

/**
 * The self-contained HTML report: one file, no network, no fonts, no frameworks.
 *
 * <p>The template is the same one the Python and TypeScript implementations use, kept beside this
 * class as a resource. Only differing cells are embedded and the payload is gzip then base64, which
 * the browser decodes natively with {@code DecompressionStream}; {@code --no-compress} writes plain
 * JSON for anything older than about 2023.
 */
public final class ReportRenderer {

  private ReportRenderer() {}

  private static final String RESOURCE = "report.html";

  private static final ObjectMapper MAPPER = JsonMapper.builder().build();

  /** The template, read once. */
  private static final String TEMPLATE = loadTemplate();

  private static String loadTemplate() {
    try (InputStream in = ReportRenderer.class.getResourceAsStream(RESOURCE)) {
      if (in == null) {
        throw new IllegalStateException("Report template missing from the jar: " + RESOURCE);
      }
      return new String(in.readAllBytes(), StandardCharsets.UTF_8);
    } catch (IOException e) {
      throw new UncheckedIOException("Cannot read the report template", e);
    }
  }

  /** Renders a report with the payload gzipped. */
  public static String render(CompareResult result) {
    return render(result, true);
  }

  /**
   * Renders a report.
   *
   * @param compress gzip and base64 the payload; {@code false} embeds plain JSON
   */
  public static String render(CompareResult result, boolean compress) {
    String raw;
    try {
      raw = MAPPER.writeValueAsString(result);
    } catch (IOException e) {
      throw new UncheckedIOException("Cannot serialise the result", e);
    }

    String payload;
    String mode;
    if (compress) {
      payload = Base64.getEncoder().encodeToString(gzip(raw));
      mode = "gzip";
    } else {
      // The payload sits inside a <script> element, so a literal "</" would end it early.
      payload = raw.replace("</", "<\\/");
      mode = "json";
    }

    String title = result.meta().a().name() + " vs " + result.meta().b().name();
    // String.replace takes literals, so the payload needs no regex escaping.
    return TEMPLATE
        .replace("__TITLE__", escapeHtml(title))
        .replace("__MODE__", mode)
        .replace("__PAYLOAD__", payload);
  }

  /** GZIP at maximum compression, to match the other implementations byte for byte in size. */
  private static final class MaxGzipOutputStream extends GZIPOutputStream {
    MaxGzipOutputStream(ByteArrayOutputStream out) throws IOException {
      super(out);
      def.setLevel(Deflater.BEST_COMPRESSION);
    }
  }

  private static byte[] gzip(String s) {
    var out = new ByteArrayOutputStream();
    try (var gz = new MaxGzipOutputStream(out)) {
      gz.write(s.getBytes(StandardCharsets.UTF_8));
    } catch (IOException e) {
      throw new UncheckedIOException("Cannot compress the payload", e);
    }
    return out.toByteArray();
  }

  static String escapeHtml(String s) {
    return s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&#x27;");
  }
}
