#define _GNU_SOURCE

#include "parallel.h"

#include <pthread.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

typedef struct {
    void   (*fn)(void *ctx, unsigned part);
    void    *ctx;
    unsigned part;
} Job;

static void *job_entry(void *p) {
    Job *j = p;
    j->fn(j->ctx, j->part);
    return NULL;
}

void run_parts(void (*fn)(void *, unsigned), void *ctx, unsigned ways) {
    if (ways <= 1) { fn(ctx, 0); return; }
    pthread_t *tid = calloc(ways, sizeof *tid);
    Job       *job = calloc(ways, sizeof *job);
    if (!tid || !job) {                       /* no memory for threads: do it inline */
        for (unsigned p = 0; p < ways; p++) fn(ctx, p);
        free(tid);
        free(job);
        return;
    }
    for (unsigned p = 1; p < ways; p++) {
        job[p].fn = fn;
        job[p].ctx = ctx;
        job[p].part = p;
        if (pthread_create(&tid[p], NULL, job_entry, &job[p]) != 0) {
            fn(ctx, p);                       /* no thread to be had: the same work, here */
            tid[p] = 0;
        }
    }
    fn(ctx, 0);
    for (unsigned p = 1; p < ways; p++)
        if (tid[p]) pthread_join(tid[p], NULL);
    free(tid);
    free(job);
}

void *alloc_huge(size_t bytes) {
    const size_t huge = (size_t)2 << 20;
    const size_t rounded = (bytes + huge - 1) & ~(huge - 1);
    void *p = NULL;
    if (posix_memalign(&p, huge, rounded) != 0) return malloc(bytes);
#ifdef MADV_HUGEPAGE
    madvise(p, rounded, MADV_HUGEPAGE);
#endif
    return p;
}

unsigned cpu_count(void) {
    const long n = sysconf(_SC_NPROCESSORS_ONLN);
    return n > 0 ? (unsigned)n : 1u;
}
