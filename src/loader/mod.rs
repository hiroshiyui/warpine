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
mod seh;
mod cmd;

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

/// CPU context saved at the time of a hardware fault or software exception raise.
///
/// Used by `FrameKind::ExceptionHandler` to restore the full register state
/// when an exception handler returns `XCPT_CONTINUE_EXECUTION`.
#[derive(Clone, Debug)]
pub(crate) struct FaultContext {
    pub eax: u32, pub ebx: u32, pub ecx: u32, pub edx: u32,
    pub esi: u32, pub edi: u32, pub ebp: u32,
    pub esp: u32, pub eip: u32, pub eflags: u32,
    pub cs: u16, pub ds: u16, pub es: u16, pub fs: u16, pub gs: u16, pub ss: u16,
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
    /// An OS/2 exception handler callback (SEH).
    ///
    /// EAX on return is `XCPT_CONTINUE_EXECUTION` (restore fault context and
    /// resume), `XCPT_CONTINUE_SEARCH` (try the next handler in the chain), or
    /// any other value (treated as XCPT_CONTINUE_SEARCH).
    ExceptionHandler {
        /// Saved CPU state at fault time; restored on XCPT_CONTINUE_EXECUTION.
        saved: Box<FaultContext>,
        /// Pointer to the next `EXCEPTIONREGISTRATIONRECORD` to invoke if the
        /// handler returns XCPT_CONTINUE_SEARCH (XCPT_CHAIN_END = no more).
        next_handler: u32,
        /// Guest pointer to the `EXCEPTIONREPORTRECORD` passed to each handler.
        exc_report: u32,
        /// Guest pointer to the `CONTEXTRECORD` passed to each handler.
        ctx_record: u32,
        /// Guest memory blocks to free once exception handling is complete.
        guest_allocs: Vec<u32>,
    },
    /// A WM_CREATE callback dispatched synchronously by WinCreateStdWindow.
    /// After the client window procedure returns, the frame window handle
    /// (`h_frame`) is written into EAX as the return value of WinCreateStdWindow.
    WmCreate { h_frame: u32 },
    /// A modal dialog message loop (WinDlgBox / WinProcessDlg).
    ///
    /// After each message callback returns, the CALLBACK_RET_TRAP handler
    /// checks `hwnd_dlg.dialog_dismissed`.  If dismissed, it returns
    /// `dialog_result` to the WinDlgBox/WinProcessDlg caller.  Otherwise it
    /// waits for the next queued message and dispatches it to `dlg_proc`.
    DlgRunLoop {
        dlg_proc: u32,
        hwnd_dlg: u32,
        hmq:      u32,
    },
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
    /// Dispatch WM_CREATE synchronously from WinCreateStdWindow.
    /// After the client window procedure returns, `h_frame` is placed in EAX
    /// as the return value of WinCreateStdWindow (regardless of WM_CREATE's
    /// own return value).
    WmCreateCallback {
        wnd_proc: u32,
        hwnd:     u32,
        h_frame:  u32,
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
    /// Run a modal dialog message loop on behalf of WinDlgBox / WinProcessDlg.
    ///
    /// `vcpu.rs` dispatches messages from `hmq` to `dlg_proc` in a loop,
    /// blocking on the queue condvar between messages.  The loop terminates
    /// when `hwnd_dlg.dialog_dismissed` is set by WinDismissDlg, returning
    /// `dialog_result` to the original API caller.
    DlgRunLoop {
        dlg_proc: u32,
        hwnd_dlg: u32,
        hmq:      u32,
    },
    /// Invoke an OS/2 exception handler as a guest callback.
    ///
    /// Emitted by `DosRaiseException` and by the IDT exception dispatch path.
    /// `vcpu.rs` converts this into a `CallbackFrame { FrameKind::ExceptionHandler }`
    /// and sets up the guest stack/IP to call the handler with the four standard
    /// OS/2 exception-handler arguments.
    ExceptionDispatch {
        /// Guest function pointer of the handler to call.
        handler_addr: u32,
        /// Guest pointer to the `EXCEPTIONREPORTRECORD`.
        exc_report:   u32,
        /// Guest pointer to the current `EXCEPTIONREGISTRATIONRECORD`.
        reg_rec:      u32,
        /// Guest pointer to the `CONTEXTRECORD`.
        ctx_record:   u32,
        /// Saved CPU state (used to restore on XCPT_CONTINUE_EXECUTION).
        saved:        Box<FaultContext>,
        /// Next handler record pointer (for XCPT_CONTINUE_SEARCH walk).
        next_handler: u32,
        /// Guest allocations to free when handling is complete.
        guest_allocs: Vec<u32>,
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
        mp1: u32,
        mp2: u32,
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
                    "#Menu" => {
                        // Collect top-level menu items and open-submenu state.
                        let (items, open_item) = {
                            let wm2 = self.shared.window_mgr.lock_or_recover();
                            let win = wm2.get_window(hwnd);
                            let it = win.map(|w| w.menu_items.clone()).unwrap_or_default();
                            let oi = win.and_then(|w| w.window_ulong.get(&-1)).copied().unwrap_or(0);
                            (it, oi)
                        };
                        // Gray menu bar background.
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + cy,
                            color: 0x00D4D0C8, fill: true,
                        });
                        // Bottom border line.
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: frame_hwnd, x1: x, y1: y, x2: x + cx, y2: y + 1,
                            color: 0x00808080, fill: true,
                        });
                        // Top-level item names (strip ~ accelerator marker).
                        let mut item_x = x + 4;
                        for (i, item) in items.iter().enumerate() {
                            if item.style & MIS_SEPARATOR != 0 { continue; }
                            let label: String = item.text.chars().filter(|&c| c != '~').collect();
                            let label_w = (label.len() as i32 * 8) + 8;
                            // Highlight the open item.
                            if open_item > 0 && i == (open_item - 1) as usize {
                                let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                                    handle: frame_hwnd,
                                    x1: item_x, y1: y, x2: item_x + label_w, y2: y + cy,
                                    color: 0x00000080, fill: true,
                                });
                                let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                    handle: frame_hwnd, x: item_x + 2, y: y + 2,
                                    text: label, color: 0x00FFFFFF,
                                });
                            } else {
                                let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                    handle: frame_hwnd, x: item_x + 2, y: y + 2,
                                    text: label, color: 0x00000000,
                                });
                            }
                            item_x += label_w;
                        }
                        // Draw open dropdown if any.
                        if open_item > 0 {
                            let idx = (open_item - 1) as usize;
                            if let Some(parent_item) = items.get(idx) {
                                // Compute dropdown x: same as the open item's x position.
                                let mut drop_x = x + 4;
                                for (i, item) in items.iter().enumerate() {
                                    if i >= idx { break; }
                                    if item.style & MIS_SEPARATOR != 0 { continue; }
                                    let lw = (item.text.chars().filter(|&c| c != '~').count() as i32 * 8) + 8;
                                    drop_x += lw;
                                }
                                const DROP_W: i32 = 120;
                                const ITEM_H: i32 = 16;
                                let child_count = parent_item.children.len() as i32;
                                let drop_h = child_count * ITEM_H;
                                // Dropdown sits just below the menu bar (lower y in OS/2 coords).
                                let drop_y2 = y; // top of dropdown = bottom of menu bar
                                let drop_y1 = drop_y2 - drop_h;
                                // Background + border.
                                let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                                    handle: frame_hwnd,
                                    x1: drop_x, y1: drop_y1, x2: drop_x + DROP_W, y2: drop_y2,
                                    color: 0x00D4D0C8, fill: true,
                                });
                                let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                                    handle: frame_hwnd,
                                    x1: drop_x, y1: drop_y1, x2: drop_x + DROP_W, y2: drop_y2,
                                    color: 0x00808080, fill: false,
                                });
                                // Draw child items top-to-bottom (decreasing OS/2 y).
                                for (ci, child) in parent_item.children.iter().enumerate() {
                                    let item_y2 = drop_y2 - (ci as i32 * ITEM_H);
                                    let item_y1 = item_y2 - ITEM_H;
                                    if child.style & MIS_SEPARATOR != 0 {
                                        let sep_y = (item_y1 + item_y2) / 2;
                                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                                            handle: frame_hwnd,
                                            x1: drop_x + 2, y1: sep_y, x2: drop_x + DROP_W - 2, y2: sep_y + 1,
                                            color: 0x00808080, fill: true,
                                        });
                                    } else {
                                        let label: String = child.text.chars().filter(|&c| c != '~').collect();
                                        let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                            handle: frame_hwnd,
                                            x: drop_x + 4, y: item_y1 + 2,
                                            text: label, color: 0x00000000,
                                        });
                                    }
                                }
                            }
                        }
                        // Flush immediately — no WinEndPaint caller for this pseudo-window.
                        let _ = sender.send(crate::gui::GUIMessage::PresentBuffer { handle: frame_hwnd });
                    }
                    "#Dialog" => {
                        // Collect child-window metadata while holding the lock.
                        let children = {
                            let wm2 = self.shared.window_mgr.lock_or_recover();
                            let child_hwnds = wm2.get_window(hwnd)
                                .map(|w| w.children.clone())
                                .unwrap_or_default();
                            child_hwnds.iter().filter_map(|&ch| {
                                let (ax, ay, acx, acy) = wm2.get_abs_rect_in_frame(ch);
                                wm2.get_window(ch).map(|w| {
                                    (w.class_name.clone(), ax, ay, acx, acy, w.text.clone())
                                })
                            }).collect::<Vec<_>>()
                        };
                        // The dialog has its own SDL2 window; use hwnd as the handle.
                        // Gray background.
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: hwnd, x1: 0, y1: 0, x2: cx, y2: cy,
                            color: 0x00D4D0C8, fill: true,
                        });
                        // Navy title bar at the top (high OS/2 y = low SDL2 y = top of window).
                        const DLG_TITLE_H: i32 = 16;
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: hwnd, x1: 0, y1: cy - DLG_TITLE_H, x2: cx, y2: cy,
                            color: 0x00000080, fill: true,
                        });
                        if !text.is_empty() {
                            let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                handle: hwnd, x: 4, y: cy - DLG_TITLE_H + 2,
                                text: text.clone(), color: 0x00FFFFFF,
                            });
                        }
                        // Dialog border.
                        let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                            handle: hwnd, x1: 0, y1: 0, x2: cx, y2: cy,
                            color: 0x00808080, fill: false,
                        });
                        // Child controls.
                        for (cname, chi_x, chi_y, chi_cx, chi_cy, ctext) in children {
                            match cname.as_str() {
                                "WC_BUTTON" => {
                                    let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                                        handle: hwnd, x1: chi_x, y1: chi_y,
                                        x2: chi_x + chi_cx, y2: chi_y + chi_cy,
                                        color: 0x00D4D0C8, fill: true,
                                    });
                                    let _ = sender.send(crate::gui::GUIMessage::DrawBox {
                                        handle: hwnd, x1: chi_x, y1: chi_y,
                                        x2: chi_x + chi_cx, y2: chi_y + chi_cy,
                                        color: 0x00808080, fill: false,
                                    });
                                    if !ctext.is_empty() {
                                        let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                            handle: hwnd, x: chi_x + 2, y: chi_y + chi_cy / 4,
                                            text: ctext, color: 0x00000000,
                                        });
                                    }
                                }
                                "WC_STATIC" => {
                                    if !ctext.is_empty() {
                                        let _ = sender.send(crate::gui::GUIMessage::DrawText {
                                            handle: hwnd, x: chi_x, y: chi_y + chi_cy / 4,
                                            text: ctext, color: 0x00000000,
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                        let _ = sender.send(crate::gui::GUIMessage::PresentBuffer { handle: hwnd });
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

            WM_BUTTON1DOWN if class_name == "#Menu" => {
                // Hit-test: is click in the menu bar or in an open dropdown?
                // Coordinates in mp1 are frame-relative OS/2 bottom-left.
                let click_x = (mp1 & 0xFFFF) as i32;
                let click_y = ((mp1 >> 16) & 0xFFFF) as i32;

                let (items, open_item) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let win = wm.get_window(hwnd);
                    let it = win.map(|w| w.menu_items.clone()).unwrap_or_default();
                    let oi = win.and_then(|w| w.window_ulong.get(&-1)).copied().unwrap_or(0);
                    (it, oi)
                };

                // Menu bar spans [y, y+cy] in OS/2 coords (y/cy from abs_rect).
                if click_y >= y && click_y < y + cy {
                    // Click is in the menu bar — find which top-level item was hit.
                    let mut item_x = x + 4;
                    let mut hit_idx: Option<usize> = None;
                    for (i, item) in items.iter().enumerate() {
                        if item.style & MIS_SEPARATOR != 0 { continue; }
                        let label_w = (item.text.chars().filter(|&c| c != '~').count() as i32 * 8) + 8;
                        if click_x >= item_x && click_x < item_x + label_w {
                            hit_idx = Some(i);
                            break;
                        }
                        item_x += label_w;
                    }
                    if let Some(idx) = hit_idx {
                        // Toggle: clicking the already-open item closes it.
                        let new_open = if open_item == idx as u32 + 1 { 0 } else { idx as u32 + 1 };
                        {
                            let mut wm = self.shared.window_mgr.lock_or_recover();
                            if let Some(win) = wm.get_window_mut(hwnd) {
                                if new_open == 0 { win.window_ulong.remove(&-1); }
                                else { win.window_ulong.insert(-1, new_open); }
                            }
                        }
                        // Repaint the menu bar (and dropdown if newly opened).
                        return self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                    } else if open_item > 0 {
                        // Click on empty part of menu bar: close dropdown.
                        { self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd).map(|w| w.window_ulong.remove(&-1)); }
                        return self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                    }
                } else if open_item > 0 {
                    // Click below the menu bar — check if it's in the open dropdown.
                    let idx = (open_item - 1) as usize;
                    if let Some(parent_item) = items.get(idx) {
                        let mut drop_x = x + 4;
                        for (i, item) in items.iter().enumerate() {
                            if i >= idx { break; }
                            if item.style & MIS_SEPARATOR != 0 { continue; }
                            let lw = (item.text.chars().filter(|&c| c != '~').count() as i32 * 8) + 8;
                            drop_x += lw;
                        }
                        const DROP_W: i32 = 120;
                        const ITEM_H: i32 = 16;
                        let child_count = parent_item.children.len() as i32;
                        let drop_y2 = y; // same as menu bar bottom
                        let drop_y1 = drop_y2 - child_count * ITEM_H;

                        if click_x >= drop_x && click_x < drop_x + DROP_W
                           && click_y >= drop_y1 && click_y < drop_y2
                        {
                            // Determine which child item was clicked.
                            let child_idx = ((drop_y2 - 1 - click_y) / ITEM_H) as usize;
                            if let Some(child) = parent_item.children.get(child_idx)
                                && child.style & MIS_SEPARATOR == 0
                            {
                                let cmd_id = child.id as u32;
                                // Close dropdown before dispatching.
                                { self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd).map(|w| w.window_ulong.remove(&-1)); }
                                self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                                // Post WM_COMMAND (CMDSRC_MENU=2) to the client window.
                                // parent = frame_hwnd; client = frame_to_client[frame_hwnd].
                                let (client_hwnd, mq_arc_opt) = {
                                    let wm = self.shared.window_mgr.lock_or_recover();
                                    let client = wm.frame_to_client.get(&parent).copied().unwrap_or(parent);
                                    let hmq = wm.find_hmq_for_hwnd(client);
                                    let mq = hmq.and_then(|h| wm.get_mq(h));
                                    (client, mq)
                                };
                                if let Some(mq_arc) = mq_arc_opt {
                                    let mut mq = mq_arc.lock_or_recover();
                                    // mp1 = MPFROM2SHORT(id, CMDSRC_MENU=2)
                                    mq.messages.push_back(OS2Message {
                                        hwnd: client_hwnd,
                                        msg: WM_COMMAND,
                                        mp1: cmd_id | (2u32 << 16), // CMDSRC_MENU
                                        mp2: 0,
                                        time: 0, x: 0, y: 0,
                                    });
                                    mq.cond.notify_one();
                                }
                                return ApiResult::Normal(0);
                            }
                        } else {
                            // Click outside dropdown: close it.
                            { self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd).map(|w| w.window_ulong.remove(&-1)); }
                            return self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                        }
                    }
                }
                ApiResult::Normal(0)
            }

            WM_MENUEND if class_name == "#Menu" => {
                // Close any open dropdown and redraw the menu bar.
                { self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd).map(|w| w.window_ulong.remove(&-1)); }
                self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0)
            }

            WM_BUTTON1DOWN if class_name == "#Dialog" => {
                // Hit-test child WC_BUTTON controls; post WM_COMMAND to the dialog's HMQ.
                let click_x = (mp1 & 0xFFFF) as i32;
                let click_y = ((mp1 >> 16) & 0xFFFF) as i32;
                let (child_info, mq_opt) = {
                    let wm = self.shared.window_mgr.lock_or_recover();
                    let child_hwnds = wm.get_window(hwnd)
                        .map(|w| w.children.clone())
                        .unwrap_or_default();
                    let info: Vec<(String, i32, i32, i32, i32, u32)> = child_hwnds.iter()
                        .filter_map(|&ch| {
                            let (ax, ay, acx, acy) = wm.get_abs_rect_in_frame(ch);
                            wm.get_window(ch).map(|w| (w.class_name.clone(), ax, ay, acx, acy, w.id))
                        }).collect();
                    let hmq = wm.find_hmq_for_hwnd(hwnd);
                    let mq = hmq.and_then(|h| wm.get_mq(h));
                    (info, mq)
                };
                for (cname, chi_x, chi_y, chi_cx, chi_cy, chi_id) in child_info {
                    if cname == "WC_BUTTON"
                        && click_x >= chi_x && click_x < chi_x + chi_cx
                        && click_y >= chi_y && click_y < chi_y + chi_cy
                    {
                        if let Some(ref mq_arc) = mq_opt {
                            let mut mq = mq_arc.lock_or_recover();
                            mq.messages.push_back(OS2Message {
                                hwnd,
                                msg: WM_COMMAND,
                                mp1: chi_id,
                                mp2: 0,
                                time: 0, x: 0, y: 0,
                            });
                            mq.cond.notify_one();
                        }
                        break;
                    }
                }
                ApiResult::Normal(0)
            }

            LM_INSERTITEM => {
                // mp1 = MPFROMSHORT(index) — insert position; LIT_END (0xFFFF) = append
                // mp2 = MPFROMP(pszItem)   — guest pointer to item string
                let item_text = if mp2 != 0 { self.read_guest_string(mp2) } else { String::new() };
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(win) = wm.get_window_mut(hwnd) {
                    let idx = mp1 as i32;
                    if idx < 0 || idx as usize >= win.listbox_items.len() {
                        win.listbox_items.push(item_text);
                        ApiResult::Normal(win.listbox_items.len() as u32 - 1)
                    } else {
                        win.listbox_items.insert(idx as usize, item_text);
                        ApiResult::Normal(idx as u32)
                    }
                } else {
                    ApiResult::Normal(u32::MAX) // LIT_ERROR
                }
            }

            LM_QUERYITEMCOUNT => {
                let wm = self.shared.window_mgr.lock_or_recover();
                let count = wm.get_window(hwnd).map(|w| w.listbox_items.len()).unwrap_or(0);
                ApiResult::Normal(count as u32)
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

