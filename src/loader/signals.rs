// SPDX-License-Identifier: GPL-3.0-only
//
//! Signal exceptions: `DosSendSignalException` (DOSCALLS 379),
//! `DosSetSignalExceptionFocus` (378), `DosAcknowledgeSignalException` (418)
//! and the delivery of `XCPT_SIGNAL` to thread 1.
//!
//! Behaviour from IBM's *Control Program Programming Reference* and
//! *Programming Guide* (OS/2 Warp 4 Toolkit, "Signal Exceptions",
//! "Handling Signal Exceptions", "XCPT_SIGNAL"):
//! - `XCPT_SIGNAL` (`ExceptionInfo[0]` = `XCPT_SIGNAL_INTR` 1 Ctrl+C,
//!   `XCPT_SIGNAL_KILLPROC` 3, `XCPT_SIGNAL_BREAK` 4) goes only to thread 1
//!   of the receiving process, through its exception handlers; its default
//!   action — no handler returns `XCPT_CONTINUE_EXECUTION` — ends the process;
//! - INTR and BREAK are delivered only while the process's focus counter
//!   (`DosSetSignalExceptionFocus`: +1 for `SIG_SETFOCUS`, −1 for
//!   `SIG_UNSETFOCUS`, `ERROR_ALREADY_RESET` at 0, `ERROR_NESTING_TOO_DEEP`
//!   above 65535) is above 0; otherwise the process is ended;
//! - after a signal of a type was delivered, further ones of that type are
//!   held until a handler returns `XCPT_CONTINUE_EXECUTION` or the process
//!   calls `DosAcknowledgeSignalException`; only one is held, they are not
//!   queued;
//! - `DosSendSignalException(pid, ulException)` sends INTR or BREAK to
//!   another process (`ERROR_INVALID_SIGNAL_NUMBER` for other numbers,
//!   `ERROR_INVALID_PROCID` for an unknown process).
//!
//! In Warpine a child process is another host process running Warpine; a
//! signal reaches it as a host real-time signal (`host_signal.rs`). The
//! process itself (its own PID) can also be signalled.
//!
//! Warpine's choices where the documents say nothing:
//! - a signal reaches thread 1 when it next leaves guest code: at once if
//!   it runs guest code (it is kicked), after the call if it is blocked in
//!   one (the call is not interrupted);
//! - it is held while thread 1 runs 16-bit code, while its stack has no
//!   room for the handler frame, and while the exit list runs;
//! - a process ended by a signal ends with exit code 1, its exit-list
//!   routines get `TC_KILLPROCESS`;
//! - with thread 1 ended (other threads still running) a signal whose
//!   default action applies ends the process at once.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::threads::MAIN_TID;
use super::vm_backend::VcpuBackend;
use super::{ApiResult, FaultContext};
use log::debug;
use std::sync::atomic::Ordering;

/// Highest signal-focus count (`DosSetSignalExceptionFocus`).
pub const MAX_SIGNAL_FOCUS: u32 = 65535;
/// Exit code of a process ended by a signal exception.
pub const SIGNAL_EXIT_CODE: u32 = 1;
/// Bytes the handler call frame needs below thread 1's stack pointer.
const HANDLER_FRAME_BYTES: u32 = 20;

fn signal_bit(signal: u32) -> Option<u32> {
    matches!(signal, XCPT_SIGNAL_INTR | XCPT_SIGNAL_KILLPROC | XCPT_SIGNAL_BREAK).then(|| 1 << signal)
}

/// The process's signal-exception state (pure bookkeeping).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SignalState {
    focus: u32,
    /// Signals waiting for delivery (bit = signal number).
    pending: u32,
    /// Signals delivered and not yet acknowledged.
    held: u32,
}

impl SignalState {
    pub fn focus(&self) -> u32 { self.focus }

    /// `DosSetSignalExceptionFocus`: the new count, or the error.
    pub fn set_focus(&mut self, on: bool) -> Result<u32, u32> {
        if on {
            if self.focus >= MAX_SIGNAL_FOCUS { return Err(ERROR_NESTING_TOO_DEEP); }
            self.focus += 1;
        } else {
            if self.focus == 0 { return Err(ERROR_ALREADY_RESET); }
            self.focus -= 1;
        }
        Ok(self.focus)
    }

    /// A signal arrived. One per type waits; a second one is dropped.
    pub fn post(&mut self, signal: u32) -> bool {
        let Some(bit) = signal_bit(signal) else { return false };
        self.pending |= bit;
        true
    }

    /// Whether a signal can be delivered now.
    pub fn deliverable(&self) -> bool {
        self.pending & !self.held != 0
    }

    /// The next signal to deliver (lowest number first); it is held until
    /// acknowledged.
    pub fn take(&mut self) -> Option<u32> {
        let ready = self.pending & !self.held;
        if ready == 0 { return None; }
        let signal = ready.trailing_zeros();
        self.pending &= !(1 << signal);
        self.held |= 1 << signal;
        Some(signal)
    }

    /// `DosAcknowledgeSignalException`, or a handler continuing execution.
    pub fn acknowledge(&mut self, signal: u32) -> u32 {
        let Some(bit) = signal_bit(signal) else { return ERROR_INVALID_SIGNAL_NUMBER };
        self.held &= !bit;
        NO_ERROR
    }

    /// Whether `signal` ends the process without reaching its handlers.
    pub fn ends_process(&self, signal: u32) -> bool {
        signal != XCPT_SIGNAL_KILLPROC && self.focus == 0
    }
}

/// What thread 1 does about a pending signal at the top of its vCPU loop.
pub(crate) enum SignalAction {
    /// Nothing to deliver now.
    None,
    /// Call the handler chain (an `ApiResult::ExceptionDispatch`).
    Dispatch(ApiResult),
    /// The default action: end the process.
    EndProcess,
}

impl super::Loader {
    /// DosSetSignalExceptionFocus (ordinal 378).
    pub fn dos_set_signal_exception_focus(&self, flag: u32, pul_times: u32) -> u32 {
        let mut signals = self.shared.threads.signals.lock_or_recover();
        let mut next = *signals;
        let count = match next.set_focus(flag != SIG_UNSETFOCUS) {
            Ok(c) => c,
            Err(e) => return e,
        };
        if pul_times != 0 && self.guest_write::<u32>(pul_times, count).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        *signals = next;
        debug!("  DosSetSignalExceptionFocus({}) -> count {}", flag, count);
        NO_ERROR
    }

    /// DosAcknowledgeSignalException (ordinal 418).
    pub fn dos_acknowledge_signal_exception(&self, signal: u32) -> u32 {
        let rc = self.shared.threads.signals.lock_or_recover().acknowledge(signal);
        if rc == NO_ERROR {
            self.signal_thread_1();
        }
        rc
    }

    /// DosSendSignalException (ordinal 379).
    pub fn dos_send_signal_exception(&self, pid: u32, signal: u32) -> u32 {
        debug!("  DosSendSignalException(pid={}, signal={})", pid, signal);
        if signal != XCPT_SIGNAL_INTR && signal != XCPT_SIGNAL_BREAK {
            return ERROR_INVALID_SIGNAL_NUMBER;
        }
        if pid == PROCESS_ID {
            self.post_signal(signal);
            return NO_ERROR;
        }
        let host_pid = {
            let mut procs = self.shared.process_mgr.lock_or_recover();
            // A child that has ended cannot be signalled (its status stays
            // for DosWaitChild).
            procs.children.get_mut(&pid)
                .and_then(|child| matches!(child.try_wait(), Ok(None)).then(|| child.id()))
        };
        match host_pid {
            Some(host_pid) if super::host_signal::send_to_process(host_pid, signal) => NO_ERROR,
            _ => ERROR_INVALID_PROCID,
        }
    }

    /// A signal exception for this process arrived (from itself or from
    /// another Warpine process): thread 1 gets it when it next leaves the
    /// guest.
    pub(crate) fn post_signal(&self, signal: u32) {
        let ends = {
            let mut signals = self.shared.threads.signals.lock_or_recover();
            if !signals.post(signal) { return; }
            signals.ends_process(signal)
        };
        let main_runs = self.shared.threads.table.lock_or_recover().is_running(MAIN_TID);
        if !main_runs {
            // No thread 1 to run handlers or the exit list.
            if ends {
                self.request_process_exit(SIGNAL_EXIT_CODE);
            }
            return;
        }
        self.signal_thread_1();
    }

    /// Tell thread 1 to look for a deliverable signal.
    fn signal_thread_1(&self) {
        let threads = &self.shared.threads;
        if threads.signals.lock_or_recover().deliverable() {
            threads.signal_pending.store(true, Ordering::Release);
            threads.table.lock_or_recover().kick(MAIN_TID);
        }
    }

    /// Start taking signal exceptions from other Warpine processes.
    pub(crate) fn listen_for_signals(&self) {
        let shared = std::sync::Arc::clone(&self.shared);
        let vm = std::sync::Arc::clone(&self.vm);
        super::host_signal::listen(move |signal| {
            let loader = super::Loader { vm: std::sync::Arc::clone(&vm), shared: std::sync::Arc::clone(&shared) };
            loader.post_signal(signal);
        });
    }

    /// Thread 1 at the top of its vCPU loop: deliver a pending signal.
    pub(crate) fn signal_action(&self, vcpu: &dyn VcpuBackend, vcpu_id: u32) -> SignalAction {
        let threads = &self.shared.threads;
        if super::threads::tid_of_vcpu(vcpu_id) != MAIN_TID
            || !threads.signal_pending.load(Ordering::Acquire)
            || self.runs_exit_list(vcpu_id) {
            return SignalAction::None;
        }
        let (Ok(regs), Ok(sregs)) = (vcpu.get_regs(), vcpu.get_sregs()) else { return SignalAction::None };
        // Only 32-bit code with room on its stack can take a handler call.
        let esp = regs.rsp as u32;
        let room = esp.checked_sub(HANDLER_FRAME_BYTES)
            .is_some_and(|low| self.guest_read::<u32>(low).is_some() && self.guest_read::<u32>(esp - 4).is_some());
        if sregs.cs.db == 0 || sregs.ss.db == 0 || !room {
            return SignalAction::None;
        }
        let (signal, ends) = {
            let mut signals = threads.signals.lock_or_recover();
            let Some(signal) = signals.take() else {
                threads.signal_pending.store(false, Ordering::Release);
                return SignalAction::None;
            };
            if !signals.deliverable() {
                threads.signal_pending.store(false, Ordering::Release);
            }
            (signal, signals.ends_process(signal))
        };
        debug!("  [VCPU {}] XCPT_SIGNAL {} for thread 1", vcpu_id, signal);
        if ends {
            return SignalAction::EndProcess;
        }
        let fault = FaultContext {
            eax: regs.rax as u32, ebx: regs.rbx as u32, ecx: regs.rcx as u32, edx: regs.rdx as u32,
            esi: regs.rsi as u32, edi: regs.rdi as u32, ebp: regs.rbp as u32,
            esp, eip: regs.rip as u32, eflags: regs.rflags as u32,
            cs: sregs.cs.selector, ds: sregs.ds.selector, es: sregs.es.selector,
            fs: sregs.fs.selector, gs: sregs.gs.selector, ss: sregs.ss.selector,
        };
        match self.chain_dispatch(vcpu_id, fault, XCPT_SIGNAL, 0, &[signal]) {
            Some(dispatch) => SignalAction::Dispatch(dispatch),
            None => SignalAction::EndProcess,
        }
    }

    /// A handler returned from an exception: an `XCPT_SIGNAL` it continued
    /// counts as acknowledged.
    pub(crate) fn signal_continued(&self, exc_report: u32) {
        if self.guest_read::<u32>(exc_report.wrapping_add(ERR_NUM)) == Some(XCPT_SIGNAL)
            && let Some(signal) = self.guest_read::<u32>(exc_report.wrapping_add(ERR_PARAMS)) {
            self.dos_acknowledge_signal_exception(signal);
        }
    }

    /// The termination code an unhandled exception gives exit-list routines.
    pub(crate) fn termination_code(&self, exc_report: u32) -> u32 {
        if self.guest_read::<u32>(exc_report.wrapping_add(ERR_NUM)) == Some(XCPT_SIGNAL) {
            TC_KILLPROCESS
        } else {
            TC_EXCEPTION
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;
    use super::super::threads::{MAIN_VCPU_ID, ThreadMemory};
    use super::super::vm_backend::mock::MockVcpu;

    #[test]
    fn focus_counts_and_its_limits() {
        let mut s = SignalState::default();
        assert_eq!(s.set_focus(false), Err(ERROR_ALREADY_RESET));
        assert_eq!(s.set_focus(true), Ok(1));
        assert_eq!(s.set_focus(true), Ok(2));
        assert_eq!(s.set_focus(false), Ok(1));
        for _ in 1..MAX_SIGNAL_FOCUS { s.set_focus(true).unwrap(); }
        assert_eq!(s.focus(), MAX_SIGNAL_FOCUS);
        assert_eq!(s.set_focus(true), Err(ERROR_NESTING_TOO_DEEP));
        assert_eq!(s.focus(), MAX_SIGNAL_FOCUS);
    }

    #[test]
    fn one_signal_per_type_is_held_until_acknowledged() {
        let mut s = SignalState::default();
        assert!(!s.post(2), "not a signal number");
        assert!(s.post(XCPT_SIGNAL_BREAK));
        assert!(s.post(XCPT_SIGNAL_INTR));
        assert_eq!(s.take(), Some(XCPT_SIGNAL_INTR));
        assert_eq!(s.take(), Some(XCPT_SIGNAL_BREAK));
        assert_eq!(s.take(), None);
        // Two more INTRs while the first is unacknowledged: one is kept.
        s.post(XCPT_SIGNAL_INTR);
        s.post(XCPT_SIGNAL_INTR);
        assert!(!s.deliverable());
        assert_eq!(s.acknowledge(XCPT_SIGNAL_INTR), NO_ERROR);
        assert_eq!(s.take(), Some(XCPT_SIGNAL_INTR));
        s.acknowledge(XCPT_SIGNAL_INTR);
        assert_eq!(s.take(), None);
        assert_eq!(s.acknowledge(0), ERROR_INVALID_SIGNAL_NUMBER);
        assert_eq!(s.acknowledge(XCPT_SIGNAL_KILLPROC), NO_ERROR);
    }

    #[test]
    fn without_focus_intr_and_break_end_the_process() {
        let mut s = SignalState::default();
        assert!(s.ends_process(XCPT_SIGNAL_INTR));
        assert!(s.ends_process(XCPT_SIGNAL_BREAK));
        assert!(!s.ends_process(XCPT_SIGNAL_KILLPROC));
        s.set_focus(true).unwrap();
        assert!(!s.ends_process(XCPT_SIGNAL_INTR));
    }

    #[test]
    fn set_focus_writes_the_count() {
        let loader = Loader::new_mock();
        let p = 0x3000;
        assert_eq!(loader.dos_set_signal_exception_focus(SIG_UNSETFOCUS, p), ERROR_ALREADY_RESET);
        assert_eq!(loader.dos_set_signal_exception_focus(SIG_SETFOCUS, p), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(p), Some(1));
        assert_eq!(loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0), NO_ERROR);
        assert_eq!(loader.dos_set_signal_exception_focus(SIG_UNSETFOCUS, p), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(p), Some(1));
        // A bad pointer changes nothing.
        assert_eq!(loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0xFFFF_FFF0), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.shared.threads.signals.lock_or_recover().focus(), 1);
    }

    #[test]
    fn send_checks_signal_and_process() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_send_signal_exception(PROCESS_ID, XCPT_SIGNAL_KILLPROC), ERROR_INVALID_SIGNAL_NUMBER);
        assert_eq!(loader.dos_send_signal_exception(PROCESS_ID, 0), ERROR_INVALID_SIGNAL_NUMBER);
        assert_eq!(loader.dos_send_signal_exception(12345, XCPT_SIGNAL_INTR), ERROR_INVALID_PROCID);
        // An ended child cannot be signalled.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        let pid = loader.shared.process_mgr.lock_or_recover().add_child(child);
        assert_eq!(loader.dos_send_signal_exception(pid, XCPT_SIGNAL_BREAK), ERROR_INVALID_PROCID);
    }

    /// A vCPU of thread 1 in 32-bit code with a stack at `esp`.
    fn main_vcpu(esp: u64) -> MockVcpu {
        let mut v = MockVcpu::new();
        v.regs.rsp = esp;
        v.regs.rip = 0x10000;
        v.sregs.cs.db = 1;
        v.sregs.ss.db = 1;
        v
    }

    #[test]
    fn a_signal_to_itself_reaches_thread_1_and_kicks_it() {
        let loader = Loader::new_mock();
        let vcpu = main_vcpu(0x8000);
        loader.shared.threads.table.lock_or_recover().set_kicker(MAIN_TID, vcpu.kick.clone());
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        assert_eq!(loader.dos_send_signal_exception(PROCESS_ID, XCPT_SIGNAL_INTR), NO_ERROR);
        assert_eq!(vcpu.kick.kicks.load(Ordering::SeqCst), 1, "thread 1 was kicked");
        // No handler on the chain: the default action ends the process.
        loader.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, XCPT_CHAIN_END);
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::EndProcess));
        // Other threads never take signals.
        loader.post_signal(XCPT_SIGNAL_BREAK);
        assert!(matches!(loader.signal_action(&vcpu, 2), SignalAction::None));
    }

    #[test]
    fn a_handler_gets_xcpt_signal_with_the_interrupted_context() {
        let loader = Loader::new_mock();
        let vcpu = main_vcpu(0x8000);
        let (rec, handler) = (0x6000u32, 0x0001_2340u32);
        loader.guest_write::<u32>(rec, XCPT_CHAIN_END);
        loader.guest_write::<u32>(rec + 4, handler);
        loader.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, rec);
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        loader.post_signal(XCPT_SIGNAL_BREAK);
        let SignalAction::Dispatch(ApiResult::ExceptionDispatch { handler_addr, exc_report, ctx_record, saved, .. })
            = loader.signal_action(&vcpu, MAIN_VCPU_ID) else { panic!("no dispatch") };
        assert_eq!(handler_addr, handler);
        assert_eq!(loader.guest_read::<u32>(exc_report + ERR_NUM), Some(XCPT_SIGNAL));
        assert_eq!(loader.guest_read::<u32>(exc_report + ERR_PARAMS), Some(XCPT_SIGNAL_BREAK));
        assert_eq!((saved.eip, saved.esp), (0x10000, 0x8000));
        assert_eq!(loader.guest_read::<u32>(ctx_record + CTX_EIP), Some(0x10000));
        assert_eq!(loader.termination_code(exc_report), TC_KILLPROCESS);
        // Held: a second BREAK waits for the handler to continue.
        loader.post_signal(XCPT_SIGNAL_BREAK);
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::None));
        loader.signal_continued(exc_report);
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::Dispatch(_)));
    }

    #[test]
    fn a_signal_waits_for_16_bit_code_and_a_usable_stack() {
        let loader = Loader::new_mock();
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        loader.post_signal(XCPT_SIGNAL_INTR);
        let mut vcpu = main_vcpu(0x8000);
        vcpu.sregs.cs.db = 0;
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::None));
        let vcpu = main_vcpu(8);
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::None));
        let vcpu = main_vcpu(0xFFFF_FFF0);
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::None));
        assert!(loader.shared.threads.signals.lock_or_recover().deliverable(), "still pending");
    }

    #[test]
    fn with_thread_1_gone_a_signal_ends_the_process() {
        let loader = Loader::new_mock();
        {
            let mut t = loader.shared.threads.table.lock_or_recover();
            t.start(2, ThreadMemory { stack: 0x1000, tib: 0x2000 });
            t.end(MAIN_TID);
        }
        loader.post_signal(XCPT_SIGNAL_INTR);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), SIGNAL_EXIT_CODE as i32);
    }
}
