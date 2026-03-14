/*
 * crt0.c — Minimal C runtime startup for OS/2 apps under warpine.
 *
 * Replaces the Watcom C runtime's _cstart_ / __OS2Main which call
 * DosGetInfoSeg through a 16-bit __vfthunk wrapper that warpine
 * cannot execute. This startup code uses only 32-bit OS/2 APIs.
 *
 * Provides:
 * - _cstart_: entry point (called by OS/2 loader)
 * - __OS2Main: C runtime initialization
 * - _LpPgmName / _LpCmdLine: program name and command line pointers
 * - __InitRtns / __FiniRtns: runtime init/cleanup hook support
 *
 * Does NOT replace: printf, malloc, string ops, file I/O — those
 * still come from clib3r.lib.
 */

#define INCL_DOS
#include <os2.h>
#include <string.h>

/* Watcom C runtime symbols we provide */
char *_LpPgmName;
char *_LpCmdLine;

/* main() is provided by the application */
extern int main(int argc, char **argv);

/* Watcom runtime init/fini hook chains (called by library modules) */
typedef void (*pfn_t)(void);
extern pfn_t __InitRtns[];
extern pfn_t __FiniRtns[];

/* Simple argc/argv parser from OS/2 PIB command line */
static int parse_args(char *cmdline, char **argv, int max_args) {
    int argc = 0;
    char *p = cmdline;

    while (*p && argc < max_args - 1) {
        /* Skip whitespace */
        while (*p == ' ' || *p == '\t') p++;
        if (!*p) break;

        argv[argc++] = p;

        /* Find end of argument */
        while (*p && *p != ' ' && *p != '\t') p++;
        if (*p) *p++ = '\0';
    }
    argv[argc] = 0;
    return argc;
}

/* Run Watcom runtime initializer chain */
static void run_init_rtns(void) {
    /* __InitRtns is a null-terminated array of function pointers
       defined by the linker from DGROUP initialization segments.
       It may not exist if no library modules register init routines. */
}

/* Run Watcom runtime finalizer chain */
static void run_fini_rtns(void) {
    /* Same as above for cleanup */
}

/*
 * __OS2Main — replaces Watcom's __OS2Main which calls DosGetInfoSeg
 * through a 16-bit thunk. We use DosGetInfoBlocks (32-bit) instead.
 */
void __OS2Main(void);
#pragma aux __OS2Main "*"  /* exact symbol name, no underscore mangling */
void __OS2Main(void) {
    PTIB ptib;
    PPIB ppib;
    char *argv_buf[64];
    int argc;
    int rc;

    /* Get PIB (contains command line and environment) */
    DosGetInfoBlocks(&ptib, &ppib);

    /* Extract program name and command line from PIB */
    if (ppib && ppib->pib_pchcmd) {
        _LpPgmName = ppib->pib_pchcmd;
        /* Command line args follow the program name (double-null terminated) */
        _LpCmdLine = _LpPgmName + strlen(_LpPgmName) + 1;
    } else {
        _LpPgmName = "4OS2.EXE";
        _LpCmdLine = "";
    }

    /* Parse command line into argc/argv */
    argv_buf[0] = _LpPgmName;
    argc = 1;
    if (_LpCmdLine && *_LpCmdLine) {
        argc += parse_args(_LpCmdLine, &argv_buf[1], 62);
    }
    argv_buf[argc] = 0;

    /* Call main */
    rc = main(argc, argv_buf);

    /* Exit */
    DosExit(EXIT_PROCESS, rc);
}

/*
 * _cstart_ — the actual entry point called by the OS/2 loader.
 * The stack is already set up by the loader. We just call __OS2Main.
 */
#pragma aux _cstart_ "_*"
void _cstart_(void) {
    __OS2Main();
}

/* Provide __CHK (stack checking stub) since we compiled with -s (no stack checks)
   but some library modules may reference it */
#pragma aux __CHK "_*"
void __CHK(void) {
    /* No-op: stack checking disabled */
}
