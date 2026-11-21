/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * setjmp.c - setjmp and longjmp (ADR-0110)
 *
 * jmp_buf: [0] EBX, [1] ESI, [2] EDI, [3] EBP, [4] ESP after setjmp
 * returns, [5] its return address, [6] the exception-handler chain head
 * (fs:[0], the TIB's first word).
 *
 * longjmp first unwinds the handlers registered since setjmp:
 * DosUnwindException( head, NULL, NULL ) calls each with EH_UNWINDING and
 * takes it off the chain, then returns. A head that is no longer on the
 * chain is refused, and nothing is unwound.
 */
#include <os2.h>
#include <setjmp.h>

__asm__(
    ".text\n"
    ".globl setjmp\n"
    ".type setjmp, @function\n"
    "setjmp:\n"
    "    movl 4(%esp), %eax\n"
    "    movl %ebx, 0(%eax)\n"
    "    movl %esi, 4(%eax)\n"
    "    movl %edi, 8(%eax)\n"
    "    movl %ebp, 12(%eax)\n"
    "    leal 4(%esp), %ecx\n"
    "    movl %ecx, 16(%eax)\n"
    "    movl (%esp), %ecx\n"
    "    movl %ecx, 20(%eax)\n"
    "    movl %fs:0, %ecx\n"
    "    movl %ecx, 24(%eax)\n"
    "    xorl %eax, %eax\n"
    "    ret\n"
    ".size setjmp, . - setjmp\n"

    ".globl longjmp\n"
    ".type longjmp, @function\n"
    "longjmp:\n"
    "    movl 4(%esp), %edx\n"          /* env */
    "    movl 8(%esp), %eax\n"          /* value */
    "    movl %fs:0, %ecx\n"
    "    cmpl 24(%edx), %ecx\n"
    "    je 1f\n"
    "    pushl %eax\n"
    "    pushl %edx\n"
    "    pushl $0\n"                    /* pERepRec: Warpine's XCPT_UNWIND */
    "    pushl $0\n"                    /* pTargetIP: return here */
    "    pushl 24(%edx)\n"              /* phandler: the head at setjmp */
    "    call DosUnwindException\n"
    "    addl $12, %esp\n"
    "    popl %edx\n"
    "    popl %eax\n"
    "1:  testl %eax, %eax\n"
    "    jnz 2f\n"
    "    incl %eax\n"                   /* longjmp( env, 0 ) returns 1 */
    "2:  movl 0(%edx), %ebx\n"
    "    movl 4(%edx), %esi\n"
    "    movl 8(%edx), %edi\n"
    "    movl 12(%edx), %ebp\n"
    "    movl 16(%edx), %esp\n"
    "    jmp *20(%edx)\n"
    ".size longjmp, . - longjmp\n"
);
