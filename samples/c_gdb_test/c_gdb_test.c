/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * c_gdb_test.c - a program to debug at the source level (ADR-0102)
 *
 * Built with -g by GCC or clang; lx-link writes c_gdb_test.dbg beside
 * c_gdb_test.exe, the debug companion GDB reads with symbol-file while it
 * debugs the program through Warpine's stub (warpine --gdb <port>). The
 * integration test sets a breakpoint on add_points, prints its arguments
 * and a structure, finishes it for the return value, and reads a global.
 *
 * It also calls scale_point in gdbdll.dll, which it imports, and dyn_twice
 * in gdbdyn.dll, which it loads with DosLoadModule: the stub lists both
 * DLLs' companions at their load addresses (ADR-0104), and the test stops
 * in each at its source line.
 *
 * Run as "c_gdb_test spin" it loops in guest code without calling Warpine,
 * until a debugger sets `released`: the test interrupts it there (a stop
 * request must kick the vCPU out of the guest).
 */
#include <os2.h>
#include <stdio.h>
#include "gdb_point.h"

int counter;
volatile int released;
volatile unsigned long spins;
static const char greeting[] = "debugging";

int add_points(struct point *a, const struct point *b)
{
    a->x += b->x;
    a->y += b->y;
    counter++;
    return a->x + a->y;
}

/* dyn_twice(v) from gdbdyn.dll, or -1. */
static int call_dyn_twice(int v)
{
    HMODULE hmod;
    PFN pfn;
    char fail[64];
    int result = -1;
    if (DosLoadModule((PSZ)fail, sizeof fail, "GDBDYN", &hmod) != 0) return -1;
    if (DosQueryProcAddr(hmod, 0, "dyn_twice", &pfn) == 0)
        result = ((int (*)(int))(void (*)(void))pfn)(v);
    DosFreeModule(hmod);
    return result;
}

int main(int argc, char **argv)
{
    struct point p = { 1, 2 }, q = { 30, 40 };
    if (argc > 1 && argv[1][0] == 's') {
        while (!released) spins++;
        return 0;
    }
    int sum = add_points(&p, &q);
    int grown = scale_point(&p, 3);
    int twice = call_dyn_twice(21);
    printf("c_gdb_test: %s, sum %d, counter %d\n", greeting, sum, counter);
    printf("c_gdb_test: scaled by %d, twice %d\n", grown, twice);
    return sum == 73 && grown == 62 && twice == 42 ? 0 : 1;
}
