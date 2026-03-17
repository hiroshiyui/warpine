// SPDX-License-Identifier: GPL-3.0-only
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

use std::sync::atomic::Ordering;
use tracing::debug;

use super::constants::{NLS_BASE, MDM_BASE};
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
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_info_seg(a[0],a[1])) },
    ApiEntry { ordinal: 75,  module: "DOSCALLS", name: "DosQueryFileMode16", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_file_mode_16(a[0],a[1])) },
    ApiEntry { ordinal: 84,  module: "DOSCALLS", name: "DosSetFileMode",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_file_mode(a[0],a[1])) },
    ApiEntry { ordinal: 110, module: "DOSCALLS", name: "DosForceDelete",   argc: 1,
               handler: |l,_v,_i,a| { debug!("DosForceDelete stub"); ApiResult::Normal(l.dos_delete(a[0])) } },

    // ── DOSCALLS — init / control ─────────────────────────────────────────
    ApiEntry { ordinal: 209, module: "DOSCALLS", name: "DosSetMaxFH",      argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_max_fh(a[0])) },
    ApiEntry { ordinal: 210, module: "DOSCALLS", name: "DosSetVerify",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_verify(a[0])) },
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

    // ── DOSCALLS — time / misc ────────────────────────────────────────────
    ApiEntry { ordinal: 229, module: "DOSCALLS", name: "DosSleep",         argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_sleep(a[0])) },
    ApiEntry { ordinal: 230, module: "DOSCALLS", name: "DosGetDateTime",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_date_time(a[0])) },
    ApiEntry { ordinal: 231, module: "DOSCALLS", name: "DosDevConfig",     argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_dev_config(a[0],a[1])) },

    // DosExit: stores exit code and signals shutdown; vcpu loop observes flag
    ApiEntry { ordinal: 234, module: "DOSCALLS", name: "DosExit",          argc: 2,
               handler: |l,_v,_i,a| {
                   l.shared.exit_code.store(a[1] as i32, Ordering::Relaxed);
                   l.shared.exit_requested.store(true, Ordering::Relaxed);
                   ApiResult::Normal(0)
               } },

    // ── DOSCALLS — processes ──────────────────────────────────────────────
    ApiEntry { ordinal: 235, module: "DOSCALLS", name: "DosKillProcess",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_kill_process(a[0],a[1])) },

    // ── DOSCALLS — pipes ──────────────────────────────────────────────────
    ApiEntry { ordinal: 239, module: "DOSCALLS", name: "DosCreatePipe",    argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_pipe(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 241, module: "DOSCALLS", name: "DosConnectNPipe",  argc: 1,
               handler: |_l,_v,_i,_a| { debug!("DosConnectNPipe stub"); ApiResult::Normal(0) } },
    ApiEntry { ordinal: 243, module: "DOSCALLS", name: "DosCreateNPipe",   argc: 6,
               handler: |_l,_v,_i,_a| { debug!("DosCreateNPipe stub");   ApiResult::Normal(0) } },
    ApiEntry { ordinal: 250, module: "DOSCALLS", name: "DosSetNPHState",   argc: 2,
               handler: |_l,_v,_i,_a| { debug!("DosSetNPHState stub");    ApiResult::Normal(0) } },

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
    ApiEntry { ordinal: 280, module: "DOSCALLS", name: "DosWaitChild",     argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_wait_child(a[0],a[1],a[2],a[3])) },
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
    ApiEntry { ordinal: 291, module: "DOSCALLS", name: "DosQueryCp",       argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_cp(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 292, module: "DOSCALLS", name: "DosSetDateTime",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_date_time(a[0])) },

    // ── DOSCALLS — memory ─────────────────────────────────────────────────
    ApiEntry { ordinal: 299, module: "DOSCALLS", name: "DosAllocMem",      argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_alloc_mem(a[0],a[1])) },
    ApiEntry { ordinal: 300, module: "DOSCALLS", name: "DosAllocSharedMem", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_alloc_shared_mem(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 301, module: "DOSCALLS", name: "DosGetNamedSharedMem", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_named_shared_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 302, module: "DOSCALLS", name: "DosGetSharedMem",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_shared_mem(a[0],a[1])) },
    ApiEntry { ordinal: 304, module: "DOSCALLS", name: "DosFreeMem",       argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_free_mem(a[0])) },
    ApiEntry { ordinal: 305, module: "DOSCALLS", name: "DosSetMem",        argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_mem(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 306, module: "DOSCALLS", name: "DosQueryMem",      argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_mem(a[0],a[1],a[2])) },

    // ── DOSCALLS — threading ──────────────────────────────────────────────
    // DosCreateThread: ptid, pfn, param, [flags@16 unused], cbStack
    ApiEntry { ordinal: 311, module: "DOSCALLS", name: "DosCreateThread",  argc: 5,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_create_thread(id,a[0],a[1],a[2],a[4])) },
    // DosGetInfoBlocks: needs live vcpu to read FS.base for TIB address
    ApiEntry { ordinal: 312, module: "DOSCALLS", name: "DosGetInfoBlocks", argc: 2,
               handler: |l,v,_i,a| ApiResult::Normal(l.dos_get_info_blocks(v,a[0],a[1])) },

    // ── DOSCALLS — module loading ─────────────────────────────────────────
    ApiEntry { ordinal: 317, module: "DOSCALLS", name: "DosDebug",         argc: 1,
               handler: |_l,_v,_i,_a| { debug!("DosDebug stub"); ApiResult::Normal(87) } },
    ApiEntry { ordinal: 318, module: "DOSCALLS", name: "DosLoadModule",    argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_load_module(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 319, module: "DOSCALLS", name: "DosQueryModuleHandle", argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_module_handle(a[0],a[1])) },
    ApiEntry { ordinal: 321, module: "DOSCALLS", name: "DosQueryProcAddr", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_proc_addr(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 322, module: "DOSCALLS", name: "DosFreeModule",    argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_free_module(a[0])) },
    ApiEntry { ordinal: 323, module: "DOSCALLS", name: "DosQueryAppType",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_app_type(a[0],a[1])) },

    // ── DOSCALLS — event semaphores ───────────────────────────────────────
    ApiEntry { ordinal: 324, module: "DOSCALLS", name: "DosCreateEventSem", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_event_sem(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 325, module: "DOSCALLS", name: "DosOpenEventSem",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_open_event_sem(a[0],a[1])) },
    ApiEntry { ordinal: 326, module: "DOSCALLS", name: "DosCloseEventSem", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close_event_sem(a[0])) },
    ApiEntry { ordinal: 328, module: "DOSCALLS", name: "DosPostEventSem",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_post_event_sem(a[0])) },
    ApiEntry { ordinal: 329, module: "DOSCALLS", name: "DosWaitEventSem",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_wait_event_sem(a[0],a[1])) },

    // ── DOSCALLS — mutex semaphores ───────────────────────────────────────
    ApiEntry { ordinal: 331, module: "DOSCALLS", name: "DosCreateMutexSem", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_mutex_sem(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 332, module: "DOSCALLS", name: "DosOpenMutexSem",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_open_mutex_sem(a[0],a[1])) },
    ApiEntry { ordinal: 333, module: "DOSCALLS", name: "DosCloseMutexSem", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close_mutex_sem(a[0])) },
    ApiEntry { ordinal: 334, module: "DOSCALLS", name: "DosRequestMutexSem", argc: 2,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_request_mutex_sem(id,a[0],a[1])) },
    ApiEntry { ordinal: 335, module: "DOSCALLS", name: "DosReleaseMutexSem", argc: 1,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_release_mutex_sem(id,a[0])) },

    // ── DOSCALLS — muxwait semaphores ─────────────────────────────────────
    ApiEntry { ordinal: 337, module: "DOSCALLS", name: "DosCreateMuxWaitSem", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_mux_wait_sem(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 339, module: "DOSCALLS", name: "DosCloseMuxWaitSem",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close_mux_wait_sem(a[0])) },
    ApiEntry { ordinal: 340, module: "DOSCALLS", name: "DosWaitMuxWaitSem",   argc: 3,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_wait_mux_wait_sem(id,a[0],a[1],a[2])) },
    ApiEntry { ordinal: 342, module: "DOSCALLS", name: "DosDeleteMuxWaitSem", argc: 2,
               handler: |_l,_v,_i,_a| { debug!("DosDeleteMuxWaitSem stub"); ApiResult::Normal(0) } },

    // ── DOSCALLS — system info ────────────────────────────────────────────
    ApiEntry { ordinal: 348, module: "DOSCALLS", name: "DosQuerySysInfo",  argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_sys_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 349, module: "DOSCALLS", name: "DosWaitThread",    argc: 2,
               handler: |l,_v,id,a| ApiResult::Normal(l.dos_wait_thread(id,a[0])) },

    // ── DOSCALLS — resources ──────────────────────────────────────────────
    ApiEntry { ordinal: 352, module: "DOSCALLS", name: "DosGetResource",   argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_get_resource(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 353, module: "DOSCALLS", name: "DosFreeResource",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_free_resource(a[0])) },

    // ── DOSCALLS — exception handling ─────────────────────────────────────
    ApiEntry { ordinal: 354, module: "DOSCALLS", name: "DosSetExceptionHandler",   argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_exception_handler(a[0])) },
    ApiEntry { ordinal: 355, module: "DOSCALLS", name: "DosUnsetExceptionHandler", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_unset_exception_handler(a[0])) },
    ApiEntry { ordinal: 356, module: "DOSCALLS", name: "DosRaiseException", argc: 1,
               handler: |_l,_v,_i,_a| { debug!("DosRaiseException stub"); ApiResult::Normal(0) } },
    ApiEntry { ordinal: 357, module: "DOSCALLS", name: "DosUnwindException", argc: 3,
               handler: |_l,_v,_i,_a| { debug!("DosUnwindException stub"); ApiResult::Normal(0) } },

    // ── DOSCALLS — process state / signals ───────────────────────────────
    ApiEntry { ordinal: 368, module: "DOSCALLS", name: "DosQuerySysState", argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_sys_state(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 372, module: "DOSCALLS", name: "DosEnumAttribute", argc: 7,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_enum_attribute(a[0],a[1],a[2],a[3],a[4],a[5],a[6])) },
    ApiEntry { ordinal: 378, module: "DOSCALLS", name: "DosSetSignalExceptionFocus", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_signal_exception_focus(a[0])) },
    ApiEntry { ordinal: 380, module: "DOSCALLS", name: "DosEnterMustComplete", argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_enter_must_complete(a[0])) },
    ApiEntry { ordinal: 381, module: "DOSCALLS", name: "DosExitMustComplete",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_exit_must_complete(a[0])) },
    ApiEntry { ordinal: 382, module: "DOSCALLS", name: "DosSetRelMaxFH",   argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_set_rel_max_fh(a[0],a[1])) },
    ApiEntry { ordinal: 397, module: "DOSCALLS", name: "DosQueryCtryInfo", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_ctry_info(a[0],a[1],a[2],a[3])) },
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
    ApiEntry { ordinal: 572, module: "DOSCALLS", name: "DosQueryResourceSize", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_resource_size(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: 639, module: "DOSCALLS", name: "DosProtectSetFileLocks", argc: 6,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_protect_set_file_locks(a[0],a[1],a[2],a[3],a[4],a[5])) },

    // ── QUECALLS (base 1024) ──────────────────────────────────────────────
    ApiEntry { ordinal: 1024 + 9,  module: "QUECALLS", name: "DosReadQueue",   argc: 8,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_read_queue(a[0],a[1],a[2],a[3],a[4],a[5],a[6],a[7])) },
    ApiEntry { ordinal: 1024 + 10, module: "QUECALLS", name: "DosPurgeQueue",  argc: 1,
               handler: |l,_v,_i,a| { l.dos_purge_queue(a[0]); ApiResult::Normal(0) } },
    ApiEntry { ordinal: 1024 + 11, module: "QUECALLS", name: "DosCloseQueue",  argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_close_queue(a[0])) },
    ApiEntry { ordinal: 1024 + 12, module: "QUECALLS", name: "DosQueryQueue",  argc: 2,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_queue(a[0],a[1])) },
    ApiEntry { ordinal: 1024 + 14, module: "QUECALLS", name: "DosWriteQueue",  argc: 5,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_write_queue(a[0],a[1],a[2],a[3],a[4])) },
    ApiEntry { ordinal: 1024 + 15, module: "QUECALLS", name: "DosOpenQueue",   argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_open_queue(a[0],a[1],a[2])) },
    ApiEntry { ordinal: 1024 + 16, module: "QUECALLS", name: "DosCreateQueue", argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_create_queue(a[0],a[1],a[2])) },

    // ── NLS — National Language Support (base NLS_BASE = 7168) ───────────
    // Ordinal 5: dual behavior — acts as DosQueryCp (cb < 44) or
    //            DosQueryCtryInfo (cb ≥ 44) depending on caller's buffer size
    ApiEntry { ordinal: NLS_BASE + 5, module: "NLS", name: "NlsQueryCp",       argc: 3,
               handler: |l,_v,_i,a| {
                   if a[0] >= 44 {
                       ApiResult::Normal(l.dos_query_ctry_info(a[0], a[1], a[2], 0))
                   } else {
                       ApiResult::Normal(l.dos_query_cp(a[0], a[1], a[2]))
                   }
               } },
    ApiEntry { ordinal: NLS_BASE + 6, module: "NLS", name: "NlsQueryCtryInfo", argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.dos_query_ctry_info(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: NLS_BASE + 7, module: "NLS", name: "NlsMapCase",       argc: 3,
               handler: |l,_v,_i,a| {
                   let (cb, pch) = (a[0], a[2]);
                   for i in 0..cb {
                       if let Some(ch) = l.guest_read::<u8>(pch + i) {
                           if ch.is_ascii_lowercase() {
                               let _ = l.guest_write::<u8>(pch + i, ch.to_ascii_uppercase());
                           }
                       }
                   }
                   ApiResult::Normal(0)
               } },
    ApiEntry { ordinal: NLS_BASE + 8, module: "NLS", name: "NlsGetDBCSEv",     argc: 3,
               handler: |l,_v,_i,a| {
                   // Returns empty DBCS lead-byte table (Western locale only)
                   if a[2] != 0 && a[0] >= 2 {
                       let _ = l.guest_write::<u16>(a[2], 0);
                   }
                   ApiResult::Normal(0)
               } },

    // ── MDM — MMPM/2 Media Device Manager (base MDM_BASE = 10240) ────────
    ApiEntry { ordinal: MDM_BASE + 1, module: "MDM", name: "mciSendCommand",   argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.mci_send_command(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: MDM_BASE + 2, module: "MDM", name: "mciSendString",    argc: 4,
               handler: |l,_v,_i,a| ApiResult::Normal(l.mci_send_string(a[0],a[1],a[2],a[3])) },
    ApiEntry { ordinal: MDM_BASE + 3, module: "MDM", name: "mciFreeBlock",     argc: 1,
               handler: |l,_v,_i,a| ApiResult::Normal(l.mci_free_block(a[0])) },
    ApiEntry { ordinal: MDM_BASE + 4, module: "MDM", name: "mciGetLastError",  argc: 3,
               handler: |l,_v,_i,a| ApiResult::Normal(l.mci_get_last_error(a[0],a[1],a[2])) },
];

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::api_trace;

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

        let e = find(NLS_BASE + 5).expect("NlsQueryCp missing from registry");
        assert_eq!(e.name, "NlsQueryCp");
        assert_eq!(e.module, "NLS");

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
            368, 378, 380, 381, 300, 301, 302, 305, 306, 291, 289, 397, 318,
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
