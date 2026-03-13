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
    - At 2337 lines, `loader.rs` handles KVM setup, memory management, handle tables, semaphores, queues, PM window management, PMGPI drawing, filesystem APIs, and the VMEXIT loop. This makes review, testing, and modification difficult.
    - **Suggested split:** `kvm.rs` (VMM core, VMEXIT loop), `guest_mem.rs` (bounds-checked memory wrapper), `doscalls.rs` (filesystem, memory, thread APIs), `pm_win.rs` (PMWIN handlers), `pm_gpi.rs` (PMGPI handlers), `ipc.rs` (semaphores, pipes, queues).

- [ ] **Bump-only memory allocator**
    - `MemoryManager::free()` removes the `AllocBlock` tracking entry but never reclaims the space — `next_free` only grows. Repeated alloc/free cycles exhaust the guest address space.
    - **Fix:** Implement a free-list allocator, or at minimum coalesce freed blocks at the top of the allocation range.
    - **Files:** `src/loader.rs` `MemoryManager`

- [ ] **Magic numbers → named constants**
    - WM_ message constants used as hex literals in `gui.rs` (0x0029, 0x0007, 0x0023, 0x007A, 0x0070, 0x0071, 0x0072) instead of named constants already defined in `loader.rs`. TIB/PIB addresses (0x70000, 0x71000, 0x60000) hardcoded in multiple places without constants. Mock handles (0x1234 for HAB, 0x5000 for HPOINTER) are unexplained literals.
    - **Fix:** Define shared constants in a `constants.rs` module and use them everywhere.
    - **Files:** `src/gui.rs`, `src/loader.rs`

- [ ] **Buffer allocation integer overflow in GUI**
    - `vec![0xFFFFFFFF; (size.width * size.height) as usize]` — `u32 * u32` can wrap to 0 for large window sizes, creating a tiny buffer. All subsequent rendering would then index out of bounds (currently guarded by bounds checks, but the buffer would be wrong).
    - **Fix:** Use `(width as usize).checked_mul(height as usize)` and handle overflow. Add early return for zero dimensions in `render_rect_to_buffer`.
    - **Files:** `src/gui.rs` `CreateWindow` handler, `Resized` handler, `render_rect_to_buffer`

- [ ] **LX parser hardening for malformed inputs**
    - `object_count` and `module_num_pages` from the LX header are used directly for `Vec::with_capacity` and loop counts without upper-bound validation. A crafted LX file could trigger multi-gigabyte allocations. `page_offset_shift` is not validated (values >= 32 cause undefined behavior in shift operations). Fixup parsing can read past declared page boundaries. `eip_object`/`esp_object` are not validated against `object_table` bounds.
    - **Fix:** Validate header fields against file size. Add `assert!(page_offset_shift < 32)`. Bounds-check fixup reads. Validate entry point object indices.
    - **Files:** `src/lx/lx.rs`, `src/lx/header.rs`

- [ ] **Dead code cleanup**
    - `api.rs` bridge functions (`DosWrite`, `DosExit`, `DosQuerySysInfo` etc.) are `extern "C"` FFI stubs from an earlier architecture — never called in the current KVM-based execution model. `_saved_rax` in `CallbackFrame` is stored but never read. `wm.ps_map.get(&hps)` in WinEndPaint reads and discards a value. `dos_find_first` has duplicate `if/else` branches that do the same thing.
    - **Files:** `src/api.rs`, `src/loader.rs`

### P3 — Low (polish)

- [ ] **Replace `unwrap()` on mutex locks**
    - 50+ `.lock().unwrap()` calls across the codebase. If any thread panics while holding a lock (poisoned mutex), the entire process panics. Use `.lock().unwrap_or_else(|e| e.into_inner())` or a wrapper function.
    - **Files:** `src/loader.rs`, `src/gui.rs`

- [ ] **Deduplicate frame-to-client lookup**
    - The pattern of finding a frame hwnd from a client hwnd via `frame_to_client` is copy-pasted at 7 locations in `handle_pmwin_call` and `handle_pmgpi_call`. Should be a method on `WindowManager`.
    - **Files:** `src/loader.rs`

- [ ] **Replace `println!` with structured logging**
    - All 30+ diagnostic messages use `println!`. Should use the `log` or `tracing` crate with configurable levels (debug for API calls, info for lifecycle events, warn for stubs).
    - **Files:** All source files

- [ ] **Timer thread leak**
    - `WinStartTimer` spawns threads that loop until an `AtomicBool` is set. `JoinHandle` is dropped without joining. If the guest creates/destroys timers repeatedly, threads accumulate. On app exit without `WinStopTimer`, threads run until `process::exit`.
    - **Files:** `src/loader.rs` ordinals 884/885

- [ ] **Cargo.toml improvements**
    - No `[profile.release]` section (add `lto = true`, `strip = true`, `codegen-units = 1`). No `rust-toolchain.toml` (edition 2024 requires Rust 1.85+). Exact version pins prevent `cargo update` from pulling patches. Consider optional `gui` feature flag for winit/softbuffer.
    - **Files:** `Cargo.toml`

- [ ] **`dos_create_thread` creates unnecessary KVM instance**
    - Each new thread creates `Kvm::new()` (a new `/dev/kvm` fd) that is immediately discarded. All threads share the same VM — the extra fd wastes resources and is confusing.
    - **Files:** `src/loader.rs` `dos_create_thread`

## General Improvements
- [x] Add unit tests for LX parser and GUI rendering.
- [ ] Improve error handling and logging (possibly using `log` or `tracing` crates).
- [x] Create a sample 32-bit OS/2 "Hello World" binary for testing.
- [x] Pivot to Unicorn Engine for platform-agnostic 32-bit emulation.
