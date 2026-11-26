// SPDX-License-Identifier: GPL-3.0-or-later
//
// Smaller DOSCALLS and QUECALLS calls (ADR-0086): the high-resolution
// timer, the module an address lies in, error classification, a
// procedure's type, thread and process checks, file-handle protection
// (the DosProtect* calls on top of the plain file calls) and DosPeekQueue.
// Signatures from Open Watcom's `bsedos.h` and the Toolkit's.

use std::time::Instant;

use super::constants::*;
use super::mutex_ext::MutexExt;

/// The rate DosTmrQueryTime counts at: the PC timer's 1.193182 MHz
/// (unverified for OS/2).
pub const TIMER_FREQUENCY: u64 = 1_193_182;
/// `<bsedos.h>`: DosQueryProcType's answers.
const PT_32BIT: u32 = 1;
/// `<bseerr.h>`: error classes, actions and loci (DosErrClass).
const ERRCLASS_OUTRES: u32 = 1;
const ERRCLASS_TEMPSIT: u32 = 2;
const ERRCLASS_AUTH: u32 = 3;
const ERRCLASS_INTRN: u32 = 4;
const ERRCLASS_APPERR: u32 = 7;
const ERRCLASS_NOTFND: u32 = 8;
const ERRCLASS_BADFMT: u32 = 9;
const ERRCLASS_LOCKED: u32 = 10;
const ERRCLASS_ALREADY: u32 = 12;
const ERRCLASS_UNK: u32 = 13;
const ERRACT_RETRY: u32 = 1;
const ERRACT_DLYRET: u32 = 2;
const ERRACT_USER: u32 = 3;
const ERRACT_ABORT: u32 = 4;
const ERRLOC_UNK: u32 = 1;
const ERRLOC_DISK: u32 = 2;
const ERRLOC_MEM: u32 = 5;
/// `<bseerr.h>`: errors these calls return.
const ERROR_INVALID_THREADID: u32 = 309;
const ERROR_QUE_ELEMENT_NOT_EXIST: u32 = 333;
const ERROR_QUE_INVALID_HANDLE: u32 = 337;
const ERROR_QUE_EMPTY: u32 = 342;
const ERROR_INVALID_ADDRESS: u32 = 487;
/// `DCWW_WAIT` (0) / `DCWW_NOWAIT` (1).
pub const DCWW_WAIT: u32 = 0;
/// The multiplier that makes a handle's FHLOCK (a handle-protection
/// lock id); any value a program does not guess works (unverified).
const FHLOCK_SALT: u32 = 0x9E37_79B1;

/// The FHLOCK of handle `hf`.
pub fn fh_lock(hf: u32) -> u32 { hf.wrapping_mul(FHLOCK_SALT) | 1 }

/// DosErrClass's classification of OS/2 error `code`: (class, action,
/// locus). From the classes' meanings in `<bseerr.h>`; the per-error
/// table OS/2 uses is not public (unverified).
pub fn error_class(code: u32) -> (u32, u32, u32) {
    match code {
        2 | 3 | 15 | 18 | 110 | 123 => (ERRCLASS_NOTFND, ERRACT_USER, ERRLOC_DISK),
        4 | 8 | 112 => (ERRCLASS_OUTRES, ERRACT_ABORT, if code == 8 { ERRLOC_MEM } else { ERRLOC_DISK }),
        5 | 19 | 65 => (ERRCLASS_AUTH, ERRACT_USER, ERRLOC_DISK),
        32 | 33 => (ERRCLASS_LOCKED, ERRACT_DLYRET, ERRLOC_DISK),
        80 | 183 => (ERRCLASS_ALREADY, ERRACT_USER, ERRLOC_DISK),
        6 | 87 | 111 | 124 | 487 => (ERRCLASS_APPERR, ERRACT_ABORT, ERRLOC_UNK),
        11 | 13 | 193 => (ERRCLASS_BADFMT, ERRACT_ABORT, ERRLOC_UNK),
        95 | 170 => (ERRCLASS_TEMPSIT, ERRACT_RETRY, ERRLOC_UNK),
        1 => (ERRCLASS_INTRN, ERRACT_ABORT, ERRLOC_UNK),
        _ => (ERRCLASS_UNK, ERRACT_ABORT, ERRLOC_UNK),
    }
}

/// When the timer started (the process's first DosTmrQueryTime).
fn timer_start() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(Instant::now)
}

impl super::Loader {
    /// DosTmrQueryFreq(PULONG pulTmrFreq).
    pub fn dos_tmr_query_freq(&self, p: u32) -> u32 {
        match self.guest_write::<u32>(p, TIMER_FREQUENCY as u32) { Some(()) => NO_ERROR, None => ERROR_INVALID_PARAMETER }
    }

    /// DosTmrQueryTime(PQWORD pqwTmrTime): the counter, at
    /// `TIMER_FREQUENCY`, low ULONG first.
    pub fn dos_tmr_query_time(&self, p: u32) -> u32 {
        let ticks = (timer_start().elapsed().as_nanos() * TIMER_FREQUENCY as u128 / 1_000_000_000) as u64;
        match self.guest_write_bytes(p, &ticks.to_le_bytes()) { Some(()) => NO_ERROR, None => ERROR_INVALID_PARAMETER }
    }

    /// DosQueryModFromEIP(PHMODULE phMod, PULONG pObjNum, ULONG cbBuf, PCHAR pBuf,
    /// PULONG pOffset, ULONG address): the module, object (from 0,
    /// unverified) and offset `address` lies in, and the module's name.
    pub fn dos_query_mod_from_eip(&self, phmod: u32, pobj: u32, cb: u32, pbuf: u32, poff: u32, address: u32) -> u32 {
        let found = {
            let dlls = self.shared.dll_mgr.lock_or_recover();
            let exe = dlls.exe_objects.iter().enumerate()
                .find(|&(_, &(base, size))| address >= base && address - base < size)
                .map(|(i, &(base, _))| (self.guest_read::<u32>(PIB_BASE + 8).unwrap_or(0), i as u32, address - base, dlls.exe_name.clone()));
            exe.or_else(|| dlls.find_object(address))
        };
        let Some((hmod, obj, off, name)) = found else { return ERROR_INVALID_ADDRESS };
        if name.len() as u32 >= cb { return ERROR_BUFFER_OVERFLOW; }
        let writes = [self.guest_write::<u32>(phmod, hmod), self.guest_write::<u32>(pobj, obj),
                      self.guest_write::<u32>(poff, off), self.guest_write_bytes(pbuf, name.as_bytes()),
                      self.guest_write::<u8>(pbuf.wrapping_add(name.len() as u32), 0)];
        if writes.iter().all(Option::is_some) { NO_ERROR } else { ERROR_INVALID_PARAMETER }
    }

    /// DosErrClass(ULONG code, PULONG pClass, PULONG pAction, PULONG pLocus).
    pub fn dos_err_class(&self, code: u32, pclass: u32, paction: u32, plocus: u32) -> u32 {
        let (class, action, locus) = error_class(code);
        let ok = [self.guest_write::<u32>(pclass, class), self.guest_write::<u32>(paction, action),
                  self.guest_write::<u32>(plocus, locus)];
        if ok.iter().all(Option::is_some) { NO_ERROR } else { ERROR_INVALID_PARAMETER }
    }

    /// DosQueryProcType(HMODULE hmod, ULONG ordinal, PCSZ pszName, PULONG pulProcType):
    /// every procedure Warpine finds is 32-bit (its 16-bit entry points
    /// are reached through thunks).
    pub fn dos_query_proc_type(&self, hmod: u32, ordinal: u32, psz: u32, ptype: u32) -> u32 {
        let Some(scratch) = self.alloc_zeroed(4) else { return ERROR_NOT_ENOUGH_MEMORY };
        let rc = self.dos_query_proc_addr(hmod, ordinal, psz, scratch);
        self.shared.mem_mgr.lock_or_recover().free(scratch);
        if rc != NO_ERROR { return rc; }
        match self.guest_write::<u32>(ptype, PT_32BIT) { Some(()) => NO_ERROR, None => ERROR_INVALID_PARAMETER }
    }

    /// DosVerifyPidTid(PID pid, TID tid): whether thread `tid` of process
    /// `pid` runs; only this process's threads can be checked.
    pub fn dos_verify_pid_tid(&self, pid: u32, tid: u32) -> u32 {
        let own = self.guest_read::<u32>(PIB_BASE).unwrap_or(0);
        if pid != own { return ERROR_INVALID_PROCID; }
        if self.shared.threads.table.lock_or_recover().is_running(tid) { NO_ERROR } else { ERROR_INVALID_THREADID }
    }

    /// DosCancelLockRequest(HFILE hf, PFILELOCK pflLock): Warpine's lock
    /// requests do not wait in the background, so there is never one to
    /// cancel; a file handle answers NO_ERROR (unverified).
    pub fn dos_cancel_lock_request(&self, hf: u32, plock: u32) -> u32 {
        if plock == 0 { return ERROR_INVALID_PARAMETER; }
        if self.shared.drive_mgr.lock_or_recover().is_file(hf) { NO_ERROR } else { ERROR_INVALID_HANDLE }
    }

    // ── List I/O (DosListIO, DosListIOL) ─────────────────────────────────

    /// DosListIO (ordinal 996) / DosListIOL (990, `large`): `count`
    /// `LISTIO` (52 bytes) or `LISTIOL` (56: a `LONGLONG` offset) entries
    /// at `plist`, each a seek of its handle (`CmdFlag`'s low bits:
    /// `FILE_BEGIN` / `FILE_CURRENT` / `FILE_END`) to `Offset` and then a
    /// read (`LISTIO_READ`) or write (`LISTIO_WRITE`) of `NumBytes` at
    /// `pBuffer`; each entry gets its `Actual` and `RetCode`.
    /// `LISTIO_ORDERED` and `LISTIO_UNORDERED` both run the entries in
    /// order. Warpine's choices (unverified): the result is the first
    /// entry's error, or `NO_ERROR`; an entry with neither or both of
    /// `LISTIO_READ` / `LISTIO_WRITE` fails with `ERROR_INVALID_PARAMETER`
    /// and the others still run; more than `MAX_LISTIO_ENTRIES` is
    /// `ERROR_INVALID_PARAMETER`.
    pub fn dos_list_io(&self, mode: u32, count: u32, plist: u32, large: bool) -> u32 {
        const SEEK_METHOD: u32 = 3;
        if mode != LISTIO_ORDERED && mode != LISTIO_UNORDERED { return ERROR_INVALID_PARAMETER; }
        if count > MAX_LISTIO_ENTRIES { return ERROR_INVALID_PARAMETER; }
        // Field offsets; the LONGLONG offset moves the rest by 4.
        let (size, shift) = if large { (LISTIOL_SIZE, 4) } else { (LISTIO_SIZE, 0) };
        let mut first_error = NO_ERROR;
        for i in 0..count {
            let Some(e) = i.checked_mul(size).and_then(|o| plist.checked_add(o)) else { return ERROR_INVALID_PARAMETER };
            let field = |off: u32| e.wrapping_add(off);
            let (Some(hf), Some(cmd)) = (self.guest_read::<u32>(e), self.guest_read::<u32>(field(4))) else {
                return ERROR_INVALID_PARAMETER;
            };
            let offset = if large { self.guest_read::<i64>(field(8)) } else { self.guest_read::<i32>(field(8)).map(i64::from) };
            let (Some(offset), Some(buf), Some(len)) =
                (offset, self.guest_read::<u32>(field(12 + shift)), self.guest_read::<u32>(field(16 + shift))) else {
                return ERROR_INVALID_PARAMETER;
            };
            let actual = field(20 + shift);
            let _ = self.guest_write::<u32>(actual, 0);
            let rc = match cmd & (LISTIO_READ | LISTIO_WRITE) {
                LISTIO_READ | LISTIO_WRITE => match self.dos_set_file_ptr_l(hf, offset, cmd & SEEK_METHOD, 0) {
                    NO_ERROR if cmd & LISTIO_READ != 0 => self.dos_read(hf, buf, len, actual),
                    NO_ERROR => self.dos_write(hf, buf, len, actual),
                    e => e,
                },
                _ => ERROR_INVALID_PARAMETER,
            };
            let _ = self.guest_write::<u32>(field(24 + shift), rc);
            if first_error == NO_ERROR { first_error = rc; }
        }
        first_error
    }

    // ── File-handle protection (DosProtect*) ────────────────────────────

    /// Whether FHLOCK `lock` protects handle `hf` (else
    /// ERROR_INVALID_HANDLE).
    fn protected(&self, hf: u32, lock: u32) -> Result<(), u32> {
        if lock == fh_lock(hf) { Ok(()) } else { Err(ERROR_INVALID_HANDLE) }
    }

    /// DosProtectOpen(…, pfhFileHandleLockID): DosOpen, and the handle's
    /// FHLOCK.
    #[allow(clippy::too_many_arguments)]
    pub fn dos_protect_open(&self, name: u32, phf: u32, paction: u32, flags: u32, mode: u32, peaop2: u32, plock: u32) -> u32 {
        let rc = self.dos_open_with_eas(name, phf, paction, flags, mode, peaop2);
        if rc == NO_ERROR {
            let hf = self.guest_read::<u32>(phf).unwrap_or(0);
            if plock == 0 || self.guest_write::<u32>(plock, fh_lock(hf)).is_none() { return ERROR_INVALID_PARAMETER; }
        }
        rc
    }

    /// The other DosProtect* calls: the plain call, once the FHLOCK is
    /// the handle's.
    pub fn dos_protect(&self, hf: u32, lock: u32, call: impl FnOnce() -> u32) -> u32 {
        match self.protected(hf, lock) { Ok(()) => call(), Err(rc) => rc }
    }

    // ── DosPeekQueue ────────────────────────────────────────────────────

    /// DosPeekQueue(HQUEUE hq, PREQUESTDATA pRequest, PULONG pcbData, PPVOID ppbuf,
    /// PULONG pElement, BOOL32 fWait, PBYTE pbPriority, HEV hsem): a copy of
    /// an element, left in the queue: the first (`*pElement` 0), or the one
    /// after the element code given; its code goes to `*pElement`.
    #[allow(clippy::too_many_arguments)]
    pub fn dos_peek_queue(&self, hq: u32, preq: u32, pcb: u32, ppbuf: u32, pelem: u32, wait: u32, pprio: u32) -> u32 {
        let after = self.guest_read::<u32>(pelem).unwrap_or(0);
        let Some(q_arc) = self.shared.queue_mgr.lock_or_recover().get(hq) else { return ERROR_QUE_INVALID_HANDLE };
        let (cond, cond_lock) = {
            let q = q_arc.lock_or_recover();
            (std::sync::Arc::clone(&q.cond), std::sync::Arc::clone(&q.cond_lock))
        };
        loop {
            if self.stop_or_restart() { return ERROR_INTERRUPT; }
            let found = {
                let q = q_arc.lock_or_recover();
                let start = match after {
                    0 => Some(0),
                    code => q.items.iter().position(|e| e.id == code).map(|p| p + 1),
                };
                match start {
                    None => return ERROR_QUE_ELEMENT_NOT_EXIST,
                    Some(i) => q.items.get(i).map(|e| (e.id, e.data.clone(), e.event, e.priority)),
                }
            };
            if let Some((id, data, event, priority)) = found {
                let Some(buf) = self.alloc_zeroed((data.len() as u32).max(1)) else { return ERROR_NOT_ENOUGH_MEMORY };
                self.guest_write_bytes(buf, &data);
                self.guest_write::<u32>(ppbuf, buf);
                self.guest_write::<u32>(pcb, data.len() as u32);
                self.guest_write::<u32>(pelem, id);
                if preq != 0 { self.guest_write::<u32>(preq + 4, event); }
                if pprio != 0 { self.guest_write::<u8>(pprio, priority as u8); }
                return NO_ERROR;
            }
            if wait != DCWW_WAIT { return ERROR_QUE_EMPTY; }
            let guard = cond_lock.lock_or_recover();
            let _ = cond.wait_timeout(guard, std::time::Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_classified() {
        assert_eq!(error_class(2), (ERRCLASS_NOTFND, ERRACT_USER, ERRLOC_DISK));
        assert_eq!(error_class(8), (ERRCLASS_OUTRES, ERRACT_ABORT, ERRLOC_MEM));
        assert_eq!(error_class(32).0, ERRCLASS_LOCKED);
        assert_eq!(error_class(9999), (ERRCLASS_UNK, ERRACT_ABORT, ERRLOC_UNK));
    }

    #[test]
    fn built_in_modules_are_loaded_and_only_find_what_they_implement() {
        let l = super::super::Loader::new_mock();
        let buf = l.alloc_zeroed(64).unwrap();
        l.guest_write_bytes(buf, b"DOSCALLS\0");
        assert_eq!(l.dos_query_module_handle(buf, buf + 16), NO_ERROR, "DOSCALLS is always loaded");
        let hmod = l.guest_read::<u32>(buf + 16).unwrap();
        assert_eq!(l.dos_query_proc_addr(hmod, 234, 0, buf + 20), NO_ERROR, "DosExit");
        assert_ne!(l.guest_read::<u32>(buf + 20), Some(0));
        assert_eq!(l.dos_query_proc_addr(hmod, 991, 0, buf + 20), ERROR_PROC_NOT_FOUND, "DOS16FSRAMSEMREQUEST2 is not implemented");
    }

    /// DosListIO checks its mode and bound before any entry; an entry with
    /// a bad command or handle gets its RetCode, and the call the first
    /// entry's error.
    #[test]
    fn list_io_checks_mode_count_and_entries() {
        let l = super::super::Loader::new_mock();
        let p = l.alloc_zeroed(2 * LISTIOL_SIZE).unwrap();
        assert_eq!(l.dos_list_io(0, 1, p, false), ERROR_INVALID_PARAMETER, "no mode");
        assert_eq!(l.dos_list_io(LISTIO_ORDERED, MAX_LISTIO_ENTRIES + 1, p, true), ERROR_INVALID_PARAMETER);
        assert_eq!(l.dos_list_io(LISTIO_ORDERED, 0, p, false), NO_ERROR, "nothing to do");
        // Entry 0: no command; entry 1 (LISTIOL): a read of a handle that is no file.
        l.guest_write::<u32>(p + LISTIOL_SIZE, 9999);
        l.guest_write::<u32>(p + LISTIOL_SIZE + 4, LISTIO_READ);
        assert_eq!(l.dos_list_io(LISTIO_UNORDERED, 2, p, true), ERROR_INVALID_PARAMETER, "the first entry's error");
        assert_eq!(l.guest_read::<u32>(p + 28), Some(ERROR_INVALID_PARAMETER), "entry 0's RetCode");
        assert_ne!(l.guest_read::<u32>(p + LISTIOL_SIZE + 28), Some(NO_ERROR), "entry 1 still ran and failed");
    }

    #[test]
    fn locks_belong_to_their_handle() {
        assert_ne!(fh_lock(3), fh_lock(4));
        assert_eq!(fh_lock(3), fh_lock(3));
        assert_ne!(fh_lock(0), 0);
    }
}
