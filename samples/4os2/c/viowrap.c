/*
 * viowrap.c — Direct 32-bit VIO/KBD API wrappers for warpine.
 *
 * Overrides the Watcom C runtime's __vfthunk-based 16-bit VIO/KBD
 * wrappers with direct 32-bit calls. These symbols are linked from
 * object files (not library), so they take precedence over clib3r.lib.
 */

/* Avoid including OS/2 VIO headers which define 16-bit signatures.
 * We define just the types we need. */
typedef unsigned short USHORT;
typedef unsigned short *PUSHORT;
typedef unsigned long  ULONG;
typedef char *PCH;
typedef unsigned char *PBYTE;
typedef void *PVOID;

/* 32-bit API imports (resolved by warpine via os2386.lib) */
USHORT _System _vio32_GetMode(PVOID, USHORT);
USHORT _System _vio32_GetCurPos(PUSHORT, PUSHORT, USHORT);
USHORT _System _vio32_SetCurPos(USHORT, USHORT, USHORT);
USHORT _System _vio32_WrtTTY(PCH, USHORT, USHORT);
USHORT _System _vio32_ScrollUp(USHORT, USHORT, USHORT, USHORT, USHORT, PBYTE, USHORT);
USHORT _System _vio32_ScrollDn(USHORT, USHORT, USHORT, USHORT, USHORT, PBYTE, USHORT);
USHORT _System _vio32_WrtCharStrAtt(PCH, USHORT, USHORT, USHORT, PBYTE, USHORT);
USHORT _System _vio32_WrtNCell(PBYTE, USHORT, USHORT, USHORT, USHORT);
USHORT _System _vio32_WrtNAttr(PBYTE, USHORT, USHORT, USHORT, USHORT);
USHORT _System _vio32_ReadCellStr(PCH, PUSHORT, USHORT, USHORT, USHORT);
USHORT _System _vio32_SetCurType(PVOID, USHORT);
USHORT _System _vio32_SetState(PVOID, USHORT);
USHORT _System _vio32_GetConfig(USHORT, PVOID, USHORT);

USHORT _System _kbd32_CharIn(PVOID, USHORT, USHORT);
USHORT _System _kbd32_GetStatus(PVOID, USHORT);

#pragma import(_vio32_GetMode, "VIOCALLS", 3)
#pragma import(_vio32_GetCurPos, "VIOCALLS", 4)
#pragma import(_vio32_SetCurPos, "VIOCALLS", 15)
#pragma import(_vio32_WrtTTY, "VIOCALLS", 30)
#pragma import(_vio32_ScrollUp, "VIOCALLS", 7)
#pragma import(_vio32_ScrollDn, "VIOCALLS", 8)
#pragma import(_vio32_WrtCharStrAtt, "VIOCALLS", 26)
#pragma import(_vio32_WrtNCell, "VIOCALLS", 28)
#pragma import(_vio32_WrtNAttr, "VIOCALLS", 27)
#pragma import(_vio32_ReadCellStr, "VIOCALLS", 24)
#pragma import(_vio32_SetCurType, "VIOCALLS", 16)
#pragma import(_vio32_SetState, "VIOCALLS", 51)
#pragma import(_vio32_GetConfig, "VIOCALLS", 46)
#pragma import(_kbd32_CharIn, "KBDCALLS", 4)
#pragma import(_kbd32_GetStatus, "KBDCALLS", 10)

/* Override clib3r.lib's VIO wrappers */
USHORT _System VioGetMode(PVOID p, USHORT h) { return _vio32_GetMode(p, h); }
USHORT _System VioGetCurPos(PUSHORT r, PUSHORT c, USHORT h) { return _vio32_GetCurPos(r, c, h); }
USHORT _System VioSetCurPos(USHORT r, USHORT c, USHORT h) { return _vio32_SetCurPos(r, c, h); }
USHORT _System VioWrtTTY(PCH s, USHORT l, USHORT h) { return _vio32_WrtTTY(s, l, h); }
USHORT _System VioScrollUp(USHORT a, USHORT b, USHORT c, USHORT d, USHORT n, PBYTE e, USHORT h) { return _vio32_ScrollUp(a,b,c,d,n,e,h); }
USHORT _System VioScrollDn(USHORT a, USHORT b, USHORT c, USHORT d, USHORT n, PBYTE e, USHORT h) { return _vio32_ScrollDn(a,b,c,d,n,e,h); }
USHORT _System VioWrtCharStrAtt(PCH s, USHORT l, USHORT r, USHORT c, PBYTE a, USHORT h) { return _vio32_WrtCharStrAtt(s,l,r,c,a,h); }
USHORT _System VioWrtNCell(PBYTE c, USHORT n, USHORT r, USHORT co, USHORT h) { return _vio32_WrtNCell(c,n,r,co,h); }
USHORT _System VioWrtNAttr(PBYTE a, USHORT n, USHORT r, USHORT c, USHORT h) { return _vio32_WrtNAttr(a,n,r,c,h); }
USHORT _System VioReadCellStr(PCH b, PUSHORT l, USHORT r, USHORT c, USHORT h) { return _vio32_ReadCellStr(b,l,r,c,h); }
USHORT _System VioSetCurType(PVOID p, USHORT h) { return _vio32_SetCurType(p, h); }
USHORT _System VioSetState(PVOID p, USHORT h) { return _vio32_SetState(p, h); }
USHORT _System VioGetConfig(USHORT r, PVOID p, USHORT h) { return _vio32_GetConfig(r, p, h); }
USHORT _System KbdCharIn(PVOID p, USHORT w, USHORT h) { return _kbd32_CharIn(p, w, h); }
USHORT _System KbdGetStatus(PVOID p, USHORT h) { return _kbd32_GetStatus(p, h); }
