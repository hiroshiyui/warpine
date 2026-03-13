// SPDX-License-Identifier: GPL-3.0-only

pub mod constants;
pub mod mutex_ext;
pub mod managers;
pub mod ipc;
pub mod pm_types;
mod guest_mem;
mod doscalls;
mod pm_win;
mod pm_gpi;
mod stubs;

pub use constants::*;
pub use mutex_ext::MutexExt;
pub use managers::*;
pub use ipc::*;
pub use pm_types::*;

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
/// Level 2: mem_mgr, handle_mgr, hdir_mgr, resource_mgr, shmem_mgr  (independent resource managers)
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
        let guest_mem_size = 128 * 1024 * 1024;
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
                        if (record.source_type & 0x0F) == 0x07 {
                            self.guest_write::<u32>(source_phys as u32, target_addr as u32).expect("fixup: write OOB");
                        } else if (record.source_type & 0x0F) == 0x08 {
                            self.guest_write::<i32>(source_phys as u32, (target_addr as isize - (source_phys as isize + 4)) as i32).expect("fixup: write OOB");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_import(&self, module: &str, ordinal: u32) -> u64 {
        if module == "DOSCALLS" { MAGIC_API_BASE + ordinal as u64 }
        else if module == "QUECALLS" { MAGIC_API_BASE + 1024 + ordinal as u64 }
        else if module == "PMWIN" { MAGIC_API_BASE + PMWIN_BASE as u64 + ordinal as u64 }
        else if module == "PMGPI" { MAGIC_API_BASE + PMGPI_BASE as u64 + ordinal as u64 }
        else if module == "KBDCALLS" { MAGIC_API_BASE + KBDCALLS_BASE as u64 + ordinal as u64 }
        else if module == "VIOCALLS" { MAGIC_API_BASE + VIOCALLS_BASE as u64 + ordinal as u64 }
        else { 0 }
    }

    fn setup_stubs(&self) {
        for i in 0..STUB_AREA_SIZE {
            self.guest_write::<u8>(MAGIC_API_BASE as u32 + i, 0xCC).expect("setup_stubs: write OOB");
        }
    }

    fn setup_guest(&self, lx_file: &LxFile) -> (u64, u64, u64) {
        let entry_eip = lx_file.object_table[lx_file.header.eip_object as usize - 1].base_address as u64 + lx_file.header.eip as u64;
        let entry_esp = lx_file.object_table[lx_file.header.esp_object as usize - 1].base_address as u64 + lx_file.header.esp as u64;

        let tib_base = TIB_BASE as u64;
        let cmdline_addr = ENV_ADDR + 10;
        let env_data = b"PATH=C:\\\0\0HELLO.EXE\0";
        self.guest_write_bytes(ENV_ADDR, env_data).expect("setup_guest: env write OOB");
        self.guest_write::<u32>(TIB_BASE + 0x18, TIB_BASE).expect("setup_guest: TIB self-ptr OOB");
        self.guest_write::<u32>(TIB_BASE + 0x30, PIB_BASE).expect("setup_guest: TIB->PIB OOB");
        self.guest_write::<u32>(PIB_BASE + 0x00, 42).expect("setup_guest: PIB PID OOB");
        self.guest_write::<u32>(PIB_BASE + 0x0C, ENV_ADDR).expect("setup_guest: PIB env OOB");
        self.guest_write::<u32>(PIB_BASE + 0x10, cmdline_addr).expect("setup_guest: PIB cmdline OOB");

        self.setup_stubs();
        (entry_eip, entry_esp, tib_base)
    }

    fn create_initial_vcpu(&self, entry_eip: u64, entry_esp: u64) -> VcpuFd {
        let vcpu = self.vm.create_vcpu(0).unwrap();
        let mut regs = vcpu.get_regs().unwrap();
        regs.rip = entry_eip;
        regs.rsp = entry_esp - 20;
        regs.rflags = 2;
        vcpu.set_regs(&regs).unwrap();

        let cmdline_addr = ENV_ADDR + 10;
        let sp = regs.rsp as u32;
        self.guest_write::<u32>(sp, EXIT_TRAP_ADDR).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 4, 1).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 8, 0).expect("create_initial_vcpu: stack write OOB");
        self.guest_write::<u32>(sp + 12, ENV_ADDR).expect("create_initial_vcpu: stack write OOB");
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
        sregs.cr0 |= 1; vcpu.set_sregs(&sregs).unwrap();

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
                283 => self.dos_get_info_blocks(vcpu, read_stack(4), read_stack(8)),
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
                342 => 0, // DosQueryAppType (stub)
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
            let kbd_ordinal = ordinal - KBDCALLS_BASE;
            warn!("Warning: Unknown KBDCALLS Ordinal {} on VCPU {}", kbd_ordinal, vcpu_id);
            ApiResult::Normal(0)
        } else if ordinal < STUB_AREA_SIZE {
            // VIOCALLS
            let vio_ordinal = ordinal - VIOCALLS_BASE;
            warn!("Warning: Unknown VIOCALLS Ordinal {} on VCPU {}", vio_ordinal, vcpu_id);
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
