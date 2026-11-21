/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * c_gdb_test.c - a program to debug at the source level (ADR-0102)
 *
 * Built with -g by GCC or clang; lx-link writes c_gdb_test.dbg beside
 * c_gdb_test.exe, the debug companion GDB reads with symbol-file while it
 * debugs the program through Warpine's stub (warpine --gdb <port>). The
 * integration test sets a breakpoint on add_points, prints its arguments
 * and a structure, finishes it for the return value, and reads a global.
 */
#include <os2.h>
#include <stdio.h>

struct point {
    int x, y;
};

int counter;
static const char greeting[] = "debugging";

int add_points(struct point *a, const struct point *b)
{
    a->x += b->x;
    a->y += b->y;
    counter++;
    return a->x + a->y;
}

int main(void)
{
    struct point p = { 1, 2 }, q = { 30, 40 };
    int sum = add_points(&p, &q);
    printf("c_gdb_test: %s, sum %d, counter %d\n", greeting, sum, counter);
    return sum == 73 ? 0 : 1;
}
