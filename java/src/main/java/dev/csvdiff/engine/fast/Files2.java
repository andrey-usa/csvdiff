package dev.csvdiff.engine.fast;

import dev.csvdiff.CsvDiffException;
import dev.csvdiff.Options;
import java.io.IOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.nio.channels.FileChannel;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;

/** How the fast engines get at a file's bytes. */
public final class Files2 {

  private Files2() {}

  /** The largest file that fits in a single Java array, and so the {@code simd} engine's ceiling. */
  public static final long MAX_HEAP_SLAB = Integer.MAX_VALUE - 8L;

  /**
   * Reads the whole file onto the heap.
   *
   * <p>Simple, and it keeps the bytes where the JIT expects them, but it is bounded twice over: by
   * the maximum length of a Java array, and by the heap the JVM was given. Both ceilings show up in
   * the benchmark, which is the point of having this engine next to {@link #map}.
   */
  public static Slab load(Path path, Options opt) throws IOException {
    long size = Files.size(path);
    if (size > MAX_HEAP_SLAB) {
      throw new CsvDiffException(
          "%s is %.1f GB, past the 2 GB that one Java array can hold. Use --engine mmap, which reads it in place."
              .formatted(path, size / 1e9));
    }
    byte[] bytes = Files.readAllBytes(path);
    // A heap array presented as a MemorySegment, so one scanner serves both engines.
    return new Slab(MemorySegment.ofArray(bytes), Arena.ofConfined());
  }

  /**
   * Maps the file into the address space, so its bytes never enter the heap at all.
   *
   * <p>The pages are the operating system's page cache, faulted in as the scan reaches them and
   * evicted under pressure, so a file larger than RAM is read without ever holding all of it. The
   * heap then only carries the index — about 150 bytes a row against the row itself.
   *
   * <p>The arena is shared rather than confined because the sharded engine hands the segment to
   * several threads; a shared arena is closed explicitly and the mapping lives exactly as long as
   * the comparison.
   */
  public static Slab map(Path path, Options opt) throws IOException {
    var arena = Arena.ofShared();
    try (FileChannel channel = FileChannel.open(path, StandardOpenOption.READ)) {
      long size = channel.size();
      MemorySegment mapped = channel.map(FileChannel.MapMode.READ_ONLY, 0, size, arena);
      return new Slab(mapped, arena);
    } catch (IOException | RuntimeException e) {
      arena.close();
      throw e;
    }
  }

  /** The delimiter to use: the one asked for, or the one the header suggests. */
  public static byte delimiter(Slab slab, Options opt) {
    if (opt.delimiter() != null) {
      char c = opt.delimiter();
      if (c > 0x7F) {
        throw new CsvDiffException(
            "This engine needs a single-byte delimiter; " + c + " is not one. Use --engine native.");
      }
      return (byte) c;
    }
    MemorySegment seg = slab.main();
    long end = Math.min(seg.byteSize(), Scanner.nextOf1(seg, 0, seg.byteSize(), (byte) '\n'));
    return Csv2.detect(seg, end);
  }
}
