// SPDX-License-Identifier: GPL-3.0-or-later
//! Message files (ADR-0105): a file DosGetMessage reads, a bound segment
//! in a program, and the text os2msg compiles. A file that parses must
//! write back to bytes that parse to the same messages.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/loader/msgfile.rs"]
#[allow(dead_code)]
mod msgfile;

fuzz_target!(|data: &[u8]| {
    if let Ok(file) = msgfile::parse(data) {
        let bytes = msgfile::write(&file).expect("a parsed file writes back");
        assert_eq!(msgfile::parse(&bytes).ok().as_ref(), Some(&file), "a message file round-trips");
    }
    let _ = msgfile::segment_entries(data);
    let _ = msgfile::compile(data, 850);
});
