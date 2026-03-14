/*
 * screen_test.c — Verify VIO screen mode, resolution, and cursor APIs.
 *
 * Tests that warpine's VIO subsystem correctly reports screen dimensions
 * and handles basic cursor/screen operations. This is a 32-bit LX app
 * that calls VIO APIs directly (no 16-bit thunks).
 *
 * Tests:
 *  1. VioGetMode — screen columns, rows, resolution
 *  2. VioGetCurPos / VioSetCurPos — cursor positioning
 *  3. VioWrtTTY — direct screen output
 *  4. VioGetConfig — video adapter info
 *  5. BDA (BIOS Data Area) readback — verify 0x44A/0x484 values
 *  6. VioSetCurType — cursor shape
 */

#define INCL_VIO
#define INCL_DOS
#include <os2.h>

static ULONG dummy;

static void print(const char *msg) {
    const char *p = msg;
    ULONG len = 0;
    while (*p++) len++;
    DosWrite(1, (PVOID)msg, len, &dummy);
}

static void print_num(ULONG val) {
    char buf[12];
    int i = 11;
    buf[i--] = 0;
    if (val == 0) { buf[i--] = '0'; }
    else { while (val > 0) { buf[i--] = '0' + (val % 10); val /= 10; } }
    print(&buf[i + 1]);
}

static void print_hex(ULONG val) {
    char buf[9];
    int i;
    for (i = 7; i >= 0; i--) {
        int nib = val & 0xF;
        buf[i] = (nib < 10) ? ('0' + nib) : ('A' + nib - 10);
        val >>= 4;
    }
    buf[8] = 0;
    print("0x");
    print(buf);
}

static void check(const char *label, int pass, int *passed, int *failed) {
    print("  ");
    print(label);
    if (pass) { print(" OK\r\n"); (*passed)++; }
    else { print(" FAILED\r\n"); (*failed)++; }
}

int main(void) {
    int passed = 0;
    int failed = 0;
    VIOMODEINFO vioMode;
    VIOCURSORINFO curInfo;
    VIOCONFIGINFO cfgInfo;
    USHORT row, col;
    APIRET rc;

    print("=== Screen Mode Test ===\r\n\r\n");

    /* ── Test 1: VioGetMode ── */
    print("Test 1: VioGetMode\r\n");
    vioMode.cb = sizeof(vioMode);
    rc = VioGetMode(&vioMode, 0);
    print("  rc="); print_num(rc); print("\r\n");
    check("VioGetMode returns 0", rc == 0, &passed, &failed);

    print("  cb="); print_num(vioMode.cb); print("\r\n");
    print("  fbType="); print_num(vioMode.fbType); print("\r\n");
    print("  color="); print_num(vioMode.color); print("\r\n");
    print("  col="); print_num(vioMode.col); print("\r\n");
    print("  row="); print_num(vioMode.row); print("\r\n");
    print("  hres="); print_num(vioMode.hres); print("\r\n");
    print("  vres="); print_num(vioMode.vres); print("\r\n");

    check("col >= 40", vioMode.col >= 40, &passed, &failed);
    check("col <= 256", vioMode.col <= 256, &passed, &failed);
    check("row >= 20", vioMode.row >= 20, &passed, &failed);
    check("row <= 100", vioMode.row <= 100, &passed, &failed);
    check("fbType == 1 (text)", vioMode.fbType == 1, &passed, &failed);
    check("color == 4 (16-color)", vioMode.color == 4, &passed, &failed);

    /* ── Test 2: VioGetCurPos / VioSetCurPos ── */
    print("\r\nTest 2: VioGetCurPos / VioSetCurPos\r\n");
    rc = VioGetCurPos(&row, &col, 0);
    print("  GetCurPos rc="); print_num(rc);
    print(" row="); print_num(row);
    print(" col="); print_num(col);
    print("\r\n");
    check("VioGetCurPos returns 0", rc == 0, &passed, &failed);

    /* Move cursor to a specific position */
    rc = VioSetCurPos(5, 10, 0);
    check("VioSetCurPos(5,10) returns 0", rc == 0, &passed, &failed);

    /* Verify cursor moved */
    rc = VioGetCurPos(&row, &col, 0);
    print("  After SetCurPos: row="); print_num(row);
    print(" col="); print_num(col); print("\r\n");
    check("Cursor at row 5", row == 5, &passed, &failed);
    check("Cursor at col 10", col == 10, &passed, &failed);

    /* Reset cursor to 0,0 */
    VioSetCurPos(0, 0, 0);

    /* ── Test 3: VioWrtTTY ── */
    print("\r\nTest 3: VioWrtTTY\r\n");
    rc = VioWrtTTY("VioWrtTTY output test\r\n", 22, 0);
    check("VioWrtTTY returns 0", rc == 0, &passed, &failed);

    /* ── Test 4: VioGetConfig ── */
    print("\r\nTest 4: VioGetConfig\r\n");
    cfgInfo.cb = sizeof(cfgInfo);
    rc = VioGetConfig(0, &cfgInfo, 0);
    print("  rc="); print_num(rc);
    print(" adapter="); print_num(cfgInfo.adapter);
    print(" display="); print_num(cfgInfo.display);
    print("\r\n");
    check("VioGetConfig returns 0", rc == 0, &passed, &failed);

    /* ── Test 5: BDA readback ── */
    print("\r\nTest 5: BDA (BIOS Data Area) readback\r\n");
    {
        unsigned char  *bda_mode = (unsigned char *)0x449;
        unsigned short *bda_cols = (unsigned short *)0x44A;
        unsigned char  *bda_rows_m1 = (unsigned char *)0x484;

        print("  BDA video mode: "); print_hex(*bda_mode); print("\r\n");
        print("  BDA columns: "); print_num(*bda_cols); print("\r\n");
        print("  BDA rows-1: "); print_num(*bda_rows_m1); print("\r\n");

        check("BDA mode == 0x03 (VGA text)", *bda_mode == 0x03, &passed, &failed);
        check("BDA columns > 0", *bda_cols > 0, &passed, &failed);
        check("BDA columns matches VioGetMode", *bda_cols == vioMode.col, &passed, &failed);
        check("BDA rows matches VioGetMode", (*bda_rows_m1 + 1) == vioMode.row, &passed, &failed);
    }

    /* ── Test 6: VioSetCurType ── */
    print("\r\nTest 6: VioSetCurType\r\n");
    curInfo.yStart = 0;
    curInfo.cEnd = 15;
    curInfo.cx = 1;
    curInfo.attr = 0; /* visible */
    rc = VioSetCurType(&curInfo, 0);
    check("VioSetCurType returns 0", rc == 0, &passed, &failed);

    /* ── Summary ── */
    print("\r\n=== Results ===\r\n");
    print("Passed: "); print_num(passed); print("\r\n");
    print("Failed: "); print_num(failed); print("\r\n");

    if (failed == 0) { print("\r\nAll tests PASSED!\r\n"); }
    else { print("\r\nSome tests FAILED!\r\n"); }

    DosExit(EXIT_PROCESS, failed > 0 ? 1 : 0);
    return 0;
}
