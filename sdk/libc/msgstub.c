/* SPDX-License-Identifier: GPL-3.0-or-later */
/* msgstub.c - DosGetMessage (ADR-0101). The import (MSG.6) takes the
 * address of a message segment before the documented arguments and removes
 * it itself on return, as the library stubs of other OS/2 compilers call it.
 * This stub has no message segment: it puts a null one under the return
 * address and jumps to the import, so the caller's seven arguments are
 * all the caller removes. */
__asm__(
    ".text\n"
    ".globl __warpine_DosGetMessage\n"
    "__warpine_DosGetMessage:\n"
    "    popl  %ecx\n"            /* the caller's return address */
    "    pushl $0\n"              /* no message segment */
    "    pushl %ecx\n"
    "    jmp   DosGetMessage\n"
);
