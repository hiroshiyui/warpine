/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/pmshl.h - the shell (PMSHAPI): profiles, the switch list, starting
 * programs (ADR-0100), declared from IBM's PM Programming Reference.
 * Included by pm.h. */
#ifndef WARPINE_OS2_PMSHL_H
#define WARPINE_OS2_PMSHL_H

#ifdef __cplusplus
extern "C" {
#endif

#pragma pack(push, 2)

/* ── Profiles ───────────────────────────────────────────────────────── */
typedef LHANDLE HINI, *PHINI;
#define HINI_PROFILE        ((HINI)0)
#define HINI_USERPROFILE    ((HINI)-1)
#define HINI_SYSTEMPROFILE  ((HINI)-2)
#define HINI_USER           HINI_USERPROFILE
#define HINI_SYSTEM         HINI_SYSTEMPROFILE

HINI    APIENTRY PrfOpenProfile(HAB hab, PCSZ pszFileName);
BOOL    APIENTRY PrfCloseProfile(HINI hini);
LONG    APIENTRY PrfQueryProfileInt(HINI hini, PCSZ pszApp, PCSZ pszKey, LONG sDefault);
ULONG   APIENTRY PrfQueryProfileString(HINI hini, PCSZ pszApp, PCSZ pszKey, PCSZ pszDefault, PVOID pBuffer,
                                       ULONG cchBufferMax);
BOOL    APIENTRY PrfWriteProfileString(HINI hini, PCSZ pszApp, PCSZ pszKey, PCSZ pszData);
BOOL    APIENTRY PrfQueryProfileSize(HINI hini, PCSZ pszApp, PCSZ pszKey, PULONG pulReqLen);
BOOL    APIENTRY PrfQueryProfileData(HINI hini, PCSZ pszApp, PCSZ pszKey, PVOID pBuffer, PULONG pulBuffLen);
BOOL    APIENTRY PrfWriteProfileData(HINI hini, PCSZ pszApp, PCSZ pszKey, PVOID pData, ULONG cchDataLen);

/* ── The switch list ────────────────────────────────────────────────── */
typedef LHANDLE HSWITCH, *PHSWITCH;
typedef LHANDLE HPROGRAM;
#define MAXNAMEL      60
#define SWL_VISIBLE   0x04
#define SWL_JUMPABLE  0x02

typedef struct _SWCNTRL {
    HWND     hwnd;
    HWND     hwndIcon;
    HPROGRAM hprog;
    PID      idProcess;
    ULONG    idSession;
    ULONG    uchVisibility;
    ULONG    fbJump;
    CHAR     szSwtitle[MAXNAMEL + 4];
    ULONG    bProgType;
} SWCNTRL, *PSWCNTRL;

typedef struct _SWENTRY {
    HSWITCH hswitch;
    SWCNTRL swctl;
} SWENTRY, *PSWENTRY;

typedef struct _SWBLOCK {
    ULONG   cswentry;
    SWENTRY aswentry[1];
} SWBLOCK, *PSWBLOCK;

HSWITCH APIENTRY WinAddSwitchEntry(PSWCNTRL pswctl);
HSWITCH APIENTRY WinCreateSwitchEntry(HAB hab, PSWCNTRL pswctl);
ULONG   APIENTRY WinChangeSwitchEntry(HSWITCH hswitch, PSWCNTRL pswctl);
ULONG   APIENTRY WinQuerySwitchEntry(HSWITCH hswitch, PSWCNTRL pswctl);
HSWITCH APIENTRY WinQuerySwitchHandle(HWND hwnd, PID pid);
ULONG   APIENTRY WinQuerySwitchList(HAB hab, PSWBLOCK pswblk, ULONG usDataLength);
ULONG   APIENTRY WinRemoveSwitchEntry(HSWITCH hswitch);
ULONG   APIENTRY WinSwitchToProgram(HSWITCH hswitch);

/* ── Starting programs ──────────────────────────────────────────────── */
typedef LHANDLE HAPP;
#define PROG_DEFAULT        0
#define PROG_FULLSCREEN     1
#define PROG_WINDOWABLEVIO  2
#define PROG_PM             3
#define SAF_INSTALLEDCMDLINE  0x0001
#define SAF_STARTCHILDAPP     0x0002

typedef struct _PROGTYPE {
    ULONG progc;
    ULONG fbVisible;
} PROGTYPE, *PPROGTYPE;

typedef struct _PROGDETAILS {
    ULONG    Length;
    PROGTYPE progt;
    PSZ      pszTitle;
    PSZ      pszExecutable;
    PSZ      pszParameters;
    PSZ      pszStartupDir;
    PSZ      pszIcon;
    PSZ      pszEnvironment;
    SWP      swpInitial;
} PROGDETAILS, *PPROGDETAILS;

HAPP    APIENTRY WinStartApp(HWND hwndNotify, PPROGDETAILS pDetails, PCSZ pszParams, PVOID Reserved,
                             ULONG fbOptions);
BOOL    APIENTRY WinTerminateApp(HAPP happ);

#pragma pack(pop)

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_OS2_PMSHL_H */
