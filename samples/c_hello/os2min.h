/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2min.h - the few OS/2 calls c_hello uses, for GCC and clang.
 *
 * OS/2's 32-bit _System convention is cdecl on i386: arguments pushed
 * right to left, the caller removes them, the result in EAX, EBX / ESI /
 * EDI / EBP kept. lx-link resolves the names through targets/os2api.def
 * (DosWrite is DOSCALLS.282). A full os2.h for GCC and clang is M2. */
#ifndef OS2MIN_H
#define OS2MIN_H

typedef unsigned long ULONG;
typedef ULONG APIRET;
typedef ULONG HFILE;

#define NO_ERROR      0
#define EXIT_PROCESS  1
#define STDOUT        1

APIRET DosWrite(HFILE hFile, const void *pBuffer, ULONG cbWrite, ULONG *pcbActual);
void   DosExit(ULONG action, ULONG result);

#endif
