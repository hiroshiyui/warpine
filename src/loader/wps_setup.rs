// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Workplace Shell setup strings and the user-profile names of the SDK's
//! `WPAbstract` (ADR-0129, ADR-0130), as the host's PMWP calls need them:
//! no `Loader`, so the fuzz target (`fuzz/fuzz_targets/wps_setup.rs`)
//! reads this file alone.
//!
//! - A setup string is `KEY=value;KEY2=value;…`; `^;` is a semicolon in a
//!   value; keys match without regard to case; a key's last occurrence
//!   wins (the rules of `libwps`'s `wpScanSetupString`,
//!   `sdk/wps/wpobject.c`).
//! - The profile names and the record header are those of
//!   `sdk/wps/wpsint.h`: records by handle (eight hex digits) in
//!   [`APP_OBJECTS`], object IDs mapped to handles (four bytes,
//!   little-endian) in [`APP_OBJECT_IDS`], setup strings queued for an
//!   object until a process awakens it in [`APP_PENDING`], the next handle
//!   in [`APP_HANDLES`] / [`KEY_NEXT`].

/// Records by handle.
pub const APP_OBJECTS: &[u8] = b"WrpWPS:Objects";
/// Object IDs mapped to handles.
pub const APP_OBJECT_IDS: &[u8] = b"WrpWPS:ObjectIDs";
/// Setup strings queued for an object, by handle.
pub const APP_PENDING: &[u8] = b"WrpWPS:Pending";
/// The next handle to give.
pub const APP_HANDLES: &[u8] = b"WrpWPS:Handles";
pub const KEY_NEXT: &[u8] = b"Next";
/// The first handle.
pub const FIRST_HANDLE: u32 = 0x0002_0001;
/// A record's magic ("WWPO") and version.
pub const RECORD_MAGIC: u32 = 0x4F50_5757;
pub const RECORD_VERSION: u32 = 1;
/// The longest class name a record keeps.
pub const MAX_CLASS_NAME: usize = 255;

/// The profile key of handle `h`.
pub fn handle_key(h: u32) -> String {
    format!("{h:08X}")
}

/// The value of `key` in setup string `setup` (unescaped; its last
/// occurrence); `None` when it is not there.
pub fn scan(setup: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    let mut found = None;
    let mut i = 0;
    while i < setup.len() {
        // One "NAME=value" item, up to an unescaped ';'.
        let start = i;
        let mut eq = None;
        while i < setup.len() && setup[i] != b';' {
            if setup[i] == b'^' && setup.get(i + 1) == Some(&b';') {
                i += 1;
            } else if setup[i] == b'=' && eq.is_none() {
                eq = Some(i);
            }
            i += 1;
        }
        if let Some(eq) = eq && setup[start..eq].eq_ignore_ascii_case(key) {
            let mut value = Vec::new();
            let mut j = eq + 1;
            while j < i {
                if setup[j] == b'^' && setup.get(j + 1) == Some(&b';') { j += 1; }
                value.push(setup[j]);
                j += 1;
            }
            found = Some(value);
        }
        i += 1;
    }
    found
}

/// `value` written for a setup string: each `;` as `^;`. A value that
/// ends in `^` cannot be followed by another item (the `;` after it would
/// read as `^;`): [`title_value`] drops such carets from a title.
pub fn escape(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    for &b in value {
        if b == b';' { out.push(b'^'); }
        out.push(b);
    }
    out
}

/// A title as `WinCreateObject` queues it: escaped, without carets at its
/// end, which would join it to the next item.
pub fn title_value(title: &[u8]) -> Vec<u8> {
    let end = title.iter().rposition(|&b| b != b'^').map_or(0, |i| i + 1);
    escape(&title[..end])
}

/// `more` queued after the setup string `queued`, a `;` between them
/// unless `queued` already ends an item (an escaped `^;` does not).
pub fn append(queued: &[u8], more: &[u8]) -> Vec<u8> {
    let mut out = queued.to_vec();
    let ends_item = out.ends_with(b";") && !out.ends_with(b"^;");
    if !out.is_empty() && !ends_item && !more.is_empty() { out.push(b';'); }
    out.extend_from_slice(more);
    out
}

/// The record of an object a process has not made yet: class `class`, no
/// saved values (so `libwps` makes it with `wpSetupOnce` when it awakens
/// it); `None` for a class name that is empty or too long.
pub fn new_record(class: &[u8]) -> Option<Vec<u8>> {
    if class.is_empty() || class.len() > MAX_CLASS_NAME || class.contains(&0) { return None; }
    let mut r = Vec::with_capacity(16 + class.len());
    for word in [RECORD_MAGIC, RECORD_VERSION, class.len() as u32] { r.extend(word.to_le_bytes()); }
    r.extend_from_slice(class);
    r.extend(0u32.to_le_bytes());
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_strings_scan_as_libwps_does() {
        let s = b"TITLE=Lunch;text=a^;b;OBJECTID=<WP_X>;EMPTY=;BROKEN";
        assert_eq!(scan(s, b"title").as_deref(), Some(&b"Lunch"[..]), "any case");
        assert_eq!(scan(s, b"TEXT").as_deref(), Some(&b"a;b"[..]), "^; a semicolon");
        assert_eq!(scan(s, b"OBJECTID").as_deref(), Some(&b"<WP_X>"[..]));
        assert_eq!(scan(s, b"EMPTY").as_deref(), Some(&b""[..]));
        assert_eq!(scan(s, b"BROKEN"), None, "no '='");
        assert_eq!(scan(s, b"TIT"), None, "a whole key only");
        assert_eq!(scan(b"", b"X"), None);
        assert_eq!(scan(b"A=x^", b"A").as_deref(), Some(&b"x^"[..]), "a '^' at the end stays");
        assert_eq!(scan(b"A=1;B=2;a=3", b"A").as_deref(), Some(&b"3"[..]), "the last occurrence wins");
    }

    #[test]
    fn escaped_values_scan_back() {
        for v in [&b"plain"[..], b"semi;colon", b"^;", b"^", b";;", b"a=b"] {
            let mut setup = b"K=".to_vec();
            setup.extend(escape(v));
            assert_eq!(scan(&setup, b"K").as_deref(), Some(v), "{:?}", String::from_utf8_lossy(v));
        }
    }

    #[test]
    fn queued_setups_append_and_records_start_empty() {
        assert_eq!(append(b"", b"A=1"), b"A=1");
        assert_eq!(append(b"A=1", b"B=2"), b"A=1;B=2");
        assert_eq!(append(b"A=1;", b"B=2"), b"A=1;B=2");
        assert_eq!(append(b"A=1", b""), b"A=1");
        // A value ending in an escaped semicolon is not an item's end
        // (found by the wps_setup fuzz target).
        assert_eq!(append(b"K=^;", b"Z=1"), b"K=^;;Z=1");
        assert_eq!(scan(&append(b"K=^;", b"Z=1"), b"Z").as_deref(), Some(&b"1"[..]));
        let r = new_record(b"Card").unwrap();
        assert_eq!(&r[..4], &RECORD_MAGIC.to_le_bytes());
        assert_eq!(&r[12..16], b"Card");
        assert_eq!(&r[16..], &0u32.to_le_bytes(), "no saved values");
        assert!(new_record(b"").is_none() && new_record(&[b'x'; 256]).is_none() && new_record(b"a\0b").is_none());
        assert_eq!(handle_key(0x0002_0001), "00020001");
        // A title's carets at its end are dropped, or the next item would
        // join it (found by the fuzz target).
        assert_eq!(title_value(b"a;b^^"), b"a^;b");
        let mut queued = b"TITLE=".to_vec();
        queued.extend(title_value(b"x^"));
        assert_eq!(scan(&append(&queued, b"Z=1"), b"Z").as_deref(), Some(&b"1"[..]));
    }
}
