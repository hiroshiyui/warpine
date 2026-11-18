// SPDX-License-Identifier: GPL-3.0-only
//
//! `DosAsyncTimer`, `DosStartTimer`, `DosStopTimer` (DOSCALLS 350, 351, 290).
//!
//! Behaviour from IBM's *Control Program Programming Reference*:
//! - `DosAsyncTimer(msec, hsem, phtimer)` posts the event semaphore `hsem`
//!   once, `msec` milliseconds later; `DosStartTimer` posts it every `msec`
//!   milliseconds until stopped; both return a timer handle in `*phtimer`;
//! - the interval is rounded up to the next system clock tick (whose length
//!   `DosQuerySysInfo(QSV_TIMER_INTERVAL)` reports in tenths of a ms);
//! - `hsem` must be a shared event semaphore (`ERROR_TS_SEMHANDLE`); no
//!   timer left is `ERROR_TS_NOTIMER`;
//! - `DosStopTimer(htimer)` stops either kind (`ERROR_TS_HANDLE` for an
//!   unknown handle); the semaphore's state afterwards is unknown.
//!
//! Each timer is one host thread that sleeps on its stop signal until the
//! next deadline (deadlines are `start + k × interval`, so a periodic timer
//! does not drift), then posts the semaphore like `DosPostEventSem` (post
//! counts add up when the program does not reset it). Timers end with the
//! process.
//!
//! Warpine's choices where the documents say nothing:
//! - an interval of 0 is one clock tick;
//! - a NULL `phtimer` is `ERROR_INVALID_PARAMETER`, checked before the
//!   semaphore;
//! - at most `MAX_DOS_TIMERS` timers run at once (`ERROR_TS_NOTIMER`
//!   beyond);
//! - a one-shot timer's handle is invalid (`ERROR_TS_HANDLE`) once it has
//!   posted; a timer whose semaphore was closed stops by itself;
//! - a semaphore is shared when it was created with `DC_SEM_SHARED` or a
//!   name.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_types::TimerStop;
use super::SharedState;
use log::debug;

/// Most timers that run at once.
pub const MAX_DOS_TIMERS: usize = 64;
/// First timer handle (`HTIMER`); handles are never 0.
const FIRST_HTIMER: u32 = 0x0001_0001;
/// Longest a timer thread sleeps before it looks at the process-exit flag.
const EXIT_POLL: Duration = Duration::from_millis(100);

/// `msec` rounded up to whole clock ticks (at least one).
pub fn rounded_interval(msec: u32) -> Duration {
    let ticks = msec.div_ceil(DOS_TIMER_TICK_MS).max(1) as u64;
    Duration::from_millis(ticks * DOS_TIMER_TICK_MS as u64)
}

/// The running timers of the process (`SharedState::dos_timers`).
pub struct DosTimerTable {
    timers: HashMap<u32, Arc<TimerStop>>,
    next_handle: u32,
}

impl Default for DosTimerTable {
    fn default() -> Self {
        DosTimerTable { timers: HashMap::new(), next_handle: FIRST_HTIMER }
    }
}

impl DosTimerTable {
    /// A handle for a new timer, or `ERROR_TS_NOTIMER`.
    fn insert(&mut self, stop: Arc<TimerStop>) -> Result<u32, u32> {
        if self.timers.len() >= MAX_DOS_TIMERS {
            return Err(ERROR_TS_NOTIMER);
        }
        let mut h = self.next_handle;
        while h == 0 || self.timers.contains_key(&h) {
            h = h.wrapping_add(1);
        }
        self.next_handle = h.wrapping_add(1);
        self.timers.insert(h, stop);
        Ok(h)
    }

    /// `DosStopTimer`.
    pub fn stop(&mut self, h: u32) -> u32 {
        match self.timers.remove(&h) {
            Some(stop) => { stop.stop(); NO_ERROR }
            None => ERROR_TS_HANDLE,
        }
    }

    /// Timer `h` ended by itself: forget it if it is still this timer.
    fn finished(&mut self, h: u32, stop: &Arc<TimerStop>) {
        if self.timers.get(&h).is_some_and(|s| Arc::ptr_eq(s, stop)) {
            self.timers.remove(&h);
        }
    }

    /// The process ends: stop every timer.
    pub fn stop_all(&mut self) {
        for (_, stop) in self.timers.drain() {
            stop.stop();
        }
    }

    pub fn len(&self) -> usize { self.timers.len() }
    pub fn is_empty(&self) -> bool { self.timers.is_empty() }
}

/// Post event semaphore `hsem`; false when it no longer exists.
fn post(shared: &SharedState, hsem: u32) -> bool {
    shared.sem_mgr.lock_or_recover().post_event(hsem) != ERROR_INVALID_HANDLE
}

/// Sleep until `deadline`, waking for the stop signal and to look at the
/// process-exit flag. True: the timer is to end.
fn sleep_until(shared: &SharedState, stop: &TimerStop, deadline: Instant) -> bool {
    loop {
        if shared.exit_requested.load(Ordering::Relaxed) {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return stop.is_stopped();
        }
        if stop.sleep((deadline - now).min(EXIT_POLL)) {
            return true;
        }
    }
}

/// The thread of timer `h`.
fn run_timer(shared: Arc<SharedState>, stop: Arc<TimerStop>, h: u32, hsem: u32, interval: Duration, periodic: bool) {
    let start = Instant::now();
    let mut fired: u32 = 0;
    loop {
        fired = fired.saturating_add(1);
        let deadline = start + interval.saturating_mul(fired);
        if sleep_until(&shared, &stop, deadline) {
            break;
        }
        if !post(&shared, hsem) {
            debug!("  timer {:#x}: semaphore {:#x} is gone, timer ends", h, hsem);
            break;
        }
        if !periodic {
            break;
        }
    }
    shared.dos_timers.lock_or_recover().finished(h, &stop);
}

impl super::Loader {
    /// DosAsyncTimer (ordinal 350) / DosStartTimer (351).
    pub fn dos_start_timer(&self, msec: u32, hsem: u32, phtimer: u32, periodic: bool) -> u32 {
        if phtimer == 0 || self.guest_read::<u32>(phtimer).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        if !self.shared.sem_mgr.lock_or_recover().is_shared_event(hsem) {
            return ERROR_TS_SEMHANDLE;
        }
        let stop = Arc::new(TimerStop::default());
        let h = {
            let mut table = self.shared.dos_timers.lock_or_recover();
            match table.insert(Arc::clone(&stop)) {
                Ok(h) => h,
                Err(e) => return e,
            }
        };
        let interval = rounded_interval(msec);
        let shared = Arc::clone(&self.shared);
        let thread_stop = Arc::clone(&stop);
        let spawned = std::thread::Builder::new()
            .name(format!("dos-timer-{h:#x}"))
            .spawn(move || run_timer(shared, thread_stop, h, hsem, interval, periodic));
        if spawned.is_err() {
            self.shared.dos_timers.lock_or_recover().finished(h, &stop);
            return ERROR_TS_NOTIMER;
        }
        debug!("  {}({} ms → {:?}, hsem={:#x}) → htimer {:#x}",
               if periodic { "DosStartTimer" } else { "DosAsyncTimer" }, msec, interval, hsem, h);
        self.guest_write::<u32>(phtimer, h);
        NO_ERROR
    }

    /// DosStopTimer (ordinal 290).
    pub fn dos_stop_timer(&self, htimer: u32) -> u32 {
        self.shared.dos_timers.lock_or_recover().stop(htimer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;

    fn scratch(loader: &Loader) -> u32 {
        loader.shared.mem_mgr.lock_or_recover().alloc(4096).unwrap()
    }

    fn shared_event(loader: &Loader) -> u32 {
        loader.shared.sem_mgr.lock_or_recover().create_event(None, DC_SEM_SHARED, false).unwrap()
    }

    fn post_count(loader: &Loader, hev: u32) -> u32 {
        let sem = loader.shared.sem_mgr.lock_or_recover().get_event(hev).unwrap();
        sem.0.lock_or_recover().post_count
    }

    #[test]
    fn intervals_round_up_to_clock_ticks() {
        let tick = DOS_TIMER_TICK_MS as u64;
        assert_eq!(rounded_interval(0), Duration::from_millis(tick));
        assert_eq!(rounded_interval(1), Duration::from_millis(tick));
        assert_eq!(rounded_interval(DOS_TIMER_TICK_MS), Duration::from_millis(tick));
        assert_eq!(rounded_interval(DOS_TIMER_TICK_MS + 1), Duration::from_millis(2 * tick));
        assert_eq!(rounded_interval(u32::MAX), Duration::from_millis(u32::MAX.div_ceil(DOS_TIMER_TICK_MS) as u64 * tick));
    }

    #[test]
    fn qsv_timer_interval_is_the_tick_in_tenths_of_a_millisecond() {
        let loader = Loader::new_mock();
        let buf = scratch(&loader);
        assert_eq!(loader.dos_query_sys_info(QSV_TIMER_INTERVAL, QSV_TIMER_INTERVAL, buf, 4), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(buf), Some(DOS_TIMER_TICK_MS * 10));
    }

    #[test]
    fn async_timer_posts_once() {
        let loader = Loader::new_mock();
        let buf = scratch(&loader);
        let hev = shared_event(&loader);
        assert_eq!(loader.dos_start_timer(10, hev, buf, false), NO_ERROR);
        let h = loader.guest_read::<u32>(buf).unwrap();
        assert_ne!(h, 0);
        std::thread::sleep(Duration::from_millis(DOS_TIMER_TICK_MS as u64 * 4));
        assert_eq!(post_count(&loader, hev), 1);
        assert!(loader.shared.dos_timers.lock_or_recover().is_empty(), "done");
        assert_eq!(loader.dos_stop_timer(h), ERROR_TS_HANDLE);
    }

    #[test]
    fn start_timer_posts_until_stopped() {
        let loader = Loader::new_mock();
        let buf = scratch(&loader);
        let hev = shared_event(&loader);
        assert_eq!(loader.dos_start_timer(1, hev, buf, true), NO_ERROR);
        let h = loader.guest_read::<u32>(buf).unwrap();
        std::thread::sleep(Duration::from_millis(DOS_TIMER_TICK_MS as u64 * 5));
        assert_eq!(loader.dos_stop_timer(h), NO_ERROR);
        let posts = post_count(&loader, hev);
        assert!(posts >= 2, "posted {posts} times");
        std::thread::sleep(Duration::from_millis(DOS_TIMER_TICK_MS as u64 * 3));
        assert_eq!(post_count(&loader, hev), posts, "no post after the stop");
        assert_eq!(loader.dos_stop_timer(h), ERROR_TS_HANDLE);
    }

    #[test]
    fn stopping_an_async_timer_before_it_fires() {
        let loader = Loader::new_mock();
        let buf = scratch(&loader);
        let hev = shared_event(&loader);
        assert_eq!(loader.dos_start_timer(5000, hev, buf, false), NO_ERROR);
        let h = loader.guest_read::<u32>(buf).unwrap();
        assert_eq!(loader.dos_stop_timer(h), NO_ERROR);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(post_count(&loader, hev), 0);
    }

    #[test]
    fn semaphore_must_be_a_shared_event() {
        let loader = Loader::new_mock();
        let buf = scratch(&loader);
        let private = loader.shared.sem_mgr.lock_or_recover().create_event(None, 0, false).unwrap();
        let named = loader.shared.sem_mgr.lock_or_recover()
            .create_event(Some("\\SEM32\\TIMER"), 0, false).unwrap();
        let mutex = loader.shared.sem_mgr.lock_or_recover().create_mutex(None, DC_SEM_SHARED, None).unwrap();
        assert_eq!(loader.dos_start_timer(10, private, buf, false), ERROR_TS_SEMHANDLE);
        assert_eq!(loader.dos_start_timer(10, mutex, buf, true), ERROR_TS_SEMHANDLE);
        assert_eq!(loader.dos_start_timer(10, 0xBAD, buf, true), ERROR_TS_SEMHANDLE);
        assert_eq!(loader.dos_start_timer(10, named, 0, true), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.dos_start_timer(10, named, buf, true), NO_ERROR);
        loader.shared.dos_timers.lock_or_recover().stop_all();
    }

    #[test]
    fn timers_are_limited_and_end_with_the_process() {
        let loader = Loader::new_mock();
        let buf = scratch(&loader);
        let hev = shared_event(&loader);
        for _ in 0..MAX_DOS_TIMERS {
            assert_eq!(loader.dos_start_timer(60_000, hev, buf, true), NO_ERROR);
        }
        assert_eq!(loader.dos_start_timer(60_000, hev, buf, true), ERROR_TS_NOTIMER);
        loader.request_process_exit(0);
        assert!(loader.shared.dos_timers.lock_or_recover().is_empty());
    }

    #[test]
    fn a_closed_semaphore_ends_the_timer() {
        let loader = Loader::new_mock();
        let buf = scratch(&loader);
        let hev = shared_event(&loader);
        assert_eq!(loader.dos_start_timer(1, hev, buf, true), NO_ERROR);
        assert_eq!(loader.shared.sem_mgr.lock_or_recover().close(super::super::ipc::SemKind::Event, hev), NO_ERROR);
        std::thread::sleep(Duration::from_millis(DOS_TIMER_TICK_MS as u64 * 3));
        assert!(loader.shared.dos_timers.lock_or_recover().is_empty());
    }

    #[test]
    fn handles_are_unique_and_never_zero() {
        let mut t = DosTimerTable { timers: HashMap::new(), next_handle: u32::MAX };
        let a = t.insert(Arc::default()).unwrap();
        let b = t.insert(Arc::default()).unwrap();
        assert_eq!(a, u32::MAX);
        assert_eq!(b, 1, "0 is skipped");
        assert_eq!(t.len(), 2);
        assert_eq!(t.stop(a), NO_ERROR);
        assert_eq!(t.stop(a), ERROR_TS_HANDLE);
    }
}
