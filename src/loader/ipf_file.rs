// SPDX-License-Identifier: GPL-3.0-or-later
//
//! INF / HLP help files (ADR-0132): the header and the table of contents.
//!
//! The layout is the one Open Watcom's IPF compiler `wipfc` writes (its
//! `header.hpp`, `toc.hpp`, `page.cpp`), the public specification ADR-0132
//! takes; all values are little-endian and packed.
//!
//! - The header (155 bytes): `"HSP"`, flags (`0x01` an INF book, `0x10` an
//!   HLP library), its size, the version, then the counts and file offsets
//!   of the tables (TOC, panel numbers and names, index, full-text search,
//!   cells, dictionary, images, NLS, extended header) and a 48-byte title.
//! - The TOC: `tocCount` entries, each found through the `u32` file offset
//!   at `tocOffsetOffset + 4 * i`. An entry is its size, flags (nesting
//!   level in the low four bits, then extended, hidden, has-children), the
//!   number of cells its text occupies, then — when extended — a flags word
//!   and the optional origin (5 bytes), size (5), style (2), group (2) and
//!   control (2) it announces, then a `u16` cell index per cell, then the
//!   title, to the end of the entry.
//!
//! No `Loader`: the fuzz target (`fuzz/fuzz_targets/ipf_file.rs`) reads this
//! file alone. Every offset and size comes from the file and is checked
//! against it; nothing is read past the data.

/// The header's size.
pub const HEADER_SIZE: usize = 155;
const MAGIC: &[u8; 3] = b"HSP";
const TITLE_AT: usize = HEADER_SIZE - TITLE_SIZE;
const TITLE_SIZE: usize = 48;
/// The header's flags.
const FLAG_INF: u8 = 0x01;
const FLAG_HLP: u8 = 0x10;
/// A TOC entry's fixed part, and its flags.
const TOC_FIXED: usize = 3;
const TOC_LEVEL: u8 = 0x0F;
const TOC_EXTENDED: u8 = 0x20;
const TOC_HIDDEN: u8 = 0x40;
const TOC_HAS_CHILDREN: u8 = 0x80;
/// An extended entry's flags word, and the parts it announces with their
/// sizes, in the order they follow it.
const EXT_SET_POS: u16 = 0x0001;
const EXT_SET_SIZE: u16 = 0x0002;
const EXT_SET_STYLE: u16 = 0x0008;
const EXT_SET_CTRL: u16 = 0x0040;
const EXT_SET_GROUP: u16 = 0x0400;
const EXT_PARTS: [(u16, usize); 5] = [(EXT_SET_POS, 5), (EXT_SET_SIZE, 5), (EXT_SET_STYLE, 2),
                                      (EXT_SET_GROUP, 2), (EXT_SET_CTRL, 2)];

/// Why a file is not one this reader takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpfError {
    /// Shorter than its header, or without `"HSP"`.
    NotIpf,
    /// A table or entry lies outside the file, or does not fit its size.
    Malformed(&'static str),
}

/// What the header says (the parts read so far).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpfHeader {
    /// An HLP help library (else an INF book).
    pub help_library: bool,
    pub version: (u8, u8),
    pub toc_count: u16,
    pub toc_offset_offset: u32,
    pub cell_count: u16,
    pub dict_count: u16,
    /// The database title, its bytes up to the first NUL (codepage text).
    pub title: Vec<u8>,
}

/// One table-of-contents entry: a panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    /// Its heading level, 1 for `:h1.`.
    pub level: u8,
    pub hidden: bool,
    pub has_children: bool,
    pub extended: bool,
    /// The cells holding its text.
    pub cells: Vec<u16>,
    /// Its title (codepage text).
    pub title: Vec<u8>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|x| u16::from_le_bytes([x[0], x[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

/// The header of the INF / HLP file in `data`.
pub fn parse_header(data: &[u8]) -> Result<IpfHeader, IpfError> {
    if data.len() < HEADER_SIZE || &data[..3] != MAGIC {
        return Err(IpfError::NotIpf);
    }
    let flags = data[3];
    if flags & (FLAG_INF | FLAG_HLP) == 0 {
        return Err(IpfError::NotIpf);
    }
    let hdrsize = u16_at(data, 4).unwrap_or(0) as usize;
    if hdrsize < HEADER_SIZE {
        return Err(IpfError::Malformed("header size"));
    }
    let title = &data[TITLE_AT..TITLE_AT + TITLE_SIZE];
    let title = title[..title.iter().position(|&c| c == 0).unwrap_or(TITLE_SIZE)].to_vec();
    // Offsets of the header's fields (header.hpp's order).
    Ok(IpfHeader {
        help_library: flags & FLAG_HLP != 0,
        version: (data[6], data[7]),
        toc_count: u16_at(data, 8).unwrap_or(0),
        toc_offset_offset: u32_at(data, 18).unwrap_or(0),
        cell_count: u16_at(data, 62).unwrap_or(0),
        dict_count: u16_at(data, 72).unwrap_or(0),
        title,
    })
}

/// The table of contents of the file in `data`, whose header is `header`.
pub fn parse_toc(data: &[u8], header: &IpfHeader) -> Result<Vec<TocEntry>, IpfError> {
    let count = header.toc_count as usize;
    let table = header.toc_offset_offset as usize;
    if table.checked_add(4 * count).is_none_or(|end| end > data.len()) {
        return Err(IpfError::Malformed("TOC offset table"));
    }
    (0..count).map(|i| {
        let at = u32_at(data, table + 4 * i).ok_or(IpfError::Malformed("TOC offset"))? as usize;
        parse_toc_entry(data, at)
    }).collect()
}

/// The TOC entry at file offset `at`.
fn parse_toc_entry(data: &[u8], at: usize) -> Result<TocEntry, IpfError> {
    let fixed = data.get(at..at + TOC_FIXED).ok_or(IpfError::Malformed("TOC entry"))?;
    let (size, flags, cell_count) = (fixed[0] as usize, fixed[1], fixed[2] as usize);
    let entry = data.get(at..at + size).filter(|_| size >= TOC_FIXED).ok_or(IpfError::Malformed("TOC entry size"))?;
    let mut pos = TOC_FIXED;
    if flags & TOC_EXTENDED != 0 {
        let ext = u16_at(entry, pos).ok_or(IpfError::Malformed("extended TOC entry"))?;
        pos += 2;
        pos += EXT_PARTS.iter().filter(|(bit, _)| ext & bit != 0).map(|(_, n)| n).sum::<usize>();
    }
    let cells_end = pos + 2 * cell_count;
    if cells_end > entry.len() {
        return Err(IpfError::Malformed("TOC entry cells"));
    }
    let cells = (0..cell_count).map(|k| u16_at(entry, pos + 2 * k).unwrap_or(0)).collect();
    Ok(TocEntry {
        level: flags & TOC_LEVEL,
        hidden: flags & TOC_HIDDEN != 0,
        has_children: flags & TOC_HAS_CHILDREN != 0,
        extended: flags & TOC_EXTENDED != 0,
        cells,
        title: entry[cells_end..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal file: a header naming `entries` (raw TOC entries) through
    /// an offset table placed after them.
    fn file(flags: u8, title: &[u8], entries: &[Vec<u8>]) -> Vec<u8> {
        let mut f = vec![0u8; HEADER_SIZE];
        f[..3].copy_from_slice(MAGIC);
        f[3] = flags;
        f[4..6].copy_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
        f[6] = 2;
        f[7] = 2;
        f[8..10].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        f[TITLE_AT..TITLE_AT + title.len()].copy_from_slice(title);
        let offsets: Vec<u32> = entries.iter().map(|e| {
            let at = f.len() as u32;
            f.extend_from_slice(e);
            at
        }).collect();
        let table = f.len() as u32;
        f[18..22].copy_from_slice(&table.to_le_bytes());
        for o in offsets { f.extend_from_slice(&o.to_le_bytes()); }
        f
    }

    fn entry(flags: u8, cells: &[u16], ext: Option<(u16, usize)>, title: &[u8]) -> Vec<u8> {
        let mut e = vec![0, flags, cells.len() as u8];
        if let Some((word, extra)) = ext {
            e.extend_from_slice(&word.to_le_bytes());
            e.extend(std::iter::repeat_n(0xEE, extra));
        }
        for c in cells { e.extend_from_slice(&c.to_le_bytes()); }
        e.extend_from_slice(title);
        e[0] = e.len() as u8;
        e
    }

    #[test]
    fn headers_and_tocs_parse() {
        let f = file(FLAG_INF, b"My Book", &[
            entry(1 | TOC_HAS_CHILDREN, &[0], None, b"Introduction"),
            entry(2, &[1, 2], None, b"Details"),
            entry(1 | TOC_HIDDEN | TOC_EXTENDED, &[3], Some((EXT_SET_POS | EXT_SET_STYLE, 7)), b"Hidden"),
        ]);
        let h = parse_header(&f).unwrap();
        assert_eq!((h.help_library, h.version, h.toc_count, h.title.as_slice()), (false, (2, 2), 3, &b"My Book"[..]));
        let toc = parse_toc(&f, &h).unwrap();
        assert_eq!(toc.iter().map(|e| (e.level, e.title.as_slice())).collect::<Vec<_>>(),
                   vec![(1, &b"Introduction"[..]), (2, &b"Details"[..]), (1, &b"Hidden"[..])]);
        assert!(toc[0].has_children && !toc[1].has_children);
        assert_eq!(toc[1].cells, vec![1, 2]);
        assert!(toc[2].hidden && toc[2].extended, "the extended parts skipped");
        assert_eq!(toc[2].cells, vec![3]);
        assert!(parse_header(&file(FLAG_HLP, b"", &[])).unwrap().help_library);
    }

    #[test]
    fn junk_is_refused_and_never_overread() {
        assert_eq!(parse_header(b"HSP"), Err(IpfError::NotIpf));
        let mut f = file(FLAG_INF, b"T", &[entry(1, &[0], None, b"A")]);
        let h = parse_header(&f).unwrap();
        // An entry claiming more cells than it holds.
        let at = HEADER_SIZE;
        f[at + 2] = 50;
        assert_eq!(parse_toc(&f, &h), Err(IpfError::Malformed("TOC entry cells")));
        // An offset table past the end.
        let big = IpfHeader { toc_count: 0xFFFF, ..h.clone() };
        assert_eq!(parse_toc(&f, &big), Err(IpfError::Malformed("TOC offset table")));
        // A TOC offset past the end.
        let mut g = file(FLAG_INF, b"T", &[entry(1, &[], None, b"A")]);
        let table = u32_at(&g, 18).unwrap() as usize;
        g[table..table + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_toc(&g, &parse_header(&g).unwrap()).is_err());
        let mut bad = file(FLAG_INF, b"T", &[]);
        bad[3] = 0;
        assert_eq!(parse_header(&bad), Err(IpfError::NotIpf), "neither INF nor HLP");
    }
}
