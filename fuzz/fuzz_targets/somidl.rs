// SPDX-License-Identifier: GPL-3.0-or-later
//! The SOM IDL compiler's front end (ADR-0122): an IDL file may come from
//! anywhere a program's sources do. It is preprocessed and parsed in
//! memory, reading no file; nesting, include depth and size are bounded.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/cpp.rs"]
#[allow(dead_code)]
mod cpp;
#[path = "../../src/somidl/mod.rs"]
#[allow(dead_code)]
mod somidl;

fuzz_target!(|data: &[u8]| {
    if let Ok(defs) = somidl::parse_in_memory(data) {
        let _ = somidl::dump::dump(&defs);
    }
});
