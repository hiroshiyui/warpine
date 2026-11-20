// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Host (Linux) signals Warpine uses, and nothing else of them.
//!
//! - **The vCPU kick signal** (`kick_signal()`, a real-time signal): sent to
//!   the host thread of a vCPU that runs guest code, so that `KVM_RUN`
//!   returns `EINTR` (Linux KVM API, `immediate_exit`: "In the common
//!   scenario where a signal is used to 'kick' a VCPU out of KVM_RUN …").
//!   Its handler does nothing; `SA_RESTART` restarts the system calls it
//!   might interrupt on other threads.
//! - **Requests from other processes** (`HostRequest`): the OS/2 signal
//!   exceptions `XCPT_SIGNAL_INTR`, `XCPT_SIGNAL_BREAK` and
//!   `XCPT_SIGNAL_KILLPROC` (`DosSendSignalException`, `DosKillProcess`
//!   with `DKP_PROCESS`) and "end this process and its descendants"
//!   (`DosKillProcess` with `DKP_PROCESSTREE`) travel as four other
//!   real-time signals. `SIGINT` (Ctrl+C at a terminal that is not in raw
//!   mode, `kill -INT`) arrives as `XCPT_SIGNAL_INTR`, `SIGQUIT` (Ctrl+\ at
//!   such a terminal, which has no Break key; ADR-0093) as
//!   `XCPT_SIGNAL_BREAK`, and `SIGTERM` as `XCPT_SIGNAL_KILLPROC`, so a host that ends a Warpine process the
//!   polite way runs its exit list. The handler only stores the request in
//!   an atomic and writes one byte to a pipe (both async-signal-safe); a
//!   listener thread reads the pipe and hands the request to the process
//!   (`signals.rs`).
//! - **Child processes by pidfd** (`ProcessHandle`): `DosKillProcess`
//!   signals a child through a pidfd taken while the child is known not to
//!   have been reaped, so the signal (and the `SIGKILL` after the grace
//!   period) can never reach another process that reused the pid.
//!
//! All `unsafe` here is a libc call whose arguments are plain values or
//! point at memory owned by this module; each block says why it is sound.

use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Once;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::time::Duration;

use super::constants::{XCPT_SIGNAL_BREAK, XCPT_SIGNAL_INTR, XCPT_SIGNAL_KILLPROC};

/// Offsets from `SIGRTMIN` of Warpine's signals (glibc reserves the ones
/// below its `SIGRTMIN`).
const KICK_OFFSET: i32 = 2;
const INTR_OFFSET: i32 = 3;
const BREAK_OFFSET: i32 = 4;
const KILLPROC_OFFSET: i32 = 5;
const KILL_TREE_OFFSET: i32 = 6;

/// What another process (or the host) asks of this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRequest {
    /// An OS/2 signal exception (`XCPT_SIGNAL_*`).
    Signal(u32),
    /// `DosKillProcess(DKP_PROCESSTREE)`: end this process and its
    /// descendants.
    KillTree,
}

/// Every request, in the order the listener hands them on.
const REQUESTS: [HostRequest; 4] = [
    HostRequest::Signal(XCPT_SIGNAL_INTR),
    HostRequest::Signal(XCPT_SIGNAL_KILLPROC),
    HostRequest::Signal(XCPT_SIGNAL_BREAK),
    HostRequest::KillTree,
];

/// Bit of `request` in the pending set (signal numbers are 1 … 4).
const fn request_bit(request: HostRequest) -> u32 {
    match request {
        HostRequest::Signal(s) => 1 << (s & 0xF),
        HostRequest::KillTree => 1 << 16,
    }
}

/// The host signal that kicks a vCPU out of `KVM_RUN`.
pub fn kick_signal() -> i32 {
    libc::SIGRTMIN() + KICK_OFFSET
}

/// The real-time signal that carries `request` to another Warpine process.
pub fn request_signal(request: HostRequest) -> Option<i32> {
    let offset = match request {
        HostRequest::Signal(XCPT_SIGNAL_INTR) => INTR_OFFSET,
        HostRequest::Signal(XCPT_SIGNAL_BREAK) => BREAK_OFFSET,
        HostRequest::Signal(XCPT_SIGNAL_KILLPROC) => KILLPROC_OFFSET,
        HostRequest::KillTree => KILL_TREE_OFFSET,
        HostRequest::Signal(_) => return None,
    };
    Some(libc::SIGRTMIN() + offset)
}

/// The host signal that carries OS/2 signal exception `os2_signal` to
/// another Warpine process, if there is one for it.
pub fn host_signal_for(os2_signal: u32) -> Option<i32> {
    request_signal(HostRequest::Signal(os2_signal))
}

/// The request a host signal carries: Warpine's real-time signals, and
/// `SIGINT` / `SIGQUIT` / `SIGTERM` from the host (async-signal-safe:
/// arithmetic only).
pub fn request_for(host_signal: i32) -> Option<HostRequest> {
    match host_signal {
        libc::SIGINT => return Some(HostRequest::Signal(XCPT_SIGNAL_INTR)),
        libc::SIGQUIT => return Some(HostRequest::Signal(XCPT_SIGNAL_BREAK)),
        libc::SIGTERM => return Some(HostRequest::Signal(XCPT_SIGNAL_KILLPROC)),
        _ => {}
    }
    REQUESTS.into_iter().find(|&r| request_signal(r) == Some(host_signal))
}

/// No-op handler of the kick signal: its only purpose is to be delivered.
extern "C" fn on_kick(_sig: libc::c_int) {}

/// Install `handler` for `sig` with `SA_RESTART`.
fn install(sig: i32, handler: extern "C" fn(libc::c_int)) -> bool {
    // SAFETY: `sigaction` is a plain value struct; all-zero is a valid
    // initial state (empty mask, no flags) before the fields are set.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler as *const () as libc::sighandler_t;
    action.sa_flags = libc::SA_RESTART;
    // SAFETY: `action.sa_mask` is owned and initialised above;
    // `sigemptyset` only writes it.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    // SAFETY: `action` is a fully initialised `sigaction` whose handler is
    // an `extern "C"` function that only does async-signal-safe work;
    // passing null for the old action is allowed.
    unsafe { libc::sigaction(sig, &action, std::ptr::null_mut()) == 0 }
}

/// Install the kick signal's handler (once per process).
pub fn install_kick_handler() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if !install(kick_signal(), on_kick) {
            log::warn!("cannot install the vCPU kick signal handler: {}", std::io::Error::last_os_error());
        }
    });
}

thread_local! {
    static CACHED_TID: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// The Linux thread id of the calling thread (cached per thread).
pub fn current_tid() -> i32 {
    CACHED_TID.with(|c| {
        if c.get() == 0 {
            // `gettid` cannot fail and has no preconditions.
            // SAFETY: a plain system call without arguments.
            c.set(unsafe { libc::gettid() });
        }
        c.get()
    })
}

/// Send the kick signal to thread `tid` of this process. A thread that is
/// gone (or a reused id) is harmless: `tgkill` only reaches this process's
/// threads, and the handler does nothing.
pub fn kick_thread(tid: i32) {
    if tid <= 0 { return; }
    // SAFETY: plain values; `tgkill` restricts the signal to this process.
    unsafe { libc::tgkill(std::process::id() as i32, tid, kick_signal()); }
}

/// Send OS/2 signal exception `os2_signal` to host process `host_pid` (a
/// child Warpine process). False: no such process, or no host signal for it.
pub fn send_to_process(host_pid: u32, os2_signal: u32) -> bool {
    let Some(sig) = host_signal_for(os2_signal) else { return false };
    let Ok(pid) = i32::try_from(host_pid) else { return false };
    if pid <= 0 { return false; }
    // SAFETY: plain values; `pid` is positive, so exactly one process.
    unsafe { libc::kill(pid, sig) == 0 }
}

/// Send OS/2 signal exception `os2_signal` to the host process `pid`
/// started at `start` (a session record, `session_threads.rs`): through a
/// pidfd, after checking that the process holding the pid is still that
/// one, so a reused pid is never signalled. False when it is not there.
pub fn send_to_identity(pid: u32, start: u64, os2_signal: u32) -> bool {
    let Some(sig) = host_signal_for(os2_signal) else { return false };
    let Some(handle) = ProcessHandle::open(pid) else { return false };
    // The pidfd holds the process it was opened on; the start time says
    // it is the recorded one.
    if super::session_threads::proc_start_time(pid) != Some(start) { return false; }
    handle.send(sig)
}

/// A child process held by a pidfd (Linux 5.3+): what is sent through it
/// reaches exactly that process, never one that reused its pid.
#[derive(Debug)]
pub struct ProcessHandle(OwnedFd);

impl ProcessHandle {
    /// Take a pidfd of process `host_pid`. The caller must know the process
    /// has not been reaped (a child not yet waited for), so the pid is its.
    pub fn open(host_pid: u32) -> Option<Self> {
        let pid = libc::pid_t::try_from(host_pid).ok().filter(|&p| p > 0)?;
        // SAFETY: `pidfd_open(pid, 0)` takes plain values and returns a new
        // descriptor or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        let fd = i32::try_from(fd).ok().filter(|&fd| fd >= 0)?;
        // SAFETY: `fd` was just returned by the kernel and is owned by no
        // one else.
        Some(ProcessHandle(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Send host signal `sig` to the process. False once it has ended.
    pub fn send(&self, sig: i32) -> bool {
        // SAFETY: plain values; the pidfd is open; no `siginfo` (null).
        unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, self.0.as_raw_fd(), sig,
                          std::ptr::null::<libc::siginfo_t>(), 0) == 0
        }
    }

    /// Wait up to `timeout` for the process to end; true when it has.
    pub fn wait_exit(&self, timeout: Duration) -> bool {
        let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let mut p = libc::pollfd { fd: self.0.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let wait = i32::try_from(left.as_millis()).unwrap_or(ms);
            // SAFETY: `p` is one valid pollfd for the duration of the call.
            let n = unsafe { libc::poll(&mut p, 1, wait) };
            if n > 0 { return true; }
            if n == 0 { return false; }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return false;
            }
        }
    }
}

/// Requests received but not yet read by the listener (`request_bit`).
static RECEIVED: AtomicU32 = AtomicU32::new(0);
/// Write end of the listener's pipe (-1: none).
static WAKE_FD: AtomicI32 = AtomicI32::new(-1);

/// Handler of the request signals: async-signal-safe only (arithmetic,
/// atomic store, `write`).
extern "C" fn on_request(sig: libc::c_int) {
    if let Some(r) = request_for(sig) {
        RECEIVED.fetch_or(request_bit(r), Ordering::SeqCst);
        let fd = WAKE_FD.load(Ordering::SeqCst);
        if fd >= 0 {
            let byte = 1u8;
            // SAFETY: `byte` lives on this stack frame for the call; `write`
            // is async-signal-safe; a full pipe (non-blocking) just fails,
            // and the listener reads `RECEIVED` anyway.
            unsafe { libc::write(fd, (&byte as *const u8).cast(), 1); }
        }
    }
}

/// The host signals the listener takes.
fn request_host_signals() -> Vec<i32> {
    let mut sigs: Vec<i32> = REQUESTS.into_iter().filter_map(request_signal).collect();
    sigs.extend([libc::SIGINT, libc::SIGQUIT, libc::SIGTERM]);
    sigs
}

/// Start listening for requests from other processes and the host (once
/// per process): `deliver(request)` runs on a listener thread for each.
pub fn listen(deliver: impl Fn(HostRequest) + Send + 'static) {
    static ONCE: Once = Once::new();
    let mut deliver = Some(deliver);
    ONCE.call_once(|| {
        let Some(deliver) = deliver.take() else { return };
        let Ok((mut reader, writer)) = UnixStream::pair() else {
            log::warn!("signal exceptions: no pipe; other processes cannot signal this one");
            return;
        };
        if writer.set_nonblocking(true).is_err() { return; }
        // The write end stays open for the life of the process.
        WAKE_FD.store(writer.into_raw_fd(), Ordering::SeqCst);
        let spawned = std::thread::Builder::new().name("os2-signals".into()).spawn(move || {
            let mut buf = [0u8; 64];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => return,
                    Err(e) if e.kind() != std::io::ErrorKind::Interrupted => return,
                    _ => {}
                }
                let bits = RECEIVED.swap(0, Ordering::SeqCst);
                for r in REQUESTS {
                    if bits & request_bit(r) != 0 { deliver(r); }
                }
            }
        });
        if spawned.is_err() { return; }
        for sig in request_host_signals() {
            if !install(sig, on_request) {
                log::warn!("cannot install the handler of signal {}", sig);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_numbers_are_distinct_real_time_signals() {
        let kick = kick_signal();
        let mut sigs: Vec<i32> = REQUESTS.into_iter().map(|r| request_signal(r).unwrap()).collect();
        sigs.push(kick);
        for (i, a) in sigs.iter().enumerate() {
            assert!(*a >= libc::SIGRTMIN() && *a <= libc::SIGRTMAX());
            assert!(sigs[i + 1..].iter().all(|b| b != a), "{sigs:?}");
        }
        for r in REQUESTS {
            assert_eq!(request_for(request_signal(r).unwrap()), Some(r));
        }
        assert_eq!(request_for(kick), None);
        assert_eq!(request_for(libc::SIGINT), Some(HostRequest::Signal(XCPT_SIGNAL_INTR)));
        assert_eq!(request_for(libc::SIGQUIT), Some(HostRequest::Signal(XCPT_SIGNAL_BREAK)));
        assert_eq!(request_for(libc::SIGTERM), Some(HostRequest::Signal(XCPT_SIGNAL_KILLPROC)));
        assert_eq!(request_for(libc::SIGHUP), None);
        assert_eq!(host_signal_for(2), None);
        let bits: Vec<u32> = REQUESTS.into_iter().map(request_bit).collect();
        assert!(bits.iter().enumerate().all(|(i, b)| bits[i + 1..].iter().all(|c| c != b)));
    }

    #[test]
    fn kicking_a_thread_interrupts_a_blocking_call_only_once_handled() {
        install_kick_handler();
        // A kick to a sleeping thread must not kill the process (the
        // handler is installed) and must not cut the sleep short for good.
        let (tx, rx) = std::sync::mpsc::channel();
        let sleeper = std::thread::spawn(move || {
            tx.send(current_tid()).unwrap();
            let start = std::time::Instant::now();
            std::thread::sleep(std::time::Duration::from_millis(50));
            start.elapsed()
        });
        let tid = rx.recv().unwrap();
        kick_thread(tid);
        assert!(sleeper.join().unwrap() >= std::time::Duration::from_millis(50));
        assert_eq!(current_tid(), current_tid());
        kick_thread(0); // ignored
    }

    #[test]
    fn sending_to_no_process_fails() {
        assert!(!send_to_process(0, XCPT_SIGNAL_INTR));
        assert!(!send_to_process(u32::MAX, XCPT_SIGNAL_INTR));
        assert!(!send_to_process(std::process::id(), 2));
        assert!(ProcessHandle::open(0).is_none());
        assert!(ProcessHandle::open(u32::MAX).is_none());
    }

    #[test]
    fn a_pidfd_reaches_only_its_child_and_sees_it_end() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let h = ProcessHandle::open(child.id()).expect("pidfd_open (Linux 5.3+)");
        assert!(!h.wait_exit(Duration::from_millis(20)), "still running");
        assert!(h.send(libc::SIGKILL));
        assert!(h.wait_exit(Duration::from_secs(5)), "ended");
        child.wait().unwrap();
        // Reaped: nothing reaches a process that may reuse the pid.
        assert!(!h.send(libc::SIGKILL));
        assert!(h.wait_exit(Duration::from_millis(1)));
    }
}
