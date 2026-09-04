package dev.csvdiff.engine.fast;

import java.lang.foreign.MemorySegment;
import java.nio.ByteOrder;
import jdk.incubator.vector.ByteVector;
import jdk.incubator.vector.VectorMask;
import jdk.incubator.vector.VectorOperators;
import jdk.incubator.vector.VectorSpecies;

/**
 * Real SIMD, through the Vector API: one register-wide compare per 32 or 64 bytes.
 *
 * <p>{@link ByteVector} compares a whole register against a broadcast byte and hands back a mask
 * whose lowest set lane is the next hit. On long runs with no delimiter — a wide free-text column,
 * say — nothing beats it. On the short fields a keyed extract is made of, {@link SwarScan} often
 * wins because it has nothing to set up.
 *
 * <p>The Vector API is still an incubator module, so this is the only class that touches it and
 * {@link #available()} says whether it loaded.
 */
public final class VectorScan implements Scan {

  private static final VectorSpecies<Byte> SPECIES = ByteVector.SPECIES_PREFERRED;
  private static final int LANES = SPECIES.length();
  private static final ByteOrder ORDER = ByteOrder.nativeOrder();

  /** How wide the vectors are here, for the run metadata. */
  public static int lanes() {
    return LANES;
  }

  @Override
  public String technique() {
    return "vector-" + LANES;
  }

  @Override
  public boolean available() {
    try {
      return SPECIES.length() > 0;
    } catch (LinkageError e) {
      return false;
    }
  }

  @Override
  public long nextOf2(MemorySegment seg, long from, long end, byte a, byte b) {
    long pos = from;
    ByteVector va = ByteVector.broadcast(SPECIES, a);
    ByteVector vb = ByteVector.broadcast(SPECIES, b);

    while (pos + LANES <= end) {
      ByteVector v = ByteVector.fromMemorySegment(SPECIES, seg, pos, ORDER);
      VectorMask<Byte> hit =
          v.compare(VectorOperators.EQ, va).or(v.compare(VectorOperators.EQ, vb));
      if (hit.anyTrue()) {
        return pos + hit.firstTrue();
      }
      pos += LANES;
    }
    for (; pos < end; pos++) {
      byte x = seg.get(BYTE, pos);
      if (x == a || x == b) {
        return pos;
      }
    }
    return end;
  }

  @Override
  public long nextOf1(MemorySegment seg, long from, long end, byte a) {
    long pos = from;
    ByteVector va = ByteVector.broadcast(SPECIES, a);

    while (pos + LANES <= end) {
      ByteVector v = ByteVector.fromMemorySegment(SPECIES, seg, pos, ORDER);
      VectorMask<Byte> hit = v.compare(VectorOperators.EQ, va);
      if (hit.anyTrue()) {
        return pos + hit.firstTrue();
      }
      pos += LANES;
    }
    for (; pos < end; pos++) {
      if (seg.get(BYTE, pos) == a) {
        return pos;
      }
    }
    return end;
  }
}
