/*
 * pm_controls_test.c — Verify PM built-in window controls.
 *
 * Creates a frame window containing one of each built-in PM control class,
 * exercises text/enable/query APIs, then self-terminates via WM_TIMER (500 ms).
 *
 * Controls tested:
 *  WC_STATIC     — static text label
 *  WC_BUTTON     — push button; WinQueryWindowText verifies text
 *  WC_ENTRYFIELD — single-line text entry; WinSetWindowText/WinQueryWindowText round-trip
 *  WC_SCROLLBAR  — vertical scroll bar; WinEnableWindow/WinIsWindowEnabled
 *  WC_LISTBOX    — list box; LM_INSERTITEM / LM_QUERYITEMCOUNT
 *  WC_MLE        — multi-line entry field
 *
 * Results are printed via DosWrite before DosExit.
 */

#define INCL_WIN
#define INCL_GPI
#define INCL_DOS
#include <os2.h>
#include <string.h>

/* ---------- tiny integer-to-string helper (no libc printf) ---------- */
static ULONG g_dummy;

static void print(const char *msg)
{
    ULONG len = 0;
    const char *p = msg;
    while (*p++) len++;
    DosWrite(1, (PVOID)msg, len, &g_dummy);
}

static void print_num(ULONG v)
{
    char b[12];
    int i = 11;
    b[i--] = 0;
    if (!v) { b[i--] = '0'; }
    else { while (v) { b[i--] = '0' + v % 10; v /= 10; } }
    print(&b[i + 1]);
}

static void check(const char *lbl, int ok, int *pass, int *fail)
{
    print("  "); print(lbl);
    if (ok) { print(" OK\r\n"); (*pass)++; }
    else    { print(" FAILED\r\n"); (*fail)++; }
}

/* ---------- control IDs and timer ---------- */
#define ID_STATIC    101
#define ID_BUTTON    102
#define ID_ENTRY     103
#define ID_SCROLLBAR 104
#define ID_LISTBOX   105
#define ID_MLE       106
#define ID_TIMER     201

/* LM_* list-box messages (may not be in older Watcom headers) */
#ifndef LM_INSERTITEM
#define LM_INSERTITEM     0x0150
#define LM_QUERYITEMCOUNT 0x0151
#define LIT_END           (-1)
#endif

static HAB  g_hab;
static int  g_passed = 0, g_failed = 0;
static HWND g_hwndStatic, g_hwndButton, g_hwndEntry,
            g_hwndScroll, g_hwndList,   g_hwndMle;

/* ---------- window procedure ---------- */
MRESULT EXPENTRY ClientWndProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2)
{
    switch (msg) {

    case WM_CREATE:
    {
        /* Create one of each built-in PM control */
        g_hwndStatic = WinCreateWindow(hwnd, WC_STATIC, "Static Label",
            WS_VISIBLE | SS_TEXT, 10, 280, 200, 25,
            hwnd, HWND_TOP, ID_STATIC, NULL, NULL);

        g_hwndButton = WinCreateWindow(hwnd, WC_BUTTON, "Click Me",
            WS_VISIBLE | BS_PUSHBUTTON, 10, 240, 120, 28,
            hwnd, HWND_TOP, ID_BUTTON, NULL, NULL);

        g_hwndEntry = WinCreateWindow(hwnd, WC_ENTRYFIELD, "initial text",
            WS_VISIBLE | ES_AUTOSCROLL, 10, 200, 200, 25,
            hwnd, HWND_TOP, ID_ENTRY, NULL, NULL);

        g_hwndScroll = WinCreateWindow(hwnd, WC_SCROLLBAR, NULL,
            WS_VISIBLE | SBS_VERT, 230, 150, 18, 120,
            hwnd, HWND_TOP, ID_SCROLLBAR, NULL, NULL);

        g_hwndList = WinCreateWindow(hwnd, WC_LISTBOX, NULL,
            WS_VISIBLE | LS_NOADJUSTPOS, 10, 60, 200, 100,
            hwnd, HWND_TOP, ID_LISTBOX, NULL, NULL);

        g_hwndMle = WinCreateWindow(hwnd, WC_MLE, "Line one\nLine two",
            WS_VISIBLE | MLS_WORDWRAP, 10, 5, 200, 50,
            hwnd, HWND_TOP, ID_MLE, NULL, NULL);

        WinStartTimer(g_hab, hwnd, ID_TIMER, 3000);
        return 0;
    }

    case WM_TIMER:
    {
        /* Declare all locals at the top (C89 requirement) */
        char   buf[64];
        ULONG  n;
        BOOL   wasEnabled;
        LONG   count;

        if (SHORT1FROMMP(mp1) != ID_TIMER) break;
        WinStopTimer(g_hab, hwnd, ID_TIMER);

        print("\r\n--- Running control checks ---\r\n");

        /* WC_STATIC */
        check("WC_STATIC hwnd non-NULL",
              g_hwndStatic != NULLHANDLE, &g_passed, &g_failed);

        /* WC_BUTTON — text query */
        check("WC_BUTTON hwnd non-NULL",
              g_hwndButton != NULLHANDLE, &g_passed, &g_failed);
        if (g_hwndButton) {
            n = WinQueryWindowText(g_hwndButton, sizeof(buf), buf);
            check("WinQueryWindowText(button) == 'Click Me'",
                  n > 0 && strcmp(buf, "Click Me") == 0, &g_passed, &g_failed);
        }

        /* WC_ENTRYFIELD — set/get text round-trip */
        check("WC_ENTRYFIELD hwnd non-NULL",
              g_hwndEntry != NULLHANDLE, &g_passed, &g_failed);
        if (g_hwndEntry) {
            WinSetWindowText(g_hwndEntry, "updated");
            n = WinQueryWindowText(g_hwndEntry, sizeof(buf), buf);
            check("WinSetWindowText/QueryWindowText round-trip",
                  n > 0 && strcmp(buf, "updated") == 0, &g_passed, &g_failed);
        }

        /* WC_SCROLLBAR — enable / disable */
        check("WC_SCROLLBAR hwnd non-NULL",
              g_hwndScroll != NULLHANDLE, &g_passed, &g_failed);
        if (g_hwndScroll) {
            wasEnabled = WinIsWindowEnabled(g_hwndScroll);
            check("Scrollbar initially enabled", wasEnabled, &g_passed, &g_failed);
            WinEnableWindow(g_hwndScroll, FALSE);
            check("WinEnableWindow(FALSE) disables scrollbar",
                  !WinIsWindowEnabled(g_hwndScroll), &g_passed, &g_failed);
            WinEnableWindow(g_hwndScroll, TRUE);
            check("WinEnableWindow(TRUE) re-enables scrollbar",
                  WinIsWindowEnabled(g_hwndScroll), &g_passed, &g_failed);
        }

        /* WC_LISTBOX — insert items and query count */
        check("WC_LISTBOX hwnd non-NULL",
              g_hwndList != NULLHANDLE, &g_passed, &g_failed);
        if (g_hwndList) {
            WinSendMsg(g_hwndList, LM_INSERTITEM, MPFROMSHORT(LIT_END),
                       MPFROMP("Apple"));
            WinSendMsg(g_hwndList, LM_INSERTITEM, MPFROMSHORT(LIT_END),
                       MPFROMP("Banana"));
            WinSendMsg(g_hwndList, LM_INSERTITEM, MPFROMSHORT(LIT_END),
                       MPFROMP("Cherry"));
            count = (LONG)WinSendMsg(g_hwndList, LM_QUERYITEMCOUNT, 0, 0);
            print("  Listbox item count="); print_num((ULONG)count); print("\r\n");
            check("LM_INSERTITEM x3 -> LM_QUERYITEMCOUNT == 3",
                  count == 3, &g_passed, &g_failed);
        }

        /* WC_MLE */
        check("WC_MLE hwnd non-NULL",
              g_hwndMle != NULLHANDLE, &g_passed, &g_failed);

        /* Summary */
        print("\r\n=== Results ===\r\n");
        print("Passed: "); print_num((ULONG)g_passed); print("\r\n");
        print("Failed: "); print_num((ULONG)g_failed); print("\r\n");
        if (g_failed == 0) print("\r\nAll tests PASSED!\r\n");
        else               print("\r\nSome tests FAILED!\r\n");

        WinPostMsg(hwnd, WM_QUIT, 0, 0);
        return 0;
    }

    case WM_PAINT:
    {
        POINTL pt;
        HPS    hps = WinBeginPaint(hwnd, NULLHANDLE, NULL);
        RECTL  rcl;

        WinQueryWindowRect(hwnd, &rcl);
        WinFillRect(hps, &rcl, CLR_WHITE);

        GpiSetColor(hps, CLR_BLACK);
        pt.x = 10;
        pt.y = rcl.yTop - 20;
        GpiCharStringAt(hps, &pt, 30, "PM Built-in Controls Test");

        WinEndPaint(hps);
        return 0;
    }

    case WM_CLOSE:
        WinPostMsg(hwnd, WM_QUIT, 0, 0);
        return 0;
    }
    return WinDefWindowProc(hwnd, msg, mp1, mp2);
}

int main(void)
{
    HMQ   hmq;
    HWND  hwndFrame, hwndClient;
    QMSG  qmsg;
    ULONG flFrameFlags = FCF_TITLEBAR | FCF_SIZEBORDER | FCF_MINMAX |
                         FCF_SYSMENU | FCF_TASKLIST;

    print("=== PM Built-in Controls Test ===\r\n");

    g_hab = WinInitialize(0);
    if (!g_hab) { print("WinInitialize failed\r\n"); return 1; }

    hmq = WinCreateMsgQueue(g_hab, 0);
    if (!hmq) { WinTerminate(g_hab); return 1; }

    WinRegisterClass(g_hab, "PMCtrlTest", ClientWndProc, CS_SIZEREDRAW, 0);

    hwndFrame = WinCreateStdWindow(
        HWND_DESKTOP, WS_VISIBLE,
        &flFrameFlags, "PMCtrlTest",
        "PM Built-in Controls Test",
        0, NULLHANDLE, 0, &hwndClient);

    if (!hwndFrame) {
        print("WinCreateStdWindow failed\r\n");
        WinDestroyMsgQueue(hmq);
        WinTerminate(g_hab);
        return 1;
    }

    while (WinGetMsg(g_hab, &qmsg, NULLHANDLE, 0, 0))
        WinDispatchMsg(g_hab, &qmsg);

    WinDestroyWindow(hwndFrame);
    WinDestroyMsgQueue(hmq);
    WinTerminate(g_hab);

    return g_failed > 0 ? 1 : 0;
}
