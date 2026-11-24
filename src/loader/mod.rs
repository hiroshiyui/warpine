// SPDX-License-Identifier: GPL-3.0-or-later

pub mod constants;
pub mod mutex_ext;
pub mod managers;
pub mod ipc;
pub mod pm_types;
pub mod pm_edit;
pub mod pm_list;
pub mod pm_input;
mod pm_msgbox;
mod pm_controls;
mod pm_dialog;
pub mod res_templates;
pub mod pointer_codec;
pub mod wav;
mod pm_error;
mod pm_button;
mod pm_owner;
mod pm_hook;
mod pm_message;
mod pm_combo;
mod pm_spin;
mod pm_slider;
mod pm_circslider;
mod pm_valueset;
mod pm_notebook;
mod pm_container;
pub mod pm_drag;
pub mod pm_dragimage;
pub mod pm_scrollbar;
pub mod pm_cursor;
mod pm_destroy;
pub mod pm_pointer;
mod pm_window;
mod pm_winutil;
mod dos_misc;
mod pm_frame;
mod pm_accel;
mod pm_winmisc;
pub mod pm_atom;
pub mod pm_dde;
pub mod pm_switch;
pub mod pm_startapp;
mod msgcalls;
pub mod msgfile;
pub mod vio_frame;
pub mod pm_minmax;
mod pm_sysmenu;
pub(crate) mod pm_menubar;
mod pm_menuitems;
mod pm_rect;
mod pm_drawtext;
mod pm_paint;
mod pm_presparam;
mod pm_paint_style;
mod pm_static;
mod pm_query;
mod pm_remote;
mod pm_enum;
pub mod pm_keystate;
pub mod pm_syscolor;
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
mod ring;
mod vmem;
pub(crate) mod paging;
pub(crate) mod track;
pub(crate) mod exit_report;
pub(crate) mod file_attrs;
mod pm_track;
mod vcpu;
mod api_dispatch;
mod doscalls;
mod pm_win;
mod pm_gpi;
pub mod pm_text;
mod bitmap_codec;
mod pm_bitmap;
pub mod pm_region;
pub mod pm_area;
pub mod pm_shapes;
pub mod pm_xform;
mod pm_coords;
pub mod pm_psstate;
mod stubs;
pub mod clock;
pub mod infoseg;
pub mod shared_arena;
pub mod shared_sem;
mod warpine_api;
pub mod console;
pub mod ansi;
mod kbdcalls;
mod viocalls;
pub mod vio_text;
mod process;
pub mod locale;
pub mod codepage;
pub mod mmpm;
pub mod mmpm_errtext;
pub mod crash_dump;
pub mod api_ring;
pub mod gdb_stub;
pub mod uconv;
pub mod tcpip;
pub mod tcpip_util;
pub mod npipe;
mod npipe_dir;
mod npipe_api;
mod npipe_sem;
pub mod inherit;
pub mod config_sys;
pub mod config_sys_parse;
pub mod session_threads;
pub mod moucalls;
pub mod profile;
pub mod profile_text;
mod pmshapi;
pub mod helpmgr;
mod pmspl;
mod pm_stddlg;
mod pm_filedlg;
mod pm_fontdlg;
mod seh;
mod cmd;
pub mod rexx;
pub mod rexx_vfs;
pub mod rexxapi;
pub mod threads;
mod host_signal;
pub(crate) mod signals;
mod interrupt;
mod exitlist;
pub mod som;
pub mod som_dispatch;
mod critsec;
pub mod dostimer;
pub mod suballoc;
mod modpath;

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
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32};
use log::{info, warn};
use crate::gui::font::{self, CjkRegion, FontSpec};
use crate::gui::surface::{DrawOp, Mix};

/// Key information pushed into the SDL text-mode keyboard queue.
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
#[derive(Clone, Debug, Default)]
pub(crate) struct FaultContext {
    pub eax: u32, pub ebx: u32, pub ecx: u32, pub edx: u32,
    pub esi: u32, pub edi: u32, pub ebp: u32,
    pub esp: u32, pub eip: u32, pub eflags: u32,
    pub cs: u16, pub ds: u16, pub es: u16, pub fs: u16, pub gs: u16, pub ss: u16,
}

impl FaultContext {
    /// A context with no registers set but a thread's flat segments (0x08 /
    /// 0x10 / 0x18 with RPL 3, ADR-0113) and EFLAGS' reserved bit.
    pub(crate) fn flat() -> Self {
        use ring::user_selector;
        let data = user_selector(0x10);
        FaultContext {
            eflags: 2,
            cs: user_selector(0x08), ds: data, es: data, fs: user_selector(0x18), gs: data, ss: data,
            ..Default::default()
        }
    }
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
    /// `WM_MINMAXFRAME` sent to a frame's client before the frame is
    /// minimized, maximized or restored (`op`, one `SWP_*` state flag) to
    /// `target`. A FALSE reply lets the change happen
    /// (`WindowManager::finish_minmax`); the API caller then gets `ret`.
    MinMaxFrame { frame: u32, op: u32, target: pm_minmax::WinRect, ret: u32 },
    /// An exception handler called by `DosUnwindException` with
    /// `EH_UNWINDING` (`seh.rs`, ADR-0110): when it returns, its record
    /// leaves the chain and the unwind goes on.
    UnwindHandler { unwind: Box<seh::Unwind> },
    /// A modal dialog message loop (WinDlgBox / WinProcessDlg / WinMessageBox).
    ///
    /// After each message callback returns, the CALLBACK_RET_TRAP handler
    /// checks `hwnd_dlg.dialog_dismissed`.  If dismissed, it returns
    /// `dialog_result` to the caller.  Otherwise it waits for the next queued
    /// message and dispatches it to its window's procedure (`dlg_proc` for
    /// the dialog; built-in windows and a message box, whose `dlg_proc` is 0,
    /// are handled on the host).
    DlgRunLoop {
        dlg_proc: u32,
        hwnd_dlg: u32,
        hmq:      u32,
    },
    /// A `WM_DESTROY` sent while windows are destroyed (`pm_destroy.rs`).
    /// When the window procedure returns, the chain goes on with the next
    /// window; after the last one the windows are removed and the chain's
    /// result is returned to the caller.
    DestroyWindows { chain: Box<pm_destroy::DestroyChain> },
    /// An owner-draw call (`pm_owner.rs`): `call` was sent with presentation
    /// space `hps` and its structure (`block_len` words) at `block_at` on
    /// the guest stack. On return the call is finished and the chain goes
    /// on with the next one.
    /// A call of a chain of guest calls (`pm_hook.rs`: hooks, the window
    /// procedures of a broadcast). `stops`: the call was one whose TRUE
    /// result ends the chain's calls.
    CallChain { chain: Box<pm_hook::CallChain>, stops: bool },
    OwnerDraw {
        chain: Box<pm_owner::OwnerChain>,
        call: pm_owner::OwnerCall,
        hps: u32,
        block_at: u32,
        block_len: u32,
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
    /// `DosExit(EXIT_THREAD, rc)`: end the calling thread with result `rc`.
    ExitThread(u32),
    /// `DosExit(EXIT_PROCESS, rc)`: end the process with result `rc`
    /// (after its exit list, `exitlist.rs`).
    ExitProcess(u32),
    /// The running exit-list routine has ended (`DosExitList(EXLST_EXIT)`):
    /// call the next one.
    ExitListNext,
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
    /// Send `WM_MINMAXFRAME` to `client` (procedure `wnd_proc`) before frame
    /// `frame` changes state (`op`) to `target`: `vcpu.rs` puts the `SWP`
    /// on the guest stack and calls the procedure; unless it returns TRUE
    /// the change is made. The API returns `ret`.
    MinMaxFrame {
        wnd_proc: u32,
        client:   u32,
        frame:    u32,
        op:       u32,
        target:   pm_minmax::WinRect,
        ret:      u32,
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
    /// Run a modal dialog message loop on behalf of WinDlgBox / WinProcessDlg
    /// / WinMessageBox (`dlg_proc` 0: a dialog without a dialog procedure).
    ///
    /// `vcpu.rs` dispatches messages from `hmq` to their windows' procedures in a loop,
    /// blocking on the queue condvar between messages.  The loop terminates
    /// when `hwnd_dlg.dialog_dismissed` is set by WinDismissDlg, returning
    /// `dialog_result` to the original API caller.
    DlgRunLoop {
        dlg_proc: u32,
        hwnd_dlg: u32,
        hmq:      u32,
    },
    /// Send `WM_DESTROY` to `hwnd` through `wnd_proc` as part of a window
    /// destruction (`WinDestroyWindow`, the end of `WinDlgBox`); `vcpu.rs`
    /// continues `chain` when the procedure returns
    /// (`FrameKind::DestroyWindows`).
    DestroyWindows {
        chain:    Box<pm_destroy::DestroyChain>,
        hwnd:     u32,
        wnd_proc: u32,
    },
    /// Make the guest calls of `chain` (`pm_hook.rs`) before the API
    /// returns: hooks, or the window procedures of a broadcast; `vcpu.rs`
    /// (`continue_call_chain`) then returns or runs the API again.
    CallChain(Box<pm_hook::CallChain>),
    /// `DosUnwindException`: call the handlers of `Unwind` one by one
    /// (`vcpu.rs` makes the calls, `seh.rs` the steps), then go on where
    /// it says.
    Unwind(Box<seh::Unwind>),
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
/// Level 1: threads.table              (lightweight, rarely contended)
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
/// - `sem_mgr` may lock, inside, the semaphore objects and then the
///   `SemSignal` generation lock (`ipc.rs`), which is a leaf; muxwait
///   waiters block on `SemSignal` holding nothing else.
/// - `pipe_sems` is taken alone (no other `SharedState` lock held or taken
///   inside it); its watcher threads lock `sem_mgr` only after releasing it.
pub struct SharedState {
    pub mem_mgr: Mutex<MemoryManager>,
    pub handle_mgr: Mutex<HandleManager>,
    pub resource_mgr: Mutex<ResourceManager>,
    pub shmem_mgr: Mutex<SharedMemManager>,
    /// The session's shared arena, once attached (`shared_arena.rs`);
    /// without it shared memory stays within this process.
    pub arena: std::sync::OnceLock<shared_arena::Arena>,
    /// The session's shared semaphores, once attached (`shared_sem.rs`).
    pub shared_sems: std::sync::OnceLock<Arc<shared_sem::SharedSems>>,
    /// The link to the session's desktop server, once connected (ADR-0052):
    /// calls on other programs' windows go through it (`pm_remote.rs`).
    pub desktop_link: std::sync::OnceLock<Arc<crate::desktop::client::DesktopLink>>,
    pub process_mgr: Mutex<ProcessManager>,
    pub sem_mgr: Mutex<SemaphoreManager>,
    pub hdir_mgr: Mutex<HDirManager>,
    pub queue_mgr: Mutex<QueueManager>,
    pub window_mgr: Mutex<WindowManager>,
    /// Last PM error (`WinGetLastError`) of every thread.
    pub pm_errors: pm_error::PmErrorStore,
    /// PM pointers and the one shown (taken before `window_mgr`).
    pub pointers: Mutex<pm_pointer::PointerManager>,
    /// Pointer input for a drag, from the GUI thread to `DrgDrag`
    /// (`pm_drag.rs`; its lock is taken alone).
    pub drag_channel: pm_drag::DragChannel,
    /// The SOM runtime (`som.rs`; its lock is taken alone or before `mem_mgr`).
    pub som: Mutex<som::SomRuntime>,
    /// The REXX API's registrations, queues and running programs
    /// (`rexxapi.rs`; its lock is taken alone).
    pub rexx_api: Mutex<rexxapi::RexxApi>,
    pub drive_mgr: Mutex<DriveManager>,
    /// Profiles opened with `PrfOpenProfile` (taken before `drive_mgr`).
    pub profiles: Mutex<pmshapi::ProfileTable>,
    pub console_mgr: Mutex<console::VioManager>,
    pub mmpm_mgr: Mutex<mmpm::MmpmManager>,
    /// Audio output, played by the main-thread loop (`gui::audio`).
    pub audio: crate::gui::audio::AudioService,
    pub dll_mgr: Mutex<DllManager>,
    pub uconv_mgr: Mutex<uconv::UconvManager>,
    pub socket_mgr: Mutex<tcpip::SocketManager>,
    /// Named pipes served by this process and the session's pipe directory.
    pub npipes: Mutex<npipe::NamedPipes>,
    /// Semaphores attached to named-pipe ends (`npipe_sem.rs`); taken alone,
    /// with no other `SharedState` lock held or taken inside it.
    pub pipe_sems: Mutex<npipe_sem::PipeSemTable>,
    /// Command line and environment the process was started with.
    pub startup: Mutex<inherit::StartupInfo>,
    /// Executable name as provided on the command line
    pub exe_name: Mutex<String>,
    pub guest_mem: GuestMemory,
    /// The process's threads: running, ended, idle vCPUs (`threads.rs`).
    pub threads: threads::Threads,
    /// Its part of the session's `THREADS=` accounting (`session_threads.rs`).
    pub thread_slots: Mutex<session_threads::ThreadSlots>,
    /// The words of the thread-local area handed out (`paging.rs`).
    pub tlm: Mutex<paging::TlmArea>,
    /// The paging arena's blocks in use (`paging.rs`, ADR-0113).
    pub paging_arena: Mutex<paging::PagingArena>,
    /// Whether the process page tables are written (`paging.rs`, ADR-0114).
    pub process_tables_ready: AtomicBool,
    /// The page states of `DosAllocMem` allocations (`vmem.rs`, ADR-0114).
    pub vmem: Mutex<vmem::PageMap>,
    /// Counted up when pages lose access: every vCPU flushes its TLB once
    /// it sees a new value (`vmem.rs`).
    pub tlb_epoch: std::sync::atomic::AtomicU64,
    /// Set while allocations or frees wait for the page tables
    /// (`MemoryManager::pending_flag`, `vmem.rs`).
    pub alloc_pending: Arc<AtomicBool>,
    /// The read ends of children's exit reports, by host pid (`exit_report.rs`).
    pub exit_reports: Mutex<std::collections::HashMap<u32, std::fs::File>>,
    /// `DosAsyncTimer` / `DosStartTimer` timers (`dostimer.rs`).
    pub dos_timers: Mutex<dostimer::DosTimerTable>,
    /// `DosSubSetMem` pools, kept on the host (`suballoc.rs`).
    pub suballoc: Mutex<suballoc::SubAllocTable>,
    /// Children `DosKillProcess` was used on; whether this process is
    /// being ended from outside (`signals.rs`).
    pub kills: Mutex<signals::KillTable>,
    /// The process's clock: the host's plus the offset `DosSetDateTime`
    /// set or the parent passed on (`clock.rs`, ADR-0039).
    pub clock: clock::SessionClock,
    /// Whether the `GINFOSEG` refresher thread runs (`infoseg.rs`).
    pub infoseg_refresher: AtomicBool,
    pub exit_requested: AtomicBool,
    /// A VIO session may switch between its PM window and full screen with
    /// Alt+Home (the program is window-compatible); set by `run_vio_session`.
    pub vio_switchable: AtomicBool,
    /// Alt+Home was pressed in a switchable VIO session: the GUI loop returns
    /// so `run_vio_session` can show the other display (`gui::vio_window`).
    pub vio_switch_requested: AtomicBool,
    pub exit_code: AtomicI32,
    pub locale: locale::Os2Locale,
    /// Active process codepage — updated by DosSetProcessCp; read by DosQueryCp.
    /// Starts as `locale.codepage` (host locale or `WARPINE_CODEPAGE`).
    pub active_codepage: AtomicU32,
    /// Keyboard event queue for SDL text-mode (CLI) apps.
    /// The VCPU thread blocks on `kbd_cond` when `use_sdl_text` is set.
    pub kbd_queue: Mutex<VecDeque<KbdKeyInfo>>,
    /// Condvar paired with `kbd_queue` to wake `KbdCharIn` when a key arrives.
    pub kbd_cond: Condvar,
    /// When `true`, `KbdCharIn` reads from `kbd_queue` instead of termios stdin.
    /// Set before spawning the VCPU for SDL text-mode CLI apps.
    pub use_sdl_text: AtomicBool,
    /// The VIO session's mouse (MOUCALLS): event queue, pointer, settings.
    pub mou_mgr: Mutex<moucalls::MouManager>,
    /// Condvar paired with `mou_mgr` to wake `MouReadEventQue` (wait).
    pub mou_cond: Condvar,
    /// Ring buffer of the last 256 OS/2 API calls — always populated,
    /// regardless of log level, so crash dumps include full call history.
    pub api_ring: Mutex<ApiRingBuffer>,
    /// Drive C's CONFIG.SYS, read once when first needed
    /// ([`SharedState::config_sys`], `config_sys.rs`).
    pub config_sys_cell: std::sync::OnceLock<Arc<config_sys::ConfigSys>>,
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
        let guest_mem_size = GUEST_MEM_SIZE as usize;
        let guest_mem = GuestMemory::alloc(guest_mem_size);
        vm.register_guest_memory(0, guest_mem_size as u64, guest_mem.host_base_addr()).unwrap();

        // Private allocations end where the shared arena begins.
        let mem_mgr = MemoryManager::new(DYNAMIC_ALLOC_BASE, SHARED_ARENA_BASE);
        let alloc_pending = mem_mgr.pending_flag();
        let handle_mgr = HandleManager::new();
        let resource_mgr = ResourceManager::new();
        let shmem_mgr = SharedMemManager::new();
        let process_mgr = ProcessManager::new();
        let sem_mgr = SemaphoreManager::new();
        let hdir_mgr = HDirManager::new();
        let queue_mgr = QueueManager::new();
        let mut window_mgr = WindowManager::new();
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
        // The host locale (or WARPINE_COUNTRY / WARPINE_CODEPAGE) chooses the
        // country and the starting process codepage, which the VIO screen and
        // the PM CJK face preference follow (as DosSetProcessCp does later).
        let locale = locale::Os2Locale::from_host();
        let mut console_mgr = console::VioManager::new();
        console_mgr.codepage = locale.codepage as u16;
        crate::gui::font::set_active_codepage(locale.codepage);

        // The command line, environment and process id a parent passed.
        let startup = inherit::StartupInfo::from_env();
        // The current drive and directories the parent had.
        if let Some(cwd) = &startup.cwd { inherit::apply_cwd(&mut drive_mgr, cwd); }
        window_mgr.switch_list.own_process = startup.process_id();

        let shared = Arc::new(SharedState {
            mem_mgr: Mutex::new(mem_mgr),
            handle_mgr: Mutex::new(handle_mgr),
            resource_mgr: Mutex::new(resource_mgr),
            shmem_mgr: Mutex::new(shmem_mgr),
            arena: std::sync::OnceLock::new(),
            shared_sems: std::sync::OnceLock::new(),
            desktop_link: std::sync::OnceLock::new(),
            process_mgr: Mutex::new(process_mgr),
            sem_mgr: Mutex::new(sem_mgr),
            hdir_mgr: Mutex::new(hdir_mgr),
            queue_mgr: Mutex::new(queue_mgr),
            window_mgr: Mutex::new(window_mgr),
            pm_errors: Default::default(),
            profiles: Default::default(),
            pointers: Default::default(),
            drive_mgr: Mutex::new(drive_mgr),
            console_mgr: Mutex::new(console_mgr),
            mmpm_mgr: Mutex::new(mmpm::MmpmManager::new()),
            audio: Default::default(),
            dll_mgr: Mutex::new(DllManager::new()),
            uconv_mgr: Mutex::new(uconv::UconvManager::new()),
            socket_mgr: Mutex::new(tcpip::SocketManager::new()),
            npipes: Mutex::new(npipe::NamedPipes::new(npipe::PipeDir::from_env())),
            pipe_sems: Default::default(),
            startup: Mutex::new(startup),
            exe_name: Mutex::new(String::new()),
            guest_mem,
            threads: Default::default(),
            thread_slots: Default::default(),
            tlm: Default::default(),
            paging_arena: Default::default(),
            process_tables_ready: AtomicBool::new(false),
            vmem: Default::default(),
            tlb_epoch: Default::default(),
            alloc_pending,
            exit_reports: Default::default(),
            dos_timers: Default::default(),
            suballoc: Default::default(),
            kills: Default::default(),
            clock: clock::SessionClock::from_env(),
            infoseg_refresher: AtomicBool::new(false),
            drag_channel: Default::default(),
            som: Default::default(),
            rexx_api: Default::default(),
            exit_requested: AtomicBool::new(false),
            vio_switchable: AtomicBool::new(false),
            vio_switch_requested: AtomicBool::new(false),
            exit_code: AtomicI32::new(0),
            active_codepage: AtomicU32::new(locale.codepage),
            locale,
            kbd_queue: Mutex::new(VecDeque::new()),
            kbd_cond: Condvar::new(),
            use_sdl_text: AtomicBool::new(false),
            mou_mgr: Mutex::new(moucalls::MouManager::new()),
            mou_cond: Condvar::new(),
            api_ring: Mutex::new(ApiRingBuffer::new()),
            config_sys_cell: std::sync::OnceLock::new(),
            begin_libpath: Mutex::new(inherit::inherited_libpath(inherit::BEGINLIBPATH_ENV)),
            end_libpath: Mutex::new(inherit::inherited_libpath(inherit::ENDLIBPATH_ENV)),
            gdb_state: None,
        });

        let loader = Loader { vm, shared };
        // Pipes a parent process passed on (DosExecPgm).
        loader.adopt_inherited_handles();
        loader
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
    /// Whether the calling thread must give up waiting: the process ends,
    /// or another thread runs the exit list (`exitlist.rs`).
    pub(crate) fn shutting_down(&self) -> bool {
        self.shared.exit_requested.load(std::sync::atomic::Ordering::Relaxed)
            || self.shared.threads.exit_list_stops_caller()
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
        /// Drive C: of mock loaders, relative to the crate root (git-ignored).
        const MOCK_DRIVE_C: &str = "target/test-drive-c";
        use vm_backend::mock::MockVmBackend;
        let vm: Arc<dyn VmBackend> = Arc::new(MockVmBackend);
        let guest_mem_size = 64 * 1024 * 1024; // 64 MB (must cover DYNAMIC_ALLOC_BASE = 0x02000000)
        let guest_mem = GuestMemory::alloc(guest_mem_size);
        vm.register_guest_memory(0, guest_mem_size as u64, guest_mem.host_base_addr()).unwrap();
        let mem_mgr = MemoryManager::new(DYNAMIC_ALLOC_BASE, DYNAMIC_ALLOC_BASE + guest_mem_size as u32);
        let alloc_pending = mem_mgr.pending_flag();
        let shared = Arc::new(SharedState {
            mem_mgr:      Mutex::new(mem_mgr),
            handle_mgr:   Mutex::new(HandleManager::new()),
            resource_mgr: Mutex::new(ResourceManager::new()),
            shmem_mgr:    Mutex::new(SharedMemManager::new()),
            arena: std::sync::OnceLock::new(),
            shared_sems: std::sync::OnceLock::new(),
            desktop_link: std::sync::OnceLock::new(),
            process_mgr:  Mutex::new(ProcessManager::new()),
            sem_mgr:      Mutex::new(SemaphoreManager::new()),
            hdir_mgr:     Mutex::new(HDirManager::new()),
            queue_mgr:    Mutex::new(QueueManager::new()),
            window_mgr:   Mutex::new(WindowManager::new()),
            pm_errors:    Default::default(),
            profiles:     Default::default(),
            pointers:     Default::default(),
            // Never the user's own drive C (`$XDG_DATA_HOME/warpine/drive_c`):
            // a test drive under the build directory, shared by the tests.
            drive_mgr:    Mutex::new(DriveManager::with_drive_c(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(MOCK_DRIVE_C))),
            console_mgr:  Mutex::new(console::VioManager::new()),
            mmpm_mgr:     Mutex::new(mmpm::MmpmManager::new()),
            audio:        Default::default(),
            dll_mgr:      Mutex::new(DllManager::new()),
            uconv_mgr:    Mutex::new(uconv::UconvManager::new()),
            socket_mgr:   Mutex::new(tcpip::SocketManager::new()),
            npipes:       Default::default(),
            pipe_sems:    Default::default(),
            startup:      Default::default(),
            exe_name:     Mutex::new(String::new()),
            guest_mem,
            threads:      Default::default(),
            thread_slots: Default::default(),
            tlm:          Default::default(),
            paging_arena: Default::default(),
            process_tables_ready: AtomicBool::new(false),
            vmem: Default::default(),
            tlb_epoch: Default::default(),
            alloc_pending,
            exit_reports: Default::default(),
            dos_timers:   Default::default(),
            suballoc:     Default::default(),
            kills:        Default::default(),
            clock:        Default::default(),
            infoseg_refresher: AtomicBool::new(false),
            drag_channel: Default::default(),
            som: Default::default(),
            rexx_api: Default::default(),
            exit_requested: AtomicBool::new(false),
            vio_switchable: AtomicBool::new(false),
            vio_switch_requested: AtomicBool::new(false),
            exit_code:    AtomicI32::new(0),
            // Pinned to the default country / codepage so tests do not
            // depend on the host's locale.
            locale:       {
                let mut l = locale::Os2Locale::from_host();
                (l.country, l.codepage) = locale::DEFAULT_COUNTRY_CP;
                l
            },
            active_codepage: AtomicU32::new(locale::DEFAULT_COUNTRY_CP.1),
            kbd_queue:    Mutex::new(VecDeque::new()),
            kbd_cond:     Condvar::new(),
            use_sdl_text: AtomicBool::new(false),
            mou_mgr:      Mutex::new(moucalls::MouManager::new()),
            mou_cond:     Condvar::new(),
            api_ring:     Mutex::new(ApiRingBuffer::new()),
            config_sys_cell: std::sync::OnceLock::new(),
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
                 w.text.clone(), w.parent)
            })
        };
        let (class_name, x, y, cx, cy, text, parent) = match info {
            Some(v) => v,
            None    => return ApiResult::Normal(0),
        };

        // A help instance (helpmgr.rs) accepts every message (HM_*) and
        // does nothing: Warpine has no help viewer.
        if class_name == helpmgr::HELP_INSTANCE_CLASS {
            return ApiResult::Normal(0);
        }
        // A standard frame (of the application's class, without a procedure
        // of its own) runs the system frame procedure: WinDefWindowProc,
        // with the frame formatting messages (pm_frame.rs).
        let std_frame = {
            let wm = self.shared.window_mgr.lock_or_recover();
            wm.frame_to_client.contains_key(&hwnd) && !wm.is_dialog(hwnd)
                && !pm_types::WindowManager::is_builtin_class(&class_name)
        };
        if std_frame {
            return self.def_window_proc(interrupt::current_vcpu().unwrap_or(0), hwnd, msg, mp1, mp2);
        }
        // The frame procedure's formatting messages (pm_frame.rs), also
        // for a subclass procedure passing them on.
        if let Some(r) = self.frame_default_message(hwnd, msg, mp1, mp2) {
            return ApiResult::Normal(r);
        }
        // What a dialog asks its controls: their kind, and whether a
        // mnemonic is theirs (pm_dialog.rs).
        if msg == WM_QUERYDLGCODE {
            let style = self.shared.window_mgr.lock_or_recover().get_window(hwnd).map_or(0, |w| w.style);
            return ApiResult::Normal(pm_dialog::dialog_code(&class_name, style));
        }
        // The pointer over a control: its own, or the one its owner
        // returns from WM_CONTROLPOINTER (pm_pointer.rs).
        if msg == WM_MOUSEMOVE {
            self.control_mouse_pointer(hwnd, &class_name);
        }
        if msg == WM_MATCHMNEMONIC {
            let matched = char::from_u32(mp1 & 0xFF).is_some_and(|ch| {
                pm_dialog::mnemonic(&text).is_some_and(|m| m.eq_ignore_ascii_case(&ch))
            });
            return ApiResult::Normal(matched as u32);
        }

        // A dialog without a dialog procedure (a message box): its default
        // dialog procedure. (Painting is the dialog's own, below.)
        if class_name == pm_types::WindowManager::DIALOG_CLASS && msg != WM_PAINT {
            let procless = self.shared.window_mgr.lock_or_recover()
                .get_window(hwnd).is_some_and(|w| w.pfn_wp == 0);
            if procless {
                // The standard file and font dialogs (pm_stddlg.rs).
                if let Some(r) = self.std_dialog_procless(hwnd, msg, mp1, mp2) {
                    return r;
                }
                return self.procless_dialog_message(hwnd, msg, mp1, mp2);
            }
        }

        // Sliders and value sets (pm_slider.rs, pm_valueset.rs); a value set
        // answers a double click itself (VN_ENTER).
        if msg != WM_PAINT && class_name == pm_slider::CLASS_SLIDER
            && let Some(r) = self.slider_message(hwnd, msg, mp1, mp2) {
            return r;
        }
        if msg != WM_PAINT && class_name == pm_circslider::CLASS_CIRCSLIDER
            && let Some(r) = self.circslider_message(hwnd, msg, mp1, mp2) {
            return r;
        }
        if msg != WM_PAINT && class_name == pm_valueset::CLASS_VALUESET
            && let Some(r) = self.valueset_message(hwnd, msg, mp1, mp2) {
            return r;
        }
        // Notebooks (pm_notebook.rs).
        if msg != WM_PAINT && class_name == pm_notebook::CLASS_NOTEBOOK
            && let Some(r) = self.notebook_message(hwnd, msg, mp1, mp2) {
            return r;
        }
        // The direct-editing MLE of a container: Esc discards the edit.
        if msg == WM_CHAR && class_name == pm_controls::CLASS_MLE
            && self.shared.window_mgr.lock_or_recover().get_window(parent)
                .is_some_and(|p| p.class_name == pm_container::CLASS_CONTAINER)
            && let Some(r) = self.container_edit_key(parent, hwnd, mp1, mp2) {
            return r;
        }
        // Containers (pm_container.rs); a container answers a double click
        // itself (CN_ENTER).
        if msg != WM_PAINT && class_name == pm_container::CLASS_CONTAINER
            && let Some(r) = self.container_message(hwnd, msg, mp1, mp2) {
            return r;
        }

        // The second press of a double click: a list box selects and tells
        // its owner LN_ENTER; every other built-in control treats it as a press.
        if msg == WM_BUTTON1DBLCLK {
            if class_name == pm_controls::CLASS_LISTBOX {
                return self.listbox_double_click(hwnd, cy, mp1, mp2);
            }
            return self.dispatch_builtin_control(hwnd, WM_BUTTON1DOWN, mp1, mp2);
        }

        // Combination boxes and spin buttons (their parts are ordinary
        // built-in controls).
        if msg != WM_PAINT && class_name == pm_combo::CLASS_COMBOBOX
            && let Some(r) = self.combo_message(hwnd, msg, mp1, mp2) {
            return r;
        }
        if msg != WM_PAINT && class_name == pm_spin::CLASS_SPINBUTTON
            && let Some(r) = self.spin_message(hwnd, msg, mp1, mp2) {
            return r;
        }

        match msg {
            WM_PAINT => {
                // A built-in window paints all of itself: it is valid now.
                // A hidden one — hidden since its WM_PAINT was queued, like
                // a container's scroll bar the view no longer needs — or
                // one in a hidden embedded dialog (a notebook page turned
                // away) draws nothing.
                let frame_hwnd = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    wm.painted(hwnd);
                    // (WC_* controls only: Warpine's own pseudo-windows such
                    // as #Menu, never marked visible, still pass WM_PAINT on.)
                    let hidden = wm.get_window(hwnd).is_some_and(|w| !w.visible && w.class_name.starts_with("WC_"));
                    if hidden || wm.in_hidden_embedded_dialog(hwnd) {
                        return ApiResult::Normal(0);
                    }
                    wm.find_frame_for_hwnd(hwnd)
                };
                let p = Painter::new(self, frame_hwnd, hwnd);

                match class_name.as_str() {
                    "WC_BUTTON" => {
                        let style = self.shared.window_mgr.lock_or_recover()
                            .get_window(hwnd).map_or(0, |w| w.style);
                        // A disabled button's label is greyed.
                        let p = if style & WS_DISABLED != 0 { p.greyed() } else { p };
                        if pm_button::has_check_state(style) {
                            self.paint_check_button(&p, hwnd, (x, y, cx, cy), &text);
                        } else if style & BS_PRIMARYSTYLES == BS_USERBUTTON {
                            // The owner paints it (BN_PAINT, pm_owner.rs) on
                            // the button face.
                            p.fill((x, y, x + cx - 1, y + cy - 1), CONTROL_FACE_COLOR);
                            self.shared.window_mgr.lock_or_recover().queue_owner_call(pm_owner::OwnerCall {
                                control: hwnd, kind: pm_owner::OwnerCallKind::UserButton,
                            });
                        } else {
                            self.paint_button(&p, hwnd, (x, y, cx, cy), &text);
                        }
                    }
                    // Text, group box, icon, bitmap, rectangle or frame
                    // (pm_static.rs).
                    "WC_STATIC" => self.paint_static(&p, hwnd, (x, y, cx, cy), &text),
                    pm_scrollbar::CLASS_SCROLLBAR => {
                        self.paint_scrollbar(&p, hwnd, (x, y, cx, cy));
                    }
                    pm_controls::CLASS_ENTRYFIELD => {
                        self.paint_entry_field(&p, hwnd, (x, y, cx, cy));
                    }
                    pm_controls::CLASS_MLE => {
                        self.paint_mle(&p, hwnd, (x, y, cx, cy));
                    }
                    pm_controls::CLASS_LISTBOX => {
                        self.paint_listbox(&p, hwnd, (x, y, cx, cy));
                    }
                    pm_combo::CLASS_COMBOBOX => self.paint_combo(&p, hwnd, (x, y, cx, cy)),
                    pm_spin::CLASS_SPINBUTTON => self.paint_spin(&p, hwnd, (x, y, cx, cy)),
                    // Sliders and value sets draw parts that can reach
                    // past their window (texts, fixed sizes): clipped to it.
                    pm_slider::CLASS_SLIDER => {
                        let p = Painter::new(self, frame_hwnd, hwnd).clipped_to((x, y, cx, cy));
                        self.paint_slider(&p, hwnd, (x, y, cx, cy));
                    }
                    pm_circslider::CLASS_CIRCSLIDER => {
                        let p = Painter::new(self, frame_hwnd, hwnd).clipped_to((x, y, cx, cy));
                        self.paint_circslider(&p, hwnd, (x, y, cx, cy), &text);
                    }
                    pm_valueset::CLASS_VALUESET => {
                        let p = Painter::new(self, frame_hwnd, hwnd).clipped_to((x, y, cx, cy));
                        self.paint_valueset(&p, hwnd, (x, y, cx, cy));
                    }
                    pm_notebook::CLASS_NOTEBOOK => {
                        let p = Painter::new(self, frame_hwnd, hwnd).clipped_to((x, y, cx, cy));
                        self.paint_notebook(&p, hwnd, (x, y, cx, cy));
                    }
                    pm_container::CLASS_CONTAINER => {
                        let p = Painter::new(self, frame_hwnd, hwnd).clipped_to((x, y, cx, cy));
                        self.paint_container(&p, hwnd, (x, y, cx, cy));
                    }
                    "#Menu" => {
                        // The menu bar and any open pull-down are frame chrome:
                        // the compositor draws them from `OS2Window::menu_items`
                        // and the open-item state on every composite, so they
                        // never share pixels with the client's surface.
                        // Painting therefore only has to request a recomposite.
                        self.shared.window_mgr.lock_or_recover().mark_dirty(frame_hwnd);
                    }
                    pm_types::WindowManager::DIALOG_CLASS => {
                        // The dialog face, then its controls on top. A dialog
                        // frame's title bar and border are chrome drawn by the
                        // compositor (from `frame_flags` and the window text).
                        // A dialog on the desktop fills its own surface
                        // from (0, 0); an embedded one (ADR-0042) its place
                        // in the surface of the frame it lies in, clipped.
                        let d = if frame_hwnd == hwnd {
                            Painter::new(self, hwnd, hwnd)
                        } else {
                            Painter::new(self, frame_hwnd, hwnd).clipped_to((x, y, cx, cy))
                        };
                        d.fill((x, y, x + cx, y + cy), d.bg.unwrap_or(CONTROL_FACE_COLOR));
                        // An embedded dialog draws its own chrome (ADR-0069).
                        if frame_hwnd != hwnd {
                            self.paint_embedded_dialog_chrome(&d, hwnd, (x, y, cx, cy));
                        }
                        self.paint_message_box_icon(&d, hwnd);
                        let controls = self.shared.window_mgr.lock_or_recover()
                            .builtin_control_descendants(hwnd);
                        for control in controls {
                            self.dispatch_builtin_control(control, WM_PAINT, 0, 0);
                        }
                        // Dialogs embedded in this one paint over it again.
                        let mut wm = self.shared.window_mgr.lock_or_recover();
                        for inner in wm.embedded_dialogs(hwnd) {
                            wm.invalidate_window_only(inner);
                        }
                    }
                    _ => {} // Unknown built-in class — silently ignore WM_PAINT
                }
                // A dropped combination box list over this control is drawn
                // over it again (pm_combo.rs).
                let over = self.shared.window_mgr.lock_or_recover().dropped_lists_over(hwnd);
                for list in over {
                    self.dispatch_builtin_control(list, WM_PAINT, 0, 0);
                }
                ApiResult::Normal(0)
            }

            WM_BUTTON1DOWN if class_name == "WC_BUTTON" => self.button_down(hwnd),

            WM_BUTTON1UP if class_name == "WC_BUTTON" => self.button_up(hwnd, cx, cy, mp1),

            WM_CHAR if class_name == "WC_BUTTON" => self.button_char(hwnd, mp1, mp2),

            BM_SETCHECK | BM_QUERYCHECK | BM_QUERYCHECKINDEX | BM_CLICK | BM_QUERYHILITE
            | BM_SETHILITE | BM_SETDEFAULT if class_name == "WC_BUTTON" => {
                self.button_message(hwnd, msg, mp1)
            }

            MM_INSERTITEM..=MM_ISITEMVALID if class_name == "#Menu"
                && let Some(r) = self.menu_item_message(hwnd, msg, mp1, mp2) => r,
            MM_STARTMENUMODE if class_name == "#Menu" => {
                ApiResult::Normal(self.start_menu_mode(hwnd, mp1 & 0xFFFF != 0) as u32)
            }
            MM_ENDMENUMODE if class_name == "#Menu" => {
                self.end_menu_mode(hwnd);
                ApiResult::Normal(0)
            }

            WM_BUTTON1DOWN if class_name == "#Menu" => {
                // A click ends keyboard menu mode (pm_menubar.rs).
                if let Some(w) = self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd) {
                    w.window_ulong.remove(&pm_menubar::MENU_MODE_KEY);
                }
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
                    let mut item_x = x + MENU_BAR_ITEM_INSET;
                    let mut hit_idx: Option<usize> = None;
                    for (i, item) in items.iter().enumerate() {
                        if item.style & MIS_SEPARATOR != 0 { continue; }
                        let label_w = pm_types::menu_bar_item_width(&item.text);
                        if click_x >= item_x && click_x < item_x + label_w {
                            hit_idx = Some(i);
                            break;
                        }
                        item_x += label_w;
                    }
                    if let Some(idx) = hit_idx {
                        // Toggle: clicking the already-open item closes it.
                        let new_open = if open_item == idx as u32 + 1 { 0 } else { idx as u32 + 1 };
                        // The client hears of it first (pm_menubar.rs), so
                        // WM_INITMENU comes before the items are drawn.
                        match items.get(idx) {
                            Some(item) if new_open != 0 => self.mouse_menu_opened(hwnd, open_item != 0, item.id),
                            _ => self.mouse_menu_closed(hwnd),
                        }
                        {
                            let mut wm = self.shared.window_mgr.lock_or_recover();
                            if let Some(win) = wm.get_window_mut(hwnd) {
                                win.window_ulong.remove(&pm_menubar::CASCADE_KEY);
                                if new_open == 0 { win.window_ulong.remove(&-1); }
                                else { win.window_ulong.insert(-1, new_open); }
                            }
                            // Owner-drawn items of the opening pull-down are
                            // drawn by the owner (pm_owner.rs).
                            // (measured first, WM_MEASUREITEM, pm_menubar.rs).
                            if new_open != 0 && let Some(parent) = items.get(idx) {
                                for kind in pm_menubar::pulldown_owner_calls(parent) {
                                    wm.queue_owner_call(pm_owner::OwnerCall { control: hwnd, kind });
                                }
                            }
                        }
                        // Repaint the menu bar (and dropdown if newly opened).
                        return self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                    } else if open_item > 0 {
                        // Click on empty part of menu bar: close dropdown.
                        self.mouse_menu_closed(hwnd);
                        self.close_pulldown(hwnd);
                        return self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                    }
                } else if open_item > 0 {
                    // Click below the menu bar — check if it's in the open dropdown.
                    let idx = (open_item - 1) as usize;
                    if let Some(parent_item) = items.get(idx) {
                        let mut drop_x = x + MENU_BAR_ITEM_INSET;
                        for (i, item) in items.iter().enumerate() {
                            if i >= idx { break; }
                            if item.style & MIS_SEPARATOR != 0 { continue; }
                            drop_x += pm_types::menu_bar_item_width(&item.text);
                        }
                        const DROP_W: i32 = MENU_DROPDOWN_W;
                        // The owner's measured row height (WM_MEASUREITEM).
                        let item_h = parent_item.row_h();
                        let child_count = parent_item.children.len() as i32;
                        let drop_y2 = y; // same as menu bar bottom
                        let drop_y1 = drop_y2 - child_count * item_h;

                        // An open cascade (pm_menubar.rs) lies over the
                        // pull-down's side: a click in it chooses.
                        if let Some((c_x, c_y2, sub)) = self.open_cascade(hwnd, parent_item, drop_x, drop_y2) {
                            let sub_h = sub.row_h();
                            let c_y1 = c_y2 - sub.children.len() as i32 * sub_h;
                            if click_x >= c_x && click_x < c_x + DROP_W && click_y >= c_y1 && click_y < c_y2 {
                                let pick = sub.children.get(((c_y2 - 1 - click_y) / sub_h) as usize)
                                    .filter(|c| c.style & MIS_SEPARATOR == 0 && c.attr & MIA_DISABLED == 0
                                                && c.children.is_empty());
                                if let Some(c) = pick {
                                    self.close_pulldown(hwnd);
                                    self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                                    self.mouse_menu_chose(hwnd, c.id, c.style);
                                }
                                return ApiResult::Normal(0);
                            }
                        }
                        if click_x >= drop_x && click_x < drop_x + DROP_W
                           && click_y >= drop_y1 && click_y < drop_y2
                        {
                            // Determine which child item was clicked.
                            let child_idx = ((drop_y2 - 1 - click_y) / item_h) as usize;
                            if let Some(child) = parent_item.children.get(child_idx)
                                && child.style & MIS_SEPARATOR == 0 && child.attr & MIA_DISABLED == 0
                                && !child.children.is_empty()
                            {
                                // A row with a cascade opens or closes it.
                                self.mouse_cascade(hwnd, parent_item, child_idx);
                                return self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                            }
                            if let Some(child) = parent_item.children.get(child_idx)
                                && child.style & MIS_SEPARATOR == 0 && child.attr & MIA_DISABLED == 0
                            {
                                // Close dropdown before dispatching.
                                self.close_pulldown(hwnd);
                                self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                                // WM_MENUSELECT(TRUE) to the client, whose
                                // reply decides whether the command (with
                                // MPFROM2SHORT(CMDSRC_MENU, fPointer = TRUE))
                                // is posted; then WM_MENUEND (pm_menubar.rs).
                                self.mouse_menu_chose(hwnd, child.id, child.style);
                                return ApiResult::Normal(0);
                            }
                        } else {
                            // Click outside dropdown: close it.
                            self.mouse_menu_closed(hwnd);
                            self.close_pulldown(hwnd);
                            return self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0);
                        }
                    }
                }
                ApiResult::Normal(0)
            }

            WM_MENUEND if class_name == "#Menu" => {
                // Close any open dropdown and redraw the menu bar.
                self.close_pulldown(hwnd);
                self.dispatch_builtin_control(hwnd, WM_PAINT, 0, 0)
            }

            WM_BUTTON1DOWN if class_name == pm_scrollbar::CLASS_SCROLLBAR => {
                self.scrollbar_button_down(hwnd, mp1)
            }
            WM_MOUSEMOVE if class_name == pm_scrollbar::CLASS_SCROLLBAR => {
                self.scrollbar_mouse_move(hwnd, mp1)
            }
            WM_BUTTON1UP if class_name == pm_scrollbar::CLASS_SCROLLBAR => self.scrollbar_button_up(hwnd),
            WM_CHAR if class_name == pm_scrollbar::CLASS_SCROLLBAR => self.scrollbar_char(hwnd, mp1, mp2),
            WM_TIMER if mp1 == TID_SCROLL && class_name == pm_scrollbar::CLASS_SCROLLBAR => {
                self.scrollbar_repeat(hwnd)
            }
            SBM_SETSCROLLBAR | SBM_SETPOS | SBM_QUERYPOS | SBM_QUERYRANGE | SBM_SETTHUMBSIZE
                if class_name == pm_scrollbar::CLASS_SCROLLBAR =>
            {
                self.scrollbar_message(hwnd, msg, mp1, mp2).unwrap_or(ApiResult::Normal(0))
            }

            WM_BUTTON1DOWN if pm_controls::is_input_control(&class_name) => {
                self.control_button_down(hwnd, &class_name, cy, mp1, mp2)
            }

            WM_MOUSEMOVE if pm_controls::is_input_control(&class_name) => {
                self.control_mouse_move(hwnd, &class_name, cy, mp1)
            }

            WM_BUTTON1UP if pm_controls::is_input_control(&class_name) => self.control_button_up(hwnd),

            WM_CHAR if pm_controls::is_input_control(&class_name) => {
                // SPBS_NUMERICONLY fields take digits only (pm_spin.rs).
                if self.spin_refuses_char(hwnd, mp1, mp2) {
                    return ApiResult::Normal(1);
                }
                self.control_char(hwnd, &class_name, mp1, mp2)
            }

            EM_CUT | EM_COPY | EM_PASTE | EM_CLEAR | MLM_CUT | MLM_COPY | MLM_PASTE | MLM_CLEAR
                if pm_controls::is_edit_control(&class_name) =>
            {
                let cmd = match msg {
                    EM_CUT | MLM_CUT => pm_controls::ClipboardCmd::Cut,
                    EM_COPY | MLM_COPY => pm_controls::ClipboardCmd::Copy,
                    EM_PASTE | MLM_PASTE => pm_controls::ClipboardCmd::Paste,
                    _ => pm_controls::ClipboardCmd::Clear,
                };
                let changed = self.edit_clipboard(hwnd, class_name == pm_controls::CLASS_MLE, cmd);
                ApiResult::Normal(changed as u32)
            }

            // EM_SETSEL: mp1 = MPFROM2SHORT(first, last); the caret ends at last.
            EM_SETSEL if class_name == pm_controls::CLASS_ENTRYFIELD => {
                self.edit_set_selection(hwnd, false, (mp1 & 0xFFFF) as usize, (mp1 >> 16) as usize)
            }

            // EM_QUERYSEL: MRFROM2SHORT(first, last) of the selection.
            EM_QUERYSEL if class_name == pm_controls::CLASS_ENTRYFIELD => {
                let (a, c) = self.edit_selection(hwnd, false);
                ApiResult::Normal((a.min(c) as u32 & 0xFFFF) | ((a.max(c) as u32 & 0xFFFF) << 16))
            }

            // MLM_SETSEL: mp1 = anchor, mp2 = cursor insertion points.
            MLM_SETSEL if class_name == pm_controls::CLASS_MLE => {
                self.edit_set_selection(hwnd, true, mp1 as usize, mp2 as usize)
            }

            // MLM_QUERYSEL: mp1 = MLFQS_* mode.
            MLM_QUERYSEL if class_name == pm_controls::CLASS_MLE => {
                let (a, c) = self.edit_selection(hwnd, true);
                ApiResult::Normal(match mp1 & 0xFFFF {
                    MLFQS_MINSEL => a.min(c) as u32,
                    MLFQS_MAXSEL => a.max(c) as u32,
                    MLFQS_ANCHORSEL => a as u32,
                    MLFQS_CURSORSEL => c as u32,
                    _ => (a.min(c) as u32 & 0xFFFF) | ((a.max(c) as u32 & 0xFFFF) << 16),
                })
            }

            EM_SETREADONLY if class_name == pm_controls::CLASS_ENTRYFIELD => {
                self.edit_set_read_only(hwnd, mp1 & 0xFFFF != 0)
            }
            MLM_SETREADONLY if class_name == pm_controls::CLASS_MLE => {
                self.edit_set_read_only(hwnd, mp1 & 0xFFFF != 0)
            }
            WM_TIMER if mp1 == TID_CURSOR && pm_controls::is_edit_control(&class_name) => {
                self.edit_caret_blink(hwnd)
            }
            WM_TIMER if mp1 == TID_SCROLL && pm_controls::is_input_control(&class_name) => {
                self.control_drag_scroll(hwnd, &class_name, cy)
            }

            MLM_SETWRAP if class_name == pm_controls::CLASS_MLE => {
                self.mle_set_wrap(hwnd, mp1 & 0xFFFF != 0)
            }
            MLM_QUERYWRAP if class_name == pm_controls::CLASS_MLE => {
                let wm = self.shared.window_mgr.lock_or_recover();
                let style = wm.get_window(hwnd).map_or(0, |w| w.style);
                ApiResult::Normal((style & MLS_WORDWRAP != 0) as u32)
            }
            EM_QUERYREADONLY | MLM_QUERYREADONLY if pm_controls::is_edit_control(&class_name) => {
                let wm = self.shared.window_mgr.lock_or_recover();
                let style = wm.get_window(hwnd).map_or(0, |w| w.style);
                ApiResult::Normal((style & pm_controls::read_only_style(&class_name) != 0) as u32)
            }

            LM_SELECTITEM => self.listbox_select_item(hwnd, mp1, mp2),

            LM_DELETEITEM if class_name == pm_controls::CLASS_LISTBOX => self.listbox_delete_item(hwnd, mp1),

            LM_DELETEALL if class_name == pm_controls::CLASS_LISTBOX => self.listbox_delete_all(hwnd),

            LM_QUERYSELECTION => self.listbox_query_selection(hwnd, mp1),

            LM_INSERTITEM => {
                // mp1 = MPFROMSHORT(index) — insert position; LIT_END (0xFFFF) = append
                // mp2 = MPFROMP(pszItem)   — guest pointer to item string
                let item_text = if mp2 != 0 { self.read_guest_string_cp(mp2, self.window_codepage(hwnd)) } else { String::new() };
                // LIT_SORTASCENDING / LIT_SORTDESCENDING insert in order.
                let (index, owner_draw) = {
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let Some(win) = wm.get_window_mut(hwnd) else {
                        return ApiResult::Normal(LIT_ERROR as u32);
                    };
                    if win.listbox_items.len() >= pm_controls::MAX_LIST_ITEMS {
                        return ApiResult::Normal(LIT_MEMERROR as u32);
                    }
                    let idx = mp1 as u16 as i16 as i32;
                    let at = match idx {
                        LIT_SORTASCENDING => win.listbox_items.iter().position(|t| t.as_str() > item_text.as_str()),
                        LIT_SORTDESCENDING => win.listbox_items.iter().position(|t| t.as_str() < item_text.as_str()),
                        i if i < 0 => None,
                        i => Some(i as usize).filter(|&i| i < win.listbox_items.len()),
                    };
                    let at = at.unwrap_or(win.listbox_items.len());
                    win.listbox_items.insert(at, item_text);
                    if at < win.item_handles.len() {
                        win.item_handles.insert(at, 0);
                    }
                    // Selected items keep their selection when they are
                    // pushed down by an item inserted at or before them.
                    win.listbox_sel.inserted(at);
                    (at as u32, win.style & LS_OWNERDRAW != 0)
                };
                if owner_draw {
                    // The owner gives the item height (pm_owner.rs).
                    let mut wm = self.shared.window_mgr.lock_or_recover();
                    let measuring = wm.owner_calls.iter().any(|c| c.control == hwnd
                        && matches!(c.kind, pm_owner::OwnerCallKind::MeasureList { .. }));
                    if !measuring {
                        wm.queue_owner_call(pm_owner::OwnerCall {
                            control: hwnd, kind: pm_owner::OwnerCallKind::MeasureList { index },
                        });
                    }
                }
                self.repaint_control(hwnd);
                ApiResult::Normal(index)
            }

            LM_QUERYITEMTEXT | LM_QUERYITEMTEXTLENGTH | LM_SETITEMTEXT | LM_SETITEMHANDLE
            | LM_QUERYITEMHANDLE | LM_SEARCHSTRING | LM_SETTOPINDEX | LM_QUERYTOPINDEX
            | LM_SETITEMHEIGHT if class_name == pm_controls::CLASS_LISTBOX => {
                self.listbox_message(hwnd, msg, mp1, mp2)
            }

            EM_SETTEXTLIMIT if class_name == pm_controls::CLASS_ENTRYFIELD => {
                if let Some(w) = self.shared.window_mgr.lock_or_recover().get_window_mut(hwnd) {
                    w.text_limit = (mp1 & 0xFFFF) as usize;
                }
                ApiResult::Normal(1)
            }

            LM_QUERYITEMCOUNT => {
                let wm = self.shared.window_mgr.lock_or_recover();
                let count = wm.get_window(hwnd).map(|w| w.listbox_items.len()).unwrap_or(0);
                ApiResult::Normal(count as u32)
            }

            _ => ApiResult::Normal(0),
        }
    }

}


// ── Built-in control painting helpers ────────────────────────────────────────

/// Face colour of push buttons and dialogs.
const CONTROL_FACE_COLOR: u32 = 0x00D4_D0C8;
/// Face colour of a push button while it is held down.
const CONTROL_PRESSED_COLOR: u32 = 0x00B0_ACA4;
/// Outline of the default push button and the focus mark on a button.
const CONTROL_EMPHASIS_COLOR: u32 = 0x0000_0000;
/// Background of text-holding controls (entry field, MLE, list box).
const CONTROL_FIELD_COLOR: u32 = 0x00FF_FFFF;
const CONTROL_BORDER_COLOR: u32 = 0x0080_8080;
const CONTROL_TEXT_COLOR: u32 = 0x0000_0000;
/// The text of a disabled control: the grey of disabled menu items.
const CONTROL_DISABLED_TEXT_COLOR: u32 = 0x0080_8080;
/// Horizontal gap between a control's border and its text.
const CONTROL_TEXT_INSET: i32 = 4;
/// Height of one text row in a control (the system font's line height).
const CONTROL_LINE_H: i32 = font::SYSTEM_FONT_PX as i32;

/// Shaft of scroll bars.
const CONTROL_SCROLLBAR_COLOR: u32 = 0x00A0_A0A0;

/// Draws into one frame's surface (see [`Loader::draw`]).
pub(crate) struct Painter<'a> {
    pub loader: &'a Loader,
    pub frame: u32,
    /// The regional face of CJK text: that of the painted window's queue
    /// codepage, so a window whose queue is set to CP949 (`WinSetCp`) shows
    /// Hangul and hanja in the Korean face whatever the process codepage.
    pub cjk: CjkRegion,
    /// Text colour: the window's `PP_FOREGROUNDCOLOR` (inherited), else
    /// black (`pm_presparam.rs`).
    pub fg: u32,
    /// The window's `PP_BACKGROUNDCOLOR`, inherited.
    pub bg: Option<u32>,
    /// The window's own `PP_BACKGROUNDCOLOR` (not inherited): the field of
    /// entry fields, MLEs and list boxes.
    pub own_bg: Option<u32>,
    /// The face of the window's `PP_FONTNAMESIZE` (inherited); `None` = the
    /// system font.
    pub face: Option<FontSpec>,
    /// Draw only inside this region of the surface (`None`: everywhere).
    pub clip: Option<crate::gui::region::Region>,
}

impl<'a> Painter<'a> {
    /// A painter for window `hwnd`, drawing into `frame`'s surface.
    pub fn new(loader: &'a Loader, frame: u32, hwnd: u32) -> Self {
        let cjk = CjkRegion::from_codepage(loader.window_codepage(hwnd));
        let wm = loader.shared.window_mgr.lock_or_recover();
        Painter {
            loader, frame, cjk,
            fg: wm.pres_color(hwnd, pm_presparam::PP_FOREGROUNDCOLOR, false).unwrap_or(CONTROL_TEXT_COLOR),
            bg: wm.pres_color(hwnd, pm_presparam::PP_BACKGROUNDCOLOR, false),
            own_bg: wm.pres_color(hwnd, pm_presparam::PP_BACKGROUNDCOLOR, true),
            face: wm.pres_font(hwnd),
            clip: None,
        }
    }

    /// This painter drawing only inside the rectangle `(x, y, cx, cy)` (a
    /// control's own area, as PM clips a window's drawing to it).
    pub fn clipped_to(mut self, (x, y, cx, cy): (i32, i32, i32, i32)) -> Self {
        use crate::gui::region::{Region, RRect};
        self.clip = Some(Region::rect(RRect::new(x, y, x.saturating_add(cx), y.saturating_add(cy))));
        self
    }

    /// A copy of this painter drawing only inside `(x, y, cx, cy)` and its
    /// own clip (a column or cell of a control).
    pub fn clone_clipped(&self, (x, y, cx, cy): (i32, i32, i32, i32)) -> Painter<'a> {
        use crate::gui::region::{Region, RRect, SetOp};
        let cell = Region::rect(RRect::new(x, y, x.saturating_add(cx), y.saturating_add(cy)));
        let clip = match &self.clip {
            None => Some(cell),
            Some(c) => Some(Region::combine(c, &cell, SetOp::And).unwrap_or_else(Region::empty)),
        };
        Painter { loader: self.loader, frame: self.frame, cjk: self.cjk, fg: self.fg, bg: self.bg,
                  own_bg: self.own_bg, face: self.face, clip }
    }

    /// This painter writing text in the disabled grey.
    pub fn greyed(mut self) -> Self {
        self.fg = CONTROL_DISABLED_TEXT_COLOR;
        self
    }

    pub fn op(&self, op: DrawOp) {
        match &self.clip {
            None => self.loader.draw(self.frame, op),
            Some(clip) => self.loader.draw_clipped(pm_types::DrawTarget::Frame(self.frame), op, Some(clip)),
        }
    }

    /// The font control text is drawn in: the presentation-parameter face,
    /// else the system font, with this painter's CJK face.
    pub fn font(&self) -> FontSpec {
        FontSpec { cjk: self.cjk, ..self.face.unwrap_or(FontSpec::SYSTEM) }
    }

    /// One line of text in [`Painter::font`] whose line box has its bottom
    /// at `y` (the baseline is the font's descent above it).
    pub fn text(&self, x: i32, y: i32, text: &str, color: u32) {
        match self.face {
            None => self.op(DrawOp::Text { x, y, text: text.to_string(), color, cjk: self.cjk }),
            Some(_) => {
                let font = self.font();
                let y = y.saturating_add(font::metrics(font).descent);
                self.op(DrawOp::TextFont { x, y, text: text.to_string(), color, font, mix: Mix::Overpaint });
            }
        }
    }

    /// Overpaint the rectangle with corners `(x1, y1)`–`(x2, y2)` (inclusive).
    pub fn fill(&self, (x1, y1, x2, y2): (i32, i32, i32, i32), color: u32) {
        self.op(DrawOp::Box { x1, y1, x2, y2, color, fill: true, mix: Mix::Overpaint });
    }
}

impl Loader {
    /// Apply `op` to the surface of `frame` and mark the frame dirty.
    ///
    /// The drawing happens here, on the calling thread. The frame is marked
    /// dirty while its surface is locked, so a composite that picks up the
    /// dirty mark waits for the drawing to finish before reading the pixels.
    pub(crate) fn draw(&self, frame: u32, op: DrawOp) {
        self.draw_to(pm_types::DrawTarget::Frame(frame), op);
    }

    /// Apply `op` to the surface behind `target`: a frame (marked dirty as
    /// in [`Loader::draw`]) or a bitmap (not on screen, nothing to mark).
    pub(crate) fn draw_to(&self, target: pm_types::DrawTarget, op: DrawOp) {
        self.draw_clipped(target, op, None);
    }

    /// [`Loader::draw_to`] keeping only the pels inside `clip` (the clip
    /// of a presentation space; `None` draws unclipped).
    pub(crate) fn draw_clipped(&self, target: pm_types::DrawTarget, op: DrawOp,
                               clip: Option<&crate::gui::region::Region>) {
        use crate::gui::region::{RRect, Region, SetOp};
        use pm_types::DrawTarget;
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let frame = match target {
            DrawTarget::Frame(f) | DrawTarget::Client(f) | DrawTarget::Child { frame: f, .. } => Some(f),
            _ => None,
        };
        // WinLockWindowUpdate: nothing reaches a locked frame (pm_paint_style.rs).
        if let Some(frame) = frame && wm.is_update_locked(frame) {
            return;
        }
        let Some(surface) = wm.target_surface(target) else { return };
        let (origin, extent) = wm.target_view(target);
        let visible = wm.target_visible(target);
        let mut s = surface.lock_or_recover();
        if let Some(frame) = frame {
            wm.mark_dirty(frame);
        }
        drop(wm);
        // A client that lies elsewhere in its frame's surface (above a
        // horizontal frame scroll bar) or covers only part of it (beside a
        // vertical one): the drawing moves to the client's place and stays
        // inside the client.
        // A child window also stays inside its ancestors.
        let whole = extent.is_some_and(|(cx, cy)| origin == (0, 0) && (cx, cy) == (s.width as i32, s.height as i32));
        match visible {
            Some(v) if !whole || v != RRect::new(0, 0, s.width as i32, s.height as i32) => {
                let mut op = op;
                op.translate(origin.0, origin.1);
                let inside = Region::rect(v);
                let mut c = match clip {
                    Some(c) => Region::combine(c, &inside, SetOp::And).unwrap_or(inside),
                    None => inside,
                };
                c.offset(origin.0, origin.1);
                s.draw_clipped(op, Some(&c));
            }
            _ => s.draw_clipped(op, clip),
        }
    }
}

/// Fill a control's rectangle `(x, y, cx, cy)` and outline it.
fn control_box(p: &Painter, (x, y, cx, cy): (i32, i32, i32, i32), fill: u32) {
    for (color, fill) in [(fill, true), (CONTROL_BORDER_COLOR, false)] {
        p.op(DrawOp::Box { x1: x, y1: y, x2: x + cx, y2: y + cy, color, fill, mix: Mix::Overpaint });
    }
}

/// Gap between the bottom of a text line box and a mnemonic underline.
const MNEMONIC_UNDERLINE_Y: i32 = 2;

/// Draw a control label (line-box bottom-left at `(x, y)`) clipped to
/// `max_w` pixels: its `~` mnemonic marker is not shown and the character
/// it marks is underlined. Returns the width drawn.
pub(crate) fn control_label(p: &Painter, x: i32, y: i32, text: &str, max_w: i32) -> i32 {
    let (shown, mark) = pm_dialog::mnemonic_label(text);
    let label = font::fit_text(&shown, max_w, p.font());
    if label.is_empty() {
        return 0;
    }
    control_text(p, x, y, label);
    if let Some(mark) = mark
        && let Some((start, ch)) = label.char_indices().nth(mark)
    {
        let x1 = x + font::text_width(&label[..start], p.font());
        let w = font::text_width(&label[start..start + ch.len_utf8()], p.font());
        let uy = y + MNEMONIC_UNDERLINE_Y;
        p.fill((x1, uy, x1 + w - 1, uy), p.fg);
    }
    font::text_width(label, p.font())
}

/// Draw one line of system-font text whose line box has its bottom at `y`.
fn control_text(p: &Painter, x: i32, y: i32, text: &str) {
    p.text(x, y, text, p.fg);
}
