/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/dos.h - the Control Program (DOSCALLS and QUECALLS), declared from
 * IBM's Control Program Programming Reference for GCC and clang
 * (ADR-0096). Every call is APIENTRY (the 32-bit system convention). */
#ifndef WARPINE_OS2_DOS_H
#define WARPINE_OS2_DOS_H

#include <os2/os2def.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── Limits ─────────────────────────────────────────────────────────── */
#define CCHMAXPATH       260
#define CCHMAXPATHCOMP   256

/* ── Standard handles ───────────────────────────────────────────────── */
#define HF_STDIN   0
#define HF_STDOUT  1
#define HF_STDERR  2

/* ── Dates and times ────────────────────────────────────────────────── */
typedef struct _FDATE {
    USHORT day   : 5;
    USHORT month : 4;
    USHORT year  : 7;   /* since 1980 */
} FDATE, *PFDATE;

typedef struct _FTIME {
    USHORT twosecs : 5;
    USHORT minutes : 6;
    USHORT hours   : 5;
} FTIME, *PFTIME;

typedef struct _DATETIME {
    UCHAR  hours;
    UCHAR  minutes;
    UCHAR  seconds;
    UCHAR  hundredths;
    UCHAR  day;
    UCHAR  month;
    USHORT year;
    SHORT  timezone;    /* minutes west of UTC, -1 when undefined */
    UCHAR  weekday;     /* 0 = Sunday */
} DATETIME, *PDATETIME;

/* ── Files ──────────────────────────────────────────────────────────── */
/* DosOpen: the action taken (*pulAction). */
#define FILE_EXISTED     0x0001
#define FILE_CREATED     0x0002
#define FILE_TRUNCATED   0x0003

/* DosOpen: fsOpenFlags. */
#define OPEN_ACTION_FAIL_IF_EXISTS     0x0000
#define OPEN_ACTION_OPEN_IF_EXISTS     0x0001
#define OPEN_ACTION_REPLACE_IF_EXISTS  0x0002
#define OPEN_ACTION_FAIL_IF_NEW        0x0000
#define OPEN_ACTION_CREATE_IF_NEW      0x0010

/* DosOpen: fsOpenMode. */
#define OPEN_ACCESS_READONLY       0x0000
#define OPEN_ACCESS_WRITEONLY      0x0001
#define OPEN_ACCESS_READWRITE      0x0002
#define OPEN_SHARE_DENYREADWRITE   0x0010
#define OPEN_SHARE_DENYWRITE       0x0020
#define OPEN_SHARE_DENYREAD        0x0030
#define OPEN_SHARE_DENYNONE        0x0040
#define OPEN_FLAGS_NOINHERIT       0x0080
#define OPEN_FLAGS_SEQUENTIAL      0x0100
#define OPEN_FLAGS_RANDOM          0x0200
#define OPEN_FLAGS_NO_CACHE        0x1000
#define OPEN_FLAGS_FAIL_ON_ERROR   0x2000
#define OPEN_FLAGS_WRITE_THROUGH   0x4000
#define OPEN_FLAGS_DASD            0x8000

/* File attributes; MUST_HAVE_* in DosFindFirst's flAttribute. */
#define FILE_NORMAL      0x0000
#define FILE_READONLY    0x0001
#define FILE_HIDDEN      0x0002
#define FILE_SYSTEM      0x0004
#define FILE_DIRECTORY   0x0010
#define FILE_ARCHIVED    0x0020
#define MUST_HAVE_READONLY   (FILE_READONLY  << 8)
#define MUST_HAVE_HIDDEN     (FILE_HIDDEN    << 8)
#define MUST_HAVE_SYSTEM     (FILE_SYSTEM    << 8)
#define MUST_HAVE_DIRECTORY  (FILE_DIRECTORY << 8)
#define MUST_HAVE_ARCHIVED   (FILE_ARCHIVED  << 8)

/* DosSetFilePtr. */
#define FILE_BEGIN    0
#define FILE_CURRENT  1
#define FILE_END      2

/* Information levels. */
#define FIL_STANDARD          1
#define FIL_QUERYEASIZE       2
#define FIL_QUERYEASFROMLIST  3
#define FIL_QUERYFULLNAME     5

/* DosFindFirst. */
#define HDIR_SYSTEM  ((HDIR)1)
#define HDIR_CREATE  ((HDIR)-1)

/* DosCopy. */
#define DCPY_EXISTING  0x0001
#define DCPY_APPEND    0x0002
#define DCPY_FAILEAS   0x0004

/* DosSetFHState / DosQueryFHState. */
#define OPEN_FLAGS_INHERIT_MASK  OPEN_FLAGS_NOINHERIT

/* DosQueryHType: the handle type in the low byte. */
#define HANDTYPE_FILE    0x0000
#define HANDTYPE_DEVICE  0x0001
#define HANDTYPE_PIPE    0x0002

typedef struct _FILESTATUS3 {
    FDATE fdateCreation;
    FTIME ftimeCreation;
    FDATE fdateLastAccess;
    FTIME ftimeLastAccess;
    FDATE fdateLastWrite;
    FTIME ftimeLastWrite;
    ULONG cbFile;
    ULONG cbFileAlloc;
    ULONG attrFile;
} FILESTATUS3, *PFILESTATUS3;

typedef struct _FILESTATUS4 {
    FDATE fdateCreation;
    FTIME ftimeCreation;
    FDATE fdateLastAccess;
    FTIME ftimeLastAccess;
    FDATE fdateLastWrite;
    FTIME ftimeLastWrite;
    ULONG cbFile;
    ULONG cbFileAlloc;
    ULONG attrFile;
    ULONG cbList;
} FILESTATUS4, *PFILESTATUS4;

/* The find buffers are packed: the name follows its length byte. Use
 * oNextEntryOffset (0 for the last entry) to walk several. */
#pragma pack(push, 1)
typedef struct _FILEFINDBUF3 {
    ULONG oNextEntryOffset;
    FDATE fdateCreation;
    FTIME ftimeCreation;
    FDATE fdateLastAccess;
    FTIME ftimeLastAccess;
    FDATE fdateLastWrite;
    FTIME ftimeLastWrite;
    ULONG cbFile;
    ULONG cbFileAlloc;
    ULONG attrFile;
    UCHAR cchName;
    CHAR  achName[CCHMAXPATHCOMP];
} FILEFINDBUF3, *PFILEFINDBUF3;

typedef struct _FILEFINDBUF4 {
    ULONG oNextEntryOffset;
    FDATE fdateCreation;
    FTIME ftimeCreation;
    FDATE fdateLastAccess;
    FTIME ftimeLastAccess;
    FDATE fdateLastWrite;
    FTIME ftimeLastWrite;
    ULONG cbFile;
    ULONG cbFileAlloc;
    ULONG attrFile;
    ULONG cbList;
    UCHAR cchName;
    CHAR  achName[CCHMAXPATHCOMP];
} FILEFINDBUF4, *PFILEFINDBUF4;
#pragma pack(pop)

typedef struct _FILELOCK {
    LONG lOffset;
    LONG lRange;
} FILELOCK, *PFILELOCK;

/* ── Extended attributes ────────────────────────────────────────────── */
#define FEA_NEEDEA               0x80
#define ENUMEA_LEVEL_NO_VALUE    1
#define ENUMEA_REFTYPE_FHANDLE   0
#define ENUMEA_REFTYPE_PATH      1
#define ENUMEA_REFTYPE_MVMT      2

/* Packed: each name follows its length byte, and entries are walked by
 * oNextEntryOffset (0 for the last). */
#pragma pack(push, 1)
typedef struct _GEA2 {
    ULONG oNextEntryOffset;
    BYTE  cbName;
    CHAR  szName[1];
} GEA2, *PGEA2;

typedef struct _GEA2LIST {
    ULONG cbList;
    GEA2  list[1];
} GEA2LIST, *PGEA2LIST;

typedef struct _FEA2 {
    ULONG  oNextEntryOffset;
    BYTE   fEA;
    BYTE   cbName;
    USHORT cbValue;
    CHAR   szName[1];
} FEA2, *PFEA2;

typedef struct _FEA2LIST {
    ULONG cbList;
    FEA2  list[1];
} FEA2LIST, *PFEA2LIST;
#pragma pack(pop)

/* DosEnumAttribute's entries have the FEA2 layout. */
typedef FEA2 DENA2, *PDENA2;

typedef struct _EAOP2 {
    PGEA2LIST fpGEA2List;
    PFEA2LIST fpFEA2List;
    ULONG     oError;
} EAOP2, *PEAOP2;

APIRET APIENTRY DosOpen(PCSZ pszFileName, PHFILE phf, PULONG pulAction, ULONG cbFile,
                        ULONG ulAttribute, ULONG fsOpenFlags, ULONG fsOpenMode, PEAOP2 peaop2);
APIRET APIENTRY DosClose(HFILE hFile);
APIRET APIENTRY DosRead(HFILE hFile, PVOID pBuffer, ULONG cbRead, PULONG pcbActual);
APIRET APIENTRY DosWrite(HFILE hFile, const void *pBuffer, ULONG cbWrite, PULONG pcbActual);
APIRET APIENTRY DosSetFilePtr(HFILE hFile, LONG ib, ULONG method, PULONG ibActual);
APIRET APIENTRY DosSetFileSize(HFILE hFile, ULONG cbSize);
APIRET APIENTRY DosResetBuffer(HFILE hFile);
APIRET APIENTRY DosDupHandle(HFILE hFile, PHFILE pHfile);
APIRET APIENTRY DosQueryHType(HFILE hFile, PULONG pType, PULONG pAttr);
APIRET APIENTRY DosQueryFHState(HFILE hFile, PULONG pMode);
APIRET APIENTRY DosSetFHState(HFILE hFile, ULONG mode);
APIRET APIENTRY DosSetMaxFH(ULONG cFH);
APIRET APIENTRY DosSetRelMaxFH(PLONG pcbReqCount, PULONG pcbCurMaxFH);
APIRET APIENTRY DosSetFileLocks(HFILE hFile, PFILELOCK pflUnlock, PFILELOCK pflLock,
                                ULONG timeout, ULONG flags);
APIRET APIENTRY DosCancelLockRequest(HFILE hFile, PFILELOCK pflLock);
APIRET APIENTRY DosCreatePipe(PHFILE phfRead, PHFILE phfWrite, ULONG cb);

APIRET APIENTRY DosDelete(PCSZ pszFile);
APIRET APIENTRY DosForceDelete(PCSZ pszFile);
APIRET APIENTRY DosMove(PCSZ pszOld, PCSZ pszNew);
APIRET APIENTRY DosCopy(PCSZ pszSource, PCSZ pszTarget, ULONG option);
APIRET APIENTRY DosEditName(ULONG metalevel, PCSZ pszSource, PCSZ pszEdit,
                            PBYTE pszTarget, ULONG cbTarget);
APIRET APIENTRY DosCreateDir(PCSZ pszDirName, PEAOP2 peaop2);
APIRET APIENTRY DosDeleteDir(PCSZ pszDir);
APIRET APIENTRY DosSetCurrentDir(PCSZ pszDir);
APIRET APIENTRY DosQueryCurrentDir(ULONG disknum, PBYTE pBuf, PULONG pcbBuf);
APIRET APIENTRY DosQueryCurrentDisk(PULONG pdisknum, PULONG plogical);
APIRET APIENTRY DosSetDefaultDisk(ULONG disknum);
APIRET APIENTRY DosFindFirst(PCSZ pszFileSpec, PHDIR phdir, ULONG flAttribute, PVOID pfindbuf,
                             ULONG cbBuf, PULONG pcFileNames, ULONG ulInfoLevel);
APIRET APIENTRY DosFindNext(HDIR hDir, PVOID pfindbuf, ULONG cbfindbuf, PULONG pcFilenames);
APIRET APIENTRY DosFindClose(HDIR hDir);
APIRET APIENTRY DosQueryPathInfo(PCSZ pszPathName, ULONG ulInfoLevel, PVOID pInfoBuf, ULONG cbInfoBuf);
APIRET APIENTRY DosQueryFileInfo(HFILE hf, ULONG ulInfoLevel, PVOID pInfo, ULONG cbInfoBuf);
APIRET APIENTRY DosSetPathInfo(PCSZ pszPathName, ULONG ulInfoLevel, PVOID pInfoBuf,
                               ULONG cbInfoBuf, ULONG flOptions);
APIRET APIENTRY DosSetFileInfo(HFILE hf, ULONG ulInfoLevel, PVOID pInfoBuf, ULONG cbInfoBuf);
APIRET APIENTRY DosEnumAttribute(ULONG ulRefType, PVOID pvFile, ULONG ulEntry, PVOID pvBuf,
                                 ULONG cbBuf, PULONG pulCount, ULONG ulInfoLevel);
APIRET APIENTRY DosQueryFSInfo(ULONG disknum, ULONG infolevel, PVOID pBuf, ULONG cbBuf);
APIRET APIENTRY DosQueryFSAttach(PCSZ pszDeviceName, ULONG ulOrdinal, ULONG ulFSAInfoLevel,
                                 PVOID pfsqb, PULONG pcbBuffLength);
APIRET APIENTRY DosSetVerify(BOOL32 fVerify);
APIRET APIENTRY DosQueryVerify(PBOOL32 pBool);
APIRET APIENTRY DosSearchPath(ULONG flag, PCSZ pszPathOrName, PCSZ pszFilename,
                              PBYTE pBuf, ULONG cbBuf);
APIRET APIENTRY DosDevIOCtl(HFILE hDevice, ULONG category, ULONG function,
                            PVOID pParams, ULONG cbParmLenMax, PULONG pcbParmLen,
                            PVOID pData, ULONG cbDataLenMax, PULONG pcbDataLen);

/* DosSearchPath flags. */
#define SEARCH_PATH            0x0000
#define SEARCH_CUR_DIRECTORY   0x0001
#define SEARCH_ENVIRONMENT     0x0002
#define SEARCH_IGNORENETERRS   0x0004

/* DosQueryFSInfo levels. */
#define FSIL_ALLOC  1
#define FSIL_VOLSER 2

typedef struct _FSALLOCATE {
    ULONG  idFileSystem;
    ULONG  cSectorUnit;
    ULONG  cUnit;
    ULONG  cUnitAvail;
    USHORT cbSector;
} FSALLOCATE, *PFSALLOCATE;

/* Protected file handles: the calls with a lock ID only its holder knows. */
typedef ULONG FHLOCK, *PFHLOCK;
APIRET APIENTRY DosProtectOpen(PCSZ pszFileName, PHFILE phf, PULONG pulAction, ULONG cbFile,
                               ULONG ulAttribute, ULONG fsOpenFlags, ULONG fsOpenMode, PEAOP2 peaop2,
                               PFHLOCK pfhFileHandleLockID);
APIRET APIENTRY DosProtectClose(HFILE hFile, FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectRead(HFILE hFile, PVOID pBuffer, ULONG cbRead, PULONG pcbActual,
                               FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectWrite(HFILE hFile, const void *pBuffer, ULONG cbWrite, PULONG pcbActual,
                                FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectSetFilePtr(HFILE hFile, LONG ib, ULONG method, PULONG ibActual,
                                     FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectSetFileSize(HFILE hFile, ULONG cbSize, FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectQueryFHState(HFILE hFile, PULONG pMode, FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectSetFHState(HFILE hFile, ULONG mode, FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectQueryFileInfo(HFILE hf, ULONG ulInfoLevel, PVOID pInfo, ULONG cbInfoBuf,
                                        FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectSetFileInfo(HFILE hf, ULONG ulInfoLevel, PVOID pInfoBuf, ULONG cbInfoBuf,
                                      FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectSetFileLocks(HFILE hFile, PFILELOCK pflUnlock, PFILELOCK pflLock, ULONG timeout,
                                       ULONG flags, FHLOCK fhFileHandleLockID);
APIRET APIENTRY DosProtectEnumAttribute(ULONG ulRefType, PVOID pvFile, ULONG ulEntry, PVOID pvBuf,
                                        ULONG cbBuf, PULONG pulCount, ULONG ulInfoLevel,
                                        FHLOCK fhFileHandleLockID);

/* ── Memory ─────────────────────────────────────────────────────────── */
#define PAG_READ      0x00000001
#define PAG_WRITE     0x00000002
#define PAG_EXECUTE   0x00000004
#define PAG_GUARD     0x00000008
#define PAG_COMMIT    0x00000010
#define PAG_DECOMMIT  0x00000020
#define OBJ_TILE      0x00000040
#define OBJ_GETTABLE  0x00000100
#define OBJ_GIVEABLE  0x00000200
#define OBJ_ANY       0x00000400
#define PAG_DEFAULT   0x00000400
#define PAG_SHARED    0x00002000
#define PAG_FREE      0x00004000
#define PAG_BASE      0x00010000
#define fPERM         (PAG_EXECUTE | PAG_READ | PAG_WRITE)
#define fSHARE        (OBJ_GETTABLE | OBJ_GIVEABLE)

#define DOSSUB_INIT        0x01
#define DOSSUB_GROW        0x02
#define DOSSUB_SPARSE_OBJ  0x04
#define DOSSUB_SERIALIZE   0x08

APIRET APIENTRY DosAllocMem(PPVOID ppb, ULONG cb, ULONG flag);
APIRET APIENTRY DosFreeMem(PVOID pb);
APIRET APIENTRY DosSetMem(PVOID pb, ULONG cb, ULONG flag);
APIRET APIENTRY DosQueryMem(PVOID pb, PULONG pcb, PULONG pFlag);
APIRET APIENTRY DosAllocSharedMem(PPVOID ppb, PCSZ pszName, ULONG cb, ULONG flag);
APIRET APIENTRY DosGetSharedMem(PVOID pb, ULONG flag);
APIRET APIENTRY DosGetNamedSharedMem(PPVOID ppb, PCSZ pszName, ULONG flag);
APIRET APIENTRY DosGiveSharedMem(PVOID pb, PID pid, ULONG flag);
APIRET APIENTRY DosSubSetMem(PVOID pbBase, ULONG flag, ULONG cb);
APIRET APIENTRY DosSubAllocMem(PVOID pbBase, PPVOID ppb, ULONG cb);
APIRET APIENTRY DosSubFreeMem(PVOID pbBase, PVOID pb, ULONG cb);
APIRET APIENTRY DosSubUnsetMem(PVOID pbBase);
APIRET APIENTRY DosAllocThreadLocalMemory(ULONG cb, PULONG *p);
APIRET APIENTRY DosFreeThreadLocalMemory(ULONG *p);

/* ── Processes and threads ──────────────────────────────────────────── */
#define EXIT_THREAD   0
#define EXIT_PROCESS  1

#define EXEC_SYNC         0
#define EXEC_ASYNC        1
#define EXEC_ASYNCRESULT  2
#define EXEC_TRACE        3
#define EXEC_BACKGROUND   4
#define EXEC_LOAD         5

#define DCWA_PROCESS      0
#define DCWA_PROCESSTREE  1
#define DCWW_WAIT         0
#define DCWW_NOWAIT       1

#define TC_EXIT          0
#define TC_HARDERROR     1
#define TC_TRAP          2
#define TC_KILLPROCESS   3
#define TC_EXCEPTION     4

#define CREATE_READY      0
#define CREATE_SUSPENDED  1
#define STACK_SPARSE      0
#define STACK_COMMITTED   2

#define DKP_PROCESSTREE  0
#define DKP_PROCESS      1

#define PRTYS_PROCESS      0
#define PRTYS_PROCESSTREE  1
#define PRTYS_THREAD       2
#define PRTYC_NOCHANGE          0
#define PRTYC_IDLETIME          1
#define PRTYC_REGULAR           2
#define PRTYC_TIMECRITICAL      3
#define PRTYC_FOREGROUNDSERVER  4

#define EXLST_ADD     1
#define EXLST_REMOVE  2
#define EXLST_EXIT    3

typedef struct _RESULTCODES {
    ULONG codeTerminate;
    ULONG codeResult;
} RESULTCODES, *PRESULTCODES;

typedef struct _TIB2 {
    ULONG  tib2_ultid;
    ULONG  tib2_ulpri;
    ULONG  tib2_version;
    USHORT tib2_usMCCount;
    USHORT tib2_fMCForceFlag;
} TIB2, *PTIB2;

typedef struct _TIB {
    PVOID tib_pexchain;
    PVOID tib_pstack;
    PVOID tib_pstacklimit;
    PTIB2 tib_ptib2;
    ULONG tib_version;
    ULONG tib_ordinal;
} TIB, *PTIB;

typedef struct _PIB {
    ULONG pib_ulpid;
    ULONG pib_ulppid;
    ULONG pib_hmte;
    PCHAR pib_pchcmd;
    PCHAR pib_pchenv;
    ULONG pib_flstatus;
    ULONG pib_ultype;
} PIB, *PPIB;

typedef void (APIENTRY *PFNTHREAD)(ULONG);
typedef void (APIENTRY *PFNEXITLIST)(ULONG);

void   APIENTRY DosExit(ULONG action, ULONG result) __attribute__((__noreturn__));
APIRET APIENTRY DosCreateThread(PTID ptid, PFNTHREAD pfn, ULONG param, ULONG flag, ULONG cbStack);
APIRET APIENTRY DosKillThread(TID tid);
APIRET APIENTRY DosWaitThread(PTID ptid, ULONG option);
APIRET APIENTRY DosSuspendThread(TID tid);
APIRET APIENTRY DosResumeThread(TID tid);
APIRET APIENTRY DosSleep(ULONG msec);
APIRET APIENTRY DosExecPgm(PCHAR pObjname, LONG cbObjname, ULONG execFlag, PCSZ pArg,
                           PCSZ pEnv, PRESULTCODES pRes, PCSZ pName);
APIRET APIENTRY DosWaitChild(ULONG action, ULONG option, PRESULTCODES pres, PPID ppid, PID pid);
APIRET APIENTRY DosKillProcess(ULONG action, PID pid);
APIRET APIENTRY DosVerifyPidTid(PID pid, TID tid);
APIRET APIENTRY DosGetInfoBlocks(PTIB *pptib, PPIB *pppib);
APIRET APIENTRY DosSetPriority(ULONG scope, ULONG ulClass, LONG delta, ULONG PorTid);
APIRET APIENTRY DosEnterCritSec(void);
APIRET APIENTRY DosExitCritSec(void);
APIRET APIENTRY DosEnterMustComplete(PULONG pulNesting);
APIRET APIENTRY DosExitMustComplete(PULONG pulNesting);
APIRET APIENTRY DosExitList(ULONG ordercode, PFNEXITLIST pfn);
APIRET APIENTRY DosQueryAppType(PCSZ pszName, PULONG pFlags);
APIRET APIENTRY DosScanEnv(PCSZ pszName, PCSZ *ppszValue);
APIRET APIENTRY DosShutdown(ULONG ulReserved);

/* ── Modules and resources ──────────────────────────────────────────── */
#define PT_16BIT  0
#define PT_32BIT  1

APIRET APIENTRY DosLoadModule(PSZ pszName, ULONG cbName, PCSZ pszModname, PHMODULE phmod);
APIRET APIENTRY DosFreeModule(HMODULE hmod);
APIRET APIENTRY DosQueryProcAddr(HMODULE hmod, ULONG ordinal, PCSZ pszName, PFN *ppfn);
APIRET APIENTRY DosQueryProcType(HMODULE hmod, ULONG ordinal, PCSZ pszName, PULONG pulproctype);
APIRET APIENTRY DosQueryModuleHandle(PCSZ pszModname, PHMODULE phmod);
APIRET APIENTRY DosQueryModuleName(HMODULE hmod, ULONG cbName, PCHAR pch);
APIRET APIENTRY DosQueryModFromEIP(HMODULE *phMod, ULONG *pObjNum, ULONG BuffLen,
                                   PCHAR pBuff, ULONG *pOffset, ULONG Address);
/* Resource types. */
#define RT_POINTER     1
#define RT_BITMAP      2
#define RT_MENU        3
#define RT_DIALOG      4
#define RT_STRING      5
#define RT_FONTDIR     6
#define RT_FONT        7
#define RT_ACCELTABLE  8
#define RT_RCDATA      9
#define RT_MESSAGE     10
#define RT_DLGINCLUDE  11

APIRET APIENTRY DosGetResource(HMODULE hmod, ULONG idType, ULONG idName, PPVOID ppb);
APIRET APIENTRY DosFreeResource(PVOID pb);
APIRET APIENTRY DosQueryResourceSize(HMODULE hmod, ULONG idt, ULONG idn, PULONG pulsize);

#define BEGIN_LIBPATH  1
#define END_LIBPATH    2
APIRET APIENTRY DosQueryExtLIBPATH(PSZ pszExtLIBPATH, ULONG flags);
APIRET APIENTRY DosSetExtLIBPATH(PCSZ pszExtLIBPATH, ULONG flags);

/* ── Semaphores ─────────────────────────────────────────────────────── */
#define DC_SEM_SHARED          0x01
#define DCMW_WAIT_ANY          0x02
#define DCMW_WAIT_ALL          0x04
#define SEM_INDEFINITE_WAIT    ((ULONG)-1)
#define SEM_IMMEDIATE_RETURN   0UL

typedef struct _SEMRECORD {
    HSEM  hsemCur;
    ULONG ulUser;
} SEMRECORD, *PSEMRECORD;

APIRET APIENTRY DosCreateEventSem(PCSZ pszName, PHEV phev, ULONG flAttr, BOOL32 fState);
APIRET APIENTRY DosOpenEventSem(PCSZ pszName, PHEV phev);
APIRET APIENTRY DosCloseEventSem(HEV hev);
APIRET APIENTRY DosResetEventSem(HEV hev, PULONG pulPostCt);
APIRET APIENTRY DosPostEventSem(HEV hev);
APIRET APIENTRY DosWaitEventSem(HEV hev, ULONG ulTimeout);
APIRET APIENTRY DosQueryEventSem(HEV hev, PULONG pulPostCt);

APIRET APIENTRY DosCreateMutexSem(PCSZ pszName, PHMTX phmtx, ULONG flAttr, BOOL32 fState);
APIRET APIENTRY DosOpenMutexSem(PCSZ pszName, PHMTX phmtx);
APIRET APIENTRY DosCloseMutexSem(HMTX hmtx);
APIRET APIENTRY DosRequestMutexSem(HMTX hmtx, ULONG ulTimeout);
APIRET APIENTRY DosReleaseMutexSem(HMTX hmtx);
APIRET APIENTRY DosQueryMutexSem(HMTX hmtx, PPID ppid, PTID ptid, PULONG pulCount);

APIRET APIENTRY DosCreateMuxWaitSem(PCSZ pszName, PHMUX phmux, ULONG cSemRec,
                                    PSEMRECORD pSemRec, ULONG flAttr);
APIRET APIENTRY DosOpenMuxWaitSem(PCSZ pszName, PHMUX phmux);
APIRET APIENTRY DosCloseMuxWaitSem(HMUX hmux);
APIRET APIENTRY DosWaitMuxWaitSem(HMUX hmux, ULONG ulTimeout, PULONG pulUser);
APIRET APIENTRY DosAddMuxWaitSem(HMUX hmux, PSEMRECORD pSemRec);
APIRET APIENTRY DosDeleteMuxWaitSem(HMUX hmux, HSEM hSem);
APIRET APIENTRY DosQueryMuxWaitSem(HMUX hmux, PULONG pcSemRec, PSEMRECORD pSemRec,
                                   PULONG pflAttr);

/* ── Date, time and timers ──────────────────────────────────────────── */
APIRET APIENTRY DosGetDateTime(PDATETIME pdt);
APIRET APIENTRY DosSetDateTime(PDATETIME pdt);
APIRET APIENTRY DosAsyncTimer(ULONG msec, HSEM hsem, PHTIMER phtimer);
APIRET APIENTRY DosStartTimer(ULONG msec, HSEM hsem, PHTIMER phtimer);
APIRET APIENTRY DosStopTimer(HTIMER htimer);
APIRET APIENTRY DosTmrQueryFreq(PULONG pulTmrFreq);
APIRET APIENTRY DosTmrQueryTime(PQWORD pqwTmrTime);

/* ── System information ─────────────────────────────────────────────── */
#define QSV_MAX_PATH_LENGTH        1
#define QSV_MAX_TEXT_SESSIONS      2
#define QSV_MAX_PM_SESSIONS        3
#define QSV_MAX_VDM_SESSIONS       4
#define QSV_BOOT_DRIVE             5
#define QSV_DYN_PRI_VARIATION      6
#define QSV_MAX_WAIT               7
#define QSV_MIN_SLICE              8
#define QSV_MAX_SLICE              9
#define QSV_PAGE_SIZE             10
#define QSV_VERSION_MAJOR         11
#define QSV_VERSION_MINOR         12
#define QSV_VERSION_REVISION      13
#define QSV_MS_COUNT              14
#define QSV_TIME_LOW              15
#define QSV_TIME_HIGH             16
#define QSV_TOTPHYSMEM            17
#define QSV_TOTRESMEM             18
#define QSV_TOTAVAILMEM           19
#define QSV_MAXPRMEM              20
#define QSV_MAXSHMEM              21
#define QSV_TIMER_INTERVAL        22
#define QSV_MAX_COMP_LENGTH       23

APIRET APIENTRY DosQuerySysInfo(ULONG iStart, ULONG iLast, PVOID pBuf, ULONG cbBuf);
APIRET APIENTRY DosDevConfig(PVOID pdevinfo, ULONG item);
APIRET APIENTRY DosBeep(ULONG freq, ULONG dur);
APIRET APIENTRY DosQueryCp(ULONG cb, PULONG arCP, PULONG pcCP);
APIRET APIENTRY DosSetProcessCp(ULONG cp);

/* ── Errors ─────────────────────────────────────────────────────────── */
#define FERR_DISABLEHARDERR    0x0000
#define FERR_ENABLEHARDERR     0x0001
#define FERR_ENABLEEXCEPTION   0x0000
#define FERR_DISABLEEXCEPTION  0x0002

APIRET APIENTRY DosError(ULONG error);
APIRET APIENTRY DosErrClass(ULONG code, PULONG pClass, PULONG pAction, PULONG pLocus);

/* ── Exceptions and signals ─────────────────────────────────────────── */
#define XCPT_CONTINUE_SEARCH       0x00000000UL
#define XCPT_CONTINUE_EXECUTION    0xFFFFFFFFUL
#define XCPT_CHAIN_END             ((PEXCEPTIONREGISTRATIONRECORD)0xFFFFFFFFUL)

#define EH_NONCONTINUABLE  0x01
#define EH_UNWINDING       0x02
#define EH_EXIT_UNWIND     0x04
#define EH_NESTED_CALL     0x10

#define XCPT_GUARD_PAGE_VIOLATION      0x80000001UL
#define XCPT_UNABLE_TO_GROW_STACK      0x80010001UL
#define XCPT_ACCESS_VIOLATION          0xC0000005UL
#define XCPT_IN_PAGE_ERROR             0xC0000006UL
#define XCPT_ILLEGAL_INSTRUCTION       0xC000001CUL
#define XCPT_INVALID_LOCK_SEQUENCE     0xC000001DUL
#define XCPT_FLOAT_DENORMAL_OPERAND    0xC0000094UL
#define XCPT_FLOAT_DIVIDE_BY_ZERO      0xC0000095UL
#define XCPT_FLOAT_INEXACT_RESULT      0xC0000096UL
#define XCPT_FLOAT_INVALID_OPERATION   0xC0000097UL
#define XCPT_FLOAT_OVERFLOW            0xC0000098UL
#define XCPT_FLOAT_STACK_CHECK         0xC0000099UL
#define XCPT_FLOAT_UNDERFLOW           0xC000009AUL
#define XCPT_INTEGER_DIVIDE_BY_ZERO    0xC000009BUL
#define XCPT_INTEGER_OVERFLOW          0xC000009CUL
#define XCPT_PRIVILEGED_INSTRUCTION    0xC000009DUL
#define XCPT_DATATYPE_MISALIGNMENT     0xC000009EUL
#define XCPT_BREAKPOINT                0xC000009FUL
#define XCPT_SINGLE_STEP               0xC00000A0UL
#define XCPT_SIGNAL                    0xC0010003UL
#define XCPT_SIGNAL_INTR      1
#define XCPT_SIGNAL_KILLPROC  3
#define XCPT_SIGNAL_BREAK     4

#define EXCEPTION_MAXIMUM_PARAMETERS 4

typedef struct _EXCEPTIONREPORTRECORD {
    ULONG  ExceptionNum;
    ULONG  fHandlerFlags;
    struct _EXCEPTIONREPORTRECORD *NestedExceptionReportRecord;
    PVOID  ExceptionAddress;
    ULONG  cParameters;
    ULONG  ExceptionInfo[EXCEPTION_MAXIMUM_PARAMETERS];
} EXCEPTIONREPORTRECORD, *PEXCEPTIONREPORTRECORD;

#define CONTEXT_CONTROL   0x00000001UL
#define CONTEXT_INTEGER   0x00000002UL
#define CONTEXT_SEGMENTS  0x00000004UL
#define CONTEXT_FLOATING_POINT 0x00000008UL
#define CONTEXT_FULL (CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_SEGMENTS)

/* The processor state at an exception. The floating-point save area is
 * kept as bytes here (Warpine does not fill it yet). */
typedef struct _CONTEXT {
    ULONG ContextFlags;
    BYTE  ctx_fpu[0x88];
    ULONG ctx_SegGs;
    ULONG ctx_SegFs;
    ULONG ctx_SegEs;
    ULONG ctx_SegDs;
    ULONG ctx_RegEdi;
    ULONG ctx_RegEsi;
    ULONG ctx_RegEax;
    ULONG ctx_RegEbx;
    ULONG ctx_RegEcx;
    ULONG ctx_RegEdx;
    ULONG ctx_RegEbp;
    ULONG ctx_RegEip;
    ULONG ctx_SegCs;
    ULONG ctx_EFlags;
    ULONG ctx_RegEsp;
    ULONG ctx_SegSs;
} CONTEXTRECORD, *PCONTEXTRECORD;

struct _EXCEPTIONREGISTRATIONRECORD;
typedef ULONG APIENTRY ERR(PEXCEPTIONREPORTRECORD, struct _EXCEPTIONREGISTRATIONRECORD *,
                           PCONTEXTRECORD, PVOID);
typedef ERR *PERR;

/* Must live on the stack of the thread that registers it. */
typedef struct _EXCEPTIONREGISTRATIONRECORD {
    struct _EXCEPTIONREGISTRATIONRECORD *prev_structure;
    PERR ExceptionHandler;
} EXCEPTIONREGISTRATIONRECORD, *PEXCEPTIONREGISTRATIONRECORD;

APIRET APIENTRY DosSetExceptionHandler(PEXCEPTIONREGISTRATIONRECORD pERegRec);
APIRET APIENTRY DosUnsetExceptionHandler(PEXCEPTIONREGISTRATIONRECORD pERegRec);
APIRET APIENTRY DosRaiseException(PEXCEPTIONREPORTRECORD pexcept);
APIRET APIENTRY DosUnwindException(PEXCEPTIONREGISTRATIONRECORD phandler, PVOID pTargetIP,
                                   PEXCEPTIONREPORTRECORD pERepRec);
APIRET APIENTRY DosSetSignalExceptionFocus(BOOL32 flag, PULONG pulTimes);
APIRET APIENTRY DosSendSignalException(PID pid, ULONG exception);
APIRET APIENTRY DosAcknowledgeSignalException(ULONG ulSignalNum);

/* ── Named pipes ────────────────────────────────────────────────────── */
#define NP_ACCESS_INBOUND       0x0000
#define NP_ACCESS_OUTBOUND      0x0001
#define NP_ACCESS_DUPLEX        0x0002
#define NP_INHERIT              0x0000
#define NP_NOINHERIT            0x0080
#define NP_WRITEBEHIND          0x0000
#define NP_NOWRITEBEHIND        0x4000
#define NP_READMODE_BYTE        0x0000
#define NP_READMODE_MESSAGE     0x0100
#define NP_TYPE_BYTE            0x0000
#define NP_TYPE_MESSAGE         0x0400
#define NP_END_CLIENT           0x0000
#define NP_END_SERVER           0x4000
#define NP_WAIT                 0x0000
#define NP_NOWAIT               0x8000
#define NP_UNLIMITED_INSTANCES  0x00FF
#define NP_DEFAULT_WAIT         0UL
#define NP_INDEFINITE_WAIT      ((ULONG)-1)

#define NP_STATE_DISCONNECTED  0x0001
#define NP_STATE_LISTENING     0x0002
#define NP_STATE_CONNECTED     0x0003
#define NP_STATE_CLOSING       0x0004

typedef struct _AVAILDATA {
    USHORT cbpipe;
    USHORT cbmessage;
} AVAILDATA, *PAVAILDATA;

typedef struct _PIPESEMSTATE {
    BYTE   fStatus;
    BYTE   fFlag;
    USHORT usKey;
    USHORT usAvail;
} PIPESEMSTATE, *PPIPESEMSTATE;

APIRET APIENTRY DosCreateNPipe(PCSZ pszName, PHPIPE pHpipe, ULONG openmode, ULONG pipemode,
                               ULONG cbOutbuf, ULONG cbInbuf, ULONG msec);
APIRET APIENTRY DosConnectNPipe(HPIPE hpipe);
APIRET APIENTRY DosDisConnectNPipe(HPIPE hpipe);
APIRET APIENTRY DosCallNPipe(PCSZ pszName, PVOID pInbuf, ULONG cbIn, PVOID pOutbuf,
                             ULONG cbOut, PULONG pcbActual, ULONG msec);
APIRET APIENTRY DosTransactNPipe(HPIPE hpipe, PVOID pOutbuf, ULONG cbOut, PVOID pInbuf,
                                 ULONG cbIn, PULONG pcbRead);
APIRET APIENTRY DosPeekNPipe(HPIPE hpipe, PVOID pBuf, ULONG cbBuf, PULONG pcbActual,
                             PAVAILDATA pAvail, PULONG pState);
APIRET APIENTRY DosQueryNPHState(HPIPE hpipe, PULONG pState);
APIRET APIENTRY DosSetNPHState(HPIPE hpipe, ULONG state);
APIRET APIENTRY DosWaitNPipe(PCSZ pszName, ULONG ulTimeout);
APIRET APIENTRY DosQueryNPipeInfo(HPIPE hpipe, ULONG infolevel, PVOID pBuf, ULONG size);
APIRET APIENTRY DosSetNPipeSem(HPIPE hpipe, HSEM hsem, ULONG key);
APIRET APIENTRY DosQueryNPipeSemState(HSEM hsem, PPIPESEMSTATE pnpss, ULONG cbBuf);

/* ── Queues (QUECALLS) ──────────────────────────────────────────────── */
#define QUE_FIFO               0x0000
#define QUE_LIFO               0x0001
#define QUE_PRIORITY           0x0002
#define QUE_NOCONVERT_ADDRESS  0x0000
#define QUE_CONVERT_ADDRESS    0x0004

typedef struct _REQUESTDATA {
    PID   pid;
    ULONG ulData;
} REQUESTDATA, *PREQUESTDATA;

APIRET APIENTRY DosCreateQueue(PHQUEUE phq, ULONG priority, PCSZ pszName);
APIRET APIENTRY DosOpenQueue(PPID ppid, PHQUEUE phq, PCSZ pszName);
APIRET APIENTRY DosCloseQueue(HQUEUE hq);
APIRET APIENTRY DosWriteQueue(HQUEUE hq, ULONG request, ULONG cbData, PVOID pbData,
                              ULONG priority);
APIRET APIENTRY DosReadQueue(HQUEUE hq, PREQUESTDATA pRequest, PULONG pcbData, PPVOID ppbuf,
                             ULONG element, BOOL32 wait, PBYTE ppriority, HEV hsem);
APIRET APIENTRY DosPeekQueue(HQUEUE hq, PREQUESTDATA pRequest, PULONG pcbData, PPVOID ppbuf,
                             PULONG element, BOOL32 nowait, PBYTE ppriority, HEV hsem);
APIRET APIENTRY DosPurgeQueue(HQUEUE hq);
APIRET APIENTRY DosQueryQueue(HQUEUE hq, PULONG pcbEntries);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_OS2_DOS_H */
