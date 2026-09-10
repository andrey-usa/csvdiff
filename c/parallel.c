#define _GNU_SOURCE

#include "parallel.h"

#include <stdlib.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <pthread.h>
#include <sys/mman.h>
#include <unistd.h>
#endif

typedef struct {
    void   (*fn)(void *ctx, unsigned part);
    void    *ctx;
    unsigned part;
} Job;

/*
 * A thread is the one thing this file needs that the two platforms spell
 * differently, so it is the one thing wrapped. Three operations, each the exact
 * shape `run_parts` below asks for -- a handle that compares false when there is
 * no thread, a start that reports failure rather than aborting, and a join that
 * also releases the handle Windows hands back.
 *
 * pthreads exists on Windows through mingw-w64's winpthreads, and this does not
 * use it: `pthread_t` is a struct there, so `tid[p] = 0` and `if (tid[p])` --
 * which is how `run_parts` records a thread it could not start -- would not
 * compile. Twenty lines of CreateThread cost less than reshaping the caller
 * around a portability library, and drop a DLL from the binary besides.
 */
#ifdef _WIN32

typedef HANDLE thread_t;

static DWORD WINAPI job_entry(LPVOID p) {
    Job *j = p;
    j->fn(j->ctx, j->part);
    return 0;
}

static int thread_start(thread_t *t, Job *j) {
    *t = CreateThread(NULL, 0, job_entry, j, 0, NULL);
    return *t ? 0 : -1;
}

static void thread_wait(thread_t t) {
    WaitForSingleObject(t, INFINITE);
    CloseHandle(t);
}

#else

typedef pthread_t thread_t;

static void *job_entry(void *p) {
    Job *j = p;
    j->fn(j->ctx, j->part);
    return NULL;
}

static int thread_start(thread_t *t, Job *j) {
    return pthread_create(t, NULL, job_entry, j);
}

static void thread_wait(thread_t t) {
    pthread_join(t, NULL);
}

#endif

void run_parts(void (*fn)(void *, unsigned), void *ctx, unsigned ways) {
    if (ways <= 1) { fn(ctx, 0); return; }
    thread_t *tid = calloc(ways, sizeof *tid);
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
        if (thread_start(&tid[p], &job[p]) != 0) {
            fn(ctx, p);                       /* no thread to be had: the same work, here */
            tid[p] = 0;
        }
    }
    fn(ctx, 0);
    for (unsigned p = 1; p < ways; p++)
        if (tid[p]) thread_wait(tid[p]);
    free(tid);
    free(job);
}

void *alloc_huge(size_t bytes) {
#ifdef _WIN32
    /*
     * Plain malloc on Windows, and the header already promises that. Large
     * pages there are not a hint a process can leave on a range: they need
     * SeLockMemoryPrivilege, which an ordinary account does not hold, and
     * VirtualAlloc with MEM_LARGE_PAGES, which returns memory `free` must not
     * be given. The 2 MB alignment on its own buys nothing without the pages
     * behind it, so this asks for none of it rather than paying for the shape
     * of an optimisation it cannot have.
     */
    return malloc(bytes);
#else
    const size_t huge = (size_t)2 << 20;
    const size_t rounded = (bytes + huge - 1) & ~(huge - 1);
    void *p = NULL;
    if (posix_memalign(&p, huge, rounded) != 0) return malloc(bytes);
#ifdef MADV_HUGEPAGE
    madvise(p, rounded, MADV_HUGEPAGE);
#endif
    return p;
#endif
}

unsigned cpu_count(void) {
#ifdef _WIN32
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    return si.dwNumberOfProcessors > 0 ? (unsigned)si.dwNumberOfProcessors : 1u;
#else
    const long n = sysconf(_SC_NPROCESSORS_ONLN);
    return n > 0 ? (unsigned)n : 1u;
#endif
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
