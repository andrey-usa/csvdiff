package dev.csvdiff.engine.fast;

/**
 * Everything at once: mapped with the FFM API, scanned with SWAR, indexed on every core.
 *
 * <p>This is the configuration the One Billion Row Challenge entries converged on, applied to a
 * comparison instead of an aggregation — memory-mapped input, word-at-a-time scanning, hashing and
 * key equality, an open-addressed table of primitives, and one chunk per core merged in file order.
 *
 * <p>Against {@code shard} it prices SWAR against the Vector API under parallelism; against
 * {@code swar} it prices the parallelism. And unlike {@code shard} it runs without
 * {@code --add-modules}, so it is the one to reach for by default.
 */
public final class TurboEngine extends FastEngine {

  public TurboEngine() {
    super(Source.MAPPED, SwarScan::new, true);
  }
}
