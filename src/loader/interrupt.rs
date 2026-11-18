// SPDX-License-Identifier: GPL-3.0-only
//
//! Interrupting a thread blocked in an API call.
//!
//! A signal exception for thread 1 (`signals.rs`), a request that the
//! process end (`signals::request_termination`) and `DosKillThread` must
//! reach a thread that waits inside Warpine — in `DosSleep`, a semaphore
//! wait, a read of the keyboard or a pipe, `WinGetMsg` … —, not only one
//! that runs guest code (that one is kicked, ADR-0023).
//!
//! The vCPU loop marks the host thread as being in an API call
//! (`begin_call` / `end_call`, thread-local state). A blocking call looks
//! at `Loader::interrupted` where it already looks at the process end
//! (every wait wakes at least every 100 ms), and then either
//! - returns `ERROR_INTERRUPT`, for the calls whose reference lists it
//!   (`DosSleep`, `DosWaitEventSem`, `DosRequestMutexSem`,
//!   `DosWaitMuxWaitSem`, `DosWaitThread`, `DosWaitChild`): the signal is
//!   delivered as the call returns, and a handler that continues sees the
//!   call fail with `ERROR_INTERRUPT`; or
//! - gives up through `Loader::interrupt_restart`: the call's result is
//!   dropped and the guest is left at the API thunk with its stack as at
//!   the call, so the signal is delivered "at the call" and the call is made
//!   again when a handler continues. Such a call must not have changed
//!   anything before it blocked (reads that took no data, `WinGetMsg` that
//!   removed no message, a pipe connect or wait).
//!
//! Signals only interrupt thread 1 while it can take the handler call
//! (32-bit code with room on its stack, decided at `begin_call`), and not
//! while it runs the exit list — the same rules `signal_action` applies,
//! so an interrupted call always finds the signal delivered and is not
//! interrupted again at once.

use std::cell::Cell;
use std::sync::atomic::Ordering;

use super::mutex_ext::MutexExt;
use super::threads::MAIN_TID;

/// The API call the calling host thread is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CallState {
    /// OS/2 thread id (0: not in a call the vCPU loop knows).
    tid: u32,
    /// Thread 1 can take a signal-handler call (32-bit, stack room).
    signals: bool,
    /// The call gave up to be made again.
    restart: bool,
}

thread_local! {
    static CALL: Cell<CallState> = const { Cell::new(CallState { tid: 0, signals: false, restart: false }) };
}

/// Thread `tid` enters an API call; `signals`: a signal may interrupt it.
pub(crate) fn begin_call(tid: u32, signals: bool) {
    CALL.with(|c| c.set(CallState { tid, signals, restart: false }));
}

/// The call is over: whether it gave up to be made again.
pub(crate) fn end_call() -> bool {
    CALL.with(|c| c.replace(CallState::default()).restart)
}

impl super::Loader {
    /// Whether the call the calling thread is blocked in should stop: its
    /// thread was killed, or — thread 1 — a signal or a termination waits.
    pub(crate) fn interrupted(&self) -> bool {
        let call = CALL.with(Cell::get);
        if call.tid == 0 {
            return false;
        }
        let threads = &self.shared.threads;
        if threads.table.lock_or_recover().is_killed(call.tid) {
            return true;
        }
        if call.tid != MAIN_TID
            || !threads.signal_pending.load(Ordering::Acquire)
            || threads.exit_list_active.load(Ordering::Acquire) {
            return false;
        }
        let signals = threads.signals.lock_or_recover();
        signals.termination_pending() || (call.signals && signals.deliverable())
    }

    /// `interrupted`, for a call that is then made again: its result is
    /// dropped (see the module documentation).
    pub(crate) fn interrupt_restart(&self) -> bool {
        let stop = self.interrupted();
        if stop {
            CALL.with(|c| c.set(CallState { restart: true, ..c.get() }));
        }
        stop
    }

    /// A blocking wait that is made again when interrupted gives up: the
    /// process ends, or `interrupt_restart`.
    pub(crate) fn stop_or_restart(&self) -> bool {
        self.shutting_down() || self.interrupt_restart()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;
    use super::super::constants::*;
    use super::super::threads::ThreadMemory;

    #[test]
    fn nothing_interrupts_outside_a_call() {
        let loader = Loader::new_mock();
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        loader.post_signal(XCPT_SIGNAL_INTR);
        assert!(!loader.interrupted(), "not in a call");
        begin_call(MAIN_TID, true);
        assert!(loader.interrupted());
        assert!(!end_call());
        assert!(!loader.interrupted());
    }

    #[test]
    fn a_signal_interrupts_thread_1_only_when_it_can_take_it() {
        let loader = Loader::new_mock();
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        begin_call(MAIN_TID, true);
        assert!(!loader.interrupted(), "nothing pending");
        loader.post_signal(XCPT_SIGNAL_BREAK);
        assert!(loader.interrupted());
        // 16-bit code or no stack room: the signal waits for the call.
        begin_call(MAIN_TID, false);
        assert!(!loader.interrupted());
        // Other threads never take signals.
        begin_call(2, true);
        assert!(!loader.interrupted());
        // Nor does the exit-list thread.
        begin_call(MAIN_TID, true);
        loader.shared.threads.exit_list_active.store(true, Ordering::Release);
        assert!(!loader.interrupted());
        loader.shared.threads.exit_list_active.store(false, Ordering::Release);
        // Held (delivered, not acknowledged): nothing interrupts.
        loader.shared.threads.signals.lock_or_recover().take();
        assert!(!loader.interrupted());
        end_call();
    }

    #[test]
    fn a_termination_interrupts_thread_1_whatever_its_code() {
        let loader = Loader::new_mock();
        begin_call(MAIN_TID, false);
        assert!(!loader.interrupted());
        super::super::signals::request_termination(&loader.shared, 3, TC_EXIT);
        assert!(loader.interrupted());
        assert!(loader.interrupt_restart());
        assert!(end_call(), "the call is made again");
        assert!(!end_call(), "the flag is reset");
    }

    #[test]
    fn a_killed_thread_is_interrupted() {
        let loader = Loader::new_mock();
        loader.shared.threads.table.lock_or_recover().start(2, ThreadMemory { stack: 0x1000, tib: 0x2000 });
        begin_call(2, false);
        assert!(!loader.stop_or_restart());
        assert!(!end_call());
        begin_call(2, false);
        assert_eq!(loader.dos_kill_thread(MAIN_VCPU_FOR_TEST, 2), NO_ERROR);
        assert!(loader.stop_or_restart());
        assert!(end_call());
    }

    const MAIN_VCPU_FOR_TEST: u32 = super::super::threads::MAIN_VCPU_ID;
}
