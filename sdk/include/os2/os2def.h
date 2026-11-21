/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/os2def.h - the basic OS/2 types and the calling conventions, for
 * GCC and clang building 32-bit OS/2 programs (ADR-0096).
 *
 * Conventions:
 *   APIENTRY / _System  the 32-bit system convention: cdecl on i386,
 *                       arguments right to left, the caller removes them,
 *                       the result in EAX, EBX / ESI / EDI / EBP kept.
 *   OS2_PASCAL          VIO, KBD and MOU: arguments left to right, the
 *                       callee removes them. GCC and clang have no such
 *                       convention, so sub.h declares each call stdcall
 *                       with its arguments reversed (the same stack) and
 *                       wraps it in an inline function in the documented
 *                       order. */
#ifndef WARPINE_OS2_OS2DEF_H
#define WARPINE_OS2_OS2DEF_H

/* The resource compiler (os2rc) reads only the #defines. */
#if !defined(RC_INVOKED) && (!defined(__GNUC__) || !defined(__i386__))
#  error "Warpine's os2.h is for GCC or clang building i386 code (-m32, or --target=i386-unknown-none)"
#endif

#define APIENTRY    __attribute__((__cdecl__))
#define EXPENTRY    APIENTRY
#ifndef _System
#  define _System   APIENTRY
#endif
#define OS2_PASCAL  __attribute__((__stdcall__))
/* Other OS/2 compilers' convention keywords, for their sources: __syscall
 * is _System; _Optlink becomes cdecl too (both sides of a call are
 * compiled the same way). */
#ifndef __syscall
#  define __syscall APIENTRY
#endif
#ifndef _Optlink
#  define _Optlink
#endif
/* The import a declaration resolves to, when its C name differs. */
#define OS2_IMPORT(name) __asm__(#name)

#ifndef NULL
#  define NULL ((void *)0)
#endif
#ifndef NO_ERROR
#  define NO_ERROR 0
#endif
#define FALSE 0
#define TRUE  1

#define VOID void
typedef void           *PVOID;
typedef PVOID          *PPVOID;
typedef char            CHAR;
typedef unsigned char   UCHAR;
typedef short           SHORT;
typedef unsigned short  USHORT;
typedef long            LONG;
typedef unsigned long   ULONG;
typedef unsigned char   BYTE;
typedef unsigned long   BOOL;
typedef unsigned long   BOOL32;
typedef unsigned short  BOOL16;

typedef CHAR           *PCHAR;
typedef UCHAR          *PUCHAR;
typedef SHORT          *PSHORT;
typedef USHORT         *PUSHORT;
typedef LONG           *PLONG;
typedef ULONG          *PULONG;
typedef BYTE           *PBYTE;
typedef BOOL           *PBOOL;
typedef BOOL32         *PBOOL32;

/* Strings are plain char here, so string literals need no casts. */
typedef char           *PSZ;
typedef const char     *PCSZ;
typedef char           *PCH;
typedef const char     *PCCH;

typedef ULONG           APIRET;
typedef USHORT          APIRET16;

typedef ULONG           LHANDLE;
#define NULLHANDLE ((LHANDLE)0)
typedef USHORT          SHANDLE;
typedef LHANDLE         HFILE,   *PHFILE;
typedef LHANDLE         HMODULE, *PHMODULE;
typedef LHANDLE         HPIPE,   *PHPIPE;
typedef LHANDLE         HQUEUE,  *PHQUEUE;
typedef LHANDLE         HDIR,    *PHDIR;
typedef LHANDLE         HTIMER,  *PHTIMER;
typedef ULONG           HSEM,    *PHSEM;
typedef ULONG           HEV,     *PHEV;
typedef ULONG           HMTX,    *PHMTX;
typedef ULONG           HMUX,    *PHMUX;
typedef ULONG           PID,     *PPID;
typedef ULONG           TID,     *PTID;
typedef SHANDLE         HVIO,    *PHVIO;
typedef SHANDLE         HKBD,    *PHKBD;
typedef SHANDLE         HMOU,    *PHMOU;

typedef int (APIENTRY *PFN)(void);

typedef struct _QWORD {
    ULONG ulLo;
    ULONG ulHi;
} QWORD, *PQWORD;

#define MAKEULONG(l, h)  ((ULONG)(((USHORT)(l)) | ((ULONG)((USHORT)(h))) << 16))
#define MAKEUSHORT(l, h) ((USHORT)(((UCHAR)(l)) | ((USHORT)((UCHAR)(h))) << 8))
#define LOUSHORT(l)      ((USHORT)((ULONG)(l)))
#define HIUSHORT(l)      ((USHORT)(((ULONG)(l) >> 16) & 0xffff))
#define LOUCHAR(w)       ((UCHAR)(w))
#define HIUCHAR(w)       ((UCHAR)(((USHORT)(w) >> 8) & 0xff))

#endif /* WARPINE_OS2_OS2DEF_H */
