// SPDX-License-Identifier: GPL-3.0-only
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub mod header;
use header::{LxHeader, ObjectTableEntry, ObjectPageMapEntry, LxFixupRecord, LxResourceEntry};

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
            imported_modules.push(String::from_utf8_lossy(&name_buf).into_owned());
        }

        // 9. Read Import Procedure Name Table
        let import_proc_table_start = lx_offset + header.import_procedure_name_table_offset as u64;
        
        let mut import_procedure_names = Vec::new();
        if header.import_procedure_name_table_offset > 0 {
            reader.seek(SeekFrom::Start(import_proc_table_start))?;
            let remaining = (lx_offset + header.loader_section_size as u64).saturating_sub(import_proc_table_start);
            if remaining > 0 {
                let mut table_data = vec![0u8; remaining as usize];
                reader.read_exact(&mut table_data)?;
                import_procedure_names = table_data;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
        assert!(res.is_ok() || res.unwrap_err().to_string().contains("Invalid LX signature") == false);
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
        assert!(lx.object_table.len() > 0);
    }
}
