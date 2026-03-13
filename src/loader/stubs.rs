// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 stub and minimal API implementations for initialization-time APIs.
// These are needed to get text-mode apps like 4OS2 past their init sequence.

use log::{debug, warn};

use super::constants::*;
use super::mutex_ext::MutexExt;

impl super::Loader {
    // ── Step 1: Critical Init Stubs ──

    /// DosError (ordinal 212): enable/disable hard error popups.
    pub fn dos_error(&self, flag: u32) -> u32 {
        debug!("  DosError({})", flag);
        NO_ERROR
    }

    /// DosSetMaxFH (ordinal 291): set maximum file handles.
    pub fn dos_set_max_fh(&self, count: u32) -> u32 {
        debug!("  DosSetMaxFH({})", count);
        NO_ERROR
    }

    /// DosBeep (ordinal 286): produce an audible beep.
    pub fn dos_beep(&self, freq: u32, dur: u32) -> u32 {
        debug!("  DosBeep(freq={}, dur={})", freq, dur);
        print!("\x07"); // BEL character
        NO_ERROR
    }

    /// DosSetExceptionHandler (ordinal 354): install exception handler.
    /// Minimal: store the handler registration record address but don't invoke it.
    pub fn dos_set_exception_handler(&self, preg_rec: u32) -> u32 {
        debug!("  DosSetExceptionHandler(pRegRec=0x{:08X})", preg_rec);
        NO_ERROR
    }

    /// DosUnsetExceptionHandler (ordinal 355): remove exception handler.
    pub fn dos_unset_exception_handler(&self, preg_rec: u32) -> u32 {
        debug!("  DosUnsetExceptionHandler(pRegRec=0x{:08X})", preg_rec);
        NO_ERROR
    }

    /// DosSetSignalExceptionFocus (ordinal 356).
    pub fn dos_set_signal_exception_focus(&self, flag: u32) -> u32 {
        debug!("  DosSetSignalExceptionFocus({})", flag);
        NO_ERROR
    }

    /// DosAcknowledgeSignalException (ordinal 418).
    pub fn dos_acknowledge_signal_exception(&self, signal: u32) -> u32 {
        debug!("  DosAcknowledgeSignalException({})", signal);
        NO_ERROR
    }

    /// DosQuerySysState (ordinal 378): query system state info.
    /// Stub — returns ERROR_INVALID_FUNCTION since we don't have real system state.
    pub fn dos_query_sys_state(&self, func: u32, arg1: u32, pid: u32, p_buf: u32, cb_buf: u32) -> u32 {
        debug!("  DosQuerySysState(func={}, arg1={}, pid={}, pBuf=0x{:08X}, cbBuf={})", func, arg1, pid, p_buf, cb_buf);
        ERROR_INVALID_FUNCTION
    }

    /// DosEnterMustComplete (ordinal 380).
    pub fn dos_enter_must_complete(&self, p_nesting: u32) -> u32 {
        debug!("  DosEnterMustComplete");
        if p_nesting != 0 {
            self.guest_write::<u32>(p_nesting, 1);
        }
        NO_ERROR
    }

    /// DosExitMustComplete (ordinal 381).
    pub fn dos_exit_must_complete(&self, p_nesting: u32) -> u32 {
        debug!("  DosExitMustComplete");
        if p_nesting != 0 {
            self.guest_write::<u32>(p_nesting, 0);
        }
        NO_ERROR
    }

    // ── Step 2: Shared Memory ──

    /// DosAllocSharedMem (ordinal 300): allocate shared memory.
    /// Delegates to existing MemoryManager; if named, registers in SharedMemManager.
    pub fn dos_alloc_shared_mem(&self, ppb: u32, psz_name: u32, cb: u32, _flag: u32) -> u32 {
        let name = if psz_name != 0 {
            let n = self.read_guest_string(psz_name);
            if !n.is_empty() { Some(n) } else { None }
        } else {
            None
        };
        debug!("  DosAllocSharedMem(name={:?}, cb={})", name, cb);
        match self.shared.mem_mgr.lock_or_recover().alloc(cb) {
            Some(addr) => {
                self.guest_write::<u32>(ppb, addr);
                if let Some(name) = name {
                    self.shared.shmem_mgr.lock_or_recover().register(name, addr);
                }
                NO_ERROR
            }
            None => ERROR_NOT_ENOUGH_MEMORY,
        }
    }

    /// DosGetNamedSharedMem (ordinal 301): get existing shared memory by name.
    pub fn dos_get_named_shared_mem(&self, ppb: u32, psz_name: u32, _flag: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosGetNamedSharedMem(name='{}')", name);
        if let Some(addr) = self.shared.shmem_mgr.lock_or_recover().find_by_name(&name) {
            self.guest_write::<u32>(ppb, addr);
            NO_ERROR
        } else {
            ERROR_FILE_NOT_FOUND
        }
    }

    /// DosGetSharedMem (ordinal 302): get access to shared memory.
    /// In our flat model all memory is already accessible.
    pub fn dos_get_shared_mem(&self, _pb: u32, _flag: u32) -> u32 {
        NO_ERROR
    }

    /// DosSetMem (ordinal 305): commit/decommit pages.
    /// All memory is already committed in our flat model.
    pub fn dos_set_mem(&self, _pb: u32, _cb: u32, _flag: u32) -> u32 {
        NO_ERROR
    }

    /// DosQueryMem (ordinal 306): query memory region attributes.
    pub fn dos_query_mem(&self, pb: u32, pcb: u32, p_flag: u32) -> u32 {
        debug!("  DosQueryMem(pb=0x{:08X})", pb);
        // Return page-aligned size and committed+read+write flags
        if pcb != 0 {
            self.guest_write::<u32>(pcb, 4096);
        }
        if p_flag != 0 {
            // PAG_COMMIT=0x10, PAG_READ=0x01, PAG_WRITE=0x02
            self.guest_write::<u32>(p_flag, 0x13);
        }
        NO_ERROR
    }

    // ── Step 3: Codepage and Country Info ──

    /// DosQueryCp (ordinal 291): get active codepage.
    /// NOTE: ordinal 291 is shared; see dispatch comment.
    pub fn dos_query_cp(&self, cb: u32, p_cp_list: u32, pcb: u32) -> u32 {
        debug!("  DosQueryCp");
        if p_cp_list != 0 && cb >= 4 {
            self.guest_write::<u32>(p_cp_list, 437); // CP 437 (US)
        }
        if pcb != 0 {
            self.guest_write::<u32>(pcb, 4);
        }
        NO_ERROR
    }

    /// DosSetProcessCp (ordinal 289).
    pub fn dos_set_process_cp(&self, cp: u32) -> u32 {
        debug!("  DosSetProcessCp({})", cp);
        NO_ERROR
    }

    /// DosQueryCtryInfo (ordinal 397): get country/locale information.
    pub fn dos_query_ctry_info(&self, cb: u32, _p_ctry_code: u32, p_ctry_info: u32, pcb_actual: u32) -> u32 {
        debug!("  DosQueryCtryInfo");
        if p_ctry_info != 0 && cb >= 24 {
            // COUNTRYINFO struct: US defaults
            self.guest_write::<u32>(p_ctry_info, 1);       // country
            self.guest_write::<u32>(p_ctry_info + 4, 437);  // codepage
            self.guest_write::<u32>(p_ctry_info + 8, 0);    // fsDateFmt: 0=MDY
            self.guest_write::<u8>(p_ctry_info + 12, b'$'); // szCurrency
            self.guest_write::<u8>(p_ctry_info + 13, 0);
            self.guest_write::<u8>(p_ctry_info + 18, b','); // szThousandsSeparator
            self.guest_write::<u8>(p_ctry_info + 19, 0);
            self.guest_write::<u8>(p_ctry_info + 20, b'.'); // szDecimal
            self.guest_write::<u8>(p_ctry_info + 21, 0);
        }
        if pcb_actual != 0 {
            self.guest_write::<u32>(pcb_actual, 24);
        }
        NO_ERROR
    }

    /// DosMapCase (ordinal 305): case mapping. ASCII toupper for now.
    /// NOTE: This is listed under ordinal 305 in some references but may conflict with DosSetMem.
    /// Verify actual ordinal before dispatching.
    pub fn dos_map_case(&self, cb: u32, _p_ctry_code: u32, p_str: u32) -> u32 {
        debug!("  DosMapCase(cb={}, pStr=0x{:08X})", cb, p_str);
        if p_str != 0 {
            for i in 0..cb {
                if let Some(b) = self.guest_read::<u8>(p_str + i) {
                    if b >= b'a' && b <= b'z' {
                        self.guest_write::<u8>(p_str + i, b - 32);
                    }
                }
            }
        }
        NO_ERROR
    }

    // ── Step 4: Module Loading Stubs ──

    /// DosLoadModule (ordinal 318): load a DLL.
    pub fn dos_load_module(&self, psz_fail_name: u32, cb_fail_name: u32, psz_mod_name: u32, phmod: u32) -> u32 {
        let name = self.read_guest_string(psz_mod_name);
        debug!("  DosLoadModule('{}')", name);
        // Write the failing module name to the error buffer
        if psz_fail_name != 0 && cb_fail_name > 0 {
            let bytes = name.as_bytes();
            let copy_len = bytes.len().min(cb_fail_name as usize - 1);
            self.guest_write_bytes(psz_fail_name, &bytes[..copy_len]);
            self.guest_write::<u8>(psz_fail_name + copy_len as u32, 0);
        }
        if phmod != 0 {
            self.guest_write::<u32>(phmod, 0);
        }
        ERROR_MOD_NOT_FOUND
    }

    /// DosFreeModule (ordinal 322): free a loaded DLL.
    pub fn dos_free_module(&self, hmod: u32) -> u32 {
        debug!("  DosFreeModule({})", hmod);
        NO_ERROR
    }

    /// DosQueryModuleHandle (ordinal 319): query module handle.
    pub fn dos_query_module_handle(&self, psz_mod_name: u32, phmod: u32) -> u32 {
        let name = self.read_guest_string(psz_mod_name);
        debug!("  DosQueryModuleHandle('{}')", name);
        if phmod != 0 {
            self.guest_write::<u32>(phmod, 0);
        }
        ERROR_MOD_NOT_FOUND
    }

    /// DosQueryProcAddr (ordinal 321): resolve function address from DLL.
    pub fn dos_query_proc_addr(&self, hmod: u32, ordinal: u32, psz_name: u32, p_pfn: u32) -> u32 {
        let name = if psz_name != 0 { self.read_guest_string(psz_name) } else { String::new() };
        debug!("  DosQueryProcAddr(hmod={}, ord={}, name='{}')", hmod, ordinal, name);
        if p_pfn != 0 {
            self.guest_write::<u32>(p_pfn, 0);
        }
        ERROR_PROC_NOT_FOUND
    }

    /// DosGetMessage (ordinal 317): get message from MSG file.
    pub fn dos_get_message(&self, _p_table: u32, _c_table: u32, p_buf: u32, cb_buf: u32,
                           msg_num: u32, psz_file: u32, pcb_msg: u32) -> u32 {
        let file = self.read_guest_string(psz_file);
        debug!("  DosGetMessage(file='{}', msg={})", file, msg_num);
        // Write a placeholder message
        let msg = format!("SYS{:04}: Message not available", msg_num);
        let bytes = msg.as_bytes();
        let copy_len = bytes.len().min(cb_buf as usize);
        if p_buf != 0 && copy_len > 0 {
            self.guest_write_bytes(p_buf, &bytes[..copy_len]);
        }
        if pcb_msg != 0 {
            self.guest_write::<u32>(pcb_msg, copy_len as u32);
        }
        NO_ERROR
    }

    // ── Step 5: File Metadata APIs ──

    /// DosCopy (ordinal 258): copy a file.
    pub fn dos_copy(&self, psz_src: u32, psz_dst: u32, _option: u32) -> u32 {
        let src = self.read_guest_string(psz_src);
        let dst = self.read_guest_string(psz_dst);
        let src_path = match self.translate_path(&src) { Ok(p) => p, Err(e) => return e };
        let dst_path = match self.translate_path(&dst) { Ok(p) => p, Err(e) => return e };
        match std::fs::copy(src_path, dst_path) {
            Ok(_) => NO_ERROR,
            Err(_) => ERROR_FILE_NOT_FOUND,
        }
    }

    /// DosForceDelete (ordinal 259): delete without undelete.
    pub fn dos_force_delete(&self, psz_name: u32) -> u32 {
        self.dos_delete(psz_name) // alias to existing delete
    }

    /// DosEditName (ordinal 261): wildcard filename transformation.
    /// Transforms source name using edit string pattern (e.g., *.txt + *.bak → file.bak).
    pub fn dos_edit_name(&self, _meta_level: u32, psz_source: u32, psz_edit: u32, psz_target: u32, cb_target: u32) -> u32 {
        let source = self.read_guest_string(psz_source);
        let edit = self.read_guest_string(psz_edit);
        debug!("  DosEditName('{}', '{}')", source, edit);

        let result = Self::apply_edit_name(&source, &edit);
        let bytes = result.as_bytes();
        let copy_len = bytes.len().min(cb_target as usize - 1);
        if psz_target != 0 && copy_len > 0 {
            self.guest_write_bytes(psz_target, &bytes[..copy_len]);
            self.guest_write::<u8>(psz_target + copy_len as u32, 0);
        }
        NO_ERROR
    }

    /// Apply OS/2 DosEditName wildcard transformation.
    /// '*' in edit copies rest of source component, '?' copies one char from source.
    pub(crate) fn apply_edit_name(source: &str, edit: &str) -> String {
        let mut result = String::new();
        let src_bytes = source.as_bytes();
        let mut si = 0;
        for &eb in edit.as_bytes() {
            match eb {
                b'*' => {
                    // Copy from source until '.' or end
                    while si < src_bytes.len() && src_bytes[si] != b'.' {
                        result.push(src_bytes[si] as char);
                        si += 1;
                    }
                }
                b'?' => {
                    if si < src_bytes.len() && src_bytes[si] != b'.' {
                        result.push(src_bytes[si] as char);
                        si += 1;
                    }
                }
                b'.' => {
                    result.push('.');
                    // Advance source past '.'
                    while si < src_bytes.len() && src_bytes[si] != b'.' {
                        si += 1;
                    }
                    if si < src_bytes.len() {
                        si += 1; // skip the '.'
                    }
                }
                _ => {
                    result.push(eb as char);
                    if si < src_bytes.len() && src_bytes[si] != b'.' {
                        si += 1;
                    }
                }
            }
        }
        result
    }

    /// DosSetFileInfo (ordinal 279): set file timestamps/attributes.
    pub fn dos_set_file_info(&self, hf: u32, level: u32, _p_info: u32, _cb_info: u32) -> u32 {
        debug!("  DosSetFileInfo(hf={}, level={})", hf, level);
        NO_ERROR
    }

    /// DosSetFileMode (ordinal 267): set file attributes.
    pub fn dos_set_file_mode(&self, psz_name: u32, attr: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosSetFileMode('{}', 0x{:04X})", name, attr);
        NO_ERROR
    }

    /// DosSetPathInfo (ordinal 219): set path timestamps/attributes.
    pub fn dos_set_path_info(&self, psz_name: u32, level: u32, _p_info: u32, _cb_info: u32, _flag: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosSetPathInfo('{}', level={})", name, level);
        NO_ERROR
    }

    /// DosQueryFHState (ordinal 276): get file handle state.
    pub fn dos_query_fh_state(&self, hf: u32, p_mode: u32) -> u32 {
        debug!("  DosQueryFHState(hf={})", hf);
        if p_mode != 0 {
            self.guest_write::<u32>(p_mode, 0);
        }
        NO_ERROR
    }

    /// DosSetFHState (ordinal 277): set file handle state.
    pub fn dos_set_fh_state(&self, hf: u32, mode: u32) -> u32 {
        debug!("  DosSetFHState(hf={}, mode=0x{:04X})", hf, mode);
        NO_ERROR
    }

    /// DosQueryFSInfo (ordinal 278 at level 1/2): get disk info.
    /// Note: ordinal 278 is already used for DosQueryFileInfo (level-based).
    /// DosQueryFSInfo has a different ordinal; verify before dispatch.
    pub fn dos_query_fs_info(&self, drive: u32, level: u32, p_buf: u32, cb_buf: u32) -> u32 {
        debug!("  DosQueryFSInfo(drive={}, level={})", drive, level);
        if level == 1 && p_buf != 0 && cb_buf >= 18 {
            // FSALLOCATE struct
            self.guest_write::<u32>(p_buf, 0);          // idFileSystem
            self.guest_write::<u32>(p_buf + 4, 100000);  // cSectorUnit (sectors per alloc unit)
            self.guest_write::<u32>(p_buf + 8, 500000);  // cUnit (total alloc units)
            self.guest_write::<u32>(p_buf + 12, 250000); // cUnitAvail (free alloc units)
            self.guest_write::<u16>(p_buf + 16, 512);    // cbSector (bytes per sector)
        } else if level == 2 && p_buf != 0 && cb_buf >= 16 {
            // FSINFO struct with volume label
            self.guest_write::<u32>(p_buf, 0);  // volume serial
            self.guest_write::<u8>(p_buf + 4, 7); // label length
            self.guest_write_bytes(p_buf + 5, b"WARPINE");
        }
        NO_ERROR
    }

    /// DosQueryFSAttach (ordinal 277): query filesystem type.
    /// Returns basic drive info.
    pub fn dos_query_fs_attach(&self, psz_dev: u32, _ordinal: u32, level: u32, p_buf: u32, pcb_buf: u32) -> u32 {
        let dev = if psz_dev != 0 { self.read_guest_string(psz_dev) } else { String::new() };
        debug!("  DosQueryFSAttach('{}', level={})", dev, level);
        if p_buf != 0 {
            // FSQBUFFER2 minimal: iType=3 (local drive), szName, szFSDName
            self.guest_write::<u16>(p_buf, 3); // iType = local
            self.guest_write::<u16>(p_buf + 2, 2); // cbName
            self.guest_write_bytes(p_buf + 4, b"C:\0");
            self.guest_write::<u16>(p_buf + 7, 4); // cbFSDName
            self.guest_write_bytes(p_buf + 9, b"FAT\0");
        }
        if pcb_buf != 0 {
            self.guest_write::<u32>(pcb_buf, 13);
        }
        NO_ERROR
    }

    /// DosQueryVerify: check verify-after-write state.
    pub fn dos_query_verify(&self, p_flag: u32) -> u32 {
        if p_flag != 0 {
            self.guest_write::<u32>(p_flag, 0); // verify off
        }
        NO_ERROR
    }

    /// DosSetVerify: set verify-after-write mode.
    pub fn dos_set_verify(&self, _flag: u32) -> u32 {
        NO_ERROR
    }

    // ── Step 6: Device I/O Stubs ──

    /// DosDevIOCtl (ordinal 284): device I/O control.
    pub fn dos_dev_ioctl(&self, hdev: u32, category: u32, function: u32, _p_params: u32, _cb_params: u32,
                         _pcb_params: u32, _p_data: u32, _cb_data: u32, _pcb_data: u32) -> u32 {
        debug!("  DosDevIOCtl(hdev={}, cat={}, func={})", hdev, category, function);
        ERROR_INVALID_FUNCTION
    }

    /// DosDevConfig (ordinal 231): get device configuration.
    pub fn dos_dev_config(&self, p_info: u32, item: u32) -> u32 {
        debug!("  DosDevConfig(item={})", item);
        if p_info != 0 {
            let val: u8 = match item {
                0 => 0, // number of printers
                1 => 0, // number of RS232 ports
                2 => 0, // number of diskette drives
                3 => 1, // math coprocessor present
                4 => 0, // PC submodel type
                5 => 0, // PC model type
                6 => 1, // number of disk drives
                _ => 0,
            };
            self.guest_write::<u8>(p_info, val);
        }
        NO_ERROR
    }

    // ── Step 7: Semaphore Extensions ──

    /// DosOpenEventSem (ordinal 325): open existing event semaphore by name.
    pub fn dos_open_event_sem(&self, psz_name: u32, phev: u32) -> u32 {
        if psz_name != 0 {
            let name = self.read_guest_string(psz_name);
            debug!("  DosOpenEventSem('{}')", name);
            let sem_mgr = self.shared.sem_mgr.lock_or_recover();
            if let Some(h) = sem_mgr.open_event_by_name(&name) {
                self.guest_write::<u32>(phev, h);
                return NO_ERROR;
            }
            return ERROR_FILE_NOT_FOUND;
        }
        // If name is null, phev should already contain a valid handle
        let h = self.guest_read::<u32>(phev).unwrap_or(0);
        debug!("  DosOpenEventSem(handle={})", h);
        let sem_mgr = self.shared.sem_mgr.lock_or_recover();
        if sem_mgr.get_event(h).is_some() { NO_ERROR } else { ERROR_INVALID_HANDLE }
    }

    /// DosOpenMutexSem (ordinal 332): open existing mutex semaphore by name.
    pub fn dos_open_mutex_sem(&self, psz_name: u32, phmtx: u32) -> u32 {
        if psz_name != 0 {
            let name = self.read_guest_string(psz_name);
            debug!("  DosOpenMutexSem('{}')", name);
            let sem_mgr = self.shared.sem_mgr.lock_or_recover();
            if let Some(h) = sem_mgr.open_mutex_by_name(&name) {
                self.guest_write::<u32>(phmtx, h);
                return NO_ERROR;
            }
            return ERROR_FILE_NOT_FOUND;
        }
        let h = self.guest_read::<u32>(phmtx).unwrap_or(0);
        debug!("  DosOpenMutexSem(handle={})", h);
        let sem_mgr = self.shared.sem_mgr.lock_or_recover();
        if sem_mgr.get_mutex(h).is_some() { NO_ERROR } else { ERROR_INVALID_HANDLE }
    }

    // ── Step 8: Named Pipe Stubs ──

    pub fn dos_create_npipe(&self, psz_name: u32, _ph: u32, _open_mode: u32, _pipe_mode: u32, _out_sz: u32, _in_sz: u32, _timeout: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        warn!("  DosCreateNPipe('{}') - not implemented", name);
        ERROR_INVALID_FUNCTION
    }

    pub fn dos_connect_npipe(&self, _h: u32) -> u32 {
        warn!("  DosConnectNPipe - not implemented");
        ERROR_INVALID_FUNCTION
    }

    pub fn dos_set_nph_state(&self, _h: u32, _state: u32) -> u32 {
        warn!("  DosSetNPHState - not implemented");
        ERROR_INVALID_FUNCTION
    }

    // ── Step 9: Session Management Stubs ──

    pub fn dos_start_session(&self, _p_start_data: u32, _p_id_session: u32, _p_pid: u32) -> u32 {
        warn!("  DosStartSession - not implemented");
        ERROR_INVALID_FUNCTION
    }

    pub fn dos_set_session(&self, _id_session: u32, _p_status: u32) -> u32 {
        warn!("  DosSetSession - not implemented");
        ERROR_INVALID_FUNCTION
    }

    pub fn dos_stop_session(&self, _scope: u32, _id_session: u32) -> u32 {
        warn!("  DosStopSession - not implemented");
        ERROR_INVALID_FUNCTION
    }

    // ── System Information (needed by 4OS2 init, also used by Subsystem 2) ──

    /// DosQuerySysInfo (ordinal 348): query system information.
    /// Returns an array of ULONG values for QSV_* indexes iStart through iLast.
    pub fn dos_query_sys_info(&self, i_start: u32, i_last: u32, p_buf: u32, cb_buf: u32) -> u32 {
        debug!("  DosQuerySysInfo(iStart={}, iLast={})", i_start, i_last);
        if i_start == 0 || i_last < i_start {
            return ERROR_INVALID_LEVEL;
        }
        let count = (i_last - i_start + 1) as usize;
        if cb_buf < (count * 4) as u32 {
            return ERROR_BUFFER_OVERFLOW;
        }
        for idx in i_start..=i_last {
            let val: u32 = match idx {
                QSV_MAX_PATH_LENGTH => 260,
                QSV_MAX_TEXT_SESSIONS => 16,
                QSV_MAX_PM_SESSIONS => 16,
                QSV_MAX_VDM_SESSIONS => 0,
                QSV_BOOT_DRIVE => 3,         // C: (1=A, 2=B, 3=C)
                QSV_DYN_PRI_VARIATION => 1,
                QSV_MAX_WAIT => 5,           // seconds
                QSV_MIN_SLICE => 32,         // milliseconds
                QSV_MAX_SLICE => 248,        // milliseconds
                QSV_PAGE_SIZE => 4096,
                QSV_VERSION_MAJOR => 20,     // OS/2 Warp 4
                QSV_VERSION_MINOR => 45,
                QSV_VERSION_REVISION => 0,
                QSV_TOTPHYSMEM => 128 * 1024 * 1024,
                QSV_TOTRESMEM => 16 * 1024 * 1024,
                QSV_TOTAVAILMEM => 96 * 1024 * 1024,
                QSV_MAXPRMEM => 512 * 1024 * 1024, // max private memory
                QSV_MAXSHMEM => 256 * 1024 * 1024,  // max shared memory
                QSV_TIMER_INTERVAL => 32,            // milliseconds
                QSV_MAX_COMP_LENGTH => 255,
                _ => 0, // unknown index
            };
            let offset = (idx - i_start) * 4;
            self.guest_write::<u32>(p_buf + offset, val);
        }
        NO_ERROR
    }

    /// DosGetDateTime (ordinal 230): get current date and time.
    pub fn dos_get_date_time(&self, p_dt: u32) -> u32 {
        debug!("  DosGetDateTime");
        if p_dt == 0 { return ERROR_INVALID_FUNCTION; }

        // Use libc to get broken-down time
        let mut tv = libc::timeval { tv_sec: 0, tv_usec: 0 };
        unsafe { libc::gettimeofday(&mut tv, std::ptr::null_mut()); }
        let time_t = tv.tv_sec;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&time_t, &mut tm); }

        // OS/2 DATETIME struct layout (12 bytes):
        //   0: UCHAR hours, 1: UCHAR minutes, 2: UCHAR seconds, 3: UCHAR hundredths
        //   4: UCHAR day, 5: UCHAR month, 6-7: USHORT year
        //   8-9: SHORT timezone, 10: UCHAR weekday
        self.guest_write::<u8>(p_dt, tm.tm_hour as u8);
        self.guest_write::<u8>(p_dt + 1, tm.tm_min as u8);
        self.guest_write::<u8>(p_dt + 2, tm.tm_sec as u8);
        self.guest_write::<u8>(p_dt + 3, (tv.tv_usec / 10000) as u8); // hundredths
        self.guest_write::<u8>(p_dt + 4, tm.tm_mday as u8);
        self.guest_write::<u8>(p_dt + 5, (tm.tm_mon + 1) as u8); // tm_mon is 0-based
        self.guest_write::<u16>(p_dt + 6, (tm.tm_year + 1900) as u16);
        self.guest_write::<i16>(p_dt + 8, (tm.tm_gmtoff / 60) as i16); // timezone in minutes
        self.guest_write::<u8>(p_dt + 10, tm.tm_wday as u8);
        NO_ERROR
    }

    /// DosSetDateTime (ordinal 231): set system date/time. Stub.
    pub fn dos_set_date_time(&self, _p_dt: u32) -> u32 {
        debug!("  DosSetDateTime (stub)");
        NO_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::super::Loader;

    #[test]
    fn test_edit_name_star_extension() {
        assert_eq!(Loader::apply_edit_name("readme.txt", "*.bak"), "readme.bak");
    }

    #[test]
    fn test_edit_name_star_star() {
        assert_eq!(Loader::apply_edit_name("hello.exe", "*.*"), "hello.exe");
    }

    #[test]
    fn test_edit_name_question_mark() {
        assert_eq!(Loader::apply_edit_name("abc.txt", "??x.*"), "abx.txt");
    }

    #[test]
    fn test_edit_name_literal() {
        assert_eq!(Loader::apply_edit_name("anything.c", "output.o"), "output.o");
    }

    #[test]
    fn test_edit_name_no_extension() {
        assert_eq!(Loader::apply_edit_name("makefile", "*.bak"), "makefile.bak");
    }

    #[test]
    fn test_qsv_constants_valid_ranges() {
        use super::super::constants::*;
        // QSV indexes are 1-based and sequential ranges should work
        assert_eq!(QSV_MAX_PATH_LENGTH, 1);
        assert_eq!(QSV_BOOT_DRIVE, 5);
        assert_eq!(QSV_PAGE_SIZE, 10);
        assert_eq!(QSV_VERSION_MAJOR, 11);
        assert_eq!(QSV_VERSION_MINOR, 12);
        assert_eq!(QSV_VERSION_REVISION, 13);
        assert_eq!(QSV_TOTPHYSMEM, 17);
        assert_eq!(QSV_MAX_COMP_LENGTH, 23);
        // Ensure range queries would work: iStart <= iLast
        assert!(QSV_VERSION_MAJOR < QSV_VERSION_REVISION);
        assert!(QSV_MAX_PATH_LENGTH < QSV_MAX_COMP_LENGTH);
    }
}
