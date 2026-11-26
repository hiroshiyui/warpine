// SPDX-License-Identifier: GPL-3.0-or-later
//! INF / HLP help files (src/loader/ipf_file.rs, ADR-0132): parsing the
//! header and the table of contents never panics or reads past the data;
//! every entry's level fits its four bits and its cells fit its size.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/loader/ipf_file.rs"]
#[allow(dead_code)]
mod ipf_file;

fuzz_target!(|data: &[u8]| {
    let Ok(header) = ipf_file::parse_header(data) else { return };
    assert!(data.len() >= ipf_file::HEADER_SIZE);
    if let Ok(toc) = ipf_file::parse_toc(data, &header) {
        assert_eq!(toc.len(), header.toc_count as usize);
        for e in &toc {
            assert!(e.level <= 15);
            assert!(3 + 2 * e.cells.len() + e.title.len() <= 255, "an entry fits its size byte");
        }
    }
});
