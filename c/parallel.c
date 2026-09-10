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

/* ------------------------------------------------------------------------- */
/* The memory budget                                                          */
/* ------------------------------------------------------------------------- */

/*
 * One process, one comparison, so a file-scope total is the whole mechanism.
 * It is written before threads start and read after they finish -- every
 * `budget_take` here happens on the calling thread, at a phase boundary, which
 * is why it needs no lock.
 */
static size_t budget_cap = 0;
static size_t budget_so_far = 0;
static int    budget_over = 0;

void budget_set(size_t mb) {
    budget_cap = mb ? mb * (size_t)1024 * 1024 : 0;
    budget_so_far = 0;
    budget_over = 0;
}

size_t budget_limit(void) { return budget_cap; }
size_t budget_used(void)  { return budget_so_far; }
int    budget_exceeded(void) { return budget_over; }

int budget_take(size_t bytes) {
    if (budget_cap == 0) return 0;
    /* An overflowing sum is over any ceiling, so saturate rather than wrap. */
    if (bytes > budget_cap - budget_so_far) { budget_over = 1; return -1; }
    budget_so_far += bytes;
    return 0;
}
