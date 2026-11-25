// SPDX-License-Identifier: GPL-3.0-or-later
//
//! An `FEA2LIST` a program passes to set extended attributes
//! (`DosSetPathInfo` level 2, `FIL_QUERYEASIZE`; Control Program
//! Reference): `cbList`, then `FEA2` entries — `oNextEntryOffset`, `fEA`,
//! `cbName`, `cbValue`, the name and its NUL, the value — each next entry
//! `oNextEntryOffset` bytes on, the last with 0. No `Loader`: the fuzz
//! target (`fuzz/fuzz_targets/fea2_list.rs`) reads this file alone.

/// The largest list taken (an EA list of OS/2 is at most 64 KB).
pub const MAX_FEA2_LIST: usize = 64 * 1024;
/// The most entries in one list.
pub const MAX_FEA2_ENTRIES: usize = 1024;
/// An entry's fixed part.
const FEA2_HEADER: usize = 8;

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
