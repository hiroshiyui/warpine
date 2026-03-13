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
        thread::sleep(std::time::Duration::from_millis(msec as u64));
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

    pub fn dos_alloc_mem(&self, ppb: u32, cb: u32) -> u32 {
        match self.shared.mem_mgr.lock_or_recover().alloc(cb) {
            Some(addr) => {
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
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { return 640; } // ERROR_TIMEOUT
                let (guard, result) = cvar.wait_timeout(sem, remaining).unwrap();
                sem = guard;
                if result.timed_out() && !sem.posted { return 640; }
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
                        let (guard, result) = cvar.wait_timeout(sem, remaining).unwrap();
                        sem = guard;
                        if result.timed_out() { continue; } // re-check ownership after timeout
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

    pub fn dos_wait_thread(&self, vcpu_id: u32, ptid_ptr: u32) -> u32 {
        let tid = self.guest_read::<u32>(ptid_ptr).unwrap_or(0);
        debug!("  [VCPU {}] Waiting for thread {}...", vcpu_id, tid);
        let mut handle = None;
        for _ in 0..100 {
            handle = self.shared.threads.lock_or_recover().remove(&tid);
            if handle.is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if let Some(h) = handle {
            h.join().unwrap();
            0
        } else { 309 }
    }
}
