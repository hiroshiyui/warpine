// SPDX-License-Identifier: GPL-3.0-or-later
//! The resource compiler (ADR-0099): a resource script, and the headers
//! it includes, may come from anywhere a program's sources do. The script
//! is compiled in memory to a .res file, reading no file.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/bin/os2rc.rs"]
#[allow(dead_code)]
mod os2rc;

fuzz_target!(|data: &[u8]| {
    let _ = os2rc::compile_in_memory(data);
});
