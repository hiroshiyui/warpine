/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * paging_abuse_test.c - a hostile guest against Warpine's paging (ADR-0088)
 *
 * Warpine's guests run at ring 0, so a program can rewrite its own page
 * tables or load CR3 / CR0 itself; on OS/2 neither is possible (the
 * instructions are privileged, the tables are not the program's). The
 * hardware (EPT / NPT) keeps the guest in its own memory, and Warpine ends
 * such a program as a trap: its exit list runs (TC_TRAP, with the tables
 * put back) and the process ends with exit code 1. Built five times:
 *
 *   pgabuse_zero.exe   zeroes its page directory: a triple fault at once
 *   pgabuse_cr3.exe    loads CR3 with an address past guest memory: the
 *                      vCPU spins inside KVM with no exit; the paging
 *                      watch's kick ends it
 *   pgabuse_remap.exe  points its thread-local page elsewhere, then spins
 *   pgabuse_off.exe    turns paging off, then spins
 *   pgabuse_past.exe   leaves paging alone and reads past guest memory in
 *                      a loop: each read exits to Warpine (and reads 0);
 *                      a run of them with nothing else ends it as a trap
 *
 * Output: "pgabuse: <name>: ok" / "pgabuse: <name>: FAIL" before the
 * attack, then the exit-list routine's "pgabuse: exit list tc=<n>". Only
 * DosWrite, no C library output.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define PAGE        4096UL
#define TLM_ENTRY   (0x9F000UL >> 12)   /* the thread-local page's table entry */

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
    say("pgabuse: "); say(what); say(good ? ": ok\r\n" : ": FAIL\r\n");
}

static void say_num(ULONG v)
{
    char buf[12];
    int  i = 11;
    buf[i] = '\0';
    do { buf[--i] = (char)('0' + v % 10); v /= 10; } while (v && i);
    say(buf + i);
}

static VOID APIENTRY routine(ULONG tc)
{
    say("pgabuse: exit list tc="); say_num(tc); say("\r\n");
    DosExitList(EXLST_EXIT, 0);
}

#ifdef __WATCOMC__
static ULONG read_cr3(void);
#pragma aux read_cr3 = "mov eax, cr3" value [eax];
static void load_cr3(ULONG v);
#pragma aux load_cr3 = "mov cr3, eax" parm [eax];
static ULONG read_cr0(void);
#pragma aux read_cr0 = "mov eax, cr0" value [eax];
static void load_cr0(ULONG v);
#pragma aux load_cr0 = "mov cr0, eax" parm [eax];
#else
static inline ULONG read_cr3(void) { ULONG v; __asm__ volatile("mov %%cr3, %0" : "=r"(v)); return v; }
static inline void load_cr3(ULONG v) { __asm__ volatile("mov %0, %%cr3" : : "r"(v) : "memory"); }
static inline ULONG read_cr0(void) { ULONG v; __asm__ volatile("mov %%cr0, %0" : "=r"(v)); return v; }
static inline void load_cr0(ULONG v) { __asm__ volatile("mov %0, %%cr0" : : "r"(v) : "memory"); }
#endif

int main(void)
{
    ULONG   cr3 = read_cr3();
    PULONG  tlm;

    check("runs paged", (read_cr0() & 0x80000000UL) != 0 && cr3 != 0);
    check("DosExitList", DosExitList(EXLST_ADD, (PFNEXITLIST)routine) == NO_ERROR);
    check("DosAllocThreadLocalMemory", DosAllocThreadLocalMemory(2, &tlm) == NO_ERROR);
#if defined(ZERO)
    say("pgabuse: zeroing the page directory\r\n");
    memset((void *)cr3, 0, PAGE);
    load_cr3(cr3);
#elif defined(CR3)
    say("pgabuse: loading CR3 past guest memory\r\n");
    load_cr3(0xF0000000UL);
#elif defined(REMAP)
    say("pgabuse: pointing the thread-local page elsewhere\r\n");
    ((PULONG)(cr3 + PAGE))[TLM_ENTRY] = 0x00100000UL | 7;
    load_cr3(cr3);
#elif defined(PAGING_OFF)
    say("pgabuse: turning paging off\r\n");
    load_cr0(read_cr0() & ~0x80000000UL);
#elif defined(PAST)
    say("pgabuse: reading past guest memory\r\n");
    for (;;) tlm[0] += *(volatile ULONG *)0x40000000UL;
#endif
    /* Warpine is to end the program here; no API call gives it an exit. */
    for (;;) tlm[0]++;
}
