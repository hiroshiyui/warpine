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
- [x] **Resource Loading**
    - [x] LX resource table parsing (`LxResourceEntry`, `ResourceManager`).
    - [x] `DosGetResource`, `DosFreeResource`, `DosQueryResourceSize`.
    - [x] `WinLoadString` — string table bundle parsing.
    - [x] `WinLoadMenu` — creates menu window from resource (template parsing deferred).
    - [x] `WinLoadAccelTable` / `WinSetAccelTable` / `WinTranslateAccel` — accelerator table loading and key translation.
    - [x] `WinLoadDlg` / `WinDlgBox` — improved stubs with resource ID logging (dialog template parsing deferred).
- [x] **Additional**
    - [x] Full `WinSetWindowPos` with GUI resize/move support.

## Phase 3.5: Text-Mode Application Support (4OS2 Compatibility Target)

Target application: [4OS2](https://github.com/StevenLevine/4os2) — a commercial-grade OS/2 command shell that exercises nearly every DOSCALLS surface plus the full Kbd/Vio console subsystem. Getting 4OS2 to run validates CLI compatibility broadly.

**Recommended implementation order:** Infrastructure → Subsystem 3 stubs (unblock init) → Subsystem 2 directory/sysinfo → Subsystem 1 console I/O → Subsystem 2 process execution → remaining APIs.

### Infrastructure: Expand Thunk Stub Area — COMPLETED
- [x] Add `KBDCALLS_BASE = 4096` and `VIOCALLS_BASE = 5120` constants to `constants.rs`
- [x] Add `STUB_AREA_SIZE = 8192` constant (expanded from implicit 4096)
- [x] Update `setup_stubs()` loop to `0..STUB_AREA_SIZE`
- [x] Update `run_vcpu()` bounds check from `MAGIC_API_BASE + 4096` to `MAGIC_API_BASE + STUB_AREA_SIZE`
- [x] Add `"KBDCALLS"` and `"VIOCALLS"` branches in `resolve_import()`
- [x] Add KBDCALLS and VIOCALLS dispatch branches in `handle_api_call_ex()`

### Subsystem 1: Console I/O (KBDCALLS.DLL + VIOCALLS.DLL)

New modules: `src/loader/console.rs` (VioManager), `src/loader/kbdcalls.rs`, `src/loader/viocalls.rs`.

Implementation approach: map Kbd calls to Linux termios (raw mode input) and Vio calls to ANSI escape sequences for terminal output. `VioManager` maintains a screen buffer (char+attr cells), cursor position, screen dimensions, and ANSI mode flag.

- [ ] **Console Manager (`console.rs`)**
    - [ ] `VioManager` struct: screen buffer `Vec<(u8, u8)>`, cursor (row, col), dimensions (default 25×80), ANSI mode, cursor type, codepage
    - [ ] Terminal raw mode RAII guard (`RawModeGuard` using termios) with proper cleanup on drop/panic
    - [ ] ANSI escape sequence helpers for cursor movement, scrolling, color mapping
    - [ ] Add `console_mgr: Mutex<VioManager>` to `SharedState`
- [ ] **Minimal VIO — Output (Step 1, required for any visible output)**
    - [ ] `VioWrtTTY` (ordinal 30) — write string to stdout with ANSI cursor positioning; update screen buffer
    - [ ] `VioGetMode` (ordinal 3) — return screen dimensions (rows/cols)
    - [ ] `VioGetCurPos` (ordinal 4) — return current cursor row/col
    - [ ] `VioSetCurPos` (ordinal 15) — move cursor via ANSI `\033[row;colH`; update VioManager state
- [ ] **Minimal KBD — Input (Step 2, required for interactive use)**
    - [ ] `KbdCharIn` (ordinal 4) — blocking read from stdin in raw mode; map Linux keycodes to OS/2 `KBDKEYINFO` struct (charcode, scancode, status, shift state); check `shutting_down()` during wait
    - [ ] `KbdGetStatus` (ordinal 10) — return keyboard status flags from VioManager
- [ ] **Screen Manipulation (Step 3, needed for 4OS2 line editor)**
    - [ ] `VioScrollUp` (ordinal 7) / `VioScrollDn` (ordinal 8) — scroll region via ANSI scroll commands; update screen buffer
    - [ ] `VioWrtCharStrAtt` (ordinal 26) — write attributed string using ANSI color codes; update screen buffer
    - [ ] `VioWrtNCell` (ordinal 28) — fill N cells (char+attr); update screen buffer
    - [ ] `VioWrtNAttr` (ordinal 27) — fill N attribute bytes; update screen buffer
    - [ ] `VioReadCellStr` (ordinal 24) — read char+attr from internal screen buffer
    - [ ] `VioSetCurType` (ordinal 16) — cursor shape via ANSI `\033[?25h/l`
- [ ] **Stubs and Configuration (Step 4)**
    - [ ] `KbdStringIn` (ordinal 9) — read string; can build on repeated `KbdCharIn`
    - [ ] `VioSetAnsi` (ordinal 38) / `VioGetAnsi` (ordinal 39) — ANSI mode flag management
    - [ ] `VioSetState` (ordinal 51) — stub, return 0
    - [ ] `VioSetCp` (ordinal 42) — stub, return 0
    - [ ] `VioGetConfig` (ordinal 46) — return VGA adapter defaults

### Subsystem 2: Process Management

New module: `src/loader/process.rs`. Add `ProcessManager` to `SharedState` (or to `managers.rs`).

`ProcessManager` fields: `children: HashMap<u32, std::process::Child>`, `next_pid: u32`, `current_disk: u8` (default 3 = C:), `current_dir: String` (OS/2 current directory, tracked separately from host cwd for sandbox safety).

- [ ] **Directory Management (Step 1, needed for shell prompt)**
    - [ ] `DosSetCurrentDir` (ordinal 255) — store path in ProcessManager; translate and validate via `translate_path()`
    - [ ] `DosQueryCurrentDir` (ordinal 274) — read from ProcessManager, write to guest buffer (OS/2 returns path without drive letter)
    - [ ] `DosQueryCurrentDisk` (ordinal 275) — return `current_disk`
    - [ ] `DosSetDefaultDisk` (ordinal 220) — set `current_disk`
- [ ] **System Information (Step 2, needed for 4OS2 init)**
    - [ ] `DosQuerySysInfo` (ordinal 348) — real implementation: return max path length (260), OS version major/minor (20/45 = Warp 4.5), boot drive (3=C:), max text sessions, page size, etc. Query range `iStart..iLast` indexes into QSV_* table.
    - [ ] `DosGetDateTime` (ordinal 230) — fill OS/2 `DATETIME` struct from system clock (hours, minutes, seconds, hundredths, day, month, year, timezone, weekday)
    - [ ] `DosSetDateTime` (ordinal 231) — stub, return 0 (setting system time not meaningful)
- [ ] **Process Execution (Step 3, core shell functionality)**
    - [ ] `DosExecPgm` (ordinal 283) — for sync (execFlag=0): spawn `warpine <child.exe>` via `std::process::Command`, wait, return exit code in `RESULTCODES` struct at `pRes`; for async (execFlag=1,2): spawn and track in ProcessManager; parse double-null-terminated `pArg` string
    - [ ] `DosWaitChild` (ordinal 280) — `child.wait()` or `child.try_wait()`; write `RESULTCODES`
    - [ ] `DosKillProcess` (ordinal 237) — `child.kill()`
    - [ ] `DosQueryAppType` (ordinal 323) — parse target LX header flags or return `FAPPTYP_NOTWINDOWCOMPAT` (1) for .exe files

### Subsystem 3: Shared Memory, Exception Handling, and Init Stubs

New module: `src/loader/stubs.rs` for simple stub handlers. Add `SharedMemManager` and exception handler storage.

- [ ] **Critical Init Stubs (Step 1, required for 4OS2 to start)**
    - [ ] `DosError` (ordinal 212) — stub, return 0
    - [ ] `DosSetMaxFH` (ordinal 291) — stub, return 0
    - [ ] `DosBeep` (ordinal 286) — print BEL character `\x07` to stdout
    - [ ] `DosSetExceptionHandler` (ordinal 354) — store guest exception handler address (minimal implementation)
    - [ ] `DosUnsetExceptionHandler` (ordinal 355) — remove stored handler
    - [ ] `DosSetSignalExceptionFocus` (ordinal 356) — stub, return 0
    - [ ] `DosAcknowledgeSignalException` (ordinal 418) — stub, return 0
    - [ ] `DosEnterMustComplete` (ordinal 380) / `DosExitMustComplete` (ordinal 381) — stub, return 0
- [ ] **Shared Memory (Step 2, 4OS2 init `_exit()`s on failure)**
    - [ ] `DosAllocSharedMem` (ordinal 300) — delegate to existing `MemoryManager::alloc()`; if named, register name→address in `SharedMemManager`
    - [ ] `DosGetNamedSharedMem` (ordinal 301) — look up name in `SharedMemManager`, return address or `ERROR_FILE_NOT_FOUND`
    - [ ] `DosGetSharedMem` (ordinal 302) — stub, return 0 (all memory already accessible in flat model)
    - [ ] `DosSetMem` (ordinal 305) — stub, return 0 (all memory already committed)
    - [ ] `DosQueryMem` (ordinal 306) — return basic info: `PAG_COMMIT | PAG_READ | PAG_WRITE`
- [ ] **Codepage and Country Info (Step 3)**
    - [ ] `DosQueryCp` (ordinal 291) — return codepage 437
    - [ ] `DosSetProcessCp` (ordinal 289) — stub, return 0
    - [ ] `DosQueryCtryInfo` (ordinal 397) — return US defaults (country=1, codepage=437, date format=0 MDY, currency='$', thousands=',', decimal='.')
    - [ ] `DosMapCase` (ordinal 305... verify ordinal) — case mapping; ASCII toupper for now
- [ ] **Module Loading Stubs (Step 4, 4OS2 tries to load PM DLLs)**
    - [ ] `DosLoadModule` (ordinal 318) — return `ERROR_MOD_NOT_FOUND` (126); log requested module name
    - [ ] `DosFreeModule` (ordinal 322) — stub, return 0
    - [ ] `DosQueryModuleHandle` (ordinal 319) — return `ERROR_MOD_NOT_FOUND`
    - [ ] `DosQueryProcAddr` (ordinal 321) — return `ERROR_PROC_NOT_FOUND` (127)
    - [ ] `DosGetMessage` (ordinal 317) — stub, return error
- [ ] **File Metadata APIs (Step 5)**
    - [ ] `DosCopy` (ordinal 258) — implement with `std::fs::copy()` after path translation
    - [ ] `DosForceDelete` (ordinal 259) — alias to existing `dos_delete()`
    - [ ] `DosEditName` (ordinal 261) — wildcard filename transformation (e.g., `*.txt` + `*.bak`)
    - [ ] `DosSetFileInfo` (ordinal 279) — stub, return 0
    - [ ] `DosSetFileMode` (ordinal 267) — stub, return 0
    - [ ] `DosSetPathInfo` (ordinal 276) — stub, return 0
    - [ ] `DosQueryFHState` (ordinal 276) / `DosSetFHState` (ordinal 277) — stub, return 0
    - [ ] `DosQueryFSInfo` (ordinal 278) — return disk info defaults (total/free space, volume label)
    - [ ] `DosQueryFSAttach` (ordinal 277) — return drive type defaults
    - [ ] `DosQueryVerify` / `DosSetVerify` — stub, return 0
- [ ] **Device I/O Stubs (Step 6)**
    - [ ] `DosDevIOCtl` (ordinal 284) — stub by category; return 0 for most, `ERROR_INVALID_FUNCTION` for serial
    - [ ] `DosDevConfig` (ordinal 231) — return hardware defaults (0 printers, 0 serial, 1 coprocessor, 1 disk)
- [ ] **Semaphore Extensions (Step 7)**
    - [ ] `DosOpenEventSem` (ordinal 325) — name-based lookup in `SemaphoreManager`
    - [ ] `DosOpenMutexSem` (ordinal 332) — name-based lookup in `SemaphoreManager`
- [ ] **Named Pipe Stubs (Step 8, lower priority)**
    - [ ] `DosCreateNPipe` / `DosConnectNPipe` / `DosSetNPHState` — stub, return error
- [ ] **Session Management Stubs (Step 9, lower priority)**
    - [ ] `DosStartSession` / `DosSetSession` / `DosStopSession` — stub, return error
    - [ ] `DosQueryAppType` — if not done in Subsystem 2

### Verification
- [ ] `cargo build` — compiles cleanly with all three subsystems
- [ ] `cargo test` — all existing + new tests pass
- [ ] Unit tests for `VioManager` screen buffer operations (write, scroll, read back)
- [ ] Unit tests for `KbdCharIn` scancode mapping (Linux keycode → OS/2 KBDKEYINFO)
- [ ] Unit tests for `DosEditName` wildcard pattern replacement
- [ ] Unit tests for `DosQuerySysInfo` QSV_* index range handling
- [ ] Unit tests for `SharedMemManager` name registration and lookup
- [ ] Existing samples still work: `cargo run -- samples/hello/hello.exe`
- [ ] Stretch goal: 4OS2 boots to a prompt and accepts basic commands

## Phase 4: Multimedia and 16-bit Support
- [ ] **Audio/Video (MMPM2)**
    - [ ] Reimplement multimedia APIs using PulseAudio/ALSA or SDL.
- [ ] **16-bit Compatibility**
    - [ ] Integrate a lightweight x86 emulator for 16-bit code execution.
    - [ ] Support NE (New Executable) format parsing and loading.
