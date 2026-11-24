/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * vio_kbd_test.c - VIO text calls and the logical keyboard.
 *
 * VIO: VioWrtCharStr / VioWrtNChar / VioReadCharStr (wrapping at the end of
 * a line, stopping at the end of the screen, keeping attributes, row and
 * column errors), VioScrollLf / VioScrollRt and VioScrollUp inside a
 * rectangle (0 lines, limits beyond the screen), VioGetCp, VioPopUp /
 * VioEndPopUp (the screen saved and put back), VioScrLock / VioScrUnLock
 * and VioGetState.
 * KBD: KbdGetStatus / KbdSetStatus (masks, lengths, the turnaround
 * character), KbdGetCp / KbdSetCp, KbdGetHWID, KbdOpen / KbdClose,
 * KbdGetFocus / KbdFreeFocus, KbdSynch, and KbdPeek / KbdCharIn /
 * KbdFlushBuffer on the keys "ab" the test pipes in, then KbdStringIn on
 * the keys that follow ("xz", Backspace, "y", CR, then "q", Backspace,
 * ";"): in ASCII mode (the default, with echo) Backspace edits and CR ends
 * the line, echoed to the screen; in binary mode every byte is kept, none
 * echoed, until the buffer is full.
 *
 * Each check prints "<name> OK" or "<name> FAILED", then
 * "Passed: n" / "Failed: n".
 */

#define INCL_VIO
#define INCL_KBD
#define INCL_DOS
#include <os2.h>

#ifdef __WATCOMC__
/* Open Watcom declares this one as KbdGetHWId, so its KbdGetHWID macro has
 * no prototype: declare the import it names. */
USHORT _Pascal KBD16GETHWID(PKBDHWID, HKBD);
#define GET_HWID KBD16GETHWID
#else
#define GET_HWID KbdGetHWID
#endif

static ULONG dummy;
static int passed, failed;

static void print(const char *msg)
{
    ULONG len = 0;
    while (msg[len]) len++;
    DosWrite(1, (PVOID)msg, len, &dummy);
}

static void check(const char *label, int pass)
{
    print(label);
    if (pass) { print(" OK\r\n"); passed++; }
    else      { print(" FAILED\r\n"); failed++; }
}

static int same(const char *a, const char *b, int n)
{
    int i;
    for (i = 0; i < n; i++) if (a[i] != b[i]) return 0;
    return 1;
}

/* The n characters at (row, col), or 0 when the read fails. */
static int read_at(USHORT row, USHORT col, char *buf, USHORT n)
{
    USHORT cb = n;
    return VioReadCharStr(buf, &cb, row, col, 0) == 0 && cb == n;
}

static void vio_checks(void)
{
    VIOMODEINFO mode;
    char buf[16];
    BYTE cell[2];
    BYTE attr = 0x1E;
    USHORT cb, cp, rows, cols;
    BYTE cells[4];
    UCHAR not_locked = 9;
    USHORT opts;

    mode.cb = sizeof(mode);
    VioGetMode(&mode, 0);
    rows = mode.row;
    cols = mode.col;

    check("VioWrtCharStr", VioWrtCharStr("HELLO", 5, 2, 0, 0) == 0);
    check("VioReadCharStr reads it back", read_at(2, 0, buf, 5) && same(buf, "HELLO", 5));
    VioWrtNAttr(&attr, 5, 2, 0, 0);
    VioWrtCharStr("WORLD", 5, 2, 0, 0);
    cb = sizeof(cells);
    VioReadCellStr((PCH)cells, &cb, 2, 0, 0);
    check("VioWrtCharStr keeps the attribute", cells[0] == 'W' && cells[1] == 0x1E);
    VioWrtCharStr("XY", 2, 3, cols - 1, 0);
    check("a string wraps to the next line", read_at(4, 0, buf, 1) && buf[0] == 'Y');
    check("VioWrtNChar", VioWrtNChar((PBYTE)"*", 3, 5, 0, 0) == 0 && read_at(5, 0, buf, 3) && same(buf, "***", 3));
    cb = 10;
    check("a read stops at the end of the screen",
          VioReadCharStr(buf, &cb, rows - 1, cols - 3, 0) == 0 && cb == 3);
    check("a row beyond the screen", VioWrtCharStr("x", 1, rows, 0, 0) == 358);
    check("a column beyond the screen", VioWrtCharStr("x", 1, 0, cols, 0) == 359);

    cell[0] = '.'; cell[1] = 0x07;
    VioWrtCharStr("ABCDE", 5, 6, 0, 0);
    check("VioScrollLf", VioScrollLf(6, 0, 6, 4, 1, cell, 0) == 0 && read_at(6, 0, buf, 5) && same(buf, "BCDE.", 5));
    check("VioScrollRt", VioScrollRt(6, 0, 6, 4, 2, cell, 0) == 0 && read_at(6, 0, buf, 5) && same(buf, "..BCD", 5));
    VioWrtCharStr("abcd", 4, 7, 0, 0);
    VioWrtCharStr("efgh", 4, 8, 0, 0);
    VioScrollUp(7, 1, 8, 2, 1, cell, 0);
    check("VioScrollUp stays inside its columns",
          read_at(7, 0, buf, 4) && same(buf, "afgd", 4) && read_at(8, 0, buf + 4, 4) && same(buf + 4, "e..h", 4));
    VioScrollUp(7, 0, 8, 3, 0, cell, 0);
    check("0 lines scroll nothing", read_at(7, 0, buf, 4) && same(buf, "afgd", 4));
    cell[0] = '#';
    VioScrollUp(10, 0, 0xFFFF, 0xFFFF, 0xFFFF, cell, 0);
    check("limits beyond the screen fill to its end",
          read_at(rows - 1, cols - 1, buf, 1) && buf[0] == '#' && read_at(9, 0, buf + 1, 1) && buf[1] != '#');

    check("VioGetCp", VioGetCp(0, &cp, 0) == 0 && cp != 0);
    check("VioSetCp", VioSetCp(0, 850, 0) == 0 && VioGetCp(0, &cp, 0) == 0 && cp == 850);
    check("VioSetCp refuses an unknown codepage", VioSetCp(0, 12345, 0) == 469);
    VioSetCp(0, 0, 0); /* back to the process codepage */

    VioWrtCharStr("SAVE", 4, 0, 0, 0);
    opts = 1; /* wait, not transparent */
    {
        /* Read the pop-up screen before printing: the check lines go to the
         * screen too. */
        int rc = VioPopUp(&opts, 0);
        int blank = read_at(0, 0, buf, 4) && same(buf, "    ", 4);
        check("VioPopUp", rc == 0);
        check("the pop-up screen starts blank", blank);
    }
    check("a second pop-up is refused", VioPopUp(&opts, 0) == 406);
    VioWrtCharStr("POP!", 4, 0, 0, 0);
    check("VioEndPopUp", VioEndPopUp(0) == 0);
    check("the screen comes back", read_at(0, 0, buf, 4) && same(buf, "SAVE", 4));
    check("VioEndPopUp without a pop-up", VioEndPopUp(0) == 405);

    check("VioScrLock", VioScrLock(1, &not_locked, 0) == 0 && not_locked == 0);
    check("VioScrUnLock", VioScrUnLock(0) == 0);
    check("VioScrUnLock without the lock", VioScrUnLock(0) == 367);

    {
        VIOOVERSCAN overscan;
        VIOINTENSITY intensity;
        overscan.cb = sizeof(overscan); overscan.type = 1;
        intensity.cb = sizeof(intensity); intensity.type = 2;
        check("VioGetState overscan", VioGetState(&overscan, 0) == 0);
        check("VioGetState intensity", VioGetState(&intensity, 0) == 0);
        overscan.color = 3;
        check("VioSetState overscan", VioSetState(&overscan, 0) == 0);
        overscan.color = 0;
        check("VioGetState reports it", VioGetState(&overscan, 0) == 0 && overscan.color == 3);
    }
}

static void kbd_checks(void)
{
    KBDINFO info;
    KBDHWID hwid;
    KBDKEYINFO key;
    USHORT cp = 0;
    HKBD h = 0;

    info.cb = sizeof(info);
    check("KbdGetStatus", KbdGetStatus(&info, 0) == 0 && info.cb == 10);
    check("the default: ASCII mode, echo on",
          info.fsMask == (KEYBOARD_ECHO_ON | KEYBOARD_ASCII_MODE) && info.chTurnAround == 0x0D);
    info.cb = sizeof(info);
    info.fsMask = KEYBOARD_ECHO_ON | KEYBOARD_ECHO_OFF;
    check("echo on and off together", KbdSetStatus(&info, 0) == 377);
    info.fsMask = KEYBOARD_BINARY_MODE | KEYBOARD_ASCII_MODE;
    check("binary and ASCII together", KbdSetStatus(&info, 0) == 378);
    info.cb = 8;
    info.fsMask = KEYBOARD_ECHO_ON;
    check("a short KBDINFO", KbdSetStatus(&info, 0) == 376);
    info.cb = sizeof(info);
    info.fsMask = KEYBOARD_MODIFY_TURNAROUND;
    info.chTurnAround = 0x1B;
    check("KbdSetStatus turnaround", KbdSetStatus(&info, 0) == 0);
    info.cb = sizeof(info);
    KbdGetStatus(&info, 0);
    check("the turnaround character is kept", info.chTurnAround == 0x1B);

    check("KbdSetCp", KbdSetCp(0, 850, 0) == 0);
    check("KbdGetCp", KbdGetCp(0, &cp, 0) == 0 && cp == 850);
    check("an unknown codepage", KbdSetCp(0, 12345, 0) == 448);

    hwid.cb = sizeof(hwid);
    check("KbdGetHWID", GET_HWID(&hwid, 0) == 0 && hwid.cb == sizeof(hwid));

    check("KbdOpen", KbdOpen(&h) == 0 && h != 0);
    check("KbdGetFocus", KbdGetFocus(IO_WAIT, h) == 0);
    check("KbdFreeFocus", KbdFreeFocus(h) == 0);
    check("KbdClose", KbdClose(h) == 0);
    check("a closed handle", KbdClose(h) == 439);
    check("KbdSynch", KbdSynch(IO_WAIT) == 0);
    {
        KBDTRANS t = { 0 };
        t.chScan = 0x1E; /* A */
        check("KbdXlate", KbdXlate(&t, 0) == 0 && t.chChar == 'a' && (t.fbStatus & KBDTRF_FINAL_CHAR_IN));
        t.fsState = KBDSTF_LEFTSHIFT;
        check("KbdXlate with Shift", KbdXlate(&t, 0) == 0 && t.chChar == 'A');
    }

    /* The test pipes in "ab". */
    check("KbdPeek sees the first key", KbdPeek(&key, 0) == 0 && key.chChar == 'a' && (key.fbStatus & 0x40));
    check("KbdPeek leaves it", KbdPeek(&key, 0) == 0 && key.chChar == 'a');
    check("KbdCharIn takes it", KbdCharIn(&key, IO_WAIT, 0) == 0 && key.chChar == 'a');
    check("then the next key", KbdCharIn(&key, IO_WAIT, 0) == 0 && key.chChar == 'b');
    {
        static const char blank[4] = { ' ', ' ', ' ', ' ' };
        char line[8], scr[4];
        STRINGINBUF sib;
        BYTE space = ' ';
        USHORT rc;
        int shown;
        /* ASCII mode with echo, CR as the turnaround character again. */
        info.cb = sizeof(info);
        info.fsMask = KEYBOARD_ECHO_ON | KEYBOARD_ASCII_MODE | KEYBOARD_MODIFY_TURNAROUND;
        info.chTurnAround = 0x0D;
        KbdSetStatus(&info, 0);
        VioWrtNChar(&space, 80, 20, 0, 0);
        VioSetCurPos(20, 0, 0);
        sib.cb = sizeof(line);
        sib.cchIn = 0;
        /* The screen is read before any result is printed (printing
           writes at the cursor). */
        rc = KbdStringIn(line, &sib, IO_WAIT, 0);
        shown = read_at(20, 0, scr, 4) && same(scr, "xy  ", 4);
        check("KbdStringIn (ASCII)", rc == 0);
        check("-> Backspace edits, CR ends the line", sib.cchIn == 2 && same(line, "xy", 2));
        check("-> echoed", shown);
        /* Binary mode: no editing, no echo, until the buffer is full. */
        info.fsMask = KEYBOARD_BINARY_MODE;
        KbdSetStatus(&info, 0);
        VioWrtNChar(&space, 80, 21, 0, 0);
        VioSetCurPos(21, 0, 0);
        sib.cb = 3;
        sib.cchIn = 0;
        rc = KbdStringIn(line, &sib, IO_WAIT, 0);
        shown = !(read_at(21, 0, scr, 4) && same(scr, blank, 4));
        check("KbdStringIn (binary)", rc == 0);
        check("-> every byte, until the buffer is full",
              sib.cchIn == 3 && line[0] == 'q' && line[1] == 0x08 && line[2] == ';');
        check("-> not echoed", !shown);
        info.fsMask = KEYBOARD_ASCII_MODE;
        KbdSetStatus(&info, 0);
    }
    check("KbdFlushBuffer", KbdFlushBuffer(0) == 0);
}

int main(void)
{
    char num[12];
    int i;

    vio_checks();
    kbd_checks();

    print("Passed: ");
    i = 11; num[i--] = 0;
    if (passed == 0) num[i--] = '0';
    while (passed > 0) { num[i--] = '0' + passed % 10; passed /= 10; }
    print(&num[i + 1]);
    print("\r\nFailed: ");
    i = 11; num[i--] = 0;
    if (failed == 0) num[i--] = '0';
    { int f = failed; while (f > 0) { num[i--] = '0' + f % 10; f /= 10; } }
    print(&num[i + 1]);
    print("\r\n");
    return failed;
}
