package dev.csvdiff.engine.fast;

/**
 * The file on the heap, scanned with the Vector API.
 *
 * <p>Against {@code native} this isolates two things: vectorised scanning instead of a byte-at-a-
 * time reader, and a join over byte ranges instead of over Strings. Nothing else differs — same
 * hash join, same single thread.
 *
 * <p>It inherits the heap's limits. One Java array holds 2 GB, so a file past that is refused with
 * a message pointing at {@code mmap}; and the index plus the bytes have to fit in {@code -Xmx},
 * which on a default JVM is a quarter of the machine.
 */
public final class SimdEngine extends FastEngine {

  public SimdEngine() {
    super(Source.HEAP, false);
  }
}
