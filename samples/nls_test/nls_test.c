/*
 * nls_test.c — Verify NLS (National Language Support) and date/time APIs.
 *
 * Tests:
 *  1. DosQueryCp — codepage query
 *  2. DosQueryCtryInfo — country info (date/time separators, formats)
 *  3. DosGetDateTime — system date and time
 *  4. DosMapCase — uppercase conversion
 *  5. COUNTRYINFO field verification (separators, formats)
 *  6. Date formatting sanity check
 */

#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>


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

static void check(const char *label, int pass, int *passed, int *failed) {
    print("  ");
    print(label);
    if (pass) { print(" OK\r\n"); (*passed)++; }
    else { print(" FAILED\r\n"); (*failed)++; }
}

int main(void) {
    int passed = 0, failed = 0;
    ULONG codepages[4];
    ULONG numCP;
    COUNTRYCODE cc;
    COUNTRYINFO ci;
    ULONG infoLen;
    DATETIME dt;
    APIRET rc;
    char testBuf[10];

    print("=== NLS / Date-Time Test ===\r\n\r\n");

    /* ── Test 1: DosQueryCp ── */
    print("Test 1: DosQueryCp\r\n");
    memset(codepages, 0, sizeof(codepages));
    numCP = 0;
    rc = DosQueryCp(sizeof(codepages), codepages, &numCP);
    print("  rc="); print_num(rc); print("\r\n");
    print("  codepage[0]="); print_num(codepages[0]); print("\r\n");
    print("  numCP="); print_num(numCP); print("\r\n");
    check("DosQueryCp returns 0", rc == 0, &passed, &failed);
    check("codepage > 0", codepages[0] > 0, &passed, &failed);

    /* ── Test 2: DosQueryCtryInfo ── */
    print("\r\nTest 2: DosQueryCtryInfo\r\n");
    cc.country = 0;
    cc.codepage = 0;
    memset(&ci, 0, sizeof(ci));
    infoLen = 0;
    rc = DosQueryCtryInfo(sizeof(ci), &cc, &ci, &infoLen);
    print("  rc="); print_num(rc); print("\r\n");
    print("  infoLen="); print_num(infoLen); print("\r\n");
    check("DosQueryCtryInfo returns 0", rc == 0, &passed, &failed);

    /* ── Test 3: COUNTRYINFO fields ── */
    print("\r\nTest 3: COUNTRYINFO fields\r\n");
    print("  country="); print_num(ci.country); print("\r\n");
    print("  codepage="); print_num(ci.codepage); print("\r\n");
    print("  fsDateFmt="); print_num(ci.fsDateFmt); print("\r\n");
    print("  szCurrency='"); print(ci.szCurrency); print("'\r\n");
    print("  szThousandsSeparator='"); print(ci.szThousandsSeparator); print("'\r\n");
    print("  szDecimal='"); print(ci.szDecimal); print("'\r\n");
    print("  szDateSeparator='"); print(ci.szDateSeparator); print("'\r\n");
    print("  szTimeSeparator='"); print(ci.szTimeSeparator); print("'\r\n");
    print("  fsTimeFmt="); print_num(ci.fsTimeFmt); print("\r\n");

    check("country > 0", ci.country > 0, &passed, &failed);
    check("codepage > 0", ci.codepage > 0, &passed, &failed);
    check("szDateSeparator not empty", ci.szDateSeparator[0] != 0, &passed, &failed);
    check("szTimeSeparator not empty", ci.szTimeSeparator[0] != 0, &passed, &failed);
    check("szDecimal not empty", ci.szDecimal[0] != 0, &passed, &failed);

    /* ── Test 4: DosGetDateTime ── */
    print("\r\nTest 4: DosGetDateTime\r\n");
    memset(&dt, 0, sizeof(dt));
    rc = DosGetDateTime(&dt);
    print("  rc="); print_num(rc); print("\r\n");
    print("  date="); print_num(dt.year); print("-");
    print_num(dt.month); print("-"); print_num(dt.day); print("\r\n");
    print("  time="); print_num(dt.hours); print(":");
    print_num(dt.minutes); print(":"); print_num(dt.seconds); print("\r\n");
    check("DosGetDateTime returns 0", rc == 0, &passed, &failed);
    check("year >= 2020", dt.year >= 2020, &passed, &failed);
    check("month 1-12", dt.month >= 1 && dt.month <= 12, &passed, &failed);
    check("day 1-31", dt.day >= 1 && dt.day <= 31, &passed, &failed);

    /* ── Test 5: DosMapCase ── */
    print("\r\nTest 5: DosMapCase\r\n");
    strcpy(testBuf, "hello");
    rc = DosMapCase(5, &cc, testBuf);
    print("  rc="); print_num(rc); print("\r\n");
    print("  result='"); print(testBuf); print("'\r\n");
    check("DosMapCase returns 0", rc == 0, &passed, &failed);
    check("DosMapCase converts to HELLO", strcmp(testBuf, "HELLO") == 0, &passed, &failed);

    /* ── Test 6: Date format string ── */
    print("\r\nTest 6: Date format verification\r\n");
    {
        char dateStr[20];
        /* Format a date manually using the country info separators */
        if (ci.szDateSeparator[0] != 0) {
            char sep = ci.szDateSeparator[0];
            if (ci.fsDateFmt == 0) {
                /* MDY */
                sprintf(dateStr, "%02u%c%02u%c%04u", dt.month, sep, dt.day, sep, dt.year);
            } else if (ci.fsDateFmt == 1) {
                /* DMY */
                sprintf(dateStr, "%02u%c%02u%c%04u", dt.day, sep, dt.month, sep, dt.year);
            } else {
                /* YMD */
                sprintf(dateStr, "%04u%c%02u%c%02u", dt.year, sep, dt.month, sep, dt.day);
            }
            print("  Formatted date: "); print(dateStr); print("\r\n");
            check("Date string length > 8", strlen(dateStr) > 8, &passed, &failed);
        } else {
            print("  Cannot format date (no separator)\r\n");
            failed++;
        }
    }

    /* ── Summary ── */
    print("\r\n=== Results ===\r\n");
    print("Passed: "); print_num(passed); print("\r\n");
    print("Failed: "); print_num(failed); print("\r\n");
    if (failed == 0) { print("\r\nAll tests PASSED!\r\n"); }
    else { print("\r\nSome tests FAILED!\r\n"); }

    DosExit(EXIT_PROCESS, failed > 0 ? 1 : 0);
    return 0;
}
