// SPDX-License-Identifier: GPL-3.0-or-later
//
//! A child's whole exit code for its parent (ADR-0090).
//!
//! Every OS/2 process is a host process of its own (ADR-0020), and a host
//! process's exit status carries only 8 bits, while `DosExit` takes a ULONG
//! and `RESULTCODES.codeResult` returns it. So a parent starting a child
//! (`admit_child`) makes a pipe, passes its write end to the child
//! (`WARPINE_EXIT_FD=<fd>:<the parent's host pid>`), and keeps the read
//! end, by the child's host pid. The child adopts the write end at its
//! start ([`adopt_from_env`]) and writes its exit code there, four bytes,
//! just before it ends ([`report`]). Reaping the child, the parent reads
//! it ([`super::SharedState::child_exit_code`]).
//!
//! The report is used only when its low 8 bits agree with the host exit
//! status; a child that wrote nothing (killed, or not a Warpine process)
//! gives the host status. The descriptor in the environment is believed
//! only by a direct child of the host process named there, and only when
//! it is a pipe opened for writing; this process makes it close-on-exec
//! at once, so nothing it starts later inherits it.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::ExitStatus;
use std::sync::{Mutex, OnceLock};

/// The child's end, and its parent.
pub const EXIT_FD_ENV: &str = "WARPINE_EXIT_FD";
/// Bytes of a report: the exit code, little-endian.
const REPORT_LEN: usize = 4;

/// This process's write end, when its parent gave one.
static REPORT: OnceLock<Mutex<Option<File>>> = OnceLock::new();

/// A pipe for one child: the parent keeps `read`, the child gets `write`.
#[derive(Debug)]
pub struct ExitPipe {
    pub read: File,
    write: OwnedFd,
}

impl ExitPipe {
    /// A new pipe, both ends close-on-exec.
    pub fn new() -> std::io::Result<ExitPipe> {
        let mut fds = [0i32; 2];
        // SAFETY: pipe2 writes two descriptors into `fds`, an array of two.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: both descriptors were just made and are owned by no one else.
        let (read, write) = unsafe { (File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok(ExitPipe { read, write })
    }

    /// Give the write end to the child `cmd` starts.
    pub fn pass_to(&self, cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        let fd = self.write.as_raw_fd();
        cmd.env(EXIT_FD_ENV, format!("{fd}:{}", std::process::id()));
        // SAFETY: the hook runs in the forked child before `exec` and only
        // calls fcntl (async-signal-safe) on a descriptor this pipe keeps open.
        unsafe {
            cmd.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    /// The child started (or not): the parent's copy of the write end
    /// closes, so the read end sees the end once the child's goes.
    pub fn into_read(self) -> File {
        drop(self.write);
        self.read
    }
}

/// Parse `<fd>:<parent pid>`.
fn parse_env(value: &str) -> Option<(i32, u32)> {
    let (fd, parent) = value.split_once(':')?;
    let digits = |s: &str| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(fd) || !digits(parent) { return None; }
    let fd: i32 = fd.parse().ok()?;
    let parent: u32 = parent.parse().ok()?;
    (fd > 2 && parent > 0).then_some((fd, parent))
}

/// Whether `fd` is a pipe this process may write.
fn is_write_pipe(fd: i32) -> bool {
    // SAFETY: fstat fills a zeroed stat; fcntl(F_GETFL) reads flags only.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 || st.st_mode & libc::S_IFMT != libc::S_IFIFO {
            return false;
        }
        let fl = libc::fcntl(fd, libc::F_GETFL);
        fl >= 0 && fl & libc::O_ACCMODE == libc::O_WRONLY
    }
}

/// Take the write end the parent passed, if it is believable (see the
/// module documentation). Called once at start.
pub fn adopt_from_env() {
    let adopted = std::env::var(EXIT_FD_ENV).ok()
        .and_then(|v| parse_env(&v))
        // SAFETY: getppid takes no arguments.
        .filter(|&(_, parent)| parent == unsafe { libc::getppid() } as u32)
        .filter(|&(fd, _)| is_write_pipe(fd))
        .map(|(fd, _)| {
            // SAFETY: plain fcntl on a descriptor checked above; the File
            // owns it from here (nothing else in this process uses it).
            unsafe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                File::from_raw_fd(fd)
            }
        });
    let _ = REPORT.set(Mutex::new(adopted));
}

/// Tell the parent this process's exit code (nothing without a pipe).
pub fn report(code: u32) {
    let Some(slot) = REPORT.get() else { return };
    let taken = slot.lock().ok().and_then(|mut g| g.take());
    if let Some(mut f) = taken {
        let _ = f.write_all(&code.to_le_bytes());
    }
}

/// The exit code of a child that ended with `status`, from its report on
/// `read` (the child has ended, so the read does not block: data, or the
/// end of the pipe). `None` when `status` gives no code (killed by a host
/// signal).
pub fn full_code(read: Option<File>, status: ExitStatus) -> Option<u32> {
    let host = status.code()?;
    let mut buf = [0u8; REPORT_LEN];
    let reported = read.and_then(|mut f| {
        let mut got = 0;
        while got < REPORT_LEN {
            match f.read(&mut buf[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        (got == REPORT_LEN).then(|| u32::from_le_bytes(buf))
    });
    Some(match reported {
        Some(full) if full & 0xFF == host as u32 & 0xFF => full,
        _ => host as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn the_environment_value_is_parsed_strictly() {
        assert_eq!(parse_env("7:1234"), Some((7, 1234)));
        for bad in ["", "7", ":1", "7:", "2:1234", "7:0", "-7:1", "7:12a", "99999999999:1", "7:1:2", " 7:1"] {
            assert_eq!(parse_env(bad), None, "{bad:?}");
        }
    }

    /// A pipe's report gives the whole code when it agrees with the host
    /// status; the host status otherwise.
    #[test]
    fn a_report_gives_the_whole_code() {
        let ended = |code: i32| ExitStatus::from_raw(code << 8);
        let p = ExitPipe::new().unwrap();
        let mut w = File::from(p.write.try_clone().unwrap());
        w.write_all(&0x1234_5678u32.to_le_bytes()).unwrap();
        drop(w);
        assert_eq!(full_code(Some(p.into_read()), ended(0x78)), Some(0x1234_5678));
        // Disagreeing with the status: the status wins.
        let p = ExitPipe::new().unwrap();
        File::from(p.write.try_clone().unwrap()).write_all(&300u32.to_le_bytes()).unwrap();
        assert_eq!(full_code(Some(p.into_read()), ended(3)), Some(3));
        // Nothing written, or a short report: the status.
        let p = ExitPipe::new().unwrap();
        assert_eq!(full_code(Some(p.into_read()), ended(9)), Some(9));
        let p = ExitPipe::new().unwrap();
        File::from(p.write.try_clone().unwrap()).write_all(&[1, 2]).unwrap();
        assert_eq!(full_code(Some(p.into_read()), ended(1)), Some(1));
        assert_eq!(full_code(None, ended(5)), Some(5));
        // Killed by a host signal: no code.
        assert_eq!(full_code(None, ExitStatus::from_raw(libc::SIGKILL)), None);
    }

    #[test]
    fn only_a_writable_pipe_is_believed() {
        let p = ExitPipe::new().unwrap();
        assert!(is_write_pipe(p.write.as_raw_fd()));
        assert!(!is_write_pipe(p.read.as_raw_fd()), "the read end");
        let f = tempfile::tempfile().unwrap();
        assert!(!is_write_pipe(f.as_raw_fd()), "a file");
        assert!(!is_write_pipe(9999), "no descriptor");
    }
}
