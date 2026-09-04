package dev.csvdiff.engine.fast;

/**
 * The file mapped into the address space with the Foreign Function and Memory API, scanned with the
 * Vector API.
 *
 * <p>Against {@code simd} this isolates the cost of getting the bytes into the heap: the read
 * itself, the 2 GB array ceiling, and the garbage collector's view of several gigabytes of live
 * data it can never move usefully. Here the bytes are the operating system's page cache, faulted in
 * as the scan reaches them, and the heap carries only the index.
 *
 * <p>That makes it the only engine here besides DuckDB that is not bounded by RAM.
 */
public final class MmapEngine extends FastEngine {

  public MmapEngine() {
    super(Source.MAPPED, false);
  }
}
