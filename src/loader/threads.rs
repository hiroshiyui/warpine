// SPDX-License-Identifier: GPL-3.0-only
//
//! The threads of the OS/2 process: which run, which have ended and not yet
//! been waited for, and the idle vCPUs left behind by ended threads.
//!
//! Behaviour from IBM's *Control Program Programming Reference*:
//! - `DosExit(EXIT_THREAD, rc)` ends the calling thread; "if the current
//!   thread is the last thread, the process is ended" (with `rc`).
//! - `DosExit(EXIT_PROCESS, rc)` ends all threads and the process.
//! - `DosWaitThread(&tid, DCWW_WAIT)`: "if a thread has already ended, the
//!   call returns immediately"; `*ptid == 0` waits for the next thread of
//!   the process to end and returns its id; `DCWW_NOWAIT` returns
//!   `ERROR_THREAD_NOT_TERMINATED` if the thread has not ended; waiting for
//!   the current thread or for thread 1 is `ERROR_INVALID_THREADID`.
//!
//! A thread function returning ends the thread like `DosExit(EXIT_THREAD,
//! eax)`. Thread 1 (the main thread) returning from the program's entry point
//! ends the process.
//!
//! KVM cannot destroy a vCPU, and vCPU ids are limited, so the vCPU of an
//! ended thread is kept here and reused by the next `DosCreateThread`.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::constants::{ERROR_CRITSEC_OVERFLOW, ERROR_CRITSEC_UNDERFLOW, ERROR_INVALID_THREADID,
                       ERROR_NOT_FROZEN, NO_ERROR};
use super::mutex_ext::MutexExt;
use super::vm_backend::{VcpuBackend, VcpuKick};

/// OS/2 thread id of the main thread.
pub const MAIN_TID: u32 = 1;
/// Warpine's id of the main thread's vCPU (and of its per-thread state).
pub const MAIN_VCPU_ID: u32 = 0;
/// Thread id given to the first thread `DosCreateThread` creates.
pub const FIRST_CREATED_TID: u32 = 2;
/// Ended threads remembered for `DosWaitThread` before the oldest is
/// forgotten (a program that never waits must not grow the table forever).
pub const MAX_ENDED_THREADS: usize = 4096;
/// How often a waiting `DosWaitThread` looks at the process-exit flag.
const WAIT_POLL: Duration = Duration::from_millis(100);

/// `DosExit` actions (`<bsedos.h>`).
pub const EXIT_THREAD: u32 = 0;
pub const EXIT_PROCESS: u32 = 1;
/// `DosWaitThread` options (`<bsedos.h>`).
pub const DCWW_WAIT: u32 = 0;
pub const DCWW_NOWAIT: u32 = 1;

/// The OS/2 thread id of the thread running on vCPU `vcpu_id`.
pub fn tid_of_vcpu(vcpu_id: u32) -> u32 {
    if vcpu_id == MAIN_VCPU_ID { MAIN_TID } else { vcpu_id }
}

/// How a vCPU loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadEnd {
    /// The thread ended by itself (`DosExit(EXIT_THREAD)`, a return from
    /// the thread function, `DosKillThread`) with this result code.
    Thread(u32),
    /// The thread stopped because the process is ending.
    Process,
}

/// What `ThreadTable::end` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOutcome {
    /// Other threads still run.
    ThreadOnly,
    /// This was the last running thread: the process ends.
    LastThread,
}

/// What `ThreadTable::wait` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// This thread has ended (and is now forgotten).
    Ended(u32),
    /// No thread the caller may wait for has ended yet.
    Running,
    /// Not a thread the caller can wait for.
    Invalid,
}

/// Guest memory Warpine allocated for a created thread, freed when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadMemory {
    pub stack: u32,
    pub tib: u32,
}

/// The process's threads (pure bookkeeping, no blocking).
pub struct ThreadTable {
    running: BTreeSet<u32>,
    /// Created threads that ended and were not yet waited for, oldest first.
    ended: VecDeque<u32>,
    killed: BTreeSet<u32>,
    memory: HashMap<u32, ThreadMemory>,
    next_tid: u32,
    /// vCPU ids handed to KVM so far (the main thread's is `MAIN_VCPU_ID`).
    next_kvm_id: u64,
    idle_vcpus: Vec<Box<dyn VcpuBackend>>,
    /// Suspend counts of suspended threads (`DosSuspendThread`).
    suspended: BTreeMap<u32, u32>,
    /// Kick handles of the vCPUs of running threads (`VcpuKick`).
    kickers: HashMap<u32, Arc<dyn VcpuKick>>,
}

impl Default for ThreadTable {
    fn default() -> Self { Self::new() }
}

impl ThreadTable {
    /// A process whose main thread runs.
    pub fn new() -> Self {
        ThreadTable {
            running: BTreeSet::from([MAIN_TID]),
            ended: VecDeque::new(),
            killed: BTreeSet::new(),
            memory: HashMap::new(),
            next_tid: FIRST_CREATED_TID,
            next_kvm_id: MAIN_VCPU_ID as u64 + 1,
            idle_vcpus: Vec::new(),
            suspended: BTreeMap::new(),
            kickers: HashMap::new(),
        }
    }

    /// Reserve the id of a thread about to be created. Thread ids are not
    /// reused, so per-thread state left by an ended thread never leaks into
    /// a new one.
    pub fn next_tid(&mut self) -> Option<u32> {
        let tid = self.next_tid;
        self.next_tid = self.next_tid.checked_add(1)?;
        Some(tid)
    }

    /// Record thread `tid` as running, with the guest memory to free at its end.
    pub fn start(&mut self, tid: u32, memory: ThreadMemory) {
        self.running.insert(tid);
        self.memory.insert(tid, memory);
    }

    /// An idle vCPU, or the id to create a new one with.
    pub fn take_vcpu(&mut self) -> Result<Box<dyn VcpuBackend>, u64> {
        self.idle_vcpus.pop().ok_or_else(|| {
            let id = self.next_kvm_id;
            self.next_kvm_id += 1;
            id
        })
    }

    /// Keep the vCPU of an ended thread for the next created thread.
    pub fn put_vcpu(&mut self, vcpu: Box<dyn VcpuBackend>) {
        self.idle_vcpus.push(vcpu);
    }

    /// The TIB of thread `tid`: `TIB_BASE` for thread 1, the TIB Warpine
    /// allocated for a created thread; `None` for a thread that is not running.
    pub fn tib_of(&self, tid: u32) -> Option<u32> {
        if tid == MAIN_TID { return Some(super::constants::TIB_BASE); }
        self.memory.get(&tid).map(|m| m.tib)
    }

    pub fn is_running(&self, tid: u32) -> bool {
        self.running.contains(&tid)
    }

    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Thread `tid` has ended. Returns whether it was the last one and the
    /// guest memory to free.
    pub fn end(&mut self, tid: u32) -> (EndOutcome, Option<ThreadMemory>) {
        let was_running = self.running.remove(&tid);
        self.killed.remove(&tid);
        self.suspended.remove(&tid);
        self.kickers.remove(&tid);
        if was_running && tid != MAIN_TID {
            if self.ended.len() >= MAX_ENDED_THREADS {
                self.ended.pop_front();
            }
            self.ended.push_back(tid);
        }
        let outcome = if self.running.is_empty() { EndOutcome::LastThread } else { EndOutcome::ThreadOnly };
        (outcome, self.memory.remove(&tid))
    }

    /// `DosWaitThread` from thread `caller` for thread `tid` (0 = any).
    /// An ended thread is reported once.
    pub fn wait(&mut self, caller: u32, tid: u32) -> WaitOutcome {
        if tid == MAIN_TID || tid == caller {
            return WaitOutcome::Invalid;
        }
        if tid == 0 {
            if let Some(t) = self.ended.pop_front() {
                return WaitOutcome::Ended(t);
            }
            // Only threads other than the caller and thread 1 can end for it.
            let others = self.running.iter().any(|&t| t != caller && t != MAIN_TID);
            return if others { WaitOutcome::Running } else { WaitOutcome::Invalid };
        }
        if let Some(pos) = self.ended.iter().position(|&t| t == tid) {
            self.ended.remove(pos);
            return WaitOutcome::Ended(tid);
        }
        if self.running.contains(&tid) { WaitOutcome::Running } else { WaitOutcome::Invalid }
    }

    /// `DosKillThread`: flag running thread `tid` to end at its next exit to
    /// Warpine. Not the caller itself.
    pub fn kill(&mut self, caller: u32, tid: u32) -> bool {
        if tid == caller || !self.running.contains(&tid) {
            return false;
        }
        self.killed.insert(tid);
        true
    }

    pub fn is_killed(&self, tid: u32) -> bool {
        self.killed.contains(&tid)
    }

    /// `DosSuspendThread` from `caller`: one more suspension of running
    /// thread `tid` (not the caller itself).
    pub fn suspend(&mut self, caller: u32, tid: u32) -> u32 {
        if tid == caller || !self.running.contains(&tid) {
            return ERROR_INVALID_THREADID;
        }
        let count = self.suspended.entry(tid).or_insert(0);
        *count = count.saturating_add(1);
        NO_ERROR
    }

    /// `DosResumeThread`: one suspension less; the thread runs again at 0.
    pub fn resume(&mut self, tid: u32) -> u32 {
        if !self.running.contains(&tid) {
            return ERROR_INVALID_THREADID;
        }
        match self.suspended.get_mut(&tid) {
            None => ERROR_NOT_FROZEN,
            Some(count) => {
                *count -= 1;
                if *count == 0 {
                    self.suspended.remove(&tid);
                }
                NO_ERROR
            }
        }
    }

    pub fn suspend_count(&self, tid: u32) -> u32 {
        self.suspended.get(&tid).copied().unwrap_or(0)
    }

    /// Thread `tid` runs on the vCPU `kicker` kicks.
    pub fn set_kicker(&mut self, tid: u32, kicker: Arc<dyn VcpuKick>) {
        self.kickers.insert(tid, kicker);
    }

    /// Make thread `tid` leave guest code and look at its flags.
    pub fn kick(&self, tid: u32) {
        if let Some(k) = self.kickers.get(&tid) {
            k.kick();
        }
    }

    /// Kick every running thread except `except` (0: all of them).
    pub fn kick_all_but(&self, except: u32) {
        for (_, k) in self.kickers.iter().filter(|(t, _)| **t != except) {
            k.kick();
        }
    }
}

/// Highest nesting count of a critical section: the count is a word
/// (`DosEnterCritSec`).
pub const MAX_CRITSEC_NESTING: u32 = 0xFFFF;

/// The process's critical section (`DosEnterCritSec` / `DosExitCritSec`):
/// while a thread owns it, the other threads do not run. Pure bookkeeping.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CritSection {
    /// The owning thread (OS/2 thread id) while the count is above 0.
    owner: Option<u32>,
    count: u32,
}

impl CritSection {
    pub fn owner(&self) -> Option<u32> { self.owner }
    pub fn count(&self) -> u32 { self.count }

    /// Whether thread `tid` must not run now.
    pub fn blocks(&self, tid: u32) -> bool {
        self.owner.is_some_and(|o| o != tid)
    }

    /// `DosEnterCritSec` by `tid`, which does not block (`blocks` is false).
    /// At the highest count nothing changes: `ERROR_CRITSEC_OVERFLOW`.
    pub fn enter(&mut self, tid: u32) -> u32 {
        debug_assert!(!self.blocks(tid));
        if self.count >= MAX_CRITSEC_NESTING {
            return ERROR_CRITSEC_OVERFLOW;
        }
        self.owner = Some(tid);
        self.count += 1;
        NO_ERROR
    }

    /// `DosExitCritSec` by `tid`: one level less; the section ends at 0.
    /// Not in a section: `ERROR_CRITSEC_UNDERFLOW`, nothing changes.
    pub fn exit(&mut self, tid: u32) -> u32 {
        if self.owner != Some(tid) || self.count == 0 {
            return ERROR_CRITSEC_UNDERFLOW;
        }
        self.count -= 1;
        if self.count == 0 {
            self.owner = None;
        }
        NO_ERROR
    }

    /// Thread `tid` ended: a section it owned ends with it. Whether it did.
    pub fn release(&mut self, tid: u32) -> bool {
        if self.owner == Some(tid) {
            *self = CritSection::default();
            true
        } else {
            false
        }
    }
}

/// The thread table with a condition variable signalled when a thread ends,
/// the process's exit list (`exitlist.rs`) and its critical section.
#[derive(Default)]
pub struct Threads {
    pub table: Mutex<ThreadTable>,
    pub ended: Condvar,
    /// Lock order: `table` before `exit_list`.
    pub exit_list: Mutex<super::exitlist::ExitList>,
    /// Exit-list processing runs (a quick look before `exit_list`).
    pub exit_list_active: AtomicBool,
    /// Lock order: `crit` before `table` and `exit_list`.
    pub crit: Mutex<CritSection>,
    /// Signalled when the critical section ends (and when the process
    /// ends or a thread is killed, so that blocked threads look again).
    pub crit_cond: Condvar,
    /// A thread owns the critical section (a quick look before `crit`).
    pub crit_held: AtomicBool,
    /// Signalled when a thread is resumed (and when the process ends).
    pub resumed: Condvar,
    /// Signal exceptions for thread 1 (`signals.rs`).
    pub signals: Mutex<super::signals::SignalState>,
    /// A signal may be deliverable (a quick look before `signals`).
    pub signal_pending: AtomicBool,
}

impl Threads {
    /// Whether the calling host thread must stop waiting because another
    /// thread runs the exit list.
    pub fn exit_list_stops_caller(&self) -> bool {
        self.exit_list_active.load(Ordering::Acquire)
            && self.exit_list.lock_or_recover().run()
                .is_some_and(|r| r.host != std::thread::current().id())
    }

    /// Block until `wait` gives an answer other than `Running`, the process
    /// ends or the wait is interrupted (`exiting`: `None`), or — with
    /// `DCWW_NOWAIT` — at once. `exiting` runs without the table locked (it
    /// may look at the table itself).
    pub fn wait_for(&self, caller: u32, tid: u32, no_wait: bool, exiting: impl Fn() -> bool) -> Option<WaitOutcome> {
        loop {
            let outcome = self.table.lock_or_recover().wait(caller, tid);
            if outcome != WaitOutcome::Running || no_wait {
                return Some(outcome);
            }
            if exiting() {
                return None;
            }
            let mut table = self.table.lock_or_recover();
            // Look again under this lock: an end notified between the two
            // locks would otherwise be missed until `WAIT_POLL`.
            let outcome = table.wait(caller, tid);
            if outcome != WaitOutcome::Running {
                return Some(outcome);
            }
            table = self.ended.wait_timeout(table, WAIT_POLL)
                .unwrap_or_else(|e| e.into_inner()).0;
            drop(table);
        }
    }

    /// Block thread `tid` while another thread owns the critical section.
    /// True: it may run; false: `stop()` (process ending, thread killed)
    /// said to give up first. `stop` runs with `crit` locked.
    pub fn wait_crit_sec(&self, tid: u32, stop: impl Fn() -> bool) -> bool {
        if !self.crit_held.load(Ordering::Acquire) {
            return true;
        }
        let mut crit = self.crit.lock_or_recover();
        while crit.blocks(tid) {
            if stop() {
                return false;
            }
            crit = self.crit_cond.wait_timeout(crit, WAIT_POLL)
                .unwrap_or_else(|e| e.into_inner()).0;
        }
        true
    }

    /// `DosEnterCritSec` by `tid`: wait while another thread owns the
    /// section, then enter it. `None`: `stop()` said to give up. The other
    /// threads are kicked out of guest code, so that they stop at once.
    pub fn enter_crit_sec(&self, tid: u32, stop: impl Fn() -> bool) -> Option<u32> {
        let mut crit = self.crit.lock_or_recover();
        while crit.blocks(tid) {
            if stop() {
                return None;
            }
            crit = self.crit_cond.wait_timeout(crit, WAIT_POLL)
                .unwrap_or_else(|e| e.into_inner()).0;
        }
        let rc = crit.enter(tid);
        self.crit_held.store(crit.owner().is_some(), Ordering::Release);
        // Lock order: `crit` before `table`.
        self.table.lock_or_recover().kick_all_but(tid);
        Some(rc)
    }

    /// Block thread `tid` while it is suspended (`DosSuspendThread`) and
    /// `stop()` (process ending, exit list) does not say to give up. A
    /// killed thread stays suspended until it is resumed. Whether it was
    /// suspended (the caller looks at its flags again then).
    pub fn wait_while_suspended(&self, tid: u32, stop: impl Fn() -> bool) -> bool {
        let mut table = self.table.lock_or_recover();
        let suspended = table.suspend_count(tid) > 0;
        while table.suspend_count(tid) > 0 && !stop() {
            table = self.resumed.wait_timeout(table, WAIT_POLL)
                .unwrap_or_else(|e| e.into_inner()).0;
        }
        suspended
    }

    /// `DosSuspendThread` by `caller` for `tid`: the thread stops at once
    /// if it runs guest code, else when it next leaves Warpine.
    pub fn suspend(&self, caller: u32, tid: u32) -> u32 {
        let mut table = self.table.lock_or_recover();
        let rc = table.suspend(caller, tid);
        if rc == NO_ERROR {
            table.kick(tid);
        }
        rc
    }

    /// `DosResumeThread` for `tid`.
    pub fn resume(&self, tid: u32) -> u32 {
        let rc = self.table.lock_or_recover().resume(tid);
        if rc == NO_ERROR {
            self.resumed.notify_all();
        }
        rc
    }

    /// `DosExitCritSec` by `tid`.
    pub fn exit_crit_sec(&self, tid: u32) -> u32 {
        let mut crit = self.crit.lock_or_recover();
        let rc = crit.exit(tid);
        self.crit_held.store(crit.owner().is_some(), Ordering::Release);
        drop(crit);
        self.crit_cond.notify_all();
        rc
    }

    /// Thread `tid` ended: end a critical section it owned.
    pub fn release_crit_sec(&self, tid: u32) {
        let mut crit = self.crit.lock_or_recover();
        if crit.release(tid) {
            self.crit_held.store(false, Ordering::Release);
            drop(crit);
            self.crit_cond.notify_all();
        }
    }

    /// Wake threads blocked by the critical section so that they look at
    /// the process-exit and kill flags again.
    pub fn wake_crit_waiters(&self) {
        self.crit_cond.notify_all();
        self.resumed.notify_all();
    }

    /// Block until the process ends (the main thread ended by itself while
    /// other threads still run).
    pub fn wait_for_process_end(&self, exiting: impl Fn() -> bool) {
        let mut table = self.table.lock_or_recover();
        while !exiting() {
            table = self.ended.wait_timeout(table, WAIT_POLL)
                .unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::vm_backend::mock::MockVcpu;
    use crate::loader::vm_backend::VcpuBackend;

    const MEM: ThreadMemory = ThreadMemory { stack: 0x1000, tib: 0x2000 };

    fn with_threads(tids: &[u32]) -> ThreadTable {
        let mut t = ThreadTable::new();
        for &tid in tids {
            t.start(tid, MEM);
        }
        t
    }

    #[test]
    fn main_thread_runs_and_created_ids_start_at_two() {
        let mut t = ThreadTable::new();
        assert!(t.is_running(MAIN_TID));
        assert_eq!(t.next_tid(), Some(FIRST_CREATED_TID));
        assert_eq!(t.next_tid(), Some(FIRST_CREATED_TID + 1));
        assert_eq!(tid_of_vcpu(MAIN_VCPU_ID), MAIN_TID);
        assert_eq!(tid_of_vcpu(5), 5);
    }

    #[test]
    fn ending_one_thread_keeps_the_process() {
        let mut t = with_threads(&[2, 3]);
        assert_eq!(t.end(2), (EndOutcome::ThreadOnly, Some(MEM)));
        assert!(!t.is_running(2));
        assert_eq!(t.running_count(), 2);
    }

    #[test]
    fn the_last_thread_ends_the_process() {
        let mut t = with_threads(&[2]);
        assert_eq!(t.end(MAIN_TID).0, EndOutcome::ThreadOnly);
        assert_eq!(t.end(2).0, EndOutcome::LastThread);
    }

    #[test]
    fn main_thread_ending_while_others_run_keeps_the_process() {
        let mut t = with_threads(&[2]);
        assert_eq!(t.end(MAIN_TID), (EndOutcome::ThreadOnly, None));
        // Thread 1 cannot be waited for, ended or not.
        assert_eq!(t.wait(2, MAIN_TID), WaitOutcome::Invalid);
    }

    #[test]
    fn wait_for_an_ended_thread_returns_once() {
        let mut t = with_threads(&[2, 3]);
        assert_eq!(t.wait(MAIN_TID, 2), WaitOutcome::Running);
        t.end(2);
        assert_eq!(t.wait(MAIN_TID, 2), WaitOutcome::Ended(2));
        assert_eq!(t.wait(MAIN_TID, 2), WaitOutcome::Invalid);
    }

    #[test]
    fn wait_for_any_thread_takes_them_in_end_order() {
        let mut t = with_threads(&[2, 3, 4]);
        assert_eq!(t.wait(MAIN_TID, 0), WaitOutcome::Running);
        t.end(4);
        t.end(2);
        assert_eq!(t.wait(MAIN_TID, 0), WaitOutcome::Ended(4));
        assert_eq!(t.wait(MAIN_TID, 0), WaitOutcome::Ended(2));
        assert_eq!(t.wait(MAIN_TID, 0), WaitOutcome::Running);
        t.end(3);
        assert_eq!(t.wait(MAIN_TID, 0), WaitOutcome::Ended(3));
        // Nothing left that could end: no endless wait.
        assert_eq!(t.wait(MAIN_TID, 0), WaitOutcome::Invalid);
    }

    #[test]
    fn wait_for_self_main_or_unknown_is_invalid() {
        let mut t = with_threads(&[2]);
        assert_eq!(t.wait(2, 2), WaitOutcome::Invalid);
        assert_eq!(t.wait(2, MAIN_TID), WaitOutcome::Invalid);
        assert_eq!(t.wait(MAIN_TID, 99), WaitOutcome::Invalid);
        // Thread 2 waiting for any: only thread 1 is left, which cannot be waited for.
        assert_eq!(t.wait(2, 0), WaitOutcome::Invalid);
    }

    #[test]
    fn ended_threads_are_bounded() {
        let mut t = ThreadTable::new();
        for tid in 2..(2 + MAX_ENDED_THREADS as u32 + 1) {
            t.start(tid, MEM);
            t.end(tid);
        }
        assert_eq!(t.wait(MAIN_TID, 2), WaitOutcome::Invalid);
        assert_eq!(t.wait(MAIN_TID, 3), WaitOutcome::Ended(3));
    }

    #[test]
    fn kill_flags_only_other_running_threads() {
        let mut t = with_threads(&[2]);
        assert!(!t.kill(2, 2));
        assert!(!t.kill(MAIN_TID, 7));
        assert!(t.kill(MAIN_TID, 2));
        assert!(t.is_killed(2));
        t.end(2);
        assert!(!t.is_killed(2));
    }

    #[test]
    fn vcpus_of_ended_threads_are_reused() {
        let mut t = ThreadTable::new();
        assert_eq!(t.take_vcpu().err(), Some(1));
        assert_eq!(t.take_vcpu().err(), Some(2));
        t.put_vcpu(Box::new(MockVcpu::new()));
        assert!(t.take_vcpu().is_ok());
        assert_eq!(t.take_vcpu().err(), Some(3));
    }

    #[test]
    fn suspensions_count_and_end_with_the_thread() {
        let mut t = with_threads(&[2]);
        assert_eq!(t.suspend(2, 2), ERROR_INVALID_THREADID, "not itself");
        assert_eq!(t.suspend(MAIN_TID, 9), ERROR_INVALID_THREADID);
        assert_eq!(t.resume(2), ERROR_NOT_FROZEN);
        assert_eq!(t.suspend(MAIN_TID, 2), NO_ERROR);
        assert_eq!(t.suspend(MAIN_TID, 2), NO_ERROR);
        assert_eq!(t.suspend_count(2), 2);
        assert_eq!(t.resume(2), NO_ERROR);
        assert_eq!(t.suspend_count(2), 1);
        t.end(2);
        assert_eq!(t.suspend_count(2), 0);
        assert_eq!(t.resume(2), ERROR_INVALID_THREADID);
    }

    #[test]
    fn kicks_reach_registered_running_threads() {
        let mut t = with_threads(&[2, 3]);
        let (a, b, c) = (MockVcpu::new(), MockVcpu::new(), MockVcpu::new());
        t.set_kicker(MAIN_TID, a.kicker());
        t.set_kicker(2, b.kicker());
        t.set_kicker(3, c.kicker());
        t.kick(2);
        t.kick(7);
        t.kick_all_but(3);
        let n = |v: &MockVcpu| v.kick.kicks.load(Ordering::SeqCst);
        assert_eq!((n(&a), n(&b), n(&c)), (1, 2, 0));
        t.end(2);
        t.kick_all_but(0);
        assert_eq!((n(&a), n(&b), n(&c)), (2, 2, 1));
    }

    #[test]
    fn wait_while_suspended_returns_on_resume_or_stop() {
        let threads = std::sync::Arc::new(Threads::default());
        threads.table.lock_or_recover().start(2, MEM);
        assert!(!threads.wait_while_suspended(2, || false), "not suspended");
        assert_eq!(threads.suspend(MAIN_TID, 2), NO_ERROR);
        assert!(threads.wait_while_suspended(2, || true), "gives up when told to");
        let other = std::sync::Arc::clone(&threads);
        let resumer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            other.resume(2)
        });
        let start = std::time::Instant::now();
        assert!(threads.wait_while_suspended(2, || false));
        assert!(start.elapsed() >= Duration::from_millis(20));
        assert_eq!(resumer.join().unwrap(), NO_ERROR);
    }

    #[test]
    fn wait_for_returns_when_a_thread_ends_elsewhere() {
        let threads = std::sync::Arc::new(Threads::default());
        threads.table.lock_or_recover().start(2, MEM);
        let other = std::sync::Arc::clone(&threads);
        let ender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            other.table.lock_or_recover().end(2);
            other.ended.notify_all();
        });
        assert_eq!(threads.wait_for(MAIN_TID, 2, false, || false), Some(WaitOutcome::Ended(2)));
        ender.join().unwrap();
    }

    #[test]
    fn crit_section_nests_and_ends_at_zero() {
        let mut c = CritSection::default();
        assert!(!c.blocks(MAIN_TID));
        assert_eq!(c.exit(MAIN_TID), ERROR_CRITSEC_UNDERFLOW);
        assert_eq!(c.enter(2), NO_ERROR);
        assert_eq!(c.enter(2), NO_ERROR);
        assert!(c.blocks(MAIN_TID));
        assert!(!c.blocks(2));
        assert_eq!(c.exit(MAIN_TID), ERROR_CRITSEC_UNDERFLOW);
        assert_eq!(c.exit(2), NO_ERROR);
        assert!(c.blocks(MAIN_TID), "still nested once");
        assert_eq!(c.exit(2), NO_ERROR);
        assert!(!c.blocks(MAIN_TID));
        assert_eq!((c.owner(), c.count()), (None, 0));
        assert_eq!(c.exit(2), ERROR_CRITSEC_UNDERFLOW);
    }

    #[test]
    fn crit_section_overflow_keeps_the_count() {
        let mut c = CritSection::default();
        for _ in 0..MAX_CRITSEC_NESTING {
            assert_eq!(c.enter(2), NO_ERROR);
        }
        assert_eq!(c.enter(2), ERROR_CRITSEC_OVERFLOW);
        assert_eq!(c.count(), MAX_CRITSEC_NESTING);
        assert_eq!(c.exit(2), NO_ERROR);
        assert_eq!(c.count(), MAX_CRITSEC_NESTING - 1);
    }

    #[test]
    fn crit_section_ends_with_its_thread() {
        let mut c = CritSection::default();
        c.enter(2);
        c.enter(2);
        assert!(!c.release(3));
        assert!(c.release(2));
        assert!(!c.blocks(MAIN_TID));
    }

    #[test]
    fn other_threads_wait_until_the_section_ends() {
        let threads = std::sync::Arc::new(Threads::default());
        assert!(threads.wait_crit_sec(MAIN_TID, || false), "no section: runs");
        assert_eq!(threads.enter_crit_sec(2, || false), Some(NO_ERROR));
        assert!(threads.wait_crit_sec(2, || false), "the owner runs");
        assert!(!threads.wait_crit_sec(MAIN_TID, || true), "gives up when told to");
        let other = std::sync::Arc::clone(&threads);
        let ender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            other.exit_crit_sec(2)
        });
        let start = std::time::Instant::now();
        assert!(threads.wait_crit_sec(MAIN_TID, || false));
        assert!(start.elapsed() >= Duration::from_millis(20), "it waited");
        assert_eq!(ender.join().unwrap(), NO_ERROR);
        // Entering waits as well, and a thread end releases the section.
        threads.enter_crit_sec(3, || false);
        assert_eq!(threads.enter_crit_sec(MAIN_TID, || true), None);
        threads.release_crit_sec(3);
        assert_eq!(threads.enter_crit_sec(MAIN_TID, || false), Some(NO_ERROR));
    }

    #[test]
    fn wait_for_no_wait_and_process_end() {
        let threads = Threads::default();
        threads.table.lock_or_recover().start(2, MEM);
        assert_eq!(threads.wait_for(MAIN_TID, 2, true, || false), Some(WaitOutcome::Running));
        assert_eq!(threads.wait_for(MAIN_TID, 2, false, || true), None);
        threads.wait_for_process_end(|| true);
    }
}
