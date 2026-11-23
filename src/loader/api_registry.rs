// SPDX-License-Identifier: GPL-3.0-or-later
//
// API thunk registry: static sorted table of OS/2 API handlers.
//
// Every registered `ApiEntry` maps a flat Warpine ordinal to a handler fn.
// The table is sorted ascending by ordinal, enabling O(log n) binary-search
// dispatch via `find()`.
//
// `args[i]` = guest_read(ESP + 4 + i*4):
//   args[0] = first  stack arg  (ESP+4)
//   args[1] = second stack arg  (ESP+8)
//   …
//   args[8] = ninth  stack arg  (ESP+36)   ← DosDevIOCtl maximum
//
// PMWIN, PMGPI, KBDCALLS, and VIOCALLS are NOT in this registry; they use
// their own sub-dispatchers in api_dispatch.rs.
//
// To add a new API:
//   1. Implement `pub fn dos_xxx(&self, ...) -> u32` in the appropriate file.
//   2. Insert an `ApiEntry` in REGISTRY (keep sorted by ordinal).
//   3. The dispatch and tracing machinery picks it up automatically.

#![allow(clippy::too_many_arguments)]

use tracing::debug;

use super::constants::{NLS_BASE, MSG_BASE, MDM_BASE, UCONV_BASE, SO32DLL_BASE, TCP32DLL_BASE,
                       PMSHAPI_BASE, HELPMGR_BASE, PMSPL_BASE, PMCTLS_BASE, WARPINE_BASE, PMDRAG_BASE, SOM_BASE,
                       REXX_BASE, REXXAPI_BASE, PMWP_BASE,
                       module_base};
use super::pmspl::SplEnum;
use super::rexxapi::{Kind, RexxSignal};
use super::ApiResult;
use super::vm_backend::VcpuBackend;

// ── Public types ──────────────────────────────────────────────────────────────

/// A registered OS/2 API thunk entry.
pub struct ApiEntry {
    /// Flat Warpine ordinal (absolute, not module-relative).
    pub ordinal: u32,
    /// Owning DLL name ("DOSCALLS", "QUECALLS", "NLS", "MDM", …).
    pub module: &'static str,
    /// OS/2 API name (e.g. "DosOpen").
    pub name: &'static str,
    /// Number of u32 arguments in the OS/2 API signature.
    pub argc: u8,
    /// Type-erased handler function.
    ///
    /// Parameters:
    /// - `loader`  — `&Loader` for all subsystem managers and guest memory
    /// - `vcpu`    — `&mut dyn VcpuBackend` for APIs that read vCPU registers
    /// - `vcpu_id` — thread ID for APIs with per-thread ownership semantics
    /// - `args`    — pre-read snapshot: `args[i]` = guest stack word i
    pub(crate) handler: fn(&super::Loader, &mut dyn VcpuBackend, u32, [u32; 10]) -> ApiResult,
}

// ── Public functions ──────────────────────────────────────────────────────────

/// Binary-search the registry for a handler by flat ordinal.
/// Returns `None` for ordinals not covered by this registry
/// (e.g., PMWIN/PMGPI/KBDCALLS/VIOCALLS — handled by sub-dispatchers).
#[inline]
pub fn find(ordinal: u32) -> Option<&'static ApiEntry> {
    REGISTRY
        .binary_search_by_key(&ordinal, |e| e.ordinal)
        .ok()
        .map(|i| &REGISTRY[i])
}

/// Look up an entry by module name and function name (linear search).
/// Used to resolve `ExternalName` LX fixups and `DosQueryProcAddr` by name
/// for registry-based modules. Returns the module's own ordinal (the
/// registry's flat ordinal less the module base — what `resolve_import`
/// takes), `None` if not found.
pub fn find_by_module_and_name(module: &str, name: &str) -> Option<u32> {
    let base = module_base(module)?;
    REGISTRY.iter()
        .find(|e| e.module == module && e.name == name)
        .map(|e| e.ordinal - base)
}

/// Iterate all registered entries (for compatibility reports, tests, etc.).
pub fn all() -> &'static [ApiEntry] {
    REGISTRY
}

// ── Registry table ────────────────────────────────────────────────────────────
//
// INVARIANT: entries must be sorted strictly ascending by `ordinal`.
// Verified by `test_registry_sorted` below.

static REGISTRY: &[ApiEntry] = &[
    // ── 16-bit shim ordinals (DOSCALLS) ──────────────────────────────────
    ApiEntry { ordinal: 8,   module: "DOSCALLS", name: "DosGetInfoSeg",    argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.dos_get_info_seg(i,a[0],a[1])) },
    ApiEntry { ordinal: 75,  module: "DOSCALLS", name: "DosQueryFileMode16", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_file_mode_16(a[0],a[1])) },
    ApiEntry { ordinal: 84,  module: "DOSCALLS", name: "DosSetFileMode",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_file_mode(a[0],a[1])) },
    ApiEntry { ordinal: 110, module: "DOSCALLS", name: "DosForceDelete",   argc: 1,
               handler: |l,_v,_i,a| { debug!("DosForceDelete stub"); ApiResult::Normal(l.dos_delete(a[0])) } },
    ApiEntry { ordinal: 111, module: "DOSCALLS", name: "DosKillThread",    argc: 1,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_kill_thread(id,a[0])) },

    // ── DOSCALLS — init / control ─────────────────────────────────────────
    ApiEntry { ordinal: 209, module: "DOSCALLS", name: "DosSetMaxFH",      argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_max_fh(a[0])) },
    ApiEntry { ordinal: 210, module: "DOSCALLS", name: "DosSetVerify",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_verify(a[0])) },
    ApiEntry { ordinal: 211, module: "DOSCALLS", name: "DosErrClass", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_err_class(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 212, module: "DOSCALLS", name: "DosError",         argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_error(a[0])) },

    // ── DOSCALLS — file metadata ──────────────────────────────────────────
    ApiEntry { ordinal: 218, module: "DOSCALLS", name: "DosSetFileInfo",   argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_file_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 219, module: "DOSCALLS", name: "DosSetPathInfo",   argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_path_info(a[0],a[1],a[2],a[3],a[4])) },

    // ── DOSCALLS — drives / directories ──────────────────────────────────
    ApiEntry { ordinal: 220, module: "DOSCALLS", name: "DosSetDefaultDisk", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_default_disk(a[0])) },
    ApiEntry { ordinal: 221, module: "DOSCALLS", name: "DosSetFHState",    argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_fh_state(a[0],a[1])) },
    ApiEntry { ordinal: 223, module: "DOSCALLS", name: "DosQueryPathInfo", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_path_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 224, module: "DOSCALLS", name: "DosQueryHType",    argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_h_type(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 225, module: "DOSCALLS", name: "DosQueryVerify",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_verify(a[0])) },
    ApiEntry { ordinal: 226, module: "DOSCALLS", name: "DosDeleteDir",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_delete_dir(a[0])) },
    ApiEntry { ordinal: 227, module: "DOSCALLS", name: "DosScanEnv",       argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_scan_env(a[0],a[1])) },
    // DosSearchPath(flag, pszPathOrName, pszFilename, pBuf, cbBuf) (`modpath.rs`)
    ApiEntry { ordinal: 228, module: "DOSCALLS", name: "DosSearchPath",    argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_search_path(a[0],a[1],a[2],a[3],a[4])) },

    // ── DOSCALLS — time / misc ────────────────────────────────────────────
    ApiEntry { ordinal: 229, module: "DOSCALLS", name: "DosSleep",         argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_sleep(a[0])) },
    ApiEntry { ordinal: 230, module: "DOSCALLS", name: "DosGetDateTime",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_date_time(a[0])) },
    ApiEntry { ordinal: 231, module: "DOSCALLS", name: "DosDevConfig",     argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_dev_config(a[0],a[1])) },
    // DosEnterCritSec / DosExitCritSec: the other threads are kicked out of
    // guest code and held until the section ends (`critsec.rs`).
    ApiEntry { ordinal: 232, module: "DOSCALLS", name: "DosEnterCritSec",  argc: 0,
               handler: |l,_v,id,_a| ApiResult::Normal(l.dos_enter_crit_sec(id)) },
    ApiEntry { ordinal: 233, module: "DOSCALLS", name: "DosExitCritSec",   argc: 0,
               handler: |l,_v,id,_a| ApiResult::Normal(l.dos_exit_crit_sec(id)) },

    // DosExit(action, result): EXIT_THREAD ends the calling thread (the
    // vCPU loop ends it; the last thread ends the process), EXIT_PROCESS
    // runs the exit list (DosExitList) and ends the process.
    ApiEntry { ordinal: 234, module: "DOSCALLS", name: "DosExit",          argc: 2,
               handler: |l,_v,id,a| l.dos_exit(id, a[0], a[1]) },

    // ── DOSCALLS — processes / priorities ────────────────────────────────
    ApiEntry { ordinal: 235, module: "DOSCALLS", name: "DosKillProcess",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_kill_process(a[0],a[1])) },
    ApiEntry { ordinal: 236, module: "DOSCALLS", name: "DosSetPriority",   argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_priority(a[0],a[1],a[2],a[3])) },
    // DosResumeThread / DosSuspendThread: suspend counts (`threads.rs`); a
    // thread spinning in guest code is kicked out of it at once.
    ApiEntry { ordinal: 237, module: "DOSCALLS", name: "DosResumeThread",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_resume_thread(a[0])) },
    ApiEntry { ordinal: 238, module: "DOSCALLS", name: "DosSuspendThread", argc: 1,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_suspend_thread(id,a[0])) },

    // ── DOSCALLS — pipes ──────────────────────────────────────────────────
    ApiEntry { ordinal: 239, module: "DOSCALLS", name: "DosCreatePipe",    argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_pipe(a[0],a[1],a[2])) },
    // Named pipes (`npipe_api.rs`).
    ApiEntry { ordinal: 240, module: "DOSCALLS", name: "DosCallNPipe",     argc: 7,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_call_npipe(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) },
    ApiEntry { ordinal: 241, module: "DOSCALLS", name: "DosConnectNPipe",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_connect_npipe(a[0])) },
    ApiEntry { ordinal: 242, module: "DOSCALLS", name: "DosDisConnectNPipe", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_disconnect_npipe(a[0])) },
    ApiEntry { ordinal: 243, module: "DOSCALLS", name: "DosCreateNPipe",   argc: 7,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_npipe(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) },
    ApiEntry { ordinal: 244, module: "DOSCALLS", name: "DosPeekNPipe",     argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_peek_npipe(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: 245, module: "DOSCALLS", name: "DosQueryNPHState", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_nph_state(a[0],a[1])) },
    ApiEntry { ordinal: 246, module: "DOSCALLS", name: "DosRawReadNPipe",  argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_raw_read_npipe(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 247, module: "DOSCALLS", name: "DosRawWriteNPipe", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_raw_write_npipe(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 248, module: "DOSCALLS", name: "DosQueryNPipeInfo", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_npipe_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 249, module: "DOSCALLS", name: "DosQueryNPipeSemState", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_npipe_sem_state(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 250, module: "DOSCALLS", name: "DosSetNPHState",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_nph_state(a[0],a[1])) },
    ApiEntry { ordinal: 251, module: "DOSCALLS", name: "DosSetNPipeSem",   argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_npipe_sem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 252, module: "DOSCALLS", name: "DosTransactNPipe", argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_transact_npipe(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: 253, module: "DOSCALLS", name: "DosWaitNPipe",     argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_wait_npipe(a[0],a[1])) },

    // ── DOSCALLS — file I/O ───────────────────────────────────────────────
    ApiEntry { ordinal: 254, module: "DOSCALLS", name: "DosResetBuffer",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_reset_buffer(a[0])) },
    ApiEntry { ordinal: 255, module: "DOSCALLS", name: "DosSetCurrentDir", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_current_dir(a[0])) },
    // DosSetFilePtr args: hf, ib (signed i32), method, pibNew
    ApiEntry { ordinal: 256, module: "DOSCALLS", name: "DosSetFilePtr",    argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_file_ptr(a[0],a[1] as i32,a[2],a[3])) },
    ApiEntry { ordinal: 257, module: "DOSCALLS", name: "DosClose",         argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close(a[0])) },
    ApiEntry { ordinal: 258, module: "DOSCALLS", name: "DosCopy",          argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_copy(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 259, module: "DOSCALLS", name: "DosDelete",        argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_delete(a[0])) },
    ApiEntry { ordinal: 260, module: "DOSCALLS", name: "DosDupHandle",     argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_dup_handle(a[0],a[1])) },
    ApiEntry { ordinal: 261, module: "DOSCALLS", name: "DosEditName",      argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_edit_name(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 263, module: "DOSCALLS", name: "DosFindClose",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_find_close(a[0])) },
    ApiEntry { ordinal: 264, module: "DOSCALLS", name: "DosFindFirst",     argc: 7,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_find_first(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) },
    ApiEntry { ordinal: 265, module: "DOSCALLS", name: "DosFindNext",      argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_find_next(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 267, module: "DOSCALLS", name: "DOS16REQUESTVDD",  argc: 3,
               handler: |_l,_v,_i,_a| { debug!("DOS16REQUESTVDD stub"); ApiResult::Normal(0) } },
    ApiEntry { ordinal: 270, module: "DOSCALLS", name: "DosCreateDir",     argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_dir(a[0])) },
    ApiEntry { ordinal: 271, module: "DOSCALLS", name: "DosMove",          argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_move(a[0],a[1])) },
    ApiEntry { ordinal: 272, module: "DOSCALLS", name: "DosSetFileSize",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_file_size(a[0],a[1])) },
    // DosOpen: pszName, phf, pulAction, [cbFile@16 unused], [ulAttr@20 unused], fsOpenFlags, fsOpenMode
    ApiEntry { ordinal: 273, module: "DOSCALLS", name: "DosOpen",          argc: 7,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_open(a[0],a[1],a[2],a[5],a[6])) },
    ApiEntry { ordinal: 274, module: "DOSCALLS", name: "DosQueryCurrentDir",  argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_current_dir(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 275, module: "DOSCALLS", name: "DosQueryCurrentDisk", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_current_disk(a[0],a[1])) },
    ApiEntry { ordinal: 276, module: "DOSCALLS", name: "DosQueryFHState",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_fh_state(a[0],a[1])) },
    ApiEntry { ordinal: 277, module: "DOSCALLS", name: "DosQueryFSAttach", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_fs_attach(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 278, module: "DOSCALLS", name: "DosQueryFSInfo",   argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_fs_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 279, module: "DOSCALLS", name: "DosQueryFileInfo", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_file_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 280, module: "DOSCALLS", name: "DosWaitChild",     argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_wait_child(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 281, module: "DOSCALLS", name: "DosRead",          argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_read(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 282, module: "DOSCALLS", name: "DosWrite",         argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_write(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 283, module: "DOSCALLS", name: "DosExecPgm",       argc: 7,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_exec_pgm(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) },
    ApiEntry { ordinal: 284, module: "DOSCALLS", name: "DosDevIOCtl",      argc: 9,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_dev_ioctl(a[0],a[1],a[2],a[3],a[4],a[5],a[6],a[7],a[8])) },
    ApiEntry { ordinal: 285, module: "DOSCALLS", name: "DosFSCtl",         argc: 7,
               handler: |_l,_v,_i,_a| { debug!("DosFSCtl stub"); ApiResult::Normal(0) } },
    ApiEntry { ordinal: 286, module: "DOSCALLS", name: "DosBeep",          argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_beep(a[0],a[1])) },

    // ── DOSCALLS — codepage / country ─────────────────────────────────────
    ApiEntry { ordinal: 289, module: "DOSCALLS", name: "DosSetProcessCp",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_process_cp(a[0])) },
    // DosStopTimer(htimer) (`dostimer.rs`)
    ApiEntry { ordinal: 290, module: "DOSCALLS", name: "DosStopTimer",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_stop_timer(a[0])) },
    ApiEntry { ordinal: 291, module: "DOSCALLS", name: "DosQueryCp",       argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_cp(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 292, module: "DOSCALLS", name: "DosSetDateTime",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_date_time(a[0])) },

    // DosExitList(ordercode, pfn): EXLST_EXIT ends the running exit-list
    // routine; the vCPU loop calls the next one (`exitlist.rs`).
    ApiEntry { ordinal: 296, module: "DOSCALLS", name: "DosExitList",      argc: 2,
               handler: |l,_v,id,a| l.dos_exit_list(id, a[0], a[1]) },

    // ── DOSCALLS — memory ─────────────────────────────────────────────────
    ApiEntry { ordinal: 299, module: "DOSCALLS", name: "DosAllocMem",      argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_alloc_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 300, module: "DOSCALLS", name: "DosAllocSharedMem", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_alloc_shared_mem(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 301, module: "DOSCALLS", name: "DosGetNamedSharedMem", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_named_shared_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 302, module: "DOSCALLS", name: "DosGetSharedMem",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_shared_mem(a[0],a[1])) },
    ApiEntry { ordinal: 303, module: "DOSCALLS", name: "DosGiveSharedMem", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_give_shared_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 304, module: "DOSCALLS", name: "DosFreeMem",       argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_free_mem(a[0])) },
    ApiEntry { ordinal: 305, module: "DOSCALLS", name: "DosSetMem",        argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 306, module: "DOSCALLS", name: "DosQueryMem",      argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_mem(a[0],a[1],a[2])) },

    // ── DOSCALLS — threading ──────────────────────────────────────────────
    // DosCreateThread: ptid, pfn, param, flags (CREATE_SUSPENDED), cbStack
    ApiEntry { ordinal: 311, module: "DOSCALLS", name: "DosCreateThread",  argc: 5,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_create_thread(id,a[0],a[1],a[2],a[3],a[4])) },
    // DosGetInfoBlocks: needs live vcpu to read FS.base for TIB address
    ApiEntry { ordinal: 312, module: "DOSCALLS", name: "DosGetInfoBlocks", argc: 2,
               handler: |l,v,_i,a| ApiResult::Normal(l.dos_get_info_blocks(v,a[0],a[1])) },

    // ── DOSCALLS — module loading ─────────────────────────────────────────
    ApiEntry { ordinal: 317, module: "DOSCALLS", name: "DosDebug",         argc: 1,
               handler: |_l,_v,_i,_a| { debug!("DosDebug stub"); ApiResult::Normal(87) } },
    ApiEntry { ordinal: 318, module: "DOSCALLS", name: "DosLoadModule",    argc: 4,
               handler: |l,_v,_i,a| l.dos_load_module(a[0],a[1],a[2],a[3]) },
    ApiEntry { ordinal: 319, module: "DOSCALLS", name: "DosQueryModuleHandle", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_module_handle(a[0],a[1])) },
    // DosQueryModuleName(hmod, cbName, pch) (`modpath.rs`)
    ApiEntry { ordinal: 320, module: "DOSCALLS", name: "DosQueryModuleName", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_module_name(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 321, module: "DOSCALLS", name: "DosQueryProcAddr", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_proc_addr(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 322, module: "DOSCALLS", name: "DosFreeModule",    argc: 1,
               handler: |l,_v,_i,a| l.dos_free_module(a[0]) },
    ApiEntry { ordinal: 323, module: "DOSCALLS", name: "DosQueryAppType",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_app_type(a[0],a[1])) },

    // ── DOSCALLS — event semaphores ───────────────────────────────────────
    ApiEntry { ordinal: 324, module: "DOSCALLS", name: "DosCreateEventSem", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_event_sem(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 325, module: "DOSCALLS", name: "DosOpenEventSem",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_open_event_sem(a[0],a[1])) },
    ApiEntry { ordinal: 326, module: "DOSCALLS", name: "DosCloseEventSem", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close_event_sem(a[0])) },
    ApiEntry { ordinal: 327, module: "DOSCALLS", name: "DosResetEventSem", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_reset_event_sem(a[0],a[1])) },
    ApiEntry { ordinal: 328, module: "DOSCALLS", name: "DosPostEventSem",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_post_event_sem(a[0])) },
    ApiEntry { ordinal: 329, module: "DOSCALLS", name: "DosWaitEventSem",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_wait_event_sem(a[0],a[1])) },
    ApiEntry { ordinal: 330, module: "DOSCALLS", name: "DosQueryEventSem", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_event_sem(a[0],a[1])) },

    // ── DOSCALLS — mutex semaphores ───────────────────────────────────────
    ApiEntry { ordinal: 331, module: "DOSCALLS", name: "DosCreateMutexSem", argc: 4,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_create_mutex_sem(id,a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 332, module: "DOSCALLS", name: "DosOpenMutexSem",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_open_mutex_sem(a[0],a[1])) },
    ApiEntry { ordinal: 333, module: "DOSCALLS", name: "DosCloseMutexSem", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close_mutex_sem(a[0])) },
    ApiEntry { ordinal: 334, module: "DOSCALLS", name: "DosRequestMutexSem", argc: 2,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_request_mutex_sem(id,a[0],a[1])) },
    ApiEntry { ordinal: 335, module: "DOSCALLS", name: "DosReleaseMutexSem", argc: 1,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_release_mutex_sem(id,a[0])) },
    ApiEntry { ordinal: 336, module: "DOSCALLS", name: "DosQueryMutexSem", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_mutex_sem(a[0],a[1],a[2],a[3])) },

    // ── DOSCALLS — muxwait semaphores ─────────────────────────────────────
    ApiEntry { ordinal: 337, module: "DOSCALLS", name: "DosCreateMuxWaitSem", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_mux_wait_sem(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 338, module: "DOSCALLS", name: "DosOpenMuxWaitSem",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_open_mux_wait_sem(a[0],a[1])) },
    ApiEntry { ordinal: 339, module: "DOSCALLS", name: "DosCloseMuxWaitSem",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close_mux_wait_sem(a[0])) },
    ApiEntry { ordinal: 340, module: "DOSCALLS", name: "DosWaitMuxWaitSem",   argc: 3,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_wait_mux_wait_sem(id,a[0],a[1],a[2])) },
    ApiEntry { ordinal: 341, module: "DOSCALLS", name: "DosAddMuxWaitSem",    argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_add_mux_wait_sem(a[0],a[1])) },
    ApiEntry { ordinal: 342, module: "DOSCALLS", name: "DosDeleteMuxWaitSem", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_delete_mux_wait_sem(a[0],a[1])) },
    ApiEntry { ordinal: 343, module: "DOSCALLS", name: "DosQueryMuxWaitSem",  argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_mux_wait_sem(a[0],a[1],a[2],a[3])) },

    // ── DOSCALLS — memory suballocation (`suballoc.rs`) ───────────────────
    ApiEntry { ordinal: 344, module: "DOSCALLS", name: "DosSubSetMem",     argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_sub_set_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 345, module: "DOSCALLS", name: "DosSubAllocMem",   argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_sub_alloc_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 346, module: "DOSCALLS", name: "DosSubFreeMem",    argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_sub_free_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 347, module: "DOSCALLS", name: "DosSubUnsetMem",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_sub_unset_mem(a[0])) },

    // ── DOSCALLS — system info ────────────────────────────────────────────
    ApiEntry { ordinal: 348, module: "DOSCALLS", name: "DosQuerySysInfo",  argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_sys_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 349, module: "DOSCALLS", name: "DosWaitThread",    argc: 2,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_wait_thread(id,a[0],a[1])) },

    // ── DOSCALLS — timers (`dostimer.rs`) ─────────────────────────────────
    // DosAsyncTimer / DosStartTimer(msec, hsem, phtimer)
    ApiEntry { ordinal: 350, module: "DOSCALLS", name: "DosAsyncTimer",    argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_start_timer(a[0],a[1],a[2],false)) },
    ApiEntry { ordinal: 351, module: "DOSCALLS", name: "DosStartTimer",    argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_start_timer(a[0],a[1],a[2],true)) },

    // ── DOSCALLS — resources ──────────────────────────────────────────────
    ApiEntry { ordinal: 352, module: "DOSCALLS", name: "DosGetResource",   argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_resource(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 353, module: "DOSCALLS", name: "DosFreeResource",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_free_resource(a[0])) },

    // ── DOSCALLS — exception handling ─────────────────────────────────────
    ApiEntry { ordinal: 354, module: "DOSCALLS", name: "DosSetExceptionHandler",   argc: 1,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_set_exception_handler(id,a[0])) },
    ApiEntry { ordinal: 355, module: "DOSCALLS", name: "DosUnsetExceptionHandler", argc: 1,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_unset_exception_handler(id,a[0])) },
    ApiEntry { ordinal: 356, module: "DOSCALLS", name: "DosRaiseException", argc: 1,
               handler: |l,v,id,a| l.dos_raise_exception(v,id,a[0]) },
    ApiEntry { ordinal: 357, module: "DOSCALLS", name: "DosUnwindException", argc: 3,
               handler: |l,_v,id,a| l.dos_unwind_exception(id,a[0],a[1],a[2]) },

    // ── DOSCALLS — process state / signals ───────────────────────────────
    ApiEntry { ordinal: 360, module: "DOSCALLS", name: "DosQueryModFromEIP", argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_mod_from_eip(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: 362, module: "DOSCALLS", name: "DosTmrQueryFreq", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_tmr_query_freq(a[0])) },
    ApiEntry { ordinal: 363, module: "DOSCALLS", name: "DosTmrQueryTime", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_tmr_query_time(a[0])) },
    ApiEntry { ordinal: 368, module: "DOSCALLS", name: "DosQuerySysState", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_sys_state(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 372, module: "DOSCALLS", name: "DosEnumAttribute", argc: 7,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_enum_attribute(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) },
    // Signal exceptions (`signals.rs`): delivered to thread 1's handlers.
    ApiEntry { ordinal: 378, module: "DOSCALLS", name: "DosSetSignalExceptionFocus", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_signal_exception_focus(a[0],a[1])) },
    ApiEntry { ordinal: 379, module: "DOSCALLS", name: "DosSendSignalException", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_send_signal_exception(a[0],a[1])) },
    ApiEntry { ordinal: 380, module: "DOSCALLS", name: "DosEnterMustComplete", argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.dos_must_complete(i,a[0],true)) },
    ApiEntry { ordinal: 381, module: "DOSCALLS", name: "DosExitMustComplete",  argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.dos_must_complete(i,a[0],false)) },
    ApiEntry { ordinal: 382, module: "DOSCALLS", name: "DosSetRelMaxFH",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_rel_max_fh(a[0],a[1])) },
    ApiEntry { ordinal: 415, module: "DOSCALLS", name: "DosShutdown",      argc: 1,
               handler: |_l,_v,_i,_a| { debug!("DosShutdown stub"); ApiResult::Normal(0) } },
    ApiEntry { ordinal: 418, module: "DOSCALLS", name: "DosAcknowledgeSignalException", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_acknowledge_signal_exception(a[0])) },

    // ── DOSCALLS — segment / selector utilities ───────────────────────────
    ApiEntry { ordinal: 425, module: "DOSCALLS", name: "DosFlatToSel",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_flat_to_sel(a[0])) },
    ApiEntry { ordinal: 426, module: "DOSCALLS", name: "DosSelToFlat",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_sel_to_flat(a[0])) },
    ApiEntry { ordinal: 428, module: "DOSCALLS", name: "DosSetFileLocks",  argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_file_locks(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 429, module: "DOSCALLS", name: "DosCancelLockRequest", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_cancel_lock_request(a[0],a[1])) },
    ApiEntry { ordinal: 454, module: "DOSCALLS", name: "DosAllocThreadLocalMemory", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_alloc_thread_local_memory(a[0],a[1])) },
    ApiEntry { ordinal: 455, module: "DOSCALLS", name: "DosFreeThreadLocalMemory", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_free_thread_local_memory(a[0])) },
    ApiEntry { ordinal: 460, module: "DOSCALLS", name: "DosVerifyPidTid", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_verify_pid_tid(a[0],a[1])) },
    ApiEntry { ordinal: 572, module: "DOSCALLS", name: "DosQueryResourceSize", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_resource_size(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 586, module: "DOSCALLS", name: "DosQueryProcType", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_proc_type(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 621, module: "DOSCALLS", name: "DosProtectSetFilePtr", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[4],|| l.dos_set_file_ptr(a[0],a[1] as i32,a[2],a[3]))) },
    ApiEntry { ordinal: 636, module: "DOSCALLS", name: "DosProtectEnumAttribute", argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(if a[0] == 0 { l.dos_protect(l.guest_read::<u32>(a[1]).unwrap_or(0),a[7],|| l.dos_enum_attribute(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) } else { l.dos_enum_attribute(a[0],a[1],a[2],a[3],a[4],a[5],a[6]) }) },
    ApiEntry { ordinal: 637, module: "DOSCALLS", name: "DosProtectOpen", argc: 9,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect_open(a[0],a[1],a[2],a[5],a[6],a[8])) },
    ApiEntry { ordinal: 638, module: "DOSCALLS", name: "DosProtectClose", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[1],|| l.dos_close(a[0]))) },
    ApiEntry { ordinal: 639, module: "DOSCALLS", name: "DosProtectSetFileLocks", argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect_set_file_locks(a[0],a[1],a[2],a[3],a[4],a[5])) },

    // ── DOSCALLS — extended LIBPATH ───────────────────────────────────────
    ApiEntry { ordinal: 640, module: "DOSCALLS", name: "DosProtectSetFileSize", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[2],|| l.dos_set_file_size(a[0],a[1]))) },
    ApiEntry { ordinal: 641, module: "DOSCALLS", name: "DosProtectRead", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[4],|| l.dos_read(a[0],a[1],a[2],a[3]))) },
    ApiEntry { ordinal: 642, module: "DOSCALLS", name: "DosProtectWrite", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[4],|| l.dos_write(a[0],a[1],a[2],a[3]))) },
    ApiEntry { ordinal: 643, module: "DOSCALLS", name: "DosProtectSetFileInfo", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[4],|| l.dos_set_file_info(a[0],a[1],a[2],a[3]))) },
    ApiEntry { ordinal: 644, module: "DOSCALLS", name: "DosProtectSetFHState", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[2],|| l.dos_set_fh_state(a[0],a[1]))) },
    ApiEntry { ordinal: 645, module: "DOSCALLS", name: "DosProtectQueryFHState", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[2],|| l.dos_query_fh_state(a[0],a[1]))) },
    ApiEntry { ordinal: 646, module: "DOSCALLS", name: "DosProtectQueryFileInfo", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect(a[0],a[4],|| l.dos_query_file_info(a[0],a[1],a[2],a[3]))) },
    ApiEntry { ordinal: 873, module: "DOSCALLS", name: "DosSetExtLIBPATH",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_ext_libpath(a[0],a[1])) },
    ApiEntry { ordinal: 874, module: "DOSCALLS", name: "DosQueryExtLIBPATH", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_ext_libpath(a[0],a[1])) },

    // ── QUECALLS (base 1024) ──────────────────────────────────────────────
    ApiEntry { ordinal: 1024 + 9,  module: "QUECALLS", name: "DosReadQueue",   argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_read_queue(a[0],a[1],a[2],a[3],a[4],a[5],a[6],a[7])) },
    ApiEntry { ordinal: 1024 + 10, module: "QUECALLS", name: "DosPurgeQueue",  argc: 1,
               handler: |l,_v,_i,a| { l.dos_purge_queue(a[0]); ApiResult::Normal(0) } },
    ApiEntry { ordinal: 1024 + 11, module: "QUECALLS", name: "DosCloseQueue",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close_queue(a[0])) },
    ApiEntry { ordinal: 1024 + 12, module: "QUECALLS", name: "DosQueryQueue",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_queue(a[0],a[1])) },
    ApiEntry { ordinal: 1024 + 13, module: "QUECALLS", name: "DosPeekQueue",   argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_peek_queue(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) },
    ApiEntry { ordinal: 1024 + 14, module: "QUECALLS", name: "DosWriteQueue",  argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_write_queue(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 1024 + 15, module: "QUECALLS", name: "DosOpenQueue",   argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_open_queue(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 1024 + 16, module: "QUECALLS", name: "DosCreateQueue", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_queue(a[0],a[1],a[2])) },

    // ── NLS — National Language Support (base NLS_BASE = 7168) ───────────
    // The 32-bit NLS calls, at the ordinals of the toolkit's <bseord.h>
    // (ORD_DOS32QUERYCTRYINFO … ORD_DOS32QUERYCOLLATE) and os2386.lib.
    ApiEntry { ordinal: NLS_BASE + 5, module: "NLS", name: "DosQueryCtryInfo", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_ctry_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: NLS_BASE + 6, module: "NLS", name: "DosQueryDBCSEnv",  argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_dbcs_env(a[0],a[1],a[2])) },
    ApiEntry { ordinal: NLS_BASE + 7, module: "NLS", name: "DosMapCase",       argc: 3,
               handler: |l,_v,_i,a| {
                   // a[0]=cb, a[1]=pcc, a[2]=pch
                   let cp = l.nls_codepage(a[1]);
                   l.map_case_guest_buf(a[2], a[0], cp);
                   ApiResult::Normal(0)
               } },
    ApiEntry { ordinal: NLS_BASE + 8, module: "NLS", name: "DosQueryCollate",  argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_collate(a[0],a[1],a[2],a[3])) },

    // ── MSG — OS/2 Message DLL (base MSG_BASE = 8192) ────────────────────
    // 32-bit ordinals of Open Watcom's os2386.lib (msgcalls.rs). MSG.6 is
    // DOS32TRUEGETMESSAGE: the library's DosGetMessage (4OS2 uses it)
    // passes the message segment first, then DosGetMessage's arguments.
    ApiEntry { ordinal: MSG_BASE + 4, module: "MSG", name: "DosInsertMessage",  argc: 7,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_insert_message(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) },
    ApiEntry { ordinal: MSG_BASE + 5, module: "MSG", name: "DosPutMessage",     argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_put_message(a[0],a[1],a[2])) },
    ApiEntry { ordinal: MSG_BASE + 6, module: "MSG", name: "DosGetMessage",     argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_message(a[0],a[1],a[2],a[3],a[4],a[5],a[6],a[7])) },
    ApiEntry { ordinal: MSG_BASE + 8, module: "MSG", name: "DosQueryMessageCP", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_message_cp(a[0],a[1],a[2],a[3])) },

    // ── MDM — MMPM/2 Media Device Manager (base MDM_BASE = 10240) ────────
    ApiEntry { ordinal: MDM_BASE + 1, module: "MDM", name: "mciSendCommand",   argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.mci_send_command(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: MDM_BASE + 2, module: "MDM", name: "mciSendString",    argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.mci_send_string(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: MDM_BASE + 3, module: "MDM", name: "mciFreeBlock",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.mci_free_block(a[0])) },
    ApiEntry { ordinal: MDM_BASE + 4, module: "MDM", name: "mciGetLastError",  argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.mci_get_last_error(a[0],a[1],a[2])) },

    // ── UCONV — Unicode conversion (base UCONV_BASE = 12288) ─────────────
    ApiEntry { ordinal: UCONV_BASE + 1, module: "UCONV", name: "UniCreateUconvObject", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.uni_create_uconv_object(a[0],a[1])) },
    ApiEntry { ordinal: UCONV_BASE + 2, module: "UCONV", name: "UniFreeUconvObject",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.uni_free_uconv_object(a[0])) },
    ApiEntry { ordinal: UCONV_BASE + 3, module: "UCONV", name: "UniUconvToUcs",        argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.uni_uconv_to_ucs(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: UCONV_BASE + 4, module: "UCONV", name: "UniUconvFromUcs",      argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.uni_uconv_from_ucs(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: UCONV_BASE + 6, module: "UCONV", name: "UniMapCpToUcsCp",     argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.uni_map_cp_to_ucs_cp(a[0],a[1],a[2])) },

    // ── SO32DLL — OS/2 TCP/IP socket API (base SO32DLL_BASE = 13312) ─────────
    ApiEntry { ordinal: SO32DLL_BASE + 1,  module: "SO32DLL",  name: "accept",           argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_accept(a[0],a[1],a[2])) },
    ApiEntry { ordinal: SO32DLL_BASE + 2,  module: "SO32DLL",  name: "bind",             argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_bind(a[0],a[1],a[2])) },
    ApiEntry { ordinal: SO32DLL_BASE + 3,  module: "SO32DLL",  name: "connect",          argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_connect(a[0],a[1],a[2])) },
    ApiEntry { ordinal: SO32DLL_BASE + 4,  module: "SO32DLL",  name: "gethostid",        argc: 0,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.so_gethostid()) },
    ApiEntry { ordinal: SO32DLL_BASE + 5,  module: "SO32DLL",  name: "gethostname",      argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_gethostname(a[0],a[1])) },
    ApiEntry { ordinal: SO32DLL_BASE + 6,  module: "SO32DLL",  name: "getpeername",      argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getpeername(a[0],a[1],a[2])) },
    ApiEntry { ordinal: SO32DLL_BASE + 7,  module: "SO32DLL",  name: "getsockname",      argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getsockname(a[0],a[1],a[2])) },
    ApiEntry { ordinal: SO32DLL_BASE + 8,  module: "SO32DLL",  name: "getsockopt",       argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getsockopt(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: SO32DLL_BASE + 10, module: "SO32DLL",  name: "listen",           argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_listen(a[0],a[1])) },
    ApiEntry { ordinal: SO32DLL_BASE + 11, module: "SO32DLL",  name: "recv",             argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_recv(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: SO32DLL_BASE + 12, module: "SO32DLL",  name: "recvfrom",         argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_recvfrom(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: SO32DLL_BASE + 13, module: "SO32DLL",  name: "select",           argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_select(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: SO32DLL_BASE + 14, module: "SO32DLL",  name: "send",             argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_send(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: SO32DLL_BASE + 15, module: "SO32DLL",  name: "sendto",           argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_sendto(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: SO32DLL_BASE + 16, module: "SO32DLL",  name: "setsockopt",       argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_setsockopt(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: SO32DLL_BASE + 17, module: "SO32DLL",  name: "shutdown",         argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_shutdown(a[0],a[1])) },
    ApiEntry { ordinal: SO32DLL_BASE + 18, module: "SO32DLL",  name: "sock_errno",       argc: 0,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.so_sock_errno()) },
    ApiEntry { ordinal: SO32DLL_BASE + 19, module: "SO32DLL",  name: "socket",           argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_socket(a[0],a[1],a[2])) },
    ApiEntry { ordinal: SO32DLL_BASE + 20, module: "SO32DLL",  name: "soclose",          argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_close(a[0])) },
    ApiEntry { ordinal: SO32DLL_BASE + 21, module: "SO32DLL",  name: "psock_errno",      argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_psock_errno(a[0])) },
    ApiEntry { ordinal: SO32DLL_BASE + 22, module: "SO32DLL",  name: "sock_init",        argc: 0,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.so_sock_init()) },
    ApiEntry { ordinal: SO32DLL_BASE + 40, module: "SO32DLL",  name: "gethostbyname",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_gethostbyname(a[0])) },
    ApiEntry { ordinal: SO32DLL_BASE + 41, module: "SO32DLL",  name: "gethostbyaddr",   argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_gethostbyaddr(a[0],a[1],a[2])) },
    ApiEntry { ordinal: SO32DLL_BASE + 42, module: "SO32DLL",  name: "getservbyname",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getservbyname(a[0],a[1])) },
    ApiEntry { ordinal: SO32DLL_BASE + 43, module: "SO32DLL",  name: "getservbyport",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getservbyport(a[0],a[1])) },
    ApiEntry { ordinal: SO32DLL_BASE + 44, module: "SO32DLL",  name: "getprotobyname",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getprotobyname(a[0])) },
    ApiEntry { ordinal: SO32DLL_BASE + 45, module: "SO32DLL",  name: "getprotobynumber", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getprotobynumber(a[0])) },

    // ── TCP32DLL — re-export layer for SO32DLL (base TCP32DLL_BASE = 14336) ──
    ApiEntry { ordinal: TCP32DLL_BASE + 1,  module: "TCP32DLL", name: "accept",          argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_accept(a[0],a[1],a[2])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 2,  module: "TCP32DLL", name: "bind",            argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_bind(a[0],a[1],a[2])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 3,  module: "TCP32DLL", name: "connect",         argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_connect(a[0],a[1],a[2])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 6,  module: "TCP32DLL", name: "getpeername",     argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getpeername(a[0],a[1],a[2])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 7,  module: "TCP32DLL", name: "getsockname",     argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getsockname(a[0],a[1],a[2])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 8,  module: "TCP32DLL", name: "getsockopt",      argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getsockopt(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 10, module: "TCP32DLL", name: "listen",          argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_listen(a[0],a[1])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 11, module: "TCP32DLL", name: "recv",            argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_recv(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 12, module: "TCP32DLL", name: "recvfrom",        argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_recvfrom(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 13, module: "TCP32DLL", name: "select",          argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_select(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 14, module: "TCP32DLL", name: "send",            argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_send(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 15, module: "TCP32DLL", name: "sendto",          argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_sendto(a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 16, module: "TCP32DLL", name: "setsockopt",      argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_setsockopt(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 17, module: "TCP32DLL", name: "shutdown",        argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_shutdown(a[0],a[1])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 18, module: "TCP32DLL", name: "sock_errno",      argc: 0,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.so_sock_errno()) },
    ApiEntry { ordinal: TCP32DLL_BASE + 19, module: "TCP32DLL", name: "socket",          argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_socket(a[0],a[1],a[2])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 20, module: "TCP32DLL", name: "soclose",         argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_close(a[0])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 22, module: "TCP32DLL", name: "sock_init",       argc: 0,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.so_sock_init()) },
    ApiEntry { ordinal: TCP32DLL_BASE + 40, module: "TCP32DLL", name: "gethostbyname",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_gethostbyname(a[0])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 41, module: "TCP32DLL", name: "gethostbyaddr",  argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_gethostbyaddr(a[0],a[1],a[2])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 42, module: "TCP32DLL", name: "getservbyname",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getservbyname(a[0],a[1])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 43, module: "TCP32DLL", name: "getservbyport",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getservbyport(a[0],a[1])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 44, module: "TCP32DLL", name: "getprotobyname", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getprotobyname(a[0])) },
    ApiEntry { ordinal: TCP32DLL_BASE + 45, module: "TCP32DLL", name: "getprotobynumber", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.so_getprotobynumber(a[0])) },
    // ── PMSHAPI — profiles (base PMSHAPI_BASE = 16384) ───────────────────────
    ApiEntry { ordinal: PMSHAPI_BASE + 101, module: "PMSHAPI", name: "PrfQueryProfileSize",   argc: 4,
               handler: |l,_v,i,a| ApiResult::Normal(l.prf_query_profile_size(i,a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 102, module: "PMSHAPI", name: "PrfOpenProfile",        argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.prf_open_profile(i,a[0],a[1])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 103, module: "PMSHAPI", name: "PrfCloseProfile",       argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.prf_close_profile(i,a[0])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 114, module: "PMSHAPI", name: "PrfQueryProfileInt",    argc: 4,
               handler: |l,_v,i,a| ApiResult::Normal(l.prf_query_profile_int(i,a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 115, module: "PMSHAPI", name: "PrfQueryProfileString", argc: 6,
               handler: |l,_v,i,a| ApiResult::Normal(l.prf_query_profile_string(i,a[0],a[1],a[2],a[3],a[4],a[5])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 116, module: "PMSHAPI", name: "PrfWriteProfileString", argc: 4,
               handler: |l,_v,i,a| ApiResult::Normal(l.prf_write_profile_string(i,a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 117, module: "PMSHAPI", name: "PrfQueryProfileData",   argc: 5,
               handler: |l,_v,i,a| ApiResult::Normal(l.prf_query_profile_data(i,a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 118, module: "PMSHAPI", name: "PrfWriteProfileData",   argc: 5,
               handler: |l,_v,i,a| ApiResult::Normal(l.prf_write_profile_data(i,a[0],a[1],a[2],a[3],a[4])) },
    // ── PMSHAPI — starting applications, switch list (pm_startapp.rs, pm_switch.rs) ──
    ApiEntry { ordinal: PMSHAPI_BASE + 119, module: "PMSHAPI", name: "WinStartApp",           argc: 5,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_start_app(i,a[0],a[1],a[2],a[4])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 120, module: "PMSHAPI", name: "WinAddSwitchEntry",     argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_add_switch_entry(i,a[0])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 121, module: "PMSHAPI", name: "WinCreateSwitchEntry",  argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_add_switch_entry(i,a[1])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 123, module: "PMSHAPI", name: "WinChangeSwitchEntry",  argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_change_switch_entry(i,a[0],a[1])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 124, module: "PMSHAPI", name: "WinQuerySwitchEntry",   argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_query_switch_entry(i,a[0],a[1])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 125, module: "PMSHAPI", name: "WinQuerySwitchHandle",  argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_query_switch_handle(i,a[0],a[1])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 126, module: "PMSHAPI", name: "WinQuerySwitchList",    argc: 3,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_query_switch_list(i,a[1],a[2])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 129, module: "PMSHAPI", name: "WinRemoveSwitchEntry",  argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_remove_switch_entry(i,a[0])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 130, module: "PMSHAPI", name: "WinTerminateApp",       argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_terminate_app(i,a[0])) },
    ApiEntry { ordinal: PMSHAPI_BASE + 131, module: "PMSHAPI", name: "WinSwitchToProgram",    argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_switch_to_program(i,a[0])) },

    // ── HELPMGR — Help Manager (base HELPMGR_BASE = 17408) ───────────────────
    ApiEntry { ordinal: HELPMGR_BASE + 51, module: "HELPMGR", name: "WinCreateHelpInstance",    argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_create_help_instance(i,a[0],a[1])) },
    ApiEntry { ordinal: HELPMGR_BASE + 52, module: "HELPMGR", name: "WinDestroyHelpInstance",   argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_destroy_help_instance(i,a[0])) },
    ApiEntry { ordinal: HELPMGR_BASE + 53, module: "HELPMGR", name: "WinQueryHelpInstance",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.win_query_help_instance(a[0])) },
    ApiEntry { ordinal: HELPMGR_BASE + 54, module: "HELPMGR", name: "WinAssociateHelpInstance", argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_associate_help_instance(i,a[0],a[1])) },
    ApiEntry { ordinal: HELPMGR_BASE + 55, module: "HELPMGR", name: "WinLoadHelpTable",         argc: 3,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_load_help_table(i,a[0],a[1],a[2])) },
    ApiEntry { ordinal: HELPMGR_BASE + 56, module: "HELPMGR", name: "WinCreateHelpTable",       argc: 2,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_create_help_table(i,a[0],a[1])) },

    // ── PMSPL — spooler without printers (base PMSPL_BASE = 18432; pmspl.rs) ─
    ApiEntry { ordinal: PMSPL_BASE + 301, module: "PMSPL", name: "SplQmOpen",             argc: 3,
               handler: |l,_v,i,a| ApiResult::Normal(l.spl_qm_open(i,a[1],a[2])) },
    ApiEntry { ordinal: PMSPL_BASE + 302, module: "PMSPL", name: "SplQmStartDoc",         argc: 2,
               handler: |l,_v,i,_a| ApiResult::Normal(l.spl_qm_invalid(i)) },
    ApiEntry { ordinal: PMSPL_BASE + 303, module: "PMSPL", name: "SplQmEndDoc",           argc: 1,
               handler: |l,_v,i,_a| ApiResult::Normal(l.spl_qm_invalid(i)) },
    ApiEntry { ordinal: PMSPL_BASE + 304, module: "PMSPL", name: "SplQmWrite",            argc: 3,
               handler: |l,_v,i,_a| ApiResult::Normal(l.spl_qm_invalid(i)) },
    ApiEntry { ordinal: PMSPL_BASE + 305, module: "PMSPL", name: "SplQmAbort",            argc: 1,
               handler: |l,_v,i,_a| ApiResult::Normal(l.spl_qm_invalid(i)) },
    ApiEntry { ordinal: PMSPL_BASE + 306, module: "PMSPL", name: "SplQmClose",            argc: 1,
               handler: |l,_v,i,_a| ApiResult::Normal(l.spl_qm_invalid(i)) },
    ApiEntry { ordinal: PMSPL_BASE + 308, module: "PMSPL", name: "SplQmAbortDoc",         argc: 1,
               handler: |l,_v,i,_a| ApiResult::Normal(l.spl_qm_invalid(i)) },
    ApiEntry { ordinal: PMSPL_BASE + 381, module: "PMSPL", name: "SplQueryDevice",        argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.spl_query(a[0],a[5],super::pmspl::NERR_DestNotFound)) },
    ApiEntry { ordinal: PMSPL_BASE + 382, module: "PMSPL", name: "SplEnumDevice",         argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.spl_enum(SplEnum::Device,a[0],a[1],a[4],a[5],a[6])) },
    ApiEntry { ordinal: PMSPL_BASE + 397, module: "PMSPL", name: "SplQueryQueue",         argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.spl_query(a[0],a[5],super::pmspl::NERR_QNotFound)) },
    ApiEntry { ordinal: PMSPL_BASE + 399, module: "PMSPL", name: "SplEnumQueue",          argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.spl_enum(SplEnum::Queue,a[0],a[1],a[4],a[5],a[6])) },
    ApiEntry { ordinal: PMSPL_BASE + 406, module: "PMSPL", name: "SplEnumDriver",         argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.spl_enum(SplEnum::Other,a[0],a[1],a[4],a[5],a[6])) },
    ApiEntry { ordinal: PMSPL_BASE + 407, module: "PMSPL", name: "SplEnumQueueProcessor", argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.spl_enum(SplEnum::Other,a[0],a[1],a[4],a[5],a[6])) },
    ApiEntry { ordinal: PMSPL_BASE + 408, module: "PMSPL", name: "SplEnumPort",           argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.spl_enum(SplEnum::Other,a[0],a[1],a[4],a[5],a[6])) },
    ApiEntry { ordinal: PMSPL_BASE + 441, module: "PMSPL", name: "SplEnumPrinter",        argc: 9,
               handler: |l,_v,_i,a| ApiResult::Normal(l.spl_enum(SplEnum::Printer,a[0],a[1],a[5],a[6],a[7])) },

    // ── PMCTLS — standard dialogs (base PMCTLS_BASE = 19456; pm_filedlg.rs, pm_fontdlg.rs)
    ApiEntry { ordinal: PMCTLS_BASE + 2, module: "PMCTLS", name: "WinFontDlg",         argc: 3,
               handler: |l,_v,i,a| l.win_font_dlg(i,a[0],a[1],a[2]) },
    ApiEntry { ordinal: PMCTLS_BASE + 3, module: "PMCTLS", name: "WinDefFontDlgProc",  argc: 4,
               handler: |l,_v,_i,a| l.font_dlg_message(a[0],a[1],a[2],a[3]) },
    ApiEntry { ordinal: PMCTLS_BASE + 4, module: "PMCTLS", name: "WinFileDlg",         argc: 3,
               handler: |l,_v,i,a| l.win_file_dlg(i,a[0],a[1],a[2]) },
    ApiEntry { ordinal: PMCTLS_BASE + 5, module: "PMCTLS", name: "WinDefFileDlgProc",  argc: 4,
               handler: |l,_v,_i,a| l.file_dlg_message(a[0],a[1],a[2],a[3]) },
    ApiEntry { ordinal: PMCTLS_BASE + 6, module: "PMCTLS", name: "WinFreeFileDlgList", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.win_free_file_dlg_list(a[0])) },
    // The class is always there (pm_circslider.rs); registering it succeeds.
    ApiEntry { ordinal: PMCTLS_BASE + 24, module: "PMCTLS", name: "WinRegisterCircularSlider", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(1) },

    // ── WARPINE — Warpine's own extensions (base WARPINE_BASE = 20480; warpine_api.rs)
    ApiEntry { ordinal: WARPINE_BASE + 1, module: "WARPINE", name: "WrpTime64",        argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.wrp_time64(a[0])) },
    ApiEntry { ordinal: WARPINE_BASE + 2, module: "WARPINE", name: "WrpGetDateTime64", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.wrp_get_date_time64(a[0],a[1])) },
    ApiEntry { ordinal: WARPINE_BASE + 3, module: "WARPINE", name: "WrpLocalTime64",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.wrp_local_time64(a[0],a[1])) },
    ApiEntry { ordinal: WARPINE_BASE + 4, module: "WARPINE", name: "WrpMakeTime64",    argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.wrp_make_time64(a[0],a[1])) },
    ApiEntry { ordinal: WARPINE_BASE + 5, module: "WARPINE", name: "WrpBuiltinWindowProc", argc: 4,
               handler: |l,_v,_i,a| l.wrp_builtin_window_proc(a[0],a[1],a[2],a[3]) },
    ApiEntry { ordinal: WARPINE_BASE + 6, module: "WARPINE", name: "WrpStartProcess", argc: 0,
               handler: |l,_v,i,_a| l.dll_init_chain(i, super::pm_hook::ChainThen::StartProcess) },
    ApiEntry { ordinal: WARPINE_BASE + 7, module: "WARPINE", name: "WrpRegisterUnwindTables",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.wrp_register_unwind_tables(a[0])) },
    ApiEntry { ordinal: WARPINE_BASE + 8, module: "WARPINE", name: "WrpDeregisterUnwindTables", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.wrp_deregister_unwind_tables(a[0])) },
    ApiEntry { ordinal: WARPINE_BASE + 9, module: "WARPINE", name: "WrpQueryUnwindTables",      argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.wrp_query_unwind_tables(a[0],a[1])) },
    ApiEntry { ordinal: WARPINE_BASE + 10, module: "WARPINE", name: "WrpQueryRuntimeBlock",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.wrp_query_runtime_block(a[0])) },

    // ── PMDRAG — drag and drop (base PMDRAG_BASE = 21504; pm_drag.rs, ADR-0045)
    ApiEntry { ordinal: PMDRAG_BASE + 32, module: "PMDRAG", name: "DrgAccessDraginfo",          argc: 1,
               handler: |l,_v,i,a| l.drg_access_draginfo(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 33, module: "PMDRAG", name: "DrgAddStrHandle",            argc: 1,
               handler: |l,_v,i,a| l.drg_add_str_handle(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 34, module: "PMDRAG", name: "DrgAllocDraginfo",           argc: 1,
               handler: |l,_v,i,a| l.drg_alloc_draginfo(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 35, module: "PMDRAG", name: "DrgAllocDragtransfer",       argc: 1,
               handler: |l,_v,i,a| l.drg_alloc_dragtransfer(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 36, module: "PMDRAG", name: "DrgDeleteDraginfoStrHandles", argc: 1,
               handler: |l,_v,i,a| l.drg_delete_draginfo_str_handles(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 37, module: "PMDRAG", name: "DrgDeleteStrHandle",         argc: 1,
               handler: |l,_v,i,a| l.drg_delete_str_handle(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 38, module: "PMDRAG", name: "DrgDrag",                    argc: 6,
               handler: |l,_v,i,a| l.drg_drag(i,a[0],a[1],a[2],a[3],a[4]) },
    ApiEntry { ordinal: PMDRAG_BASE + 39, module: "PMDRAG", name: "DrgFreeDraginfo",            argc: 1,
               handler: |l,_v,i,a| l.drg_free_draginfo(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 40, module: "PMDRAG", name: "DrgFreeDragtransfer",        argc: 1,
               handler: |l,_v,i,a| l.drg_free_dragtransfer(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 41, module: "PMDRAG", name: "DrgGetPS",                   argc: 1,
               handler: |l,_v,i,a| l.drg_get_ps(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 42, module: "PMDRAG", name: "DrgPostTransferMsg",         argc: 6,
               handler: |l,_v,i,a| { l.drg_transfer_reset(a[1],a[2]);
                   l.drg_transfer_to_other(i,&a,true).unwrap_or_else(|| l.drg_post(i,a[0],a[1],a[2],a[3])) } },
    ApiEntry { ordinal: PMDRAG_BASE + 43, module: "PMDRAG", name: "DrgPushDraginfo",            argc: 2,
               handler: |l,_v,i,a| l.drg_push_draginfo(i,a[0],a[1]) },
    ApiEntry { ordinal: PMDRAG_BASE + 44, module: "PMDRAG", name: "DrgQueryDragitem",           argc: 4,
               handler: |l,_v,i,a| l.drg_copy_item(i,a[0],a[2],a[1],a[3],false) },
    ApiEntry { ordinal: PMDRAG_BASE + 45, module: "PMDRAG", name: "DrgQueryDragitemCount",      argc: 1,
               handler: |l,_v,i,a| l.drg_query_dragitem_count(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 46, module: "PMDRAG", name: "DrgQueryDragitemPtr",        argc: 2,
               handler: |l,_v,i,a| l.drg_query_dragitem_ptr(i,a[0],a[1]) },
    ApiEntry { ordinal: PMDRAG_BASE + 47, module: "PMDRAG", name: "DrgQueryNativeRMF",          argc: 3,
               handler: |l,_v,_i,a| l.drg_query_native_rmf(a[0],a[1],a[2],false) },
    ApiEntry { ordinal: PMDRAG_BASE + 48, module: "PMDRAG", name: "DrgQueryNativeRMFLen",       argc: 1,
               handler: |l,_v,_i,a| l.drg_query_native_rmf(a[0],0,0,true) },
    ApiEntry { ordinal: PMDRAG_BASE + 49, module: "PMDRAG", name: "DrgQueryStrName",            argc: 3,
               handler: |l,_v,i,a| l.drg_query_str_name(i,a[0],a[1],a[2]) },
    ApiEntry { ordinal: PMDRAG_BASE + 50, module: "PMDRAG", name: "DrgQueryStrNameLen",         argc: 1,
               handler: |l,_v,i,a| l.drg_query_str_name_len(i,a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 51, module: "PMDRAG", name: "DrgQueryTrueType",           argc: 3,
               handler: |l,_v,_i,a| l.drg_query_true_type(a[0],a[1],a[2],false) },
    ApiEntry { ordinal: PMDRAG_BASE + 52, module: "PMDRAG", name: "DrgQueryTrueTypeLen",        argc: 1,
               handler: |l,_v,_i,a| l.drg_query_true_type(a[0],0,0,true) },
    ApiEntry { ordinal: PMDRAG_BASE + 53, module: "PMDRAG", name: "DrgReleasePS",               argc: 1,
               handler: |l,_v,_i,a| l.drg_release_ps(a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 54, module: "PMDRAG", name: "DrgSendTransferMsg",         argc: 4,
               handler: |l,_v,i,a| { l.drg_transfer_reset(a[1],a[2]);
                   l.drg_transfer_to_other(i,&a,false).unwrap_or_else(|| l.win_send_msg(i,a[0],a[1],a[2],a[3])) } },
    ApiEntry { ordinal: PMDRAG_BASE + 55, module: "PMDRAG", name: "DrgSetDragPointer",          argc: 2,
               handler: |l,_v,i,a| l.drg_set_drag_pointer(i,a[0],a[1]) },
    ApiEntry { ordinal: PMDRAG_BASE + 56, module: "PMDRAG", name: "DrgSetDragImage",            argc: 4,
               handler: |l,_v,i,a| l.drg_set_drag_image(i,a[0],a[1],a[2]) },
    ApiEntry { ordinal: PMDRAG_BASE + 57, module: "PMDRAG", name: "DrgSetDragitem",             argc: 4,
               handler: |l,_v,i,a| l.drg_copy_item(i,a[0],a[1],a[2],a[3],true) },
    ApiEntry { ordinal: PMDRAG_BASE + 58, module: "PMDRAG", name: "DrgVerifyNativeRMF",         argc: 2,
               handler: |l,_v,_i,a| l.drg_verify_native_rmf(a[0],a[1]) },
    ApiEntry { ordinal: PMDRAG_BASE + 59, module: "PMDRAG", name: "DrgVerifyRMF",               argc: 3,
               handler: |l,_v,_i,a| l.drg_verify_rmf(a[0],a[1],a[2]) },
    ApiEntry { ordinal: PMDRAG_BASE + 60, module: "PMDRAG", name: "DrgVerifyTrueType",          argc: 2,
               handler: |l,_v,_i,a| l.drg_verify_type(a[0],a[1],true) },
    ApiEntry { ordinal: PMDRAG_BASE + 61, module: "PMDRAG", name: "DrgVerifyType",              argc: 2,
               handler: |l,_v,_i,a| l.drg_verify_type(a[0],a[1],false) },
    ApiEntry { ordinal: PMDRAG_BASE + 62, module: "PMDRAG", name: "DrgVerifyTypeSet",           argc: 4,
               handler: |l,_v,_i,a| l.drg_verify_type_set(a[0],a[1],a[2],a[3]) },
    ApiEntry { ordinal: PMDRAG_BASE + 65, module: "PMDRAG", name: "DrgDragFiles",               argc: 9,
               handler: |l,_v,i,a| l.drg_drag_files(i,&a) },
    ApiEntry { ordinal: PMDRAG_BASE + 70, module: "PMDRAG", name: "DrgQueryDraginfoPtr",        argc: 1,
               handler: |l,_v,_i,a| l.drg_query_draginfo_ptr(a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 71, module: "PMDRAG", name: "DrgQueryDraginfoPtrFromHwnd", argc: 1,
               handler: |l,_v,_i,a| l.drg_query_draginfo_from_hwnd(a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 72, module: "PMDRAG", name: "DrgQueryDraginfoPtrFromDragitem", argc: 1,
               handler: |l,_v,_i,a| l.drg_info_from_item(a[0]) },
    ApiEntry { ordinal: PMDRAG_BASE + 73, module: "PMDRAG", name: "DrgQueryDragStatus",         argc: 0,
               handler: |l,_v,_i,_a| l.drg_query_drag_status() },
    ApiEntry { ordinal: PMDRAG_BASE + 74, module: "PMDRAG", name: "DrgReallocDraginfo",         argc: 2,
               handler: |l,_v,i,a| l.drg_realloc_draginfo(i,a[0],a[1]) },
    // ── SOM: the SOM runtime kernel (som.rs, ADR-0059, ADR-0070). Ordinals 1–47 are
    // IBM's SOM 1.0 exports as osFree lists them (osfree-project/osfree, SOM/som/makefile,
    // BSD-3-Clause); 3072 and up are Warpine's own. ──
    ApiEntry { ordinal: SOM_BASE + 1, module: "SOM", name: "SOMCalloc", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 2, module: "SOM", name: "SOMClassCClassData", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 3, module: "SOM", name: "SOMClassClassData", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 4, module: "SOM", name: "SOMClassInitFuncName", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 5, module: "SOM", name: "SOMClassMgrCClassData", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 6, module: "SOM", name: "SOMClassMgrClassData", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 7, module: "SOM", name: "SOMClassMgrObject", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 8, module: "SOM", name: "SOMDeleteModule", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 9, module: "SOM", name: "SOMError", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 10, module: "SOM", name: "SOMFree", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 11, module: "SOM", name: "SOMLoadModule", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 12, module: "SOM", name: "SOMMalloc", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 13, module: "SOM", name: "SOMObjectCClassData", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 14, module: "SOM", name: "SOMObjectClassData", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 15, module: "SOM", name: "SOMOutCharRoutine", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 16, module: "SOM", name: "SOMRealloc", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 17, module: "SOM", name: "SOM_AssertLevel", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 18, module: "SOM", name: "SOM_TraceLevel", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 19, module: "SOM", name: "SOM_WarnLevel", argc: 0,
               handler: |_l,_v,_i,_a| ApiResult::Normal(0) },  // data: resolve_import gives its address
    ApiEntry { ordinal: SOM_BASE + 20, module: "SOM", name: "SOMClassMgrNewClass", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,20,&a) },
    ApiEntry { ordinal: SOM_BASE + 21, module: "SOM", name: "SOMClassNewClass", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,21,&a) },
    ApiEntry { ordinal: SOM_BASE + 22, module: "SOM", name: "SOMObjectNewClass", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,22,&a) },
    ApiEntry { ordinal: SOM_BASE + 23, module: "SOM", name: "somAssert", argc: 5,
               handler: |l,_v,i,a| l.som_call(i,23,&a) },
    ApiEntry { ordinal: SOM_BASE + 24, module: "SOM", name: "somBeginPersistentIds", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,24,&a) },
    ApiEntry { ordinal: SOM_BASE + 25, module: "SOM", name: "somCheckArgs", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,25,&a) },
    ApiEntry { ordinal: SOM_BASE + 26, module: "SOM", name: "somCheckId", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,26,&a) },
    ApiEntry { ordinal: SOM_BASE + 27, module: "SOM", name: "somCompareIds", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,27,&a) },
    ApiEntry { ordinal: SOM_BASE + 28, module: "SOM", name: "somConstructClass", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,28,&a) },
    ApiEntry { ordinal: SOM_BASE + 29, module: "SOM", name: "somEndPersistentIds", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,29,&a) },
    ApiEntry { ordinal: SOM_BASE + 30, module: "SOM", name: "somEnvironmentNew", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,30,&a) },
    ApiEntry { ordinal: SOM_BASE + 31, module: "SOM", name: "somIdFromString", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,31,&a) },
    ApiEntry { ordinal: SOM_BASE + 32, module: "SOM", name: "somLPrintf", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,32,&a) },
    ApiEntry { ordinal: SOM_BASE + 33, module: "SOM", name: "somParentResolve", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,33,&a) },
    ApiEntry { ordinal: SOM_BASE + 34, module: "SOM", name: "somPrefixLevel", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,34,&a) },
    ApiEntry { ordinal: SOM_BASE + 35, module: "SOM", name: "somPrintf", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,35,&a) },
    ApiEntry { ordinal: SOM_BASE + 36, module: "SOM", name: "somRegisterId", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,36,&a) },
    ApiEntry { ordinal: SOM_BASE + 37, module: "SOM", name: "somResolve", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,37,&a) },
    ApiEntry { ordinal: SOM_BASE + 38, module: "SOM", name: "somSaveMetrics", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,38,&a) },
    ApiEntry { ordinal: SOM_BASE + 39, module: "SOM", name: "somSetExpectedIds", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,39,&a) },
    ApiEntry { ordinal: SOM_BASE + 40, module: "SOM", name: "somStringFromId", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,40,&a) },
    ApiEntry { ordinal: SOM_BASE + 41, module: "SOM", name: "somTest", argc: 5,
               handler: |l,_v,i,a| l.som_call(i,41,&a) },
    ApiEntry { ordinal: SOM_BASE + 42, module: "SOM", name: "somTestCls", argc: 4,
               handler: |l,_v,i,a| l.som_call(i,42,&a) },
    ApiEntry { ordinal: SOM_BASE + 43, module: "SOM", name: "somTotalRegIds", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,43,&a) },
    ApiEntry { ordinal: SOM_BASE + 44, module: "SOM", name: "somUniqueKey", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,44,&a) },
    ApiEntry { ordinal: SOM_BASE + 45, module: "SOM", name: "somVprintf", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,45,&a) },
    ApiEntry { ordinal: SOM_BASE + 46, module: "SOM", name: "somWriteMetrics", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,46,&a) },
    ApiEntry { ordinal: SOM_BASE + 47, module: "SOM", name: "somDataResolve", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,47,&a) },
    ApiEntry { ordinal: SOM_BASE + 3072, module: "SOM", name: "somResolveByName", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3072,&a) },
    ApiEntry { ordinal: SOM_BASE + 3073, module: "SOM", name: "somClassResolve", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3073,&a) },
    ApiEntry { ordinal: SOM_BASE + 3074, module: "SOM", name: "somGetGlobalEnvironment", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,3074,&a) },
    ApiEntry { ordinal: SOM_BASE + 3075, module: "SOM", name: "somIsObj", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3075,&a) },
    ApiEntry { ordinal: SOM_BASE + 3076, module: "SOM", name: "somSetOutChar", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3076,&a) },
    ApiEntry { ordinal: SOM_BASE + 3077, module: "SOM", name: "somParentNumResolve", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3077,&a) },
    ApiEntry { ordinal: SOM_BASE + 3078, module: "SOM", name: "somBuildClass", argc: 4,
               handler: |l,_v,i,a| l.som_call(i,3078,&a) },
    ApiEntry { ordinal: SOM_BASE + 3079, module: "SOM", name: "somDataResolveChk", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3079,&a) },
    ApiEntry { ordinal: SOM_BASE + 3080, module: "SOM", name: "WrpSomMalloc", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3080,&a) },
    ApiEntry { ordinal: SOM_BASE + 3081, module: "SOM", name: "WrpSomCalloc", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3081,&a) },
    ApiEntry { ordinal: SOM_BASE + 3082, module: "SOM", name: "WrpSomRealloc", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3082,&a) },
    ApiEntry { ordinal: SOM_BASE + 3083, module: "SOM", name: "WrpSomFree", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3083,&a) },
    ApiEntry { ordinal: SOM_BASE + 3084, module: "SOM", name: "WrpSomMetaclassFor", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3084,&a) },
    ApiEntry { ordinal: SOM_BASE + 3085, module: "SOM", name: "WrpSomLoadModule", argc: 6,
               handler: |l,_v,i,a| l.som_call(i,3085,&a) },
    ApiEntry { ordinal: SOM_BASE + 3086, module: "SOM", name: "WrpSomDeleteModule", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3086,&a) },
    ApiEntry { ordinal: SOM_BASE + 3087, module: "SOM", name: "WrpSomError", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3087,&a) },
    ApiEntry { ordinal: SOM_BASE + 3088, module: "SOM", name: "WrpSomOutChar", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3088,&a) },
    ApiEntry { ordinal: SOM_BASE + 3089, module: "SOM", name: "WrpSomClassInitFuncName", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,3089,&a) },
    ApiEntry { ordinal: SOM_BASE + 3090, module: "SOM", name: "WrpSomDataExport", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3090,&a) },
    ApiEntry { ordinal: SOM_BASE + 3091, module: "SOM", name: "WrpSomInitCtrlNew", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3091,&a) },
    ApiEntry { ordinal: SOM_BASE + 3092, module: "SOM", name: "WrpSomInitCtrlFree", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3092,&a) },
    ApiEntry { ordinal: SOM_BASE + 3093, module: "SOM", name: "WrpSomInitWalk", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3093,&a) },
    ApiEntry { ordinal: SOM_BASE + 3094, module: "SOM", name: "WrpSomDestructWalk", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3094,&a) },
    ApiEntry { ordinal: SOM_BASE + 3095, module: "SOM", name: "WrpSomSetDirectInitClasses", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3095,&a) },
    ApiEntry { ordinal: SOM_BASE + 3198, module: "SOM", name: "WrpSomMethodNotImplemented", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,3198,&a) },
    ApiEntry { ordinal: SOM_BASE + 3199, module: "SOM", name: "WrpSomMethodError", argc: 0,
               handler: |l,_v,i,a| l.som_call(i,3199,&a) },
    ApiEntry { ordinal: SOM_BASE + 3200, module: "SOM", name: "WrpSOMObject_somInit", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3200,&a) },
    ApiEntry { ordinal: SOM_BASE + 3201, module: "SOM", name: "WrpSOMObject_somUninit", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3201,&a) },
    ApiEntry { ordinal: SOM_BASE + 3202, module: "SOM", name: "WrpSOMObject_somFree", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3202,&a) },
    ApiEntry { ordinal: SOM_BASE + 3203, module: "SOM", name: "WrpSOMObject_somGetClass", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3203,&a) },
    ApiEntry { ordinal: SOM_BASE + 3204, module: "SOM", name: "WrpSOMObject_somGetClassName", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3204,&a) },
    ApiEntry { ordinal: SOM_BASE + 3205, module: "SOM", name: "WrpSOMObject_somGetSize", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3205,&a) },
    ApiEntry { ordinal: SOM_BASE + 3206, module: "SOM", name: "WrpSOMObject_somIsA", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3206,&a) },
    ApiEntry { ordinal: SOM_BASE + 3207, module: "SOM", name: "WrpSOMObject_somIsInstanceOf", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3207,&a) },
    ApiEntry { ordinal: SOM_BASE + 3208, module: "SOM", name: "WrpSOMObject_somRespondsTo", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3208,&a) },
    ApiEntry { ordinal: SOM_BASE + 3209, module: "SOM", name: "WrpSOMObject_somPrintSelf", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3209,&a) },
    ApiEntry { ordinal: SOM_BASE + 3210, module: "SOM", name: "WrpSOMObject_somDumpSelf", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3210,&a) },
    ApiEntry { ordinal: SOM_BASE + 3211, module: "SOM", name: "WrpSOMObject_somDumpSelfInt", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3211,&a) },
    ApiEntry { ordinal: SOM_BASE + 3212, module: "SOM", name: "WrpSOMObject_somDefaultInit", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3212,&a) },
    ApiEntry { ordinal: SOM_BASE + 3213, module: "SOM", name: "WrpSOMObject_somDestruct", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3213,&a) },
    ApiEntry { ordinal: SOM_BASE + 3214, module: "SOM", name: "WrpSOMObject_somDefaultCopyInit", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3214,&a) },
    ApiEntry { ordinal: SOM_BASE + 3215, module: "SOM", name: "WrpSOMObject_somDefaultConstCopyInit", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3215,&a) },
    ApiEntry { ordinal: SOM_BASE + 3216, module: "SOM", name: "WrpSOMObject_somDefaultVCopyInit", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3216,&a) },
    ApiEntry { ordinal: SOM_BASE + 3217, module: "SOM", name: "WrpSOMObject_somDefaultConstVCopyInit", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3217,&a) },
    ApiEntry { ordinal: SOM_BASE + 3218, module: "SOM", name: "WrpSOMObject_somDefaultAssign", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3218,&a) },
    ApiEntry { ordinal: SOM_BASE + 3219, module: "SOM", name: "WrpSOMObject_somDefaultConstAssign", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3219,&a) },
    ApiEntry { ordinal: SOM_BASE + 3220, module: "SOM", name: "WrpSOMObject_somDefaultVAssign", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3220,&a) },
    ApiEntry { ordinal: SOM_BASE + 3221, module: "SOM", name: "WrpSOMObject_somDefaultConstVAssign", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3221,&a) },
    ApiEntry { ordinal: SOM_BASE + 3300, module: "SOM", name: "WrpSOMClass_somNew", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3300,&a) },
    ApiEntry { ordinal: SOM_BASE + 3301, module: "SOM", name: "WrpSOMClass_somNewNoInit", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3301,&a) },
    ApiEntry { ordinal: SOM_BASE + 3302, module: "SOM", name: "WrpSOMClass_somGetName", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3302,&a) },
    ApiEntry { ordinal: SOM_BASE + 3303, module: "SOM", name: "WrpSOMClass_somGetParent", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3303,&a) },
    ApiEntry { ordinal: SOM_BASE + 3304, module: "SOM", name: "WrpSOMClass_somGetInstanceSize", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3304,&a) },
    ApiEntry { ordinal: SOM_BASE + 3305, module: "SOM", name: "WrpSOMClass_somGetInstancePartSize", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3305,&a) },
    ApiEntry { ordinal: SOM_BASE + 3306, module: "SOM", name: "WrpSOMClass_somGetInstanceToken", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3306,&a) },
    ApiEntry { ordinal: SOM_BASE + 3307, module: "SOM", name: "WrpSOMClass_somGetNumMethods", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3307,&a) },
    ApiEntry { ordinal: SOM_BASE + 3308, module: "SOM", name: "WrpSOMClass_somGetNumStaticMethods", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3308,&a) },
    ApiEntry { ordinal: SOM_BASE + 3309, module: "SOM", name: "WrpSOMClass_somDescendedFrom", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3309,&a) },
    ApiEntry { ordinal: SOM_BASE + 3310, module: "SOM", name: "WrpSOMClass_somInitClass", argc: 7,
               handler: |l,_v,i,a| l.som_call(i,3310,&a) },
    ApiEntry { ordinal: SOM_BASE + 3311, module: "SOM", name: "WrpSOMClass_somAddStaticMethod", argc: 6,
               handler: |l,_v,i,a| l.som_call(i,3311,&a) },
    ApiEntry { ordinal: SOM_BASE + 3312, module: "SOM", name: "WrpSOMClass_somOverrideSMethod", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3312,&a) },
    ApiEntry { ordinal: SOM_BASE + 3313, module: "SOM", name: "WrpSOMClass_somClassReady", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3313,&a) },
    ApiEntry { ordinal: SOM_BASE + 3314, module: "SOM", name: "WrpSOMClass_somGetMethodToken", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3314,&a) },
    ApiEntry { ordinal: SOM_BASE + 3315, module: "SOM", name: "WrpSOMClass_somSupportsMethod", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3315,&a) },
    ApiEntry { ordinal: SOM_BASE + 3316, module: "SOM", name: "WrpSOMClass_somInitMIClass", argc: 10,
               handler: |l,_v,i,a| l.som_call(i,3316,&a) },
    ApiEntry { ordinal: SOM_BASE + 3317, module: "SOM", name: "WrpSOMClass_somCheckVersion", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3317,&a) },
    ApiEntry { ordinal: SOM_BASE + 3318, module: "SOM", name: "WrpSOMClass_somGetVersionNumbers", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3318,&a) },
    ApiEntry { ordinal: SOM_BASE + 3319, module: "SOM", name: "WrpSOMClass_somGetClassMtab", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3319,&a) },
    ApiEntry { ordinal: SOM_BASE + 3320, module: "SOM", name: "WrpSOMClass_somGetInstanceOffset", argc: 1,
               handler: |l,_v,i,a| l.som_call(i,3320,&a) },
    ApiEntry { ordinal: SOM_BASE + 3321, module: "SOM", name: "WrpSOMClass_somFindMethod", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3321,&a) },
    ApiEntry { ordinal: SOM_BASE + 3322, module: "SOM", name: "WrpSOMClass_somFindMethodOk", argc: 3,
               handler: |l,_v,i,a| l.som_call(i,3322,&a) },
    ApiEntry { ordinal: SOM_BASE + 3323, module: "SOM", name: "WrpSOMClass_somFindSMethod", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3323,&a) },
    ApiEntry { ordinal: SOM_BASE + 3324, module: "SOM", name: "WrpSOMClass_somFindSMethodOk", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3324,&a) },
    ApiEntry { ordinal: SOM_BASE + 3325, module: "SOM", name: "WrpSOMClass_somLookupMethod", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3325,&a) },
    ApiEntry { ordinal: SOM_BASE + 3326, module: "SOM", name: "WrpSOMClass_somAllocate", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3326,&a) },
    ApiEntry { ordinal: SOM_BASE + 3327, module: "SOM", name: "WrpSOMClass_somDeallocate", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3327,&a) },
    ApiEntry { ordinal: SOM_BASE + 3400, module: "SOM", name: "WrpSOMClassMgr_somFindClass", argc: 4,
               handler: |l,_v,i,a| l.som_call(i,3400,&a) },
    ApiEntry { ordinal: SOM_BASE + 3401, module: "SOM", name: "WrpSOMClassMgr_somClassFromId", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3401,&a) },
    ApiEntry { ordinal: SOM_BASE + 3402, module: "SOM", name: "WrpSOMClassMgr_somRegisterClass", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3402,&a) },
    ApiEntry { ordinal: SOM_BASE + 3403, module: "SOM", name: "WrpSOMClassMgr_somFindClsInFile", argc: 5,
               handler: |l,_v,i,a| l.som_call(i,3403,&a) },
    ApiEntry { ordinal: SOM_BASE + 3404, module: "SOM", name: "WrpSOMClassMgr_somLocateClassFile", argc: 4,
               handler: |l,_v,i,a| l.som_call(i,3404,&a) },
    ApiEntry { ordinal: SOM_BASE + 3405, module: "SOM", name: "WrpSOMClassMgr_somLoadClassFile", argc: 5,
               handler: |l,_v,i,a| l.som_call(i,3405,&a) },
    ApiEntry { ordinal: SOM_BASE + 3406, module: "SOM", name: "WrpSOMClassMgr_somUnregisterClass", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3406,&a) },
    ApiEntry { ordinal: SOM_BASE + 3407, module: "SOM", name: "WrpSOMClassMgr_somUnloadClassFile", argc: 2,
               handler: |l,_v,i,a| l.som_call(i,3407,&a) },
    // ── REXX / REXXAPI — the REXX API (rexxapi.rs, ADR-0085) ─────────────────
    ApiEntry { ordinal: REXX_BASE + 1, module: "REXX", name: "RexxStart", argc: 9,
               handler: |l,_v,i,a| l.rexx_start(i,a) },
    ApiEntry { ordinal: REXX_BASE + 2, module: "REXX", name: "RexxVariablePool", argc: 1,
               handler: |l,_v,i,a| ApiResult::Normal(l.rexx_variable_pool(i,a[0])) },
    ApiEntry { ordinal: REXX_BASE + 3, module: "REXX", name: "RexxSetHalt", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_signal(a[0],a[1],RexxSignal::Halt)) },
    ApiEntry { ordinal: REXX_BASE + 4, module: "REXX", name: "RexxSetTrace", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_signal(a[0],a[1],RexxSignal::TraceOn)) },
    ApiEntry { ordinal: REXX_BASE + 5, module: "REXX", name: "RexxResetTrace", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_signal(a[0],a[1],RexxSignal::TraceOff)) },
    ApiEntry { ordinal: REXXAPI_BASE + 1, module: "REXXAPI", name: "RexxCreateQueue", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_create_queue(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: REXXAPI_BASE + 2, module: "REXXAPI", name: "RexxDeleteQueue", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_delete_queue(a[0])) },
    ApiEntry { ordinal: REXXAPI_BASE + 3, module: "REXXAPI", name: "RexxAddQueue", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_add_queue(a[0],a[1],a[2])) },
    ApiEntry { ordinal: REXXAPI_BASE + 4, module: "REXXAPI", name: "RexxPullQueue", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_pull_queue(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: REXXAPI_BASE + 5, module: "REXXAPI", name: "RexxQueryQueue", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_query_queue(a[0],a[1])) },
    ApiEntry { ordinal: REXXAPI_BASE + 6, module: "REXXAPI", name: "RexxRegisterSubcomDll", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_register_dll(Kind::Subcom,a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: REXXAPI_BASE + 7, module: "REXXAPI", name: "RexxRegisterSubcomExe", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_register_exe(Kind::Subcom,a[0],a[1],a[2])) },
    ApiEntry { ordinal: REXXAPI_BASE + 8, module: "REXXAPI", name: "RexxQuerySubcom", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_query(Kind::Subcom,a[0],a[2],a[3])) },
    ApiEntry { ordinal: REXXAPI_BASE + 9, module: "REXXAPI", name: "RexxDeregisterSubcom", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_deregister(Kind::Subcom,a[0])) },
    ApiEntry { ordinal: REXXAPI_BASE + 11, module: "REXXAPI", name: "RexxRegisterExitDll", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_register_dll(Kind::Exit,a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: REXXAPI_BASE + 12, module: "REXXAPI", name: "RexxRegisterExitExe", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_register_exe(Kind::Exit,a[0],a[1],a[2])) },
    ApiEntry { ordinal: REXXAPI_BASE + 13, module: "REXXAPI", name: "RexxDeregisterExit", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_deregister(Kind::Exit,a[0])) },
    ApiEntry { ordinal: REXXAPI_BASE + 14, module: "REXXAPI", name: "RexxQueryExit", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_query(Kind::Exit,a[0],a[2],a[3])) },
    ApiEntry { ordinal: REXXAPI_BASE + 17, module: "REXXAPI", name: "RexxRegisterFunctionDll", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_register_dll(Kind::Function,a[0],a[1],a[2],0)) },
    ApiEntry { ordinal: REXXAPI_BASE + 18, module: "REXXAPI", name: "RexxRegisterFunctionExe", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_register_exe(Kind::Function,a[0],a[1],0)) },
    ApiEntry { ordinal: REXXAPI_BASE + 19, module: "REXXAPI", name: "RexxDeregisterFunction", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_deregister(Kind::Function,a[0])) },
    ApiEntry { ordinal: REXXAPI_BASE + 20, module: "REXXAPI", name: "RexxQueryFunction", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.rexx_query(Kind::Function,a[0],0,0)) },
    ApiEntry { ordinal: REXXAPI_BASE + 22, module: "REXXAPI", name: "RexxAddMacro", argc: 3,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.rexx_macro_space("RexxAddMacro")) },
    ApiEntry { ordinal: REXXAPI_BASE + 23, module: "REXXAPI", name: "RexxDropMacro", argc: 1,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.rexx_macro_space("RexxDropMacro")) },
    ApiEntry { ordinal: REXXAPI_BASE + 24, module: "REXXAPI", name: "RexxClearMacroSpace", argc: 0,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.rexx_macro_space("RexxClearMacroSpace")) },
    ApiEntry { ordinal: REXXAPI_BASE + 25, module: "REXXAPI", name: "RexxLoadMacroSpace", argc: 3,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.rexx_macro_space("RexxLoadMacroSpace")) },
    ApiEntry { ordinal: REXXAPI_BASE + 26, module: "REXXAPI", name: "RexxQueryMacro", argc: 2,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.rexx_macro_space("RexxQueryMacro")) },
    ApiEntry { ordinal: REXXAPI_BASE + 27, module: "REXXAPI", name: "RexxReorderMacro", argc: 2,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.rexx_macro_space("RexxReorderMacro")) },
    ApiEntry { ordinal: REXXAPI_BASE + 28, module: "REXXAPI", name: "RexxSaveMacroSpace", argc: 3,
               handler: |l,_v,_i,_a| ApiResult::Normal(l.rexx_macro_space("RexxSaveMacroSpace")) },

    // ── PMWP — the Workplace Shell (window positions only; pm_winutil.rs) ──
    ApiEntry { ordinal: PMWP_BASE + 207, module: "PMWP", name: "WinStoreWindowPos", argc: 3,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_store_window_pos(i,a[0],a[1],a[2])) },
    ApiEntry { ordinal: PMWP_BASE + 208, module: "PMWP", name: "WinRestoreWindowPos", argc: 3,
               handler: |l,_v,i,a| ApiResult::Normal(l.win_restore_window_pos(i,a[0],a[1],a[2])) },
];

// ── Compatibility report ──────────────────────────────────────────────────────

/// Pure-stub ordinals: handlers that return 0 without doing any real work.
/// These are maintained manually alongside the registry entries above.
const STUB_ORDINALS: &[u32] = &[
    267,  // DOS16REQUESTVDD
    285,  // DosFSCtl
    317,  // DosDebug
    415,  // DosShutdown
    // SOM entries no source describes (metrics), and somConstructClass
    // until classes are built from IBM's static class data (ADR-0072).
    SOM_BASE + 28,  // somConstructClass
    SOM_BASE + 38,  // somSaveMetrics
    SOM_BASE + 46,  // somWriteMetrics
    // The REXX macro space is not there (ADR-0085).
    REXXAPI_BASE + 22, REXXAPI_BASE + 23, REXXAPI_BASE + 24, REXXAPI_BASE + 25,
    REXXAPI_BASE + 26, REXXAPI_BASE + 27, REXXAPI_BASE + 28,
];

/// VIOCALLS ordinals of `viocalls.rs` (18 implemented, 4 stubs).
const VIOCALLS_ORDINALS: u32 = 22;
/// KBDCALLS ordinals of `kbdcalls.rs`.
const KBDCALLS_ORDINALS: u32 = 3;
/// MOUCALLS ordinals of `moucalls.rs`.
const MOUCALLS_ORDINALS: u32 = 27;
/// PMWIN ordinals dispatched by `pm_win.rs` and `pm_winutil.rs` (checked
/// against their `N => { // WinName(` arms by a test).
const PMWIN_ORDINALS: u32 = 218;
/// PMGPI ordinals dispatched by `pm_gpi.rs`, `pm_region.rs`, `pm_area.rs`,
/// `pm_text.rs`, `pmspl.rs`, `pm_coords.rs` and `pm_psstate.rs` (checked against
/// their arms by a test), of which `PMGPI_STUBS` are stubs (`GpiLoadFonts`,
/// `GpiLoadPublicFonts`, `GpiUnloadPublicFonts`).
const PMGPI_ORDINALS: u32 = 145;
const PMGPI_STUBS: u32 = 3;

/// Ordinals handled by the sub-dispatchers rather than the registry.
const SUB_DISPATCHER_ORDINALS: u32 =
    VIOCALLS_ORDINALS + KBDCALLS_ORDINALS + MOUCALLS_ORDINALS + PMWIN_ORDINALS + PMGPI_ORDINALS;

/// Generate a human-readable OS/2 API compatibility report.
///
/// Sections:
/// 1. Per-module table of all registry entries (implemented vs. stub)
/// 2. Sub-dispatcher summaries for VIOCALLS, KBDCALLS, MOUCALLS, PMWIN, PMGPI
/// 3. Aggregate statistics
pub fn compat_report() -> String {
    use std::collections::BTreeMap;
    let mut out = String::new();

    out.push_str("Warpine OS/2 API Compatibility Report\n");
    out.push_str("======================================\n\n");

    // Group registry entries by module (BTreeMap for stable alphabetical order)
    let mut by_module: BTreeMap<&str, Vec<&ApiEntry>> = BTreeMap::new();
    for entry in REGISTRY {
        by_module.entry(entry.module).or_default().push(entry);
    }

    let mut total_impl  = 0u32;
    let mut total_stubs = 0u32;

    for (module, entries) in &by_module {
        let module_stubs = entries.iter().filter(|e| STUB_ORDINALS.contains(&e.ordinal)).count();
        let module_impl  = entries.len() - module_stubs;
        out.push_str(&format!(
            "{module}  ({} implemented, {} stub)\n",
            module_impl, module_stubs
        ));
        for e in entries {
            let tag = if STUB_ORDINALS.contains(&e.ordinal) { "[stub]" } else { "      " };
            out.push_str(&format!("  {tag}  [{:>4}]  {}\n", e.ordinal, e.name));
        }
        out.push('\n');
        total_impl  += module_impl  as u32;
        total_stubs += module_stubs as u32;
    }

    // Sub-dispatcher summaries
    out.push_str("Sub-dispatchers (separate from registry)\n");
    out.push_str("----------------------------------------\n");
    out.push_str("  VIOCALLS   18 implemented, 4 stub  (VioWrtTTY, VioGetMode, VioSetMode, VioSetCurPos, \
                  VioGetCurPos, VioScrollUp, VioScrollDn, VioWrtCharStrAtt, VioWrtCellStr, VioWrtNCell, \
                  VioWrtNAttr, VioReadCellStr, VioSetCurType, VioGetCurType, VioGetConfig, VioGetAnsi, VioSetAnsi, \
                  VioCheckCharType, \
                  VioGetBuf[stub], VioShowBuf[stub], VioSetState[stub], VioSetCp[stub])\n");
    out.push_str("  KBDCALLS    3 implemented  (KbdCharIn, KbdStringIn, KbdGetStatus)\n");
    out.push_str("  MOUCALLS   20 implemented  (MouOpen, MouClose, MouReadEventQue, MouFlushQue, \
                  MouGetNumQueEl, MouGet/SetPtrPos, MouGetNumButtons, MouGetNumMickeys, \
                  MouGet/SetDevStatus, MouGet/SetEventMask, MouGet/SetScaleFact, \
                  MouDrawPtr, MouRemovePtr, MouGet/SetPtrShape, MouSynch)\n");
    out.push_str(&format!("  PMWIN     {PMWIN_ORDINALS} implemented  (WinInitialize, message queues, hooks, broadcasts, window mgmt, \
                  input, paint, timers, menus, dialogs, controls, clipboard, resources, regions, atoms, DDE)\n"));
    out.push_str(&format!("  PMGPI     {} implemented, {PMGPI_STUBS} stub  (presentation spaces, colour and mix, \
                  lines, boxes, arcs, text, text layout and character attributes, logical fonts, font metrics, bitmaps, regions, clipping, \
                  areas, paths, curves, transforms, save / restore)\n", PMGPI_ORDINALS - PMGPI_STUBS));
    out.push('\n');

    // Aggregate statistics
    let registry_total = REGISTRY.len() as u32;
    out.push_str("Summary\n");
    out.push_str("-------\n");
    out.push_str(&format!("  Registry entries:  {registry_total}\n"));
    out.push_str(&format!("    Implemented:     {total_impl}\n"));
    out.push_str(&format!("    Stub (no-op):    {total_stubs}\n"));
    out.push_str(&format!("  Sub-dispatchers:   {SUB_DISPATCHER_ORDINALS} ordinals (VIOCALLS + KBDCALLS + MOUCALLS + PMWIN + PMGPI)\n"));
    out.push_str(&format!(
        "  Grand total:       {} API entry points covered\n",
        registry_total + SUB_DISPATCHER_ORDINALS
    ));

    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::api_trace;

    // ── compat_report ────────────────────────────────────────────────────────

    #[test]
    fn test_compat_report_structure() {
        let report = super::compat_report();
        // Must contain the header
        assert!(report.contains("Warpine OS/2 API Compatibility Report"),
            "missing header");
        // Must list all four registry modules
        assert!(report.contains("DOSCALLS"), "missing DOSCALLS section");
        assert!(report.contains("QUECALLS"), "missing QUECALLS section");
        assert!(report.contains("NLS"),      "missing NLS section");
        assert!(report.contains("MDM"),      "missing MDM section");
        // Must mention all four sub-dispatchers
        assert!(report.contains("VIOCALLS"), "missing VIOCALLS");
        assert!(report.contains("KBDCALLS"), "missing KBDCALLS");
        assert!(report.contains("MOUCALLS"), "missing MOUCALLS");
        assert!(report.contains("PMWIN"),    "missing PMWIN");
        assert!(report.contains("PMGPI"),    "missing PMGPI");
        // Spot-check a known implemented and a known stub API name
        assert!(report.contains("DosOpen"),        "DosOpen missing");
        assert!(report.contains("DosCreateNPipe"), "DosCreateNPipe missing");
        assert!(report.contains("[stub]"),         "no stubs marked");
        // Summary section present
        assert!(report.contains("Grand total:"), "missing summary");
        let total = REGISTRY.len() as u32 + super::SUB_DISPATCHER_ORDINALS;
        assert!(report.contains(&format!("{total} API entry points")), "wrong grand total");
    }

    #[test]
    fn test_compat_report_stub_count() {
        let report = super::compat_report();
        // Count only registry-section stub lines, which are formatted as
        // "  [stub]  [ordinal]  Name" — prefix with two spaces + "[stub]".
        let stub_lines = report.lines()
            .filter(|l| l.trim_start().starts_with("[stub]"))
            .count();
        assert_eq!(stub_lines, super::STUB_ORDINALS.len(),
            "stub count in report ({stub_lines}) != STUB_ORDINALS len ({})",
            super::STUB_ORDINALS.len());
    }

    /// Distinct ordinals of the `N => {` arms followed by a `// ApiName(`
    /// comment, the form every PM sub-dispatcher arm takes.
    fn dispatched_ordinals(sources: &[&str]) -> std::collections::BTreeSet<u32> {
        let mut ords = std::collections::BTreeSet::new();
        for src in sources {
            let lines: Vec<&str> = src.lines().map(str::trim).collect();
            for pair in lines.windows(2) {
                let Some(ord) = pair[0].strip_suffix(" => {").and_then(|o| o.parse::<u32>().ok())
                    else { continue };
                let Some(rest) = pair[1].strip_prefix("// ") else { continue };
                if rest.starts_with(|c: char| c.is_ascii_uppercase()) {
                    ords.insert(ord);
                }
            }
        }
        ords
    }

    #[test]
    fn test_compat_report_pm_counts_match_the_dispatchers() {
        let win = dispatched_ordinals(&[include_str!("pm_win.rs"), include_str!("pm_winutil.rs")]);
        assert_eq!(win.len() as u32, super::PMWIN_ORDINALS, "PMWIN_ORDINALS is stale");
        let gpi = dispatched_ordinals(&[include_str!("pm_gpi.rs"), include_str!("pm_region.rs"),
                                        include_str!("pm_area.rs"), include_str!("pm_text.rs"),
                                        include_str!("pmspl.rs"), include_str!("pm_coords.rs"),
                                        include_str!("pm_psstate.rs")]);
        assert_eq!(gpi.len() as u32, super::PMGPI_ORDINALS, "PMGPI_ORDINALS is stale");
        // build.rs's name tables (DosQueryProcAddr by name) list the same calls.
        let names = |m| super::super::descriptors::pm_dispatched(m).expect("a PM table")
            .iter().map(|&(_, o)| o).collect::<std::collections::BTreeSet<u32>>();
        assert_eq!(names("PMWIN"), win, "PMWIN name table");
        assert_eq!(names("PMGPI"), gpi, "PMGPI name table");
    }

    #[test]
    fn test_compat_report_marks_only_no_op_stubs() {
        // SEH works: DosRaiseException / DosUnwindException are not stubs.
        for ord in [356, 357] {
            assert!(!super::STUB_ORDINALS.contains(&ord), "ordinal {ord} is implemented");
        }
        let report = super::compat_report();
        assert!(report.contains("DOSCALLS  (160 implemented, 4 stub)"), "{report}");
    }

    // ── registry ─────────────────────────────────────────────────────────────

    #[test]
    fn test_registry_sorted() {
        for w in REGISTRY.windows(2) {
            assert!(
                w[0].ordinal < w[1].ordinal,
                "Registry not sorted ascending: ordinal {} ({}) must be < {} ({})",
                w[0].ordinal, w[0].name, w[1].ordinal, w[1].name
            );
        }
    }

    #[test]
    fn test_registry_no_duplicates() {
        // With a sorted slice, duplicates are adjacent
        for w in REGISTRY.windows(2) {
            assert_ne!(
                w[0].ordinal, w[1].ordinal,
                "Duplicate ordinal {} ({}) in registry",
                w[0].ordinal, w[0].name
            );
        }
    }

    /// For every registry entry where api_trace has a non-"?" name, the names
    /// must agree.  This is a consistency regression guard.
    #[test]
    fn test_registry_names_match_api_trace() {
        for entry in REGISTRY {
            let traced = api_trace::ordinal_to_name(entry.ordinal);
            if traced != "?" {
                assert_eq!(
                    traced, entry.name,
                    "Name mismatch for ordinal {}: registry={:?}, api_trace={:?}",
                    entry.ordinal, entry.name, traced
                );
            }
        }
    }

    #[test]
    fn test_registry_modules_match_api_trace() {
        for entry in REGISTRY {
            let traced_mod = api_trace::module_for_ordinal(entry.ordinal);
            assert_eq!(
                traced_mod, entry.module,
                "Module mismatch for ordinal {} ({}): registry={:?}, api_trace={:?}",
                entry.ordinal, entry.name, entry.module, traced_mod
            );
        }
    }

    #[test]
    fn test_find_known_ordinals() {
        let e = find(273).expect("DosOpen missing from registry");
        assert_eq!(e.name, "DosOpen");
        assert_eq!(e.module, "DOSCALLS");

        let e = find(282).expect("DosWrite missing from registry");
        assert_eq!(e.name, "DosWrite");

        let e = find(1024 + 16).expect("DosCreateQueue missing from registry");
        assert_eq!(e.name, "DosCreateQueue");
        assert_eq!(e.module, "QUECALLS");

        // NLS ordinals as in <bseord.h> (ORD_DOS32QUERYCTRYINFO = 5 …).
        for (ord, name) in [(5, "DosQueryCtryInfo"), (6, "DosQueryDBCSEnv"),
                            (7, "DosMapCase"), (8, "DosQueryCollate")] {
            let e = find(NLS_BASE + ord).unwrap_or_else(|| panic!("{name} missing from registry"));
            assert_eq!(e.name, name);
            assert_eq!(e.module, "NLS");
        }
        // DOSCALLS 373 is DosQueryDOSProperty (not implemented), not an NLS call.
        assert!(find(373).is_none());
        // DOSCALLS 397 is no DOSCALLS export at all (<bseord.h>); DosQueryCtryInfo is NLS 5.
        assert!(find(397).is_none());

        let e = find(MDM_BASE + 1).expect("mciSendCommand missing from registry");
        assert_eq!(e.name, "mciSendCommand");
        assert_eq!(e.module, "MDM");
    }

    #[test]
    fn test_find_unknown_ordinals() {
        assert!(find(0).is_none());          // not registered
        assert!(find(2048).is_none());        // PMWIN — handled by sub-dispatcher
        assert!(find(u32::MAX).is_none());
    }

    /// Every ordinal listed in the existing dispatch regression guard must
    /// have a registry entry so dispatch behaviour is preserved exactly.
    #[test]
    fn test_all_previously_dispatched_ordinals_present() {
        let expected: &[u32] = &[
            256, 257, 259, 271, 226, 270, 273, 281, 282, 229, 311, 234, 239,
            312, 283, 280, 235, 264, 265, 263, 223, 255, 274, 275, 220, 278,
            299, 304, 324, 326, 328, 329, 331, 333, 334, 335, 337, 339, 340,
            323, 342, 349, 352, 353, 572, 212, 209, 286, 354, 355, 356, 418,
            368, 378, 380, 381, 300, 301, 302, 305, 306, 291, 289, 318,
            322, 319, 321, 317, 258, 261, 279, 267, 219, 276, 277, 284, 231,
            325, 332, 230, 348, 382, 272, 260, 254, 210, 225, 292, 218, 285,
            357, 372, 428, 639, 415, 425, 426, 241, 243, 250, 221, 224, 110,
            8, 75, 84,
        ];
        for &ord in expected {
            assert!(
                find(ord).is_some(),
                "Ordinal {} ({}) missing from registry",
                ord,
                api_trace::ordinal_to_name(ord)
            );
        }
    }
}
