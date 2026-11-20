/* SPDX-License-Identifier: GPL-3.0-or-later */
/* util.c - the second object of c_hello: its own .rodata / .data / .bss
 * sections (same names as hello.c's), a common symbol shared with hello.c,
 * and output through DosWrite. */
#include "os2min.h"

/* A tentative definition: with -fcommon a common symbol, merged with the
 * one in hello.c. */
int shared_counter;

static const char util_tag[] = "util";
static const char *const util_words[] = { "zero", "one", "two" };
static int util_calls;

unsigned str_len(const char *s)
{
    unsigned n = 0;
    while (s[n]) n++;
    return n;
}

void put(const char *s)
{
    ULONG done;
    util_calls++;
    shared_counter++;
    DosWrite(STDOUT, s, str_len(s), &done);
}

const char *util_word(int i) { return util_words[i]; }
const char *util_name(void) { return util_tag; }
int util_call_count(void) { return util_calls; }
