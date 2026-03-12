use crate::lx::LxFile;
use crate::lx::header::FixupTarget;
use crate::api;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use unicorn_engine::{Unicorn, RegisterX86, Prot};
use unicorn_engine::unicorn_const::{Arch, Mode, HookType};

pub struct Loader {
}

impl Loader {
    pub fn new() -> Self {
        Loader {}
    }

    pub fn load<P: AsRef<Path>>(&mut self, lx_file: &LxFile, path: P) -> io::Result<Unicorn<'static, ()>> {
        let mut emu = Unicorn::new(Arch::X86, Mode::MODE_32)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to init Unicorn: {:?}", e)))?;

        // Map larger zero-page (first 64KB)
        emu.mem_map(0, 0x10000, Prot::READ | Prot::WRITE)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to map zero-page: {:?}", e)))?;

        let mut file = File::open(path)?;
        let data_pages_base = lx_file.header.data_pages_offset as u64;

        for (i, obj) in lx_file.object_table.iter().enumerate() {
            println!("Mapping Object {} at 0x{:08X} (size 0x{:08X})...", i + 1, obj.base_address, obj.size);
            let mut perm = Prot::NONE;
            if (obj.flags & 0x0001) != 0 { perm |= Prot::READ; }
            if (obj.flags & 0x0002) != 0 { perm |= Prot::WRITE; }
            if (obj.flags & 0x0004) != 0 { perm |= Prot::EXEC; }
            let addr = obj.base_address as u64;
            let size = ((obj.size as u64 + 4095) & !4095) as u64;
            emu.mem_map(addr, size, perm).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("mmap failed: {:?}", e)))?;

            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.page_map.len() { break; }
                let page_entry = &lx_file.page_map[page_idx];
                let page_file_offset = data_pages_base + ((page_entry.data_offset as u64) << lx_file.header.page_offset_shift);
                let target_addr = obj.base_address as u64 + (p * 4096) as u64;
                if page_entry.data_size > 0 {
                    file.seek(SeekFrom::Start(page_file_offset))?;
                    let mut page_data = vec![0u8; page_entry.data_size as usize];
                    file.read_exact(&mut page_data)?;
                    emu.mem_write(target_addr, &page_data).unwrap();
                }
            }
        }
        self.apply_fixups(lx_file, &mut emu)?;
        Ok(emu)
    }

    fn apply_fixups(&self, lx_file: &LxFile, emu: &mut Unicorn<'static, ()>) -> io::Result<()> {
        for obj in &lx_file.object_table {
            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.fixup_records_by_page.len() { break; }
                let records = &lx_file.fixup_records_by_page[page_idx];
                for record in records {
                    let target_addr = match &record.target {
                        FixupTarget::Internal { object_num, target_offset } => {
                            let target_obj = lx_file.object_table.get((*object_num as usize).wrapping_sub(1));
                            target_obj.map(|to| to.base_address as u64 + *target_offset as u64).unwrap_or(0)
                        },
                        FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal } => {
                            let mod_name = lx_file.imported_modules.get((*module_ordinal as usize).wrapping_sub(1))
                                .map(|s| s.as_str()).unwrap_or("");
                            self.resolve_import(mod_name, *proc_ordinal)
                        },
                        _ => 0,
                    };
                    if target_addr == 0 { continue; }
                    for &source_offset in &record.source_offsets {
                        let source_addr = obj.base_address as u64 + (p * 4096) as u64 + source_offset as u64;
                        match record.source_type & 0x0F {
                            0x07 => {
                                let bytes = (target_addr as u32).to_le_bytes();
                                emu.mem_write(source_addr, &bytes).unwrap();
                            },
                            0x08 => {
                                let rel = (target_addr as i64) - (source_addr as i64 + 4);
                                let bytes = (rel as i32).to_le_bytes();
                                emu.mem_write(source_addr, &bytes).unwrap();
                            },
                            _ => {},
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_import(&self, module: &str, ordinal: u32) -> u64 {
        if module == "DOSCALLS" {
            match ordinal {
                282 => 0xFFFF0001,
                234 => 0xFFFF0002,
                348 => 0xFFFF0003,
                _ => 0,
            }
        } else { 0 }
    }

    pub fn run(&self, lx_file: &LxFile, emu: &mut Unicorn<'static, ()>) -> ! {
        let entry_obj = lx_file.object_table.get((lx_file.header.eip_object as usize).wrapping_sub(1)).unwrap();
        let entry_eip = entry_obj.base_address as u64 + lx_file.header.eip as u64;
        let stack_obj = lx_file.object_table.get((lx_file.header.esp_object as usize).wrapping_sub(1)).unwrap();
        let entry_esp = stack_obj.base_address as u64 + lx_file.header.esp as u64;

        // Map System Data area (including environment at 0x60000)
        emu.mem_map(0x50000, 0x20000, Prot::READ | Prot::WRITE).unwrap();
        let env_addr = 0x60000;
        let env_data = b"PATH=C:\\\0\0HELLO.EXE\0";
        emu.mem_write(env_addr, env_data).unwrap();
        let cmdline_addr = env_addr + 10;

        emu.reg_write(RegisterX86::EIP, entry_eip).unwrap();
        emu.reg_write(RegisterX86::ESP, entry_esp).unwrap();

        let mut esp = entry_esp;
        let mut push = |emu: &mut Unicorn<'static, ()>, val: u32| {
            esp -= 4;
            emu.mem_write(esp, &val.to_le_bytes()).unwrap();
        };
        push(emu, cmdline_addr as u32); 
        push(emu, env_addr as u32); 
        push(emu, 0); 
        push(emu, 1); 
        push(emu, 0xFFFFEEEE); 
        emu.reg_write(RegisterX86::ESP, esp).unwrap();

        emu.add_mem_hook(HookType::MEM_READ_UNMAPPED | HookType::MEM_WRITE_UNMAPPED, 0, 0xFFFFFFFF, |_emu, mem_type, addr, size, value| {
            println!("Unmapped memory access: {:?} at 0x{:08X} (size {}, value 0x{:08X})", mem_type, addr, size, value);
            true
        }).unwrap();

        emu.add_code_hook(0xFFFF0000, 0xFFFFFFFF, move |emu, addr, _size| {
            match addr {
                0xFFFF0001 => { 
                    let sp = emu.reg_read(RegisterX86::ESP).unwrap();
                    let mut buf4 = [0u8; 4];
                    emu.mem_read(sp + 4, &mut buf4).unwrap(); let fd = u32::from_le_bytes(buf4);
                    emu.mem_read(sp + 8, &mut buf4).unwrap(); let buf_ptr = u32::from_le_bytes(buf4);
                    emu.mem_read(sp + 12, &mut buf4).unwrap(); let len = u32::from_le_bytes(buf4);
                    emu.mem_read(sp + 16, &mut buf4).unwrap(); let actual_ptr = u32::from_le_bytes(buf4);
                    let mut data = vec![0u8; len as usize];
                    emu.mem_read(buf_ptr as u64, &mut data).unwrap();
                    let res = match api::doscalls::dos_write(fd, &data) {
                        Ok(actual) => {
                            if actual_ptr != 0 { emu.mem_write(actual_ptr as u64, &actual.to_le_bytes()).unwrap(); }
                            0
                        },
                        Err(_) => 1,
                    };
                    emu.reg_write(RegisterX86::EAX, res as u64).unwrap();
                    emu.mem_read(sp, &mut buf4).unwrap();
                    emu.reg_write(RegisterX86::EIP, u32::from_le_bytes(buf4) as u64).unwrap();
                    emu.reg_write(RegisterX86::ESP, sp + 20).unwrap();
                },
                0xFFFF0002 | 0xFFFFEEEE => { std::process::exit(0); },
                0xFFFF0003 => { 
                    let sp = emu.reg_read(RegisterX86::ESP).unwrap();
                    emu.reg_write(RegisterX86::EAX, 0).unwrap();
                    let mut buf4 = [0u8; 4];
                    emu.mem_read(sp, &mut buf4).unwrap();
                    emu.reg_write(RegisterX86::EIP, u32::from_le_bytes(buf4) as u64).unwrap();
                    emu.reg_write(RegisterX86::ESP, sp + 20).unwrap();
                }
                _ => {
                    emu.emu_stop().unwrap();
                }
            }
        }).unwrap();

        // Map Magic API area (for hooks to work, memory must be mapped and fetchable)
        emu.mem_map(0xFFFF0000, 4096, Prot::READ | Prot::EXEC).unwrap();
        let nops = [0x90u8; 4096];
        emu.mem_write(0xFFFF0000, &nops).unwrap();

        println!("Starting OS/2 Emulation...");
        if let Err(e) = emu.emu_start(entry_eip, 0, 0, 0) {
            println!("Emulation failed: {:?}", e);
            println!("  EIP: 0x{:08X}", emu.reg_read(RegisterX86::EIP).unwrap());
        }
        std::process::exit(0);
    }
}
