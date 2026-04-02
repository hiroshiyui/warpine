// SPDX-License-Identifier: GPL-3.0-only
use std::io::{self, Read, Seek, SeekFrom};

// OS/2 LX resource type constants (OS/2 numbering, not Windows).
// Reference: IBM OS/2 Warp 4 Toolkit <pmwin.h> / <os2.h> RT_* macros.
pub const RT_POINTER: u16 = 1;
pub const RT_BITMAP: u16 = 2;
pub const RT_MENU: u16 = 3;
pub const RT_DIALOG: u16 = 4;
pub const RT_STRING: u16 = 5;
pub const RT_FONTDIR: u16 = 6;
pub const RT_FONT: u16 = 7;
pub const RT_ACCELTABLE: u16 = 8;
pub const RT_RCDATA: u16 = 9;
pub const RT_MESSAGE: u16 = 10;
pub const RT_DLGINCLUDE: u16 = 11;

#[derive(Debug, Default, Clone)]
#[repr(C)]
pub struct LxHeader {
    pub signature: [u8; 2],            // 00h
    pub byte_order: u8,               // 02h
    pub word_order: u8,               // 03h
    pub format_level: u32,            // 04h
    pub cpu_type: u16,                // 08h
    pub os_type: u16,                 // 0Ah
    pub module_version: u32,          // 0Ch
    pub module_flags: u32,            // 10h
    pub module_num_pages: u32,        // 14h
    pub eip_object: u32,              // 18h
    pub eip: u32,                     // 1Ch
    pub esp_object: u32,              // 20h
    pub esp: u32,                     // 24h
    pub page_size: u32,               // 28h
    pub page_offset_shift: u32,       // 2Ch
    pub fixup_section_size: u32,      // 30h
    pub fixup_section_checksum: u32,  // 34h
    pub loader_section_size: u32,     // 38h
    pub loader_section_checksum: u32, // 3Ch
    pub object_table_offset: u32,     // 40h
    pub object_count: u32,            // 44h
    pub object_page_map_offset: u32,  // 48h
    pub object_iter_data_map_offset: u32, // 4Ch
    pub resource_table_offset: u32,   // 50h
    pub resource_count: u32,          // 54h
    pub resident_name_table_offset: u32, // 58h
    pub entry_table_offset: u32,      // 5Ch
    pub module_directives_offset: u32,// 60h
    pub module_directives_count: u32, // 64h
    pub fixup_page_table_offset: u32, // 68h
    pub fixup_record_table_offset: u32, // 6Ch
    pub imported_modules_name_table_offset: u32, // 70h
    pub imported_modules_count: u32,  // 74h
    pub import_procedure_name_table_offset: u32, // 78h
    pub per_page_checksum_table_offset: u32, // 7Ch
    pub data_pages_offset: u32,       // 80h
    pub num_preload_pages: u32,       // 84h
    pub non_resident_name_table_offset: u32, // 88h
    pub non_resident_name_table_length: u32, // 8Ch
    pub non_resident_name_table_checksum: u32, // 90h
    pub auto_ds_object: u32,          // 94h
    pub debug_info_offset: u32,       // 98h
    pub debug_info_length: u32,       // 9Ch
    pub num_instance_preload: u32,    // A0h
    pub num_instance_demand: u32,     // A4h
    pub heap_size: u32,               // A8h
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

#[derive(Debug, Default, Clone)]
#[repr(C)]
pub struct LxResourceEntry {
    pub type_id: u16,    // Resource type (RT_MENU, RT_STRING, etc.)
    pub name_id: u16,    // Resource ID
    pub size: u32,       // Size in bytes
    pub object_num: u16, // 1-based object index containing the data
    pub offset: u32,     // Offset within that object
}

impl LxResourceEntry {
    pub const SIZE: usize = 14;

    pub fn read<R: Read + Seek>(reader: &mut R) -> io::Result<Self> {
        let mut buf2 = [0u8; 2];
        let mut buf4 = [0u8; 4];
        let mut entry = Self::default();

        reader.read_exact(&mut buf2)?; entry.type_id = u16::from_le_bytes(buf2);
        reader.read_exact(&mut buf2)?; entry.name_id = u16::from_le_bytes(buf2);
        reader.read_exact(&mut buf4)?; entry.size = u32::from_le_bytes(buf4);
        reader.read_exact(&mut buf2)?; entry.object_num = u16::from_le_bytes(buf2);
        reader.read_exact(&mut buf4)?; entry.offset = u32::from_le_bytes(buf4);

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
            reader.read_exact(&mut b1)?;
            let count = b1[0];
            for _ in 0..count {
                reader.read_exact(&mut b2)?;
                source_offsets.push(u16::from_le_bytes(b2));
            }
        } else {
            reader.read_exact(&mut b2)?;
            source_offsets.push(u16::from_le_bytes(b2));
        }

        let target_type = target_flags & 0x03;
        let index_is_16bit = (target_flags & 0x40) != 0;
        let offset_is_32bit = (target_flags & 0x10) != 0;

        let target = match target_type {
            0 => {
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
            1 => {
                let module_ordinal = if index_is_16bit {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2)
                } else {
                    reader.read_exact(&mut b1)?; b1[0] as u16
                };
                let proc_ordinal = if (target_flags & 0x80) != 0 {
                    reader.read_exact(&mut b1)?; b1[0] as u32
                } else if offset_is_32bit {
                    reader.read_exact(&mut b4)?; u32::from_le_bytes(b4)
                } else {
                    reader.read_exact(&mut b2)?; u16::from_le_bytes(b2) as u32
                };
                FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal }
            },
            2 => {
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
            3 => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_read_lx_header_minimal() {
        let mut data = vec![0u8; 196];
        data[0] = b'L';
        data[1] = b'X';
        data[0x08] = 2; // CPU Type 386
        data[0x0A] = 1; // OS Type OS/2
        data[0x1C] = 0x9A; // EIP
        data[0x80] = 0xCC; // Data Pages Offset
        data[0x81] = 0x01;

        let mut cursor = Cursor::new(data);
        let header = LxHeader::read(&mut cursor, 0).expect("Should parse header");

        assert_eq!(header.signature, *b"LX");
        assert_eq!(header.cpu_type, 2);
        assert_eq!(header.os_type, 1);
        assert_eq!(header.eip, 0x9A);
        assert_eq!(header.data_pages_offset, 0x1CC);
    }

    #[test]
    fn test_read_resource_entry() {
        let mut data = vec![0u8; 14];
        // type_id = 6 (arbitrary raw value — just tests the parser round-trip)
        data[0] = 6; data[1] = 0;
        // name_id = 42
        data[2] = 42; data[3] = 0;
        // size = 0x100
        data[4] = 0x00; data[5] = 0x01; data[6] = 0x00; data[7] = 0x00;
        // object_num = 2
        data[8] = 2; data[9] = 0;
        // offset = 0x200
        data[10] = 0x00; data[11] = 0x02; data[12] = 0x00; data[13] = 0x00;

        let mut cursor = Cursor::new(data);
        let entry = LxResourceEntry::read(&mut cursor).expect("Should parse resource entry");

        assert_eq!(entry.type_id, 6);
        assert_eq!(entry.name_id, 42);
        assert_eq!(entry.size, 0x100);
        assert_eq!(entry.object_num, 2);
        assert_eq!(entry.offset, 0x200);
    }

    #[test]
    fn test_read_object_table_entry() {
        let mut data = vec![0u8; 24];
        data[0x00] = 0xC4; data[0x01] = 0x05; // Size 0x5C4
        data[0x04] = 0x00; data[0x05] = 0x00; data[0x06] = 0x01; // Base 0x10000
        data[0x08] = 0x05; data[0x09] = 0x20; // Flags 0x2005

        let mut cursor = Cursor::new(data);
        let entry = ObjectTableEntry::read(&mut cursor).expect("Should parse object entry");

        assert_eq!(entry.size, 0x5C4);
        assert_eq!(entry.base_address, 0x10000);
        assert_eq!(entry.flags, 0x2005);
    }
}
