package dev.csvdiff.engine.fast;

import dev.csvdiff.CsvDiffException;
import dev.csvdiff.Options;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;

/**
 * Builds a {@link RowIndex} with every core parsing at once.
 *
 * <p>Rows are independent, so parsing is embarrassingly parallel: the file is cut into one chunk
 * per thread at row boundaries, and each thread scans its own chunk, splitting keys and hashing
 * them. What is <em>not</em> parallel is the key table, because first-occurrence-wins depends on
 * row order — a shard cannot know whether its first row for a key is the file's first. So the
 * shards are merged in file order afterwards, and the merge only inserts pre-computed hashes, which
 * is a fraction of the work the parse was.
 *
 * <p>Threads are platform threads, not virtual ones: this is CPU-bound work with no blocking, which
 * is exactly the case a virtual thread does nothing for.
 */
final class ShardedBuilder {

  private ShardedBuilder() {}

  /** One chunk's rows, in file order. */
  private record Shard(long[] starts, long[] hashes, int count) {}

  static void build(
      RowIndex index, Slab slab, Options opt, byte delimiter,
      int[] columns, int width, long dataStart) {

    int threads =
        opt.threads() != null ? opt.threads() : Runtime.getRuntime().availableProcessors();
    long end = slab.size();
    long span = end - dataStart;
    // Below a few megabytes the split costs more than it saves.
    if (threads <= 1 || span < (4L << 20)) {
      index.build(dataStart);
      return;
    }

    long[] bounds = boundaries(slab, dataStart, end, delimiter, threads);
    int chunks = bounds.length - 1;

    List<Shard> shards;
    try (ExecutorService pool = Executors.newFixedThreadPool(chunks, runner())) {
      var futures = new ArrayList<Future<Shard>>(chunks);
      for (int i = 0; i < chunks; i++) {
        long from = bounds[i];
        long to = bounds[i + 1];
        futures.add(pool.submit(() -> scan(slab, opt, delimiter, columns, width, from, to, end)));
      }
      shards = new ArrayList<>(chunks);
      for (Future<Shard> future : futures) {
        shards.add(future.get());
      }
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new CsvDiffException("Interrupted while indexing", e);
    } catch (ExecutionException e) {
      Throwable cause = e.getCause();
      if (cause instanceof RuntimeException runtime) {
        throw runtime;
      }
      throw new CsvDiffException("Indexing failed: " + cause, cause);
    }

    index.prepareMerge();
    for (Shard shard : shards) {
      index.appendShard(shard.starts(), shard.hashes(), shard.count());
    }
    index.finishMerge();
  }

  private static java.util.concurrent.ThreadFactory runner() {
    return Thread.ofPlatform().name("csvdiff-shard-", 0).daemon(true).factory();
  }

  /**
   * Chunk boundaries, each moved forward to the start of a row.
   *
   * <p>A row belongs to the chunk it starts in, so a chunk may read past its own end to finish the
   * row it began. That is why the boundaries only have to be row starts, not row ends.
   */
  private static long[] boundaries(Slab slab, long from, long end, byte delimiter, int chunks) {
    var bounds = new ArrayList<Long>(chunks + 1);
    bounds.add(from);
    long step = (end - from) / chunks;
    for (int i = 1; i < chunks; i++) {
      long at = Scanner.nextRowStart(slab.main(), from + i * step, end, delimiter);
      if (at > bounds.getLast() && at < end) {
        bounds.add(at);
      }
    }
    bounds.add(end);
    var out = new long[bounds.size()];
    for (int i = 0; i < out.length; i++) {
      out[i] = bounds.get(i);
    }
    return out;
  }

  /** One thread's pass: split the key columns of every row that starts in the chunk, and hash them. */
  private static Shard scan(
      Slab slab, Options opt, byte delimiter, int[] columns, int width,
      long from, long to, long end) {

    var parser = new RowParser(slab, delimiter, columns);
    long[] fields = new long[width];
    int keySize = opt.key().size();
    int estimate = (int) Math.max(16, (to - from) / 96);
    var starts = new long[estimate];
    var hashes = new long[estimate];
    int n = 0;

    var seg = slab.main();
    long pos = from;
    while (pos < to) {
      byte b = seg.get(java.lang.foreign.ValueLayout.JAVA_BYTE, pos);
      if (b == '\n') {
        pos++;
        continue;
      }
      if (b == '\r'
          && pos + 1 < end
          && seg.get(java.lang.foreign.ValueLayout.JAVA_BYTE, pos + 1) == '\n') {
        pos += 2;
        continue;
      }
      long next = parser.parseRow(pos, end, fields);
      if (n == starts.length) {
        starts = java.util.Arrays.copyOf(starts, n * 2);
        hashes = java.util.Arrays.copyOf(hashes, n * 2);
      }
      long h = Bytes.seed();
      for (int i = 0; i < keySize; i++) {
        h = Bytes.hash(slab, fields[i], opt, h);
      }
      starts[n] = pos;
      hashes[n] = h;
      n++;
      if (next <= pos) {
        break;
      }
      pos = next;
    }
    return new Shard(starts, hashes, n);
  }
}
