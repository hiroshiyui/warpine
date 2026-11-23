/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * ansi_test.c — ANSI escape sequences in VIO TTY output.
 *
 * OS/2 carries out ANSI escape sequences written with VioWrtTTY (and
 * DosWrite to the screen) while ANSI support is on — the default. This
 * sample writes sequences through both calls and reads the screen back with
 * VioReadCellStr / VioGetCurPos: cursor movement (CUP/HVP, CUU/CUD/CUF/CUB,
 * clamped at the edges), save / restore, ED (ESC[2J), EL (ESC[K), SGR
 * colours, wrap off (ESC[=7l), the text display modes (ESC[=1h, ESC[=3h),
 * a sequence split across two writes, and
 * VioSetAnsi(ANSI_OFF) showing the sequence as characters.
 *
 * The results are collected while the screen is in use and written at the
 * end, one "ok" / "FAIL" line per check, to standard output and to
 * ANSI_TEST.OUT in the current directory, then "done: N failed".
 */

#define INCL_DOS
#define INCL_VIO
#include <os2.h>

#define ESC "\x1b"

static char results[4096];
static ULONG results_len;
static int failed;

static ULONG slen(const char *s) { ULONG n = 0; while (s[n]) n++; return n; }

static void add(const char *s) {
    ULONG n = slen(s);
    if (results_len + n < sizeof(results)) {
        ULONG i;
        for (i = 0; i < n; i++) results[results_len + i] = s[i];
        results_len += n;
    }
}

static void add_num(ULONG v) {
    char buf[12];
    int i = 11;
    buf[i--] = 0;
    if (v == 0) buf[i--] = '0';
    while (v > 0) { buf[i--] = (char)('0' + v % 10); v /= 10; }
    add(&buf[i + 1]);
}

static void add_hex(ULONG v) {
    static const char digits[] = "0123456789ABCDEF";
    char buf[3];
    buf[0] = digits[(v >> 4) & 0xF];
    buf[1] = digits[v & 0xF];
    buf[2] = 0;
    add("0x");
    add(buf);
}

static void check(const char *label, int pass) {
    add(pass ? "ok " : "FAIL ");
    add(label);
    add("\r\n");
    if (!pass) failed++;
}

static void tty(const char *s) { VioWrtTTY((PCH)s, (USHORT)slen(s), 0); }

static void dos_write(const char *s) {
    ULONG actual;
    DosWrite(1, (PVOID)s, slen(s), &actual);
}

static void cur(USHORT *row, USHORT *col) { VioGetCurPos(row, col, 0); }

/* The character and attribute of cell (row, col). */
static void cell(USHORT row, USHORT col, UCHAR *ch, UCHAR *attr) {
    UCHAR buf[2];
    USHORT cb = sizeof(buf);
    buf[0] = buf[1] = 0;
    VioReadCellStr((PCH)buf, &cb, row, col, 0);
    *ch = buf[0];
    *attr = buf[1];
}

static int cursor_at(USHORT row, USHORT col) {
    USHORT r, c;
    cur(&r, &c);
    if (r != row || c != col) {
        add("  cursor at (");
        add_num(r); add(", "); add_num(c); add(")\r\n");
    }
    return r == row && c == col;
}

static int cell_is(USHORT row, USHORT col, UCHAR want_ch, UCHAR want_attr) {
    UCHAR ch, attr;
    cell(row, col, &ch, &attr);
    if (ch != want_ch || attr != want_attr) {
        add("  cell ("); add_num(row); add(", "); add_num(col); add(") = ");
        add_hex(ch); add("/"); add_hex(attr); add("\r\n");
    }
    return ch == want_ch && attr == want_attr;
}

int main(void) {
    USHORT ansi = 0xFFFF;
    UCHAR ch, attr;
    HFILE hf;
    ULONG action, actual;
    VIOMODEINFO mode;
    USHORT last_col;

    mode.cb = sizeof(mode);
    VioGetMode(&mode, 0);
    last_col = (USHORT)(mode.col - 1);

    VioGetAnsi(&ansi, 0);
    check("ANSI is on by default", ansi == ANSI_ON);

    /* ED: clear the screen and home the cursor. */
    tty("junk" ESC "[2J");
    check("ESC[2J homes the cursor", cursor_at(0, 0));
    check("ESC[2J clears the screen", cell_is(0, 0, ' ', 0x07));

    /* CUP / HVP (1-based; empty and 0 parameters mean 1). */
    tty(ESC "[5;10H");
    check("ESC[5;10H", cursor_at(4, 9));
    tty("AB");
    check("text at the cursor", cell_is(4, 9, 'A', 0x07) && cell_is(4, 10, 'B', 0x07));
    tty(ESC "[;7f");
    check("ESC[;7f", cursor_at(0, 6));
    tty(ESC "[H");
    check("ESC[H", cursor_at(0, 0));

    /* CUU / CUD / CUF / CUB, clamped at the screen's edges. */
    tty(ESC "[10;10H" ESC "[2A");
    check("ESC[2A", cursor_at(7, 9));
    tty(ESC "[3B");
    check("ESC[3B", cursor_at(10, 9));
    tty(ESC "[4C");
    check("ESC[4C", cursor_at(10, 13));
    tty(ESC "[D");
    check("ESC[D (default 1)", cursor_at(10, 12));
    tty(ESC "[99A");
    check("ESC[99A stops at the top", cursor_at(0, 12));
    tty(ESC "[999C");
    check("ESC[999C stops at the right edge", cursor_at(0, last_col));
    tty(ESC "[999D");
    check("ESC[999D stops at the left edge", cursor_at(0, 0));

    /* Save / restore. */
    tty(ESC "[3;4H" ESC "[s" ESC "[20;1H" ESC "[u");
    check("ESC[s / ESC[u", cursor_at(2, 3));

    /* SGR. */
    tty(ESC "[6;1H" ESC "[1;31;44mX" ESC "[0mY" ESC "[7mZ" ESC "[0;33m" "W" ESC "[m");
    check("ESC[1;31;44m = bright red on blue", cell_is(5, 0, 'X', 0x1C));
    check("ESC[0m = light grey on black", cell_is(5, 1, 'Y', 0x07));
    check("ESC[7m = reverse video", cell_is(5, 2, 'Z', 0x70));
    check("ESC[33m = brown", cell_is(5, 3, 'W', 0x06));

    /* EL: erase from the cursor to the end of the line. */
    tty(ESC "[7;1HHELLO" ESC "[7;3H" ESC "[K");
    check("ESC[K keeps the start of the line", cell_is(6, 1, 'E', 0x07));
    check("ESC[K erases the rest", cell_is(6, 2, ' ', 0x07) && cell_is(6, 4, ' ', 0x07));
    check("ESC[K leaves the cursor", cursor_at(6, 2));

    /* A sequence split across two writes. */
    tty(ESC "[");
    tty("12;2");
    tty("0H");
    check("a sequence split across writes", cursor_at(11, 19));

    /* Wrap off: text stays in the last column. */
    tty(ESC "[=7l" ESC "[9;1H" ESC "[999C" "PQ");
    check("ESC[=7l: no wrap", cursor_at(8, last_col) && cell_is(8, last_col, 'Q', 0x07));
    tty(ESC "[=7h" ESC "[9;1H" ESC "[999C" "PQ");
    check("ESC[=7h: wrap again", cursor_at(9, 1) && cell_is(9, 0, 'Q', 0x07));

    /* Display modes: 1 = 40 x 25 text, 3 = 80 x 25 text, both cleared. */
    tty(ESC "[=1h");
    mode.cb = sizeof(mode);
    VioGetMode(&mode, 0);
    check("ESC[=1h: 40 x 25", mode.col == 40 && mode.row == 25 && cursor_at(0, 0));
    tty(ESC "[=3h");
    VioGetMode(&mode, 0);
    check("ESC[=3h: 80 x 25 again", mode.col == 80 && mode.row == 25);

    /* TAB: to the next multiple of 8. */
    tty(ESC "[11;3H\tT");
    check("TAB to column 8", cell_is(10, 8, 'T', 0x07));

    /* DosWrite to the screen interprets ANSI too. */
    dos_write(ESC "[13;5H" ESC "[32mD" ESC "[0m");
    check("DosWrite: ESC[13;5H and colour", cell_is(12, 4, 'D', 0x02));
    check("DosWrite: cursor after the text", cursor_at(12, 5));

    /* ANSI off: the sequence is written as characters. */
    VioSetAnsi(ANSI_OFF, 0);
    VioGetAnsi(&ansi, 0);
    check("VioSetAnsi(ANSI_OFF)", ansi == ANSI_OFF);
    tty(ESC "[15;1H" "x");
    cell(12, 5, &ch, &attr);
    check("ANSI off: ESC is a character", ch == 0x1B);
    check("ANSI off: the rest follows", cell_is(12, 6, '[', 0x07) && cell_is(12, 11, 'H', 0x07) && cell_is(12, 12, 'x', 0x07));
    check("VioSetAnsi(2) is rejected", VioSetAnsi(2, 0) != 0);
    VioSetAnsi(ANSI_ON, 0);

    /* Leave a clean screen for the result lines. */
    tty(ESC "[2J");
    /* A byte above 0x7F: a host pipe gets it in the process codepage
       (CP437 0x9B is a cent sign, CP850 0x9B an o with a stroke). */
    dos_write("high byte: \x9b\r\n");

    add("done: ");
    add_num((ULONG)failed);
    add(" failed\r\n");
    DosWrite(1, results, results_len, &actual);
    if (DosOpen((PSZ)"ANSI_TEST.OUT", &hf, &action, 0, FILE_NORMAL,
                OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYWRITE, NULL) == 0) {
        DosWrite(hf, results, results_len, &actual);
        DosClose(hf);
    }
    return failed;
}
