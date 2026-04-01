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
