package dev.csvdiff.engine.fast;

/**
 * {@code mmap}, with the index built by every core at once.
 *
 * <p>Against {@code mmap} this isolates the parallel speed-up. Parsing is where a comparison of
 * this shape spends most of its time and it is embarrassingly parallel — rows are independent — so
 * the file is cut into one chunk per core at row boundaries, each thread indexes its own chunk, and
 * the shards are then merged in file order so the result is identical to the single-threaded one.
 *
 * <p>The merge is what keeps it honest: first-occurrence-wins depends on row order, so the shards
 * cannot simply be unioned.
 */
public final class ShardEngine extends FastEngine {

  public ShardEngine() {
    super(Source.MAPPED, true);
  }
}
