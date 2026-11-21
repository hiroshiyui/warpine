/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/pmhelp.h - the Help Manager (HELPMGR) (ADR-0100), declared from
 * IBM's PM Programming Reference. Included by pm.h. */
#ifndef WARPINE_OS2_PMHELP_H
#define WARPINE_OS2_PMHELP_H

#ifdef __cplusplus
extern "C" {
#endif

#pragma pack(push, 2)

#define HM_MSG_BASE            0x0220
#define HM_DISMISS_WINDOW      0x0221
#define HM_DISPLAY_HELP        0x0222
#define HM_EXT_HELP            0x0223
#define HM_SET_ACTIVE_WINDOW   0x0224
#define HM_LOAD_HELP_TABLE     0x0225
#define HM_CREATE_HELP_TABLE   0x0226
#define HM_HELP_INDEX          0x022A
#define HM_HELP_CONTENTS       0x022B
#define HM_KEYS_HELP           0x022C
#define HM_CONTROL             0x0244

/* A help subtable is an array of USHORTs (its size, then pairs). */
typedef USHORT HELPSUBTABLE, *PHELPSUBTABLE;

typedef struct _HELPTABLE {
    USHORT        idAppWindow;
    PHELPSUBTABLE phstHelpSubTable;
    USHORT        idExtPanel;
} HELPTABLE, *PHELPTABLE;

typedef struct _HELPINIT {
    ULONG      cb;
    ULONG      ulReturnCode;
    PSZ        pszTutorialName;
    PHELPTABLE phtHelpTable;
    HMODULE    hmodHelpTableModule;
    HMODULE    hmodAccelActionBarModule;
    ULONG      idAccelTable;
    ULONG      idActionBar;
    PSZ        pszHelpWindowTitle;
    ULONG      fShowPanelId;
    PSZ        pszHelpLibraryName;
} HELPINIT, *PHELPINIT;

HWND    APIENTRY WinCreateHelpInstance(HAB hab, PHELPINIT phinitHMInitStructure);
BOOL    APIENTRY WinDestroyHelpInstance(HWND hwndHelpInstance);
HWND    APIENTRY WinQueryHelpInstance(HWND hwndApp);
BOOL    APIENTRY WinAssociateHelpInstance(HWND hwndHelpInstance, HWND hwndApp);
BOOL    APIENTRY WinLoadHelpTable(HWND hwndHelpInstance, ULONG idHelpTable, HMODULE Module);
BOOL    APIENTRY WinCreateHelpTable(HWND hwndHelpInstance, PHELPTABLE phtHelpTable);

#pragma pack(pop)

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_OS2_PMHELP_H */
