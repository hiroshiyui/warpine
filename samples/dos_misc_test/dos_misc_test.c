/* SPDX-License-Identifier: GPL-3.0-or-later */
/* dos_misc_test.c - smaller DOSCALLS / QUECALLS calls (ADR-0086): the
   high-resolution timer, DosQueryModFromEIP, DosErrClass,
   DosQueryProcType, DosVerifyPidTid, the DosProtect* file calls,
   DosCancelLockRequest, and DosPeekQueue with element codes (and
   DosReadQueue's DCWW_NOWAIT). Writes "ok: name" or "FAIL: name" per
   check, then "dos_misc_test: done <passed>/<total>". */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <string.h>

/* Calls whose prototypes Open Watcom's headers lack (Toolkit bsedos.h). */
APIRET APIENTRY DosVerifyPidTid(PID pid, TID tid);

static int passed = 0, total = 0;

static void check(const char *what, int good)
{
    total++;
    if (good) { passed++; printf("ok: %s\n", what); }
    else printf("FAIL: %s\n", what);
}

int main(void)
{
    ULONG   freq = 0, cls, act, loc, obj, off, type, action, n, state;
    QWORD   t1, t2;
    HMODULE hmod;
    char    name[64], buf[16];
    PTIB    ptib;
    PPIB    ppib;
    HFILE   hf;
    FHLOCK  lock;
    HQUEUE  hq;
    REQUESTDATA req;
    PVOID   data;
    ULONG   len, elem;
    BYTE    prio;
    FILELOCK fl;
    FILESTATUS3 fs3;

    /* The timer. */
    check("DosTmrQueryFreq", DosTmrQueryFreq(&freq) == NO_ERROR && freq > 1000000);
    DosTmrQueryTime(&t1);
    DosSleep(30);
    DosTmrQueryTime(&t2);
    check("DosTmrQueryTime advances", t2.ulLo - t1.ulLo > freq / 100 || t2.ulHi > t1.ulHi);

    /* The module an address lies in. */
    check("DosQueryModFromEIP", DosQueryModFromEIP(&hmod, &obj, sizeof name, name, &off, (ULONG)main) == NO_ERROR);
    check("DosQueryModFromEIP name", strcmp(name, "DOS_MISC_TEST") == 0);
    check("DosQueryModFromEIP object", obj == 0 && off > 0);
    check("DosQueryModFromEIP elsewhere", DosQueryModFromEIP(&hmod, &obj, sizeof name, name, &off, 16) == ERROR_INVALID_ADDRESS);
    check("DosQueryModFromEIP small buffer", DosQueryModFromEIP(&hmod, &obj, 3, name, &off, (ULONG)main) == ERROR_BUFFER_OVERFLOW);

    /* Error classes, procedure types, threads. */
    check("DosErrClass", DosErrClass(ERROR_FILE_NOT_FOUND, &cls, &act, &loc) == NO_ERROR
          && cls == ERRCLASS_NOTFND && act == ERRACT_USER && loc == ERRLOC_DISK);
    DosQueryModuleHandle("DOSCALLS", &hmod);
    check("DosQueryProcType", DosQueryProcType(hmod, 234, NULL, &type) == NO_ERROR && type == PT_32BIT);
    DosGetInfoBlocks(&ptib, &ppib);
    check("DosVerifyPidTid", DosVerifyPidTid(ppib->pib_ulpid, ptib->tib_ptib2->tib2_ultid) == NO_ERROR);
    check("DosVerifyPidTid no thread", DosVerifyPidTid(ppib->pib_ulpid, 99) == ERROR_INVALID_THREADID);
    check("DosVerifyPidTid no process", DosVerifyPidTid(ppib->pib_ulpid + 1000, 1) == ERROR_INVALID_PROCID);

    /* A protected file handle. */
    check("DosProtectOpen", DosProtectOpen("PROT.TMP", &hf, &action, 0, FILE_NORMAL,
          OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
          OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL, &lock) == NO_ERROR);
    check("DosProtectWrite", DosProtectWrite(hf, (ULONG)"warpine", 7, &n, lock) == NO_ERROR && n == 7);
    check("DosProtectSetFilePtr", DosProtectSetFilePtr(hf, 2, FILE_BEGIN, &n, lock) == NO_ERROR && n == 2);
    check("DosProtectRead", DosProtectRead(hf, buf, 3, &n, lock) == NO_ERROR && n == 3 && memcmp(buf, "rpi", 3) == 0);
    check("DosProtectQueryFHState", DosProtectQueryFHState(hf, &state, lock) == NO_ERROR);
    check("DosProtectQueryFileInfo", DosProtectQueryFileInfo(hf, FIL_STANDARD, &fs3, sizeof fs3, lock) == NO_ERROR && fs3.cbFile == 7);
    check("DosProtectSetFileSize", DosProtectSetFileSize(hf, 4, lock) == NO_ERROR);
    check("DosProtect wrong lock", DosProtectRead(hf, buf, 1, &n, lock + 2) == ERROR_INVALID_HANDLE);
    memset(&fl, 0, sizeof fl);
    check("DosCancelLockRequest", DosCancelLockRequest(hf, &fl) == NO_ERROR);
    check("DosProtectClose", DosProtectClose(hf, lock) == NO_ERROR);
    DosDelete("PROT.TMP");

    /* Queues: peeking by element code, reading one element. */
    DosCreateQueue(&hq, QUE_FIFO, "\\QUEUES\\DOSMISC");
    DosWriteQueue(hq, 1, 6, "first", 0);
    DosWriteQueue(hq, 2, 7, "second", 0);
    elem = 0;
    check("DosPeekQueue first", DosPeekQueue(hq, &req, &len, &data, &elem, DCWW_NOWAIT, &prio, 0) == NO_ERROR
          && strcmp((char *)data, "first") == 0 && req.ulData == 1);
    check("DosPeekQueue next", DosPeekQueue(hq, &req, &len, &data, &elem, DCWW_NOWAIT, &prio, 0) == NO_ERROR
          && strcmp((char *)data, "second") == 0);
    check("DosPeekQueue past the end", DosPeekQueue(hq, &req, &len, &data, &elem, DCWW_NOWAIT, &prio, 0) == ERROR_QUE_EMPTY);
    check("DosReadQueue an element", DosReadQueue(hq, &req, &len, &data, elem, DCWW_NOWAIT, &prio, 0) == NO_ERROR
          && strcmp((char *)data, "second") == 0);
    DosQueryQueue(hq, &n);
    check("the other element stays", n == 1);
    check("DosReadQueue first", DosReadQueue(hq, &req, &len, &data, 0, DCWW_NOWAIT, &prio, 0) == NO_ERROR
          && strcmp((char *)data, "first") == 0);
    check("DosReadQueue DCWW_NOWAIT empty", DosReadQueue(hq, &req, &len, &data, 0, DCWW_NOWAIT, &prio, 0) == ERROR_QUE_EMPTY);
    check("DosReadQueue a gone element", DosReadQueue(hq, &req, &len, &data, elem, DCWW_NOWAIT, &prio, 0) == ERROR_QUE_ELEMENT_NOT_EXIST);
    DosCloseQueue(hq);

    printf("dos_misc_test: done %d/%d\n", passed, total);
    return total - passed;
}
