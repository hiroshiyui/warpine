// SPDX-License-Identifier: GPL-3.0-or-later
//
//! `WARPINE.DLL`: Warpine's own extensions for programs that can be
//! recompiled (ADR-0039). Not an OS/2 module — a program must check that
//! `DosLoadModule("WARPINE")` succeeds, so it still runs on OS/2.
//!
//! The 64-bit time calls give a signed 64-bit count of seconds since
//! 1970-01-01 00:00 UTC (`LONGLONG`, little-endian) on the session clock
//! (`clock.rs`), which neither ends in 2038 (a signed 32-bit `time_t`) nor
//! in 2106 (`QSV_TIME_LOW`, Open Watcom's unsigned `time_t`). All take
//! pointers, so the `_System` (cdecl) argument list stays 32-bit:
//!
//! | Ordinal | Call | |
//! |---|---|---|
//! | 1 | `WrpTime64(PLONGLONG ptime)` | the current time |
//! | 2 | `WrpGetDateTime64(PDATETIME pdt, PLONGLONG ptime)` | the current time as a local `DATETIME` and as seconds, read once |
//! | 3 | `WrpLocalTime64(const LONGLONG *ptime, PDATETIME pdt)` | seconds → local `DATETIME` |
//! | 4 | `WrpMakeTime64(const DATETIME *pdt, PLONGLONG ptime)` | local `DATETIME` → seconds |
//! | 5 | `WrpBuiltinWindowProc(HWND, ULONG, MPARAM, MPARAM)` | the window procedure of the built-in classes (`pm_window.rs`): programs get its address from `WinSubclassWindow`, `QWP_PFNWP` or `WinQueryClassInfo`, not by name |
//!
//! Errors: `ERROR_INVALID_PARAMETER` for a null or unreadable pointer, or a
//! time whose year is outside 1–9999; `ERROR_TS_DATETIME` for a `DATETIME`
//! field out of range. `pdt->hundredths` is 0 from `WrpLocalTime64` and
//! ignored by `WrpMakeTime64`, as are the weekday and time zone.

use log::debug;
use super::MutexExt;

use super::clock::{self, LocalTime};
use super::constants::*;

/// The years the conversions accept.
pub const WRP_MIN_YEAR: i32 = 1;
pub const WRP_MAX_YEAR: i32 = 9999;

const MS_PER_SECOND: i64 = 1000;

impl super::Loader {
    /// Run the `_DLL_InitTerm(hmod, 0)` of the modules waiting for it
    /// (`DllManager::take_pending_init`: a module after those it imports)
    /// as one call chain, then go on as `then` says: `WARPINE.6`, where a
    /// program starts when DLLs it imports have such routines, returns into
    /// the program's entry point (ADR-0059).
    pub(crate) fn dll_init_chain(&self, vcpu: u32, then: super::pm_hook::ChainThen) -> super::ApiResult {
        let pending = self.shared.dll_mgr.lock_or_recover().take_pending_init();
        let steps = pending.into_iter()
            .map(|(hmod, entry)| super::pm_hook::CallStep::hook(entry, vec![hmod, 0], false))
            .collect();
        self.start_chain(super::pm_hook::CallChain::new(vcpu, steps, false, then))
    }

    /// `WrpStartProcess`'s `_DLL_InitTerm` calls returned: into the
    /// program, or, when one returned 0, end it with
    /// `ERROR_INIT_ROUTINE_FAILED` (OS/2 does not start such a program).
    pub(crate) fn start_inits_done(&self, chain: &super::pm_hook::CallChain) -> super::pm_hook::ChainEnd {
        if chain.results.contains(&0) {
            log::warn!("a DLL's _DLL_InitTerm failed: the program does not start");
            let _ = self.write_handle(super::constants::STDERR_HANDLE,
                b"The initialization routine of a DLL the program uses failed; the program does not start.\r\n");
            return super::pm_hook::ChainEnd::Exit(super::constants::ERROR_INIT_ROUTINE_FAILED);
        }
        super::pm_hook::ChainEnd::Return(0)
    }

    /// The chain of `DosLoadModule`'s `_DLL_InitTerm` calls returned: the
    /// handle goes to `*phmod` when every routine said TRUE; else
    /// `ERROR_INIT_ROUTINE_FAILED`.
    pub(crate) fn dll_inits_done(&self, hmod: u32, phmod: u32, chain: &super::pm_hook::CallChain) -> u32 {
        if chain.results.contains(&0) {
            log::warn!("DosLoadModule: a _DLL_InitTerm of module {hmod:#x} or a module it imports failed");
            if phmod != 0 { let _ = self.guest_write::<u32>(phmod, 0); }
            return super::constants::ERROR_INIT_ROUTINE_FAILED;
        }
        if phmod != 0 { let _ = self.guest_write::<u32>(phmod, hmod); }
        super::constants::NO_ERROR
    }

    /// Write a `LONGLONG` at `p`.
    fn write_longlong(&self, p: u32, v: i64) -> u32 {
        match (p != 0).then(|| self.guest_write_bytes(p, &v.to_le_bytes())).flatten() {
            Some(()) => NO_ERROR,
            None => ERROR_INVALID_PARAMETER,
        }
    }

    /// `secs` as local time within the accepted years.
    fn wrp_local(secs: i64) -> Option<LocalTime> {
        clock::unix_to_local(secs).filter(|lt| (WRP_MIN_YEAR..=WRP_MAX_YEAR).contains(&lt.year))
    }

    /// `WrpRegisterUnwindTables(begin)` (WARPINE.7, ADR-0107): the
    /// `.eh_frame` records starting at `begin` (ending with a zero word)
    /// are the unwind tables of the module whose object holds `begin`.
    /// `ERROR_INVALID_ADDRESS` when no module of the process holds it.
    pub fn wrp_register_unwind_tables(&self, begin: u32) -> u32 {
        if self.shared.dll_mgr.lock_or_recover().register_unwind_tables(begin) { NO_ERROR } else { ERROR_INVALID_ADDRESS }
    }

    /// `WrpDeregisterUnwindTables(begin)` (WARPINE.8): forget them
    /// (a DLL's runtime ends; freeing the DLL forgets them too).
    pub fn wrp_deregister_unwind_tables(&self, begin: u32) -> u32 {
        if self.shared.dll_mgr.lock_or_recover().deregister_unwind_tables(begin) { NO_ERROR } else { ERROR_INVALID_ADDRESS }
    }

    /// `WrpQueryUnwindTables(pc, pBegin)` (WARPINE.9): the unwind tables
    /// of the module whose code holds `pc`, for an unwinder that reached
    /// another module's frame. `ERROR_INVALID_ADDRESS` when that module
    /// registered none (or no module holds `pc`).
    pub fn wrp_query_unwind_tables(&self, pc: u32, p_begin: u32) -> u32 {
        if p_begin == 0 { return ERROR_INVALID_PARAMETER; }
        let Some(begin) = self.shared.dll_mgr.lock_or_recover().unwind_tables_for(pc) else { return ERROR_INVALID_ADDRESS };
        if self.guest_write::<u32>(p_begin, begin).is_none() { return ERROR_INVALID_PARAMETER; }
        NO_ERROR
    }

    /// `WrpQueryRuntimeBlock(ppBlock)` (WARPINE.10, ADR-0107): the address
    /// of the calling thread's runtime block, `RUNTIME_BLOCK_SIZE` bytes
    /// that every module of the process shares on that thread (the same
    /// address on every thread, mapped to each thread's own page).
    pub fn wrp_query_runtime_block(&self, pp_block: u32) -> u32 {
        if pp_block == 0 || self.guest_write::<u32>(pp_block, RUNTIME_BLOCK).is_none() { return ERROR_INVALID_PARAMETER; }
        NO_ERROR
    }

    /// `WrpTime64` (WARPINE.1).
    pub fn wrp_time64(&self, p_time: u32) -> u32 {
        debug!("  WrpTime64");
        self.write_longlong(p_time, self.shared.clock.now_secs())
    }

    /// `WrpGetDateTime64` (WARPINE.2): both forms of one reading.
    pub fn wrp_get_date_time64(&self, p_dt: u32, p_time: u32) -> u32 {
        debug!("  WrpGetDateTime64");
        if p_dt == 0 || p_time == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let ms = self.shared.clock.now_ms();
        let secs = ms.div_euclid(MS_PER_SECOND);
        let Some(lt) = Self::wrp_local(secs) else { return ERROR_INVALID_PARAMETER };
        let rc = self.write_datetime(p_dt, &lt, (ms.rem_euclid(MS_PER_SECOND) / 10) as u8);
        if rc != NO_ERROR {
            return rc;
        }
        self.write_longlong(p_time, secs)
    }

    /// `WrpLocalTime64` (WARPINE.3).
    pub fn wrp_local_time64(&self, p_time: u32, p_dt: u32) -> u32 {
        let Some(b) = (p_time != 0).then(|| self.guest_read_bytes(p_time, 8)).flatten() else {
            return ERROR_INVALID_PARAMETER;
        };
        let secs = i64::from_le_bytes(b.try_into().expect("8 bytes"));
        debug!("  WrpLocalTime64({secs})");
        if p_dt == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        match Self::wrp_local(secs) {
            Some(lt) => self.write_datetime(p_dt, &lt, 0),
            None => ERROR_INVALID_PARAMETER,
        }
    }

    /// `WrpMakeTime64` (WARPINE.4).
    pub fn wrp_make_time64(&self, p_dt: u32, p_time: u32) -> u32 {
        let Some((lt, _)) = (p_dt != 0).then(|| self.read_datetime(p_dt)).flatten() else {
            return ERROR_INVALID_PARAMETER;
        };
        debug!("  WrpMakeTime64({lt:?})");
        if !(WRP_MIN_YEAR..=WRP_MAX_YEAR).contains(&lt.year) {
            return ERROR_INVALID_PARAMETER;
        }
        match clock::local_to_unix(&lt) {
            Some(secs) => self.write_longlong(p_time, secs),
            None => ERROR_TS_DATETIME,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::loader::managers::LoadedDll;

    const OUT: u32 = 0x0010_0000;

    /// A loader whose program has objects at 64 KB and 128 KB, and a DLL
    /// (handle 0x1001) with one at 0x0200_0000.
    fn with_modules() -> Loader {
        let l = Loader::new_mock();
        let mut d = l.shared.dll_mgr.lock_or_recover();
        d.exe_objects = vec![(0x1_0000, 0x1000), (0x2_0000, 0x1000)];
        d.register(LoadedDll { name: "X".into(), handle: 0x1001, object_bases: vec![0x0200_0000],
                               object_sizes: vec![0x2000], exports_by_ordinal: Default::default(),
                               exports_by_name: Default::default(), ref_count: 1, initterm_addr: None, is_builtin: false });
        drop(d);
        l
    }

    #[test]
    fn unwind_tables_are_found_by_an_address_in_their_module() {
        let l = with_modules();
        assert_eq!(l.wrp_register_unwind_tables(0x2_0100), NO_ERROR, "the program's tables, in its data object");
        assert_eq!(l.wrp_register_unwind_tables(0x0200_0800), NO_ERROR, "the DLL's");
        assert_eq!(l.wrp_register_unwind_tables(0x0300_0000), ERROR_INVALID_ADDRESS, "in no module");
        assert_eq!(l.wrp_query_unwind_tables(0x1_0010, OUT), NO_ERROR, "code of the program");
        assert_eq!(l.guest_read::<u32>(OUT), Some(0x2_0100));
        assert_eq!(l.wrp_query_unwind_tables(0x0200_0010, OUT), NO_ERROR, "code of the DLL");
        assert_eq!(l.guest_read::<u32>(OUT), Some(0x0200_0800));
        assert_eq!(l.wrp_query_unwind_tables(0x0400_0000, OUT), ERROR_INVALID_ADDRESS);
        assert_eq!(l.wrp_query_unwind_tables(0x1_0010, 0), ERROR_INVALID_PARAMETER);
        assert_eq!(l.wrp_deregister_unwind_tables(0x0200_0800), NO_ERROR);
        assert_eq!(l.wrp_deregister_unwind_tables(0x0200_0800), ERROR_INVALID_ADDRESS, "already gone");
        assert_eq!(l.wrp_query_unwind_tables(0x0200_0010, OUT), ERROR_INVALID_ADDRESS);
    }

    #[test]
    fn freeing_a_dll_forgets_its_unwind_tables() {
        let l = with_modules();
        assert_eq!(l.wrp_register_unwind_tables(0x0200_0800), NO_ERROR);
        assert!(l.shared.dll_mgr.lock_or_recover().decrement_refcount(0x1001).is_some());
        assert_eq!(l.wrp_query_unwind_tables(0x0200_0010, OUT), ERROR_INVALID_ADDRESS);
    }

    #[test]
    fn the_runtime_block_lies_in_the_thread_local_page_past_its_words() {
        let l = Loader::new_mock();
        assert_eq!(l.wrp_query_runtime_block(OUT), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(OUT), Some(RUNTIME_BLOCK));
        assert_eq!(l.wrp_query_runtime_block(0), ERROR_INVALID_PARAMETER);
    }

    /// The _DLL_InitTerm routines waiting run as one chain (WARPINE.6,
    /// DosLoadModule); DosLoadModule's handle is written only when every
    /// one said TRUE.
    #[test]
    fn pending_dll_inits_run_as_one_chain() {
        use super::super::pm_hook::{CallStep, ChainEnd, ChainThen};
        use super::super::{ApiResult, Loader};
        let l = Loader::new_mock();
        {
            let mut m = l.shared.dll_mgr.lock_or_recover();
            m.add_pending_init(0x1001, 0x5000);
            m.add_pending_init(0x1002, 0x6000);
        }
        let ApiResult::CallChain(mut c) = l.dll_init_chain(0, ChainThen::LoadModule { hmod: 0x1002, phmod: 0x3000 })
            else { panic!() };
        assert_eq!(c.steps.front(), Some(&CallStep::hook(0x5000, vec![0x1001, 0], false)), "the imported one first");
        assert!(!l.shared.dll_mgr.lock_or_recover().has_pending_init());
        c.steps.clear();
        c.returned(1, false);
        c.returned(1, false);
        assert_eq!(l.finish_chain(&c), ChainEnd::Return(NO_ERROR));
        assert_eq!(l.guest_read::<u32>(0x3000), Some(0x1002));
        c.returned(0, false);
        assert_eq!(l.finish_chain(&c), ChainEnd::Return(ERROR_INIT_ROUTINE_FAILED));
        assert_eq!(l.guest_read::<u32>(0x3000), Some(0), "no handle");
    }

    /// At process start (WrpStartProcess): into the program when every
    /// _DLL_InitTerm said TRUE, else the process ends with
    /// ERROR_INIT_ROUTINE_FAILED through DosExit (ADR-0063).
    #[test]
    fn a_failed_startup_routine_ends_the_program() {
        use super::super::pm_hook::{ChainEnd, ChainThen};
        use super::super::{ApiResult, Loader};
        let l = Loader::new_mock();
        l.shared.dll_mgr.lock_or_recover().add_pending_init(0x1001, 0x5000);
        let ApiResult::CallChain(mut c) = l.dll_init_chain(0, ChainThen::StartProcess) else { panic!() };
        c.steps.clear();
        c.returned(1, false);
        assert_eq!(l.finish_chain(&c), ChainEnd::Return(0));
        c.returned(0, false);
        assert_eq!(l.finish_chain(&c), ChainEnd::Exit(ERROR_INIT_ROUTINE_FAILED));
    }

    use super::*;
    use crate::loader::Loader;

    use crate::loader::mutex_ext::MutexExt;

    /// 2200-01-01 00:00 UTC.
    const Y2200: i64 = 7_258_118_400;

    /// A mock loader and guest buffers for a `DATETIME` and a `LONGLONG`.
    fn setup() -> (Loader, u32, u32) {
        let l = Loader::new_mock();
        let buf = l.shared.mem_mgr.lock_or_recover().alloc(64).unwrap();
        (l, buf, buf + 32)
    }

    fn longlong(l: &Loader, p: u32) -> i64 {
        i64::from_le_bytes(l.guest_read_bytes(p, 8).unwrap().try_into().unwrap())
    }

    fn year(l: &Loader, p_dt: u32) -> u16 {
        l.guest_read::<u16>(p_dt + 6).unwrap()
    }

    #[test]
    fn time64_follows_the_session_clock_past_2106() {
        let (l, p_dt, p_time) = setup();
        l.shared.clock.set_now_ms(Y2200 * MS_PER_SECOND);
        assert_eq!(l.wrp_time64(p_time), NO_ERROR);
        assert!((longlong(&l, p_time) - Y2200).abs() < 5);
        assert_eq!(l.wrp_get_date_time64(p_dt, p_time), NO_ERROR);
        assert!((longlong(&l, p_time) - Y2200).abs() < 5);
        assert!((2199..=2200).contains(&year(&l, p_dt)), "year {}", year(&l, p_dt));
        assert_eq!(l.wrp_time64(0), ERROR_INVALID_PARAMETER);
        assert_eq!(l.wrp_get_date_time64(0, p_time), ERROR_INVALID_PARAMETER);
    }

    #[test]
    fn local_and_make_time64_round_trip() {
        let (l, p_dt, p_time) = setup();
        for secs in [-86_400, 0, (1 << 31), (1 << 32), Y2200] {
            l.guest_write_bytes(p_time, &secs.to_le_bytes()).unwrap();
            assert_eq!(l.wrp_local_time64(p_time, p_dt), NO_ERROR);
            l.guest_write_bytes(p_time, &[0; 8]).unwrap();
            assert_eq!(l.wrp_make_time64(p_dt, p_time), NO_ERROR);
            assert_eq!(longlong(&l, p_time), secs);
        }
    }

    #[test]
    fn conversions_reject_bad_input() {
        let (l, p_dt, p_time) = setup();
        // Far beyond year 9999.
        l.guest_write_bytes(p_time, &(Y2200 * 1000).to_le_bytes()).unwrap();
        assert_eq!(l.wrp_local_time64(p_time, p_dt), ERROR_INVALID_PARAMETER);
        assert_eq!(l.wrp_local_time64(0, p_dt), ERROR_INVALID_PARAMETER);
        // 2039-02-29 does not exist.
        let lt = LocalTime::new(2039, 2, 29, 0, 0, 0);
        assert_eq!(l.write_datetime(p_dt, &lt, 0), NO_ERROR);
        assert_eq!(l.wrp_make_time64(p_dt, p_time), ERROR_TS_DATETIME);
        assert_eq!(l.wrp_make_time64(0, p_time), ERROR_INVALID_PARAMETER);
        let lt = LocalTime::new(2039, 3, 1, 0, 0, 0);
        assert_eq!(l.write_datetime(p_dt, &lt, 0), NO_ERROR);
        assert_eq!(l.wrp_make_time64(p_dt, 0), ERROR_INVALID_PARAMETER);
    }
}
