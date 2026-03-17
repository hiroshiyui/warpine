// SPDX-License-Identifier: GPL-3.0-only
//
// Structured API trace helpers.
//
// Provides two lookup functions used by the tracing spans in `api_dispatch`:
//   - `ordinal_to_name(ordinal)` → human-readable OS/2 API name (or "?")
//   - `module_for_ordinal(ordinal)` → owning DLL name ("DOSCALLS", "PMWIN", …)
//
// These are pure data functions — no I/O, no global state.  The tracing
// subscriber (configured in `main::init_logging`) decides whether and how
// the spans are rendered.

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

        // ── MDM / MMPM/2 (MDM_BASE + local ordinal) ───────────────────────
        o if (MDM_BASE..STUB_AREA_SIZE).contains(&o) => match o - MDM_BASE {
            1 => "mciSendCommand",
            2 => "mciSendString",
            3 => "mciFreeBlock",
            4 => "mciGetLastError",
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
    else if ordinal < STUB_AREA_SIZE { "MDM" }        // 10240–12287
    else                            { "?" }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(module_for_ordinal(STUB_AREA_SIZE - 1),"MDM");
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
            357, 372, 428, 639, 415, 425, 426, 241, 243, 250, 221, 224, 110,
            8, 75, 84,
        ];
        for &ord in dispatched {
            assert_ne!(ordinal_to_name(ord), "?",
                "ordinal {} is dispatched but has no name in ordinal_to_name()", ord);
        }
    }
}
