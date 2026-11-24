// SPDX-License-Identifier: GPL-3.0-or-later
//! The profile parser (src/loader/profile_text.rs): `Prf*` profiles are
//! files on drives a program can write, in Warpine's text format. A file
//! that parses must write back to text that parses to the same profile.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/loader/profile_text.rs"]
#[allow(dead_code)]
mod profile_text;

use profile_text::*;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_PROFILE_FILE { return; }
    if let Ok(p) = Profile::parse(data) {
        let text = p.serialize();
        assert_eq!(Profile::parse(&text).as_ref().ok(), Some(&p), "round trip");
    }
    let _ = parse_profile_int(data);
});
