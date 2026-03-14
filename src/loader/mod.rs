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
pub mod locale;

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
use kvm_bindings::{kvm_userspace_memory_region, kvm_guest_debug, kvm_segment, KVM_GUESTDBG_ENABLE, KVM_GUESTDBG_USE_SW_BP};
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
    /// Executable name as provided on the command line
    pub exe_name: Mutex<String>,
    pub guest_mem: *mut u8,
    pub guest_mem_size: usize,
    pub next_tid: Mutex<u32>,
    pub threads: Mutex<HashMap<u32, thread::JoinHandle<()>>>,
    pub exit_requested: AtomicBool,
    pub exit_code: AtomicI32,
    pub locale: locale::Os2Locale,
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
            exe_name: Mutex::new(String::new()),
            guest_mem,
            guest_mem_size,
            next_tid: Mutex::new(1),
            threads: Mutex::new(HashMap::new()),
            exit_requested: AtomicBool::new(false),
            exit_code: AtomicI32::new(0),
            locale: locale::Os2Locale::from_host(),
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
                    // Trace VIO/KBD import fixups
                    if let FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal } = &record.target {
                        let module = lx_file.imported_modules.get((*module_ordinal as usize).wrapping_sub(1));
                        if let Some(m) = module {
                            if m == "VIOCALLS" || m == "KBDCALLS" {
                                debug!("  Fixup: {}.{} -> target 0x{:08X}, src_type=0x{:02X}",
                                       m, proc_ordinal, target_addr, record.source_type & 0x0F);
                            }
                        }
                    }
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
                                let offset16 = (target_addr & 0xFFFF) as u16;
                                let selector = 0x08u16; // flat code segment
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
        Ok(())
    }

    // ── NE (16-bit) loader methods ──

    /// Load an NE (16-bit) executable into guest memory.
    pub fn load_ne<P: AsRef<Path>>(&mut self, ne_file: &crate::ne::NeFile, path: P) -> io::Result<()> {
        let mut file = File::open(path)?;
        let shift = ne_file.header.alignment_shift_count;

        for (i, seg) in ne_file.segment_table.iter().enumerate() {
            let guest_addr = NE_SEGMENT_BASE + (i as u32) * TILE_SIZE;
            let data_len = seg.actual_data_length();
            let file_off = seg.file_offset(shift);

            if data_len > 0 && seg.data_offset != 0 {
                file.seek(SeekFrom::Start(file_off))?;
                let buf = self.guest_slice_mut(guest_addr, data_len as usize)
                    .expect("load_ne: segment target OOB");
                file.read_exact(buf)?;
            }
            debug!("  NE Segment {}: {} bytes at 0x{:08X} ({})",
                i + 1, data_len, guest_addr, if seg.is_code() { "CODE" } else { "DATA" });
        }

        self.apply_ne_fixups(ne_file, &mut file)
    }

    /// Apply NE relocations for all segments.
    fn apply_ne_fixups(&self, ne_file: &crate::ne::NeFile, _file: &mut File) -> io::Result<()> {
        use crate::ne::header::*;

        // Build segment-to-selector/base map (1-based segment numbers)
        let seg_base = |seg_num: u8| -> u32 {
            NE_SEGMENT_BASE + ((seg_num as u32).wrapping_sub(1)) * TILE_SIZE
        };
        let seg_selector = |seg_num: u8| -> u16 {
            let tile_idx = (NE_SEGMENT_BASE / TILE_SIZE) + (seg_num as u32).wrapping_sub(1);
            ((TILED_SEL_START_INDEX + tile_idx) * 8) as u16
        };

        for (seg_idx, relocs) in ne_file.relocations_by_segment.iter().enumerate() {
            let seg_guest_base = seg_base((seg_idx + 1) as u8);

            for reloc in relocs {
                // Resolve the target value
                let (target_offset, target_selector) = match &reloc.target {
                    NeRelocationTarget::InternalRef { segment_num, offset } => {
                        let sel = seg_selector(*segment_num);
                        debug!("  NE fixup: InternalRef seg{}+0x{:04X} -> sel=0x{:04X}, type={}",
                            segment_num, offset, sel, reloc.source_type);
                        (*offset, sel)
                    }
                    NeRelocationTarget::ImportOrdinal { module_index, ordinal } => {
                        let mod_name = ne_file.imported_modules
                            .get((*module_index as usize).wrapping_sub(1))
                            .map(|s| s.as_str())
                            .unwrap_or("?");
                        debug!("  NE fixup: {}.#{} -> thunk", mod_name, ordinal);
                        // Resolve to 16-bit thunk area: selector=NE_THUNK_SELECTOR, offset=module_base+ordinal
                        let thunk_offset = self.resolve_import_16(mod_name, *ordinal);
                        (thunk_offset, NE_THUNK_SELECTOR)
                    }
                    NeRelocationTarget::ImportName { module_index, name_offset: _ } => {
                        let mod_name = ne_file.imported_modules
                            .get((*module_index as usize).wrapping_sub(1))
                            .map(|s| s.as_str())
                            .unwrap_or("?");
                        warn!("NE import-by-name from {} not implemented", mod_name);
                        (0, NE_THUNK_SELECTOR)
                    }
                    NeRelocationTarget::OsFixup { fixup_type } => {
                        debug!("  NE OS fixup type {}", fixup_type);
                        // OS fixup type 1 = floating-point fixup (NOP on 386+)
                        continue;
                    }
                };

                let is_additive = reloc.flags & RELFLAG_ADDITIVE != 0;

                // Apply the fixup, following the chain for non-additive fixups
                let mut offset = reloc.source_offset;
                loop {
                    let patch_addr = seg_guest_base + offset as u32;
                    // Read the next chain offset BEFORE writing (non-additive fixups store chain pointers)
                    let next_offset = if !is_additive {
                        self.guest_read::<u16>(patch_addr).unwrap_or(0xFFFF)
                    } else {
                        0xFFFF
                    };

                    match reloc.source_type {
                        RELOC_FAR_POINTER => {
                            // Write offset:selector (4 bytes)
                            self.guest_write::<u16>(patch_addr, target_offset);
                            self.guest_write::<u16>(patch_addr + 2, target_selector);
                        }
                        RELOC_SELECTOR => {
                            // Write just the selector (2 bytes)
                            self.guest_write::<u16>(patch_addr, target_selector);
                        }
                        RELOC_OFFSET => {
                            // Write just the offset (2 bytes)
                            self.guest_write::<u16>(patch_addr, target_offset);
                        }
                        RELOC_LOBYTE => {
                            // Write low byte of offset
                            self.guest_write::<u8>(patch_addr, target_offset as u8);
                        }
                        _ => {
                            warn!("Unknown NE relocation source type: {}", reloc.source_type);
                        }
                    }

                    // Follow chain (non-additive: each location stores pointer to next)
                    if is_additive || next_offset == 0xFFFF {
                        break;
                    }
                    offset = next_offset;
                }
            }
        }
        Ok(())
    }

    /// Resolve a 16-bit import to an offset within the NE thunk tile.
    /// Uses the same module base offsets as 32-bit (DOSCALLS=0, KBDCALLS=4096, etc.)
    fn resolve_import_16(&self, module: &str, ordinal: u16) -> u16 {
        let base: u16 = if module == "DOSCALLS" { 0 }
            else if module == "VIOCALLS" { VIOCALLS_BASE as u16 }
            else if module == "KBDCALLS" { KBDCALLS_BASE as u16 }
            else if module == "SESMGR" { SESMGR_BASE as u16 }
            else if module == "NLS" { NLS_BASE as u16 }
            else if module == "MSG" { MSG_BASE as u16 }
            else {
                warn!("Unknown 16-bit import module: {} ordinal {}", module, ordinal);
                STUB_AREA_SIZE as u16 - 1
            };
        base + ordinal
    }

    /// Set up guest memory for an NE executable and return (entry_cs_sel, entry_ip, ss_sel, sp).
    fn setup_guest_ne(&self, ne_file: &crate::ne::NeFile) -> (u16, u16, u16, u16) {
        // Reuse common setup: TIB, PIB, environment, BDA
        let tib_base = TIB_BASE as u64;
        let exe_name = self.shared.exe_name.lock_or_recover().clone();
        let os2_exe = if exe_name.is_empty() { String::from("C:\\APP.EXE") }
        else {
            let basename = std::path::Path::new(&exe_name)
                .file_name()
                .map(|f| f.to_string_lossy().to_uppercase())
                .unwrap_or_else(|| "APP.EXE".into());
            format!("C:\\{}", basename)
        };
        let mut env_block: Vec<u8> = Vec::new();
        env_block.extend_from_slice(b"PATH=C:\\\0");
        env_block.extend_from_slice(b"COMSPEC=C:\\CMD.EXE\0");
        env_block.extend_from_slice(b"OS=OS2\0");
        env_block.push(0);
        let cmdline_offset = env_block.len() as u32;
        env_block.extend_from_slice(os2_exe.as_bytes());
        env_block.push(0);
        let env_addr = self.shared.mem_mgr.lock_or_recover()
            .alloc(env_block.len() as u32)
            .expect("setup_guest_ne: env alloc failed");
        let cmdline_addr = env_addr + cmdline_offset;
        self.guest_write_bytes(env_addr, &env_block).expect("setup_guest_ne: env write OOB");

        // TIB/PIB setup
        let tib2_addr = TIB_BASE + 0x40;
        self.guest_write::<u32>(TIB_BASE + 0x0C, tib2_addr).unwrap();
        self.guest_write::<u32>(TIB_BASE + 0x18, TIB_BASE).unwrap();
        self.guest_write::<u32>(TIB_BASE + 0x30, PIB_BASE).unwrap();
        self.guest_write::<u32>(tib2_addr + 0x00, 1).unwrap();
        self.guest_write::<u32>(tib2_addr + 0x04, 0).unwrap();
        self.guest_write::<u32>(PIB_BASE + 0x00, 42).unwrap();
        self.guest_write::<u32>(PIB_BASE + 0x0C, cmdline_addr).unwrap();
        self.guest_write::<u32>(PIB_BASE + 0x10, env_addr).unwrap();

        // BDA
        {
            let console = self.shared.console_mgr.lock_or_recover();
            let cols = console.cols as u16;
            let rows = console.rows as u16;
            drop(console);
            self.guest_write::<u8>(0x449, 0x03).unwrap();
            self.guest_write::<u16>(0x44A, cols).unwrap();
            self.guest_write::<u16>(0x44C, cols * rows * 2).unwrap();
            self.guest_write::<u16>(0x44E, 0).unwrap();
            self.guest_write::<u16>(0x450, 0).unwrap();
            self.guest_write::<u8>(0x462, 0).unwrap();
            self.guest_write::<u16>(0x463, 0x3D4).unwrap();
            self.guest_write::<u8>(0x484, (rows - 1) as u8).unwrap();
            self.guest_write::<u16>(0x485, 16).unwrap();
        }

        // Set up 32-bit API stubs (still needed for some internal dispatches)
        self.setup_stubs();

        // Set up 16-bit API thunk tile at NE_THUNK_BASE: fill with INT 3
        for i in 0..TILE_SIZE {
            self.guest_write::<u8>(NE_THUNK_BASE + i, 0xCC).expect("setup_ne_thunks: write OOB");
        }

        // Set up GDT with NE segment descriptors
        // First, set up the standard entries (null, code32, data32, fs)
        self.guest_write::<u64>(GDT_BASE, 0).unwrap();
        self.guest_write::<u64>(GDT_BASE + 8, Self::make_gdt_entry(0, 0xFFFFF, 0x9B, 0xCF)).unwrap();
        self.guest_write::<u64>(GDT_BASE + 16, Self::make_gdt_entry(0, 0xFFFFF, 0x93, 0xCF)).unwrap();
        self.guest_write::<u64>(GDT_BASE + 24, Self::make_gdt_entry(TIB_BASE, 0xFFF, 0x93, 0xCF)).unwrap();

        // 16-bit thunk code segment (NE_THUNK_GDT_INDEX)
        let thunk_gdt_offset = GDT_BASE + NE_THUNK_GDT_INDEX * 8;
        self.guest_write::<u64>(thunk_gdt_offset,
            Self::make_gdt_entry(NE_THUNK_BASE, 0xFFFF, 0x9B, 0x00)).unwrap(); // 16-bit code, exec+read
        debug!("  GDT[{}] = 16-bit thunk code at 0x{:08X}, selector=0x{:04X}",
            NE_THUNK_GDT_INDEX, NE_THUNK_BASE, NE_THUNK_SELECTOR);

        // NE segment descriptors
        let mut max_gdt_index = NE_THUNK_GDT_INDEX;
        for (i, seg) in ne_file.segment_table.iter().enumerate() {
            let guest_base = NE_SEGMENT_BASE + (i as u32) * TILE_SIZE;
            let tile_idx = guest_base / TILE_SIZE;
            let gdt_idx = TILED_SEL_START_INDEX + tile_idx;
            let selector = gdt_idx * 8;
            let limit = seg.actual_min_alloc().saturating_sub(1).min(0xFFFF);
            let access = if seg.is_code() { 0x9B } else { 0x93 }; // code exec+read or data read+write
            let gdt_offset = GDT_BASE + gdt_idx * 8;
            self.guest_write::<u64>(gdt_offset,
                Self::make_gdt_entry(guest_base, limit, access, 0x00)).unwrap(); // 16-bit, byte granularity
            debug!("  GDT[{}] = NE seg {} ({}) at 0x{:08X}, limit=0x{:04X}, selector=0x{:04X}",
                gdt_idx, i + 1, if seg.is_code() { "CODE" } else { "DATA" },
                guest_base, limit, selector);
            if gdt_idx > max_gdt_index { max_gdt_index = gdt_idx; }
        }

        // Set up IDT (for exception handling)
        {
            const NUM_VECTORS: u32 = 32;
            for i in 0..NUM_VECTORS {
                let handler_addr = IDT_HANDLER_BASE + i * 16;
                let has_error_code = matches!(i, 8 | 10 | 11 | 12 | 13 | 14 | 17);
                let mut off = 0u32;
                if !has_error_code {
                    self.guest_write::<u8>(handler_addr + off, 0x6A).unwrap();
                    self.guest_write::<u8>(handler_addr + off + 1, 0x00).unwrap();
                    off += 2;
                }
                self.guest_write::<u8>(handler_addr + off, 0x6A).unwrap();
                self.guest_write::<u8>(handler_addr + off + 1, i as u8).unwrap();
                off += 2;
                self.guest_write::<u8>(handler_addr + off, 0xCC).unwrap();

                let idt_entry_addr = IDT_BASE + i * 8;
                let offset_lo = (handler_addr & 0xFFFF) as u16;
                let offset_hi = ((handler_addr >> 16) & 0xFFFF) as u16;
                self.guest_write::<u16>(idt_entry_addr, offset_lo).unwrap();
                self.guest_write::<u16>(idt_entry_addr + 2, 0x08).unwrap();
                self.guest_write::<u16>(idt_entry_addr + 4, 0x8E00).unwrap();
                self.guest_write::<u16>(idt_entry_addr + 6, offset_hi).unwrap();
            }
        }

        // Return entry point and stack as selectors
        let entry_cs = ne_file.header.entry_cs();
        let entry_ip = ne_file.header.entry_ip();
        let stack_ss = ne_file.header.stack_ss();
        let stack_sp = ne_file.header.stack_sp();

        let cs_sel = if entry_cs > 0 {
            let tile_idx = (NE_SEGMENT_BASE / TILE_SIZE) + (entry_cs as u32 - 1);
            ((TILED_SEL_START_INDEX + tile_idx) * 8) as u16
        } else { 0x08 }; // fallback to 32-bit code

        let ss_sel = if stack_ss > 0 {
            let tile_idx = (NE_SEGMENT_BASE / TILE_SIZE) + (stack_ss as u32 - 1);
            ((TILED_SEL_START_INDEX + tile_idx) * 8) as u16
        } else { 0x10 }; // fallback to 32-bit data

        // SP=0 in 16-bit means top of segment (wrap around)
        let actual_sp = if stack_sp == 0 {
            let ss_seg = &ne_file.segment_table[(stack_ss as usize).wrapping_sub(1)];
            ss_seg.actual_min_alloc() as u16
        } else { stack_sp };

        info!("NE entry: CS:IP = 0x{:04X}:0x{:04X}, SS:SP = 0x{:04X}:0x{:04X}",
            cs_sel, entry_ip, ss_sel, actual_sp);

        (cs_sel, entry_ip, ss_sel, actual_sp)
    }

    /// Run an NE (16-bit) CLI application.
    pub fn setup_and_run_ne_cli(self, ne_file: &crate::ne::NeFile) -> ! {
        let (cs_sel, entry_ip, ss_sel, sp) = self.setup_guest_ne(ne_file);

        let vcpu = self.vm.create_vcpu(0).unwrap();
        let mut regs = vcpu.get_regs().unwrap();
        regs.rip = entry_ip as u64;
        regs.rsp = sp as u64;
        regs.rflags = 2;
        vcpu.set_regs(&regs).unwrap();

        // Set up 16-bit protected mode segments
        let mut sregs = vcpu.get_sregs().unwrap();
        // GDT
        sregs.gdt.base = GDT_BASE as u64;
        // GDT must cover all NE segment entries + thunk entry
        let last_seg = ne_file.segment_table.len() as u32;
        let last_tile_idx = (NE_SEGMENT_BASE / TILE_SIZE) + last_seg.saturating_sub(1);
        let max_gdt_idx = (TILED_SEL_START_INDEX + last_tile_idx).max(NE_THUNK_GDT_INDEX);
        sregs.gdt.limit = ((max_gdt_idx + 1) * 8 - 1) as u16;
        // IDT
        sregs.idt.base = IDT_BASE as u64;
        sregs.idt.limit = (32 * 8 - 1) as u16;
        // CR0: protected mode enabled
        sregs.cr0 = 0x00000011; // PE + ET

        // CS: 16-bit code segment
        let cs_base = self.gdt_entry_base(cs_sel);
        let cs_limit = self.gdt_entry_limit(cs_sel);
        sregs.cs = kvm_segment {
            base: cs_base as u64, limit: cs_limit,
            selector: cs_sel, type_: 11, present: 1, dpl: 0, db: 0, s: 1, l: 0, g: 0, avl: 0, unusable: 0, padding: 0
        };

        // DS/ES: data segment (auto data segment or same as SS)
        let ds_sel = ss_sel; // Use stack segment as default data segment
        let ds_base = self.gdt_entry_base(ds_sel);
        let ds_limit = self.gdt_entry_limit(ds_sel);
        let ds_seg = kvm_segment {
            base: ds_base as u64, limit: ds_limit,
            selector: ds_sel, type_: 3, present: 1, dpl: 0, db: 0, s: 1, l: 0, g: 0, avl: 0, unusable: 0, padding: 0
        };
        sregs.ds = ds_seg;
        sregs.es = ds_seg;

        // SS: stack segment
        let ss_base = self.gdt_entry_base(ss_sel);
        let ss_limit = self.gdt_entry_limit(ss_sel);
        sregs.ss = kvm_segment {
            base: ss_base as u64, limit: ss_limit,
            selector: ss_sel, type_: 3, present: 1, dpl: 0, db: 0, s: 1, l: 0, g: 0, avl: 0, unusable: 0, padding: 0
        };

        // FS/GS: use 32-bit flat data for now
        let flat_seg = kvm_segment {
            base: 0, limit: 0xFFFFFFFF,
            selector: 0x10, type_: 3, present: 1, dpl: 0, db: 1, s: 1, l: 0, g: 1, avl: 0, unusable: 0, padding: 0
        };
        sregs.fs = flat_seg;
        sregs.gs = flat_seg;

        vcpu.set_sregs(&sregs).unwrap();

        // Enable guest debugging for INT 3 breakpoints
        let debug = kvm_guest_debug { control: KVM_GUESTDBG_ENABLE | KVM_GUESTDBG_USE_SW_BP, ..Default::default() };
        vcpu.set_guest_debug(&debug).unwrap();

        info!("Starting NE 16-bit execution at 0x{:04X}:0x{:04X}", cs_sel, entry_ip);
        self.run_vcpu(vcpu, 0, TIB_BASE as u64);

        self.shared.console_mgr.lock_or_recover().disable_raw_mode();
        let code = self.shared.exit_code.load(std::sync::atomic::Ordering::Relaxed);
        std::process::exit(code);
    }

    /// Handle a 16-bit NE API call. Ordinal includes module base offset.
    fn handle_ne_api_call(&self, vcpu: &mut VcpuFd, vcpu_id: u32, ordinal: u16) -> u32 {
        let regs = vcpu.get_regs().unwrap();
        let sregs = vcpu.get_sregs().unwrap();
        let ss_base = sregs.ss.base as u32;
        let sp = regs.rsp as u16;
        // Stack after CALL FAR: [ret_IP:16] [ret_CS:16] [args...]
        // Args start at SP+4 (Pascal: pushed left-to-right, last arg closest to return addr)
        let arg_base = ss_base + sp as u32 + 4;

        // Read a 16-bit word from the argument area
        let read_arg16 = |off: u32| -> u16 {
            self.guest_read::<u16>(arg_base + off).unwrap_or(0)
        };
        // Read a far pointer (offset:segment) and convert to flat address
        let read_far_ptr = |off: u32| -> u32 {
            let ptr_off = read_arg16(off) as u32;
            let ptr_seg = read_arg16(off + 2);
            self.gdt_entry_base(ptr_seg) + ptr_off
        };

        // Dispatch based on module base + ordinal
        // 16-bit DOSCALLS ordinals (0..4095)
        if (ordinal as u32) < KBDCALLS_BASE {
            match ordinal {
                // DosExit(fTerminate:16, usExitCode:16) — Pascal: last pushed first
                // Stack: [ret] [usExitCode:16] [fTerminate:16]
                5 => {
                    let exit_code = read_arg16(0);
                    let _terminate = read_arg16(2);
                    debug!("  16-bit DosExit(code={})", exit_code);
                    self.shared.exit_code.store(exit_code as i32, std::sync::atomic::Ordering::Relaxed);
                    self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                    0
                }
                // DosWrite(hf:16, pBuf:far, cbBuf:16, pcbWritten:far) — Pascal
                // Pushed: hf, pBuf(seg:off), cbBuf, pcbWritten(seg:off)
                // Stack (after ret): [pcbWritten_off:16] [pcbWritten_seg:16] [cbBuf:16] [pBuf_off:16] [pBuf_seg:16] [hf:16]
                138 => {
                    let pcb_written = read_far_ptr(0);
                    let cb_buf = read_arg16(4);
                    let p_buf = read_far_ptr(6);
                    let hf = read_arg16(10);
                    debug!("  16-bit DosWrite(hf={}, buf=0x{:08X}, cb={}, pcb=0x{:08X})",
                        hf, p_buf, cb_buf, pcb_written);
                    if let Some(data) = self.guest_slice_mut(p_buf, cb_buf as usize) {
                        if hf == 1 || hf == 2 {
                            match crate::api::doscalls::dos_write(hf as u32, data) {
                                Ok(actual) => {
                                    if pcb_written != 0 {
                                        self.guest_write::<u16>(pcb_written, actual as u16);
                                    }
                                    return 0;
                                }
                                Err(_) => return 1,
                            }
                        }
                    }
                    1 // error
                }
                // DosGetInfoSeg(pGlobalSeg:far, pLocalSeg:far) — ordinal 8
                8 => {
                    debug!("  16-bit DosGetInfoSeg (stub)");
                    0
                }
                // DosSetSigHandler — ordinal 41
                41 => { debug!("  16-bit DosSetSigHandler (stub)"); 0 }
                // DosSetVec (exception vector) — ordinal 49
                49 => { debug!("  16-bit DosSetVec (stub)"); 0 }
                // DosGetInfoSeg (16-bit, get GINFOSEG/LINFOSEG selectors) — ordinal 8
                // Already handled above
                // DosGetHugeShift — ordinal 41? No, that's DosSetSigHandler
                // DosGetMachineMode — ordinal 49? No, that's DosSetVec
                // DosGetPID — ordinal 92
                92 => { debug!("  16-bit DosGetInfoSeg (ordinal 92, stub)"); 0 }
                // DosGetEnv — ordinal 94
                94 => {
                    // Returns environment selector and command line offset
                    // Pascal: DosGetEnv(pEnvSel:far, pCmdOffset:far)
                    let p_cmd_offset = read_far_ptr(0);
                    let p_env_sel = read_far_ptr(4);
                    debug!("  16-bit DosGetEnv(pEnvSel=0x{:08X}, pCmdOffset=0x{:08X})", p_env_sel, p_cmd_offset);
                    // Write environment selector (use data segment selector)
                    // and command line offset (0 for now)
                    if p_env_sel != 0 {
                        self.guest_write::<u16>(p_env_sel, 0); // env selector (stub)
                    }
                    if p_cmd_offset != 0 {
                        self.guest_write::<u16>(p_cmd_offset, 0); // cmd offset
                    }
                    0
                }
                _ => {
                    warn!("  [VCPU {}] Unimplemented 16-bit DOSCALLS ordinal {}", vcpu_id, ordinal);
                    0
                }
            }
        } else {
            warn!("  [VCPU {}] Unimplemented 16-bit API ordinal {} (module base {})",
                vcpu_id, ordinal, if ordinal as u32 >= VIOCALLS_BASE { "VIOCALLS" }
                else if ordinal as u32 >= KBDCALLS_BASE { "KBDCALLS" }
                else { "?" });
            0
        }
    }

    /// Return the number of argument bytes for Pascal callee cleanup of a 16-bit API.
    fn ne_api_arg_bytes(&self, ordinal: u16) -> u16 {
        if (ordinal as u32) < KBDCALLS_BASE {
            match ordinal {
                5 => 4,    // DosExit: 2 + 2
                8 => 8,    // DosGetInfoSeg: 4 + 4
                41 => 10,  // DosSetSigHandler: 4 + 2 + 4 (pfnSigHandler:far, fAction:16, pPrevHandler:far, ...)
                49 => 8,   // DosSetVec: 2 + 4 + 4? approximately
                92 => 4,   // DosGetInfoSeg: far pointer
                94 => 8,   // DosGetEnv: 4 + 4 (pEnvSel:far, pCmdOffset:far)
                138 => 12, // DosWrite: 2 + 4 + 2 + 4
                _ => 0,    // Unknown
            }
        } else if (ordinal as u32) < VIOCALLS_BASE + 1024 {
            let vio_ord = ordinal as u32 - VIOCALLS_BASE;
            match vio_ord {
                19 => 8,  // VioWrtTTY: 4 + 2 + 2 (pStr:far, cb:16, hvio:16)
                _ => 0,
            }
        } else {
            0
        }
    }

    /// Read GDT entry base address from guest memory.
    fn gdt_entry_base(&self, selector: u16) -> u32 {
        let gdt_idx = (selector / 8) as u32;
        let entry = self.guest_read::<u64>(GDT_BASE + gdt_idx * 8).unwrap_or(0);
        let base_lo = ((entry >> 16) & 0xFFFF) as u32;
        let base_mid = ((entry >> 32) & 0xFF) as u32;
        let base_hi = ((entry >> 56) & 0xFF) as u32;
        base_lo | (base_mid << 16) | (base_hi << 24)
    }

    /// Read GDT entry limit from guest memory.
    fn gdt_entry_limit(&self, selector: u16) -> u32 {
        let gdt_idx = (selector / 8) as u32;
        let entry = self.guest_read::<u64>(GDT_BASE + gdt_idx * 8).unwrap_or(0);
        let limit_lo = (entry & 0xFFFF) as u32;
        let limit_hi = ((entry >> 48) & 0x0F) as u32;
        limit_lo | (limit_hi << 16)
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

    /// Build a GDT descriptor entry.
    fn make_gdt_entry(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
        let mut entry: u64 = 0;
        entry |= (limit & 0xFFFF) as u64;
        entry |= ((base & 0xFFFF) as u64) << 16;
        entry |= (((base >> 16) & 0xFF) as u64) << 32;
        entry |= (access as u64) << 40;
        entry |= ((((limit >> 16) & 0x0F) as u64) | ((flags as u64) & 0xF0)) << 48;
        entry |= (((base >> 24) & 0xFF) as u64) << 56;
        entry
    }

    /// Set up GDT (with 16-bit tiled segments) and IDT for CPU exception handling.
    ///
    /// GDT layout:
    ///   [0] null, [1] 32-bit code (0x08), [2] 32-bit data (0x10), [3] FS data (0x18),
    ///   [4..4099] 16-bit data tiles (0x20..0x7FF8) — one per 64KB segment of guest memory.
    ///
    /// The tiled 16-bit descriptors allow OS/2 16:16 (selector:offset) addressing to work
    /// correctly for LSS, JMP FAR, CALL FAR, and other segmented instructions.
    fn setup_idt(&self) {
        const NUM_VECTORS: u32 = 32;

        // ── GDT entries ──

        // Entry 0: null descriptor
        self.guest_write::<u64>(GDT_BASE, 0).unwrap();
        // Entry 1 (selector 0x08): 32-bit code — base=0, limit=4GB, exec/read
        self.guest_write::<u64>(GDT_BASE + 8, Self::make_gdt_entry(0, 0xFFFFF, 0x9B, 0xCF)).unwrap();
        // Entry 2 (selector 0x10): 32-bit data — base=0, limit=4GB, read/write
        self.guest_write::<u64>(GDT_BASE + 16, Self::make_gdt_entry(0, 0xFFFFF, 0x93, 0xCF)).unwrap();
        // Entry 3 (selector 0x18): FS data — base set via sregs
        self.guest_write::<u64>(GDT_BASE + 24, Self::make_gdt_entry(0, 0xFFFFF, 0x93, 0xCF)).unwrap();

        // GDT tiling constants (TILED_SEL_START_INDEX, NUM_TILES, etc.) are
        // defined in constants.rs for future Phase 5 NE (16-bit) app support.
        // Tiled 16-bit segment descriptors are NOT populated — the preferred
        // approach for 32-bit LX apps is to recompile with modified bsesub.h
        // (APIENTRY16 → _System) to eliminate 16-bit thunks at the source level.
        debug!("GDT: 4 entries (tiling reserved for Phase 5)");

        // ── IDT with exception handler stubs ──
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
        // TIB2 is placed right after TIB (TIB is ~0x40 bytes, TIB2 at TIB_BASE + 0x40)
        let tib2_addr = TIB_BASE + 0x40;
        self.guest_write::<u32>(TIB_BASE + 0x0C, tib2_addr).expect("setup_guest: TIB.ptib2 OOB"); // tib_ptib2
        self.guest_write::<u32>(TIB_BASE + 0x18, TIB_BASE).expect("setup_guest: TIB self-ptr OOB");
        self.guest_write::<u32>(TIB_BASE + 0x30, PIB_BASE).expect("setup_guest: TIB->PIB OOB");
        // TIB2 fields
        self.guest_write::<u32>(tib2_addr + 0x00, 1).expect("setup_guest: TIB2.tid OOB"); // tib2_ultid = thread 1
        self.guest_write::<u32>(tib2_addr + 0x04, 0).expect("setup_guest: TIB2.pri OOB"); // tib2_ulpri = normal
        self.guest_write::<u32>(PIB_BASE + 0x00, 42).expect("setup_guest: PIB PID OOB");
        self.guest_write::<u32>(PIB_BASE + 0x0C, cmdline_addr).expect("setup_guest: PIB pchcmd OOB");
        self.guest_write::<u32>(PIB_BASE + 0x10, env_addr).expect("setup_guest: PIB pchenv OOB");

        // Initialize BIOS Data Area (BDA) at flat address 0x400 with VGA 80x25 text mode.
        // Some OS/2 code and C runtime libraries read BDA directly for screen dimensions.
        {
            let console = self.shared.console_mgr.lock_or_recover();
            let cols = console.cols as u16;
            let rows = console.rows as u16;
            drop(console);
            self.guest_write::<u8>(0x449, 0x03).unwrap();         // Current video mode: VGA 80x25 color text
            self.guest_write::<u16>(0x44A, cols).unwrap();         // Number of columns
            self.guest_write::<u16>(0x44C, cols * rows * 2).unwrap(); // Page size (chars * 2 bytes each)
            self.guest_write::<u16>(0x44E, 0).unwrap();            // Current page offset
            self.guest_write::<u16>(0x450, 0).unwrap();            // Cursor position page 0 (row=0, col=0)
            self.guest_write::<u8>(0x462, 0).unwrap();             // Current display page
            self.guest_write::<u16>(0x463, 0x3D4).unwrap();        // CRTC base I/O port (color)
            self.guest_write::<u8>(0x484, (rows - 1) as u8).unwrap(); // Number of rows - 1
            self.guest_write::<u16>(0x485, 16).unwrap();           // Character height (16 scanlines)
            debug!("BDA initialized: VGA mode 0x03, {}x{} text", cols, rows);
        }

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
        // Restore terminal before process::exit() which skips all destructors.
        // Must restore termios FIRST so OPOST is active, then emit ANSI reset.
        self.shared.console_mgr.lock_or_recover().disable_raw_mode();
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
        self.shared.console_mgr.lock_or_recover().disable_raw_mode();
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
        sregs.gdt.base = GDT_BASE as u64;
        sregs.gdt.limit = 4 * 8 - 1; // 4 entries (tiling reserved for Phase 5)
        sregs.idt.base = IDT_BASE as u64;
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
                    if rip >= IDT_HANDLER_BASE as u64 && rip < IDT_HANDLER_BASE as u64 + 32 * 16 {
                        let regs = vcpu.get_regs().unwrap();
                        // Stack layout: [vector_num] [error_code_or_fake] [fault_EIP] [fault_CS] [fault_EFLAGS]
                        let vector = self.guest_read::<u32>(regs.rsp as u32).unwrap_or(0xFF);
                        let error_code = self.guest_read::<u32>(regs.rsp as u32 + 4).unwrap_or(0);
                        let fault_eip = self.guest_read::<u32>(regs.rsp as u32 + 8).unwrap_or(0);
                        let fault_cs = self.guest_read::<u32>(regs.rsp as u32 + 12).unwrap_or(0);
                        let fault_eflags = self.guest_read::<u32>(regs.rsp as u32 + 16).unwrap_or(0);

                        error!("  [VCPU {}] CPU Exception #{} at EIP=0x{:08X} CS=0x{:04X} EFLAGS=0x{:08X} ErrorCode=0x{:X}",
                               vcpu_id, vector, fault_eip, fault_cs, fault_eflags, error_code);
                        error!("    ESP=0x{:08X} EAX=0x{:08X} EBX=0x{:08X} ECX=0x{:08X} EDX=0x{:08X}",
                               regs.rsp, regs.rax, regs.rbx, regs.rcx, regs.rdx);
                        self.shared.exit_code.store(1, std::sync::atomic::Ordering::Relaxed);
                        self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    // 16-bit NE API thunk: breakpoint in the NE thunk tile
                    if rip >= NE_THUNK_BASE as u64 && rip < (NE_THUNK_BASE + TILE_SIZE) as u64 {
                        let ordinal = (rip - NE_THUNK_BASE as u64) as u16;
                        {
                            let regs_dbg = vcpu.get_regs().unwrap();
                            let sregs_dbg = vcpu.get_sregs().unwrap();
                            let ss_base_dbg = sregs_dbg.ss.base as u32;
                            let sp_dbg = regs_dbg.rsp as u16;
                            let ret_ip_dbg = self.guest_read::<u16>(ss_base_dbg + sp_dbg as u32).unwrap_or(0);
                            let ret_cs_dbg = self.guest_read::<u16>(ss_base_dbg + sp_dbg as u32 + 2).unwrap_or(0);
                            debug!("  [VCPU {}] 16-bit API call: ordinal {} at 0x{:08X}, ret=0x{:04X}:0x{:04X}, SP=0x{:04X}",
                                vcpu_id, ordinal, rip, ret_cs_dbg, ret_ip_dbg, sp_dbg);
                        }
                        let result = self.handle_ne_api_call(&mut vcpu, vcpu_id, ordinal);
                        let mut regs = vcpu.get_regs().unwrap();
                        regs.rax = result as u64;
                        // Pop far return address (IP + CS = 4 bytes) and restore CS:IP
                        let sregs = vcpu.get_sregs().unwrap();
                        let ss_base = sregs.ss.base as u32;
                        let sp = regs.rsp as u16;
                        let ret_ip = self.guest_read::<u16>(ss_base + sp as u32).unwrap_or(0);
                        let ret_cs_sel = self.guest_read::<u16>(ss_base + sp as u32 + 2).unwrap_or(0);
                        regs.rsp = (sp.wrapping_add(4)) as u64; // pop IP + CS
                        // Pascal callee cleanup: pop arguments
                        let arg_bytes = self.ne_api_arg_bytes(ordinal);
                        regs.rsp = ((regs.rsp as u16).wrapping_add(arg_bytes)) as u64;
                        // Set return CS:IP
                        regs.rip = ret_ip as u64;
                        vcpu.set_regs(&regs).unwrap();
                        // Update CS segment register to the return code segment
                        let mut sregs = vcpu.get_sregs().unwrap();
                        let cs_base = self.gdt_entry_base(ret_cs_sel);
                        let cs_limit = self.gdt_entry_limit(ret_cs_sel);
                        sregs.cs.base = cs_base as u64;
                        sregs.cs.limit = cs_limit;
                        sregs.cs.selector = ret_cs_sel;
                        sregs.cs.type_ = 11; // code, exec+read
                        sregs.cs.db = 0; // 16-bit
                        sregs.cs.present = 1;
                        sregs.cs.s = 1;
                        vcpu.set_sregs(&sregs).unwrap();
                        continue;
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
                                // VIO/KBD use Pascal calling convention (callee cleanup).
                                // Pop the arguments from the stack after the return address.
                                if ordinal >= VIOCALLS_BASE && ordinal < SESMGR_BASE {
                                    regs.rsp += self.viocalls_arg_bytes(ordinal - VIOCALLS_BASE);
                                } else if ordinal >= KBDCALLS_BASE && ordinal < VIOCALLS_BASE {
                                    regs.rsp += self.kbdcalls_arg_bytes(ordinal - KBDCALLS_BASE);
                                }
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
                239 => self.dos_create_pipe(read_stack(4), read_stack(8), read_stack(12)),
                312 => self.dos_get_info_blocks(vcpu, read_stack(4), read_stack(8)),
                283 => self.dos_exec_pgm(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                280 => self.dos_wait_child(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                235 => self.dos_kill_process(read_stack(4), read_stack(8)), // DosKillProcess
                264 => self.dos_find_first(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                265 => self.dos_find_next(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                263 => self.dos_find_close(read_stack(4)),
                223 => self.dos_query_path_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                // Directory management
                255 => self.dos_set_current_dir(read_stack(4)),
                274 => self.dos_query_current_dir(read_stack(4), read_stack(8), read_stack(12)),
                275 => self.dos_query_current_disk(read_stack(4), read_stack(8)),
                220 => self.dos_set_default_disk(read_stack(4)),
                278 => self.dos_query_fs_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
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
                342 => { debug!("DosDeleteMuxWaitSem stub"); 0 }, // DosDeleteMuxWaitSem
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
                356 => { debug!("DosRaiseException stub"); 0 }, // DosRaiseException
                418 => self.dos_acknowledge_signal_exception(read_stack(4)),
                368 => self.dos_query_sys_state(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)), // DosQuerySysState
                378 => self.dos_set_signal_exception_focus(read_stack(4)), // DosSetSignalExceptionFocus
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
                397 => self.dos_query_ctry_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)), // DosQueryCtryInfo
                // Step 4: Module loading stubs
                318 => self.dos_load_module(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                322 => self.dos_free_module(read_stack(4)),
                319 => self.dos_query_module_handle(read_stack(4), read_stack(8)),
                321 => self.dos_query_proc_addr(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                317 => { debug!("DosDebug stub"); 87 }, // DosDebug (not implemented)
                // Step 5: File metadata APIs
                258 => self.dos_copy(read_stack(4), read_stack(8), read_stack(12)),
                261 => self.dos_edit_name(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                279 => self.dos_query_file_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                267 => { debug!("DOS16REQUESTVDD stub"); 0 }, // DOS16REQUESTVDD
                219 => self.dos_set_path_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                276 => self.dos_query_fh_state(read_stack(4), read_stack(8)),
                277 => self.dos_query_fs_attach(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)), // DosQueryFSAttach
                // Ordinals 297/298 do not exist in DOSCALLS — removed (were phantom duplicates)
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
                218 => self.dos_set_file_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)), // DosSetFileInfo
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
            // QUECALLS ordinals from doc/os2_ordinals.md
            let res = match ordinal - 1024 {
                16 => self.dos_create_queue(read_stack(4), read_stack(8), read_stack(12)),   // DosCreateQueue
                15 => self.dos_open_queue(read_stack(4), read_stack(8), read_stack(12)),     // DosOpenQueue
                14 => self.dos_write_queue(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)), // DosWriteQueue
                9  => self.dos_read_queue(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28), read_stack(32)), // DosReadQueue
                11 => self.dos_close_queue(read_stack(4)),                                   // DosCloseQueue
                10 => { self.dos_purge_queue(read_stack(4)); 0 },                           // DosPurgeQueue
                12 => self.dos_query_queue(read_stack(4), read_stack(8)),                    // DosQueryQueue
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
                    // NLS ordinal 5 — _System convention
                    // On real OS/2, this is DosQueryCp for small cb (codepage query).
                    // But the CRT wrapper calls it with cb=44 (sizeof COUNTRYINFO)
                    // to retrieve full country information. When cb >= 44, the
                    // layout appears to be: (cb, pcc, pci_output)
                    let cb = read_stack(4);
                    let arg2 = read_stack(8);
                    let arg3 = read_stack(12);
                    debug!("NLS ordinal 5: cb={} arg2=0x{:08X} arg3=0x{:08X}", cb, arg2, arg3);
                    if cb >= 44 {
                        // Return full COUNTRYINFO to arg3 (the output buffer)
                        self.dos_query_ctry_info(cb, arg2, arg3, 0)
                    } else {
                        // Standard DosQueryCp: (cb, pCP, pcb)
                        self.dos_query_cp(cb, arg2, arg3)
                    }
                }
                6 => {
                    // DosQueryCtryInfo(cb, pcc, pci, pcb_actual) — _System convention
                    let cb = read_stack(4);
                    let pcc = read_stack(8);
                    let pci = read_stack(12);
                    let pcb = read_stack(16);
                    debug!("NLS DosQueryCtryInfo: cb={} pcc=0x{:08X} pci=0x{:08X} pcb=0x{:08X}", cb, pcc, pci, pcb);
                    self.dos_query_ctry_info(cb, pcc, pci, pcb)
                }
                7 => {
                    // DosMapCase(cb, pcc, pch) — _System convention
                    let cb = read_stack(4);
                    let _pcc = read_stack(8);
                    let pch = read_stack(12);
                    debug!("NLS DosMapCase(cb={}, pch=0x{:08X})", cb, pch);
                    for i in 0..cb {
                        if let Some(ch) = self.guest_read::<u8>(pch + i) {
                            if ch >= b'a' && ch <= b'z' {
                                let _ = self.guest_write::<u8>(pch + i, ch - 32);
                            }
                        }
                    }
                    0
                }
                8 => {
                    // DosGetDBCSEv(cb, pcc, pch) — _System convention
                    let cb = read_stack(4);
                    let _pcc = read_stack(8);
                    let pch = read_stack(12);
                    debug!("NLS DosGetDBCSEv(cb={}, pch=0x{:08X})", cb, pch);
                    if pch != 0 && cb >= 2 {
                        let _ = self.guest_write::<u16>(pch, 0);
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
