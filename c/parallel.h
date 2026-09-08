/*
 * The two things every parallel phase in this port needs, in one place.
 *
 * Both were written for the columnar Parquet path first and are wanted verbatim
 * by the CSV one, which is the usual sign that they belong beside both rather
 * than inside either.
 */
#ifndef CSVDIFF_PARALLEL_H
#define CSVDIFF_PARALLEL_H

#include <stddef.h>

/*
 * One shape for every parallel phase: part 0 runs on the calling thread and the
 * rest get one each. A thread that will not spawn is not a failure -- its part
 * runs inline, slower and still right -- so this returns nothing and every
 * caller reports its own errors through its own context.
 */
void run_parts(void (*fn)(void *ctx, unsigned part), void *ctx, unsigned ways);

/*
 * 2 MB-aligned, and asked for on huge pages. Uninitialised: the caller fills it,
 * because one table here wants zeroes and another wants -1.
 *
 * Worth it only for an allocation that is rare and long-lived and then probed at
 * random. A hash table over ten million keys is 128 MB, which on 4 KB pages is
 * 32,768 pages against a TLB holding perhaps 1,500, so nearly every probe takes
 * a page walk on top of its cache miss. On 2 MB pages the same table is 64
 * entries. It is *not* worth it for a buffer allocated in a loop: with
 * `transparent_hugepage` set to `madvise` each request makes the kernel compact
 * memory to find a 2 MB run, and that cost scales with the number of asks. This
 * was measured three ways and the rule is in c/README.md.
 *
 * Falls back to plain malloc where the alignment cannot be had, so a caller
 * never has to care.
 */
void *alloc_huge(size_t bytes);

/* How many parts to split into when nobody said. At least 1. */
unsigned cpu_count(void);

#endif
