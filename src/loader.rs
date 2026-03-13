// SPDX-License-Identifier: GPL-3.0-only
use crate::lx::LxFile;
use crate::lx::header::FixupTarget;
use crate::api;
use std::fs::{File, OpenOptions, ReadDir};
use std::io::{self, Read, Write, Seek, SeekFrom};
use std::path::Path;
use std::ptr;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Condvar};
use std::thread;
use kvm_ioctls::{Kvm, VmFd, VcpuFd};
use kvm_bindings::{kvm_userspace_memory_region, kvm_guest_debug, KVM_GUESTDBG_ENABLE, KVM_GUESTDBG_USE_SW_BP};

const MAGIC_API_BASE: u64 = 0x01000000;
const EXIT_TRAP_ADDR: u32 = 0x010003FF;
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

pub struct EventSemaphore {
    pub posted: bool,
    _attr: u32,
    _name: Option<String>,
}

pub struct MutexSemaphore {
    pub owner_tid: Option<u32>,
    pub request_count: u32,
    _attr: u32,
    _name: Option<String>,
}

pub struct SemaphoreManager {
    event_sems: HashMap<u32, Arc<(Mutex<EventSemaphore>, Condvar)>>,
    mutex_sems: HashMap<u32, Arc<(Mutex<MutexSemaphore>, Condvar)>>,
    next_handle: u32,
}

impl SemaphoreManager {
    pub fn new() -> Self {
        SemaphoreManager {
            event_sems: HashMap::new(),
            mutex_sems: HashMap::new(),
            next_handle: 1,
        }
    }

    pub fn create_event(&mut self, name: Option<String>, attr: u32, posted: bool) -> u32 {
        let h = self.next_handle;
        self.event_sems.insert(h, Arc::new((Mutex::new(EventSemaphore { posted, _attr: attr, _name: name }), Condvar::new())));
        self.next_handle += 1;
        h
    }

    pub fn get_event(&self, h: u32) -> Option<Arc<(Mutex<EventSemaphore>, Condvar)>> {
        self.event_sems.get(&h).cloned()
    }

    pub fn close_event(&mut self, h: u32) -> bool {
        self.event_sems.remove(&h).is_some()
    }

    pub fn create_mutex(&mut self, name: Option<String>, attr: u32, state: bool) -> u32 {
        let h = self.next_handle;
        let owner_tid = if state { Some(0) } else { None };
        let request_count = if state { 1 } else { 0 };
        self.mutex_sems.insert(h, Arc::new((Mutex::new(MutexSemaphore { owner_tid, request_count, _attr: attr, _name: name }), Condvar::new())));
        self.next_handle += 1;
        h
    }

    pub fn get_mutex(&self, h: u32) -> Option<Arc<(Mutex<MutexSemaphore>, Condvar)>> {
        self.mutex_sems.get(&h).cloned()
    }

    pub fn close_mutex(&mut self, h: u32) -> bool {
        self.mutex_sems.remove(&h).is_some()
    }
}

pub struct HDirManager {
    iterators: HashMap<u32, ReadDir>,
    next_handle: u32,
}

impl HDirManager {
    pub fn new() -> Self {
        HDirManager {
            iterators: HashMap::new(),
            next_handle: 10,
        }
    }

    pub fn add(&mut self, it: ReadDir) -> u32 {
        let h = self.next_handle;
        self.iterators.insert(h, it);
        self.next_handle += 1;
        h
    }

    pub fn get_mut(&mut self, h: u32) -> Option<&mut ReadDir> {
        self.iterators.get_mut(&h)
    }

    pub fn close(&mut self, h: u32) -> bool {
        self.iterators.remove(&h).is_some()
    }
}

pub struct SharedState {
    pub mem_mgr: Mutex<MemoryManager>,
    pub handle_mgr: Mutex<HandleManager>,
    pub sem_mgr: Mutex<SemaphoreManager>,
    pub hdir_mgr: Mutex<HDirManager>,
    pub guest_mem: *mut u8,
    pub guest_mem_size: usize,
    pub next_tid: Mutex<u32>,
    pub threads: Mutex<HashMap<u32, thread::JoinHandle<()>>>,
}

unsafe impl Send for SharedState {}
unsafe impl Sync for SharedState {}

pub struct Loader {
    _kvm: Kvm,
    vm: Arc<VmFd>,
    shared: Arc<SharedState>,
}

impl Loader {
    pub fn new() -> Self {
        let kvm = Kvm::new().expect("Failed to open /dev/kvm");
        let vm = Arc::new(kvm.create_vm().expect("Failed to create VM"));
        let guest_mem_size = 128 * 1024 * 1024;
        let guest_mem = unsafe {
            libc::mmap(ptr::null_mut(), guest_mem_size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE, -1, 0) as *mut u8
        };
        unsafe { ptr::write_bytes(guest_mem, 0, guest_mem_size); }
        let mem_region = kvm_userspace_memory_region { slot: 0, guest_phys_addr: 0, memory_size: guest_mem_size as u64, userspace_addr: guest_mem as u64, flags: 0 };
        unsafe { vm.set_user_memory_region(mem_region).unwrap(); }
        
        let mem_mgr = MemoryManager::new(DYNAMIC_ALLOC_BASE, guest_mem_size as u32);
        let handle_mgr = HandleManager::new();
        let sem_mgr = SemaphoreManager::new();
        let hdir_mgr = HDirManager::new();

        let shared = Arc::new(SharedState {
            mem_mgr: Mutex::new(mem_mgr),
            handle_mgr: Mutex::new(handle_mgr),
            sem_mgr: Mutex::new(sem_mgr),
            hdir_mgr: Mutex::new(hdir_mgr),
            guest_mem,
            guest_mem_size,
            next_tid: Mutex::new(1),
            threads: Mutex::new(HashMap::new()),
        });

        Loader { _kvm: kvm, vm, shared }
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
                    unsafe { file.read_exact(std::slice::from_raw_parts_mut(self.shared.guest_mem.add(target), lx_file.page_map[page_idx].data_size as usize))?; }
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
                            let ptr = self.shared.guest_mem.add(source_phys);
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
                let ptr = self.shared.guest_mem.add(MAGIC_API_BASE as usize + i);
                *ptr = 0xCC; // INT 3
            }
        }
    }

    pub fn run(self, lx_file: &LxFile) -> ! {
        let entry_eip = lx_file.object_table[lx_file.header.eip_object as usize - 1].base_address as u64 + lx_file.header.eip as u64;
        let entry_esp = lx_file.object_table[lx_file.header.esp_object as usize - 1].base_address as u64 + lx_file.header.esp as u64;
        
        let tib_base = 0x70000;
        let pib_base = 0x71000;
        let env_addr = 0x60000;
        let cmdline_addr = env_addr + 10;
        let env_data = b"PATH=C:\\\0\0HELLO.EXE\0";
        unsafe { 
            ptr::copy_nonoverlapping(env_data.as_ptr(), self.shared.guest_mem.add(env_addr), env_data.len());
            ptr::write_unaligned(self.shared.guest_mem.add(tib_base as usize + 0x18) as *mut u32, tib_base as u32);
            ptr::write_unaligned(self.shared.guest_mem.add(tib_base as usize + 0x30) as *mut u32, pib_base as u32);
            ptr::write_unaligned(self.shared.guest_mem.add(pib_base as usize + 0x00) as *mut u32, 42); 
            ptr::write_unaligned(self.shared.guest_mem.add(pib_base as usize + 0x0C) as *mut u32, env_addr as u32); 
            ptr::write_unaligned(self.shared.guest_mem.add(pib_base as usize + 0x10) as *mut u32, cmdline_addr as u32);
        }

        self.setup_stubs();

        let vcpu = self.vm.create_vcpu(0).unwrap();
        let mut regs = vcpu.get_regs().unwrap();
        regs.rip = entry_eip; regs.rsp = entry_esp - 20; regs.rflags = 2;
        vcpu.set_regs(&regs).unwrap();

        unsafe {
            let sp = self.shared.guest_mem.add(regs.rsp as usize) as *mut u32;
            ptr::write_unaligned(sp.offset(0), EXIT_TRAP_ADDR); 
            ptr::write_unaligned(sp.offset(1), 1); 
            ptr::write_unaligned(sp.offset(2), 0); 
            ptr::write_unaligned(sp.offset(3), env_addr as u32);
            ptr::write_unaligned(sp.offset(4), cmdline_addr as u32);
        }

        self.run_vcpu(vcpu, 0, tib_base as u64);
        std::process::exit(0);
    }

    fn run_vcpu(&self, mut vcpu: VcpuFd, vcpu_id: u32, tib_base: u64) {
        let mut sregs = vcpu.get_sregs().unwrap();
        sregs.cs.base = 0; sregs.cs.limit = 0xFFFFFFFF; sregs.cs.g = 1; sregs.cs.db = 1; sregs.cs.present = 1; sregs.cs.type_ = 11; sregs.cs.s = 1; sregs.cs.selector = 0x08;
        let mut ds = sregs.cs; ds.type_ = 3; ds.selector = 0x10;
        sregs.ds = ds; sregs.es = ds; sregs.gs = ds; sregs.ss = ds;
        let mut fs = ds; fs.base = tib_base; fs.limit = 0xFFF; fs.selector = 0x18; sregs.fs = fs;
        sregs.cr0 |= 1; vcpu.set_sregs(&sregs).unwrap();

        let debug = kvm_guest_debug { control: KVM_GUESTDBG_ENABLE | KVM_GUESTDBG_USE_SW_BP, ..Default::default() };
        vcpu.set_guest_debug(&debug).unwrap();

        println!("  [VCPU {}] Started at EIP=0x{:08X}", vcpu_id, vcpu.get_regs().unwrap().rip);

        loop {
            let res = vcpu.run();
            if let Err(e) = res {
                println!("  [VCPU {}] KVM Run failed: {}", vcpu_id, e);
                std::process::exit(1);
            }
            let exit = res.unwrap();
            match exit {
                kvm_ioctls::VcpuExit::Debug(_) => {
                    let rip = vcpu.get_regs().unwrap().rip;
                    if rip >= MAGIC_API_BASE && rip < MAGIC_API_BASE + 1024 {
                        if rip == EXIT_TRAP_ADDR as u64 {
                            println!("  [VCPU {}] Guest requested thread exit.", vcpu_id);
                            if vcpu_id == 0 { std::process::exit(0); }
                            else { return; }
                        }
                        self.handle_api_call(&mut vcpu, vcpu_id, (rip - MAGIC_API_BASE) as u32);
                    }
                    else {
                        println!("  [VCPU {}] Guest breakpoint at EIP=0x{:08X}.", vcpu_id, rip);
                        if vcpu_id == 0 { std::process::exit(0); }
                        else { return; }
                    }
                }
                kvm_ioctls::VcpuExit::Hlt => {
                    println!("  [VCPU {}] Guest HLT.", vcpu_id);
                    std::process::exit(0);
                }
                _ => {
                    let e = format!("{:?}", exit);
                    let rip = vcpu.get_regs().unwrap().rip;
                    println!("  [VCPU {}] Unhandled VMEXIT: {} at EIP=0x{:08X}", vcpu_id, e, rip);
                    std::process::exit(1);
                }
            }
        }
    }

    fn handle_api_call(&self, vcpu: &mut VcpuFd, vcpu_id: u32, ordinal: u32) {
        let mut regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| unsafe { ptr::read_unaligned(self.shared.guest_mem.add((esp + off) as usize) as *const u32) };
        
        println!("  [VCPU {}] API Call: Ordinal {} (ReturnAddr=0x{:08X})", vcpu_id, ordinal, read_stack(0));

        let res = match ordinal {
            256 => self.dos_set_file_ptr(read_stack(4), read_stack(8) as i32, read_stack(12), read_stack(16)),
            257 => self.dos_close(read_stack(4)),
            273 => self.dos_open(read_stack(4), read_stack(8), read_stack(12), read_stack(24), read_stack(28)),
            281 => self.dos_read(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
            282 => self.dos_write(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
            229 => self.dos_sleep(read_stack(4)),
            311 => self.dos_create_thread(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(20)),
            234 => { api::doscalls::dos_exit(read_stack(4), read_stack(8)); },
            235 => self.dos_query_h_type(read_stack(4), read_stack(8), read_stack(12)),
            283 => self.dos_get_info_blocks(vcpu, read_stack(4), read_stack(8)),
            271 => self.dos_find_first(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
            272 => self.dos_find_next(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
            275 => self.dos_query_path_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
            278 => self.dos_query_file_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
            299 => self.dos_alloc_mem(read_stack(4), read_stack(8)),
            304 => self.dos_free_mem(read_stack(4)),
            324 => self.dos_create_event_sem(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
            326 => self.dos_close_event_sem(read_stack(4)),
            328 => self.dos_post_event_sem(read_stack(4)),
            329 => self.dos_wait_event_sem(read_stack(4), read_stack(8)),
            331 => self.dos_create_mutex_sem(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
            333 => self.dos_close_mutex_sem(read_stack(4)),
            334 => self.dos_request_mutex_sem(vcpu_id, read_stack(4), read_stack(8)),
            335 => self.dos_release_mutex_sem(vcpu_id, read_stack(4)),
            348 => 0,
            349 => self.dos_wait_thread(vcpu_id, read_stack(4)),
            _ => { println!("Warning: Unknown API Ordinal {} on VCPU {}", ordinal, vcpu_id); 0 }
        };

        regs.rax = res as u64;
        
        // POP RETURN ADDRESS
        regs.rip = read_stack(0) as u64;
        regs.rsp += 4;
        
        vcpu.set_regs(&regs).unwrap();
    }

    // --- API Handlers ---

    fn dos_close(&self, hf: u32) -> u32 {
        self.shared.handle_mgr.lock().unwrap().close(hf);
        0
    }

    fn dos_set_file_ptr(&self, hf: u32, offset: i32, method: u32, actual_ptr: u32) -> u32 {
        let mut h_mgr = self.shared.handle_mgr.lock().unwrap();
        if let Some(file) = h_mgr.get_mut(hf) {
            let pos = match method {
                0 => SeekFrom::Start(offset as u64),
                1 => SeekFrom::Current(offset as i64),
                2 => SeekFrom::End(offset as i64),
                _ => return 1, // ERROR_INVALID_FUNCTION
            };
            match file.seek(pos) {
                Ok(new_pos) => {
                    if actual_ptr != 0 {
                        unsafe { ptr::write_unaligned(self.shared.guest_mem.add(actual_ptr as usize) as *mut u32, new_pos as u32); }
                    }
                    0
                }
                Err(_) => 1,
            }
        } else { 6 } // ERROR_INVALID_HANDLE
    }

    fn dos_open(&self, psz_name_ptr: u32, phf_ptr: u32, pul_action_ptr: u32, fs_open_flags: u32, fs_open_mode: u32) -> u32 {
        unsafe {
            let name_ptr = self.shared.guest_mem.add(psz_name_ptr as usize);
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
                    let h = self.shared.handle_mgr.lock().unwrap().add(file);
                    ptr::write_unaligned(self.shared.guest_mem.add(phf_ptr as usize) as *mut u32, h);
                    if pul_action_ptr != 0 {
                        ptr::write_unaligned(self.shared.guest_mem.add(pul_action_ptr as usize) as *mut u32, 1);
                    }
                    0
                },
                Err(_) => 2,
            }
        }
    }

    fn dos_read(&self, hf: u32, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        let mut h_mgr = self.shared.handle_mgr.lock().unwrap();
        if let Some(file) = h_mgr.get_mut(hf) {
            let mut data = vec![0u8; len as usize];
            match file.read(&mut data) {
                Ok(n) => {
                    unsafe {
                        ptr::copy_nonoverlapping(data.as_ptr(), self.shared.guest_mem.add(buf_ptr as usize), n);
                        if actual_ptr != 0 {
                            ptr::write_unaligned(self.shared.guest_mem.add(actual_ptr as usize) as *mut u32, n as u32);
                        }
                    }
                    0
                },
                Err(_) => 5,
            }
        } else if hf == 0 { 0 } else { 6 }
    }

    fn dos_write(&self, fd: u32, buf_ptr: u32, len: u32, actual_ptr: u32) -> u32 {
        if fd == 1 || fd == 2 {
            unsafe {
                let data = std::slice::from_raw_parts(self.shared.guest_mem.add(buf_ptr as usize), len as usize);
                match api::doscalls::dos_write(fd, data) {
                    Ok(actual) => {
                        if actual_ptr != 0 { ptr::write_unaligned(self.shared.guest_mem.add(actual_ptr as usize) as *mut u32, actual); }
                        0
                    },
                    Err(_) => 1,
                }
            }
        } else {
            let mut h_mgr = self.shared.handle_mgr.lock().unwrap();
            if let Some(file) = h_mgr.get_mut(fd) {
                unsafe {
                    let data = std::slice::from_raw_parts(self.shared.guest_mem.add(buf_ptr as usize), len as usize);
                    match file.write(data) {
                        Ok(n) => {
                            if actual_ptr != 0 { ptr::write_unaligned(self.shared.guest_mem.add(actual_ptr as usize) as *mut u32, n as u32); }
                            0
                        },
                        Err(_) => 5,
                    }
                }
            } else { 6 }
        }
    }

    fn dos_sleep(&self, msec: u32) -> u32 {
        thread::sleep(std::time::Duration::from_millis(msec as u64));
        0
    }

    fn dos_create_thread(&self, vcpu_id: u32, ptid_ptr: u32, pfn: u32, param: u32, cb_stack: u32) -> u32 {
        let stack_size = if cb_stack == 0 { 65536 } else { cb_stack };
        let mut mem_mgr = self.shared.mem_mgr.lock().unwrap();
        if let Some(stack_base) = mem_mgr.alloc(stack_size) {
            let tib_addr = mem_mgr.alloc(4096).unwrap();
            let tid = {
                let mut next_tid = self.shared.next_tid.lock().unwrap();
                let tid = *next_tid;
                *next_tid += 1;
                tid
            };
            println!("  [VCPU {}] Creating thread {} (ptid_ptr=0x{:08X}, pfn=0x{:08X}, param=0x{:08X})", vcpu_id, tid, ptid_ptr, pfn, param);

            unsafe {
                ptr::write_unaligned(self.shared.guest_mem.add(tib_addr as usize + 0x18) as *mut u32, tib_addr);
                ptr::write_unaligned(self.shared.guest_mem.add(tib_addr as usize + 0x30) as *mut u32, 0x71000);
                
                let sp = self.shared.guest_mem.add((stack_base + stack_size) as usize - 12) as *mut u32;
                ptr::write_unaligned(sp.offset(0), EXIT_TRAP_ADDR); // Exit return
                ptr::write_unaligned(sp.offset(1), param);
                
                let vm_clone = Arc::clone(&self.vm);
                let shared_clone = Arc::clone(&self.shared);
                let new_vcpu = vm_clone.create_vcpu(tid as u64).unwrap();
                let mut new_regs = new_vcpu.get_regs().unwrap();
                new_regs.rip = pfn as u64;
                new_regs.rsp = (stack_base + stack_size - 12) as u64;
                new_regs.rax = param as u64; // For _Optlink
                new_regs.rflags = 2;
                new_vcpu.set_regs(&new_regs).unwrap();

                let handle = thread::spawn(move || {
                    println!("  [THREAD {}] Host thread spawned.", tid);
                    let loader = Loader { _kvm: Kvm::new().unwrap(), vm: vm_clone, shared: shared_clone };
                    loader.run_vcpu(new_vcpu, tid, tib_addr as u64);
                });
                self.shared.threads.lock().unwrap().insert(tid, handle);
                ptr::write_unaligned(self.shared.guest_mem.add(ptid_ptr as usize) as *mut u32, tid);
            }
            0
        } else { 8 }
    }

    fn dos_query_h_type(&self, hfile: u32, ptype: u32, pattr: u32) -> u32 {
        unsafe {
            if ptype != 0 { ptr::write_unaligned(self.shared.guest_mem.add(ptype as usize) as *mut u32, if hfile < 3 { 1 } else { 0 }); }
            if pattr != 0 { ptr::write_unaligned(self.shared.guest_mem.add(pattr as usize) as *mut u32, 0); }
        }
        0
    }

    fn dos_get_info_blocks(&self, vcpu: &VcpuFd, ptib: u32, ppib: u32) -> u32 {
        let fs_base = vcpu.get_sregs().unwrap().fs.base;
        unsafe {
            if ptib != 0 { ptr::write_unaligned(self.shared.guest_mem.add(ptib as usize) as *mut u32, fs_base as u32); }
            if ppib != 0 { ptr::write_unaligned(self.shared.guest_mem.add(ppib as usize) as *mut u32, 0x71000); }
        }
        0
    }

    fn dos_find_first(&self, psz_spec_ptr: u32, phdir_ptr: u32, _attr: u32, buf_ptr: u32, buf_len: u32, pc_found_ptr: u32, level: u32) -> u32 {
        if level != 1 { return 124; }
        unsafe {
            let spec_ptr = self.shared.guest_mem.add(psz_spec_ptr as usize);
            let mut spec = String::new();
            let mut i = 0;
            while *spec_ptr.add(i) != 0 {
                spec.push(*spec_ptr.add(i) as char);
                i += 1;
            }
            let path = spec.replace('\\', "/");
            // Simplified: just use the directory of the spec
            let dir_path = Path::new(&path).parent().unwrap_or(Path::new("."));
            if let Ok(rd) = std::fs::read_dir(dir_path) {
                let mut hdir_mgr = self.shared.hdir_mgr.lock().unwrap();
                let hdir = hdir_mgr.add(rd);
                ptr::write_unaligned(self.shared.guest_mem.add(phdir_ptr as usize) as *mut u32, hdir);
                
                // Read the first entry
                return self.dos_find_next(hdir, buf_ptr, buf_len, pc_found_ptr);
            }
        }
        3 // ERROR_PATH_NOT_FOUND
    }

    fn dos_find_next(&self, hdir: u32, buf_ptr: u32, buf_len: u32, pc_found_ptr: u32) -> u32 {
        let mut hdir_mgr = self.shared.hdir_mgr.lock().unwrap();
        if let Some(rd) = hdir_mgr.get_mut(hdir) {
            if let Some(Ok(entry)) = rd.next() {
                if let Ok(meta) = entry.metadata() {
                    let name = entry.file_name().into_string().unwrap_or_default();
                    let name_bytes = name.as_bytes();
                    let name_len = name_bytes.len().min(255);
                    
                    if buf_len < (32 + name_len as u32 + 1) { return 111; }
                    
                    unsafe {
                        let ptr = self.shared.guest_mem.add(buf_ptr as usize);
                        ptr::write_unaligned(ptr.add(0) as *mut u32, 0); // oNextEntryOffset (simplified)
                        self.write_filestatus3_internal(&meta, ptr.add(4));
                        *ptr.add(24) = name_len as u8;
                        ptr::copy_nonoverlapping(name_bytes.as_ptr(), ptr.add(25), name_len);
                        *ptr.add(25 + name_len) = 0;
                        
                        if pc_found_ptr != 0 {
                            ptr::write_unaligned(self.shared.guest_mem.add(pc_found_ptr as usize) as *mut u32, 1);
                        }
                    }
                    return 0;
                }
            }
            return 18; // ERROR_NO_MORE_FILES
        }
        6 // ERROR_INVALID_HANDLE
    }

    fn dos_query_file_info(&self, hf: u32, level: u32, buf_ptr: u32, buf_len: u32) -> u32 {
        if level != 1 { return 124; }
        if buf_len < 22 { return 111; }
        let mut h_mgr = self.shared.handle_mgr.lock().unwrap();
        if let Some(file) = h_mgr.get_mut(hf) {
            if let Ok(meta) = file.metadata() {
                unsafe { self.write_filestatus3_internal(&meta, self.shared.guest_mem.add(buf_ptr as usize)); }
                return 0;
            }
        }
        6
    }

    fn dos_query_path_info(&self, psz_path_ptr: u32, level: u32, buf_ptr: u32, buf_len: u32) -> u32 {
        if level != 1 { return 124; }
        if buf_len < 22 { return 111; }
        unsafe {
            let name_ptr = self.shared.guest_mem.add(psz_path_ptr as usize);
            let mut name = String::new();
            let mut i = 0;
            while *name_ptr.add(i) != 0 {
                name.push(*name_ptr.add(i) as char);
                i += 1;
            }
            let path = name.replace('\\', "/");
            if let Ok(meta) = std::fs::metadata(&path) {
                self.write_filestatus3_internal(&meta, self.shared.guest_mem.add(buf_ptr as usize));
                return 0;
            }
        }
        3
    }

    unsafe fn write_filestatus3_internal(&self, meta: &std::fs::Metadata, ptr: *mut u8) {
        let dos_date = 0x21; // 1980-01-01
        let dos_time = 0;
        unsafe {
            ptr::write_unaligned(ptr.add(0) as *mut u16, dos_date);
            ptr::write_unaligned(ptr.add(2) as *mut u16, dos_time);
            ptr::write_unaligned(ptr.add(4) as *mut u16, dos_date);
            ptr::write_unaligned(ptr.add(6) as *mut u16, dos_time);
            ptr::write_unaligned(ptr.add(8) as *mut u16, dos_date);
            ptr::write_unaligned(ptr.add(10) as *mut u16, dos_time);
            ptr::write_unaligned(ptr.add(12) as *mut u32, meta.len() as u32);
            ptr::write_unaligned(ptr.add(16) as *mut u32, meta.len() as u32);
            let attr = if meta.is_dir() { 0x10 } else { 0x00 };
            ptr::write_unaligned(ptr.add(20) as *mut u32, attr);
        }
    }

    fn dos_alloc_mem(&self, ppb: u32, cb: u32) -> u32 {
        match self.shared.mem_mgr.lock().unwrap().alloc(cb) {
            Some(addr) => {
                unsafe { ptr::write_unaligned(self.shared.guest_mem.add(ppb as usize) as *mut u32, addr); }
                0
            },
            None => 8,
        }
    }

    fn dos_free_mem(&self, pb: u32) -> u32 {
        if self.shared.mem_mgr.lock().unwrap().free(pb) { 0 }
        else { 487 }
    }

    fn dos_create_event_sem(&self, _psz_name_ptr: u32, phev_ptr: u32, fl_attr: u32, f_state: u32) -> u32 {
        let mut sem_mgr = self.shared.sem_mgr.lock().unwrap();
        let h = sem_mgr.create_event(None, fl_attr, f_state != 0);
        unsafe { ptr::write_unaligned(self.shared.guest_mem.add(phev_ptr as usize) as *mut u32, h); }
        0
    }

    fn dos_close_event_sem(&self, hev: u32) -> u32 {
        if self.shared.sem_mgr.lock().unwrap().close_event(hev) { 0 }
        else { 6 }
    }

    fn dos_post_event_sem(&self, hev: u32) -> u32 {
        let sem_mgr = self.shared.sem_mgr.lock().unwrap();
        if let Some(sem_arc) = sem_mgr.get_event(hev) {
            let (lock, cvar) = &*sem_arc;
            let mut sem = lock.lock().unwrap();
            if sem.posted { 299 } // ERROR_ALREADY_POSTED
            else {
                sem.posted = true;
                cvar.notify_all();
                0
            }
        } else { 6 }
    }

    fn dos_wait_event_sem(&self, hev: u32, _msec: u32) -> u32 {
        let sem_arc = self.shared.sem_mgr.lock().unwrap().get_event(hev);
        if let Some(sem_arc) = sem_arc {
            let (lock, cvar) = &*sem_arc;
            let mut sem = lock.lock().unwrap();
            while !sem.posted {
                sem = cvar.wait(sem).unwrap();
            }
            0
        } else { 6 }
    }

    fn dos_create_mutex_sem(&self, _psz_name_ptr: u32, phmtx_ptr: u32, fl_attr: u32, f_state: u32) -> u32 {
        let mut sem_mgr = self.shared.sem_mgr.lock().unwrap();
        let h = sem_mgr.create_mutex(None, fl_attr, f_state != 0);
        unsafe { ptr::write_unaligned(self.shared.guest_mem.add(phmtx_ptr as usize) as *mut u32, h); }
        0
    }

    fn dos_close_mutex_sem(&self, hmtx: u32) -> u32 {
        if self.shared.sem_mgr.lock().unwrap().close_mutex(hmtx) { 0 }
        else { 6 }
    }

    fn dos_request_mutex_sem(&self, tid: u32, hmtx: u32, _msec: u32) -> u32 {
        let sem_arc = self.shared.sem_mgr.lock().unwrap().get_mutex(hmtx);
        if let Some(sem_arc) = sem_arc {
            let (lock, cvar) = &*sem_arc;
            let mut sem = lock.lock().unwrap();
            loop {
                match sem.owner_tid {
                    None => {
                        sem.owner_tid = Some(tid);
                        sem.request_count = 1;
                        return 0;
                    }
                    Some(owner) if owner == tid => {
                        sem.request_count += 1;
                        return 0;
                    }
                    _ => {
                        sem = cvar.wait(sem).unwrap();
                    }
                }
            }
        } else { 6 }
    }

    fn dos_release_mutex_sem(&self, tid: u32, hmtx: u32) -> u32 {
        let sem_arc = self.shared.sem_mgr.lock().unwrap().get_mutex(hmtx);
        if let Some(sem_arc) = sem_arc {
            let (lock, cvar) = &*sem_arc;
            let mut sem = lock.lock().unwrap();
            match sem.owner_tid {
                Some(owner) if owner == tid => {
                    sem.request_count -= 1;
                    if sem.request_count == 0 {
                        sem.owner_tid = None;
                        cvar.notify_all();
                    }
                    0
                }
                _ => 288, // ERROR_NOT_OWNER
            }
        } else { 6 }
    }

    fn dos_wait_thread(&self, vcpu_id: u32, ptid_ptr: u32) -> u32 {
        let tid = unsafe { ptr::read_unaligned(self.shared.guest_mem.add(ptid_ptr as usize) as *const u32) };
        println!("  [VCPU {}] Waiting for thread {}...", vcpu_id, tid);
        
        let mut handle = None;
        for _ in 0..100 {
            handle = self.shared.threads.lock().unwrap().remove(&tid);
            if handle.is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        if let Some(h) = handle {
            println!("  [VCPU {}] Joining thread {}...", vcpu_id, tid);
            h.join().unwrap();
            println!("  [VCPU {}] Thread {} joined successfully.", vcpu_id, tid);
            0
        } else {
            println!("  [VCPU {}] Thread {} handle not found!", vcpu_id, tid);
            309
        }
    }
}

impl Drop for SharedState {
    fn drop(&mut self) { unsafe { libc::munmap(self.guest_mem as *mut libc::c_void, self.guest_mem_size); } }
}
