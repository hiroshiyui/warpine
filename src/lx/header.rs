use std::io::{self, Read, Seek, SeekFrom};

#[derive(Debug, Default, Clone)]
#[repr(C)]
pub struct LxHeader {
    pub signature: [u8; 2],            // "LX"
    pub byte_order: u8,               // 0 = Little Endian
    pub word_order: u8,               // 0 = Little Endian
    pub format_level: u32,            // LX format level
    pub cpu_type: u16,                // 1=286, 2=386, 3=486...
    pub os_type: u16,                 // 1=OS/2
    pub module_version: u32,
    pub module_flags: u32,            // 0x04 = EXE, 0x07 = DLL
    pub module_num_pages: u32,        // Total number of pages in module
    pub eip_object: u32,              // Object number for EIP
    pub eip: u32,                     // Initial EIP
    pub esp_object: u32,              // Object number for ESP
    pub esp: u32,                     // Initial ESP
    pub page_size: u32,               // Usually 4096
    pub page_offset_shift: u32,       // Shift count for page offsets
    pub fixup_section_size: u32,      // Total size of fixup section
    pub fixup_section_checksum: u32,
    pub loader_section_size: u32,     // Total size of loader section
    pub loader_section_checksum: u32,
    pub object_table_offset: u32,     // Offset to object table
    pub object_count: u32,            // Number of objects in module
    pub object_page_map_offset: u32,  // Offset to object page map
    pub object_iter_data_map_offset: u32,
    pub resource_table_offset: u32,
    pub resource_count: u32,
    pub resident_name_table_offset: u32,
    pub entry_table_offset: u32,      // Offset to entry table
    pub module_directives_offset: u32,
    pub module_directives_count: u32,
    pub fixup_page_table_offset: u32, // Offset to fixup page table
    pub fixup_record_table_offset: u32, // Offset to fixup record table
    pub imported_modules_name_table_offset: u32,
    pub imported_modules_count: u32,
    pub import_procedure_name_table_offset: u32,
    pub per_page_checksum_table_offset: u32,
    pub data_pages_offset: u32,       // Offset to data pages
    pub num_preload_pages: u32,
    pub non_resident_name_table_offset: u32,
    pub non_resident_name_table_length: u32,
    pub non_resident_name_table_checksum: u32,
    pub auto_ds_object: u32,
    pub debug_info_offset: u32,
    pub debug_info_length: u32,
    pub num_instance_preload: u32,
    pub num_instance_demand: u32,
    pub heap_size: u32,
}

#[derive(Debug, Default, Clone)]
#[repr(C)]
pub struct ObjectTableEntry {
    pub size: u32,              // Virtual size of the object
    pub base_address: u32,      // Relocation base address
    pub flags: u32,             // Object flags (Read/Write/Exec, etc.)
    pub page_map_index: u32,    // Index into the Object Page Map (1-based)
    pub page_count: u32,        // Number of pages in this object
    pub reserved: u32,          // Reserved/Internal
}

impl ObjectTableEntry {
    pub const SIZE: usize = 24;

    pub fn read<R: Read + Seek>(reader: &mut R) -> io::Result<Self> {
        let mut buf4 = [0u8; 4];
        let mut entry = Self::default();

        reader.read_exact(&mut buf4)?; entry.size = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; entry.base_address = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; entry.flags = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; entry.page_map_index = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; entry.page_count = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; entry.reserved = u32::from_le_bytes(buf4);

        Ok(entry)
    }
}

#[derive(Debug, Default, Clone)]
#[repr(C)]
pub struct ObjectPageMapEntry {
    pub data_offset: u32,       // Offset into the Data Pages section
    pub data_size: u16,         // Size of data in the file
    pub flags: u16,            // Page flags
}

impl ObjectPageMapEntry {
    pub const SIZE: usize = 8;

    pub fn read<R: Read + Seek>(reader: &mut R) -> io::Result<Self> {
        let mut buf4 = [0u8; 4];
        let mut buf2 = [0u8; 2];
        let mut entry = Self::default();

        reader.read_exact(&mut buf4)?; entry.data_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf2)?; entry.data_size = u16::from_le_bytes(buf2);
        reader.read_exact(&mut buf2)?; entry.flags = u16::from_le_bytes(buf2);

        Ok(entry)
    }
}

#[derive(Debug, Clone)]
pub enum FixupTarget {
    Internal {
        object_num: u16,
        target_offset: u32,
    },
    ExternalOrdinal {
        module_ordinal: u16,
        proc_ordinal: u32,
    },
    ExternalName {
        module_ordinal: u16,
        proc_name_offset: u32,
    },
    InternalEntry {
        entry_ordinal: u16,
    },
}

#[derive(Debug, Clone)]
pub struct LxFixupRecord {
    pub source_type: u8,
    pub target_flags: u8,
    pub source_offsets: Vec<u16>,
    pub target: FixupTarget,
    pub additive: Option<u32>,
}

impl LxFixupRecord {
    pub fn read<R: Read + Seek>(reader: &mut R) -> io::Result<Self> {
        let mut b1 = [0u8; 1];
        let mut b2 = [0u8; 2];
        let mut b4 = [0u8; 4];

        reader.read_exact(&mut b1)?; let source_type = b1[0];
        reader.read_exact(&mut b1)?; let target_flags = b1[0];

        let mut source_offsets = Vec::new();
        if (source_type & 0x20) != 0 {
            // Multiple source offsets
            reader.read_exact(&mut b1)?;
            let count = b1[0];
            for _ in 0..count {
                reader.read_exact(&mut b2)?;
                source_offsets.push(u16::from_le_bytes(b2));
            }
        } else {
            // Single source offset
            reader.read_exact(&mut b2)?;
            source_offsets.push(u16::from_le_bytes(b2));
        }

        // Target type (bits 0-1):
        // 00 = Internal
        // 01 = External Ordinal
        // 02 = External Name
        // 03 = Internal Entry Table
        let target_type = target_flags & 0x03;
        let index_is_16bit = (target_flags & 0x40) != 0;
        let offset_is_32bit = (target_flags & 0x10) != 0;

        let target = match target_type {
            0 => { // Internal
                let object_num = if index_is_16bit {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2)
                } else {
                    reader.read_exact(&mut b1)?; b1[0] as u16
                };
                let target_offset = if offset_is_32bit {
                    reader.read_exact(&mut b4)?; u32::from_le_bytes(b4)
                } else {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2) as u32
                };
                FixupTarget::Internal { object_num, target_offset }
            },
            1 => { // External Ordinal
                let module_ordinal = if index_is_16bit {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2)
                } else {
                    reader.read_exact(&mut b1)?; b1[0] as u16
                };
                // Ordinal size (bit 7: 1=8-bit, 0=16/32 bit based on bit 4)
                let proc_ordinal = if (target_flags & 0x80) != 0 {
                    reader.read_exact(&mut b1)?; b1[0] as u32
                } else if offset_is_32bit {
                    reader.read_exact(&mut b4)?; u32::from_le_bytes(b4)
                } else {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2) as u32
                };
                FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal }
            },
            2 => { // External Name
                let module_ordinal = if index_is_16bit {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2)
                } else {
                    reader.read_exact(&mut b1)?; b1[0] as u16
                };
                let proc_name_offset = if offset_is_32bit {
                    reader.read_exact(&mut b4)?; u32::from_le_bytes(b4)
                } else {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2) as u32
                };
                FixupTarget::ExternalName { module_ordinal, proc_name_offset }
            },
            3 => { // Internal Entry Table
                let entry_ordinal = if index_is_16bit {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2)
                } else {
                    reader.read_exact(&mut b1)?; b1[0] as u16
                };
                FixupTarget::InternalEntry { entry_ordinal }
            },
            _ => unreachable!(),
        };

        let additive = if (target_flags & 0x04) != 0 {
            if (target_flags & 0x20) != 0 {
                reader.read_exact(&mut b4)?; Some(u32::from_le_bytes(b4))
            } else {
                reader.read_exact(&mut b2)?; Some(u16::from_le_bytes(b2) as u32)
            }
        } else {
            None
        };

        Ok(LxFixupRecord {
            source_type,
            target_flags,
            source_offsets,
            target,
            additive,
        })
    }
}

impl LxHeader {
    pub const SIGNATURE: [u8; 2] = *b"LX";

    pub fn read<R: Read + Seek>(reader: &mut R, offset: u64) -> io::Result<Self> {
        reader.seek(SeekFrom::Start(offset))?;
        let mut header = Self::default();
        
        let mut b1 = [0u8; 1];
        let mut buf2 = [0u8; 2];
        let mut buf4 = [0u8; 4];

        reader.read_exact(&mut header.signature)?;
        if header.signature != Self::SIGNATURE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid LX signature"));
        }

        reader.read_exact(&mut b1)?; header.byte_order = b1[0];
        reader.read_exact(&mut b1)?; header.word_order = b1[0];
        
        reader.read_exact(&mut buf4)?; header.format_level = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf2)?; header.cpu_type = u16::from_le_bytes(buf2);
        reader.read_exact(&mut buf2)?; header.os_type = u16::from_le_bytes(buf2);
        reader.read_exact(&mut buf4)?; header.module_version = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.module_flags = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.module_num_pages = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.eip_object = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.eip = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.esp_object = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.esp = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.page_size = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.page_offset_shift = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.fixup_section_size = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.fixup_section_checksum = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.loader_section_size = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.loader_section_checksum = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.object_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.object_count = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.object_page_map_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.object_iter_data_map_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.resource_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.resource_count = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.resident_name_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.entry_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.module_directives_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.module_directives_count = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.fixup_page_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.fixup_record_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.imported_modules_name_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.imported_modules_count = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.import_procedure_name_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.per_page_checksum_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.data_pages_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.num_preload_pages = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.non_resident_name_table_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.non_resident_name_table_length = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.non_resident_name_table_checksum = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.auto_ds_object = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.debug_info_offset = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.debug_info_length = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.num_instance_preload = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.num_instance_demand = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf4)?; header.heap_size = u32::from_le_bytes(buf4);

        Ok(header)
    }
}
