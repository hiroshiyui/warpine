// SPDX-License-Identifier: GPL-3.0-or-later
//! The EA lists programs pass (src/loader/ea_list.rs): FEA2LISTs to
//! DosSetPathInfo / DosSetFileInfo level 2 and GEA2LISTs to level 3 of the
//! queries. Parsing never panics, every entry it gives lies inside cbList
//! with a non-empty name without NULs, and an error names an offset inside
//! the list. What parses builds back into an FEA2LIST that parses to the
//! same entries and states its own length, and DENA2 entries never take
//! more room than they are given.
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
            let built = ea_list::build_fea2_list(&eas);
            assert_eq!(u32::from_le_bytes([built[0], built[1], built[2], built[3]]) as usize, built.len());
            if built.len() <= ea_list::MAX_FEA2_LIST {
                assert_eq!(ea_list::parse_fea2_list(&built), Ok(eas.clone()), "a built list parses back");
            }
            let room = data.len() / 2;
            let (dena, n) = ea_list::build_dena2(&eas, room);
            assert!(dena.len() <= room && n <= eas.len());
        }
        Err(offset) => assert!(offset == 0 || (offset as usize) < data.len()),
    }
    match ea_list::parse_gea2_list(data) {
        Ok(names) => {
            let cb_list = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
            assert!(names.len() <= ea_list::MAX_FEA2_ENTRIES);
            let bytes: usize = names.iter().map(|n| 5 + n.len() + 1).sum();
            assert!(bytes + 4 <= cb_list || names.is_empty(), "the names fit in cbList");
            assert!(names.iter().all(|n| !n.is_empty() && !n.contains(&0)));
        }
        Err(offset) => assert!(offset == 0 || (offset as usize) < data.len()),
    }
});
