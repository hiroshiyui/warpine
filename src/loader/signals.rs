// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Signal exceptions: `DosSendSignalException` (DOSCALLS 379),
//! `DosSetSignalExceptionFocus` (378), `DosAcknowledgeSignalException` (418)
//! and the delivery of `XCPT_SIGNAL` to thread 1; `DosKillProcess` (235);
//! the end of a process asked for from outside (a closed session, the end
//! of piped keyboard input, a fault no handler can see).
//!
//! Behaviour from IBM's *Control Program Programming Reference* and
//! *Programming Guide* (OS/2 Warp 4 Toolkit, "Signal Exceptions",
//! "Handling Signal Exceptions", "XCPT_SIGNAL", "DosKillProcess"):
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
//!   `ERROR_INVALID_PROCID` for an unknown process);
//! - `DosKillProcess(ulAction, pid)` flags the process (`DKP_PROCESS`) or
//!   it and its descendants (`DKP_PROCESSTREE`) for termination: it gets
//!   `XCPT_SIGNAL_KILLPROC`, and its exit-list routines run with
//!   `TC_KILLPROCESS`; the call returns without waiting; errors
//!   `ERROR_INVALID_DATA` (bad action), `ERROR_ZOMBIE_PROCESS` (ended, not
//!   yet waited for), `ERROR_INVALID_PROCID`; the parent's `DosWaitChild`
//!   reports `TC_KILLPROCESS` in `codeTerminate`.
//!
//! In Warpine a child process is another host process running Warpine; a
//! signal reaches it as a host real-time signal (`host_signal.rs`). The
//! process itself (its own PID) can also be signalled.
//!
//! Warpine's choices where the documents say nothing:
//! - a signal reaches thread 1 when it next leaves guest code: at once if
//!   it runs guest code (it is kicked); a blocking call it waits in is
//!   interrupted (`interrupt.rs`) — the calls documented with
//!   `ERROR_INTERRUPT` return it after the handler continued, the others
//!   are started again;
//! - it is held while thread 1 runs 16-bit code, while its stack has no
//!   room for the handler frame, and while the exit list runs;
//! - a process ended by a signal ends with exit code 1, its exit-list
//!   routines get `TC_KILLPROCESS`;
//! - with thread 1 ended (other threads still running) a signal whose
//!   default action applies ends the process at once;
//! - a process asked to end from outside (`XCPT_SIGNAL_KILLPROC` from
//!   `DosKillProcess` or the host's `SIGTERM`, its session closed) gets
//!   `KILL_GRACE` to do so; then it ends without waiting for its handlers
//!   or exit list. A parent's `DosKillProcess` also ends the child's host
//!   process (`SIGKILL` through a pidfd) if it is still there
//!   `KILL_GRACE_PARENT_MARGIN` later. A second request to end a session
//!   ends it at once;
//! - `DosKillProcess(DKP_PROCESSTREE)` on a child is passed on by that
//!   child to its own children before it handles its own signal; a
//!   grandchild cannot be named (`ERROR_INVALID_PROCID`), so
//!   `ERROR_NOT_DESCENDANT` is never returned.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::constants::*;
use super::host_signal::{HostRequest, ProcessHandle};
use super::mutex_ext::MutexExt;
use super::threads::MAIN_TID;
use super::vm_backend::VcpuBackend;
use super::{ApiResult, FaultContext, SharedState};
use log::{debug, warn};

/// Highest signal-focus count (`DosSetSignalExceptionFocus`).
pub const MAX_SIGNAL_FOCUS: u32 = 65535;
/// Exit code of a process ended by a signal exception.
pub const SIGNAL_EXIT_CODE: u32 = 1;
/// How long a process asked to end from outside has to do so (its
/// handlers and exit list run meanwhile).
pub const KILL_GRACE: Duration = Duration::from_secs(5);
/// How much longer a parent waits before it ends a killed child's host
/// process itself.
pub const KILL_GRACE_PARENT_MARGIN: Duration = Duration::from_secs(2);
/// How often the end-of-grace watcher looks at the process.
const GRACE_POLL: Duration = Duration::from_millis(50);
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
    /// The process is to end without handlers (exit code, termination
    /// code for the exit list): thread 1 runs the exit list.
    terminate: Option<(u32, u32)>,
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

    /// Ask thread 1 to end the process (exit code `rc`, exit-list routines
    /// get `tc`). The first request wins.
    pub fn request_termination(&mut self, rc: u32, tc: u32) -> bool {
        if self.terminate.is_some() { return false; }
        self.terminate = Some((rc, tc));
        true
    }

    /// The termination asked for, taken.
    pub fn take_termination(&mut self) -> Option<(u32, u32)> {
        self.terminate.take()
    }

    /// Whether a termination waits for thread 1.
    pub fn termination_pending(&self) -> bool {
        self.terminate.is_some()
    }

    /// Whether thread 1 has something to do: a signal or a termination.
    pub fn wants_thread_1(&self) -> bool {
        self.deliverable() || self.terminate.is_some()
    }
}

/// Children `DosKillProcess` was used on, and whether the process is
/// being ended from outside.
#[derive(Debug, Default)]
pub struct KillTable {
    /// Killed children not yet waited for (their `codeTerminate` is
    /// `TC_KILLPROCESS`).
    killed: HashSet<u32>,
    /// Killed children a grace-period watcher looks after.
    watched: HashSet<u32>,
    /// A request to end this process (session closed, `SIGTERM`) came.
    ending: bool,
}

impl KillTable {
    /// Child `pid` was killed; true when no watcher looks after it yet.
    pub fn mark_killed(&mut self, pid: u32) -> bool {
        self.killed.insert(pid);
        self.watched.insert(pid)
    }

    /// The grace-period watcher of `pid` is done.
    pub fn unwatch(&mut self, pid: u32) {
        self.watched.remove(&pid);
    }

    /// Child `pid` was waited for: whether it had been killed.
    pub fn reaped(&mut self, pid: u32) -> bool {
        self.killed.remove(&pid)
    }

    /// A request to end this process; true for the first one.
    pub fn begin_ending(&mut self) -> bool {
        !std::mem::replace(&mut self.ending, true)
    }
}

/// The character Ctrl+C gives (ASCII ETX).
pub const CTRL_C: u8 = 0x03;

/// The signal a byte typed at the keyboard raises: Ctrl+C is
/// `XCPT_SIGNAL_INTR` (the KBD subsystem's ASCII mode, the only one
/// Warpine has: `KbdSetStatus` binary mode is not implemented). A terminal
/// has no Ctrl+Break key.
pub fn keyboard_byte_signal(byte: u8) -> Option<u32> {
    (byte == CTRL_C).then_some(XCPT_SIGNAL_INTR)
}

/// The signal a key pressed in a VIO window or the text window raises:
/// Ctrl+C is `XCPT_SIGNAL_INTR`, Ctrl+Break (`ctrl` with Pause, or a
/// Break key) `XCPT_SIGNAL_BREAK`; `None` for any other key.
pub fn key_signal(ctrl: bool, is_c: bool, is_pause: bool, is_break: bool) -> Option<u32> {
    if is_break || (ctrl && is_pause) {
        Some(XCPT_SIGNAL_BREAK)
    } else if ctrl && is_c {
        Some(XCPT_SIGNAL_INTR)
    } else {
        None
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
    /// End the process with exit code `rc`; the exit list runs with `tc`
    /// when `exit_list` (thread 1 runs 32-bit code).
    Terminate { rc: u32, tc: u32, exit_list: bool },
}

/// End the process with `rc` now, without an exit list (unless it is
/// already ending: the first result code wins). Wakes the threads that
/// wait for it, stops the DOS timers and kicks threads out of guest code.
pub(crate) fn request_exit(shared: &SharedState, rc: u32) {
    if !shared.exit_requested.load(Ordering::Relaxed) {
        shared.exit_code.store(rc as i32, Ordering::Relaxed);
        shared.exit_requested.store(true, Ordering::Relaxed);
    }
    shared.threads.ended.notify_all();
    shared.threads.wake_crit_waiters();
    shared.dos_timers.lock_or_recover().stop_all();
    shared.kbd_cond.notify_all();
    shared.mou_cond.notify_all();
    // Threads blocked on semaphores see the process end at once.
    shared.sem_mgr.lock_or_recover().wake_all();
    // Threads spinning in guest code stop too.
    shared.threads.table.lock_or_recover().kick_all_but(0);
}

/// Tell thread 1 to look at its signals (and wake the waits it may be in).
fn wake_thread_1(shared: &SharedState) {
    let threads = &shared.threads;
    if threads.signals.lock_or_recover().wants_thread_1() {
        threads.signal_pending.store(true, Ordering::Release);
        threads.table.lock_or_recover().kick(MAIN_TID);
        // Blocking calls poll their interruption at least every 100 ms;
        // these waits are woken at once, the semaphore waits through
        // `SemaphoreManager::wake_all` (event and mutex condition
        // variables, the muxwait generation counter).
        shared.sem_mgr.lock_or_recover().wake_all();
        shared.kbd_cond.notify_all();
        shared.mou_cond.notify_all();
        threads.ended.notify_all();
        threads.resumed.notify_all();
    }
}

/// A signal exception for this process arrived (from itself, from another
/// Warpine process, from the keyboard): thread 1 gets it when it next
/// leaves the guest or the call it is blocked in.
pub(crate) fn post_signal(shared: &SharedState, signal: u32) {
    let ends = {
        let mut signals = shared.threads.signals.lock_or_recover();
        if !signals.post(signal) { return; }
        signals.ends_process(signal) || signal == XCPT_SIGNAL_KILLPROC
    };
    let main_runs = shared.threads.table.lock_or_recover().is_running(MAIN_TID);
    if !main_runs {
        // No thread 1 to run handlers or the exit list.
        if ends {
            request_exit(shared, SIGNAL_EXIT_CODE);
        }
        return;
    }
    wake_thread_1(shared);
}

/// End the process the way `DosExit(EXIT_PROCESS, rc)` would, from outside
/// thread 1: thread 1 runs the exit list (routines get `tc`) and ends the
/// process; without thread 1 the process ends at once.
pub(crate) fn request_termination(shared: &SharedState, rc: u32, tc: u32) {
    if shared.exit_requested.load(Ordering::Relaxed)
        || shared.threads.exit_list_active.load(Ordering::Acquire) {
        return; // already ending
    }
    let main_runs = shared.threads.table.lock_or_recover().is_running(MAIN_TID);
    if !main_runs {
        request_exit(shared, rc);
        return;
    }
    shared.threads.signals.lock_or_recover().request_termination(rc, tc);
    wake_thread_1(shared);
}

/// End the process `grace` from now if it has not ended by then.
pub(crate) fn exit_after(shared: &Arc<SharedState>, grace: Duration) {
    let watched = Arc::clone(shared);
    let spawned = std::thread::Builder::new().name("os2-kill-grace".into()).spawn(move || {
        let deadline = std::time::Instant::now() + grace;
        while std::time::Instant::now() < deadline {
            if watched.exit_requested.load(Ordering::Relaxed) { return; }
            std::thread::sleep(GRACE_POLL);
        }
        if !watched.exit_requested.load(Ordering::Relaxed) {
            warn!("the process did not end within {:?} of being asked to; ending it", grace);
            request_exit(&watched, SIGNAL_EXIT_CODE);
        }
    });
    if spawned.is_err() {
        // No watcher: do not wait for the process at all.
        request_exit(shared, SIGNAL_EXIT_CODE);
    }
}

/// The process is asked to end from outside: its session (desktop, VIO
/// window, text window) was closed, or `XCPT_SIGNAL_KILLPROC` came from
/// another process or the host's `SIGTERM`. Thread 1 gets
/// `XCPT_SIGNAL_KILLPROC` (its exit list runs with `TC_KILLPROCESS` unless
/// a handler goes on); after `KILL_GRACE` the process ends regardless.
/// A second request ends the process at once.
pub(crate) fn end_session(shared: &Arc<SharedState>) {
    let first = shared.kills.lock_or_recover().begin_ending();
    if !first {
        request_exit(shared, SIGNAL_EXIT_CODE);
        return;
    }
    debug!("  the process is asked to end (XCPT_SIGNAL_KILLPROC)");
    post_signal(shared, XCPT_SIGNAL_KILLPROC);
    exit_after(shared, KILL_GRACE);
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
            wake_thread_1(&self.shared);
        }
        rc
    }

    /// DosSendSignalException (ordinal 379).
    pub fn dos_send_signal_exception(&self, pid: u32, signal: u32) -> u32 {
        debug!("  DosSendSignalException(pid={}, signal={})", pid, signal);
        if signal != XCPT_SIGNAL_INTR && signal != XCPT_SIGNAL_BREAK {
            return ERROR_INVALID_SIGNAL_NUMBER;
        }
        if pid == self.shared.process_id() {
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

    /// DosKillProcess (ordinal 235): `XCPT_SIGNAL_KILLPROC` to process
    /// `pid` (`DKP_PROCESS`), or to it and its descendants
    /// (`DKP_PROCESSTREE`). Returns at once; the process ends in its own
    /// time (within `KILL_GRACE`).
    pub fn dos_kill_process(&self, action: u32, pid: u32) -> u32 {
        debug!("  DosKillProcess(action={}, pid={})", action, pid);
        if action != DKP_PROCESS && action != DKP_PROCESSTREE {
            return ERROR_INVALID_DATA;
        }
        if pid == self.shared.process_id() {
            if action == DKP_PROCESSTREE {
                self.kill_children();
            }
            self.post_signal(XCPT_SIGNAL_KILLPROC);
            return NO_ERROR;
        }
        self.kill_child(pid, action)
    }

    /// `DosKillProcess(DKP_PROCESSTREE)` for every running child.
    pub(crate) fn kill_children(&self) {
        let pids: Vec<u32> = self.shared.process_mgr.lock_or_recover().children.keys().copied().collect();
        for pid in pids {
            self.kill_child(pid, DKP_PROCESSTREE);
        }
    }

    /// Kill child `pid`: the request goes through a pidfd taken while the
    /// child is known to be unreaped; a watcher ends its host process if
    /// it is still there after the grace period.
    fn kill_child(&self, pid: u32, action: u32) -> u32 {
        let request = if action == DKP_PROCESSTREE { HostRequest::KillTree } else {
            HostRequest::Signal(XCPT_SIGNAL_KILLPROC)
        };
        let Some(sig) = super::host_signal::request_signal(request) else { return ERROR_INVALID_DATA };
        let handle = {
            let mut procs = self.shared.process_mgr.lock_or_recover();
            let Some(child) = procs.children.get_mut(&pid) else { return ERROR_INVALID_PROCID };
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(_)) => return ERROR_ZOMBIE_PROCESS,
                Err(_) => return ERROR_INVALID_PROCID,
            }
            match ProcessHandle::open(child.id()) {
                Some(h) => h,
                None => {
                    // No pidfd (Linux before 5.3): end the host process at
                    // once, without the child's handlers and exit list.
                    warn!("  DosKillProcess: no pidfd; ending child {} at once", pid);
                    let killed = child.kill().is_ok();
                    drop(procs);
                    if killed { self.shared.kills.lock_or_recover().mark_killed(pid); }
                    return if killed { NO_ERROR } else { ERROR_ZOMBIE_PROCESS };
                }
            }
        };
        if !handle.send(sig) {
            return ERROR_ZOMBIE_PROCESS; // it ended meanwhile
        }
        if self.shared.kills.lock_or_recover().mark_killed(pid) {
            let shared = Arc::clone(&self.shared);
            let watcher = std::thread::Builder::new().name("os2-kill-watch".into()).spawn(move || {
                if !handle.wait_exit(KILL_GRACE + KILL_GRACE_PARENT_MARGIN) {
                    warn!("  killed child {} did not end within the grace period; ending it", pid);
                    handle.send(libc::SIGKILL);
                }
                shared.kills.lock_or_recover().unwatch(pid);
            });
            if watcher.is_err() {
                self.shared.kills.lock_or_recover().unwatch(pid);
            }
        }
        NO_ERROR
    }

    /// `codeTerminate` for child `pid` that ended with exit code `code`
    /// (`DosWaitChild`): `TC_KILLPROCESS` for a child `DosKillProcess` ended.
    pub(crate) fn child_termination_code(&self, pid: u32, code: u32) -> u32 {
        let killed = self.shared.kills.lock_or_recover().reaped(pid);
        if killed && code == SIGNAL_EXIT_CODE { TC_KILLPROCESS } else { TC_EXIT }
    }

    /// A signal exception for this process arrived.
    pub(crate) fn post_signal(&self, signal: u32) {
        post_signal(&self.shared, signal);
    }

    /// A byte read from the keyboard (a terminal, or piped input standing
    /// for it): Ctrl+C is `XCPT_SIGNAL_INTR`, not a character. True when
    /// it was one.
    pub(crate) fn keyboard_signal(&self, byte: u8) -> bool {
        let Some(signal) = keyboard_byte_signal(byte) else { return false };
        debug!("  Ctrl+C typed: XCPT_SIGNAL_INTR");
        self.post_signal(signal);
        true
    }

    /// Start taking signal exceptions and kill requests from other Warpine
    /// processes and from the host.
    pub(crate) fn listen_for_signals(&self) {
        let shared = Arc::clone(&self.shared);
        let vm = Arc::clone(&self.vm);
        super::host_signal::listen(move |request| {
            let loader = super::Loader { vm: Arc::clone(&vm), shared: Arc::clone(&shared) };
            match request {
                HostRequest::Signal(XCPT_SIGNAL_KILLPROC) => end_session(&shared),
                HostRequest::Signal(signal) => loader.post_signal(signal),
                HostRequest::KillTree => {
                    // The descendants first: this process may be gone soon.
                    loader.kill_children();
                    end_session(&shared);
                }
            }
        });
    }

    /// Whether thread 1, now at `vcpu`'s registers, can take a handler
    /// call: 32-bit code with room on its stack.
    pub(crate) fn can_take_signal(&self, vcpu: &dyn VcpuBackend) -> bool {
        let (Ok(regs), Ok(sregs)) = (vcpu.get_regs(), vcpu.get_sregs()) else { return false };
        let esp = regs.rsp as u32;
        let room = esp.checked_sub(HANDLER_FRAME_BYTES)
            .is_some_and(|low| self.guest_read::<u32>(low).is_some() && self.guest_read::<u32>(esp - 4).is_some());
        sregs.cs.db != 0 && sregs.ss.db != 0 && room
    }

    /// Thread 1 at the top of its vCPU loop: deliver a pending signal.
    pub(crate) fn signal_action(&self, vcpu: &dyn VcpuBackend, vcpu_id: u32) -> SignalAction {
        let threads = &self.shared.threads;
        if super::threads::tid_of_vcpu(vcpu_id) != MAIN_TID
            || !threads.signal_pending.load(Ordering::Acquire)
            || self.runs_exit_list(vcpu_id)
            // A must-complete section holds signals and asynchronous
            // termination until it ends (DosExitMustComplete).
            || threads.must_complete.load(Ordering::Acquire) != 0 {
            return SignalAction::None;
        }
        // A termination asked for from outside comes before signals.
        let termination = threads.signals.lock_or_recover().take_termination();
        if let Some((rc, tc)) = termination {
            let exit_list = vcpu.get_sregs().is_ok_and(|s| s.cs.db != 0 && s.ss.db != 0);
            debug!("  [VCPU {}] the process is to end (rc={}, tc={})", vcpu_id, rc, tc);
            return SignalAction::Terminate { rc, tc, exit_list };
        }
        // Only 32-bit code with room on its stack can take a handler call.
        if !self.can_take_signal(vcpu) {
            return SignalAction::None;
        }
        let (Ok(regs), Ok(sregs)) = (vcpu.get_regs(), vcpu.get_sregs()) else { return SignalAction::None };
        let (signal, ends) = {
            let mut signals = threads.signals.lock_or_recover();
            let Some(signal) = signals.take() else {
                threads.signal_pending.store(false, Ordering::Release);
                return SignalAction::None;
            };
            if !signals.wants_thread_1() {
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
            esp: regs.rsp as u32, eip: regs.rip as u32, eflags: regs.rflags as u32,
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
            t.start(2, ThreadMemory { stack: 0x1000, tib: 0x2000, paging: 0 });
            t.end(MAIN_TID);
        }
        loader.post_signal(XCPT_SIGNAL_INTR);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), SIGNAL_EXIT_CODE as i32);
    }
}

#[cfg(test)]
mod kill_tests {
    use super::*;
    use super::super::Loader;
    use super::super::interrupt;
    use super::super::threads::{MAIN_VCPU_ID, ThreadMemory};
    use super::super::vm_backend::mock::MockVcpu;

    fn main_vcpu() -> MockVcpu {
        let mut v = MockVcpu::new();
        v.regs.rsp = 0x8000;
        v.sregs.cs.db = 1;
        v.sregs.ss.db = 1;
        v
    }

    #[test]
    fn keys_and_bytes_that_are_signals() {
        assert_eq!(keyboard_byte_signal(CTRL_C), Some(XCPT_SIGNAL_INTR));
        assert_eq!(keyboard_byte_signal(b'c'), None);
        assert_eq!(key_signal(true, true, false, false), Some(XCPT_SIGNAL_INTR));
        assert_eq!(key_signal(false, true, false, false), None, "plain c");
        assert_eq!(key_signal(true, false, true, false), Some(XCPT_SIGNAL_BREAK));
        assert_eq!(key_signal(false, false, true, false), None, "plain Pause");
        assert_eq!(key_signal(false, false, false, true), Some(XCPT_SIGNAL_BREAK));
        assert_eq!(key_signal(true, false, false, false), None);
    }

    #[test]
    fn kill_process_checks_action_and_process() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_kill_process(2, 12345), ERROR_INVALID_DATA);
        assert_eq!(loader.dos_kill_process(DKP_PROCESS, 12345), ERROR_INVALID_PROCID);
        assert_eq!(loader.dos_kill_process(DKP_PROCESSTREE, 12345), ERROR_INVALID_PROCID);
        // An ended child not yet waited for is a zombie.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        let pid = loader.shared.process_mgr.lock_or_recover().add_child(child);
        assert_eq!(loader.dos_kill_process(DKP_PROCESS, pid), ERROR_ZOMBIE_PROCESS);
    }

    #[test]
    fn killing_itself_sends_killproc_to_thread_1() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_kill_process(DKP_PROCESS, PROCESS_ID), NO_ERROR);
        // KILLPROC needs no focus; with no handler it ends the process
        // (the vCPU loop runs the exit list with TC_KILLPROCESS).
        let vcpu = main_vcpu();
        loader.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, XCPT_CHAIN_END);
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::EndProcess));
    }

    #[test]
    fn a_killed_child_ends_and_reports_tc_killprocess() {
        let loader = Loader::new_mock();
        // A host process standing for a child: the real-time signal's
        // default action ends it, as the grace period's SIGKILL would.
        let child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = loader.shared.process_mgr.lock_or_recover().add_child(child);
        assert_eq!(loader.dos_kill_process(DKP_PROCESS, pid), NO_ERROR);
        let (res, ppid) = (0x3000u32, 0x3010u32);
        assert_eq!(loader.dos_wait_child(0, 0, res, ppid, pid), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(res), Some(TC_KILLPROCESS));
        assert_eq!(loader.guest_read::<u32>(ppid), Some(pid));
        // Waited for: no longer a child.
        assert_eq!(loader.dos_kill_process(DKP_PROCESS, pid), ERROR_INVALID_PROCID);
    }

    #[test]
    fn a_child_ending_normally_reports_tc_exit() {
        let loader = Loader::new_mock();
        let child = std::process::Command::new("sh").args(["-c", "exit 1"]).spawn().unwrap();
        let pid = loader.shared.process_mgr.lock_or_recover().add_child(child);
        let res = 0x3000u32;
        assert_eq!(loader.dos_wait_child(0, 0, res, 0x3010, pid), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(res), Some(TC_EXIT));
        assert_eq!(loader.guest_read::<u32>(res + 4), Some(1));
    }

    #[test]
    fn a_signal_interrupts_dos_wait_child() {
        let loader = Loader::new_mock();
        let child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = loader.shared.process_mgr.lock_or_recover().add_child(child);
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        loader.post_signal(XCPT_SIGNAL_INTR);
        interrupt::begin_call(MAIN_TID, true);
        assert_eq!(loader.dos_wait_child(0, 0, 0x3000, 0x3010, pid), ERROR_INTERRUPT);
        assert_eq!(loader.dos_wait_child(1, 0, 0x3000, 0x3010, 0), ERROR_INTERRUPT);
        interrupt::end_call();
        // The child is still there to be waited for (or killed).
        assert_eq!(loader.dos_wait_child(0, 1, 0x3000, 0x3010, pid), 128);
        let mut child = loader.shared.process_mgr.lock_or_recover().take_child(pid).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_termination_runs_on_thread_1_once() {
        let loader = Loader::new_mock();
        let vcpu = main_vcpu();
        request_termination(&loader.shared, 7, TC_TRAP);
        // The first request wins.
        request_termination(&loader.shared, 8, TC_EXIT);
        assert!(matches!(loader.signal_action(&vcpu, 2), SignalAction::None), "not thread 1");
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID),
                         SignalAction::Terminate { rc: 7, tc: TC_TRAP, exit_list: true }));
        assert!(matches!(loader.signal_action(&vcpu, MAIN_VCPU_ID), SignalAction::None));
        // 16-bit code: the process ends without an exit list.
        let loader = Loader::new_mock();
        let mut vcpu16 = main_vcpu();
        vcpu16.sregs.cs.db = 0;
        request_termination(&loader.shared, 2, TC_EXIT);
        assert!(matches!(loader.signal_action(&vcpu16, MAIN_VCPU_ID),
                         SignalAction::Terminate { exit_list: false, .. }));
    }

    #[test]
    fn without_thread_1_a_termination_or_killproc_ends_at_once() {
        let loader = Loader::new_mock();
        {
            let mut t = loader.shared.threads.table.lock_or_recover();
            t.start(2, ThreadMemory { stack: 0x1000, tib: 0x2000, paging: 0 });
            t.end(MAIN_TID);
        }
        request_termination(&loader.shared, 4, TC_EXIT);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), 4);
        let loader = Loader::new_mock();
        loader.shared.threads.table.lock_or_recover().end(MAIN_TID);
        loader.post_signal(XCPT_SIGNAL_KILLPROC);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
    }

    #[test]
    fn ending_a_session_twice_ends_it_at_once() {
        let loader = Loader::new_mock();
        end_session(&loader.shared);
        assert!(!loader.shared.exit_requested.load(Ordering::Relaxed), "thread 1 is asked first");
        assert!(loader.shared.threads.signals.lock_or_recover().deliverable(), "KILLPROC waits");
        end_session(&loader.shared);
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
        assert_eq!(loader.shared.exit_code.load(Ordering::Relaxed), SIGNAL_EXIT_CODE as i32);
    }

    #[test]
    fn the_grace_period_ends_a_process_that_does_not_end() {
        let loader = Loader::new_mock();
        exit_after(&loader.shared, Duration::from_millis(60));
        std::thread::sleep(Duration::from_millis(20));
        assert!(!loader.shared.exit_requested.load(Ordering::Relaxed));
        let start = std::time::Instant::now();
        while !loader.shared.exit_requested.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(loader.shared.exit_requested.load(Ordering::Relaxed));
    }

    #[test]
    fn kill_table_bookkeeping() {
        let mut k = KillTable::default();
        assert!(k.mark_killed(5), "first: a watcher starts");
        assert!(!k.mark_killed(5), "watched already");
        k.unwatch(5);
        assert!(k.mark_killed(5));
        assert!(k.reaped(5));
        assert!(!k.reaped(5));
        assert!(k.begin_ending());
        assert!(!k.begin_ending());
    }

    /// Inside a must-complete section signals wait: blocked calls are not
    /// interrupted and none is delivered until the section ends.
    #[test]
    fn a_must_complete_section_holds_signals() {
        use super::super::interrupt::{begin_call, end_call};
        use super::super::threads::MAIN_VCPU_ID;
        let loader = Loader::new_mock();
        let nest = 0x3000;
        assert_eq!(loader.dos_must_complete(MAIN_VCPU_ID, nest, false), ERROR_ALREADY_RESET);
        assert_eq!(loader.dos_must_complete(MAIN_VCPU_ID, nest, true), NO_ERROR);
        assert_eq!(loader.dos_must_complete(MAIN_VCPU_ID, nest, true), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(nest), Some(2));
        loader.dos_set_signal_exception_focus(SIG_SETFOCUS, 0);
        loader.post_signal(XCPT_SIGNAL_INTR);
        begin_call(MAIN_TID, true);
        assert!(!loader.interrupted(), "held in the section");
        assert_eq!(loader.dos_must_complete(MAIN_VCPU_ID, nest, false), NO_ERROR);
        assert!(!loader.interrupted(), "still one level");
        assert_eq!(loader.dos_must_complete(MAIN_VCPU_ID, nest, false), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(nest), Some(0));
        assert!(loader.interrupted(), "delivered once it ends");
        end_call();
        // Another thread's sections are its own; a thread that ends leaves them.
        assert_eq!(loader.dos_must_complete(7, nest, true), NO_ERROR);
        assert_eq!(loader.shared.threads.must_complete.load(Ordering::Acquire), 0);
        loader.shared.threads.forget_must_complete(super::super::threads::tid_of_vcpu(7));
        assert_eq!(loader.dos_must_complete(7, nest, false), ERROR_ALREADY_RESET);
    }
}
