//! High-level OS/2 API wrappers for Warpine guest programs.
//!
//! Provides ergonomic Rust interfaces over the raw `warpine-os2-sys` bindings.

#![no_std]

use warpine_os2_sys as sys;

// ── File / I/O ───────────────────────────────────────────────────────────────

pub mod file {
    use super::sys;

    /// Write `buf` to the standard output (handle 1).
    ///
    /// Returns `Ok(bytes_written)` or `Err(rc)` on failure.
    pub fn write_stdout(buf: &[u8]) -> Result<usize, u32> {
        write_handle(1, buf)
    }

    /// Write `buf` to the standard error output (handle 2).
    ///
    /// Returns `Ok(bytes_written)` or `Err(rc)` on failure.
    pub fn write_stderr(buf: &[u8]) -> Result<usize, u32> {
        write_handle(2, buf)
    }

    /// Internal helper: write `buf` to any open file handle.
    fn write_handle(hfile: sys::HFILE, buf: &[u8]) -> Result<usize, u32> {
        let mut written: sys::ULONG = 0;
        let rc = unsafe {
            sys::DosWrite(
                hfile,
                buf.as_ptr(),
                buf.len() as sys::ULONG,
                &mut written,
            )
        };
        if rc == 0 {
            Ok(written as usize)
        } else {
            Err(rc)
        }
    }
}

// ── Memory ───────────────────────────────────────────────────────────────────

pub mod memory {
    use super::sys;

    // PAG_READ | PAG_WRITE | PAG_COMMIT
    const PAG_RWC: sys::ULONG = 0x13;

    /// Allocate `size` bytes of committed, read-write memory.
    ///
    /// Returns `Ok(ptr)` or `Err(rc)` on failure.
    pub fn alloc(size: usize) -> Result<*mut u8, u32> {
        let mut ptr: *mut u8 = core::ptr::null_mut();
        let rc = unsafe {
            sys::DosAllocMem(
                &mut ptr as *mut *mut u8 as *mut sys::PVOID,
                size as sys::ULONG,
                PAG_RWC,
            )
        };
        if rc == 0 {
            Ok(ptr)
        } else {
            Err(rc)
        }
    }

    /// Free a block previously returned by [`alloc`].
    ///
    /// Returns `Ok(())` or `Err(rc)` on failure.
    pub fn free(ptr: *mut u8) -> Result<(), u32> {
        let rc = unsafe { sys::DosFreeMem(ptr) };
        if rc == 0 { Ok(()) } else { Err(rc) }
    }

    /// Change the access/commit attributes of a memory region.
    ///
    /// `flag` bit mask: `0x01` PAG_READ, `0x02` PAG_WRITE, `0x10` PAG_COMMIT,
    /// `0x08` PAG_DECOMMIT, `0x04` PAG_GUARD.
    pub fn set_mem(ptr: *mut u8, size: usize, flag: u32) -> Result<(), u32> {
        let rc = unsafe { sys::DosSetMem(ptr, size as sys::ULONG, flag) };
        if rc == 0 { Ok(()) } else { Err(rc) }
    }
}

// ── Process ──────────────────────────────────────────────────────────────────

pub mod process {
    use super::sys;

    /// Terminate the current process with the given exit code.
    ///
    /// Never returns.
    pub fn exit(code: u32) -> ! {
        // ulTermType = 1 → terminate entire process
        unsafe { sys::DosExit(1, code) }
    }
}

// ── Thread ───────────────────────────────────────────────────────────────────

pub mod thread {
    use super::sys;

    /// Suspend execution for `ms` milliseconds.
    ///
    /// Returns the OS/2 error code (0 = success).
    pub fn sleep(ms: u32) -> u32 {
        unsafe { sys::DosSleep(ms) }
    }

    /// Create a new thread running `func(param)`.
    ///
    /// `stack_size`: requested stack size in bytes; 0 uses the system default.
    /// Returns `Ok(tid)` or `Err(rc)` on failure.
    pub fn create(
        func: unsafe extern "C" fn(u32),
        param: u32,
        stack_size: u32,
    ) -> Result<u32, u32> {
        let mut tid: sys::ULONG = 0;
        let rc = unsafe {
            sys::DosCreateThread(
                &mut tid,
                func,
                param,
                0, // DCWW_WAIT — run immediately
                stack_size,
            )
        };
        if rc == 0 { Ok(tid) } else { Err(rc) }
    }

    /// Wait for thread `tid` to terminate.
    ///
    /// Returns `Ok(tid_that_exited)` or `Err(rc)` on failure.
    pub fn wait(tid: u32) -> Result<u32, u32> {
        let mut out_tid: sys::ULONG = tid;
        let rc = unsafe { sys::DosWaitThread(&mut out_tid, 0) }; // DCWW_WAIT
        if rc == 0 { Ok(out_tid) } else { Err(rc) }
    }

    /// Forcibly terminate thread `tid`.
    ///
    /// Returns the OS/2 error code (0 = success).
    pub fn kill(tid: u32) -> u32 {
        unsafe { sys::DosKillThread(tid) }
    }
}

// ── VIO (Video I/O) ──────────────────────────────────────────────────────────

pub mod vio {
    use super::sys;

    /// Write a byte slice to the VIO screen at the current cursor position.
    ///
    /// Returns the OS/2 error code (0 = success).
    pub fn write_tty(s: &[u8]) -> u32 {
        unsafe { sys::VioWrtTTY(0, s.len() as sys::ULONG, s.as_ptr()) }
    }

    /// Get the current cursor position.
    ///
    /// Returns `Ok((row, col))` or `Err(rc)` on failure.
    pub fn get_cur_pos() -> Result<(u16, u16), u32> {
        let mut row: sys::USHORT = 0;
        let mut col: sys::USHORT = 0;
        let rc = unsafe { sys::VioGetCurPos(0, &mut col, &mut row) };
        if rc == 0 { Ok((row, col)) } else { Err(rc) }
    }

    /// Set the cursor to position (`row`, `col`), both 0-based.
    ///
    /// Returns the OS/2 error code (0 = success).
    pub fn set_cur_pos(row: u16, col: u16) -> u32 {
        unsafe { sys::VioSetCurPos(0, col, row) }
    }
}
