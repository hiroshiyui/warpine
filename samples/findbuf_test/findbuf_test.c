/*
 * findbuf_test.c — Verify FILEFINDBUF3/4/4L layout from DosFindFirst.
 *
 * Tests DosFindFirst with level 1 and level 2, dumps buffers,
 * and simulates 4OS2's xDosFindFirst FILEFINDBUF4→FILEFINDBUF4L conversion.
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

static void print_hex8(unsigned char val) {
    char buf[3];
    buf[0] = (val >> 4) < 10 ? '0' + (val >> 4) : 'A' + (val >> 4) - 10;
    buf[1] = (val & 0xF) < 10 ? '0' + (val & 0xF) : 'A' + (val & 0xF) - 10;
    buf[2] = 0;
    print(buf);
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
    FILEFINDBUF4 ffb4;
    FILEFINDBUF4L fb4l;
    HDIR hdir;
    ULONG count;
    APIRET rc;
    int passed = 0, failed = 0;

    /* Create test file */
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

    print("=== FILEFINDBUF4 -> FILEFINDBUF4L Conversion Test ===\r\n\r\n");

    /* ── Test: Simulate xDosFindFirst conversion ── */
    print("Test: DosFindFirst level 2 -> FILEFINDBUF4L conversion\r\n");

    hdir = HDIR_CREATE;
    count = 1;
    memset(&ffb4, 0, sizeof(ffb4));
    rc = DosFindFirst("C:\\FINDTEST.DAT", &hdir, FILE_NORMAL, &ffb4, sizeof(ffb4), &count, FIL_QUERYEASIZE);
    print("  DosFindFirst rc="); print_num(rc); print("\r\n");
    check("DosFindFirst returns 0", rc == 0, &passed, &failed);

    if (rc == 0) {
        print("  FILEFINDBUF4 fields:\r\n");
        print("    cbFile="); print_num(ffb4.cbFile); print("\r\n");
        print("    cbFileAlloc="); print_num(ffb4.cbFileAlloc); print("\r\n");
        print("    attrFile="); print_num(ffb4.attrFile); print("\r\n");
        print("    cbList="); print_num(ffb4.cbList); print("\r\n");
        print("    cchName="); print_num(ffb4.cchName); print("\r\n");
        print("    achName='"); print(ffb4.achName); print("'\r\n");

        /* Now simulate xDosFindFirst's conversion */
        print("\r\n  Simulating xDosFindFirst conversion:\r\n");
        memset(&fb4l, 0xCC, sizeof(fb4l));

        /* Step 1: struct copy (line 150 in wrappers.c) */
        *(PFILEFINDBUF4)&fb4l = ffb4;

        print("    After struct copy:\r\n");
        print("    Raw bytes 28-48:\r\n      ");
        dump_bytes(((unsigned char*)&fb4l) + 28, 20, 20);

        /* Step 2: fix up fields (lines 151-156 of wrappers.c) */
        /* LONGLONG is a struct in Watcom, so use memcpy for assignment */
        {
            ULONG tmp;
            tmp = ffb4.cbFile;
            memcpy(&fb4l.cbFile, &tmp, 4);
            memset(((char*)&fb4l.cbFile) + 4, 0, 4); /* zero high dword */
            tmp = ffb4.cbFileAlloc;
            memcpy(&fb4l.cbFileAlloc, &tmp, 4);
            memset(((char*)&fb4l.cbFileAlloc) + 4, 0, 4);
        }
        fb4l.attrFile = ffb4.attrFile;
        fb4l.cbList = ffb4.cbList;
        fb4l.cchName = ffb4.cchName;
        memcpy(fb4l.achName, ffb4.achName, ffb4.cchName + 1);

        print("    After fix-up:\r\n");
        {
            ULONG tmp_lo;
            memcpy(&tmp_lo, &fb4l.cbFile, 4);
            print("    fb4l.cbFile="); print_num(tmp_lo); print("\r\n");
            memcpy(&tmp_lo, &fb4l.cbFileAlloc, 4);
            print("    fb4l.cbFileAlloc="); print_num(tmp_lo); print("\r\n");
        }
        print("    fb4l.attrFile="); print_num(fb4l.attrFile); print("\r\n");
        print("    fb4l.cbList="); print_num(fb4l.cbList); print("\r\n");
        print("    fb4l.cchName="); print_num(fb4l.cchName); print("\r\n");
        print("    fb4l.achName='"); print(fb4l.achName); print("'\r\n");

        /* Verify struct offsets */
        {
            unsigned char *base = (unsigned char *)&fb4l;
            ULONG off_cbFile = (unsigned char *)&fb4l.cbFile - base;
            ULONG off_cbFileAlloc = (unsigned char *)&fb4l.cbFileAlloc - base;
            ULONG off_attrFile = (unsigned char *)&fb4l.attrFile - base;
            ULONG off_cbList = (unsigned char *)&fb4l.cbList - base;
            ULONG off_cchName = (unsigned char *)&fb4l.cchName - base;
            ULONG off_achName = (unsigned char *)fb4l.achName - base;
            print("\r\n  FILEFINDBUF4L offsets:\r\n");
            print("    cbFile: "); print_num(off_cbFile); print("\r\n");
            print("    cbFileAlloc: "); print_num(off_cbFileAlloc); print("\r\n");
            print("    attrFile: "); print_num(off_attrFile); print("\r\n");
            print("    cbList: "); print_num(off_cbList); print("\r\n");
            print("    cchName: "); print_num(off_cchName); print("\r\n");
            print("    achName: "); print_num(off_achName); print("\r\n");

            check("cbFile at 16", off_cbFile == 16, &passed, &failed);
            check("cbFileAlloc at 24", off_cbFileAlloc == 24, &passed, &failed);
            check("attrFile at 32", off_attrFile == 32, &passed, &failed);
            check("cbList at 36", off_cbList == 36, &passed, &failed);
            check("cchName at 40", off_cchName == 40, &passed, &failed);
            check("achName at 41", off_achName == 41, &passed, &failed);
        }

        {
            ULONG cbFile_lo;
            memcpy(&cbFile_lo, &fb4l.cbFile, 4);
            check("fb4l.cbFile == 13", cbFile_lo == 13, &passed, &failed);
        }
        check("fb4l.achName == FINDTEST.DAT", strcmp(fb4l.achName, "FINDTEST.DAT") == 0, &passed, &failed);

        DosFindClose(hdir);
    }

    DosDelete("C:\\FINDTEST.DAT");

    print("\r\n=== Results ===\r\n");
    print("Passed: "); print_num(passed); print("\r\n");
    print("Failed: "); print_num(failed); print("\r\n");

    if (failed == 0) { print("\r\nAll tests PASSED!\r\n"); }
    else { print("\r\nSome tests FAILED!\r\n"); }

    DosExit(EXIT_PROCESS, failed > 0 ? 1 : 0);
    return 0;
}
