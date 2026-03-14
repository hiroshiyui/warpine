/*
 * findbuf_test.c — Verify FILEFINDBUF3/FILEFINDBUF4 layout from DosFindFirst.
 *
 * Calls DosFindFirst with FIL_STANDARD (level 1) and FIL_QUERYEASIZE (level 2),
 * then dumps the raw bytes of the returned buffer to verify field offsets.
 * Also checks individual fields by name to detect layout mismatches.
 */

#define INCL_DOS
#define INCL_DOSERRORS
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

static void print_hex8(unsigned char val) {
    char buf[3];
    int hi = (val >> 4) & 0xF;
    int lo = val & 0xF;
    buf[0] = hi < 10 ? '0' + hi : 'A' + hi - 10;
    buf[1] = lo < 10 ? '0' + lo : 'A' + lo - 10;
    buf[2] = 0;
    print(buf);
}

static void print_hex32(ULONG val) {
    print_hex8((val >> 24) & 0xFF);
    print_hex8((val >> 16) & 0xFF);
    print_hex8((val >> 8) & 0xFF);
    print_hex8(val & 0xFF);
}

static void dump_bytes(unsigned char *buf, ULONG len, ULONG per_line) {
    ULONG i;
    for (i = 0; i < len; i++) {
        if (i > 0 && (i % per_line) == 0) print("\r\n    ");
        print_hex8(buf[i]);
        print(" ");
    }
    print("\r\n");
}

static void check(const char *label, int pass, int *passed, int *failed) {
    print("  ");
    print(label);
    if (pass) { print(" OK\r\n"); (*passed)++; }
    else { print(" FAILED\r\n"); (*failed)++; }
}

int main(void) {
    FILEFINDBUF3 fb3;
    FILEFINDBUF4 fb4;
    HDIR hdir;
    ULONG count;
    APIRET rc;
    int passed = 0, failed = 0;
    unsigned char *raw;

    /* Create a test file first */
    {
        HFILE hf;
        ULONG action, written;
        rc = DosOpen("C:\\FINDTEST.DAT", &hf, &action, 0, FILE_NORMAL,
                     OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                     OPEN_SHARE_DENYNONE | OPEN_ACCESS_READWRITE, NULL);
        if (rc == 0) {
            DosWrite(hf, "TestData12345", 13, &written);
            DosClose(hf);
        }
    }

    print("=== FILEFINDBUF Layout Test ===\r\n\r\n");

    /* ── Test 1: FILEFINDBUF3 (level 1) ── */
    print("Test 1: DosFindFirst with FIL_STANDARD (level 1)\r\n");
    hdir = HDIR_CREATE;
    count = 1;
    memset(&fb3, 0xCC, sizeof(fb3));  /* Fill with sentinel */
    rc = DosFindFirst("C:\\FINDTEST.DAT", &hdir, FILE_NORMAL, &fb3, sizeof(fb3), &count, FIL_STANDARD);
    print("  rc="); print_num(rc); print(" count="); print_num(count); print("\r\n");
    check("DosFindFirst level 1 returns 0", rc == 0, &passed, &failed);

    if (rc == 0) {
        print("  sizeof(FILEFINDBUF3)="); print_num(sizeof(FILEFINDBUF3)); print("\r\n");

        /* Dump raw bytes (first 48 bytes) */
        print("  Raw bytes:\r\n    ");
        dump_bytes((unsigned char *)&fb3, 48, 16);

        /* Check individual fields */
        print("  oNextEntryOffset="); print_num(fb3.oNextEntryOffset); print("\r\n");
        print("  fdateCreation="); print_num(*(USHORT*)&fb3.fdateCreation); print("\r\n");
        print("  ftimeCreation="); print_num(*(USHORT*)&fb3.ftimeCreation); print("\r\n");
        print("  fdateLastWrite="); print_num(*(USHORT*)&fb3.fdateLastWrite); print("\r\n");
        print("  ftimeLastWrite="); print_num(*(USHORT*)&fb3.ftimeLastWrite); print("\r\n");
        print("  cbFile="); print_num(fb3.cbFile); print("\r\n");
        print("  cbFileAlloc="); print_num(fb3.cbFileAlloc); print("\r\n");
        print("  attrFile="); print_num(fb3.attrFile); print("\r\n");
        print("  cchName="); print_num(fb3.cchName); print("\r\n");
        print("  achName='"); print(fb3.achName); print("'\r\n");

        check("cbFile == 13", fb3.cbFile == 13, &passed, &failed);
        check("cchName > 0", fb3.cchName > 0, &passed, &failed);
        check("achName is FINDTEST.DAT", strcmp(fb3.achName, "FINDTEST.DAT") == 0, &passed, &failed);
        check("attrFile == FILE_NORMAL (0x20)", fb3.attrFile == 0x20, &passed, &failed);

        /* Verify field offsets by checking raw bytes */
        raw = (unsigned char *)&fb3;
        {
            ULONG off_cchName = (unsigned char *)&fb3.cchName - raw;
            ULONG off_achName = (unsigned char *)fb3.achName - raw;
            print("  Offset of cchName: "); print_num(off_cchName); print("\r\n");
            print("  Offset of achName: "); print_num(off_achName); print("\r\n");
            check("cchName at offset 28", off_cchName == 28, &passed, &failed);
            check("achName at offset 29", off_achName == 29, &passed, &failed);
        }

        DosFindClose(hdir);
    }

    /* ── Test 2: FILEFINDBUF4 (level 2 — with EA size) ── */
    print("\r\nTest 2: DosFindFirst with FIL_QUERYEASIZE (level 2)\r\n");
    hdir = HDIR_CREATE;
    count = 1;
    memset(&fb4, 0xCC, sizeof(fb4));  /* Fill with sentinel */
    rc = DosFindFirst("C:\\FINDTEST.DAT", &hdir, FILE_NORMAL, &fb4, sizeof(fb4), &count, FIL_QUERYEASIZE);
    print("  rc="); print_num(rc); print(" count="); print_num(count); print("\r\n");
    check("DosFindFirst level 2 returns 0", rc == 0, &passed, &failed);

    if (rc == 0) {
        print("  sizeof(FILEFINDBUF4)="); print_num(sizeof(FILEFINDBUF4)); print("\r\n");

        /* Dump raw bytes (first 52 bytes) */
        print("  Raw bytes:\r\n    ");
        dump_bytes((unsigned char *)&fb4, 52, 16);

        /* Check individual fields */
        print("  oNextEntryOffset="); print_num(fb4.oNextEntryOffset); print("\r\n");
        print("  fdateLastWrite="); print_num(*(USHORT*)&fb4.fdateLastWrite); print("\r\n");
        print("  ftimeLastWrite="); print_num(*(USHORT*)&fb4.ftimeLastWrite); print("\r\n");
        print("  cbFile="); print_num(fb4.cbFile); print("\r\n");
        print("  cbFileAlloc="); print_num(fb4.cbFileAlloc); print("\r\n");
        print("  attrFile="); print_num(fb4.attrFile); print("\r\n");
        print("  cbList="); print_num(fb4.cbList); print("\r\n");
        print("  cchName="); print_num(fb4.cchName); print("\r\n");
        print("  achName='"); print(fb4.achName); print("'\r\n");

        check("cbFile == 13", fb4.cbFile == 13, &passed, &failed);
        check("cchName > 0", fb4.cchName > 0, &passed, &failed);
        check("achName is FINDTEST.DAT", strcmp(fb4.achName, "FINDTEST.DAT") == 0, &passed, &failed);
        check("cbList >= 4", fb4.cbList >= 4, &passed, &failed);

        /* Verify field offsets */
        raw = (unsigned char *)&fb4;
        {
            ULONG off_cbList = (unsigned char *)&fb4.cbList - raw;
            ULONG off_cchName = (unsigned char *)&fb4.cchName - raw;
            ULONG off_achName = (unsigned char *)fb4.achName - raw;
            print("  Offset of cbList: "); print_num(off_cbList); print("\r\n");
            print("  Offset of cchName: "); print_num(off_cchName); print("\r\n");
            print("  Offset of achName: "); print_num(off_achName); print("\r\n");
            check("cbList at offset 28", off_cbList == 28, &passed, &failed);
            check("cchName at offset 32", off_cchName == 32, &passed, &failed);
            check("achName at offset 33", off_achName == 33, &passed, &failed);
        }

        DosFindClose(hdir);
    }

    /* Cleanup */
    DosDelete("C:\\FINDTEST.DAT");

    /* ── Summary ── */
    print("\r\n=== Results ===\r\n");
    print("Passed: "); print_num(passed); print("\r\n");
    print("Failed: "); print_num(failed); print("\r\n");

    if (failed == 0) { print("\r\nAll tests PASSED!\r\n"); }
    else { print("\r\nSome tests FAILED!\r\n"); }

    DosExit(EXIT_PROCESS, failed > 0 ? 1 : 0);
    return 0;
}
