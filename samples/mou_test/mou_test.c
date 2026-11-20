/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * mou_test.c - MOUCALLS mouse API for a VIO program.
 *
 * Opens the mouse, checks the results of the Mou* calls (initial state after
 * MouOpen, settings and their validation, the queue), then reads the events
 * a GUI script produces and checks their fs bits and text cells:
 *
 *   a click on cell (5, 10)           - motion to the cell, button 1 down, release
 *   a right click on cell (7, 20)     - button 2 down, release
 *   a drag from (9, 30) to (9, 40)    - button 1 down, motion with button 1, release
 *
 * Each result is one "ok ..." or "FAIL ..." line, written to the screen and
 * to C:\MOU_TEST.OUT (the screen of a VIO program is not its stdout under
 * Warpine's display, so the integration test reads the file). The last line
 * is "done: N failed".
 *
 * Build: make -C samples/mou_test   (uses 4OS2's bsesub.h: APIENTRY16 ->
 * _Pascal, so the Mou* calls are direct 32-bit imports, no 16-bit thunks).
 */

#define INCL_DOS
#define INCL_MOU
#define INCL_DOSERRORS
#include <os2.h>

#define ERROR_MOUSE_INV_HANDLE 386
#define ERROR_MOUSE_INV_PARMS  387
#define ERROR_MOUSE_INV_MASK   392

static HFILE out_file = 0;
static int failed = 0;

static ULONG len(const char *s) { ULONG n = 0; while (s[n]) n++; return n; }

static void put(const char *s)
{
    ULONG done;
    DosWrite(1, (PVOID)s, len(s), &done);
    if (out_file) DosWrite(out_file, (PVOID)s, len(s), &done);
}

static void put_num(long v)
{
    char buf[16];
    int i = 15;
    int neg = v < 0;
    unsigned long u = neg ? -v : v;
    buf[i--] = 0;
    do { buf[i--] = (char)('0' + u % 10); u /= 10; } while (u);
    if (neg) buf[i--] = '-';
    put(&buf[i + 1]);
}

static void check(const char *what, int pass, long got)
{
    put(pass ? "ok " : "FAIL ");
    put(what);
    if (!pass) { put(" (got "); put_num(got); put(")"); failed++; }
    put("\r\n");
}

/* Read the next event, waiting; skip motion without buttons unless wanted. */
static USHORT read_event(HMOU hmou, MOUEVENTINFO *ev)
{
    USHORT wait = MOU_WAIT;
    return MouReadEventQue(ev, &wait, hmou);
}

/* The next event that is not plain motion. */
static USHORT next_button_event(HMOU hmou, MOUEVENTINFO *ev, int *motion_seen,
                                int *drag_seen)
{
    USHORT rc;
    for (;;) {
        rc = read_event(hmou, ev);
        if (rc != 0) return rc;
        if (ev->fs == MOUSE_MOTION) { *motion_seen = 1; continue; }
        if (ev->fs == MOUSE_MOTION_WITH_BN1_DOWN) { *drag_seen = 1; continue; }
        return 0;
    }
}

static int is_at(MOUEVENTINFO *ev, int row, int col)
{
    return ev->row == row && ev->col == col;
}

int main(void)
{
    HMOU hmou = 0;
    USHORT rc, v, wait;
    PTRLOC pos;
    SCALEFACT scale;
    MOUQUEINFO qi;
    PTRSHAPE shape;
    BYTE image[16];
    NOPTRRECT rect;
    MOUEVENTINFO ev;
    ULONG action;
    int motion, drag;

    if (DosOpen("C:\\MOU_TEST.OUT", &out_file, &action, 0, FILE_NORMAL,
                OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYWRITE, NULL) != 0)
        out_file = 0;

    put("=== MOUCALLS test ===\r\n");

    rc = MouOpen(NULL, &hmou);
    check("MouOpen", rc == 0, rc);

    rc = MouGetNumButtons(&v, hmou);
    check("MouGetNumButtons >= 2", rc == 0 && v >= 2, rc ? rc : v);
    rc = MouGetNumMickeys(&v, hmou);
    check("MouGetNumMickeys", rc == 0 && v > 0, rc);
    rc = MouGetEventMask(&v, hmou);
    check("MouOpen reports all events", rc == 0 && v == 0x7F, rc ? rc : v);
    rc = MouGetDevStatus(&v, hmou);
    check("MouGetDevStatus = 0", rc == 0 && v == 0, rc ? rc : v);
    rc = MouGetScaleFact(&scale, hmou);
    check("MouGetScaleFact = 16/8", rc == 0 && scale.rowScale == 16 && scale.colScale == 8, rc);
    rc = MouGetPtrPos(&pos, hmou);
    check("MouGetPtrPos centred (12, 40)", rc == 0 && pos.row == 12 && pos.col == 40,
          rc ? rc : pos.row * 100 + pos.col);

    pos.row = 2; pos.col = 3;
    rc = MouSetPtrPos(&pos, hmou);
    pos.row = pos.col = 0;
    MouGetPtrPos(&pos, hmou);
    check("MouSetPtrPos (2, 3)", rc == 0 && pos.row == 2 && pos.col == 3, rc);
    pos.row = 25; pos.col = 0;
    rc = MouSetPtrPos(&pos, hmou);
    check("MouSetPtrPos off screen", rc == ERROR_MOUSE_INV_PARMS, rc);

    v = 0x80;
    rc = MouSetEventMask(&v, hmou);
    check("MouSetEventMask reserved bit", rc == ERROR_MOUSE_INV_MASK, rc);
    v = 0x7F;
    rc = MouSetEventMask(&v, hmou);
    check("MouSetEventMask all", rc == 0, rc);

    v = 0x0004;
    rc = MouSetDevStatus(&v, hmou);
    check("MouSetDevStatus reserved bit", rc == ERROR_MOUSE_INV_PARMS, rc);

    scale.rowScale = 0; scale.colScale = 8;
    rc = MouSetScaleFact(&scale, hmou);
    check("MouSetScaleFact 0", rc == ERROR_MOUSE_INV_PARMS, rc);

    shape.cb = 2;
    rc = MouGetPtrShape(image, &shape, hmou);
    check("MouGetPtrShape small buffer", rc != 0 && shape.cb == 4, rc);
    shape.cb = sizeof(image);
    rc = MouGetPtrShape(image, &shape, hmou);
    check("MouGetPtrShape text 1x1", rc == 0 && shape.col == 1 && shape.row == 1, rc);
    rc = MouSetPtrShape(image, &shape, hmou);
    check("MouSetPtrShape", rc == 0, rc);

    rect.row = 0; rect.col = 0; rect.cRow = 0; rect.cCol = 79;
    rc = MouRemovePtr(&rect, hmou);
    check("MouRemovePtr", rc == 0, rc);
    rc = MouDrawPtr(hmou);
    check("MouDrawPtr", rc == 0, rc);

    rc = MouFlushQue(hmou);
    check("MouFlushQue", rc == 0, rc);
    rc = MouGetNumQueEl(&qi, hmou);
    check("MouGetNumQueEl empty", rc == 0 && qi.cEvents == 0 && qi.cmaxEvents > 0, rc);
    wait = MOU_NOWAIT;
    ev.time = 1;
    rc = MouReadEventQue(&ev, &wait, hmou);
    check("MouReadEventQue no wait: null record", rc == 0 && ev.time == 0 && ev.fs == 0, rc);
    rc = MouGetNumButtons(&v, (HMOU)(hmou + 1));
    check("bad handle", rc == ERROR_MOUSE_INV_HANDLE, rc);

    put("ready: waiting for mouse events\r\n");

    /* Click on (5, 10). */
    motion = drag = 0;
    rc = next_button_event(hmou, &ev, &motion, &drag);
    check("click: button 1 down at (5, 10)",
          rc == 0 && ev.fs == MOUSE_BN1_DOWN && is_at(&ev, 5, 10), rc ? rc : ev.fs);
    check("click: motion to the cell first", motion, 0);
    check("click: event time set", ev.time != 0, 0);
    rc = next_button_event(hmou, &ev, &motion, &drag);
    check("click: release at (5, 10)", rc == 0 && ev.fs == 0 && is_at(&ev, 5, 10), rc ? rc : ev.fs);

    /* Right click on (7, 20). */
    rc = next_button_event(hmou, &ev, &motion, &drag);
    check("rclick: button 2 down at (7, 20)",
          rc == 0 && ev.fs == MOUSE_BN2_DOWN && is_at(&ev, 7, 20), rc ? rc : ev.fs);
    rc = next_button_event(hmou, &ev, &motion, &drag);
    check("rclick: release", rc == 0 && ev.fs == 0 && is_at(&ev, 7, 20), rc ? rc : ev.fs);

    /* Drag from (9, 30) to (9, 40). */
    drag = 0;
    rc = next_button_event(hmou, &ev, &motion, &drag);
    check("drag: button 1 down at (9, 30)",
          rc == 0 && ev.fs == MOUSE_BN1_DOWN && is_at(&ev, 9, 30), rc ? rc : ev.fs);
    rc = next_button_event(hmou, &ev, &motion, &drag);
    check("drag: motion with button 1 down", drag, 0);
    check("drag: release at (9, 40)", rc == 0 && ev.fs == 0 && is_at(&ev, 9, 40), rc ? rc : ev.fs);

    rc = MouGetPtrPos(&pos, hmou);
    check("MouGetPtrPos follows the pointer", rc == 0 && pos.row == 9 && pos.col == 40, rc);

    rc = MouClose(hmou);
    check("MouClose", rc == 0, rc);
    rc = MouClose(hmou);
    check("MouClose twice", rc == ERROR_MOUSE_INV_HANDLE, rc);

    put("done: ");
    put_num(failed);
    put(" failed\r\n");
    if (out_file) DosClose(out_file);
    return failed ? 1 : 0;
}
