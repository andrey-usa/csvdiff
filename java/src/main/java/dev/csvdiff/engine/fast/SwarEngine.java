package dev.csvdiff.engine.fast;

/**
 * The file mapped with the FFM API, scanned with SWAR, on one thread.
 *
 * <p>Against {@code mmap} this isolates exactly one thing: real SIMD replaced by the bit trick that
 * stands in for it. Everything else — the mapping, the index, the join — is identical, so the
 * difference in the results table is the difference between the two techniques and nothing else.
 *
 * <p>It also needs no incubator module, which makes it the fastest engine here that runs on a
 * stock {@code java -jar} with no flags.
 */
public final class SwarEngine extends FastEngine {

  public SwarEngine() {
    super(Source.MAPPED, SwarScan::new, false);
  }
}
