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
pub mod codepage;
pub mod mmpm;
pub mod crash_dump;
pub mod api_ring;
pub mod gdb_stub;
pub mod uconv;

pub use constants::*;
pub use mutex_ext::MutexExt;
pub use managers::*;
pub use ipc::*;
pub use pm_types::*;
pub use vfs::DriveManager;
pub use vfs_hostdir::HostDirBackend;
pub use vm_backend::{VmBackend, VcpuBackend, VmExit, GuestRegs, GuestSegment, GuestSregs};
pub use guest_mem::GuestMemory;
pub use api_ring::{ApiRingBuffer, ApiCallRecord};

use kvm_backend::KvmVmBackend;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32};
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

/// Discriminates callback frames so the CALLBACK_RET_TRAP handler knows what
/// to do with the guest's EAX on return.
pub(crate) enum FrameKind {
    /// A PM window-procedure callback (WinCallWindowProc / WinDispatchMsg).
    /// EAX is returned as-is to the original API caller.
    PmCallback,
    /// A DLL _DLL_InitTerm(hmod, 0) call injected by DosLoadModule.
    /// EAX == 0 → init failed → return ERROR_INIT_ROUTINE_FAILED.
    /// EAX != 0 → init succeeded → write hmod to *phmod, return NO_ERROR.
    InitTerm { hmod: u32, phmod: u32 },
    /// A DLL _DLL_InitTerm(hmod, 1) call injected by DosFreeModule.
    /// Return value is ignored — OS/2 does not allow a DLL to refuse unload.
    /// After the call the guest pages in `object_bases` are freed.
    InitTermUnload { hmod: u32, object_bases: Vec<u32> },
}

pub(crate) struct CallbackFrame {
    saved_rip: u64,
    saved_rsp: u64,
    pub kind: FrameKind,
}

#[derive(Debug)]
pub(crate) enum ApiResult {
    Normal(u32),
    Callback {
        wnd_proc: u32,
        hwnd: u32,
        msg: u32,
        mp1: u32,
        mp2: u32,
    },
    /// Inject a guest call to `addr` with two _System args `(hmod, flag)`.
    /// Used for DLL INITTERM.  `phmod` is the address to write the handle on
    /// successful load-time init (0 for unload).  `object_bases` is non-empty
    /// for unload-time calls and holds the guest pages to free after the call.
    CallGuest {
        addr:         u32,
        hmod:         u32,
        phmod:        u32,
        object_bases: Vec<u32>,
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
    pub uconv_mgr: Mutex<uconv::UconvManager>,
    /// Executable name as provided on the command line
    pub exe_name: Mutex<String>,
    pub guest_mem: GuestMemory,
    pub next_tid: Mutex<u32>,
    pub threads: Mutex<HashMap<u32, thread::JoinHandle<()>>>,
    pub exit_requested: AtomicBool,
    pub exit_code: AtomicI32,
    pub locale: locale::Os2Locale,
    /// Active process codepage — updated by DosSetProcessCp; read by DosQueryCp.
    /// Initialized from locale.codepage at startup.
    pub active_codepage: AtomicU32,
    /// Keyboard event queue for SDL2 text-mode (CLI) apps.
    /// The VCPU thread blocks on `kbd_cond` when `use_sdl2_text` is set.
    pub kbd_queue: Mutex<VecDeque<KbdKeyInfo>>,
    /// Condvar paired with `kbd_queue` to wake `KbdCharIn` when a key arrives.
    pub kbd_cond: Condvar,
    /// When `true`, `KbdCharIn` reads from `kbd_queue` instead of termios stdin.
    /// Set before spawning the VCPU for SDL2 text-mode CLI apps.
    pub use_sdl2_text: AtomicBool,
    /// Ring buffer of the last 256 OS/2 API calls — always populated,
    /// regardless of log level, so crash dumps include full call history.
    pub api_ring: Mutex<ApiRingBuffer>,
    /// Extended LIBPATH prepended to LIBPATH (BEGINLIBPATH).
    pub begin_libpath: Mutex<String>,
    /// Extended LIBPATH appended after LIBPATH (ENDLIBPATH).
    pub end_libpath: Mutex<String>,
    /// GDB Remote Serial Protocol state.  `Some` when `--gdb <port>` is given;
    /// `None` in normal (non-debugger) runs.
    pub gdb_state: Option<Arc<gdb_stub::GdbState>>,
}

unsafe impl Send for SharedState {}
unsafe impl Sync for SharedState {}

pub struct Loader {
    pub vm: Arc<dyn VmBackend>,
    pub shared: Arc<SharedState>,
}

impl Default for Loader {
    fn default() -> Self { Self::new() }
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
            uconv_mgr: Mutex::new(uconv::UconvManager::new()),
            exe_name: Mutex::new(String::new()),
            guest_mem,
            next_tid: Mutex::new(1),
            threads: Mutex::new(HashMap::new()),
            exit_requested: AtomicBool::new(false),
            exit_code: AtomicI32::new(0),
            locale: locale::Os2Locale::from_host(),
            active_codepage: AtomicU32::new(437), // default CP437; overridden by DosSetProcessCp
            kbd_queue: Mutex::new(VecDeque::new()),
            kbd_cond: Condvar::new(),
            use_sdl2_text: AtomicBool::new(false),
            api_ring: Mutex::new(ApiRingBuffer::new()),
            begin_libpath: Mutex::new(String::new()),
            end_libpath: Mutex::new(String::new()),
            gdb_state: None,
        });

        Loader { vm, shared }
    }

    /// Attach a GDB state to this loader.
    ///
    /// Must be called before the first [`get_shared`] / [`setup_and_spawn_vcpu`]
    /// call so that `Arc::get_mut` succeeds (single owner at this point).
    pub fn set_gdb_state(&mut self, state: Arc<gdb_stub::GdbState>) {
        Arc::get_mut(&mut self.shared)
            .expect("set_gdb_state: shared Arc has multiple owners — call before get_shared()")
            .gdb_state = Some(state);
    }

    /// Check whether a shutdown has been requested.
    pub(crate) fn shutting_down(&self) -> bool {
        self.shared.exit_requested.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn post_wm_quit(&self, hwnd: u32) {
        let wm = self.shared.window_mgr.lock_or_recover();
        let hmq = wm.find_hmq_for_hwnd(hwnd);
        if let Some(hmq) = hmq
            && let Some(mq_arc) = wm.get_mq(hmq) {
                let mut mq = mq_arc.lock_or_recover();
                mq.messages.push_back(OS2Message {
                    hwnd, msg: WM_QUIT, mp1: 0, mp2: 0, time: 0, x: 0, y: 0,
                });
                mq.cond.notify_one();
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
            uconv_mgr:    Mutex::new(uconv::UconvManager::new()),
            exe_name:     Mutex::new(String::new()),
            guest_mem,
            next_tid:     Mutex::new(1),
            threads:      Mutex::new(HashMap::new()),
            exit_requested: AtomicBool::new(false),
            exit_code:    AtomicI32::new(0),
            locale:       locale::Os2Locale::from_host(),
            active_codepage: AtomicU32::new(437),
            kbd_queue:    Mutex::new(VecDeque::new()),
            kbd_cond:     Condvar::new(),
            use_sdl2_text: AtomicBool::new(false),
            api_ring:     Mutex::new(ApiRingBuffer::new()),
            begin_libpath: Mutex::new(String::new()),
            end_libpath:  Mutex::new(String::new()),
            gdb_state:    None,
        });
        Loader { vm, shared }
    }

    /// Handle a message dispatched to a window whose `pfn_wp == 0`.
    ///
    /// Called from `WinDispatchMsg` when the window has no user-registered
    /// window procedure.  Covers the built-in WC_* controls that OS/2 PM
    /// provides without requiring the app to register a class.
    pub(crate) fn dispatch_builtin_control(
        &self,
        hwnd: u32,
        msg: u32,
        _mp1: u32,
        _mp2: u32,
    ) -> ApiResult {
        // Collect window metadata (geometry, class, text, id, parent).
        let info = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let (ax, ay, acx, acy) = wm.get_abs_rect_in_frame(hwnd);
            wm.get_window(hwnd).map(|w| {
                (w.class_name.clone(), ax, ay, acx, acy,
                 w.text.clone(), w.id, w.parent)
            })
        };
        let (class_name, x, y, cx, cy, text, id, parent) = match info {
            Some(v) => v,
            None    => return ApiResult::Normal(0),
        };

        match msg {
            WM_PAINT => {
                let (frame_hwnd, gui_tx) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    (wm.find_frame_for_hwnd(hwnd), wm.gui_tx.clone())
                };
                let Some(ref sender) = gui_tx else { return ApiResult::Normal(0); };

                match class_name.as_str() {
                    "WC_BUTTON" => {
                        // Filled rectangle with gray background, border, centered label.
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + cy,
                            color: 0x00D4D0C8, fill: true,
                        });
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + cy,
                            color: 0x00808080, fill: false,
                        });
                        if !text.is_empty() {
                            let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                handle: frame_hwnd, x: x + 4, y: y + cy / 4,
                                text, color: 0x00000000,
                            });
                        }
                    }
                    "WC_STATIC" => {
                        // Plain text label, no background.
                        if !text.is_empty() {
                            let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                handle: frame_hwnd, x, y: y + cy / 4,
                                text, color: 0x00000000,
                            });
                        }
                    }
                    "WC_SCROLLBAR" => {
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + cy,
                            color: 0x00A0A0A0, fill: true,
                        });
                    }
                    "WC_ENTRYFIELD" | "WC_MLE" => {
                        // White background with gray border; current text inside.
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + cy,
                            color: 0x00FFFFFF, fill: true,
                        });
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + cy,
                            color: 0x00808080, fill: false,
                        });
                        if !text.is_empty() {
                            let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                handle: frame_hwnd, x: x + 2, y: y + cy / 4,
                                text, color: 0x00000000,
                            });
                        }
                    }
                    "WC_LISTBOX" => {
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + cy,
                            color: 0x00FFFFFF, fill: true,
                        });
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + cy,
                            color: 0x00808080, fill: false,
                        });
                    }
                    _ => {} // Unknown built-in class — silently ignore WM_PAINT
                }
                ApiResult::Normal(0)
            }

            WM_BUTTON1UP if class_name == "WC_BUTTON" => {
                // Notify parent via WM_CONTROL with BN_CLICKED.
                // mp1 = MPFROM2SHORT(id, BN_CLICKED) = (BN_CLICKED << 16) | id
                let mp1_ctrl = (BN_CLICKED << 16) | (id & 0xFFFF);
                let wm = self.shared.window_mgr.lock_or_recover();
                let hmq = wm.find_hmq_for_hwnd(parent);
                if let Some(hmq) = hmq
                    && let Some(mq_arc) = wm.get_mq(hmq) {
                        let mut mq = mq_arc.lock_or_recover();
                        mq.messages.push_back(OS2Message {
                            hwnd: parent, msg: WM_CONTROL,
                            mp1: mp1_ctrl, mp2: 0,
                            time: 0, x: 0, y: 0,
                        });
                        mq.cond.notify_one();
                }
                ApiResult::Normal(0)
            }

            _ => ApiResult::Normal(0),
        }
    }

    /// Map an OS/2 PM `lColor` value to a 0x00RRGGBB pixel colour.
    ///
    /// Handles the full CLR_* palette (negative specials + 0–15 named indices)
    /// and direct RGB values (>= 16, i.e. `RGB(r,g,b)` macro output).
    pub(crate) fn map_color(&self, clr: u32) -> u32 {
        // Cast to signed so that CLR_BLACK=-1, CLR_WHITE=-2, CLR_DEFAULT=-3 match.
        match clr as i32 {
            -5 | -4     => 0x00000000, // CLR_FALSE / CLR_TRUE — treat as black
            -3          => 0x00000000, // CLR_DEFAULT — default foreground (black)
            -2          => 0x00FFFFFF, // CLR_WHITE
            -1          => 0x00000000, // CLR_BLACK
             0          => 0x00FFFFFF, // CLR_BACKGROUND — page background (white)
             1          => 0x000000FF, // CLR_BLUE
             2          => 0x00FF0000, // CLR_RED
             3          => 0x00FF00FF, // CLR_PINK (magenta)
             4          => 0x0000FF00, // CLR_GREEN
             5          => 0x0000FFFF, // CLR_CYAN
             6          => 0x00FFFF00, // CLR_YELLOW
             7          => 0x00808080, // CLR_NEUTRAL (medium grey)
             8          => 0x00404040, // CLR_DARKGRAY
             9          => 0x00000080, // CLR_DARKBLUE
            10          => 0x00800000, // CLR_DARKRED
            11          => 0x00800080, // CLR_DARKPINK
            12          => 0x00008000, // CLR_DARKGREEN
            13          => 0x00008080, // CLR_DARKCYAN
            14          => 0x00804000, // CLR_BROWN
            15          => 0x00C0C0C0, // CLR_PALEGRAY
            n if n >= 16 => clr & 0x00FF_FFFF, // Direct 0x00RRGGBB — mask alpha
            _           => 0x00808080, // Unknown — grey
        }
    }
}

