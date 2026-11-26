// SPDX-License-Identifier: GPL-3.0-or-later
//
//! The lists of extended attributes programs pass and get (Control Program
//! Reference; layouts of the toolkit's `<bsedos.h>`), with no `Loader`: the
//! fuzz target (`fuzz/fuzz_targets/fea2_list.rs`) reads this file alone.
//!
//! - An `FEA2LIST` (`DosSetPathInfo` / `DosSetFileInfo` level 2, and what
//!   level 3 of the queries returns): `cbList`, then `FEA2` entries —
//!   `oNextEntryOffset`, `fEA`, `cbName`, `cbValue`, the name and its NUL,
//!   the value — each next entry `oNextEntryOffset` bytes on, the last
//!   with 0.
//! - A `GEA2LIST` (the names level 3 of `DosQueryPathInfo` /
//!   `DosQueryFileInfo` asks for): `cbList`, then `GEA2` entries —
//!   `oNextEntryOffset`, `cbName`, the name and its NUL.
//! - `DosEnumAttribute`'s `DENA2` entries are `FEA2`s without the values.
//!
//! Entries Warpine writes are padded to four bytes (unverified: the
//! Reference asks for aligned entries without giving the alignment).

/// The largest list taken (an EA list of OS/2 is at most 64 KB).
pub const MAX_FEA2_LIST: usize = 64 * 1024;
/// The most entries in one list.
pub const MAX_FEA2_ENTRIES: usize = 1024;
/// An entry's fixed part.
const FEA2_HEADER: usize = 8;
/// A `GEA2`'s fixed part.
const GEA2_HEADER: usize = 5;
/// Entries Warpine writes start on this boundary.
const ENTRY_ALIGN: usize = 4;

/// One extended attribute to set: an empty value deletes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fea2 {
    pub flags: u8,
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

/// The entries of the `FEA2LIST` in `list` (its `cbList` bytes and no
/// more); `Err(offset)` names the entry, from the list's start, that is
/// not well formed (`EAOP2.oError`).
pub fn parse_fea2_list(list: &[u8]) -> Result<Vec<Fea2>, u32> {
    let word = |at: usize| list.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let cb_list = word(0).ok_or(0u32)? as usize;
    if cb_list < 4 || cb_list > list.len() || cb_list > MAX_FEA2_LIST { return Err(0); }
    let list = &list[..cb_list];
    let mut out = Vec::new();
    let mut at = 4;
    while at < cb_list {
        let bad = Err(at as u32);
        if out.len() >= MAX_FEA2_ENTRIES || at + FEA2_HEADER > cb_list { return bad; }
        let next = word(at).unwrap_or(0) as usize;
        let flags = list[at + 4];
        let cb_name = list[at + 5] as usize;
        let cb_value = u16::from_le_bytes([list[at + 6], list[at + 7]]) as usize;
        let name_at = at + FEA2_HEADER;
        let value_at = name_at + cb_name + 1;
        let end = value_at + cb_value;
        if cb_name == 0 || end > cb_list || list[name_at + cb_name] != 0 { return bad; }
        let name = &list[name_at..name_at + cb_name];
        if name.contains(&0) { return bad; }
        out.push(Fea2 { flags, name: name.to_vec(), value: list[value_at..end].to_vec() });
        if next == 0 { break; }
        // The next entry starts past this one's data.
        if next < end - at { return bad; }
        at += next;
    }
    Ok(out)
}

/// The names the `GEA2LIST` in `list` asks for (its `cbList` bytes and no
/// more); `Err(offset)` names the entry, from the list's start, that is not
/// well formed (`EAOP2.oError`).
pub fn parse_gea2_list(list: &[u8]) -> Result<Vec<Vec<u8>>, u32> {
    let word = |at: usize| list.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let cb_list = word(0).ok_or(0u32)? as usize;
    if cb_list < 4 || cb_list > list.len() || cb_list > MAX_FEA2_LIST { return Err(0); }
    let list = &list[..cb_list];
    let mut out = Vec::new();
    let mut at = 4;
    while at < cb_list {
        let bad = Err(at as u32);
        if out.len() >= MAX_FEA2_ENTRIES || at + GEA2_HEADER > cb_list { return bad; }
        let next = word(at).unwrap_or(0) as usize;
        let cb_name = list[at + 4] as usize;
        let name_at = at + GEA2_HEADER;
        let end = name_at + cb_name + 1;
        if cb_name == 0 || end > cb_list || list[name_at + cb_name] != 0 { return bad; }
        let name = &list[name_at..name_at + cb_name];
        if name.contains(&0) { return bad; }
        out.push(name.to_vec());
        if next == 0 { break; }
        if next < end - at { return bad; }
        at += next;
    }
    Ok(out)
}

/// One `FEA2` entry, padded, its `oNextEntryOffset` 0; without the value
/// (but with its `cbValue`) for a `DENA2`. A name is cut at 255 bytes and
/// a value at 65535.
fn entry(ea: &Fea2, with_value: bool) -> Vec<u8> {
    let name = &ea.name[..ea.name.len().min(u8::MAX as usize)];
    let value = &ea.value[..ea.value.len().min(u16::MAX as usize)];
    let mut e = vec![0u8; 4];
    e.push(ea.flags);
    e.push(name.len() as u8);
    e.extend((value.len() as u16).to_le_bytes());
    e.extend_from_slice(name);
    e.push(0);
    if with_value { e.extend_from_slice(value); }
    e.resize(e.len().next_multiple_of(ENTRY_ALIGN), 0);
    e
}

/// Entries one after another, each `oNextEntryOffset` naming the next.
fn link(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let at = out.len();
        out.extend_from_slice(e);
        if i + 1 < entries.len() {
            out[at..at + 4].copy_from_slice(&(e.len() as u32).to_le_bytes());
        }
    }
    out
}

/// The `FEA2LIST` of `eas`, `cbList` its whole length.
pub fn build_fea2_list(eas: &[Fea2]) -> Vec<u8> {
    let body = link(&eas.iter().map(|ea| entry(ea, true)).collect::<Vec<_>>());
    let mut out = ((body.len() + 4) as u32).to_le_bytes().to_vec();
    out.extend(body);
    out
}

/// The `DENA2` entries of `eas` that fit in `room` bytes, from the first:
/// the bytes and how many there are.
pub fn build_dena2(eas: &[Fea2], room: usize) -> (Vec<u8>, usize) {
    let mut entries = Vec::new();
    let mut used = 0;
    for ea in eas {
        let e = entry(ea, false);
        if used + e.len() > room { break; }
        used += e.len();
        entries.push(e);
    }
    (link(&entries), entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An FEA2LIST of these entries, each padded to four bytes.
    fn list(entries: &[(u8, &[u8], &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (i, (flags, name, value)) in entries.iter().enumerate() {
            let mut e = vec![0u8; 4];
            e.push(*flags);
            e.push(name.len() as u8);
            e.extend((value.len() as u16).to_le_bytes());
            e.extend_from_slice(name);
            e.push(0);
            e.extend_from_slice(value);
            while !e.len().is_multiple_of(4) { e.push(0); }
            if i + 1 < entries.len() { let n = e.len() as u32; e[..4].copy_from_slice(&n.to_le_bytes()); }
            body.extend(e);
        }
        let mut out = ((body.len() + 4) as u32).to_le_bytes().to_vec();
        out.extend(body);
        out
    }

    #[test]
    fn lists_parse_entry_by_entry() {
        let l = list(&[(0, b".LONGNAME", b"\xFD\xFF\x05\x00Hello"), (0x80, b"MINE", b""), (0, b"X", b"1")]);
        let eas = parse_fea2_list(&l).unwrap();
        assert_eq!(eas.len(), 3);
        assert_eq!(eas[0], Fea2 { flags: 0, name: b".LONGNAME".to_vec(), value: b"\xFD\xFF\x05\x00Hello".to_vec() });
        assert_eq!((eas[1].flags, eas[1].value.len()), (0x80, 0), "an empty value deletes");
        assert_eq!(eas[2].name, b"X");
        // cbList bounds what is read: bytes past it are not entries.
        let mut longer = l.clone();
        longer.extend([0xFFu8; 16]);
        assert_eq!(parse_fea2_list(&longer).unwrap().len(), 3);
        assert_eq!(parse_fea2_list(&list(&[])).unwrap(), Vec::new(), "an empty list");
    }

    /// A GEA2LIST of these names, each padded to four bytes.
    fn gea_list(names: &[&[u8]]) -> Vec<u8> {
        let mut body = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let mut e = vec![0u8; 4];
            e.push(name.len() as u8);
            e.extend_from_slice(name);
            e.push(0);
            while !e.len().is_multiple_of(4) { e.push(0); }
            if i + 1 < names.len() { let n = e.len() as u32; e[..4].copy_from_slice(&n.to_le_bytes()); }
            body.extend(e);
        }
        let mut out = ((body.len() + 4) as u32).to_le_bytes().to_vec();
        out.extend(body);
        out
    }

    #[test]
    fn name_lists_parse_and_name_their_bad_entry() {
        let l = gea_list(&[b".LONGNAME", b"X"]);
        assert_eq!(parse_gea2_list(&l), Ok(vec![b".LONGNAME".to_vec(), b"X".to_vec()]));
        assert_eq!(parse_gea2_list(&gea_list(&[])), Ok(Vec::new()));
        assert_eq!(parse_gea2_list(&[3, 0, 0, 0]), Err(0));
        let mut l2 = l.clone();
        l2[4 + 4] = 0;
        assert_eq!(parse_gea2_list(&l2), Err(4), "an empty name");
        let mut l2 = l.clone();
        l2[4 + 5 + 9] = b'!';
        assert_eq!(parse_gea2_list(&l2), Err(4), "no NUL");
        let mut l2 = l.clone();
        l2[4..8].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(parse_gea2_list(&l2), Err(4), "a next offset inside the entry");
        let second = u32::from_le_bytes(l[4..8].try_into().unwrap()) as usize + 4;
        let mut l2 = l;
        l2[second + 4] = 200;
        assert_eq!(parse_gea2_list(&l2), Err(second as u32), "a name past cbList");
    }

    #[test]
    fn built_lists_parse_back() {
        let eas = vec![
            Fea2 { flags: 0x80, name: b"ONE".to_vec(), value: b"\xFD\xFF\x01\x00a".to_vec() },
            Fea2 { flags: 0, name: b"TWO".to_vec(), value: Vec::new() },
        ];
        let l = build_fea2_list(&eas);
        assert_eq!(u32::from_le_bytes(l[..4].try_into().unwrap()) as usize, l.len());
        assert_eq!(parse_fea2_list(&l), Ok(eas.clone()));
        assert_eq!(l, list(&[(0x80, b"ONE", b"\xFD\xFF\x01\x00a"), (0, b"TWO", b"")]), "the layout of the toolkit");
        assert_eq!(build_fea2_list(&[]), vec![4, 0, 0, 0]);
        // DENA2: no values, but their sizes; as many as fit.
        let (d, n) = build_dena2(&eas, 1000);
        assert_eq!(n, 2);
        assert_eq!(&d[4..8], &[0x80, 3, 5, 0], "fEA, cbName, cbValue");
        assert_eq!(&d[8..12], b"ONE\0");
        assert_eq!(u32::from_le_bytes(d[..4].try_into().unwrap()), 12);
        assert_eq!(u32::from_le_bytes(d[12..16].try_into().unwrap()), 0, "the last links nowhere");
        let (one, n) = build_dena2(&eas, 15);
        assert_eq!(n, 1, "the second does not fit");
        assert_eq!(&one[..4], &[0; 4], "the only one links nowhere");
        assert_eq!(&one[4..], &d[4..12]);
        assert_eq!(build_dena2(&eas, 11).1, 0);
    }

    #[test]
    fn malformed_lists_name_the_entry() {
        assert_eq!(parse_fea2_list(&[]), Err(0));
        assert_eq!(parse_fea2_list(&[2, 0, 0, 0]), Err(0), "cbList under 4");
        assert_eq!(parse_fea2_list(&[64, 0, 0, 0, 0]), Err(0), "cbList past the data");
        let good = list(&[(0, b"A", b"v"), (0, b"B", b"w")]);
        let second = u32::from_le_bytes(good[4..8].try_into().unwrap()) + 4;
        // No NUL after the name.
        let mut l = good.clone();
        l[4 + 8 + 1] = b'x';
        assert_eq!(parse_fea2_list(&l), Err(4));
        // A value past cbList.
        let mut l = good.clone();
        l[second as usize + 6] = 0xFF;
        assert_eq!(parse_fea2_list(&l), Err(second));
        // An empty name.
        let mut l = good.clone();
        l[4 + 5] = 0;
        assert_eq!(parse_fea2_list(&l), Err(4));
        // A next offset inside the entry.
        let mut l = good;
        l[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(parse_fea2_list(&l), Err(4));
    }
}
