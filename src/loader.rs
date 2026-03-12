// SPDX-License-Identifier: GPL-3.0-only
use crate::lx::LxFile;
use crate::lx::header::FixupTarget;
use crate::api;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write, Seek, SeekFrom};
use std::path::Path;
use std::ptr;
use std::cell::RefCell;
use std::collections::HashMap;
use kvm_ioctls::{Kvm, VmFd, VcpuFd};
use kvm_bindings::{kvm_userspace_memory_region, kvm_guest_debug, KVM_GUESTDBG_ENABLE, KVM_GUESTDBG_USE_SW_BP};

const MAGIC_API_BASE: u64 = 0x01000000;
const DYNAMIC_ALLOC_BASE: u32 = 0x02000000; // 32MB

#[derive(Debug, Clone, Copy)]
struct AllocBlock {
    addr: u32,
    _size: u32,
}

pub struct MemoryManager {
    allocated: Vec<AllocBlock>,
    next_free: u32,
    limit: u32,
}

impl MemoryManager {
    pub fn new(base: u32, limit: u32) -> Self {
        MemoryManager {
            allocated: Vec::new(),
            next_free: base,
            limit,
        }
    }

    pub fn alloc(&mut self, size: u32) -> Option<u32> {
        let size = (size + 4095) & !4095;
        if self.next_free + size > self.limit {
            return None;
        }
        let addr = self.next_free;
        self.allocated.push(AllocBlock { addr, _size: size });
        self.next_free += size;
        Some(addr)
    }

    pub fn free(&mut self, addr: u32) -> bool {
        let len_before = self.allocated.len();
        self.allocated.retain(|b| b.addr != addr);
        self.allocated.len() < len_before
    }

    pub fn is_allocated(&self, addr: u32) -> bool {
        self.allocated.iter().any(|b| b.addr == addr)
    }
}

pub struct HandleManager {
    handles: HashMap<u32, File>,
    next_handle: u32,
}

impl HandleManager {
    pub fn new() -> Self {
        HandleManager {
            handles: HashMap::new(),
            next_handle: 3,
        }
    }

    pub fn add(&mut self, file: File) -> u32 {
        let h = self.next_handle;
        self.handles.insert(h, file);
        self.next_handle += 1;
        h
    }

    pub fn get_mut(&mut self, h: u32) -> Option<&mut File> {
        self.handles.get_mut(&h)
    }

    pub fn close(&mut self, h: u32) -> bool {
        self.handles.remove(&h).is_some()
    }
}

pub struct Loader {
    _kvm: Kvm,
    vm: VmFd,
    guest_mem: *mut u8,
    guest_mem_size: usize,
    pub mem_mgr: RefCell<MemoryManager>,
    pub handle_mgr: RefCell<HandleManager>,
}

impl Loader {
    pub fn new() -> Self {
        let kvm = Kvm::new().expect("Failed to open /dev/kvm");
        let vm = kvm.create_vm().expect("Failed to create VM");
        let guest_mem_size = 128 * 1024 * 1024;
        let guest_mem = unsafe {
            libc::mmap(ptr::null_mut(), guest_mem_size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE, -1, 0) as *mut u8
        };
        unsafe { ptr::write_bytes(guest_mem, 0, guest_mem_size); }
        let mem_region = kvm_userspace_memory_region { slot: 0, guest_phys_addr: 0, memory_size: guest_mem_size as u64, userspace_addr: guest_mem as u64, flags: 0 };
        unsafe { vm.set_user_memory_region(mem_region).unwrap(); }
        
        let mem_mgr = MemoryManager::new(DYNAMIC_ALLOC_BASE, guest_mem_size as u32);
        let handle_mgr = HandleManager::new();

        Loader { _kvm: kvm, vm, guest_mem, guest_mem_size, mem_mgr: RefCell::new(mem_mgr), handle_mgr: RefCell::new(handle_mgr) }
    }

    pub fn load<P: AsRef<Path>>(&mut self, lx_file: &LxFile, path: P) -> io::Result<()> {
        let mut file = File::open(path)?;
        let data_pages_base = lx_file.header.data_pages_offset as u64;
        for (i, obj) in lx_file.object_table.iter().enumerate() {
            println!("  Mapping Object {}...", i + 1);
            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.page_map.len() { break; }
                let page_off = data_pages_base + ((lx_file.page_map[page_idx].data_offset as u64) << lx_file.header.page_offset_shift);
                let target = obj.base_address as usize + (p * 4096);
                if lx_file.page_map[page_idx].data_size > 0 {
                    file.seek(SeekFrom::Start(page_off))?;
                    unsafe { file.read_exact(std::slice::from_raw_parts_mut(self.guest_mem.add(target), lx_file.page_map[page_idx].data_size as usize))?; }
                }
            }
        }
        self.apply_fixups(lx_file)
    }

    fn apply_fixups(&self, lx_file: &LxFile) -> io::Result<()> {
        for obj in &lx_file.object_table {
            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.fixup_records_by_page.len() { break; }
                for record in &lx_file.fixup_records_by_page[page_idx] {
                    let target_addr = match &record.target {
                        FixupTarget::Internal { object_num, target_offset } => lx_file.object_table[(*object_num as usize).wrapping_sub(1)].base_address as usize + *target_offset as usize,
                        FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal } => self.resolve_import(lx_file.imported_modules.get((*module_ordinal as usize).wrapping_sub(1)).unwrap(), *proc_ordinal) as usize,
                        _ => 0,
                    };
                    if target_addr == 0 { continue; }
                    for &off in &record.source_offsets {
                        let source_phys = obj.base_address as usize + p * 4096 + off as usize;
                        unsafe {
                            let ptr = self.guest_mem.add(source_phys);
                            if (record.source_type & 0x0F) == 0x07 { ptr::write_unaligned(ptr as *mut u32, target_addr as u32); }
                            else if (record.source_type & 0x0F) == 0x08 { ptr::write_unaligned(ptr as *mut i32, (target_addr as isize - (source_phys as isize + 4)) as i32); }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_import(&self, module: &str, ordinal: u32) -> u64 {
        if module == "DOSCALLS" { MAGIC_API_BASE + ordinal as u64 } else { 0 }
    }

    fn setup_stubs(&self) {
        for i in 0..1024 {
            unsafe {
                let ptr = self.guest_mem.add(MAGIC_API_BASE as usize + i);
                *ptr = 0xCC; // INT 3
            }
        }
    }

    pub fn run(self, lx_file: &LxFile) -> ! {
        let mut vcpu = self.vm.create_vcpu(0).unwrap();
        let mut sregs = vcpu.get_sregs().unwrap();
        sregs.cs.base = 0; sregs.cs.limit = 0xFFFFFFFF; sregs.cs.g = 1; sregs.cs.db = 1; sregs.cs.present = 1; sregs.cs.type_ = 11; sregs.cs.s = 1; sregs.cs.selector = 0x08;
        let mut ds = sregs.cs; ds.type_ = 3; ds.selector = 0x10;
        sregs.ds = ds; sregs.es = ds; sregs.gs = ds; sregs.ss = ds;
        let tib_base = 0x70000;
        let mut fs = ds; fs.base = tib_base; fs.limit = 0xFFF; fs.selector = 0x18; sregs.fs = fs;
        sregs.cr0 |= 1; vcpu.set_sregs(&sregs).unwrap();

        let env_addr = 0x60000;
        let pib_base = 0x71000;
        let cmdline_addr = env_addr + 10;
        let env_data = b"PATH=C:\\\0\0HELLO.EXE\0";
        unsafe { 
            ptr::copy_nonoverlapping(env_data.as_ptr(), self.guest_mem.add(env_addr), env_data.len());
            ptr::write_unaligned(self.guest_mem.add(tib_base as usize + 0x18) as *mut u32, tib_base as u32);
            ptr::write_unaligned(self.guest_mem.add(tib_base as usize + 0x30) as *mut u32, pib_base as u32);
            ptr::write_unaligned(self.guest_mem.add(pib_base as usize + 0x00) as *mut u32, 42); 
            ptr::write_unaligned(self.guest_mem.add(pib_base as usize + 0x0C) as *mut u32, env_addr as u32); 
            ptr::write_unaligned(self.guest_mem.add(pib_base as usize + 0x10) as *mut u32, cmdline_addr as u32);
        }

        let entry_eip = lx_file.object_table[lx_file.header.eip_object as usize - 1].base_address as u64 + lx_file.header.eip as u64;
        let entry_esp = lx_file.object_table[lx_file.header.esp_object as usize - 1].base_address as u64 + lx_file.header.esp as u64;
        let mut regs = vcpu.get_regs().unwrap();
        regs.rip = entry_eip; regs.rsp = entry_esp - 20; regs.rflags = 2;
        vcpu.set_regs(&regs).unwrap();

        unsafe {
            let sp = self.guest_mem.add(regs.rsp as usize) as *mut u32;
            ptr::write_unaligned(sp.offset(0), 0xFFFFEEEE); 
            ptr::write_unaligned(sp.offset(1), 1); 
            ptr::write_unaligned(sp.offset(2), 0); 
            ptr::write_unaligned(sp.offset(3), env_addr as u32);
            ptr::write_unaligned(sp.offset(4), cmdline_addr as u32);
        }

        self.setup_stubs();
        
        let debug = kvm_guest_debug { control: KVM_GUESTDBG_ENABLE | KVM_GUESTDBG_USE_SW_BP, ..Default::default() };
        vcpu.set_guest_debug(&debug).unwrap();

        println!("Starting OS/2 KVM Hypervisor at 0x{:08X}...", entry_eip);
        let vcpu_ptr = &mut vcpu as *mut VcpuFd;
        loop {
            let exit = unsafe { (*vcpu_ptr).run().unwrap() };
            match exit {
                kvm_ioctls::VcpuExit::Debug(_) => {
                    let rip = unsafe { (*vcpu_ptr).get_regs().unwrap().rip };
                    if rip >= MAGIC_API_BASE && rip < MAGIC_API_BASE + 1024 {
                        self.handle_api_call(unsafe { &mut *vcpu_ptr }, (rip - MAGIC_API_BASE) as u32);
                    } else if rip == 0xFFFFEEEE { println!("Guest returned to loader. Exiting."); std::process::exit(0); }
                    else { println!("Guest breakpoint at EIP=0x{:08X}.", rip); std::process::exit(0); }
                }
                kvm_ioctls::VcpuExit::Hlt => { println!("Guest HLT."); std::process::exit(0); }
                _ => { println!("Unhandled VMEXIT: {:?} at EIP=0x{:08X}", exit, unsafe { (*vcpu_ptr).get_regs().unwrap().rip }); std::process::exit(1); }
            }
        }
    }

    fn handle_api_call(&self, vcpu: &mut VcpuFd, ordinal: u32) {
        let mut regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| unsafe { ptr::read_unaligned(self.guest_mem.add((esp + off) as usize) as *const u32) };
        
        let stack_cleanup = 4;

        match ordinal {
            257 => { // DosClose
                let hf = read_stack(4);
                self.handle_mgr.borrow_mut().close(hf);
                regs.rax = 0;
            },
            273 => { // DosOpen
                let psz_name_ptr = read_stack(4);
                let phf_ptr = read_stack(8);
                let pul_action_ptr = read_stack(12);
                let _cb_file = read_stack(16);
                let _ul_attr = read_stack(20);
                let fs_open_flags = read_stack(24);
                let fs_open_mode = read_stack(28);

                unsafe {
                    let name_ptr = self.guest_mem.add(psz_name_ptr as usize);
                    let mut name = String::new();
                    let mut i = 0;
                    while *name_ptr.add(i) != 0 {
                        name.push(*name_ptr.add(i) as char);
                        i += 1;
                    }
                    let path = name.replace('\\', "/");
                    
                    let mut options = OpenOptions::new();
                    match fs_open_mode & 0x07 {
                        0 => { options.read(true); },
                        1 => { options.write(true); },
                        2 => { options.read(true).write(true); },
                        _ => {},
                    }
                    
                    let action_if_exists = fs_open_flags & 0x03;
                    let action_if_new = (fs_open_flags >> 4) & 0x03;

                    if action_if_new == 1 { options.create(true); }
                    if action_if_exists == 2 { options.truncate(true); }

                    match options.open(&path) {
                        Ok(file) => {
                            let h = self.handle_mgr.borrow_mut().add(file);
                            ptr::write_unaligned(self.guest_mem.add(phf_ptr as usize) as *mut u32, h);
                            if pul_action_ptr != 0 {
                                ptr::write_unaligned(self.guest_mem.add(pul_action_ptr as usize) as *mut u32, 1);
                            }
                            regs.rax = 0;
                        },
                        Err(_) => {
                            regs.rax = 2;
                        }
                    }
                }
            },
            281 => { // DosRead
                let hf = read_stack(4);
                let buf_ptr = read_stack(8);
                let len = read_stack(12);
                let actual_ptr = read_stack(16);

                let mut h_mgr = self.handle_mgr.borrow_mut();
                if let Some(file) = h_mgr.get_mut(hf) {
                    let mut data = vec![0u8; len as usize];
                    match file.read(&mut data) {
                        Ok(n) => {
                            unsafe {
                                ptr::copy_nonoverlapping(data.as_ptr(), self.guest_mem.add(buf_ptr as usize), n);
                                if actual_ptr != 0 {
                                    ptr::write_unaligned(self.guest_mem.add(actual_ptr as usize) as *mut u32, n as u32);
                                }
                            }
                            regs.rax = 0;
                        },
                        Err(_) => regs.rax = 5,
                    }
                } else if hf == 0 { regs.rax = 0; } else { regs.rax = 6; }
            },
            282 => { // DosWrite
                let fd = read_stack(4); let buf_ptr = read_stack(8); let len = read_stack(12); let actual_ptr = read_stack(16);
                let res = if fd == 1 || fd == 2 {
                    unsafe {
                        let data = std::slice::from_raw_parts(self.guest_mem.add(buf_ptr as usize), len as usize);
                        match api::doscalls::dos_write(fd, data) {
                            Ok(actual) => {
                                if actual_ptr != 0 { ptr::write_unaligned(self.guest_mem.add(actual_ptr as usize) as *mut u32, actual); }
                                0
                            },
                            Err(_) => 1,
                        }
                    }
                } else {
                    let mut h_mgr = self.handle_mgr.borrow_mut();
                    if let Some(file) = h_mgr.get_mut(fd) {
                        unsafe {
                            let data = std::slice::from_raw_parts(self.guest_mem.add(buf_ptr as usize), len as usize);
                            match file.write(data) {
                                Ok(n) => {
                                    if actual_ptr != 0 { ptr::write_unaligned(self.guest_mem.add(actual_ptr as usize) as *mut u32, n as u32); }
                                    0
                                },
                                Err(_) => 5,
                            }
                        }
                    } else { 6 }
                };
                regs.rax = res as u64;
            },
            234 => { api::doscalls::dos_exit(read_stack(4), read_stack(8)); },
            235 => { // DosQueryHType
                let hfile = read_stack(4); let ptype = read_stack(8); let pattr = read_stack(12);
                unsafe {
                    if ptype != 0 { ptr::write_unaligned(self.guest_mem.add(ptype as usize) as *mut u32, if hfile < 3 { 1 } else { 0 }); }
                    if pattr != 0 { ptr::write_unaligned(self.guest_mem.add(pattr as usize) as *mut u32, 0); }
                }
                regs.rax = 0;
            },
            283 => { // DosGetInfoBlocks
                let ptib = read_stack(4); let ppib = read_stack(8);
                unsafe {
                    if ptib != 0 { ptr::write_unaligned(self.guest_mem.add(ptib as usize) as *mut u32, 0x70000); }
                    if ppib != 0 { ptr::write_unaligned(self.guest_mem.add(ppib as usize) as *mut u32, 0x71000); }
                }
                regs.rax = 0;
            },
            299 => { // DosAllocMem
                let ppb = read_stack(4); let cb = read_stack(8);
                match self.mem_mgr.borrow_mut().alloc(cb) {
                    Some(addr) => {
                        unsafe { ptr::write_unaligned(self.guest_mem.add(ppb as usize) as *mut u32, addr); }
                        regs.rax = 0;
                    },
                    None => regs.rax = 8,
                }
            },
            304 => { // DosFreeMem
                let pb = read_stack(4);
                if self.mem_mgr.borrow_mut().free(pb) { regs.rax = 0; }
                else { regs.rax = 487; }
            },
            305 => { // DosSetMem
                regs.rax = 0;
            },
            348 => { regs.rax = 0; },
            349 => { regs.rax = 0; }
            _ => { println!("Warning: Unknown API Ordinal {}", ordinal); regs.rax = 0; }
        }
        if stack_cleanup > 0 {
            regs.rip = read_stack(0) as u64;
            regs.rsp += stack_cleanup as u64;
        }
        vcpu.set_regs(&regs).unwrap();
    }
}

impl Drop for Loader {
    fn drop(&mut self) { unsafe { libc::munmap(self.guest_mem as *mut libc::c_void, self.guest_mem_size); } }
}
