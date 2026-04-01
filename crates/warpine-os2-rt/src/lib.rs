//! Warpine OS/2 runtime support crate.
//!
//! Provides:
//! - `_start` entry point that calls the guest's `os2_main() -> u32`
//! - Panic handler that calls `DosExit(1, 1)`
//! - Global allocator backed by `DosAllocMem` / `DosFreeMem`
//!
//! Guest binaries must define:
//! ```rust,ignore
//! #[no_mangle]
//! pub extern "C" fn os2_main() -> u32 { ... }
//! ```

#![no_std]

use core::alloc::{GlobalAlloc, Layout};

use warpine_os2_sys as sys;

// ── Entry point ──────────────────────────────────────────────────────────────

extern "C" {
    /// The guest application entry point — must be defined by the binary crate.
    fn os2_main() -> u32;
}

/// LX entry point.  Called by the OS/2 loader after all DLL INITTERMs complete.
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let code = os2_main();
    // ulTermType = 1 → terminate entire process
    sys::DosExit(1, code);
}

// ── Panic handler ────────────────────────────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // Exit with code 1 on panic; we cannot print without allocation.
    unsafe { sys::DosExit(1, 1) }
}

// ── Global allocator ─────────────────────────────────────────────────────────

struct Os2Allocator;

unsafe impl GlobalAlloc for Os2Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut ptr: *mut u8 = core::ptr::null_mut();
        let rc = sys::DosAllocMem(
            &mut ptr as *mut *mut u8 as *mut sys::PVOID,
            layout.size() as u32,
            0x13, // PAG_READ | PAG_WRITE | PAG_COMMIT
        );
        if rc != 0 {
            core::ptr::null_mut()
        } else {
            ptr
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        sys::DosFreeMem(ptr);
    }
}

#[global_allocator]
static ALLOCATOR: Os2Allocator = Os2Allocator;
