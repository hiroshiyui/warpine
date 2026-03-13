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

- [ ] **Guest memory bounds checking**
    - All 50+ guest memory accesses use raw `guest_mem.add(offset)` without validating that `offset` falls within the 128MB allocation. A malicious OS/2 binary can supply out-of-bounds pointers via API arguments (DosRead buffer, WinQueryWindowRect rect, GpiCharStringAt string, stack reads via corrupted RSP) to read/write arbitrary host memory.
    - **Fix:** Create a `guest_ptr_checked(addr: u32, len: usize) -> Option<*mut u8>` helper that validates `addr as usize + len <= GUEST_MEM_SIZE` and route all guest memory access through it. Affected: `read_stack`, `read_guest_string`, every API handler that reads/writes guest pointers.
    - **Files:** `src/loader.rs` (50+ sites), `src/api.rs` (`DosWrite` `from_raw_parts` on unvalidated pointer)

- [ ] **Filesystem sandbox (path traversal)**
    - `translate_path()` replaces backslashes and strips drive letters but does not prevent `..` traversal or absolute paths. A guest binary can access any host file: `C:\..\..\etc\passwd` → `/etc/passwd`. `DosOpen`, `DosDelete`, `DosMove`, `DosCreateDir`, `DosDeleteDir` are all affected.
    - **Fix:** Implement a chroot-like sandbox: prepend a configurable base directory, canonicalize the result, and reject paths that escape the base. Consider a `--rootfs <dir>` CLI option.
    - **Files:** `src/loader.rs` `translate_path()`, `dos_open`, `dos_delete`, `dos_move`, `dos_create_dir`, `dos_delete_dir`

- [ ] **`read_guest_string` unbounded read**
    - Reads bytes from guest memory until a null terminator with no maximum length and no bounds check against guest memory size. Can read into host memory if the guest supplies a pointer near the end of the allocation or a string without a null terminator.
    - **Fix:** Add a `max_len` parameter (e.g., 4096) and bounds-check against `GUEST_MEM_SIZE`.
    - **Files:** `src/loader.rs` `read_guest_string()`

- [ ] **`mmap` return value unchecked**
    - `libc::mmap` can return `MAP_FAILED` (usize::MAX cast to pointer). The code proceeds to `write_bytes` on the returned pointer without checking, causing immediate SIGSEGV.
    - **Fix:** Check `guest_mem == libc::MAP_FAILED` and return an error.
    - **Files:** `src/loader.rs` `Loader::new()` mmap call

### P1 — High (correctness / stability)

- [ ] **Mutex lock ordering — deadlock prevention**
    - `SharedState` has 6 mutexes (`mem_mgr`, `handle_mgr`, `sem_mgr`, `queue_mgr`, `window_mgr`, `threads`) with no defined acquisition order. Several API handlers acquire multiple locks (e.g., `dos_read_queue` holds `queue_mgr` then acquires `mem_mgr`; `WinPostMsg` holds `window_mgr` then locks inner message queue). If two threads acquire them in different orders, deadlock occurs.
    - **Fix:** Define and document a strict lock ordering (e.g., mem_mgr < handle_mgr < sem_mgr < queue_mgr < window_mgr < threads). Audit all call sites. Consider using `parking_lot` mutexes which detect deadlocks in debug mode.
    - **Files:** `src/loader.rs` — all multi-lock API handlers

- [ ] **Semaphore/mutex wait timeouts ignored**
    - `dos_wait_event_sem`, `dos_request_mutex_sem`, and `dos_wait_mux_wait_sem` accept a timeout parameter but block indefinitely via `Condvar::wait()`, ignoring it completely. Guest apps that rely on timeouts will hang forever.
    - **Fix:** Use `Condvar::wait_timeout()` with the guest-supplied millisecond value. Handle `SEM_TIMEOUT` (0xEE = 238) return code for OS/2.
    - **Files:** `src/loader.rs` — semaphore wait handlers

- [ ] **Integer overflow in `MemoryManager::alloc`**
    - `self.next_free + size` can overflow `u32` before the limit comparison, making the check pass and returning an overlapping/wrapped allocation.
    - **Fix:** Use `u32::checked_add()` and return error on overflow.
    - **Files:** `src/loader.rs` `MemoryManager::alloc()`

- [ ] **`process::exit()` in thread context**
    - `DosExit` and the VMEXIT loop call `std::process::exit()` which terminates all threads immediately, skipping destructors. This can corrupt state, leak KVM resources, and leave the terminal in a bad state.
    - **Fix:** Signal clean shutdown via an `AtomicBool` or channel, let threads wind down gracefully.
    - **Files:** `src/loader.rs` `dos_exit`, `run_vcpu`

- [ ] **`WinGetMsg` spin-wait polling**
    - Busy-waits with `thread::sleep(10ms)` in a loop, repeatedly acquiring and releasing the `window_mgr` lock. Wastes CPU and adds up to 10ms latency per message. Same pattern in `dos_wait_mux_wait_sem` and `dos_read_queue`.
    - **Fix:** Use `Condvar` on the message queue and `notify_one()` when a message is posted. The `PM_MsgQueue` struct already has a `cond: Condvar` field that is unused.
    - **Files:** `src/loader.rs` ordinal 915 (WinGetMsg), `dos_wait_mux_wait_sem`, `dos_read_queue`

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
