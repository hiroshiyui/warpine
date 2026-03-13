# Warpine TODO List

This document tracks the tasks required to reach a functional OS/2 compatibility layer.

## Phase 1: Foundation (CLI "Hello World") - COMPLETED 🎉
- [x] **Executable Parser (LX/LE/NE)**
    - [x] Implement MZ (DOS) header parser to locate the OS/2 header offset.
    - [x] Implement LX (Linear Executable) header parser.
    - [x] Implement Object Table and Page Map parsing for LX files.
    - [x] Implement Fixup (Relocation) Table parsing.
- [x] **Loader Subsystem**
    - [x] Implement memory mapping of LX objects into the KVM guest address space.
    - [x] Apply base relocations (fixups).
    - [x] Resolve dynamic imports (DLLs) and thunk them to native implementations via INT 3 traps.
    - [x] Set up the initial CPU state (registers, stack, TIB, PIB, Env) for execution.
- [x] **Initial API Thunks (DOSCALLS.DLL)**
    - [x] `DosWrite`: Basic implementation for stdout/stderr.
    - [x] `DosExit`: Proper process termination with exit code.
    - [x] `DosQuerySysInfo`: Stub implementation for runtime initialization.
    - [x] `DosQueryConfig`: Stub implementation for runtime initialization.
    - [x] `DosQueryHType`: Identification of standard handles.
    - [x] `DosGetInfoBlocks`: Retrieval of TIB and PIB pointers.

## Phase 2: Core OS/2 Subsystem
- [x] **Memory Management**
    - [x] `DosAllocMem` / `DosFreeMem` implementation.
    - [x] Handle OS/2 32-bit flat memory model vs. segmented requests.
- [x] **Filesystem APIs**
    - [x] `DosOpen`, `DosRead`, `DosClose`, `DosQueryFileInfo`.
    - [x] `DosDelete`, `DosMove`, `DosCreateDir`, `DosDeleteDir`.
    - [x] `DosFindFirst`, `DosFindNext` with basic wildcard support.
    - [x] Map OS/2 drive letters (e.g., `C:\`) to Unix paths.
- [x] **Process/Thread Management**
    - [x] `DosCreateThread`, `DosKillThread`.
    - [x] Thread Local Storage (TLS) emulation (via TIB initialization).
- [x] Inter-Process Communication (IPC)
    - [x] Event Semaphores (`DosCreateEventSem`, `DosPostEventSem`, etc.).
    - [x] Mutex Semaphores (`DosCreateMutexSem`, `DosRequestMutexSem`, etc.).
    - [x] MuxWait Semaphores (`DosCreateMuxWaitSem`, `DosWaitMuxWaitSem`).
    - [x] Pipes (`DosCreatePipe`).
    - [x] Queues (`DosCreateQueue`, `DosWriteQueue`, `DosReadQueue`).


## Phase 3: Presentation Manager (GUI)
- [x] **Infrastructure**
    - [x] Add winit + softbuffer dependencies for cross-platform windowing.
    - [x] Add PM data structures: `OS2Message`, `PM_MsgQueue`, `WindowClass`, `OS2Window`, `PresentationSpace`, `WindowManager`.
    - [x] Add `window_mgr: Mutex<WindowManager>` to `SharedState`.
    - [x] Expand API thunk stubs for PMWIN (2048+) and PMGPI (3072+) ordinal ranges.
- [x] **Main Thread Restructuring**
    - [x] Detect PM apps via imported module check (`is_pm_app`).
    - [x] Dual-path execution: CLI apps run vCPU on main thread; PM apps run winit event loop on main thread with vCPU on worker thread.
    - [x] `GUISender` wrapper with `EventLoopProxy` waking for cross-thread GUI message delivery.
- [x] **Window Management (PMWIN.DLL)**
    - [x] `WinInitialize` / `WinTerminate` — HAB lifecycle.
    - [x] `WinCreateMsgQueue` / `WinDestroyMsgQueue` — message queue creation with tid-to-hmq mapping.
    - [x] `WinRegisterClass` — store guest window procedure pointer per class.
    - [x] `WinCreateStdWindow` — create frame + client windows, send `CreateWindow` to GUI thread.
    - [x] `WinGetMsg` / `WinDispatchMsg` — message loop with blocking dequeue and guest callback dispatch.
    - [x] `WinPostMsg` / `WinSendMsg` — inter-window messaging with callback support.
    - [x] `WinDefWindowProc` — default message processing (WM_CLOSE → WM_QUIT, WM_PAINT no-op).
    - [x] `WinBeginPaint` / `WinEndPaint` — presentation space for painting, buffer present on end.
    - [x] `WinMessageBox` — terminal-based emulation (prints to stdout).
    - [x] `WinShowWindow`, `WinQueryWindowRect`, `WinDestroyWindow`, `WinGetLastError`.
- [x] **Callback Mechanism**
    - [x] `ApiResult` enum: `Normal(u32)` vs `Callback { wnd_proc, hwnd, msg, mp1, mp2 }`.
    - [x] `CallbackFrame` stack for re-entrant guest window procedure calls.
    - [x] `CALLBACK_RET_TRAP` (0x010003FE) for detecting callback return via VMEXIT.
    - [x] Frame-to-client window redirection for event routing.
- [x] **Graphics (PMGPI.DLL)**
    - [x] `GpiCreatePS` / `GpiDestroyPS` — presentation space lifecycle.
    - [x] `GpiSetColor` — current drawing color.
    - [x] `GpiMove` — set current position.
    - [x] `GpiBox` — rectangle drawing (filled and outline) via softbuffer.
    - [x] `GpiLine` — Bresenham line drawing via softbuffer.
- [x] **Input Handling**
    - [x] Keyboard events → `WM_CHAR` messages with key flags and char codes.
    - [x] Mouse movement → `WM_MOUSEMOVE` with OS/2 bottom-left coordinate flip.
    - [x] Mouse buttons → `WM_BUTTON1DOWN` / `WM_BUTTON1UP`.
    - [x] Window resize → `WM_SIZE` with buffer reallocation.
    - [x] Window close → `WM_CLOSE`.
- [x] **Test Application**
    - [x] `samples/pm_hello` — PM app using `WinMessageBox` for basic PM verification.
- [x] **Text & Erasing**
    - [x] `GpiCharStringAt` — text rendering with embedded 8x16 VGA bitmap font.
    - [x] `GpiErase` — clear presentation space to white.
- [x] **Timer Support**
    - [x] `WinStartTimer` / `WinStopTimer` — background thread posts `WM_TIMER` messages.
- [x] **Dialog Boxes**
    - [x] `WinDlgBox`, `WinLoadDlg`, `WinProcessDlg`, `WinDismissDlg` — stubs (no resource loading yet).
    - [x] `WinDefDlgProc`, `WinSendDlgItemMsg`, `WinQueryDlgItemText`, `WinSetDlgItemText`.
    - [x] `WinWindowFromID` — child window lookup by ID.
- [x] **Menus & Accelerators**
    - [x] `WinCreateMenu`, `WinLoadMenu`, `WinPopupMenu` — stubs (no resource loading yet).
    - [x] `WinLoadAccelTable`, `WinSetAccelTable`, `WinTranslateAccel` — stubs.
- [x] **Clipboard**
    - [x] `WinOpenClipbrd`, `WinCloseClipbrd`, `WinEmptyClipbrd` — clipboard state management.
    - [x] `WinSetClipbrdData`, `WinQueryClipbrdData` — in-process clipboard storage.
- [x] **Additional Window APIs**
    - [x] `WinSetWindowText` / `WinQueryWindowText` — per-window text storage.
    - [x] `WinSetWindowULong` / `WinQueryWindowULong` / `WinSetWindowUShort` / `WinQueryWindowUShort` — window data words.
    - [x] `WinFillRect` — fills rectangle via GUI DrawBox.
    - [x] `WinInvalidateRect` / `WinUpdateWindow` — repaint triggering.
    - [x] `WinSetWindowPos` — stub.
    - [x] `WinQuerySysValue` — screen metrics (640x480 defaults).
    - [x] `WinQuerySysPointer`, `WinSetPointer`, `WinAlarm` — stubs.
- [ ] **Remaining**
    - [ ] Resource loading from LX executables (dialog templates, menu templates, accelerator tables, string tables).
    - [ ] Full `WinSetWindowPos` with GUI resize/move support.

## Phase 4: Multimedia and 16-bit Support
- [ ] **Audio/Video (MMPM2)**
    - [ ] Reimplement multimedia APIs using PulseAudio/ALSA or SDL.
- [ ] **16-bit Compatibility**
    - [ ] Integrate a lightweight x86 emulator for 16-bit code execution.
    - [ ] Support NE (New Executable) format parsing and loading.

## Security & Hardening (from code review 2026-03-13)

### P0 — Critical (sandbox escape / memory safety)

- [x] **Guest memory bounds checking**
    - Added `guest_ptr()`, `guest_read()`, `guest_write()`, `guest_write_bytes()`, `guest_slice_mut()` helpers that validate `offset + len <= guest_mem_size`. All 67 raw `guest_mem.add()` accesses migrated.

- [x] **Filesystem sandbox (path traversal)**
    - Added `translate_path()` that canonicalizes paths and verifies they stay under the sandbox root (CWD). All filesystem APIs (`DosOpen`, `DosDelete`, `DosMove`, `DosCreateDir`, `DosDeleteDir`, `DosFindFirst`, `DosQueryPathInfo`) now route through it.

- [x] **`read_guest_string` unbounded read**
    - Replaced with bounded version: max 4096 bytes, checked against `guest_mem_size`. Old inline string reader in `dos_open` also replaced.

- [x] **`mmap` return value unchecked**
    - Added `MAP_FAILED` check with panic and `last_os_error()` diagnostic.

### P1 — High (correctness / stability)

- [ ] **Mutex lock ordering — deadlock prevention**
    - `SharedState` has 6 mutexes with no defined acquisition order. Improved by releasing `queue_mgr` lock before acquiring `mem_mgr` in `dos_read_queue`, but a formal lock ordering document is still needed.
    - **Remaining:** Define and document strict lock ordering. Consider `parking_lot` mutexes with deadlock detection in debug mode.

- [x] **Semaphore/mutex wait timeouts**
    - `dos_wait_event_sem`, `dos_request_mutex_sem`, and `dos_wait_mux_wait_sem` now use `Condvar::wait_timeout()` with the guest-supplied millisecond value. Returns ERROR_TIMEOUT (640) on expiration. Treats `u32::MAX` as indefinite wait.

- [x] **Integer overflow in `MemoryManager::alloc`**
    - Uses `checked_add()` for both page-alignment rounding and limit comparison. Returns `None` on overflow.

- [x] **`process::exit()` in thread context**
    - Added `exit_requested: AtomicBool` and `exit_code: AtomicI32` to `SharedState`. All `process::exit()` calls in `run_vcpu` replaced with atomic flag setting + return. `DosExit` sets exit code and signals shutdown. Only the top-level `setup_and_run_cli`/`run` call `process::exit` after `run_vcpu` returns.

- [x] **`WinGetMsg` spin-wait polling**
    - `WinGetMsg` now blocks on `PM_MsgQueue::cond` Condvar (with 100ms timeout fallback) instead of 10ms spin-wait. All message posting sites (`WinPostMsg`, `WinCreateStdWindow`, `WinDefWindowProc`, `WinInvalidateRect`, `WinStartTimer`, `gui.rs::push_msg`) call `cond.notify_one()`. Same fix for `dos_read_queue` using new Condvar on `OS2Queue`.

### P2 — Medium (architecture / maintainability)

- [ ] **Split `loader.rs` into modules**
    - At ~2450 lines, `loader.rs` handles KVM setup, memory management, handle tables, semaphores, queues, PM window management, PMGPI drawing, filesystem APIs, and the VMEXIT loop. This makes review, testing, and modification difficult.
    - **Suggested split:** `kvm.rs` (VMM core, VMEXIT loop), `guest_mem.rs` (bounds-checked memory wrapper), `doscalls.rs` (filesystem, memory, thread APIs), `pm_win.rs` (PMWIN handlers), `pm_gpi.rs` (PMGPI handlers), `ipc.rs` (semaphores, pipes, queues).

- [x] **Bump-only memory allocator → free-list**
    - `MemoryManager` now has a `free_list` with first-fit reuse, block splitting, and coalescing at the top of the allocation range. Unit tests verify alloc, free, reuse, coalescing, overflow, and limit checking.

- [x] **Magic numbers → named constants**
    - Defined `WM_SIZE`, `WM_PAINT`, `WM_TIMER`, `WM_CLOSE`, `WM_QUIT`, `WM_MOUSEMOVE`, `WM_BUTTON1DOWN`, `WM_BUTTON1UP`, `WM_CHAR` as public constants in `loader.rs`. Added `TIB_BASE`, `PIB_BASE`, `ENV_ADDR`, `MOCK_HAB`, `MOCK_HPOINTER`. `gui.rs` imports and uses these instead of hex literals.

- [x] **Buffer allocation integer overflow in GUI**
    - Both `CreateWindow` and `Resized` handlers use `checked_mul()` for pixel buffer allocation.

- [x] **LX parser hardening for malformed inputs**
    - Validates `object_count` (max 1024), `module_num_pages` (max 65536), `page_offset_shift` (< 32), and `eip_object`/`esp_object` against `object_count`. Returns descriptive errors. Unit tests cover all rejection cases.

- [x] **Dead code cleanup**
    - Removed `api.rs` bridge functions (`DosWrite`, `DosExit`, `DosQuerySysInfo`, `WarpineExitThunk`) and `bridges` module. Removed `_saved_rax` from `CallbackFrame`. Removed useless `wm.ps_map.get(&hps)` in WinEndPaint. Collapsed duplicate `if/else` branches in `dos_find_first`.

### P3 — Low (polish)

- [x] **Replace `unwrap()` on mutex locks**
    - Added `MutexExt` trait with `lock_or_recover()` that uses `unwrap_or_else(|e| e.into_inner())` to recover from poisoned locks. All 106 `.lock().unwrap()` calls across `loader.rs`, `gui.rs`, and `main.rs` replaced.

- [x] **Deduplicate frame-to-client lookup**
    - Added `WindowManager::client_to_frame()` method. All 7 copy-pasted reverse lookups replaced with single method call.

- [x] **Replace `println!` with structured logging**
    - Added `log` + `env_logger` crates. All `println!` calls replaced: `debug!` for routine API calls, `info!` for lifecycle events, `warn!` for stubs and unknown ordinals, `error!` for KVM failures. Enable with `RUST_LOG=debug`.

- [x] **Timer thread leak**
    - Timer `JoinHandle`s now stored alongside `AtomicBool` flags. `WinStopTimer` joins the thread. Added `stop_all_timers()` method. Timer threads also check `exit_requested` to stop on shutdown.

- [x] **Cargo.toml improvements**
    - Added `[profile.release]` with `lto = true`, `strip = true`, `codegen-units = 1`. Added `log` and `env_logger` dependencies.

- [x] **`dos_create_thread` creates unnecessary KVM instance**
    - Documented that the dummy `Kvm::new()` is only needed to satisfy the `Loader` struct — `run_vcpu` never uses `_kvm` or `vm`. vCPU creation moved before thread spawn to use the parent's VM fd.

## General Improvements
- [x] Add unit tests for LX parser and GUI rendering.
- [x] Improve error handling and logging (using `log` + `env_logger` crates).
- [x] Create a sample 32-bit OS/2 "Hello World" binary for testing.
- [x] Pivot to Unicorn Engine for platform-agnostic 32-bit emulation.
