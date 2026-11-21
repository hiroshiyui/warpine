/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * stack_grow_test.c - sparse thread stacks (ADR-0114)
 *
 * DosCreateThread gives a thread a sparse stack unless STACK_COMMITTED is
 * asked for (bsedos.h): reserved, committed as it grows (Control Program
 * Programming Reference). Checks:
 *   - a sparse stack's bottom is reserved, its top committed;
 *   - deep recursion grows it, and DosQueryMem then shows more committed;
 *   - a frame larger than a page, touched at its lowest address first,
 *     works (Warpine commits any page of the stack that is touched);
 *   - a STACK_COMMITTED stack is committed whole;
 *   - recursion without end raises XCPT_UNABLE_TO_GROW_STACK, which the
 *     thread's handler leaves by longjmp; the thread then ends normally.
 * Growth needs ring 3, where every program runs (ADR-0113): a fault on
 * the stack is delivered on the ring-0 stack. At ring 0, which Warpine
 * no longer offers, only the checks that hold there would run.
 * Built with Open Watcom's -s (no stack checks: Warpine's TIB, not the
 * runtime, knows the stack's bounds).
 *
 * Output: "stack_grow_test: <name>: ok" / "...: FAIL", then
 * "stack_grow_test: done <passed>/<total>". Only DosWrite.
 */
#define INCL_DOS
#define INCL_DOSEXCEPTIONS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <setjmp.h>

#define RPL_MASK    3
#define PAGE        4096UL
#define STACK_SIZE  0x10000UL      /* 64 KB */
#define FRAME_BYTES 1024
#define DEEP        40             /* about 40 KB of frames */
#define BIG_FRAME   (4 * PAGE)

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
    say("stack_grow_test: "); say(what); say(good ? ": ok\r\n" : ": FAIL\r\n");
}

static ULONG flags_at(ULONG addr)
{
    ULONG cb = PAGE, fl = 0;
    return DosQueryMem((PVOID)addr, &cb, &fl) == NO_ERROR ? fl : 0;
}

static void stack_bounds(ULONG *low, ULONG *high)
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks(&ptib, &ppib);
    *low = (ULONG)ptib->tib_pstack;
    *high = (ULONG)ptib->tib_pstacklimit;
}

static volatile ULONG deepest;      /* the lowest frame's address */

/* Recursion that keeps each frame: FRAME_BYTES of locals per level. */
static int deep(int n)
{
    volatile char buf[FRAME_BYTES];
    buf[0] = (char)n;
    buf[FRAME_BYTES - 1] = (char)n;
    if (n == 0) {
        deepest = (ULONG)buf;
        return 0;
    }
    return deep(n - 1) + buf[0] - buf[FRAME_BYTES - 1];
}

/* A frame of several pages, touched at its lowest address first. */
static int big_frame(void)
{
    volatile char buf[BIG_FRAME];
    buf[0] = 1;
    buf[BIG_FRAME - 1] = 2;
    return buf[0] + buf[BIG_FRAME - 1];
}

/* ---------- the sparse thread ---------- */

static volatile int bottom_reserved, top_committed, recursed, grown, big_ok;

static void APIENTRY sparse(ULONG arg)
{
    ULONG low, high;
    (void)arg;
    stack_bounds(&low, &high);
    bottom_reserved = (flags_at(low) & PAG_COMMIT) == 0 && (flags_at(low) & PAG_BASE) != 0;
    top_committed = (flags_at(high - PAGE) & PAG_COMMIT) != 0;
    recursed = deep(DEEP) == 0;
    grown = high - deepest >= DEEP * FRAME_BYTES / 2 && (flags_at(deepest) & PAG_COMMIT) != 0;
    big_ok = big_frame() == 3;
}

/* ---------- the committed thread ---------- */

static volatile int committed_whole;

static void APIENTRY committed(ULONG arg)
{
    ULONG low, high;
    (void)arg;
    stack_bounds(&low, &high);
    committed_whole = (flags_at(low) & PAG_COMMIT) != 0;
}

/* ---------- the thread that recurses without end ---------- */

static jmp_buf          out;
static volatile ULONG   overflow_code, overflow_flags;
static volatile int     came_back, ended;

static ULONG APIENTRY overflow_handler(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                       PCONTEXTRECORD ctx, PVOID unused)
{
    (void)reg; (void)ctx; (void)unused;
    if (rep->fHandlerFlags & (EH_UNWINDING | EH_EXIT_UNWIND)) {
        return XCPT_CONTINUE_SEARCH;
    }
    if (rep->ExceptionNum == XCPT_UNABLE_TO_GROW_STACK) {
        overflow_code = rep->ExceptionNum;
        overflow_flags = rep->fHandlerFlags;
        longjmp(out, 1);
    }
    return XCPT_CONTINUE_SEARCH;
}

static int endless(int n)
{
    volatile char buf[FRAME_BYTES];
    buf[0] = (char)n;
    return endless(n + 1) + buf[0];
}

static void APIENTRY runaway(ULONG arg)
{
    EXCEPTIONREGISTRATIONRECORD rec;
    (void)arg;
    memset(&rec, 0, sizeof(rec));
    rec.ExceptionHandler = overflow_handler;
    DosSetExceptionHandler(&rec);
    if (setjmp(out) == 0) {
        endless(0);
    } else {
        came_back = 1;
    }
    DosUnsetExceptionHandler(&rec);
    ended = 1;
}

#ifdef __WATCOMC__
static ULONG read_cs(void);
#pragma aux read_cs = "xor eax, eax" "mov ax, cs" value [eax];
#else
static ULONG read_cs(void) { ULONG v = 0; __asm__ volatile("mov %%cs, %w0" : "+r"(v)); return v; }
#endif

static int run(PFNTHREAD fn, ULONG flags)
{
    TID tid;
    if (DosCreateThread(&tid, fn, 0, flags, STACK_SIZE) != NO_ERROR) return 0;
    return DosWaitThread(&tid, DCWW_WAIT) == NO_ERROR;
}

int main(void)
{
    char buf[24];
    int  n = 0, k;

    int ring3 = (read_cs() & RPL_MASK) == 3;

    check("a thread runs", run((PFNTHREAD)sparse, 0));
    if (ring3) {
        check("its stack's bottom is reserved, the base", bottom_reserved);
        check("its top is committed", top_committed);
    } else {
        say("stack_grow_test info: ring 0, stacks committed whole\r\n");
    }
    check("deep recursion works", recursed);
    check("and has committed the stack it used", grown);
    check("a frame larger than a page works", big_ok);
    check("a STACK_COMMITTED thread runs", run((PFNTHREAD)committed, STACK_COMMITTED));
    check("its stack is committed whole", committed_whole);
    if (ring3) {
        check("a thread recursing without end runs", run((PFNTHREAD)runaway, 0));
        check("it gets XCPT_UNABLE_TO_GROW_STACK", overflow_code == XCPT_UNABLE_TO_GROW_STACK);
        check("its handler leaves it by longjmp", came_back && ended);
    }

    say("stack_grow_test: done ");
    for (k = 0; k < 2; k++) {
        int v = k ? total : passed, j = 0;
        char tmp[12];
        do { tmp[j++] = (char)('0' + v % 10); v /= 10; } while (v);
        while (j) buf[n++] = tmp[--j];
        if (!k) buf[n++] = '/';
    }
    buf[n++] = '\r'; buf[n++] = '\n'; buf[n] = '\0';
    say(buf);
    return passed == total ? 0 : 1;
}
