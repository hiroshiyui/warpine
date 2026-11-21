/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * layout.c - the size of every structure the C SDK declares that Open
 * Watcom's headers declare too, and the offset of each of its fields
 * (ADR-0101). Built by Open Watcom with its headers and by GCC and clang
 * with the SDK's, then run: the outputs must be the same, line for line.
 * fields.inc lists them (one P() per structure and per field); add a
 * structure there when the SDK gains one Open Watcom also has.
 */
#ifdef __WATCOMC__
#define INCL_BASE
#define INCL_PM
#define INCL_REXXSAA
#define INCL_SPL
#define INCL_SPLDOSPRINT
#define INCL_MCIOS2
#endif
#include <os2.h>
#include <rexxsaa.h>
#include <os2me.h>
#include <stdio.h>
#include <stddef.h>

#define P(name, value) printf("%s %u\n", name, (unsigned)(value))

int main(void)
{
#include "fields.inc"
    return 0;
}
