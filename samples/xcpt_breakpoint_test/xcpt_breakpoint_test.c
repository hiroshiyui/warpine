/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * xcpt_breakpoint_test.c - a breakpoint instruction in a program
 *
 * A program that executes INT3 (0xCC) or INT 3 (0xCD 0x03) itself, outside
 * any debugger, gets XCPT_BREAKPOINT at its exception handlers (Control
 * Program Programming Reference, exception handling). Checks:
 *   - INT3 raises XCPT_BREAKPOINT, continuable;
 *   - the context's EIP is just past the instruction (the processor's trap
 *     frame), and ExceptionAddress is the same, so a handler that continues
 *     goes on after it;
 *   - the two-byte INT 3 does the same, its EIP past both bytes;
 *   - registers the handler leaves alone survive;
 *   - with no handler, INT3 ends the process as an unhandled exception: the
 *     exit-list routine runs with TC_EXCEPTION.
 *
 * Output: "xcpt_breakpoint_test: <name>: ok" / "...: FAIL", then
 * "xcpt_breakpoint_test: done <passed>/<total>", then
 * "xcpt_breakpoint_test: INT3 with no handler", and from the exit-list
 * routine "xcpt_breakpoint_test: exit list tc=<n>". Only DosWrite.
 */
#define INCL_DOS
#define INCL_DOSEXCEPTIONS
#include <os2.h>
#include <string.h>

#define INT3_OPCODE    0xCC
#define INT_N_OPCODE   0xCD
#define BREAKPOINT_VEC 0x03
#define KEPT_VALUE     0x5EED1234UL

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
    say("xcpt_breakpoint_test: "); say(what); say(good ? ": ok\r\n" : ": FAIL\r\n");
}

static void say_num(ULONG v)
{
    char buf[12];
    int  i = 11;
    buf[i] = '\0';
    do { buf[--i] = (char)('0' + v % 10); v /= 10; } while (v && i);
    say(buf + i);
}

#ifdef __WATCOMC__
static ULONG int3_short(ULONG keep);
#pragma aux int3_short = 0xCC parm [esi] value [esi];
static ULONG int3_long(ULONG keep);
#pragma aux int3_long = 0xCD 0x03 parm [esi] value [esi];
#else
static ULONG int3_short(ULONG keep)
{
    __asm__ volatile(".byte 0xCC" : "+S"(keep) : : "memory");
    return keep;
}
static ULONG int3_long(ULONG keep)
{
    __asm__ volatile(".byte 0xCD, 0x03" : "+S"(keep) : : "memory");
    return keep;
}
#endif

static volatile ULONG seen_code, seen_flags, seen_eip, seen_addr, hits;

static ULONG APIENTRY handler(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                              PCONTEXTRECORD ctx, PVOID unused)
{
    (void)reg; (void)unused;
    if (rep->ExceptionNum == XCPT_BREAKPOINT) {
        hits++;
        seen_code = rep->ExceptionNum;
        seen_flags = rep->fHandlerFlags;
        seen_eip = ctx->ctx_RegEip;
        seen_addr = (ULONG)rep->ExceptionAddress;
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static VOID APIENTRY routine(ULONG tc)
{
    say("xcpt_breakpoint_test: exit list tc="); say_num(tc); say("\r\n");
    DosExitList(EXLST_EXIT, 0);
}

int main(void)
{
    EXCEPTIONREGISTRATIONRECORD rec;
    ULONG kept;
    const UCHAR *at;

    DosExitList(EXLST_ADD, (PFNEXITLIST)routine);
    memset(&rec, 0, sizeof(rec));
    rec.ExceptionHandler = handler;
    DosSetExceptionHandler(&rec);

    kept = int3_short(KEPT_VALUE);
    at = (const UCHAR *)seen_eip;
    check("INT3 raises XCPT_BREAKPOINT", hits == 1 && seen_code == XCPT_BREAKPOINT);
    check("it is continuable", (seen_flags & EH_NONCONTINUABLE) == 0);
    check("the context's EIP is past the INT3", seen_eip != 0 && at[-1] == INT3_OPCODE);
    check("ExceptionAddress is the context's EIP", seen_addr == seen_eip);
    check("the program goes on with its registers", kept == KEPT_VALUE);

    seen_eip = 0;
    kept = int3_long(KEPT_VALUE);
    at = (const UCHAR *)seen_eip;
    check("INT 3 (CD 03) raises XCPT_BREAKPOINT", hits == 2 && seen_code == XCPT_BREAKPOINT);
    check("its EIP is past both bytes", seen_eip != 0 && at[-2] == INT_N_OPCODE && at[-1] == BREAKPOINT_VEC);
    check("the program goes on after it", kept == KEPT_VALUE);

    DosUnsetExceptionHandler(&rec);
    say("xcpt_breakpoint_test: done "); say_num(passed); say("/"); say_num(total); say("\r\n");
    say("xcpt_breakpoint_test: INT3 with no handler\r\n");
    int3_short(0);
    say("xcpt_breakpoint_test: went on after an unhandled INT3: FAIL\r\n");
    return 0;
}
