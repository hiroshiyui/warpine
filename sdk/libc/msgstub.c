/* SPDX-License-Identifier: GPL-3.0-or-later */
/* msgstub.c - DosGetMessage (ADR-0101, ADR-0105). The import (MSG.6) takes
 * the address of a message segment before the documented arguments and
 * removes it itself on return, as the library stubs of other OS/2
 * compilers call it. This stub passes the program's bound message
 * segment, __warpine_message_segment, which lx-link defines when message
 * files (.msg, from os2msg) are among its inputs; without them the weak
 * reference is 0 and DosGetMessage reads the files from the drives. The
 * caller's seven arguments are all the caller removes. */
__asm__(
    ".text\n"
    ".weak __warpine_message_segment\n"
    ".globl __warpine_DosGetMessage\n"
    "__warpine_DosGetMessage:\n"
    "    popl  %ecx\n"                         /* the caller's return address */
    "    pushl $__warpine_message_segment\n"   /* the bound segment, or 0 */
    "    pushl %ecx\n"
    "    jmp   DosGetMessage\n"
);
