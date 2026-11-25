// SPDX-License-Identifier: GPL-3.0-or-later
//! Workplace Shell setup strings (src/loader/wps_setup.rs, ADR-0130): a
//! program's WinCreateObject / WinSetObjectData strings, scanned for keys,
//! escaped and queued. The first byte splits the input into a key and a
//! setup string. Scanning never panics; an escaped value that does not end
//! in '^' scans back to itself and lets the next queued item be found (a
//! value ending in '^' cannot be followed by an item: '^;' is an escaped
//! semicolon); a title, whose trailing carets are dropped, always can.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/loader/wps_setup.rs"]
#[allow(dead_code)]
mod wps_setup;

fuzz_target!(|data: &[u8]| {
    let Some((&split, rest)) = data.split_first() else { return };
    let (key, text) = rest.split_at((split as usize).min(rest.len()));
    let _ = wps_setup::scan(text, key);
    // An escaped value scans back, under a key it cannot contain, and the
    // next queued item is still found after it.
    if !text.ends_with(b"^") {
        let mut setup = b"K=".to_vec();
        setup.extend(wps_setup::escape(text));
        assert_eq!(wps_setup::scan(&setup, b"K").as_deref(), Some(text), "an escaped value scans back");
        let queued = wps_setup::append(&setup, b"Z=1");
        assert_eq!(wps_setup::scan(&queued, b"Z").as_deref(), Some(&b"1"[..]), "the next item is found");
        // A key queued again: the later value wins.
        let again = wps_setup::append(&queued, b"K=later");
        assert_eq!(wps_setup::scan(&again, b"K").as_deref(), Some(&b"later"[..]), "the last occurrence wins");
    }
    // A title is always followed safely.
    let mut title = b"TITLE=".to_vec();
    title.extend(wps_setup::title_value(text));
    assert_eq!(wps_setup::scan(&wps_setup::append(&title, b"Z=1"), b"Z").as_deref(), Some(&b"1"[..]), "a title is followed");
    let _ = wps_setup::new_record(key);
});
