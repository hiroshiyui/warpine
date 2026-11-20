/* SPDX-License-Identifier: GPL-3.0-or-later */
/* crt0.c - a minimal start-up for C programs linked by lx-link (M1).
 *
 * Warpine enters a 32-bit program as OS/2 does, with the module handle, a
 * reserved word, the environment and the command line on the stack, as
 * for a cdecl call. _start runs the preinit / init arrays, main, the fini
 * arrays in reverse, then ends the process with main's result. The stack
 * is realigned for the compiler's 16-byte assumption. */
#include "os2min.h"

typedef void (*init_fn)(void);
extern init_fn __preinit_array_start[], __preinit_array_end[];
extern init_fn __init_array_start[], __init_array_end[];
extern init_fn __fini_array_start[], __fini_array_end[];

int main(void);

__attribute__((force_align_arg_pointer, noreturn, used))
void _start(ULONG hmod, ULONG reserved, char *env, char *cmd)
{
    init_fn *f;
    int rc;
    (void)hmod; (void)reserved; (void)env; (void)cmd;
    for (f = __preinit_array_start; f < __preinit_array_end; f++) (*f)();
    for (f = __init_array_start; f < __init_array_end; f++) (*f)();
    rc = main();
    for (f = __fini_array_end; f > __fini_array_start; ) (*--f)();
    DosExit(EXIT_PROCESS, (ULONG)rc);
    for (;;) { }
}
