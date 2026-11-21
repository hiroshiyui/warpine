/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/warpine.h - Warpine's own extensions (module WARPINE) that the
 * SDK's run-time libraries use (ADR-0107). They are not OS/2 calls: a
 * program that uses them runs on Warpine only. */
#ifndef WARPINE_OS2_WARPINE_H
#define WARPINE_OS2_WARPINE_H

/* The unwind tables of a module's C++ runtime: the .eh_frame records
 * lx-link places at __EH_FRAME_BEGIN__, ending with a zero word. A
 * runtime registers its module's tables when it starts and deregisters
 * them when it ends; an unwinder that reaches another module's frame asks
 * for that module's tables by an address in its code. */
APIRET APIENTRY WrpRegisterUnwindTables(PVOID pBegin);
APIRET APIENTRY WrpDeregisterUnwindTables(PVOID pBegin);
APIRET APIENTRY WrpQueryUnwindTables(PVOID pCode, PVOID *ppBegin);

/* The calling thread's runtime block: WRP_RUNTIME_BLOCK_SIZE bytes, zero
 * when the thread starts, that every module of the process shares on
 * that thread (the same address on every thread). Words 0 and 1 belong to
 * the SDK's C++ runtime (the caught exceptions, the uncaught count); the
 * others are reserved. */
#define WRP_RUNTIME_BLOCK_SIZE  64
#define WRP_RUNTIME_CXX_WORD    0
APIRET APIENTRY WrpQueryRuntimeBlock(PULONG *ppBlock);

#endif
