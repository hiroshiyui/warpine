// SPDX-License-Identifier: GPL-3.0-only
//
// NE (New Executable) binary format structures and parsing.
//
// NE is the 16-bit executable format used by OS/2 1.x applications.
// An NE binary has an MZ (DOS) stub followed by the NE header at the
// offset specified by e_lfanew (MZ header offset 0x3C).

use std::io::{self, Read, Seek, SeekFrom};

// ── NE Header ──

/// NE header (64 bytes at e_lfanew offset).
#[derive(Debug, Default, Clone)]
pub struct NeHeader {
    pub signature: [u8; 2],                    // 00: "NE"
    pub linker_version: u8,                    // 02
    pub linker_revision: u8,                   // 03
    pub entry_table_offset: u16,               // 04: relative to NE header
    pub entry_table_size: u16,                 // 06: bytes
    pub checksum: u32,                         // 08
    pub module_flags: u16,                     // 0C
    pub auto_data_segment: u16,                // 0E: automatic data segment number
    pub initial_heap_size: u16,                // 10
    pub initial_stack_size: u16,               // 12
    pub initial_cs_ip: u32,                    // 14: entry point (CS in high word, IP in low word)
    pub initial_ss_sp: u32,                    // 18: initial stack (SS in high word, SP in low word)
    pub segment_count: u16,                    // 1C
    pub module_ref_count: u16,                 // 1E
    pub non_resident_name_table_size: u16,     // 20
    pub segment_table_offset: u16,             // 22: relative to NE header
    pub resource_table_offset: u16,            // 24: relative to NE header
    pub resident_name_table_offset: u16,       // 26: relative to NE header
    pub module_ref_table_offset: u16,          // 28: relative to NE header
    pub imported_names_table_offset: u16,      // 2A: relative to NE header
    pub non_resident_name_table_file_offset: u32, // 2C: absolute file offset
    pub movable_entry_count: u16,              // 30
    pub alignment_shift_count: u16,            // 32: logical sector size = 1 << shift
    pub resource_segment_count: u16,           // 34
    pub target_os: u8,                         // 36: 1=OS/2, 2=Windows
    pub additional_flags: u8,                  // 37
    pub fast_load_offset: u16,                 // 38
    pub fast_load_size: u16,                   // 3A
    pub reserved: u16,                         // 3C
    pub expected_win_version: u16,             // 3E
}

// Module flag constants
pub const NE_FLAG_SINGLEDATA: u16 = 0x0001;
pub const NE_FLAG_MULTIPLEDATA: u16 = 0x0002;
pub const NE_FLAG_PMCOMPAT: u16 = 0x0200;
pub const NE_FLAG_PMAPI: u16 = 0x0300;
pub const NE_FLAG_LIBRARY: u16 = 0x8000;

// Target OS constants
pub const NE_OS_OS2: u8 = 0x01;
pub const NE_OS_WINDOWS: u8 = 0x02;

impl NeHeader {
    pub fn read<R: Read + Seek>(reader: &mut R, offset: u64) -> io::Result<Self> {
        reader.seek(SeekFrom::Start(offset))?;
        let mut buf = [0u8; 64];
        reader.read_exact(&mut buf)?;

        let signature = [buf[0], buf[1]];
        if signature != [b'N', b'E'] {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("Invalid NE signature: {:02X} {:02X}", buf[0], buf[1])));
        }

        Ok(NeHeader {
            signature,
            linker_version: buf[2],
            linker_revision: buf[3],
            entry_table_offset: u16::from_le_bytes([buf[4], buf[5]]),
            entry_table_size: u16::from_le_bytes([buf[6], buf[7]]),
            checksum: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            module_flags: u16::from_le_bytes([buf[12], buf[13]]),
            auto_data_segment: u16::from_le_bytes([buf[14], buf[15]]),
            initial_heap_size: u16::from_le_bytes([buf[16], buf[17]]),
            initial_stack_size: u16::from_le_bytes([buf[18], buf[19]]),
            initial_cs_ip: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
            initial_ss_sp: u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]),
            segment_count: u16::from_le_bytes([buf[28], buf[29]]),
            module_ref_count: u16::from_le_bytes([buf[30], buf[31]]),
            non_resident_name_table_size: u16::from_le_bytes([buf[32], buf[33]]),
            segment_table_offset: u16::from_le_bytes([buf[34], buf[35]]),
            resource_table_offset: u16::from_le_bytes([buf[36], buf[37]]),
            resident_name_table_offset: u16::from_le_bytes([buf[38], buf[39]]),
            module_ref_table_offset: u16::from_le_bytes([buf[40], buf[41]]),
            imported_names_table_offset: u16::from_le_bytes([buf[42], buf[43]]),
            non_resident_name_table_file_offset: u32::from_le_bytes([buf[44], buf[45], buf[46], buf[47]]),
            movable_entry_count: u16::from_le_bytes([buf[48], buf[49]]),
            alignment_shift_count: u16::from_le_bytes([buf[50], buf[51]]),
            resource_segment_count: u16::from_le_bytes([buf[52], buf[53]]),
            target_os: buf[54],
            additional_flags: buf[55],
            fast_load_offset: u16::from_le_bytes([buf[56], buf[57]]),
            fast_load_size: u16::from_le_bytes([buf[58], buf[59]]),
            reserved: u16::from_le_bytes([buf[60], buf[61]]),
            expected_win_version: u16::from_le_bytes([buf[62], buf[63]]),
        })
    }

    /// Extract entry point CS (1-based segment number).
    pub fn entry_cs(&self) -> u16 { (self.initial_cs_ip >> 16) as u16 }
    /// Extract entry point IP (offset within CS segment).
    pub fn entry_ip(&self) -> u16 { (self.initial_cs_ip & 0xFFFF) as u16 }
    /// Extract stack SS (1-based segment number).
    pub fn stack_ss(&self) -> u16 { (self.initial_ss_sp >> 16) as u16 }
    /// Extract stack SP (initial stack pointer offset).
    pub fn stack_sp(&self) -> u16 { (self.initial_ss_sp & 0xFFFF) as u16 }

    pub fn is_dll(&self) -> bool { self.module_flags & NE_FLAG_LIBRARY != 0 }
    pub fn is_pm_app(&self) -> bool { self.module_flags & 0x0300 == NE_FLAG_PMAPI }
}

// ── Segment Table Entry ──

/// NE segment table entry (8 bytes per segment).
#[derive(Debug, Default, Clone)]
pub struct NeSegmentEntry {
    /// Logical-sector offset of segment data (shift by alignment_shift_count).
    pub data_offset: u16,
    /// Length of segment data in file (0 = 64KB).
    pub data_length: u16,
    /// Segment flags.
    pub flags: u16,
    /// Minimum allocation size (0 = 64KB).
    pub min_alloc_size: u16,
}

// Segment flag constants
pub const SEG_DATA: u16 = 0x0001;
pub const SEG_MOVABLE: u16 = 0x0010;
pub const SEG_PRELOAD: u16 = 0x0040;
pub const SEG_HAS_RELOCS: u16 = 0x0100;
pub const SEG_DISCARD: u16 = 0x1000;

impl NeSegmentEntry {
    pub fn read<R: Read>(reader: &mut R) -> io::Result<Self> {
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf)?;
        Ok(NeSegmentEntry {
            data_offset: u16::from_le_bytes([buf[0], buf[1]]),
            data_length: u16::from_le_bytes([buf[2], buf[3]]),
            flags: u16::from_le_bytes([buf[4], buf[5]]),
            min_alloc_size: u16::from_le_bytes([buf[6], buf[7]]),
        })
    }

    pub fn is_code(&self) -> bool { self.flags & SEG_DATA == 0 }
    pub fn is_data(&self) -> bool { self.flags & SEG_DATA != 0 }
    pub fn has_relocations(&self) -> bool { self.flags & SEG_HAS_RELOCS != 0 }

    /// Actual data length (0 means 64KB).
    pub fn actual_data_length(&self) -> u32 {
        if self.data_length == 0 { 0x10000 } else { self.data_length as u32 }
    }
    /// Actual minimum allocation size (0 means 64KB).
    pub fn actual_min_alloc(&self) -> u32 {
        if self.min_alloc_size == 0 { 0x10000 } else { self.min_alloc_size as u32 }
    }
    /// File offset of segment data given the alignment shift count.
    pub fn file_offset(&self, alignment_shift: u16) -> u64 {
        (self.data_offset as u64) << alignment_shift
    }
}

// ── Relocation Entry ──

/// NE relocation source type.
pub const RELOC_LOBYTE: u8 = 0;
pub const RELOC_SELECTOR: u8 = 2;
pub const RELOC_FAR_POINTER: u8 = 3;
pub const RELOC_OFFSET: u8 = 5;

/// NE relocation target type (flags bits 0-1).
pub const RELFLAG_INTERNALREF: u8 = 0;
pub const RELFLAG_IMPORTORDINAL: u8 = 1;
pub const RELFLAG_IMPORTNAME: u8 = 2;
pub const RELFLAG_OSFIXUP: u8 = 3;
pub const RELFLAG_ADDITIVE: u8 = 0x04;

/// NE relocation entry (8 bytes).
#[derive(Debug, Clone)]
pub struct NeRelocationEntry {
    pub source_type: u8,
    pub flags: u8,
    pub source_offset: u16,
    pub target: NeRelocationTarget,
}

#[derive(Debug, Clone)]
pub enum NeRelocationTarget {
    InternalRef { segment_num: u8, offset: u16 },
    ImportOrdinal { module_index: u16, ordinal: u16 },
    ImportName { module_index: u16, name_offset: u16 },
    OsFixup { fixup_type: u16 },
}

impl NeRelocationEntry {
    pub fn read<R: Read>(reader: &mut R) -> io::Result<Self> {
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf)?;

        let source_type = buf[0];
        let flags = buf[1];
        let source_offset = u16::from_le_bytes([buf[2], buf[3]]);

        let target_type = flags & 0x03;
        let target = match target_type {
            RELFLAG_INTERNALREF => NeRelocationTarget::InternalRef {
                segment_num: buf[4],
                offset: u16::from_le_bytes([buf[6], buf[7]]),
            },
            RELFLAG_IMPORTORDINAL => NeRelocationTarget::ImportOrdinal {
                module_index: u16::from_le_bytes([buf[4], buf[5]]),
                ordinal: u16::from_le_bytes([buf[6], buf[7]]),
            },
            RELFLAG_IMPORTNAME => NeRelocationTarget::ImportName {
                module_index: u16::from_le_bytes([buf[4], buf[5]]),
                name_offset: u16::from_le_bytes([buf[6], buf[7]]),
            },
            RELFLAG_OSFIXUP => NeRelocationTarget::OsFixup {
                fixup_type: u16::from_le_bytes([buf[4], buf[5]]),
            },
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("Unknown NE relocation target type: {}", target_type))),
        };

        Ok(NeRelocationEntry { source_type, flags, source_offset, target })
    }

    pub fn is_additive(&self) -> bool { self.flags & RELFLAG_ADDITIVE != 0 }
}

// ── Entry Table ──

/// Entry table bundle header.
pub const ENTRY_UNUSED: u8 = 0x00;
pub const ENTRY_MOVABLE: u8 = 0xFF;

/// Parsed entry point.
#[derive(Debug, Clone)]
pub struct NeEntry {
    pub ordinal: u16,
    pub flags: u8,
    pub segment_num: u8,
    pub offset: u16,
}

/// Parse the entry table from raw bytes.
pub fn parse_entry_table(data: &[u8]) -> Vec<NeEntry> {
    let mut entries = Vec::new();
    let mut pos = 0;
    let mut ordinal: u16 = 1;

    while pos < data.len() {
        let count = data[pos];
        if count == 0 { break; }
        let indicator = data[pos + 1];
        pos += 2;

        if indicator == ENTRY_UNUSED {
            // Skip `count` ordinals
            ordinal += count as u16;
            continue;
        }

        for _ in 0..count {
            if indicator == ENTRY_MOVABLE {
                // Movable entry: flags(1) + int3f(2) + segment(1) + offset(2) = 6 bytes
                if pos + 6 > data.len() { break; }
                let flags = data[pos];
                // skip int3f (2 bytes)
                let segment_num = data[pos + 3];
                let offset = u16::from_le_bytes([data[pos + 4], data[pos + 5]]);
                entries.push(NeEntry { ordinal, flags, segment_num, offset });
                pos += 6;
            } else {
                // Fixed entry: flags(1) + offset(2) = 3 bytes
                if pos + 3 > data.len() { break; }
                let flags = data[pos];
                let offset = u16::from_le_bytes([data[pos + 1], data[pos + 2]]);
                entries.push(NeEntry { ordinal, flags, segment_num: indicator, offset });
                pos += 3;
            }
            ordinal += 1;
        }
    }
    entries
}

// ── Name Table ──

/// Parse a name table (resident or non-resident).
/// Format: length-prefixed string + u16 ordinal, terminated by length=0.
pub fn parse_name_table(data: &[u8]) -> Vec<(String, u16)> {
    let mut entries = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let len = data[pos] as usize;
        if len == 0 { break; }
        pos += 1;
        if pos + len + 2 > data.len() { break; }
        let name = String::from_utf8_lossy(&data[pos..pos + len]).into_owned();
        pos += len;
        let ordinal = u16::from_le_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        entries.push((name, ordinal));
    }
    entries
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_ne_header_bytes() -> Vec<u8> {
        let mut buf = vec![0u8; 64];
        buf[0] = b'N'; buf[1] = b'E';
        buf[2] = 5; // linker version
        buf[3] = 1; // linker revision
        // segment_count at offset 28
        buf[28] = 3; buf[29] = 0; // 3 segments
        // module_ref_count at offset 30
        buf[30] = 2; buf[31] = 0; // 2 module refs
        // target_os at offset 54
        buf[54] = NE_OS_OS2;
        // alignment_shift_count at offset 50
        buf[50] = 9; buf[51] = 0; // shift=9 (512-byte sectors)
        // initial_cs_ip at offset 20: CS=1, IP=0x100
        buf[20] = 0x00; buf[21] = 0x01; buf[22] = 0x01; buf[23] = 0x00;
        // initial_ss_sp at offset 24: SS=2, SP=0x200
        buf[24] = 0x00; buf[25] = 0x02; buf[26] = 0x02; buf[27] = 0x00;
        buf
    }

    #[test]
    fn test_read_ne_header() {
        let data = make_ne_header_bytes();
        let mut cursor = Cursor::new(&data);
        let header = NeHeader::read(&mut cursor, 0).unwrap();
        assert_eq!(header.signature, [b'N', b'E']);
        assert_eq!(header.linker_version, 5);
        assert_eq!(header.segment_count, 3);
        assert_eq!(header.module_ref_count, 2);
        assert_eq!(header.target_os, NE_OS_OS2);
        assert_eq!(header.alignment_shift_count, 9);
        assert_eq!(header.entry_cs(), 1);
        assert_eq!(header.entry_ip(), 0x100);
        assert_eq!(header.stack_ss(), 2);
        assert_eq!(header.stack_sp(), 0x200);
    }

    #[test]
    fn test_read_ne_header_invalid_signature() {
        let mut data = make_ne_header_bytes();
        data[0] = b'X'; data[1] = b'X';
        let mut cursor = Cursor::new(&data);
        assert!(NeHeader::read(&mut cursor, 0).is_err());
    }

    #[test]
    fn test_read_segment_entry() {
        let data = [
            0x04, 0x00, // data_offset = 4 (sectors)
            0x00, 0x10, // data_length = 0x1000
            0x01, 0x01, // flags = SEG_DATA | SEG_HAS_RELOCS
            0x00, 0x20, // min_alloc = 0x2000
        ];
        let mut cursor = Cursor::new(&data[..]);
        let seg = NeSegmentEntry::read(&mut cursor).unwrap();
        assert_eq!(seg.data_offset, 4);
        assert_eq!(seg.data_length, 0x1000);
        assert!(seg.is_data());
        assert!(seg.has_relocations());
        assert_eq!(seg.actual_data_length(), 0x1000);
        assert_eq!(seg.actual_min_alloc(), 0x2000);
        assert_eq!(seg.file_offset(9), 4 * 512); // 4 sectors * 512 bytes
    }

    #[test]
    fn test_segment_zero_means_64k() {
        let data = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data[..]);
        let seg = NeSegmentEntry::read(&mut cursor).unwrap();
        assert_eq!(seg.actual_data_length(), 0x10000);
        assert_eq!(seg.actual_min_alloc(), 0x10000);
    }

    #[test]
    fn test_read_relocation_internal() {
        let data = [
            RELOC_FAR_POINTER, // source type
            RELFLAG_INTERNALREF, // flags: internal ref
            0x10, 0x00,        // source offset = 0x10
            0x02,              // segment 2
            0x00,              // reserved
            0x34, 0x12,        // offset 0x1234
        ];
        let mut cursor = Cursor::new(&data[..]);
        let rel = NeRelocationEntry::read(&mut cursor).unwrap();
        assert_eq!(rel.source_type, RELOC_FAR_POINTER);
        assert_eq!(rel.source_offset, 0x10);
        match rel.target {
            NeRelocationTarget::InternalRef { segment_num, offset } => {
                assert_eq!(segment_num, 2);
                assert_eq!(offset, 0x1234);
            }
            _ => panic!("Expected InternalRef"),
        }
    }

    #[test]
    fn test_read_relocation_import_ordinal() {
        let data = [
            RELOC_FAR_POINTER,
            RELFLAG_IMPORTORDINAL,
            0x20, 0x00,
            0x01, 0x00, // module index 1
            0x05, 0x00, // ordinal 5
        ];
        let mut cursor = Cursor::new(&data[..]);
        let rel = NeRelocationEntry::read(&mut cursor).unwrap();
        match rel.target {
            NeRelocationTarget::ImportOrdinal { module_index, ordinal } => {
                assert_eq!(module_index, 1);
                assert_eq!(ordinal, 5);
            }
            _ => panic!("Expected ImportOrdinal"),
        }
    }

    #[test]
    fn test_read_relocation_import_name() {
        let data = [
            RELOC_OFFSET,
            RELFLAG_IMPORTNAME,
            0x30, 0x00,
            0x02, 0x00, // module index 2
            0x0A, 0x00, // name offset 10
        ];
        let mut cursor = Cursor::new(&data[..]);
        let rel = NeRelocationEntry::read(&mut cursor).unwrap();
        match rel.target {
            NeRelocationTarget::ImportName { module_index, name_offset } => {
                assert_eq!(module_index, 2);
                assert_eq!(name_offset, 10);
            }
            _ => panic!("Expected ImportName"),
        }
    }

    #[test]
    fn test_parse_entry_table_fixed() {
        // Bundle: count=2, indicator=1 (segment 1), then 2 fixed entries (3 bytes each)
        let data = [
            2, 1,           // count=2, segment=1
            0x01, 0x10, 0x00, // flags=1, offset=0x10
            0x00, 0x20, 0x00, // flags=0, offset=0x20
            0,              // terminator
        ];
        let entries = parse_entry_table(&data);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ordinal, 1);
        assert_eq!(entries[0].segment_num, 1);
        assert_eq!(entries[0].offset, 0x10);
        assert_eq!(entries[1].ordinal, 2);
        assert_eq!(entries[1].offset, 0x20);
    }

    #[test]
    fn test_parse_entry_table_movable() {
        // Bundle: count=1, indicator=0xFF (movable), then 1 movable entry (6 bytes)
        let data = [
            1, 0xFF,
            0x01,             // flags
            0xCD, 0x3F,       // INT 3F marker
            0x02,             // segment 2
            0x50, 0x00,       // offset 0x50
            0,                // terminator
        ];
        let entries = parse_entry_table(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ordinal, 1);
        assert_eq!(entries[0].segment_num, 2);
        assert_eq!(entries[0].offset, 0x50);
    }

    #[test]
    fn test_parse_entry_table_skip() {
        // Bundle: count=3, indicator=0 (skip 3 ordinals), then count=1, segment=1
        let data = [
            3, 0,             // skip 3 ordinals
            1, 1,             // count=1, segment=1
            0x00, 0x30, 0x00, // flags=0, offset=0x30
            0,
        ];
        let entries = parse_entry_table(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ordinal, 4); // skipped 1,2,3
        assert_eq!(entries[0].offset, 0x30);
    }

    #[test]
    fn test_parse_name_table() {
        // Two entries: "MYMODULE" ordinal 0, "MyFunc" ordinal 1
        let data = [
            8, b'M', b'Y', b'M', b'O', b'D', b'U', b'L', b'E', 0x00, 0x00,
            6, b'M', b'y', b'F', b'u', b'n', b'c', 0x01, 0x00,
            0, // terminator
        ];
        let names = parse_name_table(&data);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].0, "MYMODULE");
        assert_eq!(names[0].1, 0);
        assert_eq!(names[1].0, "MyFunc");
        assert_eq!(names[1].1, 1);
    }

    #[test]
    fn test_ne_header_flags() {
        let mut header = NeHeader::default();
        header.module_flags = NE_FLAG_LIBRARY;
        assert!(header.is_dll());
        assert!(!header.is_pm_app());

        header.module_flags = NE_FLAG_PMAPI;
        assert!(!header.is_dll());
        assert!(header.is_pm_app());
    }
}
