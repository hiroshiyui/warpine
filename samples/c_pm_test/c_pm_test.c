/* SPDX-License-Identifier: GPL-3.0-or-later */
/* c_pm_test.c - a Presentation Manager program built by GCC or clang with
 * Warpine's SDK (ADR-0097): a frame and client window, the message loop,
 * window procedures called back by Warpine, sent and posted messages, a
 * timer, window words and text, rectangles, atoms, and GPI drawing read
 * back with GpiQueryPel; and resources compiled by os2rc (c_pm_test.rc,
 * ADR-0099): the frame's menu and accelerators, a dialog, a string and
 * raw data. The program ends itself; each check prints
 *
 *   c_pm_test: <what>: ok
 *   c_pm_test: <what>: FAIL got <value> want <value>
 *
 * and "c_pm_test: done <passed>/<total>" at the end. */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSPROCESS
#include <os2.h>
#include <stdio.h>
#include <string.h>
#include <stdint.h>

#include "c_pm_test.h"

/* The structures as Warpine reads and writes them. */
_Static_assert(sizeof(QMSG) == 32, "QMSG");
_Static_assert(offsetof(QMSG, ptl) == 20, "QMSG.ptl");
_Static_assert(sizeof(SWP) == 36 && offsetof(SWP, hwnd) == 24, "SWP");
_Static_assert(sizeof(FRAMECDATA) == 10 && offsetof(FRAMECDATA, idResources) == 8, "FRAMECDATA");
_Static_assert(sizeof(CLASSINFO) == 12, "CLASSINFO");
_Static_assert(sizeof(FATTRS) == 56 && offsetof(FATTRS, lMaxBaselineExt) == 44, "FATTRS");
_Static_assert(sizeof(FONTMETRICS) == 228, "FONTMETRICS");
_Static_assert(offsetof(FONTMETRICS, lEmHeight) == 68 && offsetof(FONTMETRICS, lMaxBaselineExt) == 112
               && offsetof(FONTMETRICS, sXDeviceRes) == 126 && offsetof(FONTMETRICS, sNominalPointSize) == 138,
               "FONTMETRICS offsets");

/* The other PM modules' structures, as Warpine reads them. */
_Static_assert(sizeof(SWCNTRL) == 96 && sizeof(SWENTRY) == 100, "SWCNTRL");
_Static_assert(sizeof(PROGDETAILS) == 72 && offsetof(PROGDETAILS, pszEnvironment) == 32, "PROGDETAILS");
_Static_assert(sizeof(FILEDLG) == 328 && offsetof(FILEDLG, szFullFile) == 52 && offsetof(FILEDLG, papszFQFilename) == 312
               && offsetof(FILEDLG, usDlgId) == 320 && offsetof(FILEDLG, x) == 322, "FILEDLG");
_Static_assert(sizeof(FONTDLG) == 168 && offsetof(FONTDLG, fAttrs) == 96 && offsetof(FONTDLG, sNominalPointSize) == 152
               && offsetof(FONTDLG, usDlgId) == 162 && offsetof(FONTDLG, usFamilyBufLen) == 164, "FONTDLG");
_Static_assert(sizeof(HELPINIT) == 44 && offsetof(HELPINIT, pszHelpLibraryName) == 40, "HELPINIT");
_Static_assert(sizeof(DRAGINFO) == 20 && offsetof(DRAGINFO, cditem) == 16, "DRAGINFO");
_Static_assert(sizeof(DRAGITEM) == 36 && offsetof(DRAGITEM, fsControl) == 32, "DRAGITEM");
_Static_assert(sizeof(DRAGTRANSFER) == 28, "DRAGTRANSFER");
_Static_assert(sizeof(DEVOPENSTRUC) == 36, "DEVOPENSTRUC");

#define CLIENT_CLASS "CPmTestClient"
#define WM_CHECKS    (WM_USER + 1)
#define WM_ECHO      (WM_USER + 2)
#define WM_POSTED    (WM_USER + 3)
#define TIMER_ID     7
#define CLIENT_CX    300
#define CLIENT_CY    200

static int passed, total;
static HAB hab;
static HWND frame, client;
static int created, painted, timer_ticks, posted_seen;
static LONG paint_pel;
static int aligned_in_wndproc = 1;

static void check(const char *what, long got, long want)
{
    total++;
    if (got == want) {
        passed++;
        printf("c_pm_test: %s: ok\n", what);
    } else {
        printf("c_pm_test: %s: FAIL got %ld want %ld\n", what, got, want);
    }
}

/* Drawing in WM_PAINT, read back on the spot. */
static void paint(HWND hwnd)
{
    RECTL rcl;
    HPS hps = WinBeginPaint(hwnd, NULLHANDLE, &rcl);
    WinQueryWindowRect(hwnd, &rcl);
    WinFillRect(hps, &rcl, CLR_WHITE);
    GpiSetColor(hps, CLR_BLUE);
    POINTL corner = { 10, 10 }, opposite = { 60, 40 };
    GpiMove(hps, &corner);
    GpiBox(hps, DRO_FILL, &opposite, 0, 0);
    POINTL inside = { 30, 20 };
    paint_pel = GpiQueryPel(hps, &inside);
    GpiSetColor(hps, CLR_BLACK);
    POINTL at = { 10, 100 };
    GpiCharStringAt(hps, &at, 21, "Hello from GCC/clang!");
    WinEndPaint(hps);
    painted++;
}

static MRESULT EXPENTRY client_proc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2)
{
    _Alignas(16) unsigned char probe[16];
    __asm__ volatile("" : : "r"(probe) : "memory");
    if ((uintptr_t)probe & 15) aligned_in_wndproc = 0;

    switch (msg) {
    case WM_CREATE:
        created++;
        return 0;
    case WM_PAINT:
        paint(hwnd);
        return 0;
    case WM_ECHO:
        return MRFROMLONG(LONGFROMMP(mp1) + LONGFROMMP(mp2));
    case WM_POSTED:
        posted_seen = LONGFROMMP(mp1);
        return 0;
    case WM_TIMER:
        if (SHORT1FROMMP(mp1) == TIMER_ID && ++timer_ticks == 1)
            WinPostMsg(hwnd, WM_CHECKS, MPVOID, MPVOID);
        return 0;
    case WM_CHECKS:
        WinStopTimer(hab, hwnd, TIMER_ID);
        {
            extern void run_checks(HWND);
            run_checks(hwnd);
        }
        WinPostMsg(frame, WM_CLOSE, MPVOID, MPVOID);
        return 0;
    }
    return WinDefWindowProc(hwnd, msg, mp1, mp2);
}

static char dlg_entry_text[32];

/* The dialog reads its entry field's text from the template, then ends. */
static MRESULT EXPENTRY dlg_proc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2)
{
    if (msg == WM_INITDLG) {
        WinQueryDlgItemText(hwnd, ID_DLG_EDIT, sizeof dlg_entry_text, dlg_entry_text);
        WinDismissDlg(hwnd, DLG_RESULT);
        return 0;
    }
    return WinDefDlgProc(hwnd, msg, mp1, mp2);
}

/* Profiles, the switch list, drag and drop, help. */
static void module_checks(void)
{
    char text[32] = "";
    check("PrfWriteProfileString / PrfQueryProfileString",
          PrfWriteProfileString(HINI_USERPROFILE, "CPmTest", "Key", "Value")
          && PrfQueryProfileString(HINI_USERPROFILE, "CPmTest", "Key", "none", text, sizeof text) == 6
          && !strcmp(text, "Value"), 1);
    ULONG size = 0;
    check("PrfQueryProfileSize", PrfQueryProfileSize(HINI_USERPROFILE, "CPmTest", "Key", &size) && size == 6, 1);
    check("PrfQueryProfileInt of a missing key gives the default",
          PrfQueryProfileInt(HINI_USERPROFILE, "CPmTest", "Missing", 42), 42);
    HSTR h = DrgAddStrHandle("DRM_OS2FILE");
    char name[32] = "";
    check("DrgAddStrHandle / DrgQueryStrName", h != NULLHANDLE && DrgQueryStrName(h, sizeof name, name) == 11
          && !strcmp(name, "DRM_OS2FILE") && DrgDeleteStrHandle(h), 1);
    PDRAGINFO di = DrgAllocDraginfo(2);
    check("DrgAllocDraginfo / DrgQueryDragitemCount / DrgFreeDraginfo",
          di != NULL && DrgQueryDragitemCount(di) == 2 && di->cbDragitem == sizeof(DRAGITEM) && DrgFreeDraginfo(di), 1);
    HSWITCH sw = WinQuerySwitchHandle(frame, 0);
    SWCNTRL ctl;
    memset(&ctl, 0, sizeof ctl);
    check("WinQuerySwitchHandle / WinQuerySwitchEntry find the frame's entry",
          sw != NULLHANDLE && WinQuerySwitchEntry(sw, &ctl) == 0 && ctl.hwnd == frame, 1);
    check("WinQueryHelpInstance: no help instance", WinQueryHelpInstance(frame), NULLHANDLE);
}

static void resource_checks(void)
{
    check("FCF_MENU loaded the RT_MENU resource as the frame's menu",
          WinWindowFromID(frame, FID_MENU) != NULLHANDLE, 1);
    check("FCF_ACCELTABLE loaded the RT_ACCELTABLE resource", WinQueryAccelTable(hab, frame) != NULLHANDLE, 1);
    char text[40] = "";
    LONG n = WinLoadString(hab, NULLHANDLE, ID_STR_HELLO, sizeof text, text);
    check("WinLoadString reads the STRINGTABLE", n == 16 && !strcmp(text, "Hello from os2rc"), 1);
    PVOID data = NULL;
    ULONG size = 0;
    static const unsigned char want[] = { 0x34, 0x12, 0xBC, 0x9A, 0x78, 0x56, 'r', 'a', 'w', 0 };
    check("DosGetResource / DosQueryResourceSize read the RCDATA",
          DosGetResource(NULLHANDLE, RT_RCDATA, ID_DATA, &data) == NO_ERROR
          && DosQueryResourceSize(NULLHANDLE, RT_RCDATA, ID_DATA, &size) == NO_ERROR
          && size == sizeof want && !memcmp(data, want, sizeof want), 1);
    check("WinDlgBox runs the DLGTEMPLATE and returns WinDismissDlg's result",
          WinDlgBox(HWND_DESKTOP, frame, dlg_proc, NULLHANDLE, ID_DLG, NULL), DLG_RESULT);
    check("the dialog's entry field got the template's text", !strcmp(dlg_entry_text, "typed"), 1);
}

void run_checks(HWND hwnd)
{
    resource_checks();
    module_checks();
    check("WM_CREATE reached the client once", created, 1);
    check("WM_PAINT reached the client", painted >= 1, 1);
    check("GpiBox filled with the colour GpiQueryPel reads back", paint_pel, CLR_BLUE);
    check("the window procedure ran on a 16-byte-aligned frame", aligned_in_wndproc, 1);
    check("the timer ticked", timer_ticks >= 1, 1);

    check("WinWindowFromID(frame, FID_CLIENT) is the client", WinWindowFromID(frame, FID_CLIENT), client);
    check("the client's parent is the frame", WinQueryWindow(hwnd, QW_PARENT), frame);
    check("WinQueryAnchorBlock", WinQueryAnchorBlock(hwnd) != NULLHANDLE, 1);
    check("WinIsWindow", WinIsWindow(hab, hwnd), TRUE);
    check("WinIsWindowVisible", WinIsWindowVisible(frame), TRUE);

    RECTL rcl;
    WinQueryWindowRect(frame, &rcl);
    check("the frame is the size WinSetWindowPos gave", rcl.xRight - rcl.xLeft, CLIENT_CX + 100);
    SWP swp;
    check("WinQueryWindowPos", WinQueryWindowPos(frame, &swp) && swp.cy == CLIENT_CY + 100, 1);

    check("WinSendMsg runs the procedure and returns its result",
          LONGFROMMR(WinSendMsg(hwnd, WM_ECHO, MPFROMLONG(40), MPFROMLONG(2))), 42);
    WinPostMsg(hwnd, WM_POSTED, MPFROMLONG(77), MPVOID);
    QMSG qmsg;
    while (WinPeekMsg(hab, &qmsg, hwnd, WM_POSTED, WM_POSTED, PM_REMOVE))
        WinDispatchMsg(hab, &qmsg);
    check("a posted message is peeked and dispatched", posted_seen, 77);

    char text[64];
    WinSetWindowText(frame, "C on Warpine");
    LONG n = WinQueryWindowText(frame, sizeof text, text);
    check("WinSetWindowText / WinQueryWindowText", n == 12 && !strcmp(text, "C on Warpine"), 1);
    check("WinSetWindowULong(QWL_USER)", WinSetWindowULong(hwnd, QWL_USER, 0xC0FFEE), TRUE);
    check("WinQueryWindowULong(QWL_USER)", WinQueryWindowULong(hwnd, QWL_USER), 0xC0FFEE);

    RECTL a, b, c;
    WinSetRect(hab, &a, 0, 0, 10, 10);
    WinSetRect(hab, &b, 5, 5, 20, 20);
    WinIntersectRect(hab, &c, &a, &b);
    check("WinIntersectRect", c.xLeft == 5 && c.yBottom == 5 && c.xRight == 10 && c.yTop == 10, 1);
    WinUnionRect(hab, &c, &a, &b);
    check("WinUnionRect", c.xLeft == 0 && c.xRight == 20, 1);
    POINTL in = { 3, 3 }, out = { 15, 2 };
    check("WinPtInRect", WinPtInRect(hab, &a, &in) && !WinPtInRect(hab, &a, &out), 1);

    HATOMTABLE atoms = WinQuerySystemAtomTable();
    ATOM atom = WinAddAtom(atoms, "WarpineCAtom");
    char name[32] = "";
    WinQueryAtomName(atoms, atom, name, sizeof name);
    check("WinAddAtom / WinQueryAtomName", atom != 0 && !strcmp(name, "WarpineCAtom"), 1);
    check("WinFindAtom", WinFindAtom(atoms, "WarpineCAtom"), atom);

    check("WinQuerySysValue(SV_CXSCREEN) is positive", WinQuerySysValue(HWND_DESKTOP, SV_CXSCREEN) > 0, 1);
    check("WinQuerySysPointer(SPTR_ARROW)", WinQuerySysPointer(HWND_DESKTOP, SPTR_ARROW, FALSE) != NULLHANDLE, 1);

    HPS hps = WinGetPS(hwnd);
    FONTMETRICS fm;
    check("GpiQueryFontMetrics", GpiQueryFontMetrics(hps, sizeof fm, &fm), TRUE);
    check("the font has a height", fm.lMaxBaselineExt > 0 && fm.lEmHeight > 0, 1);
    POINTL p = { 120, 30 }, pos;
    GpiMove(hps, &p);
    GpiQueryCurrentPosition(hps, &pos);
    check("GpiMove / GpiQueryCurrentPosition", pos.x == 120 && pos.y == 30, 1);
    check("GpiSetColor / GpiQueryColor", GpiSetColor(hps, CLR_RED) && GpiQueryColor(hps) == CLR_RED, 1);

    RECTL r1 = { 0, 0, 10, 10 }, r2 = { 20, 0, 30, 10 }, box;
    HRGN a1 = GpiCreateRegion(hps, 1, &r1), a2 = GpiCreateRegion(hps, 1, &r2);
    check("GpiCombineRegion(CRGN_OR) of two rectangles is complex",
          GpiCombineRegion(hps, a1, a1, a2, CRGN_OR), RGN_COMPLEX);
    check("GpiQueryRegionBox", GpiQueryRegionBox(hps, a1, &box) != RGN_ERROR
          && box.xLeft == 0 && box.xRight == 30 && box.yTop == 10, 1);
    POINTL gap = { 15, 5 }, hit = { 25, 5 };
    check("GpiPtInRegion", GpiPtInRegion(hps, a1, &gap) == PRGN_OUTSIDE
          && GpiPtInRegion(hps, a1, &hit) == PRGN_INSIDE, 1);
    GpiDestroyRegion(hps, a1);
    GpiDestroyRegion(hps, a2);

    LONG horiz = 0, vert = 0;
    check("GpiSetTextAlignment / GpiQueryTextAlignment", GpiSetTextAlignment(hps, TA_CENTER, TA_TOP)
          && GpiQueryTextAlignment(hps, &horiz, &vert) && horiz == TA_CENTER && vert == TA_TOP, 1);
    MATRIXLF m = { MAKEFIXED(1, 0), 0, 0, 0, MAKEFIXED(1, 0), 0, 7, 9, 1 }, back;
    check("GpiSetModelTransformMatrix / GpiQueryModelTransformMatrix",
          GpiSetModelTransformMatrix(hps, 9, &m, TRANSFORM_REPLACE)
          && GpiQueryModelTransformMatrix(hps, 9, &back) && back.lM31 == 7 && back.lM32 == 9, 1);
    WinReleasePS(hps);

    HENUM henum = WinBeginEnumWindows(HWND_DESKTOP);
    int found_frame = 0;
    for (HWND w; (w = WinGetNextWindow(henum)) != NULLHANDLE; ) found_frame |= w == frame;
    WinEndEnumWindows(henum);
    check("WinBeginEnumWindows finds the frame among the desktop's children", found_frame, 1);

    struct { USHORT cAccel, codepage; ACCEL a[2]; } table = {
        2, 0, { { AF_CHAR | AF_CONTROL, 'q', 101 }, { AF_VIRTUALKEY, VK_F1, 102 } } };
    HACCEL haccel = WinCreateAccelTable(hab, (PACCELTABLE)&table);
    struct { USHORT cAccel, codepage; ACCEL a[2]; } copy;
    memset(&copy, 0, sizeof copy);
    check("WinCreateAccelTable / WinCopyAccelTable", haccel != NULLHANDLE
          && WinCopyAccelTable(haccel, (PACCELTABLE)&copy, sizeof copy) && copy.cAccel == 2
          && copy.a[1].key == VK_F1 && copy.a[1].cmd == 102, 1);
    WinDestroyAccelTable(haccel);
    check("WinCompareStrings", WinCompareStrings(hab, 0, 0, "apple", "banana", 0) == WCS_LT
          && WinCompareStrings(hab, 0, 0, "same", "same", 0) == WCS_EQ, 1);
}

int main(void)
{
    hab = WinInitialize(0);
    HMQ hmq = WinCreateMsgQueue(hab, 0);
    check("WinInitialize / WinCreateMsgQueue", hab != NULLHANDLE && hmq != NULLHANDLE, 1);
    check("WinRegisterClass", WinRegisterClass(hab, CLIENT_CLASS, client_proc, CS_SIZEREDRAW, 4), TRUE);

    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINMAX | FCF_MENU | FCF_ACCELTABLE | FCF_TASKLIST;
    frame = WinCreateStdWindow(HWND_DESKTOP, 0, &flags, CLIENT_CLASS, "c_pm_test", 0, NULLHANDLE, ID_MAIN, &client);
    check("WinCreateStdWindow", frame != NULLHANDLE && client != NULLHANDLE, 1);
    WinSetWindowPos(frame, HWND_TOP, 40, 40, CLIENT_CX + 100, CLIENT_CY + 100,
                    SWP_SIZE | SWP_MOVE | SWP_SHOW | SWP_ACTIVATE);
    check("WinStartTimer", WinStartTimer(hab, client, TIMER_ID, 200) != 0, 1);

    QMSG qmsg;
    while (WinGetMsg(hab, &qmsg, NULLHANDLE, 0, 0))
        WinDispatchMsg(hab, &qmsg);

    check("WinDestroyWindow", WinDestroyWindow(frame), TRUE);
    WinDestroyMsgQueue(hmq);
    WinTerminate(hab);
    printf("c_pm_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}
