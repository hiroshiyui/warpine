/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * mem_protect_test.c - page states of allocated memory (ADR-0114)
 *
 * DosAllocMem reserves pages, committed with PAG_COMMIT; DosSetMem commits,
 * decommits and changes their access; DosQueryMem reports them (Control
 * Program Programming Reference). Checks:
 *   - a reserved page, a read-only page written, and a decommitted page
 *     raise XCPT_ACCESS_VIOLATION; committed pages start zeroed;
 *   - DosQueryMem's flags and sizes, PAG_BASE on the first page;
 *   - a guard page raises XCPT_GUARD_PAGE_VIOLATION once, with the access
 *     and address, and the program goes on; with no handler it goes on too;
 *     PAG_GUARD on DosAllocMem makes every page a guard page;
 *   - DosSetMem's and DosAllocMem's errors;
 *   - a page made read-only by one thread faults on another thread that
 *     wrote it just before (its TLB must forget the old entry);
 *   - the program's code object (no write flag) can be read, not written.
 * A probe that faults is left by longjmp from the exception handler.
 *
 * Output: "mem_protect_test: <name>: ok" / "...: FAIL", then
 * "mem_protect_test: done <passed>/<total>". Only DosWrite.
 */
#define INCL_DOS
#define INCL_DOSEXCEPTIONS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <setjmp.h>

#define PAGE     4096UL
#define RW       (PAG_READ | PAG_WRITE)
#define THREAD_STACK 0x8000UL

static int passed = 0, total = 0;

static void say(const char *s)
{
    ULONG n;
    DosWrite(1, (PVOID)s, strlen(s), &n);
}

static void check(const char *what, int good)
{
    total++;
    if (good) passed++;
    say("mem_protect_test: "); say(what); say(good ? ": ok\r\n" : ": FAIL\r\n");
}

/* ---------- probes: an access violation left by longjmp ---------- */

static jmp_buf       probe_env;
static volatile ULONG probe_code, probe_sink;

static ULONG APIENTRY probe_handler(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                    PCONTEXTRECORD ctx, PVOID unused)
{
    (void)reg; (void)ctx; (void)unused;
    if (rep->fHandlerFlags & (EH_UNWINDING | EH_EXIT_UNWIND)) {
        return XCPT_CONTINUE_SEARCH;
    }
    if (rep->ExceptionNum == XCPT_ACCESS_VIOLATION) {
        probe_code = rep->ExceptionNum;
        longjmp(probe_env, 1);
    }
    return XCPT_CONTINUE_SEARCH;
}

static void read_at(PVOID p)  { probe_sink = *(volatile ULONG *)p; }
static void write_at(PVOID p) { *(volatile ULONG *)p = 0x12345678UL; }

/* Whether the probe at p raises an access violation. */
static int faults(void (*probe)(PVOID), PVOID p)
{
    EXCEPTIONREGISTRATIONRECORD rec;

    memset(&rec, 0, sizeof(rec));
    rec.ExceptionHandler = probe_handler;
    probe_code = 0;
    DosSetExceptionHandler(&rec);
    if (setjmp(probe_env) == 0) {
        probe(p);
    }
    DosUnsetExceptionHandler(&rec);
    return probe_code == XCPT_ACCESS_VIOLATION;
}

static int query_is(PVOID p, ULONG ask, ULONG want_cb, ULONG want_flags)
{
    ULONG cb = ask, fl = 0;
    return DosQueryMem(p, &cb, &fl) == NO_ERROR && cb == want_cb && fl == want_flags;
}

/* ---------- guard pages ---------- */

static volatile ULONG guard_hits, guard_access, guard_addr;

static ULONG APIENTRY guard_handler(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                    PCONTEXTRECORD ctx, PVOID unused)
{
    (void)reg; (void)ctx; (void)unused;
    if (rep->ExceptionNum == XCPT_GUARD_PAGE_VIOLATION) {
        guard_hits++;
        guard_access = rep->ExceptionInfo[0];
        guard_addr = rep->ExceptionInfo[1];
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

/* ---------- another thread's TLB ---------- */

static HEV          wrote, protected_now;
static PULONG       shared_page;
static volatile int second_faulted = -1;

static void APIENTRY second(ULONG arg)
{
    ULONG posts;
    (void)arg;
    shared_page[0] = 1;                    /* its TLB holds a writable entry */
    DosPostEventSem(wrote);
    DosWaitEventSem(protected_now, SEM_INDEFINITE_WAIT);
    DosResetEventSem(protected_now, &posts);
    second_faulted = faults(write_at, shared_page);
}

int main(void)
{
    PBYTE  p;
    volatile BYTE *g;                      /* guard-page stores stay apart */
    PVOID  q;
    TID    tid;
    EXCEPTIONREGISTRATIONRECORD rec;
    ULONG  i, zero;

    /* Reserved, then committed. */
    check("DosAllocMem reserves 3 pages", DosAllocMem((PPVOID)&p, 3 * PAGE, RW) == NO_ERROR);
    check("DosQueryMem: reserved, the base", query_is(p, 3 * PAGE, 3 * PAGE, RW | PAG_BASE));
    check("a reserved page cannot be read", faults(read_at, p));
    check("nor written", faults(write_at, p + PAGE));
    check("DosSetMem commits a page", DosSetMem(p + PAGE, PAGE, PAG_COMMIT | PAG_DEFAULT) == NO_ERROR);
    check("DosQueryMem: committed, not the base", query_is(p + PAGE, 3 * PAGE, PAGE, RW | PAG_COMMIT));
    zero = 1;
    for (i = 0; i < PAGE; i++) zero &= p[PAGE + i] == 0;
    check("a committed page starts zeroed", zero);
    check("a committed page can be written", !faults(write_at, p + PAGE));
    check("its neighbours stay reserved", faults(read_at, p) && faults(read_at, p + 2 * PAGE));

    /* Read-only, after a write: this thread's TLB must forget the entry. */
    check("DosSetMem makes it read-only", DosSetMem(p + PAGE, PAGE, PAG_READ) == NO_ERROR);
    check("DosQueryMem: read-only", query_is(p + PAGE, PAGE, PAGE, PAG_READ | PAG_COMMIT));
    check("a read-only page cannot be written", faults(write_at, p + PAGE));
    check("but can be read", !faults(read_at, p + PAGE) && probe_sink == 0x12345678UL);
    check("DosSetMem makes it writable again", DosSetMem(p + PAGE, PAGE, RW) == NO_ERROR && !faults(write_at, p + PAGE));

    /* Decommitted. */
    check("DosSetMem decommits it", DosSetMem(p + PAGE, PAGE, PAG_DECOMMIT) == NO_ERROR);
    check("a decommitted page cannot be read", faults(read_at, p + PAGE));
    check("recommitted, it is zero again", DosSetMem(p + PAGE, PAGE, PAG_COMMIT | RW) == NO_ERROR
          && *(PULONG)(p + PAGE) == 0);

    /* Errors. */
    check("committing a committed page: ERROR_ACCESS_DENIED", DosSetMem(p + PAGE, PAGE, PAG_COMMIT | RW) == ERROR_ACCESS_DENIED);
    check("decommitting a reserved page: ERROR_ACCESS_DENIED", DosSetMem(p, PAGE, PAG_DECOMMIT) == ERROR_ACCESS_DENIED);
    check("past the allocation: ERROR_INVALID_ADDRESS", DosSetMem(p + 2 * PAGE, 2 * PAGE, PAG_COMMIT | RW) == ERROR_INVALID_ADDRESS);
    check("COMMIT and DECOMMIT together: ERROR_INVALID_PARAMETER", DosSetMem(p, PAGE, PAG_COMMIT | PAG_DECOMMIT) == ERROR_INVALID_PARAMETER);
    check("DosAllocMem without access: ERROR_INVALID_PARAMETER", DosAllocMem(&q, PAGE, PAG_COMMIT) == ERROR_INVALID_PARAMETER);
    check("DosAllocMem of 0 bytes: ERROR_INVALID_PARAMETER", DosAllocMem(&q, 0, PAG_COMMIT | RW) == ERROR_INVALID_PARAMETER);
    check("DosFreeMem", DosFreeMem(p) == NO_ERROR);

    /* Guard pages. */
    check("DosAllocMem with a guard page", DosAllocMem((PPVOID)&p, 2 * PAGE, PAG_COMMIT | RW | PAG_GUARD) == NO_ERROR);
    check("DosQueryMem: the guard", query_is(p, PAGE, PAGE, RW | PAG_COMMIT | PAG_GUARD | PAG_BASE));
    memset(&rec, 0, sizeof(rec));
    rec.ExceptionHandler = guard_handler;
    DosSetExceptionHandler(&rec);
    g = p;
    g[8] = 0x5A;
    check("touching it raises XCPT_GUARD_PAGE_VIOLATION once", guard_hits == 1);
    check("with the access and the address", guard_access == XCPT_WRITE_ACCESS && guard_addr == (ULONG)(p + 8));
    check("the write went through", g[8] == 0x5A);
    g[9] = 0x5B;
    check("the guard is gone from that page only", guard_hits == 1
          && query_is(p, 2 * PAGE, PAGE, RW | PAG_COMMIT | PAG_BASE)
          && query_is(p + PAGE, PAGE, PAGE, RW | PAG_COMMIT | PAG_GUARD));
    DosUnsetExceptionHandler(&rec);
    g[PAGE] = 0x77;
    check("with no handler, the program goes on", g[PAGE] == 0x77 && query_is(p + PAGE, PAGE, PAGE, RW | PAG_COMMIT));
    check("DosSetMem sets a guard again", DosSetMem(p, PAGE, RW | PAG_GUARD) == NO_ERROR
          && query_is(p, PAGE, PAGE, RW | PAG_COMMIT | PAG_GUARD | PAG_BASE));
    DosFreeMem(p);

    /* The program's own code. */
    check("the program's code can be read", !faults(read_at, (PVOID)check));
    check("but not written", faults(write_at, (PVOID)check));

    /* Another thread's TLB. */
    DosCreateEventSem(NULL, &wrote, 0, FALSE);
    DosCreateEventSem(NULL, &protected_now, 0, FALSE);
    DosAllocMem((PPVOID)&shared_page, PAGE, PAG_COMMIT | RW);
    if (DosCreateThread(&tid, (PFNTHREAD)second, 0, 0, THREAD_STACK) == NO_ERROR) {
        ULONG posts;
        DosWaitEventSem(wrote, SEM_INDEFINITE_WAIT);
        DosResetEventSem(wrote, &posts);
        DosSetMem(shared_page, PAGE, PAG_READ);
        DosPostEventSem(protected_now);
        DosWaitThread(&tid, DCWW_WAIT);
        check("read-only on another thread that wrote it", second_faulted == 1);
    } else {
        check("DosCreateThread", 0);
    }

    say("mem_protect_test: done ");
    {
        char buf[24];
        int  n = 0, t = total, k;
        char tmp[12];
        for (k = 0; k < 2; k++) {
            int v = k ? t : passed, j = 0;
            do { tmp[j++] = (char)('0' + v % 10); v /= 10; } while (v);
            while (j) buf[n++] = tmp[--j];
            if (!k) buf[n++] = '/';
        }
        buf[n++] = '\r'; buf[n++] = '\n'; buf[n] = '\0';
        say(buf);
    }
    return passed == total ? 0 : 1;
}
