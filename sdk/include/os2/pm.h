/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/pm.h - the Presentation Manager (PMWIN and PMGPI) for GCC and clang
 * (ADR-0097), declared from IBM's Presentation Manager Programming
 * Reference. INCL_WIN selects the window manager, INCL_GPI the graphics,
 * INCL_PM both. Every call is APIENTRY; window and dialog procedures are
 * EXPENTRY. The structures the window manager passes are packed to 2
 * bytes (FRAMECDATA is 10 bytes); the others have no padding to lose.
 *
 * The integer constants equal Warpine's own wherever Warpine defines
 * them (`gen_api check-headers`). */
#ifndef WARPINE_OS2_PM_H
#define WARPINE_OS2_PM_H

#include <os2/os2def.h>

#ifdef __cplusplus
extern "C" {
#endif

#pragma pack(push, 2)

/* ── Common types ───────────────────────────────────────────────────── */
typedef LHANDLE HAB,      *PHAB;
typedef LHANDLE HMQ,      *PHMQ;
typedef LHANDLE HWND,     *PHWND;
typedef LHANDLE HPS,      *PHPS;
typedef LHANDLE HDC,      *PHDC;
typedef LHANDLE HRGN,     *PHRGN;
typedef LHANDLE HBITMAP,  *PHBITMAP;
typedef LHANDLE HPOINTER, *PHPOINTER;
typedef LHANDLE HACCEL,   *PHACCEL;
typedef LHANDLE HATOMTABLE;
typedef LHANDLE HMF;
typedef ULONG   ATOM;
typedef ULONG   ERRORID;
typedef LONG    FIXED,    *PFIXED;
typedef LONG    COLOR;
typedef char    STR8[8],  *PSTR8;

typedef VOID *MPARAM, **PMPARAM;
typedef VOID *MRESULT, **PMRESULT;
typedef MRESULT (EXPENTRY *PFNWP)(HWND, ULONG, MPARAM, MPARAM);

typedef struct _POINTL { LONG x; LONG y; } POINTL, *PPOINTL;
typedef struct _POINTS { SHORT x; SHORT y; } POINTS, *PPOINTS;
typedef struct _RECTL  { LONG xLeft; LONG yBottom; LONG xRight; LONG yTop; } RECTL, *PRECTL;
typedef struct _SIZEL  { LONG cx; LONG cy; } SIZEL, *PSIZEL;

#define MAKEFIXED(intpart, fractpart) MAKELONG(fractpart, intpart)
#define FIXEDINT(fx)   ((SHORT)HIUSHORT(fx))
#define FIXEDFRAC(fx)  (LOUSHORT(fx))
#ifndef MAKELONG
#  define MAKELONG(l, h) ((LONG)MAKEULONG(l, h))
#endif

/* ── Message parameters ─────────────────────────────────────────────── */
#define MPVOID                  ((MPARAM)0L)
#define MPFROMP(p)              ((MPARAM)((ULONG)(p)))
#define MPFROMHWND(hwnd)        ((MPARAM)(HWND)(hwnd))
#define MPFROMCHAR(ch)          ((MPARAM)(USHORT)(ch))
#define MPFROMSHORT(s)          ((MPARAM)(USHORT)(s))
#define MPFROM2SHORT(s1, s2)    ((MPARAM)MAKELONG(s1, s2))
#define MPFROMSH2CH(s, uch1, uch2) ((MPARAM)MAKELONG(s, MAKESHORT(uch1, uch2)))
#define MPFROMLONG(l)           ((MPARAM)(ULONG)(l))
#define PVOIDFROMMP(mp)         ((VOID *)(mp))
#define HWNDFROMMP(mp)          ((HWND)(mp))
#define CHAR1FROMMP(mp)         ((UCHAR)((ULONG)(mp)))
#define CHAR2FROMMP(mp)         ((UCHAR)((ULONG)(mp) >> 8))
#define CHAR3FROMMP(mp)         ((UCHAR)((ULONG)(mp) >> 16))
#define CHAR4FROMMP(mp)         ((UCHAR)((ULONG)(mp) >> 24))
#define SHORT1FROMMP(mp)        ((USHORT)(ULONG)(mp))
#define SHORT2FROMMP(mp)        ((USHORT)((ULONG)(mp) >> 16))
#define LONGFROMMP(mp)          ((ULONG)(mp))
#define MRFROMP(p)              ((MRESULT)(VOID *)(p))
#define MRFROMSHORT(s)          ((MRESULT)(USHORT)(s))
#define MRFROM2SHORT(s1, s2)    ((MRESULT)MAKELONG(s1, s2))
#define MRFROMLONG(l)           ((MRESULT)(ULONG)(l))
#define PVOIDFROMMR(mr)         ((VOID *)(mr))
#define SHORT1FROMMR(mr)        ((USHORT)((ULONG)(mr)))
#define SHORT2FROMMR(mr)        ((USHORT)((ULONG)(mr) >> 16))
#define LONGFROMMR(mr)          ((ULONG)(mr))
#ifndef MAKESHORT
#  define MAKESHORT(l, h)       ((SHORT)MAKEUSHORT(l, h))
#endif

/* ═══ The window manager (PMWIN) ═══════════════════════════════════════ */
#if defined(INCL_WIN) || defined(INCL_PM) || !defined(WARPINE_OS2_H)

/* Messages */
#define WM_NULL                0x0000
#define WM_CREATE              0x0001
#define WM_DESTROY             0x0002
#define WM_ENABLE              0x0004
#define WM_SHOW                0x0005
#define WM_MOVE                0x0006
#define WM_SIZE                0x0007
#define WM_HITTEST             0x000C
#define WM_ACTIVATE            0x000D
#define WM_SETFOCUS            0x000F
#define WM_COMMAND             0x0020
#define WM_SYSCOMMAND          0x0021
#define WM_HELP                0x0022
#define WM_PAINT               0x0023
#define WM_TIMER               0x0024
#define WM_SEM1                0x0025
#define WM_SEM2                0x0026
#define WM_SEM3                0x0027
#define WM_SEM4                0x0028
#define WM_CLOSE               0x0029
#define WM_QUIT                0x002A
#define WM_SYSCOLORCHANGE      0x002B
#define WM_APPTERMINATENOTIFY  0x002E
#define WM_PRESPARAMCHANGED    0x002F
#define WM_CONTROL             0x0030
#define WM_VSCROLL             0x0031
#define WM_HSCROLL             0x0032
#define WM_INITMENU            0x0033
#define WM_MENUSELECT          0x0034
#define WM_MENUEND             0x0035
#define WM_DRAWITEM            0x0036
#define WM_MEASUREITEM         0x0037
#define WM_QUERYDLGCODE        0x003A
#define WM_INITDLG             0x003B
#define WM_MATCHMNEMONIC       0x003D
#define WM_FORMATFRAME         0x0041
#define WM_UPDATEFRAME         0x0042
#define WM_MINMAXFRAME         0x0046
#define WM_ERASEBACKGROUND     0x004F
#define WM_CALCFRAMERECT       0x0053
#define WM_QUERYFRAMECTLCOUNT  0x0059
#define WM_MOUSEMOVE           0x0070
#define WM_BUTTON1DOWN         0x0071
#define WM_BUTTON1UP           0x0072
#define WM_BUTTON1DBLCLK       0x0073
#define WM_BUTTON2DOWN         0x0074
#define WM_BUTTON2UP           0x0075
#define WM_BUTTON2DBLCLK       0x0076
#define WM_BUTTON3DOWN         0x0077
#define WM_BUTTON3UP           0x0078
#define WM_BUTTON3DBLCLK       0x0079
#define WM_CHAR                0x007A
#define WM_BEGINDRAG           0x0420
#define WM_ENDDRAG             0x0421
#define WM_CONTEXTMENU         0x0424
#define WM_DESTROYCLIPBOARD    0x0062
#define WM_DRAWCLIPBOARD       0x0067
#define WM_USER                0x1000

/* Window styles */
#define WS_VISIBLE       (-2147483648)
#define WS_DISABLED      0x40000000
#define WS_CLIPCHILDREN  0x20000000
#define WS_CLIPSIBLINGS  0x10000000
#define WS_PARENTCLIP    0x08000000
#define WS_SAVEBITS      0x04000000
#define WS_SYNCPAINT     0x02000000
#define WS_MINIMIZED     0x01000000
#define WS_MAXIMIZED     0x00800000
#define WS_TABSTOP       0x00020000
#define WS_GROUP         0x00010000

/* Class styles */
#define CS_MOVENOTIFY    0x00000001
#define CS_SIZEREDRAW    0x00000004
#define CS_HITTEST       0x00000008
#define CS_PUBLIC        0x00000010
#define CS_FRAME         0x00000020
#define CS_CLIPCHILDREN  0x20000000
#define CS_CLIPSIBLINGS  0x10000000
#define CS_PARENTCLIP    0x08000000
#define CS_SAVEBITS      0x04000000
#define CS_SYNCPAINT     0x02000000

/* Frame creation flags */
#define FCF_TITLEBAR       0x00000001
#define FCF_SYSMENU        0x00000002
#define FCF_MENU           0x00000004
#define FCF_SIZEBORDER     0x00000008
#define FCF_MINBUTTON      0x00000010
#define FCF_MAXBUTTON      0x00000020
#define FCF_MINMAX         0x00000030
#define FCF_VERTSCROLL     0x00000040
#define FCF_HORZSCROLL     0x00000080
#define FCF_DLGBORDER      0x00000100
#define FCF_BORDER         0x00000200
#define FCF_SHELLPOSITION  0x00000400
#define FCF_TASKLIST       0x00000800
#define FCF_NOBYTEALIGN    0x00001000
#define FCF_ICON           0x00004000
#define FCF_ACCELTABLE     0x00008000
#define FCF_SYSMODAL       0x00010000
#define FCF_HIDEBUTTON     0x01000000
#define FCF_HIDEMAX        0x01000020
#define FCF_STANDARD       0x0000CC3F

/* Frame styles */
#define FS_BORDER      0x00000100
#define FS_DLGBORDER   0x00000080
#define FS_SIZEBORDER  0x00000800

/* Frame controls */
#define FID_SYSMENU     0x8002
#define FID_TITLEBAR    0x8003
#define FID_MINMAX      0x8004
#define FID_MENU        0x8005
#define FID_VERTSCROLL  0x8006
#define FID_HORZSCROLL  0x8007
#define FID_CLIENT      0x8008

/* WinSetWindowPos flags */
#define SWP_SIZE        0x0001
#define SWP_MOVE        0x0002
#define SWP_ZORDER      0x0004
#define SWP_SHOW        0x0008
#define SWP_HIDE        0x0010
#define SWP_NOREDRAW    0x0020
#define SWP_NOADJUST    0x0040
#define SWP_ACTIVATE    0x0080
#define SWP_DEACTIVATE  0x0100
#define SWP_MINIMIZE    0x0400
#define SWP_MAXIMIZE    0x0800
#define SWP_RESTORE     0x1000

/* Special windows */
#define HWND_DESKTOP        ((HWND)1)
#define HWND_OBJECT         ((HWND)2)
#define HWND_TOP            ((HWND)3)
#define HWND_BOTTOM         ((HWND)4)
#define HWND_THREADCAPTURE  ((HWND)5)

/* WinQueryWindow */
#define QW_NEXT        0
#define QW_PREV        1
#define QW_TOP         2
#define QW_BOTTOM      3
#define QW_OWNER       4
#define QW_PARENT      5
#define QW_NEXTTOP     6
#define QW_PREVTOP     7
#define QW_FRAMEOWNER  8

/* Window words */
#define QWL_USER       0
#define QWS_ID         (-1)
#define QWL_STYLE      (-2)
#define QWL_HMQ        (-4)
#define QWS_XRESTORE   12
#define QWS_YRESTORE   14
#define QWS_CXRESTORE  16
#define QWS_CYRESTORE  18
#define QWS_XMINIMIZE  20
#define QWS_YMINIMIZE  22

/* WinPeekMsg */
#define PM_NOREMOVE  0
#define PM_REMOVE    1

/* WinMessageBox styles */
#define MB_OK                0x0000
#define MB_OKCANCEL          0x0001
#define MB_RETRYCANCEL       0x0002
#define MB_ABORTRETRYIGNORE  0x0003
#define MB_YESNO             0x0004
#define MB_YESNOCANCEL       0x0005
#define MB_CANCEL            0x0006
#define MB_ENTER             0x0007
#define MB_ENTERCANCEL       0x0008
#define MB_NOICON            0x0000
#define MB_CUANOTIFICATION   0x0000
#define MB_ICONQUESTION      0x0010
#define MB_ICONEXCLAMATION   0x0020
#define MB_CUAWARNING        0x0020
#define MB_ICONASTERISK      0x0030
#define MB_ICONHAND          0x0040
#define MB_CUACRITICAL       0x0040
#define MB_QUERY             0x0010
#define MB_WARNING           0x0020
#define MB_INFORMATION       0x0030
#define MB_ERROR             0x0040
#define MB_DEFBUTTON1        0x0000
#define MB_DEFBUTTON2        0x0100
#define MB_DEFBUTTON3        0x0200
#define MB_APPLMODAL         0x0000
#define MB_SYSTEMMODAL       0x1000
#define MB_HELP              0x2000
#define MB_MOVEABLE          0x4000
#define MB_CUSTOMICON        0x0080

/* WinMessageBox results */
#define MBID_OK      0x0001
#define MBID_CANCEL  0x0002
#define MBID_ABORT   0x0003
#define MBID_RETRY   0x0004
#define MBID_IGNORE  0x0005
#define MBID_YES     0x0006
#define MBID_NO      0x0007
#define MBID_HELP    0x0008
#define MBID_ENTER   0x0009
#define MBID_ERROR   (-1)

/* Dialog results */
#define DID_OK      0x0001
#define DID_CANCEL  0x0002
#define DID_ERROR   0xFFFF

/* WM_COMMAND sources */
#define CMDSRC_OTHER        0
#define CMDSRC_PUSHBUTTON   1
#define CMDSRC_MENU         2
#define CMDSRC_ACCELERATOR  3

/* System commands */
#define SC_SIZE         0x8000
#define SC_MOVE         0x8001
#define SC_MINIMIZE     0x8002
#define SC_MAXIMIZE     0x8003
#define SC_CLOSE        0x8004
#define SC_SYSMENU      0x8007
#define SC_RESTORE      0x8008
#define SC_TASKMANAGER  0x8011
#define SC_HIDE         0x802A

/* WinDrawText */
#define DT_LEFT             0x0000
#define DT_QUERYEXTENT      0x0002
#define DT_UNDERSCORE       0x0010
#define DT_STRIKEOUT        0x0020
#define DT_TEXTATTRS        0x0040
#define DT_EXTERNALLEADING  0x0080
#define DT_CENTER           0x0100
#define DT_RIGHT            0x0200
#define DT_TOP              0x0000
#define DT_VCENTER          0x0400
#define DT_BOTTOM           0x0800
#define DT_HALFTONE         0x1000
#define DT_MNEMONIC         0x2000
#define DT_WORDBREAK        0x4000
#define DT_ERASERECT        0x8000

/* Colours */
#define CLR_ERROR       (-255)
#define CLR_DEFAULT     (-3)
#define CLR_WHITE       (-2)
#define CLR_BLACK       (-1)
#define CLR_BACKGROUND  0
#define CLR_BLUE        1
#define CLR_RED         2
#define CLR_PINK        3
#define CLR_GREEN       4
#define CLR_CYAN        5
#define CLR_YELLOW      6
#define CLR_NEUTRAL     7
#define CLR_DARKGRAY    8
#define CLR_DARKBLUE    9
#define CLR_DARKRED     10
#define CLR_DARKPINK    11
#define CLR_DARKGREEN   12
#define CLR_DARKCYAN    13
#define CLR_BROWN       14
#define CLR_PALEGRAY    15

/* System colours */
#define SYSCLR_SHADOWHILITEBGND       (-50)
#define SYSCLR_SHADOWHILITEFGND       (-49)
#define SYSCLR_SHADOWTEXT             (-48)
#define SYSCLR_ENTRYFIELD             (-47)
#define SYSCLR_MENUDISABLEDTEXT       (-46)
#define SYSCLR_MENUHILITE             (-45)
#define SYSCLR_MENUHILITEBGND         (-44)
#define SYSCLR_PAGEBACKGROUND         (-43)
#define SYSCLR_FIELDBACKGROUND        (-42)
#define SYSCLR_BUTTONLIGHT            (-41)
#define SYSCLR_BUTTONMIDDLE           (-40)
#define SYSCLR_BUTTONDARK             (-39)
#define SYSCLR_BUTTONDEFAULT          (-38)
#define SYSCLR_TITLEBOTTOM            (-37)
#define SYSCLR_SHADOW                 (-36)
#define SYSCLR_ICONTEXT               (-35)
#define SYSCLR_DIALOGBACKGROUND       (-34)
#define SYSCLR_HILITEFOREGROUND       (-33)
#define SYSCLR_HILITEBACKGROUND       (-32)
#define SYSCLR_INACTIVETITLETEXTBGND  (-31)
#define SYSCLR_ACTIVETITLETEXTBGND    (-30)
#define SYSCLR_INACTIVETITLETEXT      (-29)
#define SYSCLR_ACTIVETITLETEXT        (-28)
#define SYSCLR_OUTPUTTEXT             (-27)
#define SYSCLR_WINDOWSTATICTEXT       (-26)
#define SYSCLR_SCROLLBAR              (-25)
#define SYSCLR_BACKGROUND             (-24)
#define SYSCLR_ACTIVETITLE            (-23)
#define SYSCLR_INACTIVETITLE          (-22)
#define SYSCLR_MENU                   (-21)
#define SYSCLR_WINDOW                 (-20)
#define SYSCLR_WINDOWFRAME            (-19)
#define SYSCLR_MENUTEXT               (-18)
#define SYSCLR_WINDOWTEXT             (-17)
#define SYSCLR_TITLETEXT              (-16)
#define SYSCLR_ACTIVEBORDER           (-15)
#define SYSCLR_INACTIVEBORDER         (-14)
#define SYSCLR_APPWORKSPACE           (-13)
#define SYSCLR_HELPBACKGROUND         (-12)
#define SYSCLR_HELPTEXT               (-11)
#define SYSCLR_HELPHILITE             (-10)
#define SYSCLR_CSYSCOLORS             41

/* System values */
#define SV_CXSIZEBORDER  4
#define SV_CYSIZEBORDER  5
#define SV_CURSORRATE    9
#define SV_CXSCREEN      20
#define SV_CYSCREEN      21
#define SV_CXVSCROLL     22
#define SV_CYHSCROLL     23
#define SV_CXBORDER      26
#define SV_CYBORDER      27
#define SV_CXDLGFRAME    28
#define SV_CYDLGFRAME    29
#define SV_CYTITLEBAR    30
#define SV_CYMENU        35
#define SV_CXFULLSCREEN  36
#define SV_CYFULLSCREEN  37
#define SV_CXPOINTER     40
#define SV_CYPOINTER     41
#define SV_POINTERLEVEL  44
#define SV_MOUSEPRESENT  48

/* System pointers */
#define SPTR_ARROW            1
#define SPTR_TEXT             2
#define SPTR_WAIT             3
#define SPTR_SIZE             4
#define SPTR_MOVE             5
#define SPTR_SIZENWSE         6
#define SPTR_SIZENESW         7
#define SPTR_SIZEWE           8
#define SPTR_SIZENS           9
#define SPTR_APPICON          10
#define SPTR_ICONINFORMATION  11
#define SPTR_ICONQUESTION     12
#define SPTR_ICONERROR        13
#define SPTR_ICONWARNING      14
#define SPTR_ILLEGAL          18
#define SPTR_FILE             19
#define SPTR_FOLDER           20
#define SPTR_MULTFILE         21
#define SPTR_PROGRAM          22
#define SPTR_PENFIRST         23
#define SPTR_CPTR             39

/* WM_CHAR flags */
#define KC_CHAR        0x0001
#define KC_VIRTUALKEY  0x0002
#define KC_SCANCODE    0x0004
#define KC_SHIFT       0x0008
#define KC_CTRL        0x0010
#define KC_ALT         0x0020
#define KC_KEYUP       0x0040
#define KC_PREVDOWN    0x0080
#define KC_LONEKEY     0x0100

/* Virtual keys */
#define VK_BUTTON1    0x0001
#define VK_BUTTON2    0x0002
#define VK_BUTTON3    0x0003
#define VK_BACKSPACE  0x0005
#define VK_TAB        0x0006
#define VK_BACKTAB    0x0007
#define VK_NEWLINE    0x0008
#define VK_SHIFT      0x0009
#define VK_CTRL       0x000A
#define VK_ALT        0x000B
#define VK_ALTGRAF    0x000C
#define VK_PAUSE      0x000D
#define VK_CAPSLOCK   0x000E
#define VK_ESC        0x000F
#define VK_SPACE      0x0010
#define VK_PAGEUP     0x0011
#define VK_PAGEDOWN   0x0012
#define VK_END        0x0013
#define VK_HOME       0x0014
#define VK_LEFT       0x0015
#define VK_UP         0x0016
#define VK_RIGHT      0x0017
#define VK_DOWN       0x0018
#define VK_PRINTSCRN  0x0019
#define VK_INSERT     0x001A
#define VK_DELETE     0x001B
#define VK_SCRLLOCK   0x001C
#define VK_NUMLOCK    0x001D
#define VK_ENTER      0x001E
#define VK_F1         0x0020
#define VK_F2         0x0021
#define VK_F3         0x0022
#define VK_F4         0x0023
#define VK_F5         0x0024
#define VK_F6         0x0025
#define VK_F7         0x0026
#define VK_F8         0x0027
#define VK_F9         0x0028
#define VK_F10        0x0029
#define VK_F11        0x002A
#define VK_F12        0x002B
#define VK_ENDDRAG    0x0038

/* Presentation parameters */
#define PP_FOREGROUNDCOLOR        0x0001
#define PP_FOREGROUNDCOLORINDEX   0x0002
#define PP_BACKGROUNDCOLOR        0x0003
#define PP_BACKGROUNDCOLORINDEX   0x0004
#define PP_HILITEFOREGROUNDCOLOR  0x0005
#define PP_HILITEBACKGROUNDCOLOR  0x0007
#define PP_BORDERCOLOR            0x000D
#define PP_FONTNAMESIZE           0x000F
#define PP_BUTTONBACKGROUNDCOLOR  0x003A
#define PP_USER                   0x8000

/* Buttons */
#define BS_PUSHBUTTON       0x0000
#define BS_CHECKBOX         0x0001
#define BS_AUTOCHECKBOX     0x0002
#define BS_RADIOBUTTON      0x0003
#define BS_AUTORADIOBUTTON  0x0004
#define BS_3STATE           0x0005
#define BS_AUTO3STATE       0x0006
#define BS_USERBUTTON       0x0007
#define BS_NOTEBOOKBUTTON   0x0008
#define BS_PRIMARYSTYLES    0x000F
#define BS_HELP             0x0100
#define BS_SYSCOMMAND       0x0200
#define BS_DEFAULT          0x0400
#define BS_NOPOINTERFOCUS   0x0800
#define BS_NOCURSORSELECT   0x2000
#define BM_CLICK            0x0120
#define BM_QUERYCHECKINDEX  0x0121
#define BM_QUERYHILITE      0x0122
#define BM_SETHILITE        0x0123
#define BM_QUERYCHECK       0x0124
#define BM_SETCHECK         0x0125
#define BM_SETDEFAULT       0x0126
#define BN_CLICKED          0x0001
#define BN_DBLCLICKED       0x0002
#define BN_PAINT            0x0003

/* Entry fields */
#define ES_CENTER         0x0001
#define ES_RIGHT          0x0002
#define ES_READONLY       0x0020
#define ES_LEFT           0x0000
#define ES_AUTOSCROLL     0x0004
#define ES_MARGIN         0x0008
#define EM_QUERYCHANGED   0x0140
#define EM_QUERYSEL       0x0141
#define EM_SETSEL         0x0142
#define EM_SETTEXTLIMIT   0x0143
#define EM_CUT            0x0144
#define EM_COPY           0x0145
#define EM_CLEAR          0x0146
#define EM_PASTE          0x0147
#define EM_QUERYREADONLY  0x014A
#define EM_SETREADONLY    0x014B
#define EM_SETINSERTMODE  0x014C
#define EN_SETFOCUS       0x0001
#define EN_KILLFOCUS      0x0002
#define EN_CHANGE         0x0004
#define EN_SCROLL         0x0008
#define EN_MEMERROR       0x0010

/* Static controls */
#define SS_TEXT           0x0001
#define SS_GROUPBOX       0x0002
#define SS_ICON           0x0003
#define SS_BITMAP         0x0004
#define SS_FGNDRECT       0x0005
#define SS_HALFTONERECT   0x0006
#define SS_BKGNDRECT      0x0007
#define SS_FGNDFRAME      0x0008
#define SS_HALFTONEFRAME  0x0009
#define SS_BKGNDFRAME     0x000A
#define SS_SYSICON        0x000B
#define SS_AUTOSIZE       0x0040

/* List boxes */
#define LS_MULTIPLESEL          0x0001
#define LS_OWNERDRAW            0x0002
#define LS_EXTENDEDSEL          0x0010
#define LM_QUERYITEMCOUNT       0x0160
#define LM_INSERTITEM           0x0161
#define LM_SETTOPINDEX          0x0162
#define LM_DELETEITEM           0x0163
#define LM_SELECTITEM           0x0164
#define LM_QUERYSELECTION       0x0165
#define LM_SETITEMTEXT          0x0166
#define LM_QUERYITEMTEXTLENGTH  0x0167
#define LM_QUERYITEMTEXT        0x0168
#define LM_SETITEMHANDLE        0x0169
#define LM_QUERYITEMHANDLE      0x016A
#define LM_SEARCHSTRING         0x016B
#define LM_SETITEMHEIGHT        0x016C
#define LM_QUERYTOPINDEX        0x016D
#define LM_DELETEALL            0x016E
#define LN_SELECT               0x0001
#define LN_SETFOCUS             0x0002
#define LN_KILLFOCUS            0x0003
#define LN_SCROLL               0x0004
#define LN_ENTER                0x0005
#define LIT_CURSOR              (-4)
#define LIT_ERROR               (-3)
#define LIT_MEMERROR            (-2)
#define LIT_NONE                (-1)
#define LIT_FIRST               (-1)
#define LIT_END                 (-1)
#define LIT_SORTASCENDING       (-2)
#define LIT_SORTDESCENDING      (-3)

/* Menus */
#define MIS_TEXT             0x0001
#define MIS_BITMAP           0x0002
#define MIS_SEPARATOR        0x0004
#define MIS_OWNERDRAW        0x0008
#define MIS_SUBMENU          0x0010
#define MIS_MULTMENU         0x0020
#define MIS_SYSCOMMAND       0x0040
#define MIS_HELP             0x0080
#define MIS_STATIC           0x0100
#define MIS_BUTTONSEPARATOR  0x0200
#define MIS_BREAK            0x0400
#define MIS_BREAKSEPARATOR   0x0800
#define MIS_GROUP            0x1000
#define MIS_SINGLE           0x2000
#define MIA_NODISMISS        0x0020
#define MIA_FRAMED           0x1000
#define MIA_CHECKED          0x2000
#define MIA_DISABLED         0x4000
#define MIA_HILITED          0x8000
#define MIT_END              (-1)
#define MIT_NONE             (-1)
#define MIT_ERROR            (-1)

/* Scroll bars */
#define SBS_HORZ           0x0000
#define SBS_VERT           0x0001
#define SBS_THUMBSIZE      0x0002
#define SBS_AUTOTRACK      0x0004
#define SBS_AUTOSIZE       0x2000
#define SB_LINEUP          0x0001
#define SB_LINELEFT        0x0001
#define SB_LINEDOWN        0x0002
#define SB_LINERIGHT       0x0002
#define SB_PAGEUP          0x0003
#define SB_PAGELEFT        0x0003
#define SB_PAGEDOWN        0x0004
#define SB_PAGERIGHT       0x0004
#define SB_SLIDERTRACK     0x0005
#define SB_SLIDERPOSITION  0x0006
#define SB_ENDSCROLL       0x0007

/* Built-in window classes */
#define WC_FRAME       ((PSZ)0xFFFF0001L)
#define WC_COMBOBOX    ((PSZ)0xFFFF0002L)
#define WC_BUTTON      ((PSZ)0xFFFF0003L)
#define WC_MENU        ((PSZ)0xFFFF0004L)
#define WC_STATIC      ((PSZ)0xFFFF0005L)
#define WC_ENTRYFIELD  ((PSZ)0xFFFF0006L)
#define WC_LISTBOX     ((PSZ)0xFFFF0007L)
#define WC_SCROLLBAR   ((PSZ)0xFFFF0008L)
#define WC_TITLEBAR    ((PSZ)0xFFFF0009L)
#define WC_MLE         ((PSZ)0xFFFF000AL)
#define WC_SPINBUTTON  ((PSZ)0xFFFF0020L)
#define WC_CONTAINER   ((PSZ)0xFFFF0025L)
#define WC_SLIDER      ((PSZ)0xFFFF0026L)
#define WC_VALUESET    ((PSZ)0xFFFF0027L)
#define WC_NOTEBOOK    ((PSZ)0xFFFF0028L)

typedef struct _QMSG {
    HWND   hwnd;
    ULONG  msg;
    MPARAM mp1;
    MPARAM mp2;
    ULONG  time;
    POINTL ptl;
    ULONG  reserved;
} QMSG, *PQMSG;

typedef struct _SWP {
    ULONG fl;
    LONG  cy;
    LONG  cx;
    LONG  y;
    LONG  x;
    HWND  hwndInsertBehind;
    HWND  hwnd;
    ULONG ulReserved1;
    ULONG ulReserved2;
} SWP, *PSWP;

typedef struct _FRAMECDATA {
    USHORT cb;
    ULONG  flCreateFlags;
    USHORT hmodResources;
    USHORT idResources;
} FRAMECDATA, *PFRAMECDATA;

typedef struct _CLASSINFO {
    ULONG flClassStyle;
    PFNWP pfnWindowProc;
    ULONG cbWindowData;
} CLASSINFO, *PCLASSINFO;

typedef struct _MENUITEM {
    SHORT  iPosition;
    USHORT afStyle;
    USHORT afAttribute;
    USHORT id;
    HWND   hwndSubMenu;
    ULONG  hItem;
} MENUITEM, *PMENUITEM;

/* WM_CHAR's parameters. */
#define CHARMSG(pmsg) ((struct _CHARMSG *)((PBYTE)(pmsg) + sizeof(MPARAM)))
struct _CHARMSG { USHORT fs; UCHAR cRepeat; UCHAR scancode; USHORT chr; USHORT vkey; };
/* WM_COMMAND's parameters. */
#define COMMANDMSG(pmsg) ((struct _COMMANDMSG *)((PBYTE)(pmsg) + sizeof(MPARAM)))
struct _COMMANDMSG { USHORT cmd; USHORT unused; USHORT source; USHORT fMouse; };
/* The mouse messages' parameters. */
#define MOUSEMSG(pmsg) ((struct _MOUSEMSG *)((PBYTE)(pmsg) + sizeof(MPARAM)))
struct _MOUSEMSG { SHORT x; SHORT y; USHORT codeHitTest; USHORT fsInp; };

/* A dialog template (from a resource or built in memory). */
typedef struct _DLGTEMPLATE *PDLGTEMPLATE;

/* Start-up, queues and messages */
HAB     APIENTRY WinInitialize(ULONG flOptions);
BOOL    APIENTRY WinTerminate(HAB hab);
HMQ     APIENTRY WinCreateMsgQueue(HAB hab, LONG cmsg);
BOOL    APIENTRY WinDestroyMsgQueue(HMQ hmq);
ULONG   APIENTRY WinQueryVersion(HAB hab);
BOOL    APIENTRY WinGetMsg(HAB hab, PQMSG pqmsg, HWND hwndFilter, ULONG msgFilterFirst, ULONG msgFilterLast);
BOOL    APIENTRY WinPeekMsg(HAB hab, PQMSG pqmsg, HWND hwndFilter, ULONG msgFilterFirst, ULONG msgFilterLast,
                            ULONG fl);
MRESULT APIENTRY WinDispatchMsg(HAB hab, PQMSG pqmsg);
BOOL    APIENTRY WinPostMsg(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2);
MRESULT APIENTRY WinSendMsg(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2);
BOOL    APIENTRY WinPostQueueMsg(HMQ hmq, ULONG msg, MPARAM mp1, MPARAM mp2);
BOOL    APIENTRY WinBroadcastMsg(HWND hwndParent, ULONG msg, MPARAM mp1, MPARAM mp2, ULONG rgf);
BOOL    APIENTRY WinWaitMsg(HAB hab, ULONG msgFirst, ULONG msgLast);
BOOL    APIENTRY WinInSendMsg(HAB hab);
ULONG   APIENTRY WinQueryQueueStatus(HWND hwndDesktop);
BOOL    APIENTRY WinQueryMsgPos(HAB hab, PPOINTL pptl);
ULONG   APIENTRY WinQueryMsgTime(HAB hab);
ULONG   APIENTRY WinGetCurrentTime(HAB hab);
ERRORID APIENTRY WinGetLastError(HAB hab);

/* Windows */
BOOL    APIENTRY WinRegisterClass(HAB hab, PCSZ pszClassName, PFNWP pfnWndProc, ULONG flStyle,
                                  ULONG cbWindowData);
BOOL    APIENTRY WinQueryClassInfo(HAB hab, PCSZ pszClassName, PCLASSINFO pClassInfo);
LONG    APIENTRY WinQueryClassName(HWND hwnd, LONG cchMax, PCH pch);
HWND    APIENTRY WinCreateStdWindow(HWND hwndParent, ULONG flStyle, PULONG pflCreateFlags,
                                    PCSZ pszClientClass, PCSZ pszTitle, ULONG styleClient, HMODULE hmod,
                                    ULONG idResources, PHWND phwndClient);
HWND    APIENTRY WinCreateWindow(HWND hwndParent, PCSZ pszClass, PCSZ pszName, ULONG flStyle, LONG x, LONG y,
                                 LONG cx, LONG cy, HWND hwndOwner, HWND hwndInsertBehind, ULONG id,
                                 PVOID pCtlData, PVOID pPresParams);
BOOL    APIENTRY WinCreateFrameControls(HWND hwndFrame, PFRAMECDATA pfcdata, PCSZ pszTitle);
BOOL    APIENTRY WinDestroyWindow(HWND hwnd);
MRESULT EXPENTRY WinDefWindowProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2);
PFNWP   APIENTRY WinSubclassWindow(HWND hwnd, PFNWP pfnwp);
BOOL    APIENTRY WinIsWindow(HAB hab, HWND hwnd);
BOOL    APIENTRY WinIsWindowVisible(HWND hwnd);
BOOL    APIENTRY WinIsWindowEnabled(HWND hwnd);
BOOL    APIENTRY WinIsWindowShowing(HWND hwnd);
BOOL    APIENTRY WinIsChild(HWND hwnd, HWND hwndParent);
BOOL    APIENTRY WinShowWindow(HWND hwnd, BOOL fShow);
BOOL    APIENTRY WinEnableWindow(HWND hwnd, BOOL fEnable);
BOOL    APIENTRY WinEnableWindowUpdate(HWND hwnd, BOOL fEnable);
BOOL    APIENTRY WinSetWindowPos(HWND hwnd, HWND hwndInsertBehind, LONG x, LONG y, LONG cx, LONG cy,
                                 ULONG fl);
BOOL    APIENTRY WinSetMultWindowPos(HAB hab, PSWP pswp, ULONG cswp);
BOOL    APIENTRY WinQueryWindowPos(HWND hwnd, PSWP pswp);
BOOL    APIENTRY WinQueryWindowRect(HWND hwnd, PRECTL prclDest);
BOOL    APIENTRY WinCalcFrameRect(HWND hwndFrame, PRECTL prcl, BOOL fClient);
BOOL    APIENTRY WinMapWindowPoints(HWND hwndFrom, HWND hwndTo, PPOINTL prgptl, LONG cwpt);
HWND    APIENTRY WinQueryWindow(HWND hwnd, LONG cmd);
HWND    APIENTRY WinWindowFromID(HWND hwndParent, ULONG id);
HWND    APIENTRY WinWindowFromPoint(HWND hwnd, PPOINTL pptl, BOOL fChildren);
BOOL    APIENTRY WinSetParent(HWND hwnd, HWND hwndNewParent, BOOL fRedraw);
BOOL    APIENTRY WinSetOwner(HWND hwnd, HWND hwndNewOwner);
HWND    APIENTRY WinQueryDesktopWindow(HAB hab, HDC hdc);
HAB     APIENTRY WinQueryAnchorBlock(HWND hwnd);
BOOL    APIENTRY WinQueryWindowProcess(HWND hwnd, PPID ppid, PTID ptid);
LONG    APIENTRY WinQueryWindowText(HWND hwnd, LONG cchBufferMax, PCH pchBuffer);
LONG    APIENTRY WinQueryWindowTextLength(HWND hwnd);
BOOL    APIENTRY WinSetWindowText(HWND hwnd, PCSZ pszText);
ULONG   APIENTRY WinQueryWindowULong(HWND hwnd, LONG index);
BOOL    APIENTRY WinSetWindowULong(HWND hwnd, LONG index, ULONG ul);
USHORT  APIENTRY WinQueryWindowUShort(HWND hwnd, LONG index);
BOOL    APIENTRY WinSetWindowUShort(HWND hwnd, LONG index, USHORT us);
PVOID   APIENTRY WinQueryWindowPtr(HWND hwnd, LONG index);
BOOL    APIENTRY WinSetWindowPtr(HWND hwnd, LONG index, PVOID p);
BOOL    APIENTRY WinSetWindowBits(HWND hwnd, LONG index, ULONG flData, ULONG flMask);
BOOL    APIENTRY WinSetPresParam(HWND hwnd, ULONG id, ULONG cbParam, PVOID pbParam);
ULONG   APIENTRY WinQueryPresParam(HWND hwnd, ULONG id1, ULONG id2, PULONG pulId, ULONG cbBuf, PVOID pbBuf,
                                   ULONG fs);
BOOL    APIENTRY WinRemovePresParam(HWND hwnd, ULONG id);
BOOL    APIENTRY WinFlashWindow(HWND hwndFrame, BOOL fFlash);
BOOL    APIENTRY WinLockWindowUpdate(HWND hwndDesktop, HWND hwndLockUpdate);

/* Focus, activation, capture, timers */
HWND    APIENTRY WinQueryActiveWindow(HWND hwndDesktop);
BOOL    APIENTRY WinSetActiveWindow(HWND hwndDesktop, HWND hwnd);
HWND    APIENTRY WinQueryFocus(HWND hwndDesktop);
BOOL    APIENTRY WinSetFocus(HWND hwndDesktop, HWND hwndSetFocus);
HWND    APIENTRY WinQueryCapture(HWND hwndDesktop);
BOOL    APIENTRY WinSetCapture(HWND hwndDesktop, HWND hwnd);
ULONG   APIENTRY WinStartTimer(HAB hab, HWND hwnd, ULONG idTimer, ULONG dtTimeout);
BOOL    APIENTRY WinStopTimer(HAB hab, HWND hwnd, ULONG ulTimer);
LONG    APIENTRY WinGetKeyState(HWND hwndDesktop, LONG vkey);
LONG    APIENTRY WinGetPhysKeyState(HWND hwndDesktop, LONG sc);

/* Painting */
HPS     APIENTRY WinBeginPaint(HWND hwnd, HPS hps, PRECTL prclPaint);
BOOL    APIENTRY WinEndPaint(HPS hps);
HPS     APIENTRY WinGetPS(HWND hwnd);
HPS     APIENTRY WinGetClipPS(HWND hwnd, HWND hwndClip, ULONG fl);
HPS     APIENTRY WinGetScreenPS(HWND hwndDesktop);
BOOL    APIENTRY WinReleasePS(HPS hps);
HDC     APIENTRY WinOpenWindowDC(HWND hwnd);
HDC     APIENTRY WinQueryWindowDC(HWND hwnd);
BOOL    APIENTRY WinInvalidateRect(HWND hwnd, PRECTL prcl, BOOL fIncludeChildren);
BOOL    APIENTRY WinInvalidateRegion(HWND hwnd, HRGN hrgn, BOOL fIncludeChildren);
BOOL    APIENTRY WinValidateRect(HWND hwnd, PRECTL prcl, BOOL fIncludeChildren);
BOOL    APIENTRY WinQueryUpdateRect(HWND hwnd, PRECTL prcl);
BOOL    APIENTRY WinUpdateWindow(HWND hwnd);
LONG    APIENTRY WinScrollWindow(HWND hwnd, LONG dx, LONG dy, PRECTL prclScroll, PRECTL prclClip,
                                 HRGN hrgnUpdate, PRECTL prclUpdate, ULONG rgfsw);
BOOL    APIENTRY WinFillRect(HPS hps, PRECTL prcl, LONG lColor);
BOOL    APIENTRY WinInvertRect(HPS hps, PRECTL prcl);
LONG    APIENTRY WinDrawText(HPS hps, LONG cchText, PCCH lpchText, PRECTL prcl, LONG clrFore, LONG clrBack,
                             ULONG flCmd);
BOOL    APIENTRY WinDrawBorder(HPS hps, PRECTL prcl, LONG cx, LONG cy, LONG clrFore, LONG clrBack,
                               ULONG flCmd);
BOOL    APIENTRY WinDrawBitmap(HPS hpsDst, HBITMAP hbm, PRECTL pwrcSrc, PPOINTL pptlDst, LONG clrFore,
                               LONG clrBack, ULONG fl);
BOOL    APIENTRY WinDrawPointer(HPS hps, LONG x, LONG y, HPOINTER hptr, ULONG fs);

/* Rectangles */
BOOL    APIENTRY WinSetRect(HAB hab, PRECTL prcl, LONG xLeft, LONG yBottom, LONG xRight, LONG yTop);
BOOL    APIENTRY WinSetRectEmpty(HAB hab, PRECTL prcl);
BOOL    APIENTRY WinCopyRect(HAB hab, PRECTL prclDst, PRECTL prclSrc);
BOOL    APIENTRY WinIsRectEmpty(HAB hab, PRECTL prcl);
BOOL    APIENTRY WinEqualRect(HAB hab, PRECTL prcl1, PRECTL prcl2);
BOOL    APIENTRY WinInflateRect(HAB hab, PRECTL prcl, LONG cx, LONG cy);
BOOL    APIENTRY WinOffsetRect(HAB hab, PRECTL prcl, LONG cx, LONG cy);
BOOL    APIENTRY WinIntersectRect(HAB hab, PRECTL prclDst, PRECTL prclSrc1, PRECTL prclSrc2);
BOOL    APIENTRY WinUnionRect(HAB hab, PRECTL prclDst, PRECTL prclSrc1, PRECTL prclSrc2);
BOOL    APIENTRY WinSubtractRect(HAB hab, PRECTL prclDst, PRECTL prclSrc1, PRECTL prclSrc2);
BOOL    APIENTRY WinPtInRect(HAB hab, PRECTL prcl, PPOINTL pptl);

/* Dialogs and message boxes */
ULONG   APIENTRY WinMessageBox(HWND hwndParent, HWND hwndOwner, PCSZ pszText, PCSZ pszCaption, ULONG idWindow,
                               ULONG flStyle);
ULONG   APIENTRY WinDlgBox(HWND hwndParent, HWND hwndOwner, PFNWP pfnDlgProc, HMODULE hmod, ULONG idDlg,
                           PVOID pCreateParams);
HWND    APIENTRY WinLoadDlg(HWND hwndParent, HWND hwndOwner, PFNWP pfnDlgProc, HMODULE hmod, ULONG idDlg,
                            PVOID pCreateParams);
HWND    APIENTRY WinCreateDlg(HWND hwndParent, HWND hwndOwner, PFNWP pfnDlgProc, PDLGTEMPLATE pdlgt,
                              PVOID pCreateParams);
ULONG   APIENTRY WinProcessDlg(HWND hwndDlg);
BOOL    APIENTRY WinDismissDlg(HWND hwndDlg, ULONG usResult);
MRESULT EXPENTRY WinDefDlgProc(HWND hwndDlg, ULONG msg, MPARAM mp1, MPARAM mp2);
MRESULT APIENTRY WinSendDlgItemMsg(HWND hwndDlg, ULONG idItem, ULONG msg, MPARAM mp1, MPARAM mp2);
BOOL    APIENTRY WinSetDlgItemText(HWND hwndDlg, ULONG idItem, PCSZ pszText);
ULONG   APIENTRY WinQueryDlgItemText(HWND hwndDlg, ULONG idItem, LONG lMaxText, PSZ pszText);
LONG    APIENTRY WinQueryDlgItemTextLength(HWND hwndDlg, ULONG idItem);
BOOL    APIENTRY WinSetDlgItemShort(HWND hwndDlg, ULONG idItem, USHORT usValue, BOOL fSigned);
BOOL    APIENTRY WinQueryDlgItemShort(HWND hwndDlg, ULONG idItem, PSHORT pResult, BOOL fSigned);
HWND    APIENTRY WinEnumDlgItem(HWND hwndDlg, HWND hwnd, ULONG code);
BOOL    APIENTRY WinMapDlgPoints(HWND hwndDlg, PPOINTL prgwptl, ULONG cwpt, BOOL fCalcWindowCoords);

/* Resources, menus, accelerators */
LONG    APIENTRY WinLoadString(HAB hab, HMODULE hmod, ULONG id, LONG cchMax, PSZ pchBuffer);
LONG    APIENTRY WinLoadMessage(HAB hab, HMODULE hmod, ULONG id, LONG cchMax, PSZ pchBuffer);
HWND    APIENTRY WinLoadMenu(HWND hwndFrame, HMODULE hmod, ULONG idMenu);
HWND    APIENTRY WinCreateMenu(HWND hwndParent, PVOID lpmt);
BOOL    APIENTRY WinPopupMenu(HWND hwndParent, HWND hwndOwner, HWND hwndMenu, LONG x, LONG y, LONG idItem,
                              ULONG fs);
HACCEL  APIENTRY WinLoadAccelTable(HAB hab, HMODULE hmod, ULONG idAccelTable);
BOOL    APIENTRY WinSetAccelTable(HAB hab, HACCEL haccel, HWND hwndFrame);
HACCEL  APIENTRY WinQueryAccelTable(HAB hab, HWND hwndFrame);
BOOL    APIENTRY WinDestroyAccelTable(HACCEL haccel);

/* System values, colours, pointers, the cursor */
LONG     APIENTRY WinQuerySysValue(HWND hwndDesktop, LONG iSysValue);
BOOL     APIENTRY WinSetSysValue(HWND hwndDesktop, LONG iSysValue, LONG lValue);
LONG     APIENTRY WinQuerySysColor(HWND hwndDesktop, LONG clr, LONG lReserved);
BOOL     APIENTRY WinAlarm(HWND hwndDesktop, ULONG rgfType);
HPOINTER APIENTRY WinQuerySysPointer(HWND hwndDesktop, LONG iptr, BOOL fLoad);
HPOINTER APIENTRY WinLoadPointer(HWND hwndDesktop, HMODULE hmod, ULONG idres);
BOOL     APIENTRY WinDestroyPointer(HPOINTER hptr);
BOOL     APIENTRY WinSetPointer(HWND hwndDesktop, HPOINTER hptrNew);
HPOINTER APIENTRY WinQueryPointer(HWND hwndDesktop);
BOOL     APIENTRY WinQueryPointerPos(HWND hwndDesktop, PPOINTL pptl);
BOOL     APIENTRY WinSetPointerPos(HWND hwndDesktop, LONG x, LONG y);
BOOL     APIENTRY WinShowPointer(HWND hwndDesktop, BOOL fShow);
BOOL     APIENTRY WinCreateCursor(HWND hwnd, LONG x, LONG y, LONG cx, LONG cy, ULONG fs, PRECTL prclClip);
BOOL     APIENTRY WinDestroyCursor(HWND hwnd);
BOOL     APIENTRY WinShowCursor(HWND hwnd, BOOL fShow);

/* The clipboard */
BOOL    APIENTRY WinOpenClipbrd(HAB hab);
BOOL    APIENTRY WinCloseClipbrd(HAB hab);
BOOL    APIENTRY WinEmptyClipbrd(HAB hab);
BOOL    APIENTRY WinSetClipbrdData(HAB hab, ULONG ulData, ULONG fmt, ULONG rgfFmtInfo);
ULONG   APIENTRY WinQueryClipbrdData(HAB hab, ULONG fmt);
BOOL    APIENTRY WinQueryClipbrdFmtInfo(HAB hab, ULONG fmt, PULONG prgfFmtInfo);
ULONG   APIENTRY WinEnumClipbrdFmts(HAB hab, ULONG fmt);

/* Atoms */
HATOMTABLE APIENTRY WinQuerySystemAtomTable(void);
HATOMTABLE APIENTRY WinCreateAtomTable(ULONG cbInitial, ULONG cBuckets);
HATOMTABLE APIENTRY WinDestroyAtomTable(HATOMTABLE hAtomTbl);
ATOM    APIENTRY WinAddAtom(HATOMTABLE hAtomTbl, PCSZ pszAtomName);
ATOM    APIENTRY WinFindAtom(HATOMTABLE hAtomTbl, PCSZ pszAtomName);
ATOM    APIENTRY WinDeleteAtom(HATOMTABLE hAtomTbl, ATOM atom);
ULONG   APIENTRY WinQueryAtomName(HATOMTABLE hAtomTbl, ATOM atom, PSZ pchBuffer, ULONG cchBufferMax);
ULONG   APIENTRY WinQueryAtomLength(HATOMTABLE hAtomTbl, ATOM atom);
ULONG   APIENTRY WinQueryAtomUsage(HATOMTABLE hAtomTbl, ATOM atom);

/* Codepages and text */
ULONG   APIENTRY WinQueryCp(HMQ hmq);
BOOL    APIENTRY WinSetCp(HMQ hmq, ULONG idCodePage);
ULONG   APIENTRY WinUpper(HAB hab, ULONG idcp, ULONG idcc, PSZ psz);
ULONG   APIENTRY WinUpperChar(HAB hab, ULONG idcp, ULONG idcc, ULONG c);

/* Semaphore waits that keep a message queue serviced */
APIRET  APIENTRY WinWaitEventSem(HEV hev, ULONG ulTimeout);
APIRET  APIENTRY WinRequestMutexSem(HMTX hmtx, ULONG ulTimeout);
APIRET  APIENTRY WinWaitMuxWaitSem(HMUX hmux, ULONG ulTimeout, PULONG pulUser);

/* Enumerating windows */
typedef LHANDLE HENUM;
HENUM   APIENTRY WinBeginEnumWindows(HWND hwnd);
HWND    APIENTRY WinGetNextWindow(HENUM henum);
BOOL    APIENTRY WinEndEnumWindows(HENUM henum);
LONG    APIENTRY WinMultWindowFromIDs(HWND hwndParent, PHWND prghwnd, ULONG idFirst, ULONG idLast);
BOOL    APIENTRY WinGetMaxPosition(HWND hwnd, PSWP pswp);
BOOL    APIENTRY WinGetMinPosition(HWND hwnd, PSWP pswp, PPOINTL pptl);
LONG    APIENTRY WinQueryUpdateRegion(HWND hwnd, HRGN hrgn);
LONG    APIENTRY WinExcludeUpdateRegion(HPS hps, HWND hwnd);
BOOL    APIENTRY WinValidateRegion(HWND hwnd, HRGN hrgn, BOOL fIncludeChildren);
BOOL    APIENTRY WinShowTrackRect(HWND hwnd, BOOL fShow);

/* Accelerator tables */
#define AF_CHAR        0x0001
#define AF_VIRTUALKEY  0x0002
#define AF_SCANCODE    0x0004
#define AF_SHIFT       0x0008
#define AF_CONTROL     0x0010
#define AF_ALT         0x0020
#define AF_LONEKEY     0x0040
#define AF_SYSCOMMAND  0x0100
#define AF_HELP        0x0200

typedef struct _ACCEL {
    USHORT fs;
    USHORT key;
    USHORT cmd;
} ACCEL, *PACCEL;

typedef struct _ACCELTABLE {
    USHORT cAccel;
    USHORT codepage;
    ACCEL  aaccel[1];
} ACCELTABLE, *PACCELTABLE;

HACCEL  APIENTRY WinCreateAccelTable(HAB hab, PACCELTABLE pAccelTable);
ULONG   APIENTRY WinCopyAccelTable(HACCEL haccel, PACCELTABLE pAccelTable, ULONG cbCopyMax);
BOOL    APIENTRY WinTranslateAccel(HAB hab, HWND hwnd, HACCEL haccel, PQMSG pqmsg);

/* Message filtering and hooks */
#define HK_SENDMSG          0
#define HK_INPUT            1
#define HK_MSGFILTER        2
#define HK_JOURNALRECORD    3
#define HK_JOURNALPLAYBACK  4
#define HK_HELP             5
#define HK_LOADER           6
#define HK_REGISTERUSERMSG  7
#define HK_MSGCONTROL       8
#define HK_FINDWORD         11
#define HK_CODEPAGECHANGED  12
#define HK_WINDOWDC         15
#define HK_DESTROYWINDOW    16
#define HK_CHECKMSGFILTER   20
#define HK_MSGINPUT         21
#define HK_LOCKUP           23
#define HK_FLUSHBUF         24

BOOL    APIENTRY WinSetHook(HAB hab, HMQ hmq, LONG iHook, PFN pfnHook, HMODULE hmod);
BOOL    APIENTRY WinReleaseHook(HAB hab, HMQ hmq, LONG iHook, PFN pfnHook, HMODULE hmod);
BOOL    APIENTRY WinCallMsgFilter(HAB hab, PQMSG pqmsg, ULONG msgf);
BOOL    APIENTRY WinGetDlgMsg(HWND hwndDlg, PQMSG pqmsg);
BOOL    APIENTRY WinCheckInput(HAB hab);
BOOL    APIENTRY WinIsThreadActive(HAB hab);

/* Error information */
typedef struct _ERRINFO {
    ULONG   cbFixedErrInfo;
    ERRORID idError;
    ULONG   cDetailLevel;
    ULONG   offaoffszMsg;
    ULONG   offBinaryData;
} ERRINFO, *PERRINFO;

PERRINFO APIENTRY WinGetErrorInfo(HAB hab);
BOOL     APIENTRY WinFreeErrorInfo(PERRINFO perrinfo);

/* The clipboard's owner and viewer */
HWND    APIENTRY WinQueryClipbrdOwner(HAB hab);
BOOL    APIENTRY WinSetClipbrdOwner(HAB hab, HWND hwnd);
HWND    APIENTRY WinQueryClipbrdViewer(HAB hab);
BOOL    APIENTRY WinSetClipbrdViewer(HAB hab, HWND hwndNewClipViewer);

/* Codepage-aware text */
#define WCS_ERROR  0
#define WCS_EQ     1
#define WCS_LT     2
#define WCS_GT     3
ULONG   APIENTRY WinCompareStrings(HAB hab, ULONG idcp, ULONG idcc, PCSZ psz1, PCSZ psz2, ULONG reserved);
ULONG   APIENTRY WinQueryCpList(HAB hab, ULONG ccpMax, PULONG prgcp);
BOOL    APIENTRY WinCpTranslateString(HAB hab, ULONG cpSrc, PCSZ pszSrc, ULONG cpDst, ULONG cchDestMax,
                                      PSZ pchDest);
PSZ     APIENTRY WinNextChar(HAB hab, ULONG idcp, ULONG idcc, PCSZ psz);
PSZ     APIENTRY WinPrevChar(HAB hab, ULONG idcp, ULONG idcc, PCSZ pszStart, PCSZ psz);

/* Cursor and pointer information */
typedef struct _CURSORINFO {
    HWND  hwnd;
    LONG  x;
    LONG  y;
    LONG  cx;
    LONG  cy;
    ULONG fs;
    RECTL rclClip;
} CURSORINFO, *PCURSORINFO;

typedef struct _POINTERINFO {
    ULONG   fPointer;
    LONG    xHotspot;
    LONG    yHotspot;
    HBITMAP hbmPointer;
    HBITMAP hbmColor;
    HBITMAP hbmMiniPointer;
    HBITMAP hbmMiniColor;
} POINTERINFO, *PPOINTERINFO;

BOOL     APIENTRY WinQueryCursorInfo(HWND hwndDesktop, PCURSORINFO pCursorInfo);
BOOL     APIENTRY WinQueryPointerInfo(HPOINTER hptr, PPOINTERINFO pPointerInfo);
HPOINTER APIENTRY WinCreatePointer(HWND hwndDesktop, HBITMAP hbmPointer, BOOL fPointer, LONG xHotspot,
                                   LONG yHotspot);
HPOINTER APIENTRY WinCreatePointerIndirect(HWND hwndDesktop, PPOINTERINFO pptri);

#endif /* INCL_WIN */

/* ═══ Graphics (PMGPI) ═════════════════════════════════════════════════ */
#if defined(INCL_GPI) || defined(INCL_PM) || !defined(WARPINE_OS2_H)

/* Results */
#define GPI_ERROR     0
#define GPI_OK        1
#define GPI_ALTERROR  (-1)

/* GpiCreatePS */
#define PU_ARBITRARY  0x0004
#define PU_PELS       0x0008
#define PU_LOMETRIC   0x000C
#define PU_HIMETRIC   0x0010
#define PU_LOENGLISH  0x0014
#define PU_HIENGLISH  0x0018
#define PU_TWIPS      0x001C
#define GPIF_DEFAULT  0x0000
#define GPIF_SHORT    0x0100
#define GPIF_LONG     0x0200
#define GPIT_NORMAL   0x0000
#define GPIT_MICRO    0x1000
#define GPIA_NOASSOC  0x0000
#define GPIA_ASSOC    0x4000

/* Mixes (FM_ foreground, BM_ background) */
#define FM_DEFAULT          0
#define FM_OR               1
#define FM_OVERPAINT        2
#define FM_XOR              4
#define FM_LEAVEALONE       5
#define FM_AND              6
#define FM_SUBTRACT         7
#define FM_MASKSRCNOT       8
#define FM_ZERO             9
#define FM_NOTMERGESRC      10
#define FM_NOTXORSRC        11
#define FM_INVERT           12
#define FM_MERGESRCNOT      13
#define FM_NOTCOPYSRC       14
#define FM_MERGENOTSRC      15
#define FM_NOTMASKSRC       16
#define FM_ONE              17
#define BM_DEFAULT          0
#define BM_OR               1
#define BM_OVERPAINT        2
#define BM_LEAVEALONE       5
#define BM_SRCTRANSPARENT   18
#define BM_DESTTRANSPARENT  19

/* Line types */
#define LINETYPE_ERROR          (-1)
#define LINETYPE_DEFAULT        0
#define LINETYPE_DOT            1
#define LINETYPE_SHORTDASH      2
#define LINETYPE_DASHDOT        3
#define LINETYPE_DOUBLEDOT      4
#define LINETYPE_LONGDASH       5
#define LINETYPE_DASHDOUBLEDOT  6
#define LINETYPE_SOLID          7
#define LINETYPE_INVISIBLE      8
#define LINETYPE_ALTERNATE      9

/* Patterns */
#define PATSYM_ERROR      (-1)
#define PATSYM_DEFAULT    0
#define PATSYM_DENSE1     1
#define PATSYM_DENSE8     8
#define PATSYM_VERT       9
#define PATSYM_HORIZ      10
#define PATSYM_DIAG1      11
#define PATSYM_DIAG2      12
#define PATSYM_DIAG3      13
#define PATSYM_DIAG4      14
#define PATSYM_NOSHADE    15
#define PATSYM_SOLID      16
#define PATSYM_HALFTONE   17
#define PATSYM_HATCH      18
#define PATSYM_DIAGHATCH  19
#define PATSYM_BLANK      64

/* GpiCharStringPos */
#define CHS_OPAQUE      0x0001
#define CHS_VECTOR      0x0002
#define CHS_LEAVEPOS    0x0008
#define CHS_CLIP        0x0010
#define CHS_UNDERSCORE  0x0200
#define CHS_STRIKEOUT   0x0400

/* GpiBox / GpiFullArc */
#define DRO_FILL         1
#define DRO_OUTLINE      2
#define DRO_OUTLINEFILL  3

/* Fonts */
#define FATTR_SEL_ITALIC       0x0001
#define FATTR_SEL_UNDERSCORE   0x0002
#define FATTR_SEL_OUTLINE      0x0008
#define FATTR_SEL_STRIKEOUT    0x0010
#define FATTR_SEL_BOLD         0x0020
#define FATTR_FONTUSE_OUTLINE  0x0004
#define LCID_DEFAULT           0x0000
#define FACESIZE               0x0020

/* Raster operations */
#define ROP_ZERO        0x0000
#define ROP_NOTSRCCOPY  0x0033
#define ROP_DSTINVERT   0x0055
#define ROP_PATINVERT   0x005A
#define ROP_SRCINVERT   0x0066
#define ROP_SRCAND      0x0088
#define ROP_SRCCOPY     0x00CC
#define ROP_SRCPAINT    0x00EE
#define ROP_PATCOPY     0x00F0
#define ROP_ONE         0x00FF
#define BBO_OR          0x0000
#define BBO_AND         0x0001
#define BBO_IGNORE      0x0002

typedef struct _SIZEF { FIXED cx; FIXED cy; } SIZEF, *PSIZEF;
typedef struct _GRADIENTL { LONG x; LONG y; } GRADIENTL, *PGRADIENTL;
typedef struct _ARCPARAMS { LONG lP; LONG lQ; LONG lR; LONG lS; } ARCPARAMS, *PARCPARAMS;

typedef struct _FATTRS {
    USHORT usRecordLength;
    USHORT fsSelection;
    LONG   lMatch;
    CHAR   szFacename[FACESIZE];
    USHORT idRegistry;
    USHORT usCodePage;
    LONG   lMaxBaselineExt;
    LONG   lAveCharWidth;
    USHORT fsType;
    USHORT fsFontUse;
} FATTRS, *PFATTRS;

/* The OS/2 2.x layout (208 bytes); pass its size to GpiQueryFontMetrics. */
typedef struct _FONTMETRICS {
    CHAR   szFamilyname[FACESIZE];
    CHAR   szFacename[FACESIZE];
    USHORT idRegistry;
    USHORT usCodePage;
    LONG   lEmHeight;
    LONG   lXHeight;
    LONG   lMaxAscender;
    LONG   lMaxDescender;
    LONG   lLowerCaseAscent;
    LONG   lLowerCaseDescent;
    LONG   lInternalLeading;
    LONG   lExternalLeading;
    LONG   lAveCharWidth;
    LONG   lMaxCharInc;
    LONG   lEmInc;
    LONG   lMaxBaselineExt;
    SHORT  sCharSlope;
    SHORT  sInlineDir;
    SHORT  sCharRot;
    USHORT usWeightClass;
    USHORT usWidthClass;
    SHORT  sXDeviceRes;
    SHORT  sYDeviceRes;
    SHORT  sFirstChar;
    SHORT  sLastChar;
    SHORT  sDefaultChar;
    SHORT  sBreakChar;
    SHORT  sNominalPointSize;
    SHORT  sMinimumPointSize;
    SHORT  sMaximumPointSize;
    USHORT fsType;
    USHORT fsDefn;
    USHORT fsSelection;
    USHORT fsCapabilities;
    LONG   lSubscriptXSize;
    LONG   lSubscriptYSize;
    LONG   lSubscriptXOffset;
    LONG   lSubscriptYOffset;
    LONG   lSuperscriptXSize;
    LONG   lSuperscriptYSize;
    LONG   lSuperscriptXOffset;
    LONG   lSuperscriptYOffset;
    LONG   lUnderscoreSize;
    LONG   lUnderscorePosition;
    LONG   lStrikeoutSize;
    LONG   lStrikeoutPosition;
    SHORT  sKerningPairs;
    SHORT  sFamilyClass;
    LONG   lMatch;
} FONTMETRICS, *PFONTMETRICS;

/* Device contexts are opened with data that is not declared yet. */
typedef struct _DEVOPENSTRUC *PDEVOPENDATA;

/* Presentation spaces */
HPS     APIENTRY GpiCreatePS(HAB hab, HDC hdc, PSIZEL psizlSize, ULONG flOptions);
BOOL    APIENTRY GpiDestroyPS(HPS hps);
BOOL    APIENTRY GpiAssociate(HPS hps, HDC hdc);
LONG    APIENTRY GpiSavePS(HPS hps);
BOOL    APIENTRY GpiRestorePS(HPS hps, LONG lPSid);
BOOL    APIENTRY GpiErase(HPS hps);
HDC     APIENTRY DevOpenDC(HAB hab, LONG lType, PCSZ pszToken, LONG lCount, PDEVOPENDATA pdopData,
                           HDC hdcComp);
HMF     APIENTRY DevCloseDC(HDC hdc);
BOOL    APIENTRY DevQueryCaps(HDC hdc, LONG lStart, LONG lCount, PLONG alArray);

/* Attributes */
BOOL    APIENTRY GpiSetColor(HPS hps, LONG lColor);
LONG    APIENTRY GpiQueryColor(HPS hps);
BOOL    APIENTRY GpiSetBackColor(HPS hps, LONG lColor);
LONG    APIENTRY GpiQueryBackColor(HPS hps);
BOOL    APIENTRY GpiSetMix(HPS hps, LONG lMixMode);
LONG    APIENTRY GpiQueryMix(HPS hps);
BOOL    APIENTRY GpiSetBackMix(HPS hps, LONG lMixMode);
LONG    APIENTRY GpiQueryBackMix(HPS hps);
BOOL    APIENTRY GpiSetLineType(HPS hps, LONG lLineType);
LONG    APIENTRY GpiQueryLineType(HPS hps);
BOOL    APIENTRY GpiSetLineWidth(HPS hps, FIXED fxLineWidth);
FIXED   APIENTRY GpiQueryLineWidth(HPS hps);
BOOL    APIENTRY GpiSetPattern(HPS hps, LONG lPatternSymbol);
LONG    APIENTRY GpiQueryPattern(HPS hps);
LONG    APIENTRY GpiQueryRGBColor(HPS hps, ULONG flOptions, LONG lColorIndex);
BOOL    APIENTRY GpiCreateLogColorTable(HPS hps, ULONG flOptions, LONG lFormat, LONG lStart, LONG lCount,
                                        PLONG alTable);

/* Lines, boxes, arcs, areas, paths */
BOOL    APIENTRY GpiMove(HPS hps, PPOINTL pptlPoint);
LONG    APIENTRY GpiLine(HPS hps, PPOINTL pptlEndPoint);
LONG    APIENTRY GpiPolyLine(HPS hps, LONG lCount, PPOINTL aptlPoints);
LONG    APIENTRY GpiBox(HPS hps, LONG lControl, PPOINTL pptlPoint, LONG lHRound, LONG lVRound);
BOOL    APIENTRY GpiQueryCurrentPosition(HPS hps, PPOINTL pptlPoint);
BOOL    APIENTRY GpiSetArcParams(HPS hps, PARCPARAMS parcpArcParams);
BOOL    APIENTRY GpiQueryArcParams(HPS hps, PARCPARAMS parcpArcParams);
LONG    APIENTRY GpiFullArc(HPS hps, LONG lControl, FIXED fxMultiplier);
LONG    APIENTRY GpiPointArc(HPS hps, PPOINTL pptl2);
LONG    APIENTRY GpiPartialArc(HPS hps, PPOINTL pptlCenter, FIXED fxMultiplier, FIXED fxStartAngle,
                               FIXED fxSweepAngle);
BOOL    APIENTRY GpiBeginArea(HPS hps, ULONG flOptions);
LONG    APIENTRY GpiEndArea(HPS hps);
BOOL    APIENTRY GpiBeginPath(HPS hps, LONG lPath);
BOOL    APIENTRY GpiEndPath(HPS hps);
BOOL    APIENTRY GpiCloseFigure(HPS hps);
LONG    APIENTRY GpiFillPath(HPS hps, LONG lPath, LONG lOptions);
LONG    APIENTRY GpiStrokePath(HPS hps, LONG lPath, ULONG flOptions);
LONG    APIENTRY GpiSetPel(HPS hps, PPOINTL pptlPoint);
LONG    APIENTRY GpiQueryPel(HPS hps, PPOINTL pptlPoint);

/* Text and fonts */
LONG    APIENTRY GpiCharString(HPS hps, LONG lCount, PCCH pchString);
LONG    APIENTRY GpiCharStringAt(HPS hps, PPOINTL pptlPoint, LONG lCount, PCCH pchString);
LONG    APIENTRY GpiCharStringPosAt(HPS hps, PPOINTL pptlStart, PRECTL prclRect, ULONG flOptions,
                                    LONG lCount, PCCH pchString, PLONG alAdx);
BOOL    APIENTRY GpiQueryTextBox(HPS hps, LONG lCount1, PCH pchString, LONG lCount2, PPOINTL aptlPoints);
LONG    APIENTRY GpiCreateLogFont(HPS hps, PSTR8 pName, LONG lLcid, PFATTRS pfatAttrs);
BOOL    APIENTRY GpiSetCharSet(HPS hps, LONG llcid);
BOOL    APIENTRY GpiDeleteSetId(HPS hps, LONG lLcid);
BOOL    APIENTRY GpiQueryFontMetrics(HPS hps, LONG lMetricsLength, PFONTMETRICS pfmMetrics);
BOOL    APIENTRY GpiSetCharBox(HPS hps, PSIZEF psizfxBox);
BOOL    APIENTRY GpiSetCharAngle(HPS hps, PGRADIENTL pgradlAngle);
BOOL    APIENTRY GpiSetCharMode(HPS hps, LONG lMode);
ULONG   APIENTRY GpiQueryCp(HPS hps);
BOOL    APIENTRY GpiSetCp(HPS hps, ULONG ulCodePage);

/* Bitmaps and regions */
HBITMAP APIENTRY GpiLoadBitmap(HPS hps, HMODULE hmod, ULONG id, LONG lWidth, LONG lHeight);
BOOL    APIENTRY GpiDeleteBitmap(HBITMAP hbm);
HBITMAP APIENTRY GpiSetBitmap(HPS hps, HBITMAP hbm);
LONG    APIENTRY GpiBitBlt(HPS hpsTarget, HPS hpsSource, LONG lCount, PPOINTL aptlPoints, LONG lRop,
                           ULONG flOptions);
HRGN    APIENTRY GpiCreateRegion(HPS hps, LONG lCount, PRECTL arclRectangles);
BOOL    APIENTRY GpiDestroyRegion(HPS hps, HRGN hrgn);
LONG    APIENTRY GpiPaintRegion(HPS hps, HRGN hrgn);
LONG    APIENTRY GpiSetClipRegion(HPS hps, HRGN hrgn, PHRGN phrgnOld);
LONG    APIENTRY GpiQueryClipBox(HPS hps, PRECTL prclBound);
LONG    APIENTRY GpiIntersectClipRectangle(HPS hps, PRECTL prclRectangle);
LONG    APIENTRY GpiExcludeClipRectangle(HPS hps, PRECTL prclRectangle);

/* Regions */
#define CRGN_OR      1
#define CRGN_COPY    2
#define CRGN_XOR     4
#define CRGN_AND     6
#define CRGN_DIFF    7
#define RGN_ERROR    0
#define RGN_NULL     1
#define RGN_RECT     2
#define RGN_COMPLEX  3
#define PRGN_ERROR   0
#define PRGN_OUTSIDE 1
#define PRGN_INSIDE  2
#define RRGN_ERROR   0
#define RRGN_OUTSIDE 1
#define RRGN_PARTIAL 2
#define RRGN_INSIDE  3
#define EQRGN_ERROR    0
#define EQRGN_NOTEQUAL 1
#define EQRGN_EQUAL    2

typedef struct _RGNRECT {
    ULONG ircStart;
    ULONG crc;
    ULONG crcReturned;
    ULONG ulDirection;
} RGNRECT, *PRGNRECT;

LONG    APIENTRY GpiCombineRegion(HPS hps, HRGN hrgnDest, HRGN hrgnSrc1, HRGN hrgnSrc2, LONG lMode);
LONG    APIENTRY GpiEqualRegion(HPS hps, HRGN hrgnSrc1, HRGN hrgnSrc2);
LONG    APIENTRY GpiPtInRegion(HPS hps, HRGN hrgn, PPOINTL pptlPoint);
LONG    APIENTRY GpiRectInRegion(HPS hps, HRGN hrgn, PRECTL prclRect);
LONG    APIENTRY GpiQueryRegionBox(HPS hps, HRGN hrgn, PRECTL prclBound);
BOOL    APIENTRY GpiQueryRegionRects(HPS hps, HRGN hrgn, PRECTL prclBound, PRGNRECT prgnrcControl,
                                     PRECTL prclRect);
BOOL    APIENTRY GpiOffsetRegion(HPS hps, HRGN hrgn, PPOINTL pptlOffset);
BOOL    APIENTRY GpiSetRegion(HPS hps, HRGN hrgn, LONG lcount, PRECTL arclRectangles);
LONG    APIENTRY GpiFrameRegion(HPS hps, HRGN hrgn, PSIZEL thickness);
HRGN    APIENTRY GpiQueryClipRegion(HPS hps);
LONG    APIENTRY GpiOffsetClipRegion(HPS hps, PPOINTL pptlPoint);

/* Paths and curves */
#define MPATH_STROKE   6
#define FPATH_ALTERNATE 0
#define FPATH_WINDING  2
#define FPATH_INCL     0
#define FPATH_EXCL     8
#define SCP_ALTERNATE  0
#define SCP_WINDING    2
#define SCP_AND        4
#define SCP_RESET      0
#define SCP_INCL       0
#define SCP_EXCL       8
HRGN    APIENTRY GpiPathToRegion(HPS hps, LONG lPath, LONG lOptions);
LONG    APIENTRY GpiOutlinePath(HPS hps, LONG lPath, LONG lOptions);
BOOL    APIENTRY GpiModifyPath(HPS hps, LONG lPath, LONG lMode);
BOOL    APIENTRY GpiSetClipPath(HPS hps, LONG lPath, LONG lOptions);
LONG    APIENTRY GpiPolyFillet(HPS hps, LONG lCount, PPOINTL aptlPoints);
LONG    APIENTRY GpiPolyFilletSharp(HPS hps, LONG lCount, PPOINTL aptlPoints, PFIXED afxPoints);
LONG    APIENTRY GpiPolySpline(HPS hps, LONG lCount, PPOINTL aptlPoints);

typedef struct _POLYGON {
    ULONG   ulPoints;
    PPOINTL aPointl;
} POLYGON, *PPOLYGON;
LONG    APIENTRY GpiPolygons(HPS hps, ULONG ulCount, PPOLYGON paplgn, ULONG flOptions, ULONG flModel);

/* Transforms */
#define TRANSFORM_REPLACE  0
#define TRANSFORM_ADD      1
#define TRANSFORM_PREEMPT  2
#define CVTC_WORLD         1
#define CVTC_MODEL         2
#define CVTC_DEFAULTPAGE   3
#define CVTC_PAGE          4
#define CVTC_DEVICE        5

typedef struct _MATRIXLF {
    FIXED fxM11;
    FIXED fxM12;
    LONG  lM13;
    FIXED fxM21;
    FIXED fxM22;
    LONG  lM23;
    LONG  lM31;
    LONG  lM32;
    LONG  lM33;
} MATRIXLF, *PMATRIXLF;

BOOL    APIENTRY GpiSetModelTransformMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray, LONG lOptions);
BOOL    APIENTRY GpiQueryModelTransformMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray);
BOOL    APIENTRY GpiSetDefaultViewMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfarray, LONG lOptions);
BOOL    APIENTRY GpiQueryDefaultViewMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray);
BOOL    APIENTRY GpiSetViewingTransformMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray, LONG lOptions);
BOOL    APIENTRY GpiQueryViewingTransformMatrix(HPS hps, LONG lCount, PMATRIXLF pmatlfArray);
BOOL    APIENTRY GpiTranslate(HPS hps, PMATRIXLF pmatrixlf, LONG lOptions, PPOINTL pptlTranslation);
BOOL    APIENTRY GpiScale(HPS hps, PMATRIXLF pmfatrixlf, LONG lOptions, PFIXED afxScale, PPOINTL pptlCenter);
BOOL    APIENTRY GpiRotate(HPS hps, PMATRIXLF pmatrixlf, LONG lOptions, FIXED fxAngle, PPOINTL pptlCenter);
BOOL    APIENTRY GpiConvert(HPS hps, LONG lSrc, LONG lTarg, LONG lCount, PPOINTL aptlPoints);
BOOL    APIENTRY GpiConvertWithMatrix(HPS hps, LONG lCountp, PPOINTL aptlPoints, LONG lCount,
                                      PMATRIXLF pmatlfArray);
BOOL    APIENTRY GpiSetPageViewport(HPS hps, PRECTL prclViewport);
BOOL    APIENTRY GpiQueryPageViewport(HPS hps, PRECTL prclViewport);
BOOL    APIENTRY GpiSetGraphicsField(HPS hps, PRECTL prclField);
BOOL    APIENTRY GpiQueryGraphicsField(HPS hps, PRECTL prclField);
BOOL    APIENTRY GpiSetViewingLimits(HPS hps, PRECTL prclLimits);
BOOL    APIENTRY GpiQueryViewingLimits(HPS hps, PRECTL prclLimits);
BOOL    APIENTRY GpiResetPS(HPS hps, ULONG flOptions);
ULONG   APIENTRY GpiQueryPS(HPS hps, PSIZEL psizlSize);
BOOL    APIENTRY GpiSetPS(HPS hps, PSIZEL psizlsize, ULONG flOptions);

/* More attributes */
#define CHDIRN_DEFAULT     0
#define CHDIRN_LEFTRIGHT   1
#define CHDIRN_TOPBOTTOM   2
#define CHDIRN_RIGHTLEFT   3
#define CHDIRN_BOTTOMTOP   4
#define TA_NORMAL_HORIZ    0x0001
#define TA_LEFT            0x0002
#define TA_CENTER          0x0003
#define TA_RIGHT           0x0004
#define TA_STANDARD_HORIZ  0x0005
#define TA_NORMAL_VERT     0x0100
#define TA_TOP             0x0200
#define TA_HALF            0x0300
#define TA_BASE            0x0400
#define TA_BOTTOM          0x0500
#define TA_STANDARD_VERT   0x0600
#define LCOL_RESET         0x0001
#define LCOL_PURECOLOR     0x0004
#define LCOLF_DEFAULT      0
#define LCOLF_INDRGB       1
#define LCOLF_CONSECRGB    2
#define LCOLF_RGB          3

BOOL    APIENTRY GpiQueryCharAngle(HPS hps, PGRADIENTL pgradlAngle);
BOOL    APIENTRY GpiSetCharDirection(HPS hps, LONG lDirection);
LONG    APIENTRY GpiQueryCharDirection(HPS hps);
LONG    APIENTRY GpiQueryCharMode(HPS hps);
BOOL    APIENTRY GpiSetCharShear(HPS hps, PPOINTL pptlAngle);
BOOL    APIENTRY GpiQueryCharShear(HPS hps, PPOINTL pptlShear);
BOOL    APIENTRY GpiSetCharExtra(HPS hps, FIXED Extra);
BOOL    APIENTRY GpiQueryCharExtra(HPS hps, PFIXED Extra);
BOOL    APIENTRY GpiSetCharBreakExtra(HPS hps, FIXED BreakExtra);
BOOL    APIENTRY GpiQueryCharBreakExtra(HPS hps, PFIXED BreakExtra);
BOOL    APIENTRY GpiSetTextAlignment(HPS hps, LONG lHoriz, LONG lVert);
BOOL    APIENTRY GpiQueryTextAlignment(HPS hps, PLONG plHoriz, PLONG plVert);
BOOL    APIENTRY GpiSetPatternRefPoint(HPS hps, PPOINTL pptlRefPoint);
BOOL    APIENTRY GpiQueryPatternRefPoint(HPS hps, PPOINTL pptlRefPoint);
BOOL    APIENTRY GpiSetLineWidthGeom(HPS hps, LONG lLineWidth);
LONG    APIENTRY GpiQueryLineWidthGeom(HPS hps);
LONG    APIENTRY GpiQueryColorIndex(HPS hps, ULONG flOptions, LONG lRgbColor);
LONG    APIENTRY GpiQueryLogColorTable(HPS hps, ULONG flOptions, LONG lStart, LONG lCount, PLONG alArray);

/* Text positions and fonts */
#define QF_PUBLIC   0x0001
#define QF_PRIVATE  0x0002
BOOL    APIENTRY GpiQueryWidthTable(HPS hps, LONG lFirstChar, LONG lCount, PLONG alData);
LONG    APIENTRY GpiCharStringPos(HPS hps, PRECTL prclRect, ULONG flOptions, LONG lCount, PCCH pchString,
                                  PLONG alAdx);
BOOL    APIENTRY GpiQueryCharStringPos(HPS hps, ULONG flOptions, LONG lCount, PCCH pchString,
                                       PLONG alXincrements, PPOINTL aptlPositions);
BOOL    APIENTRY GpiQueryCharStringPosAt(HPS hps, PPOINTL pptlStart, ULONG flOptions, LONG lCount,
                                         PCCH pchString, PLONG alXincrements, PPOINTL aptlPositions);
LONG    APIENTRY GpiQueryFonts(HPS hps, ULONG flOptions, PCSZ pszFacename, PLONG plReqFonts,
                               LONG lMetricsLength, PFONTMETRICS afmMetrics);
BOOL    APIENTRY GpiLoadFonts(HAB hab, PCSZ pszFilename);
BOOL    APIENTRY GpiLoadPublicFonts(HAB hab, PCSZ pszFilename);
BOOL    APIENTRY GpiUnloadPublicFonts(HAB hab, PCSZ pszFilename);

/* Bitmaps built in memory */
#define CBM_INIT     0x0004
#define BCA_UNCOMP   0
#define BCA_RLE8     1
#define BCA_RLE4     2
#define BCA_RLE24    4

typedef struct _RGB2 {
    BYTE bBlue;
    BYTE bGreen;
    BYTE bRed;
    BYTE fcOptions;
} RGB2, *PRGB2;

typedef struct _BITMAPINFOHEADER {
    ULONG  cbFix;
    USHORT cx;
    USHORT cy;
    USHORT cPlanes;
    USHORT cBitCount;
} BITMAPINFOHEADER, *PBITMAPINFOHEADER;

typedef struct _BITMAPINFOHEADER2 {
    ULONG  cbFix;
    ULONG  cx;
    ULONG  cy;
    USHORT cPlanes;
    USHORT cBitCount;
    ULONG  ulCompression;
    ULONG  cbImage;
    ULONG  cxResolution;
    ULONG  cyResolution;
    ULONG  cclrUsed;
    ULONG  cclrImportant;
    USHORT usUnits;
    USHORT usReserved;
    USHORT usRecording;
    USHORT usRendering;
    ULONG  cSize1;
    ULONG  cSize2;
    ULONG  ulColorEncoding;
    ULONG  ulIdentifier;
} BITMAPINFOHEADER2, *PBITMAPINFOHEADER2;

/* A header followed by its colour table (argbColor holds cclrUsed, or
 * 2^cBitCount, entries). */
typedef struct _BITMAPINFO2 {
    ULONG  cbFix;
    ULONG  cx;
    ULONG  cy;
    USHORT cPlanes;
    USHORT cBitCount;
    ULONG  ulCompression;
    ULONG  cbImage;
    ULONG  cxResolution;
    ULONG  cyResolution;
    ULONG  cclrUsed;
    ULONG  cclrImportant;
    USHORT usUnits;
    USHORT usReserved;
    USHORT usRecording;
    USHORT usRendering;
    ULONG  cSize1;
    ULONG  cSize2;
    ULONG  ulColorEncoding;
    ULONG  ulIdentifier;
    RGB2   argbColor[1];
} BITMAPINFO2, *PBITMAPINFO2;

HBITMAP APIENTRY GpiCreateBitmap(HPS hps, PBITMAPINFOHEADER2 pbmpNew, ULONG flOptions, PBYTE pbInitData,
                                 PBITMAPINFO2 pbmiInfoTable);
LONG    APIENTRY GpiSetBitmapBits(HPS hps, LONG lScanStart, LONG lScans, PBYTE pbBuffer,
                                  PBITMAPINFO2 pbmiInfoTable);
LONG    APIENTRY GpiQueryBitmapBits(HPS hps, LONG lScanStart, LONG lScans, PBYTE pbBuffer,
                                    PBITMAPINFO2 pbmiInfoTable);
BOOL    APIENTRY GpiQueryBitmapInfoHeader(HBITMAP hbm, PBITMAPINFOHEADER2 pbmpData);
BOOL    APIENTRY GpiQueryBitmapParameters(HBITMAP hbm, PBITMAPINFOHEADER pbmpData);
BOOL    APIENTRY GpiQueryDeviceBitmapFormats(HPS hps, LONG lCount, PLONG alArray);
LONG    APIENTRY GpiDrawBits(HPS hps, PVOID pBits, PBITMAPINFO2 pbmiInfoTable, LONG lCount, PPOINTL aptlPoints,
                             LONG lRop, ULONG flOptions);
LONG    APIENTRY GpiWCBitBlt(HPS hpsTarget, HBITMAP hbmSource, LONG lCount, PPOINTL aptlPoints, LONG lRop,
                             ULONG flOptions);

#endif /* INCL_GPI */

#pragma pack(pop)

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_OS2_PM_H */
