// SPDX-License-Identifier: GPL-3.0-only
//
//! `DosEnterCritSec` / `DosExitCritSec` (DOSCALLS 232 / 233).
//!
//! Behaviour from IBM's *Control Program Programming Reference*:
//! - while a thread is in a critical section the process's other threads do
//!   not run; the section nests: `DosEnterCritSec` adds one to a count,
//!   `DosExitCritSec` takes one away, and normal dispatching resumes at 0;
//! - the count is a word: entering at its highest value does nothing and
//!   returns `ERROR_CRITSEC_OVERFLOW`; leaving at 0 does nothing and returns
//!   `ERROR_CRITSEC_UNDERFLOW`.
//!
//! How Warpine stops the other threads: each runs on its own vCPU, which can
//! only be held when it leaves the guest (an API call, a fault, any VM
//! exit). Every vCPU loop waits at the top of each pass, before it enters
//! the guest again, while another thread owns the section
//! (`Threads::wait_crit_sec`), and an API call that arrives from a thread
//! already running in guest code when the section began waits before it is
//! carried out. Entering a section kicks the other threads' vCPUs
//! (`VcpuKick`), so a thread spinning in guest code without calling the
//! system leaves the guest at once and is held there too.
//!
//! Warpine's choices where the documents say nothing:
//! - a thread that ends (`DosExit(EXIT_THREAD)`, returning, `DosKillThread`)
//!   inside a critical section ends the section;
//! - a waiting thread gives up when the process ends (the exit list runs on
//!   another thread) or it is killed, and then ends;
//! - `ERROR_INVALID_THREADID` (a call from a signal or exception handler)
//!   is not reported: exception handlers (signal exceptions included) may
//!   enter and leave the section like other code.

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::threads::tid_of_vcpu;
use log::debug;

impl super::Loader {
    /// DosEnterCritSec (ordinal 232).
    pub fn dos_enter_crit_sec(&self, vcpu_id: u32) -> u32 {
        let tid = tid_of_vcpu(vcpu_id);
        match self.shared.threads.enter_crit_sec(tid, || self.crit_sec_gives_up(tid)) {
            Some(rc) => {
                debug!("  [VCPU {}] DosEnterCritSec -> {} (count {})", vcpu_id, rc,
                       self.shared.threads.crit.lock_or_recover().count());
                rc
            }
            // The thread ends at its next pass through the vCPU loop.
            None => ERROR_INVALID_FUNCTION,
        }
    }

    /// DosExitCritSec (ordinal 233).
    pub fn dos_exit_crit_sec(&self, vcpu_id: u32) -> u32 {
        self.shared.threads.exit_crit_sec(tid_of_vcpu(vcpu_id))
    }

    /// Hold thread `vcpu_id` while another thread owns the critical
    /// section. False: the thread is to end (process ending, killed).
    pub(crate) fn wait_crit_sec(&self, vcpu_id: u32) -> bool {
        let tid = tid_of_vcpu(vcpu_id);
        self.shared.threads.wait_crit_sec(tid, || self.crit_sec_gives_up(tid))
    }

    /// Whether thread `tid`, blocked by the critical section, is to end.
    fn crit_sec_gives_up(&self, tid: u32) -> bool {
        self.shutting_down() || self.shared.threads.table.lock_or_recover().is_killed(tid)
    }
}

#[cfg(test)]
mod tests {
    use super::super::Loader;
    use super::super::constants::*;
    use super::super::threads::{MAIN_VCPU_ID, MAIN_TID, ThreadMemory};
    use super::super::mutex_ext::MutexExt;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    const MEM: ThreadMemory = ThreadMemory { stack: 0x1000, tib: 0x2000 };

    #[test]
    fn enter_and_exit_nest() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_exit_crit_sec(MAIN_VCPU_ID), ERROR_CRITSEC_UNDERFLOW);
        assert_eq!(loader.dos_enter_crit_sec(MAIN_VCPU_ID), NO_ERROR);
        assert_eq!(loader.dos_enter_crit_sec(MAIN_VCPU_ID), NO_ERROR);
        assert_eq!(loader.shared.threads.crit.lock_or_recover().owner(), Some(MAIN_TID));
        assert_eq!(loader.dos_exit_crit_sec(MAIN_VCPU_ID), NO_ERROR);
        assert_eq!(loader.dos_exit_crit_sec(MAIN_VCPU_ID), NO_ERROR);
        assert_eq!(loader.dos_exit_crit_sec(MAIN_VCPU_ID), ERROR_CRITSEC_UNDERFLOW);
        assert!(loader.wait_crit_sec(2));
    }

    #[test]
    fn other_threads_wait_and_a_kill_or_process_end_releases_them() {
        let loader = Loader::new_mock();
        loader.shared.threads.table.lock_or_recover().start(2, MEM);
        assert_eq!(loader.dos_enter_crit_sec(MAIN_VCPU_ID), NO_ERROR);
        // Thread 2 cannot exit a section it does not own.
        assert_eq!(loader.dos_exit_crit_sec(2), ERROR_CRITSEC_UNDERFLOW);
        // A killed thread stops waiting (and ends in its vCPU loop).
        loader.shared.threads.table.lock_or_recover().kill(MAIN_TID, 2);
        assert!(!loader.wait_crit_sec(2));
        assert_eq!(loader.dos_enter_crit_sec(2), ERROR_INVALID_FUNCTION);
        // So does every thread once the process ends.
        loader.shared.exit_requested.store(true, Ordering::Relaxed);
        assert!(!loader.wait_crit_sec(3));
    }

    #[test]
    fn a_waiting_thread_runs_when_the_owner_leaves() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_enter_crit_sec(MAIN_VCPU_ID), NO_ERROR);
        std::thread::scope(|s| {
            let waiter = s.spawn(|| {
                let start = std::time::Instant::now();
                assert!(loader.wait_crit_sec(2));
                start.elapsed()
            });
            std::thread::sleep(Duration::from_millis(40));
            assert_eq!(loader.dos_exit_crit_sec(MAIN_VCPU_ID), NO_ERROR);
            assert!(waiter.join().unwrap() >= Duration::from_millis(30));
        });
    }
}
