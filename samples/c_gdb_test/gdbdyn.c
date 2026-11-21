/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * gdbdyn.c - a DLL the program loads with DosLoadModule, built with -g
 * (ADR-0104)
 *
 * Loading it stops the program for GDB to read the new library list, so
 * a pending breakpoint on dyn_twice takes effect before it is called.
 */
int dyn_twice(int v)
{
    int twice = v * 2;
    return twice;
}
