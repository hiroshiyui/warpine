//! Hello World sample — Rust guest binary for Warpine OS/2.
//!
//! Build with:
//!   cargo +nightly build \
//!     -Z build-std=core,alloc \
//!     -Z build-std-features=compiler-builtins-mem \
//!     -Z json-target-spec \
//!     --target ../../targets/i686-warpine-os2.json
//!
//! Run with:
//!   cargo run --manifest-path ../../Cargo.toml -- \
//!     samples/rust_hello/target/i686-warpine-os2/debug/rust_hello.exe

#![no_std]
#![no_main]

// Pull in the runtime crate to provide `_start`, the panic handler, and the
// global allocator — even though we don't call anything from it directly.
extern crate warpine_os2_rt;

use warpine_os2 as os2;

/// Guest entry point — called by `warpine-os2-rt::_start`.
#[no_mangle]
pub extern "C" fn os2_main() -> u32 {
    if os2::file::write_stdout(b"Hello from Rust on Warpine!\r\n").is_err() {
        return 1;
    }
    os2::file::write_stdout(b"Press any key to exit...\r\n")
        .ok();
    os2::kbd::getchar();
    0
}
