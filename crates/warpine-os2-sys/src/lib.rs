//! Raw OS/2 API bindings for Warpine guest programs.
//!
//! Calling conventions:
//! - DOSCALLS: `extern "C"` (cdecl, caller-cleanup).  Warpine only pops the
//!   return address after a DOSCALLS thunk; the caller is responsible for
//!   removing arguments from the stack.
//! - VIOCALLS / KBDCALLS: `extern "stdcall"` (callee-cleanup).  Warpine pops
//!   both the return address AND all arguments inside the thunk handler.  Using
//!   stdcall prevents the Rust caller from emitting a redundant `add esp, N`.
//!
//! Pascal / stdcall argument reversal:
//! OS/2 VIO/KBD use Pascal calling convention (arguments pushed left-to-right
//! so the LAST argument lands at ESP+4).  `extern "stdcall"` pushes arguments
//! right-to-left, which would produce the wrong stack layout.  To compensate,
//! the argument order in each VIO/KBD declaration is **reversed** relative to
//! the real OS/2 prototype so that the compiled push sequence matches what
//! Warpine expects to find on the stack.

#![no_std]

// ── OS/2 type aliases ────────────────────────────────────────────────────────

/// Standard OS/2 API return code (0 = success).
pub type APIRET = u32;

/// Unsigned 32-bit integer.
pub type ULONG = u32;

/// Unsigned 16-bit integer.
pub type USHORT = u16;

/// File handle.
pub type HFILE = u32;

/// VIO (video) session handle.
pub type HVIO = u32;

/// KBD (keyboard) session handle.
pub type HKBD = u32;

/// Pointer to void (opaque byte pointer).
pub type PVOID = *mut u8;

/// Pointer to a constant NUL-terminated string.
pub type PCSZ = *const u8;

/// Thread function prototype for `DosCreateThread`.
pub type PFNTHREAD = unsafe extern "C" fn(ULONG);

// ── OS/2 structures ──────────────────────────────────────────────────────────

/// Date/time block returned by `DosGetDateTime`.
#[repr(C)]
pub struct DATETIME {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub hundredths: u8,
    pub day: u8,
    pub month: u8,
    pub year: u16,
    pub timezone: i16,
    pub weekday: u8,
}

/// Key information block filled by `KbdCharIn`.
#[repr(C)]
pub struct KBDKEYINFO {
    /// ASCII character code (0 if extended key).
    pub chChar: u8,
    /// Hardware scan code.
    pub chScan: u8,
    /// Key status flags (e.g. `0x40` = key-down complete).
    pub fbStatus: u8,
    /// NLS shift state.
    pub bNlsShift: u8,
    /// Shift/control key state bitmask.
    pub fsState: u16,
    /// Millisecond timestamp.
    pub time: u32,
}

/// Keyboard session status block for `KbdGetStatus`.
#[repr(C)]
pub struct KBDINFO {
    /// Size of this structure in bytes (must be set to `sizeof(KBDINFO)` = 10).
    pub cb: u16,
    /// Input mode mask.
    pub fsMask: u16,
    /// Turn-around character (default CR = 0x0D).
    pub chTurnAround: u16,
    /// Interim character flags.
    pub fsInterim: u16,
    /// Current shift-state flags.
    pub fsState: u16,
}

// ── DOSCALLS — extern "C" (caller-cleanup) ───────────────────────────────────

extern "C" {
    // ── Process / Thread ─────────────────────────────────────────────────────

    /// Terminate the current process or thread.
    ///
    /// `ulTermType`: 0 = current thread, 1 = entire process.
    /// Never returns.
    pub fn DosExit(ulTermType: ULONG, ulResultCode: ULONG) -> !;

    /// Create a new OS/2 thread.
    ///
    /// `ptid` receives the new thread ID.
    /// `pfn` is the thread function; it receives `param` as its sole argument.
    /// `flag`: 0 = run immediately, 1 = start suspended.
    /// `cbStack`: stack size in bytes (0 = use default, typically 8192).
    pub fn DosCreateThread(
        ptid: *mut ULONG,
        pfn: PFNTHREAD,
        param: ULONG,
        flag: ULONG,
        cbStack: ULONG,
    ) -> APIRET;

    /// Wait for a thread to terminate.
    ///
    /// On success `*ptid` holds the thread ID that exited.
    /// `option`: 0 = `DCWW_WAIT`, 1 = `DCWW_NOWAIT`.
    pub fn DosWaitThread(ptid: *mut ULONG, option: ULONG) -> APIRET;

    /// Kill (forcibly terminate) another thread in the current process.
    pub fn DosKillThread(tid: ULONG) -> APIRET;

    /// Retrieve pointers to the Thread Information Block and Process
    /// Information Block for the calling thread.
    pub fn DosGetInfoBlocks(pptib: *mut *mut u8, pppib: *mut *mut u8) -> APIRET;

    // ── Time ─────────────────────────────────────────────────────────────────

    /// Suspend execution for `msec` milliseconds.
    pub fn DosSleep(msec: ULONG) -> APIRET;

    /// Fill `pdt` with the current system date and time.
    pub fn DosGetDateTime(pdt: *mut DATETIME) -> APIRET;

    // ── System Information ────────────────────────────────────────────────────

    /// Query system variables.
    ///
    /// `iStart`/`iLast`: first/last variable index (1-based, see `QSV_*` constants).
    /// Results written to `pBuf` as an array of `ULONG` values.
    pub fn DosQuerySysInfo(
        iStart: ULONG,
        iLast: ULONG,
        pBuf: *mut ULONG,
        cbBuf: ULONG,
    ) -> APIRET;

    // ── File I/O ─────────────────────────────────────────────────────────────

    /// Write bytes to an open file handle.
    ///
    /// Warpine STDOUT handle = 1, STDERR handle = 2.
    pub fn DosWrite(
        hFile: HFILE,
        pBuffer: *const u8,
        cbWrite: ULONG,
        pcbActual: *mut ULONG,
    ) -> APIRET;

    /// Read bytes from an open file handle.
    pub fn DosRead(
        hFile: HFILE,
        pBuffer: *mut u8,
        cbRead: ULONG,
        pcbActual: *mut ULONG,
    ) -> APIRET;

    /// Open or create a file.
    ///
    /// `peaop2`: pass `core::ptr::null_mut()` when no extended attributes are needed.
    pub fn DosOpen(
        pszFileName: PCSZ,
        phf: *mut HFILE,
        pulAction: *mut ULONG,
        cbFile: ULONG,
        ulAttribute: ULONG,
        fsOpenFlags: ULONG,
        fsOpenMode: ULONG,
        peaop2: *mut u8,
    ) -> APIRET;

    /// Close an open file handle.
    pub fn DosClose(hf: HFILE) -> APIRET;

    // ── Memory ───────────────────────────────────────────────────────────────

    /// Allocate a block of memory.
    ///
    /// `flag` bit mask: `0x01` PAG_READ, `0x02` PAG_WRITE, `0x10` PAG_COMMIT.
    /// Pass `0x13` (READ|WRITE|COMMIT) for conventional usage.
    pub fn DosAllocMem(ppb: *mut PVOID, cb: ULONG, flag: ULONG) -> APIRET;

    /// Free a block previously allocated with `DosAllocMem`.
    pub fn DosFreeMem(pb: PVOID) -> APIRET;

    /// Change the access/commit attributes of an allocated memory region.
    pub fn DosSetMem(pb: PVOID, cb: ULONG, flag: ULONG) -> APIRET;

    // ── Event Semaphores ─────────────────────────────────────────────────────

    /// Create a named or anonymous event semaphore.
    ///
    /// Pass `pszName = null()` for an anonymous semaphore.
    /// `flAttr`: 0 for private.  `fState`: 0 = reset, 1 = posted.
    pub fn DosCreateEventSem(
        pszName: *const u8,
        phev: *mut u32,
        flAttr: ULONG,
        fState: ULONG,
    ) -> APIRET;

    /// Open an existing named event semaphore.
    pub fn DosOpenEventSem(pszName: *const u8, phev: *mut u32) -> APIRET;

    /// Close an event semaphore handle.
    pub fn DosCloseEventSem(hev: u32) -> APIRET;

    /// Post (signal) an event semaphore.
    pub fn DosPostEventSem(hev: u32) -> APIRET;

    /// Wait until an event semaphore is posted or the timeout expires.
    ///
    /// `ulTimeout`: milliseconds; use `0xFFFF_FFFF` (`SEM_INDEFINITE_WAIT`) to
    /// block without a timeout.
    pub fn DosWaitEventSem(hev: u32, ulTimeout: ULONG) -> APIRET;

    // ── Mutex Semaphores ─────────────────────────────────────────────────────

    /// Create a named or anonymous mutex semaphore.
    ///
    /// `fState`: 0 = unowned, 1 = owned by the calling thread.
    pub fn DosCreateMutexSem(
        pszName: *const u8,
        phmtx: *mut u32,
        flAttr: ULONG,
        fState: ULONG,
    ) -> APIRET;

    /// Open an existing named mutex semaphore.
    pub fn DosOpenMutexSem(pszName: *const u8, phmtx: *mut u32) -> APIRET;

    /// Close a mutex semaphore handle.
    pub fn DosCloseMutexSem(hmtx: u32) -> APIRET;

    /// Request (acquire) a mutex semaphore.
    ///
    /// `ulTimeout`: milliseconds; use `0xFFFF_FFFF` to block without a timeout.
    pub fn DosRequestMutexSem(hmtx: u32, ulTimeout: ULONG) -> APIRET;

    /// Release a mutex semaphore owned by the calling thread.
    pub fn DosReleaseMutexSem(hmtx: u32) -> APIRET;
}

// ── VIOCALLS — extern "stdcall" (callee-cleanup), reversed arg order ─────────
//
// Real OS/2 prototype (Pascal / left-to-right push):
//   VioWrtTTY(pch, cb, hvio)  → stack: ...[pch][cb][hvio] ESP+4=hvio
//
// To reproduce this layout with stdcall (right-to-left push) we reverse the
// argument list in the declaration: declare(hvio, cb, pch) causes the compiler
// to push pch first (deepest), then cb, then hvio (shallowest = ESP+4). ✓

extern "stdcall" {
    /// Write a byte string to the VIO screen at the current cursor position.
    ///
    /// Real OS/2 prototype: `VioWrtTTY(pch, cb, hvio) -> APIRET`
    /// Declared reversed for Pascal/stdcall compatibility; pass `hvio = 0`
    /// for the default session.
    pub fn VioWrtTTY(hvio: HVIO, cb: ULONG, pch: *const u8) -> APIRET;

    /// Get the current cursor row and column.
    ///
    /// Real OS/2 prototype: `VioGetCurPos(pRow, pCol, hvio) -> APIRET`
    /// Declared reversed; `*pRow` and `*pCol` are filled on success.
    pub fn VioGetCurPos(hvio: HVIO, pCol: *mut USHORT, pRow: *mut USHORT) -> APIRET;

    /// Set the cursor to the specified row and column (0-based).
    ///
    /// Real OS/2 prototype: `VioSetCurPos(row, col, hvio) -> APIRET`
    /// Declared reversed; pass `hvio = 0` for the default session.
    pub fn VioSetCurPos(hvio: HVIO, col: USHORT, row: USHORT) -> APIRET;
}

// ── KBDCALLS — extern "stdcall" (callee-cleanup), reversed arg order ─────────

extern "stdcall" {
    /// Read one keystroke from the keyboard.
    ///
    /// Real OS/2 prototype: `KbdCharIn(pKeyInfo, wait, hkbd) -> APIRET`
    /// Declared reversed; `wait`: 0 = `IO_WAIT`, 1 = `IO_NOWAIT`.
    /// Pass `hkbd = 0` for the default session.
    pub fn KbdCharIn(hkbd: HKBD, wait: ULONG, pKeyInfo: *mut KBDKEYINFO) -> APIRET;

    /// Query the current keyboard session status.
    ///
    /// Real OS/2 prototype: `KbdGetStatus(pInfo, hkbd) -> APIRET`
    /// Declared reversed; `pInfo->cb` must be set to `size_of::<KBDINFO>()` before
    /// the call.  Pass `hkbd = 0` for the default session.
    pub fn KbdGetStatus(hkbd: HKBD, pInfo: *mut KBDINFO) -> APIRET;
}
