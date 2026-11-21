// SPDX-License-Identifier: GPL-3.0-or-later
//! The NE parser: a 16-bit program file is untrusted input.
#![no_main]

use libfuzzer_sys::fuzz_target;

// Through a module whose path is Warpine's src/: ne.rs's own `mod
// header;` then resolves to src/ne/header.rs, and its `crate::ne`
// paths to the re-export below.
#[path = "../../src"]
#[allow(dead_code)]
mod src {
    pub mod ne;
}
use src::ne;

fuzz_target!(|data: &[u8]| {
    let _ = ne::NeFile::parse(std::io::Cursor::new(data));
});
