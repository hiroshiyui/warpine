/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * ring_test.c - the processor state a 32-bit program sees (ADR-0113)
 *
 * Warpine is moving its 32-bit guests from ring 0 to ring 3, as OS/2 runs
 * applications. These checks hold at either ring:
 *   - the task register names a TSS in the GDT (every 32-bit thread has
 *     its own, with its ring-0 stack);
 *   - the program's segments are the flat ones Warpine gives it (code
 *     0x08, data 0x10, the TIB at 0x18), at the RPL of the current ring;
 *   - FS still reaches the TIB, whose tib_ptib2 points at its TIB2;
 *   - all of this holds on a second thread too.
 * At ring 3, where every program runs (before ADR-0113's step 5,
 * WARPINE_RING=0 could run them at ring 0, where these do not hold):
 *   - CLI, which only ring 0 may execute, raises XCPT_PRIVILEGED_INSTRUCTION
 *     at its own address, and the program goes on past it when its handler
 *     continues;
 *   - a handler that sets IOPL 3 in the context record does not get it:
 *     the program's EFLAGS still have IOPL 0;
 *   - Warpine's own pages cannot be read: the GDT, the ring-0 window (the
 *     thread's TSS and ring-0 stack) and the paging arena (every thread's
 *     page tables) raise XCPT_ACCESS_VIOLATION.
 * At either ring: the 4 MB of the API thunks cannot be written (read-only,
 * and CR0.WP holds ring 0 to it too).
 * A probe that faults is left by longjmp from the exception handler.
 * The ring itself is printed as information ("ring_test info: running at ring
 * <n>"), not checked.
 *
 * Output: "ring_test: <name>: ok" / "ring_test: <name>: FAIL", then
 * "ring_test: done <passed>/<total>". Only DosWrite, no C library output.
 */
#define INCL_DOS
#define INCL_DOSEXCEPTIONS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <setjmp.h>

#define CODE_SEL 0x08
#define DATA_SEL 0x10
#define TIB_SEL  0x18
#define RPL_MASK 3
#define TI_LDT   4
/* Warpine's guest memory layout (ADR-0094, ADR-0113). */
#define GDT_ADDR      0x01800000UL
#define RING0_WINDOW  0x01841000UL
#define PAGING_ARENA  0x01842000UL
#define THUNK_CHUNK   0x01200000UL  /* in the API thunks' 4 MB, past the thunks */
#define API_THUNKS    0x01000100UL  /* a thunk */
#define PROC_TABLES   0x01200000UL  /* the process's page tables (ADR-0114) */
#define PDPT_AREA     0x0DC00000UL  /* the threads' PDPTs (ADR-0131) */
#define CLI_LEN  1
#define IOPL_MASK 0x3000UL

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
    say("ring_test: "); say(what); say(good ? ": ok\r\n" : ": FAIL\r\n");
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
static ULONG read_tr(void);
#pragma aux read_tr = "xor eax, eax" "str ax" value [eax];
static ULONG read_cs(void);
#pragma aux read_cs = "xor eax, eax" "mov ax, cs" value [eax];
static ULONG read_ds(void);
#pragma aux read_ds = "xor eax, eax" "mov ax, ds" value [eax];
static ULONG read_ss(void);
#pragma aux read_ss = "xor eax, eax" "mov ax, ss" value [eax];
static ULONG read_fs(void);
#pragma aux read_fs = "xor eax, eax" "mov ax, fs" value [eax];
static ULONG read_fs_word(ULONG off);
#pragma aux read_fs_word = "mov eax, fs:[eax]" parm [eax] value [eax];
static void do_cli(void);
#pragma aux do_cli = "cli";
static ULONG read_eflags(void);
#pragma aux read_eflags = "pushfd" "pop eax" value [eax];
#else
static inline ULONG read_tr(void) { ULONG v = 0; __asm__ volatile("str %w0" : "+r"(v)); return v; }
static inline ULONG read_cs(void) { ULONG v = 0; __asm__ volatile("mov %%cs, %w0" : "+r"(v)); return v; }
static inline ULONG read_ds(void) { ULONG v = 0; __asm__ volatile("mov %%ds, %w0" : "+r"(v)); return v; }
static inline ULONG read_ss(void) { ULONG v = 0; __asm__ volatile("mov %%ss, %w0" : "+r"(v)); return v; }
static inline ULONG read_fs(void) { ULONG v = 0; __asm__ volatile("mov %%fs, %w0" : "+r"(v)); return v; }
static inline ULONG read_fs_word(ULONG off) { ULONG v; __asm__ volatile("movl %%fs:(%1), %0" : "=r"(v) : "r"(off)); return v; }
static inline void do_cli(void) { __asm__ volatile("cli" ::: "memory"); }
static inline ULONG read_eflags(void) { ULONG v; __asm__ volatile("pushfl; popl %0" : "=r"(v)); return v; }
#endif

static volatile ULONG seen_code, seen_eip;

/* Continue past a privileged instruction, asking for IOPL 3 on the way. */
static ULONG APIENTRY handler(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                              PCONTEXTRECORD ctx, PVOID unused)
{
    (void)reg; (void)unused;
    if (rep->ExceptionNum == XCPT_PRIVILEGED_INSTRUCTION && seen_code == 0) {
        seen_code = rep->ExceptionNum;
        seen_eip = ctx->ctx_RegEip;
        ctx->ctx_RegEip += CLI_LEN;
        ctx->ctx_EFlags |= IOPL_MASK;
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static jmp_buf          probe_env;
static volatile ULONG    probe_code;

/* Leave an access violation by longjmp back into faults(). */
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

static volatile ULONG    probe_sink;

/* The value goes to a volatile global: Open Watcom drops a volatile read
 * cast to void. */
static void read_at(ULONG addr)  { probe_sink = *(volatile ULONG *)addr; }
static void write_at(ULONG addr) { *(volatile UCHAR *)addr = 0; }

/* Whether reading (or writing) the byte at addr raises an access violation. */
static int faults(void (*probe)(ULONG), ULONG addr)
{
    EXCEPTIONREGISTRATIONRECORD rec;

    memset(&rec, 0, sizeof(rec));
    rec.ExceptionHandler = probe_handler;
    probe_code = 0;
    DosSetExceptionHandler(&rec);
    if (setjmp(probe_env) == 0) {
        probe(addr);
    }
    DosUnsetExceptionHandler(&rec);
    return probe_code == XCPT_ACCESS_VIOLATION;
}

static void check_protected(int ring3)
{
    if (ring3) {
        check("the GDT cannot be read", faults(read_at, GDT_ADDR));
        check("the ring-0 window cannot be read", faults(read_at, RING0_WINDOW));
        check("the paging arena cannot be read", faults(read_at, PAGING_ARENA));
    }
    check("the API thunks' 4 MB cannot be written", faults(write_at, THUNK_CHUNK));
    if (ring3) {
        check("the process tables cannot be read", faults(read_at, PROC_TABLES));
        check("the PDPTs cannot be read", faults(read_at, PDPT_AREA));
    }
}

/* No API call writes Warpine's memory for a program (ADR-0113): given an
 * output pointer there, DosQueryMem fails and writes nothing. */
static void check_api_writes(void)
{
    static ULONG here;
    ULONG cb, flags;
    ULONG targets[3] = { API_THUNKS, PROC_TABLES, PDPT_AREA };
    int   i, refused = 1;

    for (i = 0; i < 3; i++) {
        if (DosQueryMem(&here, (PULONG)targets[i], &flags) != ERROR_INVALID_PARAMETER) refused = 0;
    }
    check("DosQueryMem refuses output pointers into the thunks, tables and PDPTs", refused);
    cb = sizeof(here);
    check("the thunks still work", DosQueryMem(&here, &cb, &flags) == NO_ERROR && cb != 0);
}

static void check_privileged(void)
{
    EXCEPTIONREGISTRATIONRECORD rec;

    memset(&rec, 0, sizeof(rec));
    rec.ExceptionHandler = handler;
    DosSetExceptionHandler(&rec);
    do_cli();
    DosUnsetExceptionHandler(&rec);
    check("CLI raises XCPT_PRIVILEGED_INSTRUCTION", seen_code == XCPT_PRIVILEGED_INSTRUCTION);
    check("the exception is at the instruction", seen_eip != 0);
    check("a context record cannot raise IOPL", (read_eflags() & IOPL_MASK) == 0);
}

/* The TIB's tib_ptib2 (offset 0x0C) and its own address in the TIB2's
 * neighbourhood: DosGetInfoBlocks gives the same TIB. */
#define TIB_PTIB2 0x0C

static void check_state(const char *where)
{
    char  what[80];
    ULONG cs = read_cs(), ring = cs & RPL_MASK, tr = read_tr();
    PTIB  ptib;
    PPIB  ppib;

    DosGetInfoBlocks(&ptib, &ppib);
#define CHECK(name, cond) (strcpy(what, where), strcat(what, name), check(what, (cond)))
    CHECK("the task register names a GDT TSS", tr != 0 && (tr & TI_LDT) == 0);
    CHECK("code is the flat code selector", (cs & ~RPL_MASK) == CODE_SEL);
    CHECK("data and stack are the flat data selector at the same ring",
          read_ds() == (DATA_SEL | ring) && read_ss() == (DATA_SEL | ring));
    CHECK("FS is the TIB selector", (read_fs() & ~RPL_MASK) == TIB_SEL);
    CHECK("FS reaches the thread's TIB", read_fs_word(TIB_PTIB2) == (ULONG)ptib->tib_ptib2);
#undef CHECK
    say("ring_test info: "); say(where); say("running at ring "); say_num(ring); say("\r\n");
}

static void APIENTRY second(ULONG arg)
{
    (void)arg;
    check_state("thread 2: ");
}

int main(void)
{
    TID tid;

    check_state("");
    if ((read_cs() & RPL_MASK) == 3) {
        check_privileged();
    }
    check_protected((read_cs() & RPL_MASK) == 3);
    check_api_writes();
    if (DosCreateThread(&tid, (PFNTHREAD)second, 0, 0, 65536) == 0) {
        DosWaitThread(&tid, DCWW_WAIT);
    } else {
        check("DosCreateThread", 0);
    }
    say("ring_test: done "); say_num(passed); say("/"); say_num(total); say("\r\n");
    return passed == total ? 0 : 1;
}
