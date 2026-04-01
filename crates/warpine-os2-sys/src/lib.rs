//! Raw OS/2 API bindings for Warpine guest programs.
//!
//! OS/2 `_System` calling convention = `extern "C"` (caller-cleanup, cdecl-like).
//! These declarations map directly to the thunk stubs resolved by `lx-link`
//! via `targets/os2api.def`.

#![no_std]

// ── OS/2 type aliases ────────────────────────────────────────────────────────

/// Standard OS/2 API return code (0 = success).
pub type APIRET = u32;

/// Unsigned 32-bit integer.
pub type ULONG = u32;

/// File handle.
pub type HFILE = u32;

/// Pointer to void (opaque byte pointer).
pub type PVOID = *mut u8;

/// Pointer to a constant NUL-terminated string.
pub type PCSZ = *const u8;

// ── OS/2 API declarations ────────────────────────────────────────────────────

extern "C" {
    /// Terminate the current process or thread.
    ///
    /// `ulTermType`: 0 = current thread, 1 = entire process.
    /// Never returns.
    pub fn DosExit(ulTermType: ULONG, ulResultCode: ULONG) -> !;

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

    /// Allocate a block of memory.
    ///
    /// `flag` bit mask: `0x01` PAG_READ, `0x02` PAG_WRITE, `0x10` PAG_COMMIT.
    /// Warpine ignores the `flag` argument — pass `0x13` (READ|WRITE|COMMIT)
    /// for conventional usage.
    pub fn DosAllocMem(ppb: *mut PVOID, cb: ULONG, flag: ULONG) -> APIRET;

    /// Free a block previously allocated with `DosAllocMem`.
    pub fn DosFreeMem(pb: PVOID) -> APIRET;

    /// Change the access/commit attributes of an allocated memory region.
    pub fn DosSetMem(pb: PVOID, cb: ULONG, flag: ULONG) -> APIRET;
}
