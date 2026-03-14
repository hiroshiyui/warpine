// SPDX-License-Identifier: GPL-3.0-only
//
// NE (New Executable) format parser for OS/2 1.x 16-bit applications.
//
// Parses MZ stub → NE header → segment table → relocations → entry table →
// module references → name tables.

pub mod header;

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use log::{debug, warn};

use header::*;

/// Parsed NE executable file.
#[derive(Debug)]
pub struct NeFile {
    pub header: NeHeader,
    pub segment_table: Vec<NeSegmentEntry>,
    /// Per-segment relocation entries (index = segment index, 0-based).
    pub relocations_by_segment: Vec<Vec<NeRelocationEntry>>,
    /// Imported module names (resolved from module reference table).
    pub imported_modules: Vec<String>,
    /// Entry table (parsed entry points).
    pub entries: Vec<NeEntry>,
    /// Resident name table entries (name, ordinal).
    pub resident_names: Vec<(String, u16)>,
    /// Non-resident name table entries (name, ordinal).
    pub non_resident_names: Vec<(String, u16)>,
    /// Raw imported names table data (for name-based relocation resolution).
    pub imported_names_data: Vec<u8>,
    /// NE header file offset (for computing table positions).
    #[allow(dead_code)]
    ne_offset: u64,
}

impl NeFile {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let file = File::open(path)?;
        Self::parse(file)
    }

    pub fn parse<R: Read + Seek>(mut reader: R) -> io::Result<Self> {
        // Read MZ header to find NE offset
        let mut mz_buf = [0u8; 64];
        reader.read_exact(&mut mz_buf)?;
        if mz_buf[0] != b'M' || mz_buf[1] != b'Z' {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Not a valid MZ executable"));
        }
        let ne_offset = u32::from_le_bytes([mz_buf[60], mz_buf[61], mz_buf[62], mz_buf[63]]) as u64;
        if ne_offset < 64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("Invalid e_lfanew: 0x{:X}", ne_offset)));
        }

        // Read NE header
        let header = NeHeader::read(&mut reader, ne_offset)?;

        // Validate
        if header.segment_count > 255 {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("Excessive NE segment count: {}", header.segment_count)));
        }
        if header.target_os != NE_OS_OS2 {
            warn!("NE target OS is {} (expected 1=OS/2), proceeding anyway", header.target_os);
        }

        let shift = header.alignment_shift_count;
        debug!("NE header: {} segments, {} module refs, target_os={}, shift={}",
               header.segment_count, header.module_ref_count, header.target_os, shift);
        debug!("  Entry: CS:IP = {}:{:04X}, SS:SP = {}:{:04X}",
               header.entry_cs(), header.entry_ip(), header.stack_ss(), header.stack_sp());

        // Read segment table
        let seg_table_pos = ne_offset + header.segment_table_offset as u64;
        reader.seek(SeekFrom::Start(seg_table_pos))?;
        let mut segment_table = Vec::with_capacity(header.segment_count as usize);
        for _ in 0..header.segment_count {
            segment_table.push(NeSegmentEntry::read(&mut reader)?);
        }
        for (i, seg) in segment_table.iter().enumerate() {
            debug!("  Segment {}: offset={} len={} flags=0x{:04X} minalloc={}{}",
                   i + 1, seg.data_offset, seg.actual_data_length(), seg.flags,
                   seg.actual_min_alloc(),
                   if seg.is_code() { " [CODE]" } else { " [DATA]" });
        }

        // Read imported names table (raw bytes, needed for name resolution)
        let imported_names_pos = ne_offset + header.imported_names_table_offset as u64;
        // Size: from imported_names_table to entry_table (or end of header area)
        let imported_names_end = ne_offset + header.entry_table_offset as u64;
        let imported_names_size = if imported_names_end > imported_names_pos {
            (imported_names_end - imported_names_pos) as usize
        } else { 0 };
        let mut imported_names_data = vec![0u8; imported_names_size];
        if imported_names_size > 0 {
            reader.seek(SeekFrom::Start(imported_names_pos))?;
            reader.read_exact(&mut imported_names_data)?;
        }

        // Read module reference table and resolve names
        let mod_ref_pos = ne_offset + header.module_ref_table_offset as u64;
        reader.seek(SeekFrom::Start(mod_ref_pos))?;
        let mut imported_modules = Vec::with_capacity(header.module_ref_count as usize);
        for _ in 0..header.module_ref_count {
            let mut ref_buf = [0u8; 2];
            reader.read_exact(&mut ref_buf)?;
            let name_offset = u16::from_le_bytes(ref_buf) as usize;
            // Read length-prefixed name from imported names data
            let name = if name_offset < imported_names_data.len() {
                let len = imported_names_data[name_offset] as usize;
                if name_offset + 1 + len <= imported_names_data.len() {
                    String::from_utf8_lossy(&imported_names_data[name_offset + 1..name_offset + 1 + len]).into_owned()
                } else { String::new() }
            } else { String::new() };
            debug!("  Module ref: '{}'", name);
            imported_modules.push(name);
        }

        // Read entry table
        let entry_table_pos = ne_offset + header.entry_table_offset as u64;
        reader.seek(SeekFrom::Start(entry_table_pos))?;
        let mut entry_data = vec![0u8; header.entry_table_size as usize];
        reader.read_exact(&mut entry_data)?;
        let entries = parse_entry_table(&entry_data);
        debug!("  {} entry points parsed", entries.len());

        // Read resident name table
        let res_name_pos = ne_offset + header.resident_name_table_offset as u64;
        let res_name_end = ne_offset + header.module_ref_table_offset as u64;
        let res_name_size = if res_name_end > res_name_pos {
            (res_name_end - res_name_pos) as usize
        } else { 0 };
        let resident_names = if res_name_size > 0 {
            let mut data = vec![0u8; res_name_size];
            reader.seek(SeekFrom::Start(res_name_pos))?;
            reader.read_exact(&mut data)?;
            parse_name_table(&data)
        } else { Vec::new() };

        // Read non-resident name table
        let non_resident_names = if header.non_resident_name_table_file_offset > 0 && header.non_resident_name_table_size > 0 {
            let mut data = vec![0u8; header.non_resident_name_table_size as usize];
            reader.seek(SeekFrom::Start(header.non_resident_name_table_file_offset as u64))?;
            reader.read_exact(&mut data)?;
            parse_name_table(&data)
        } else { Vec::new() };

        // Read per-segment relocations
        let mut relocations_by_segment = Vec::with_capacity(segment_table.len());
        for (i, seg) in segment_table.iter().enumerate() {
            if seg.has_relocations() && seg.data_offset > 0 {
                let seg_data_end = seg.file_offset(shift) + seg.actual_data_length() as u64;
                reader.seek(SeekFrom::Start(seg_data_end))?;
                let mut count_buf = [0u8; 2];
                reader.read_exact(&mut count_buf)?;
                let reloc_count = u16::from_le_bytes(count_buf) as usize;
                let mut relocs = Vec::with_capacity(reloc_count);
                for _ in 0..reloc_count {
                    relocs.push(NeRelocationEntry::read(&mut reader)?);
                }
                debug!("  Segment {} relocations: {}", i + 1, reloc_count);
                relocations_by_segment.push(relocs);
            } else {
                relocations_by_segment.push(Vec::new());
            }
        }

        Ok(NeFile {
            header,
            segment_table,
            relocations_by_segment,
            imported_modules,
            entries,
            resident_names,
            non_resident_names,
            imported_names_data,
            ne_offset,
        })
    }

    /// Look up an imported name at the given offset in the imported names table.
    pub fn get_imported_name(&self, offset: u16) -> Option<String> {
        let off = offset as usize;
        if off >= self.imported_names_data.len() { return None; }
        let len = self.imported_names_data[off] as usize;
        if off + 1 + len > self.imported_names_data.len() { return None; }
        Some(String::from_utf8_lossy(&self.imported_names_data[off + 1..off + 1 + len]).into_owned())
    }

    /// Resolve an entry point by ordinal. Returns (segment_num, offset) or None.
    pub fn resolve_entry(&self, ordinal: u16) -> Option<(u8, u16)> {
        self.entries.iter()
            .find(|e| e.ordinal == ordinal)
            .map(|e| (e.segment_num, e.offset))
    }

    /// Module name (first entry in resident name table, ordinal 0).
    pub fn module_name(&self) -> Option<&str> {
        self.resident_names.first()
            .filter(|(_, ord)| *ord == 0)
            .map(|(name, _)| name.as_str())
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_parse_invalid_mz() {
        let data = vec![0u8; 128];
        let result = NeFile::parse(Cursor::new(&data));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_mz_points_to_ne() {
        // Build a minimal MZ + NE file
        let mut data = vec![0u8; 256];
        // MZ header
        data[0] = b'M'; data[1] = b'Z';
        // e_lfanew at offset 60, pointing to NE header at offset 128
        data[60] = 128; data[61] = 0; data[62] = 0; data[63] = 0;
        // NE header at offset 128
        data[128] = b'N'; data[129] = b'E';
        // segment_count = 0
        data[128 + 28] = 0; data[128 + 29] = 0;
        // alignment_shift_count = 9
        data[128 + 50] = 9;
        // target_os = OS/2
        data[128 + 54] = NE_OS_OS2;
        // entry_table_offset relative to NE header (point past the header)
        data[128 + 4] = 64; // entry table at NE+64 = 192
        // entry_table_size = 1 (just a terminator)
        data[128 + 6] = 1;
        // Put entry table terminator
        data[192] = 0;
        // segment_table_offset, resident_name, module_ref, imported_names — all at safe offsets
        data[128 + 34] = 64; // segment table at same as entry table (0 segments)
        data[128 + 38] = 64; // resident name table
        data[128 + 40] = 64; // module ref table
        data[128 + 42] = 64; // imported names table

        let result = NeFile::parse(Cursor::new(&data));
        assert!(result.is_ok(), "Failed: {:?}", result.err());
        let ne = result.unwrap();
        assert_eq!(ne.header.segment_count, 0);
        assert_eq!(ne.header.target_os, NE_OS_OS2);
        assert_eq!(ne.segment_table.len(), 0);
    }

    #[test]
    fn test_reject_excessive_segment_count() {
        let mut data = vec![0u8; 256];
        data[0] = b'M'; data[1] = b'Z';
        data[60] = 128;
        data[128] = b'N'; data[129] = b'E';
        // segment_count = 300 (> 255 limit)
        data[128 + 28] = 0x2C; data[128 + 29] = 0x01; // 300
        data[128 + 54] = NE_OS_OS2;

        let result = NeFile::parse(Cursor::new(&data));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_actual_ne_hello() {
        // Parse the actual 16-bit NE hello world sample
        let path = "samples/ne_hello/ne_hello.exe";
        if !std::path::Path::new(path).exists() {
            eprintln!("Skipping test: {} not found (run make -C samples/ne_hello first)", path);
            return;
        }
        let ne = NeFile::open(path).expect("Failed to parse NE file");

        // Verify header
        assert_eq!(ne.header.signature, [b'N', b'E']);
        assert_eq!(ne.header.target_os, NE_OS_OS2);
        assert!(!ne.header.is_dll());

        // Should have at least 1 segment
        assert!(ne.segment_table.len() >= 1, "Expected at least 1 segment, got {}", ne.segment_table.len());

        // Entry point should be valid
        let cs = ne.header.entry_cs();
        assert!(cs >= 1 && cs <= ne.segment_table.len() as u16,
                "Entry CS {} out of range (1..{})", cs, ne.segment_table.len());

        // Should import DOSCALLS
        let has_doscalls = ne.imported_modules.iter().any(|m|
            m.eq_ignore_ascii_case("DOSCALLS") || m.eq_ignore_ascii_case("DOSCALL1"));
        assert!(has_doscalls, "Expected DOSCALLS import, got: {:?}", ne.imported_modules);

        // Entry table may be empty for simple apps (entry point is in header CS:IP)
        println!("  Entry table entries: {}", ne.entries.len());
        println!("  Entry table offset: {}, size: {}", ne.header.entry_table_offset, ne.header.entry_table_size);

        // Module name should be in resident names
        if let Some(name) = ne.module_name() {
            assert!(!name.is_empty());
            println!("  NE module name: {}", name);
        }

        println!("  Segments: {}", ne.segment_table.len());
        println!("  Entry CS:IP = {}:{:04X}", cs, ne.header.entry_ip());
        println!("  Stack SS:SP = {}:{:04X}", ne.header.stack_ss(), ne.header.stack_sp());
        println!("  Imports: {:?}", ne.imported_modules);
        println!("  Entries: {}", ne.entries.len());
        for (i, seg) in ne.segment_table.iter().enumerate() {
            println!("  Seg {}: {} len={} minalloc={} flags=0x{:04X} relocs={}",
                     i + 1, if seg.is_code() { "CODE" } else { "DATA" },
                     seg.actual_data_length(), seg.actual_min_alloc(), seg.flags,
                     ne.relocations_by_segment.get(i).map_or(0, |r| r.len()));
        }
    }

    #[test]
    fn test_entry_helpers() {
        let header = NeHeader {
            initial_cs_ip: 0x0002_0100, // CS=2, IP=0x100
            initial_ss_sp: 0x0003_0400, // SS=3, SP=0x400
            ..Default::default()
        };
        assert_eq!(header.entry_cs(), 2);
        assert_eq!(header.entry_ip(), 0x100);
        assert_eq!(header.stack_ss(), 3);
        assert_eq!(header.stack_sp(), 0x400);
    }
}
