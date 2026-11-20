/* SPDX-License-Identifier: GPL-3.0-or-later */
/* tlm_test.c - DosAllocThreadLocalMemory / DosFreeThreadLocalMemory
   (ADR-0088): one pointer, each thread's own words behind it, also for
   what an API call writes there; the words reserved in every thread; the
   error codes. Writes "ok: name" or "FAIL: name" per check, then
   "tlm_test: done <passed>/<total>". */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>

static int passed = 0, total = 0;
static PULONG tlm;           /* two words */
static HEV    written, checked;
static ULONG  seen_by_2[2];
static APIRET rc_by_2;

static void check(const char *what, int good)
{
    total++;
    if (good) { passed++; printf("ok: %s\n", what); }
    else printf("FAIL: %s\n", what);
}

static VOID APIENTRY second(ULONG arg)
{
    (void)arg;
    seen_by_2[0] = tlm[0];             /* zero: a new thread's words */
    tlm[0] = 222;
    /* An API call writing into the area writes this thread's words. */
    rc_by_2 = DosQuerySysInfo(QSV_MAX_PATH_LENGTH, QSV_MAX_PATH_LENGTH, &tlm[1], sizeof(ULONG));
    DosPostEventSem(written);
    DosWaitEventSem(checked, SEM_INDEFINITE_WAIT);
    seen_by_2[0] = tlm[0];
    seen_by_2[1] = tlm[1];
}

int main(void)
{
    PULONG more, eight[4];
    TID    tid;
    int    i, got = 0;

    check("DosAllocThreadLocalMemory", DosAllocThreadLocalMemory(2, &tlm) == NO_ERROR && tlm != NULL);
    check("the words start zero", tlm[0] == 0 && tlm[1] == 0);
    tlm[0] = 111;
    tlm[1] = 7;
    check("DosAllocThreadLocalMemory 9 words", DosAllocThreadLocalMemory(9, &more) == ERROR_INVALID_PARAMETER);
    check("DosAllocThreadLocalMemory 0 words", DosAllocThreadLocalMemory(0, &more) == ERROR_INVALID_PARAMETER);
    check("DosAllocThreadLocalMemory another", DosAllocThreadLocalMemory(1, &more) == NO_ERROR && more == tlm + 2);

    DosCreateEventSem(NULL, &written, 0, FALSE);
    DosCreateEventSem(NULL, &checked, 0, FALSE);
    check("DosCreateThread", DosCreateThread(&tid, second, 0, 0, 0x4000) == NO_ERROR);
    DosWaitEventSem(written, SEM_INDEFINITE_WAIT);
    check("thread 2 saw its own zero", seen_by_2[0] == 0);
    check("thread 1 keeps its words", tlm[0] == 111 && tlm[1] == 7);
    check("thread 2's API call succeeded", rc_by_2 == NO_ERROR);
    DosPostEventSem(checked);
    DosWaitThread(&tid, DCWW_WAIT);
    check("thread 2 kept its words", seen_by_2[0] == 222 && seen_by_2[1] == 260);
    check("thread 1's still", tlm[0] == 111 && tlm[1] == 7);

    /* The area holds 32 words: 2 + 1 used, so three blocks of 8 more fit. */
    for (i = 0; i < 4; i++)
        if (DosAllocThreadLocalMemory(8, &eight[i]) == NO_ERROR) got++;
    check("the area holds 32 words", got == 3);
    check("DosFreeThreadLocalMemory", DosFreeThreadLocalMemory(more) == NO_ERROR);
    check("DosFreeThreadLocalMemory again", DosFreeThreadLocalMemory(more) == ERROR_INVALID_PARAMETER);
    check("DosFreeThreadLocalMemory inside a block", DosFreeThreadLocalMemory(tlm + 1) == ERROR_INVALID_PARAMETER);
    check("the freed word again", DosAllocThreadLocalMemory(1, &more) == NO_ERROR && more == tlm + 2);

    printf("tlm_test: done %d/%d\n", passed, total);
    return total - passed;
}
