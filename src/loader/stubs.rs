// SPDX-License-Identifier: GPL-3.0-or-later
//
// OS/2 stub and minimal API implementations for initialization-time APIs.
// These are needed to get text-mode apps like 4OS2 past their init sequence.

use log::{debug, warn};

use super::clock;
use super::constants::*;
use super::mutex_ext::MutexExt;

/// Codepages the guest may select: those `codepage.rs` can convert to and
/// from Unicode. Shared by `DosSetProcessCp` and `WinSetCp`.
pub(crate) const SUPPORTED_CODEPAGES: &[u32] =
    &[437, 850, 852, 866, 932, 936, 949, 950, 1250, 1251, 1252, 1253, 1254, 1255, 1256, 1257, 1258];

/// Whether `cp` is a codepage the guest may select.
pub(crate) fn is_supported_codepage(cp: u32) -> bool {
    SUPPORTED_CODEPAGES.contains(&cp)
}

/// Codepages a PM message queue may use (`WinSetCp`, `WinQueryCpList`): the
/// process codepages plus 1004, the PM-only "Windows Extended" codepage.
pub(crate) fn pm_codepages() -> Vec<u32> {
    let mut cps = SUPPORTED_CODEPAGES.to_vec();
    cps.push(super::codepage::CP1004);
    cps.sort_unstable();
    cps
}

/// Whether `cp` is a codepage a PM message queue may use.
pub(crate) fn is_pm_codepage(cp: u32) -> bool {
    is_supported_codepage(cp) || cp == super::codepage::CP1004
}

impl super::Loader {
    // ── Step 1: Critical Init Stubs ──

    /// DosError (ordinal 212): enable/disable hard error popups.
    pub fn dos_error(&self, flag: u32) -> u32 {
        debug!("  DosError({})", flag);
        NO_ERROR
    }

    /// DosSetMaxFH (ordinal 209): set maximum file handles.
    pub fn dos_set_max_fh(&self, count: u32) -> u32 {
        debug!("  DosSetMaxFH({})", count);
        NO_ERROR
    }

    /// DosBeep (ordinal 286): play a tone through the audio service.
    pub fn dos_beep(&self, freq: u32, dur: u32) -> u32 {
        debug!("  DosBeep(freq={}, dur={})", freq, dur);
        super::mmpm::beep_tone(&self.shared.audio, freq, dur, &|| self.interrupted() || self.shutting_down());
        NO_ERROR
    }

    /// DosSetExceptionHandler (ordinal 354): push a handler record onto the
    /// per-thread exception chain at TIB+0x00 (tib_pexchain).
    ///
    /// Sets `preg_rec->prev_structure = TIB[0x00]`, then `TIB[0x00] = preg_rec`.
    /// The chain is the calling thread's own (its TIB, `exception_chain_addr`).
    /// A record outside guest memory fails with `ERROR_INVALID_PARAMETER`.
    pub fn dos_set_exception_handler(&self, vcpu_id: u32, preg_rec: u32) -> u32 {
        debug!("  DosSetExceptionHandler(pRegRec=0x{:08X})", preg_rec);
        if preg_rec == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let Some(head) = self.exception_chain_addr(vcpu_id) else { return ERROR_INVALID_PARAMETER };
        let cur_head = self.guest_read::<u32>(head).unwrap_or(XCPT_CHAIN_END);
        // Link new record to the current chain head; it becomes the head.
        if self.guest_write::<u32>(preg_rec.wrapping_add(XERREC_PREV), cur_head).is_none()
            || self.guest_write::<u32>(head, preg_rec).is_none()
        {
            return ERROR_INVALID_PARAMETER;
        }
        NO_ERROR
    }

    /// DosUnsetExceptionHandler (ordinal 355): pop a handler record from the
    /// per-thread exception chain at TIB+0x00 (tib_pexchain).
    ///
    /// Sets `TIB[0x00] = preg_rec->prev_structure`.
    pub fn dos_unset_exception_handler(&self, vcpu_id: u32, preg_rec: u32) -> u32 {
        debug!("  DosUnsetExceptionHandler(pRegRec=0x{:08X})", preg_rec);
        if preg_rec == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let Some(head) = self.exception_chain_addr(vcpu_id) else { return ERROR_INVALID_PARAMETER };
        let Some(prev) = self.guest_read::<u32>(preg_rec.wrapping_add(XERREC_PREV)) else {
            return ERROR_INVALID_PARAMETER;
        };
        if self.guest_write::<u32>(head, prev).is_none() { return ERROR_INVALID_PARAMETER; }
        NO_ERROR
    }

    /// DosQuerySysState (ordinal 368): query system state info.
    /// Stub — returns ERROR_INVALID_FUNCTION since we don't have real system state.
    pub fn dos_query_sys_state(&self, func: u32, arg1: u32, pid: u32, p_buf: u32, cb_buf: u32) -> u32 {
        debug!("  DosQuerySysState(func={}, arg1={}, pid={}, pBuf=0x{:08X}, cbBuf={})", func, arg1, pid, p_buf, cb_buf);
        ERROR_INVALID_FUNCTION
    }

    /// DosEnterMustComplete (ordinal 380) / DosExitMustComplete (381,
    /// `enter` FALSE): a must-complete section of thread `tid` begins or
    /// ends; `*pulNesting` gets the nesting afterwards. While thread 1 is
    /// in one, signal exceptions and asynchronous process termination wait
    /// (`signals.rs`, `interrupt.rs`).
    pub fn dos_must_complete(&self, vcpu_id: u32, p_nesting: u32, enter: bool) -> u32 {
        let tid = super::threads::tid_of_vcpu(vcpu_id);
        match self.shared.threads.must_complete_step(tid, enter) {
            Ok(n) => {
                if p_nesting != 0 && self.guest_write::<u32>(p_nesting, n).is_none() {
                    return ERROR_INVALID_PARAMETER;
                }
                NO_ERROR
            }
            Err(e) => e,
        }
    }

    // ── Step 2: Shared Memory ──

    /// DosAllocSharedMem (ordinal 300): allocate shared memory. A name must
    /// be `\SHAREMEM\…` and not in use (`ERROR_ALREADY_EXISTS`); the
    /// flags need an access bit (`PAG_READ` / `PAG_WRITE` / `PAG_EXECUTE`).
    /// The block is in the session's shared arena (`shared_arena.rs`), at
    /// the same address in every process; without one (no session) it is
    /// shared by the threads of this process only.
    pub fn dos_alloc_shared_mem(&self, ppb: u32, psz_name: u32, cb: u32, flag: u32) -> u32 {
        let name = if psz_name != 0 {
            let n = self.read_guest_string(psz_name);
            if !n.is_empty() { Some(n) } else { None }
        } else {
            None
        };
        debug!("  DosAllocSharedMem(name={:?}, cb={}, flag=0x{:X})", name, cb, flag);
        const VALID: u32 = PAG_READ | PAG_WRITE | PAG_EXECUTE | PAG_GUARD | PAG_COMMIT
            | OBJ_TILE | OBJ_GETTABLE | OBJ_GIVEABLE | OBJ_ANY;
        if ppb == 0 || cb == 0 || flag & !VALID != 0 || flag & (PAG_READ | PAG_WRITE | PAG_EXECUTE) == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let name = match name {
            Some(n) => match super::shared_arena::canonical_name(&n) {
                Some(c) => Some(c),
                None => return ERROR_INVALID_NAME,
            },
            None => None,
        };
        if let Some(r) = self.arena_alloc(ppb, name.clone(), cb, flag) {
            return r;
        }
        if let Some(upper) = &name
            && self.shared.shmem_mgr.lock_or_recover().find_by_name(upper).is_some() {
            return ERROR_ALREADY_EXISTS;
        }
        let Some(addr) = self.alloc_zeroed(cb) else {
            return ERROR_NOT_ENOUGH_MEMORY;
        };
        if self.guest_write::<u32>(ppb, addr).is_none() {
            self.shared.mem_mgr.lock_or_recover().free(addr);
            return ERROR_INVALID_PARAMETER;
        }
        self.shared.shmem_mgr.lock_or_recover().register(super::managers::SharedBlock {
            addr, size: cb, flags: flag, name,
        });
        NO_ERROR
    }

    /// DosGetNamedSharedMem (ordinal 301): get existing shared memory by name.
    /// The name must be `\SHAREMEM\…` (`ERROR_INVALID_NAME`) and the flags
    /// need an access bit.
    pub fn dos_get_named_shared_mem(&self, ppb: u32, psz_name: u32, flag: u32) -> u32 {
        let raw = self.read_guest_string(psz_name);
        debug!("  DosGetNamedSharedMem(name='{}', flag=0x{:X})", raw, flag);
        if flag & !SHARED_ACCESS_FLAGS != 0 || flag & (PAG_READ | PAG_WRITE | PAG_EXECUTE) == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        let Some(name) = super::shared_arena::canonical_name(&raw) else {
            return ERROR_INVALID_NAME;
        };
        if let Some(r) = self.arena_get_named(ppb, &name) {
            return r;
        }
        if let Some(addr) = self.shared.shmem_mgr.lock_or_recover().find_by_name(&name) {
            self.guest_write::<u32>(ppb, addr);
            NO_ERROR
        } else {
            ERROR_FILE_NOT_FOUND
        }
    }

    /// DosGiveSharedMem (ordinal 303): give process `pid` access to a
    /// giveable shared block. `pb` must be the base of a block allocated
    /// with `OBJ_GIVEABLE` (else `ERROR_INVALID_ADDRESS` /
    /// `ERROR_ACCESS_DENIED`); `flag` needs an access bit and may not ask
    /// for more than the block allows. The calling process itself already
    /// has the block (`NO_ERROR`, unverified); an unknown process is
    /// `ERROR_INVALID_PROCID`. Another process of the session (a child
    /// too, even before it has started) gets the block in the shared arena;
    /// without an arena giving to another process fails with
    /// `ERROR_ACCESS_DENIED` (Warpine's limit).
    pub fn dos_give_shared_mem(&self, pb: u32, pid: u32, flag: u32) -> u32 {
        debug!("  DosGiveSharedMem(pb=0x{:08X}, pid={}, flag=0x{:X})", pb, pid, flag);
        const PERM: u32 = PAG_READ | PAG_WRITE | PAG_EXECUTE;
        if flag & !SHARED_ACCESS_FLAGS != 0 || flag & PERM == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        if super::shared_arena::in_arena(pb) && let Some(r) = self.arena_give(pb, pid, flag & PERM) {
            return r;
        }
        let Some(block) = self.shared.shmem_mgr.lock_or_recover().block(pb).cloned() else {
            return ERROR_INVALID_ADDRESS;
        };
        if block.flags & OBJ_GIVEABLE == 0 || flag & PERM & !block.flags != 0 {
            return ERROR_ACCESS_DENIED;
        }
        if pid == self.shared.process_id() {
            return NO_ERROR;
        }
        if self.shared.process_mgr.lock_or_recover().children.contains_key(&pid) {
            warn!("  DosGiveSharedMem: process {} is another Warpine process; its memory is out of reach", pid);
            return ERROR_ACCESS_DENIED;
        }
        ERROR_INVALID_PROCID
    }

    /// DosGetSharedMem (ordinal 302): get access to the gettable shared
    /// block at `pb` (`OBJ_GETTABLE`, else `ERROR_ACCESS_DENIED`; not a
    /// block: `ERROR_INVALID_ADDRESS`, unverified); the flags need an access
    /// bit. A block of this process's own (no arena) is already accessible.
    pub fn dos_get_shared_mem(&self, pb: u32, flag: u32) -> u32 {
        debug!("  DosGetSharedMem(pb=0x{:08X}, flag=0x{:X})", pb, flag);
        if flag & !SHARED_ACCESS_FLAGS != 0 || flag & (PAG_READ | PAG_WRITE | PAG_EXECUTE) == 0 {
            return ERROR_INVALID_PARAMETER;
        }
        if super::shared_arena::in_arena(pb) && let Some(r) = self.arena_get(pb) {
            return r;
        }
        match self.shared.shmem_mgr.lock_or_recover().block(pb) {
            Some(b) if b.flags & OBJ_GETTABLE != 0 => NO_ERROR,
            Some(_) => ERROR_ACCESS_DENIED,
            None => ERROR_INVALID_ADDRESS,
        }
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
    /// Validates against `SUPPORTED_CODEPAGES`; returns `ERROR_INVALID_CODE_PAGE`
    /// otherwise (also for 1004, which only a PM message queue can use).
    pub fn dos_set_process_cp(&self, cp: u32) -> u32 {
        if !is_supported_codepage(cp) {
            debug!("  DosSetProcessCp({}) → ERROR_INVALID_CODE_PAGE", cp);
            return ERROR_INVALID_CODE_PAGE;
        }
        self.shared.active_codepage.store(cp, std::sync::atomic::Ordering::Relaxed);
        // PM text picks its CJK fallback face (JP/KR/TC/SC) from the codepage.
        crate::gui::font::set_active_codepage(cp);
        // The VIO screen follows it too: the text renderer pairs DBCS cells
        // and VioCheckCharType classifies them with `VioManager::codepage`.
        // (Every supported codepage fits in a u16.)
        self.shared.console_mgr.lock_or_recover().codepage = cp as u16;
        debug!("  DosSetProcessCp({}) → NO_ERROR", cp);
        NO_ERROR
    }

    /// DosQueryCtryInfo (NLS ordinal 5): get country/locale information.
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

        // Keep `p_str + i` (and `+ 1`) from overflowing near the top of
        // the address space; reads there fail anyway.
        let cb = cb.min(u32::MAX - p_str);
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
        let name_upper = name.to_ascii_uppercase();
        let stem = name_upper.strip_suffix(".DLL").unwrap_or(&name_upper);
        if super::constants::module_base(stem).is_some() {
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
                object_sizes: Vec::new(),
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
        let result = match self.find_module_file(&name) {
            Some((module, path)) => self.load_dll(&module, &path),
            None => Err(format!("'{}' not found on host", name)),
        };

        match result {
            Ok(h) => {
                // If the DLL has an INITTERM entry, inject the call before writing
                // the handle to *phmod — the handle is written by the vcpu handler
                // after INITTERM returns successfully.
                // The modules it imports that were loaded with it
                // initialize first, then it: one chain.
                let pending = self.shared.dll_mgr.lock_or_recover().take_pending_init();
                if pending.len() > 1 || pending.first().is_some_and(|&(m, _)| m != h) {
                    let mut dll_mgr = self.shared.dll_mgr.lock_or_recover();
                    for (m, e) in pending { dll_mgr.add_pending_init(m, e); }
                    drop(dll_mgr);
                    let vcpu_id = super::interrupt::current_vcpu().unwrap_or(0);
                    return self.dll_init_chain(vcpu_id, super::pm_hook::ChainThen::LoadModule { hmod: h, phmod });
                }
                let initterm = pending.first().map(|&(_, e)| e);
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
            // A handle of no loaded module (never one, or already freed).
            if dll_mgr.find_by_handle(hmod).is_none() {
                return super::ApiResult::Normal(super::constants::ERROR_INVALID_HANDLE);
            }
            dll_mgr.decrement_refcount(hmod)
        };
        if freed.is_some() {
            // Its resources live in the pages about to be freed.
            self.shared.resource_mgr.lock_or_recover().remove_module(hmod);
            if let Some(gdb) = self.shared.gdb_state.as_ref() { gdb.libraries_changed(); }
        }
        match freed {
            Some((bases, Some(addr))) => {
                // DLL has INITTERM — inject _DLL_InitTerm(hmod, 1) before freeing.
                debug!("  DosFreeModule: hmod={} has INITTERM at {:#x}, injecting unload call", hmod, addr);
                super::ApiResult::CallGuest { addr, hmod, phmod: 0, object_bases: bases }
            }
            Some((bases, None)) => {
                // No INITTERM — free pages now.
                self.free_module_objects(bases);
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
        // A built-in module is always loaded, as OS/2's system DLLs are: it
        // gets its handle on first use (ADR-0086).
        let upper = name.to_ascii_uppercase();
        let stem = upper.strip_suffix(".DLL").unwrap_or(&upper);
        if super::constants::module_base(stem).is_some() {
            return match self.dos_load_module(0, 0, psz_mod_name, phmod) {
                super::ApiResult::Normal(rc) => rc,
                _ => ERROR_MOD_NOT_FOUND,
            };
        }
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
                // An ordinal Warpine does not implement is not found, so a
                // program (or its C runtime) probing for an optional call,
                // such as DosOpenL, falls back as on a system without it.
                // PMWIN and PMGPI know their dispatched calls (build.rs);
                // the VIO / KBD / MOU ordinals are not in the registry and
                // are resolved as before.
                const SUB_DISPATCHED: [&str; 3] = ["VIOCALLS", "KBDCALLS", "MOUCALLS"];
                let known = super::descriptors::pm_ordinal_is_dispatched(&mod_name, ordinal)
                    .unwrap_or_else(|| SUB_DISPATCHED.contains(&mod_name.as_str())
                        || super::constants::module_base(&mod_name)
                            .is_some_and(|base| super::api_registry::find(base + ordinal).is_some()));
                known.then(|| self.resolve_import(&mod_name, ordinal) as u32)
            } else if !name.is_empty() {
                // By name: the module's name table or the registry (0 = unknown).
                Some(self.resolve_import_by_name(&mod_name, &name) as u32).filter(|&a| a != 0)
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

    /// DosPutMessage (MSG ordinal 3): write a message to a file handle.
    pub fn dos_put_message(&self, hf: u32, cb_msg: u32, p_msg: u32) -> u32 {
        debug!("  DosPutMessage(hf={}, cb={})", hf, cb_msg);
        if p_msg == 0 || cb_msg == 0 { return NO_ERROR; }
        // Route through dos_write so SDL text window and terminal both work
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

    /// DosForceDelete (ordinal 110): delete without undelete.
    pub fn dos_force_delete(&self, psz_name: u32) -> u32 {
        self.dos_delete(psz_name) // alias to existing delete
    }

    /// DosEditName (ordinal 261): wildcard filename transformation.
    /// Transforms source name using edit string pattern (e.g., *.txt + *.bak → file.bak).
    /// The result and its NUL must fit `cb_target` (`ERROR_BUFFER_OVERFLOW`).
    pub fn dos_edit_name(&self, _meta_level: u32, psz_source: u32, psz_edit: u32, psz_target: u32, cb_target: u32) -> u32 {
        let source = self.read_guest_string(psz_source);
        let edit = self.read_guest_string(psz_edit);
        debug!("  DosEditName('{}', '{}')", source, edit);

        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let bytes = super::codepage::cp_encode(&Self::apply_edit_name(&source, &edit), cp);
        if psz_target == 0 { return ERROR_INVALID_PARAMETER; }
        if bytes.len() + 1 > cb_target as usize { return ERROR_BUFFER_OVERFLOW; }
        self.guest_write_bytes(psz_target, &bytes);
        self.guest_write::<u8>(psz_target + bytes.len() as u32, 0);
        NO_ERROR
    }

    /// Apply OS/2 DosEditName wildcard transformation, per character:
    /// `*` copies the rest of the source's component (up to a `.`), `?`
    /// one character of it, `.` moves to the source's next component, and
    /// any other character replaces one.
    pub(crate) fn apply_edit_name(source: &str, edit: &str) -> String {
        let src: Vec<char> = source.chars().collect();
        let mut result = String::new();
        let mut si = 0;
        for ec in edit.chars() {
            match ec {
                '*' => {
                    while si < src.len() && src[si] != '.' {
                        result.push(src[si]);
                        si += 1;
                    }
                }
                '?' => {
                    if si < src.len() && src[si] != '.' {
                        result.push(src[si]);
                        si += 1;
                    }
                }
                '.' => {
                    result.push('.');
                    while si < src.len() && src[si] != '.' { si += 1; }
                    if si < src.len() { si += 1; }
                }
                _ => {
                    result.push(ec);
                    if si < src.len() && src[si] != '.' { si += 1; }
                }
            }
        }
        result
    }

    /// DosSetFileInfo (ordinal 218): set an open file's times
    /// (`FIL_STANDARD`, a `FILESTATUS3`): see [`Self::set_file_status`].
    /// Pipes have no times to set. Level 2 (EAs) is accepted and ignored.
    pub fn dos_set_file_info(&self, hf: u32, level: u32, p_info: u32, cb_info: u32) -> u32 {
        debug!("  DosSetFileInfo(hf={}, level={})", hf, level);
        let status = match self.read_set_status(level, p_info, cb_info) {
            Ok(Some(status)) => status,
            Ok(None) => return NO_ERROR,
            Err(rc) => return rc,
        };
        if self.handle_target(hf) == Some(super::doscalls::HandleTarget::Pipe)
            || hf >= super::managers::PIPE_HANDLE_BASE {
            return NO_ERROR;
        }
        match self.shared.drive_mgr.lock_or_recover().set_file_info(hf, level, &status) {
            Ok(()) => NO_ERROR,
            Err(e) => e.0,
        }
    }

    /// DosSetFileMode (ordinal 84): set file attributes.
    ///
    /// Sets the attributes only, as `DosSetPathInfo` with times of 0 / 0
    /// would: `FILE_READONLY` is applied (`vfs_hostdir.rs`), hidden, system
    /// and archive have no host counterpart.
    pub fn dos_set_file_mode(&self, psz_name: u32, attr: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosSetFileMode('{}', 0x{:04X})", name, attr);
        let status = super::vfs::FileStatus {
            creation_date: 0, creation_time: 0, last_access_date: 0, last_access_time: 0,
            last_write_date: 0, last_write_time: 0, file_size: 0, file_alloc: 0,
            attributes: super::vfs::FileAttribute(attr),
        };
        match self.shared.drive_mgr.lock_or_recover().set_path_info(&name, FIL_STANDARD, &status) {
            Ok(()) => NO_ERROR,
            Err(e) => e.0,
        }
    }

    /// DosSetPathInfo (ordinal 219): set a file's or directory's times by
    /// name, as `DosSetFileInfo` does. `flOptions` (`DSPI_WRTTHRU`) needs
    /// nothing on the host.
    pub fn dos_set_path_info(&self, psz_name: u32, level: u32, p_info: u32, cb_info: u32, _flag: u32) -> u32 {
        let name = self.read_guest_string(psz_name);
        debug!("  DosSetPathInfo('{}', level={})", name, level);
        let status = match self.read_set_status(level, p_info, cb_info) {
            Ok(Some(status)) => status,
            Ok(None) => return NO_ERROR,
            Err(rc) => return rc,
        };
        match self.shared.drive_mgr.lock_or_recover().set_path_info(&name, level, &status) {
            Ok(()) => NO_ERROR,
            Err(e) => e.0,
        }
    }

    /// The `FILESTATUS3` a set call at `level` passes, checked: `Ok(None)`
    /// for level 2 (`FIL_QUERYEASIZE`, EAs — not set here), an error for a
    /// short buffer, another level or a date or time out of range.
    ///
    /// The access and write times are set; a date and time of 0 / 0 leave
    /// that time unchanged (Control Program Reference). Linux cannot set a
    /// creation time. Of the attributes, `FILE_READONLY` is applied to files
    /// (`vfs_hostdir.rs`); hidden, system and archive have no host
    /// counterpart. Unverified: the error OS/2 gives for an invalid date
    /// (Warpine: `ERROR_INVALID_PARAMETER`).
    fn read_set_status(&self, level: u32, p_info: u32, cb_info: u32)
        -> Result<Option<super::vfs::FileStatus>, u32>
    {
        match level {
            FIL_STANDARD => {}
            FIL_QUERYEASIZE => return Ok(None),
            _ => return Err(ERROR_INVALID_LEVEL),
        }
        if cb_info < FILESTATUS3_SIZE {
            return Err(ERROR_BUFFER_OVERFLOW);
        }
        let b = self.guest_read_bytes(p_info, FILESTATUS3_SIZE as usize).ok_or(ERROR_INVALID_PARAMETER)?;
        let word = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let dword = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let status = super::vfs::FileStatus {
            creation_date: word(0),
            creation_time: word(2),
            last_access_date: word(4),
            last_access_time: word(6),
            last_write_date: word(8),
            last_write_time: word(10),
            file_size: dword(12),
            file_alloc: dword(16),
            attributes: super::vfs::FileAttribute(dword(20)),
        };
        let pairs = [(status.creation_date, status.creation_time),
                     (status.last_access_date, status.last_access_time),
                     (status.last_write_date, status.last_write_time)];
        if pairs.iter().any(|&(d, t)| clock::fdate_request(d, t) == clock::FileTimeRequest::Invalid) {
            return Err(ERROR_INVALID_PARAMETER);
        }
        Ok(Some(status))
    }

    /// DosQueryFHState (ordinal 276): get file handle state. Pipe handles
    /// report their access and flags (`OPEN_FLAGS_NOINHERIT` …); other
    /// handles 0.
    pub fn dos_query_fh_state(&self, hf: u32, p_mode: u32) -> u32 {
        debug!("  DosQueryFHState(hf={})", hf);
        let state = self.shared.handle_mgr.lock_or_recover().entry(hf).map(|e| e.fh_state).unwrap_or(0);
        if p_mode != 0 {
            self.guest_write::<u32>(p_mode, state);
        }
        NO_ERROR
    }

    /// DosSetFHState (ordinal 221): set file handle state. On a pipe handle
    /// the settable flags (`OPEN_FLAGS_NOINHERIT`, `_FAIL_ON_ERROR`,
    /// `_WRITE_THROUGH`, `_NO_CACHE`) are kept — a non-inheritable handle
    /// is not passed to child processes; other handles accept the call.
    pub fn dos_set_fh_state(&self, hf: u32, mode: u32) -> u32 {
        debug!("  DosSetFHState(hf={}, mode=0x{:04X})", hf, mode);
        const SETTABLE: u32 = OPEN_FLAGS_NOINHERIT | OPEN_FLAGS_FAIL_ON_ERROR
            | OPEN_FLAGS_WRITE_THROUGH | OPEN_FLAGS_NO_CACHE;
        if let Some(e) = self.shared.handle_mgr.lock_or_recover().entry_mut(hf) {
            e.fh_state = (e.fh_state & !SETTABLE) | (mode & SETTABLE);
        }
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

    /// DosOpenEventSem (ordinal 325): open an event semaphore by name or handle.
    pub fn dos_open_event_sem(&self, psz_name: u32, phev: u32) -> u32 {
        self.open_sem(super::ipc::SemKind::Event, psz_name, phev)
    }

    /// DosOpenMutexSem (ordinal 332): open a mutex semaphore by name or handle.
    pub fn dos_open_mutex_sem(&self, psz_name: u32, phmtx: u32) -> u32 {
        self.open_sem(super::ipc::SemKind::Mutex, psz_name, phmtx)
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
        // One reading for both halves, so they cannot straddle a carry.
        let time = self.shared.clock.now_secs().max(0) as u64;
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
                QSV_VERSION_MAJOR => OS2_VERSION_MAJOR,     // OS/2 Warp 4.5 (major=20, minor=45 → "4.50")
                QSV_VERSION_MINOR => OS2_VERSION_MINOR,     // SetOSVersion(): minor≥40 → "4.XX" branch
                QSV_VERSION_REVISION => 0,
                QSV_MS_COUNT => ms_count(),
                // The session clock (clock.rs): past 2106 the low half wraps
                // and the high half counts on.
                QSV_TIME_LOW => time as u32,
                QSV_TIME_HIGH => (time >> 32) as u32,
                QSV_TOTPHYSMEM => 128 * 1024 * 1024,
                QSV_TOTRESMEM => 16 * 1024 * 1024,
                QSV_TOTAVAILMEM => 96 * 1024 * 1024,
                QSV_MAXPRMEM => 512 * 1024 * 1024, // max private memory
                QSV_MAXSHMEM => SHARED_ARENA_SIZE, // the shared arena
                // Tenths of a millisecond (Control Program Reference).
                QSV_TIMER_INTERVAL => DOS_TIMER_TICK_MS * 10,
                QSV_MAX_COMP_LENGTH => 255,
                _ => 0, // unknown index
            };
            let offset = (idx - i_start) * 4;
            self.guest_write::<u32>(p_buf + offset, val);
        }
        NO_ERROR
    }

    /// DosGetDateTime (ordinal 230): the current date and time of the
    /// session clock (`clock.rs`) in local time.
    ///
    /// `DATETIME` (12 bytes): hours, minutes, seconds, hundredths, day,
    /// month (UCHARs), year (USHORT), timezone (SHORT, minutes **west** of
    /// UTC — "positive if it is earlier than GMT", Control Program
    /// Reference), weekday (UCHAR, 0 = Sunday).
    pub fn dos_get_date_time(&self, p_dt: u32) -> u32 {
        debug!("  DosGetDateTime");
        if p_dt == 0 { return ERROR_INVALID_PARAMETER; }
        let ms = self.shared.clock.now_ms();
        let Some(lt) = clock::unix_to_local(ms.div_euclid(1000)) else {
            return ERROR_INVALID_PARAMETER;
        };
        let hundredths = (ms.rem_euclid(1000) / 10) as u8;
        self.write_datetime(p_dt, &lt, hundredths)
    }

    /// Write `lt` as a `DATETIME` at `p_dt`. The year is clamped to the
    /// `USHORT`.
    pub(crate) fn write_datetime(&self, p_dt: u32, lt: &clock::LocalTime, hundredths: u8) -> u32 {
        let mut b = [0u8; DATETIME_SIZE as usize];
        b[0] = lt.hours;
        b[1] = lt.minutes;
        b[2] = lt.seconds;
        b[3] = hundredths;
        b[4] = lt.day;
        b[5] = lt.month;
        b[6..8].copy_from_slice(&(lt.year.clamp(0, u16::MAX as i32) as u16).to_le_bytes());
        b[8..10].copy_from_slice(&lt.os2_timezone().to_le_bytes());
        b[10] = lt.weekday;
        match self.guest_write_bytes(p_dt, &b) {
            Some(()) => NO_ERROR,
            None => ERROR_INVALID_PARAMETER,
        }
    }

    /// Read a `DATETIME` at `p_dt`: the date and time and the hundredths,
    /// `None` when it is not readable.
    pub(crate) fn read_datetime(&self, p_dt: u32) -> Option<(clock::LocalTime, u8)> {
        let b = self.guest_read_bytes(p_dt, DATETIME_SIZE as usize)?;
        let year = i32::from(u16::from_le_bytes([b[6], b[7]]));
        Some((clock::LocalTime::new(year, b[5], b[4], b[0], b[1], b[2]), b[3]))
    }

    /// DosSetDateTime (ordinal 292): set the date and time. Warpine cannot
    /// set the host's clock; it moves the session clock of this process and
    /// the children it starts afterwards (`clock.rs`, ADR-0039).
    ///
    /// A field out of range, or a year outside 1980–9999, is
    /// `ERROR_TS_DATETIME`. The weekday and time zone are not used (the
    /// host's time zone stays in force).
    pub fn dos_set_date_time(&self, p_dt: u32) -> u32 {
        let Some((lt, hundredths)) = (p_dt != 0).then(|| self.read_datetime(p_dt)).flatten() else {
            return ERROR_INVALID_PARAMETER;
        };
        debug!("  DosSetDateTime({lt:?})");
        if hundredths >= 100 || !(clock::MIN_SET_YEAR..=clock::MAX_SET_YEAR).contains(&lt.year) {
            return ERROR_TS_DATETIME;
        }
        let Some(secs) = clock::local_to_unix(&lt) else { return ERROR_TS_DATETIME };
        self.shared.clock.set_now_ms(secs.saturating_mul(1000) + i64::from(hundredths) * 10);
        NO_ERROR
    }

    // ── Step 10: Thread / Environment / LIBPATH ──

    /// DosKillThread (ordinal 111): end another thread of the process.
    ///
    /// OS/2 signature: DosKillThread(TID tid)
    ///
    /// The thread is flagged and ends like `DosExit(EXIT_THREAD)` when it
    /// next leaves the guest: at once if it runs guest code (its vCPU is
    /// kicked), after the call if it is blocked in one. A suspended thread
    /// ends when it is resumed. The calling thread itself and threads that
    /// do not run give `ERROR_INVALID_THREADID`.
    pub fn dos_kill_thread(&self, vcpu_id: u32, tid: u32) -> u32 {
        debug!("  DosKillThread(tid={})", tid);
        let caller = super::threads::tid_of_vcpu(vcpu_id);
        let mut table = self.shared.threads.table.lock_or_recover();
        if table.kill(caller, tid) {
            table.kick(tid);
            drop(table);
            // A thread held by the caller's critical section ends now.
            self.shared.threads.wake_crit_waiters();
            NO_ERROR
        } else {
            ERROR_INVALID_THREADID
        }
    }

    /// DosSuspendThread (ordinal 238): stop another thread of the process
    /// until `DosResumeThread`. Suspensions count: each needs a resume.
    /// A thread running guest code stops at once (its vCPU is kicked), one
    /// blocked in a call when the call returns. Not the calling thread;
    /// threads that do not run give `ERROR_INVALID_THREADID`.
    pub fn dos_suspend_thread(&self, vcpu_id: u32, tid: u32) -> u32 {
        debug!("  DosSuspendThread(tid={})", tid);
        self.shared.threads.suspend(super::threads::tid_of_vcpu(vcpu_id), tid)
    }

    /// DosResumeThread (ordinal 237): undo one `DosSuspendThread`;
    /// `ERROR_NOT_FROZEN` if the thread is not suspended.
    pub fn dos_resume_thread(&self, tid: u32) -> u32 {
        debug!("  DosResumeThread(tid={})", tid);
        self.shared.threads.resume(tid)
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
        // In the codepage it was set in, NUL-terminated.
        let cp = self.shared.active_codepage.load(std::sync::atomic::Ordering::Relaxed);
        let mut bytes = super::codepage::cp_encode(&stored, cp);
        bytes.push(0);
        match self.guest_write_bytes(psz_path, &bytes) {
            Some(()) => NO_ERROR,
            None => ERROR_INVALID_PARAMETER,
        }
    }
}

/// `QSV_MS_COUNT`: milliseconds since the host booted, truncated to 32 bits
/// so it wraps like the OS/2 counter.
pub(super) fn ms_count() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable timespec; CLOCK_MONOTONIC always exists.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts); }
    (ts.tv_sec as u64).wrapping_mul(1000).wrapping_add(ts.tv_nsec as u64 / 1_000_000) as u32
}


#[cfg(test)]
mod tests {
    use super::super::Loader;
    use super::super::mutex_ext::MutexExt;

    use super::super::constants::*;

    fn put(l: &Loader, at: u32, s: &str) -> u32 {
        l.guest_write_bytes(at, s.as_bytes()).unwrap();
        l.guest_write::<u8>(at + s.len() as u32, 0).unwrap();
        at
    }

    /// Named shared memory: \SHAREMEM\ names only, one block per name (any
    /// case), the name free again after DosFreeMem; flags need an access bit.
    #[test]
    fn test_alloc_shared_mem_names_and_flags() {
        let l = Loader::new_mock();
        const PPB: u32 = 0x3000;
        let rw = PAG_COMMIT | PAG_READ | PAG_WRITE;
        let name = put(&l, 0x3100, "\\SHAREMEM\\Unit");
        assert_eq!(l.dos_alloc_shared_mem(PPB, name, 4096, rw), NO_ERROR);
        let addr = l.guest_read::<u32>(PPB).unwrap();
        let upper = put(&l, 0x3200, "\\sharemem\\UNIT");
        assert_eq!(l.dos_alloc_shared_mem(PPB, upper, 4096, rw), ERROR_ALREADY_EXISTS);
        assert_eq!(l.dos_get_named_shared_mem(PPB, upper, PAG_READ), NO_ERROR);
        assert_eq!(l.guest_read::<u32>(PPB), Some(addr));
        let bad = put(&l, 0x3300, "\\SEM32\\X");
        assert_eq!(l.dos_alloc_shared_mem(PPB, bad, 4096, rw), ERROR_INVALID_NAME);
        assert_eq!(l.dos_alloc_shared_mem(PPB, 0, 4096, PAG_COMMIT), ERROR_INVALID_PARAMETER, "no access bit");
        assert_eq!(l.dos_alloc_shared_mem(PPB, 0, 4096, rw | 0x8000_0000), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_alloc_shared_mem(0, 0, 4096, rw), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_alloc_shared_mem(PPB, 0, 0, rw), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_free_mem(addr), NO_ERROR);
        assert_eq!(l.dos_get_named_shared_mem(PPB, name, PAG_READ), ERROR_FILE_NOT_FOUND);
        assert_eq!(l.dos_alloc_shared_mem(PPB, name, 4096, rw), NO_ERROR, "name free again");
        assert_eq!(l.dos_free_mem(addr + 1), ERROR_INVALID_ADDRESS);
    }

    /// DosGiveSharedMem: giveable blocks only, compatible access, known
    /// processes; a child process is out of reach (separate Warpine).
    #[test]
    fn test_give_shared_mem() {
        let l = Loader::new_mock();
        const PPB: u32 = 0x3000;
        assert_eq!(l.dos_alloc_shared_mem(PPB, 0, 4096, PAG_COMMIT | PAG_READ | PAG_WRITE | OBJ_GIVEABLE), 0);
        let give = l.guest_read::<u32>(PPB).unwrap();
        assert_eq!(l.dos_alloc_shared_mem(PPB, 0, 4096, PAG_COMMIT | PAG_READ | OBJ_GETTABLE), 0);
        let get = l.guest_read::<u32>(PPB).unwrap();
        assert_eq!(l.dos_give_shared_mem(give, PROCESS_ID, PAG_READ | PAG_WRITE), NO_ERROR);
        assert_eq!(l.dos_give_shared_mem(give, PROCESS_ID, PAG_READ | PAG_GUARD), NO_ERROR);
        assert_eq!(l.dos_give_shared_mem(give, PROCESS_ID, PAG_COMMIT), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_give_shared_mem(give, PROCESS_ID, 0), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_give_shared_mem(give, PROCESS_ID, PAG_EXECUTE), ERROR_ACCESS_DENIED, "more than the block");
        assert_eq!(l.dos_give_shared_mem(get, PROCESS_ID, PAG_READ), ERROR_ACCESS_DENIED, "not giveable");
        assert_eq!(l.dos_give_shared_mem(give + 4096 * 8, PROCESS_ID, PAG_READ), ERROR_INVALID_ADDRESS);
        assert_eq!(l.dos_give_shared_mem(give, 0x7777, PAG_READ), ERROR_INVALID_PROCID);
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = l.shared.process_mgr.lock_or_recover().add_child(child);
        assert_eq!(l.dos_give_shared_mem(give, pid, PAG_READ), ERROR_ACCESS_DENIED, "another Warpine process");
        if let Some(mut c) = l.shared.process_mgr.lock_or_recover().take_child(pid) { let _ = c.wait(); }
    }

    /// DosMapCase on a buffer at the top of the address space: no overflow.
    #[test]
    fn test_map_case_at_the_top_of_memory() {
        let loader = Loader::new_mock();
        loader.map_case_guest_buf(u32::MAX - 1, 16, 932);
        loader.map_case_guest_buf(u32::MAX, 1, 437);
    }

    /// Query QSV indexes `first..=last` into a fresh guest buffer.
    fn query_sys_info(loader: &Loader, first: u32, last: u32) -> Vec<u32> {
        let n = last - first + 1;
        let buf = loader.shared.mem_mgr.lock_or_recover().alloc(n * 4).unwrap();
        assert_eq!(loader.dos_query_sys_info(first, last, buf, n * 4),
                   super::super::constants::NO_ERROR);
        (0..n).map(|i| loader.guest_read::<u32>(buf + i * 4).unwrap()).collect()
    }

    #[test]
    fn test_qsv_ms_count_advances_in_milliseconds() {
        use super::super::constants::QSV_MS_COUNT;
        let loader = Loader::new_mock();
        let before = query_sys_info(&loader, QSV_MS_COUNT, QSV_MS_COUNT)[0];
        std::thread::sleep(std::time::Duration::from_millis(50));
        let after = query_sys_info(&loader, QSV_MS_COUNT, QSV_MS_COUNT)[0];
        let elapsed = after.wrapping_sub(before);
        assert!((50..5000).contains(&elapsed), "elapsed {elapsed} ms");
    }

    #[test]
    fn test_qsv_time_low_high_is_the_unix_time() {
        use super::super::constants::{QSV_TIME_HIGH, QSV_TIME_LOW};
        let loader = Loader::new_mock();
        let v = query_sys_info(&loader, QSV_TIME_LOW, QSV_TIME_HIGH);
        let guest = (v[1] as u64) << 32 | v[0] as u64;
        let host = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(host.abs_diff(guest) <= 2, "guest {guest}, host {host}");
        // Well after 2020-01-01, so a wrong index (0) cannot pass.
        assert!(guest > 1_577_836_800);
    }

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

    /// Names beyond ASCII edit by character.
    #[test]
    fn test_edit_name_non_ascii() {
        assert_eq!(Loader::apply_edit_name("résumé.txt", "*.bak"), "résumé.bak");
        assert_eq!(Loader::apply_edit_name("日本語.txt", "?x*.*"), "日x語.txt");
    }

    /// The result and its NUL must fit the target buffer; a length of 0
    /// is an overflow, never a wrapped subtraction.
    #[test]
    fn test_dos_edit_name_checks_the_target_size() {
        let loader = Loader::new_mock();
        let (src, edit, target) = (0x3000u32, 0x3100u32, 0x3200u32);
        loader.guest_write_bytes(src, b"readme.txt\0");
        loader.guest_write_bytes(edit, b"*.bak\0");
        loader.guest_write_bytes(target, &[0xAA; 16]);
        assert_eq!(loader.dos_edit_name(0, src, edit, target, 0), ERROR_BUFFER_OVERFLOW);
        assert_eq!(loader.dos_edit_name(0, src, edit, target, 10), ERROR_BUFFER_OVERFLOW, "10 bytes and a NUL");
        assert_eq!(loader.guest_read::<u8>(target), Some(0xAA), "nothing written");
        assert_eq!(loader.dos_edit_name(0, src, edit, target, 11), NO_ERROR);
        assert_eq!(loader.read_guest_string(target), "readme.bak");
        assert_eq!(loader.dos_edit_name(0, src, edit, 0, 11), ERROR_INVALID_PARAMETER);
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
        assert_eq!(QSV_MS_COUNT, 14);
        assert_eq!(QSV_TIME_LOW, 15);
        assert_eq!(QSV_TIME_HIGH, 16);
        assert_eq!(QSV_TOTPHYSMEM, 17);
        assert_eq!(QSV_MAX_COMP_LENGTH, 23);
        // Ensure range queries would work: iStart <= iLast
        const { assert!(QSV_VERSION_MAJOR < QSV_VERSION_REVISION) };
        const { assert!(QSV_MAX_PATH_LENGTH < QSV_MAX_COMP_LENGTH) };
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
        // No such thread: ERROR_INVALID_THREADID.
        assert_eq!(loader.dos_kill_thread(0, 99),
                   super::super::constants::ERROR_INVALID_THREADID);
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

        // A name outside ASCII comes back in the guest's codepage (437:
        // é is 0x82), as it was set, not as UTF-8.
        loader.shared.active_codepage.store(437, std::sync::atomic::Ordering::Relaxed);
        loader.guest_write_bytes(buf, b"C:\\CAF\x82\0");
        assert_eq!(loader.dos_set_ext_libpath(buf, BEGIN_LIBPATH), NO_ERROR);
        assert_eq!(loader.dos_query_ext_libpath(out_begin, BEGIN_LIBPATH), NO_ERROR);
        assert_eq!(loader.guest_read_bytes(out_begin, 8).as_deref(), Some(&b"C:\\CAF\x82\0"[..]));
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
        // The VIO screen's codepage follows (DBCS pairing, VioCheckCharType).
        assert_eq!(loader.dos_set_process_cp(932), NO_ERROR);
        assert_eq!(loader.shared.console_mgr.lock_or_recover().codepage, 932);
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
            object_sizes: Vec::new(),
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

    /// DosQueryProcAddr by name on a builtin handle: the registry gives the
    /// module's own ordinal, added to the module base once (it used to be
    /// the flat ordinal, so the base was added twice).
    #[test]
    fn test_dos_query_proc_addr_builtin_name() {
        use super::super::mutex_ext::MutexExt;
        use super::super::constants::{NO_ERROR, ERROR_PROC_NOT_FOUND, MAGIC_API_BASE, WARPINE_BASE, PMSHAPI_BASE};
        let loader = Loader::new_mock();
        let name_addr = loader.shared.mem_mgr.lock_or_recover().alloc(32).unwrap();
        let hmod_addr = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        let pfn_addr = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        let by_name = |module: &[u8], proc_name: &[u8]| {
            loader.guest_write_bytes(name_addr, module);
            let _ = loader.dos_load_module(0, 0, name_addr, hmod_addr);
            let h = loader.guest_read::<u32>(hmod_addr).unwrap();
            loader.guest_write_bytes(name_addr, proc_name);
            let rc = loader.dos_query_proc_addr(h, 0, name_addr, pfn_addr);
            (rc, loader.guest_read::<u32>(pfn_addr).unwrap())
        };
        assert_eq!(by_name(b"WARPINE\0", b"WrpTime64\0"),
                   (NO_ERROR, MAGIC_API_BASE as u32 + WARPINE_BASE + 1));
        assert_eq!(by_name(b"WARPINE.DLL\0", b"WrpMakeTime64\0"),
                   (NO_ERROR, MAGIC_API_BASE as u32 + WARPINE_BASE + 4));
        assert_eq!(by_name(b"PMSHAPI\0", b"WinStartApp\0"),
                   (NO_ERROR, MAGIC_API_BASE as u32 + PMSHAPI_BASE + 119));
        assert_eq!(by_name(b"WARPINE\0", b"WrpNoSuchCall\0"), (ERROR_PROC_NOT_FOUND, 0));
    }

    /// A handle of no loaded module, or one already freed:
    /// ERROR_INVALID_HANDLE (it was NO_ERROR).
    #[test]
    fn test_dos_free_module_of_no_module_is_an_invalid_handle() {
        use super::super::{ApiResult, constants::{ERROR_INVALID_HANDLE, NO_ERROR}};
        let loader = Loader::new_mock();
        assert!(matches!(loader.dos_free_module(0x7777), ApiResult::Normal(ERROR_INVALID_HANDLE)));
        let h = register_fake_dll(&loader, "ONCE", vec![], None);
        assert!(matches!(loader.dos_free_module(h), ApiResult::Normal(NO_ERROR)));
        assert!(matches!(loader.dos_free_module(h), ApiResult::Normal(ERROR_INVALID_HANDLE)), "freed already");
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

    /// DosGetResource / DosQueryResourceSize look in the module named by
    /// hmod (0 = the executable); a DLL's resources go with its last
    /// DosFreeModule.
    #[test]
    fn test_resources_are_found_by_module_and_dropped_on_unload() {
        use super::super::mutex_ext::MutexExt;
        use super::super::constants::NO_ERROR;
        use super::super::managers::EXE_RESOURCE_MODULE;

        let loader = Loader::new_mock();
        let base = loader.shared.mem_mgr.lock_or_recover().alloc(64).unwrap();
        let h = register_fake_dll(&loader, "RESDLL", vec![base], None);
        loader.shared.dll_mgr.lock_or_recover().increment_refcount(h);
        {
            let mut res = loader.shared.resource_mgr.lock_or_recover();
            res.add(EXE_RESOURCE_MODULE, 5, 1, 0x7000, 11);
            res.add(h, 5, 1, base, 22);
        }
        let out = loader.shared.mem_mgr.lock_or_recover().alloc(4).unwrap();
        let size_of = |hmod: u32| {
            let rc = loader.dos_query_resource_size(hmod, 5, 1, out);
            (rc == NO_ERROR).then(|| loader.guest_read::<u32>(out).unwrap())
        };
        assert_eq!(size_of(0), Some(11), "NULLHANDLE: the executable's");
        assert_eq!(size_of(h), Some(22), "the DLL's own");
        assert_eq!(loader.dos_get_resource(h, 5, 1, out), NO_ERROR);
        assert_eq!(loader.guest_read::<u32>(out).unwrap(), base);
        assert_ne!(loader.dos_get_resource(h, 5, 2, out), NO_ERROR, "no such id in the DLL");

        let _ = loader.dos_free_module(h);
        assert_eq!(size_of(h), Some(22), "still referenced once: still there");
        let _ = loader.dos_free_module(h);
        assert_eq!(size_of(h), None, "unloaded: gone");
        assert_eq!(size_of(0), Some(11), "the executable's stay");
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
