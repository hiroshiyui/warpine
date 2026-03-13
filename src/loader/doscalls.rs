// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 DOSCALLS and QUECALLS API handler methods.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use kvm_ioctls::{Kvm, VcpuFd};
use log::debug;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::ipc::*;

impl super::Loader {
    pub fn dos_close(&self, hf: u32) -> u32 {
        self.shared.handle_mgr.lock_or_recover().close(hf);
        0
    }

    pub fn dos_set_file_ptr(&self, hf: u32, offset: i32, method: u32, actual_ptr: u32) -> u32 {
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
                    if actual_ptr != 0 {
                        self.guest_write::<u32>(actual_ptr, new_pos as u32);
                    }
                    0
                }
                Err(_) => 1,
            }
        } else { 6 }
    }

    pub fn dos_open(&self, psz_name_ptr: u32, phf_ptr: u32, pul_action_ptr: u32, fs_open_flags: u32, fs_open_mode: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let path = match self.translate_path(&name) {
            Ok(p) => p,
            Err(e) => return e,
        };

        let mut options = OpenOptions::new();
        match fs_open_mode & 0x07 {
            0 => { options.read(true); },
            1 => { options.write(true); },
            2 => { options.read(true).write(true); },
            _ => {},
        }

        let action_if_exists = fs_open_flags & 0x03;
        let action_if_new = (fs_open_flags >> 4) & 0x03;

        if action_if_new == 1 { options.create(true); }
        if action_if_exists == 2 { options.truncate(true); }

        match options.open(&path) {
            Ok(file) => {
                let h = self.shared.handle_mgr.lock_or_recover().add(file);
                self.guest_write::<u32>(phf_ptr, h);
                if pul_action_ptr != 0 {
                    self.guest_write::<u32>(pul_action_ptr, 1);
                }
                0
            },
            Err(_) => 2,
        }
    }

    pub fn dos_read(&self, hf: u32, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
        if let Some(file) = h_mgr.get_mut(hf) {
            let mut data = vec![0u8; len as usize];
            match file.read(&mut data) {
                Ok(n) => {
                    self.guest_write_bytes(buf_ptr, &data[..n]);
                    if actual_ptr != 0 {
                        self.guest_write::<u32>(actual_ptr, n as u32);
                    }
                    0
                },
                Err(_) => 5,
            }
        } else if hf == 0 { 0 } else { 6 }
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
            } else {
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
            }
        } else { 87 }
    }

    pub fn dos_delete(&self, psz_name_ptr: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let path = match self.translate_path(&name) { Ok(p) => p, Err(e) => return e };
        match fs::remove_file(path) {
            Ok(_) => 0,
            Err(_) => 2,
        }
    }

    pub fn dos_move(&self, psz_old_ptr: u32, psz_new_ptr: u32) -> u32 {
        let old_name = self.read_guest_string(psz_old_ptr);
        let new_name = self.read_guest_string(psz_new_ptr);
        let old = match self.translate_path(&old_name) { Ok(p) => p, Err(e) => return e };
        let new = match self.translate_path(&new_name) { Ok(p) => p, Err(e) => return e };
        match fs::rename(old, new) {
            Ok(_) => 0,
            Err(_) => 2,
        }
    }

    pub fn dos_create_dir(&self, psz_name_ptr: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let path = match self.translate_path(&name) { Ok(p) => p, Err(e) => return e };
        match fs::create_dir(path) {
            Ok(_) => 0,
            Err(_) => 5,
        }
    }

    pub fn dos_delete_dir(&self, psz_name_ptr: u32) -> u32 {
        let name = self.read_guest_string(psz_name_ptr);
        let path = match self.translate_path(&name) { Ok(p) => p, Err(e) => return e };
        match fs::remove_dir(path) {
            Ok(_) => 0,
            Err(_) => 5,
        }
    }

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
                // Create the vCPU using the existing VM fd (no new /dev/kvm needed)
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
                    // Dummy _kvm fd — only needed to satisfy the Loader struct.
                    // run_vcpu only uses shared, never _kvm or vm.
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

    pub fn dos_find_first(&self, psz_spec_ptr: u32, phdir_ptr: u32, _attr: u32, buf_ptr: u32, buf_len: u32, pc_found_ptr: u32, level: u32) -> u32 {
        if level != 1 { return 124; }
        let spec_raw = self.read_guest_string(psz_spec_ptr);
        let spec = spec_raw.replace('\\', "/");
        let spec_path = Path::new(&spec);
        let pattern = spec_path.file_name().and_then(|s| s.to_str()).unwrap_or("*").to_string();
        // Translate the directory part through the sandbox
        let dir_str = spec_path.parent().and_then(|p| p.to_str()).unwrap_or(".");
        let dir_path = match self.translate_path(dir_str) { Ok(p) => p, Err(e) => return e };

        let hdir = {
            if let Ok(rd) = std::fs::read_dir(if dir_path.to_str() == Some("") { Path::new(".") } else { &dir_path }) {
                let mut hdir_mgr = self.shared.hdir_mgr.lock_or_recover();
                let hdir = hdir_mgr.add(rd, pattern);
                self.guest_write::<u32>(phdir_ptr, hdir);
                hdir
            } else { return 3; }
        };

        return self.dos_find_next(hdir, buf_ptr, buf_len, pc_found_ptr);
    }

    pub fn dos_find_close(&self, hdir: u32) -> u32 {
        if self.shared.hdir_mgr.lock_or_recover().close(hdir) { 0 }
        else { 6 }
    }

    pub fn dos_find_next(&self, hdir: u32, buf_ptr: u32, buf_len: u32, pc_found_ptr: u32) -> u32 {
        let mut hdir_mgr = self.shared.hdir_mgr.lock_or_recover();
        if let Some(entry) = hdir_mgr.get_mut(hdir) {
            let pattern = entry.pattern.clone();
            while let Some(Ok(dir_entry)) = entry.iterator.next() {
                let name = dir_entry.file_name().into_string().unwrap_or_default();
                if self.match_pattern(&name, &pattern) {
                    if let Ok(meta) = dir_entry.metadata() {
                        let name_bytes = name.as_bytes();
                        let name_len = name_bytes.len().min(255);
                        if buf_len < (32 + name_len as u32 + 1) { return 111; }
                        self.guest_write::<u32>(buf_ptr, 0);
                        self.write_filestatus3_internal(&meta, buf_ptr + 4);
                        self.guest_write::<u8>(buf_ptr + 24, name_len as u8);
                        self.guest_write_bytes(buf_ptr + 25, &name_bytes[..name_len]);
                        self.guest_write::<u8>(buf_ptr + 25 + name_len as u32, 0);
                        if pc_found_ptr != 0 {
                            self.guest_write::<u32>(pc_found_ptr, 1);
                        }
                        return 0;
                    }
                }
            }
            return 18;
        }
        6
    }

    fn match_pattern(&self, name: &str, pattern: &str) -> bool {
        if pattern == "*" || pattern == "*.*" { return true; }
        let pattern_lower = pattern.to_lowercase();
        let name_lower = name.to_lowercase();

        if pattern_lower.starts_with('*') {
            let suffix = &pattern_lower[1..];
            name_lower.ends_with(suffix)
        } else if pattern_lower.ends_with('*') {
            let prefix = &pattern_lower[..pattern_lower.len()-1];
            name_lower.starts_with(prefix)
        } else {
            name_lower == pattern_lower
        }
    }

    pub fn dos_query_file_info(&self, hf: u32, level: u32, buf_ptr: u32, buf_len: u32) -> u32 {
        if level != 1 { return 124; }
        if buf_len < 22 { return 111; }
        let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
        if let Some(file) = h_mgr.get_mut(hf) {
            if let Ok(meta) = file.metadata() {
                self.write_filestatus3_internal(&meta, buf_ptr);
                return 0;
            }
        }
        6
    }

    pub fn dos_query_path_info(&self, psz_path_ptr: u32, level: u32, buf_ptr: u32, buf_len: u32) -> u32 {
        if level != 1 { return 124; }
        if buf_len < 22 { return 111; }
        let name = self.read_guest_string(psz_path_ptr);
        let path = match self.translate_path(&name) { Ok(p) => p, Err(e) => return e };
        if let Ok(meta) = std::fs::metadata(&path) {
            self.write_filestatus3_internal(&meta, buf_ptr);
            return 0;
        }
        3
    }

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

    // ── Directory Management APIs ──

    /// DosSetCurrentDir (ordinal 255): change the current directory.
    pub fn dos_set_current_dir(&self, psz_dir_name: u32) -> u32 {
        let name = self.read_guest_string(psz_dir_name);
        debug!("  DosSetCurrentDir('{}')", name);

        // Resolve the path to validate it exists and is a directory
        let resolved = match self.translate_path(&name) {
            Ok(p) => p,
            Err(e) => return e,
        };
        if !resolved.is_dir() {
            return ERROR_PATH_NOT_FOUND;
        }

        // Store the OS/2-style path (with backslashes)
        let os2_path = name.replace('/', "\\");
        let mut proc_mgr = self.shared.process_mgr.lock_or_recover();

        // Handle absolute vs relative paths
        if os2_path.len() >= 2 && os2_path.as_bytes()[1] == b':' {
            // Absolute path with drive letter — store everything after drive letter
            proc_mgr.current_dir = os2_path[2..].to_string();
        } else if os2_path.starts_with('\\') {
            // Absolute path without drive letter
            proc_mgr.current_dir = os2_path;
        } else {
            // Relative path — append to current directory
            let mut new_dir = proc_mgr.current_dir.clone();
            if !new_dir.ends_with('\\') {
                new_dir.push('\\');
            }
            new_dir.push_str(&os2_path);
            proc_mgr.current_dir = new_dir;
        }

        // Normalize: ensure starts with backslash
        if !proc_mgr.current_dir.starts_with('\\') {
            proc_mgr.current_dir.insert(0, '\\');
        }
        // Remove trailing backslash (unless root)
        if proc_mgr.current_dir.len() > 1 && proc_mgr.current_dir.ends_with('\\') {
            proc_mgr.current_dir.pop();
        }

        NO_ERROR
    }

    /// DosQueryCurrentDir (ordinal 274): get current directory.
    /// Returns the current directory without drive letter or leading backslash.
    pub fn dos_query_current_dir(&self, disk_num: u32, p_buf: u32, pcb_buf: u32) -> u32 {
        debug!("  DosQueryCurrentDir(disk={})", disk_num);
        let proc_mgr = self.shared.process_mgr.lock_or_recover();
        let dir = proc_mgr.current_dir_no_leading_slash();
        let dir_bytes = dir.as_bytes();

        if pcb_buf != 0 {
            let buf_len = self.guest_read::<u32>(pcb_buf).unwrap_or(0) as usize;
            if buf_len < dir_bytes.len() + 1 {
                // Write needed size and return buffer overflow
                self.guest_write::<u32>(pcb_buf, (dir_bytes.len() + 1) as u32);
                return ERROR_BUFFER_OVERFLOW;
            }
            self.guest_write::<u32>(pcb_buf, (dir_bytes.len() + 1) as u32);
        }

        if p_buf != 0 {
            self.guest_write_bytes(p_buf, dir_bytes);
            self.guest_write::<u8>(p_buf + dir_bytes.len() as u32, 0); // null terminate
        }
        NO_ERROR
    }

    /// DosQueryCurrentDisk (ordinal 275): get current default drive.
    pub fn dos_query_current_disk(&self, p_disk_num: u32, p_logical: u32) -> u32 {
        debug!("  DosQueryCurrentDisk");
        let proc_mgr = self.shared.process_mgr.lock_or_recover();
        if p_disk_num != 0 {
            self.guest_write::<u32>(p_disk_num, proc_mgr.current_disk as u32);
        }
        if p_logical != 0 {
            // Logical drive map: bit 2 = C: present
            self.guest_write::<u32>(p_logical, 0x04); // only C: available
        }
        NO_ERROR
    }

    /// DosSetDefaultDisk (ordinal 220): set current default drive.
    pub fn dos_set_default_disk(&self, disk_num: u32) -> u32 {
        debug!("  DosSetDefaultDisk({})", disk_num);
        if disk_num < 1 || disk_num > 26 {
            return ERROR_INVALID_FUNCTION;
        }
        self.shared.process_mgr.lock_or_recover().current_disk = disk_num as u8;
        NO_ERROR
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
                if remaining.is_zero() { return 640; } // ERROR_TIMEOUT
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
                        if remaining.is_zero() { return 640; } // ERROR_TIMEOUT
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
                if remaining.is_zero() { return 640; } // ERROR_TIMEOUT
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
        343 // ERROR_QUE_NAME_NOT_EXIST
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
        337 // ERROR_QUE_INVALID_HANDLE
    }

    pub fn dos_read_queue(&self, hq: u32, preq_ptr: u32, pcb_ptr: u32, ppbuf_ptr: u32, _elem: u32, wait: u32, pprio_ptr: u32, _hev: u32) -> u32 {
        // Get the queue Arc and its condvar outside the loop
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
                    drop(q); // Release queue lock before acquiring mem_mgr
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
            if wait == 0 { return 342; } // ERROR_QUE_EMPTY
            // Block on condvar instead of spinning
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
            6 // ERROR_INVALID_HANDLE
        }
    }

    pub fn dos_free_resource(&self, _pb: u32) -> u32 {
        // No-op: resource data lives in loaded object pages
        0
    }

    pub fn dos_query_resource_size(&self, _hmod: u32, id_type: u32, id_name: u32, p_size: u32) -> u32 {
        let res_mgr = self.shared.resource_mgr.lock_or_recover();
        if let Some((_guest_addr, size)) = res_mgr.find(id_type as u16, id_name as u16) {
            self.guest_write::<u32>(p_size, size);
            0
        } else {
            6 // ERROR_INVALID_HANDLE
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

    /// DosSetRelMaxFH (ordinal 382): adjust max file handles relative to current
    pub fn dos_set_rel_max_fh(&self, p_req_count: u32, p_cur_max_fh: u32) -> u32 {
        let req_count = self.guest_read::<i32>(p_req_count).unwrap_or(0);
        debug!("DosSetRelMaxFH(reqCount={}, pCurMaxFH=0x{:08X})", req_count, p_cur_max_fh);
        // We don't actually limit file handles, just report a reasonable max
        let cur_max: u32 = 256;
        if p_cur_max_fh != 0 {
            let _ = self.guest_write::<u32>(p_cur_max_fh, cur_max);
        }
        0
    }

    /// DosSetFileSize (ordinal 272): truncate or extend a file
    pub fn dos_set_file_size(&self, hf: u32, new_size: u32) -> u32 {
        debug!("DosSetFileSize(hf={}, size={})", hf, new_size);
        let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
        if let Some(file) = h_mgr.get_mut(hf) {
            match file.set_len(new_size as u64) {
                Ok(_) => 0,
                Err(_) => ERROR_ACCESS_DENIED,
            }
        } else {
            ERROR_INVALID_HANDLE
        }
    }

    /// DosDupHandle (ordinal 260): duplicate a file handle
    pub fn dos_dup_handle(&self, old_hf: u32, p_new_hf: u32) -> u32 {
        debug!("DosDupHandle(old={}, pNew=0x{:08X})", old_hf, p_new_hf);
        let new_hf_val = self.guest_read::<u32>(p_new_hf).unwrap_or(0xFFFFFFFF);
        let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
        // If new_hf_val is 0xFFFFFFFF, allocate a new handle
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
            // Redirect new_hf_val to point to same file as old_hf
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

    /// DosResetBuffer (ordinal 254): flush file buffers
    pub fn dos_reset_buffer(&self, hf: u32) -> u32 {
        debug!("DosResetBuffer(hf={})", hf);
        if hf == 0xFFFFFFFF {
            // Flush all file handles
            let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
            h_mgr.flush_all();
            0
        } else {
            let mut h_mgr = self.shared.handle_mgr.lock_or_recover();
            if let Some(file) = h_mgr.get_mut(hf) {
                let _ = file.flush();
                0
            } else {
                ERROR_INVALID_HANDLE
            }
        }
    }

    /// DosFlatToSel (ordinal 425): convert 32-bit flat address to 16:16 sel:off
    /// In our flat memory model, just return the address as-is in the output
    pub fn dos_flat_to_sel(&self, flat_addr: u32) -> u32 {
        debug!("DosFlatToSel(0x{:08X})", flat_addr);
        // In OS/2, this converts a 0:32 flat pointer to a 16:16 selector:offset pointer
        // Since we operate in flat mode, we return the address itself
        // The return value goes into EAX
        flat_addr
    }

    /// DosSelToFlat (ordinal 426): convert 16:16 sel:off to 32-bit flat address
    pub fn dos_sel_to_flat(&self, sel_off: u32) -> u32 {
        debug!("DosSelToFlat(0x{:08X})", sel_off);
        // Same as above - in flat mode, addresses are the same
        sel_off
    }

    /// DosGetInfoSeg (ordinal 8): 16-bit API to get global/local info segments
    pub fn dos_get_info_seg(&self, p_global_sel: u32, p_local_sel: u32) -> u32 {
        debug!("DosGetInfoSeg(pGlobal=0x{:08X}, pLocal=0x{:08X})", p_global_sel, p_local_sel);
        // Return TIB/PIB-area selectors (as flat addresses in our model)
        if p_global_sel != 0 {
            let _ = self.guest_write::<u16>(p_global_sel, (PIB_BASE >> 4) as u16);
        }
        if p_local_sel != 0 {
            let _ = self.guest_write::<u16>(p_local_sel, (TIB_BASE >> 4) as u16);
        }
        0
    }

    /// DOSQFILEMODE (ordinal 75): 16-bit query file mode
    pub fn dos_query_file_mode_16(&self, p_filename: u32, p_attr: u32) -> u32 {
        let filename = self.read_guest_string(p_filename);
        debug!("DosQFileMode('{}', pAttr=0x{:08X})", filename, p_attr);
        let path = match self.translate_path(&filename) {
            Ok(p) => p,
            Err(_) => return ERROR_FILE_NOT_FOUND,
        };
        match fs::metadata(&path) {
            Ok(md) => {
                let mut attr: u16 = 0;
                if md.is_dir() { attr |= 0x10; }
                if md.permissions().readonly() { attr |= 0x01; }
                if p_attr != 0 {
                    let _ = self.guest_write::<u16>(p_attr, attr);
                }
                0
            }
            Err(_) => ERROR_FILE_NOT_FOUND,
        }
    }
}
