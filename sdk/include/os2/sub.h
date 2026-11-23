/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/sub.h - the VIO, KBD and MOU subsystems (VIOCALLS, KBDCALLS,
 * MOUCALLS) for GCC and clang (ADR-0096).
 *
 * These calls use the Pascal convention: arguments pushed left to right,
 * so the last one is nearest the return address, and the callee removes
 * them. GCC and clang have no Pascal convention, so each call is declared
 * twice:
 *   - the import itself (_os2p_Name, linked as Name): stdcall, which also
 *     has the callee remove the arguments, with the arguments reversed so
 *     the stack is the same, every scalar widened to 32 bits;
 *   - an inline function with the documented name and argument order,
 *     linked under its own name (_os2i_Name) so it never shares a symbol
 *     with the import.
 * The structures are packed to 2 bytes, as the subsystems lay them out. */
#ifndef WARPINE_OS2_SUB_H
#define WARPINE_OS2_SUB_H

#include <os2/os2def.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OS2_SUB_INLINE static __inline__ __attribute__((__always_inline__, __unused__))

/* One subsystem call: OS2_PASCAL_API(result, Name, (documented parameters),
 * (the same reversed and widened), (the documented names, reversed)).
 * The inline function gets its own symbol (_os2i_Name), so it never
 * shares one with the import (Name). */
#define OS2_PASCAL_API(ret, name, params, rparams, rargs) \
    OS2_PASCAL ret _os2p_##name rparams OS2_IMPORT(name); \
    OS2_SUB_INLINE ret name params OS2_IMPORT(_os2i_##name); \
    OS2_SUB_INLINE ret name params { return _os2p_##name rargs; }

#pragma pack(push, 2)

/* ── VIO ────────────────────────────────────────────────────────────── */

#define VGMT_OTHER         0x01
#define VGMT_GRAPHICS      0x02
#define VGMT_DISABLEBURST  0x04
#define ANSI_OFF  0
#define ANSI_ON   1

typedef struct _VIOMODEINFO {
    USHORT cb;
    UCHAR  fbType;
    UCHAR  color;
    USHORT col;
    USHORT row;
    USHORT hres;
    USHORT vres;
    UCHAR  fmt_ID;
    UCHAR  attrib;
    ULONG  buf_addr;
    ULONG  buf_length;
    ULONG  full_length;
    ULONG  partial_length;
    PCHAR  ext_data_addr;
} VIOMODEINFO, *PVIOMODEINFO;

typedef struct _VIOCURSORINFO {
    USHORT yStart;
    USHORT cEnd;
    USHORT cx;
    USHORT attr;   /* 0xFFFF hides the cursor */
} VIOCURSORINFO, *PVIOCURSORINFO;

/* Set cb to the bytes wanted; Warpine fills the first 10 (through
 * cbMemory) and sets cb to that. */
typedef struct _VIOCONFIGINFO {
    USHORT cb;
    USHORT adapter;
    USHORT display;
    ULONG  cbMemory;
    USHORT Configuration;
    USHORT VDHVersion;
    USHORT Flags;
    ULONG  HWBufferSize;
    ULONG  FullSaveSize;
    ULONG  PartSaveSize;
    USHORT EMAdaptersOFF;
    USHORT EMDisplaysOFF;
} VIOCONFIGINFO, *PVIOCONFIGINFO;

OS2_PASCAL_API(APIRET16, VioWrtTTY, (PCCH pch, USHORT cb, HVIO hvio),
               (ULONG hvio, ULONG cb, PCCH pch), (hvio, cb, pch))

OS2_PASCAL_API(APIRET16, VioGetCurPos, (PUSHORT pusRow, PUSHORT pusColumn, HVIO hvio),
               (ULONG hvio, PUSHORT pusColumn, PUSHORT pusRow), (hvio, pusColumn, pusRow))

OS2_PASCAL_API(APIRET16, VioSetCurPos, (USHORT usRow, USHORT usColumn, HVIO hvio),
               (ULONG hvio, ULONG usColumn, ULONG usRow), (hvio, usColumn, usRow))

OS2_PASCAL_API(APIRET16, VioGetMode, (PVIOMODEINFO pvioModeInfo, HVIO hvio),
               (ULONG hvio, PVIOMODEINFO pvioModeInfo), (hvio, pvioModeInfo))

OS2_PASCAL_API(APIRET16, VioSetMode, (PVIOMODEINFO pvioModeInfo, HVIO hvio),
               (ULONG hvio, PVIOMODEINFO pvioModeInfo), (hvio, pvioModeInfo))

OS2_PASCAL_API(APIRET16, VioScrollUp, (USHORT usTopRow, USHORT usLeftCol, USHORT usBotRow, USHORT usRightCol, USHORT cbLines, PBYTE pCell, HVIO hvio),
               (ULONG hvio, PBYTE pCell, ULONG cbLines, ULONG usRightCol, ULONG usBotRow, ULONG usLeftCol, ULONG usTopRow), (hvio, pCell, cbLines, usRightCol, usBotRow, usLeftCol, usTopRow))

OS2_PASCAL_API(APIRET16, VioScrollDn, (USHORT usTopRow, USHORT usLeftCol, USHORT usBotRow, USHORT usRightCol, USHORT cbLines, PBYTE pCell, HVIO hvio),
               (ULONG hvio, PBYTE pCell, ULONG cbLines, ULONG usRightCol, ULONG usBotRow, ULONG usLeftCol, ULONG usTopRow), (hvio, pCell, cbLines, usRightCol, usBotRow, usLeftCol, usTopRow))

OS2_PASCAL_API(APIRET16, VioWrtCellStr, (PCCH pchCellStr, USHORT cb, USHORT usRow, USHORT usColumn, HVIO hvio),
               (ULONG hvio, ULONG usColumn, ULONG usRow, ULONG cb, PCCH pchCellStr), (hvio, usColumn, usRow, cb, pchCellStr))

OS2_PASCAL_API(APIRET16, VioReadCellStr, (PCH pchCellStr, PUSHORT pcb, USHORT usRow, USHORT usColumn, HVIO hvio),
               (ULONG hvio, ULONG usColumn, ULONG usRow, PUSHORT pcb, PCH pchCellStr), (hvio, usColumn, usRow, pcb, pchCellStr))

OS2_PASCAL_API(APIRET16, VioWrtNAttr, (PBYTE pAttr, USHORT cb, USHORT usRow, USHORT usColumn, HVIO hvio),
               (ULONG hvio, ULONG usColumn, ULONG usRow, ULONG cb, PBYTE pAttr), (hvio, usColumn, usRow, cb, pAttr))

OS2_PASCAL_API(APIRET16, VioWrtNCell, (PBYTE pCell, USHORT cb, USHORT usRow, USHORT usColumn, HVIO hvio),
               (ULONG hvio, ULONG usColumn, ULONG usRow, ULONG cb, PBYTE pCell), (hvio, usColumn, usRow, cb, pCell))

OS2_PASCAL_API(APIRET16, VioWrtCharStrAtt, (PCCH pch, USHORT cb, USHORT usRow, USHORT usColumn, PBYTE pAttr, HVIO hvio),
               (ULONG hvio, PBYTE pAttr, ULONG usColumn, ULONG usRow, ULONG cb, PCCH pch), (hvio, pAttr, usColumn, usRow, cb, pch))

OS2_PASCAL_API(APIRET16, VioGetCurType, (PVIOCURSORINFO pvioCursorInfo, HVIO hvio),
               (ULONG hvio, PVIOCURSORINFO pvioCursorInfo), (hvio, pvioCursorInfo))

OS2_PASCAL_API(APIRET16, VioSetCurType, (PVIOCURSORINFO pvioCursorInfo, HVIO hvio),
               (ULONG hvio, PVIOCURSORINFO pvioCursorInfo), (hvio, pvioCursorInfo))

OS2_PASCAL_API(APIRET16, VioGetAnsi, (PUSHORT pfAnsi, HVIO hvio),
               (ULONG hvio, PUSHORT pfAnsi), (hvio, pfAnsi))

OS2_PASCAL_API(APIRET16, VioSetAnsi, (USHORT fAnsi, HVIO hvio),
               (ULONG hvio, ULONG fAnsi), (hvio, fAnsi))

OS2_PASCAL_API(APIRET16, VioGetBuf, (PULONG pLVB, PUSHORT pcbLVB, HVIO hvio),
               (ULONG hvio, PUSHORT pcbLVB, PULONG pLVB), (hvio, pcbLVB, pLVB))

OS2_PASCAL_API(APIRET16, VioShowBuf, (USHORT offLVB, USHORT cb, HVIO hvio),
               (ULONG hvio, ULONG cb, ULONG offLVB), (hvio, cb, offLVB))

OS2_PASCAL_API(APIRET16, VioSetCp, (USHORT usReserved, USHORT idCodePage, HVIO hvio),
               (ULONG hvio, ULONG idCodePage, ULONG usReserved), (hvio, idCodePage, usReserved))

OS2_PASCAL_API(APIRET16, VioGetConfig, (USHORT usConfigId, PVIOCONFIGINFO pvioin, HVIO hvio),
               (ULONG hvio, PVIOCONFIGINFO pvioin, ULONG usConfigId), (hvio, pvioin, usConfigId))

OS2_PASCAL_API(APIRET16, VioSetState, (PVOID pState, HVIO hvio),
               (ULONG hvio, PVOID pState), (hvio, pState))

OS2_PASCAL_API(APIRET16, VioCheckCharType, (PUSHORT pType, USHORT usRow, USHORT usColumn, HVIO hvio),
               (ULONG hvio, ULONG usColumn, ULONG usRow, PUSHORT pType), (hvio, usColumn, usRow, pType))


/* ── KBD ────────────────────────────────────────────────────────────── */

#define IO_WAIT    0
#define IO_NOWAIT  1

#define KEYBOARD_ECHO_ON      0x0001
#define KEYBOARD_ECHO_OFF     0x0002
#define KEYBOARD_BINARY_MODE  0x0004
#define KEYBOARD_ASCII_MODE   0x0008

typedef struct _KBDKEYINFO {
    UCHAR  chChar;
    UCHAR  chScan;
    UCHAR  fbStatus;
    UCHAR  bNlsShift;
    USHORT fsState;
    ULONG  time;
} KBDKEYINFO, *PKBDKEYINFO;

typedef struct _KBDINFO {
    USHORT cb;
    USHORT fsMask;
    USHORT chTurnAround;
    USHORT fsInterim;
    USHORT fsState;
} KBDINFO, *PKBDINFO;

typedef struct _STRINGINBUF {
    USHORT cb;
    USHORT cchIn;
} STRINGINBUF, *PSTRINGINBUF;

OS2_PASCAL_API(APIRET16, KbdCharIn, (PKBDKEYINFO pkbci, USHORT fWait, HKBD hkbd),
               (ULONG hkbd, ULONG fWait, PKBDKEYINFO pkbci), (hkbd, fWait, pkbci))

OS2_PASCAL_API(APIRET16, KbdStringIn, (PCH pch, PSTRINGINBUF pchIn, USHORT fsWait, HKBD hkbd),
               (ULONG hkbd, ULONG fsWait, PSTRINGINBUF pchIn, PCH pch), (hkbd, fsWait, pchIn, pch))

OS2_PASCAL_API(APIRET16, KbdGetStatus, (PKBDINFO pkbdinfo, HKBD hkbd),
               (ULONG hkbd, PKBDINFO pkbdinfo), (hkbd, pkbdinfo))


/* ── MOU ────────────────────────────────────────────────────────────── */

#define MOU_NOWAIT  0
#define MOU_WAIT    1

#define MOUSE_MOTION                0x0001
#define MOUSE_MOTION_WITH_BN1_DOWN  0x0002
#define MOUSE_BN1_DOWN              0x0004
#define MOUSE_MOTION_WITH_BN2_DOWN  0x0008
#define MOUSE_BN2_DOWN              0x0010
#define MOUSE_MOTION_WITH_BN3_DOWN  0x0020
#define MOUSE_BN3_DOWN              0x0040

#define MOUSE_QUEUEBUSY         0x0001
#define MOUSE_BLOCKREAD         0x0002
#define MOUSE_FLUSH             0x0004
#define MOUSE_UNSUPPORTED_MODE  0x0008
#define MOUSE_DISABLED          0x0100
#define MOUSE_MICKEYS           0x0200

typedef struct _MOUEVENTINFO {
    USHORT fs;
    ULONG  time;
    SHORT  row;
    SHORT  col;
} MOUEVENTINFO, *PMOUEVENTINFO;

typedef struct _PTRLOC {
    USHORT row;
    USHORT col;
} PTRLOC, *PPTRLOC;

typedef struct _SCALEFACT {
    USHORT rowScale;
    USHORT colScale;
} SCALEFACT, *PSCALEFACT;

typedef struct _MOUQUEINFO {
    USHORT cEvents;
    USHORT cmaxEvents;
} MOUQUEINFO, *PMOUQUEINFO;

typedef struct _NOPTRRECT {
    USHORT row;
    USHORT col;
    USHORT cRow;
    USHORT cCol;
} NOPTRRECT, *PNOPTRRECT;

/* MouGetThreshold / MouSetThreshold: usLength = sizeof(THRESHOLD). */
typedef struct _THRESHOLD {
    USHORT usLength;
    USHORT usLevel1;
    USHORT usLev1Mult;
    USHORT usLevel2;
    USHORT usLev2Mult;
} THRESHOLD, *PTHRESHOLD;

typedef struct _PTRSHAPE {
    USHORT cb;
    USHORT col;
    USHORT row;
    USHORT colHot;
    USHORT rowHot;
} PTRSHAPE, *PPTRSHAPE;

OS2_PASCAL_API(APIRET16, MouOpen, (PCSZ pszDvrName, PHMOU phmou),
               (PHMOU phmou, PCSZ pszDvrName), (phmou, pszDvrName))

OS2_PASCAL_API(APIRET16, MouClose, (HMOU hmou),
               (ULONG hmou), (hmou))

OS2_PASCAL_API(APIRET16, MouReadEventQue, (PMOUEVENTINFO pmouevEvent, PUSHORT pfWait, HMOU hmou),
               (ULONG hmou, PUSHORT pfWait, PMOUEVENTINFO pmouevEvent), (hmou, pfWait, pmouevEvent))

OS2_PASCAL_API(APIRET16, MouFlushQue, (HMOU hmou),
               (ULONG hmou), (hmou))

OS2_PASCAL_API(APIRET16, MouDrawPtr, (HMOU hmou),
               (ULONG hmou), (hmou))

OS2_PASCAL_API(APIRET16, MouSynch, (USHORT fWait),
               (ULONG fWait), (fWait))

OS2_PASCAL_API(APIRET16, MouGetHotKey, (PUSHORT pfsButtons, HMOU hmou),
               (ULONG hmou, PUSHORT pfsButtons), (hmou, pfsButtons))

OS2_PASCAL_API(APIRET16, MouSetHotKey, (PUSHORT pfsButtons, HMOU hmou),
               (ULONG hmou, PUSHORT pfsButtons), (hmou, pfsButtons))

OS2_PASCAL_API(APIRET16, MouGetThreshold, (PTHRESHOLD pthreshold, HMOU hmou),
               (ULONG hmou, PTHRESHOLD pthreshold), (hmou, pthreshold))

OS2_PASCAL_API(APIRET16, MouSetThreshold, (PTHRESHOLD pthreshold, HMOU hmou),
               (ULONG hmou, PTHRESHOLD pthreshold), (hmou, pthreshold))

OS2_PASCAL_API(APIRET16, MouRegister, (PCSZ pszModName, PCSZ pszEntryName, ULONG flFuncs),
               (ULONG flFuncs, PCSZ pszEntryName, PCSZ pszModName), (flFuncs, pszEntryName, pszModName))

OS2_PASCAL_API(APIRET16, MouDeRegister, (VOID), (VOID), ())

OS2_PASCAL_API(APIRET16, MouInitReal, (PCSZ pszDvrName),
               (PCSZ pszDvrName), (pszDvrName))

OS2_PASCAL_API(APIRET16, MouRemovePtr, (PNOPTRRECT pmourtRect, HMOU hmou),
               (ULONG hmou, PNOPTRRECT pmourtRect), (hmou, pmourtRect))

OS2_PASCAL_API(APIRET16, MouGetNumButtons, (PUSHORT pcButtons, HMOU hmou),
               (ULONG hmou, PUSHORT pcButtons), (hmou, pcButtons))

OS2_PASCAL_API(APIRET16, MouGetNumMickeys, (PUSHORT pcMickeys, HMOU hmou),
               (ULONG hmou, PUSHORT pcMickeys), (hmou, pcMickeys))

OS2_PASCAL_API(APIRET16, MouGetNumQueEl, (PMOUQUEINFO qmouqi, HMOU hmou),
               (ULONG hmou, PMOUQUEINFO qmouqi), (hmou, qmouqi))

OS2_PASCAL_API(APIRET16, MouGetEventMask, (PUSHORT pfsEvents, HMOU hmou),
               (ULONG hmou, PUSHORT pfsEvents), (hmou, pfsEvents))

OS2_PASCAL_API(APIRET16, MouSetEventMask, (PUSHORT pfsEvents, HMOU hmou),
               (ULONG hmou, PUSHORT pfsEvents), (hmou, pfsEvents))

OS2_PASCAL_API(APIRET16, MouGetDevStatus, (PUSHORT pfsDevStatus, HMOU hmou),
               (ULONG hmou, PUSHORT pfsDevStatus), (hmou, pfsDevStatus))

OS2_PASCAL_API(APIRET16, MouSetDevStatus, (PUSHORT pfsDevStatus, HMOU hmou),
               (ULONG hmou, PUSHORT pfsDevStatus), (hmou, pfsDevStatus))

OS2_PASCAL_API(APIRET16, MouGetPtrPos, (PPTRLOC pmouLoc, HMOU hmou),
               (ULONG hmou, PPTRLOC pmouLoc), (hmou, pmouLoc))

OS2_PASCAL_API(APIRET16, MouSetPtrPos, (PPTRLOC pmouLoc, HMOU hmou),
               (ULONG hmou, PPTRLOC pmouLoc), (hmou, pmouLoc))

OS2_PASCAL_API(APIRET16, MouGetScaleFact, (PSCALEFACT pmouscFactors, HMOU hmou),
               (ULONG hmou, PSCALEFACT pmouscFactors), (hmou, pmouscFactors))

OS2_PASCAL_API(APIRET16, MouSetScaleFact, (PSCALEFACT pmouscFactors, HMOU hmou),
               (ULONG hmou, PSCALEFACT pmouscFactors), (hmou, pmouscFactors))

OS2_PASCAL_API(APIRET16, MouGetPtrShape, (PBYTE pBuf, PPTRSHAPE pmoupsInfo, HMOU hmou),
               (ULONG hmou, PPTRSHAPE pmoupsInfo, PBYTE pBuf), (hmou, pmoupsInfo, pBuf))

OS2_PASCAL_API(APIRET16, MouSetPtrShape, (PBYTE pBuf, PPTRSHAPE pmoupsInfo, HMOU hmou),
               (ULONG hmou, PPTRSHAPE pmoupsInfo, PBYTE pBuf), (hmou, pmoupsInfo, pBuf))


#pragma pack(pop)

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_OS2_SUB_H */
