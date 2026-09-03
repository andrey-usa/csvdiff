package dev.csvdiff;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import org.tomlj.Toml;
import org.tomlj.TomlArray;
import org.tomlj.TomlParseResult;
import org.tomlj.TomlTable;

/**
 * Named comparison profiles from {@code csvdiff.toml}, so a recurring comparison does not get
 * retyped. Searched in the working directory, then {@code ~/.config/csvdiff/}. The file format is
 * shared with the Python and TypeScript implementations:
 *
 * <pre>{@code
 * [profiles.orders]
 * key       = ["order_id", "line_no"]
 * compare   = ["qty", "price", "status"]   # omit for all common non-key columns
 * ignore    = ["updated_at"]
 * trim      = true
 * tolerance = 0.005
 * }</pre>
 */
public final class Profiles {

  private Profiles() {}

  /** Where a config file is looked for when no path is given. */
  public static List<Path> searchPath() {
    return List.of(
        Path.of("csvdiff.toml"),
        Path.of(System.getProperty("user.home"), ".config", "csvdiff", "csvdiff.toml"));
  }

  /** Loads the config, or an empty one when no file exists. */
  public static TomlParseResult load(String explicitPath) {
    List<Path> candidates = explicitPath != null ? List.of(Path.of(explicitPath)) : searchPath();
    for (Path p : candidates) {
      if (Files.isRegularFile(p)) {
        try {
          TomlParseResult toml = Toml.parse(p);
          if (toml.hasErrors()) {
            String first = toml.errors().getFirst().toString();
            throw new CsvDiffException("Cannot parse " + p + ": " + first);
          }
          return toml;
        } catch (IOException e) {
          throw new UncheckedIOException("Cannot read " + p, e);
        }
      }
    }
    return Toml.parse("");
  }

  /**
   * Looks up one profile.
   *
   * @throws CsvDiffException if the name was given but no such profile exists
   */
  public static Optional<TomlTable> profile(TomlParseResult config, String name) {
    if (name == null || name.isBlank()) {
      return Optional.empty();
    }
    TomlTable profiles = config.getTable("profiles");
    TomlTable found = profiles == null ? null : profiles.getTable(name);
    if (found == null) {
      throw new CsvDiffException("Profile not found: " + name);
    }
    return Optional.of(found);
  }

  /** Applies a profile's values as the base layer of an {@link Options.Builder}. */
  public static Options.Builder apply(Options.Builder builder, TomlTable p) {
    return builder
        .key(strings(p, "key"))
        .compare(strings(p, "compare"))
        .ignore(strings(p, "ignore"))
        .trim(p.getBoolean("trim"))
        .ignoreCase(p.getBoolean("ignore_case"))
        .emptyIsNull(p.getBoolean("empty_is_null"))
        .tolerance(p.getDouble("tolerance"))
        .maxRows(intOf(p, "max_rows"))
        .delimiter(charOf(p, "delimiter"))
        .encoding(p.getString("encoding"))
        .engine(p.getString("engine"));
  }

  private static List<String> strings(TomlTable t, String key) {
    TomlArray arr = t.getArray(key);
    if (arr == null) {
      return null;
    }
    var out = new ArrayList<String>(arr.size());
    for (int i = 0; i < arr.size(); i++) {
      out.add(String.valueOf(arr.get(i)));
    }
    return List.copyOf(out);
  }

  private static Integer intOf(TomlTable t, String key) {
    Long v = t.getLong(key);
    return v == null ? null : Math.toIntExact(v);
  }

  private static Character charOf(TomlTable t, String key) {
    String v = t.getString(key);
    return v == null || v.isEmpty() ? null : v.charAt(0);
  }

  /** Splits {@code "a, b ,c"} into a list; returns {@code null} for {@code null}. */
  public static List<String> parseList(String value) {
    if (value == null) {
      return null;
    }
    return java.util.Arrays.stream(value.split(","))
        .map(String::strip)
        .filter(s -> !s.isEmpty())
        .toList();
  }
}
