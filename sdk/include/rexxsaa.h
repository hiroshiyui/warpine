/* SPDX-License-Identifier: GPL-3.0-or-later */
/* rexxsaa.h - the REXX programming interface (REXX, REXXAPI) for GCC and
 * clang (ADR-0101), declared from IBM's REXX Reference and Warpine's own
 * REXX API (ADR-0085). Values are Warpine's where it has them. */
#ifndef WARPINE_REXXSAA_H
#define WARPINE_REXXSAA_H

#include <os2.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct _RXSTRING {
    ULONG strlength;
    PCH   strptr;
} RXSTRING, *PRXSTRING;

#define MAKERXSTRING(r, p, l)  ((r).strptr = (PCH)(p), (r).strlength = (ULONG)(l))
#define RXNULLSTRING(r)        (!(r).strptr)
#define RXZEROLENSTRING(r)     ((r).strptr && !(r).strlength)
#define RXVALIDSTRING(r)       ((r).strptr && (r).strlength)
#define RXSTRLEN(r)            (RXNULLSTRING(r) ? 0L : (r).strlength)
#define RXSTRPTR(r)            ((r).strptr)
#define RXAUTOBUFLEN           256L

/* RexxStart's call types. */
#define RXCOMMAND     0
#define RXSUBROUTINE  1
#define RXFUNCTION    2

/* System exits: the exits and their subfunctions. */
#define RXENDLST   0
#define RXFNC      2
#define RXFNCCAL   1
#define RXCMD      3
#define RXCMDHST   1
#define RXMSQ      4
#define RXMSQPLL   1
#define RXMSQPSH   2
#define RXMSQSIZ   3
#define RXMSQNAM   20
#define RXSIO      5
#define RXSIOSAY   1
#define RXSIOTRC   2
#define RXSIOTRD   3
#define RXSIODTR   4
#define RXHLT      7
#define RXHLTCLR   1
#define RXHLTTST   2
#define RXTRC      8
#define RXTRCTST   1
#define RXINI      9
#define RXINIEXT   1
#define RXTER      10
#define RXTEREXT   1
#define RXNOOFEXITS 11

#define RXEXIT_HANDLED       0
#define RXEXIT_NOT_HANDLED   1
#define RXEXIT_RAISE_ERROR   (-1)

typedef struct _RXSYSEXIT {
    PSZ  sysexit_name;
    LONG sysexit_code;
} RXSYSEXIT, *PRXSYSEXIT;

typedef struct _RXSIOSAY_PARM { RXSTRING rxsio_string; } RXSIOSAY_PARM;
typedef struct _RXSIOTRC_PARM { RXSTRING rxsio_string; } RXSIOTRC_PARM;
typedef struct _RXSIOTRD_PARM { RXSTRING rxsiotrd_retc; } RXSIOTRD_PARM;
typedef struct _RXSIODTR_PARM { RXSTRING rxsiodtr_retc; } RXSIODTR_PARM;
typedef struct _RXHLTTST_PARM { struct { unsigned rxfhhalt : 1; } rxhlt_flags; } RXHLTTST_PARM;
typedef struct _RXTRCTST_PARM { struct { unsigned rxftrace : 1; } rxtrc_flags; } RXTRCTST_PARM;

/* The variable pool. */
#define RXSHV_SET    0x00
#define RXSHV_FETCH  0x01
#define RXSHV_DROPV  0x02
#define RXSHV_SYSET  0x03
#define RXSHV_SYFET  0x04
#define RXSHV_SYDRO  0x05
#define RXSHV_NEXTV  0x06
#define RXSHV_PRIV   0x07
#define RXSHV_EXIT   0x08
#define RXSHV_NOAVL  144
#define RXSHV_OK     0x00
#define RXSHV_NEWV   0x01
#define RXSHV_LVAR   0x02
#define RXSHV_TRUNC  0x04
#define RXSHV_BADN   0x08
#define RXSHV_MEMFL  0x10
#define RXSHV_BADF   0x80

typedef struct _SHVBLOCK {
    struct _SHVBLOCK *shvnext;
    RXSTRING          shvname;
    RXSTRING          shvvalue;
    ULONG             shvnamelen;
    ULONG             shvvaluelen;
    UCHAR             shvcode;
    UCHAR             shvret;
} SHVBLOCK, *PSHVBLOCK;

/* Subcommand handlers, exits, external functions. */
#define RXSUBCOM_DROPPABLE  0x00
#define RXSUBCOM_NONDROP    0x01
#define RXSUBCOM_ERROR      0x0001
#define RXSUBCOM_FAILURE    0x0002
#define RXSUBCOM_OK         0
#define RXSUBCOM_ISREG      0x01
#define RXSUBCOM_DUP        10
#define RXSUBCOM_NOTREG     30
#define RXSUBCOM_NOCANDROP  40
#define RXSUBCOM_LOADERR    50
#define RXSUBCOM_BADENTRY   1001
#define RXSUBCOM_NOEMEM     1002
#define RXSUBCOM_BADTYPE    1003

#define RXEXIT_DROPPABLE    0x00
#define RXEXIT_NONDROP      0x01
#define RXEXIT_OK           0
#define RXEXIT_ISREG        0x01
#define RXEXIT_DUP          10
#define RXEXIT_NOTREG       30
#define RXEXIT_NOCANDROP    40
#define RXEXIT_LOADERR      50
#define RXEXIT_BADENTRY     1001
#define RXEXIT_NOEMEM       1002
#define RXEXIT_BADTYPE      1003

#define RXFUNC_OK           0
#define RXFUNC_DEFINED      10
#define RXFUNC_NOMEM        20
#define RXFUNC_NOTREG       30
#define RXFUNC_MODNOTFND    40
#define RXFUNC_ENTNOTFND    50

typedef PUCHAR PEXIT;
typedef LONG  APIENTRY RexxSubcomHandler(PRXSTRING, PUSHORT, PRXSTRING);
typedef LONG  APIENTRY RexxExitHandler(LONG, LONG, PEXIT);
typedef ULONG APIENTRY RexxFunctionHandler(PUCHAR, ULONG, PRXSTRING, PSZ, PRXSTRING);

/* External data queues. */
#define RXQUEUE_FIFO         0
#define RXQUEUE_LIFO         1
#define RXQUEUE_NOWAIT       0
#define RXQUEUE_WAIT         1
#define RXQUEUE_OK           0
#define RXQUEUE_STORAGE      1
#define RXQUEUE_SIZE         2
#define RXQUEUE_DUP          3
#define RXQUEUE_BADQNAME     5
#define RXQUEUE_PRIORITY     6
#define RXQUEUE_BADWAITFLAG  7
#define RXQUEUE_EMPTY        8
#define RXQUEUE_NOTREG       9

/* The macro space. */
#define RXMACRO_SEARCH_BEFORE  1
#define RXMACRO_SEARCH_AFTER   2
#define RXMACRO_OK             0
#define RXMACRO_NO_STORAGE     1
#define RXMACRO_NOT_FOUND      2

LONG   APIENTRY RexxStart(LONG argc, PRXSTRING argv, PCSZ pszProcName, PRXSTRING pInstore, PCSZ pszEnvName,
                          LONG lCallType, PRXSYSEXIT pExits, PSHORT pusRetCode, PRXSTRING pRexxRetVal);
ULONG  APIENTRY RexxVariablePool(PSHVBLOCK pRequest);
APIRET APIENTRY RexxSetHalt(PID pid, TID tid);
APIRET APIENTRY RexxSetTrace(PID pid, TID tid);
APIRET APIENTRY RexxResetTrace(PID pid, TID tid);

APIRET APIENTRY RexxRegisterSubcomExe(PCSZ pszEnvName, PFN pfnEntryPoint, PUCHAR pUserArea);
APIRET APIENTRY RexxRegisterSubcomDll(PCSZ pszEnvName, PCSZ pszDllName, PCSZ pszProcName, PUCHAR pUserArea,
                                      ULONG ulDrop);
APIRET APIENTRY RexxDeregisterSubcom(PCSZ pszEnvName, PCSZ pszDllName);
APIRET APIENTRY RexxQuerySubcom(PCSZ pszEnvName, PCSZ pszDllName, PUSHORT pExist, PUCHAR pUserWord);
APIRET APIENTRY RexxRegisterExitExe(PCSZ pszEnvName, PFN pfnEntryPoint, PUCHAR pUserArea);
APIRET APIENTRY RexxRegisterExitDll(PCSZ pszEnvName, PCSZ pszDllName, PCSZ pszProcName, PUCHAR pUserArea,
                                    ULONG ulDrop);
APIRET APIENTRY RexxDeregisterExit(PCSZ pszEnvName, PCSZ pszDllName);
APIRET APIENTRY RexxQueryExit(PCSZ pszEnvName, PCSZ pszDllName, PUSHORT pExist, PUCHAR pUserWord);
APIRET APIENTRY RexxRegisterFunctionExe(PCSZ pszName, PFN pfnEntryPoint);
APIRET APIENTRY RexxRegisterFunctionDll(PCSZ pszName, PCSZ pszDllName, PCSZ pszProcName);
APIRET APIENTRY RexxDeregisterFunction(PCSZ pszName);
APIRET APIENTRY RexxQueryFunction(PCSZ pszName);

APIRET APIENTRY RexxCreateQueue(PSZ pszBuffer, ULONG cbBuffer, PSZ pszRequestedName, PULONG pDupFlag);
APIRET APIENTRY RexxDeleteQueue(PSZ pszQueueName);
APIRET APIENTRY RexxQueryQueue(PSZ pszQueueName, PULONG pCount);
APIRET APIENTRY RexxAddQueue(PSZ pszQueueName, PRXSTRING pEntryData, ULONG ulAddFlag);
APIRET APIENTRY RexxPullQueue(PSZ pszQueueName, PRXSTRING pDataBuf, PDATETIME pTimeStamp, ULONG ulWaitFlag);

APIRET APIENTRY RexxAddMacro(PCSZ pszFuncName, PCSZ pszSourceFile, ULONG ulPosition);
APIRET APIENTRY RexxDropMacro(PCSZ pszFuncName);
APIRET APIENTRY RexxSaveMacroSpace(ULONG ulFuncCount, PCSZ *apszFuncNames, PCSZ pszMacroLibFile);
APIRET APIENTRY RexxLoadMacroSpace(ULONG ulFuncCount, PCSZ *apszFuncNames, PCSZ pszMacroLibFile);
APIRET APIENTRY RexxQueryMacro(PCSZ pszFuncName, PUSHORT pPosition);
APIRET APIENTRY RexxReorderMacro(PCSZ pszFuncName, ULONG ulPosition);
APIRET APIENTRY RexxClearMacroSpace(void);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_REXXSAA_H */
