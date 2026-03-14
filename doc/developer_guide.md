# Warpine Developer Guide

Warpine is an OS/2 compatibility layer for Linux. It loads 32-bit OS/2 executables (LX format) and runs them natively using KVM hardware virtualization — analogous to WINE for Windows, but targeting OS/2 instead.

This guide introduces the internals of Warpine and the OS/2 concepts it emulates.

---

## Table of Contents

1. [Background: OS/2 and LX Executables](#background-os2-and-lx-executables)
2. [Architecture Overview](#architecture-overview)
3. [LX Format Parser](#lx-format-parser)
4. [KVM Virtualization Engine](#kvm-virtualization-engine)
5. [Guest Memory Layout](#guest-memory-layout)
6. [API Thunking Mechanism](#api-thunking-mechanism)
7. [OS/2 API Emulation](#os2-api-emulation)
8. [Threading Model](#threading-model)
9. [IPC: Semaphores and Queues](#ipc-semaphores-and-queues)
10. [Presentation Manager (GUI)](#presentation-manager-gui)
11. [PM Callback Mechanism](#pm-callback-mechanism)
12. [Text-Mode Console Subsystem](#text-mode-console-subsystem)
13. [4OS2 Compatibility](#4os2-compatibility)
14. [Filesystem I/O Design](#filesystem-io-design)
15. [Module Structure](#module-structure)
16. [Adding a New API](#adding-a-new-api)

---

## Background: OS/2 and LX Executables

OS/2 is a 32-bit operating system originally developed by IBM. Its native executable format is **LX** (Linear eXecutable), a successor to the NE format. LX binaries are typically wrapped in a DOS MZ stub — the file begins with a standard MZ header whose `e_lfanew` field points to the LX header.

OS/2 applications link against system DLLs by **module name** and **ordinal number**. The primary API module is **DOSCALLS** (file/memory/thread/IPC operations). GUI applications additionally import from **PMWIN** (window management, message loop) and **PMGPI** (graphics primitives).

The OS/2 API calling convention is `_System` (also known as `APIENTRY`): arguments are pushed right-to-left on the stack, the return value is in EAX, and the **caller** cleans up the stack. This is important for Warpine's API thunking, which reads arguments from the guest stack and writes return values to guest RAX.

---

## Architecture Overview

Warpine's execution pipeline has four stages:

```
  Parse (LX)  →  Load (KVM guest memory)  →  Execute (VMEXIT loop)  →  Thunk (API dispatch)
```

1. **Parse** — `LxFile::open()` reads the MZ+LX executable: headers, object table, page map, fixup records, and import tables.
2. **Load** — `Loader::load()` allocates 128 MB of KVM guest memory, maps executable pages into it, and applies relocations (fixups).
3. **Execute** — `Loader::run_vcpu()` creates a KVM vCPU in 32-bit protected mode and enters the VMEXIT loop.
4. **Thunk** — When the guest calls an OS/2 API, it hits an INT 3 breakpoint at a magic address. The resulting `VMEXIT_DEBUG` is caught by the host, which reads arguments from the guest stack, executes the API in Rust, and writes the result back to guest RAX.

For **CLI applications**, the vCPU runs on the main thread. For **PM (GUI) applications**, the winit event loop runs on the main thread (as required by most windowing systems), and the vCPU runs on a worker thread.

---

## LX Format Parser

**Module:** `src/lx/` (`header.rs` for binary structures, `lx.rs` for orchestration)

The parser handles:

- **MZ stub detection** — Reads the DOS header to find the LX header offset.
- **LX header** — CPU type, OS type, module flags, entry point (EIP object + offset), stack (ESP object + offset), page size, object/fixup table offsets.
- **Object table** — Each object describes a segment: base address, virtual size, flags (readable/writable/executable), and page count. Objects are 1-indexed.
- **Page map** — Maps logical pages to their location in the file (data offset, data size, flags).
- **Fixup records** — Relocations that must be applied when loading. Each record has source offsets (where to patch) and a target:

  | FixupTarget variant | Description |
  |---|---|
  | `Internal { object_num, target_offset }` | Points to an offset within another object |
  | `ExternalOrdinal { module_ordinal, proc_ordinal }` | Import by module + ordinal number |
  | `ExternalName { module_ordinal, proc_name_offset }` | Import by module + procedure name |
  | `InternalEntry { entry_ordinal }` | Points to an entry in the module's entry table |

- **Import table** — List of module names (e.g., `"DOSCALLS"`, `"PMWIN"`, `"PMGPI"`).

Unit tests for the parser live alongside the source in `src/lx/`.

---

## KVM Virtualization Engine

Warpine uses Linux KVM (Kernel-based Virtual Machine) for hardware-accelerated x86 emulation. The setup in `Loader::new()`:

1. **Create VM** — Open `/dev/kvm`, create a VM file descriptor.
2. **Allocate guest memory** — `mmap` 128 MB of anonymous memory, register it as a KVM memory region at guest physical address 0.
3. **Set up GDT** — A Global Descriptor Table is written into guest memory with segments for 32-bit protected mode:
   - Code segment (selector 0x08): base 0, limit 4 GB, 32-bit, execute/read
   - Data segment (selector 0x10): base 0, limit 4 GB, 32-bit, read/write
4. **Configure vCPU** — Set segment registers (CS=0x08, DS/ES/SS=0x10), enable protected mode (CR0.PE), set EFLAGS, configure debug registers to trap INT 3 (`DR7` with `GD` bit, guest debug via `KVM_GUESTDBG_ENABLE | KVM_GUESTDBG_USE_SW_BP`).

The **VMEXIT loop** in `run_vcpu()` repeatedly calls `vcpu.run()` and matches on the exit reason:

```
loop {
    match vcpu.run() {
        VcpuExit::Debug  → handle breakpoints (API calls, exit trap, callback return)
        VcpuExit::Hlt    → guest halted
        _                → unhandled VMEXIT error
    }
}
```

---

## Guest Memory Layout

All addresses are guest physical addresses (GPA = GVA in Warpine's flat memory model):

| Address Range | Purpose |
|---|---|
| `0x00001000` – `0x0008FFFF` | GDT, executable pages (loaded from LX objects) |
| `0x00090000` (`TIB_BASE`) | Thread Information Block (TIB) for thread 1 |
| `0x00091000` (`PIB_BASE`) | Process Information Block (PIB) |
| `0x00092000` (`ENV_ADDR`) | Reserved (env block is now dynamically allocated) |
| `0x01000000` (`MAGIC_API_BASE`) | API thunk stubs (10240 bytes of INT 3 instructions) |
| `0x010003FE` (`CALLBACK_RET_TRAP`) | PM callback return trap |
| `0x010003FF` (`EXIT_TRAP_ADDR`) | Thread exit trap |
| `0x02000000` (`DYNAMIC_ALLOC_BASE`) | Dynamic allocation pool (DosAllocMem, env block) |

TIB/PIB addresses must stay below `0x100000` so that 16-bit segment arithmetic (`addr >> 4` fits in u16) works correctly for `DosGetInfoSeg` and similar APIs.

#### PIB Layout

The Process Information Block has the following key offsets:

| Offset | Field | Description |
|---|---|---|
| `+0x00` | `pib_ulpid` | Process ID |
| `+0x04` | `pib_ulppid` | Parent process ID |
| `+0x08` | `pib_hmte` | Module handle |
| `+0x0C` | `pib_pchcmd` | Pointer to command line string |
| `+0x10` | `pib_pchenv` | Pointer to environment block |

The environment block follows OS/2 format: null-terminated `KEY=VALUE` strings, double-null terminated. The command line is stored separately (typically after the environment block) and contains `program_name\0arguments\0`.

Guest memory helpers in `src/loader/guest_mem.rs` provide safe access:

- `guest_ptr(offset, len)` — Returns a host pointer to guest memory with bounds checking.
- `guest_read::<T>(offset)` / `guest_write::<T>(offset, val)` — Typed read/write.
- `guest_write_bytes(offset, data)` — Bulk write.
- `read_guest_string(ptr)` — Read a null-terminated C string (max 4096 bytes).
- `translate_path(os2_path)` — Convert `C:\path` to a Unix path, with sandbox boundary enforcement.

---

## API Thunking Mechanism

This is Warpine's core trick for intercepting OS/2 API calls without modifying the guest binary.

### Setup

During loading, `setup_stubs()` fills a 10240-byte region at `MAGIC_API_BASE` with INT 3 (0xCC) instructions. When the LX loader encounters an import fixup for a known module, `resolve_import()` maps it to a specific address:

| Module | Base Constant | Address formula | Range |
|---|---|---|---|
| DOSCALLS | — | `MAGIC_API_BASE + ordinal` | 0–1023 |
| QUECALLS | — | `MAGIC_API_BASE + 1024 + ordinal` | 1024–2047 |
| PMWIN | `PMWIN_BASE` (2048) | `MAGIC_API_BASE + 2048 + ordinal` | 2048–3071 |
| PMGPI | `PMGPI_BASE` (3072) | `MAGIC_API_BASE + 3072 + ordinal` | 3072–4095 |
| KBDCALLS | `KBDCALLS_BASE` (4096) | `MAGIC_API_BASE + 4096 + ordinal` | 4096–5119 |
| VIOCALLS | `VIOCALLS_BASE` (5120) | `MAGIC_API_BASE + 5120 + ordinal` | 5120–6143 |
| SESMGR | `SESMGR_BASE` (6144) | `MAGIC_API_BASE + 6144 + ordinal` | 6144–7167 |
| NLS | `NLS_BASE` (7168) | `MAGIC_API_BASE + 7168 + ordinal` | 7168–8191 |
| MSG | `MSG_BASE` (8192) | `MAGIC_API_BASE + 8192 + ordinal` | 8192–10239 |

The fixup is patched so the guest's `CALL` instruction targets the appropriate stub address.

### Interception

When the guest executes `CALL [api_stub]`:
1. The CPU pushes the return address and jumps to `MAGIC_API_BASE + offset`.
2. The INT 3 at that address causes a `VMEXIT_DEBUG`.
3. The host reads RIP from vcpu registers, computes `ordinal = RIP - MAGIC_API_BASE`.
4. `handle_api_call()` dispatches to the appropriate handler based on ordinal range.
5. The handler reads arguments from the guest stack using the `_System` calling convention:
   ```
   arg1 = guest_read::<u32>(ESP + 4)
   arg2 = guest_read::<u32>(ESP + 8)
   ...
   ```
6. The handler executes the API logic on the host side and returns a result.
7. The host writes the result to guest RAX and advances RIP past the INT 3.
8. The vCPU resumes and the guest continues execution.

---

## OS/2 API Emulation

### DOSCALLS (src/loader/doscalls.rs)

The primary OS/2 API module. Implemented functions:

| Category | Functions |
|---|---|
| **File I/O** | DosOpen, DosClose, DosRead (including stdin), DosWrite, DosSetFilePtr, DosDelete, DosMove, DosCopy, DosQueryPathInfo, DosQueryFileInfo, DosSetFileInfo, DosSetFileMode, DosSetPathInfo, DosQueryHType, DosQueryFHState, DosSetFHState, DosQueryFSInfo, DosQueryFSAttach, DosQueryVerify, DosSetVerify, DosEditName |
| **Directory** | DosCreateDir, DosDeleteDir, DosFindFirst, DosFindNext, DosFindClose, DosSetCurrentDir, DosQueryCurrentDir, DosQueryCurrentDisk, DosSetDefaultDisk |
| **Memory** | DosAllocMem, DosFreeMem, DosAllocSharedMem, DosGetNamedSharedMem, DosGetSharedMem, DosSetMem, DosQueryMem |
| **Threading** | DosCreateThread, DosSleep, DosWaitThread, DosGetInfoBlocks |
| **Process** | DosExecPgm, DosWaitChild, DosKillProcess, DosQueryAppType, DosExit |
| **Semaphores** | DosCreateEventSem, DosCloseEventSem, DosPostEventSem, DosWaitEventSem, DosOpenEventSem, DosCreateMutexSem, DosCloseMutexSem, DosRequestMutexSem, DosReleaseMutexSem, DosOpenMutexSem, DosCreateMuxWaitSem, DosCloseMuxWaitSem, DosWaitMuxWaitSem |
| **Queues** | DosCreateQueue, DosOpenQueue, DosWriteQueue, DosReadQueue, DosCloseQueue, DosPurgeQueue, DosQueryQueue |
| **Pipes** | DosCreatePipe |
| **System** | DosQuerySysInfo, DosGetDateTime, DosQueryCp, DosQueryCtryInfo, DosMapCase, DosDevConfig, DosDevIOCtl |
| **Stubs** | DosError, DosSetMaxFH, DosBeep, DosSetExceptionHandler, DosLoadModule, DosStartSession, and others |

All blocking operations (DosSleep, DosWaitEventSem, DosRequestMutexSem, DosWaitMuxWaitSem, DosReadQueue, DosWaitThread) check the `exit_requested` flag in 100 ms intervals to ensure clean shutdown.

### PMWIN (src/loader/pm_win.rs)

Window management APIs — approximately 50 ordinals including WinInitialize, WinCreateMsgQueue, WinRegisterClass, WinCreateStdWindow, WinGetMsg, WinDispatchMsg, WinSendMsg, WinPostMsg, WinDefWindowProc, WinBeginPaint, WinEndPaint, WinMessageBox, WinStartTimer, WinStopTimer, clipboard operations, and dialog support.

### PMGPI (src/loader/pm_gpi.rs)

Graphics Primitive Interface — GpiCreatePS, GpiDestroyPS, GpiSetColor, GpiMove, GpiLine, GpiBox, GpiCharStringAt, GpiErase.

---

## Threading Model

Each OS/2 thread maps to a native Rust thread with its own KVM vCPU. When `DosCreateThread` is called:

1. Allocate a stack in guest memory (64 KB default).
2. Create a Thread Information Block (TIB) for the new thread.
3. Assign a thread ID from the `next_tid` counter.
4. Spawn a Rust thread that creates a new `VcpuFd`, configures it identically to the primary vCPU, sets ESP to the new stack and EIP to the guest's thread function, and enters its own VMEXIT loop.

All threads share the same `SharedState` via `Arc`. Managers are protected by `Mutex` with a documented lock ordering to prevent deadlocks:

```
Level 1: next_tid, threads          (lightweight counters)
Level 2: mem_mgr, handle_mgr, hdir_mgr   (independent managers)
Level 3: queue_mgr                  (may lock inner queue mutexes)
Level 4: sem_mgr                    (may lock inner semaphore mutexes)
Level 5: window_mgr                 (may lock inner message queue mutexes)
```

All mutex acquisitions use `lock_or_recover()` (from the `MutexExt` trait) which recovers from poisoned locks rather than panicking, ensuring one thread's panic doesn't cascade.

---

## IPC: Semaphores and Queues

**Module:** `src/loader/ipc.rs`

### Semaphores

- **Event semaphores** — Binary signal (posted/reset). Backed by `Arc<(Mutex<EventSemaphore>, Condvar)>`. `DosPostEventSem` sets `posted = true` and notifies all waiters; `DosWaitEventSem` blocks on the condvar.
- **Mutex semaphores** — Ownership-based mutual exclusion with a request count for recursive locking. `DosRequestMutexSem` checks `owner_tid`; if unowned or owned by the calling thread, it acquires/increments. Otherwise it blocks on the condvar.
- **MuxWait semaphores** — Wait on multiple semaphores. Supports `wait_all` (AND) and `wait_any` (OR) modes. Polls constituent semaphores in a loop.

### Queues

OS/2 queues (`DosCreateQueue` / `DosReadQueue`) are named, prioritized message queues. Each `OS2Queue` has a `VecDeque<QueueEntry>` and a condvar for blocking reads. `DosWriteQueue` pushes an entry and signals the condvar; `DosReadQueue` dequeues (FIFO or LIFO depending on queue attributes) or blocks if empty.

---

## Presentation Manager (GUI)

**Modules:** `src/gui.rs`, `src/loader/pm_win.rs`, `src/loader/pm_gpi.rs`, `src/loader/pm_types.rs`

The Presentation Manager (PM) is OS/2's GUI subsystem. Warpine implements it using **winit** (cross-platform window management) and **softbuffer** (CPU-based framebuffer rendering).

### Dual Execution Paths

In `main.rs`, Warpine detects PM applications by checking if the executable imports the `"PMWIN"` module:

- **CLI apps** — vCPU runs on the main thread, no event loop.
- **PM apps** — winit event loop runs on the main thread (required by most windowing systems). The vCPU is spawned on a worker thread. Communication between the vCPU thread and the GUI thread happens via a channel (`GUISender` / `GUIReceiver`).

### Window Lifecycle

1. **WinInitialize** — Returns a handle to the anchor block (HAB).
2. **WinCreateMsgQueue** — Creates a `PM_MsgQueue` and maps the calling thread to it.
3. **WinRegisterClass** — Stores a `WindowClass` with the guest's window procedure address.
4. **WinCreateStdWindow** — Sends a `GUIMessage::CreateWindow` to the GUI thread (which creates the actual winit window), creates frame and client `OS2Window` entries in the `WindowManager`, and returns the frame HWND.
5. **WinGetMsg** — Blocks on the message queue condvar until a message is available. Returns FALSE on WM_QUIT (ending the message loop).
6. **WinDispatchMsg** — Invokes the guest's window procedure via the callback mechanism (see below).
7. **WinDestroyMsgQueue / WinTerminate** — Cleanup.

### Rendering Pipeline

Drawing in PM uses Presentation Spaces (HPS):

1. **WinBeginPaint** — Creates or retrieves an HPS for the window.
2. **GpiSetColor / GpiMove / GpiLine / GpiBox / GpiCharStringAt** — Modify the presentation space state and send `GUIMessage::Draw*` commands to the GUI thread.
3. **WinEndPaint** — Sends `GUIMessage::PresentBuffer` to flush the framebuffer to the screen.

The GUI thread (`GUIApp` in `src/gui.rs`) processes these messages, drawing into a pixel buffer via softbuffer, and presents it to the window surface.

### Input Handling

The GUI thread translates winit input events into OS/2 messages:
- Keyboard events → `WM_CHAR`
- Mouse movement → `WM_MOUSEMOVE`
- Mouse buttons → `WM_BUTTON1DOWN` / `WM_BUTTON1UP`
- Window resize → `WM_SIZE`
- Close request → `WM_CLOSE`

OS/2 uses a bottom-left coordinate origin (Y increases upward), so Y coordinates are flipped during translation.

---

## PM Callback Mechanism

The most complex part of Warpine is **guest callbacks** — when `WinDispatchMsg` or `WinSendMsg` needs to invoke the guest's window procedure (a function compiled into the OS/2 binary).

Since the window procedure runs in the guest (inside KVM), Warpine can't simply call it as a Rust function. Instead, it uses a **trampoline**:

1. `WinDispatchMsg` looks up the target window's class to find the `pfn_wp` (window procedure address).
2. It returns `ApiResult::Callback { wnd_proc, hwnd, msg, mp1, mp2 }` instead of `ApiResult::Normal`.
3. The VMEXIT loop saves the current RIP and RSP as a `CallbackFrame` on a per-vCPU stack.
4. It pushes the callback arguments (hwnd, msg, mp1, mp2) onto the guest stack in `_System` convention, with `CALLBACK_RET_TRAP` (0x010003FE) as the return address.
5. It sets guest RIP to the window procedure address and resumes the vCPU.
6. The guest window procedure executes normally, potentially making further API calls (which are thunked as usual).
7. When the window procedure returns, it executes `RET` which pops `CALLBACK_RET_TRAP` into EIP.
8. The INT 3 at `CALLBACK_RET_TRAP` causes another VMEXIT. The host pops the `CallbackFrame`, restores the saved RIP/RSP, reads the return value from guest EAX, and resumes the original API call flow.

This mechanism is **re-entrant** — a window procedure can call `WinSendMsg`, which triggers another callback, pushing another frame onto the callback stack. The stack unwinds naturally as each callback returns.

---

## Text-Mode Console Subsystem

**Modules:** `src/loader/console.rs`, `src/loader/viocalls.rs`, `src/loader/kbdcalls.rs`

OS/2 text-mode applications use two subsystems: **VIOCALLS** (Video I/O) for screen output and **KBDCALLS** (Keyboard) for input. Warpine implements these by mapping VIO calls to ANSI escape sequences on the host terminal and KBD calls to Linux termios raw mode input.

### VioManager (console.rs)

The `VioManager` maintains:
- A **screen buffer** of `(char, attribute)` cell pairs (row-major, CGA 16-color attributes)
- **Cursor position** (row, col) and visibility state
- **Terminal dimensions** detected via `TIOCGWINSZ` ioctl
- **Raw mode state** for keyboard input via `tcsetattr`

VIO output functions write to the screen buffer and emit ANSI escape sequences to the host terminal. CGA attribute bytes are mapped to ANSI color codes (foreground 30–37, background 40–47, with bright bit support).

### DosRead on stdin

CLI applications like 4OS2 read keyboard input via `DosRead` on file handle 0 rather than using `KbdCharIn`. The `dos_read_stdin()` handler:

1. Enables terminal raw mode (`VMIN=0, VTIME=1` for 100ms timeout polling)
2. Blocks until a byte is available, checking `exit_requested` between polls
3. **Translates CR → CR+LF** — OS/2 console convention; pressing Enter on the host sends `\r`, but OS/2 apps expect `\r\n` as the line terminator. A pending LF byte is queued in `VioManager.stdin_pending_lf` and delivered on the next `DosRead` call.
4. **Echoes characters** — Raw mode disables terminal echo, so `dos_read_stdin` writes typed characters back to stdout (including destructive backspace handling)

### 16-bit Thunk Bypass

Some OS/2 applications contain 16-bit code thunks (e.g., `LSS` instructions for loading segment:offset pairs) that cause #GP faults in Warpine's flat 32-bit mode. The VMEXIT handler detects these by:

1. Checking if the faulting instruction is `LSS` (opcodes `0x66 0x0F` or `0x0F 0xB2`)
2. Scanning the guest stack for return addresses within known code object ranges (`SharedState.code_ranges`)
3. Skipping the thunk by setting RIP to the found return address

---

## 4OS2 Compatibility

[4OS2](https://github.com/StevenLevine/4os2) is a commercial-grade OS/2 command shell (BSD-like license from JP Software). It serves as Warpine's primary text-mode compatibility target because it exercises nearly every DOSCALLS surface plus the full Kbd/Vio console subsystem.

### Setup

```bash
cd samples/4os2
./fetch_source.sh    # Clones source from GitHub at a pinned commit
make                 # Cross-compiles with Open Watcom
```

### Running

```bash
cargo run -- samples/4os2/4os2.exe
```

4OS2 boots to an interactive `[c:\]` prompt. Supported commands include `ver`, `set`, `echo`, `exit`, and other built-in commands. Use `RUST_LOG=debug` to trace API calls.

### Key implementation details

Several issues were discovered and fixed during 4OS2 bring-up that are worth noting for future OS/2 application compatibility work:

- **PIB field ordering** — `pib_pchcmd` is at offset `+0x0C` and `pib_pchenv` is at `+0x10`. Getting these swapped causes the app to read the command line string as the environment block (symptom: `set` shows no variables, `memory` reports only ~13 bytes used in env).
- **Environment block format** — Must be null-terminated `KEY=VALUE` strings followed by a double-null terminator. The command line string (`program_name\0arguments\0`) is stored separately and pointed to by `pib_pchcmd`. The env block is dynamically allocated to avoid collisions with loaded LX objects.
- **Guest memory layout** — LX objects can load at addresses up to `0x80000+`, so TIB/PIB must be placed above that range. But they must also stay below `0x100000` for 16-bit segment arithmetic (`addr >> 4` must fit in u16). The safe zone is `0x90000–0x9FFFF`.
- **CR→CRLF on stdin** — OS/2 console DosRead returns `\r\n` when Enter is pressed. Linux terminals in raw mode send only `\r`. Without translation, 4OS2 never recognizes end-of-line.
- **Character echo** — Terminal raw mode disables echo. OS/2 apps expect the console driver to echo typed characters during DosRead, so the emulation layer must do this explicitly.

---

## Filesystem I/O Design

### Motivation: From "Happens to Work" to "Guaranteed to Work"

The current filesystem I/O is **pass-through**: `translate_path()` maps OS/2 paths to host paths, and `DosOpen`/`DosRead`/`DosWrite` call `std::fs` directly with host `File` objects stored in `HandleManager`. This works for simple cases (e.g., `samples/file_test` writes `test.txt` to the host cwd) but provides no HPFS semantic guarantees:

- Case sensitivity is wrong (host FS is case-sensitive; HPFS is not)
- Extended attributes are missing entirely
- File sharing modes (`OPEN_SHARE_DENY*`) are ignored
- Wildcard matching doesn't follow OS/2 rules
- Edge cases crash or produce wrong results instead of proper error codes

The goal is a **correctness guarantee**: every valid OS/2 filesystem operation succeeds with correct HPFS behavior. Invalid operations return proper OS/2 error codes, not crashes. The only failure mode is the host side failing (disk full, permissions, etc.).

### Architecture: VFS Trait as the Correctness Boundary

The design uses a `VfsBackend` trait as the **semantic contract** between OS/2 API handlers and the storage layer. The trait defines OS/2 filesystem operations with HPFS semantics. Backend implementations must fulfill this contract regardless of how they store data.

```
  DosOpen / DosRead / DosWrite / DosFindFirst / ...    (OS/2 API layer — doscalls.rs)
                         │
                         ▼
                   DriveManager                         (drive letter → backend routing)
                         │
                         ▼
                  VfsBackend trait                      (OS/2 semantics contract)
                         │
               ┌─────────┴──────────┐
               ▼                    ▼
        HostDirBackend        HpfsImageBackend          (pluggable backends)
        (host directory)      (disk image, future)
```

**Key principle:** API handlers (`doscalls.rs`) call trait methods and never touch host filesystem primitives (`std::fs`) directly. The VFS is the correctness boundary — if the trait contract is met, OS/2 apps get correct behavior regardless of backend.

### VfsBackend Trait

The trait surface is driven by OS/2 filesystem semantics, not by what's convenient for any particular backend:

```rust
pub trait VfsBackend: Send + Sync {
    // File operations
    fn open(&self, path: &Os2Path, mode: OpenMode, flags: OpenFlags,
            sharing: SharingMode) -> Result<VfsFileHandle, Os2Error>;
    fn close(&self, handle: VfsFileHandle) -> Result<(), Os2Error>;
    fn read(&self, handle: &VfsFileHandle, buf: &mut [u8]) -> Result<usize, Os2Error>;
    fn write(&self, handle: &VfsFileHandle, buf: &[u8]) -> Result<usize, Os2Error>;
    fn seek(&self, handle: &VfsFileHandle, offset: i64, whence: SeekMode) -> Result<u64, Os2Error>;

    // Directory operations
    fn create_dir(&self, path: &Os2Path) -> Result<(), Os2Error>;
    fn delete_dir(&self, path: &Os2Path) -> Result<(), Os2Error>;
    fn delete(&self, path: &Os2Path) -> Result<(), Os2Error>;
    fn rename(&self, from: &Os2Path, to: &Os2Path) -> Result<(), Os2Error>;

    // Directory enumeration
    fn find_first(&self, spec: &Os2Path, attr_filter: u32,
                  level: u32) -> Result<(VfsFindHandle, Vec<DirEntry>), Os2Error>;
    fn find_next(&self, handle: &VfsFindHandle,
                 count: u32) -> Result<Vec<DirEntry>, Os2Error>;
    fn find_close(&self, handle: VfsFindHandle) -> Result<(), Os2Error>;

    // Metadata
    fn query_path_info(&self, path: &Os2Path, level: u32) -> Result<FileInfo, Os2Error>;
    fn query_file_info(&self, handle: &VfsFileHandle, level: u32) -> Result<FileInfo, Os2Error>;
    fn set_file_info(&self, handle: &VfsFileHandle, level: u32,
                     info: &FileInfo) -> Result<(), Os2Error>;

    // Extended attributes
    fn get_ea(&self, path: &Os2Path, name: &str) -> Result<Vec<u8>, Os2Error>;
    fn set_ea(&self, path: &Os2Path, name: &str, data: &[u8]) -> Result<(), Os2Error>;
    fn enum_ea(&self, path: &Os2Path) -> Result<Vec<EaEntry>, Os2Error>;

    // Volume information
    fn query_fs_info(&self, level: u32) -> Result<FsInfo, Os2Error>;

    // Locking
    fn set_file_locks(&self, handle: &VfsFileHandle,
                      locks: &[LockRange], unlock: &[LockRange]) -> Result<(), Os2Error>;
}
```

`VfsFileHandle` and `VfsFindHandle` are opaque types returned by the backend — the API layer never inspects their internals.

### DriveManager

The `DriveManager` replaces the current `translate_path()`. It maps drive letters to backends and resolves OS/2 paths to the correct backend + relative path:

```rust
pub struct DriveManager {
    drives: [Option<DriveMount>; 26],  // A: = 0, B: = 1, ..., Z: = 25
    current_drive: u8,                 // default: 2 (C:)
    current_dirs: [String; 26],        // per-drive current directory
}

pub struct DriveMount {
    backend: Box<dyn VfsBackend>,
    volume_label: String,
    read_only: bool,
}
```

Path resolution flow:
1. Parse drive letter (or use `current_drive` if relative)
2. Look up `DriveMount` for that drive → `ERROR_INVALID_DRIVE` if unmounted
3. Resolve relative path against `current_dirs[drive]`
4. Check for reserved device names (CON, NUL, CLOCK$, KBD$, SCREEN$) → redirect to internal handlers
5. Pass the resolved relative path to the backend

### Handle Management (absorbed into VFS)

Currently, file handles and directory search handles are managed by two separate structs in `managers.rs`:

- `HandleManager` — maps OS/2 file handles (`u32`) → `std::fs::File`
- `HDirManager` — maps OS/2 search handles (`u32`) → `HDirEntry { iterator: ReadDir, pattern: String }`

Both are absorbed into the VFS layer. The `DriveManager` owns all handle state:

```rust
pub struct DriveManager {
    drives: [Option<DriveMount>; 26],
    current_drive: u8,
    current_dirs: [String; 26],

    // Handle tables (moved from HandleManager + HDirManager)
    file_handles: HashMap<u32, OpenFile>,    // OS/2 handle → open file state
    find_handles: HashMap<u32, FindState>,   // OS/2 hdir → search state
    next_file_handle: u32,                   // starts at 3 (0/1/2 = stdin/stdout/stderr)
    next_find_handle: u32,
}

struct OpenFile {
    drive: u8,                    // which drive this file belongs to
    vfs_handle: VfsFileHandle,    // opaque handle from the backend
    sharing_mode: SharingMode,    // OS/2 sharing flags from DosOpen
}

struct FindState {
    drive: u8,
    vfs_find: VfsFindHandle,     // opaque handle from the backend
}
```

**Why absorb rather than keep separate?** The VFS needs to enforce file sharing modes, track locks per handle, and route operations to the correct drive's backend. If handles are managed externally, every API call requires a lookup in the handle table *and* a lookup in the drive table — two indirections that share no state. With handles inside the DriveManager, `dos_read(handle)` is a single lookup that yields the backend, the VFS handle, and the sharing/lock state together.

Standard handles (0=stdin, 1=stdout, 2=stderr) remain special-cased in `doscalls.rs` — they are not routed through the VFS. The old `HandleManager` and `HDirManager` in `managers.rs` are removed once migration is complete.

### HostDirBackend

The first backend implementation, using a host directory as the volume root. It translates HPFS semantics to Linux filesystem operations:

| HPFS Semantic | Linux Implementation |
|---|---|
| Case-insensitive lookup | Optimistic `stat()` → `readdir()` + `strcasecmp` fallback (WINE's pattern). Optional kernel casefold detection. |
| Extended attributes | `user.os2.*` xattrs (primary) → `.os2ea/` sidecar directory (fallback) |
| File sharing modes | In-memory sharing mode table keyed by inode; checked on open, released on close |
| Byte-range locking | `fcntl(F_SETLK)` with per-handle tracking via `VfsFileHandle` |
| Long filenames (254 chars) | Native (Linux supports 255) |
| Volume geometry | `statvfs()` on the root directory |
| Sandbox enforcement | Canonicalize + verify prefix stays within volume root |

#### Case-Insensitive Path Resolution (detail)

Strategy adopted from WINE's `lookup_unix_name()`:

1. **Optimistic `stat()`** — Try the exact path first. If it succeeds, done. This is the fast path for well-behaved applications that use consistent casing.
2. **`readdir()` fallback** — On `ENOENT`, open the parent directory, enumerate entries with `readdir()`, and compare case-insensitively. Cache the listing to avoid repeated syscalls when multiple lookups hit the same directory.
3. **Kernel casefold (optional)** — Linux 5.2+ ext4 supports per-directory case-insensitive lookup (`EXT4_CASEFOLD_FL`, via `ioctl`). Linux 6.13+ tmpfs also supports this. When detected, skip the userspace fallback entirely. This feature was developed specifically for WINE/Proton by Collabora and Valve.

Each path component is resolved independently, walking from the volume root to the target.

#### Extended Attributes (detail)

OS/2 EAs are more pervasive than NTFS alternate data streams. The `.TYPE` EA (file type association), `.LONGNAME`, and `.SUBJECT` are common. Many OS/2 applications read and write EAs routinely.

| Backend | Mechanism | Pros | Cons |
|---|---|---|---|
| Linux xattrs | `user.os2.{name}` namespace | Native, atomic, fast | Not all FS support xattrs; size limits vary (ext4: ~4KB per attr) |
| Sidecar files | `.os2ea/{filename}.ea` directory | Works everywhere | Extra I/O, cleanup on rename/delete, atomicity concerns |

Primary: xattrs. Fallback: sidecar (detected by attempting a test `setxattr` on volume root at mount time).

#### File Locking (detail)

WINE uses a hybrid wineserver + `fcntl()` approach because `fcntl()` locks are per-process (not per-handle) and release when any fd is closed. Since warpine manages all OS/2 file handles in a single host process, we can use `fcntl(F_SETLK)` more directly. The `VfsFileHandle` tracks lock ownership, avoiding the per-process vs per-handle mismatch.

### Filesystem Type Reporting

`DosQueryFSInfo` and `DosQueryFSAttach` report the filesystem type to applications. WINE learned that reporting incorrect types breaks apps in both directions: reporting `UNIXFS` broke apps expecting NTFS, but claiming unsupported features also broke apps. Warpine reports `HPFS` as the FSD name with **accurate** capability flags — only claim features we actually implement.

### Device Name Handling

OS/2 reserved device names are intercepted during path resolution in the `DriveManager`, before reaching any backend:

| OS/2 Device | Handling |
|---|---|
| `NUL` | `/dev/null` |
| `CON` | stdin/stdout (context-dependent) |
| `CLOCK$` | Internal (stub — system clock device) |
| `KBD$` | stdin / KbdCharIn handler |
| `SCREEN$` | stdout / VioWrtTTY handler |

Detected by case-insensitive match (with or without trailing extension). WINE handles the equivalent Windows devices (CON, NUL, PRN, COM1–9, LPT1–9) the same way.

### Sandbox Enforcement

The `HostDirBackend` enforces that all path resolution stays within its volume root:

1. Normalize the path (resolve `.` and `..` components)
2. Canonicalize via `realpath()` (resolves symlinks)
3. Verify the result starts with the volume root prefix
4. Reject with `ERROR_PATH_NOT_FOUND` (3) if it escapes

WINE explicitly does *not* sandbox (`"Wine is NOT a sandbox"`). Warpine's isolated-directory model provides real containment with minimal complexity — OS/2 applications expect to operate within discrete drive boundaries, so isolation is both correct and secure.

### Migration Path

The VFS is introduced incrementally:

1. **Define trait + DriveManager** — new module `src/loader/vfs.rs` (or `src/vfs/`). DriveManager absorbs `HandleManager` and `HDirManager` handle tables from the start.
2. **Implement HostDirBackend** — starts with basic open/read/write/close, then adds case-insensitive lookup, EAs, sharing modes, locking
3. **Refactor doscalls.rs** — replace `std::fs` calls with `DriveManager` / `VfsBackend` trait calls, one API at a time
4. **Remove `HandleManager` and `HDirManager`** — all file and search handle state now lives in DriveManager
5. **Remove `translate_path()`** — path resolution now lives inside DriveManager
6. **Default configuration** — `C:` maps to cwd by default, so existing samples work unchanged
7. **Gate test** — `samples/file_test` must pass end-to-end after each migration step. It exercises DosOpen (create + read modes with sharing flags), DosWrite, DosRead, DosClose through the VFS, while stdout (handle 1) stays special-cased. Expected output: `Read data: Warpine File Test Data`

---

## Module Structure

```
src/
  main.rs              Entry point, CLI/PM detection, event loop setup
  gui.rs               winit/softbuffer GUI, event handling, drawing
  api.rs               DosWrite/DosExit FFI bridge stubs
  font8x16.rs          8x16 bitmap font for text rendering
  lx/
    mod.rs             LX module re-exports
    header.rs          LX binary format structures and parsing
    lx.rs              LX file orchestration (open, parse, fixups)
  loader/
    mod.rs             Loader struct, SharedState, KVM setup, VMEXIT loop, API dispatch
    constants.rs       Named constants (addresses, message IDs, mock handles)
    mutex_ext.rs       MutexExt trait (poison-recovering lock)
    managers.rs        MemoryManager, HandleManager, HDirManager, ResourceManager
    ipc.rs             Semaphores (event, mutex, muxwait) and queues
    pm_types.rs        PM data types (windows, classes, presentation spaces, WindowManager)
    guest_mem.rs       Guest memory read/write/translate helpers
    doscalls.rs        DOSCALLS API implementations (~40 functions)
    pm_win.rs          PMWIN API implementations (~50 ordinals)
    pm_gpi.rs          PMGPI API implementations (8 ordinals)
    console.rs         VioManager: screen buffer, cursor state, raw mode, ANSI output
    viocalls.rs        VIOCALLS API implementations (VioWrtTTY, VioScrollUp, etc.)
    kbdcalls.rs        KBDCALLS API implementations (KbdCharIn, KbdStringIn, etc.)
    stubs.rs           Stub handlers for unimplemented/low-priority APIs
    process.rs         ProcessManager: DosExecPgm, DosWaitChild, directory tracking
    vfs.rs             VfsBackend trait, DriveManager, Os2Error, OS/2 data types, handle types
    vfs_hostdir.rs     HostDirBackend: HPFS-on-host-directory VfsBackend implementation
```

---

## Adding a New API

To add a new OS/2 API call:

1. **Find the ordinal** — Look up the API's ordinal number in `doc/os2_ordinals.md` or OS/2 documentation.

2. **Add the handler method** — In the appropriate file (`doscalls.rs`, `pm_win.rs`, or `pm_gpi.rs`), add a method on `impl Loader`:
   ```rust
   pub fn dos_my_new_api(&self, param1: u32, param2: u32) -> u32 {
       // Read additional args from guest memory if needed
       // Implement the API logic
       // Return the OS/2 error code (0 = NO_ERROR)
       0
   }
   ```

3. **Wire up the dispatch** — In the dispatch function (`handle_api_call()` in `mod.rs` for DOSCALLS, `handle_pmwin_call()` for PMWIN, `handle_pmgpi_call()` for PMGPI), add a match arm:
   ```rust
   ORDINAL_NUMBER => {
       let param1 = self.guest_read::<u32>(esp + 4).unwrap_or(0);
       let param2 = self.guest_read::<u32>(esp + 8).unwrap_or(0);
       ApiResult::Normal(self.dos_my_new_api(param1, param2))
   }
   ```

4. **Add a named constant** — If the ordinal is used elsewhere, add it to `constants.rs`.

5. **Test** — Build with `cargo build`, run with an OS/2 binary that uses the API, verify with `RUST_LOG=debug`.

Key conventions:
- Arguments follow `_System` calling convention: first arg at `ESP+4`, second at `ESP+8`, etc.
- Return OS/2 error codes in EAX (0 = success). Common codes: 2 = FILE_NOT_FOUND, 5 = ACCESS_DENIED, 6 = INVALID_HANDLE, 87 = INVALID_PARAMETER.
- For PM APIs that need to invoke guest code, return `ApiResult::Callback` instead of `ApiResult::Normal`.
- Use `guest_read`/`guest_write` for all guest memory access — never dereference raw guest pointers without bounds checking.

---

## Debugging

Warpine uses the `log` crate with `env_logger`. Set the `RUST_LOG` environment variable to control verbosity:

```bash
# Full debug output — shows every API call, arguments, and return values
RUST_LOG=debug cargo run -- samples/pm_demo/pm_demo.exe

# Info level — shows high-level milestones (parse, load, entry point)
RUST_LOG=info cargo run -- samples/pm_demo/pm_demo.exe

# Filter to a specific module
RUST_LOG=warpine::loader=debug cargo run -- samples/hello/hello.exe
RUST_LOG=warpine::gui=debug cargo run -- samples/pm_demo/pm_demo.exe
```

### What debug output shows

At **debug** level, each intercepted API call is logged with its vCPU ID, function name, arguments, and return value. For example:

```
[VCPU 0] DosOpen("CONFIG.SYS", ...) = 0
[VCPU 0] WinSetWindowPos hwnd=4096 x=100 y=100 cx=400 cy=300 fl=0x002B
[GUI] Resized window 4096 to 400x300
[GUI] Moved window 4096 to (100, 100)
```

This is the primary way to diagnose issues — if an OS/2 app misbehaves, the debug log shows exactly which API calls were made, in what order, and what was returned.

### Common debugging scenarios

| Symptom | What to check |
|---|---|
| App crashes immediately | Look for unhandled ordinals: `WARN ... unimplemented DOSCALLS ordinal ...` |
| Window doesn't appear | Check for `WinCreateStdWindow`, `WinSetWindowPos` with `SWP_SHOW`, and `[GUI] Created window` in logs |
| App hangs | Check if a blocking call (`WinGetMsg`, `DosWaitEventSem`, `DosReadQueue`) is waiting indefinitely |
| Drawing issues | Look for `GpiSetColor`, `GpiBox`, `GpiLine`, `GpiCharStringAt` calls and verify coordinates |
| Import resolution errors | Look for `WARN ... unresolved import` during loading |

### Rust backtrace

For panics, enable the full backtrace:

```bash
RUST_BACKTRACE=1 cargo run -- samples/pm_demo/pm_demo.exe
RUST_BACKTRACE=full cargo run -- samples/pm_demo/pm_demo.exe  # with full symbol info
```

---

## Testing

### Unit tests

Run all unit tests with:

```bash
cargo test
```

The test suite covers three areas:

| Module | What's tested |
|---|---|
| `src/lx/header.rs` | LX header parsing, object table entry parsing, resource entry parsing |
| `src/lx.rs` | MZ validation, LX signature detection, rejection of malformed binaries (excessive object/page counts, invalid EIP object, invalid page offset shift), parsing of a real `hello.exe` |
| `src/loader/managers.rs` | MemoryManager allocation, 4KB alignment, free-list reuse, top-of-heap coalescing, overflow/limit rejection, ResourceManager find operations, SharedMemManager name lookup |
| `src/loader/console.rs` | VioManager screen buffer operations (scroll up/down, read cell str, defaults), key mapping (enter, printable, backspace → OS/2 charcode/scancode) |
| `src/loader/stubs.rs` | DosEditName wildcard pattern replacement, DosQuerySysInfo QSV_* constant validation |
| `src/loader/process.rs` | ProcessManager child tracking, wait-any semantics |
| `src/gui.rs` | Y-coordinate flipping, rectangle rendering (filled/outlined), line drawing (horizontal/vertical/diagonal), text rendering pixel output and orientation |

### Integration testing with sample apps

Sample OS/2 binaries in `samples/` serve as integration tests. Build them with Open Watcom:

```bash
./vendor/setup_watcom.sh
make -C samples/<name>
```

Run each sample to verify specific subsystems:

```bash
# Core: stdout, DosExit
cargo run -- samples/hello/hello.exe

# Memory: DosAllocMem, DosFreeMem
cargo run -- samples/alloc_test/alloc_test.exe

# File I/O: DosOpen, DosRead, DosWrite, DosClose, DosSetFilePtr, DosDelete
cargo run -- samples/file_test/file_test.exe

# Directory: DosFindFirst, DosFindNext, DosFindClose
cargo run -- samples/find_test/find_test.exe

# Filesystem: DosCreateDir, DosDeleteDir, DosMove, DosQueryPathInfo
cargo run -- samples/fs_ops_test/fs_ops_test.exe

# Threading: DosCreateThread, DosSleep, DosWaitThread
cargo run -- samples/thread_test/thread_test.exe

# Pipes: DosCreatePipe, cross-thread DosRead/DosWrite
cargo run -- samples/pipe_test/pipe_test.exe

# Semaphores: DosCreateEventSem, DosPostEventSem, DosWaitEventSem
cargo run -- samples/ipc_test/ipc_test.exe

# Mutexes: DosCreateMutexSem, DosRequestMutexSem, DosReleaseMutexSem
cargo run -- samples/mutex_test/mutex_test.exe

# MuxWait: DosCreateMuxWaitSem, DosWaitMuxWaitSem
cargo run -- samples/muxwait_test/muxwait_test.exe

# Queues: DosCreateQueue, DosWriteQueue, DosReadQueue
cargo run -- samples/queue_test/queue_test.exe

# PM GUI: WinCreateStdWindow, message loop, WinSetWindowPos
cargo run -- samples/pm_hello/pm_hello.exe
cargo run -- samples/pm_demo/pm_demo.exe

# PM drawing: GpiBox, GpiLine, GpiCharStringAt, GpiSetColor
cargo run -- samples/shapes/shapes.exe
```

# Text-mode: 4OS2 command shell (interactive)
cd samples/4os2 && ./fetch_source.sh && make && cd ../..
cargo run -- samples/4os2/4os2.exe
# Should show banner, [c:\] prompt, accept commands (ver, set, exit)
```

CLI samples should print output and exit with code 0. PM samples should open a window and respond to close. 4OS2 should boot to an interactive prompt. Use `RUST_LOG=debug` to inspect API call traces if a sample misbehaves.
