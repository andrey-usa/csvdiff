package dev.csvdiff.engine.fast;

import dev.csvdiff.Options;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.charset.Charset;
import java.nio.charset.StandardCharsets;
import java.util.Locale;

/**
 * Field-level operations done on the raw bytes, without building a {@link String}.
 *
 * <p>This is where the fast engines earn their name. The in-heap engines allocate a String per cell
 * — twenty columns times ten million rows is two hundred million objects that are almost all thrown
 * away, since only the differing cells reach the report. Here a field is an offset and a length
 * into a {@link Slab}, and hashing, equality and ordering all read the bytes in place.
 *
 * <p>The results are exact rather than approximately-the-same-as the other engines. Where a byte
 * comparison could disagree with the String one — non-ASCII text under {@code --ignore-case} — the
 * field is decoded and the String path is taken. Bulk extracts are ASCII, so that is the safety net
 * rather than the common case.
 */
public final class Bytes {

  private Bytes() {}

  private static final ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;
  /** Words are read little-endian so a field's first byte is always the low byte. */
  private static final ValueLayout.OfLong LONG =
      ValueLayout.JAVA_LONG_UNALIGNED.withOrder(java.nio.ByteOrder.LITTLE_ENDIAN);
  private static final long FNV_PRIME = 0x0000_0100_0000_01B3L;
  private static final long FNV_SEED = 0xCBF2_9CE4_8422_2325L;
  /** The 64-bit mixing constant from splitmix64, used to stir a whole word at once. */
  private static final long MIX = 0x9E37_79B9_7F4A_7C15L;

  /** A field that is not there at all: a short row, or a column missing from the file. */
  public static final long ABSENT = -1L;

  private static final long OFFSET_MASK = 0xFF_FFFF_FFFFL; // 40 bits: 1 TB
  private static final int LENGTH_SHIFT = 40;
  private static final long LENGTH_MASK = 0x7F_FFFFL; // 23 bits: 8 MB
  private static final long ESCAPED = 1L << 63;

  /**
   * A field packed into one long: offset in the low 40 bits, length in the next 23, and a flag in
   * the top bit for the rare field that lives in the slab's side-buffer.
   *
   * <p>One long per field means a row's field table is a {@code long[]} rather than an object per
   * field, which at ten million rows is the difference between an array and a garbage-collection
   * problem.
   */
  public static long pack(long offset, int length) {
    return (offset & OFFSET_MASK) | ((length & LENGTH_MASK) << LENGTH_SHIFT);
  }

  public static long escaped(long field) {
    return field | ESCAPED;
  }

  public static boolean isEscaped(long field) {
    return (field & ESCAPED) != 0;
  }

  public static long offset(long field) {
    return field & OFFSET_MASK;
  }

  public static int length(long field) {
    return (int) ((field >>> LENGTH_SHIFT) & LENGTH_MASK);
  }

  /** The largest field this packing can address. */
  public static final int MAX_FIELD_LENGTH = (int) LENGTH_MASK;

  /**
   * Whether a field counts as absent under the options.
   *
   * <p>An empty field is absent whether or not it was quoted — what DuckDB's reader does, and what
   * every implementation of this tool follows. With {@code --trim} a field of only whitespace is
   * empty too, which also satisfies {@code --empty-is-null}.
   */
  public static boolean isNull(Slab slab, long field, Options opt) {
    if (field == ABSENT) {
      return true;
    }
    if (length(field) == 0) {
      return true;
    }
    return opt.trim() && normLength(slab, field, opt) == 0;
  }

  /** What {@link String#trim()} strips: anything at or below U+0020. */
  private static boolean isSpace(byte b) {
    return b >= 0 && b <= ' ';
  }

  /** ASCII lower-casing, used only where the field is known to be ASCII. */
  private static byte lower(byte b) {
    return (b >= 'A' && b <= 'Z') ? (byte) (b + 32) : b;
  }

  /** The field's first byte after {@code --trim}. */
  public static long normStart(Slab slab, long field, Options opt) {
    long start = offset(field);
    if (!opt.trim()) {
      return start;
    }
    MemorySegment seg = slab.segmentOf(field);
    long end = start + length(field);
    while (start < end && isSpace(seg.get(BYTE, start))) {
      start++;
    }
    return start;
  }

  /** The field's length after {@code --trim}. */
  public static int normLength(Slab slab, long field, Options opt) {
    int len = length(field);
    if (!opt.trim() || len == 0) {
      return len;
    }
    MemorySegment seg = slab.segmentOf(field);
    long start = offset(field);
    long end = start + len;
    while (start < end && isSpace(seg.get(BYTE, start))) {
      start++;
    }
    while (end > start && isSpace(seg.get(BYTE, end - 1))) {
      end--;
    }
    return (int) (end - start);
  }

  private static boolean isAscii(MemorySegment seg, long start, int len) {
    for (int i = 0; i < len; i++) {
      if (seg.get(BYTE, start + i) < 0) {
        return false;
      }
    }
    return true;
  }

  /**
   * A 64-bit hash of the normalised field, for the key index.
   *
   * <p>FNV-1a: one xor and one multiply per byte, no table, and spread enough for composite keys
   * made of ids and dates. Collisions are resolved by comparing the bytes, so the hash only has to
   * be fast and reasonably uniform.
   */
  public static long hash(Slab slab, long field, Options opt, long seed) {
    if (isNull(slab, field, opt)) {
      // An absent value still has to contribute, or (null, "x") and ("x", null) would collide.
      return (seed ^ MIX) * FNV_PRIME;
    }
    MemorySegment seg = slab.segmentOf(field);
    long start = normStart(slab, field, opt);
    int len = normLength(slab, field, opt);

    long h = opt.ignoreCase() ? foldedHash(seg, start, len, seed) : wordHash(seg, start, len, seed);
    // Mix the length in so a field and its prefix cannot share a hash by accident.
    return (h ^ (long) len) * FNV_PRIME;
  }

  /**
   * Hashes eight bytes per step instead of one.
   *
   * <p>The One Billion Row Challenge entries all landed here: a byte-at-a-time hash is a dependent
   * chain of a load, an xor and a multiply per byte, and on a twenty-character key that is twenty
   * round trips through the multiplier. Reading a whole {@code long} makes it three, and the loads
   * are what the memory system wanted to do anyway.
   *
   * <p>The tail is masked rather than branched over: {@code (1 << 8n) - 1} keeps the bytes that
   * belong to the field and zeroes whatever the last word dragged in after it.
   *
   * <p>An eight-byte read can safely overrun the field but never the segment, so a field lying in
   * the last eight bytes of the file has its word assembled from single bytes instead. That
   * assembled word must be <em>the same word</em> the wide read would have produced, because the
   * hash decides whether two rows share a key: if the bytes {@code K2} hashed one way in the middle
   * of a file and another way at the end of it, a join would miss the last row of a file and report
   * it as removed from one side and added to the other. It did exactly that until this was fixed.
   */
  private static long wordHash(MemorySegment seg, long start, int len, long seed) {
    long h = seed;
    long limit = seg.byteSize() - Long.BYTES;
    int i = 0;
    while (i < len) {
      int rest = Math.min(Long.BYTES, len - i);
      long word = start + i <= limit ? seg.get(LONG, start + i) : assemble(seg, start + i, rest);
      if (rest < Long.BYTES) {
        word &= (1L << (rest << 3)) - 1;
      }
      h = (h ^ mix(word)) * FNV_PRIME;
      i += rest;
    }
    return h;
  }

  /** The little-endian word an eight-byte read would give, for bytes too near the segment's end. */
  private static long assemble(MemorySegment seg, long at, int count) {
    long word = 0;
    for (int j = 0; j < count; j++) {
      word |= (seg.get(BYTE, at + j) & 0xFFL) << (j << 3);
    }
    return word;
  }

  /** Case-insensitive hashing stays byte-at-a-time: folding a word needs the bytes apart anyway. */
  private static long foldedHash(MemorySegment seg, long start, int len, long seed) {
    long h = seed;
    for (int i = 0; i < len; i++) {
      h = (h ^ (lower(seg.get(BYTE, start + i)) & 0xFF)) * FNV_PRIME;
    }
    return h;
  }

  /** splitmix64's finaliser, enough to spread one word's bits across the whole hash. */
  private static long mix(long word) {
    long x = word * MIX;
    x ^= x >>> 29;
    return x;
  }

  /** The seed a composite-key hash starts from. */
  public static long seed() {
    return FNV_SEED;
  }

  /**
   * Whether two fields hold the same value under {@code --trim} and {@code --ignore-case}.
   *
   * <p>Absent equals absent, and absent differs from anything present. This is the key-equality
   * rule; {@link #differs} adds the numeric tolerance on top for values.
   */
  public static boolean equal(Slab ls, long lf, Slab rs, long rf, Options opt) {
    boolean ln = isNull(ls, lf, opt);
    boolean rn = isNull(rs, rf, opt);
    if (ln || rn) {
      return ln && rn;
    }
    MemorySegment lseg = ls.segmentOf(lf);
    MemorySegment rseg = rs.segmentOf(rf);
    long lo = normStart(ls, lf, opt);
    int ll = normLength(ls, lf, opt);
    long ro = normStart(rs, rf, opt);
    int rl = normLength(rs, rf, opt);

    if (!opt.ignoreCase()) {
      return ll == rl && sameBytes(lseg, lo, rseg, ro, ll);
    }
    if (isAscii(lseg, lo, ll) && isAscii(rseg, ro, rl)) {
      if (ll != rl) {
        return false;
      }
      for (int i = 0; i < ll; i++) {
        if (lower(lseg.get(BYTE, lo + i)) != lower(rseg.get(BYTE, ro + i))) {
          return false;
        }
      }
      return true;
    }
    // Case folding outside ASCII can change a string's length, so this is the one path that
    // has to agree with String rather than with bytes.
    return decode(lseg, lo, ll, opt)
        .toLowerCase(Locale.ROOT)
        .equals(decode(rseg, ro, rl, opt).toLowerCase(Locale.ROOT));
  }

  /**
   * SQL's {@code IS DISTINCT FROM}, with the numeric tolerance applied where both sides parse as
   * numbers.
   */
  public static boolean differs(Slab ls, long lf, Slab rs, long rf, Options opt) {
    boolean ln = isNull(ls, lf, opt);
    boolean rn = isNull(rs, rf, opt);
    if (ln && rn) {
      return false;
    }
    if (opt.tolerance() > 0 && !ln && !rn) {
      double a = number(ls, lf, opt);
      double b = number(rs, rf, opt);
      if (!Double.isNaN(a) && !Double.isNaN(b)) {
        return Math.abs(a - b) > opt.tolerance();
      }
    }
    return !equal(ls, lf, rs, rf, opt);
  }

  /**
   * The field as a number, or NaN when it is not one.
   *
   * <p>Deliberately stricter than {@link Double#parseDouble}: {@code inf}, {@code nan}, hex and a
   * trailing {@code d} are ordinary text in a CSV, and treating them as numbers would make two
   * unequal strings compare equal under a tolerance.
   */
  public static double number(Slab slab, long field, Options opt) {
    long start = normStart(slab, field, opt);
    int len = normLength(slab, field, opt);
    if (len == 0 || len > 64) {
      return Double.NaN;
    }
    MemorySegment seg = slab.segmentOf(field);
    var chars = new char[len];
    for (int i = 0; i < len; i++) {
      byte b = seg.get(BYTE, start + i);
      boolean ok =
          (b >= '0' && b <= '9') || b == '.' || b == '-' || b == '+' || b == 'e' || b == 'E';
      if (!ok) {
        return Double.NaN;
      }
      chars[i] = (char) b;
    }
    try {
      double v = Double.parseDouble(new String(chars));
      return Double.isFinite(v) ? v : Double.NaN;
    } catch (NumberFormatException e) {
      return Double.NaN;
    }
  }

  /**
   * Whether two runs of bytes are identical, eight at a time.
   *
   * <p>{@link MemorySegment#mismatch} does the same job and is intrinsified, but it is a general
   * routine that has to work out alignment and length class first. Keys here are a handful of bytes,
   * so the setup is most of the cost; comparing whole words inline is measurably quicker, and it is
   * what the One Billion Row Challenge entries do for exactly this reason.
   */
  private static boolean sameBytes(MemorySegment a, long ao, MemorySegment b, long bo, int len) {
    int i = 0;
    long aLimit = a.byteSize() - Long.BYTES;
    long bLimit = b.byteSize() - Long.BYTES;
    for (; i + Long.BYTES <= len && ao + i <= aLimit && bo + i <= bLimit; i += Long.BYTES) {
      if (a.get(LONG, ao + i) != b.get(LONG, bo + i)) {
        return false;
      }
    }
    for (; i < len; i++) {
      if (a.get(BYTE, ao + i) != b.get(BYTE, bo + i)) {
        return false;
      }
    }
    return true;
  }

  /** The field as a normalised String, or {@code null} when it is absent. */
  public static String value(Slab slab, long field, Options opt) {
    if (isNull(slab, field, opt)) {
      return null;
    }
    String s =
        decode(slab.segmentOf(field), normStart(slab, field, opt), normLength(slab, field, opt), opt);
    return opt.ignoreCase() ? s.toLowerCase(Locale.ROOT) : s;
  }

  /** The raw bytes as a String, with no normalisation. */
  public static String decode(MemorySegment seg, long start, int len, Options opt) {
    var buf = new byte[len];
    MemorySegment.copy(seg, BYTE, start, buf, 0, len);
    Charset cs = opt.charset();
    return new String(buf, cs == null ? StandardCharsets.UTF_8 : cs);
  }
}
