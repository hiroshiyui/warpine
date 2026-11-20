// SPDX-License-Identifier: GPL-3.0-or-later
//
// OS/2 DOSCALLS and QUECALLS API handler methods.
//
// Handle routing (no fallback), resolved per handle by `handle_target`:
// - Pipe handles (HandleManager): PIPE_HANDLE_BASE+, and any handle a pipe
//   was duplicated into (`DosDupHandle(hPipe, &1)`)
// - VFS file handles (DriveManager): 3.., and standard handles redirected
//   to a file (`DosDupHandle(hFile, &1)`)
// - Console: unredirected standard handles 0/1/2 and duplicates of them
//
// A handle number is in at most one of these tables; VFS file operations
// never fall back to HandleManager and vice versa.

use std::fs::File;
use std::io::{Read, Write, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::thread;
use super::vm_backend::VcpuBackend;
use log::debug;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::managers::PIPE_HANDLE_BASE;
use super::ipc::*;
use super::vfs::*;

/// How often a semaphore wait looks at the process end.
const SEM_POLL: Duration = Duration::from_millis(100);

/// When a semaphore wait of `msec` ms (`SEM_INDEFINITE_WAIT`: forever) ends.
fn sem_deadline(msec: u32) -> std::time::Instant {
    // "Forever": far enough not to matter, near enough not to overflow.
    const FOREVER: Duration = Duration::from_secs(100 * 365 * 24 * 3600);
    let wait = if msec == u32::MAX { FOREVER } else { Duration::from_millis(msec as u64) };
    std::time::Instant::now() + wait
}

/// What a file handle leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandleTarget {
    /// A pipe (HandleManager).
    Pipe,
    /// A file (DriveManager).
    File,
    /// The console: standard stream 0 (keyboard), 1 (screen) or 2 (error).
    Console(u32),
}

impl super::Loader {
    /// Put `entry` in the pipe table under a new number. Files and pipes
    /// share one handle space (as in OS/2), the lowest free number from 3.
    pub(crate) fn add_pipe_handle(&self, entry: super::managers::PipeHandle) -> u32 {
        let mut hm = self.shared.handle_mgr.lock_or_recover();
        let h = self.shared.drive_mgr.lock_or_recover().reserve_pipe_handle();
        hm.replace(h, entry);
        h
    }

    /// Put `entry` in the pipe table at number `h`, closing what `h` was.
    pub(crate) fn put_pipe_handle_at(&self, h: u32, entry: super::managers::PipeHandle) {
        let mut hm = self.shared.handle_mgr.lock_or_recover();
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        dm.release_for_dup(h);
        dm.reserve_pipe_handle_at(h);
        hm.replace(h, entry);
    }

    /// Remove `h` from the pipe table; false when it was not there.
    pub(crate) fn close_pipe_handle(&self, h: u32) -> bool {
        let mut hm = self.shared.handle_mgr.lock_or_recover();
        let closed = hm.close(h);
        if closed {
            self.shared.drive_mgr.lock_or_recover().release_pipe_handle(h);
        }
        closed
    }

    /// Where handle `hf` leads, or `None` for a handle that is not open.
    pub(crate) fn handle_target(&self, hf: u32) -> Option<HandleTarget> {
        if self.shared.handle_mgr.lock_or_recover().contains(hf) {
            return Some(HandleTarget::Pipe);
        }
        let dm = self.shared.drive_mgr.lock_or_recover();
        if dm.is_file(hf) {
            Some(HandleTarget::File)
        } else {
            dm.console_stream(hf).map(HandleTarget::Console)
        }
    }

    // ── File I/O (via DriveManager → VfsBackend) ──

    pub fn dos_open(&self, psz_name_ptr: u32, phf_ptr: u32, pul_action_ptr: u32, fs_open_flags: u32, fs_open_mode: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        if super::npipe::is_pipe_name(&name) {
            // The client end of a named pipe (`npipe_api.rs`).
            return self.np_open_client(&name, phf_ptr, pul_action_ptr, fs_open_mode);
        }
        let mode = match OpenMode::from_raw(fs_open_mode) {
            Ok(m) => m,
            Err(e) => return e.0,
        };
        let sharing = SharingMode::from_raw(fs_open_mode);
        let flags = OpenFlags::from_raw(fs_open_flags);

        let mut dm = self.shared.drive_mgr.lock_or_recover();
        match dm.open_file(&name, mode, sharing, flags, FileAttribute::NORMAL) {
            Ok((handle, action)) => {
                self.guest_write::<u32>(phf_ptr, handle);
                if pul_action_ptr != 0 {
                    self.guest_write::<u32>(pul_action_ptr, action as u32);
                }
                0
            }
            Err(e) => e.0,
        }
    }

    pub fn dos_close(&self, hf: u32) -> u32 {
        // A standard handle stays closed afterwards — also one that was
        // redirected — until something is duplicated into it.
        match self.handle_target(hf) {
            Some(HandleTarget::Pipe) => {
                self.close_pipe_handle(hf);
                self.shared.drive_mgr.lock_or_recover().mark_std_closed(hf);
                NO_ERROR
            }
            Some(_) => {
                let mut dm = self.shared.drive_mgr.lock_or_recover();
                // The console itself (an unredirected standard handle) has
                // nothing to close but its mark.
                let rc = if dm.console_stream(hf) == Some(hf) {
                    NO_ERROR
                } else {
                    match dm.close_file(hf) {
                        Ok(()) => NO_ERROR,
                        Err(e) => e.0,
                    }
                };
                dm.mark_std_closed(hf);
                rc
            }
            None => ERROR_INVALID_HANDLE,
        }
    }

    pub fn dos_read(&self, hf: u32, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        debug!("  DosRead(hf={}, buf=0x{:08X}, len={}, actual=0x{:08X})", hf, buf_ptr, len, actual_ptr);
        let target = self.handle_target(hf);
        if let Some(HandleTarget::Console(_)) = target {
            // The console reads the keyboard whichever standard stream the
            // handle came from.
            return self.dos_read_stdin(buf_ptr, len, actual_ptr);
        }

        // Cap read length to prevent OOM from untrusted guest-supplied sizes.
        // 64 MB is far larger than any realistic single OS/2 file read.
        let len = len.min(64 * 1024 * 1024);

        if target == Some(HandleTarget::Pipe) || hf >= PIPE_HANDLE_BASE {
            // A pipe: the read may block, so it runs without the table's
            // lock (another thread may write the other end meanwhile).
            if let Some((end, fh_state)) = self.np_end(hf) {
                return self.np_read(&end, fh_state, buf_ptr, len, actual_ptr);
            }
            let Some(file) = self.shared.handle_mgr.lock_or_recover().host_file(hf) else {
                return ERROR_INVALID_HANDLE;
            };
            // Wait for data (or the writer's end) in steps, so that a
            // signal or the end of the process can interrupt the read; an
            // interrupted read has taken nothing and is made again.
            if len > 0 && !self.wait_host_readable(&file) {
                return ERROR_INTERRUPT;
            }
            let mut data = vec![0u8; len as usize];
            match (&*file).read(&mut data) {
                Ok(n) => {
                    if self.guest_write_bytes(buf_ptr, &data[..n]).is_none() {
                        return ERROR_INVALID_PARAMETER;
                    }
                    if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, n as u32); }
                    NO_ERROR
                },
                Err(_) => ERROR_ACCESS_DENIED,
            }
        } else {
            // VFS file handle
            let dm = self.shared.drive_mgr.lock_or_recover();
            let mut data = vec![0u8; len as usize];
            match dm.read_file(hf, &mut data) {
                Ok(n) => {
                    self.guest_write_bytes(buf_ptr, &data[..n]);
                    if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, n as u32); }
                    0
                }
                Err(e) => e.0,
            }
        }
    }

    /// Read from stdin (handle 0).
    ///
    /// In SDL text mode, reads from `kbd_queue` (populated by the SDL renderer)
    /// and echoes typed characters into the VioManager buffer.
    /// In terminal mode, enables raw mode and reads from the real stdin fd.
    /// Translates CR → CR+LF (OS/2 console convention) with a pending-LF mechanism.
    fn dos_read_stdin(&self, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        if len == 0 {
            if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 0); }
            return 0;
        }

        // Pending LF from a previous CR→CRLF translation (both modes).
        {
            let mut console = self.shared.console_mgr.lock_or_recover();
            if console.stdin_pending_lf {
                console.stdin_pending_lf = false;
                self.guest_write::<u8>(buf_ptr, 0x0A);
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 1); }
                return 0;
            }
        }

        if self.shared.use_sdl_text.load(Ordering::Relaxed) {
            // SDL path: block on kbd_queue, delivering one character per call.
            // Loops for characters that must be silently discarded (e.g. backspace
            // at start of line) so we never return 0 bytes to the caller.
            loop {
                let ki = {
                    let mut queue = self.shared.kbd_queue.lock().unwrap();
                    loop {
                        if self.stop_or_restart() { return ERROR_INVALID_FUNCTION; }
                        if let Some(ki) = queue.pop_front() { break ki; }
                        let (new_q, _) = self.shared.kbd_cond
                            .wait_timeout(queue, Duration::from_millis(50))
                            .unwrap();
                        queue = new_q;
                    }
                };
                let ch = ki.ch;
                if ch == 0x0D {
                    // Enter: deliver CR, queue LF for next call, echo newline.
                    let mut console = self.shared.console_mgr.lock_or_recover();
                    console.stdin_pending_lf = true;
                    console.stdin_cooked_chars = 0; // reset for next line
                    console.write_tty(b"\r\n", 437);
                    drop(console);
                    self.guest_write::<u8>(buf_ptr, 0x0D);
                } else if ch == 0x08 {
                    // Backspace: only echo destructive sequence if something has
                    // been typed on this input line; otherwise discard and wait
                    // for the next character (never return 0 bytes to caller).
                    let mut console = self.shared.console_mgr.lock_or_recover();
                    if console.stdin_cooked_chars > 0 {
                        console.stdin_cooked_chars -= 1;
                        console.write_tty(b"\x08 \x08", 437);
                        drop(console);
                        self.guest_write::<u8>(buf_ptr, ch);
                    } else {
                        // Nothing to erase — silently discard; loop for next char.
                        continue;
                    }
                } else {
                    // Printable or extended — echo and deliver.
                    let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
                    let mut console = self.shared.console_mgr.lock_or_recover();
                    if ch >= 0x20 {
                        console.stdin_cooked_chars += 1;
                        console.write_tty(&[ch], cp);
                    }
                    drop(console);
                    self.guest_write::<u8>(buf_ptr, ch);
                }
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 1); }
                return 0;
            }
        }

        // Redirected standard input (a pipe or a file) is data: read as
        // much as asked, with no echo and no CR → CR LF (OS/2 echoes and
        // edits only what is typed at the console).
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            return self.dos_read_redirected_stdin(buf_ptr, len, actual_ptr);
        }

        // Terminal (termios) path — enable raw mode and read from stdin fd.
        {
            let mut console = self.shared.console_mgr.lock_or_recover();
            console.enable_raw_mode();
        }
        loop {
            if self.stop_or_restart() { return ERROR_INVALID_FUNCTION; }
            let mut buf = [0u8; 1];
            let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1) };
            // Ctrl+C typed at a terminal is a signal, not input (piped
            // standard input is data: it may hold any byte).
            if n == 1 && std::io::IsTerminal::is_terminal(&std::io::stdin()) && self.keyboard_signal(buf[0]) {
                continue;
            }
            if n == 1 {
                let byte = buf[0];
                if byte == 0x0D {
                    // CR from Enter key → deliver CR now, queue LF for next read
                    let mut console = self.shared.console_mgr.lock_or_recover();
                    console.stdin_pending_lf = true;
                    let _ = unsafe { libc::write(libc::STDOUT_FILENO, b"\r\n".as_ptr() as *const libc::c_void, 2) };
                } else if byte == 0x08 || byte == 0x7F {
                    let _ = unsafe { libc::write(libc::STDOUT_FILENO, b"\x08 \x08".as_ptr() as *const libc::c_void, 3) };
                } else if byte >= 0x20 {
                    let _ = unsafe { libc::write(libc::STDOUT_FILENO, buf.as_ptr() as *const libc::c_void, 1) };
                }
                self.guest_write::<u8>(buf_ptr, byte);
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 1); }
                return 0;
            }
            if n == 0 {
                // A terminal in raw mode returns 0 while no key is pressed;
                // anything else (pipe, file, /dev/null) is at end of input,
                // which DosRead reports as 0 bytes read.
                if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 0); }
                    return 0;
                }
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            if n < 0 {
                let err = unsafe { *libc::__errno_location() };
                if err == libc::EAGAIN || err == libc::EINTR {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 0); }
                return ERROR_ACCESS_DENIED;
            }
        }
    }

    /// `DosRead` of a standard input that is a host pipe or file: up to
    /// `len` bytes (at most `MAX_REDIRECTED_READ` at once), 0 at its end.
    fn dos_read_redirected_stdin(&self, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        const MAX_REDIRECTED_READ: usize = 64 * 1024;
        let mut buf = vec![0u8; (len as usize).min(MAX_REDIRECTED_READ)];
        if self.guest_slice_mut(buf_ptr, buf.len()).is_none() { return ERROR_INVALID_PARAMETER; }
        loop {
            if self.stop_or_restart() { return ERROR_INVALID_FUNCTION; }
            // SAFETY: reads at most `buf.len()` bytes into a buffer of that
            // size owned here; no guest memory is involved.
            let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n >= 0 {
                let n = n as usize;
                self.guest_write_bytes(buf_ptr, &buf[..n]);
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, n as u32); }
                return NO_ERROR;
            }
            let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if err == libc::EINTR { continue; }
            if err == libc::EAGAIN {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 0); }
            return ERROR_ACCESS_DENIED;
        }
    }

    pub fn dos_write(&self, fd: u32, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        let Some(data) = self.guest_slice_mut(buf_ptr, len as usize) else { return ERROR_INVALID_PARAMETER };
        let data = data.to_vec();
        match self.write_handle(fd, &data) {
            Ok(n) => {
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, n); }
                NO_ERROR
            }
            Err(rc) => rc,
        }
    }

    /// Write `data` to handle `fd` wherever it leads (shared by the 32-bit
    /// and 16-bit `DosWrite`); returns the bytes written or an OS/2 error.
    ///
    /// The console goes through `VioManager::write_tty`, as OS/2's
    /// `DosWrite` to the screen goes through `VioWrtTTY` — so ANSI escape
    /// sequences are carried out — in the SDL text window and when the
    /// host's standard output is a terminal. Output to a host pipe or file
    /// stays a plain byte stream, while the VIO screen buffer still follows
    /// it (`VioManager::track_tty`).
    pub(crate) fn write_handle(&self, fd: u32, data: &[u8]) -> Result<u32, u32> {
        match self.handle_target(fd) {
            Some(HandleTarget::Console(stream)) => {
                // Writing to the keyboard handle writes to the screen (CON).
                let stream = if stream == 0 { STDOUT_HANDLE } else { stream };
                let cp = self.shared.active_codepage.load(Ordering::Relaxed);
                if self.console_is_tty(stream) {
                    self.shared.console_mgr.lock_or_recover().write_tty(data, cp);
                    return Ok(data.len() as u32);
                }
                // The bytes go to the host pipe or file as they are; the VIO
                // screen still follows them.
                self.shared.console_mgr.lock_or_recover().track_tty(data, cp);
                crate::api::doscalls::dos_write(stream, data).map_err(|kind| match kind {
                    std::io::ErrorKind::BrokenPipe => ERROR_BROKEN_PIPE,
                    std::io::ErrorKind::InvalidInput => ERROR_INVALID_HANDLE,
                    _ => ERROR_ACCESS_DENIED,
                })
            }
            Some(HandleTarget::Pipe) => {
                if let Some((end, fh_state)) = self.np_end(fd) {
                    return self.np_write(&end, fh_state, data);
                }
                // Written without the table's lock: a full pipe blocks
                // until its reader (maybe another thread) takes data.
                let file = self.shared.handle_mgr.lock_or_recover().host_file(fd).ok_or(ERROR_INVALID_HANDLE)?;
                (&*file).write_all(data).map(|()| data.len() as u32).map_err(|e| match e.kind() {
                    std::io::ErrorKind::BrokenPipe => ERROR_BROKEN_PIPE,
                    _ => ERROR_ACCESS_DENIED,
                })
            }
            Some(HandleTarget::File) => {
                let dm = self.shared.drive_mgr.lock_or_recover();
                dm.write_file(fd, data).map(|n| n as u32).map_err(|e| e.0)
            }
            None => Err(ERROR_INVALID_HANDLE),
        }
    }

    /// Whether console output on `stream` (1 or 2) goes through the VIO
    /// screen: always in the SDL text window; on the host only when both
    /// the stream and the host's standard output are terminals (VIO echoes
    /// to standard output).
    fn console_is_tty(&self, stream: u32) -> bool {
        use std::io::IsTerminal;
        if self.shared.use_sdl_text.load(Ordering::Relaxed) {
            return true;
        }
        let stdout_tty = std::io::stdout().is_terminal();
        stdout_tty && (stream == STDOUT_HANDLE || std::io::stderr().is_terminal())
    }

    pub fn dos_set_file_ptr(&self, hf: u32, offset: i32, method: u32, actual_ptr: u32) -> u32 {
        if self.handle_target(hf) == Some(HandleTarget::Pipe) || hf >= PIPE_HANDLE_BASE {
            let file = self.shared.handle_mgr.lock_or_recover().host_file(hf);
            if let Some(file) = file {
                let pos = match method {
                    0 => SeekFrom::Start(offset as u64),
                    1 => SeekFrom::Current(offset as i64),
                    2 => SeekFrom::End(offset as i64),
                    _ => return 1,
                };
                match (&*file).seek(pos) {
                    Ok(new_pos) => {
                        if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, new_pos as u32); }
                        0
                    }
                    Err(_) => 1,
                }
            } else { 6 }
        } else {
            let mode = match SeekMode::from_raw(method) {
                Ok(m) => m,
                Err(e) => return e.0,
            };
            let dm = self.shared.drive_mgr.lock_or_recover();
            match dm.seek_file(hf, offset as i64, mode) {
                Ok(new_pos) => {
                    if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, new_pos as u32); }
                    0
                }
                Err(e) => e.0,
            }
        }
    }

    pub fn dos_set_file_size(&self, hf: u32, new_size: u32) -> u32 {
        debug!("DosSetFileSize(hf={}, size={})", hf, new_size);
        if self.handle_target(hf) == Some(HandleTarget::Pipe) || hf >= PIPE_HANDLE_BASE {
            let file = self.shared.handle_mgr.lock_or_recover().host_file(hf);
            if let Some(file) = file {
                match file.set_len(new_size as u64) {
                    Ok(_) => 0,
                    Err(_) => ERROR_ACCESS_DENIED,
                }
            } else { ERROR_INVALID_HANDLE }
        } else {
            let dm = self.shared.drive_mgr.lock_or_recover();
            match dm.set_file_size(hf, new_size as u64) {
                Ok(()) => 0,
                Err(e) => e.0,
            }
        }
    }

    /// DosSetFileLocks (ordinal 428): lock/unlock byte ranges.
    ///
    /// OS/2 signature: DosSetFileLocks(hFile, pUnlock, pLock, timeout, flags)
    /// - pUnlock/pLock: pointer to FILELOCK struct { offset(4), range(4) }, or 0 if none
    /// - flags: bit 0 = atomic (unlock+lock in one operation), bit 1 = shared lock
    pub fn dos_set_file_locks(&self, hf: u32, p_unlock: u32, p_lock: u32, timeout: u32, _flags: u32) -> u32 {
        debug!("  DosSetFileLocks(hf={}, timeout={})", hf, timeout);

        let mut unlock_ranges = Vec::new();
        if p_unlock != 0 {
            let offset = self.guest_read::<u32>(p_unlock).unwrap_or(0);
            let length = self.guest_read::<u32>(p_unlock + 4).unwrap_or(0);
            if length > 0 {
                unlock_ranges.push(FileLockRange { offset, length });
            }
        }

        let mut lock_ranges = Vec::new();
        if p_lock != 0 {
            let offset = self.guest_read::<u32>(p_lock).unwrap_or(0);
            let length = self.guest_read::<u32>(p_lock + 4).unwrap_or(0);
            if length > 0 {
                lock_ranges.push(FileLockRange { offset, length });
            }
        }

        if unlock_ranges.is_empty() && lock_ranges.is_empty() {
            return 0;
        }

        let dm = self.shared.drive_mgr.lock_or_recover();
        match dm.set_file_locks(hf, &unlock_ranges, &lock_ranges, timeout) {
            Ok(()) => 0,
            Err(e) => e.0,
        }
    }

    /// DosProtectSetFileLocks (ordinal 639): lock/unlock with file lock ID.
    ///
    /// Same as DosSetFileLocks but with an additional fhFileHandleLockID parameter
    /// for protected file handle operations. We ignore the lock ID.
    pub fn dos_protect_set_file_locks(&self, hf: u32, p_unlock: u32, p_lock: u32, timeout: u32, flags: u32, _lock_id: u32) -> u32 {
        debug!("  DosProtectSetFileLocks(hf={})", hf);
        self.dos_set_file_locks(hf, p_unlock, p_lock, timeout, flags)
    }

    // ── Path operations (via DriveManager) ──

    pub fn dos_delete(&self, psz_name_ptr: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let dm = self.shared.drive_mgr.lock_or_recover();
        match dm.delete_file(&name) {
            Ok(()) => 0,
            Err(e) => e.0,
        }
    }

    pub fn dos_move(&self, psz_old_ptr: u32, psz_new_ptr: u32) -> u32 {
        let old_name = self.read_guest_string(psz_old_ptr);
        let new_name = self.read_guest_string(psz_new_ptr);
        let dm = self.shared.drive_mgr.lock_or_recover();
        match dm.rename_file(&old_name, &new_name) {
            Ok(()) => 0,
            Err(e) => e.0,
        }
    }

    pub fn dos_create_dir(&self, psz_name_ptr: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let dm = self.shared.drive_mgr.lock_or_recover();
        match dm.create_dir(&name) {
            Ok(()) => 0,
            Err(e) => e.0,
        }
    }

    pub fn dos_delete_dir(&self, psz_name_ptr: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let dm = self.shared.drive_mgr.lock_or_recover();
        match dm.delete_dir(&name) {
            Ok(()) => 0,
            Err(e) => e.0,
        }
    }

    pub fn dos_query_path_info(&self, psz_path_ptr: u32, level: u32, buf_ptr: u32, buf_len: u32) -> u32 {
        let name = self.read_guest_string(psz_path_ptr);
        let dm = self.shared.drive_mgr.lock_or_recover();
        match level {
            // Level 1: FILESTATUS3 (24 bytes)
            1 => {
                if buf_len < 24 { return 111; }
                match dm.query_path_info(&name, level) {
                    Ok(status) => { self.write_filestatus3_from_vfs(&status, buf_ptr); 0 }
                    Err(e) => e.0,
                }
            }
            // Level 2: FILESTATUS3 (24 bytes) + cbList (4 bytes) = FIL_QUERYEASIZE
            2 => {
                if buf_len < 28 { return 111; }
                match dm.query_path_info(&name, 1) {
                    Ok(status) => {
                        self.write_filestatus3_from_vfs(&status, buf_ptr);
                        let ea_size = self.compute_ea_size(&dm, &name);
                        self.guest_write::<u32>(buf_ptr + 24, ea_size);
                        0
                    }
                    Err(e) => e.0,
                }
            }
            // Level 3: FIL_QUERYEASFROMLIST — query specific EAs by name list
            3 => {
                // Input buffer contains a GEA2LIST: cbList(4) + GEA2 entries
                // GEA2: oNextEntryOffset(4) + cbName(1) + szName(cbName+1)
                // Output: FEA2LIST: cbList(4) + FEA2 entries
                // FEA2: oNextEntryOffset(4) + fEA(1) + cbName(1) + cbValue(2) + szName(cbName+1) + value(cbValue)
                self.dos_query_eas_from_list(&dm, &name, buf_ptr, buf_len)
            }
            _ => 124, // ERROR_INVALID_LEVEL
        }
    }

    pub fn dos_query_file_info(&self, hf: u32, level: u32, buf_ptr: u32, buf_len: u32) -> u32 {
        match level {
            1 => {
                if buf_len < 24 { return 111; }
            }
            2 => {
                if buf_len < 28 { return 111; }
            }
            3 => {
                // Level 3 uses EAOP2 (12 bytes) — handled separately
                if buf_len < 12 { return 111; }
                // For file handle-based level 3, we'd need the path from the handle.
                // Return 0 with empty FEA2LIST for now.
                return 0;
            }
            _ => return 124,
        }
        if self.handle_target(hf) == Some(HandleTarget::Pipe) || hf >= PIPE_HANDLE_BASE {
            let file = self.shared.handle_mgr.lock_or_recover().host_file(hf);
            if let Some(file) = file
                && let Ok(meta) = file.metadata() {
                    self.write_filestatus3_internal(&meta, buf_ptr);
                    if level == 2 { self.guest_write::<u32>(buf_ptr + 24, 4); }
                    return 0;
            }
            return 6; // ERROR_INVALID_HANDLE
        }
        let dm = self.shared.drive_mgr.lock_or_recover();
        match dm.query_file_info(hf, 1) {
            Ok(status) => {
                self.write_filestatus3_from_vfs(&status, buf_ptr);
                if level == 2 {
                    self.guest_write::<u32>(buf_ptr + 24, 4);
                }
                0
            }
            Err(e) => e.0,
        }
    }

    pub fn dos_reset_buffer(&self, hf: u32) -> u32 {
        debug!("DosResetBuffer(hf={})", hf);
        if hf == 0xFFFFFFFF {
            // Flush all handles in both managers
            self.shared.drive_mgr.lock_or_recover().flush_all();
            self.shared.handle_mgr.lock_or_recover().flush_all();
            0
        } else if self.handle_target(hf) == Some(HandleTarget::Pipe) || hf >= PIPE_HANDLE_BASE {
            let h_mgr = self.shared.handle_mgr.lock_or_recover();
            match h_mgr.entry(hf) {
                Some(e) => {
                    if let super::managers::PipeObject::Host(f) = &e.obj { let _ = (&**f).flush(); }
                    NO_ERROR
                }
                None => ERROR_INVALID_HANDLE,
            }
        } else if let Some(HandleTarget::Console(_)) = self.handle_target(hf) {
            // Console output is not buffered.
            0
        } else {
            match self.shared.drive_mgr.lock_or_recover().flush_file(hf) {
                Ok(()) => 0,
                Err(e) => e.0,
            }
        }
    }

    // ── Extended Attributes ──

    /// DosEnumAttribute (ordinal 372): enumerate extended attributes.
    ///
    /// OS/2 signature: DosEnumAttribute(ulRefType, pvFile, ulEntry, pvBuf, cbBuf, pulCount, ulInfoLevel)
    /// - ulRefType: 0 = pvFile is path (PCSZ), 1 = pvFile is file handle (HFILE)
    /// - ulEntry: 1-based index of first EA to return
    /// - pvBuf: output buffer for DENA1 structs
    /// - cbBuf: buffer size
    /// - pulCount: in/out count of entries
    /// - ulInfoLevel: must be 1 (ENUMEA_LEVEL_NO_VALUE)
    #[allow(clippy::too_many_arguments)]
    pub fn dos_enum_attribute(&self, ref_type: u32, pv_file: u32, ul_entry: u32,
                              pv_buf: u32, cb_buf: u32, pul_count: u32, info_level: u32) -> u32 {
        debug!("  DosEnumAttribute(refType={}, entry={}, level={})", ref_type, ul_entry, info_level);
        if info_level != 1 { return 124; } // ERROR_INVALID_LEVEL

        // Get the path to enumerate EAs on
        let path_str = if ref_type == 0 {
            // pvFile is a path string
            self.read_guest_string(pv_file)
        } else {
            // pvFile is a file handle — not yet supported, return empty
            if pul_count != 0 { self.guest_write::<u32>(pul_count, 0); }
            return 0;
        };

        let dm = self.shared.drive_mgr.lock_or_recover();
        let (drive, rel_path) = match dm.resolve_path(&path_str) {
            Ok(r) => r,
            Err(e) => return e.0,
        };
        let eas = match dm.backend(drive) {
            Ok(b) => b.enum_ea(&rel_path).unwrap_or_default(),
            Err(e) => return e.0,
        };

        let max_count = if pul_count != 0 {
            self.guest_read::<u32>(pul_count).unwrap_or(1) as usize
        } else { 1 };

        // ul_entry is 1-based
        let start = if ul_entry > 0 { (ul_entry - 1) as usize } else { 0 };
        let mut offset = 0u32;
        let mut count = 0u32;

        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        for ea in eas.iter().skip(start).take(max_count) {
            let name_bytes = super::codepage::cp_encode(&ea.name, cp);
            // DENA1 structure: reserved(4) + cbName(1) + cbValue(2) + szName(cbName+1)
            let entry_size = 4 + 1 + 2 + name_bytes.len() as u32 + 1;
            let aligned_size = (entry_size + 3) & !3; // 4-byte aligned
            if offset + aligned_size > cb_buf { break; }

            self.guest_write::<u32>(pv_buf + offset, 0);           // reserved
            self.guest_write::<u8>(pv_buf + offset + 4, name_bytes.len() as u8); // cbName
            self.guest_write::<u16>(pv_buf + offset + 5, ea.value.len() as u16); // cbValue
            self.guest_write_bytes(pv_buf + offset + 7, &name_bytes);
            self.guest_write::<u8>(pv_buf + offset + 7 + name_bytes.len() as u32, 0); // null term

            offset += aligned_size;
            count += 1;
        }

        if pul_count != 0 {
            self.guest_write::<u32>(pul_count, count);
        }
        0
    }

    // ── Directory enumeration (via DriveManager) ──

    #[allow(clippy::too_many_arguments)]
    pub fn dos_find_first(&self, psz_spec_ptr: u32, phdir_ptr: u32, attr: u32, buf_ptr: u32, buf_len: u32, pc_found_ptr: u32, level: u32) -> u32 {
        if level != 1 && level != 2 { return 124; }
        let mut spec = self.read_guest_string(psz_spec_ptr);
        debug!("  DosFindFirst('{}', attr=0x{:04X}, level={}, buf_len={})", spec, attr, level, buf_len);

        // If the spec is a bare directory (ends with \ or is just a drive letter),
        // append *.* to list its contents. OS/2 apps may pass "C:\" expecting to
        // enumerate the root directory contents (e.g., 4OS2 `dir` command).
        // Strip trailing non-printable characters (thunk skip may leave garbage bytes)
        while spec.ends_with(|c: char| c < ' ') {
            spec.pop();
        }

        // If the spec is a bare directory (ends with \ or is just a drive letter),
        // append *.* to list its contents. OS/2 apps may pass "C:\" expecting to
        // enumerate the root directory contents (e.g., 4OS2 `dir` command when the
        // wildcard appending code is in a thunk-skipped path).
        let spec_trimmed = spec.trim_end_matches(&['\\', '/'][..]);
        let is_bare_dir = spec.ends_with('\\') || spec.ends_with('/')
            || (spec_trimmed.len() == 2 && spec_trimmed.as_bytes()[1] == b':');
        if is_bare_dir {
            if !spec.ends_with('\\') && !spec.ends_with('/') {
                spec.push('\\');
            }
            spec.push_str("*.*");
            debug!("  DosFindFirst: bare directory, expanded to '{}'", spec);
        }

        let requested = if pc_found_ptr != 0 {
            self.guest_read::<u32>(pc_found_ptr).unwrap_or(1).max(1)
        } else { 1 };

        let mut dm = self.shared.drive_mgr.lock_or_recover();
        match dm.find_first(&spec, FileAttribute(attr), level) {
            Ok((hdir, first_entry)) => {
                self.guest_write::<u32>(phdir_ptr, hdir);
                let mut entries = vec![first_entry];
                for _ in 1..requested {
                    match dm.find_next(hdir) {
                        Ok(entry) => entries.push(entry),
                        Err(_) => break,
                    }
                }
                debug!("  DosFindFirst -> {} entries, first='{}'", entries.len(),
                       entries.first().map(|e| e.name.as_str()).unwrap_or("?"));
                self.write_filefindbuf3_multi(&entries, buf_ptr, buf_len, pc_found_ptr, level == 2)
            }
            Err(e) => { debug!("  DosFindFirst -> error {}", e.0); e.0 },
        }
    }

    pub fn dos_find_next(&self, hdir: u32, buf_ptr: u32, buf_len: u32, pc_found_ptr: u32) -> u32 {
        let requested = if pc_found_ptr != 0 {
            self.guest_read::<u32>(pc_found_ptr).unwrap_or(1).max(1)
        } else { 1 };

        let dm = self.shared.drive_mgr.lock_or_recover();
        let mut entries = Vec::new();
        for _ in 0..requested {
            match dm.find_next(hdir) {
                Ok(entry) => entries.push(entry),
                Err(_) => break,
            }
        }
        if entries.is_empty() {
            return 18; // ERROR_NO_MORE_FILES
        }
        let include_ea = dm.find_level(hdir) == 2;
        self.write_filefindbuf3_multi(&entries, buf_ptr, buf_len, pc_found_ptr, include_ea)
    }

    pub fn dos_find_close(&self, hdir: u32) -> u32 {
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        match dm.find_close(hdir) {
            Ok(()) => 0,
            Err(e) => e.0,
        }
    }

    // ── Directory / drive state (via DriveManager) ──

    pub fn dos_set_current_dir(&self, psz_dir_name: u32) -> u32 {
        let name = self.read_guest_string(psz_dir_name);
        debug!("  DosSetCurrentDir('{}')", name);
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        match dm.set_current_dir(&name) {
            Ok(()) => NO_ERROR,
            Err(e) => e.0,
        }
    }

    pub fn dos_query_current_dir(&self, disk_num: u32, p_buf: u32, pcb_buf: u32) -> u32 {
        debug!("  DosQueryCurrentDir(disk={})", disk_num);
        let dm = self.shared.drive_mgr.lock_or_recover();
        // 0 is the current drive, 1 is A:, … 26 is Z:.
        let drive = match disk_num {
            0 => dm.current_disk(),
            1..=26 if dm.logical_drive_map() & (1 << (disk_num - 1)) != 0 => (disk_num - 1) as u8,
            _ => return ERROR_INVALID_DRIVE,
        };
        // As OS/2 shows it: backslashes, no drive and no leading backslash.
        let dir = dm.current_dir_os2(drive);
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let dir_bytes = super::codepage::cp_encode(&dir, cp);

        if pcb_buf != 0 {
            let buf_len = self.guest_read::<u32>(pcb_buf).unwrap_or(0) as usize;
            if buf_len < dir_bytes.len() + 1 {
                self.guest_write::<u32>(pcb_buf, (dir_bytes.len() + 1) as u32);
                return ERROR_BUFFER_OVERFLOW;
            }
            self.guest_write::<u32>(pcb_buf, (dir_bytes.len() + 1) as u32);
        }

        if p_buf != 0 {
            self.guest_write_bytes(p_buf, &dir_bytes);
            self.guest_write::<u8>(p_buf + dir_bytes.len() as u32, 0);
        }
        NO_ERROR
    }

    pub fn dos_query_current_disk(&self, p_disk_num: u32, p_logical: u32) -> u32 {
        debug!("  DosQueryCurrentDisk");
        let dm = self.shared.drive_mgr.lock_or_recover();
        if p_disk_num != 0 {
            self.guest_write::<u32>(p_disk_num, dm.current_disk_os2() as u32);
        }
        if p_logical != 0 {
            self.guest_write::<u32>(p_logical, dm.logical_drive_map());
        }
        NO_ERROR
    }

    pub fn dos_set_default_disk(&self, disk_num: u32) -> u32 {
        debug!("  DosSetDefaultDisk({})", disk_num);
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        match dm.set_current_disk(disk_num as u8) {
            Ok(()) => {
                self.shared.process_mgr.lock_or_recover().current_disk = disk_num as u8;
                NO_ERROR
            }
            Err(e) => e.0,
        }
    }

    // ── Helpers ──

    /// Compute total EA size for a path (cbList value: 4 bytes minimum for empty list).
    fn compute_ea_size(&self, dm: &super::vfs::DriveManager, os2_path: &str) -> u32 {
        let (drive, rel_path) = match dm.resolve_path(os2_path) {
            Ok(r) => r,
            Err(_) => return 4,
        };
        match dm.backend(drive) {
            Ok(b) => b.enum_ea(&rel_path).map(|eas| {
                if eas.is_empty() { 4 } else {
                    // FEA2LIST cbList: 4 (header) + sum of FEA2 entries
                    // Each FEA2: oNextEntryOffset(4) + fEA(1) + cbName(1) + cbValue(2) + name(cbName+1) + value(cbValue)
                    eas.iter().map(|ea| 9 + ea.name.len() as u32 + ea.value.len() as u32).sum::<u32>() + 4
                }
            }).unwrap_or(4),
            Err(_) => 4,
        }
    }

    /// DosQueryPathInfo level 3 (FIL_QUERYEASFROMLIST): query specific EAs by name.
    ///
    /// The input buf_ptr initially contains a GEA2LIST (list of EA names to query).
    /// The output overwrites it with an FEA2LIST (EA names + values).
    /// OS/2 uses an EAOP2 struct at buf_ptr: pGEA2List(4) + pFEA2List(4) + oError(4) = 12 bytes.
    fn dos_query_eas_from_list(&self, dm: &super::vfs::DriveManager, os2_path: &str, buf_ptr: u32, buf_len: u32) -> u32 {
        if buf_len < 12 { return 111; }

        // EAOP2: pGEA2List(4) + pFEA2List(4) + oError(4)
        let p_gea2list = self.guest_read::<u32>(buf_ptr).unwrap_or(0);
        let p_fea2list = self.guest_read::<u32>(buf_ptr + 4).unwrap_or(0);

        if p_gea2list == 0 || p_fea2list == 0 { return 87; } // ERROR_INVALID_PARAMETER

        // Parse GEA2LIST: cbList(4) + GEA2 entries
        let gea_cb_list = self.guest_read::<u32>(p_gea2list).unwrap_or(0);
        if gea_cb_list < 4 { return 87; }

        // Read EA names from GEA2LIST
        let mut ea_names = Vec::new();
        let mut pos = 4u32; // skip cbList
        while pos < gea_cb_list {
            let o_next = self.guest_read::<u32>(p_gea2list + pos).unwrap_or(0);
            let cb_name = self.guest_read::<u8>(p_gea2list + pos + 4).unwrap_or(0) as u32;
            if cb_name == 0 { break; }
            // cb_name is a u8 (max 255) from guest input; cap at OS/2 max path length for safety.
            let cb_name = cb_name.min(260u32);
            let mut name_buf = vec![0u8; cb_name as usize];
            for i in 0..cb_name {
                name_buf[i as usize] = self.guest_read::<u8>(p_gea2list + pos + 5 + i).unwrap_or(0);
            }
            let name = String::from_utf8_lossy(&name_buf).into_owned();
            ea_names.push(name);
            if o_next == 0 { break; }
            pos += o_next;
        }

        // Resolve path and query each EA
        let (drive, rel_path) = match dm.resolve_path(os2_path) {
            Ok(r) => r,
            Err(e) => return e.0,
        };
        let backend = match dm.backend(drive) {
            Ok(b) => b,
            Err(e) => return e.0,
        };

        // Build FEA2LIST in output buffer
        let mut out_pos = 4u32; // skip cbList (will be written at the end)
        let mut prev_fea2_ptr: Option<u32> = None;

        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        for ea_name in &ea_names {
            let ea = match backend.get_ea(&rel_path, ea_name) {
                Ok(ea) => ea,
                Err(_) => EaEntry { name: ea_name.clone(), value: Vec::new(), flags: 0 },
            };
            let name_bytes = super::codepage::cp_encode(&ea.name, cp);

            // FEA2: oNextEntryOffset(4) + fEA(1) + cbName(1) + cbValue(2) + szName(cbName+1) + value(cbValue)
            let fea2_size = 4 + 1 + 1 + 2 + name_bytes.len() as u32 + 1 + ea.value.len() as u32;
            let aligned_size = (fea2_size + 3) & !3;

            let fea2_ptr = p_fea2list + out_pos;
            self.guest_write::<u32>(fea2_ptr, 0); // oNextEntryOffset (patched below)
            self.guest_write::<u8>(fea2_ptr + 4, ea.flags);
            self.guest_write::<u8>(fea2_ptr + 5, name_bytes.len() as u8);
            self.guest_write::<u16>(fea2_ptr + 6, ea.value.len() as u16);
            self.guest_write_bytes(fea2_ptr + 8, &name_bytes);
            self.guest_write::<u8>(fea2_ptr + 8 + name_bytes.len() as u32, 0); // null term
            if !ea.value.is_empty() {
                self.guest_write_bytes(fea2_ptr + 9 + name_bytes.len() as u32, &ea.value);
            }

            if let Some(prev) = prev_fea2_ptr {
                self.guest_write::<u32>(prev, fea2_ptr - prev);
            }
            prev_fea2_ptr = Some(fea2_ptr);
            out_pos += aligned_size;
        }

        // Write FEA2LIST cbList
        self.guest_write::<u32>(p_fea2list, out_pos);
        // Clear EAOP2 oError
        self.guest_write::<u32>(buf_ptr + 8, 0);
        0
    }

    /// Write a VFS FileStatus to guest memory as FILESTATUS3 (24 bytes).
    fn write_filestatus3_from_vfs(&self, status: &FileStatus, offset: u32) {
        self.guest_write::<u16>(offset, status.creation_date);
        self.guest_write::<u16>(offset + 2, status.creation_time);
        self.guest_write::<u16>(offset + 4, status.last_access_date);
        self.guest_write::<u16>(offset + 6, status.last_access_time);
        self.guest_write::<u16>(offset + 8, status.last_write_date);
        self.guest_write::<u16>(offset + 10, status.last_write_time);
        self.guest_write::<u32>(offset + 12, status.file_size);
        self.guest_write::<u32>(offset + 16, status.file_alloc);
        self.guest_write::<u32>(offset + 20, status.attributes.0);
    }

    /// Write multiple VFS DirEntry items to guest memory as packed FILEFINDBUF3 structs.
    /// Each entry's oNextEntryOffset points to the next (4-byte aligned); last entry has 0.
    ///
    /// When `include_ea_size` is true (level 2 / FIL_QUERYEASIZE), each entry has an extra
    /// cbList (4 bytes) field after FILESTATUS3, making the layout:
    /// oNextEntryOffset(4) + FILESTATUS3(24) + cbList(4) + cchName(1) + achName(var+1)
    ///
    /// Returns 0 on success, or an OS/2 error code.
    fn write_filefindbuf3_multi(&self, entries: &[DirEntry], buf_ptr: u32, buf_len: u32,
                                pc_found_ptr: u32, include_ea_size: bool) -> u32 {
        let mut offset = 0u32;
        let mut count = 0u32;
        let mut prev_offset_field: Option<u32> = None;
        let cp = self.shared.active_codepage.load(Ordering::Relaxed);

        let ea_field_size: u32 = if include_ea_size { 4 } else { 0 };

        for entry in entries.iter() {
            let name_bytes = super::codepage::cp_encode(&entry.name, cp);
            let name_len = name_bytes.len().min(255);
            // FILEFINDBUF3: oNextEntryOffset(4) + FILESTATUS3(24) [+ cbList(4)] + cchName(1) + achName(name_len+1)
            let entry_size = 4 + 24 + ea_field_size + 1 + name_len as u32 + 1;
            let aligned_size = (entry_size + 3) & !3;

            if offset + entry_size > buf_len { break; }

            let entry_ptr = buf_ptr + offset;
            let name_offset = 28 + ea_field_size; // cchName offset

            self.guest_write::<u32>(entry_ptr, 0); // oNextEntryOffset
            self.write_filestatus3_from_vfs(&entry.status, entry_ptr + 4);
            if include_ea_size {
                self.guest_write::<u32>(entry_ptr + 28, 4); // cbList: 4 = empty EA list
            }
            self.guest_write::<u8>(entry_ptr + name_offset, name_len as u8);
            self.guest_write_bytes(entry_ptr + name_offset + 1, &name_bytes[..name_len]);
            self.guest_write::<u8>(entry_ptr + name_offset + 1 + name_len as u32, 0);

            if let Some(prev_entry_ptr) = prev_offset_field {
                self.guest_write::<u32>(prev_entry_ptr, entry_ptr - prev_entry_ptr);
            }

            prev_offset_field = Some(entry_ptr);
            offset += aligned_size;
            count += 1;
        }

        if pc_found_ptr != 0 {
            self.guest_write::<u32>(pc_found_ptr, count);
        }
        if count == 0 { 111 } else { 0 }
    }

    /// Write std::fs::Metadata as FILESTATUS3 (legacy, for pipe handles).
    fn write_filestatus3_internal(&self, meta: &std::fs::Metadata, offset: u32) {
        let dos_date: u16 = 0x21; // 1980-01-01
        let dos_time: u16 = 0;
        self.guest_write::<u16>(offset, dos_date);
        self.guest_write::<u16>(offset + 2, dos_time);
        self.guest_write::<u16>(offset + 4, dos_date);
        self.guest_write::<u16>(offset + 6, dos_time);
        self.guest_write::<u16>(offset + 8, dos_date);
        self.guest_write::<u16>(offset + 10, dos_time);
        self.guest_write::<u32>(offset + 12, meta.len() as u32);
        self.guest_write::<u32>(offset + 16, meta.len() as u32);
        let attr: u32 = if meta.is_dir() { 0x10 } else { 0x00 };
        self.guest_write::<u32>(offset + 20, attr);
    }

    // ── Non-filesystem APIs (unchanged) ──

    /// Wait until host file `file` (a pipe) has data or its writer is gone;
    /// false when the wait gave up (the process ends, or `interrupt_restart`).
    pub(crate) fn wait_host_readable(&self, file: &std::fs::File) -> bool {
        use std::os::fd::AsRawFd;
        const READ_POLL_MS: i32 = 100;
        loop {
            let mut p = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: `p` is one valid pollfd for the duration of the call.
            let n = unsafe { libc::poll(&mut p, 1, READ_POLL_MS) };
            let interrupted = n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted;
            if n != 0 && !interrupted {
                // Ready, closed, or an error the read itself reports.
                return true;
            }
            if self.stop_or_restart() {
                return false;
            }
        }
    }

    pub fn dos_sleep(&self, msec: u32) -> u32 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(msec as u64);
        while std::time::Instant::now() < deadline {
            if self.interrupted() { return ERROR_INTERRUPT; }
            if self.shutting_down() { return 0; }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            thread::sleep(remaining.min(std::time::Duration::from_millis(100)));
        }
        0
    }

    /// DosExit (ordinal 234): `EXIT_THREAD` ends the calling thread (the
    /// vCPU loop ends it, `run_vcpu` decides whether the process ends with
    /// it); `EXIT_PROCESS` (and any other action) ends the process, after
    /// its exit list (the vCPU loop runs it). In an exit-list routine
    /// DosExit ends the routine.
    pub(crate) fn dos_exit(&self, vcpu_id: u32, action: u32, result: u32) -> super::ApiResult {
        if self.runs_exit_list(vcpu_id) {
            return super::ApiResult::ExitListNext;
        }
        if action == super::threads::EXIT_THREAD {
            return super::ApiResult::ExitThread(result);
        }
        super::ApiResult::ExitProcess(result)
    }

    /// End the process with `result` unless it is already ending (the first
    /// result code wins).
    pub fn request_process_exit(&self, result: u32) {
        super::signals::request_exit(&self.shared, result);
    }

    /// DosCreateThread (ordinal 311): a new thread runs `pfn(param)` on its
    /// own stack and vCPU (an idle vCPU of an ended thread if there is one);
    /// returning from `pfn` lands on `EXIT_TRAP_ADDR` and ends the thread.
    /// `CREATE_SUSPENDED` starts it suspended (`DosResumeThread` lets it
    /// run); `STACK_COMMITTED` changes nothing: Warpine's memory is always
    /// committed. Other flag bits are ignored.
    pub fn dos_create_thread(&self, vcpu_id: u32, ptid_ptr: u32, pfn: u32, param: u32, flags: u32, cb_stack: u32) -> u32 {
        use super::threads::ThreadMemory;
        let stack_size = if cb_stack == 0 { DEFAULT_THREAD_STACK } else { cb_stack };
        // Return address + parameter, below the top of the stack.
        const INITIAL_FRAME: u32 = 12;
        // EFLAGS bit 1 is reserved and always set.
        const INITIAL_RFLAGS: u64 = 2;
        if stack_size < INITIAL_FRAME { return ERROR_INVALID_PARAMETER; }
        let (stack_base, tib_addr) = {
            let mut mem_mgr = self.shared.mem_mgr.lock_or_recover();
            let Some(stack_base) = mem_mgr.alloc(stack_size) else { return ERROR_NOT_ENOUGH_MEMORY };
            let Some(tib_addr) = mem_mgr.alloc(THREAD_TIB_SIZE) else {
                mem_mgr.free(stack_base);
                return ERROR_NOT_ENOUGH_MEMORY;
            };
            (stack_base, tib_addr)
        };
        // The thread's page tables and thread-local page (paging.rs), when
        // the creating thread runs paged (32-bit programs do).
        let paging = if super::guest_mem::thread_tlm_backing() != 0 {
            match self.alloc_paging_block() {
                Some(p) => p,
                None => {
                    let mut mem_mgr = self.shared.mem_mgr.lock_or_recover();
                    mem_mgr.free(stack_base);
                    mem_mgr.free(tib_addr);
                    return ERROR_NOT_ENOUGH_MEMORY;
                }
            }
        } else { 0 };
        let free_memory = || {
            let mut mem_mgr = self.shared.mem_mgr.lock_or_recover();
            mem_mgr.free(stack_base);
            mem_mgr.free(tib_addr);
            if paging != 0 { mem_mgr.free(paging); }
        };
        let Some(stack_top) = stack_base.checked_add(stack_size) else {
            free_memory();
            return ERROR_NOT_ENOUGH_MEMORY;
        };
        let (tid, vcpu) = {
            let mut table = self.shared.threads.table.lock_or_recover();
            let Some(tid) = table.next_tid() else {
                drop(table);
                free_memory();
                return ERROR_MAX_THRDS_REACHED;
            };
            let vcpu = match table.take_vcpu() {
                Ok(v) => Ok(v),
                Err(kvm_id) => self.vm.create_vcpu(kvm_id),
            };
            match vcpu {
                Ok(v) => (tid, v),
                Err(e) => {
                    drop(table);
                    log::warn!("  [VCPU {}] DosCreateThread: no vCPU: {}", vcpu_id, e);
                    free_memory();
                    return ERROR_MAX_THRDS_REACHED;
                }
            }
        };
        debug!("  [VCPU {}] Creating thread {} (ptid_ptr=0x{:08X}, pfn=0x{:08X}, param=0x{:08X})", vcpu_id, tid, ptid_ptr, pfn, param);

        // TIB + TIB2 (the memory may have belonged to an ended thread).
        let tib2_addr = tib_addr + TIB2_FROM_TIB;
        let tib_ok = self.guest_write_bytes(tib_addr, &[0u8; THREAD_TIB_SIZE as usize]).is_some()
            && self.guest_write::<u32>(tib_addr + TIB_EXCHAIN_OFFSET, XCPT_CHAIN_END).is_some()
            && self.guest_write::<u32>(tib_addr + TIB_PSTACK_OFFSET, stack_base).is_some()
            && self.guest_write::<u32>(tib_addr + TIB_PSTACKLIMIT_OFFSET, stack_top).is_some()
            && self.guest_write::<u32>(tib_addr + TIB_PTIB2_OFFSET, tib2_addr).is_some()
            && self.guest_write::<u32>(tib_addr + TIB_ORDINAL_OFFSET, tid).is_some()
            && self.guest_write::<u32>(tib_addr + TIB_SELF_OFFSET, tib_addr).is_some()
            && self.guest_write::<u32>(tib_addr + TIB_PIB_OFFSET, PIB_BASE).is_some()
            && self.guest_write::<u32>(tib2_addr + TIB2_TID_OFFSET, tid).is_some()
            && self.guest_write::<u32>(tib2_addr + TIB2_PRI_OFFSET, 0).is_some();
        let sp_addr = stack_top - INITIAL_FRAME;
        let stack_ok = self.guest_write::<u32>(sp_addr, EXIT_TRAP_ADDR).is_some()
            && self.guest_write::<u32>(sp_addr + 4, param).is_some();
        if !tib_ok || !stack_ok {
            self.shared.threads.table.lock_or_recover().put_vcpu(vcpu);
            free_memory();
            return ERROR_NOT_ENOUGH_MEMORY;
        }

        let mut new_vcpu = vcpu;
        self.setup_vcpu_segments_32bit(&mut *new_vcpu, tib_addr as u64, paging);
        // Fresh registers: a reused vCPU must not carry the old thread's.
        let new_regs = super::vm_backend::GuestRegs {
            rip: pfn as u64, rsp: sp_addr as u64, rax: param as u64,
            rflags: INITIAL_RFLAGS, ..Default::default()
        };
        if new_vcpu.set_regs(&new_regs).is_err() {
            free_memory();
            return ERROR_MAX_THRDS_REACHED;
        }
        // A slot of the session's THREADS= limit (session_threads.rs).
        if self.shared.thread_slots.lock_or_recover().acquire_thread().is_err() {
            log::warn!("  [VCPU {}] DosCreateThread: THREADS= limit reached", vcpu_id);
            self.shared.threads.table.lock_or_recover().put_vcpu(new_vcpu);
            free_memory();
            return ERROR_MAX_THRDS_REACHED;
        }

        {
            let mut table = self.shared.threads.table.lock_or_recover();
            table.start(tid, ThreadMemory { stack: stack_base, tib: tib_addr, paging });
            if flags & CREATE_SUSPENDED != 0 {
                table.suspend(super::threads::tid_of_vcpu(vcpu_id), tid);
            }
        }
        if ptid_ptr != 0 { self.guest_write::<u32>(ptid_ptr, tid); }
        let shared_clone = Arc::clone(&self.shared);
        let vm_clone = Arc::clone(&self.vm);
        // The host thread is detached: DosWaitThread waits on the thread
        // table, and the vCPU goes back to the table when the thread ends.
        thread::spawn(move || {
            let loader = super::Loader { vm: vm_clone, shared: shared_clone };
            loader.run_vcpu(new_vcpu, tid, tib_addr as u64);
        });
        NO_ERROR
    }

    pub fn dos_query_h_type(&self, hfile: u32, ptype: u32, pattr: u32) -> u32 {
        // For the console (unredirected standard handles and duplicates of
        // them) report a character device with the console device attribute
        // word: bit 0 = stdin device, bit 1 = stdout device. Both bits are set
        // because the console (CON:) is simultaneously the input and output
        // device. Without these bits, QueryIsConsole(STDIN) returns false and
        // 4OS2 falls back to getchar() (DosRead), bypassing KbdCharIn. That
        // creates double backspace/echo processing: dos_read_stdin does it
        // once and egets does it again, producing garbled command-line input.
        let (htype, attr) = match self.handle_target(hfile) {
            Some(HandleTarget::Console(_)) => (HANDTYPE_DEVICE, CONSOLE_DEVICE_ATTR),
            Some(HandleTarget::Pipe) => (HANDTYPE_PIPE, 0),
            Some(HandleTarget::File) => (HANDTYPE_FILE, 0),
            None => return ERROR_INVALID_HANDLE,
        };
        if ptype != 0 { self.guest_write::<u32>(ptype, htype); }
        if pattr != 0 { self.guest_write::<u32>(pattr, attr); }
        0
    }

    /// DosCreatePipe (ordinal 239): an anonymous pipe, a host pipe whose
    /// descriptors are close-on-exec — a child process gets an end only as
    /// an inheritable handle (`inherit.rs`), so a reader sees the end of
    /// data once every write handle is closed.
    pub fn dos_create_pipe(&self, phf_read_ptr: u32, phf_write_ptr: u32, _size: u32) -> u32 {
        use super::managers::{PipeHandle, PipeObject};
        if phf_read_ptr == 0 || phf_write_ptr == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let Ok((r, w)) = std::io::pipe() else { return ERROR_NOT_ENOUGH_MEMORY };
        let f_read = File::from(std::os::fd::OwnedFd::from(r));
        let f_write = File::from(std::os::fd::OwnedFd::from(w));
        let h_read = self.add_pipe_handle(PipeHandle { obj: PipeObject::Host(Arc::new(f_read)), fh_state: OPEN_ACCESS_READONLY });
        let h_write = self.add_pipe_handle(PipeHandle { obj: PipeObject::Host(Arc::new(f_write)), fh_state: OPEN_ACCESS_WRITEONLY });
        if self.guest_write::<u32>(phf_read_ptr, h_read).is_none()
            || self.guest_write::<u32>(phf_write_ptr, h_write).is_none() {
            self.close_pipe_handle(h_read);
            self.close_pipe_handle(h_write);
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }

    pub fn dos_get_info_blocks(&self, vcpu: &dyn VcpuBackend, ptib: u32, ppib: u32) -> u32 {
        let fs_base = vcpu.get_sregs().unwrap().fs.base;
        if ptib != 0 { self.guest_write::<u32>(ptib, fs_base as u32); }
        if ppib != 0 { self.guest_write::<u32>(ppib, PIB_BASE); }
        0
    }

    pub fn dos_alloc_mem(&self, ppb: u32, cb: u32) -> u32 {
        debug!("DosAllocMem(ppb=0x{:08X}, cb=0x{:08X} [{}])", ppb, cb, cb);
        // Committed memory is zero (a reused block is cleared).
        match self.alloc_zeroed(cb) {
            Some(addr) => {
                debug!("  -> allocated at 0x{:08X}", addr);
                self.guest_write::<u32>(ppb, addr);
                0
            },
            None => 8,
        }
    }

    /// DosFreeMem (ordinal 304).
    pub fn dos_free_mem(&self, pb: u32) -> u32 {
        // A block of the shared arena is let go; it is gone once no
        // process of the session holds it (`shared_arena.rs`).
        if super::shared_arena::in_arena(pb) && let Some(r) = self.arena_free(pb) {
            return r;
        }
        let freed = self.shared.mem_mgr.lock_or_recover().free_block(pb);
        let Some(size) = freed else { return ERROR_INVALID_ADDRESS };
        // A shared block's name is free again.
        self.shared.shmem_mgr.lock_or_recover().remove(pb);
        // So are suballocation pools in it that were never unset.
        self.shared.suballoc.lock_or_recover().forget_range(pb, size);
        NO_ERROR
    }

    // ── Semaphores (`ipc.rs` holds the model) ──

    /// The semaphore name at `psz` (none for a NULL pointer).
    fn sem_name(&self, psz: u32) -> Option<String> {
        (psz != 0).then(|| self.read_guest_string(psz))
    }

    /// Write a new semaphore's handle to `ph`, or return the error.
    fn store_sem_handle(&self, created: Result<u32, u32>, ph: u32) -> u32 {
        match created {
            Ok(h) => {
                if self.guest_write::<u32>(ph, h).is_some() { NO_ERROR } else { ERROR_INVALID_PARAMETER }
            }
            Err(e) => e,
        }
    }

    /// Wait on `cvar` / `guard` until `ready` or `msec` ms
    /// (`SEM_INDEFINITE_WAIT`: forever) pass; polls the process end. While
    /// it blocks the thread counts among the semaphore's waiters
    /// (`ERROR_SEM_BUSY` on close); `SemaphoreManager::wake_all` wakes it.
    fn wait_sem<'a, T: Waiters>(&self, cvar: &std::sync::Condvar, mut guard: std::sync::MutexGuard<'a, T>,
                       msec: u32, mut ready: impl FnMut(&mut T) -> bool) -> Result<std::sync::MutexGuard<'a, T>, u32> {
        if ready(&mut guard) { return Ok(guard); }
        let deadline = sem_deadline(msec);
        *guard.waiters_mut() += 1;
        let result = loop {
            if self.interrupted() { break Err(ERROR_INTERRUPT); }
            if self.shutting_down() { break Err(ERROR_TIMEOUT); }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { break Err(ERROR_TIMEOUT); }
            guard = cvar.wait_timeout(guard, remaining.min(SEM_POLL))
                .unwrap_or_else(|e| e.into_inner()).0;
            if ready(&mut guard) { break Ok(()); }
        };
        let w = guard.waiters_mut();
        *w = w.saturating_sub(1);
        result.map(|()| guard)
    }

    /// The session's shared semaphores, when a semaphore with these flags
    /// and name belongs there (`DC_SEM_SHARED` or named; `shared_sem.rs`).
    fn shared_sems_for(&self, attr: u32, named: bool) -> Option<&super::shared_sem::SharedSems> {
        (attr & DC_SEM_SHARED != 0 || named).then(|| self.shared.shared_sems.get().map(|s| &**s)).flatten()
    }

    /// Create a shared semaphore: the name checked (also against this
    /// process's muxwait names), the handle stored at `ph`.
    fn create_shared_sem(&self, sems: &super::shared_sem::SharedSems, name: Option<String>, attr: u32,
                         state: super::shared_sem::State, ph: u32) -> u32 {
        let name = match name.map(|n| canonical_sem_name(&n)).transpose() {
            Ok(n) => n,
            Err(e) => return e,
        };
        if name.as_ref().is_some_and(|n| self.shared.sem_mgr.lock_or_recover().local_name_exists(n)) {
            return ERROR_DUPLICATE_NAME;
        }
        if self.guest_ptr(ph, 4).is_none() { return ERROR_INVALID_PARAMETER; }
        self.store_sem_handle(sems.create_sem(name, attr, state), ph)
    }

    /// Wait for shared semaphore `h` until `attempt` succeeds or `msec` ms
    /// pass, as `wait_sem` does: the thread counts among the semaphore's
    /// waiters (`ERROR_SEM_BUSY`), sleeps on the session's generation word
    /// and looks at the process end every `SEM_POLL`.
    fn wait_shared_sem<R>(&self, sems: &super::shared_sem::SharedSems, h: u32, msec: u32,
                          mut attempt: impl FnMut() -> Result<Option<R>, u32>) -> Result<R, u32> {
        let deadline = sem_deadline(msec);
        let mut waiting = None;
        loop {
            let seen = sems.generation();
            if let Some(r) = attempt()? { return Ok(r); }
            if self.interrupted() { return Err(ERROR_INTERRUPT); }
            if self.shutting_down() { return Err(ERROR_TIMEOUT); }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { return Err(ERROR_TIMEOUT); }
            waiting.get_or_insert_with(|| sems.waiting(h));
            sems.wait_change(seen, remaining.min(SEM_POLL));
        }
    }

    /// DosCreateEventSem (ordinal 324).
    pub fn dos_create_event_sem(&self, psz_name_ptr: u32, phev_ptr: u32, fl_attr: u32, f_state: u32) -> u32 {
        let name = self.sem_name(psz_name_ptr);
        if let Some(sems) = self.shared_sems_for(fl_attr, name.is_some()) {
            if fl_attr & !(DC_SEM_SHARED | DCE_POSTONE | DCE_AUTORESET) != 0 { return ERROR_INVALID_PARAMETER; }
            let state = super::shared_sem::State::Event { post_count: (f_state != 0) as u32 };
            return self.create_shared_sem(sems, name, fl_attr, state, phev_ptr);
        }
        let created = self.shared.sem_mgr.lock_or_recover().create_event(name.as_deref(), fl_attr, f_state != 0);
        self.store_sem_handle(created, phev_ptr)
    }

    /// DosCloseEventSem (ordinal 326).
    pub fn dos_close_event_sem(&self, hev: u32) -> u32 {
        if let Some(sems) = self.shared_handle(hev) { return sems.close(SemKind::Event, hev); }
        self.shared.sem_mgr.lock_or_recover().close(SemKind::Event, hev)
    }

    /// DosResetEventSem (ordinal 327): the post count goes to `*pulPostCount`
    /// and back to 0.
    pub fn dos_reset_event_sem(&self, hev: u32, pul_post_count: u32) -> u32 {
        if let Some(sems) = self.shared_handle(hev) {
            let (rc, count) = match sems.reset(hev) {
                Err(e) => return e,
                Ok(Ok(count)) => (NO_ERROR, count),
                Ok(Err(e)) => (e, 0),
            };
            if pul_post_count == 0 || self.guest_write::<u32>(pul_post_count, count).is_none() {
                return ERROR_INVALID_PARAMETER;
            }
            return rc;
        }
        let Some(sem_arc) = self.shared.sem_mgr.lock_or_recover().get_event(hev) else { return ERROR_INVALID_HANDLE };
        if pul_post_count == 0 { return ERROR_INVALID_PARAMETER; }
        let mut sem = sem_arc.0.lock_or_recover();
        let (rc, count) = match sem.reset() {
            Ok(count) => (NO_ERROR, count),
            Err(e) => (e, 0),
        };
        if self.guest_write::<u32>(pul_post_count, count).is_none() { return ERROR_INVALID_PARAMETER; }
        rc
    }

    /// DosPostEventSem (ordinal 328).
    pub fn dos_post_event_sem(&self, hev: u32) -> u32 {
        self.shared.sem_mgr.lock_or_recover().post_event(hev)
    }

    /// DosWaitEventSem (ordinal 329); a `DCE_POSTONE` / `DCE_AUTORESET`
    /// semaphore takes its post(s) when it releases the wait.
    pub fn dos_wait_event_sem(&self, hev: u32, msec: u32) -> u32 {
        if let Some(sems) = self.shared_handle(hev) {
            return match self.wait_shared_sem(sems, hev, msec, || sems.try_event(hev).map(|p| p.then_some(()))) {
                Ok(()) => NO_ERROR,
                Err(e) => e,
            };
        }
        let Some(sem_arc) = self.shared.sem_mgr.lock_or_recover().get_event(hev) else { return ERROR_INVALID_HANDLE };
        let (lock, cvar) = &*sem_arc;
        match self.wait_sem(cvar, lock.lock_or_recover(), msec, |s| s.posted()) {
            Ok(mut sem) => { sem.consume(); NO_ERROR }
            Err(e) => e,
        }
    }

    /// DosQueryEventSem (ordinal 330): the post count.
    pub fn dos_query_event_sem(&self, hev: u32, pul_post_count: u32) -> u32 {
        if let Some(sems) = self.shared_handle(hev) {
            let count = match sems.post_count(hev) { Ok(c) => c, Err(e) => return e };
            if pul_post_count == 0 { return ERROR_INVALID_PARAMETER; }
            return if self.guest_write::<u32>(pul_post_count, count).is_some() { NO_ERROR } else { ERROR_INVALID_PARAMETER };
        }
        let Some(sem_arc) = self.shared.sem_mgr.lock_or_recover().get_event(hev) else { return ERROR_INVALID_HANDLE };
        if pul_post_count == 0 { return ERROR_INVALID_PARAMETER; }
        let count = sem_arc.0.lock_or_recover().post_count;
        if self.guest_write::<u32>(pul_post_count, count).is_some() { NO_ERROR } else { ERROR_INVALID_PARAMETER }
    }

    /// DosCreateMutexSem (ordinal 331); created owned, it belongs to the caller.
    pub fn dos_create_mutex_sem(&self, vcpu_id: u32, psz_name_ptr: u32, phmtx_ptr: u32, fl_attr: u32, f_state: u32) -> u32 {
        let name = self.sem_name(psz_name_ptr);
        if let Some(sems) = self.shared_sems_for(fl_attr, name.is_some()) {
            let owner = (f_state != 0).then(|| super::shared_sem::Owner {
                proc: sems.me(), tid: super::threads::tid_of_vcpu(vcpu_id),
            });
            let state = super::shared_sem::State::Mutex { owner, count: owner.is_some() as u32, died: None };
            return self.create_shared_sem(sems, name, fl_attr, state, phmtx_ptr);
        }
        let owner = (f_state != 0).then_some(vcpu_id);
        let created = self.shared.sem_mgr.lock_or_recover().create_mutex(name.as_deref(), fl_attr, owner);
        self.store_sem_handle(created, phmtx_ptr)
    }

    /// DosCloseMutexSem (ordinal 333).
    pub fn dos_close_mutex_sem(&self, hmtx: u32) -> u32 {
        if let Some(sems) = self.shared_handle(hmtx) { return sems.close(SemKind::Mutex, hmtx); }
        self.shared.sem_mgr.lock_or_recover().close(SemKind::Mutex, hmtx)
    }

    /// DosRequestMutexSem (ordinal 334): `ERROR_SEM_OWNER_DIED` (with the
    /// mutex) when its last owner ended without releasing it.
    pub fn dos_request_mutex_sem(&self, vcpu_id: u32, hmtx: u32, msec: u32) -> u32 {
        if let Some(sems) = self.shared_handle(hmtx) {
            let tid = super::threads::tid_of_vcpu(vcpu_id);
            return match self.wait_shared_sem(sems, hmtx, msec, || sems.try_mutex(hmtx, tid)) {
                Ok(true) => ERROR_SEM_OWNER_DIED,
                Ok(false) => NO_ERROR,
                Err(e) => e,
            };
        }
        let Some(sem_arc) = self.shared.sem_mgr.lock_or_recover().get_mutex(hmtx) else { return ERROR_INVALID_HANDLE };
        let (lock, cvar) = &*sem_arc;
        match self.wait_sem(cvar, lock.lock_or_recover(), msec, |s| s.available_to(vcpu_id)) {
            Ok(mut sem) => if sem.acquire(vcpu_id) { ERROR_SEM_OWNER_DIED } else { NO_ERROR },
            Err(e) => e,
        }
    }

    /// DosReleaseMutexSem (ordinal 335).
    pub fn dos_release_mutex_sem(&self, vcpu_id: u32, hmtx: u32) -> u32 {
        if let Some(sems) = self.shared_handle(hmtx) {
            return sems.release(hmtx, super::threads::tid_of_vcpu(vcpu_id));
        }
        self.shared.sem_mgr.lock_or_recover().release_mutex(hmtx, vcpu_id)
    }

    /// DosQueryMutexSem (ordinal 336): owner process and thread and the
    /// request count (all 0 when unowned). An owner that ended without
    /// releasing it: its process and thread, count 0 (unverified), and
    /// `ERROR_SEM_OWNER_DIED`.
    pub fn dos_query_mutex_sem(&self, hmtx: u32, ppid: u32, ptid: u32, pul_count: u32) -> u32 {
        if let Some(sems) = self.shared_handle(hmtx) {
            let (pid, tid, count, rc) = match sems.query_mutex(hmtx) { Ok(q) => q, Err(e) => return e };
            if ppid == 0 || ptid == 0 || pul_count == 0 { return ERROR_INVALID_PARAMETER; }
            let written = self.guest_write::<u32>(ppid, pid).is_some()
                && self.guest_write::<u32>(ptid, tid).is_some()
                && self.guest_write::<u32>(pul_count, count).is_some();
            return if written { rc } else { ERROR_INVALID_PARAMETER };
        }
        let Some(sem_arc) = self.shared.sem_mgr.lock_or_recover().get_mutex(hmtx) else { return ERROR_INVALID_HANDLE };
        if ppid == 0 || ptid == 0 || pul_count == 0 { return ERROR_INVALID_PARAMETER; }
        let own_pid = self.shared.process_id();
        let (pid, tid, count, rc) = {
            let sem = sem_arc.0.lock_or_recover();
            match (sem.died_tid, sem.owner_vcpu) {
                (Some(tid), _) => (own_pid, tid, 0, ERROR_SEM_OWNER_DIED),
                (None, Some(v)) => (own_pid, super::threads::tid_of_vcpu(v), sem.request_count, NO_ERROR),
                (None, None) => (0, 0, 0, NO_ERROR),
            }
        };
        let written = self.guest_write::<u32>(ppid, pid).is_some()
            && self.guest_write::<u32>(ptid, tid).is_some()
            && self.guest_write::<u32>(pul_count, count).is_some();
        if written { rc } else { ERROR_INVALID_PARAMETER }
    }

    /// Read `count` SEMRECORDs at `ptr`.
    fn read_sem_records(&self, ptr: u32, count: u32) -> Option<Vec<MuxWaitRecord>> {
        (0..count).map(|i| {
            let at = ptr.checked_add(i.checked_mul(SEMRECORD_SIZE)?)?;
            Some(MuxWaitRecord {
                hsem: self.guest_read::<u32>(at)?,
                user: self.guest_read::<u32>(at.checked_add(4)?)?,
            })
        }).collect()
    }

    /// DosCreateMuxWaitSem (ordinal 337).
    pub fn dos_create_mux_wait_sem(&self, psz_name_ptr: u32, phmux_ptr: u32, count: u32, records_ptr: u32, fl_attr: u32) -> u32 {
        if count as usize > MAX_MUXWAIT_SEMS { return ERROR_TOO_MANY_SEMAPHORES; }
        let Some(records) = self.read_sem_records(records_ptr, count) else { return ERROR_INVALID_PARAMETER };
        let name = self.sem_name(psz_name_ptr);
        let created = self.shared.sem_mgr.lock_or_recover().create_mux(name.as_deref(), fl_attr, &records);
        self.store_sem_handle(created, phmux_ptr)
    }

    /// DosOpenMuxWaitSem (ordinal 338): by name, else the handle at `phmux`.
    pub fn dos_open_mux_wait_sem(&self, psz_name_ptr: u32, phmux_ptr: u32) -> u32 {
        self.open_sem(SemKind::MuxWait, psz_name_ptr, phmux_ptr)
    }

    /// DosOpenEventSem / DosOpenMutexSem / DosOpenMuxWaitSem: open by the
    /// name at `psz_name` or else by the handle at `ph`; the handle goes to `ph`.
    pub fn open_sem(&self, kind: SemKind, psz_name: u32, ph: u32) -> u32 {
        let name = self.sem_name(psz_name);
        let h = if name.is_some() { 0 } else {
            let Some(h) = self.guest_read::<u32>(ph) else { return ERROR_INVALID_PARAMETER };
            h
        };
        debug!("  open {:?} semaphore name={:?} handle={}", kind, name, h);
        if kind != SemKind::MuxWait && let Some(sems) = self.shared.shared_sems.get() {
            let canonical = match name.as_deref().map(canonical_sem_name).transpose() {
                Ok(n) => n,
                Err(e) => return e,
            };
            if super::shared_sem::is_shared(h) || canonical.is_some() {
                let opened = sems.open(kind, canonical.as_deref(), h);
                // A name of this process's own muxwait semaphores is not
                // the kind asked for either.
                return self.store_sem_handle(opened, ph);
            }
        }
        let opened = self.shared.sem_mgr.lock_or_recover().open(kind, name.as_deref(), h);
        self.store_sem_handle(opened, ph)
    }

    /// The session's shared semaphores when `h` is one of their handles.
    fn shared_handle(&self, h: u32) -> Option<&super::shared_sem::SharedSems> {
        if !super::shared_sem::is_shared(h) { return None; }
        self.shared.shared_sems.get().map(|s| &**s)
    }

    /// DosCloseMuxWaitSem (ordinal 339).
    pub fn dos_close_mux_wait_sem(&self, hmux: u32) -> u32 {
        self.shared.sem_mgr.lock_or_recover().close(SemKind::MuxWait, hmux)
    }

    /// DosWaitMuxWaitSem (ordinal 340): wait for any or all of the list's
    /// semaphores; `*pulUser` gets the user value of the record that ended
    /// the wait; `ERROR_SEM_OWNER_DIED` when a mutex it took had lost its
    /// owner. Blocks on the manager's `SemSignal` (ipc.rs), woken by every
    /// post, release, ended owner, close and list change, and by
    /// `SemaphoreManager::wake_all`; the process end is looked at every
    /// `SEM_POLL` as in `wait_sem`.
    pub fn dos_wait_mux_wait_sem(&self, vcpu_id: u32, hmux: u32, msec: u32, pul_user_ptr: u32) -> u32 {
        let deadline = sem_deadline(msec);
        let (mux, signal) = {
            let sm = self.shared.sem_mgr.lock_or_recover();
            (sm.get_mux(hmux), sm.signal.clone())
        };
        let Some(mux) = mux else { return ERROR_INVALID_HANDLE };
        let mut waiting = None;
        loop {
            // The generation first: a change after this look wakes the wait.
            let seen = signal.generation();
            let shared_seen = self.shared.shared_sems.get()
                .filter(|_| mux.lock_or_recover().records.iter().any(|r| super::shared_sem::is_shared(r.hsem)))
                .map(|s| (s.clone(), s.generation()));
            let ready = self.shared.sem_mgr.lock_or_recover().try_mux(hmux, vcpu_id);
            match ready {
                Err(e) => return e,
                Ok(Some((user, died))) => {
                    if pul_user_ptr != 0 {
                        self.guest_write::<u32>(pul_user_ptr, user);
                    }
                    return if died { ERROR_SEM_OWNER_DIED } else { NO_ERROR };
                }
                Ok(None) => {}
            }
            if self.interrupted() { return ERROR_INTERRUPT; }
            if self.shutting_down() { return ERROR_TIMEOUT; }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { return ERROR_TIMEOUT; }
            // Counted among the list's waiters while blocked (ERROR_SEM_BUSY).
            waiting.get_or_insert_with(|| MuxWaiting::new(mux.clone()));
            // A list with shared semaphores sleeps on the session's word
            // (local changes wake it too); others on the process's signal.
            match &shared_seen {
                Some((sems, seen)) => sems.wait_change(*seen, remaining.min(SEM_POLL)),
                None => signal.wait_change(seen, remaining.min(SEM_POLL)),
            }
        }
    }

    /// DosAddMuxWaitSem (ordinal 341).
    pub fn dos_add_mux_wait_sem(&self, hmux: u32, psem_record: u32) -> u32 {
        let Some(rec) = self.read_sem_records(psem_record, 1) else { return ERROR_INVALID_PARAMETER };
        self.shared.sem_mgr.lock_or_recover().add_mux_record(hmux, rec[0])
    }

    /// DosDeleteMuxWaitSem (ordinal 342).
    pub fn dos_delete_mux_wait_sem(&self, hmux: u32, hsem: u32) -> u32 {
        self.shared.sem_mgr.lock_or_recover().delete_mux_record(hmux, hsem)
    }

    /// DosQueryMuxWaitSem (ordinal 343): `*pcSemRec` records fit at
    /// `pSemRec`; too few → `ERROR_PARAM_TOO_SMALL` with the number needed.
    pub fn dos_query_mux_wait_sem(&self, hmux: u32, pc_sem_rec: u32, p_sem_rec: u32, pfl_attr: u32) -> u32 {
        let (records, attr) = match self.shared.sem_mgr.lock_or_recover().query_mux(hmux) {
            Ok(q) => q,
            Err(e) => return e,
        };
        let Some(room) = self.guest_read::<u32>(pc_sem_rec) else { return ERROR_INVALID_PARAMETER };
        let needed = records.len() as u32;
        if room < needed {
            self.guest_write::<u32>(pc_sem_rec, needed);
            return ERROR_PARAM_TOO_SMALL;
        }
        for (i, rec) in (0u32..).zip(&records) {
            let written = p_sem_rec.checked_add(i * SEMRECORD_SIZE).is_some_and(|at| {
                self.guest_write::<u32>(at, rec.hsem).is_some()
                    && at.checked_add(4).and_then(|u| self.guest_write::<u32>(u, rec.user)).is_some()
            });
            if !written { return ERROR_INVALID_PARAMETER; }
        }
        if self.guest_write::<u32>(pc_sem_rec, needed).is_none()
            || self.guest_write::<u32>(pfl_attr, attr).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }

    pub fn dos_create_queue(&self, phq_ptr: u32, attr: u32, psz_name_ptr: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let mut queue_mgr = self.shared.queue_mgr.lock_or_recover();
        let h = queue_mgr.create(name, attr);
        self.guest_write::<u32>(phq_ptr, h);
        0
    }

    pub fn dos_open_queue(&self, _ppid_ptr: u32, phq_ptr: u32, psz_name_ptr: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let queue_mgr = self.shared.queue_mgr.lock_or_recover();
        if let Some(h) = queue_mgr.find_by_name(&name) {
            self.guest_write::<u32>(phq_ptr, h);
            return 0;
        }
        343
    }

    pub fn dos_write_queue(&self, hq: u32, event: u32, len: u32, buf_ptr: u32, priority: u32) -> u32 {
        let queue_mgr = self.shared.queue_mgr.lock_or_recover();
        if let Some(q_arc) = queue_mgr.get(hq) {
            let mut q = q_arc.lock_or_recover();
            // Cap len to prevent OOM from untrusted guest-supplied sizes.
            let len = len.min(64 * 1024 * 1024);
            let mut data = vec![0u8; len as usize];
            if let Some(src) = self.guest_slice_mut(buf_ptr, len as usize) {
                data.copy_from_slice(src);
            }
            let id = q.next_id;
            q.next_id = q.next_id.wrapping_add(1).max(1);
            q.items.push_back(QueueEntry { data, event, priority, id });
            q.cond.notify_one();
            return 0;
        }
        337
    }

    #[allow(clippy::too_many_arguments)]
    /// DosReadQueue: take the first element, or (`elem` not 0) the one
    /// whose element code DosPeekQueue gave; with DCWW_WAIT wait for one,
    /// with DCWW_NOWAIT return ERROR_QUE_EMPTY at once.
    pub fn dos_read_queue(&self, hq: u32, preq_ptr: u32, pcb_ptr: u32, ppbuf_ptr: u32, elem: u32, wait: u32, pprio_ptr: u32, _hev: u32) -> u32 {
        let (q_arc, cond, cond_lock) = {
            let queue_mgr = self.shared.queue_mgr.lock_or_recover();
            if let Some(q_arc) = queue_mgr.get(hq) {
                let q = q_arc.lock_or_recover();
                let cond = Arc::clone(&q.cond);
                let cond_lock = Arc::clone(&q.cond_lock);
                drop(q);
                (q_arc, cond, cond_lock)
            } else { return 337; }
        };

        loop {
            if self.stop_or_restart() { return 342; }
            {
                let mut q = q_arc.lock_or_recover();
                let at = match elem {
                    0 => Some(0),
                    code => match q.items.iter().position(|e| e.id == code) {
                        Some(p) => Some(p),
                        None => return 333, // ERROR_QUE_ELEMENT_NOT_EXIST
                    },
                };
                if let Some(entry) = at.and_then(|p| q.items.remove(p)) {
                    let len = entry.data.len() as u32;
                    drop(q);
                    let mut mem_mgr = self.shared.mem_mgr.lock_or_recover();
                    if let Some(guest_addr) = mem_mgr.alloc(len) {
                        self.guest_write_bytes(guest_addr, &entry.data);
                        self.guest_write::<u32>(ppbuf_ptr, guest_addr);
                        self.guest_write::<u32>(pcb_ptr, len);
                        if preq_ptr != 0 {
                            self.guest_write::<u32>(preq_ptr + 4, entry.event);
                        }
                        if pprio_ptr != 0 {
                            self.guest_write::<u8>(pprio_ptr, entry.priority as u8);
                        }
                        return 0;
                    }
                    return 8;
                }
            }
            // DCWW_NOWAIT (1): no waiting.
            if wait != super::dos_misc::DCWW_WAIT { return 342; }
            let guard = cond_lock.lock_or_recover();
            let _ = cond.wait_timeout(guard, std::time::Duration::from_millis(100)).unwrap();
        }
    }

    pub fn dos_close_queue(&self, hq: u32) -> u32 {
        if self.shared.queue_mgr.lock_or_recover().close(hq) { 0 }
        else { 337 }
    }

    pub fn dos_purge_queue(&self, hq: u32) {
        let queue_mgr = self.shared.queue_mgr.lock_or_recover();
        if let Some(q_arc) = queue_mgr.get(hq) {
            let mut q = q_arc.lock_or_recover();
            q.items.clear();
        }
    }

    pub fn dos_query_queue(&self, hq: u32, pcb_ptr: u32) -> u32 {
        let queue_mgr = self.shared.queue_mgr.lock_or_recover();
        if let Some(q_arc) = queue_mgr.get(hq) {
            let q = q_arc.lock_or_recover();
            self.guest_write::<u32>(pcb_ptr, q.items.len() as u32);
            return 0;
        }
        337
    }

    /// DosGetResource: the resource of module `hmod` (0 = the executable).
    pub fn dos_get_resource(&self, hmod: u32, id_type: u32, id_name: u32, ppb: u32) -> u32 {
        let res_mgr = self.shared.resource_mgr.lock_or_recover();
        if let Some((guest_addr, _size)) = res_mgr.find(hmod, id_type as u16, id_name as u16) {
            self.guest_write::<u32>(ppb, guest_addr);
            0
        } else {
            6
        }
    }

    pub fn dos_free_resource(&self, _pb: u32) -> u32 {
        0
    }

    /// DosQueryResourceSize: of module `hmod` (0 = the executable).
    pub fn dos_query_resource_size(&self, hmod: u32, id_type: u32, id_name: u32, p_size: u32) -> u32 {
        let res_mgr = self.shared.resource_mgr.lock_or_recover();
        if let Some((_guest_addr, size)) = res_mgr.find(hmod, id_type as u16, id_name as u16) {
            self.guest_write::<u32>(p_size, size);
            0
        } else {
            6
        }
    }

    /// DosWaitThread (ordinal 349): wait for thread `*ptid` (0 = the next
    /// thread to end) to end. A thread that has already ended is reported
    /// at once, once; `DCWW_NOWAIT` does not wait. Waiting for the current
    /// thread or for thread 1 fails with `ERROR_INVALID_THREADID`.
    pub fn dos_wait_thread(&self, vcpu_id: u32, ptid_ptr: u32, option: u32) -> u32 {
        use super::threads::{tid_of_vcpu, WaitOutcome, DCWW_NOWAIT, DCWW_WAIT};
        let Some(tid) = self.guest_read::<u32>(ptid_ptr) else { return ERROR_INVALID_PARAMETER };
        if option != DCWW_WAIT && option != DCWW_NOWAIT { return ERROR_INVALID_PARAMETER; }
        let caller = tid_of_vcpu(vcpu_id);
        debug!("  [VCPU {}] DosWaitThread(tid={}, option={})", vcpu_id, tid, option);
        match self.shared.threads.wait_for(caller, tid, option == DCWW_NOWAIT,
                                           || self.shutting_down() || self.interrupted()) {
            Some(WaitOutcome::Ended(ended)) => {
                self.guest_write::<u32>(ptid_ptr, ended);
                NO_ERROR
            }
            Some(WaitOutcome::Running) => ERROR_THREAD_NOT_TERMINATED,
            Some(WaitOutcome::Invalid) => ERROR_INVALID_THREADID,
            // A signal for thread 1, or this thread was killed.
            None if self.interrupted() => ERROR_INTERRUPT,
            // The process is ending; this thread will not run on.
            None => ERROR_INVALID_THREADID,
        }
    }

    pub fn dos_set_rel_max_fh(&self, p_req_count: u32, p_cur_max_fh: u32) -> u32 {
        let req_count = self.guest_read::<i32>(p_req_count).unwrap_or(0);
        debug!("DosSetRelMaxFH(reqCount={}, pCurMaxFH=0x{:08X})", req_count, p_cur_max_fh);
        let cur_max: u32 = 256;
        if p_cur_max_fh != 0 {
            let _ = self.guest_write::<u32>(p_cur_max_fh, cur_max);
        }
        0
    }

    /// DosDupHandle (ordinal 260): duplicate `old_hf` into the handle at
    /// `*p_new_hf` — closing what that handle was — or, when it is
    /// `NEW_HANDLE_REQUEST` (0xFFFFFFFF), into a new handle written back.
    /// Works for files, pipes and the console alike, so a shell can redirect
    /// standard output to a file (`DosDupHandle(hFile, &1)`) and back to a
    /// saved duplicate of the console.
    pub fn dos_dup_handle(&self, old_hf: u32, p_new_hf: u32) -> u32 {
        debug!("DosDupHandle(old={}, pNew=0x{:08X})", old_hf, p_new_hf);
        let Some(new_hf_val) = self.guest_read::<u32>(p_new_hf) else { return ERROR_INVALID_PARAMETER };
        let requested = (new_hf_val != NEW_HANDLE_REQUEST).then_some(new_hf_val);
        let Some(target) = self.handle_target(old_hf) else { return ERROR_INVALID_HANDLE };
        if requested == Some(old_hf) {
            return NO_ERROR;
        }
        if let Some(n) = requested && n >= PIPE_HANDLE_BASE && target != HandleTarget::Pipe {
            // Numbers from PIPE_HANDLE_BASE on belong to the pipe table.
            return ERROR_INVALID_HANDLE;
        }
        let new_h = match target {
            HandleTarget::Pipe => {
                // The duplicate shares the pipe end and copies the handle
                // state ("ties all handle-specific information", CP
                // Reference); the end closes with its last handle.
                let Some(dup) = self.shared.handle_mgr.lock_or_recover().entry(old_hf).cloned() else {
                    return ERROR_INVALID_HANDLE;
                };
                match requested {
                    Some(n) => {
                        self.put_pipe_handle_at(n, dup);
                        n
                    }
                    None => self.add_pipe_handle(dup),
                }
            }
            HandleTarget::File | HandleTarget::Console(_) => {
                if let Some(n) = requested {
                    self.close_pipe_handle(n);
                }
                let mut dm = self.shared.drive_mgr.lock_or_recover();
                match target {
                    HandleTarget::Console(stream) => dm.dup_console(stream, requested),
                    _ => match dm.dup_file(old_hf, requested) {
                        Ok(h) => h,
                        Err(e) => return e.0,
                    },
                }
            }
        };
        if requested.is_none() {
            self.guest_write::<u32>(p_new_hf, new_h);
        }
        NO_ERROR
    }

    /// DosFlatToSel (ordinal 425): convert 32-bit flat address to 16:16 tiled selector:offset.
    ///
    /// Returns a packed 16:16 pointer: high 16 bits = tile selector, low 16 bits = offset.
    /// Tile selector = (TILED_SEL_START_INDEX + tile_index) * 8 where tile_index = flat >> 16.
    pub fn dos_flat_to_sel(&self, flat_addr: u32) -> u32 {
        let tile_index = flat_addr >> 16;
        let offset = flat_addr & 0xFFFF;
        let selector = (TILED_SEL_START_INDEX + tile_index) * 8;
        debug!("DosFlatToSel(0x{:08X}) -> sel=0x{:04X} off=0x{:04X}", flat_addr, selector, offset);
        (selector << 16) | offset
    }

    /// DosSelToFlat (ordinal 426): convert 16:16 tiled selector:offset to 32-bit flat address.
    ///
    /// The input is a packed 16:16 pointer: high 16 bits = selector, low 16 bits = offset.
    pub fn dos_sel_to_flat(&self, sel_off: u32) -> u32 {
        let selector = (sel_off >> 16) as u16;
        let offset = sel_off & 0xFFFF;
        let gdt_index = (selector / 8) as u32;
        let flat = if gdt_index >= TILED_CODE_START_INDEX {
            // Code tile selector → tile_index = gdt_index - TILED_CODE_START_INDEX
            (gdt_index - TILED_CODE_START_INDEX) * TILE_SIZE + offset
        } else if gdt_index >= TILED_SEL_START_INDEX {
            // Data tile selector → tile_index = gdt_index - TILED_SEL_START_INDEX
            (gdt_index - TILED_SEL_START_INDEX) * TILE_SIZE + offset
        } else {
            // Not a tile selector (e.g., null or standard descriptor) — return offset as-is
            offset
        };
        debug!("DosSelToFlat(sel=0x{:04X} off=0x{:04X}) -> 0x{:08X}", selector, offset, flat);
        flat
    }

    /// The codepage an NLS call works on: `COUNTRYCODE.codepage` (offset 4)
    /// when `pcc` is given and the field is not 0, else the process codepage
    /// ("If codepage is set to 0, ... the current process code page of the
    /// caller", CP Reference). `COUNTRYCODE.country` is not used: Warpine
    /// has one set of country data, from the host locale.
    pub fn nls_codepage(&self, pcc: u32) -> u32 {
        let given = if pcc != 0 { pcc.checked_add(4).and_then(|p| self.guest_read::<u32>(p)).unwrap_or(0) } else { 0 };
        if given != 0 { given } else { self.shared.active_codepage.load(Ordering::Relaxed) }
    }

    /// DosQueryDBCSEnv (NLS ordinal 6): the DBCS environmental vector.
    ///
    /// The vector is a series of `(first, last)` byte pairs representing the
    /// lead-byte ranges of the codepage (`nls_codepage`), terminated by
    /// `(0x00, 0x00)`; for SBCS codepages only the terminator. The 16-bit
    /// `DOS16GETDBCSEV` (NLS ordinal 4) is not implemented.
    ///
    /// A buffer that is too small gets "as much information as possible ...
    /// in the order in which the data would appear" (CP Reference); the call
    /// then returns `ERROR_NLS_TABLE_TRUNCATED` — Warpine's choice: the
    /// reference lists the code but does not say when it is returned.
    pub fn dos_query_dbcs_env(&self, cb: u32, pcc: u32, pbuf: u32) -> u32 {
        use crate::loader::locale::dbcs_lead_ranges;
        if pbuf == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let mut vec: Vec<u8> = dbcs_lead_ranges(self.nls_codepage(pcc))
            .iter()
            .flat_map(|&(first, last)| [first, last])
            .collect();
        vec.extend_from_slice(&[0, 0]);
        self.write_nls_table(cb, pbuf, &vec)
    }

    /// DosQueryCollate (NLS ordinal 8): the collating sequence table of the
    /// codepage (`nls_codepage`) — one sort weight for each byte 0x00..0xFF,
    /// 256 bytes ("A length value of 256 bytes is sufficient"). `*pcch` gets
    /// the number of bytes returned. The weights are Warpine's own
    /// (`codepage::collate_table`); a short buffer is handled as in
    /// [`dos_query_dbcs_env`](Self::dos_query_dbcs_env).
    pub fn dos_query_collate(&self, cb: u32, pcc: u32, pch: u32, pcch: u32) -> u32 {
        if pch == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let table = super::codepage::collate_table(self.nls_codepage(pcc));
        let rc = self.write_nls_table(cb, pch, &table);
        if pcch != 0 {
            let _ = self.guest_write::<u32>(pcch, cb.min(table.len() as u32));
        }
        rc
    }

    /// Write `data` into a caller's `cb`-byte NLS buffer: all of it, or as
    /// much as fits and `ERROR_NLS_TABLE_TRUNCATED`. A null or unwritable
    /// buffer fails with `ERROR_INVALID_PARAMETER` (not in the reference's
    /// list of codes: Warpine's choice).
    fn write_nls_table(&self, cb: u32, pbuf: u32, data: &[u8]) -> u32 {
        let n = (cb as usize).min(data.len());
        if self.guest_write_bytes(pbuf, &data[..n]).is_none() {
            return ERROR_INVALID_PARAMETER;
        }
        if n < data.len() { ERROR_NLS_TABLE_TRUNCATED } else { NO_ERROR }
    }

    pub fn dos_query_file_mode_16(&self, p_filename: u32, p_attr: u32) -> u32 {
        let filename = self.read_guest_string(p_filename);
        debug!("DosQFileMode('{}', pAttr=0x{:08X})", filename, p_attr);
        let dm = self.shared.drive_mgr.lock_or_recover();
        match dm.query_path_info(&filename, 1) {
            Ok(status) => {
                let mut attr: u16 = 0;
                if status.attributes.contains(FileAttribute::DIRECTORY) { attr |= 0x10; }
                if status.attributes.contains(FileAttribute::READONLY) { attr |= 0x01; }
                if p_attr != 0 {
                    let _ = self.guest_write::<u16>(p_attr, attr);
                }
                0
            }
            Err(_) => ERROR_FILE_NOT_FOUND,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Loader;
    use super::super::mutex_ext::MutexExt;
    use super::super::constants::*;
    use super::super::managers::PIPE_HANDLE_BASE;
    use std::sync::atomic::Ordering;

    /// DosQueryCurrentDir gives the directory as OS/2 writes it, with
    /// backslashes, and refuses a drive that is not mounted.
    #[test]
    fn query_current_dir_uses_backslashes_and_checks_the_drive() {
        let loader = Loader::new_mock();
        let drive = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(drive.path().join("SUB/DEEP")).unwrap();
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.mount(2, Box::new(super::super::vfs_hostdir::HostDirBackend::new(drive.path().to_path_buf()).unwrap()));
            dm.set_current_disk(3).unwrap();
            dm.set_current_dir(r"C:\SUB\DEEP").unwrap();
        }
        let (pcb, buf) = (0x3000u32, 0x3100u32);
        let query = |disk: u32| {
            loader.guest_write::<u32>(pcb, 64).unwrap();
            loader.dos_query_current_dir(disk, buf, pcb)
        };
        assert_eq!(query(0), NO_ERROR);
        assert_eq!(loader.read_guest_string(buf), r"SUB\DEEP");
        assert_eq!(query(3), NO_ERROR, "C: by number");
        assert_eq!(query(4), ERROR_INVALID_DRIVE, "D: is not mounted");
        assert_eq!(query(27), ERROR_INVALID_DRIVE);
        assert_eq!(query(300), ERROR_INVALID_DRIVE, "no wrap-around to a drive");
    }

    // ── DosFlatToSel / DosSelToFlat ──────────────────────────────────────────

    #[test]
    fn test_dos_flat_to_sel_tile_zero() {
        let loader = Loader::new_mock();
        // Flat 0x00001234 → tile 0 → selector = TILED_SEL_START_INDEX * 8 = 0x30, offset = 0x1234
        let result = loader.dos_flat_to_sel(0x00001234);
        let selector = (result >> 16) as u16;
        let offset   = (result & 0xFFFF) as u16;
        assert_eq!(selector, (TILED_SEL_START_INDEX * 8) as u16, "tile 0 selector mismatch");
        assert_eq!(offset, 0x1234);
    }

    #[test]
    fn test_dos_flat_to_sel_tile_one() {
        let loader = Loader::new_mock();
        // Flat 0x00015678 → tile 1 → selector = (6+1)*8 = 0x38, offset = 0x5678
        let result = loader.dos_flat_to_sel(0x00015678);
        let selector = (result >> 16) as u16;
        let offset   = (result & 0xFFFF) as u16;
        assert_eq!(selector, ((TILED_SEL_START_INDEX + 1) * 8) as u16);
        assert_eq!(offset, 0x5678);
    }

    #[test]
    fn test_dos_sel_to_flat_roundtrip() {
        let loader = Loader::new_mock();
        // Round-trip: flat → 16:16 → flat must recover the original address.
        for &flat in &[0x00001000u32, 0x00010000, 0x00051372, 0x0000FFFF, 0x00100000] {
            let packed = loader.dos_flat_to_sel(flat);
            let recovered = loader.dos_sel_to_flat(packed);
            assert_eq!(recovered, flat, "round-trip failed for 0x{:08X}", flat);
        }
    }

    #[test]
    fn test_dos_sel_to_flat_non_tile_selector() {
        let loader = Loader::new_mock();
        // Selector 0x00 (null) or 0x10 (standard data) with offset 0x1234
        // → gdt_index < TILED_SEL_START_INDEX → return offset only
        let packed = (0x0010u32 << 16) | 0x1234u32;
        let flat = loader.dos_sel_to_flat(packed);
        assert_eq!(flat, 0x1234);
    }

    #[test]
    fn test_dos_sel_to_flat_code_tile_selector() {
        let loader = Loader::new_mock();
        // Code tile for tile_index=1 → flat address 0x10000 + offset
        let code_tile_sel = (TILED_CODE_START_INDEX + 1) * 8; // GDT[4103]
        let packed = (code_tile_sel << 16) | 0x5678;
        let flat = loader.dos_sel_to_flat(packed);
        assert_eq!(flat, 0x15678, "code tile selector should map to flat 0x15678");
    }

    // ── DosAllocMem / DosFreeMem ─────────────────────────────────────────────

    #[test]
    fn test_dos_alloc_and_free_mem() {
        let loader = Loader::new_mock();
        let ppb = 0x2000u32;
        assert_eq!(loader.dos_alloc_mem(ppb, 4096), 0);
        let addr = loader.guest_read::<u32>(ppb).unwrap();
        assert_ne!(addr, 0);
        assert_eq!(loader.dos_free_mem(addr), 0);
    }

    #[test]
    fn test_dos_free_mem_invalid_address() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_free_mem(0xDEAD), 487); // ERROR_INVALID_ADDRESS
    }

    // ── DosQueryHType ────────────────────────────────────────────────────────

    #[test]
    fn test_dos_query_h_type_handles() {
        let loader = Loader::new_mock();
        let p_type = 0x2000u32;
        let p_attr = 0x2004u32;
        // Handles 0–2 (stdin/stdout/stderr) → type 1 (character device),
        // attr 0x0003 (stdin+stdout console device bits — makes QueryIsConsole return true)
        for hf in 0u32..3 {
            assert_eq!(loader.dos_query_h_type(hf, p_type, p_attr), 0);
            assert_eq!(loader.guest_read::<u32>(p_type), Some(1));
            assert_eq!(loader.guest_read::<u32>(p_attr), Some(0x0003));
        }
        // A handle that is not open
        assert_eq!(loader.dos_query_h_type(100, p_type, p_attr), ERROR_INVALID_HANDLE);
    }

    #[test]
    fn test_dos_query_h_type_null_pointers() {
        let loader = Loader::new_mock();
        // Null output pointers must not crash
        assert_eq!(loader.dos_query_h_type(0, 0, 0), 0);
    }

    // ── DosRead len=0 ────────────────────────────────────────────────────────

    #[test]
    fn test_dos_read_stdin_len_zero() {
        let loader = Loader::new_mock();
        let actual_ptr = 0x2000u32;
        let rc = loader.dos_read(0, 0x3000, 0, actual_ptr);
        assert_eq!(rc, 0);
        assert_eq!(loader.guest_read::<u32>(actual_ptr), Some(0));
    }

    // ── DosWrite ─────────────────────────────────────────────────────────────

    #[test]
    fn test_dos_write_to_sdl_console_updates_buffer() {
        let loader = Loader::new_mock();
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        loader.guest_write_bytes(0x3000, b"AB");
        let actual_ptr = 0x2000u32;
        let rc = loader.dos_write(1, 0x3000, 2, actual_ptr);
        assert_eq!(rc, 0);
        assert_eq!(loader.guest_read::<u32>(actual_ptr), Some(2));
        // Cursor should have advanced 2 columns
        let console = loader.shared.console_mgr.lock_or_recover();
        assert_eq!(console.cursor_col, 2);
    }

    #[test]
    fn test_dos_write_len_zero() {
        let loader = Loader::new_mock();
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        let actual_ptr = 0x2000u32;
        let rc = loader.dos_write(1, 0x3000, 0, actual_ptr);
        assert_eq!(rc, 0);
        assert_eq!(loader.guest_read::<u32>(actual_ptr), Some(0));
    }

    // ── DosSetRelMaxFH ───────────────────────────────────────────────────────

    #[test]
    fn test_dos_set_rel_max_fh() {
        let loader = Loader::new_mock();
        let p_req = 0x2000u32;
        let p_cur = 0x2004u32;
        loader.guest_write::<i32>(p_req, 0).unwrap(); // no change requested
        assert_eq!(loader.dos_set_rel_max_fh(p_req, p_cur), 0);
        assert_eq!(loader.guest_read::<u32>(p_cur), Some(256));
    }

    // ── Event semaphores ─────────────────────────────────────────────────────

    #[test]
    fn test_dos_event_sem_create_close() {
        let loader = Loader::new_mock();
        let phev = 0x2000u32;
        assert_eq!(loader.dos_create_event_sem(0, phev, 0, 0), 0);
        let hev = loader.guest_read::<u32>(phev).unwrap();
        assert_ne!(hev, 0);
        assert_eq!(loader.dos_close_event_sem(hev), 0);
    }

    #[test]
    fn test_dos_event_sem_wait_presignaled() {
        let loader = Loader::new_mock();
        let phev = 0x2000u32;
        // f_state=1: create already-posted semaphore
        loader.dos_create_event_sem(0, phev, 0, 1);
        let hev = loader.guest_read::<u32>(phev).unwrap();
        // Wait 0 ms — already posted, must return immediately
        assert_eq!(loader.dos_wait_event_sem(hev, 0), 0);
    }

    #[test]
    fn test_dos_event_sem_post_then_wait() {
        let loader = Loader::new_mock();
        let phev = 0x2000u32;
        loader.dos_create_event_sem(0, phev, 0, 0);
        let hev = loader.guest_read::<u32>(phev).unwrap();
        assert_eq!(loader.dos_post_event_sem(hev), 0);
        assert_eq!(loader.dos_wait_event_sem(hev, 0), 0);
    }

    #[test]
    fn test_dos_event_sem_wait_timeout() {
        let loader = Loader::new_mock();
        let phev = 0x2000u32;
        loader.dos_create_event_sem(0, phev, 0, 0); // not posted
        let hev = loader.guest_read::<u32>(phev).unwrap();
        // 0 ms timeout on unposted semaphore → 640 (ERROR_TIMEOUT)
        assert_eq!(loader.dos_wait_event_sem(hev, 0), 640);
    }

    #[test]
    fn test_dos_event_sem_double_post_returns_already_posted() {
        let loader = Loader::new_mock();
        let phev = 0x2000u32;
        loader.dos_create_event_sem(0, phev, 0, 0);
        let hev = loader.guest_read::<u32>(phev).unwrap();
        assert_eq!(loader.dos_post_event_sem(hev), 0);
        assert_eq!(loader.dos_post_event_sem(hev), 299); // ERROR_ALREADY_POSTED
    }

    #[test]
    fn test_dos_event_sem_invalid_handle() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_close_event_sem(9999), 6); // ERROR_INVALID_HANDLE
        assert_eq!(loader.dos_post_event_sem(9999), 6);
        assert_eq!(loader.dos_wait_event_sem(9999, 0), 6);
    }

    // ── Mutex semaphores ─────────────────────────────────────────────────────

    #[test]
    fn test_dos_mutex_sem_create_request_release_close() {
        let loader = Loader::new_mock();
        let phmtx = 0x2000u32;
        // Create unowned mutex (f_state=0)
        assert_eq!(loader.dos_create_mutex_sem(0, 0, phmtx, 0, 0), 0);
        let hmtx = loader.guest_read::<u32>(phmtx).unwrap();
        assert_ne!(hmtx, 0);
        // Request from tid=1 with 0 ms timeout — unowned, must succeed
        assert_eq!(loader.dos_request_mutex_sem(1, hmtx, 0), 0);
        // Release
        assert_eq!(loader.dos_release_mutex_sem(1, hmtx), 0);
        // Close
        assert_eq!(loader.dos_close_mutex_sem(hmtx), 0);
    }

    #[test]
    fn test_dos_mutex_sem_invalid_handle() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_close_mutex_sem(9999), 6); // ERROR_INVALID_HANDLE
    }

    // ── Post counts, mutex queries, names, muxwait lists ─────────────────────

    const PH: u32 = 0x2000;
    const PUL: u32 = 0x2004;
    const PNAME: u32 = 0x2100;
    const PREC: u32 = 0x3000;

    fn new_event(loader: &Loader, posted: bool) -> u32 {
        assert_eq!(loader.dos_create_event_sem(0, PH, 0, posted as u32), NO_ERROR);
        loader.guest_read::<u32>(PH).unwrap()
    }

    fn new_mutex(loader: &Loader, owner: Option<u32>) -> u32 {
        assert_eq!(loader.dos_create_mutex_sem(owner.unwrap_or(0), 0, PH, 0, owner.is_some() as u32), NO_ERROR);
        loader.guest_read::<u32>(PH).unwrap()
    }

    fn put_name(loader: &Loader, name: &str) -> u32 {
        loader.guest_write_bytes(PNAME, name.as_bytes()).unwrap();
        loader.guest_write::<u8>(PNAME + name.len() as u32, 0).unwrap();
        PNAME
    }

    fn put_records(loader: &Loader, recs: &[(u32, u32)]) -> u32 {
        for (i, &(h, user)) in recs.iter().enumerate() {
            loader.guest_write::<u32>(PREC + 8 * i as u32, h).unwrap();
            loader.guest_write::<u32>(PREC + 8 * i as u32 + 4, user).unwrap();
        }
        PREC
    }

    fn new_mux(loader: &Loader, recs: &[(u32, u32)], attr: u32) -> u32 {
        let p = put_records(loader, recs);
        assert_eq!(loader.dos_create_mux_wait_sem(0, PH, recs.len() as u32, p, attr), NO_ERROR);
        loader.guest_read::<u32>(PH).unwrap()
    }

    #[test]
    fn test_event_post_count_reset_and_query() {
        let loader = Loader::new_mock();
        let hev = new_event(&loader, false);
        assert_eq!(loader.dos_query_event_sem(hev, PUL), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(0));
        assert_eq!(loader.dos_reset_event_sem(hev, PUL), ERROR_ALREADY_RESET);
        assert_eq!(loader.dos_post_event_sem(hev), NO_ERROR);
        assert_eq!(loader.dos_post_event_sem(hev), ERROR_ALREADY_POSTED, "still counted");
        assert_eq!(loader.dos_post_event_sem(hev), ERROR_ALREADY_POSTED);
        assert_eq!(loader.dos_query_event_sem(hev, PUL), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(3));
        assert_eq!(loader.dos_reset_event_sem(hev, PUL), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(3), "the count before the reset");
        assert_eq!(loader.dos_reset_event_sem(hev, PUL), ERROR_ALREADY_RESET);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(0));
        assert_eq!(loader.dos_wait_event_sem(hev, 0), ERROR_TIMEOUT);
        // Created posted: one post.
        let posted = new_event(&loader, true);
        assert_eq!(loader.dos_query_event_sem(posted, PUL), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(1));
        // Bad handles and pointers.
        assert_eq!(loader.dos_reset_event_sem(9999, PUL), ERROR_INVALID_HANDLE);
        assert_eq!(loader.dos_query_event_sem(9999, PUL), ERROR_INVALID_HANDLE);
        assert_eq!(loader.dos_query_event_sem(hev, 0), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.dos_reset_event_sem(posted, 0), ERROR_INVALID_PARAMETER);
        // A mutex is not an event.
        let hmtx = new_mutex(&loader, None);
        assert_eq!(loader.dos_query_event_sem(hmtx, PUL), ERROR_INVALID_HANDLE);
    }

    #[test]
    fn test_event_post_count_limit() {
        let loader = Loader::new_mock();
        let hev = new_event(&loader, false);
        let sem = loader.shared.sem_mgr.lock_or_recover().get_event(hev).unwrap();
        sem.0.lock_or_recover().post_count = MAX_EVENT_POSTS;
        assert_eq!(loader.dos_post_event_sem(hev), ERROR_TOO_MANY_POSTS);
        assert_eq!(sem.0.lock_or_recover().post_count, MAX_EVENT_POSTS);
    }

    #[test]
    fn test_query_mutex_reports_owner_and_requests() {
        let loader = Loader::new_mock();
        const PPID: u32 = 0x2010;
        const PTID: u32 = 0x2014;
        let hmtx = new_mutex(&loader, None);
        assert_eq!(loader.dos_query_mutex_sem(hmtx, PPID, PTID, PUL), NO_ERROR);
        assert_eq!([PPID, PTID, PUL].map(|p| loader.guest_read::<u32>(p).unwrap()), [0, 0, 0]);
        // vCPU 0 is thread 1.
        assert_eq!(loader.dos_request_mutex_sem(0, hmtx, 0), NO_ERROR);
        assert_eq!(loader.dos_request_mutex_sem(0, hmtx, 0), NO_ERROR);
        assert_eq!(loader.dos_query_mutex_sem(hmtx, PPID, PTID, PUL), NO_ERROR);
        assert_eq!([PPID, PTID, PUL].map(|p| loader.guest_read::<u32>(p).unwrap()), [PROCESS_ID, 1, 2]);
        assert_eq!(loader.dos_request_mutex_sem(3, hmtx, 0), ERROR_TIMEOUT);
        assert_eq!(loader.dos_release_mutex_sem(3, hmtx), ERROR_NOT_OWNER);
        assert_eq!(loader.dos_release_mutex_sem(0, hmtx), NO_ERROR);
        assert_eq!(loader.dos_release_mutex_sem(0, hmtx), NO_ERROR);
        assert_eq!(loader.dos_release_mutex_sem(0, hmtx), ERROR_NOT_OWNER);
        // Created owned: owned by the creating thread.
        let owned = new_mutex(&loader, Some(3));
        assert_eq!(loader.dos_query_mutex_sem(owned, PPID, PTID, PUL), NO_ERROR);
        assert_eq!([PPID, PTID, PUL].map(|p| loader.guest_read::<u32>(p).unwrap()), [PROCESS_ID, 3, 1]);
        assert_eq!(loader.dos_request_mutex_sem(0, owned, 0), ERROR_TIMEOUT);
        assert_eq!(loader.dos_query_mutex_sem(9999, PPID, PTID, PUL), ERROR_INVALID_HANDLE);
        assert_eq!(loader.dos_query_mutex_sem(owned, 0, PTID, PUL), ERROR_INVALID_PARAMETER);
    }

    #[test]
    fn test_named_semaphores_open_and_close_by_use() {
        let loader = Loader::new_mock();
        let name = put_name(&loader, "\\SEM32\\WARPINE\\EV");
        assert_eq!(loader.dos_create_event_sem(name, PH, 0, 0), NO_ERROR);
        let hev = loader.guest_read::<u32>(PH).unwrap();
        assert_eq!(loader.dos_create_mutex_sem(0, name, PH, 0, 0), ERROR_DUPLICATE_NAME);
        let bad = put_name(&loader, "\\PIPE\\X");
        assert_eq!(loader.dos_create_event_sem(bad, PH, 0, 0), ERROR_INVALID_NAME);
        // Open by name (any case) and by handle: three uses.
        let lower = put_name(&loader, "\\sem32\\warpine\\ev");
        loader.guest_write::<u32>(PH, 0).unwrap();
        assert_eq!(loader.dos_open_event_sem(lower, PH), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PH), Some(hev));
        assert_eq!(loader.dos_open_event_sem(0, PH), NO_ERROR);
        assert_eq!(loader.dos_open_mutex_sem(lower, PH), ERROR_SEM_NOT_FOUND, "not a mutex");
        let unknown = put_name(&loader, "\\SEM32\\NONE");
        assert_eq!(loader.dos_open_event_sem(unknown, PH), ERROR_SEM_NOT_FOUND);
        for _ in 0..3 {
            assert!(matches!(loader.dos_post_event_sem(hev), NO_ERROR | ERROR_ALREADY_POSTED));
            assert_eq!(loader.dos_close_event_sem(hev), NO_ERROR);
        }
        assert_eq!(loader.dos_post_event_sem(hev), ERROR_INVALID_HANDLE, "the last close deletes it");
    }

    #[test]
    fn test_mux_records_checked_on_create_and_add() {
        let loader = Loader::new_mock();
        let (e1, e2) = (new_event(&loader, false), new_event(&loader, false));
        let m1 = new_mutex(&loader, None);
        let p = put_records(&loader, &[(e1, 1), (m1, 2)]);
        assert_eq!(loader.dos_create_mux_wait_sem(0, PH, 2, p, DCMW_WAIT_ANY), ERROR_WRONG_TYPE);
        let p = put_records(&loader, &[(e1, 1), (e1, 2)]);
        assert_eq!(loader.dos_create_mux_wait_sem(0, PH, 2, p, DCMW_WAIT_ANY), ERROR_DUPLICATE_HANDLE);
        let p = put_records(&loader, &[(9999, 1)]);
        assert_eq!(loader.dos_create_mux_wait_sem(0, PH, 1, p, DCMW_WAIT_ANY), ERROR_INVALID_HANDLE);
        assert_eq!(loader.dos_create_mux_wait_sem(0, PH, MAX_MUXWAIT_SEMS as u32 + 1, PREC, DCMW_WAIT_ANY),
                   ERROR_TOO_MANY_SEMAPHORES);
        let p = put_records(&loader, &[(e1, 1)]);
        for attr in [0, DCMW_WAIT_ANY | DCMW_WAIT_ALL, DCMW_WAIT_ANY | 0x100] {
            assert_eq!(loader.dos_create_mux_wait_sem(0, PH, 1, p, attr), ERROR_INVALID_PARAMETER, "{attr:#x}");
        }
        let hmux = new_mux(&loader, &[(e1, 1)], DCMW_WAIT_ANY);
        let rec = |h: u32, user: u32| put_records(&loader, &[(h, user)]);
        assert_eq!(loader.dos_add_mux_wait_sem(hmux, rec(e1, 5)), ERROR_DUPLICATE_HANDLE);
        assert_eq!(loader.dos_add_mux_wait_sem(hmux, rec(m1, 5)), ERROR_WRONG_TYPE);
        assert_eq!(loader.dos_add_mux_wait_sem(hmux, rec(hmux, 5)), ERROR_WRONG_TYPE, "no muxwait in a list");
        assert_eq!(loader.dos_add_mux_wait_sem(hmux, rec(9999, 5)), ERROR_INVALID_HANDLE);
        assert_eq!(loader.dos_add_mux_wait_sem(9999, rec(e2, 5)), ERROR_INVALID_HANDLE);
        assert_eq!(loader.dos_add_mux_wait_sem(hmux, rec(e2, 2)), NO_ERROR);
        for _ in 2..MAX_MUXWAIT_SEMS {
            let e = new_event(&loader, false);
            assert_eq!(loader.dos_add_mux_wait_sem(hmux, rec(e, 0)), NO_ERROR);
        }
        let extra = new_event(&loader, false);
        assert_eq!(loader.dos_add_mux_wait_sem(hmux, rec(extra, 0)), ERROR_TOO_MANY_SEMAPHORES);
    }

    #[test]
    fn test_query_mux_wait_sem() {
        let loader = Loader::new_mock();
        const PC: u32 = 0x2020;
        const PATTR: u32 = 0x2024;
        const PBUF: u32 = 0x4000;
        let (e1, e2) = (new_event(&loader, false), new_event(&loader, false));
        let hmux = new_mux(&loader, &[(e1, 10), (e2, 20)], DCMW_WAIT_ALL);
        loader.guest_write::<u32>(PC, 1).unwrap();
        loader.guest_write::<u32>(PBUF, 0xDEAD).unwrap();
        assert_eq!(loader.dos_query_mux_wait_sem(hmux, PC, PBUF, PATTR), ERROR_PARAM_TOO_SMALL);
        assert_eq!(loader.guest_read::<u32>(PC), Some(2), "the number needed");
        assert_eq!(loader.guest_read::<u32>(PBUF), Some(0xDEAD), "nothing written");
        loader.guest_write::<u32>(PC, 8).unwrap();
        assert_eq!(loader.dos_query_mux_wait_sem(hmux, PC, PBUF, PATTR), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PC), Some(2));
        let got: Vec<u32> = (0..4).map(|i| loader.guest_read::<u32>(PBUF + 4 * i).unwrap()).collect();
        assert_eq!(got, vec![e1, 10, e2, 20]);
        assert_eq!(loader.guest_read::<u32>(PATTR), Some(DCMW_WAIT_ALL));
        assert_eq!(loader.dos_query_mux_wait_sem(e1, PC, PBUF, PATTR), ERROR_INVALID_HANDLE);
        // A named muxwait semaphore is shared; it opens by name.
        let name = put_name(&loader, "\\SEM32\\MUX");
        let p = put_records(&loader, &[(e1, 1)]);
        assert_eq!(loader.dos_create_mux_wait_sem(name, PH, 1, p, DCMW_WAIT_ANY), NO_ERROR);
        let named = loader.guest_read::<u32>(PH).unwrap();
        loader.guest_write::<u32>(PC, 8).unwrap();
        assert_eq!(loader.dos_query_mux_wait_sem(named, PC, PBUF, PATTR), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PATTR), Some(DCMW_WAIT_ANY | DC_SEM_SHARED));
        loader.guest_write::<u32>(PH, 0).unwrap();
        assert_eq!(loader.dos_open_mux_wait_sem(name, PH), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PH), Some(named));
        assert_eq!(loader.dos_open_mux_wait_sem(0, PH), NO_ERROR);
        loader.guest_write::<u32>(PH, e1).unwrap();
        assert_eq!(loader.dos_open_mux_wait_sem(0, PH), ERROR_INVALID_HANDLE, "an event is not a muxwait");
        for _ in 0..3 {
            assert_eq!(loader.dos_close_mux_wait_sem(named), NO_ERROR);
        }
        assert_eq!(loader.dos_close_mux_wait_sem(named), ERROR_INVALID_HANDLE);
    }

    #[test]
    fn test_delete_mux_wait_sem() {
        let loader = Loader::new_mock();
        let e1 = new_event(&loader, false);
        let m1 = new_mutex(&loader, None);
        let hmux = new_mux(&loader, &[(e1, 1)], DCMW_WAIT_ANY);
        assert_eq!(loader.dos_delete_mux_wait_sem(hmux, m1), ERROR_INVALID_HANDLE, "not in the list");
        assert_eq!(loader.dos_delete_mux_wait_sem(hmux, e1), NO_ERROR);
        assert_eq!(loader.dos_delete_mux_wait_sem(hmux, e1), ERROR_EMPTY_MUXWAIT);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, hmux, 0, PUL), ERROR_EMPTY_MUXWAIT);
        // Empty again: the list takes the other kind.
        let p = put_records(&loader, &[(m1, 7)]);
        assert_eq!(loader.dos_add_mux_wait_sem(hmux, p), NO_ERROR);
        assert_eq!(loader.dos_delete_mux_wait_sem(9999, m1), ERROR_INVALID_HANDLE);
    }

    #[test]
    fn test_wait_mux_on_events() {
        let loader = Loader::new_mock();
        let (e1, e2) = (new_event(&loader, false), new_event(&loader, false));
        let any = new_mux(&loader, &[(e1, 11), (e2, 22)], DCMW_WAIT_ANY);
        let all = new_mux(&loader, &[(e1, 11), (e2, 22)], DCMW_WAIT_ALL);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, any, 0, PUL), ERROR_TIMEOUT);
        assert_eq!(loader.dos_post_event_sem(e2), NO_ERROR);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, any, 0, PUL), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(22));
        assert_eq!(loader.dos_wait_mux_wait_sem(0, all, 0, PUL), ERROR_TIMEOUT);
        assert_eq!(loader.dos_post_event_sem(e1), NO_ERROR);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, all, 0, PUL), NO_ERROR);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, 9999, 0, PUL), ERROR_INVALID_HANDLE);
        // A semaphore of the list closed meanwhile.
        assert_eq!(loader.dos_close_event_sem(e1), NO_ERROR);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, all, 0, PUL), ERROR_INVALID_HANDLE);
    }

    #[test]
    fn test_wait_mux_takes_the_mutexes() {
        let loader = Loader::new_mock();
        let (m1, m2) = (new_mutex(&loader, Some(3)), new_mutex(&loader, None));
        let any = new_mux(&loader, &[(m1, 1), (m2, 2)], DCMW_WAIT_ANY);
        let all = new_mux(&loader, &[(m1, 1), (m2, 2)], DCMW_WAIT_ALL);
        // m1 belongs to thread 3: waiting for any takes m2.
        assert_eq!(loader.dos_wait_mux_wait_sem(0, any, 0, PUL), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(2));
        let owner = |h: u32| loader.shared.sem_mgr.lock_or_recover().get_mutex(h).unwrap()
            .0.lock_or_recover().owner_vcpu;
        assert_eq!(owner(m2), Some(0));
        assert_eq!(loader.dos_wait_mux_wait_sem(0, all, 0, PUL), ERROR_TIMEOUT);
        assert_eq!(owner(m1), Some(3), "nothing taken by a wait that did not end");
        assert_eq!(loader.dos_release_mutex_sem(3, m1), NO_ERROR);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, all, 0, PUL), NO_ERROR);
        assert_eq!((owner(m1), owner(m2)), (Some(0), Some(0)));
        assert_eq!(loader.dos_release_mutex_sem(0, m2), NO_ERROR);
        assert_eq!(loader.dos_release_mutex_sem(0, m2), NO_ERROR, "taken twice: by both waits");
        assert_eq!(loader.dos_release_mutex_sem(0, m2), ERROR_NOT_OWNER);
    }

    /// Wait until `h` (of `kind`) has `n` blocked threads.
    fn wait_for_waiters(loader: &Loader, kind: super::SemKind, h: u32, n: u32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let w = {
                let sm = loader.shared.sem_mgr.lock_or_recover();
                match kind {
                    super::SemKind::Event => sm.get_event(h).unwrap().0.lock_or_recover().waiters,
                    super::SemKind::Mutex => sm.get_mutex(h).unwrap().0.lock_or_recover().waiters,
                    super::SemKind::MuxWait => sm.get_mux(h).unwrap().lock_or_recover().waiters,
                }
            };
            if w == n { return; }
            assert!(std::time::Instant::now() < deadline, "no waiter came");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn test_event_postone_and_autoreset() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_create_event_sem(0, PH, 0x0002, 0), ERROR_INVALID_PARAMETER);
        assert_eq!(loader.dos_create_event_sem(0, PH, DCE_POSTONE | DC_SEM_SHARED, 0), NO_ERROR);
        let one = loader.guest_read::<u32>(PH).unwrap();
        assert_eq!(loader.dos_create_event_sem(0, PH, DCE_AUTORESET, 0), NO_ERROR);
        let auto = loader.guest_read::<u32>(PH).unwrap();
        let count = |h: u32| { loader.dos_query_event_sem(h, PUL); loader.guest_read::<u32>(PUL).unwrap() };
        // DCE_POSTONE: each post releases one wait.
        assert_eq!(loader.dos_post_event_sem(one), NO_ERROR);
        assert_eq!(loader.dos_post_event_sem(one), ERROR_ALREADY_POSTED);
        assert_eq!(loader.dos_wait_event_sem(one, 0), NO_ERROR);
        assert_eq!(count(one), 1);
        assert_eq!(loader.dos_wait_event_sem(one, 0), NO_ERROR);
        assert_eq!(loader.dos_wait_event_sem(one, 0), ERROR_TIMEOUT);
        // DCE_AUTORESET: a released wait resets it.
        loader.dos_post_event_sem(auto);
        loader.dos_post_event_sem(auto);
        assert_eq!(loader.dos_wait_event_sem(auto, 0), NO_ERROR);
        assert_eq!(count(auto), 0);
        assert_eq!(loader.dos_wait_event_sem(auto, 0), ERROR_TIMEOUT);
        // A plain event stays posted.
        let plain = new_event(&loader, true);
        assert_eq!(loader.dos_wait_event_sem(plain, 0), NO_ERROR);
        assert_eq!(count(plain), 1);
        // In a muxwait list the event that ends the wait takes its post.
        let any = new_mux(&loader, &[(one, 1), (auto, 2)], DCMW_WAIT_ANY);
        loader.dos_post_event_sem(auto);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, any, 0, PUL), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(2));
        assert_eq!(loader.dos_wait_mux_wait_sem(0, any, 0, PUL), ERROR_TIMEOUT);
    }

    #[test]
    fn test_mutex_owner_died() {
        let loader = Loader::new_mock();
        let m = new_mutex(&loader, Some(3));
        assert_eq!(loader.dos_request_mutex_sem(3, m, 0), NO_ERROR);
        assert_eq!(loader.shared.sem_mgr.lock_or_recover().owner_ended(4, 4), 0, "not the owner");
        assert_eq!(loader.shared.sem_mgr.lock_or_recover().owner_ended(3, 3), 1);
        const PPID: u32 = 0x2010;
        const PTID: u32 = 0x2014;
        assert_eq!(loader.dos_query_mutex_sem(m, PPID, PTID, PUL), ERROR_SEM_OWNER_DIED);
        assert_eq!(loader.guest_read::<u32>(PTID), Some(3), "the dead owner");
        assert_eq!(loader.guest_read::<u32>(PUL), Some(0));
        // A new thread on the dead owner's vCPU does not own it.
        assert_eq!(loader.dos_release_mutex_sem(3, m), ERROR_NOT_OWNER);
        // The next request gets it, once with ERROR_SEM_OWNER_DIED.
        assert_eq!(loader.dos_request_mutex_sem(5, m, 0), ERROR_SEM_OWNER_DIED);
        assert_eq!(loader.dos_query_mutex_sem(m, PPID, PTID, PUL), NO_ERROR);
        assert_eq!((loader.guest_read::<u32>(PTID), loader.guest_read::<u32>(PUL)), (Some(5), Some(1)));
        assert_eq!(loader.dos_request_mutex_sem(5, m, 0), NO_ERROR);
        assert_eq!(loader.dos_release_mutex_sem(5, m), NO_ERROR);
        assert_eq!(loader.dos_release_mutex_sem(5, m), NO_ERROR);
        // Muxwait lists of mutexes report it too.
        let (m1, m2) = (new_mutex(&loader, Some(6)), new_mutex(&loader, Some(7)));
        let all = new_mux(&loader, &[(m1, 1), (m2, 2)], DCMW_WAIT_ALL);
        loader.shared.sem_mgr.lock_or_recover().owner_ended(6, 6);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, all, 0, PUL), ERROR_TIMEOUT, "m2 still owned");
        assert_eq!(loader.dos_release_mutex_sem(7, m2), NO_ERROR);
        assert_eq!(loader.dos_wait_mux_wait_sem(0, all, 0, PUL), ERROR_SEM_OWNER_DIED);
        assert_eq!(loader.guest_read::<u32>(PUL), Some(2));
        assert_eq!(loader.dos_release_mutex_sem(0, m1), NO_ERROR, "taken all the same");
    }

    #[test]
    fn test_owner_died_wakes_a_waiter() {
        let loader = Loader::new_mock();
        let m = new_mutex(&loader, Some(3));
        std::thread::scope(|s| {
            let waiter = s.spawn(|| loader.dos_request_mutex_sem(4, m, u32::MAX));
            wait_for_waiters(&loader, super::SemKind::Mutex, m, 1);
            assert_eq!(loader.dos_close_mutex_sem(m), ERROR_SEM_BUSY);
            loader.shared.sem_mgr.lock_or_recover().owner_ended(3, 3);
            assert_eq!(waiter.join().unwrap(), ERROR_SEM_OWNER_DIED);
        });
        assert_eq!(loader.dos_release_mutex_sem(4, m), NO_ERROR);
        assert_eq!(loader.dos_close_mutex_sem(m), NO_ERROR, "an owned mutex closes");
    }

    #[test]
    fn test_close_busy_while_a_thread_waits() {
        let loader = Loader::new_mock();
        let ev = new_event(&loader, false);
        let mux = new_mux(&loader, &[(ev, 9)], DCMW_WAIT_ANY);
        std::thread::scope(|s| {
            let on_event = s.spawn(|| loader.dos_wait_event_sem(ev, u32::MAX));
            let on_mux = s.spawn(|| {
                let rc = loader.dos_wait_mux_wait_sem(1, mux, u32::MAX, 0);
                (rc, std::time::Instant::now())
            });
            wait_for_waiters(&loader, super::SemKind::Event, ev, 1);
            wait_for_waiters(&loader, super::SemKind::MuxWait, mux, 1);
            assert_eq!(loader.dos_close_event_sem(ev), ERROR_SEM_BUSY);
            assert_eq!(loader.dos_close_mux_wait_sem(mux), ERROR_SEM_BUSY);
            let posted = std::time::Instant::now();
            assert_eq!(loader.dos_post_event_sem(ev), NO_ERROR);
            assert_eq!(on_event.join().unwrap(), NO_ERROR);
            let (rc, woke) = on_mux.join().unwrap();
            assert_eq!(rc, NO_ERROR);
            assert!(woke.duration_since(posted) < std::time::Duration::from_secs(5));
        });
        assert_eq!(loader.dos_close_mux_wait_sem(mux), NO_ERROR);
        assert_eq!(loader.dos_close_event_sem(ev), NO_ERROR);
    }

    #[test]
    fn test_wait_all_mutexes_woken_by_release() {
        let loader = Loader::new_mock();
        let (m1, m2) = (new_mutex(&loader, Some(3)), new_mutex(&loader, Some(4)));
        let all = new_mux(&loader, &[(m1, 1), (m2, 2)], DCMW_WAIT_ALL);
        std::thread::scope(|s| {
            let waiter = s.spawn(|| loader.dos_wait_mux_wait_sem(5, all, u32::MAX, 0));
            wait_for_waiters(&loader, super::SemKind::MuxWait, all, 1);
            assert_eq!(loader.dos_release_mutex_sem(3, m1), NO_ERROR);
            assert_eq!(loader.dos_release_mutex_sem(4, m2), NO_ERROR);
            assert_eq!(waiter.join().unwrap(), NO_ERROR);
        });
        assert_eq!(loader.dos_release_mutex_sem(5, m1), NO_ERROR);
        assert_eq!(loader.dos_release_mutex_sem(5, m2), NO_ERROR);
    }

    // ── Queue ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_dos_queue_create_write_query_read_close() {
        let loader = Loader::new_mock();
        let phq = 0x2000u32;
        loader.guest_write_bytes(0x3000, b"\\QUEUES\\TEST\0");
        assert_eq!(loader.dos_create_queue(phq, 0, 0x3000), 0);
        let hq = loader.guest_read::<u32>(phq).unwrap();
        assert_ne!(hq, 0);

        // Write 3-byte payload
        loader.guest_write_bytes(0x4000, b"xyz");
        assert_eq!(loader.dos_write_queue(hq, 0, 3, 0x4000, 0), 0);

        // Query count → 1
        let pcb = 0x5000u32;
        assert_eq!(loader.dos_query_queue(hq, pcb), 0);
        assert_eq!(loader.guest_read::<u32>(pcb), Some(1));

        // Read queue (wait=1 IO_WAIT, but item already present)
        let pcb_len = 0x6000u32;
        let ppbuf   = 0x6004u32;
        assert_eq!(loader.dos_read_queue(hq, 0, pcb_len, ppbuf, 0, 0, 0, 0), 0, "DCWW_WAIT with an element");
        assert_eq!(loader.guest_read::<u32>(pcb_len), Some(3)); // 3 bytes

        // Queue now empty — NOWAIT read must return 342 (ERROR_QUE_EMPTY)
        assert_eq!(loader.dos_read_queue(hq, 0, pcb_len, ppbuf, 0, 1, 0, 0), 342, "DCWW_NOWAIT on an empty queue");

        assert_eq!(loader.dos_close_queue(hq), 0);
    }

    // ── DosSleep ─────────────────────────────────────────────────────────────

    #[test]
    fn test_dos_sleep_zero_returns_immediately() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_sleep(0), 0);
    }

    // ── DosQueryDBCSEnv / dos_query_dbcs_env ─────────────────────────────────

    #[test]
    fn test_dos_query_dbcs_env_sbcs_writes_terminator() {
        // Default codepage is CP437 (SBCS) — vector is just (0,0).
        let loader = Loader::new_mock();
        let buf = 0x5000u32;
        assert_eq!(loader.dos_query_dbcs_env(2, 0, buf), 0);
        assert_eq!(loader.guest_read::<u8>(buf),     Some(0x00));
        assert_eq!(loader.guest_read::<u8>(buf + 1), Some(0x00));
    }

    #[test]
    fn test_dos_query_dbcs_env_cp932_writes_two_ranges() {
        let loader = Loader::new_mock();
        // Set active codepage to CP932 (Shift-JIS).
        loader.shared.active_codepage.store(932, Ordering::Relaxed);
        let buf = 0x5000u32;
        // CP932 has 2 ranges → needs 6 bytes.
        assert_eq!(loader.dos_query_dbcs_env(6, 0, buf), 0);
        // Range 1: (0x81, 0x9F)
        assert_eq!(loader.guest_read::<u8>(buf),     Some(0x81));
        assert_eq!(loader.guest_read::<u8>(buf + 1), Some(0x9F));
        // Range 2: (0xE0, 0xFC)
        assert_eq!(loader.guest_read::<u8>(buf + 2), Some(0xE0));
        assert_eq!(loader.guest_read::<u8>(buf + 3), Some(0xFC));
        // Terminator
        assert_eq!(loader.guest_read::<u8>(buf + 4), Some(0x00));
        assert_eq!(loader.guest_read::<u8>(buf + 5), Some(0x00));
    }

    #[test]
    fn test_dos_query_dbcs_env_cp936_writes_one_range() {
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(936, Ordering::Relaxed);
        let buf = 0x5000u32;
        // CP936 has 1 range → needs 4 bytes.
        assert_eq!(loader.dos_query_dbcs_env(4, 0, buf), 0);
        assert_eq!(loader.guest_read::<u8>(buf),     Some(0x81));
        assert_eq!(loader.guest_read::<u8>(buf + 1), Some(0xFE));
        assert_eq!(loader.guest_read::<u8>(buf + 2), Some(0x00));
        assert_eq!(loader.guest_read::<u8>(buf + 3), Some(0x00));
    }

    #[test]
    fn test_dos_query_dbcs_env_short_buffer_gets_what_fits() {
        let loader = Loader::new_mock();
        loader.shared.active_codepage.store(932, Ordering::Relaxed);
        let buf = 0x5000u32;
        loader.guest_write_bytes(buf, &[0xEE; 6]).unwrap();
        // CP932 needs 6 bytes; supply only 4: the first range and the start
        // of the second, in order, and nothing past cb.
        assert_eq!(loader.dos_query_dbcs_env(4, 0, buf), ERROR_NLS_TABLE_TRUNCATED);
        let got = loader.guest_read_bytes(buf, 6).unwrap();
        assert_eq!(got, [0x81, 0x9F, 0xE0, 0xFC, 0xEE, 0xEE]);
    }

    #[test]
    fn test_dos_query_dbcs_env_null_buf_is_invalid() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_query_dbcs_env(256, 0, 0), ERROR_INVALID_PARAMETER);
    }

    #[test]
    fn test_nls_calls_follow_the_countrycode_codepage() {
        // COUNTRYCODE { country, codepage }: a non-zero codepage is used
        // instead of the process codepage (CP437 here).
        let loader = Loader::new_mock();
        let (pcc, buf) = (0x4000u32, 0x5000u32);
        loader.guest_write::<u32>(pcc, 0).unwrap();
        loader.guest_write::<u32>(pcc + 4, 936).unwrap();
        assert_eq!(loader.nls_codepage(pcc), 936);
        assert_eq!(loader.dos_query_dbcs_env(4, pcc, buf), NO_ERROR);
        assert_eq!(loader.guest_read::<u8>(buf), Some(0x81));
        // codepage 0 → the process codepage; no COUNTRYCODE likewise.
        loader.guest_write::<u32>(pcc + 4, 0).unwrap();
        assert_eq!(loader.nls_codepage(pcc), 437);
        assert_eq!(loader.nls_codepage(0), 437);
        // A COUNTRYCODE pointer at the top of the address space does not wrap.
        assert_eq!(loader.nls_codepage(u32::MAX - 1), 437);
    }

    #[test]
    fn test_dos_query_collate_returns_256_weights() {
        let loader = Loader::new_mock();
        let (buf, pcch) = (0x5000u32, 0x4800u32);
        assert_eq!(loader.dos_query_collate(256, 0, buf, pcch), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(pcch), Some(256));
        let table = loader.guest_read_bytes(buf, 256).unwrap();
        assert_eq!(table, crate::loader::codepage::collate_table(437));
        assert_eq!(table[b'a' as usize], table[b'A' as usize], "case-blind letters");
    }

    #[test]
    fn test_dos_query_collate_short_buffer_is_truncated() {
        let loader = Loader::new_mock();
        let (buf, pcch) = (0x5000u32, 0x4800u32);
        loader.guest_write::<u8>(buf + 16, 0xEE).unwrap();
        assert_eq!(loader.dos_query_collate(16, 0, buf, pcch), ERROR_NLS_TABLE_TRUNCATED);
        assert_eq!(loader.guest_read::<u32>(pcch), Some(16));
        assert_eq!(loader.guest_read::<u8>(buf + 16), Some(0xEE), "nothing past cb");
        assert_eq!(loader.dos_query_collate(256, 0, 0, pcch), ERROR_INVALID_PARAMETER);
    }

    // ── Handle redirection (DosDupHandle) ────────────────────────────────────

    /// A mock loader whose drive C: is a fresh temporary directory, in SDL
    /// text mode (console output goes to the VIO buffer, not the test's
    /// standard output).
    fn loader_with_drive_c() -> (Loader, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let loader = Loader::new_mock();
        loader.shared.use_sdl_text.store(true, Ordering::Relaxed);
        loader.shared.console_mgr.lock_or_recover().enable_sdl_mode();
        loader.shared.drive_mgr.lock_or_recover().mount(2,
            Box::new(super::super::vfs_hostdir::HostDirBackend::new(dir.path().to_path_buf()).unwrap()));
        (loader, dir)
    }

    const NAME: u32 = 0x4000;
    const PHF: u32 = 0x4100;
    const PACTION: u32 = 0x4104;
    const PNEW: u32 = 0x4108;
    const TEXT: u32 = 0x4200;
    const PACTUAL: u32 = 0x4300;

    /// DosOpen of `name` for writing, created or replaced.
    fn create(loader: &Loader, name: &str) -> u32 {
        loader.guest_write_bytes(NAME, name.as_bytes());
        loader.guest_write::<u8>(NAME + name.len() as u32, 0);
        // OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS, write-only, deny none.
        assert_eq!(loader.dos_open(NAME, PHF, PACTION, 0x12, 0x41), 0);
        loader.guest_read::<u32>(PHF).unwrap()
    }

    fn write(loader: &Loader, hf: u32, text: &[u8]) -> u32 {
        loader.guest_write_bytes(TEXT, text);
        loader.dos_write(hf, TEXT, text.len() as u32, PACTUAL)
    }

    fn dup(loader: &Loader, old: u32, new: u32) -> (u32, u32) {
        loader.guest_write::<u32>(PNEW, new);
        let rc = loader.dos_dup_handle(old, PNEW);
        (rc, loader.guest_read::<u32>(PNEW).unwrap())
    }

    /// The redirection a shell does for `echo text > file`: save the console
    /// with DosDupHandle(1, NEW), DosDupHandle(hFile, 1), write, put the
    /// console back with DosDupHandle(saved, 1). The text lands in the file,
    /// the console output after it on the screen.
    #[test]
    fn test_dup_handle_redirects_stdout_to_a_file_and_back() {
        let (loader, dir) = loader_with_drive_c();
        let hf = create(&loader, "C:\\OUT.TXT");
        let (rc, saved) = dup(&loader, 1, NEW_HANDLE_REQUEST);
        assert_eq!(rc, 0);
        assert!(saved >= 3 && saved != hf, "a new handle: {saved}");
        assert_eq!(dup(&loader, hf, 1).0, 0);
        assert_eq!(loader.dos_close(hf), 0, "the file stays open through handle 1");
        assert_eq!(write(&loader, 1, b"hello\r\n"), 0);
        assert_eq!(loader.guest_read::<u32>(PACTUAL), Some(7));
        assert_eq!(dup(&loader, saved, 1).0, 0, "back to the console");
        assert_eq!(loader.dos_close(saved), 0);
        assert_eq!(std::fs::read(dir.path().join("OUT.TXT")).unwrap(), b"hello\r\n");
        assert_eq!(write(&loader, 1, b"Z"), 0);
        let c = loader.shared.console_mgr.lock_or_recover();
        assert_eq!(c.buffer[0].0, 'Z', "console output reaches the screen again");
    }

    /// Duplicated file handles share the file pointer; the file stays open
    /// until the last of them is closed.
    #[test]
    fn test_dup_handle_shares_the_file_pointer() {
        let (loader, dir) = loader_with_drive_c();
        let hf = create(&loader, "C:\\SHARE.TXT");
        let (rc, h2) = dup(&loader, hf, NEW_HANDLE_REQUEST);
        assert_eq!(rc, 0);
        assert_eq!(write(&loader, hf, b"ab"), 0);
        assert_eq!(write(&loader, h2, b"cd"), 0);
        assert_eq!(loader.dos_close(hf), 0);
        assert_eq!(write(&loader, h2, b"ef"), 0, "still open through the duplicate");
        assert_eq!(loader.dos_close(h2), 0);
        assert_eq!(write(&loader, h2, b"x"), ERROR_INVALID_HANDLE);
        assert_eq!(std::fs::read(dir.path().join("SHARE.TXT")).unwrap(), b"abcdef");
    }

    /// DosQueryHType follows the redirection: console = character device
    /// with the console attribute, file = file, pipe = pipe.
    #[test]
    fn test_query_h_type_of_redirected_handles() {
        let (loader, _dir) = loader_with_drive_c();
        let (pt, pa) = (0x4400u32, 0x4404u32);
        assert_eq!(loader.dos_query_h_type(1, pt, pa), 0);
        assert_eq!(loader.guest_read::<u32>(pt), Some(HANDTYPE_DEVICE));
        assert_eq!(loader.guest_read::<u32>(pa), Some(CONSOLE_DEVICE_ATTR));
        let hf = create(&loader, "C:\\T.TXT");
        dup(&loader, hf, 1);
        assert_eq!(loader.dos_query_h_type(1, pt, pa), 0);
        assert_eq!(loader.guest_read::<u32>(pt), Some(HANDTYPE_FILE));
        assert_eq!(loader.dos_create_pipe(0x4500, 0x4504, 0), 0);
        let hw = loader.guest_read::<u32>(0x4504).unwrap();
        dup(&loader, hw, 2);
        assert_eq!(loader.dos_query_h_type(2, pt, pa), 0);
        assert_eq!(loader.guest_read::<u32>(pt), Some(HANDTYPE_PIPE));
        assert_eq!(loader.dos_query_h_type(0x0FFF, pt, pa), ERROR_INVALID_HANDLE);
    }

    /// A pipe duplicated into handle 1 receives DosWrite(1).
    #[test]
    fn test_dup_handle_redirects_stdout_to_a_pipe() {
        let (loader, _dir) = loader_with_drive_c();
        assert_eq!(loader.dos_create_pipe(0x4500, 0x4504, 0), 0);
        let (hr, hw) = (loader.guest_read::<u32>(0x4500).unwrap(), loader.guest_read::<u32>(0x4504).unwrap());
        assert_eq!(dup(&loader, hw, 1).0, 0);
        assert_eq!(write(&loader, 1, b"piped"), 0);
        assert_eq!(loader.dos_read(hr, 0x4600, 5, PACTUAL), 0);
        assert_eq!(loader.guest_read::<u32>(PACTUAL), Some(5));
        let mut got = [0u8; 5];
        for (i, b) in got.iter_mut().enumerate() { *b = loader.guest_read::<u8>(0x4600 + i as u32).unwrap(); }
        assert_eq!(&got, b"piped");
    }

    /// Files and pipes share one handle space: a pipe takes the lowest free
    /// number, a file opened next does not get the same one, closed numbers
    /// are used again, and a second close fails.
    #[test]
    fn test_pipes_and_files_share_small_handle_numbers() {
        let (loader, _dir) = loader_with_drive_c();
        assert_eq!(loader.dos_create_pipe(0x4500, 0x4504, 0), 0);
        let (hr, hw) = (loader.guest_read::<u32>(0x4500).unwrap(), loader.guest_read::<u32>(0x4504).unwrap());
        assert_eq!((hr, hw), (3, 4), "the lowest free numbers");
        loader.guest_write_bytes(0x4700, b"C:\\SHARE.TXT\0").unwrap();
        assert_eq!(loader.dos_open(0x4700, 0x4800, 0x4804, 0x12, 0x42), 0);
        let hf = loader.guest_read::<u32>(0x4800).unwrap();
        assert_eq!(hf, 5, "a file after the pipe");
        assert_eq!(loader.dos_close(hr), 0);
        assert_eq!(loader.dos_close(hr), ERROR_INVALID_HANDLE, "closed twice");
        assert_eq!(loader.dos_create_pipe(0x4500, 0x4504, 0), 0);
        assert_eq!(loader.guest_read::<u32>(0x4500), Some(3), "number 3 used again");
        assert_eq!(loader.guest_read::<u32>(0x4504), Some(6));
        assert_eq!(loader.dos_close(hf), 0);
        assert_eq!(loader.dos_close(hw), 0);
        // A pipe duplicated over a file closes the file.
        assert_eq!(loader.dos_open(0x4700, 0x4800, 0x4804, 0x01, 0x42), 0);
        let hf = loader.guest_read::<u32>(0x4800).unwrap();
        assert_eq!(dup(&loader, 3, hf).0, 0);
        assert_eq!(loader.handle_target(hf), Some(super::HandleTarget::Pipe));
        assert_eq!(loader.dos_create_pipe(0, 0x4504, 0), ERROR_INVALID_PARAMETER);
    }

    /// DosQueryFHState / DosSetFHState on pipe handles: access mode and the
    /// inheritance flag; a duplicate copies the state.
    #[test]
    fn test_pipe_handle_state() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_create_pipe(0x4500, 0x4504, 0), 0);
        let (hr, hw) = (loader.guest_read::<u32>(0x4500).unwrap(), loader.guest_read::<u32>(0x4504).unwrap());
        assert_eq!(loader.dos_query_fh_state(hr, 0x4600), 0);
        assert_eq!(loader.guest_read::<u32>(0x4600), Some(OPEN_ACCESS_READONLY));
        assert_eq!(loader.dos_query_fh_state(hw, 0x4600), 0);
        assert_eq!(loader.guest_read::<u32>(0x4600), Some(OPEN_ACCESS_WRITEONLY));
        // Only the settable flags change; the access mode stays.
        assert_eq!(loader.dos_set_fh_state(hw, OPEN_FLAGS_NOINHERIT | OPEN_ACCESS_READWRITE), 0);
        assert_eq!(loader.dos_query_fh_state(hw, 0x4600), 0);
        assert_eq!(loader.guest_read::<u32>(0x4600), Some(OPEN_ACCESS_WRITEONLY | OPEN_FLAGS_NOINHERIT));
        assert_eq!(dup(&loader, hw, 9).0, 0);
        assert_eq!(loader.dos_query_fh_state(9, 0x4600), 0);
        assert_eq!(loader.guest_read::<u32>(0x4600), Some(OPEN_ACCESS_WRITEONLY | OPEN_FLAGS_NOINHERIT));
        // Only the read end is passed to a child (both write handles are not).
        let spec = loader.child_handles().spec();
        assert!(spec.starts_with(&format!("{hr},")) && !spec.contains(';'), "{spec}");
    }

    /// DosClose(1) closes standard output: writes fail until something is
    /// duplicated into it again.
    #[test]
    fn test_closed_standard_handle_until_duplicated_into() {
        let (loader, _dir) = loader_with_drive_c();
        let (_, saved) = dup(&loader, 1, NEW_HANDLE_REQUEST);
        assert_eq!(loader.dos_close(1), 0);
        assert_eq!(write(&loader, 1, b"x"), ERROR_INVALID_HANDLE);
        assert_eq!(write(&loader, saved, b"y"), 0, "the duplicate still writes to the screen");
        assert_eq!(dup(&loader, saved, 1).0, 0);
        assert_eq!(write(&loader, 1, b"z"), 0);
        let c = loader.shared.console_mgr.lock_or_recover();
        assert_eq!((c.buffer[0].0, c.buffer[1].0), ('y', 'z'));
    }

    #[test]
    fn test_dup_handle_rejects_unknown_handles() {
        let (loader, _dir) = loader_with_drive_c();
        assert_eq!(dup(&loader, 0x0FFF, NEW_HANDLE_REQUEST).0, ERROR_INVALID_HANDLE);
        // Dup into itself: nothing to do.
        assert_eq!(dup(&loader, 1, 1).0, 0);
        // Pipe-table numbers are not for files or the console.
        assert_eq!(dup(&loader, 1, PIPE_HANDLE_BASE + 5).0, ERROR_INVALID_HANDLE);
    }

    /// DosWrite to the console carries out ANSI sequences (OS/2 routes it
    /// through VioWrtTTY).
    #[test]
    fn test_dos_write_interprets_ansi_on_the_console() {
        let (loader, _dir) = loader_with_drive_c();
        assert_eq!(write(&loader, 1, b"abc\x1b[2;3H\x1b[1;32mX"), 0);
        assert_eq!(loader.guest_read::<u32>(PACTUAL), Some(17), "every byte counts as written");
        let c = loader.shared.console_mgr.lock_or_recover();
        let cols = c.cols as usize;
        assert_eq!(c.buffer[cols + 2], ('X', 0x0A));
        assert_eq!((c.cursor_row, c.cursor_col), (1, 3));
    }
}
