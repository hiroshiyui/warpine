// SPDX-License-Identifier: GPL-3.0-or-later
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub mod header;
use header::{LxHeader, ObjectTableEntry, ObjectPageMapEntry, LxFixupRecord, LxResourceEntry};

/// An exported entry from the LX entry table.
#[derive(Debug, Clone)]
pub struct LxExport {
    pub ordinal: u32,
    pub object_num: u16,
    pub offset: u32,
}

/// The module an import names: a name with the `.DLL` extension (as
/// Open Watcom's import libraries write a DLL linked as `name x.dll`)
/// stands for the module without it, as the OS/2 loader treats it.
pub fn module_name_of(name: &str) -> String {
    let n = name.len();
    if n > 4 && name.is_char_boundary(n - 4) && name[n - 4..].eq_ignore_ascii_case(".DLL") {
        name[..n - 4].to_string()
    } else {
        name.to_string()
    }
}

/// Largest import procedure name table read (a bound on hostile files).
const MAX_IMPORT_PROC_TABLE: u64 = 1 << 20;

#[derive(Debug)]
pub struct LxFile {
    pub header: LxHeader,
    pub object_table: Vec<ObjectTableEntry>,
    pub page_map: Vec<ObjectPageMapEntry>,
    pub fixup_page_table: Vec<u32>,
    pub fixup_records_by_page: Vec<Vec<LxFixupRecord>>,
    pub resources: Vec<LxResourceEntry>,
    pub imported_modules: Vec<String>,
    pub import_procedure_names: Vec<u8>,
    pub exports: Vec<LxExport>,
    pub nonresident_names: Vec<(u32, String)>,
    /// Exported names of the resident names table (ordinal, name), the
    /// module's own name (ordinal 0) left out. Open Watcom's linker puts a
    /// DLL's exported names here.
    pub resident_names: Vec<(u32, String)>,
}

impl LxFile {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let file = File::open(path)?;
        Self::parse(file)
    }

    pub fn parse<R: Read + Seek>(mut reader: R) -> io::Result<Self> {
        // 1. Read MZ Header
        let mut mz_header = [0u8; 64];
        reader.read_exact(&mut mz_header)?;
        
        if &mz_header[0..2] != b"MZ" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid MZ signature"));
        }

        // 2. Read LX header offset from MZ header at 0x3C
        let lx_offset = u32::from_le_bytes([
            mz_header[0x3C],
            mz_header[0x3D],
            mz_header[0x3E],
            mz_header[0x3F],
        ]) as u64;

        // 3. Read LX Header
        let header = LxHeader::read(&mut reader, lx_offset)?;

        // Validate header fields to prevent resource exhaustion from malformed inputs
        const MAX_OBJECTS: u32 = 1024;
        const MAX_PAGES: u32 = 65536; // 256MB at 4KB pages
        if header.object_count > MAX_OBJECTS {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("Object count {} exceeds maximum {}", header.object_count, MAX_OBJECTS)));
        }
        if header.module_num_pages > MAX_PAGES {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("Page count {} exceeds maximum {}", header.module_num_pages, MAX_PAGES)));
        }
        if header.page_offset_shift >= 32 {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("Invalid page_offset_shift: {}", header.page_offset_shift)));
        }
        if header.eip_object > 0 && header.eip_object > header.object_count {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("eip_object {} exceeds object_count {}", header.eip_object, header.object_count)));
        }
        if header.esp_object > 0 && header.esp_object > header.object_count {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("esp_object {} exceeds object_count {}", header.esp_object, header.object_count)));
        }

        // 4. Read Object Table
        let object_table_start = lx_offset + header.object_table_offset as u64;
        reader.seek(SeekFrom::Start(object_table_start))?;

        let mut object_table = Vec::with_capacity(header.object_count as usize);
        for _ in 0..header.object_count {
            object_table.push(ObjectTableEntry::read(&mut reader)?);
        }

        // 5. Read Object Page Map
        let page_map_start = lx_offset + header.object_page_map_offset as u64;
        reader.seek(SeekFrom::Start(page_map_start))?;

        let mut page_map = Vec::with_capacity(header.module_num_pages as usize);
        for _ in 0..header.module_num_pages {
            page_map.push(ObjectPageMapEntry::read(&mut reader)?);
        }

        // 5b. Read Resource Table
        const MAX_RESOURCES: u32 = 4096;
        if header.resource_count > MAX_RESOURCES {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("Resource count {} exceeds maximum {}", header.resource_count, MAX_RESOURCES)));
        }
        let mut resources = Vec::with_capacity(header.resource_count as usize);
        if header.resource_count > 0 && header.resource_table_offset > 0 {
            let resource_table_start = lx_offset + header.resource_table_offset as u64;
            reader.seek(SeekFrom::Start(resource_table_start))?;
            for _ in 0..header.resource_count {
                resources.push(LxResourceEntry::read(&mut reader)?);
            }
        }

        // 6. Read Fixup Page Table
        let fixup_page_table_start = lx_offset + header.fixup_page_table_offset as u64;
        reader.seek(SeekFrom::Start(fixup_page_table_start))?;
        
        let num_fixup_pages = header.module_num_pages + 1;
        let mut fixup_page_table = Vec::with_capacity(num_fixup_pages as usize);
        let mut buf4 = [0u8; 4];
        for _ in 0..num_fixup_pages {
            reader.read_exact(&mut buf4)?;
            fixup_page_table.push(u32::from_le_bytes(buf4));
        }

        // 7. Read Fixup Records by Page
        let fixup_record_table_start = lx_offset + header.fixup_record_table_offset as u64;
        let mut fixup_records_by_page = Vec::with_capacity(header.module_num_pages as usize);
        
        for i in 0..header.module_num_pages as usize {
            let start_offset = fixup_page_table[i] as u64;
            let end_offset = fixup_page_table[i+1] as u64;
            let mut page_records = Vec::new();
            
            if end_offset > start_offset {
                reader.seek(SeekFrom::Start(fixup_record_table_start + start_offset))?;
                while reader.stream_position()? < fixup_record_table_start + end_offset {
                    page_records.push(LxFixupRecord::read(&mut reader)?);
                }
            }
            fixup_records_by_page.push(page_records);
        }

        // 8. Read Import Module Name Table
        let import_module_table_start = lx_offset + header.imported_modules_name_table_offset as u64;
        reader.seek(SeekFrom::Start(import_module_table_start))?;
        
        let mut imported_modules = Vec::with_capacity(header.imported_modules_count as usize);
        for _ in 0..header.imported_modules_count {
            let mut len_buf = [0u8; 1];
            reader.read_exact(&mut len_buf)?;
            let len = len_buf[0] as usize;
            let mut name_buf = vec![0u8; len];
            reader.read_exact(&mut name_buf)?;
            imported_modules.push(module_name_of(&String::from_utf8_lossy(&name_buf)));
        }

        // 9. Read Import Procedure Name Table
        let import_proc_table_start = lx_offset + header.import_procedure_name_table_offset as u64;

        let mut import_procedure_names = Vec::new();
        if header.import_procedure_name_table_offset > 0 {
            reader.seek(SeekFrom::Start(import_proc_table_start))?;
            // The table is the last part of the fixup section, which starts
            // at the fixup page table (not of the loader section).
            let fixup_end = lx_offset + header.fixup_page_table_offset as u64 + header.fixup_section_size as u64;
            let remaining = fixup_end.saturating_sub(import_proc_table_start).min(MAX_IMPORT_PROC_TABLE);
            if remaining > 0 {
                let mut table_data = vec![0u8; remaining as usize];
                reader.read_exact(&mut table_data)?;
                import_procedure_names = table_data;
            }
        }

        // 10. Parse Entry Table (ordinal → object + offset)
        let mut exports = Vec::new();
        if header.entry_table_offset > 0 {
            let entry_table_start = lx_offset + header.entry_table_offset as u64;
            reader.seek(SeekFrom::Start(entry_table_start))?;
            let mut ordinal = 1u32;
            loop {
                let mut b1 = [0u8; 1];
                if reader.read_exact(&mut b1).is_err() { break; }
                let count = b1[0];
                if count == 0 { break; }

                let mut b2 = [0u8; 1];
                reader.read_exact(&mut b2)?;
                let bundle_type = b2[0];

                match bundle_type {
                    0 => {
                        // EMPTY — skip `count` ordinal slots
                        ordinal += count as u32;
                    }
                    3 => {
                        // ENTRY32 — object_num then count×(flags:u8 + offset:u32)
                        let mut obj_buf = [0u8; 2];
                        reader.read_exact(&mut obj_buf)?;
                        let object_num = u16::from_le_bytes(obj_buf);
                        for _ in 0..count {
                            let mut entry_buf = [0u8; 5]; // flags(1) + offset(4)
                            reader.read_exact(&mut entry_buf)?;
                            let offset = u32::from_le_bytes([entry_buf[1], entry_buf[2], entry_buf[3], entry_buf[4]]);
                            exports.push(LxExport { ordinal, object_num, offset });
                            ordinal += 1;
                        }
                    }
                    1 => {
                        // ENTRY16 — count×(flags:u8 + offset:u16)
                        for _ in 0..count {
                            let mut skip = [0u8; 3];
                            reader.read_exact(&mut skip)?;
                            ordinal += 1;
                        }
                    }
                    2 => {
                        // CALLGATE — count×(flags:u8 + offset:u16 + gate:u16)
                        for _ in 0..count {
                            let mut skip = [0u8; 5];
                            reader.read_exact(&mut skip)?;
                            ordinal += 1;
                        }
                    }
                    _ => {
                        // Unknown bundle type — stop parsing
                        break;
                    }
                }
            }
        }

        // 11. Parse Non-Resident Names Table (absolute file offset, not relative to LX header)
        let mut nonresident_names = Vec::new();
        if header.non_resident_name_table_offset > 0 && header.non_resident_name_table_length > 0 {
            reader.seek(SeekFrom::Start(header.non_resident_name_table_offset as u64))?;
            nonresident_names = read_name_table(&mut reader, header.non_resident_name_table_length as usize)?;
        }
        // 12. The Resident Names Table (relative to the LX header), up to the
        // entry table that follows it.
        let mut resident_names = Vec::new();
        if header.resident_name_table_offset > 0 {
            let limit = header.entry_table_offset.checked_sub(header.resident_name_table_offset)
                .filter(|&n| n > 0).map_or(MAX_NAME_TABLE, |n| (n as usize).min(MAX_NAME_TABLE));
            reader.seek(SeekFrom::Start(lx_offset + header.resident_name_table_offset as u64))?;
            resident_names = read_name_table(&mut reader, limit)?;
            resident_names.retain(|&(ordinal, _)| ordinal != 0);
        }

        Ok(LxFile {
            header,
            object_table,
            page_map,
            fixup_page_table,
            fixup_records_by_page,
            resources,
            imported_modules,
            import_procedure_names,
            exports,
            nonresident_names,
            resident_names,
        })
    }

    pub fn get_proc_name(&self, offset: u32) -> Option<String> {
        let offset = offset as usize;
        if offset >= self.import_procedure_names.len() {
            return None;
        }
        let len = self.import_procedure_names[offset] as usize;
        if offset + 1 + len > self.import_procedure_names.len() {
            return None;
        }
        let name_bytes = &self.import_procedure_names[offset + 1 .. offset + 1 + len];
        Some(String::from_utf8_lossy(name_bytes).into_owned())
    }
}

/// Most bytes of a names table read.
const MAX_NAME_TABLE: usize = 1 << 20;

/// A names table (resident or non-resident): entries of a length byte,
/// the name and a 16-bit ordinal, ended by a zero length, the end of the
/// file or `limit` bytes (at most [`MAX_NAME_TABLE`]).
fn read_name_table<R: Read>(reader: &mut R, limit: usize) -> io::Result<Vec<(u32, String)>> {
    let mut names = Vec::new();
    let mut used = 0usize;
    let limit = limit.min(MAX_NAME_TABLE);
    loop {
        let mut lb = [0u8; 1];
        if used >= limit || reader.read_exact(&mut lb).is_err() { break; }
        let len = lb[0] as usize;
        if len == 0 || used + 1 + len + 2 > limit { break; }
        let mut entry = vec![0u8; len + 2];
        if reader.read_exact(&mut entry).is_err() { break; }
        used += 1 + len + 2;
        let ordinal = u16::from_le_bytes([entry[len], entry[len + 1]]) as u32;
        names.push((ordinal, String::from_utf8_lossy(&entry[..len]).into_owned()));
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    /// Imports by name: the import procedure name table lies in the fixup
    /// section (som_test's programs import SOM.DLL by name; skipped when the
    /// sample is not built).
    #[test]
    fn imports_by_name_read_their_names() {
        let path = "samples/som_test/hello.exe";
        if !std::path::Path::new(path).exists() { return; }
        let lx = LxFile::open(path).expect("parse");
        let names: Vec<String> = lx.fixup_records_by_page.iter().flatten().filter_map(|r| match r.target {
            crate::lx::header::FixupTarget::ExternalName { proc_name_offset, .. } => lx.get_proc_name(proc_name_offset),
            _ => None,
        }).collect();
        assert!(names.iter().any(|n| n == "somResolve"), "{names:?}");
        assert!(names.iter().any(|n| n == "SOMObjectClassData"), "{names:?}");
        assert!(lx.imported_modules.iter().any(|m| m == "SOM"));
    }

    use super::*;
    use std::io::Cursor;

    #[test]
    fn an_imported_module_name_loses_its_dll_extension() {
        assert_eq!(module_name_of("COURSE.DLL"), "COURSE");
        assert_eq!(module_name_of("course.dll"), "course");
        assert_eq!(module_name_of("DOSCALLS"), "DOSCALLS");
        assert_eq!(module_name_of(".DLL"), ".DLL", "nothing before it");
        assert_eq!(module_name_of("MY.DLLX"), "MY.DLLX");
    }

    #[test]
    fn test_parse_invalid_mz() {
        let data = vec![0u8; 64];
        let cursor = Cursor::new(data);
        let res = LxFile::parse(cursor);
        assert!(res.is_err());
    }

    #[test]
    fn test_parse_mz_points_to_lx() {
        let mut data = vec![0u8; 1024];
        data[0] = b'M'; data[1] = b'Z';
        data[0x3C] = 0x80; // LX at 0x80
        
        // LX Header at 0x80
        data[0x80] = b'L'; data[0x81] = b'X';
        
        let cursor = Cursor::new(data);
        let res = LxFile::parse(cursor);
        // It will fail because object tables etc are missing, 
        // but it should at least get past MZ check
        assert!(res.is_ok() || !res.unwrap_err().to_string().contains("Invalid LX signature"));
    }

    #[test]
    fn test_reject_excessive_object_count() {
        let mut data = vec![0u8; 1024];
        data[0] = b'M'; data[1] = b'Z';
        data[0x3C] = 0x80;
        data[0x80] = b'L'; data[0x81] = b'X';
        // Set object_count to 2000 (exceeds MAX_OBJECTS=1024)
        let count: u32 = 2000;
        data[0x80 + 0x44..0x80 + 0x48].copy_from_slice(&count.to_le_bytes());
        let cursor = Cursor::new(data);
        let res = LxFile::parse(cursor);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("Object count"));
    }

    #[test]
    fn test_reject_excessive_page_count() {
        let mut data = vec![0u8; 1024];
        data[0] = b'M'; data[1] = b'Z';
        data[0x3C] = 0x80;
        data[0x80] = b'L'; data[0x81] = b'X';
        // Set module_num_pages to 100000 (exceeds MAX_PAGES=65536)
        let pages: u32 = 100000;
        data[0x80 + 0x14..0x80 + 0x18].copy_from_slice(&pages.to_le_bytes());
        let cursor = Cursor::new(data);
        let res = LxFile::parse(cursor);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("Page count"));
    }

    #[test]
    fn test_reject_invalid_page_offset_shift() {
        let mut data = vec![0u8; 1024];
        data[0] = b'M'; data[1] = b'Z';
        data[0x3C] = 0x80;
        data[0x80] = b'L'; data[0x81] = b'X';
        // Set page_offset_shift to 32 (invalid)
        let shift: u32 = 32;
        data[0x80 + 0x2C..0x80 + 0x30].copy_from_slice(&shift.to_le_bytes());
        let cursor = Cursor::new(data);
        let res = LxFile::parse(cursor);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("page_offset_shift"));
    }

    #[test]
    fn test_reject_invalid_eip_object() {
        let mut data = vec![0u8; 1024];
        data[0] = b'M'; data[1] = b'Z';
        data[0x3C] = 0x80;
        data[0x80] = b'L'; data[0x81] = b'X';
        // object_count = 2, eip_object = 5
        let count: u32 = 2;
        data[0x80 + 0x44..0x80 + 0x48].copy_from_slice(&count.to_le_bytes());
        let eip_obj: u32 = 5;
        data[0x80 + 0x18..0x80 + 0x1C].copy_from_slice(&eip_obj.to_le_bytes());
        let cursor = Cursor::new(data);
        let res = LxFile::parse(cursor);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("eip_object"));
    }

    #[test]
    fn test_parse_actual_hello_exe() {
        let path = Path::new("samples/hello/hello.exe");
        if !path.exists() {
            println!("Skipping actual file test (hello.exe not found)");
            return;
        }
        let res = LxFile::open(path);
        assert!(res.is_ok(), "Failed to parse actual hello.exe: {:?}", res.err());
        let lx = res.unwrap();
        assert_eq!(lx.header.signature, *b"LX");
        assert!(!lx.object_table.is_empty());
    }

    #[test]
    fn test_parse_entry_table_and_nonresident_names() {
        // samples/dll_noinit/noinit.dll (built with the Open Watcom
        // samples) — only run if present
        let path = "samples/dll_noinit/noinit.dll";
        if !std::path::Path::new(path).exists() { return; }
        let lx = LxFile::open(path).expect("parse noinit.dll");
        // Entry table: 3 exports in object 1
        assert_eq!(lx.exports.len(), 3, "expected 3 exports");
        assert!(lx.exports.iter().all(|e| e.object_num == 1), "all in object 1");
        assert!(lx.exports.iter().enumerate().all(|(i, e)| e.ordinal == (i + 1) as u32), "ordinals 1..3");
        // Non-resident names: the two exported by ordinal only.
        let nonresident: Vec<&str> = lx.nonresident_names.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(nonresident, ["NoInitAdd", "NoInitTwice"]);
        assert!(lx.resident_names.iter().any(|(o, n)| *o == 3 && n == "NoInitAnswer"), "the resident one");
    }

    /// A names table: length, name, ordinal; ended by a zero length, the
    /// end of the data or the byte limit (a truncated entry is dropped).
    #[test]
    fn name_tables_are_read_within_their_bounds() {
        let table = [7, b'G', b'R', b'E', b'E', b'T', b'E', b'R', 0, 0,
                     13, b'S', b'O', b'M', b'I', b'n', b'i', b't', b'M', b'o', b'd', b'u', b'l', b'e', 1, 0,
                     0, 3, b'x', b'y', b'z', 9, 0];
        let names = super::read_name_table(&mut &table[..], table.len()).unwrap();
        assert_eq!(names, vec![(0, "GREETER".to_string()), (1, "SOMInitModule".to_string())]);
        // A limit inside the second entry keeps only the first.
        assert_eq!(super::read_name_table(&mut &table[..], 20).unwrap().len(), 1);
        // A table cut short by the end of the data.
        assert_eq!(super::read_name_table(&mut &table[..14], 1000).unwrap().len(), 1);
    }

    /// Open Watcom's DLLs name their exports in the resident names table
    /// (the SOM sample's greeter.dll, when built).
    #[test]
    fn resident_names_are_read() {
        let path = "samples/som_test/greeter.dll";
        if !std::path::Path::new(path).exists() { return; }
        let lx = LxFile::open(path).expect("parse greeter.dll");
        let names: Vec<&str> = lx.resident_names.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["SOMInitModule", "GreeterNewClass", "GreeterClassData"], "module name left out");
        assert!(lx.resident_names.iter().all(|(o, _)| *o != 0));
    }
}
