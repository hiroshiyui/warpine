use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub mod header;
use header::{LxHeader, ObjectTableEntry, ObjectPageMapEntry};

pub struct LxFile {
    pub header: LxHeader,
    pub object_table: Vec<ObjectTableEntry>,
    pub page_map: Vec<ObjectPageMapEntry>,
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

        Ok(LxFile { 
            header,
            object_table,
            page_map,
        })
    }
}
