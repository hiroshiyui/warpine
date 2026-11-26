/* SPDX-License-Identifier: GPL-3.0-or-later */
/* largefile_test.c - the large-file calls: a sparse file grown past 4 GB
   with DosSetFileSizeL, data written and read back at an offset past 4 GB
   reached with DosSetFilePtrL, 64-bit sizes from DosQueryFileInfo /
   DosQueryPathInfo (FIL_STANDARDL, FIL_QUERYEASIZEL) and DosFindFirst
   (FILEFINDBUF3L), FILELOCKL ranges past 4 GB, and the DosProtect*L forms.
   Writes "largefile_test: <name>: ok" or "... FAIL got <v> want <v>", then
   "largefile_test: done <p>/<t>". */
#define INCL_DOS
#define INCL_DOSERRORS
#define INCL_LONGLONG
#include <os2.h>
#include <stdio.h>
#include <string.h>

#define FILE_NAME "LARGE.DAT"
#define GB        (1024LL * 1024 * 1024)
#define BIG       (5 * GB)
#define FAR_OFF   (4 * GB + 10)

static int passed = 0, total = 0;

static void check(const char *what, ULONG got, ULONG want)
{
    total++;
    if (got == want) { passed++; printf("largefile_test: %s: ok\n", what); }
    else printf("largefile_test: %s: FAIL got %lx want %lx\n", what, got, want);
}

int main(void)
{
    HFILE        hf, hf2;
    ULONG        action, n, count;
    LONGLONG     pos;
    FILESTATUS3L fs3l;
    FILESTATUS4L fs4l;
    FILESTATUS3  fs3;
    FILELOCKL    lock;
    FILEFINDBUF3L find;
    HDIR         hdir = HDIR_CREATE;
    FHLOCK       fhl;
    char         text[8];

    DosDelete((PSZ)FILE_NAME);
    check("DosOpenL", DosOpenL((PSZ)FILE_NAME, &hf, &action, 0, FILE_NORMAL,
          OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_OPEN_IF_EXISTS,
          OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL), NO_ERROR);
    check("-> created", action, FILE_CREATED);
    check("DosSetFileSizeL(5 GB)", DosSetFileSizeL(hf, BIG), NO_ERROR);
    check("DosSetFileSizeL(negative)", DosSetFileSizeL(hf, -1), ERROR_INVALID_PARAMETER);

    /* Data past 4 GB. */
    check("DosSetFilePtrL(past 4 GB)", DosSetFilePtrL(hf, FAR_OFF, FILE_BEGIN, &pos), NO_ERROR);
    check("-> the new position", pos == FAR_OFF, 1);
    check("DosWrite", DosWrite(hf, "tail", 4, &n), NO_ERROR);
    check("DosSetFilePtrL(back 4)", DosSetFilePtrL(hf, -4, FILE_CURRENT, &pos), NO_ERROR);
    check("-> back at the data", pos == FAR_OFF, 1);
    memset(text, 0, sizeof text);
    check("DosRead", DosRead(hf, text, 4, &n), NO_ERROR);
    check("-> the data", n == 4 && memcmp(text, "tail", 4) == 0, 1);
    check("DosSetFilePtrL(from the end)", DosSetFilePtrL(hf, 0, FILE_END, &pos), NO_ERROR);
    check("-> the size", pos == BIG, 1);

    /* 64-bit sizes; the 32-bit level cannot hold them. */
    check("DosQueryFileInfo(FIL_STANDARDL)", DosQueryFileInfo(hf, FIL_STANDARDL, &fs3l, sizeof fs3l), NO_ERROR);
    check("-> cbFile", fs3l.cbFile == BIG, 1);
    check("-> cbFileAlloc covers it", fs3l.cbFileAlloc >= BIG, 1);
    check("DosQueryPathInfo(FIL_QUERYEASIZEL)",
          DosQueryPathInfo((PSZ)FILE_NAME, FIL_QUERYEASIZEL, &fs4l, sizeof fs4l), NO_ERROR);
    check("-> cbFile", fs4l.cbFile == BIG, 1);
    check("-> cbList of no EAs", fs4l.cbList, 4);
    check("DosQueryFileInfo(FIL_STANDARDL, short)",
          DosQueryFileInfo(hf, FIL_STANDARDL, &fs3l, sizeof fs3l - 1), ERROR_BUFFER_OVERFLOW);
    check("DosQueryFileInfo(FIL_STANDARD)", DosQueryFileInfo(hf, FIL_STANDARD, &fs3, sizeof fs3), NO_ERROR);
    check("-> saturated", fs3.cbFile, 0xFFFFFFFFUL);

    /* DosFindFirst at FIL_STANDARDL. */
    count = 1;
    check("DosFindFirst(FIL_STANDARDL)", DosFindFirst((PSZ)FILE_NAME, &hdir, FILE_NORMAL, &find, sizeof find,
          &count, FIL_STANDARDL), NO_ERROR);
    check("-> cbFile", find.cbFile == BIG, 1);
    check("-> the name", strcmp(find.achName, FILE_NAME), 0);
    DosFindClose(hdir);

    /* Lock ranges past 4 GB. */
    lock.lOffset = FAR_OFF;
    lock.lRange = 4;
    check("DosSetFileLocksL(lock)", DosSetFileLocksL(hf, NULL, &lock, 0, 0), NO_ERROR);
    check("DosSetFileLocksL(unlock)", DosSetFileLocksL(hf, &lock, NULL, 0, 0), NO_ERROR);
    lock.lOffset = -1;
    check("DosSetFileLocksL(negative)", DosSetFileLocksL(hf, NULL, &lock, 0, 0), ERROR_INVALID_PARAMETER);
    check("DosCancelLockRequestL", DosCancelLockRequestL(hf, &lock), NO_ERROR);
    check("DosClose", DosClose(hf), NO_ERROR);

    /* The protected forms. */
    check("DosProtectOpenL", DosProtectOpenL((PSZ)FILE_NAME, &hf2, &action, 0, FILE_NORMAL,
          OPEN_ACTION_OPEN_IF_EXISTS, OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL, &fhl), NO_ERROR);
    check("DosProtectSetFilePtrL", DosProtectSetFilePtrL(hf2, FAR_OFF, FILE_BEGIN, &pos, fhl), NO_ERROR);
    check("-> the new position", pos == FAR_OFF, 1);
    check("DosProtectSetFilePtrL(wrong lock)",
          DosProtectSetFilePtrL(hf2, 0, FILE_BEGIN, &pos, fhl + 1), ERROR_INVALID_HANDLE);
    check("DosProtectSetFileSizeL(back to 1 KB)", DosProtectSetFileSizeL(hf2, 1024, fhl), NO_ERROR);
    lock.lOffset = 0;
    lock.lRange = 10;
    check("DosProtectSetFileLocksL", DosProtectSetFileLocksL(hf2, NULL, &lock, 0, 0, fhl), NO_ERROR);
    check("DosProtectClose", DosProtectClose(hf2, fhl), NO_ERROR);
    check("DosQueryPathInfo(FIL_STANDARD)", DosQueryPathInfo((PSZ)FILE_NAME, FIL_STANDARD, &fs3, sizeof fs3), NO_ERROR);
    check("-> 1 KB now", fs3.cbFile, 1024);
    check("DosDelete", DosDelete((PSZ)FILE_NAME), NO_ERROR);

    printf("largefile_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}
