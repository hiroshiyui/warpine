/*
 * dbcs_test.c — Verify DBCS VIO APIs: DosQueryDBCSEnv and VioCheckCharType.
 *
 * Tests:
 *  1. DosSetProcessCp(932) — switch active codepage to Shift-JIS
 *  2. DosQueryCp — confirm active codepage is now 932
 *  3. DosQueryDBCSEnv — retrieve CP932 lead-byte ranges; expect (0x81,0x9F) and (0xE0,0xFC)
 *  4. VioWrtCellStr — write SJIS pair 0x82,0xA0 (hiragana あ) + one SBCS space to row 2
 *  5. VioCheckCharType — col 0 must be 2 (DBCS-lead)
 *  6. VioCheckCharType — col 1 must be 3 (DBCS-trail)
 *  7. VioCheckCharType — col 2 must be 0 (SBCS)
 *  8. VioCheckCharType — out-of-bounds row must return non-zero error code
 */

#define INCL_DOS
#define INCL_DOSNLS
#define INCL_VIO
#include <os2.h>
#include <string.h>

/* VioCheckCharType may be absent in older Open Watcom OS/2 headers — declare manually */
#ifndef VioCheckCharType
APIRET APIENTRY VioCheckCharType(USHORT *pType, USHORT usRow, USHORT usCol, HVIO hvio);
#endif

static ULONG dummy;

static void print(const char *msg) {
    ULONG len = 0;
    const char *p = msg;
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

static void print_hex8(ULONG val) {
    char buf[5];
    int i;
    for (i = 3; i >= 0; i--) {
        int n = val & 0xF;
        buf[i] = (n < 10) ? ('0' + n) : ('A' + n - 10);
        val >>= 4;
    }
    buf[4] = 0;
    print("0x");
    print(buf);
}

static void check(const char *label, int pass, int *passed, int *failed) {
    print("  ");
    print(label);
    if (pass) { print(" OK\r\n"); (*passed)++; }
    else       { print(" FAILED\r\n"); (*failed)++; }
}

int main(void) {
    int passed = 0, failed = 0;
    APIRET rc;
    ULONG cpList[4];
    ULONG cpSize;
    COUNTRYCODE cc;
    char dbcsBuf[20];
    USHORT type;
    BYTE cells[6];

    print("=== DBCS VIO API Test ===\r\n\r\n");

    /* ── Test 1: DosSetProcessCp ── */
    print("Test 1: DosSetProcessCp(932)\r\n");
    rc = DosSetProcessCp(932);
    print("  rc="); print_num(rc); print("\r\n");
    check("DosSetProcessCp(932) returns 0", rc == 0, &passed, &failed);

    /* ── Test 2: DosQueryCp ── */
    print("\r\nTest 2: DosQueryCp — confirm CP932 active\r\n");
    memset(cpList, 0, sizeof(cpList));
    cpSize = 0;
    rc = DosQueryCp(sizeof(cpList), cpList, &cpSize);
    print("  rc="); print_num(rc);
    print("  codepage="); print_num(cpList[0]); print("\r\n");
    check("DosQueryCp returns 0", rc == 0, &passed, &failed);
    check("Active codepage is 932", cpList[0] == 932, &passed, &failed);

    /* ── Test 3: DosQueryDBCSEnv ── */
    print("\r\nTest 3: DosQueryDBCSEnv — CP932 lead-byte table\r\n");
    cc.country = 0;
    cc.codepage = 0; /* use current (932 from DosSetProcessCp above) */
    memset(dbcsBuf, 0, sizeof(dbcsBuf));
    rc = DosQueryDBCSEnv(sizeof(dbcsBuf), &cc, dbcsBuf);
    print("  rc="); print_num(rc); print("\r\n");
    check("DosQueryDBCSEnv returns 0", rc == 0, &passed, &failed);

    {
        unsigned char *p = (unsigned char *)dbcsBuf;
        int pairs = 0;
        int found81 = 0, foundE0 = 0;
        while (p[0] != 0 || p[1] != 0) {
            print("  range (");
            print_hex8(p[0]); print(","); print_hex8(p[1]); print(")\r\n");
            if (p[0] == 0x81 && p[1] == 0x9F) found81 = 1;
            if (p[0] == 0xE0 && p[1] == 0xFC) foundE0 = 1;
            pairs++;
            p += 2;
        }
        print("  terminator (0,0) present\r\n");
        check("CP932 has >= 2 lead-byte ranges", pairs >= 2, &passed, &failed);
        check("CP932 range (0x81,0x9F) present", found81, &passed, &failed);
        check("CP932 range (0xE0,0xFC) present", foundE0, &passed, &failed);
    }

    /* ── Test 4: Write DBCS pair to VIO buffer ── */
    print("\r\nTest 4: VioWrtCellStr — write SJIS 0x82,0xA0 + SBCS space at row 2, col 0\r\n");
    /* Each VIO cell is (char_byte, attr_byte). DBCS lead/trail each occupy one cell. */
    cells[0] = 0x82; cells[1] = 0x07; /* DBCS lead byte, grey-on-black */
    cells[2] = 0xA0; cells[3] = 0x07; /* DBCS trail byte, same attr   */
    cells[4] = ' ';  cells[5] = 0x07; /* SBCS space after the pair    */
    rc = VioWrtCellStr((PCH)cells, 6, 2, 0, 0);
    print("  VioWrtCellStr rc="); print_num(rc); print("\r\n");
    check("VioWrtCellStr returns 0", rc == 0, &passed, &failed);

    /* ── Test 5: col 0 is DBCS-lead ── */
    print("\r\nTest 5: VioCheckCharType(row=2, col=0) — expect DBCS-lead (2)\r\n");
    type = 0xFF;
    rc = VioCheckCharType(&type, 2, 0, 0);
    print("  rc="); print_num(rc); print("  type="); print_num(type); print("\r\n");
    check("VioCheckCharType(2,0) returns 0", rc == 0, &passed, &failed);
    check("col 0 classified as DBCS-lead (type==2)", type == 2, &passed, &failed);

    /* ── Test 6: col 1 is DBCS-trail ── */
    print("\r\nTest 6: VioCheckCharType(row=2, col=1) — expect DBCS-trail (3)\r\n");
    type = 0xFF;
    rc = VioCheckCharType(&type, 2, 1, 0);
    print("  rc="); print_num(rc); print("  type="); print_num(type); print("\r\n");
    check("VioCheckCharType(2,1) returns 0", rc == 0, &passed, &failed);
    check("col 1 classified as DBCS-trail (type==3)", type == 3, &passed, &failed);

    /* ── Test 7: col 2 is SBCS space ── */
    print("\r\nTest 7: VioCheckCharType(row=2, col=2) — expect SBCS (0)\r\n");
    type = 0xFF;
    rc = VioCheckCharType(&type, 2, 2, 0);
    print("  rc="); print_num(rc); print("  type="); print_num(type); print("\r\n");
    check("VioCheckCharType(2,2) returns 0", rc == 0, &passed, &failed);
    check("col 2 classified as SBCS (type==0)", type == 0, &passed, &failed);

    /* ── Test 8: out-of-bounds row returns error ── */
    print("\r\nTest 8: VioCheckCharType(row=9999) — expect non-zero error\r\n");
    type = 0xFF;
    rc = VioCheckCharType(&type, 9999, 0, 0);
    print("  rc="); print_num(rc); print("\r\n");
    check("Out-of-bounds row returns non-zero error", rc != 0, &passed, &failed);

    /* ── Summary ── */
    print("\r\n=== Results ===\r\n");
    print("Passed: "); print_num(passed); print("\r\n");
    print("Failed: "); print_num(failed); print("\r\n");
    if (failed == 0) print("\r\nAll tests PASSED!\r\n");
    else             print("\r\nSome tests FAILED!\r\n");

    {
        ULONG bytesRead;
        char ch;
        print("\r\nPress ENTER to exit...\r\n");
        DosRead(0, &ch, 1, &bytesRead);
    }

    DosExit(EXIT_PROCESS, failed > 0 ? 1 : 0);
    return 0;
}
