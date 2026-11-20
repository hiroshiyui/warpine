/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * proc_test.c - processes (ADR-0090): a child's whole exit code reaches
 * its parent, and signal exceptions reach the parent and other processes
 * of the session.
 *
 * The program starts copies of itself; a copy's mode is the command line's
 * second string at pib_pchcmd ("child <mode> [arg]"), never argv.
 *
 *   child exit <n>       DosExit(EXIT_PROCESS, n)
 *   child signalparent   XCPT_SIGNAL_BREAK to its parent; an unknown pid
 *                        is ERROR_INVALID_PROCID; exits 0 when both held
 *   child sleeper        a handler and the signal focus; posts the ready
 *                        semaphore; exits 0x1234 when XCPT_SIGNAL_BREAK
 *                        came (1 after 10 s without)
 *   child sendto <pid>   XCPT_SIGNAL_BREAK to <pid>; exits with the rc
 *
 * The parent checks the exit codes (EXEC_SYNC, DosWaitChild for one child
 * and for any child), that its handler got the child's signal, and that a
 * sibling's signal reached the sleeper.
 *
 * Output: "proc_test: <name>: ok" / "... FAIL got <v> want <v>", then
 * "proc_test: done <passed>/<total>".
 */
#define INCL_DOS
#define INCL_DOSEXCEPTIONS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define READY_SEM "\\SEM32\\PROCTEST\\READY"

static int passed = 0, total = 0;
static volatile ULONG got_signal;

static void check(const char *what, ULONG got, ULONG want)
{
    total++;
    if (got == want) { passed++; printf("proc_test: %s: ok\n", what); }
    else printf("proc_test: %s: FAIL got %lu want %lu\n", what, got, want);
    fflush(stdout);
}

static ULONG APIENTRY handler(PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                              PCONTEXTRECORD ctx, PVOID unused)
{
    (void)reg; (void)ctx; (void)unused;
    if (rep->ExceptionNum == XCPT_SIGNAL) {
        got_signal = rep->ExceptionInfo[0];
        DosAcknowledgeSignalException(rep->ExceptionInfo[0]);
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

/* The command line's arguments: the second string at pib_pchcmd. */
static const char *command_args(PPIB *pppib)
{
    PTIB ptib;
    DosGetInfoBlocks(&ptib, pppib);
    if ((*pppib)->pib_pchcmd == NULL) return "";
    return (*pppib)->pib_pchcmd + strlen((*pppib)->pib_pchcmd) + 1;
}

/* Start a copy of this program in `mode`; EXEC_SYNC fills `res`, the
   asynchronous modes put the child's pid in res->codeTerminate. */
static APIRET start(ULONG how, const char *mode, RESULTCODES *res)
{
    PTIB ptib;
    PPIB ppib;
    char me[CCHMAXPATH], args[CCHMAXPATH + 64], *p;
    DosGetInfoBlocks(&ptib, &ppib);
    if (DosQueryModuleName(ppib->pib_hmte, sizeof(me), me) != NO_ERROR) return 1;
    memset(args, 0, sizeof(args));
    strcpy(args, me);
    p = args + strlen(me) + 1;
    strcpy(p, "child ");
    strcat(p, mode);
    return DosExecPgm(NULL, 0, how, (PSZ)args, NULL, res, (PSZ)me);
}

static int child_main(const char *mode, PPIB ppib)
{
    EXCEPTIONREGISTRATIONRECORD rec;
    ULONG times, i;
    HEV ready;
    if (strncmp(mode, "exit ", 5) == 0) {
        DosExit(EXIT_PROCESS, strtoul(mode + 5, NULL, 10));
    }
    if (strcmp(mode, "signalparent") == 0) {
        APIRET to_parent = DosSendSignalException(ppib->pib_ulppid, XCPT_SIGNAL_BREAK);
        APIRET unknown = DosSendSignalException(9999, XCPT_SIGNAL_BREAK);
        return (to_parent == NO_ERROR && unknown == ERROR_INVALID_PROCID) ? 0 : 1;
    }
    if (strcmp(mode, "sleeper") == 0) {
        memset(&rec, 0, sizeof(rec));
        rec.ExceptionHandler = handler;
        DosSetExceptionHandler(&rec);
        DosSetSignalExceptionFocus(SIG_SETFOCUS, &times);
        if (DosOpenEventSem((PSZ)READY_SEM, &ready) == NO_ERROR) {
            DosPostEventSem(ready);
            DosCloseEventSem(ready);
        }
        for (i = 0; i < 200 && got_signal == 0; i++) DosSleep(50);
        DosUnsetExceptionHandler(&rec);
        return got_signal == XCPT_SIGNAL_BREAK ? 0x1234 : 1;
    }
    if (strncmp(mode, "sendto ", 7) == 0) {
        return (int)DosSendSignalException(strtoul(mode + 7, NULL, 10), XCPT_SIGNAL_BREAK);
    }
    return 99;
}

int main(void)
{
    PPIB ppib;
    const char *args = command_args(&ppib);
    RESULTCODES res, waited;
    PID ended, sleeper, sender;
    EXCEPTIONREGISTRATIONRECORD rec;
    ULONG times, i;
    HEV ready;
    char mode[32];

    if (strncmp(args, "child ", 6) == 0) return child_main(args + 6, ppib);

    /* Whole exit codes. */
    check("EXEC_SYNC", start(EXEC_SYNC, "exit 74565", &res), NO_ERROR);
    check("-> codeTerminate TC_EXIT", res.codeTerminate, TC_EXIT);
    check("-> the whole exit code 0x12345", res.codeResult, 0x12345);
    check("EXEC_ASYNCRESULT", start(EXEC_ASYNCRESULT, "exit 66051", &res), NO_ERROR);
    check("DosWaitChild(DCWA_PROCESS)", DosWaitChild(DCWA_PROCESS, DCWW_WAIT, &waited, &ended, res.codeTerminate), NO_ERROR);
    check("-> the whole exit code 0x10203", waited.codeResult, 0x10203);
    check("EXEC_ASYNCRESULT again", start(EXEC_ASYNCRESULT, "exit 258", &res), NO_ERROR);
    check("DosWaitChild(DCWA_PROCESSTREE)", DosWaitChild(DCWA_PROCESSTREE, DCWW_WAIT, &waited, &ended, 0), NO_ERROR);
    check("-> that child", ended, res.codeTerminate);
    check("-> its whole exit code 258", waited.codeResult, 258);

    /* A child signals its parent. */
    memset(&rec, 0, sizeof(rec));
    rec.ExceptionHandler = handler;
    DosSetExceptionHandler(&rec);
    DosSetSignalExceptionFocus(SIG_SETFOCUS, &times);
    got_signal = 0;
    check("start the signalling child", start(EXEC_ASYNCRESULT, "signalparent", &res), NO_ERROR);
    for (i = 0; i < 200 && got_signal == 0; i++) DosSleep(50);
    check("the parent's handler got XCPT_SIGNAL_BREAK", got_signal, XCPT_SIGNAL_BREAK);
    DosWaitChild(DCWA_PROCESS, DCWW_WAIT, &waited, &ended, res.codeTerminate);
    check("-> the child's sends: parent ok, unknown pid ERROR_INVALID_PROCID", waited.codeResult, 0);
    DosSetSignalExceptionFocus(SIG_UNSETFOCUS, &times);
    DosUnsetExceptionHandler(&rec);

    /* A child signals its sibling. */
    check("DosCreateEventSem(ready)", DosCreateEventSem((PSZ)READY_SEM, &ready, 0, FALSE), NO_ERROR);
    check("start the sleeper", start(EXEC_ASYNCRESULT, "sleeper", &res), NO_ERROR);
    sleeper = res.codeTerminate;
    check("-> it is ready", DosWaitEventSem(ready, 10000), NO_ERROR);
    sprintf(mode, "sendto %lu", sleeper);
    check("start the sender", start(EXEC_ASYNCRESULT, mode, &res), NO_ERROR);
    sender = res.codeTerminate;
    DosWaitChild(DCWA_PROCESS, DCWW_WAIT, &waited, &ended, sender);
    check("-> its DosSendSignalException to the sibling", waited.codeResult, NO_ERROR);
    DosWaitChild(DCWA_PROCESS, DCWW_WAIT, &waited, &ended, sleeper);
    check("-> the sleeper got it (exit code 0x1234)", waited.codeResult, 0x1234);
    DosCloseEventSem(ready);

    printf("proc_test: done %d/%d\n", passed, total);
    return passed == total ? 0 : 1;
}
