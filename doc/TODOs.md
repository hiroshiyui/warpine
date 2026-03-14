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

- [x] **Console Manager (`console.rs`)** — `VioManager` struct with screen buffer, cursor state, terminal dimensions, raw mode via termios, ANSI color mapping (CGA→ANSI), `console_mgr: Mutex<VioManager>` in SharedState
- [x] **Minimal VIO — Output (Step 1)** — `VioWrtTTY` (30), `VioGetMode` (3), `VioGetCurPos` (4), `VioSetCurPos` (15)
- [x] **Minimal KBD — Input (Step 2)** — `KbdCharIn` (4) with blocking/non-blocking modes and escape sequence parsing (arrow keys, Home/End/PgUp/PgDn/Delete), `KbdGetStatus` (10)
- [x] **Screen Manipulation (Step 3)** — `VioScrollUp` (7), `VioScrollDn` (8), `VioWrtCharStrAtt` (26), `VioWrtNCell` (28), `VioWrtNAttr` (27), `VioReadCellStr` (24), `VioSetCurType` (16)
- [x] **Stubs and Configuration (Step 4)** — `KbdStringIn` (9) with echo and backspace, `VioSetAnsi` (38), `VioGetAnsi` (39), `VioSetState` (51 stub), `VioSetCp` (42 stub), `VioGetConfig` (46)

### Subsystem 2: Process Management

New module: `src/loader/process.rs`. Add `ProcessManager` to `SharedState` (or to `managers.rs`).

`ProcessManager` fields: `children: HashMap<u32, std::process::Child>`, `next_pid: u32`, `current_disk: u8` (default 3 = C:), `current_dir: String` (OS/2 current directory, tracked separately from host cwd for sandbox safety).

- [x] **Directory Management (Step 1)** — `DosSetCurrentDir` (255), `DosQueryCurrentDir` (274), `DosQueryCurrentDisk` (275), `DosSetDefaultDisk` (220) with `ProcessManager` tracking current disk/directory. `translate_path()` updated to resolve relative paths against OS/2 current directory. Fixed `DosQueryPathInfo` ordinal from 275 to correct 223.
- [x] **System Information (Step 2)** — `DosQuerySysInfo` (348, full QSV_* table), `DosGetDateTime` (230, real via libc), `DosSetDateTime` (stub) — implemented in Subsystem 3.
- [x] **Process Execution (Step 3)** — `DosExecPgm` (283) with sync/async modes and double-null arg parsing, `DosWaitChild` (280) with specific/any-child and wait/nowait, `DosKillProcess` (237), `DosQueryAppType` (323) with MZ header detection. ProcessManager extended with child tracking. Fixed `DosGetInfoBlocks` ordinal from 283 to correct 312.

### Subsystem 3: Shared Memory, Exception Handling, and Init Stubs

New module: `src/loader/stubs.rs` for simple stub handlers. Add `SharedMemManager` and exception handler storage.

- [x] **Critical Init Stubs (Step 1)** — `DosError`, `DosSetMaxFH`, `DosBeep`, `DosSetExceptionHandler`/`DosUnsetExceptionHandler`, `DosSetSignalExceptionFocus`, `DosAcknowledgeSignalException`, `DosEnterMustComplete`/`DosExitMustComplete`
- [x] **Shared Memory (Step 2)** — `DosAllocSharedMem`, `DosGetNamedSharedMem`, `DosGetSharedMem`, `DosSetMem`, `DosQueryMem` with `SharedMemManager`
- [x] **Codepage and Country Info (Step 3)** — `DosQueryCp` (CP 437), `DosSetProcessCp`, `DosQueryCtryInfo` (US defaults), `DosMapCase`
- [x] **Module Loading Stubs (Step 4)** — `DosLoadModule`, `DosFreeModule`, `DosQueryModuleHandle`, `DosQueryProcAddr`, `DosGetMessage`
- [x] **File Metadata APIs (Step 5)** — `DosCopy`, `DosEditName` (with wildcard transform), `DosSetFileInfo`, `DosSetFileMode`, `DosSetPathInfo`, `DosQueryFHState`/`DosSetFHState`, `DosQueryFSInfo`, `DosQueryFSAttach`, `DosQueryVerify`/`DosSetVerify`
- [x] **Device I/O Stubs (Step 6)** — `DosDevIOCtl`, `DosDevConfig`
- [x] **Semaphore Extensions (Step 7)** — `DosOpenEventSem`, `DosOpenMutexSem` with name-based lookup
- [x] **Named Pipe Stubs (Step 8)** — `DosCreateNPipe`, `DosConnectNPipe`, `DosSetNPHState` (return error)
- [x] **Session Management Stubs (Step 9)** — `DosStartSession`, `DosSetSession`, `DosStopSession` (return error)
- [x] **System Info** — `DosQuerySysInfo` (full QSV_* table), `DosGetDateTime` (real via libc), `DosSetDateTime` (stub)

### Verification
- [x] `cargo build` — compiles cleanly
- [x] `cargo test` — all 49 tests pass
- [x] Unit tests for `VioManager` screen buffer operations (scroll up/down, read cell str, defaults)
- [x] Unit tests for key mapping (enter, printable, backspace → OS/2 charcode/scancode)
- [x] Unit tests for `DosEditName` wildcard pattern replacement (5 test cases)
- [x] Unit tests for `ResourceManager` find operations
- [x] Unit tests for `DosQuerySysInfo` QSV_* constant validation
- [x] Unit tests for `SharedMemManager` name registration and lookup
- [x] Existing samples verified: hello, alloc_test, file_test, pipe_test, thread_test, find_test, mutex_test
- [x] 4OS2 boots to a prompt and accepts basic commands (`ver`, `set`, `exit`, etc.)

## Phase 4: Filesystem I/O (HPFS-Compatible Virtual Filesystem)

Goal: build an isolated virtual filesystem with HPFS semantics and a correctness guarantee — every valid OS/2 filesystem operation succeeds with correct behavior; invalid operations return proper OS/2 error codes, never crashes. The only failure mode is the host side failing (disk full, permissions, etc.).

### Architecture: VFS Trait with Pluggable Backends (Option C)

The current filesystem I/O is pass-through: `translate_path()` maps OS/2 paths to host paths, and `DosOpen`/`DosRead`/`DosWrite` call `std::fs` directly. This "happens to work" for simple cases (e.g., `samples/file_test`) but provides no HPFS semantic guarantees — case sensitivity is wrong, EAs are missing, sharing modes are ignored, and edge cases crash or silently corrupt.

The new design introduces a **VFS trait** as the **correctness boundary** between OS/2 API handlers and the storage backend:

```
  DosOpen/DosRead/DosWrite/DosFindFirst/...   (OS/2 API layer — doscalls.rs)
                    │
                    ▼
              VfsBackend trait                  (OS/2 semantics contract)
                    │
          ┌─────────┴─────────┐
          ▼                   ▼
   HostDirBackend       HpfsImageBackend       (pluggable backends)
   (host directory)     (disk image, future)
```

**Key principle:** The VFS trait defines OS/2 filesystem semantics. Any implementation of the trait must ensure that every valid OS/2 filesystem operation works correctly. The API handlers (`doscalls.rs`) call trait methods and never touch host filesystem primitives directly.

**`HandleManager` integration:** `DriveManager` absorbs `HandleManager` and `HDirManager` — it owns the file handle table (`HashMap<u32, FileEntry>`) and find handle table (`HashMap<u32, FindEntry>`) directly. The old `HandleManager` and `HDirManager` in `managers.rs` remain in place during the transition period and will be removed in Step 7.

### Design Notes (informed by WINE's filesystem approach)

WINE's filesystem layer (`dlls/ntdll/unix/file.c`, `server/fd.c`) provides proven patterns for mapping a foreign OS's filesystem expectations onto Linux:

- **Drive mapping:** WINE uses symlinks in `~/.wine/dosdevices/` (`c:` → `drive_c/`). Simple, inspectable with standard tools. Warpine can adopt a similar config-driven approach (e.g., `drives.toml` or a `dosdevices/`-style directory).
- **Case-insensitive lookup:** WINE's `lookup_unix_name()` tries exact `stat()` first; on failure, falls back to `readdir()` + `strcasecmp`. Directory listings are cached. Linux 5.2+ ext4 and 6.13+ tmpfs support kernel-level case folding (`EXT4_CASEFOLD_FL`), developed in collaboration with Valve/Collabora for WINE/Proton — detect and use when available.
- **Extended attributes:** WINE does *not* implement NTFS alternate data streams. However, OS/2 EAs are more pervasive than NTFS ADS (e.g., `.TYPE` EA for file type association), so we need real EA support. Linux `user.*` xattrs is the primary backend; sidecar files as fallback for filesystems without xattr support.
- **File locking:** WINE uses a hybrid wineserver + `fcntl()` approach because `fcntl()` locks are per-process (not per-handle) and release when any fd to the file is closed. Since warpine manages all OS/2 file handles through a single-process handle manager, we can use `fcntl(F_SETLK)` more directly without a separate lock tracking layer.
- **Filesystem type reporting:** WINE learned the hard way — reporting `UNIXFS` broke apps expecting NTFS, but claiming unimplemented features (named streams, ACLs) also broke apps. We should report `HPFS` with *accurate* capability flags, only claiming features we actually implement.
- **Sandbox:** WINE explicitly provides *no* security sandbox (`Z:` → `/` gives full access). Warpine can do better: since OS/2 apps expect isolated drives, enforce that paths stay within their mapped volume directory. Path traversal prevention (`..` past volume root) gives real isolation with minimal complexity.
- **Reserved device names:** WINE maps CON → console, NUL → `/dev/null`, COM* → `/dev/ttyS*`. OS/2 has similar devices (CON, NUL, CLOCK$, KBD$, SCREEN$) that need mapping.

### Step 1: VFS Trait and Drive Manager — COMPLETED
- [x] **`VfsBackend` trait** — 21 methods defining OS/2 filesystem semantics: `open`, `close`, `read`, `write`, `seek`, `set_file_size`, `flush`, `find_first`, `find_next`, `find_close`, `create_dir`, `delete_dir`, `delete`, `rename`, `copy`, `query_path_info`, `query_file_info`, `set_file_info`, `set_path_info`, `get_ea`, `set_ea`, `enum_ea`, `query_fs_info_alloc`, `query_fs_info_volume`, `fs_name`, `set_file_locks`
- [x] **`VfsFileHandle` / `VfsFindHandle`** — opaque handle types (newtype over `u64`)
- [x] **`Os2Error`** — typed error codes with 20 named constants and Debug/Display
- [x] **OS/2 data types** — `OpenMode`, `SharingMode`, `OpenFlags`, `OpenAction`, `SeekMode`, `FileAttribute`, `FileStatus`, `DirEntry`, `EaEntry`, `FsAllocate`, `FsVolumeInfo`, `FileLockRange` with `from_raw()` parsers
- [x] **`DriveManager`** — maps drive letters (A:–Z:) to `Box<dyn VfsBackend>`. Owns file and search handle tables (absorbs `HandleManager` and `HDirManager` responsibilities). Per-drive current directory tracking. OS/2 path resolution with drive letter extraction
- [x] **Wired into `SharedState`** — `drive_mgr: Mutex<DriveManager>` added alongside existing managers
- [x] **15 unit tests** — error constants, type parsers, DriveManager path resolution, handle allocation, drive mounting, per-drive current directory
- [x] **Drive configuration** — default config: C: → `~/.local/share/warpine/drive_c/` (XDG-compliant, auto-created). `DriveConfig` struct stores host path, volume label, and read-only flag. CLI/config file override deferred to Step 2

### Step 2: HostDir Backend (first implementation) — COMPLETED
- [x] **`HostDirBackend`** — implements `VfsBackend` using a host directory as storage, providing HPFS semantics on top of the Linux filesystem. All 21 trait methods implemented (EAs and file locking are stubs, deferred to Steps 3–4)
- [x] **Case-insensitive, case-preserving lookup** — optimistic `stat()` first, `readdir()` + case-insensitive match fallback (WINE's proven pattern). Resolves each path component independently walking from volume root
- [x] **Long filename support** — filenames up to 254 characters (HPFS limit), `FILENAME_EXCED_RANGE` error if exceeded
- [x] **File sharing modes** — `SharingTable` enforces OS/2 `DosOpen` sharing flags (`DENY_READWRITE`, `DENY_WRITE`, `DENY_READ`, `DENY_NONE`) with bidirectional compatibility checking
- [x] **Sandbox enforcement** — canonicalize + prefix check prevents path traversal escapes
- [x] **OS/2 wildcard matching** — `*` and `?` with case-insensitive comparison
- [x] **Gate test passes** — `test_file_test_gate` mirrors `samples/file_test` exactly (create → write → close → reopen → read → verify)
- [x] **17 unit tests** — wildcard matching, case-insensitive lookup (flat + nested), case-preserving creation, file_test gate, sharing mode enforcement, directory ops, find_first/next, sandbox, metadata, FS info, rename, copy, sharing compatibility
- [ ] **Directory listing caching** — cache `readdir()` results to reduce overhead (optimization, deferred)
- [ ] **Kernel casefold detection** — detect `EXT4_CASEFOLD_FL` for zero-overhead case insensitivity (optimization, deferred)
- [ ] **Device name mapping** — CON, NUL, CLOCK$, KBD$, SCREEN$ → appropriate host devices or internal handlers (deferred to DriveManager level)

### Step 3: Extended Attributes (EAs)
- [ ] **EA storage backend** — persist OS/2 extended attributes using host xattrs (Linux `user.os2.*` namespace) as primary backend, with sidecar `.os2ea/` directory as fallback for filesystems without xattr support
- [ ] **`DosSetFileInfo` / `DosQueryFileInfo`** — FIL_QUERYEASIZE (level 2) and FIL_QUERYEASFROMLIST (level 3) support
- [ ] **`DosSetPathInfo` / `DosQueryPathInfo`** — EA read/write by path
- [ ] **`DosFindFirst` / `DosFindNext`** — return EA size in FILEFINDBUF3 and support FILEFINDBUF3L (level 12/FIL_QUERYEASFROMLISTL)
- [ ] **`DosEnumAttribute`** — enumerate EAs on a file

### Step 4: Filesystem Information and Locking
- [ ] **`DosQueryFSInfo`** — return correct HPFS volume geometry: volume label, serial number, sector size (512), cluster size, total/free space derived from host `statvfs()`
- [ ] **`DosSetFSInfo`** — set volume label (store in `.vol_label` file in volume root)
- [ ] **`DosQueryFSAttach`** — report drive type as local HPFS (`"HPFS"` FSD name) with accurate capability flags (only claim what we implement), enumerate attached drives
- [ ] **`DosSetFileLocks`** — byte-range locking via Linux `fcntl(F_SETLK)`. VFS tracks lock ownership per `VfsFileHandle`, avoiding WINE's per-process vs per-handle mismatch
- [ ] **`DosProtectSetFileLocks`** — protected variant with file lock ID

### Step 5: Directory Enumeration Improvements
- [ ] **Wildcard matching** — full OS/2 wildcard semantics (`*`, `?`, dot-handling rules matching HPFS behavior)
- [ ] **`DosFindFirst` attributes filter** — respect `MUST_HAVE_*` attribute bits, hidden/system/directory filtering
- [ ] **`DosFindNext` multi-entry** — support `ulSearchCount > 1` returning multiple FILEFINDBUF3 entries per call
- [ ] **`DosFindClose`** — proper search handle cleanup

### Step 6: Path Translation Hardening
- [ ] **Sandbox enforcement** — prevent path traversal escapes (`..` past volume root), resolve symlinks and verify they stay within volume boundary. Unlike WINE (which explicitly does *not* sandbox), warpine enforces real isolation per drive
- [ ] **UNC path handling** — `\\server\share` paths return `ERROR_NETWORK_ACCESS_DENIED` or map to a configured directory
- [ ] **`DosQueryPathInfo`** — return correct HPFS attributes for all info levels

### Step 7: Migrate API Handlers
- [ ] **Refactor `doscalls.rs`** — replace all direct `std::fs` calls with `DriveManager` / `VfsBackend` trait method calls
- [ ] **Remove `HandleManager` and `HDirManager`** — file and search handle state now lives inside `DriveManager`
- [ ] **Remove `translate_path()`** — path resolution now lives inside the VFS/DriveManager
- [ ] **Backward compatibility** — existing samples (`file_test`, `find_test`, `fs_ops_test`) work unchanged via default drive configuration (C: → cwd)

### Verification

#### Gate test: `samples/file_test`

The VFS design must pass `samples/file_test` end-to-end as the minimum viability gate. This test exercises the core file I/O path through the VFS:

1. `DosOpen("test.txt", ..., 0x0012, 0x0012)` — create file with `OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS`, sharing mode `OPEN_SHARE_DENYNONE | OPEN_ACCESS_READWRITE`
2. `DosWrite(hf, "Warpine File Test Data", 22)` — write 22 bytes through `VfsBackend::write()`
3. `DosClose(hf)` — close via `DriveManager`, releasing the `VfsFileHandle`
4. `DosOpen("test.txt", ..., 0x0001, 0x0040)` — reopen read-only with `OPEN_SHARE_DENYWRITE`
5. `DosRead(hf, buffer, 22)` — read back through `VfsBackend::read()`, verify 22 bytes returned
6. `DosWrite(1, ...)` — stdout (handle 1, special-cased outside VFS)
7. `DosClose(hf)` + `DosExit(1, 0)`

Expected output: `Read data: Warpine File Test Data`

This validates: DriveManager path resolution (relative path `"test.txt"` → volume root + cwd), VfsFileHandle lifecycle (open → write → close → reopen → read → close), `HostDirBackend` creating and reading a real file, and that stdout (handle 0/1/2) remains special-cased outside the VFS.

#### Full test suite
- [ ] `cargo build` — compiles cleanly
- [ ] `cargo test` — all existing + new tests pass
- [ ] Unit tests for `VfsBackend` trait (using mock/in-memory backend)
- [ ] Unit tests for case-insensitive path resolution (exact-match fast path + readdir fallback)
- [ ] Unit tests for EA storage and retrieval (xattr backend + sidecar fallback)
- [ ] Unit tests for file sharing mode enforcement
- [ ] Unit tests for wildcard matching (HPFS rules)
- [ ] Unit tests for `DosQueryFSInfo` volume geometry
- [ ] Unit tests for path traversal sandbox enforcement (symlink escape, `..` past root)
- [ ] Unit tests for device name mapping (CON, NUL, CLOCK$)
- [ ] `samples/file_test` — gate test (DosOpen/DosWrite/DosRead/DosClose through VFS)
- [ ] `samples/find_test` — DosFindFirst/DosFindNext through VFS
- [ ] `samples/fs_ops_test` — DosCreateDir/DosDeleteDir/DosMove/DosQueryPathInfo through VFS
- [ ] 4OS2 `dir`, `tree`, `copy`, `move`, `del`, `md`, `rd` commands work correctly
- [ ] File attributes (`attrib` command) work correctly

## Phase 5: Multimedia and 16-bit Support
- [ ] **Audio/Video (MMPM/2)**
    - [ ] Reimplement multimedia APIs using PulseAudio/ALSA or SDL.
- [ ] **16-bit Compatibility**
    - [ ] Integrate a lightweight x86 emulator for 16-bit code execution.
    - [ ] Support NE (New Executable) format parsing and loading.
