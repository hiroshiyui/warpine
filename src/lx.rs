use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub mod header;
use header::{LxHeader, ObjectTableEntry, ObjectPageMapEntry, LxFixupRecord};

pub struct LxFile {
    pub header: LxHeader,
    pub object_table: Vec<ObjectTableEntry>,
    pub page_map: Vec<ObjectPageMapEntry>,
    pub fixup_page_table: Vec<u32>,
    pub fixup_records: Vec<LxFixupRecord>,
    pub imported_modules: Vec<String>,
    pub import_procedure_names: Vec<u8>,
}

impl LxFile {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let mut file = File::open(path)?;
        
        // 1. Read MZ Header
        let mut mz_header = [0u8; 64];
        file.read_exact(&mut mz_header)?;
        
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
        let header = LxHeader::read(&mut file, lx_offset)?;

        // 4. Read Object Table
        let object_table_start = lx_offset + header.object_table_offset as u64;
        file.seek(SeekFrom::Start(object_table_start))?;
        
        let mut object_table = Vec::with_capacity(header.object_count as usize);
        for _ in 0..header.object_count {
            object_table.push(ObjectTableEntry::read(&mut file)?);
        }

        // 5. Read Object Page Map
        let page_map_start = lx_offset + header.object_page_map_offset as u64;
        file.seek(SeekFrom::Start(page_map_start))?;

        let mut page_map = Vec::with_capacity(header.module_num_pages as usize);
        for _ in 0..header.module_num_pages {
            page_map.push(ObjectPageMapEntry::read(&mut file)?);
        }

        // 6. Read Fixup Page Table
        let fixup_page_table_start = lx_offset + header.fixup_page_table_offset as u64;
        file.seek(SeekFrom::Start(fixup_page_table_start))?;
        
        let num_fixup_pages = header.module_num_pages + 1;
        let mut fixup_page_table = Vec::with_capacity(num_fixup_pages as usize);
        let mut buf4 = [0u8; 4];
        for _ in 0..num_fixup_pages {
            file.read_exact(&mut buf4)?;
            fixup_page_table.push(u32::from_le_bytes(buf4));
        }

        // 7. Read Fixup Records
        let fixup_record_table_start = lx_offset + header.fixup_record_table_offset as u64;
        let fixup_record_table_end = fixup_record_table_start + *fixup_page_table.last().unwrap() as u64;
        file.seek(SeekFrom::Start(fixup_record_table_start))?;

        let mut fixup_records = Vec::new();
        while file.stream_position()? < fixup_record_table_end {
            fixup_records.push(LxFixupRecord::read(&mut file)?);
        }

        // 8. Read Import Module Name Table
        let import_module_table_start = lx_offset + header.imported_modules_name_table_offset as u64;
        file.seek(SeekFrom::Start(import_module_table_start))?;
        
        let mut imported_modules = Vec::with_capacity(header.imported_modules_count as usize);
        for _ in 0..header.imported_modules_count {
            let mut len_buf = [0u8; 1];
            file.read_exact(&mut len_buf)?;
            let len = len_buf[0] as usize;
            let mut name_buf = vec![0u8; len];
            file.read_exact(&mut name_buf)?;
            imported_modules.push(String::from_utf8_lossy(&name_buf).into_owned());
        }

        // 9. Read Import Procedure Name Table
        let import_proc_table_start = lx_offset + header.import_procedure_name_table_offset as u64;
        
        let mut import_procedure_names = Vec::new();
        if header.import_procedure_name_table_offset > 0 {
            file.seek(SeekFrom::Start(import_proc_table_start))?;
            let remaining = (lx_offset + header.loader_section_size as u64).saturating_sub(import_proc_table_start);
            if remaining > 0 {
                let mut table_data = vec![0u8; remaining as usize];
                file.read_exact(&mut table_data)?;
                import_procedure_names = table_data;
            }
        }

        Ok(LxFile { 
            header,
            object_table,
            page_map,
            fixup_page_table,
            fixup_records,
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
