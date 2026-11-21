/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * setjmp.h - non-local jumps for the Warpine SDK (ADR-0110)
 *
 * longjmp unwinds the OS/2 exception handlers registered since setjmp,
 * as Open Watcom's does: it calls DosUnwindException with the chain head
 * setjmp saw, so each handler is called with EH_UNWINDING and leaves the
 * chain. A handler may leave an exception by longjmp.
 */
#ifndef WARPINE_SETJMP_H
#define WARPINE_SETJMP_H

#ifdef __cplusplus
extern "C" {
#endif

/* EBX, ESI, EDI, EBP, ESP after the return, the return address, the
 * thread's exception-handler chain head, one word spare. */
typedef int jmp_buf[8];

int setjmp(jmp_buf env) __attribute__((returns_twice));
void longjmp(jmp_buf env, int value) __attribute__((noreturn));

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_SETJMP_H */
