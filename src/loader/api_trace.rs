// SPDX-License-Identifier: GPL-3.0-only
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

// ── Ordinal → API name ────────────────────────────────────────────────────────

/// Map a warpine flat ordinal to its OS/2 API name.
///
/// Returns `"?"` for unknown ordinals.  The returned `&'static str` is safe to
/// embed as a `tracing` span field without any allocation.
pub fn ordinal_to_name(ordinal: u32) -> &'static str {
    match ordinal {
        // ── DOSCALLS (0–1023) ──────────────────────────────────────────────
        8   => "DosGetInfoSeg",
        75  => "DosQueryFileMode16",
        84  => "DosSetFileMode",
        110 => "DosForceDelete",
        209 => "DosSetMaxFH",
        210 => "DosSetVerify",
        212 => "DosError",
        218 => "DosSetFileInfo",
        219 => "DosSetPathInfo",
        220 => "DosSetDefaultDisk",
        221 => "DosSetFHState",
        223 => "DosQueryPathInfo",
        224 => "DosQueryHType",
        225 => "DosQueryVerify",
        226 => "DosDeleteDir",
        229 => "DosSleep",
        230 => "DosGetDateTime",
        231 => "DosDevConfig",
        234 => "DosExit",
        235 => "DosKillProcess",
        239 => "DosCreatePipe",
        241 => "DosConnectNPipe",
        243 => "DosCreateNPipe",
        250 => "DosSetNPHState",
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
        291 => "DosQueryCp",
        292 => "DosSetDateTime",
        299 => "DosAllocMem",
        300 => "DosAllocSharedMem",
        301 => "DosGetNamedSharedMem",
        302 => "DosGetSharedMem",
        304 => "DosFreeMem",
        305 => "DosSetMem",
        306 => "DosQueryMem",
        311 => "DosCreateThread",
        312 => "DosGetInfoBlocks",
        317 => "DosDebug",
        318 => "DosLoadModule",
        319 => "DosQueryModuleHandle",
        321 => "DosQueryProcAddr",
        322 => "DosFreeModule",
        323 => "DosQueryAppType",
        324 => "DosCreateEventSem",
        325 => "DosOpenEventSem",
        326 => "DosCloseEventSem",
        328 => "DosPostEventSem",
        329 => "DosWaitEventSem",
        331 => "DosCreateMutexSem",
        332 => "DosOpenMutexSem",
        333 => "DosCloseMutexSem",
        334 => "DosRequestMutexSem",
        335 => "DosReleaseMutexSem",
        337 => "DosCreateMuxWaitSem",
        339 => "DosCloseMuxWaitSem",
        340 => "DosWaitMuxWaitSem",
        342 => "DosDeleteMuxWaitSem",
        348 => "DosQuerySysInfo",
        349 => "DosWaitThread",
        352 => "DosGetResource",
        353 => "DosFreeResource",
        354 => "DosSetExceptionHandler",
        355 => "DosUnsetExceptionHandler",
        356 => "DosRaiseException",
        357 => "DosUnwindException",
        368 => "DosQuerySysState",
        372 => "DosEnumAttribute",
        373 => "DosQueryDBCSEnv",
        378 => "DosSetSignalExceptionFocus",
        380 => "DosEnterMustComplete",
        381 => "DosExitMustComplete",
        382 => "DosSetRelMaxFH",
        397 => "DosQueryCtryInfo",
        415 => "DosShutdown",
        418 => "DosAcknowledgeSignalException",
        425 => "DosFlatToSel",
        426 => "DosSelToFlat",
        428 => "DosSetFileLocks",
        572 => "DosQueryResourceSize",
        639 => "DosProtectSetFileLocks",

        // ── QUECALLS (1024 + local ordinal) ───────────────────────────────
        o if (1024..2048).contains(&o) => match o - 1024 {
            9  => "DosReadQueue",
            10 => "DosPurgeQueue",
            11 => "DosCloseQueue",
            12 => "DosQueryQueue",
            14 => "DosWriteQueue",
            15 => "DosOpenQueue",
            16 => "DosCreateQueue",
            _  => "?",
        },

        // ── NLS — National Language Support (NLS_BASE + local ordinal) ──────
        o if (NLS_BASE..MSG_BASE).contains(&o) => match o - NLS_BASE {
            5 => "NlsQueryCp",
            6 => "NlsQueryCtryInfo",
            7 => "NlsMapCase",
            8 => "NlsGetDBCSEv",
            _ => "?",
        },

        // ── MSG — OS/2 Message DLL (MSG_BASE + local ordinal) ────────────
        o if (MSG_BASE..MDM_BASE).contains(&o) => match o - MSG_BASE {
            3 => "DosPutMessage",
            6 => "DosGetMessage",
            _ => "?",
        },

        // ── MDM / MMPM/2 (MDM_BASE + local ordinal) ───────────────────────
        o if (MDM_BASE..UCONV_BASE).contains(&o) => match o - MDM_BASE {
            1 => "mciSendCommand",
            2 => "mciSendString",
            3 => "mciFreeBlock",
            4 => "mciGetLastError",
            _ => "?",
        },

        // ── UCONV — Unicode conversion (UCONV_BASE + local ordinal) ───────
        o if (UCONV_BASE..STUB_AREA_SIZE).contains(&o) => match o - UCONV_BASE {
            1 => "UniCreateUconvObject",
            2 => "UniFreeUconvObject",
            3 => "UniUconvToUcs",
            4 => "UniUconvFromUcs",
            6 => "UniMapCpToUcsCp",
            _ => "?",
        },

        // Higher subsystems (PMWIN, PMGPI, KBDCALLS, VIOCALLS, …) carry
        // their own internal dispatch tables; the top-level name is just "?"
        // here — callers use `module_for_ordinal` for the DLL name.
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
    else if ordinal < MSG_BASE      { "NLS" }         // 7168–8191
    else if ordinal < MDM_BASE      { "MSG" }         // 8192–10239
    else if ordinal < UCONV_BASE    { "MDM" }         // 10240–12287
    else if ordinal < STUB_AREA_SIZE { "UCONV" }      // 12288–16383
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
        229 => &["ulSleep"],
        230 => &["pDT"],
        231 => &["pDevInfo", "ulDevInfo"],
        234 => &["ulAction", "ulResult"],
        235 => &["ulAction", "PID"],
        239 => &["phfRead", "phfWrite", "ulPipeSize"],
        241 => &["hFile"],
        243 => &["pszName", "phPipe", "openMode", "pipeMode", "cbInBuf", "cbOutBuf"],
        250 => &["hPipe", "flState"],
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
        291 => &["cb", "arCP", "pcCP"],
        292 => &["pDT"],
        299 => &["ppb", "cb", "flAttr"],
        300 => &["ppb", "pszName", "cb", "flag"],
        301 => &["ppb", "pszName", "flag"],
        302 => &["pb", "flag"],
        304 => &["pb"],
        305 => &["pb", "cb", "flag"],
        306 => &["pb", "pcb", "pfl"],
        311 => &["ptid", "pfn", "param", "flag", "cbStack"],
        312 => &["pptib", "pppib"],
        317 => &["pdbgbuf"],
        318 => &["pszObj", "cbObj", "pszModule", "phmod"],
        319 => &["pszModuleName", "phmod"],
        321 => &["hmod", "ordinal", "pszName", "ppfn"],
        322 => &["hmod"],
        323 => &["pszName", "pFlags"],
        324 => &["pszName", "phev", "flAttr", "fState"],
        325 => &["pszName", "phev"],
        326 => &["hev"],
        328 => &["hev"],
        329 => &["hev", "ulTimeout"],
        331 => &["pszName", "phmtx", "flAttr", "fState"],
        332 => &["pszName", "phmtx"],
        333 => &["hmtx"],
        334 => &["hmtx", "ulTimeout"],
        335 => &["hmtx"],
        337 => &["pszName", "phmux", "cSemRec", "pSemRec", "flAttr"],
        339 => &["hmux"],
        340 => &["hmux", "ulTimeout", "pulUser"],
        342 => &["hmux", "flAttr"],
        348 => &["iStart", "iLast", "pBuf", "cbBuf"],
        349 => &["ptid", "option"],
        352 => &["hmod", "ulTypeID", "ulNameID", "ppb"],
        353 => &["pb"],
        354 => &["pERegRec"],
        355 => &["pERegRec"],
        356 => &["pexcept", "pRegRec"],
        357 => &["pHandler", "pTargetIP", "pexcept"],
        368 => &["EntityList", "EntityLevel", "PID", "TID", "pDataBuf", "cbBuf"],
        372 => &["ulRefType", "pvFile", "ulEntry", "pvBuf", "cbBuf", "pcbActual", "ulInfoLevel"],
        373 => &["cb", "pcc", "pBuf"],
        378 => &["fEnable", "pulTimes"],
        380 => &["pulNesting"],
        381 => &["pulNesting"],
        382 => &["pcbReqCount", "pcbCurMaxFH"],
        397 => &["cb", "pCountryCode", "pCountryInfo", "pcb"],
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
            5 => &["cb", "arCP", "pcCP"],
            6 => &["cb", "pCountryCode", "pCountryInfo", "pcb"],
            7 => &["cb", "pCC", "pString"],
            8 => &["cb", "pCC", "pBuf"],
            _ => &[],
        },

        // ── MSG ───────────────────────────────────────────────────────────
        o if (MSG_BASE..MDM_BASE).contains(&o) => match o - MSG_BASE {
            3 => &["pszPathName", "pszBuffer", "cbBuffer", "ulMsgNumber", "pTable", "cTable", "hFile"],
            6 => &["ulMsgNumber", "pTable", "cTable", "pBuffer", "cbBuffer", "pcbMsg", "pszFile"],
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
        o if (UCONV_BASE..STUB_AREA_SIZE).contains(&o) => match o - UCONV_BASE {
            1 => &["ucsName", "puobj"],
            2 => &["uobj"],
            3 => &["uobj", "ppInBuf", "pInBytesLeft", "ppOutBuf", "pOutCharsLeft", "pNumSubs"],
            4 => &["uobj", "ppInBuf", "pInCharsLeft", "ppOutBuf", "pOutBytesLeft", "pNumSubs"],
            6 => &["ulCodePage", "ucsCodePage", "n"],
            _ => &[],
        },

        _ => &[],
    }
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
    let parts: Vec<String> = arg_names.iter().enumerate().map(|(i, &aname)| {
        let val = args[i];
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
        assert_eq!(ordinal_to_name(2048), "?"); // PMWIN — higher subsystem
        assert_eq!(ordinal_to_name(9999), "?");
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
        assert_eq!(module_for_ordinal(UCONV_BASE - 1),    "MDM");
        assert_eq!(module_for_ordinal(UCONV_BASE),         "UCONV");
        assert_eq!(module_for_ordinal(STUB_AREA_SIZE - 1), "UCONV");
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
            368, 378, 380, 381, 300, 301, 302, 305, 306, 291, 289, 397, 318,
            322, 319, 321, 317, 258, 261, 279, 267, 219, 276, 277, 284, 231,
            325, 332, 230, 348, 382, 272, 260, 254, 210, 225, 292, 218, 285,
            357, 372, 373, 428, 639, 415, 425, 426, 241, 243, 250, 221, 224, 110,
            8, 75, 84,
        ];
        for &ord in dispatched {
            assert_ne!(ordinal_to_name(ord), "?",
                "ordinal {} is dispatched but has no name in ordinal_to_name()", ord);
        }
    }
}
