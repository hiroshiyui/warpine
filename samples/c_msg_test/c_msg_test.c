/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * c_msg_test.c - DosGetMessage with Warpine's message files (ADR-0105)
 *
 * APP.MSG (from app.txt by os2msg) is bound into the program by lx-link;
 * DSK.MSG is read from C:\MSGS, which CONFIG.SYS puts on DPATH (the
 * integration test sets that up). Checks:
 *   - an error message gets its id, and %1 / %2 their strings;
 *   - an information message has none, and keeps its two lines;
 *   - a prompt ends without a line end;
 *   - a number with no message: ERROR_MR_MID_NOT_FOUND, with a text;
 *   - a file that does not exist: ERROR_MR_UN_ACC_MSGF, with a text;
 *   - a file not in Warpine's format: ERROR_MR_INV_MSGF_FORMAT;
 *   - a buffer too small: ERROR_MR_MSG_TOO_LONG, the message cut;
 *   - more than nine strings: ERROR_MR_INV_IVCOUNT;
 *   - a plain name found along DPATH, and a full path.
 *
 * Each check is written as "c_msg_test: <name>: ok" or "... FAIL", then
 * "c_msg_test: done <passed>/<total>".
 */
#include <os2.h>
#include <stdio.h>
#include <string.h>

static int passed, total;

static void check(const char *name, int ok)
{
    total++;
    if (ok) passed++;
    printf("c_msg_test: %s: %s\n", name, ok ? "ok" : "FAIL");
}

static char buf[256];
static ULONG cb;

/* DosGetMessage into buf, NUL-ended; its return code. */
static APIRET get(PCHAR *table, ULONG count, ULONG size, ULONG number, PCSZ file)
{
    APIRET rc;
    memset(buf, 0, sizeof buf);
    cb = 0;
    rc = DosGetMessage(table, count, buf, size, number, (PSZ)file, &cb);
    if (cb < sizeof buf) buf[cb] = 0;
    return rc;
}

int main(void)
{
    PCHAR strings[10] = { "x.txt", "C:\\DATA", "3", "4", "5", "6", "7", "8", "9", "10" };
    HFILE hf;
    ULONG action, written;
    APIRET rc;

    rc = get(strings, 2, sizeof buf, 1, "APP.MSG");
    check("bound error message", rc == NO_ERROR && strcmp(buf, "APP0001: The file x.txt was not found in C:\\DATA.\r\n") == 0);
    check("-> its length", cb == strlen(buf));

    rc = get(NULL, 0, sizeof buf, 2, "app.msg");
    check("information, two lines, any case", rc == NO_ERROR && strcmp(buf, "Two lines\r\nof information.\r\n") == 0);

    rc = get(NULL, 0, sizeof buf, 4, "APP.MSG");
    check("a prompt has no line end", rc == NO_ERROR && strcmp(buf, "Continue (Y/N)?") == 0);

    rc = get(strings + 2, 2, sizeof buf, 5, "APP.MSG");
    check("a warning gets its id", rc == NO_ERROR && strcmp(buf, "APP0005: Only 3 of 4 bytes were written.\r\n") == 0);

    rc = get(NULL, 0, sizeof buf, 3, "APP.MSG");
    check("no message 3", rc == ERROR_MR_MID_NOT_FOUND && cb > 0 && strstr(buf, "0003") != NULL);

    rc = get(NULL, 0, sizeof buf, 1, "NOSUCH.MSG");
    check("no such file", rc == ERROR_MR_UN_ACC_MSGF && strstr(buf, "NOSUCH.MSG") != NULL);

    rc = get(strings, 2, 10, 1, "APP.MSG");
    check("a buffer too small", rc == ERROR_MR_MSG_TOO_LONG && cb == 10 && memcmp(buf, "APP0001: T", 10) == 0);

    rc = get(strings, 10, sizeof buf, 1, "APP.MSG");
    check("ten strings", rc == ERROR_MR_INV_IVCOUNT);

    rc = get(NULL, 0, sizeof buf, 10, "DSK.MSG");
    check("a file along DPATH", rc == NO_ERROR && strcmp(buf, "Read from the drive.\r\n") == 0);

    rc = get(NULL, 0, sizeof buf, 10, "C:\\MSGS\\DSK.MSG");
    check("a file by its path", rc == NO_ERROR && strcmp(buf, "Read from the drive.\r\n") == 0);

    if (DosOpen("C:\\BAD.MSG", &hf, &action, 0, FILE_NORMAL,
                OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL) == NO_ERROR) {
        DosWrite(hf, "not a message file", 18, &written);
        DosClose(hf);
    }
    rc = get(NULL, 0, sizeof buf, 1, "C:\\BAD.MSG");
    check("a file in another format", rc == ERROR_MR_INV_MSGF_FORMAT && cb > 0);

    printf("c_msg_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}
