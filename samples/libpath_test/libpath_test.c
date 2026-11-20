/* SPDX-License-Identifier: GPL-3.0-or-later */
/* libpath_test.c - CONFIG.SYS's LIBPATH and SET (ADR-0092). Run with a
   drive C whose CONFIG.SYS has
       LIBPATH=C:\LIBA;C:\LIBB;.
       SET K3VAR=from config.sys
       SET PATH=C:\BIN;C:\
       SET TMP=
   and copies of noinit.dll as C:\LIBB\K3LIB.DLL, C:\K3DOT.DLL,
   C:\END\K3END.DLL, C:\BEGIN\K3BOTH.DLL and C:\LIBB\K3BOTH.DLL
   (tests/integration.rs sets it up). Writes "libpath_test: <name>: ok" or
   "... FAIL got <v> want <v>", then "libpath_test: done <p>/<t>". */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <string.h>

static int passed = 0, total = 0;

static void check(const char *what, ULONG got, ULONG want)
{
    total++;
    if (got == want) { passed++; printf("libpath_test: %s: ok\n", what); }
    else printf("libpath_test: %s: FAIL got %lu want %lu\n", what, got, want);
}

static void check_str(const char *what, const char *got, const char *want)
{
    total++;
    if (got != NULL && strcmp(got, want) == 0) { passed++; printf("libpath_test: %s: ok\n", what); }
    else printf("libpath_test: %s: FAIL got '%s' want '%s'\n", what, got ? got : "(none)", want);
}

/* The value of environment variable `name`, or NULL. */
static const char *env(const char *name)
{
    PSZ value;
    return DosScanEnv((PCSZ)name, &value) == NO_ERROR ? (const char *)value : NULL;
}

/* Load module `name`; its full name in `path`, or the error as text. */
static void load(const char *name, char *path, ULONG cb)
{
    HMODULE hmod = 0;
    char failed[32];
    APIRET rc = DosLoadModule((PSZ)failed, sizeof failed, (PSZ)name, &hmod);
    if (rc != NO_ERROR) { sprintf(path, "error %lu", rc); return; }
    if (DosQueryModuleName(hmod, cb, (PCHAR)path) != NO_ERROR) strcpy(path, "no name");
}

int main(void)
{
    char path[CCHMAXPATH];
    char ext[1024];
    HMODULE hmod;
    PFN answer;
    char failed[32];

    /* SET statements: the environment a program starts with. */
    check_str("SET adds a variable", env("K3VAR"), "from config.sys");
    check_str("SET replaces PATH", env("PATH"), "C:\\BIN;C:\\");
    check_str("SET TMP= removes TMP", env("TMP") ? env("TMP") : "(removed)", "(removed)");
    check_str("COMSPEC is kept", env("COMSPEC"), "C:\\CMD.EXE");

    /* LIBPATH: C:\LIBA has none, so C:\LIBB. */
    load("K3LIB", path, sizeof path);
    check_str("LIBPATH finds K3LIB", path, "C:\\LIBB\\K3LIB.DLL");
    hmod = 0;
    DosLoadModule((PSZ)failed, sizeof failed, (PSZ)"K3LIB", &hmod);
    answer = NULL;
    DosQueryProcAddr(hmod, 0, (PSZ)"NoInitAnswer", &answer);
    check("the DLL found runs", answer ? (ULONG)((int (APIENTRY *)(void))answer)() : 0, 42);
    load("K3DOT", path, sizeof path);
    check_str("'.' is the current directory", path, "C:\\K3DOT.DLL");

    /* ENDLIBPATH, searched after LIBPATH. */
    load("K3END", path, sizeof path);
    check_str("K3END not on LIBPATH", path, "error 126");
    check("DosSetExtLIBPATH END", DosSetExtLIBPATH((PCSZ)"C:\\END", END_LIBPATH), NO_ERROR);
    load("K3END", path, sizeof path);
    check_str("ENDLIBPATH finds K3END", path, "C:\\END\\K3END.DLL");

    /* BEGINLIBPATH, searched before LIBPATH. */
    check("DosSetExtLIBPATH BEGIN", DosSetExtLIBPATH((PCSZ)"C:\\BEGIN", BEGIN_LIBPATH), NO_ERROR);
    load("K3BOTH", path, sizeof path);
    check_str("BEGINLIBPATH comes first", path, "C:\\BEGIN\\K3BOTH.DLL");
    ext[0] = '\0';
    DosQueryExtLIBPATH((PSZ)ext, BEGIN_LIBPATH);
    check_str("DosQueryExtLIBPATH BEGIN", ext, "C:\\BEGIN");

    load("K3NONE", path, sizeof path);
    check_str("a missing module", path, "error 126");

    printf("libpath_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}
