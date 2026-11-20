// SPDX-License-Identifier: GPL-3.0-or-later
//
//! `DosExitList` (DOSCALLS 296): routines a process runs when it ends.
//!
//! Behaviour from IBM's *Control Program Programming Reference*
//! (DosExitList) and *Control Program Guide* ("Creating an Exit List"):
//! - the low byte of the order code is the action (`EXLST_ADD`,
//!   `EXLST_REMOVE`, `EXLST_EXIT`); the high byte of its low word is the
//!   invocation order, valid only with `EXLST_ADD` (else 0); the high word
//!   must be 0;
//! - routines run in ascending order (0 first, 255 last); routines of the
//!   same order run last-added first;
//! - before each routine the stack is reset to its initial value; the
//!   routine gets the termination code (`TC_EXIT`, `TC_HARDERROR`,
//!   `TC_TRAP`, `TC_KILLPROCESS`, `TC_EXCEPTION`) at `[ESP+4]` and ends with
//!   `DosExitList(EXLST_EXIT, 0)`, which goes on with the next routine;
//! - all other threads are ended first; a routine may add or remove other
//!   routines while the list runs;
//! - "if thread 1 has already exited … the exit-list functions cannot run".
//!
//! Warpine's choices where the documents say nothing (or describe a hang):
//! - the routines run on thread 1, which must still be running: a
//!   `DosExit(EXIT_PROCESS)` or a fault on another thread asks thread 1 to
//!   run them (as a termination from outside does) and waits at most
//!   `KILL_GRACE` for it; a thread 1 in 16-bit code ends the process
//!   without them;
//! - a routine that returns, calls `DosExit`, or has an unhandled
//!   exception has ended like one calling `EXLST_EXIT` (OS/2 would hang);
//! - an `EXLST_EXIT` outside exit-list processing is `ERROR_INVALID_FUNCTION`;
//!   removing a routine that is not on the list and adding a NULL routine
//!   are `ERROR_INVALID_DATA`; adding a routine already on the list moves it
//!   to its new order; more than `MAX_EXIT_ROUTINES` is `ERROR_NOT_ENOUGH_MEMORY`;
//! - the exception handler chain of the exit-list thread is emptied before
//!   each routine (its registration records were on the reset stack).

use std::sync::atomic::Ordering;
use std::thread::ThreadId;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::vm_backend::VcpuBackend;
use super::{ApiResult, CallbackFrame};
use log::{debug, warn};

/// Most routines on a process's exit list.
pub const MAX_EXIT_ROUTINES: usize = 256;
/// Bits of the order code: action, invocation order, reserved high word.
const ACTION_MASK: u32 = 0xFF;
const ORDER_SHIFT: u32 = 8;
const ORDER_MASK: u32 = 0xFF;
const RESERVED_MASK: u32 = 0xFFFF_0000;
/// Bytes the routine's call frame takes: return address + termination code.
const ROUTINE_FRAME: u32 = 8;
/// How often a thread waiting for thread 1 to end the process looks.
const THREAD_1_POLL: std::time::Duration = std::time::Duration::from_millis(20);
/// EFLAGS bit 1 is reserved and always set.
const ROUTINE_RFLAGS: u64 = 2;

/// A parsed `DosExitList` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitListAction {
    Add { order: u8 },
    Remove,
    Exit,
}

/// Parse a `DosExitList` order code.
pub fn parse_order_code(ordercode: u32) -> Result<ExitListAction, u32> {
    if ordercode & RESERVED_MASK != 0 {
        return Err(ERROR_INVALID_DATA);
    }
    let order = ((ordercode >> ORDER_SHIFT) & ORDER_MASK) as u8;
    match ordercode & ACTION_MASK {
        EXLST_ADD => Ok(ExitListAction::Add { order }),
        EXLST_REMOVE | EXLST_EXIT if order != 0 => Err(ERROR_INVALID_DATA),
        EXLST_REMOVE => Ok(ExitListAction::Remove),
        EXLST_EXIT => Ok(ExitListAction::Exit),
        _ => Err(ERROR_INVALID_FUNCTION),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExitRoutine {
    order: u8,
    pfn: u32,
    /// When it was added (later = called first among the same order).
    seq: u64,
}

/// The exit-list processing under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitRun {
    /// The vCPU (thread) running the routines.
    pub vcpu_id: u32,
    /// Its host thread (`Loader::shutting_down` is false there only).
    pub host: ThreadId,
    /// Termination code passed to every routine.
    pub tc: u32,
    /// The process's exit code once the list is done.
    pub rc: u32,
    /// Stack pointer each routine starts from.
    pub initial_esp: u32,
}

/// A process's exit list (pure bookkeeping).
#[derive(Debug, Default)]
pub struct ExitList {
    routines: Vec<ExitRoutine>,
    next_seq: u64,
    run: Option<ExitRun>,
    /// Processing has run (the process is ending); it never runs twice.
    done: bool,
}

impl ExitList {
    /// `EXLST_ADD`.
    pub fn add(&mut self, order: u8, pfn: u32) -> u32 {
        if pfn == 0 {
            return ERROR_INVALID_DATA;
        }
        self.routines.retain(|r| r.pfn != pfn);
        if self.routines.len() >= MAX_EXIT_ROUTINES {
            return ERROR_NOT_ENOUGH_MEMORY;
        }
        self.routines.push(ExitRoutine { order, pfn, seq: self.next_seq });
        self.next_seq += 1;
        NO_ERROR
    }

    /// `EXLST_REMOVE`.
    pub fn remove(&mut self, pfn: u32) -> u32 {
        let before = self.routines.len();
        self.routines.retain(|r| r.pfn != pfn);
        if self.routines.len() == before { ERROR_INVALID_DATA } else { NO_ERROR }
    }

    pub fn len(&self) -> usize { self.routines.len() }
    pub fn is_empty(&self) -> bool { self.routines.is_empty() }

    /// Begin exit-list processing; false when the list is empty or has
    /// already run (or runs).
    pub fn start(&mut self, run: ExitRun) -> bool {
        if self.run.is_some() || self.done || self.routines.is_empty() {
            return false;
        }
        self.run = Some(run);
        true
    }

    pub fn run(&self) -> Option<ExitRun> { self.run }

    /// The next routine to call, taken off the list: lowest order first,
    /// last added first among equals.
    pub fn take_next(&mut self) -> Option<u32> {
        let i = (0..self.routines.len())
            .min_by_key(|&i| (self.routines[i].order, std::cmp::Reverse(self.routines[i].seq)))?;
        Some(self.routines.remove(i).pfn)
    }

    /// The processing is over: the exit code to end the process with.
    pub fn finish(&mut self) -> Option<u32> {
        self.done = true;
        self.run.take().map(|r| r.rc)
    }
}

/// How `Loader::begin_exit_list` went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitListStart {
    /// No exit list runs (empty, already ran, or thread 1 has ended).
    NotStarted,
    /// The vCPU now runs the first routine.
    Running,
    /// No routine could be called: the process is ending.
    Finished,
}

impl super::Loader {
    /// DosExitList (ordinal 296).
    pub(crate) fn dos_exit_list(&self, vcpu_id: u32, ordercode: u32, pfn: u32) -> ApiResult {
        let action = match parse_order_code(ordercode) {
            Ok(a) => a,
            Err(e) => return ApiResult::Normal(e),
        };
        let mut list = self.shared.threads.exit_list.lock_or_recover();
        match action {
            ExitListAction::Add { order } => ApiResult::Normal(list.add(order, pfn)),
            ExitListAction::Remove => ApiResult::Normal(list.remove(pfn)),
            ExitListAction::Exit if list.run().is_some_and(|r| r.vcpu_id == vcpu_id) => ApiResult::ExitListNext,
            ExitListAction::Exit => ApiResult::Normal(ERROR_INVALID_FUNCTION),
        }
    }

    /// `DosExit(EXIT_PROCESS)` on thread `vcpu_id`: on a thread other
    /// than thread 1, with routines on the list and thread 1 running,
    /// thread 1 runs them (see [`Self::wait_for_thread_1_to_end`]); true
    /// then, and the caller's thread ends. False: the caller ends the
    /// process itself.
    pub(crate) fn exit_on_thread_1(&self, vcpu_id: u32, rc: u32) -> bool {
        if super::threads::tid_of_vcpu(vcpu_id) == super::threads::MAIN_TID
            || self.shared.threads.exit_list.lock_or_recover().is_empty()
            || !self.shared.threads.table.lock_or_recover().is_running(super::threads::MAIN_TID) {
            return false;
        }
        debug!("  [VCPU {}] DosExit(EXIT_PROCESS): thread 1 runs the exit list", vcpu_id);
        self.wait_for_thread_1_to_end(rc, TC_EXIT);
        true
    }

    /// Ask thread 1 to end the process with `rc`, its exit list getting
    /// `tc` (as a termination from outside, `signals.rs`), and wait until
    /// it has taken over or the process ends: at most `KILL_GRACE`, then
    /// the process ends without the list.
    pub(crate) fn wait_for_thread_1_to_end(&self, rc: u32, tc: u32) {
        super::signals::request_termination(&self.shared, rc, tc);
        let deadline = std::time::Instant::now() + super::signals::KILL_GRACE;
        while !self.shutting_down() && std::time::Instant::now() < deadline {
            std::thread::sleep(THREAD_1_POLL);
        }
        if !self.shutting_down() {
            warn!("  thread 1 did not end the process within {:?}; it ends now", super::signals::KILL_GRACE);
            self.request_process_exit(rc);
        }
    }

    /// Whether thread `vcpu_id` runs the exit list.
    pub(crate) fn runs_exit_list(&self, vcpu_id: u32) -> bool {
        self.shared.threads.exit_list_active.load(Ordering::Acquire)
            && self.shared.threads.exit_list.lock_or_recover().run().is_some_and(|r| r.vcpu_id == vcpu_id)
    }

    /// Whether thread `vcpu_id` must stop: another thread runs the exit list.
    pub(crate) fn stopped_by_exit_list(&self, vcpu_id: u32) -> bool {
        self.shared.threads.exit_list_active.load(Ordering::Acquire)
            && self.shared.threads.exit_list.lock_or_recover().run().is_some_and(|r| r.vcpu_id != vcpu_id)
    }

    /// End the process with `rc`, unless another thread runs the exit list
    /// (it ends the process when the list is done).
    pub(crate) fn end_process(&self, rc: u32) {
        if !self.shared.threads.exit_list_active.load(Ordering::Acquire) {
            self.request_process_exit(rc);
        }
    }

    /// Claim exit-list processing for thread `vcpu_id` (the process ends
    /// with `rc`, routines get `tc`). The caller decides whether thread 1
    /// still runs.
    pub(crate) fn claim_exit_list(&self, vcpu_id: u32, rc: u32, tc: u32, initial_esp: u32) -> bool {
        let run = ExitRun { vcpu_id, host: std::thread::current().id(), tc, rc, initial_esp };
        let claimed = self.shared.threads.exit_list.lock_or_recover().start(run);
        if claimed {
            self.shared.threads.exit_list_active.store(true, Ordering::Release);
            debug!("  [VCPU {}] exit-list processing (tc={}, rc={})", vcpu_id, tc, rc);
            // Wake the threads blocked in the system so that they stop
            // (`begin_exit_list` kicks those running guest code; the caller
            // may hold the thread table here).
            self.shared.threads.ended.notify_all();
            self.shared.threads.wake_crit_waiters();
            self.shared.kbd_cond.notify_all();
            self.shared.mou_cond.notify_all();
        }
        claimed
    }

    /// The process ends (`DosExit(EXIT_PROCESS)`, thread 1 returning, an
    /// unhandled exception) on thread `vcpu_id`: run the exit list there if
    /// there is one and thread 1 still runs.
    pub(crate) fn begin_exit_list(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, rc: u32, tc: u32,
                                  initial_esp: u32, callback_stack: &mut Vec<CallbackFrame>) -> ExitListStart {
        let main_alive = self.shared.threads.table.lock_or_recover().is_running(super::threads::MAIN_TID);
        if !main_alive || !self.claim_exit_list(vcpu_id, rc, tc, initial_esp) {
            return ExitListStart::NotStarted;
        }
        // The other threads stop, also those spinning in guest code.
        self.shared.threads.table.lock_or_recover().kick_all_but(super::threads::tid_of_vcpu(vcpu_id));
        // Window procedures, dialog loops … the thread was in are gone.
        callback_stack.clear();
        if self.next_exit_routine(vcpu) { ExitListStart::Running } else { ExitListStart::Finished }
    }

    /// Call the next exit-list routine on `vcpu` (true), or — none left —
    /// end the process with the exit-list's exit code (false).
    pub(crate) fn next_exit_routine(&self, vcpu: &mut dyn VcpuBackend) -> bool {
        loop {
            let (next, run) = {
                let mut list = self.shared.threads.exit_list.lock_or_recover();
                (list.take_next(), list.run())
            };
            let Some(run) = run else { return false };
            let Some(pfn) = next else {
                let rc = self.shared.threads.exit_list.lock_or_recover().finish().unwrap_or(run.rc);
                debug!("  exit list done; the process ends (rc={})", rc);
                self.request_process_exit(rc);
                self.shared.threads.exit_list_active.store(false, Ordering::Release);
                return false;
            };
            if self.call_exit_routine(vcpu, pfn, run) {
                return true;
            }
            warn!("  exit-list routine {:#x} cannot be called (stack {:#x})", pfn, run.initial_esp);
        }
    }

    /// Set `vcpu` up to run routine `pfn(tc)` from the initial stack.
    fn call_exit_routine(&self, vcpu: &mut dyn VcpuBackend, pfn: u32, run: ExitRun) -> bool {
        let Some(sp) = run.initial_esp.checked_sub(ROUTINE_FRAME) else { return false };
        let (Ok(mut regs), Ok(sregs)) = (vcpu.get_regs(), vcpu.get_sregs()) else { return false };
        let framed = self.guest_write::<u32>(sp, EXIT_TRAP_ADDR).is_some()
            && self.guest_write::<u32>(sp + 4, run.tc).is_some();
        if !framed {
            return false;
        }
        // The handlers' registration records were on the stack just reset:
        // empty this thread's chain (its own TIB, which FS points at).
        self.guest_write::<u32>((sregs.fs.base as u32).wrapping_add(TIB_EXCHAIN_OFFSET), XCPT_CHAIN_END);
        debug!("  calling exit-list routine {:#x}(tc={})", pfn, run.tc);
        regs.rip = pfn as u64;
        regs.rsp = sp as u64;
        regs.rflags = ROUTINE_RFLAGS;
        vcpu.set_regs(&regs).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_on(vcpu_id: u32) -> ExitRun {
        ExitRun { vcpu_id, host: std::thread::current().id(), tc: TC_EXIT, rc: 3, initial_esp: 0x8_0000 }
    }

    fn order_code(action: u32, order: u32) -> u32 { action | order << ORDER_SHIFT }

    #[test]
    fn order_codes_parse_as_documented() {
        assert_eq!(parse_order_code(order_code(EXLST_ADD, 0x80)), Ok(ExitListAction::Add { order: 0x80 }));
        assert_eq!(parse_order_code(EXLST_REMOVE), Ok(ExitListAction::Remove));
        assert_eq!(parse_order_code(EXLST_EXIT), Ok(ExitListAction::Exit));
        // Order only with EXLST_ADD; high word reserved; unknown actions.
        assert_eq!(parse_order_code(order_code(EXLST_REMOVE, 1)), Err(ERROR_INVALID_DATA));
        assert_eq!(parse_order_code(order_code(EXLST_EXIT, 1)), Err(ERROR_INVALID_DATA));
        assert_eq!(parse_order_code(0x1_0000 | EXLST_ADD), Err(ERROR_INVALID_DATA));
        assert_eq!(parse_order_code(0), Err(ERROR_INVALID_FUNCTION));
        assert_eq!(parse_order_code(4), Err(ERROR_INVALID_FUNCTION));
    }

    #[test]
    fn routines_run_by_order_and_last_added_first() {
        let mut l = ExitList::default();
        assert_eq!(l.add(0x20, 0xA), NO_ERROR);
        assert_eq!(l.add(0x10, 0xB), NO_ERROR);
        assert_eq!(l.add(0x20, 0xC), NO_ERROR);
        assert_eq!(l.add(0xFF, 0xD), NO_ERROR);
        assert_eq!(l.add(0x00, 0xE), NO_ERROR);
        assert!(l.start(run_on(0)));
        let order: Vec<u32> = std::iter::from_fn(|| l.take_next()).collect();
        assert_eq!(order, vec![0xE, 0xB, 0xC, 0xA, 0xD]);
        assert_eq!(l.finish(), Some(3));
    }

    #[test]
    fn remove_add_again_and_errors() {
        let mut l = ExitList::default();
        assert_eq!(l.add(1, 0), ERROR_INVALID_DATA);
        assert_eq!(l.remove(0x10), ERROR_INVALID_DATA);
        assert_eq!(l.add(5, 0x10), NO_ERROR);
        assert_eq!(l.add(1, 0x20), NO_ERROR);
        // Adding again moves the routine instead of listing it twice.
        assert_eq!(l.add(0, 0x10), NO_ERROR);
        assert_eq!(l.len(), 2);
        assert_eq!(l.remove(0x20), NO_ERROR);
        assert_eq!(l.remove(0x20), ERROR_INVALID_DATA);
        assert!(l.start(run_on(0)));
        assert_eq!(l.take_next(), Some(0x10));
        assert_eq!(l.take_next(), None);
    }

    #[test]
    fn the_list_is_bounded() {
        let mut l = ExitList::default();
        for pfn in 1..=MAX_EXIT_ROUTINES as u32 {
            assert_eq!(l.add(0, pfn), NO_ERROR);
        }
        assert_eq!(l.add(0, 0xFFFF), ERROR_NOT_ENOUGH_MEMORY);
        // Re-adding one already listed still works.
        assert_eq!(l.add(1, 1), NO_ERROR);
    }

    #[test]
    fn routines_added_while_the_list_runs_run_too() {
        let mut l = ExitList::default();
        l.add(0x10, 0xA);
        assert!(l.start(run_on(2)));
        assert_eq!(l.take_next(), Some(0xA));
        l.add(0x05, 0xB);
        l.add(0x20, 0xC);
        assert_eq!(l.remove(0xC), NO_ERROR);
        assert_eq!(l.take_next(), Some(0xB));
        assert_eq!(l.take_next(), None);
    }

    #[test]
    fn processing_starts_once_and_never_on_an_empty_list() {
        let mut l = ExitList::default();
        assert!(!l.start(run_on(0)), "empty list");
        l.add(0, 0xA);
        assert!(l.start(run_on(0)));
        assert!(!l.start(run_on(1)), "already running");
        assert_eq!(l.run().map(|r| r.vcpu_id), Some(0));
        l.finish();
        l.add(0, 0xB);
        assert!(!l.start(run_on(0)), "already ran");
        assert_eq!(l.run(), None);
    }
}
