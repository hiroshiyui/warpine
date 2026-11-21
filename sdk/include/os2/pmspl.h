/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/pmspl.h - the spooler's queue manager calls (PMSPL) (ADR-0100),
 * declared from IBM's PM Programming Reference. Included by pm.h.
 * The Spl*Enum / Spl*Query calls are in os2/pmctl.h. */
#ifndef WARPINE_OS2_PMSPL_H
#define WARPINE_OS2_PMSPL_H

#ifdef __cplusplus
extern "C" {
#endif

typedef LHANDLE HSPL;
typedef PSZ *PQMOPENDATA;
#define SPL_ERROR 0L

HSPL    APIENTRY SplQmOpen(PCSZ pszToken, LONG lCount, PQMOPENDATA pqmdopData);
BOOL    APIENTRY SplQmStartDoc(HSPL hspl, PCSZ pszDocName);
ULONG   APIENTRY SplQmEndDoc(HSPL hspl);
BOOL    APIENTRY SplQmWrite(HSPL hspl, LONG lCount, PVOID pData);
BOOL    APIENTRY SplQmAbort(HSPL hspl);
BOOL    APIENTRY SplQmAbortDoc(HSPL hspl);
BOOL    APIENTRY SplQmClose(HSPL hspl);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_OS2_PMSPL_H */
