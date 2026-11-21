/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * gdbdll.c - a DLL the program imports, built with -g (ADR-0104)
 *
 * lx-link writes gdbdll.dbg beside gdbdll.dll; Warpine's GDB stub lists
 * it at the DLL's load address, so a breakpoint on scale_point stops at
 * its source line with no add-symbol-file.
 */
#include "gdb_point.h"

int scale_point(struct point *p, int k)
{
    int before = p->x;
    p->x *= k;
    p->y *= k;
    return p->x - before;
}
