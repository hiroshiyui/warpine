// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 DOSCALLS and QUECALLS API handler methods.
//
// Handle routing (no fallback):
// - Handles 0/1/2: stdin/stdout/stderr (special-cased)
// - Handles 3..PIPE_HANDLE_BASE-1: VFS file handles (DriveManager)
// - Handles PIPE_HANDLE_BASE+: pipe handles (HandleManager)
//
// VFS file operations never fall back to HandleManager and vice versa.
// This ensures VFS bugs are caught immediately rather than masked.

use std::fs::File;
use std::io::{Read, Write, Seek, SeekFrom};
use std::sync::Arc;
use std::thread;
use kvm_ioctls::{Kvm, VcpuFd};
use log::debug;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::managers::PIPE_HANDLE_BASE;
use super::ipc::*;
use super::vfs::*;

impl super::Loader {
    // ── File I/O (via DriveManager → VfsBackend) ──

    pub fn dos_open(&self, psz_name_ptr: u32, phf_ptr: u32, pul_action_ptr: u32, fs_open_flags: u32, fs_open_mode: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
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
        if hf >= PIPE_HANDLE_BASE {
            self.shared.handle_mgr.lock_or_recover().close(hf);
            0
        } else {
            match self.shared.drive_mgr.lock_or_recover().close_file(hf) {
                Ok(()) => 0,
                Err(e) => e.0,
            }
        }
    }

    pub fn dos_read(&self, hf: u32, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        debug!("  DosRead(hf={}, buf=0x{:08X}, len={}, actual=0x{:08X})", hf, buf_ptr, len, actual_ptr);
        if hf == 0 {
            return self.dos_read_stdin(buf_ptr, len, actual_ptr);
        }

        if hf >= PIPE_HANDLE_BASE {
            // Pipe handle
            let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
            if let Some(file) = h_mgr.get_mut(hf) {
                let mut data = vec![0u8; len as usize];
                match file.read(&mut data) {
                    Ok(n) => {
                        self.guest_write_bytes(buf_ptr, &data[..n]);
                        if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, n as u32); }
                        0
                    },
                    Err(_) => 5,
                }
            } else { 6 }
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

    /// Read from stdin (handle 0) using the host terminal.
    /// Enables raw mode and reads one byte at a time, blocking until input is available.
    /// Translates CR (0x0D) → CR+LF (OS/2 console convention) and echoes characters.
    fn dos_read_stdin(&self, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        if len == 0 {
            if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 0); }
            return 0;
        }
        // Check for pending LF from previous CR→CRLF translation
        {
            let mut console = self.shared.console_mgr.lock_or_recover();
            if console.stdin_pending_lf {
                console.stdin_pending_lf = false;
                self.guest_write::<u8>(buf_ptr, 0x0A); // LF
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 1); }
                return 0;
            }
            console.enable_raw_mode();
        }
        // Block until at least one byte is available
        loop {
            if self.shutting_down() { return ERROR_INVALID_FUNCTION; }
            let mut buf = [0u8; 1];
            let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1) };
            if n == 1 {
                let byte = buf[0];
                if byte == 0x0D {
                    // CR from Enter key → deliver CR now, queue LF for next read
                    let mut console = self.shared.console_mgr.lock_or_recover();
                    console.stdin_pending_lf = true;
                    // Echo CR+LF to terminal
                    let _ = unsafe { libc::write(libc::STDOUT_FILENO, b"\r\n".as_ptr() as *const libc::c_void, 2) };
                } else if byte == 0x08 || byte == 0x7F {
                    // Backspace — echo destructive backspace
                    let _ = unsafe { libc::write(libc::STDOUT_FILENO, b"\x08 \x08".as_ptr() as *const libc::c_void, 3) };
                } else if byte >= 0x20 {
                    // Printable character — echo to terminal
                    let _ = unsafe { libc::write(libc::STDOUT_FILENO, buf.as_ptr() as *const libc::c_void, 1) };
                }
                self.guest_write::<u8>(buf_ptr, byte);
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 1); }
                return 0;
            }
            if n == 0 {
                // VTIME timeout expired with no data — retry
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            if n < 0 {
                let err = unsafe { *libc::__errno_location() };
                if err == libc::EAGAIN || err == libc::EINTR {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, 0); }
                return ERROR_ACCESS_DENIED;
            }
        }
    }

    pub fn dos_write(&self, fd: u32, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        if let Some(data) = self.guest_slice_mut(buf_ptr, len as usize) {
            if fd == 1 || fd == 2 {
                match crate::api::doscalls::dos_write(fd, data) {
                    Ok(actual) => {
                        if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, actual); }
                        0
                    },
                    Err(_) => 1,
                }
            } else if fd >= PIPE_HANDLE_BASE {
                let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
                if let Some(file) = h_mgr.get_mut(fd) {
                    match file.write(data) {
                        Ok(n) => {
                            if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, n as u32); }
                            0
                        },
                        Err(_) => 5,
                    }
                } else { 6 }
            } else {
                let dm = self.shared.drive_mgr.lock_or_recover();
                match dm.write_file(fd, data) {
                    Ok(n) => {
                        if actual_ptr != 0 { self.guest_write::<u32>(actual_ptr, n as u32); }
                        0
                    }
                    Err(e) => e.0,
                }
            }
        } else { 87 }
    }

    pub fn dos_set_file_ptr(&self, hf: u32, offset: i32, method: u32, actual_ptr: u32) -> u32 {
        if hf >= PIPE_HANDLE_BASE {
            let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
            if let Some(file) = h_mgr.get_mut(hf) {
                let pos = match method {
                    0 => SeekFrom::Start(offset as u64),
                    1 => SeekFrom::Current(offset as i64),
                    2 => SeekFrom::End(offset as i64),
                    _ => return 1,
                };
                match file.seek(pos) {
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
        if hf >= PIPE_HANDLE_BASE {
            let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
            if let Some(file) = h_mgr.get_mut(hf) {
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

    /// DosSetFileLocks (ordinal 349): lock/unlock byte ranges.
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

    /// DosProtectSetFileLocks (ordinal 613): lock/unlock with file lock ID.
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
        if hf >= PIPE_HANDLE_BASE {
            let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
            if let Some(file) = h_mgr.get_mut(hf) {
                if let Ok(meta) = file.metadata() {
                    self.write_filestatus3_internal(&meta, buf_ptr);
                    if level == 2 { self.guest_write::<u32>(buf_ptr + 24, 4); }
                    return 0;
                }
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
        } else if hf >= PIPE_HANDLE_BASE {
            let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
            if let Some(file) = h_mgr.get_mut(hf) {
                let _ = file.flush();
                0
            } else { ERROR_INVALID_HANDLE }
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
            Ok(b) => match b.enum_ea(&rel_path) {
                Ok(eas) => eas,
                Err(_) => Vec::new(),
            },
            Err(e) => return e.0,
        };

        let max_count = if pul_count != 0 {
            self.guest_read::<u32>(pul_count).unwrap_or(1) as usize
        } else { 1 };

        // ul_entry is 1-based
        let start = if ul_entry > 0 { (ul_entry - 1) as usize } else { 0 };
        let mut offset = 0u32;
        let mut count = 0u32;

        for ea in eas.iter().skip(start).take(max_count) {
            // DENA1 structure: reserved(4) + cbName(1) + cbValue(2) + szName(cbName+1)
            let entry_size = 4 + 1 + 2 + ea.name.len() as u32 + 1;
            let aligned_size = (entry_size + 3) & !3; // 4-byte aligned
            if offset + aligned_size > cb_buf { break; }

            self.guest_write::<u32>(pv_buf + offset, 0);           // reserved
            self.guest_write::<u8>(pv_buf + offset + 4, ea.name.len() as u8); // cbName
            self.guest_write::<u16>(pv_buf + offset + 5, ea.value.len() as u16); // cbValue
            self.guest_write_bytes(pv_buf + offset + 7, ea.name.as_bytes());
            self.guest_write::<u8>(pv_buf + offset + 7 + ea.name.len() as u32, 0); // null term

            offset += aligned_size;
            count += 1;
        }

        if pul_count != 0 {
            self.guest_write::<u32>(pul_count, count);
        }
        0
    }

    // ── Directory enumeration (via DriveManager) ──

    pub fn dos_find_first(&self, psz_spec_ptr: u32, phdir_ptr: u32, attr: u32, buf_ptr: u32, buf_len: u32, pc_found_ptr: u32, level: u32) -> u32 {
        if level != 1 && level != 2 { return 124; }
        let spec = self.read_guest_string(psz_spec_ptr);

        let requested = if pc_found_ptr != 0 {
            self.guest_read::<u32>(pc_found_ptr).unwrap_or(1).max(1)
        } else { 1 };

        let mut dm = self.shared.drive_mgr.lock_or_recover();
        match dm.find_first(&spec, FileAttribute(attr)) {
            Ok((hdir, first_entry)) => {
                self.guest_write::<u32>(phdir_ptr, hdir);
                let mut entries = vec![first_entry];
                for _ in 1..requested {
                    match dm.find_next(hdir) {
                        Ok(entry) => entries.push(entry),
                        Err(_) => break,
                    }
                }
                self.write_filefindbuf3_multi(&entries, buf_ptr, buf_len, pc_found_ptr, level == 2)
            }
            Err(e) => e.0,
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
        // TODO: track find level per handle to pass correct include_ea_size flag
        self.write_filefindbuf3_multi(&entries, buf_ptr, buf_len, pc_found_ptr, false)
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
            Ok(()) => {
                // Also update ProcessManager for backward compatibility (DosExecPgm uses it)
                let os2_path = name.replace('/', "\\");
                let mut proc_mgr = self.shared.process_mgr.lock_or_recover();
                if os2_path.len() >= 2 && os2_path.as_bytes()[1] == b':' {
                    proc_mgr.current_dir = os2_path[2..].to_string();
                } else if os2_path.starts_with('\\') {
                    proc_mgr.current_dir = os2_path;
                } else {
                    let mut new_dir = proc_mgr.current_dir.clone();
                    if !new_dir.ends_with('\\') { new_dir.push('\\'); }
                    new_dir.push_str(&os2_path);
                    proc_mgr.current_dir = new_dir;
                }
                if !proc_mgr.current_dir.starts_with('\\') {
                    proc_mgr.current_dir.insert(0, '\\');
                }
                if proc_mgr.current_dir.len() > 1 && proc_mgr.current_dir.ends_with('\\') {
                    proc_mgr.current_dir.pop();
                }
                NO_ERROR
            }
            Err(e) => e.0,
        }
    }

    pub fn dos_query_current_dir(&self, disk_num: u32, p_buf: u32, pcb_buf: u32) -> u32 {
        debug!("  DosQueryCurrentDir(disk={})", disk_num);
        let dm = self.shared.drive_mgr.lock_or_recover();
        let drive = if disk_num == 0 { dm.current_disk() } else { (disk_num as u8) - 1 };
        let dir = dm.current_dir(drive);
        let dir_bytes = dir.as_bytes();

        if pcb_buf != 0 {
            let buf_len = self.guest_read::<u32>(pcb_buf).unwrap_or(0) as usize;
            if buf_len < dir_bytes.len() + 1 {
                self.guest_write::<u32>(pcb_buf, (dir_bytes.len() + 1) as u32);
                return ERROR_BUFFER_OVERFLOW;
            }
            self.guest_write::<u32>(pcb_buf, (dir_bytes.len() + 1) as u32);
        }

        if p_buf != 0 {
            self.guest_write_bytes(p_buf, dir_bytes);
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

        for ea_name in &ea_names {
            let ea = match backend.get_ea(&rel_path, ea_name) {
                Ok(ea) => ea,
                Err(_) => EaEntry { name: ea_name.clone(), value: Vec::new(), flags: 0 },
            };

            // FEA2: oNextEntryOffset(4) + fEA(1) + cbName(1) + cbValue(2) + szName(cbName+1) + value(cbValue)
            let fea2_size = 4 + 1 + 1 + 2 + ea.name.len() as u32 + 1 + ea.value.len() as u32;
            let aligned_size = (fea2_size + 3) & !3;

            let fea2_ptr = p_fea2list + out_pos;
            self.guest_write::<u32>(fea2_ptr, 0); // oNextEntryOffset (patched below)
            self.guest_write::<u8>(fea2_ptr + 4, ea.flags);
            self.guest_write::<u8>(fea2_ptr + 5, ea.name.len() as u8);
            self.guest_write::<u16>(fea2_ptr + 6, ea.value.len() as u16);
            self.guest_write_bytes(fea2_ptr + 8, ea.name.as_bytes());
            self.guest_write::<u8>(fea2_ptr + 8 + ea.name.len() as u32, 0); // null term
            if !ea.value.is_empty() {
                self.guest_write_bytes(fea2_ptr + 9 + ea.name.len() as u32, &ea.value);
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

        let ea_field_size: u32 = if include_ea_size { 4 } else { 0 };

        for entry in entries.iter() {
            let name_bytes = entry.name.as_bytes();
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

    pub fn dos_sleep(&self, msec: u32) -> u32 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(msec as u64);
        while std::time::Instant::now() < deadline {
            if self.shutting_down() { return 0; }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            thread::sleep(remaining.min(std::time::Duration::from_millis(100)));
        }
        0
    }

    pub fn dos_create_thread(&self, vcpu_id: u32, ptid_ptr: u32, pfn: u32, param: u32, cb_stack: u32) -> u32 {
        let stack_size = if cb_stack == 0 { 65536 } else { cb_stack };
        let mut mem_mgr = self.shared.mem_mgr.lock_or_recover();
        if let Some(stack_base) = mem_mgr.alloc(stack_size) {
            let tib_addr = mem_mgr.alloc(4096).unwrap();
            let tid = {
                let mut next_tid = self.shared.next_tid.lock_or_recover();
                let tid = *next_tid;
                *next_tid += 1;
                tid
            };
            debug!("  [VCPU {}] Creating thread {} (ptid_ptr=0x{:08X}, pfn=0x{:08X}, param=0x{:08X})", vcpu_id, tid, ptid_ptr, pfn, param);

            self.guest_write::<u32>(tib_addr + 0x18, tib_addr).expect("dos_create_thread: TIB self-ptr OOB");
            self.guest_write::<u32>(tib_addr + 0x30, PIB_BASE).expect("dos_create_thread: TIB->PIB OOB");

            let sp_addr = stack_base + stack_size - 12;
            self.guest_write::<u32>(sp_addr, EXIT_TRAP_ADDR).expect("dos_create_thread: stack write OOB");
            self.guest_write::<u32>(sp_addr + 4, param).expect("dos_create_thread: stack write OOB");

            {
                let new_vcpu = self.vm.create_vcpu(tid as u64).unwrap();
                let mut new_regs = new_vcpu.get_regs().unwrap();
                new_regs.rip = pfn as u64;
                new_regs.rsp = (stack_base + stack_size - 12) as u64;
                new_regs.rax = param as u64;
                new_regs.rflags = 2;
                new_vcpu.set_regs(&new_regs).unwrap();

                let shared_clone = Arc::clone(&self.shared);
                let vm_clone = Arc::clone(&self.vm);
                let handle = thread::spawn(move || {
                    let kvm = Kvm::new().unwrap();
                    let loader = super::Loader { _kvm: kvm, vm: vm_clone, shared: shared_clone };
                    loader.run_vcpu(new_vcpu, tid, tib_addr as u64);
                });
                self.shared.threads.lock_or_recover().insert(tid, handle);
                self.guest_write::<u32>(ptid_ptr, tid);
            }
            0
        } else { 8 }
    }

    pub fn dos_query_h_type(&self, hfile: u32, ptype: u32, pattr: u32) -> u32 {
        if ptype != 0 { self.guest_write::<u32>(ptype, if hfile < 3 { 1 } else { 0 }); }
        if pattr != 0 { self.guest_write::<u32>(pattr, 0); }
        0
    }

    pub fn dos_create_pipe(&self, phf_read_ptr: u32, phf_write_ptr: u32, _size: u32) -> u32 {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } == 0 {
            use std::os::unix::io::FromRawFd;
            let f_read = unsafe { File::from_raw_fd(fds[0]) };
            let f_write = unsafe { File::from_raw_fd(fds[1]) };

            let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
            let h_read = h_mgr.add(f_read);
            let h_write = h_mgr.add(f_write);

            self.guest_write::<u32>(phf_read_ptr, h_read);
            self.guest_write::<u32>(phf_write_ptr, h_write);
            0
        } else { 8 }
    }

    pub fn dos_get_info_blocks(&self, vcpu: &VcpuFd, ptib: u32, ppib: u32) -> u32 {
        let fs_base = vcpu.get_sregs().unwrap().fs.base;
        if ptib != 0 { self.guest_write::<u32>(ptib, fs_base as u32); }
        if ppib != 0 { self.guest_write::<u32>(ppib, PIB_BASE); }
        0
    }

    pub fn dos_alloc_mem(&self, ppb: u32, cb: u32) -> u32 {
        debug!("DosAllocMem(ppb=0x{:08X}, cb=0x{:08X} [{}])", ppb, cb, cb);
        match self.shared.mem_mgr.lock_or_recover().alloc(cb) {
            Some(addr) => {
                debug!("  -> allocated at 0x{:08X}", addr);
                self.guest_write::<u32>(ppb, addr);
                0
            },
            None => 8,
        }
    }

    pub fn dos_free_mem(&self, pb: u32) -> u32 {
        if self.shared.mem_mgr.lock_or_recover().free(pb) { 0 }
        else { 487 }
    }

    pub fn dos_create_event_sem(&self, _psz_name_ptr: u32, phev_ptr: u32, fl_attr: u32, f_state: u32) -> u32 {
        let mut sem_mgr = self.shared.sem_mgr.lock_or_recover();
        let h = sem_mgr.create_event(None, fl_attr, f_state != 0);
        self.guest_write::<u32>(phev_ptr, h);
        0
    }

    pub fn dos_close_event_sem(&self, hev: u32) -> u32 {
        if self.shared.sem_mgr.lock_or_recover().close_event(hev) { 0 }
        else { 6 }
    }

    pub fn dos_post_event_sem(&self, hev: u32) -> u32 {
        let sem_mgr = self.shared.sem_mgr.lock_or_recover();
        if let Some(sem_arc) = sem_mgr.get_event(hev) {
            let (lock, cvar) = &*sem_arc;
            let mut sem = lock.lock_or_recover();
            if sem.posted { 299 }
            else {
                sem.posted = true;
                cvar.notify_all();
                0
            }
        } else { 6 }
    }

    pub fn dos_wait_event_sem(&self, hev: u32, msec: u32) -> u32 {
        let sem_arc = self.shared.sem_mgr.lock_or_recover().get_event(hev);
        if let Some(sem_arc) = sem_arc {
            let (lock, cvar) = &*sem_arc;
            let mut sem = lock.lock_or_recover();
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(
                if msec == u32::MAX { u64::MAX / 2 } else { msec as u64 }
            );
            while !sem.posted {
                if self.shutting_down() { return 640; }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { return 640; }
                let wait_time = remaining.min(std::time::Duration::from_millis(100));
                let (guard, result) = cvar.wait_timeout(sem, wait_time).unwrap();
                sem = guard;
                if result.timed_out() && !sem.posted {
                    if deadline.saturating_duration_since(std::time::Instant::now()).is_zero() { return 640; }
                }
            }
            0
        } else { 6 }
    }

    pub fn dos_create_mutex_sem(&self, _psz_name_ptr: u32, phmtx_ptr: u32, fl_attr: u32, f_state: u32) -> u32 {
        let mut sem_mgr = self.shared.sem_mgr.lock_or_recover();
        let h = sem_mgr.create_mutex(None, fl_attr, f_state != 0);
        self.guest_write::<u32>(phmtx_ptr, h);
        0
    }

    pub fn dos_close_mutex_sem(&self, hmtx: u32) -> u32 {
        if self.shared.sem_mgr.lock_or_recover().close_mutex(hmtx) { 0 }
        else { 6 }
    }

    pub fn dos_request_mutex_sem(&self, tid: u32, hmtx: u32, msec: u32) -> u32 {
        let sem_arc = self.shared.sem_mgr.lock_or_recover().get_mutex(hmtx);
        if let Some(sem_arc) = sem_arc {
            let (lock, cvar) = &*sem_arc;
            let mut sem = lock.lock_or_recover();
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(
                if msec == u32::MAX { u64::MAX / 2 } else { msec as u64 }
            );
            loop {
                if self.shutting_down() { return 640; }
                match sem.owner_tid {
                    None => {
                        sem.owner_tid = Some(tid);
                        sem.request_count = 1;
                        return 0;
                    }
                    Some(owner) if owner == tid => {
                        sem.request_count += 1;
                        return 0;
                    }
                    _ => {
                        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                        if remaining.is_zero() { return 640; }
                        let wait_time = remaining.min(std::time::Duration::from_millis(100));
                        let (guard, _result) = cvar.wait_timeout(sem, wait_time).unwrap();
                        sem = guard;
                    }
                }
            }
        } else { 6 }
    }

    pub fn dos_release_mutex_sem(&self, tid: u32, hmtx: u32) -> u32 {
        let sem_arc = self.shared.sem_mgr.lock_or_recover().get_mutex(hmtx);
        if let Some(sem_arc) = sem_arc {
            let (lock, cvar) = &*sem_arc;
            let mut sem = lock.lock_or_recover();
            match sem.owner_tid {
                Some(owner) if owner == tid => {
                    sem.request_count -= 1;
                    if sem.request_count == 0 {
                        sem.owner_tid = None;
                        cvar.notify_all();
                    }
                    0
                }
                _ => 288,
            }
        } else { 6 }
    }

    pub fn dos_create_mux_wait_sem(&self, _psz_name_ptr: u32, phmux_ptr: u32, count: u32, records_ptr: u32, fl_attr: u32) -> u32 {
        let mut records = Vec::new();
        for i in 0..count {
            let hsem = self.guest_read::<u32>(records_ptr + i * 8).unwrap_or(0);
            let user = self.guest_read::<u32>(records_ptr + i * 8 + 4).unwrap_or(0);
            records.push(MuxWaitRecord { hsem: SemHandle::Event(hsem), user });
        }
        let wait_all = (fl_attr & 4) != 0;
        let mut sem_mgr = self.shared.sem_mgr.lock_or_recover();
        let h = sem_mgr.create_mux(None, fl_attr, records, wait_all);
        self.guest_write::<u32>(phmux_ptr, h);
        0
    }

    pub fn dos_close_mux_wait_sem(&self, hmux: u32) -> u32 {
        if self.shared.sem_mgr.lock_or_recover().close_mux(hmux) { 0 }
        else { 6 }
    }

    pub fn dos_wait_mux_wait_sem(&self, tid: u32, hmux: u32, msec: u32, pul_user_ptr: u32) -> u32 {
        let mux = self.shared.sem_mgr.lock_or_recover().get_mux(hmux);
        if let Some(mux) = mux {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(
                if msec == u32::MAX { u64::MAX / 2 } else { msec as u64 }
            );
            loop {
                if self.shutting_down() { return 640; }
                let mut ready_idx = None;
                let mut all_ready = true;

                for (i, rec) in mux.records.iter().enumerate() {
                    let h = match rec.hsem { SemHandle::Event(h) | SemHandle::Mutex(h) => h };
                    let sem_mgr = self.shared.sem_mgr.lock_or_recover();
                    let is_ready = if let Some(ev_arc) = sem_mgr.get_event(h) {
                        ev_arc.0.lock_or_recover().posted
                    } else if let Some(mtx_arc) = sem_mgr.get_mutex(h) {
                        let mtx = mtx_arc.0.lock_or_recover();
                        mtx.owner_tid.is_none() || mtx.owner_tid == Some(tid)
                    } else { false };

                    if is_ready { ready_idx = Some(i); }
                    else { all_ready = false; }
                }

                if (mux.wait_all && all_ready) || (!mux.wait_all && ready_idx.is_some()) {
                    if let Some(idx) = ready_idx {
                        if pul_user_ptr != 0 {
                            self.guest_write::<u32>(pul_user_ptr, mux.records[idx].user);
                        }
                    }
                    return 0;
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { return 640; }
                thread::sleep(remaining.min(std::time::Duration::from_millis(10)));
            }
        }
        6
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
            let mut data = vec![0u8; len as usize];
            if let Some(src) = self.guest_slice_mut(buf_ptr, len as usize) {
                data.copy_from_slice(src);
            }
            q.items.push_back(QueueEntry { data, event, priority });
            q.cond.notify_one();
            return 0;
        }
        337
    }

    pub fn dos_read_queue(&self, hq: u32, preq_ptr: u32, pcb_ptr: u32, ppbuf_ptr: u32, _elem: u32, wait: u32, pprio_ptr: u32, _hev: u32) -> u32 {
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
            if self.shutting_down() { return 342; }
            {
                let mut q = q_arc.lock_or_recover();
                if let Some(entry) = q.items.pop_front() {
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
            if wait == 0 { return 342; }
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

    pub fn dos_get_resource(&self, _hmod: u32, id_type: u32, id_name: u32, ppb: u32) -> u32 {
        let res_mgr = self.shared.resource_mgr.lock_or_recover();
        if let Some((guest_addr, _size)) = res_mgr.find(id_type as u16, id_name as u16) {
            self.guest_write::<u32>(ppb, guest_addr);
            0
        } else {
            6
        }
    }

    pub fn dos_free_resource(&self, _pb: u32) -> u32 {
        0
    }

    pub fn dos_query_resource_size(&self, _hmod: u32, id_type: u32, id_name: u32, p_size: u32) -> u32 {
        let res_mgr = self.shared.resource_mgr.lock_or_recover();
        if let Some((_guest_addr, size)) = res_mgr.find(id_type as u16, id_name as u16) {
            self.guest_write::<u32>(p_size, size);
            0
        } else {
            6
        }
    }

    pub fn dos_wait_thread(&self, vcpu_id: u32, ptid_ptr: u32) -> u32 {
        let tid = self.guest_read::<u32>(ptid_ptr).unwrap_or(0);
        debug!("  [VCPU {}] Waiting for thread {}...", vcpu_id, tid);
        let mut handle = None;
        for _ in 0..100 {
            if self.shutting_down() { return 309; }
            handle = self.shared.threads.lock_or_recover().remove(&tid);
            if handle.is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if let Some(h) = handle {
            h.join().unwrap();
            0
        } else { 309 }
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

    pub fn dos_dup_handle(&self, old_hf: u32, p_new_hf: u32) -> u32 {
        debug!("DosDupHandle(old={}, pNew=0x{:08X})", old_hf, p_new_hf);
        let new_hf_val = self.guest_read::<u32>(p_new_hf).unwrap_or(0xFFFFFFFF);
        // DupHandle still uses HandleManager — VFS doesn't support dup yet
        let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
        if new_hf_val == 0xFFFFFFFF {
            if let Some(file) = h_mgr.get(old_hf) {
                match file.try_clone() {
                    Ok(dup) => {
                        let new_h = h_mgr.insert(dup);
                        let _ = self.guest_write::<u32>(p_new_hf, new_h);
                        0
                    }
                    Err(_) => ERROR_INVALID_HANDLE,
                }
            } else {
                ERROR_INVALID_HANDLE
            }
        } else {
            if let Some(file) = h_mgr.get(old_hf) {
                match file.try_clone() {
                    Ok(dup) => {
                        h_mgr.replace(new_hf_val, dup);
                        0
                    }
                    Err(_) => ERROR_INVALID_HANDLE,
                }
            } else {
                ERROR_INVALID_HANDLE
            }
        }
    }

    pub fn dos_flat_to_sel(&self, flat_addr: u32) -> u32 {
        debug!("DosFlatToSel(0x{:08X})", flat_addr);
        flat_addr
    }

    pub fn dos_sel_to_flat(&self, sel_off: u32) -> u32 {
        debug!("DosSelToFlat(0x{:08X})", sel_off);
        sel_off
    }

    pub fn dos_get_info_seg(&self, p_global_sel: u32, p_local_sel: u32) -> u32 {
        debug!("DosGetInfoSeg(pGlobal=0x{:08X}, pLocal=0x{:08X})", p_global_sel, p_local_sel);
        if p_global_sel != 0 {
            let _ = self.guest_write::<u16>(p_global_sel, (PIB_BASE >> 4) as u16);
        }
        if p_local_sel != 0 {
            let _ = self.guest_write::<u16>(p_local_sel, (TIB_BASE >> 4) as u16);
        }
        0
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
