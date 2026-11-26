/* SPDX-License-Identifier: GPL-3.0-or-later */
/* ea_test.c - extended attributes by handle and by path: DosSetFileInfo
   level 2 sets them on an open file, DosQueryFileInfo / DosQueryPathInfo
   level 2 report the size of the list holding them all and level 3 returns
   the names a GEA2LIST asks for (one the file lacks with no value), never
   past the FEA2LIST's cbList; DosEnumAttribute lists them by handle
   (ENUMEA_REFTYPE_FHANDLE) and by path (ENUMEA_REFTYPE_PATH) as DENA2
   entries; DosOpen's EAOP2 sets the EAs of the file it creates, and
   DosFindFirst's FIL_QUERYEASIZE reports a file's list size; a file moved
   while open keeps its EAs reachable by handle. Writes "ea_test: <name>: ok" or "... FAIL got <v> want <v>",
   then "ea_test: done <p>/<t>". */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <string.h>
#include <stddef.h>

#define FILE_NAME "EATEST.DAT"
#define NEW_NAME  "EATEST2.DAT"
#define MOVED     "EATEST3.DAT"
#define BUF_SIZE  512

static int passed = 0, total = 0;

static void check(const char *what, ULONG got, ULONG want)
{
    total++;
    if (got == want) { passed++; printf("ea_test: %s: ok\n", what); }
    else printf("ea_test: %s: FAIL got %lx want %lx\n", what, got, want);
}

/* Append one FEA2 (padded to 4 bytes) at `at`; its size. */
static ULONG put_fea(char *at, const char *name, const char *value)
{
    PFEA2 fea = (PFEA2)at;
    ULONG size = offsetof(FEA2, szName) + strlen(name) + 1 + strlen(value);
    fea->oNextEntryOffset = 0;
    fea->fEA = 0;
    fea->cbName = (BYTE)strlen(name);
    fea->cbValue = (USHORT)strlen(value);
    strcpy(fea->szName, name);
    memcpy(fea->szName + fea->cbName + 1, value, fea->cbValue);
    return (size + 3) & ~3UL;
}

/* Append one GEA2 (padded to 4 bytes) at `at`; its size. */
static ULONG put_gea(char *at, const char *name)
{
    PGEA2 gea = (PGEA2)at;
    ULONG size = offsetof(GEA2, szName) + strlen(name) + 1;
    gea->oNextEntryOffset = 0;
    gea->cbName = (BYTE)strlen(name);
    strcpy(gea->szName, name);
    return (size + 3) & ~3UL;
}

/* A FEA2LIST of up to two name/value pairs (an empty value deletes). */
static PFEA2LIST fea_list(char *buf, const char *n1, const char *v1, const char *n2, const char *v2)
{
    PFEA2LIST list = (PFEA2LIST)buf;
    ULONG first = put_fea((char *)list->list, n1, v1);
    ULONG second = 0;
    if (n2 != NULL) {
        ((PFEA2)list->list)->oNextEntryOffset = first;
        second = put_fea((char *)list->list + first, n2, v2);
    }
    list->cbList = sizeof(ULONG) + first + second;
    return list;
}

/* A GEA2LIST of two names. */
static PGEA2LIST gea_list(char *buf, const char *n1, const char *n2)
{
    PGEA2LIST list = (PGEA2LIST)buf;
    ULONG first = put_gea((char *)list->list, n1);
    ((PGEA2)list->list)->oNextEntryOffset = first;
    list->cbList = sizeof(ULONG) + first + put_gea((char *)list->list + first, n2);
    return list;
}

int main(void)
{
    static char fbuf[BUF_SIZE], gbuf[BUF_SIZE], out[BUF_SIZE];
    HFILE       hf;
    ULONG       action, count, room;
    EAOP2       eaop;
    FILESTATUS4 fs4;
    PFEA2       fea;
    HFILE       hf2;
    HDIR        hdir = HDIR_CREATE;
    FILEFINDBUF4 find;

    DosDelete((PSZ)FILE_NAME);
    check("DosOpen", DosOpen((PSZ)FILE_NAME, &hf, &action, 0, FILE_NORMAL,
          OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_OPEN_IF_EXISTS,
          OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL), NO_ERROR);

    /* Set by handle. */
    eaop.fpGEA2List = NULL;
    eaop.fpFEA2List = fea_list(fbuf, "WARPINE.ONE", "first", "WARPINE.TWO", "2");
    eaop.oError = 0xFFFF;
    check("DosSetFileInfo(level 2)", DosSetFileInfo(hf, FIL_QUERYEASIZE, &eaop, sizeof eaop), NO_ERROR);
    check("-> oError cleared", eaop.oError, 0);

    /* Level 2: the size of the list holding both. */
    check("DosQueryFileInfo(level 2)", DosQueryFileInfo(hf, FIL_QUERYEASIZE, &fs4, sizeof fs4), NO_ERROR);
    check("-> cbList holds both", fs4.cbList, eaop.fpFEA2List->cbList);
    check("DosQueryPathInfo(level 2)", DosQueryPathInfo((PSZ)FILE_NAME, FIL_QUERYEASIZE, &fs4, sizeof fs4), NO_ERROR);
    check("-> the same by path", fs4.cbList, eaop.fpFEA2List->cbList);

    /* Level 3 by handle: one present (asked in another case), one not. */
    eaop.fpGEA2List = gea_list(gbuf, "warpine.two", "WARPINE.NONE");
    eaop.fpFEA2List = (PFEA2LIST)out;
    memset(out, 0xAA, sizeof out);
    eaop.fpFEA2List->cbList = sizeof out;
    check("DosQueryFileInfo(level 3)", DosQueryFileInfo(hf, FIL_QUERYEASFROMLIST, &eaop, sizeof eaop), NO_ERROR);
    fea = eaop.fpFEA2List->list;
    check("-> the value asked for", fea->cbValue == 1 && fea->szName[fea->cbName + 1] == '2', 1);
    check("-> the flag kept", fea->fEA, 0);
    fea = (PFEA2)((char *)fea + fea->oNextEntryOffset);
    check("-> one the file lacks: no value", fea->cbValue, 0);
    check("-> its name", strcmp(fea->szName, "WARPINE.NONE"), 0);
    check("-> the last entry", fea->oNextEntryOffset, 0);

    /* Level 3 by path, into a buffer one byte too small. */
    room = eaop.fpFEA2List->cbList;
    memset(out, 0xAA, sizeof out);
    eaop.fpFEA2List->cbList = room - 1;
    check("DosQueryPathInfo(level 3, too small)",
          DosQueryPathInfo((PSZ)FILE_NAME, FIL_QUERYEASFROMLIST, &eaop, sizeof eaop), ERROR_BUFFER_OVERFLOW);
    check("-> nothing written past cbList", (UCHAR)out[sizeof(ULONG)], 0xAA);
    eaop.fpFEA2List->cbList = room;
    check("DosQueryPathInfo(level 3)",
          DosQueryPathInfo((PSZ)FILE_NAME, FIL_QUERYEASFROMLIST, &eaop, sizeof eaop), NO_ERROR);
    check("-> the same list", eaop.fpFEA2List->cbList, room);
    ((PGEA2)eaop.fpGEA2List->list)->cbName = 0;
    check("DosQueryPathInfo(an empty name)",
          DosQueryPathInfo((PSZ)FILE_NAME, FIL_QUERYEASFROMLIST, &eaop, sizeof eaop), ERROR_EA_LIST_INCONSISTENT);
    check("-> oError names the entry", eaop.oError, sizeof(ULONG));

    /* DosEnumAttribute by handle and by path. */
    count = (ULONG)-1;
    check("DosEnumAttribute(by handle)",
          DosEnumAttribute(ENUMEA_REFTYPE_FHANDLE, &hf, 1, out, sizeof out, &count, ENUMEA_LEVEL_NO_VALUE), NO_ERROR);
    check("-> both", count, 2);
    fea = (PFEA2)out;
    check("-> a DENA2 names one", strncmp(fea->szName, "WARPINE.", 8), 0);
    count = (ULONG)-1;
    check("DosEnumAttribute(by path)",
          DosEnumAttribute(ENUMEA_REFTYPE_PATH, (PSZ)FILE_NAME, 1, out, sizeof out, &count, ENUMEA_LEVEL_NO_VALUE), NO_ERROR);
    check("-> both", count, 2);
    count = (ULONG)-1;
    check("DosEnumAttribute(from the second)",
          DosEnumAttribute(ENUMEA_REFTYPE_PATH, (PSZ)FILE_NAME, 2, out, sizeof out, &count, ENUMEA_LEVEL_NO_VALUE), NO_ERROR);
    check("-> one", count, 1);

    /* An empty value deletes, by handle. */
    eaop.fpFEA2List = fea_list(fbuf, "WARPINE.ONE", "", NULL, NULL);
    check("DosSetFileInfo(delete one)", DosSetFileInfo(hf, FIL_QUERYEASIZE, &eaop, sizeof eaop), NO_ERROR);
    count = (ULONG)-1;
    DosEnumAttribute(ENUMEA_REFTYPE_FHANDLE, &hf, 1, out, sizeof out, &count, ENUMEA_LEVEL_NO_VALUE);
    check("-> one left", count, 1);

    check("DosClose", DosClose(hf), NO_ERROR);
    check("DosDelete", DosDelete((PSZ)FILE_NAME), NO_ERROR);

    /* DosOpen's EAOP2 on a new file; DosFindFirst's cbList. */
    DosDelete((PSZ)NEW_NAME);
    eaop.fpFEA2List = fea_list(fbuf, "WARPINE.MADE", "at open", NULL, NULL);
    eaop.oError = 0xFFFF;
    check("DosOpen(with an EAOP2)", DosOpen((PSZ)NEW_NAME, &hf2, &action, 0, FILE_NORMAL,
          OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_FAIL_IF_EXISTS,
          OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, &eaop), NO_ERROR);
    check("-> oError cleared", eaop.oError, 0);
    count = (ULONG)-1;
    DosEnumAttribute(ENUMEA_REFTYPE_FHANDLE, &hf2, 1, out, sizeof out, &count, ENUMEA_LEVEL_NO_VALUE);
    check("-> the new file has its EA", count, 1);
    /* Moved while open: its EAs are still reached by handle. */
    DosDelete((PSZ)MOVED);
    check("DosMove(while open)", DosMove((PSZ)NEW_NAME, (PSZ)MOVED), NO_ERROR);
    eaop.fpFEA2List = fea_list(fbuf, "WARPINE.AFTER", "moved", NULL, NULL);
    check("DosSetFileInfo(after the move)", DosSetFileInfo(hf2, FIL_QUERYEASIZE, &eaop, sizeof eaop), NO_ERROR);
    count = (ULONG)-1;
    DosEnumAttribute(ENUMEA_REFTYPE_PATH, (PSZ)MOVED, 1, out, sizeof out, &count, ENUMEA_LEVEL_NO_VALUE);
    check("-> both EAs on the moved file", count, 2);
    count = (ULONG)-1;
    DosEnumAttribute(ENUMEA_REFTYPE_FHANDLE, &hf2, 1, out, sizeof out, &count, ENUMEA_LEVEL_NO_VALUE);
    check("-> and by its handle", count, 2);
    DosClose(hf2);
    count = 1;
    check("DosFindFirst(FIL_QUERYEASIZE)", DosFindFirst((PSZ)MOVED, &hdir, FILE_NORMAL, &find, sizeof find,
          &count, FIL_QUERYEASIZE), NO_ERROR);
    check("-> cbList is its list's size", find.cbList > 4, 1);
    DosFindClose(hdir);
    check("DosDelete(the new file)", DosDelete((PSZ)MOVED), NO_ERROR);
    printf("ea_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}
