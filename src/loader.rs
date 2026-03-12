use crate::lx::LxFile;
use crate::lx::header::FixupTarget;
use crate::api;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::ptr;
use libc::{mmap, munmap, mprotect, MAP_ANONYMOUS, MAP_PRIVATE, MAP_FIXED, PROT_READ, PROT_WRITE, PROT_EXEC};

pub struct MappedObject {
    pub base_addr: usize,
    pub size: usize,
}

pub struct Loader {
    mapped_objects: Vec<MappedObject>,
}

impl Loader {
    pub fn new() -> Self {
        Loader {
            mapped_objects: Vec::new(),
        }
    }

    pub fn load<P: AsRef<Path>>(&mut self, lx_file: &LxFile, path: P) -> io::Result<()> {
        let mut file = File::open(path)?;

        let mut mz_header = [0u8; 64];
        file.read_exact(&mut mz_header)?;

        let data_pages_base = lx_file.header.data_pages_offset as u64;

        for (i, obj) in lx_file.object_table.iter().enumerate() {
            println!("Mapping Object {} at 0x{:08X} (size 0x{:08X})...", i + 1, obj.base_address, obj.size);

            let addr = obj.base_address as *mut libc::c_void;
            let size = ((obj.size as usize + 4095) & !4095) as usize;

            let map_ptr = unsafe {
                mmap(
                    addr,
                    size,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                    -1,
                    0,
                )
            };

            if map_ptr == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }

            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.page_map.len() {
                    break;
                }
                let page_entry = &lx_file.page_map[page_idx];
                let page_file_offset = data_pages_base + ((page_entry.data_offset as u64) << lx_file.header.page_offset_shift);
                let target_addr = (obj.base_address as usize + p * 4096) as *mut u8;

                if page_entry.data_size > 0 {
                    file.seek(SeekFrom::Start(page_file_offset))?;
                    let mut page_data = vec![0u8; page_entry.data_size as usize];
                    file.read_exact(&mut page_data)?;
                    unsafe {
                        ptr::copy_nonoverlapping(page_data.as_ptr(), target_addr, page_entry.data_size as usize);
                    }
                }
            }

            self.mapped_objects.push(MappedObject {
                base_addr: obj.base_address as usize,
                size,
            });
        }

        // Apply Fixups
        self.apply_fixups(lx_file)?;

        // Apply final protections
        for (i, obj) in lx_file.object_table.iter().enumerate() {
            let mut prot = 0;
            if (obj.flags & 0x0001) != 0 { prot |= PROT_READ; }
            if (obj.flags & 0x0002) != 0 { prot |= PROT_WRITE; }
            if (obj.flags & 0x0004) != 0 { prot |= PROT_EXEC; }
            
            let mapped = &self.mapped_objects[i];
            unsafe {
                mprotect(mapped.base_addr as *mut libc::c_void, mapped.size, prot);
            }
        }

        Ok(())
    }

    fn apply_fixups(&self, lx_file: &LxFile) -> io::Result<()> {
        for obj in &lx_file.object_table {
            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.fixup_records_by_page.len() {
                    break;
                }
                
                let records = &lx_file.fixup_records_by_page[page_idx];
                for record in records {
                    let target_addr = match &record.target {
                        FixupTarget::Internal { object_num, target_offset } => {
                            let target_obj = lx_file.object_table.get((*object_num as usize).wrapping_sub(1));
                            if let Some(to) = target_obj {
                                to.base_address as usize + *target_offset as usize
                            } else {
                                continue;
                            }
                        },
                        FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal } => {
                            let mod_name = lx_file.imported_modules.get((*module_ordinal as usize).wrapping_sub(1))
                                .map(|s| s.as_str()).unwrap_or("");
                            self.resolve_import(mod_name, *proc_ordinal).unwrap_or(0)
                        },
                        FixupTarget::ExternalName { module_ordinal, proc_name_offset } => {
                            let mod_name = lx_file.imported_modules.get((*module_ordinal as usize).wrapping_sub(1))
                                .map(|s| s.as_str()).unwrap_or("");
                            let proc_name = lx_file.get_proc_name(*proc_name_offset).unwrap_or_else(|| "".to_string());
                            self.resolve_import_by_name(mod_name, &proc_name).unwrap_or(0)
                        },
                        _ => continue,
                    };

                    if target_addr == 0 {
                        continue;
                    }

                    for &source_offset in &record.source_offsets {
                        let source_addr = obj.base_address as usize + p * 4096 + source_offset as usize;
                        
                        unsafe {
                            match record.source_type & 0x0F {
                                0x07 => { // 32-bit Offset
                                    ptr::write_unaligned(source_addr as *mut u32, target_addr as u32);
                                },
                                0x08 => { // 32-bit Self-relative
                                    let rel = (target_addr as isize) - (source_addr as isize + 4);
                                    ptr::write_unaligned(source_addr as *mut i32, rel as i32);
                                },
                                _ => println!("Warning: Unhandled source type 0x{:02X}", record.source_type),
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_import(&self, module: &str, ordinal: u32) -> Option<usize> {
        if module == "DOSCALLS" {
            match ordinal {
                282 => Some(api::bridges::DosWrite as *const () as usize),
                234 => Some(api::bridges::DosExit as *const () as usize),
                348 => Some(api::bridges::DosQuerySysInfo as *const () as usize),
                _ => {
                    println!("Warning: Unresolved DOSCALLS ordinal {}", ordinal);
                    None
                }
            }
        } else {
            println!("Warning: Unresolved module {}", module);
            None
        }
    }

    fn resolve_import_by_name(&self, module: &str, name: &str) -> Option<usize> {
        println!("Warning: Import by name not yet supported ({} in {})", name, module);
        None
    }

    pub fn run(&self, lx_file: &LxFile) -> ! {
        let entry_obj = lx_file.object_table.get((lx_file.header.eip_object as usize).wrapping_sub(1))
            .expect("Invalid entry object");
        let entry_eip = entry_obj.base_address as usize + lx_file.header.eip as usize;

        let stack_obj = lx_file.object_table.get((lx_file.header.esp_object as usize).wrapping_sub(1))
            .expect("Invalid stack object");
        let entry_esp = stack_obj.base_address as usize + lx_file.header.esp as usize;

        println!("Jumping to OS/2 Entry Point:");
        println!("  EIP: 0x{:08X}", entry_eip);
        println!("  ESP: 0x{:08X}", entry_esp);

        // For now, we'll just exit.
        // To actually run this, we need a 32-bit environment and inline assembly.
        std::process::exit(0);
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        for obj in &self.mapped_objects {
            unsafe {
                munmap(obj.base_addr as *mut libc::c_void, obj.size);
            }
        }
    }
}
