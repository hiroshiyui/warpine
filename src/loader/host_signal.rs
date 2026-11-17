// SPDX-License-Identifier: GPL-3.0-only
//
//! Host (Linux) signals Warpine uses, and nothing else of them.
//!
//! - **The vCPU kick signal** (`kick_signal()`, a real-time signal): sent to
//!   the host thread of a vCPU that runs guest code, so that `KVM_RUN`
//!   returns `EINTR` (Linux KVM API, `immediate_exit`: "In the common
//!   scenario where a signal is used to 'kick' a VCPU out of KVM_RUN …").
//!   Its handler does nothing; `SA_RESTART` restarts the system calls it
//!   might interrupt on other threads.
//! - **OS/2 signal exceptions between Warpine processes**
//!   (`DosSendSignalException` to a child process): `XCPT_SIGNAL_INTR` and
//!   `XCPT_SIGNAL_BREAK` travel as two other real-time signals. Their
//!   handler only stores the signal in an atomic and writes one byte to a
//!   pipe (both async-signal-safe); a listener thread reads the pipe and
//!   hands the signal to the process (`signals.rs`).
//!
//! All `unsafe` here is a libc call whose arguments are plain values or
//! point at memory owned by this module; each block says why it is sound.

use std::io::Read;
use std::os::fd::IntoRawFd;
use std::os::unix::net::UnixStream;
use std::sync::Once;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use super::constants::{XCPT_SIGNAL_BREAK, XCPT_SIGNAL_INTR};

/// Offsets from `SIGRTMIN` of Warpine's signals (glibc reserves the ones
/// below its `SIGRTMIN`).
const KICK_OFFSET: i32 = 2;
const INTR_OFFSET: i32 = 3;
const BREAK_OFFSET: i32 = 4;

/// The host signal that kicks a vCPU out of `KVM_RUN`.
pub fn kick_signal() -> i32 {
    libc::SIGRTMIN() + KICK_OFFSET
}

/// The host signal that carries OS/2 signal exception `os2_signal` to
/// another Warpine process, if there is one for it.
pub fn host_signal_for(os2_signal: u32) -> Option<i32> {
    match os2_signal {
        XCPT_SIGNAL_INTR => Some(libc::SIGRTMIN() + INTR_OFFSET),
        XCPT_SIGNAL_BREAK => Some(libc::SIGRTMIN() + BREAK_OFFSET),
        _ => None,
    }
}

/// The OS/2 signal exception a host signal carries (inverse of `host_signal_for`).
pub fn os2_signal_for(host_signal: i32) -> Option<u32> {
    [XCPT_SIGNAL_INTR, XCPT_SIGNAL_BREAK].into_iter()
        .find(|&s| host_signal_for(s) == Some(host_signal))
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

/// Signals received but not yet read by the listener (bit = OS/2 number).
static RECEIVED: AtomicU32 = AtomicU32::new(0);
/// Write end of the listener's pipe (-1: none).
static WAKE_FD: AtomicI32 = AtomicI32::new(-1);

/// Handler of the OS/2-signal host signals: async-signal-safe only (atomic
/// store, `write`).
extern "C" fn on_os2_signal(sig: libc::c_int) {
    if let Some(s) = os2_signal_for(sig) {
        RECEIVED.fetch_or(1 << s, Ordering::SeqCst);
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

/// Start listening for OS/2 signal exceptions from other Warpine processes
/// (once per process): `deliver(signal)` runs on a listener thread for each.
pub fn listen(deliver: impl Fn(u32) + Send + 'static) {
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
                for s in [XCPT_SIGNAL_INTR, XCPT_SIGNAL_BREAK] {
                    if bits & (1 << s) != 0 { deliver(s); }
                }
            }
        });
        if spawned.is_err() { return; }
        for s in [XCPT_SIGNAL_INTR, XCPT_SIGNAL_BREAK] {
            if let Some(sig) = host_signal_for(s)
                && !install(sig, on_os2_signal) {
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
        let intr = host_signal_for(XCPT_SIGNAL_INTR).unwrap();
        let brk = host_signal_for(XCPT_SIGNAL_BREAK).unwrap();
        assert!(kick >= libc::SIGRTMIN() && brk <= libc::SIGRTMAX());
        assert!(kick != intr && intr != brk && kick != brk);
        assert_eq!(os2_signal_for(intr), Some(XCPT_SIGNAL_INTR));
        assert_eq!(os2_signal_for(brk), Some(XCPT_SIGNAL_BREAK));
        assert_eq!(os2_signal_for(kick), None);
        assert_eq!(host_signal_for(super::super::constants::XCPT_SIGNAL_KILLPROC), None);
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
        assert!(!send_to_process(std::process::id(), super::super::constants::XCPT_SIGNAL_KILLPROC));
    }
}
