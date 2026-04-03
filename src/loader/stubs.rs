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

    /// DosBeep (ordinal 286): produce an audible beep via SDL2 audio.
    pub fn dos_beep(&self, freq: u32, dur: u32) -> u32 {
        debug!("  DosBeep(freq={}, dur={})", freq, dur);
        super::mmpm::beep_tone(freq, dur);
        NO_ERROR
    }

    /// DosSetExceptionHandler (ordinal 354): push a handler record onto the
    /// per-thread exception chain at TIB+0x00 (tib_pexchain).
    ///
    /// Sets `preg_rec->prev_structure = TIB[0x00]`, then `TIB[0x00] = preg_rec`.
    pub fn dos_set_exception_handler(&self, preg_rec: u32) -> u32 {
        debug!("  DosSetExceptionHandler(pRegRec=0x{:08X})", preg_rec);
        if preg_rec == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let cur_head = self.guest_read::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET)
            .unwrap_or(XCPT_CHAIN_END);
        // Link new record to the current chain head.
        self.guest_write::<u32>(preg_rec + XERREC_PREV, cur_head).unwrap();
        // New record becomes the chain head.
        self.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, preg_rec).unwrap();
        NO_ERROR
    }

    /// DosUnsetExceptionHandler (ordinal 355): pop a handler record from the
    /// per-thread exception chain at TIB+0x00 (tib_pexchain).
    ///
    /// Sets `TIB[0x00] = preg_rec->prev_structure`.
    pub fn dos_unset_exception_handler(&self, preg_rec: u32) -> u32 {
        debug!("  DosUnsetExceptionHandler(pRegRec=0x{:08X})", preg_rec);
        if preg_rec == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let prev = self.guest_read::<u32>(preg_rec + XERREC_PREV)
            .unwrap_or(XCPT_CHAIN_END);
        self.guest_write::<u32>(TIB_BASE + TIB_EXCHAIN_OFFSET, prev).unwrap();
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
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        debug!("  DosQueryCp(cp={})", cp);
        if p_cp_list != 0 && cb >= 4 {
            self.guest_write::<u32>(p_cp_list, cp);
        }
        if pcb != 0 {
            self.guest_write::<u32>(pcb, 4);
        }
        NO_ERROR
    }

    /// DosSetProcessCp (ordinal 289): set the active codepage for this process.
    /// Validates against the supported list (437, 850, 852, 932, 949, 950, 1250–1258).
    /// Returns ERROR_INVALID_CODE_PAGE (470) for unrecognised values.
    pub fn dos_set_process_cp(&self, cp: u32) -> u32 {
        const SUPPORTED: &[u32] = &[437, 850, 852, 932, 949, 950, 1250, 1251, 1252, 1253, 1254, 1255, 1256, 1257, 1258];
        if !SUPPORTED.contains(&cp) {
            debug!("  DosSetProcessCp({}) → ERROR_INVALID_CODE_PAGE", cp);
            return ERROR_INVALID_CODE_PAGE;
        }
        self.shared.active_codepage.store(cp, std::sync::atomic::Ordering::Relaxed);
        debug!("  DosSetProcessCp({}) → NO_ERROR", cp);
        NO_ERROR
    }

    /// DosQueryCtryInfo (ordinal 397): get country/locale information.
    pub fn dos_query_ctry_info(&self, cb: u32, _p_ctry_code: u32, p_ctry_info: u32, pcb_actual: u32) -> u32 {
        debug!("  DosQueryCtryInfo(cb={}, pcc=0x{:08X}, p_ctry_info=0x{:08X}, pcb_actual=0x{:08X})", cb, _p_ctry_code, p_ctry_info, pcb_actual);
        // COUNTRYINFO struct layout (38 bytes):
        // +0:  country (ULONG)
        // +4:  codepage (ULONG)
        // +8:  fsDateFmt (ULONG) 0=MDY, 1=DMY, 2=YMD
        // +12: szCurrency[5]
        // +17: szThousandsSeparator[2]
        // +19: szDecimal[2]
        // +21: szDateSeparator[2]
        // +23: szTimeSeparator[2]
        // +25: fsCurrencyFmt (UCHAR)
        // +26: cDecimalPlace (UCHAR)
        // +27: fsTimeFmt (UCHAR) 0=12-hour, 1=24-hour
        // +28: abReserved1[2] (USHORT×2)
        // +32: szDataSeparator[2]
        // +34: abReserved2[5] (USHORT×5)
        let locale = &self.shared.locale;
        let info_size = 44u32; // Full Watcom COUNTRYINFO size (including reserved fields)
        let write_size = cb.min(info_size);
        if p_ctry_info != 0 && write_size > 0 {
            // Zero-fill only the bytes we're allowed to write
            for i in 0..write_size {
                self.guest_write::<u8>(p_ctry_info + i, 0);
            }
            // Write fields from host locale, only if they fit within cb
            if write_size >= 4 { self.guest_write::<u32>(p_ctry_info, locale.country); }
            if write_size >= 8 { self.guest_write::<u32>(p_ctry_info + 4, locale.codepage); }
            if write_size >= 12 { self.guest_write::<u32>(p_ctry_info + 8, locale.date_fmt); }
            if write_size >= 17 { self.guest_write_bytes(p_ctry_info + 12, &locale.currency); }
            if write_size >= 19 { self.guest_write_bytes(p_ctry_info + 17, &[locale.thousands_sep, 0]); }
            if write_size >= 21 { self.guest_write_bytes(p_ctry_info + 19, &[locale.decimal_sep, 0]); }
            if write_size >= 23 { self.guest_write_bytes(p_ctry_info + 21, &[locale.date_sep, 0]); }
            if write_size >= 25 { self.guest_write_bytes(p_ctry_info + 23, &[locale.time_sep, 0]); }
            if write_size >= 26 { self.guest_write::<u8>(p_ctry_info + 25, locale.currency_fmt); }
            if write_size >= 27 { self.guest_write::<u8>(p_ctry_info + 26, locale.decimal_places); }
            if write_size >= 28 { self.guest_write::<u8>(p_ctry_info + 27, locale.time_fmt); }
            if write_size >= 34 { self.guest_write_bytes(p_ctry_info + 32, &[locale.data_sep, 0]); }
        }
        if pcb_actual != 0 {
            self.guest_write::<u32>(pcb_actual, info_size);
        }
        NO_ERROR
    }

    /// DosMapCase (ordinal 305): in-place uppercase of a guest byte string.
    ///
    /// Uses the active process codepage for non-ASCII bytes so that accented
    /// characters in CP850/CP852/CP866 (and Windows SBCS codepages) are uppercased
    /// correctly.  For DBCS codepages (CP932/936/949/950) adjacent lead+trail pairs
    /// are decoded, uppercased as a Unicode scalar, and re-encoded as a pair.
    /// Multi-char Unicode results (e.g. ß→SS) are left unchanged, matching OS/2's
    /// fixed-width NLS behaviour.
    pub fn dos_map_case(&self, cb: u32, _p_ctry_code: u32, p_str: u32) -> u32 {
        debug!("  DosMapCase(cb={}, pStr=0x{:08X})", cb, p_str);
        if p_str != 0 {
            let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
            self.map_case_guest_buf(p_str, cb, cp);
        }
        NO_ERROR
    }

    /// In-place uppercase of a guest byte buffer using the given codepage.
    ///
    /// SBCS codepages: maps each byte individually via `cp_map_case_upper`.
    /// DBCS codepages (CP932/936/949/950): when a lead byte is detected the
    /// following trail byte is consumed as a pair, decoded to Unicode, uppercased
    /// (1:1 only), and re-encoded; the pair index advances by 2.  An orphaned lead
    /// byte at the end of the buffer is left unchanged.
    pub fn map_case_guest_buf(&self, p_str: u32, cb: u32, cp: u32) {
        use super::codepage::{cp_map_case_upper, cp_to_encoding, decode_dbcs};
        use super::locale::is_dbcs_lead_byte;

        let mut i = 0u32;
        while i < cb {
            let Some(b) = self.guest_read::<u8>(p_str + i) else { break };

            if is_dbcs_lead_byte(b, cp) && i + 1 < cb {
                // DBCS pair: read trail byte and attempt Unicode case mapping.
                if let Some(trail) = self.guest_read::<u8>(p_str + i + 1) {
                    let ch = decode_dbcs(b, trail, cp);
                    if ch != '\u{FFFD}' {
                        let mut upper_it = ch.to_uppercase();
                        let upper_ch = upper_it.next().unwrap_or(ch);
                        let is_single = upper_it.next().is_none();
                        if is_single && upper_ch != ch
                            && let Some(enc) = cp_to_encoding(cp)
                        {
                            let s = upper_ch.to_string();
                            let (encoded, _, _) = enc.encode(&s);
                            if encoded.len() == 2 {
                                let _ = self.guest_write::<u8>(p_str + i, encoded[0]);
                                let _ = self.guest_write::<u8>(p_str + i + 1, encoded[1]);
                            }
                        }
                    }
                    i += 2;
                    continue;
                }
            }

            // SBCS byte (or orphaned DBCS lead at end of buffer).
            let upper = cp_map_case_upper(b, cp);
            if upper != b {
                let _ = self.guest_write::<u8>(p_str + i, upper);
            }
            i += 1;
        }
    }

    // ── Step 4: Module Loading Stubs ──

    /// DosLoadModule (ordinal 318): load a DLL.
    ///
    /// If the module is already loaded its reference count is incremented and
    /// the existing handle is returned.  Otherwise the DLL (and all of its
    /// imported user-DLL dependencies) is loaded recursively.
    ///
    /// When the loaded DLL has a `_DLL_InitTerm` entry point (LX eip_object != 0)
    /// this returns `ApiResult::CallGuest` so the vCPU injects a call to
    /// `_DLL_InitTerm(hmod, 0)` before resuming the guest.  The phmod output
    /// pointer is written only after INITTERM succeeds.
    pub(crate) fn dos_load_module(&self, psz_fail_name: u32, cb_fail_name: u32, psz_mod_name: u32, phmod: u32) -> super::ApiResult {
        use super::ApiResult;
        use super::managers::LoadedDll;
        use std::collections::HashMap;
        let name = self.read_guest_string(psz_mod_name);
        debug!("  DosLoadModule('{}')", name);

        // Builtin modules (UCONV, MDM, VIOCALLS, …) have no real DLL file on the
        // host.  Register a synthetic handle so DosQueryProcAddr can resolve their
        // ordinals to warpine thunk stubs at runtime.
        const BUILTINS: &[&str] = &[
            "DOSCALLS", "QUECALLS", "PMWIN", "PMGPI", "KBDCALLS",
            "VIOCALLS", "SESMGR", "NLS", "MSG", "MDM", "UCONV",
            "SO32DLL", "TCP32DLL",
        ];
        let name_upper = name.to_ascii_uppercase();
        let stem = name_upper.strip_suffix(".DLL").unwrap_or(&name_upper);
        if BUILTINS.contains(&stem) {
            let mut dll_mgr = self.shared.dll_mgr.lock_or_recover();
            // Reuse an existing registration (e.g. from a previous DosLoadModule call).
            if let Some(existing) = dll_mgr.find_by_name(stem) {
                let h = existing.handle;
                drop(dll_mgr);
                if phmod != 0 { let _ = self.guest_write::<u32>(phmod, h); }
                return ApiResult::Normal(NO_ERROR);
            }
            let h = dll_mgr.alloc_handle();
            dll_mgr.register(LoadedDll {
                name: stem.to_string(),
                handle: h,
                object_bases: vec![],
                exports_by_ordinal: HashMap::new(),
                exports_by_name: HashMap::new(),
                ref_count: 1,
                initterm_addr: None,
                is_builtin: true,
            });
            drop(dll_mgr);
            debug!("  DosLoadModule: registered builtin '{}' as hmod={:#x}", stem, h);
            if phmod != 0 { let _ = self.guest_write::<u32>(phmod, h); }
            return ApiResult::Normal(NO_ERROR);
        }

        // `load_dll` handles the "already loaded → increment refcount" case internally.
        let result = match self.find_dll_path(&name) {
            Some(path) => self.load_dll(&name, &path),
            None => Err(format!("'{}' not found on host", name)),
        };

        match result {
            Ok(h) => {
                // If the DLL has an INITTERM entry, inject the call before writing
                // the handle to *phmod — the handle is written by the vcpu handler
                // after INITTERM returns successfully.
                let initterm = {
                    let dll_mgr = self.shared.dll_mgr.lock_or_recover();
                    dll_mgr.find_by_handle(h).and_then(|d| d.initterm_addr)
                };
                if let Some(addr) = initterm {
                    debug!("  DosLoadModule: injecting _DLL_InitTerm(hmod={:#x}, 0) at 0x{:08X}", h, addr);
                    ApiResult::CallGuest { addr, hmod: h, phmod, object_bases: vec![] }
                } else {
                    if phmod != 0 { self.guest_write::<u32>(phmod, h); }
                    ApiResult::Normal(NO_ERROR)
                }
            }
            Err(e) => {
                log::warn!("DosLoadModule('{}') failed: {}", name, e);
                if psz_fail_name != 0 && cb_fail_name > 0 {
                    let bytes = name.as_bytes();
                    let copy_len = bytes.len().min(cb_fail_name as usize - 1);
                    self.guest_write_bytes(psz_fail_name, &bytes[..copy_len]);
                    self.guest_write::<u8>(psz_fail_name + copy_len as u32, 0);
                }
                if phmod != 0 { self.guest_write::<u32>(phmod, 0); }
                ApiResult::Normal(ERROR_MOD_NOT_FOUND)
            }
        }
    }

    /// DosFreeModule (ordinal 322): decrement a DLL's reference count.
    ///
    /// When the count reaches zero:
    /// - If the DLL has an INITTERM entry point, inject `_DLL_InitTerm(hmod, 1)`
    ///   via `ApiResult::CallGuest`; the vcpu handler frees pages after the call.
    /// - Otherwise free the guest memory pages immediately and return `NO_ERROR`.
    pub(crate) fn dos_free_module(&self, hmod: u32) -> super::ApiResult {
        debug!("  DosFreeModule(hmod={})", hmod);
        let freed = {
            let mut dll_mgr = self.shared.dll_mgr.lock_or_recover();
            dll_mgr.decrement_refcount(hmod)
        };
        match freed {
            Some((bases, Some(addr))) => {
                // DLL has INITTERM — inject _DLL_InitTerm(hmod, 1) before freeing.
                debug!("  DosFreeModule: hmod={} has INITTERM at {:#x}, injecting unload call", hmod, addr);
                super::ApiResult::CallGuest { addr, hmod, phmod: 0, object_bases: bases }
            }
            Some((bases, None)) => {
                // No INITTERM — free pages now.
                let mut mem = self.shared.mem_mgr.lock_or_recover();
                for base in bases {
                    mem.free(base);
                }
                debug!("  DosFreeModule: hmod={} unloaded", hmod);
                super::ApiResult::Normal(NO_ERROR)
            }
            None => super::ApiResult::Normal(NO_ERROR),
        }
    }

    /// DosQueryModuleHandle (ordinal 319): query module handle.
    pub fn dos_query_module_handle(&self, psz_mod_name: u32, phmod: u32) -> u32 {
        let name = self.read_guest_string(psz_mod_name);
        debug!("  DosQueryModuleHandle('{}')", name);
        let dll_mgr = self.shared.dll_mgr.lock_or_recover();
        if let Some(dll) = dll_mgr.find_by_name(&name) {
            if phmod != 0 { self.guest_write::<u32>(phmod, dll.handle); }
            NO_ERROR
        } else {
            if phmod != 0 { self.guest_write::<u32>(phmod, 0); }
            ERROR_MOD_NOT_FOUND
        }
    }

    /// DosQueryProcAddr (ordinal 321): resolve a function address from a loaded DLL.
    pub fn dos_query_proc_addr(&self, hmod: u32, ordinal: u32, psz_name: u32, p_pfn: u32) -> u32 {
        let name = if psz_name != 0 { self.read_guest_string(psz_name) } else { String::new() };
        debug!("  DosQueryProcAddr(hmod={:#x}, ord={}, name='{}')", hmod, ordinal, name);

        let (maybe_name, maybe_addr) = {
            let dll_mgr = self.shared.dll_mgr.lock_or_recover();
            let maybe_dll = dll_mgr.find_by_handle(hmod);
            if let Some(dll) = maybe_dll {
                if dll.is_builtin {
                    // Builtin: resolve to warpine thunk stub address below.
                    (Some(dll.name.clone()), None)
                } else if !name.is_empty() {
                    (None, dll.exports_by_name.get(&name.to_ascii_uppercase()).copied())
                } else if ordinal != 0 {
                    (None, dll.exports_by_ordinal.get(&ordinal).copied())
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        };

        // For builtin modules, compute the thunk stub address directly.
        let addr = if let Some(mod_name) = maybe_name {
            if ordinal != 0 {
                let thunk = self.resolve_import(&mod_name, ordinal);
                Some(thunk as u32)
            } else {
                None
            }
        } else {
            maybe_addr
        };

        if let Some(guest_addr) = addr {
            if p_pfn != 0 { let _ = self.guest_write::<u32>(p_pfn, guest_addr); }
            NO_ERROR
        } else {
            if p_pfn != 0 { let _ = self.guest_write::<u32>(p_pfn, 0); }
            ERROR_PROC_NOT_FOUND
        }
    }

    /// DosGetMessage (ordinal 317): get message from MSG file.
    #[allow(clippy::too_many_arguments)]
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

    /// DosPutMessage (MSG ordinal 3): write a message to a file handle.
    pub fn dos_put_message(&self, hf: u32, cb_msg: u32, p_msg: u32) -> u32 {
        debug!("  DosPutMessage(hf={}, cb={})", hf, cb_msg);
        if p_msg == 0 || cb_msg == 0 { return NO_ERROR; }
        // Route through dos_write so SDL2 text window and terminal both work
        self.dos_write(hf, p_msg, cb_msg, 0)
    }

    // ── Step 5: File Metadata APIs ──

    /// DosCopy (ordinal 258): copy a file.
    pub fn dos_copy(&self, psz_src: u32, psz_dst: u32, _option: u32) -> u32 {
        let src = self.read_guest_string(psz_src);
        let dst = self.read_guest_string(psz_dst);
        let dm = self.shared.drive_mgr.lock_or_recover();
        match dm.copy_file(&src, &dst) {
            Ok(()) => NO_ERROR,
            Err(e) => e.0,
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

    /// DosQueryFSInfo (ordinal 278): query filesystem allocation or volume info.
    ///
    /// drive: 0 = default, 1 = A:, 2 = B:, 3 = C:, …
    /// level 1 → FSALLOCATE (18 bytes); level 2 → FSINFO (volume label, 16+ bytes).
    pub fn dos_query_fs_info(&self, drive: u32, level: u32, p_buf: u32, cb_buf: u32) -> u32 {
        debug!("  DosQueryFSInfo(drive={}, level={})", drive, level);
        if p_buf == 0 { return ERROR_INVALID_FUNCTION; }

        // Resolve drive index (0 = current default)
        let drive_idx = if drive == 0 {
            self.shared.drive_mgr.lock_or_recover().current_disk()
        } else {
            (drive - 1) as u8
        };

        let dm = self.shared.drive_mgr.lock_or_recover();

        match level {
            1 => {
                // FSALLOCATE: idFileSystem(4) + cSectorUnit(4) + cUnit(4) + cUnitAvail(4) + cbSector(2) = 18 bytes
                if cb_buf < 18 { return ERROR_BUFFER_OVERFLOW; }
                let info = match dm.backend(drive_idx) {
                    Ok(b) => match b.query_fs_info_alloc() {
                        Ok(i) => i,
                        Err(e) => return e.0,
                    },
                    Err(e) => return e.0,
                };
                self.guest_write::<u32>(p_buf,      info.id_filesystem);
                self.guest_write::<u32>(p_buf +  4, info.sectors_per_unit);
                self.guest_write::<u32>(p_buf +  8, info.total_units);
                self.guest_write::<u32>(p_buf + 12, info.available_units);
                self.guest_write::<u16>(p_buf + 16, info.bytes_per_sector);
                NO_ERROR
            }
            2 => {
                // FSINFO: ulVSN(4) + vol(1-byte len + up-to-11 chars + NUL) = 17 bytes min
                if cb_buf < 17 { return ERROR_BUFFER_OVERFLOW; }
                let info = match dm.backend(drive_idx) {
                    Ok(b) => match b.query_fs_info_volume() {
                        Ok(i) => i,
                        Err(e) => return e.0,
                    },
                    Err(e) => return e.0,
                };
                self.guest_write::<u32>(p_buf, info.serial_number);
                let label = info.label.as_bytes();
                let label_len = label.len().min(11) as u8;
                self.guest_write::<u8>(p_buf + 4, label_len);
                self.guest_write_bytes(p_buf + 5, &label[..label_len as usize]);
                // NUL-terminate
                self.guest_write::<u8>(p_buf + 5 + label_len as u32, 0);
                NO_ERROR
            }
            _ => ERROR_INVALID_FUNCTION,
        }
    }

    /// DosQueryFSAttach (ordinal 277): query filesystem type.
    ///
    /// OS/2 signature: DosQueryFSAttach(pszDevName, ulOrdinal, ulFSAInfoLevel, pfsqb, pcbBuf)
    /// Returns FSQBUFFER2: iType(2) + cbName(2) + szName(cbName+1) + cbFSDName(2) + szFSDName(cbFSDName+1) + cbFSAData(2) + rgFSAData(cbFSAData)
    pub fn dos_query_fs_attach(&self, psz_dev: u32, _ordinal: u32, level: u32, p_buf: u32, pcb_buf: u32) -> u32 {
        let dev = if psz_dev != 0 { self.read_guest_string(psz_dev) } else { String::new() };
        debug!("  DosQueryFSAttach('{}', level={})", dev, level);

        // Determine which drive is being queried
        let dm = self.shared.drive_mgr.lock_or_recover();
        let drive_letter = if dev.len() >= 2 && dev.as_bytes()[1] == b':' {
            dev.as_bytes()[0].to_ascii_uppercase()
        } else {
            b'A' + dm.current_disk()
        };
        let drive_idx = drive_letter - b'A';

        // Get filesystem name from the backend
        let fsd_name = match dm.backend(drive_idx) {
            Ok(b) => b.fs_name(),
            Err(_) => return ERROR_INVALID_DRIVE,
        };

        let dev_name = format!("{}:", drive_letter as char);
        let dev_name_bytes = dev_name.as_bytes();
        let fsd_name_bytes = fsd_name.as_bytes();

        // FSQBUFFER2 layout: fixed header (8 bytes) + szName + szFSDName
        let total_size = 8 + dev_name_bytes.len() + 1 + fsd_name_bytes.len() + 1;

        if pcb_buf != 0 {
            let buf_avail = self.guest_read::<u32>(pcb_buf).unwrap_or(0) as usize;
            if buf_avail < total_size {
                self.guest_write::<u32>(pcb_buf, total_size as u32);
                return ERROR_BUFFER_OVERFLOW;
            }
            self.guest_write::<u32>(pcb_buf, total_size as u32);
        }

        if p_buf != 0 {
            // FSQBUFFER2 layout: lengths first, then variable-length strings
            self.guest_write::<u16>(p_buf, 3);  // +0: iType = 3 (local drive)
            self.guest_write::<u16>(p_buf + 2, dev_name_bytes.len() as u16); // +2: cbName
            self.guest_write::<u16>(p_buf + 4, fsd_name_bytes.len() as u16); // +4: cbFSDName
            self.guest_write::<u16>(p_buf + 6, 0); // +6: cbFSAData = 0
            // +8: szName (null-terminated)
            self.guest_write_bytes(p_buf + 8, dev_name_bytes);
            self.guest_write::<u8>(p_buf + 8 + dev_name_bytes.len() as u32, 0);
            // +8+cbName+1: szFSDName (null-terminated)
            let fsd_off = p_buf + 8 + dev_name_bytes.len() as u32 + 1;
            self.guest_write_bytes(fsd_off, fsd_name_bytes);
            self.guest_write::<u8>(fsd_off + fsd_name_bytes.len() as u32, 0);
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
    #[allow(clippy::too_many_arguments)]
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

    #[allow(clippy::too_many_arguments)]
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
                QSV_VERSION_MAJOR => 20,     // OS/2 Warp 4.5 (major=20, minor=45 → "4.50")
                QSV_VERSION_MINOR => 45,     // SetOSVersion(): minor≥40 → "4.XX" branch
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

    // ── Step 10: Thread / Environment / LIBPATH ──

    /// DosKillThread (ordinal 111): terminate a thread.
    ///
    /// OS/2 signature: DosKillThread(TID tid)
    ///
    /// Warpine removes the JoinHandle from the thread table, orphaning the
    /// host thread.  True async cancellation is not implemented — this is
    /// sufficient for apps that kill subsidiary worker threads before exit.
    pub fn dos_kill_thread(&self, tid: u32) -> u32 {
        debug!("  DosKillThread(tid={})", tid);
        let removed = self.shared.threads.lock_or_recover().remove(&tid);
        if removed.is_some() { NO_ERROR } else { ERROR_INVALID_HANDLE }
    }

    /// DosScanEnv (ordinal 227): scan the process environment for a variable.
    ///
    /// OS/2 signature: DosScanEnv(PCSZ pszName, PCSZ *ppszValue)
    ///
    /// Reads the environment block pointed to by PIB.pib_pchenv (PIB+0x10),
    /// which is a sequence of `NAME=VALUE\0` strings terminated by an extra
    /// `\0`.  On success, writes the guest pointer to the value substring
    /// into *ppszValue and returns NO_ERROR.
    pub fn dos_scan_env(&self, psz_name: u32, pp_value: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosScanEnv('{}')", name);
        if name.is_empty() { return ERROR_ENVVAR_NOT_FOUND; }

        // PIB.pib_pchenv is at PIB_BASE + 0x10
        let env_addr = match self.guest_read::<u32>(super::constants::PIB_BASE + 0x10) {
            Some(a) => a,
            None => return ERROR_ENVVAR_NOT_FOUND,
        };

        // Scan the double-null–terminated environment block.
        let mut cursor = env_addr;
        loop {
            let entry = self.read_guest_string(cursor);
            if entry.is_empty() { break; } // double-null: end of block

            // Find `NAME=VALUE` — compare the prefix including the '='
            let prefix = format!("{}=", name);
            if entry.to_ascii_uppercase().starts_with(&prefix.to_ascii_uppercase()) {
                // Write the guest pointer to the value part into *ppszValue
                let value_offset = cursor + prefix.len() as u32;
                self.guest_write::<u32>(pp_value, value_offset);
                return NO_ERROR;
            }
            cursor += entry.len() as u32 + 1; // skip past the NUL terminator
        }

        ERROR_ENVVAR_NOT_FOUND
    }

    /// DosSetPriority (ordinal 236): set thread/process priority.
    ///
    /// OS/2 signature: DosSetPriority(ULONG scope, ULONG prtyClass, LONG delta, ULONG target)
    ///
    /// Priority scheduling is not implemented; this is a no-op stub that
    /// satisfies apps which call it during initialisation.
    pub fn dos_set_priority(&self, scope: u32, prty_class: u32, delta: u32, target: u32) -> u32 {
        debug!("  DosSetPriority(scope={}, class={}, delta={}, target={})", scope, prty_class, delta as i32, target);
        NO_ERROR
    }

    /// DosSetExtLIBPATH (ordinal 873): set extended LIBPATH prefix or suffix.
    ///
    /// OS/2 signature: DosSetExtLIBPATH(PCSZ pszExtLIBPATH, ULONG flags)
    /// - flags 1 (BEGIN_LIBPATH): prepend path
    /// - flags 2 (END_LIBPATH):   append path
    pub fn dos_set_ext_libpath(&self, psz_path: u32, flags: u32) -> u32 {
        let path = if psz_path != 0 { self.read_guest_string(psz_path) } else { String::new() };
        debug!("  DosSetExtLIBPATH('{}', flags={})", path, flags);
        match flags {
            BEGIN_LIBPATH => { *self.shared.begin_libpath.lock_or_recover() = path; NO_ERROR }
            END_LIBPATH   => { *self.shared.end_libpath.lock_or_recover() = path;   NO_ERROR }
            _             => ERROR_INVALID_FUNCTION,
        }
    }

    /// DosQueryExtLIBPATH (ordinal 874): query extended LIBPATH prefix or suffix.
    ///
    /// OS/2 signature: DosQueryExtLIBPATH(PSZ pszExtLIBPATH, ULONG flags)
    ///
    /// The caller provides a pre-allocated buffer of at least CCHMAXPATH (260)
    /// bytes at pszExtLIBPATH.  Writes the stored path as a NUL-terminated
    /// string.
    pub fn dos_query_ext_libpath(&self, psz_path: u32, flags: u32) -> u32 {
        debug!("  DosQueryExtLIBPATH(flags={})", flags);
        if psz_path == 0 { return ERROR_INVALID_FUNCTION; }
        let stored = match flags {
            BEGIN_LIBPATH => self.shared.begin_libpath.lock_or_recover().clone(),
            END_LIBPATH   => self.shared.end_libpath.lock_or_recover().clone(),
            _             => return ERROR_INVALID_FUNCTION,
        };
        self.guest_write_bytes(psz_path, stored.as_bytes());
        self.guest_write::<u8>(psz_path + stored.len() as u32, 0); // NUL terminator
        NO_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::super::Loader;
    use super::super::mutex_ext::MutexExt;

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

    #[test]
    fn test_dos_set_priority_is_noop() {
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_set_priority(1, 2, 0, 0),
                   super::super::constants::NO_ERROR);
    }

    #[test]
    fn test_dos_kill_thread_invalid_tid() {
        let loader = Loader::new_mock();
        // No threads registered — should return ERROR_INVALID_HANDLE
        assert_eq!(loader.dos_kill_thread(99),
                   super::super::constants::ERROR_INVALID_HANDLE);
    }

    #[test]
    fn test_dos_ext_libpath_roundtrip() {
        use super::super::constants::{BEGIN_LIBPATH, END_LIBPATH, NO_ERROR};

        let loader = Loader::new_mock();

        // Reserve a guest buffer at a known address
        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(512).unwrap();

        // Set and query BEGINLIBPATH
        let path = b"C:\\MYLIB\0";
        loader.guest_write_bytes(buf, path);
        assert_eq!(loader.dos_set_ext_libpath(buf, BEGIN_LIBPATH), NO_ERROR);

        let out_buf = loader.shared.mem_mgr.lock_or_recover().alloc(512).unwrap();
        assert_eq!(loader.dos_query_ext_libpath(out_buf, BEGIN_LIBPATH), NO_ERROR);
        assert_eq!(loader.read_guest_string(out_buf), "C:\\MYLIB");

        // Set and query ENDLIBPATH
        let path2 = b"D:\\EXTRA\0";
        loader.guest_write_bytes(buf, path2);
        assert_eq!(loader.dos_set_ext_libpath(buf, END_LIBPATH), NO_ERROR);

        let out_buf2 = loader.shared.mem_mgr.lock_or_recover().alloc(512).unwrap();
        assert_eq!(loader.dos_query_ext_libpath(out_buf2, END_LIBPATH), NO_ERROR);
        assert_eq!(loader.read_guest_string(out_buf2), "D:\\EXTRA");

        // BEGINLIBPATH is still independent
        let out_begin = loader.shared.mem_mgr.lock_or_recover().alloc(512).unwrap();
        assert_eq!(loader.dos_query_ext_libpath(out_begin, BEGIN_LIBPATH), NO_ERROR);
        assert_eq!(loader.read_guest_string(out_begin), "C:\\MYLIB");

        // Invalid flag
        assert_ne!(loader.dos_query_ext_libpath(out_buf, 99), NO_ERROR);
    }

    #[test]
    fn test_dos_scan_env_found_and_not_found() {
        use super::super::constants::{NO_ERROR, ERROR_ENVVAR_NOT_FOUND, PIB_BASE};

        let loader = Loader::new_mock();

        // Build a tiny environment block: "FOO=bar\0PATH=C:\\\0\0"
        let env = b"FOO=bar\0PATH=C:\\\0\0";
        let env_addr = loader.shared.mem_mgr.lock_or_recover().alloc(64).unwrap();
        loader.guest_write_bytes(env_addr, env);

        // Write env_addr into PIB.pib_pchenv
        loader.guest_write::<u32>(PIB_BASE + 0x10, env_addr);

        // Write the variable name into guest memory
        let name_buf = loader.shared.mem_mgr.lock_or_recover().alloc(16).unwrap();
        loader.guest_write_bytes(name_buf, b"FOO\0");

        // Pointer for *ppszValue
        let ptr_buf = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();

        // Should find FOO and point at "bar"
        assert_eq!(loader.dos_scan_env(name_buf, ptr_buf), NO_ERROR);
        let value_ptr = loader.guest_read::<u32>(ptr_buf).unwrap();
        assert_eq!(loader.read_guest_string(value_ptr), "bar");

        // Case-insensitive lookup: "foo" should also find "FOO=bar"
        loader.guest_write_bytes(name_buf, b"foo\0");
        assert_eq!(loader.dos_scan_env(name_buf, ptr_buf), NO_ERROR);

        // Non-existent variable
        loader.guest_write_bytes(name_buf, b"MISSING\0");
        assert_eq!(loader.dos_scan_env(name_buf, ptr_buf), ERROR_ENVVAR_NOT_FOUND);
    }

    #[test]
    fn test_dos_set_process_cp_valid() {
        use super::super::constants::NO_ERROR;
        use std::sync::atomic::Ordering;

        let loader = Loader::new_mock();
        assert_eq!(loader.dos_set_process_cp(850), NO_ERROR);
        assert_eq!(loader.shared.active_codepage.load(Ordering::Relaxed), 850);
    }

    #[test]
    fn test_dos_set_process_cp_invalid() {
        use super::super::constants::ERROR_INVALID_CODE_PAGE;
        use std::sync::atomic::Ordering;

        let loader = Loader::new_mock();
        let result = loader.dos_set_process_cp(9999);
        assert_eq!(result, ERROR_INVALID_CODE_PAGE);
        // Active codepage must remain unchanged (still the default 437)
        assert_eq!(loader.shared.active_codepage.load(Ordering::Relaxed), 437);
    }

    #[test]
    fn test_dos_query_cp_reflects_set_process_cp() {
        use super::super::constants::NO_ERROR;
        use super::super::mutex_ext::MutexExt;

        let loader = Loader::new_mock();
        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(8).unwrap();
        let pcb = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();

        // Default should be 437
        assert_eq!(loader.dos_query_cp(4, buf, pcb), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(buf).unwrap(), 437);

        // After DosSetProcessCp(850), DosQueryCp must return 850
        assert_eq!(loader.dos_set_process_cp(850), NO_ERROR);
        assert_eq!(loader.dos_query_cp(4, buf, pcb), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(buf).unwrap(), 850);
    }

    #[test]
    fn test_dos_set_process_cp_all_supported() {
        use super::super::constants::NO_ERROR;

        let loader = Loader::new_mock();
        for &cp in &[437u32, 850, 852, 932, 949, 950, 1250, 1251, 1252, 1253, 1254, 1255, 1256, 1257, 1258] {
            assert_eq!(loader.dos_set_process_cp(cp), NO_ERROR, "cp={cp} should be supported");
        }
    }

    // ── DosFreeModule ─────────────────────────────────────────────────────────

    /// Helper: register a fake DLL in the dll_mgr and return its handle.
    fn register_fake_dll(loader: &Loader, name: &str, bases: Vec<u32>, initterm_addr: Option<u32>) -> u32 {
        use super::super::managers::LoadedDll;
        use std::collections::HashMap;
        use super::super::mutex_ext::MutexExt;

        let mut dll_mgr = loader.shared.dll_mgr.lock_or_recover();
        let h = dll_mgr.alloc_handle();
        dll_mgr.register(LoadedDll {
            name: name.to_ascii_uppercase(),
            handle: h,
            object_bases: bases,
            exports_by_ordinal: HashMap::new(),
            exports_by_name: HashMap::new(),
            ref_count: 1,
            initterm_addr,
            is_builtin: false,
        });
        h
    }

    /// DosLoadModule on a builtin module name must return NO_ERROR and a valid handle.
    #[test]
    fn test_dos_load_module_builtin_returns_ok() {
        use super::super::{ApiResult, mutex_ext::MutexExt};
        use super::super::constants::NO_ERROR;
        let loader = Loader::new_mock();

        // Write "UCONV\0" to guest memory so read_guest_string works.
        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(8).unwrap();
        loader.guest_write_bytes(name_addr, b"UCONV\0");
        let hmod_addr = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();

        let result = loader.dos_load_module(0, 0, name_addr, hmod_addr);
        assert!(matches!(result, ApiResult::Normal(NO_ERROR)));

        let h = loader.guest_read::<u32>(hmod_addr).unwrap();
        assert_ne!(h, 0, "handle must be non-zero");

        let dll_mgr = loader.shared.dll_mgr.lock_or_recover();
        let dll = dll_mgr.find_by_handle(h).expect("builtin handle must be registered");
        assert_eq!(dll.name, "UCONV");
        assert!(dll.is_builtin, "must be marked as builtin");
    }

    /// Second DosLoadModule call on the same builtin must reuse the existing handle.
    #[test]
    fn test_dos_load_module_builtin_reuse_handle() {
        use super::super::mutex_ext::MutexExt;
        let loader = Loader::new_mock();

        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(8).unwrap();
        loader.guest_write_bytes(name_addr, b"MDM\0");
        let hmod1 = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        let hmod2 = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();

        let _ = loader.dos_load_module(0, 0, name_addr, hmod1);
        let _ = loader.dos_load_module(0, 0, name_addr, hmod2);

        let h1 = loader.guest_read::<u32>(hmod1).unwrap();
        let h2 = loader.guest_read::<u32>(hmod2).unwrap();
        assert_eq!(h1, h2, "same builtin must return the same handle");
    }

    /// DosQueryProcAddr on a builtin handle resolves to the expected thunk address.
    #[test]
    fn test_dos_query_proc_addr_builtin_ordinal() {
        use super::super::mutex_ext::MutexExt;
        use super::super::constants::{NO_ERROR, MAGIC_API_BASE, UCONV_BASE, MDM_BASE};
        let loader = Loader::new_mock();

        // Load UCONV builtin
        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(8).unwrap();
        loader.guest_write_bytes(name_addr, b"UCONV\0");
        let hmod_addr = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        let pfn_addr = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();

        let _ = loader.dos_load_module(0, 0, name_addr, hmod_addr);
        let h = loader.guest_read::<u32>(hmod_addr).unwrap();

        // Ordinal 1 = UniCreateUconvObject → thunk at MAGIC_API_BASE + UCONV_BASE + 1
        let rc = loader.dos_query_proc_addr(h, 1, 0, pfn_addr);
        assert_eq!(rc, NO_ERROR, "DosQueryProcAddr for builtin ordinal 1 must succeed");
        let got = loader.guest_read::<u32>(pfn_addr).unwrap();
        let expected = (MAGIC_API_BASE as u32) + UCONV_BASE + 1;
        assert_eq!(got, expected, "thunk address mismatch for UCONV ordinal 1");

        // Load MDM builtin; ordinal 2 = mciSendString → MAGIC_API_BASE + MDM_BASE + 2
        loader.guest_write_bytes(name_addr, b"MDM\0");
        let hmod2 = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        let _ = loader.dos_load_module(0, 0, name_addr, hmod2);
        let h2 = loader.guest_read::<u32>(hmod2).unwrap();

        let rc2 = loader.dos_query_proc_addr(h2, 2, 0, pfn_addr);
        assert_eq!(rc2, NO_ERROR);
        let got2 = loader.guest_read::<u32>(pfn_addr).unwrap();
        assert_eq!(got2, (MAGIC_API_BASE as u32) + MDM_BASE + 2);
    }

    #[test]
    fn test_dos_free_module_still_referenced_returns_normal() {
        use super::super::{ApiResult, mutex_ext::MutexExt};
        use super::super::constants::NO_ERROR;

        let loader = Loader::new_mock();
        let h = register_fake_dll(&loader, "TESTDLL", vec![0x1000], None);
        // Bump refcount to 2 first
        loader.shared.dll_mgr.lock_or_recover().increment_refcount(h);

        let result = loader.dos_free_module(h);
        assert!(matches!(result, ApiResult::Normal(c) if c == NO_ERROR));
        // DLL still loaded with refcount 1
        assert!(loader.shared.dll_mgr.lock_or_recover().find_by_handle(h).is_some());
    }

    #[test]
    fn test_dos_free_module_no_initterm_returns_normal_and_frees_pages() {
        use super::super::{ApiResult, mutex_ext::MutexExt};
        use super::super::constants::NO_ERROR;

        let loader = Loader::new_mock();
        let base = loader.shared.mem_mgr.lock_or_recover().alloc(64).unwrap();
        let h = register_fake_dll(&loader, "NODLL", vec![base], None);

        let result = loader.dos_free_module(h);
        assert!(matches!(result, ApiResult::Normal(c) if c == NO_ERROR));
        // DLL removed from manager
        assert!(loader.shared.dll_mgr.lock_or_recover().find_by_handle(h).is_none());
        // Page returned to allocator — re-alloc should succeed at the same address
        let new_base = loader.shared.mem_mgr.lock_or_recover().alloc(64).unwrap();
        assert_eq!(new_base, base, "freed page should be reusable");
    }

    #[test]
    fn test_dos_free_module_with_initterm_returns_call_guest() {
        use super::super::ApiResult;

        let loader = Loader::new_mock();
        let base = loader.shared.mem_mgr.lock_or_recover().alloc(64).unwrap();
        let initterm = 0x5000u32;
        let h = register_fake_dll(&loader, "INITTDLL", vec![base], Some(initterm));

        let result = loader.dos_free_module(h);
        match result {
            ApiResult::CallGuest { addr, hmod, phmod, ref object_bases } => {
                assert_eq!(addr, initterm);
                assert_eq!(hmod, h);
                assert_eq!(phmod, 0, "phmod must be 0 for unload");
                assert_eq!(object_bases, &vec![base], "bases must be carried for deferred free");
            }
            other => panic!("expected CallGuest, got {:?}", other),
        }
        // DLL must already be removed from manager (pages deferred, but DLL entry gone)
        assert!(loader.shared.dll_mgr.lock_or_recover().find_by_handle(h).is_none());
    }

    // ── dos_map_case / map_case_guest_buf ─────────────────────────────────────

    /// Allocate a small guest buffer, write `data`, call `dos_map_case`, read back.
    fn run_map_case(loader: &Loader, cp: u32, data: &[u8]) -> Vec<u8> {
        use std::sync::atomic::Ordering;
        loader.shared.active_codepage.store(cp, Ordering::Relaxed);
        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(data.len() as u32 + 4).unwrap();
        loader.guest_write_bytes(buf, data);
        loader.dos_map_case(data.len() as u32, 0, buf);
        (0..data.len()).map(|i| loader.guest_read::<u8>(buf + i as u32).unwrap()).collect()
    }

    // ASCII lowercase → uppercase in CP437.
    #[test]
    fn test_dos_map_case_ascii_cp437() {
        let loader = Loader::new_mock();
        let result = run_map_case(&loader, 437, b"hello");
        assert_eq!(result, b"HELLO");
    }

    // CP850: é (0x82) → É (0x90)
    #[test]
    fn test_dos_map_case_cp850_e_acute() {
        let loader = Loader::new_mock();
        let result = run_map_case(&loader, 850, &[0x82]);
        assert_eq!(result, &[0x90]);
    }

    // CP866: Cyrillic а (0xA0) → А (0x80)
    #[test]
    fn test_dos_map_case_cp866_cyrillic() {
        let loader = Loader::new_mock();
        // "аб" in CP866: 0xA0 0xA1 → "АБ": 0x80 0x81
        let result = run_map_case(&loader, 866, &[0xA0, 0xA1]);
        assert_eq!(result, &[0x80, 0x81],
            "CP866 аб should uppercase to АБ");
    }

    // CP932 (Shift-JIS): DBCS pair あ (0x82 0xA0, U+3042) — no case variant →
    // bytes unchanged.
    #[test]
    fn test_dos_map_case_cp932_hiragana_unchanged() {
        let loader = Loader::new_mock();
        // あ has no uppercase variant in Unicode; bytes must be unchanged.
        let result = run_map_case(&loader, 932, &[0x82, 0xA0]);
        assert_eq!(result, &[0x82, 0xA0],
            "CP932 あ (no case) must be unchanged");
    }

    // CP932 mixed: ASCII before DBCS pair.
    #[test]
    fn test_dos_map_case_cp932_mixed_ascii_and_dbcs() {
        let loader = Loader::new_mock();
        // 'a' (0x61) followed by DBCS あ (0x82 0xA0)
        let result = run_map_case(&loader, 932, &[0x61, 0x82, 0xA0]);
        assert_eq!(result[0], b'A', "ASCII 'a' must uppercase to 'A'");
        assert_eq!(result[1], 0x82, "DBCS lead byte must be unchanged");
        assert_eq!(result[2], 0xA0, "DBCS trail byte must be unchanged");
    }

    // Null pointer → NO_ERROR, no crash.
    #[test]
    fn test_dos_map_case_null_ptr() {
        use super::super::constants::NO_ERROR;
        let loader = Loader::new_mock();
        assert_eq!(loader.dos_map_case(4, 0, 0), NO_ERROR);
    }
}
