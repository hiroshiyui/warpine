/*
 * dir_test.c — 32-bit OS/2 directory listing test.
 *
 * Lists files in the current directory (C:\) using DosFindFirst/DosFindNext,
 * bypassing 16-bit thunks entirely. This verifies that the VFS directory
 * enumeration works correctly even when 4OS2's `dir` command is blocked
 * by thunk issues.
 *
 * Exercises:
 * - DosFindFirst with *.* wildcard
 * - DosFindNext iteration
 * - DosFindClose cleanup
 * - DosQueryCurrentDisk / DosQueryCurrentDir for header
 * - FILEFINDBUF3 field parsing (name, size, attributes, date/time)
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

static void print_size_padded(ULONG size) {
    /* Right-align size in 10-char field */
    char buf[11];
    int i;
    for (i = 0; i < 10; i++) buf[i] = ' ';
    buf[10] = 0;
    i = 9;
    if (size == 0) { buf[i--] = '0'; }
    else { while (size > 0 && i >= 0) { buf[i--] = '0' + (size % 10); size /= 10; } }
    print(buf);
}

static void print_date(FDATE fd) {
    /* FDATE: day(5), month(4), year-1980(7) */
    USHORT date = *(USHORT *)&fd;
    USHORT year = (date >> 9) + 1980;
    USHORT month = (date >> 5) & 0x0F;
    USHORT day = date & 0x1F;
    if (month < 10) print("0");
    print_num(month);
    print("-");
    if (day < 10) print("0");
    print_num(day);
    print("-");
    print_num(year);
}

static void print_time(FTIME ft) {
    /* FTIME: twosecs(5), minutes(6), hours(5) */
    USHORT time = *(USHORT *)&ft;
    USHORT hour = time >> 11;
    USHORT minute = (time >> 5) & 0x3F;
    if (hour < 10) print(" ");
    print_num(hour);
    print(":");
    if (minute < 10) print("0");
    print_num(minute);
}

int main(void) {
    HDIR hdir = HDIR_CREATE;
    FILEFINDBUF3 fb;
    ULONG count;
    ULONG disk, logical;
    ULONG total_files = 0;
    ULONG total_size = 0;
    APIRET rc;
    char curdir[256];
    ULONG cbBuf;

    /* Print directory header */
    DosQueryCurrentDisk(&disk, &logical);
    print("\r\n Directory of ");
    {
        char dl = (char)('A' + disk - 1);
        DosWrite(1, &dl, 1, &dummy);
    }
    print(":\\");
    cbBuf = sizeof(curdir);
    rc = DosQueryCurrentDir(0, curdir, &cbBuf);
    if (rc == 0 && curdir[0] != 0) {
        print(curdir);
    }
    print("\r\n\r\n");

    /* Find all files */
    count = 1;
    rc = DosFindFirst("*.*", &hdir, FILE_NORMAL | FILE_DIRECTORY | FILE_HIDDEN | FILE_SYSTEM | FILE_READONLY,
                      &fb, sizeof(fb), &count, FIL_STANDARD);

    if (rc != 0) {
        print("  No files found (rc=");
        print_num(rc);
        print(")\r\n");
        DosExit(EXIT_PROCESS, rc);
    }

    while (rc == 0 && count > 0) {
        /* Date */
        print_date(fb.fdateLastWrite);
        print("  ");

        /* Time */
        print_time(fb.ftimeLastWrite);
        print("  ");

        /* Size or <DIR> */
        if (fb.attrFile & FILE_DIRECTORY) {
            print("     <DIR>");
        } else {
            print_size_padded(fb.cbFile);
            total_size += fb.cbFile;
        }
        print("  ");

        /* Name */
        print(fb.achName);
        print("\r\n");

        total_files++;

        /* Next */
        count = 1;
        rc = DosFindNext(hdir, &fb, sizeof(fb), &count);
    }

    DosFindClose(hdir);

    /* Summary */
    print("        ");
    print_num(total_files);
    print(" file(s)    ");
    print_num(total_size);
    print(" bytes\r\n");

    DosExit(EXIT_PROCESS, 0);
    return 0;
}
