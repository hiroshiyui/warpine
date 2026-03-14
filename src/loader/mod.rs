// SPDX-License-Identifier: GPL-3.0-only

pub mod constants;
pub mod mutex_ext;
pub mod managers;
pub mod ipc;
pub mod pm_types;
pub mod vfs;
pub mod vfs_hostdir;
mod guest_mem;
mod doscalls;
mod pm_win;
mod pm_gpi;
mod stubs;
pub mod console;
mod kbdcalls;
mod viocalls;
mod process;

pub use constants::*;
pub use mutex_ext::MutexExt;
pub use managers::*;
pub use ipc::*;
pub use pm_types::*;
pub use vfs::DriveManager;
pub use vfs_hostdir::HostDirBackend;

use crate::lx::LxFile;
use crate::lx::header::FixupTarget;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::ptr;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicI32};
use std::thread;
use kvm_ioctls::{Kvm, VmFd, VcpuFd};
use kvm_bindings::{kvm_userspace_memory_region, kvm_guest_debug, KVM_GUESTDBG_ENABLE, KVM_GUESTDBG_USE_SW_BP};
use log::{info, debug, warn, error};

struct CallbackFrame {
    saved_rip: u64,
    saved_rsp: u64,
}

pub(crate) enum ApiResult {
    Normal(u32),
    Callback {
        wnd_proc: u32,
        hwnd: u32,
        msg: u32,
        mp1: u32,
        mp2: u32,
    },
}

/// Shared state for all vCPU threads, the GUI event loop, and timer threads.
///
/// # Lock ordering (acquire in this order to prevent deadlocks)
///
/// ```text
/// Level 1: next_tid, threads          (lightweight, rarely contended)
/// Level 2: mem_mgr, handle_mgr, hdir_mgr, resource_mgr, shmem_mgr, drive_mgr  (independent resource managers)
/// Level 3: queue_mgr                  (may lock inner Arc<Mutex<OS2Queue>>)
/// Level 4: sem_mgr                    (may lock inner semaphore mutexes)
/// Level 5: window_mgr                 (may lock inner Arc<Mutex<PM_MsgQueue>>)
/// ```
///
/// Rules:
/// - Never acquire a higher-level lock while holding a lower-level lock.
/// - Inner mutexes (OS2Queue, PM_MsgQueue, EventSemaphore, MutexSemaphore)
///   are always acquired *after* their parent manager and released *before*
///   acquiring any other SharedState mutex.
/// - Release locks before blocking (condvar wait, thread join, sleep).
pub struct SharedState {
    pub mem_mgr: Mutex<MemoryManager>,
    pub handle_mgr: Mutex<HandleManager>,
    pub resource_mgr: Mutex<ResourceManager>,
    pub shmem_mgr: Mutex<SharedMemManager>,
    pub process_mgr: Mutex<ProcessManager>,
    pub sem_mgr: Mutex<SemaphoreManager>,
    pub hdir_mgr: Mutex<HDirManager>,
    pub queue_mgr: Mutex<QueueManager>,
    pub window_mgr: Mutex<WindowManager>,
    pub drive_mgr: Mutex<DriveManager>,
    pub console_mgr: Mutex<console::VioManager>,
    /// Code object address ranges (base, end) for return address scanning in thunk bypass
    pub code_ranges: Mutex<Vec<(u32, u32)>>,
    /// Executable name as provided on the command line
    pub exe_name: Mutex<String>,
    pub guest_mem: *mut u8,
    pub guest_mem_size: usize,
    pub next_tid: Mutex<u32>,
    pub threads: Mutex<HashMap<u32, thread::JoinHandle<()>>>,
    pub exit_requested: AtomicBool,
    pub exit_code: AtomicI32,
}

unsafe impl Send for SharedState {}
unsafe impl Sync for SharedState {}

pub struct Loader {
    pub(crate) _kvm: Kvm,
    pub vm: Arc<VmFd>,
    pub shared: Arc<SharedState>,
}

impl Loader {
    pub fn new() -> Self {
        let kvm = Kvm::new().expect("Failed to open /dev/kvm");
        let vm = Arc::new(kvm.create_vm().expect("Failed to create VM"));
        let guest_mem_size = 256 * 1024 * 1024;
        let guest_mem_raw = unsafe {
            libc::mmap(ptr::null_mut(), guest_mem_size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE, -1, 0)
        };
        if guest_mem_raw == libc::MAP_FAILED {
            panic!("Failed to mmap {} bytes for guest memory: {}", guest_mem_size, std::io::Error::last_os_error());
        }
        let guest_mem = guest_mem_raw as *mut u8;
        unsafe { ptr::write_bytes(guest_mem, 0, guest_mem_size); }
        let mem_region = kvm_userspace_memory_region { slot: 0, guest_phys_addr: 0, memory_size: guest_mem_size as u64, userspace_addr: guest_mem as u64, flags: 0 };
        unsafe { vm.set_user_memory_region(mem_region).unwrap(); }

        let mem_mgr = MemoryManager::new(DYNAMIC_ALLOC_BASE, guest_mem_size as u32);
        let handle_mgr = HandleManager::new();
        let resource_mgr = ResourceManager::new();
        let shmem_mgr = SharedMemManager::new();
        let process_mgr = ProcessManager::new();
        let sem_mgr = SemaphoreManager::new();
        let hdir_mgr = HDirManager::new();
        let queue_mgr = QueueManager::new();
        let window_mgr = WindowManager::new();
        let mut drive_mgr = DriveManager::with_default_config();
        // Mount HostDirBackend on C: using the configured path
        if let Some(config) = drive_mgr.drive_config(2).cloned() {
            match HostDirBackend::new(config.host_path.clone()) {
                Ok(backend) => {
                    drive_mgr.mount(2, Box::new(backend));
                    info!("Mounted C: → {}", config.host_path.display());
                }
                Err(e) => {
                    warn!("Failed to mount C: drive at {}: {:?}", config.host_path.display(), e);
                }
            }
        }
        let console_mgr = console::VioManager::new();

        let shared = Arc::new(SharedState {
            mem_mgr: Mutex::new(mem_mgr),
            handle_mgr: Mutex::new(handle_mgr),
            resource_mgr: Mutex::new(resource_mgr),
            shmem_mgr: Mutex::new(shmem_mgr),
            process_mgr: Mutex::new(process_mgr),
            sem_mgr: Mutex::new(sem_mgr),
            hdir_mgr: Mutex::new(hdir_mgr),
            queue_mgr: Mutex::new(queue_mgr),
            window_mgr: Mutex::new(window_mgr),
            drive_mgr: Mutex::new(drive_mgr),
            console_mgr: Mutex::new(console_mgr),
            code_ranges: Mutex::new(Vec::new()),
            exe_name: Mutex::new(String::new()),
            guest_mem,
            guest_mem_size,
            next_tid: Mutex::new(1),
            threads: Mutex::new(HashMap::new()),
            exit_requested: AtomicBool::new(false),
            exit_code: AtomicI32::new(0),
        });

        Loader { _kvm: kvm, vm, shared }
    }

    pub fn is_pm_app(&self, lx_file: &LxFile) -> bool {
        lx_file.imported_modules.iter().any(|m| m == "PMWIN" || m == "PMGPI")
    }

    pub fn get_shared(&self) -> Arc<SharedState> {
        Arc::clone(&self.shared)
    }

    pub fn load<P: AsRef<Path>>(&mut self, lx_file: &LxFile, path: P) -> io::Result<()> {
        let mut file = File::open(path)?;
        let data_pages_base = lx_file.header.data_pages_offset as u64;
        for (i, obj) in lx_file.object_table.iter().enumerate() {
            debug!("  Mapping Object {}...", i + 1);
            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.page_map.len() { break; }
                let page_off = data_pages_base + ((lx_file.page_map[page_idx].data_offset as u64) << lx_file.header.page_offset_shift);
                let target = obj.base_address as usize + (p * 4096);
                if lx_file.page_map[page_idx].data_size > 0 {
                    file.seek(SeekFrom::Start(page_off))?;
                    file.read_exact(self.guest_slice_mut(target as u32, lx_file.page_map[page_idx].data_size as usize).expect("load: page target OOB"))?;
                }
            }
        }
        // Store code object address ranges for thunk bypass stack scanning
        {
            let mut ranges = self.shared.code_ranges.lock_or_recover();
            for obj in &lx_file.object_table {
                // Object flags bit 2 = executable
                if obj.flags & 0x0004 != 0 {
                    let base = obj.base_address;
                    let end = base + obj.size;
                    ranges.push((base, end));
                }
            }
        }

        // Populate resource manager with precomputed guest addresses
        if !lx_file.resources.is_empty() {
            let mut res_mgr = self.shared.resource_mgr.lock_or_recover();
            for res in &lx_file.resources {
                let obj_idx = (res.object_num as usize).wrapping_sub(1);
                if obj_idx < lx_file.object_table.len() {
                    let guest_addr = lx_file.object_table[obj_idx].base_address + res.offset;
                    res_mgr.add(res.type_id, res.name_id, guest_addr, res.size);
                    debug!("  Resource: type={} id={} addr=0x{:08X} size={}", res.type_id, res.name_id, guest_addr, res.size);
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
                        let src_type = record.source_type & 0x0F;
                        match src_type {
                            0x07 => {
                                // 32-bit offset
                                self.guest_write::<u32>(source_phys as u32, target_addr as u32).expect("fixup: write OOB");
                            }
                            0x08 => {
                                // 32-bit self-relative
                                self.guest_write::<i32>(source_phys as u32, (target_addr as isize - (source_phys as isize + 4)) as i32).expect("fixup: write OOB");
                            }
                            0x02 | 0x03 => {
                                // 16:16 far pointer (selector:offset)
                                // In our flat 32-bit model, encode as offset in code segment (selector 0x08)
                                // The guest code will do a far call: CALL selector:offset
                                // We write: offset (16-bit) + selector (16-bit) = 4 bytes
                                let offset16 = (target_addr & 0xFFFF) as u16;
                                let selector = 0x08u16; // code segment selector
                                self.guest_write::<u16>(source_phys as u32, offset16).expect("fixup: 16:16 offset OOB");
                                self.guest_write::<u16>(source_phys as u32 + 2, selector).expect("fixup: 16:16 sel OOB");
                            }
                            0x05 => {
                                // 16-bit offset
                                self.guest_write::<u16>(source_phys as u32, (target_addr & 0xFFFF) as u16).expect("fixup: 16-bit offset OOB");
                            }
                            0x06 => {
                                // 16:32 far pointer (6 bytes: 32-bit offset + 16-bit selector)
                                self.guest_write::<u32>(source_phys as u32, target_addr as u32).expect("fixup: 16:32 offset OOB");
                                self.guest_write::<u16>(source_phys as u32 + 4, 0x08).expect("fixup: 16:32 selector OOB");
                            }
                            _ => {
                                // Unknown source type — log but don't crash
                                log::warn!("Unhandled fixup source type 0x{:02X} at 0x{:08X}", src_type, source_phys);
                            }
                        }
                    }
                }
            }
        }
        // Patch 16-bit thunk stubs: replace them with near JMPs to bypass 16:32 stack switching
        self.patch_16bit_thunks(lx_file);
        Ok(())
    }

    /// Patch 16-bit API thunk stubs (Object 1) to use near JMPs instead of LSS+JMP FAR.
    /// The thunks have 16:32 far pointers (written by type 0x06 fixups) to their targets.
    /// We read the 32-bit offset from each fixup location and replace the thunk entry with a near JMP.
    fn patch_16bit_thunks(&self, lx_file: &LxFile) {
        // Build a map of internal code addresses → API stub addresses.
        // When a type 0x06 (16:32 far pointer) fixup targets an internal address,
        // we need to figure out what API it wraps. We do this by scanning the target
        // code for a CALL or JMP to a MAGIC_API_BASE address.
        //
        // For fixups that directly target an external ordinal (already resolved to
        // MAGIC_API_BASE+), we can jump straight there.
        for obj in &lx_file.object_table {
            let obj_page_start = (obj.page_map_index as usize).saturating_sub(1);
            for p in 0..obj.page_count as usize {
                let page_idx = obj_page_start + p;
                if page_idx >= lx_file.fixup_records_by_page.len() { break; }
                for record in &lx_file.fixup_records_by_page[page_idx] {
                    let src_type = record.source_type & 0x0F;
                    if src_type != 0x06 { continue; } // only 16:32 pointer fixups

                    // Determine the jump target for this thunk
                    let api_target = match &record.target {
                        FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal } => {
                            // Direct external import — resolve to API stub
                            let module = lx_file.imported_modules
                                .get((*module_ordinal as usize).wrapping_sub(1));
                            if let Some(module) = module {
                                Some(self.resolve_import(module, *proc_ordinal) as u32)
                            } else { None }
                        }
                        FixupTarget::Internal { object_num, target_offset } => {
                            // Internal target — the 32-bit thunk entry code.
                            // Scan the target code for an INT 3 at MAGIC_API_BASE (API call)
                            // or a CALL/JMP to a MAGIC_API_BASE address.
                            let target_obj = lx_file.object_table
                                .get((*object_num as usize).wrapping_sub(1));
                            if let Some(tobj) = target_obj {
                                let target_addr = tobj.base_address as u32 + *target_offset as u32;
                                self.scan_thunk_for_api_target(target_addr)
                            } else { None }
                        }
                        _ => None,
                    };

                    for &off in &record.source_offsets {
                        let fixup_addr = obj.base_address as u32 + p as u32 * 4096 + off as u32;
                        let target = self.guest_read::<u32>(fixup_addr).unwrap_or(0);
                        if target == 0 { continue; }

                        // Use API stub target if we found one, otherwise fall back to original target
                        let jump_target = api_target.unwrap_or(target);

                        // The thunk entry starts 11 bytes before the fixup (based on observed pattern)
                        // Replace the entry with: JMP near target (E9 rel32) + NOP padding
                        let entry_start = fixup_addr.wrapping_sub(11);
                        let rel32 = (jump_target as i64 - (entry_start as i64 + 5)) as i32;
                        self.guest_write::<u8>(entry_start, 0xE9).unwrap(); // JMP rel32
                        self.guest_write::<i32>(entry_start + 1, rel32).unwrap();
                        for i in 5..17u32 {
                            self.guest_write::<u8>(entry_start + i, 0x90).unwrap();
                        }
                        if api_target.is_some() {
                            debug!("Patched 16-bit thunk at 0x{:08X} -> JMP API stub 0x{:08X}", entry_start, jump_target);
                        } else {
                            debug!("Patched 16-bit thunk at 0x{:08X} -> JMP 0x{:08X} (internal, no API found)", entry_start, jump_target);
                        }
                    }
                }
            }
        }
    }

    /// Scan a 32-bit thunk entry for the API call it wraps.
    /// The thunk code typically has: LSS (which faults), then some setup,
    /// then a CALL or JMP to a MAGIC_API_BASE address (INT 3 stub).
    /// We scan up to 64 bytes looking for a CALL (E8 rel32) or JMP (E9 rel32)
    /// whose target is in the MAGIC_API_BASE range.
    fn scan_thunk_for_api_target(&self, start_addr: u32) -> Option<u32> {
        let api_base = MAGIC_API_BASE as u32;
        let api_end = api_base + STUB_AREA_SIZE as u32;

        for offset in 0..64u32 {
            let addr = start_addr + offset;
            let byte = self.guest_read::<u8>(addr).unwrap_or(0);
            match byte {
                0xE8 | 0xE9 => {
                    // CALL rel32 or JMP rel32
                    if let Some(rel) = self.guest_read::<i32>(addr + 1) {
                        let target = (addr as i64 + 5 + rel as i64) as u32;
                        if target >= api_base && target < api_end {
                            return Some(target);
                        }
                    }
                }
                0xFF => {
                    // Check for indirect CALL/JMP with absolute target
                    // FF /2 (CALL r/m32) or FF /4 (JMP r/m32) with mod=00 rm=101 (disp32)
                    let modrm = self.guest_read::<u8>(addr + 1).unwrap_or(0);
                    let reg = (modrm >> 3) & 7;
                    let mod_bits = modrm >> 6;
                    let rm = modrm & 7;
                    if (reg == 2 || reg == 4) && mod_bits == 0 && rm == 5 {
                        // [disp32] form
                        if let Some(disp) = self.guest_read::<u32>(addr + 2) {
                            if disp >= api_base && disp < api_end {
                                return Some(disp);
                            }
                        }
                    }
                }
                0xCC => {
                    // INT 3 — if we're already at MAGIC_API_BASE range, this IS the API stub
                    if addr >= api_base && addr < api_end {
                        return Some(addr);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn resolve_import(&self, module: &str, ordinal: u32) -> u64 {
        if module == "DOSCALLS" { MAGIC_API_BASE + ordinal as u64 }
        else if module == "QUECALLS" { MAGIC_API_BASE + 1024 + ordinal as u64 }
        else if module == "PMWIN" { MAGIC_API_BASE + PMWIN_BASE as u64 + ordinal as u64 }
        else if module == "PMGPI" { MAGIC_API_BASE + PMGPI_BASE as u64 + ordinal as u64 }
        else if module == "KBDCALLS" { MAGIC_API_BASE + KBDCALLS_BASE as u64 + ordinal as u64 }
        else if module == "VIOCALLS" { MAGIC_API_BASE + VIOCALLS_BASE as u64 + ordinal as u64 }
        else if module == "SESMGR" { MAGIC_API_BASE + SESMGR_BASE as u64 + ordinal as u64 }
        else if module == "NLS" { MAGIC_API_BASE + NLS_BASE as u64 + ordinal as u64 }
        else if module == "MSG" { MAGIC_API_BASE + MSG_BASE as u64 + ordinal as u64 }
        else {
            warn!("Unknown import module: {} ordinal {}", module, ordinal);
            // Return a valid stub address so the guest doesn't crash on unresolved imports
            // Use a dedicated range at end of stub area
            MAGIC_API_BASE + (STUB_AREA_SIZE as u64 - 1)
        }
    }

    fn setup_stubs(&self) {
        for i in 0..STUB_AREA_SIZE {
            self.guest_write::<u8>(MAGIC_API_BASE as u32 + i, 0xCC).expect("setup_stubs: write OOB");
        }
    }

    /// Set up a minimal GDT and IDT so CPU exceptions cause VMEXIT via INT 3.
    /// GDT at 0x00080000, IDT at 0x00081000, exception handler stubs at 0x00081800.
    fn setup_idt(&self) {
        const GDT_BASE: u32 = 0x00080000;
        const IDT_BASE: u32 = 0x00081000;
        const IDT_HANDLER_BASE: u32 = 0x00081800;
        const NUM_VECTORS: u32 = 32;

        // Set up GDT entries
        // Entry 0: null descriptor (required)
        self.guest_write::<u64>(GDT_BASE, 0).unwrap();
        // Entry 1 (selector 0x08): code segment — base=0, limit=0xFFFFF, 32-bit, execute/read
        // Byte layout: limit_lo(2), base_lo(2), base_mid(1), access(1), flags_limit_hi(1), base_hi(1)
        // access: P=1, DPL=0, S=1, type=0xB (exec/read/accessed) = 0x9B
        // flags: G=1 (4K granularity), D/B=1 (32-bit), limit_hi=0xF = 0xCF
        let code_desc: u64 = 0x00CF9B000000FFFF;
        self.guest_write::<u64>(GDT_BASE + 8, code_desc).unwrap();
        // Entry 2 (selector 0x10): data segment — base=0, limit=0xFFFFF, 32-bit, read/write
        // access: P=1, DPL=0, S=1, type=0x3 (read/write/accessed) = 0x93
        let data_desc: u64 = 0x00CF93000000FFFF;
        self.guest_write::<u64>(GDT_BASE + 16, data_desc).unwrap();
        // Entry 3 (selector 0x18): FS segment — base will be set via sregs, same attributes as data
        self.guest_write::<u64>(GDT_BASE + 24, data_desc).unwrap();

        // Set up IDT with exception handler stubs
        for i in 0..NUM_VECTORS {
            let handler_addr = IDT_HANDLER_BASE + i * 16;  // 16 bytes per handler
            // For exceptions with error codes (#DF=8, #TS=10, #NP=11, #SS=12, #GP=13, #PF=14, #AC=17):
            //   CPU pushes: [error_code] [EIP] [CS] [EFLAGS]
            // For exceptions without error codes:
            //   CPU pushes: [EIP] [CS] [EFLAGS]
            let has_error_code = matches!(i, 8 | 10 | 11 | 12 | 13 | 14 | 17);
            let mut off = 0u32;
            if !has_error_code {
                // PUSH 0 as fake error code to unify stack layout
                self.guest_write::<u8>(handler_addr + off, 0x6A).unwrap(); // PUSH imm8
                self.guest_write::<u8>(handler_addr + off + 1, 0x00).unwrap();
                off += 2;
            }
            // PUSH imm8 <vector number>
            self.guest_write::<u8>(handler_addr + off, 0x6A).unwrap();
            self.guest_write::<u8>(handler_addr + off + 1, i as u8).unwrap();
            off += 2;
            // INT 3
            self.guest_write::<u8>(handler_addr + off, 0xCC).unwrap();

            // IDT entry: 32-bit interrupt gate
            let idt_entry_addr = IDT_BASE + i * 8;
            let offset_lo = (handler_addr & 0xFFFF) as u16;
            let offset_hi = ((handler_addr >> 16) & 0xFFFF) as u16;
            self.guest_write::<u16>(idt_entry_addr, offset_lo).unwrap();
            self.guest_write::<u16>(idt_entry_addr + 2, 0x08).unwrap(); // code selector
            self.guest_write::<u16>(idt_entry_addr + 4, 0x8E00).unwrap(); // P=1, DPL=0, 32-bit int gate
            self.guest_write::<u16>(idt_entry_addr + 6, offset_hi).unwrap();
        }
    }

    fn setup_guest(&self, lx_file: &LxFile) -> (u64, u64, u64) {
        let entry_eip = lx_file.object_table[lx_file.header.eip_object as usize - 1].base_address as u64 + lx_file.header.eip as u64;
        let entry_esp = lx_file.object_table[lx_file.header.esp_object as usize - 1].base_address as u64 + lx_file.header.esp as u64;

        let tib_base = TIB_BASE as u64;
        // Build OS/2 environment block: null-terminated KEY=VALUE strings, double-null terminated,
        // followed by the program name (null-terminated).
        let exe_name = self.shared.exe_name.lock_or_recover().clone();
        let os2_exe = if exe_name.is_empty() {
            String::from("C:\\APP.EXE")
        } else {
            // Convert Unix path to OS/2 style: C:\path\to\exe
            let basename = std::path::Path::new(&exe_name)
                .file_name()
                .map(|f| f.to_string_lossy().to_uppercase())
                .unwrap_or_else(|| "APP.EXE".into());
            format!("C:\\{}", basename)
        };
        let mut env_block: Vec<u8> = Vec::new();
        env_block.extend_from_slice(b"PATH=C:\\\0");
        env_block.extend_from_slice(b"COMSPEC=C:\\4OS2.EXE\0");
        env_block.extend_from_slice(b"OS=OS2\0");
        env_block.extend_from_slice(b"TMP=C:\\TMP\0");
        env_block.push(0); // double-null terminates the environment
        let cmdline_offset = env_block.len() as u32;
        env_block.extend_from_slice(os2_exe.as_bytes());
        env_block.push(0);
        // Allocate env block via memory manager so it's properly tracked
        let env_addr = self.shared.mem_mgr.lock_or_recover()
            .alloc(env_block.len() as u32)
            .expect("setup_guest: env alloc failed");
        let cmdline_addr = env_addr + cmdline_offset;
        debug!("  Environment block ({} bytes) at 0x{:08X}: {:02X?}",
            env_block.len(), env_addr, &env_block);
        self.guest_write_bytes(env_addr, &env_block).expect("setup_guest: env write OOB");
        self.guest_write::<u32>(TIB_BASE + 0x18, TIB_BASE).expect("setup_guest: TIB self-ptr OOB");
        self.guest_write::<u32>(TIB_BASE + 0x30, PIB_BASE).expect("setup_guest: TIB->PIB OOB");
        self.guest_write::<u32>(PIB_BASE + 0x00, 42).expect("setup_guest: PIB PID OOB");
        self.guest_write::<u32>(PIB_BASE + 0x0C, cmdline_addr).expect("setup_guest: PIB pchcmd OOB");
        self.guest_write::<u32>(PIB_BASE + 0x10, env_addr).expect("setup_guest: PIB pchenv OOB");

        self.setup_stubs();
        self.setup_idt();
        (entry_eip, entry_esp, tib_base)
    }

    fn create_initial_vcpu(&self, entry_eip: u64, entry_esp: u64) -> VcpuFd {
        let vcpu = self.vm.create_vcpu(0).unwrap();
        let mut regs = vcpu.get_regs().unwrap();
        regs.rip = entry_eip;
        regs.rsp = entry_esp - 20;
        regs.rflags = 2;
        vcpu.set_regs(&regs).unwrap();

        // OS/2 initial stack: [return_addr] [hmod] [reserved] [env_ptr] [cmdline_ptr]
        // PIB layout: +0x0C = pib_pchcmd, +0x10 = pib_pchenv
        let cmdline_addr = self.guest_read::<u32>(PIB_BASE + 0x0C).unwrap_or(ENV_ADDR);
        let env_addr = self.guest_read::<u32>(PIB_BASE + 0x10).unwrap_or(ENV_ADDR);
        let sp = regs.rsp as u32;
        self.guest_write::<u32>(sp, EXIT_TRAP_ADDR).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 4, 0).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 8, 0).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 12, env_addr).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 16, cmdline_addr).expect("create_initial_vcpu: stack write OOB");
        vcpu
    }

    pub fn setup_and_run_cli(self, lx_file: &LxFile) -> ! {
        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        self.run_vcpu(vcpu, 0, tib_base);
        let code = self.shared.exit_code.load(std::sync::atomic::Ordering::Relaxed);
        std::process::exit(code);
    }

    pub fn setup_and_spawn_vcpu(self: Arc<Self>, lx_file: &LxFile) {
        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        let loader = self;
        thread::spawn(move || {
            loader.run_vcpu(vcpu, 0, tib_base);
        });
    }

    /// Legacy run method for backwards compatibility
    pub fn run(self, lx_file: &LxFile, gui_sender: crate::gui::GUISender) -> ! {
        self.shared.window_mgr.lock_or_recover().gui_tx = Some(gui_sender);

        let (entry_eip, entry_esp, tib_base) = self.setup_guest(lx_file);
        let vcpu = self.create_initial_vcpu(entry_eip, entry_esp);
        self.run_vcpu(vcpu, 0, tib_base);
        let code = self.shared.exit_code.load(std::sync::atomic::Ordering::Relaxed);
        std::process::exit(code);
    }

    pub(crate) fn run_vcpu(&self, mut vcpu: VcpuFd, vcpu_id: u32, tib_base: u64) {
        let mut sregs = vcpu.get_sregs().unwrap();
        sregs.cs.base = 0; sregs.cs.limit = 0xFFFFFFFF; sregs.cs.g = 1; sregs.cs.db = 1; sregs.cs.present = 1; sregs.cs.type_ = 11; sregs.cs.s = 1; sregs.cs.selector = 0x08;
        let mut ds = sregs.cs; ds.type_ = 3; ds.selector = 0x10;
        sregs.ds = ds; sregs.es = ds; sregs.gs = ds; sregs.ss = ds;
        let mut fs = ds; fs.base = tib_base; fs.limit = 0xFFF; fs.selector = 0x18; sregs.fs = fs;
        // PE=1 (protected mode), clear EM/TS to enable FPU, set NE for native FPU errors
        sregs.cr0 = (sregs.cr0 | 1 | (1 << 5)) & !(1u64 << 2) & !(1u64 << 3);
        // Also set CR4.OSFXSR to enable SSE instructions
        sregs.cr4 |= 1 << 9;
        // Set up GDT and IDT registers
        sregs.gdt.base = 0x00080000;
        sregs.gdt.limit = 4 * 8 - 1; // 4 entries × 8 bytes
        sregs.idt.base = 0x00081000;
        sregs.idt.limit = 32 * 8 - 1;
        vcpu.set_sregs(&sregs).unwrap();

        let debug = kvm_guest_debug { control: KVM_GUESTDBG_ENABLE | KVM_GUESTDBG_USE_SW_BP, ..Default::default() };
        vcpu.set_guest_debug(&debug).unwrap();

        debug!("  [VCPU {}] Started at EIP=0x{:08X}", vcpu_id, vcpu.get_regs().unwrap().rip);

        let mut callback_stack: Vec<CallbackFrame> = Vec::new();

        loop {
            // Check if shutdown has been requested
            if self.shared.exit_requested.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let res = vcpu.run();
            if let Err(e) = res {
                error!("  [VCPU {}] KVM Run failed: {}", vcpu_id, e);
                self.shared.exit_code.store(1, std::sync::atomic::Ordering::Relaxed);
                self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            let exit = res.unwrap();
            match exit {
                kvm_ioctls::VcpuExit::Debug(_) => {
                    let rip = vcpu.get_regs().unwrap().rip;
                    // Check if this is from an IDT exception handler stub
                    const IDT_HANDLER_BASE: u64 = 0x00081800;
                    if rip >= IDT_HANDLER_BASE && rip < IDT_HANDLER_BASE + 32 * 16 {
                        let regs = vcpu.get_regs().unwrap();
                        // Stack layout: [vector_num] [error_code_or_fake] [fault_EIP] [fault_CS] [fault_EFLAGS]
                        let vector = self.guest_read::<u32>(regs.rsp as u32).unwrap_or(0xFF);
                        let error_code = self.guest_read::<u32>(regs.rsp as u32 + 4).unwrap_or(0);
                        let fault_eip = self.guest_read::<u32>(regs.rsp as u32 + 8).unwrap_or(0);
                        let fault_cs = self.guest_read::<u32>(regs.rsp as u32 + 12).unwrap_or(0);
                        let fault_eflags = self.guest_read::<u32>(regs.rsp as u32 + 16).unwrap_or(0);

                        // Handle #GP from LSS instruction.
                        // LSS (Load Stack Segment) faults because our flat 32-bit GDT has no
                        // 16-bit segment selectors. Two strategies:
                        // 1. Stack scan: find a verified CALL return address and skip the entire
                        //    thunk (works for init-time thunks where the API call is bypassed)
                        // 2. LSS emulation: load the offset portion into the dest register and
                        //    advance past the instruction (works when the code after LSS is valid
                        //    flat-mode code)
                        if vector == 13 {
                            let byte0 = self.guest_read::<u8>(fault_eip as u32).unwrap_or(0);
                            let byte1 = self.guest_read::<u8>(fault_eip as u32 + 1).unwrap_or(0);
                            let is_lss = (byte0 == 0x66 && byte1 == 0x0F) || (byte0 == 0x0F && byte1 == 0xB2);
                            if is_lss {
                                debug!("  [VCPU {}] LSS #GP at 0x{:08X}", vcpu_id, fault_eip);

                                // Strategy 1: Stack scan for verified return address
                                let scan_esp = regs.rsp as u32 + 20;
                                let mut ret_addr = 0u32;
                                let code_ranges = self.shared.code_ranges.lock_or_recover().clone();
                                for i in 0..64 {
                                    let val = self.guest_read::<u32>(scan_esp + i * 4).unwrap_or(0);
                                    let in_code = code_ranges.iter().any(|&(base, end)| val >= base && val < end);
                                    if !in_code || val == fault_eip as u32 { continue; }
                                    let is_call_e8 = self.guest_read::<u8>(val - 5).unwrap_or(0) == 0xE8;
                                    let b2 = self.guest_read::<u8>(val - 2).unwrap_or(0);
                                    let b1 = self.guest_read::<u8>(val - 1).unwrap_or(0);
                                    let is_call_ff = b2 == 0xFF && (b1 & 0x38) == 0x10;
                                    if is_call_e8 || is_call_ff {
                                        ret_addr = val;
                                        let orig_esp = scan_esp + i * 4 + 4;
                                        let mut new_regs = vcpu.get_regs().unwrap();
                                        new_regs.rax = 0;
                                        new_regs.rip = ret_addr as u64;
                                        new_regs.rsp = orig_esp as u64;
                                        vcpu.set_regs(&new_regs).unwrap();
                                        debug!("  Thunk skip: returning to 0x{:08X} with ESP=0x{:08X}", ret_addr, orig_esp);
                                        break;
                                    }
                                }
                                if ret_addr != 0 {
                                    continue;
                                }

                                // Strategy 2: LSS emulation (no verified return address found)
                                // Emulate LSS as a flat-mode operation: load the 32-bit offset
                                // portion into the destination register, ignore the 16-bit segment
                                // selector (keep SS unchanged). This allows the thunk code to
                                // continue with the correct stack pointer value.
                                let mut new_regs = vcpu.get_regs().unwrap();
                                // Parse LSS instruction to get dest register and memory operand
                                let (prefix_len, modrm_offset) = if byte0 == 0x66 { (1u32, 3u32) } else { (0u32, 2u32) };
                                let modrm = self.guest_read::<u8>(fault_eip as u32 + modrm_offset).unwrap_or(0);
                                let mod_bits = modrm >> 6;
                                let reg = (modrm >> 3) & 7; // destination register
                                let rm = modrm & 7;

                                // Compute memory operand address and instruction length
                                let (mem_addr, extra_len) = match (mod_bits, rm) {
                                    (0, 4) => {
                                        let sib = self.guest_read::<u8>(fault_eip as u32 + modrm_offset + 1).unwrap_or(0);
                                        // Simplified SIB: just use base register
                                        let base_reg = sib & 7;
                                        let base_val = match base_reg {
                                            0 => new_regs.rax, 1 => new_regs.rcx, 2 => new_regs.rdx,
                                            3 => new_regs.rbx, 4 => new_regs.rsp, 5 => new_regs.rbp,
                                            6 => new_regs.rsi, 7 => new_regs.rdi, _ => 0,
                                        };
                                        (base_val as u32, 1u32) // SIB byte
                                    }
                                    (0, 5) => {
                                        let disp = self.guest_read::<u32>(fault_eip as u32 + modrm_offset + 1).unwrap_or(0);
                                        (disp, 4u32)
                                    }
                                    (0, _) => {
                                        let base_val = match rm {
                                            0 => new_regs.rax, 1 => new_regs.rcx, 2 => new_regs.rdx,
                                            3 => new_regs.rbx, 5 => new_regs.rbp,
                                            6 => new_regs.rsi, 7 => new_regs.rdi, _ => 0,
                                        };
                                        (base_val as u32, 0u32)
                                    }
                                    (1, 4) => {
                                        let sib = self.guest_read::<u8>(fault_eip as u32 + modrm_offset + 1).unwrap_or(0);
                                        let disp = self.guest_read::<i8>(fault_eip as u32 + modrm_offset + 2).unwrap_or(0);
                                        let base_reg = sib & 7;
                                        let base_val = match base_reg {
                                            0 => new_regs.rax, 1 => new_regs.rcx, 2 => new_regs.rdx,
                                            3 => new_regs.rbx, 4 => new_regs.rsp, 5 => new_regs.rbp,
                                            6 => new_regs.rsi, 7 => new_regs.rdi, _ => 0,
                                        };
                                        ((base_val as i64 + disp as i64) as u32, 2u32)
                                    }
                                    (1, _) => {
                                        let disp = self.guest_read::<i8>(fault_eip as u32 + modrm_offset + 1).unwrap_or(0);
                                        let base_val = match rm {
                                            0 => new_regs.rax, 1 => new_regs.rcx, 2 => new_regs.rdx,
                                            3 => new_regs.rbx, 5 => new_regs.rbp,
                                            6 => new_regs.rsi, 7 => new_regs.rdi, _ => 0,
                                        };
                                        ((base_val as i64 + disp as i64) as u32, 1u32)
                                    }
                                    (2, _) => {
                                        let disp = self.guest_read::<i32>(fault_eip as u32 + modrm_offset + 1).unwrap_or(0);
                                        let base_val = match rm {
                                            0 => new_regs.rax, 1 => new_regs.rcx, 2 => new_regs.rdx,
                                            3 => new_regs.rbx, 4 => new_regs.rsp, 5 => new_regs.rbp,
                                            6 => new_regs.rsi, 7 => new_regs.rdi, _ => 0,
                                        };
                                        ((base_val as i64 + disp as i64) as u32, 4u32)
                                    }
                                    _ => (0, 0),
                                };

                                // Read the 32-bit offset from memory (first 4 bytes of the 6-byte far pointer)
                                let loaded_offset = self.guest_read::<u32>(mem_addr).unwrap_or(0);
                                // Set destination register (LSS loads into reg:SS, we load offset into reg)
                                match reg {
                                    0 => new_regs.rax = loaded_offset as u64,
                                    1 => new_regs.rcx = loaded_offset as u64,
                                    2 => new_regs.rdx = loaded_offset as u64,
                                    3 => new_regs.rbx = loaded_offset as u64,
                                    4 => new_regs.rsp = loaded_offset as u64,
                                    5 => new_regs.rbp = loaded_offset as u64,
                                    6 => new_regs.rsi = loaded_offset as u64,
                                    7 => new_regs.rdi = loaded_offset as u64,
                                    _ => {}
                                }

                                let lss_len = prefix_len + 2 + 1 + extra_len; // prefix + 0F B2 + ModR/M + extra
                                new_regs.rsp = (regs.rsp as u32 + 20) as u64; // pop exception frame first
                                new_regs.rip = fault_eip as u64 + lss_len as u64;
                                new_regs.rflags = fault_eflags as u64;
                                // If LSS targets ESP, override with the loaded value
                                if reg == 4 {
                                    new_regs.rsp = loaded_offset as u64;
                                }
                                vcpu.set_regs(&new_regs).unwrap();
                                debug!("  LSS emulation: reg{} = 0x{:08X} (from [0x{:08X}]), EIP 0x{:08X} -> 0x{:08X}",
                                       reg, loaded_offset, mem_addr, fault_eip, new_regs.rip);
                                continue;
                            }
                        }

                        error!("  [VCPU {}] CPU Exception #{} at EIP=0x{:08X} CS=0x{:04X} EFLAGS=0x{:08X} ErrorCode=0x{:X}",
                               vcpu_id, vector, fault_eip, fault_cs, fault_eflags, error_code);
                        error!("    ESP=0x{:08X} EAX=0x{:08X} EBX=0x{:08X} ECX=0x{:08X} EDX=0x{:08X}",
                               regs.rsp, regs.rax, regs.rbx, regs.rcx, regs.rdx);
                        self.shared.exit_code.store(1, std::sync::atomic::Ordering::Relaxed);
                        self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    if rip >= MAGIC_API_BASE && rip < MAGIC_API_BASE + STUB_AREA_SIZE as u64 {
                        if rip == EXIT_TRAP_ADDR as u64 {
                            info!("  [VCPU {}] Guest requested thread exit.", vcpu_id);
                            self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                            return;
                        }
                        if rip == CALLBACK_RET_TRAP as u64 {
                            // Return from a PM callback
                            if let Some(frame) = callback_stack.pop() {
                                let mut regs = vcpu.get_regs().unwrap();
                                let result = regs.rax as u32;
                                regs.rip = frame.saved_rip;
                                regs.rsp = frame.saved_rsp;
                                regs.rax = result as u64;
                                // _System calling convention is caller-cleanup, so the guest
                                // caller will do `add esp, N` itself — we must NOT pop args here.
                                vcpu.set_regs(&regs).unwrap();
                                continue;
                            } else {
                                error!("  [VCPU {}] CALLBACK_RET_TRAP with empty callback stack!", vcpu_id);
                                return;
                            }
                        }
                        let ordinal = (rip - MAGIC_API_BASE) as u32;
                        let api_result = self.handle_api_call_ex(&mut vcpu, vcpu_id, ordinal);
                        match api_result {
                            ApiResult::Normal(res) => {
                                let mut regs = vcpu.get_regs().unwrap();
                                regs.rax = res as u64;
                                regs.rip = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for return address") as u64;
                                regs.rsp += 4;
                                vcpu.set_regs(&regs).unwrap();
                            }
                            ApiResult::Callback { wnd_proc, hwnd, msg, mp1, mp2 } => {
                                let mut regs = vcpu.get_regs().unwrap();
                                let return_addr = self.guest_read::<u32>(regs.rsp as u32)
                                    .expect("Stack read OOB for callback return address");
                                // Save current state; saved_rsp is past the return address.
                                // The caller will clean up its own args (_System is caller-cleanup).
                                callback_stack.push(CallbackFrame {
                                    saved_rip: return_addr as u64,
                                    saved_rsp: regs.rsp + 4,
                                });
                                // Set up guest stack for callback: push ret addr + 4 args = 20 bytes
                                regs.rsp -= 20;
                                let sp = regs.rsp as u32;
                                self.guest_write::<u32>(sp, CALLBACK_RET_TRAP).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 4, hwnd).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 8, msg).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 12, mp1).expect("Callback stack write OOB");
                                self.guest_write::<u32>(sp + 16, mp2).expect("Callback stack write OOB");
                                regs.rip = wnd_proc as u64;
                                vcpu.set_regs(&regs).unwrap();
                            }
                        }
                    }
                    else {
                        warn!("  [VCPU {}] Guest breakpoint at EIP=0x{:08X}.", vcpu_id, rip);
                        self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                }
                kvm_ioctls::VcpuExit::Hlt => {
                    info!("  [VCPU {}] Guest HLT.", vcpu_id);
                    self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
                kvm_ioctls::VcpuExit::MmioRead(addr, data) => {
                    // Guest read from unmapped memory — return zeros
                    warn!("  [VCPU {}] MMIO read at 0x{:08X} ({} bytes) — returning zeros",
                           vcpu_id, addr, data.len());
                    for byte in data.iter_mut() { *byte = 0; }
                }
                kvm_ioctls::VcpuExit::MmioWrite(addr, _data) => {
                    // Guest write to unmapped memory — silently ignore
                    warn!("  [VCPU {}] MMIO write at 0x{:08X} — ignoring", vcpu_id, addr);
                }
                kvm_ioctls::VcpuExit::Shutdown => {
                    let regs = vcpu.get_regs().unwrap();
                    let sregs = vcpu.get_sregs().unwrap();
                    error!("  [VCPU {}] Guest shutdown (triple fault)", vcpu_id);
                    error!("    EIP=0x{:08X} ESP=0x{:08X} EAX=0x{:08X} EBX=0x{:08X}", regs.rip, regs.rsp, regs.rax, regs.rbx);
                    error!("    ECX=0x{:08X} EDX=0x{:08X} ESI=0x{:08X} EDI=0x{:08X}", regs.rcx, regs.rdx, regs.rsi, regs.rdi);
                    error!("    EBP=0x{:08X} EFLAGS=0x{:08X} CR0=0x{:08X} CR2=0x{:08X}", regs.rbp, regs.rflags, sregs.cr0, sregs.cr2);
                    error!("    CS=0x{:04X} DS=0x{:04X} SS=0x{:04X} FS=0x{:04X}", sregs.cs.selector, sregs.ds.selector, sregs.ss.selector, sregs.fs.selector);
                    self.shared.exit_code.store(1, std::sync::atomic::Ordering::Relaxed);
                    self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
                _ => {
                    let e = format!("{:?}", exit);
                    let rip = vcpu.get_regs().unwrap().rip;
                    error!("  [VCPU {}] Unhandled VMEXIT: {} at EIP=0x{:08X}", vcpu_id, e, rip);
                    self.shared.exit_code.store(1, std::sync::atomic::Ordering::Relaxed);
                    self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            }
        }
    }

    fn handle_api_call_ex(&self, vcpu: &mut VcpuFd, vcpu_id: u32, ordinal: u32) -> ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };

        debug!("  [VCPU {}] API Call: Ordinal {} (ReturnAddr=0x{:08X})", vcpu_id, ordinal, read_stack(0));

        if ordinal < 1024 {
            // DOSCALLS
            let res = match ordinal {
                256 => self.dos_set_file_ptr(read_stack(4), read_stack(8) as i32, read_stack(12), read_stack(16)),
                257 => self.dos_close(read_stack(4)),
                259 => self.dos_delete(read_stack(4)),
                271 => self.dos_move(read_stack(4), read_stack(8)),
                226 => self.dos_delete_dir(read_stack(4)),
                270 => self.dos_create_dir(read_stack(4)),
                273 => self.dos_open(read_stack(4), read_stack(8), read_stack(12), read_stack(24), read_stack(28)),
                281 => self.dos_read(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                282 => self.dos_write(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                229 => self.dos_sleep(read_stack(4)),
                311 => self.dos_create_thread(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(20)),
                234 => {
                    // DosExit: signal clean shutdown instead of process::exit
                    let _action = read_stack(4);
                    let result = read_stack(8);
                    self.shared.exit_code.store(result as i32, std::sync::atomic::Ordering::Relaxed);
                    self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                    return ApiResult::Normal(0); // won't be used; run_vcpu will exit
                },
                235 => self.dos_query_h_type(read_stack(4), read_stack(8), read_stack(12)),
                239 => self.dos_create_pipe(read_stack(4), read_stack(8), read_stack(12)),
                312 => self.dos_get_info_blocks(vcpu, read_stack(4), read_stack(8)),
                283 => self.dos_exec_pgm(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                280 => self.dos_wait_child(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                237 => self.dos_kill_process(read_stack(4), read_stack(8)),
                264 => self.dos_find_first(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                265 => self.dos_find_next(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                263 => self.dos_find_close(read_stack(4)),
                223 => self.dos_query_path_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                // Directory management
                255 => self.dos_set_current_dir(read_stack(4)),
                274 => self.dos_query_current_dir(read_stack(4), read_stack(8), read_stack(12)),
                275 => self.dos_query_current_disk(read_stack(4), read_stack(8)),
                220 => self.dos_set_default_disk(read_stack(4)),
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
                337 => self.dos_create_mux_wait_sem(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                339 => self.dos_close_mux_wait_sem(read_stack(4)),
                340 => self.dos_wait_mux_wait_sem(vcpu_id, read_stack(4), read_stack(8), read_stack(12)),
                323 => self.dos_query_app_type(read_stack(4), read_stack(8)),
                342 => 0, // DosQueryAppType old ordinal (stub)
                349 => self.dos_wait_thread(vcpu_id, read_stack(4)),
                352 => self.dos_get_resource(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                353 => self.dos_free_resource(read_stack(4)),
                572 => self.dos_query_resource_size(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                // Step 1: Critical init stubs
                212 => self.dos_error(read_stack(4)),
                209 => self.dos_set_max_fh(read_stack(4)),
                286 => self.dos_beep(read_stack(4), read_stack(8)),
                354 => self.dos_set_exception_handler(read_stack(4)),
                355 => self.dos_unset_exception_handler(read_stack(4)),
                356 => self.dos_set_signal_exception_focus(read_stack(4)),
                418 => self.dos_acknowledge_signal_exception(read_stack(4)),
                378 => self.dos_query_sys_state(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                380 => self.dos_enter_must_complete(read_stack(4)),
                381 => self.dos_exit_must_complete(read_stack(4)),
                // Step 2: Shared memory
                300 => self.dos_alloc_shared_mem(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                301 => self.dos_get_named_shared_mem(read_stack(4), read_stack(8), read_stack(12)),
                302 => self.dos_get_shared_mem(read_stack(4), read_stack(8)),
                305 => self.dos_set_mem(read_stack(4), read_stack(8), read_stack(12)),
                306 => self.dos_query_mem(read_stack(4), read_stack(8), read_stack(12)),
                // Step 3: Codepage and country info
                291 => self.dos_query_cp(read_stack(4), read_stack(8), read_stack(12)),
                289 => self.dos_set_process_cp(read_stack(4)),
                397 => self.dos_query_ctry_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                // Step 4: Module loading stubs
                318 => self.dos_load_module(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                322 => self.dos_free_module(read_stack(4)),
                319 => self.dos_query_module_handle(read_stack(4), read_stack(8)),
                321 => self.dos_query_proc_addr(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                317 => self.dos_get_message(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                // Step 5: File metadata APIs
                258 => self.dos_copy(read_stack(4), read_stack(8), read_stack(12)),
                261 => self.dos_edit_name(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                279 => self.dos_set_file_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                267 => self.dos_set_file_mode(read_stack(4), read_stack(8)),
                219 => self.dos_set_path_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                276 => self.dos_query_fh_state(read_stack(4), read_stack(8)),
                277 => self.dos_set_fh_state(read_stack(4), read_stack(8)),
                297 => self.dos_query_fs_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                298 => self.dos_query_fs_attach(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                // Step 6: Device I/O stubs
                284 => self.dos_dev_ioctl(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28), read_stack(32), read_stack(36)),
                231 => self.dos_dev_config(read_stack(4), read_stack(8)),  // NOTE: may conflict with DosSetDateTime
                // Step 7: Semaphore extensions
                325 => self.dos_open_event_sem(read_stack(4), read_stack(8)),
                332 => self.dos_open_mutex_sem(read_stack(4), read_stack(8)),
                // Step 8: Named pipe stubs
                230 => self.dos_get_date_time(read_stack(4)),
                348 => self.dos_query_sys_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                // Additional APIs needed by 4OS2
                382 => self.dos_set_rel_max_fh(read_stack(4), read_stack(8)),
                272 => self.dos_set_file_size(read_stack(4), read_stack(8)),
                260 => self.dos_dup_handle(read_stack(4), read_stack(8)),
                254 => self.dos_reset_buffer(read_stack(4)),
                210 => self.dos_set_verify(read_stack(4)),
                225 => self.dos_query_verify(read_stack(4)),
                292 => self.dos_set_date_time(read_stack(4)),
                218 => self.dos_set_file_size(read_stack(4), read_stack(8)), // DosSetFileSize alias
                285 => { debug!("DosFSCtl stub"); 0 }, // DosFSCtl - stub
                357 => { debug!("DosUnwindException stub"); 0 }, // DosUnwindException - stub
                372 => self.dos_enum_attribute(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                428 => self.dos_set_file_locks(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                639 => self.dos_protect_set_file_locks(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24)),
                415 => { debug!("DosShutdown stub"); 0 }, // DosShutdown - stub
                425 => self.dos_flat_to_sel(read_stack(4)), // DosFlatToSel
                426 => self.dos_sel_to_flat(read_stack(4)), // DosSelToFlat
                241 => { debug!("DosConnectNPipe stub"); 0 }, // DosConnectNPipe - stub
                243 => { debug!("DosCreateNPipe stub"); 0 }, // DosCreateNPipe - stub
                250 => { debug!("DosSetNPHState stub"); 0 }, // DosSetNPHState - stub
                221 => self.dos_set_fh_state(read_stack(4), read_stack(8)), // alias for old ordinal
                224 => self.dos_query_h_type(read_stack(4), read_stack(8), read_stack(12)), // alias
                110 => { debug!("DosForceDelete stub (ord 110)"); self.dos_delete(read_stack(4)) },
                // 16-bit thunks
                8 => self.dos_get_info_seg(read_stack(4), read_stack(8)),
                75 => self.dos_query_file_mode_16(read_stack(4), read_stack(8)),
                84 => self.dos_set_file_mode(read_stack(4), read_stack(8)),
                _ => { warn!("Warning: Unknown API Ordinal {} on VCPU {}", ordinal, vcpu_id); 0 }
            };
            ApiResult::Normal(res)
        } else if ordinal < 2048 {
            // QUECALLS
            let res = match ordinal - 1024 {
                16 => self.dos_create_queue(read_stack(4), read_stack(8), read_stack(12)),
                10 => self.dos_open_queue(read_stack(4), read_stack(8), read_stack(12)),
                14 => self.dos_write_queue(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                9 => self.dos_read_queue(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28), read_stack(32)),
                11 => self.dos_close_queue(read_stack(4)),
                12 => { self.dos_purge_queue(read_stack(4)); 0 },
                13 => self.dos_query_queue(read_stack(4), read_stack(8)),
                _ => { warn!("Warning: Unknown QUECALLS Ordinal {} on VCPU {}", ordinal - 1024, vcpu_id); 0 }
            };
            ApiResult::Normal(res)
        } else if ordinal < PMGPI_BASE {
            // PMWIN
            self.handle_pmwin_call(vcpu, vcpu_id, ordinal - PMWIN_BASE)
        } else if ordinal < KBDCALLS_BASE {
            // PMGPI
            self.handle_pmgpi_call(vcpu, vcpu_id, ordinal - PMGPI_BASE)
        } else if ordinal < VIOCALLS_BASE {
            // KBDCALLS
            self.handle_kbdcalls(vcpu, vcpu_id, ordinal - KBDCALLS_BASE)
        } else if ordinal < SESMGR_BASE {
            // VIOCALLS
            self.handle_viocalls(vcpu, vcpu_id, ordinal - VIOCALLS_BASE)
        } else if ordinal < NLS_BASE {
            // SESMGR
            let sesmgr_ord = ordinal - SESMGR_BASE;
            warn!("SESMGR stub: ordinal {} on VCPU {}", sesmgr_ord, vcpu_id);
            ApiResult::Normal(0)
        } else if ordinal < MSG_BASE {
            // NLS (National Language Support)
            let nls_ord = ordinal - NLS_BASE;
            let res = match nls_ord {
                5 => {
                    // DosMapCase(cb, pcc, pch) — convert string to uppercase
                    let cb = read_stack(4);
                    let _pcc = read_stack(8);
                    let pch = read_stack(12);
                    debug!("NLS DosMapCase(cb={}, pch=0x{:08X})", cb, pch);
                    // Convert the buffer in-place to uppercase (ASCII)
                    for i in 0..cb {
                        if let Some(ch) = self.guest_read::<u8>(pch + i) {
                            if ch >= b'a' && ch <= b'z' {
                                let _ = self.guest_write::<u8>(pch + i, ch - 32);
                            }
                        }
                    }
                    0
                }
                6 => {
                    // UniCreateUconvObject — return error to skip Unicode init
                    warn!("NLS ordinal 6 stub — returning error");
                    ERROR_INVALID_FUNCTION
                }
                7 => {
                    // DosGetDBCSEv(cb, pcc, pch) — get DBCS lead byte ranges
                    let cb = read_stack(4);
                    let _pcc = read_stack(8);
                    let pch = read_stack(12);
                    debug!("NLS DosGetDBCSEv(cb={}, pch=0x{:08X})", cb, pch);
                    // Return empty DBCS lead byte table (no DBCS for Western locales)
                    if pch != 0 && cb >= 2 {
                        let _ = self.guest_write::<u16>(pch, 0); // empty table = two zero bytes
                    }
                    0
                }
                _ => {
                    warn!("NLS stub: ordinal {} on VCPU {}", nls_ord, vcpu_id);
                    0
                }
            };
            ApiResult::Normal(res)
        } else if ordinal < STUB_AREA_SIZE {
            // MSG
            let msg_ord = ordinal - MSG_BASE;
            warn!("MSG stub: ordinal {} on VCPU {}", msg_ord, vcpu_id);
            ApiResult::Normal(0)
        } else {
            warn!("Warning: Unknown API Base Ordinal {} on VCPU {}", ordinal, vcpu_id);
            ApiResult::Normal(0)
        }
    }

    /// Check whether a shutdown has been requested.
    pub(crate) fn shutting_down(&self) -> bool {
        self.shared.exit_requested.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn post_wm_quit(&self, hwnd: u32) {
        let wm = self.shared.window_mgr.lock_or_recover();
        let hmq = wm.find_hmq_for_hwnd(hwnd);
        if let Some(hmq) = hmq {
            if let Some(mq_arc) = wm.get_mq(hmq) {
                let mut mq = mq_arc.lock_or_recover();
                mq.messages.push_back(OS2Message {
                    hwnd, msg: WM_QUIT, mp1: 0, mp2: 0, time: 0, x: 0, y: 0,
                });
                mq.cond.notify_one();
            }
        }
    }

    pub(crate) fn map_color(&self, clr: u32) -> u32 {
        match clr {
            0 => 0x00000000, // Black
            1 => 0x000000FF, // Blue
            2 => 0x00FF0000, // Red
            3 => 0x00FF00FF, // Pink
            4 => 0x0000FF00, // Green
            5 => 0x0000FFFF, // Cyan
            6 => 0x00FFFF00, // Yellow
            7 => 0x00FFFFFF, // White
            _ => 0x00808080, // Grey
        }
    }
}

impl Drop for SharedState {
    fn drop(&mut self) { unsafe { libc::munmap(self.guest_mem as *mut libc::c_void, self.guest_mem_size); } }
}
