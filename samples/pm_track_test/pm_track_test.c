/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_track_test.c - WinTrackRect / WinShowTrackRect (ADR-0089)
 *
 * Driven by a GUI script (tests/integration.rs, PM_TRACK_STEPS):
 *
 *   a drag in the client     WM_BUTTON1DOWN tracks a 100 x 50 rectangle
 *                            (TF_MOVE): the drag's offset moves it, the
 *                            release accepts (TRUE)
 *   key k, right right up,   a keyboard size of the right and top edges,
 *   enter                    5 pels a step: accepted (TRUE)
 *   key c, right, esc        a move cancelled: FALSE, rclTrack the start
 *   key g, right x3, enter   TF_GRID (10) with a keyboard step of 4: the
 *                            moved edge snaps; TF_ALLINBOUNDARY holds it
 *   key q                    the last checks, then the window closes
 *
 * Also checked: WinShowTrackRect with no tracking is FALSE; the outline
 * is gone after a tracking (the pels where it was are as before).
 *
 * Output: "pm_track_test: <name>: ok" or "... FAIL", then
 * "pm_track_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#include <os2.h>
#include <stdio.h>

static int passed = 0, total = 0;
static HWND hwndClient;

static void check(const char *what, int good)
{
    total++;
    if (good) passed++;
    printf("pm_track_test: %s: %s\n", what, good ? "ok" : "FAIL");
    fflush(stdout);
}

static void setup(TRACKINFO *ti, LONG l, LONG b, LONG r, LONG t, ULONG fs)
{
    ti->cxBorder = ti->cyBorder = 2;
    ti->cxGrid = ti->cyGrid = 10;
    ti->cxKeyboard = ti->cyKeyboard = 5;
    ti->rclTrack.xLeft = l; ti->rclTrack.yBottom = b;
    ti->rclTrack.xRight = r; ti->rclTrack.yTop = t;
    ti->rclBoundary.xLeft = 0; ti->rclBoundary.yBottom = 0;
    ti->rclBoundary.xRight = 1000; ti->rclBoundary.yTop = 1000;
    ti->ptlMinTrackSize.x = ti->ptlMinTrackSize.y = 4;
    ti->ptlMaxTrackSize.x = ti->ptlMaxTrackSize.y = 1000;
    ti->fs = fs;
}

static int is(const RECTL *r, LONG l, LONG b, LONG rt, LONG t)
{
    return r->xLeft == l && r->yBottom == b && r->xRight == rt && r->yTop == t;
}

static void report(const char *what, BOOL rc, const RECTL *r)
{
    printf("track: %s rc=%d rect=%ld,%ld,%ld,%ld\n", what, rc, r->xLeft, r->yBottom, r->xRight, r->yTop);
}

static MRESULT EXPENTRY ClientProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2)
{
    static BOOL dragged = FALSE;
    TRACKINFO ti;
    BOOL rc;
    HPS hps;
    RECTL rcl;

    switch (msg) {
    case WM_PAINT:
        hps = WinBeginPaint(hwnd, NULLHANDLE, &rcl);
        WinFillRect(hps, &rcl, CLR_WHITE);
        WinEndPaint(hps);
        return 0;
    case WM_BUTTON1DOWN:
        if (dragged) break;
        dragged = TRUE;
        hps = WinGetPS(hwnd);
        GpiCreateLogColorTable(hps, 0, LCOLF_RGB, 0, 0, NULL);
        {
            POINTL corner = { 90, 80 };   /* where the outline ends up */
            LONG before = GpiQueryPel(hps, &corner);
            setup(&ti, 50, 50, 150, 100, TF_MOVE);
            rc = WinTrackRect(hwnd, NULLHANDLE, &ti);
            report("pointer", rc, &ti.rclTrack);
            check("WinTrackRect with the pointer accepts", rc == TRUE);
            check("the drag's offset moved the rectangle", is(&ti.rclTrack, 90, 80, 190, 130));
            check("the outline is gone afterwards", GpiQueryPel(hps, &corner) == before);
        }
        WinReleasePS(hps);
        return (MRESULT)TRUE;
    case WM_CHAR:
        if (SHORT1FROMMP(mp1) & KC_KEYUP) break;
        if (!(SHORT1FROMMP(mp1) & KC_CHAR)) break;
        switch (SHORT1FROMMP(mp2)) {
        case 'k':
            setup(&ti, 10, 10, 60, 40, TF_RIGHT | TF_TOP);
            rc = WinTrackRect(hwnd, NULLHANDLE, &ti);
            report("keyboard", rc, &ti.rclTrack);
            check("a keyboard size accepts with Enter", rc == TRUE);
            check("the right and top edges stepped", is(&ti.rclTrack, 10, 10, 70, 45));
            return (MRESULT)TRUE;
        case 'c':
            setup(&ti, 20, 20, 80, 60, TF_MOVE);
            rc = WinTrackRect(hwnd, NULLHANDLE, &ti);
            report("cancel", rc, &ti.rclTrack);
            check("Esc cancels", rc == FALSE);
            check("a cancel leaves the starting rectangle", is(&ti.rclTrack, 20, 20, 80, 60));
            return (MRESULT)TRUE;
        case 'g':
            setup(&ti, 100, 100, 200, 150, TF_RIGHT | TF_GRID | TF_ALLINBOUNDARY);
            ti.cxKeyboard = 4;
            ti.rclBoundary.xRight = 215;
            rc = WinTrackRect(hwnd, NULLHANDLE, &ti);
            report("grid", rc, &ti.rclTrack);
            check("TF_GRID snaps, TF_ALLINBOUNDARY holds", rc == TRUE && is(&ti.rclTrack, 100, 100, 210, 150));
            return (MRESULT)TRUE;
        case 'q':
            check("WinShowTrackRect without a tracking is FALSE", WinShowTrackRect(hwnd, FALSE) == FALSE);
            printf("pm_track_test: done %d/%d\n", passed, total);
            fflush(stdout);
            WinPostMsg(hwnd, WM_QUIT, 0, 0);
            return (MRESULT)TRUE;
        }
        break;
    }
    return WinDefWindowProc(hwnd, msg, mp1, mp2);
}

int main(void)
{
    HAB  hab = WinInitialize(0);
    HMQ  hmq = WinCreateMsgQueue(hab, 0);
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_SHELLPOSITION | FCF_TASKLIST;
    HWND hwndFrame;
    QMSG qmsg;

    WinRegisterClass(hab, "TrackTest", ClientProc, CS_SIZEREDRAW, 0);
    hwndFrame = WinCreateStdWindow(HWND_DESKTOP, WS_VISIBLE, &flags, "TrackTest",
                                   "WinTrackRect", 0, NULLHANDLE, 0, &hwndClient);
    WinSetFocus(HWND_DESKTOP, hwndClient);
    while (WinGetMsg(hab, &qmsg, NULLHANDLE, 0, 0))
        WinDispatchMsg(hab, &qmsg);
    WinDestroyWindow(hwndFrame);
    WinDestroyMsgQueue(hmq);
    WinTerminate(hab);
    return 0;
}
