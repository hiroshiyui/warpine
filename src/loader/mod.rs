// SPDX-License-Identifier: GPL-3.0-only

pub mod constants;
pub mod mutex_ext;
pub mod managers;
pub mod ipc;
pub mod pm_types;
pub mod vfs;
pub mod vfs_hostdir;
pub mod vm_backend;
pub mod api_trace;
pub mod api_registry;
mod kvm_backend;
mod guest_mem;
mod lx_loader;
mod ne_exec;
mod descriptors;
mod vcpu;
mod api_dispatch;
mod doscalls;
mod pm_win;
mod pm_gpi;
mod stubs;
pub mod console;
mod kbdcalls;
mod viocalls;
mod process;
pub mod locale;
pub mod mmpm;

pub use constants::*;
pub use mutex_ext::MutexExt;
pub use managers::*;
pub use ipc::*;
pub use pm_types::*;
pub use vfs::DriveManager;
pub use vfs_hostdir::HostDirBackend;
pub use vm_backend::{VmBackend, VcpuBackend, VmExit, GuestRegs, GuestSegment, GuestSregs};
pub use guest_mem::GuestMemory;

use kvm_backend::KvmVmBackend;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{AtomicBool, AtomicI32};
use std::thread;
use log::{info, warn};

/// Key information pushed into the SDL2 text-mode keyboard queue.
///
/// Mirrors the fields that OS/2 `KBDKEYINFO` needs for `KbdCharIn`:
/// - `ch`    — `chChar`: ASCII character code (0 for extended keys)
/// - `scan`  — `chScan`: OS/2 scan code
/// - `state` — `fsState`: shift-state flags (Shift/Ctrl/Alt bitmask)
#[derive(Clone, Copy, Debug)]
pub struct KbdKeyInfo {
    pub ch: u8,
    pub scan: u8,
    pub state: u16,
}

pub(crate) struct CallbackFrame {
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
    pub mmpm_mgr: Mutex<mmpm::MmpmManager>,
    pub dll_mgr: Mutex<DllManager>,
    /// Executable name as provided on the command line
    pub exe_name: Mutex<String>,
    pub guest_mem: GuestMemory,
    pub next_tid: Mutex<u32>,
    pub threads: Mutex<HashMap<u32, thread::JoinHandle<()>>>,
    pub exit_requested: AtomicBool,
    pub exit_code: AtomicI32,
    pub locale: locale::Os2Locale,
    /// Keyboard event queue for SDL2 text-mode (CLI) apps.
    /// The VCPU thread blocks on `kbd_cond` when `use_sdl2_text` is set.
    pub kbd_queue: Mutex<VecDeque<KbdKeyInfo>>,
    /// Condvar paired with `kbd_queue` to wake `KbdCharIn` when a key arrives.
    pub kbd_cond: Condvar,
    /// When `true`, `KbdCharIn` reads from `kbd_queue` instead of termios stdin.
    /// Set before spawning the VCPU for SDL2 text-mode CLI apps.
    pub use_sdl2_text: AtomicBool,
}

unsafe impl Send for SharedState {}
unsafe impl Sync for SharedState {}

pub struct Loader {
    pub vm: Arc<dyn VmBackend>,
    pub shared: Arc<SharedState>,
}

impl Loader {
    pub fn new() -> Self {
        let vm: Arc<dyn VmBackend> = Arc::new(KvmVmBackend::new());
        let guest_mem_size = 256 * 1024 * 1024;
        let guest_mem = GuestMemory::alloc(guest_mem_size);
        vm.register_guest_memory(0, guest_mem_size as u64, guest_mem.host_base_addr()).unwrap();

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
            mmpm_mgr: Mutex::new(mmpm::MmpmManager::new()),
            dll_mgr: Mutex::new(DllManager::new()),
            exe_name: Mutex::new(String::new()),
            guest_mem,
            next_tid: Mutex::new(1),
            threads: Mutex::new(HashMap::new()),
            exit_requested: AtomicBool::new(false),
            exit_code: AtomicI32::new(0),
            locale: locale::Os2Locale::from_host(),
            kbd_queue: Mutex::new(VecDeque::new()),
            kbd_cond: Condvar::new(),
            use_sdl2_text: AtomicBool::new(false),
        });

        Loader { vm, shared }
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

    /// Create a lightweight [`Loader`] for unit tests — no `/dev/kvm` required.
    ///
    /// Uses [`vm_backend::mock::MockVmBackend`] so the test process never
    /// opens `/dev/kvm`.  Allocates 1 MB of guest memory (sufficient for
    /// testing API handler logic).  No C: drive is mounted, avoiding
    /// filesystem I/O in pure unit tests.
    #[cfg(test)]
    pub fn new_mock() -> Self {
        use vm_backend::mock::MockVmBackend;
        let vm: Arc<dyn VmBackend> = Arc::new(MockVmBackend);
        let guest_mem_size = 64 * 1024 * 1024; // 64 MB (must cover DYNAMIC_ALLOC_BASE = 0x02000000)
        let guest_mem = GuestMemory::alloc(guest_mem_size);
        vm.register_guest_memory(0, guest_mem_size as u64, guest_mem.host_base_addr()).unwrap();
        let shared = Arc::new(SharedState {
            mem_mgr:      Mutex::new(MemoryManager::new(DYNAMIC_ALLOC_BASE, DYNAMIC_ALLOC_BASE + guest_mem_size as u32)),
            handle_mgr:   Mutex::new(HandleManager::new()),
            resource_mgr: Mutex::new(ResourceManager::new()),
            shmem_mgr:    Mutex::new(SharedMemManager::new()),
            process_mgr:  Mutex::new(ProcessManager::new()),
            sem_mgr:      Mutex::new(SemaphoreManager::new()),
            hdir_mgr:     Mutex::new(HDirManager::new()),
            queue_mgr:    Mutex::new(QueueManager::new()),
            window_mgr:   Mutex::new(WindowManager::new()),
            drive_mgr:    Mutex::new(DriveManager::with_default_config()),
            console_mgr:  Mutex::new(console::VioManager::new()),
            mmpm_mgr:     Mutex::new(mmpm::MmpmManager::new()),
            dll_mgr:      Mutex::new(DllManager::new()),
            exe_name:     Mutex::new(String::new()),
            guest_mem,
            next_tid:     Mutex::new(1),
            threads:      Mutex::new(HashMap::new()),
            exit_requested: AtomicBool::new(false),
            exit_code:    AtomicI32::new(0),
            locale:       locale::Os2Locale::from_host(),
            kbd_queue:    Mutex::new(VecDeque::new()),
            kbd_cond:     Condvar::new(),
            use_sdl2_text: AtomicBool::new(false),
        });
        Loader { vm, shared }
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

