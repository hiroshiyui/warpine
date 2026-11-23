// SPDX-License-Identifier: GPL-3.0-or-later
//
// Structured API trace helpers.
//
// Provides lookup functions used by the tracing spans in `api_dispatch`:
//   - `ordinal_to_name(ordinal)` → human-readable OS/2 API name (or "?")
//   - `module_for_ordinal(ordinal)` → owning DLL name ("DOSCALLS", "PMWIN", …)
//   - `arg_names_for_ordinal(ordinal)` → parameter name list for strace output
//   - `format_call(name, ordinal, args, read_str)` → strace-style call string
//
// The pure data functions (ordinal_to_name, module_for_ordinal,
// arg_names_for_ordinal) have no I/O or global state.  format_call accepts
// a caller-supplied closure for string dereferencing so that this module
// remains free of any dependency on Loader.

use super::constants::*;

// ── Ordinal → API name  (AUTO-GENERATED from targets/os2api.def) ──────────────
//
// Regenerate with: cargo run --bin gen_api -- gen-trace
// Do not edit the two functions below manually; edit os2api.def instead.

/// Map a warpine flat ordinal to its OS/2 API name.
///
/// Returns `"?"` for unknown ordinals.
pub fn ordinal_to_name(ordinal: u32) -> &'static str {
    match ordinal {
        // ── DOSCALLS ──────────────────────────────────────────────
        8 => "DosGetInfoSeg",
        75 => "DosQueryFileMode16",
        84 => "DosSetFileMode",
        110 => "DosForceDelete",
        111 => "DosKillThread",
        209 => "DosSetMaxFH",
        210 => "DosSetVerify",
        211 => "DosErrClass",
        212 => "DosError",
        218 => "DosSetFileInfo",
        219 => "DosSetPathInfo",
        220 => "DosSetDefaultDisk",
        221 => "DosSetFHState",
        223 => "DosQueryPathInfo",
        224 => "DosQueryHType",
        225 => "DosQueryVerify",
        226 => "DosDeleteDir",
        227 => "DosScanEnv",
        228 => "DosSearchPath",
        229 => "DosSleep",
        230 => "DosGetDateTime",
        231 => "DosDevConfig",
        232 => "DosEnterCritSec",
        233 => "DosExitCritSec",
        234 => "DosExit",
        235 => "DosKillProcess",
        236 => "DosSetPriority",
        237 => "DosResumeThread",
        238 => "DosSuspendThread",
        239 => "DosCreatePipe",
        240 => "DosCallNPipe",
        241 => "DosConnectNPipe",
        242 => "DosDisConnectNPipe",
        243 => "DosCreateNPipe",
        244 => "DosPeekNPipe",
        245 => "DosQueryNPHState",
        246 => "DosRawReadNPipe",
        247 => "DosRawWriteNPipe",
        248 => "DosQueryNPipeInfo",
        249 => "DosQueryNPipeSemState",
        250 => "DosSetNPHState",
        251 => "DosSetNPipeSem",
        252 => "DosTransactNPipe",
        253 => "DosWaitNPipe",
        254 => "DosResetBuffer",
        255 => "DosSetCurrentDir",
        256 => "DosSetFilePtr",
        257 => "DosClose",
        258 => "DosCopy",
        259 => "DosDelete",
        260 => "DosDupHandle",
        261 => "DosEditName",
        263 => "DosFindClose",
        264 => "DosFindFirst",
        265 => "DosFindNext",
        267 => "DOS16REQUESTVDD",
        270 => "DosCreateDir",
        271 => "DosMove",
        272 => "DosSetFileSize",
        273 => "DosOpen",
        274 => "DosQueryCurrentDir",
        275 => "DosQueryCurrentDisk",
        276 => "DosQueryFHState",
        277 => "DosQueryFSAttach",
        278 => "DosQueryFSInfo",
        279 => "DosQueryFileInfo",
        280 => "DosWaitChild",
        281 => "DosRead",
        282 => "DosWrite",
        283 => "DosExecPgm",
        284 => "DosDevIOCtl",
        285 => "DosFSCtl",
        286 => "DosBeep",
        289 => "DosSetProcessCp",
        290 => "DosStopTimer",
        291 => "DosQueryCp",
        292 => "DosSetDateTime",
        296 => "DosExitList",
        299 => "DosAllocMem",
        300 => "DosAllocSharedMem",
        301 => "DosGetNamedSharedMem",
        302 => "DosGetSharedMem",
        303 => "DosGiveSharedMem",
        304 => "DosFreeMem",
        305 => "DosSetMem",
        306 => "DosQueryMem",
        311 => "DosCreateThread",
        312 => "DosGetInfoBlocks",
        317 => "DosDebug",
        318 => "DosLoadModule",
        319 => "DosQueryModuleHandle",
        320 => "DosQueryModuleName",
        321 => "DosQueryProcAddr",
        322 => "DosFreeModule",
        323 => "DosQueryAppType",
        324 => "DosCreateEventSem",
        325 => "DosOpenEventSem",
        326 => "DosCloseEventSem",
        327 => "DosResetEventSem",
        328 => "DosPostEventSem",
        329 => "DosWaitEventSem",
        330 => "DosQueryEventSem",
        331 => "DosCreateMutexSem",
        332 => "DosOpenMutexSem",
        333 => "DosCloseMutexSem",
        334 => "DosRequestMutexSem",
        335 => "DosReleaseMutexSem",
        336 => "DosQueryMutexSem",
        337 => "DosCreateMuxWaitSem",
        338 => "DosOpenMuxWaitSem",
        339 => "DosCloseMuxWaitSem",
        340 => "DosWaitMuxWaitSem",
        341 => "DosAddMuxWaitSem",
        342 => "DosDeleteMuxWaitSem",
        343 => "DosQueryMuxWaitSem",
        344 => "DosSubSetMem",
        345 => "DosSubAllocMem",
        346 => "DosSubFreeMem",
        347 => "DosSubUnsetMem",
        348 => "DosQuerySysInfo",
        349 => "DosWaitThread",
        350 => "DosAsyncTimer",
        351 => "DosStartTimer",
        352 => "DosGetResource",
        353 => "DosFreeResource",
        354 => "DosSetExceptionHandler",
        355 => "DosUnsetExceptionHandler",
        356 => "DosRaiseException",
        357 => "DosUnwindException",
        360 => "DosQueryModFromEIP",
        362 => "DosTmrQueryFreq",
        363 => "DosTmrQueryTime",
        368 => "DosQuerySysState",
        372 => "DosEnumAttribute",
        378 => "DosSetSignalExceptionFocus",
        379 => "DosSendSignalException",
        380 => "DosEnterMustComplete",
        381 => "DosExitMustComplete",
        382 => "DosSetRelMaxFH",
        415 => "DosShutdown",
        418 => "DosAcknowledgeSignalException",
        425 => "DosFlatToSel",
        426 => "DosSelToFlat",
        428 => "DosSetFileLocks",
        429 => "DosCancelLockRequest",
        454 => "DosAllocThreadLocalMemory",
        455 => "DosFreeThreadLocalMemory",
        460 => "DosVerifyPidTid",
        572 => "DosQueryResourceSize",
        586 => "DosQueryProcType",
        621 => "DosProtectSetFilePtr",
        636 => "DosProtectEnumAttribute",
        637 => "DosProtectOpen",
        638 => "DosProtectClose",
        639 => "DosProtectSetFileLocks",
        640 => "DosProtectSetFileSize",
        641 => "DosProtectRead",
        642 => "DosProtectWrite",
        643 => "DosProtectSetFileInfo",
        644 => "DosProtectSetFHState",
        645 => "DosProtectQueryFHState",
        646 => "DosProtectQueryFileInfo",
        873 => "DosSetExtLIBPATH",
        874 => "DosQueryExtLIBPATH",

        // ── QUECALLS ──────────────────────────────────────────────
        o if (1024..PMWIN_BASE).contains(&o) => match o - 1024 {
            9  => "DosReadQueue",
            10 => "DosPurgeQueue",
            11 => "DosCloseQueue",
            12 => "DosQueryQueue",
            13 => "DosPeekQueue",
            14 => "DosWriteQueue",
            15 => "DosOpenQueue",
            16 => "DosCreateQueue",
            _  => "?",
        },

        // ── KBDCALLS ──────────────────────────────────────────────
        o if (KBDCALLS_BASE..VIOCALLS_BASE).contains(&o) => match o - KBDCALLS_BASE {
            4  => "KbdCharIn",
            9  => "KbdStringIn",
            10 => "KbdGetStatus",
            _  => "?",
        },

        // ── VIOCALLS ──────────────────────────────────────────────
        o if (VIOCALLS_BASE..SESMGR_BASE).contains(&o) => match o - VIOCALLS_BASE {
            3  => "VioGetAnsi",
            5  => "VioSetAnsi",
            7  => "VioScrollUp",
            9  => "VioGetCurPos",
            10 => "VioWrtCellStr",
            15 => "VioSetCurPos",
            19 => "VioWrtTTY",
            21 => "VioGetMode",
            22 => "VioSetMode",
            24 => "VioReadCellStr",
            26 => "VioWrtNAttr",
            27 => "VioGetCurType",
            31 => "VioGetBuf",
            32 => "VioSetCurType",
            47 => "VioScrollDn",
            75 => "VioCheckCharType",
            42 => "VioSetCp",
            43 => "VioShowBuf",
            46 => "VioGetConfig",
            48 => "VioWrtCharStrAtt",
            51 => "VioSetState",
            52 => "VioWrtNCell",
            _  => "?",
        },

        // ── NLS ───────────────────────────────────────────────────
        o if (NLS_BASE..MSG_BASE).contains(&o) => match o - NLS_BASE {
            5 => "DosQueryCtryInfo",
            6 => "DosQueryDBCSEnv",
            7 => "DosMapCase",
            8 => "DosQueryCollate",
            _ => "?",
        },

        // ── MSG ───────────────────────────────────────────────────
        o if (MSG_BASE..MDM_BASE).contains(&o) => match o - MSG_BASE {
            4 => "DosInsertMessage",
            5 => "DosPutMessage",
            6 => "DosGetMessage",
            8 => "DosQueryMessageCP",
            _ => "?",
        },

        // ── MDM / MMPM/2 ──────────────────────────────────────────
        o if (MDM_BASE..UCONV_BASE).contains(&o) => match o - MDM_BASE {
            1 => "mciSendCommand",
            2 => "mciSendString",
            3 => "mciFreeBlock",
            4 => "mciGetLastError",
            _ => "?",
        },

        // ── UCONV ─────────────────────────────────────────────────
        o if (UCONV_BASE..SO32DLL_BASE).contains(&o) => match o - UCONV_BASE {
            1 => "UniCreateUconvObject",
            2 => "UniFreeUconvObject",
            3 => "UniUconvToUcs",
            4 => "UniUconvFromUcs",
            6 => "UniMapCpToUcsCp",
            _ => "?",
        },

        // ── SO32DLL ────────────────────────────────────────────────
        o if (SO32DLL_BASE..TCP32DLL_BASE).contains(&o) => match o - SO32DLL_BASE {
            1  => "accept",
            2  => "bind",
            3  => "connect",
            4  => "gethostid",
            5  => "gethostname",
            6  => "getpeername",
            7  => "getsockname",
            8  => "getsockopt",
            10 => "listen",
            11 => "recv",
            12 => "recvfrom",
            13 => "select",
            14 => "send",
            15 => "sendto",
            16 => "setsockopt",
            17 => "shutdown",
            18 => "sock_errno",
            19 => "socket",
            20 => "soclose",
            21 => "psock_errno",
            22 => "sock_init",
            40 => "gethostbyname",
            41 => "gethostbyaddr",
            42 => "getservbyname",
            43 => "getservbyport",
            44 => "getprotobyname",
            45 => "getprotobynumber",
            _ => "?",
        },

        // ── TCP32DLL ───────────────────────────────────────────────
        o if (TCP32DLL_BASE..MOUCALLS_BASE).contains(&o) => match o - TCP32DLL_BASE {
            1  => "accept",
            2  => "bind",
            3  => "connect",
            6  => "getpeername",
            7  => "getsockname",
            8  => "getsockopt",
            10 => "listen",
            11 => "recv",
            12 => "recvfrom",
            13 => "select",
            14 => "send",
            15 => "sendto",
            16 => "setsockopt",
            17 => "shutdown",
            18 => "sock_errno",
            19 => "socket",
            20 => "soclose",
            22 => "sock_init",
            40 => "gethostbyname",
            41 => "gethostbyaddr",
            42 => "getservbyname",
            43 => "getservbyport",
            44 => "getprotobyname",
            45 => "getprotobynumber",
            _ => "?",
        },

        // ── MOUCALLS ───────────────────────────────────────────────
        o if (MOUCALLS_BASE..PMSHAPI_BASE).contains(&o) => match o - MOUCALLS_BASE {
            1  => "MouGetPtrShape",
            2  => "MouSetPtrShape",
            3  => "MouGetNumMickeys",
            4  => "MouGetHotKey",
            6  => "MouGetScaleFact",
            7  => "MouFlushQue",
            8  => "MouGetNumButtons",
            9  => "MouClose",
            10 => "MouSetHotKey",
            11 => "MouSetScaleFact",
            13 => "MouGetNumQueEl",
            14 => "MouDeRegister",
            15 => "MouGetEventMask",
            16 => "MouSetEventMask",
            17 => "MouOpen",
            18 => "MouRemovePtr",
            19 => "MouGetPtrPos",
            20 => "MouReadEventQue",
            21 => "MouSetPtrPos",
            22 => "MouGetDevStatus",
            23 => "MouSynch",
            24 => "MouRegister",
            25 => "MouSetDevStatus",
            26 => "MouDrawPtr",
            27 => "MouInitReal",
            29 => "MouGetThreshold",
            30 => "MouSetThreshold",
            _  => "?",
        },

        // ── PMSHAPI (profiles) ─────────────────────────────────────
        o if (PMSHAPI_BASE..HELPMGR_BASE).contains(&o) => match o - PMSHAPI_BASE {
            101 => "PrfQueryProfileSize",
            102 => "PrfOpenProfile",
            103 => "PrfCloseProfile",
            114 => "PrfQueryProfileInt",
            115 => "PrfQueryProfileString",
            116 => "PrfWriteProfileString",
            117 => "PrfQueryProfileData",
            118 => "PrfWriteProfileData",
            119 => "WinStartApp",
            120 => "WinAddSwitchEntry",
            121 => "WinCreateSwitchEntry",
            123 => "WinChangeSwitchEntry",
            124 => "WinQuerySwitchEntry",
            125 => "WinQuerySwitchHandle",
            126 => "WinQuerySwitchList",
            129 => "WinRemoveSwitchEntry",
            130 => "WinTerminateApp",
            131 => "WinSwitchToProgram",
            _   => "?",
        },

        // ── HELPMGR ────────────────────────────────────────────────
        o if (HELPMGR_BASE..PMSPL_BASE).contains(&o) => match o - HELPMGR_BASE {
            51 => "WinCreateHelpInstance",
            52 => "WinDestroyHelpInstance",
            53 => "WinQueryHelpInstance",
            54 => "WinAssociateHelpInstance",
            55 => "WinLoadHelpTable",
            56 => "WinCreateHelpTable",
            _  => "?",
        },

        // ── PMSPL (spooler) ────────────────────────────────────────
        o if (PMSPL_BASE..PMCTLS_BASE).contains(&o) => match o - PMSPL_BASE {
            301 => "SplQmOpen",
            302 => "SplQmStartDoc",
            303 => "SplQmEndDoc",
            304 => "SplQmWrite",
            305 => "SplQmAbort",
            306 => "SplQmClose",
            308 => "SplQmAbortDoc",
            381 => "SplQueryDevice",
            382 => "SplEnumDevice",
            397 => "SplQueryQueue",
            399 => "SplEnumQueue",
            406 => "SplEnumDriver",
            407 => "SplEnumQueueProcessor",
            408 => "SplEnumPort",
            441 => "SplEnumPrinter",
            _   => "?",
        },

        // ── PMCTLS (standard dialogs) ──────────────────────────────
        o if (PMCTLS_BASE..WARPINE_BASE).contains(&o) => match o - PMCTLS_BASE {
            2 => "WinFontDlg",
            3 => "WinDefFontDlgProc",
            4 => "WinFileDlg",
            5 => "WinDefFileDlgProc",
            6 => "WinFreeFileDlgList",
            24 => "WinRegisterCircularSlider",
            _ => "?",
        },

        // ── WARPINE (Warpine's own extensions) ─────────────────────
        o if (WARPINE_BASE..PMDRAG_BASE).contains(&o) => match o - WARPINE_BASE {
            1 => "WrpTime64",
            2 => "WrpGetDateTime64",
            3 => "WrpLocalTime64",
            4 => "WrpMakeTime64",
            5 => "WrpBuiltinWindowProc",
            7 => "WrpRegisterUnwindTables",
            8 => "WrpDeregisterUnwindTables",
            9 => "WrpQueryUnwindTables",
            10 => "WrpQueryRuntimeBlock",
            _ => "?",
        },

        // ── PMDRAG (drag and drop; pm_drag.rs) ─────────────────────────
        o if (PMDRAG_BASE..SOM_BASE).contains(&o) => match o - PMDRAG_BASE {
            32 => "DrgAccessDraginfo",
            33 => "DrgAddStrHandle",
            34 => "DrgAllocDraginfo",
            35 => "DrgAllocDragtransfer",
            36 => "DrgDeleteDraginfoStrHandles",
            37 => "DrgDeleteStrHandle",
            38 => "DrgDrag",
            39 => "DrgFreeDraginfo",
            40 => "DrgFreeDragtransfer",
            41 => "DrgGetPS",
            42 => "DrgPostTransferMsg",
            43 => "DrgPushDraginfo",
            44 => "DrgQueryDragitem",
            45 => "DrgQueryDragitemCount",
            46 => "DrgQueryDragitemPtr",
            47 => "DrgQueryNativeRMF",
            48 => "DrgQueryNativeRMFLen",
            49 => "DrgQueryStrName",
            50 => "DrgQueryStrNameLen",
            51 => "DrgQueryTrueType",
            52 => "DrgQueryTrueTypeLen",
            53 => "DrgReleasePS",
            54 => "DrgSendTransferMsg",
            55 => "DrgSetDragPointer",
            56 => "DrgSetDragImage",
            57 => "DrgSetDragitem",
            58 => "DrgVerifyNativeRMF",
            59 => "DrgVerifyRMF",
            60 => "DrgVerifyTrueType",
            61 => "DrgVerifyType",
            62 => "DrgVerifyTypeSet",
            65 => "DrgDragFiles",
            70 => "DrgQueryDraginfoPtr",
            71 => "DrgQueryDraginfoPtrFromHwnd",
            72 => "DrgQueryDraginfoPtrFromDragitem",
            73 => "DrgQueryDragStatus",
            74 => "DrgReallocDraginfo",
            _ => "?",
        },

        // ── SOM (the SOM runtime kernel; som.rs) ──────────────────────
        o if (SOM_BASE..REXX_BASE).contains(&o) => match o - SOM_BASE {
            1 => "SOMCalloc",
            2 => "SOMClassCClassData",
            3 => "SOMClassClassData",
            4 => "SOMClassInitFuncName",
            5 => "SOMClassMgrCClassData",
            6 => "SOMClassMgrClassData",
            7 => "SOMClassMgrObject",
            8 => "SOMDeleteModule",
            9 => "SOMError",
            10 => "SOMFree",
            11 => "SOMLoadModule",
            12 => "SOMMalloc",
            13 => "SOMObjectCClassData",
            14 => "SOMObjectClassData",
            15 => "SOMOutCharRoutine",
            16 => "SOMRealloc",
            17 => "SOM_AssertLevel",
            18 => "SOM_TraceLevel",
            19 => "SOM_WarnLevel",
            20 => "SOMClassMgrNewClass",
            21 => "SOMClassNewClass",
            22 => "SOMObjectNewClass",
            23 => "somAssert",
            24 => "somBeginPersistentIds",
            25 => "somCheckArgs",
            26 => "somCheckId",
            27 => "somCompareIds",
            28 => "somConstructClass",
            29 => "somEndPersistentIds",
            30 => "somEnvironmentNew",
            31 => "somIdFromString",
            32 => "somLPrintf",
            33 => "somParentResolve",
            34 => "somPrefixLevel",
            35 => "somPrintf",
            36 => "somRegisterId",
            37 => "somResolve",
            38 => "somSaveMetrics",
            39 => "somSetExpectedIds",
            40 => "somStringFromId",
            41 => "somTest",
            42 => "somTestCls",
            43 => "somTotalRegIds",
            44 => "somUniqueKey",
            45 => "somVprintf",
            46 => "somWriteMetrics",
            47 => "somDataResolve",
            3072 => "somResolveByName",
            3073 => "somClassResolve",
            3074 => "somGetGlobalEnvironment",
            3075 => "somIsObj",
            3076 => "somSetOutChar",
            3077 => "somParentNumResolve",
            3078 => "somBuildClass",
            3079 => "somDataResolveChk",
            3080 => "WrpSomMalloc",
            3081 => "WrpSomCalloc",
            3082 => "WrpSomRealloc",
            3083 => "WrpSomFree",
            3084 => "WrpSomMetaclassFor",
            3085 => "WrpSomLoadModule",
            3086 => "WrpSomDeleteModule",
            3087 => "WrpSomError",
            3088 => "WrpSomOutChar",
            3089 => "WrpSomClassInitFuncName",
            3090 => "WrpSomDataExport",
            3091 => "WrpSomInitCtrlNew",
            3092 => "WrpSomInitCtrlFree",
            3093 => "WrpSomInitWalk",
            3094 => "WrpSomDestructWalk",
            3095 => "WrpSomSetDirectInitClasses",
            3096 => "somSetException",
            3097 => "somExceptionId",
            3098 => "somExceptionValue",
            3099 => "somExceptionFree",
            3198 => "WrpSomMethodNotImplemented",
            3199 => "WrpSomMethodError",
            3200 => "WrpSOMObject_somInit",
            3201 => "WrpSOMObject_somUninit",
            3202 => "WrpSOMObject_somFree",
            3203 => "WrpSOMObject_somGetClass",
            3204 => "WrpSOMObject_somGetClassName",
            3205 => "WrpSOMObject_somGetSize",
            3206 => "WrpSOMObject_somIsA",
            3207 => "WrpSOMObject_somIsInstanceOf",
            3208 => "WrpSOMObject_somRespondsTo",
            3209 => "WrpSOMObject_somPrintSelf",
            3210 => "WrpSOMObject_somDumpSelf",
            3211 => "WrpSOMObject_somDumpSelfInt",
            3212 => "WrpSOMObject_somDefaultInit",
            3213 => "WrpSOMObject_somDestruct",
            3214 => "WrpSOMObject_somDefaultCopyInit",
            3215 => "WrpSOMObject_somDefaultConstCopyInit",
            3216 => "WrpSOMObject_somDefaultVCopyInit",
            3217 => "WrpSOMObject_somDefaultConstVCopyInit",
            3218 => "WrpSOMObject_somDefaultAssign",
            3219 => "WrpSOMObject_somDefaultConstAssign",
            3220 => "WrpSOMObject_somDefaultVAssign",
            3221 => "WrpSOMObject_somDefaultConstVAssign",
            3300 => "WrpSOMClass_somNew",
            3301 => "WrpSOMClass_somNewNoInit",
            3302 => "WrpSOMClass_somGetName",
            3303 => "WrpSOMClass_somGetParent",
            3304 => "WrpSOMClass_somGetInstanceSize",
            3305 => "WrpSOMClass_somGetInstancePartSize",
            3306 => "WrpSOMClass_somGetInstanceToken",
            3307 => "WrpSOMClass_somGetNumMethods",
            3308 => "WrpSOMClass_somGetNumStaticMethods",
            3309 => "WrpSOMClass_somDescendedFrom",
            3310 => "WrpSOMClass_somInitClass",
            3311 => "WrpSOMClass_somAddStaticMethod",
            3312 => "WrpSOMClass_somOverrideSMethod",
            3313 => "WrpSOMClass_somClassReady",
            3314 => "WrpSOMClass_somGetMethodToken",
            3315 => "WrpSOMClass_somSupportsMethod",
            3316 => "WrpSOMClass_somInitMIClass",
            3317 => "WrpSOMClass_somCheckVersion",
            3318 => "WrpSOMClass_somGetVersionNumbers",
            3319 => "WrpSOMClass_somGetClassMtab",
            3320 => "WrpSOMClass_somGetInstanceOffset",
            3321 => "WrpSOMClass_somFindMethod",
            3322 => "WrpSOMClass_somFindMethodOk",
            3323 => "WrpSOMClass_somFindSMethod",
            3324 => "WrpSOMClass_somFindSMethodOk",
            3325 => "WrpSOMClass_somLookupMethod",
            3326 => "WrpSOMClass_somAllocate",
            3327 => "WrpSOMClass_somDeallocate",
            3400 => "WrpSOMClassMgr_somFindClass",
            3401 => "WrpSOMClassMgr_somClassFromId",
            3402 => "WrpSOMClassMgr_somRegisterClass",
            3403 => "WrpSOMClassMgr_somFindClsInFile",
            3404 => "WrpSOMClassMgr_somLocateClassFile",
            3405 => "WrpSOMClassMgr_somLoadClassFile",
            3406 => "WrpSOMClassMgr_somUnregisterClass",
            3407 => "WrpSOMClassMgr_somUnloadClassFile",
            _ => "?",
        },

        // ── REXX (the REXX API; rexxapi.rs) ────────────────────────────
        o if (REXX_BASE..REXXAPI_BASE).contains(&o) => match o - REXX_BASE {
            1 => "RexxStart",
            2 => "RexxVariablePool",
            3 => "RexxSetHalt",
            4 => "RexxSetTrace",
            5 => "RexxResetTrace",
            _ => "?",
        },

        // ── REXXAPI (registrations and queues; rexxapi.rs) ─────────────
        o if (REXXAPI_BASE..PMWP_BASE).contains(&o) => match o - REXXAPI_BASE {
            1 => "RexxCreateQueue",
            2 => "RexxDeleteQueue",
            3 => "RexxAddQueue",
            4 => "RexxPullQueue",
            5 => "RexxQueryQueue",
            6 => "RexxRegisterSubcomDll",
            7 => "RexxRegisterSubcomExe",
            8 => "RexxQuerySubcom",
            9 => "RexxDeregisterSubcom",
            11 => "RexxRegisterExitDll",
            12 => "RexxRegisterExitExe",
            13 => "RexxDeregisterExit",
            14 => "RexxQueryExit",
            17 => "RexxRegisterFunctionDll",
            18 => "RexxRegisterFunctionExe",
            19 => "RexxDeregisterFunction",
            20 => "RexxQueryFunction",
            22 => "RexxAddMacro",
            23 => "RexxDropMacro",
            24 => "RexxClearMacroSpace",
            25 => "RexxLoadMacroSpace",
            26 => "RexxQueryMacro",
            27 => "RexxReorderMacro",
            28 => "RexxSaveMacroSpace",
            _ => "?",
        },

        // ── PMWP (the Workplace Shell; pm_winutil.rs) ───────────────────
        o if (PMWP_BASE..STUB_AREA_SIZE).contains(&o) => match o - PMWP_BASE {
            207 => "WinStoreWindowPos",
            208 => "WinRestoreWindowPos",
            _ => "?",
        },

        _ => "?",
    }
}

// ── Ordinal → DLL/module name ─────────────────────────────────────────────────

/// Map a flat warpine ordinal to the OS/2 DLL that owns it.
pub fn module_for_ordinal(ordinal: u32) -> &'static str {
    if ordinal < 1024               { "DOSCALLS" }
    else if ordinal < PMWIN_BASE    { "QUECALLS" }   // 1024–2047
    else if ordinal < PMGPI_BASE    { "PMWIN" }      // 2048–3071
    else if ordinal < KBDCALLS_BASE { "PMGPI" }      // 3072–4095
    else if ordinal < VIOCALLS_BASE { "KBDCALLS" }   // 4096–5119
    else if ordinal < SESMGR_BASE   { "VIOCALLS" }   // 5120–6143
    else if ordinal < NLS_BASE      { "SESMGR" }     // 6144–7167
    else if ordinal < MSG_BASE      { "NLS" }        // 7168–8191
    else if ordinal < MDM_BASE      { "MSG" }        // 8192–10239
    else if ordinal < UCONV_BASE    { "MDM" }        // 10240–12287
    else if ordinal < SO32DLL_BASE  { "UCONV" }     // 12288–13311
    else if ordinal < TCP32DLL_BASE { "SO32DLL" }   // 13312–14335
    else if ordinal < MOUCALLS_BASE { "TCP32DLL" } // 14336–15359
    else if ordinal < PMSHAPI_BASE  { "MOUCALLS" } // 15360–16383
    else if ordinal < HELPMGR_BASE  { "PMSHAPI" }  // 16384–17407
    else if ordinal < PMSPL_BASE    { "HELPMGR" }  // 17408–18431
    else if ordinal < PMCTLS_BASE   { "PMSPL" }    // 18432–19455
    else if ordinal < WARPINE_BASE { "PMCTLS" }    // 19456–20479
    else if ordinal < PMDRAG_BASE  { "WARPINE" }   // 20480–21503
    else if ordinal < SOM_BASE     { "PMDRAG" }    // 21504–22527
    else if ordinal < REXX_BASE    { "SOM" }       // 22528–26623 (SOM_ORDINALS wide)
    else if ordinal < REXXAPI_BASE { "REXX" }      // 26624–27647
    else if ordinal < PMWP_BASE    { "REXXAPI" }   // 27648–28671
    else if ordinal < STUB_AREA_SIZE { "PMWP" }    // 28672–29695
    else                            { "?" }
}

// ── Ordinal → argument names ──────────────────────────────────────────────────

/// Return the ordered parameter names for a given warpine flat ordinal.
///
/// Returns an empty slice for ordinals whose argument metadata is not yet
/// recorded (unknown, higher subsystem, or intentionally omitted).
pub fn arg_names_for_ordinal(ordinal: u32) -> &'static [&'static str] {
    match ordinal {
        // ── DOSCALLS ──────────────────────────────────────────────────────
        8   => &["pGlobal", "pLocal"],
        75  => &["pszName", "pulAttribute"],
        84  => &["pszName", "ulAttribute"],
        110 => &["pszPathName"],
        209 => &["cFH"],
        210 => &["fVerify"],
        212 => &["fEnable"],
        218 => &["hFile", "ulInfoLevel", "pInfoBuf", "cbInfoBuf"],
        219 => &["pszPathName", "ulInfoLevel", "pInfoBuf", "cbInfoBuf", "flOptions"],
        220 => &["usDriveNumber"],
        221 => &["hFile", "flState"],
        223 => &["pszPathName", "ulInfoLevel", "pInfoBuf", "cbInfoBuf"],
        224 => &["hFile", "pulType", "pAttr"],
        225 => &["pfVerify"],
        226 => &["pszDirName"],
        228 => &["flag", "pszPathOrName", "pszFilename", "pBuf", "cbBuf"],
        229 => &["ulSleep"],
        230 => &["pDT"],
        231 => &["pDevInfo", "ulDevInfo"],
        234 => &["ulAction", "ulResult"],
        235 => &["ulAction", "PID"],
        239 => &["phfRead", "phfWrite", "ulPipeSize"],
        240 => &["pszName", "pInbuf", "cbIn", "pOutbuf", "cbOut", "pcbActual", "msec"],
        241 => &["hPipe"],
        242 => &["hPipe"],
        243 => &["pszName", "phPipe", "openMode", "pipeMode", "cbOutBuf", "cbInBuf", "msec"],
        244 => &["hPipe", "pBuf", "cbBuf", "pcbActual", "pAvail", "pState"],
        245 => &["hPipe", "pState"],
        248 => &["hPipe", "infolevel", "pBuf", "cbBuf"],
        249 => &["hsem", "pnpss", "cbBuf"],
        250 => &["hPipe", "flState"],
        251 => &["hPipe", "hsem", "key"],
        252 => &["hPipe", "pOutbuf", "cbOut", "pInbuf", "cbIn", "pcbRead"],
        253 => &["pszName", "msec"],
        254 => &["hFile"],
        255 => &["pszDirName"],
        256 => &["hFile", "ib", "method", "pibNew"],
        257 => &["hFile"],
        258 => &["pszOldName", "pszNewName", "ulOption"],
        259 => &["pszPathName"],
        260 => &["hFile", "phFile2"],
        261 => &["metaLevel", "pszSrc", "pszEdit", "pszDst", "cbDst"],
        263 => &["hDir"],
        264 => &["pszFileSpec", "phDir", "flAttribute", "pfindbuf", "cbBuf", "pcFileNames", "ulInfoLevel"],
        265 => &["hDir", "pfindbuf", "cbBuf", "pcFileNames"],
        267 => &["hVDD", "usReqFn", "pRequest"],
        270 => &["pszDirName", "pEABuf"],
        271 => &["pszOldName", "pszNewName"],
        272 => &["hFile", "cbSize"],
        273 => &["pszFileName", "phFile", "pulAction", "cbFile", "ulAttribute", "fsOpenFlags", "fsOpenMode", "peaop2"],
        274 => &["usDriveNumber", "pszDir", "pcbDirPath"],
        275 => &["pusDriveNumber", "pLogicalDriveMap"],
        276 => &["hFile", "pflState"],
        277 => &["pszDeviceName", "ulOrdinal", "ulFSAInfoLevel", "pfsqb", "pcbBuf"],
        278 => &["usDriveNumber", "ulFSInfoLevel", "pBuf", "cbBuf"],
        279 => &["hFile", "ulInfoLevel", "pInfoBuf", "cbInfoBuf"],
        280 => &["ulAction", "ulWait", "pRes", "ppid"],
        281 => &["hFile", "pBuf", "cbBuf", "pcbActual"],
        282 => &["hFile", "pBuf", "cbBuf", "pcbActual"],
        283 => &["pObjname", "cbObjname", "execFlag", "pArg", "pEnv", "pRes", "pszProgName"],
        284 => &["hDevice", "ulCategory", "ulFunction", "pParams", "cbParmList", "pcbParmList", "pData", "cbData", "pcbData"],
        285 => &["pszPathName", "iFunc", "pFSData", "cbFSData", "pBuf", "cbBuf", "hDir"],
        286 => &["ulFreq", "ulDuration"],
        289 => &["ulCodePage"],
        290 => &["htimer"],
        291 => &["cb", "arCP", "pcCP"],
        292 => &["pDT"],
        299 => &["ppb", "cb", "flAttr"],
        300 => &["ppb", "pszName", "cb", "flag"],
        301 => &["ppb", "pszName", "flag"],
        302 => &["pb", "flag"],
        303 => &["pb", "pid", "flag"],
        304 => &["pb"],
        305 => &["pb", "cb", "flag"],
        306 => &["pb", "pcb", "pfl"],
        311 => &["ptid", "pfn", "param", "flag", "cbStack"],
        312 => &["pptib", "pppib"],
        317 => &["pdbgbuf"],
        318 => &["pszObj", "cbObj", "pszModule", "phmod"],
        319 => &["pszModuleName", "phmod"],
        320 => &["hmod", "cbName", "pch"],
        321 => &["hmod", "ordinal", "pszName", "ppfn"],
        322 => &["hmod"],
        323 => &["pszName", "pFlags"],
        324 => &["pszName", "phev", "flAttr", "fState"],
        325 => &["pszName", "phev"],
        326 => &["hev"],
        296 => &["ulOrderCode", "pfn"],
        327 => &["hev", "pulPostCt"],
        328 => &["hev"],
        329 => &["hev", "ulTimeout"],
        330 => &["hev", "pulPostCt"],
        331 => &["pszName", "phmtx", "flAttr", "fState"],
        332 => &["pszName", "phmtx"],
        333 => &["hmtx"],
        334 => &["hmtx", "ulTimeout"],
        335 => &["hmtx"],
        336 => &["hmtx", "ppid", "ptid", "pulCount"],
        337 => &["pszName", "phmux", "cSemRec", "pSemRec", "flAttr"],
        338 => &["pszName", "phmux"],
        339 => &["hmux"],
        340 => &["hmux", "ulTimeout", "pulUser"],
        341 => &["hmux", "pSemRec"],
        342 => &["hmux", "hsem"],
        343 => &["hmux", "pcSemRec", "pSemRec", "pflAttr"],
        344 => &["pOffset", "flags", "cbPool"],
        345 => &["pOffset", "ppBlock", "cb"],
        346 => &["pOffset", "pBlock", "cb"],
        347 => &["pOffset"],
        348 => &["iStart", "iLast", "pBuf", "cbBuf"],
        349 => &["ptid", "option"],
        350 => &["msec", "hsem", "phtimer"],
        351 => &["msec", "hsem", "phtimer"],
        352 => &["hmod", "ulTypeID", "ulNameID", "ppb"],
        353 => &["pb"],
        354 => &["pERegRec"],
        355 => &["pERegRec"],
        356 => &["pexcept", "pRegRec"],
        357 => &["pHandler", "pTargetIP", "pexcept"],
        237 | 238 => &["tid"],
        379 => &["pid", "exception"],
        368 => &["EntityList", "EntityLevel", "PID", "TID", "pDataBuf", "cbBuf"],
        372 => &["ulRefType", "pvFile", "ulEntry", "pvBuf", "cbBuf", "pcbActual", "ulInfoLevel"],
        378 => &["fEnable", "pulTimes"],
        380 => &["pulNesting"],
        381 => &["pulNesting"],
        382 => &["pcbReqCount", "pcbCurMaxFH"],
        415 => &["ulReserved"],
        418 => &["ulSignalNum"],
        425 => &["ptr"],
        426 => &["ptr"],
        428 => &["hFile", "pUnlockRange", "pLockRange", "ulTimeout", "ulFlags"],
        572 => &["hmod", "ulTypeID", "ulNameID", "pcb"],
        639 => &["hFile", "pUnlockRange", "pLockRange", "ulTimeout", "ulFlags"],

        // ── QUECALLS (1024 + local ordinal) ───────────────────────────────
        o if (1024..2048).contains(&o) => match o - 1024 {
            9  => &["hq", "pRequest", "pcbData", "ppbuf", "ulElement", "fWait", "pElemCode", "hsem"],
            10 => &["hq"],
            11 => &["hq"],
            12 => &["hq", "pcbEntries"],
            14 => &["hq", "ulRequest", "cbData", "pbData", "ulPriority"],
            15 => &["ppid", "phq", "pszName"],
            16 => &["phq", "flQueueAttr", "pszName"],
            _  => &[],
        },

        // ── NLS ───────────────────────────────────────────────────────────
        o if (NLS_BASE..MSG_BASE).contains(&o) => match o - NLS_BASE {
            5 => &["cb", "pcc", "pci", "pcbActual"],
            6 => &["cb", "pcc", "pBuf"],
            7 => &["cb", "pcc", "pch"],
            8 => &["cb", "pcc", "pch", "pcch"],
            _ => &[],
        },

        // ── MSG ───────────────────────────────────────────────────────────
        o if (MSG_BASE..MDM_BASE).contains(&o) => match o - MSG_BASE {
            4 => &["pTable", "cTable", "pszMsg", "cbMsg", "pBuf", "cbBuf", "pcbMsg"],
            5 => &["hFile", "cbMsg", "pBuf"],
            6 => &["pMsgSeg", "pTable", "cTable", "pBuf", "cbBuf", "ulMsgNumber", "pszFile", "pcbMsg"],
            8 => &["pb", "cb", "pszFilename", "pcbBuf"],
            _ => &[],
        },

        // ── MDM ───────────────────────────────────────────────────────────
        o if (MDM_BASE..UCONV_BASE).contains(&o) => match o - MDM_BASE {
            1 => &["usDeviceID", "usMessage", "ulParam1", "pParam2"],
            2 => &["pszCommandBuf", "pszReturnString", "usReturnLength", "hwndCallback"],
            3 => &["pMemToFree"],
            4 => &["pszErrorBuf", "usBufLen"],
            _ => &[],
        },

        // ── UCONV ─────────────────────────────────────────────────────────
        o if (UCONV_BASE..SO32DLL_BASE).contains(&o) => match o - UCONV_BASE {
            1 => &["ucsName", "puobj"],
            2 => &["uobj"],
            3 => &["uobj", "ppInBuf", "pInBytesLeft", "ppOutBuf", "pOutCharsLeft", "pNumSubs"],
            4 => &["uobj", "ppInBuf", "pInCharsLeft", "ppOutBuf", "pOutBytesLeft", "pNumSubs"],
            6 => &["ulCodePage", "ucsCodePage", "n"],
            _ => &[],
        },

        // ── SO32DLL ───────────────────────────────────────────────────────
        o if (SO32DLL_BASE..TCP32DLL_BASE).contains(&o) => match o - SO32DLL_BASE {
            1  => &["s", "addr", "addrlen"],
            2  => &["s", "name", "namelen"],
            3  => &["s", "name", "namelen"],
            5  => &["name", "namelen"],
            6  => &["s", "name", "namelen"],
            7  => &["s", "name", "namelen"],
            8  => &["s", "level", "optname", "optval", "optlen"],
            10 => &["s", "backlog"],
            11 => &["s", "buf", "len", "flags"],
            12 => &["s", "buf", "len", "flags", "from", "fromlen"],
            13 => &["nfds", "readfds", "writefds", "exceptfds", "timeout"],
            14 => &["s", "msg", "len", "flags"],
            15 => &["s", "msg", "len", "flags", "to", "tolen"],
            16 => &["s", "level", "optname", "optval", "optlen"],
            17 => &["s", "how"],
            19 => &["domain", "type", "protocol"],
            20 => &["s"],
            21 => &["pszMsg"],
            40 => &["pszName"],
            41 => &["addr", "len", "type"],
            42 => &["pszName", "pszProto"],
            43 => &["port", "pszProto"],
            44 => &["pszName"],
            45 => &["proto"],
            _ => &[],
        },

        // ── TCP32DLL ──────────────────────────────────────────────────────
        o if (TCP32DLL_BASE..MOUCALLS_BASE).contains(&o) => match o - TCP32DLL_BASE {
            1  => &["s", "addr", "addrlen"],
            2  => &["s", "name", "namelen"],
            3  => &["s", "name", "namelen"],
            6  => &["s", "name", "namelen"],
            7  => &["s", "name", "namelen"],
            8  => &["s", "level", "optname", "optval", "optlen"],
            10 => &["s", "backlog"],
            11 => &["s", "buf", "len", "flags"],
            12 => &["s", "buf", "len", "flags", "from", "fromlen"],
            13 => &["nfds", "readfds", "writefds", "exceptfds", "timeout"],
            14 => &["s", "msg", "len", "flags"],
            15 => &["s", "msg", "len", "flags", "to", "tolen"],
            16 => &["s", "level", "optname", "optval", "optlen"],
            17 => &["s", "how"],
            19 => &["domain", "type", "protocol"],
            20 => &["s"],
            40 => &["pszName"],
            41 => &["addr", "len", "type"],
            42 => &["pszName", "pszProto"],
            43 => &["port", "pszProto"],
            44 => &["pszName"],
            45 => &["proto"],
            _ => &[],
        },

        // ── MOUCALLS (declaration order; the call is Pascal, see
        //    `pascal_order`) ─────────────────────────────────────────────
        o if (MOUCALLS_BASE..PMSHAPI_BASE).contains(&o) => match o - MOUCALLS_BASE {
            1 | 2 => &["pBuf", "pShape", "hmou"],
            3  => &["pcMickeys", "hmou"],
            4 | 10 => &["pfsButtons", "hmou"],
            6 | 11 => &["pScale", "hmou"],
            7 | 9 | 26 => &["hmou"],
            8  => &["pcButtons", "hmou"],
            13 => &["pQueInfo", "hmou"],
            15 | 16 => &["pfsMask", "hmou"],
            17 => &["pszDriver", "phmou"],
            18 => &["pRect", "hmou"],
            19 | 21 => &["pPos", "hmou"],
            20 => &["pEvent", "pfWait", "hmou"],
            22 | 25 => &["pfsStatus", "hmou"],
            23 => &["fWait"],
            24 => &["pszModName", "pszEntryName", "flFuncs"],
            27 => &["pszDriver"],
            29 | 30 => &["pThreshold", "hmou"],
            _ => &[],
        },

        // ── PMSHAPI ───────────────────────────────────────────────────────
        o if (PMSHAPI_BASE..HELPMGR_BASE).contains(&o) => match o - PMSHAPI_BASE {
            101 => &["hini", "pszApp", "pszKey", "pulSize"],
            102 => &["hab", "pszFileName"],
            103 => &["hini"],
            114 => &["hini", "pszApp", "pszKey", "lDefault"],
            115 => &["hini", "pszApp", "pszKey", "pszDefault", "pBuffer", "cchBufferMax"],
            116 => &["hini", "pszApp", "pszKey", "pszData"],
            117 => &["hini", "pszApp", "pszKey", "pBuffer", "pulBufferMax"],
            118 => &["hini", "pszApp", "pszKey", "pData", "cchDataLen"],
            119 => &["hwndNotifyWindow", "pDetails", "pszParams", "Reserved", "fbOptions"],
            120 => &["pswctl"],
            121 => &["hab", "pswctl"],
            123 => &["hswitch", "pswctl"],
            124 => &["hswitch", "pswctl"],
            125 => &["hwnd", "pid"],
            126 => &["hab", "pswblk", "usDataLength"],
            129 => &["hswitch"],
            130 => &["happ"],
            131 => &["hswitch"],
            _ => &[],
        },

        // ── HELPMGR ───────────────────────────────────────────────────────
        o if (HELPMGR_BASE..PMSPL_BASE).contains(&o) => match o - HELPMGR_BASE {
            51 => &["hab", "phinit"],
            52 => &["hwndHelp"],
            53 => &["hwndApp"],
            54 => &["hwndHelp", "hwndApp"],
            55 => &["hwndHelp", "idHelpTable", "hmod"],
            56 => &["hwndHelp", "phtHelpTable"],
            _ => &[],
        },

        // ── PMSPL ─────────────────────────────────────────────────────────
        o if (PMSPL_BASE..PMCTLS_BASE).contains(&o) => match o - PMSPL_BASE {
            301 => &["pszToken", "lCount", "pqmdopData"],
            302 => &["hspl", "pszDocName"],
            303 | 305 | 306 | 308 => &["hspl"],
            304 => &["hspl", "lCount", "pData"],
            381 => &["pszComputerName", "pszPrintDeviceName", "ulLevel", "pBuf", "cbBuf", "pcbNeeded"],
            397 => &["pszComputerName", "pszQueueName", "ulLevel", "pBuf", "cbBuf", "pcbNeeded"],
            382 | 399 | 406 | 407 | 408 =>
                &["pszComputerName", "ulLevel", "pBuf", "cbBuf", "pcReturned", "pcTotal", "pcbNeeded", "pReserved"],
            441 => &["pszComputerName", "uLevel", "flType", "pBuf", "cbbuf", "pcReturned", "pcTotal",
                     "pcbNeeded", "pReserved"],
            _ => &[],
        },

        // ── PMCTLS ────────────────────────────────────────────────────────
        o if (PMCTLS_BASE..WARPINE_BASE).contains(&o) => match o - PMCTLS_BASE {
            2 => &["hwndP", "hwndO", "pfntd"],
            3 | 5 => &["hwnd", "msg", "mp1", "mp2"],
            4 => &["hwndP", "hwndO", "pfild"],
            6 => &["papszFQFilename"],
            _ => &[],
        },

        // ── WARPINE ───────────────────────────────────────────────────────
        o if (WARPINE_BASE..PMDRAG_BASE).contains(&o) => match o - WARPINE_BASE {
            1 => &["ptime"],
            2 => &["pdt", "ptime"],
            3 => &["ptime", "pdt"],
            4 => &["pdt", "ptime"],
            5 => &["hwnd", "msg", "mp1", "mp2"],
            _ => &[],
        },

        // ── PMDRAG ────────────────────────────────────────────────────────
        o if (PMDRAG_BASE..SOM_BASE).contains(&o) => match o - PMDRAG_BASE {
            32 => &["pdinfo"],
            33 => &["pszString"],
            34 => &["cditem"],
            35 => &["cdxfer"],
            36 => &["pdinfo"],
            37 => &["hstr"],
            38 => &["hwndSource", "pdinfo", "pdimg", "cdimg", "vkTerminate", "pRsvd"],
            39 => &["pdinfo"],
            40 => &["pdxfer"],
            41 => &["hwnd"],
            42 => &["hwnd", "msg", "pdxfer", "fl", "ulRsvd", "fRetry"],
            43 => &["pdinfo", "hwndDest"],
            44 => &["pdinfo", "cbBuffer", "pditem", "iItem"],
            45 => &["pdinfo"],
            46 => &["pdinfo", "i"],
            47 => &["pditem", "cbBuffer", "pBuffer"],
            48 => &["pditem"],
            49 => &["hstr", "cbBuffer", "pBuffer"],
            50 => &["hstr"],
            51 => &["pditem", "cbBuffer", "pBuffer"],
            52 => &["pditem"],
            53 => &["hps"],
            54 => &["hwnd", "msg", "mp1", "mp2"],
            55 => &["pdinfo", "hptr"],
            56 => &["pdinfo", "pdimg", "cdimg", "pRsvd"],
            57 => &["pdinfo", "pditem", "cbBuffer", "iItem"],
            58 => &["pditem", "pszRMF"],
            59 => &["pditem", "pszMech", "pszFmt"],
            60 => &["pditem", "pszType"],
            61 => &["pditem", "pszType"],
            62 => &["pditem", "pszType", "cbMatch", "pszMatch"],
            72 => &["pditem"],
            73 => &[],
            _ => &[],
        },

        _ => &[],
    }
}

/// Whether `ordinal`'s module passes its arguments in Pascal order (VIO,
/// KBD and MOU calls, `OS2_PASCAL_API`): the last argument at ESP+4.
pub fn pascal_order(ordinal: u32) -> bool {
    matches!(module_for_ordinal(ordinal), "VIOCALLS" | "KBDCALLS" | "MOUCALLS")
}

// ── Call formatter ────────────────────────────────────────────────────────────

/// Format an API call as a strace-style string, e.g.:
///   `DosWrite(hFile=5, pBuf=0x02001000, cbBuf=42, pcbActual=0x02001100)`
///
/// Formatting rules applied per argument name:
/// - Names starting with `psz` and a non-null value → dereference via
///   `read_guest_str` and render as a quoted string.
/// - Names starting with `h` but not `hw` (file/sem/queue handles) → decimal.
/// - Everything else → `0x{:X}` hex.
///
/// `read_guest_str` is a caller-supplied closure so this module stays free
/// of any dependency on `Loader` or other subsystem types.
pub fn format_call(
    name: &str,
    ordinal: u32,
    args: &[u32; 10],
    read_guest_str: &dyn Fn(u32) -> String,
) -> String {
    let arg_names = arg_names_for_ordinal(ordinal);
    if arg_names.is_empty() {
        return format!("{}(…)", name);
    }
    // `args` are the stack words from ESP+4 up; a Pascal call pushed its
    // first argument first, so it lies highest.
    let n = arg_names.len();
    let pascal = pascal_order(ordinal);
    let parts: Vec<String> = arg_names.iter().enumerate().map(|(i, &aname)| {
        let val = args[if pascal { n - 1 - i } else { i }];
        if aname.starts_with("psz") && val != 0 {
            format!("{}={:?}", aname, read_guest_str(val))
        } else if aname.starts_with('h') && !aname.starts_with("hw") {
            // File/sem/queue/module handles: decimal is more readable
            format!("{}={}", aname, val)
        } else {
            format!("{}=0x{:X}", aname, val)
        }
    }).collect();
    format!("{}({})", name, parts.join(", "))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── arg_names_for_ordinal ─────────────────────────────────────────────

    #[test]
    fn pascal_calls_name_their_arguments_in_declaration_order() {
        // MouGetPtrPos(pPos, hmou): hmou at ESP+4, pPos above it.
        let mut args = [0u32; 10];
        args[0] = 1;
        args[1] = 0x2000;
        assert!(pascal_order(MOUCALLS_BASE + 19));
        assert!(!pascal_order(8));
        assert_eq!(format_call("MouGetPtrPos", MOUCALLS_BASE + 19, &args, &|_| String::new()),
                   "MouGetPtrPos(pPos=0x2000, hmou=1)");
        assert_eq!(arg_names_for_ordinal(MOUCALLS_BASE + 24), &["pszModName", "pszEntryName", "flFuncs"]);
        assert_eq!(ordinal_to_name(MOUCALLS_BASE + 30), "MouSetThreshold");
    }


    #[test]
    fn test_arg_names_dos_write() {
        let names = arg_names_for_ordinal(282);
        assert_eq!(names, &["hFile", "pBuf", "cbBuf", "pcbActual"]);
    }

    #[test]
    fn test_arg_names_dos_open() {
        let names = arg_names_for_ordinal(273);
        assert_eq!(names[0], "pszFileName");
        assert_eq!(names.len(), 8);
    }

    #[test]
    fn test_arg_names_dos_sleep() {
        assert_eq!(arg_names_for_ordinal(229), &["ulSleep"]);
    }

    #[test]
    fn test_arg_names_unknown_returns_empty() {
        assert!(arg_names_for_ordinal(0).is_empty());
        assert!(arg_names_for_ordinal(1).is_empty());
        assert!(arg_names_for_ordinal(9999).is_empty());
    }

    #[test]
    fn test_arg_names_quecalls() {
        // DosWriteQueue: QUECALLS local ordinal 14 = flat 1024+14 = 1038
        let names = arg_names_for_ordinal(1024 + 14);
        assert_eq!(names[0], "hq");
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn test_arg_names_mdm() {
        let names = arg_names_for_ordinal(MDM_BASE + 2); // mciSendString
        assert_eq!(names[0], "pszCommandBuf");
        assert_eq!(names.len(), 4);
    }

    // ── format_call ──────────────────────────────────────────────────────

    #[test]
    fn test_format_call_dos_write() {
        let mut args = [0u32; 10];
        args[0] = 5;            // hFile
        args[1] = 0x02001000;   // pBuf
        args[2] = 42;           // cbBuf
        args[3] = 0x02001100;   // pcbActual
        let s = format_call("DosWrite", 282, &args, &|_| String::new());
        assert_eq!(s, "DosWrite(hFile=5, pBuf=0x2001000, cbBuf=0x2A, pcbActual=0x2001100)");
    }

    #[test]
    fn test_format_call_dos_open_dereferences_psz() {
        let mut args = [0u32; 10];
        args[0] = 0x1000; // pszFileName (non-null)
        let s = format_call("DosOpen", 273, &args,
            &|ptr| if ptr == 0x1000 { "C:\\test.txt".to_string() } else { String::new() });
        // {Debug} escapes backslash: C:\test.txt becomes "C:\\test.txt" in output
        assert!(s.starts_with("DosOpen(pszFileName="), "got: {}", s);
        assert!(s.contains("test.txt"), "got: {}", s);
    }

    #[test]
    fn test_format_call_psz_null_not_dereferenced() {
        let args = [0u32; 10]; // all null
        let s = format_call("DosOpen", 273, &args, &|_| panic!("should not call"));
        // pszFileName=0x0 (null → not dereferenced)
        assert!(s.contains("pszFileName=0x0"), "got: {}", s);
    }

    #[test]
    fn test_format_call_unknown_ordinal() {
        let args = [0u32; 10];
        let s = format_call("?", 9999, &args, &|_| String::new());
        assert_eq!(s, "?(…)");
    }

    #[test]
    fn test_format_call_handle_is_decimal() {
        let mut args = [0u32; 10];
        args[0] = 7; // hFile=7
        let s = format_call("DosClose", 257, &args, &|_| String::new());
        assert_eq!(s, "DosClose(hFile=7)");
    }

    // ── ordinal_to_name ───────────────────────────────────────────────────

    #[test]
    fn test_known_doscalls_ordinals() {
        assert_eq!(ordinal_to_name(282), "DosWrite");
        assert_eq!(ordinal_to_name(281), "DosRead");
        assert_eq!(ordinal_to_name(273), "DosOpen");
        assert_eq!(ordinal_to_name(257), "DosClose");
        assert_eq!(ordinal_to_name(234), "DosExit");
        assert_eq!(ordinal_to_name(299), "DosAllocMem");
        assert_eq!(ordinal_to_name(304), "DosFreeMem");
        assert_eq!(ordinal_to_name(311), "DosCreateThread");
        assert_eq!(ordinal_to_name(312), "DosGetInfoBlocks");
    }

    #[test]
    fn test_known_quecalls_ordinals() {
        assert_eq!(ordinal_to_name(1024 + 16), "DosCreateQueue");
        assert_eq!(ordinal_to_name(1024 + 15), "DosOpenQueue");
        assert_eq!(ordinal_to_name(1024 + 14), "DosWriteQueue");
        assert_eq!(ordinal_to_name(1024 + 9),  "DosReadQueue");
        assert_eq!(ordinal_to_name(1024 + 11), "DosCloseQueue");
        assert_eq!(ordinal_to_name(1024 + 10), "DosPurgeQueue");
        assert_eq!(ordinal_to_name(1024 + 12), "DosQueryQueue");
    }

    #[test]
    fn test_unknown_ordinals_return_question() {
        assert_eq!(ordinal_to_name(0),    "?");
        assert_eq!(ordinal_to_name(999),  "?");
        assert_eq!(ordinal_to_name(1024 + 99), "?"); // unknown QUECALLS sub-ordinal
        assert_eq!(ordinal_to_name(2048), "?"); // PMWIN — not in def
        assert_eq!(ordinal_to_name(3072), "?"); // PMGPI — not in def
        assert_eq!(ordinal_to_name(9999), "?");
    }

    #[test]
    fn test_kbdcalls_and_viocalls_ordinal_names() {
        assert_eq!(ordinal_to_name(KBDCALLS_BASE + 4),  "KbdCharIn");
        assert_eq!(ordinal_to_name(KBDCALLS_BASE + 9),  "KbdStringIn");
        assert_eq!(ordinal_to_name(KBDCALLS_BASE + 10), "KbdGetStatus");
        assert_eq!(ordinal_to_name(KBDCALLS_BASE + 99), "?");
        assert_eq!(ordinal_to_name(VIOCALLS_BASE + 19), "VioWrtTTY");
        assert_eq!(ordinal_to_name(VIOCALLS_BASE + 9),  "VioGetCurPos");
        assert_eq!(ordinal_to_name(VIOCALLS_BASE + 15), "VioSetCurPos");
        assert_eq!(ordinal_to_name(VIOCALLS_BASE + 75), "VioCheckCharType");
        assert_eq!(ordinal_to_name(VIOCALLS_BASE + 99), "?");
    }

    #[test]
    fn test_newly_named_doscalls_ordinals() {
        assert_eq!(ordinal_to_name(111), "DosKillThread");
        assert_eq!(ordinal_to_name(227), "DosScanEnv");
        assert_eq!(ordinal_to_name(236), "DosSetPriority");
        assert_eq!(ordinal_to_name(873), "DosSetExtLIBPATH");
        assert_eq!(ordinal_to_name(874), "DosQueryExtLIBPATH");
    }

    #[test]
    fn test_nls_all_ordinals_named() {
        assert_eq!(ordinal_to_name(NLS_BASE + 5), "DosQueryCtryInfo");
        assert_eq!(ordinal_to_name(NLS_BASE + 6), "DosQueryDBCSEnv");
        assert_eq!(ordinal_to_name(NLS_BASE + 7), "DosMapCase");
        assert_eq!(ordinal_to_name(NLS_BASE + 8), "DosQueryCollate");
    }

    #[test]
    fn test_module_for_ordinal_doscalls_range() {
        assert_eq!(module_for_ordinal(0),    "DOSCALLS");
        assert_eq!(module_for_ordinal(282),  "DOSCALLS");
        assert_eq!(module_for_ordinal(1023), "DOSCALLS");
    }

    #[test]
    fn test_module_for_ordinal_quecalls_range() {
        assert_eq!(module_for_ordinal(1024), "QUECALLS");
        assert_eq!(module_for_ordinal(2047), "QUECALLS");
    }

    #[test]
    fn test_module_for_ordinal_pm_and_higher() {
        assert_eq!(module_for_ordinal(PMWIN_BASE),    "PMWIN");
        assert_eq!(module_for_ordinal(PMGPI_BASE),    "PMGPI");
        assert_eq!(module_for_ordinal(KBDCALLS_BASE), "KBDCALLS");
        assert_eq!(module_for_ordinal(VIOCALLS_BASE), "VIOCALLS");
        assert_eq!(module_for_ordinal(SESMGR_BASE),   "SESMGR");
        assert_eq!(module_for_ordinal(NLS_BASE),      "NLS");
        assert_eq!(module_for_ordinal(MSG_BASE),      "MSG");
        assert_eq!(module_for_ordinal(MDM_BASE),      "MDM");
    }

    #[test]
    fn test_module_for_ordinal_boundaries() {
        // One before each base falls in the previous module
        assert_eq!(module_for_ordinal(PMWIN_BASE - 1),    "QUECALLS");
        assert_eq!(module_for_ordinal(PMGPI_BASE - 1),    "PMWIN");
        assert_eq!(module_for_ordinal(KBDCALLS_BASE - 1), "PMGPI");
        assert_eq!(module_for_ordinal(VIOCALLS_BASE - 1), "KBDCALLS");
        assert_eq!(module_for_ordinal(SESMGR_BASE - 1),   "VIOCALLS");
        assert_eq!(module_for_ordinal(NLS_BASE - 1),      "SESMGR");
        assert_eq!(module_for_ordinal(MSG_BASE - 1),      "NLS");
        assert_eq!(module_for_ordinal(MDM_BASE - 1),      "MSG");
        assert_eq!(module_for_ordinal(UCONV_BASE - 1),      "MDM");
        assert_eq!(module_for_ordinal(UCONV_BASE),          "UCONV");
        assert_eq!(module_for_ordinal(SO32DLL_BASE - 1),   "UCONV");
        assert_eq!(module_for_ordinal(SO32DLL_BASE),        "SO32DLL");
        assert_eq!(module_for_ordinal(TCP32DLL_BASE - 1),  "SO32DLL");
        assert_eq!(module_for_ordinal(TCP32DLL_BASE),       "TCP32DLL");
        assert_eq!(module_for_ordinal(MOUCALLS_BASE - 1),  "TCP32DLL");
        assert_eq!(module_for_ordinal(MOUCALLS_BASE),       "MOUCALLS");
        assert_eq!(module_for_ordinal(PMSHAPI_BASE - 1),   "MOUCALLS");
        assert_eq!(module_for_ordinal(PMSHAPI_BASE),        "PMSHAPI");
        assert_eq!(module_for_ordinal(HELPMGR_BASE - 1),   "PMSHAPI");
        assert_eq!(module_for_ordinal(HELPMGR_BASE),        "HELPMGR");
        assert_eq!(module_for_ordinal(PMSPL_BASE - 1),     "HELPMGR");
        assert_eq!(module_for_ordinal(PMSPL_BASE),          "PMSPL");
        assert_eq!(module_for_ordinal(PMCTLS_BASE - 1),    "PMSPL");
        assert_eq!(module_for_ordinal(PMCTLS_BASE),         "PMCTLS");
        assert_eq!(module_for_ordinal(WARPINE_BASE - 1),   "PMCTLS");
        assert_eq!(module_for_ordinal(WARPINE_BASE),       "WARPINE");
        assert_eq!(module_for_ordinal(PMDRAG_BASE - 1),    "WARPINE");
        assert_eq!(module_for_ordinal(PMDRAG_BASE),        "PMDRAG");
        assert_eq!(module_for_ordinal(SOM_BASE - 1),       "PMDRAG");
        assert_eq!(module_for_ordinal(SOM_BASE),           "SOM");
        assert_eq!(module_for_ordinal(REXX_BASE - 1),      "SOM");
        assert_eq!(module_for_ordinal(REXX_BASE),          "REXX");
        assert_eq!(module_for_ordinal(PMWP_BASE - 1),      "REXXAPI");
        assert_eq!(module_for_ordinal(STUB_AREA_SIZE - 1), "PMWP");
        assert_eq!(module_for_ordinal(STUB_AREA_SIZE),     "?");
    }

    #[test]
    fn test_pmshapi_and_helpmgr_ordinal_names() {
        assert_eq!(ordinal_to_name(PMSHAPI_BASE + 101), "PrfQueryProfileSize");
        assert_eq!(ordinal_to_name(PMSHAPI_BASE + 102), "PrfOpenProfile");
        assert_eq!(ordinal_to_name(PMSHAPI_BASE + 118), "PrfWriteProfileData");
        assert_eq!(ordinal_to_name(PMSHAPI_BASE + 104), "?");
        assert_eq!(ordinal_to_name(HELPMGR_BASE + 51), "WinCreateHelpInstance");
        assert_eq!(ordinal_to_name(HELPMGR_BASE + 56), "WinCreateHelpTable");
        assert_eq!(ordinal_to_name(HELPMGR_BASE + 74), "?");
        // MOUCALLS ends where PMSHAPI begins.
        assert_eq!(ordinal_to_name(MOUCALLS_BASE + 26), "MouDrawPtr");
        assert_eq!(arg_names_for_ordinal(PMSHAPI_BASE + 115).len(), 6);
        assert_eq!(arg_names_for_ordinal(HELPMGR_BASE + 54), &["hwndHelp", "hwndApp"]);
    }

    #[test]
    fn test_pmspl_and_pmctls_ordinal_names() {
        assert_eq!(ordinal_to_name(PMSPL_BASE + 301), "SplQmOpen");
        assert_eq!(ordinal_to_name(PMSPL_BASE + 399), "SplEnumQueue");
        assert_eq!(ordinal_to_name(PMSPL_BASE + 441), "SplEnumPrinter");
        assert_eq!(ordinal_to_name(PMSPL_BASE + 307), "?");
        assert_eq!(ordinal_to_name(PMCTLS_BASE + 2), "WinFontDlg");
        assert_eq!(ordinal_to_name(PMCTLS_BASE + 4), "WinFileDlg");
        assert_eq!(ordinal_to_name(PMCTLS_BASE + 6), "WinFreeFileDlgList");
        assert_eq!(ordinal_to_name(PMCTLS_BASE + 24), "WinRegisterCircularSlider");
        assert_eq!(ordinal_to_name(PMCTLS_BASE + 25), "?");
        // HELPMGR ends where PMSPL begins.
        assert_eq!(ordinal_to_name(HELPMGR_BASE + 56), "WinCreateHelpTable");
        assert_eq!(arg_names_for_ordinal(PMSPL_BASE + 441).len(), 9);
        assert_eq!(arg_names_for_ordinal(PMCTLS_BASE + 4), &["hwndP", "hwndO", "pfild"]);
    }

    #[test]
    fn test_moucalls_ordinal_names() {
        assert_eq!(ordinal_to_name(MOUCALLS_BASE + 17), "MouOpen");
        assert_eq!(ordinal_to_name(MOUCALLS_BASE + 20), "MouReadEventQue");
        assert_eq!(ordinal_to_name(MOUCALLS_BASE + 26), "MouDrawPtr");
        assert_eq!(ordinal_to_name(MOUCALLS_BASE + 14), "MouDeRegister");
        assert_eq!(ordinal_to_name(MOUCALLS_BASE + 5), "?");
        // TCP32DLL ends where MOUCALLS begins.
        assert_eq!(ordinal_to_name(TCP32DLL_BASE + 19), "socket");
    }

    #[test]
    fn test_mdm_ordinal_names() {
        assert_eq!(ordinal_to_name(MDM_BASE + 1), "mciSendCommand");
        assert_eq!(ordinal_to_name(MDM_BASE + 2), "mciSendString");
        assert_eq!(ordinal_to_name(MDM_BASE + 3), "mciFreeBlock");
        assert_eq!(ordinal_to_name(MDM_BASE + 4), "mciGetLastError");
        assert_eq!(ordinal_to_name(MDM_BASE + 99), "?");
    }

    /// Every ordinal dispatched in `api_dispatch.rs` must have a name entry.
    /// This is a regression guard — if a new ordinal is added to the dispatch
    /// table but not to `ordinal_to_name`, this test will catch it only for
    /// the DOSCALLS range (PMWIN/GPI/KBD/VIO have their own dispatch).
    #[test]
    fn test_all_dispatched_doscalls_are_named() {
        let dispatched: &[u32] = &[
            256, 257, 259, 271, 226, 270, 273, 281, 282, 229, 311, 234, 239,
            312, 283, 280, 235, 264, 265, 263, 223, 255, 274, 275, 220, 278,
            299, 304, 324, 326, 328, 329, 331, 333, 334, 335, 337, 339, 340,
            323, 342, 349, 352, 353, 572, 212, 209, 286, 354, 355, 356, 418,
            368, 378, 380, 381, 300, 301, 302, 305, 306, 291, 289, 318,
            322, 319, 321, 317, 258, 261, 279, 267, 219, 276, 277, 284, 231,
            325, 332, 230, 348, 382, 272, 260, 254, 210, 225, 292, 218, 285,
            357, 372, 428, 639, 415, 425, 426, 241, 243, 250, 221, 224, 110,
            8, 75, 84, 237, 238, 379,
        ];
        for &ord in dispatched {
            assert_ne!(ordinal_to_name(ord), "?",
                "ordinal {} is dispatched but has no name in ordinal_to_name()", ord);
        }
    }
}
