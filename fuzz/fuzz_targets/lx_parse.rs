// SPDX-License-Identifier: GPL-3.0-or-later
//! The LX parser: a program or DLL file is untrusted input.
#![no_main]

use libfuzzer_sys::fuzz_target;

// Through a module whose path is Warpine's src/: lx.rs's own `mod
// header;` then resolves to src/lx/header.rs, and its `crate::lx`
// paths to the re-export below.
#[path = "../../src"]
#[allow(dead_code)]
mod src {
    pub mod lx;
}
use src::lx;

fuzz_target!(|data: &[u8]| {
    let _ = lx::LxFile::parse(std::io::Cursor::new(data));
});
