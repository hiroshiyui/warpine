/* SPDX-License-Identifier: GPL-3.0-or-later */
/* attr_test.c - file attributes (ADR-0091): hidden, system and archive
   are kept and reported, DosFindFirst's filter sees them, writing archives
   a file again, DosCopy and DosMove carry them. Writes "attr_test: <name>:
   ok" or "... FAIL got <v> want <v>", then "attr_test: done <p>/<t>". */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <string.h>

static int passed = 0, total = 0;

static void check(const char *what, ULONG got, ULONG want)
{
    total++;
    if (got == want) { passed++; printf("attr_test: %s: ok\n", what); }
    else printf("attr_test: %s: FAIL got %lx want %lx\n", what, got, want);
}

static ULONG attrs_of(const char *name)
{
    FILESTATUS3 fs;
    if (DosQueryPathInfo((PSZ)name, FIL_STANDARD, &fs, sizeof fs) != NO_ERROR) return 0xFFFF;
    return fs.attrFile;
}

static ULONG find(const char *pattern, ULONG filter)
{
    HDIR h = HDIR_CREATE;
    FILEFINDBUF3 buf;
    ULONG n = 1, rc = DosFindFirst((PSZ)pattern, &h, filter, &buf, sizeof buf, &n, FIL_STANDARD);
    if (rc != NO_ERROR) return 0xF000 | rc;
    DosFindClose(h);
    return buf.attrFile;
}

static void write_file(const char *name, const char *text)
{
    HFILE hf;
    ULONG action, n;
    DosOpen((PSZ)name, &hf, &action, 0, FILE_NORMAL,
            OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_OPEN_IF_EXISTS,
            OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL);
    DosWrite(hf, (PVOID)text, strlen(text), &n);
    DosClose(hf);
}

int main(void)
{
    FILESTATUS3 fs;

    DosForceDelete((PSZ)"ATTR1.TMP");
    DosForceDelete((PSZ)"ATTR2.TMP");
    DosForceDelete((PSZ)"ATTR3.TMP");
    write_file("ATTR1.TMP", "one");
    check("a new file is archived", attrs_of("ATTR1.TMP"), FILE_ARCHIVED);

    DosQueryPathInfo((PSZ)"ATTR1.TMP", FIL_STANDARD, &fs, sizeof fs);
    fs.attrFile = FILE_HIDDEN | FILE_SYSTEM;
    check("DosSetPathInfo(hidden | system)", DosSetPathInfo((PSZ)"ATTR1.TMP", FIL_STANDARD, &fs, sizeof fs, 0), NO_ERROR);
    check("-> kept", attrs_of("ATTR1.TMP"), FILE_HIDDEN | FILE_SYSTEM);
    check("DosFindFirst(FILE_NORMAL) skips it", find("ATTR1.TMP", FILE_NORMAL), 0xF000 | ERROR_NO_MORE_FILES);
    check("DosFindFirst(hidden | system) finds it", find("ATTR1.TMP", FILE_HIDDEN | FILE_SYSTEM), FILE_HIDDEN | FILE_SYSTEM);

    write_file("ATTR1.TMP", "two");
    check("writing archives it again", attrs_of("ATTR1.TMP"), FILE_HIDDEN | FILE_SYSTEM | FILE_ARCHIVED);

    check("DosQueryPathInfo", DosQueryPathInfo((PSZ)"ATTR1.TMP", FIL_STANDARD, &fs, sizeof fs), NO_ERROR);
    fs.attrFile = FILE_READONLY | FILE_HIDDEN;
    check("DosSetPathInfo(read-only | hidden)", DosSetPathInfo((PSZ)"ATTR1.TMP", FIL_STANDARD, &fs, sizeof fs, 0), NO_ERROR);
    check("-> both", attrs_of("ATTR1.TMP"), FILE_READONLY | FILE_HIDDEN);

    check("DosCopy", DosCopy((PSZ)"ATTR1.TMP", (PSZ)"ATTR2.TMP", 0), NO_ERROR);
    check("-> the copy has the attributes", attrs_of("ATTR2.TMP"), FILE_READONLY | FILE_HIDDEN);

    fs.attrFile = FILE_SYSTEM;
    DosSetPathInfo((PSZ)"ATTR2.TMP", FIL_STANDARD, &fs, sizeof fs, 0);
    check("DosMove", DosMove((PSZ)"ATTR2.TMP", (PSZ)"ATTR3.TMP"), NO_ERROR);
    check("-> the moved file keeps them", attrs_of("ATTR3.TMP"), FILE_SYSTEM);

    fs.attrFile = FILE_NORMAL;
    DosSetPathInfo((PSZ)"ATTR1.TMP", FIL_STANDARD, &fs, sizeof fs, 0);
    check("all cleared", attrs_of("ATTR1.TMP"), FILE_NORMAL);
    DosDelete((PSZ)"ATTR1.TMP");
    DosDelete((PSZ)"ATTR3.TMP");
    check("deleted", attrs_of("ATTR3.TMP"), 0xFFFF);

    printf("attr_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}
