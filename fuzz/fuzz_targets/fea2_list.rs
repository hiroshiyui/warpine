// SPDX-License-Identifier: GPL-3.0-or-later
//! FEA2LISTs a program passes to DosSetPathInfo level 2 (src/loader/
//! ea_list.rs): parsing never panics, every entry it gives lies inside
//! cbList with a non-empty name without NULs, and an error names an offset
//! inside the list.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/loader/ea_list.rs"]
#[allow(dead_code)]
mod ea_list;

fuzz_target!(|data: &[u8]| {
    match ea_list::parse_fea2_list(data) {
        Ok(eas) => {
            let cb_list = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
            assert!(eas.len() <= ea_list::MAX_FEA2_ENTRIES);
            let bytes: usize = eas.iter().map(|e| 8 + e.name.len() + 1 + e.value.len()).sum();
            assert!(bytes + 4 <= cb_list || eas.is_empty(), "the entries fit in cbList");
            assert!(eas.iter().all(|e| !e.name.is_empty() && !e.name.contains(&0) && e.value.len() <= 0xFFFF));
        }
        Err(offset) => assert!(offset == 0 || (offset as usize) < data.len()),
    }
});
