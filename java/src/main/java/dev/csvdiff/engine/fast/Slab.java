package dev.csvdiff.engine.fast;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;

/**
 * Where one file's bytes live, and the small side-buffer for the fields that could not be used
 * where they lay.
 *
 * <p>{@code main} is the file itself: a heap array for the {@code simd} engine, a mapped region for
 * {@code mmap} and {@code shard}. Almost every field is a slice of it, which is the point — the
 * bytes are read where they already are rather than copied into a String.
 *
 * <p>{@code escapes} holds the exceptions. A quoted field containing a doubled quote
 * ({@code "a""b"}) has a value that is not any contiguous slice of the file, so it is unescaped
 * once at parse time and stored here. Those fields are rare, so the buffer stays small and the hot
 * path keeps a perfectly-predicted branch rather than an escape-aware reader.
 */
public final class Slab implements AutoCloseable {

  private static final ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;

  private final MemorySegment main;
  private final Arena arena;

  // Written from every shard thread while the sharded engine parses, and read afterwards.
  private volatile MemorySegment escapes = MemorySegment.NULL;
  private long escapeUsed;

  public Slab(MemorySegment main, Arena arena) {
    this.main = main;
    this.arena = arena;
  }

  public MemorySegment main() {
    return main;
  }

  public long size() {
    return main.byteSize();
  }

  /** The segment a packed field points into. */
  public MemorySegment segmentOf(long field) {
    return Bytes.isEscaped(field) ? escapes : main;
  }

  /**
   * Copies an unescaped field into the side-buffer and returns it packed.
   *
   * <p>The buffer grows by doubling; a file with no doubled quotes never allocates it at all.
   */
  public synchronized long intern(byte[] value, int length) {
    if (escapeUsed + length > escapes.byteSize()) {
      long want = Math.max(1 << 16, Math.max(escapes.byteSize() * 2, escapeUsed + length));
      MemorySegment grown = arena.allocate(want);
      if (escapeUsed > 0) {
        MemorySegment.copy(escapes, 0, grown, 0, escapeUsed);
      }
      escapes = grown;
    }
    MemorySegment.copy(value, 0, escapes, BYTE, escapeUsed, length);
    long field = Bytes.escaped(Bytes.pack(escapeUsed, length));
    escapeUsed += length;
    return field;
  }

  @Override
  public void close() {
    arena.close();
  }
}
