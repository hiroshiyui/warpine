# Warpine Developer Guide

Warpine is an OS/2 compatibility layer for Linux. It loads 32-bit OS/2 executables (LX format) and runs them natively using KVM hardware virtualization — analogous to WINE for Windows, but targeting OS/2 instead.

This guide introduces the internals of Warpine and the OS/2 concepts it emulates.

---

## Table of Contents

1. [Background: OS/2 and LX Executables](#background-os2-and-lx-executables)
2. [Architecture Overview](#architecture-overview)
3. [LX Format Parser](#lx-format-parser)
4. [NE Format Parser](#ne-format-parser)
5. [KVM Virtualization Engine](#kvm-virtualization-engine)
6. [Guest Memory Layout](#guest-memory-layout)
7. [API Thunking Mechanism](#api-thunking-mechanism)
8. [OS/2 API Emulation](#os2-api-emulation) (includes MMPM/2 audio subsystem)
9. [Threading Model](#threading-model)
10. [IPC: Semaphores and Queues](#ipc-semaphores-and-queues)
11. [Presentation Manager (GUI)](#presentation-manager-gui)
12. [PM Callback Mechanism](#pm-callback-mechanism)
13. [Text-Mode Console Subsystem](#text-mode-console-subsystem)
14. [NLS (National Language Support)](#nls-national-language-support)
15. [4OS2 Compatibility](#4os2-compatibility)
16. [Filesystem I/O Design](#filesystem-io-design)
17. [Module Structure](#module-structure)
18. [Adding a New API](#adding-a-new-api)
19. [Developer Tooling](#developer-tooling) (GDB stub, crash dump, API ring buffer)
20. [Appendix: Development Phases](#appendix-development-phases)
21. [Rust Guest Toolchain](#rust-guest-toolchain)

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

For **CLI applications**, the vCPU runs on the main thread. For **PM (GUI) applications**, the SDL event loop runs on the main thread (as required by most windowing systems), and the vCPU runs on a worker thread.

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

## NE Format Parser

**Module:** `src/ne/` (`header.rs` for binary structures, `ne.rs` for orchestration)

The NE (New Executable) format is used by OS/2 1.x 16-bit applications. Warpine now supports both LX (32-bit) and NE (16-bit) execution. The NE parser in `src/ne/` provides the binary structures; `src/loader/ne_exec.rs` provides the full NE loader and 16-bit execution path.

The parser handles:

- **MZ stub detection** — Same as LX: reads `e_lfanew` to find the NE header.
- **NE header** — Magic (`NE`), linker version, segment/resource/module table offsets and counts, entry table offset, flags, initial CS/IP and SS/SP.
- **Segment table** — Each `NeSegmentEntry` describes a 16-bit segment: file offset, size, flags (code/data/movable/preload), and minimum allocation size.
- **Relocation records** — Per-segment relocation entries with source type (byte/word/far) and targets (internal, imported ordinal, imported name, OS fixup).
- **Export table** — Entry table and exported name table mapping names to entry points.
- **Import table** — Module reference table listing names of imported DLLs.
- **Resource table** — Resource type/name table mapping resource IDs to file offsets.

The parser has 16 unit tests in `src/ne/`. NE loading and execution is implemented in `src/loader/ne_exec.rs` — see [§20 Appendix: Development Phases](#appendix-development-phases) for details.

---

## KVM Virtualization Engine

Warpine uses Linux KVM (Kernel-based Virtual Machine) for hardware-accelerated x86 emulation. The setup in `Loader::new()`:

1. **Create VM** — Open `/dev/kvm`, create a VM file descriptor.
2. **Allocate guest memory** — `mmap` 128 MB of anonymous memory, register it as a KVM memory region at guest physical address 0.
3. **Set up GDT** — A Global Descriptor Table (6150 entries, ~49 KB at `GDT_BASE` 0x80000) is written into guest memory:
   - GDT[0]: null descriptor
   - GDT[1] (selector 0x08): 32-bit code segment — base 0, limit 4 GB, execute/read
   - GDT[2] (selector 0x10): 32-bit data segment — base 0, limit 4 GB, read/write
   - GDT[3] (selector 0x18): FS data segment (TIB pointer)
   - GDT[4] (selector 0x20): 16-bit data alias — base 0, limit 0xFFFF, DPL=0 (used for SS in 16-bit mode)
   - GDT[5] (selector 0x28): 16-bit code alias — base 0, limit 0xFFFF (Far16 thunk entry: `JMP FAR 0x0028:offset`)
   - GDT[6..4101] (selectors 0x30, 0x38, ...): 4096 tiled 16-bit **data** descriptors — one per 64 KB, DPL=2, read/write; enable 16:16 (selector:offset) addressing for `DosFlatToSel`/`DosSelToFlat` and NE segment DS/ES loads
   - GDT[4102..6149] (selectors 0x8030+): 2048 tiled 16-bit **code** descriptors — same bases as data tiles, execute/read, DPL=0; required by CALL FAR fixups targeting executable NE objects and by `DosFlatToSel` for code addresses
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
| `0x01000000` (`MAGIC_API_BASE`) | API thunk stubs (12288 bytes of INT 3 instructions) |
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

During loading, `setup_stubs()` fills a 12288-byte region at `MAGIC_API_BASE` with INT 3 (0xCC) instructions. When the LX loader encounters an import fixup for a known module, `resolve_import()` maps it to a specific address:

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
| MDM | `MDM_BASE` (10240) | `MAGIC_API_BASE + 10240 + ordinal` | 10240–12287 |

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
| **Audio** | DosBeep (real sine-wave tones via MMPM/2 beep_tone) |
| **Stubs** | DosError, DosSetMaxFH, DosSetExceptionHandler, DosLoadModule, DosStartSession, and others |

All blocking operations (DosSleep, DosWaitEventSem, DosRequestMutexSem, DosWaitMuxWaitSem, DosReadQueue, DosWaitThread) check the `exit_requested` flag in 100 ms intervals to ensure clean shutdown.

### MMPM/2 (src/loader/mmpm.rs)

Multimedia Presentation Manager/2 APIs. Dispatched at `MDM_BASE` (10240) + ordinal:

| Ordinal | Function | Notes |
|---|---|---|
| 1 | `mciSendCommand` | Structured MCI command (MCI_OPEN, MCI_CLOSE, MCI_PLAY, MCI_STOP, etc.) |
| 2 | `mciSendString` | Text-string MCI command (`"open waveaudio alias wave1"`) |
| 3 | `mciFreeBlock` | Free an MCI-allocated block |
| 4 | `mciGetLastError` | Retrieve the last MCI error string |

`DosBeep` (DOSCALLS ordinal 286) is also routed through `mmpm::beep_tone()`, which generates a sine-wave tone at the requested frequency and blocks for its duration.

`MmpmManager` tracks open waveaudio devices (with their alias, decoded PCM, position, volume and playing voice) and the last error. Constants and parameter-block layouts (`MCI_OPEN_PARMS`, `MCI_PLAY_PARMS`, `MCI_SET_PARMS`, `MCI_STATUS_PARMS`), message numbers, `MM_MCINOTIFY` (0x0500) with its notify codes, and the `MCIERR_*` codes (base 5000) follow the toolkit headers `<mcios2.h>` / `<meerror.h>`. WAV files are decoded in Rust (`parse_wav`: uncompressed 8/16/24/32-bit PCM and 32-bit float; compressed formats are refused).

**Audio output** (`src/gui/audio.rs`). SDL3 audio belongs to the main thread: in the safe sdl3-rs API the `Sdl` context is created there and `AudioSubsystem` / `AudioStream` are neither `Send` nor `Sync`. So the API threads never call SDL: they hand PCM data and a gain to `SharedState::audio` (`AudioService`) and get a voice number back. The main-thread loop (PM desktop or VGA text window) owns an `SdlAudio`, attached at renderer creation, before the guest starts; each loop pass drains the commands into SDL audio streams (which convert to the device format and apply the gain) and reports how much of each voice is still queued. `MCI_WAIT` and `DosBeep` block on the service until the voice ends, `MCI_NOTIFY` watcher threads wait on it with a cancel flag, and `MCI_SET` volume changes reach the playing voice. While voices play, the PM loop waits at most `AUDIO_PUMP_INTERVAL` (10 ms). Without a main-thread loop (`WARPINE_HEADLESS=1`, or a CLI app whose stdout is not a terminal) no backend is attached and audio is silent.

### PMWIN (src/loader/pm_win.rs)

Window management APIs — approximately 50 ordinals including WinInitialize, WinCreateMsgQueue, WinRegisterClass, WinCreateStdWindow, WinGetMsg, WinDispatchMsg, WinSendMsg, WinPostMsg, WinDefWindowProc, WinBeginPaint, WinEndPaint, WinMessageBox, WinStartTimer, WinStopTimer, clipboard operations, and dialog support.

#### Built-in window class atoms (MAKEINTATOM encoding)

OS/2 PM identifies built-in window classes (`WC_BUTTON`, `WC_STATIC`, etc.) via numeric atoms encoded as **`MAKEINTATOM(n) = 0xFFFF0000 | n`**, *not* as bare integers.  The Open Watcom `<pmwin.h>` header defines them as:

```c
#define WC_FRAME        ((PSZ)0xFFFF0001L)
#define WC_COMBOBOX     ((PSZ)0xFFFF0002L)
#define WC_BUTTON       ((PSZ)0xFFFF0003L)
#define WC_MENU         ((PSZ)0xFFFF0004L)
#define WC_STATIC       ((PSZ)0xFFFF0005L)
#define WC_ENTRYFIELD   ((PSZ)0xFFFF0006L)
#define WC_LISTBOX      ((PSZ)0xFFFF0007L)
#define WC_SCROLLBAR    ((PSZ)0xFFFF0008L)
#define WC_TITLEBAR     ((PSZ)0xFFFF0009L)
#define WC_MLE          ((PSZ)0xFFFF000AL)
#define WC_SPINBUTTON   ((PSZ)0xFFFF000FL)
#define WC_CONTAINER    ((PSZ)0xFFFF0025L)
#define WC_NOTEBOOK     ((PSZ)0xFFFF0028L)
```

The `pszClass` argument to `WinCreateWindow` (ordinal 909) arrives as a 32-bit guest integer.  `resolve_class_atom()` in `pm_win.rs` maps these MAKEINTATOM values to canonical Rust strings (`"WC_BUTTON"`, `"WC_LISTBOX"`, etc.) which are stored in `OS2Window::class_name`.  The corresponding `WC_*_ATOM` constants in `constants.rs` must always use the `0xFFFF000x` format.

> **Pitfall:** Bare integers (`WC_BUTTON = 3`) look plausible but are *wrong* — the compiler emits `0xFFFF0003` in the binary.  Using bare integers causes `read_guest_string` to be called on an invalid address, returning an empty class name, and making the renderer unable to identify or draw any built-in control.

`WinSendMsg` routes to `dispatch_builtin_control()` (in `mod.rs`) for any window whose `pfn_wp == 0` (i.e., a built-in class with no user-registered procedure).  Currently handled messages:

| Message | Value | Action |
|---|---|---|
| `LM_INSERTITEM` | `0x0161` | Appends or inserts a string into `OS2Window::listbox_items` |
| `LM_QUERYITEMCOUNT` | `0x0160` | Returns `listbox_items.len()` |

**Note on `LM_*` values:** The Open Watcom headers define `LM_INSERTITEM = 0x0161` and `LM_QUERYITEMCOUNT = 0x0160`.  Older SDK documentation and some headers show `0x0150`/`0x0151` — those are incorrect for OS/2 Warp 4.

### PMGPI (src/loader/pm_gpi.rs)

Graphics Primitive Interface, 64 ordinals (see the reference manual for the list): presentation spaces, colours and mixes, lines, boxes, arcs, area patterns, line widths, text and fonts, read-back, colour tables, and — in `pm_bitmap.rs` — device contexts, bitmaps and bit-block transfers. Arc geometry and the pattern table are in `pm_shapes.rs`. How drawing reaches the screen: "Rendering Pipeline" below.

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

**Modules:** `src/gui/renderer.rs`, `src/gui/sdl_renderer.rs`, `src/gui/headless.rs`, `src/loader/pm_win.rs`, `src/loader/pm_gpi.rs`, `src/loader/pm_types.rs`

The Presentation Manager (PM) is OS/2's GUI subsystem. Warpine implements it using **SDL** (window management, event handling, and hardware-accelerated framebuffer rendering via streaming textures).

### Three Execution Paths

`main.rs` selects an execution mode based on the executable's imports and environment:

- **CLI apps (SDL text window)** — default for apps that do not import `PMWIN`. The vCPU runs on a worker thread. `SdlTextRenderer` runs `run_text_loop()` on the main thread (required by SDL), rendering a 640×400 VGA text window.
- **CLI apps (terminal / headless)** — `WARPINE_HEADLESS=1`. The vCPU runs on the main thread; terminal ANSI output and raw-mode keyboard are used.
- **PM apps** — SDL event loop runs on the main thread. The vCPU is spawned on a worker thread. Drawing goes through shared per-frame surfaces (see Rendering Pipeline); the few requests only the GUI thread can carry out — host clipboard, mouse capture, message boxes — go through a channel (`GUISender` / `GUIReceiver`).

### Window Lifecycle

1. **WinInitialize** — Returns a handle to the anchor block (HAB).
2. **WinCreateMsgQueue** — Creates a `PM_MsgQueue` and maps the calling thread to it.
3. **WinRegisterClass** — Stores a `WindowClass` with the guest's window procedure address.
4. **WinCreateStdWindow** — Creates frame and client `OS2Window` entries in the `WindowManager` and the frame's drawing surface (from then on the compositor shows the frame), and returns the frame HWND.
5. **WinGetMsg** — Blocks on the message queue condvar until a message is available. Returns FALSE on WM_QUIT (ending the message loop).
6. **WinDispatchMsg** — Invokes the guest's window procedure via the callback mechanism (see below).
7. **WinDestroyMsgQueue / WinTerminate** — Cleanup.

### Rendering Pipeline

Drawing in PM uses Presentation Spaces (HPS):

1. **WinBeginPaint** — Creates or retrieves an HPS for the window.
2. **GpiSetColor / GpiSetMix / GpiMove / GpiLine / GpiBox / GpiSetPel / GpiCharStringAt** — Update the presentation space state and draw, on the calling thread, into the surface of the frame the window belongs to.
3. **WinEndPaint** — Repaints the window's built-in controls on top and marks the frame dirty.

The GUI thread (`run_pm_loop` in `src/gui/renderer.rs`, backend `SdlRenderer` in `src/gui/sdl_renderer.rs`) works out where each frame goes and what changed, and a *compositor* (`src/gui/sdl_renderer/compositor.rs`) puts the frame surfaces and their chrome on the host window — see Compositing below.

**Surfaces.** `src/gui/surface.rs`: each frame or dialog has a `Surface` (its client-area pixels, 0x00RRGGBB, top row first) in `WindowManager::surfaces`, shared as `Arc<Mutex<Surface>>`. API threads draw into it directly with `DrawOp`s — `Box`, `Line`, `Text` (system font, line-box bottom at `y`), `TextFont` (GPI, baseline at `y`), `Pel`, `Clear` — through `Loader::draw` (or a `Painter` for built-in controls), in PM coordinates (bottom-left origin, corners inclusive), clipped to the surface.

- *Read-back and mixes.* Because pixels exist as soon as the call returns, `GpiQueryPel` reads them back, and drawing honours the presentation space's foreground mix (`Mix`, the 16 `FM_*` raster operations of `GpiSetMix`: overpaint, XOR, invert, AND, OR…). Overpainted text is anti-aliased; text drawn with another mix uses whole pels (coverage ≥ 50 %) so XOR text can be undone. `GpiQueryPel` returns the default colour-table index of the pel when one matches (`CLR_BLACK` for black) and the RGB value otherwise, and `CLR_NOINDEX` outside the surface (colour tables and the background mix: see below).
- *Locking.* `Loader::draw` takes the window-manager lock, gets or creates the surface (`WindowManager::frame_surface`; dialogs get theirs on first draw), locks it, marks the frame dirty, releases the window-manager lock and draws. Marking dirty while the surface is locked means a composite that picks up the mark waits for the drawing to finish. The window-manager lock may be held while locking a surface, never the reverse, so the API threads and the compositor cannot deadlock.
- *Bitmaps and memory presentation spaces* (`src/loader/pm_bitmap.rs`). A presentation space draws into its *draw target* (`WindowManager::ps_draw_target`): the bitmap selected with `GpiSetBitmap` if any, else its window's frame. `DevOpenDC(OD_MEMORY)` → `GpiCreatePS(hdc, GPIA_ASSOC)` → `GpiCreateBitmap` → `GpiSetBitmap` gives an off-screen PS whose drawing lands in `WindowManager::bitmaps` (each a `Surface`, kept true colour; the requested depth is only reported back). `WinOpenWindowDC` + `GpiCreatePS(GPIA_ASSOC)` gives a window PS; `GpiAssociate` re-associates. Bitmap data (`GpiCreateBitmap(CBM_INIT)`, `GpiSetBitmapBits` / `GpiQueryBitmapBits`, `GpiDrawBits`, `GpiLoadBitmap` of an `RT_BITMAP` resource — a `BM` file or the first entry of a `BA` array) is decoded and encoded by the codec at the top of `pm_bitmap.rs`: uncompressed 1/4/8/24/32-bit, both `BITMAPINFOHEADER` (cbFix 12, RGB entries) and `BITMAPINFOHEADER2` (cbFix 16–64, RGB2 entries), rows bottom-up padded to 4 bytes. Run-length encoded data (`BCA_RLE8` at 8 bits, `BCA_RLE4` at 4, `BCA_RLE24` at 24; `BCA_HUFFMAN1D` is refused) is decoded by `decode_rows` / `decode_rle` from `cbImage` bytes (required, and capped by `max_rle_len`): runs, end of row, end of bitmap, delta, and literal runs padded to 16 bits; pels a delta or an early end of row skips keep their value (`put_rows`); runs past the row end are cut off and a truncated stream stops. `GpiQueryBitmapBits` always returns uncompressed data and says so in the header. `GpiQueryDeviceBitmapFormats` lists (1, 24), (1, 1), (1, 4), (1, 8), (1, 32), and `DevQueryCaps(CAPS_BITMAP_FORMATS)` their number. `GpiQueryBitmapBits` writes the size and a standard colour table (black/white, the 16 VGA colours, or those plus a colour cube and grey ramp) into the caller's header.
- *Bit-block transfers.* `GpiBitBlt` (2, 3 or 4 points; device coordinates, rectangles exclusive at the top and right), `GpiWCBitBlt` and `GpiDrawBits` (target inclusive), and `WinDrawBitmap` (`DBM_STRETCH`, `DBM_INVERT`, `DBM_XOR`) share `Loader::blit`: the target is clipped to its surface, each target pel samples the source by nearest pel (stretching / compressing), and `DrawOp::Blit` applies a ternary raster operation (`rop3`: bit `P<<2|S<<1|D` of the `ROP_*` code) with the PS colour as the pattern. The source is read completely before the target is written, so overlapping copies within one surface are safe, and only one surface is locked at a time.
- *Colour tables.* `PresentationSpace` keeps colour values as set (`color_attr`, reported by `GpiQueryColor`) and resolved (`color`). `GpiCreateLogColorTable` switches to RGB values (`LCOLF_RGB`), loads entries (`LCOLF_CONSECRGB`, `LCOLF_INDRGB`) over the default table, or resets it; `GpiQueryRGBColor`, `GpiQueryColorIndex` (nearest entry) and `GpiQueryLogColorTable` read it, and `GpiQueryPel` answers in RGB in RGB mode, else with the table index of an exactly matching colour (RGB when none matches).
- *Shapes.* `DrawOp::Shape` rasterises a path (open or closed) with an `AreaFill` (an 8 × 8 `Pattern` with foreground mix and optional background mix) and a `Stroke` (colour, mix, 16-pel style, width up to `MAX_STROKE_WIDTH` = 32 pels, a square brush centred on the path). It marks the pels in a mask over the shape's area first — inside (pel centres between edge crossings, even-odd) and outline (Bresenham per segment, the style running on across segments) — and then paints each pel once, so XOR shapes can be undone and a filled outlined shape does not mix its boundary twice. A closed rectangle covers exactly the pels of `DrawOp::Box`; points are capped at `MAX_SHAPE_POINTS` and clamped to ±2²⁴.
- *Arcs* (`pm_shapes.rs`). Arcs lie on the unit circle transformed by the arc parameters (`ARCPARAMS` P, Q, R, S: x = xc + m(P cos t + R sin t), y = yc + m(S cos t + Q sin t); default a circle) and scaled by the FIXED multiplier m, generated as polylines with a point about every 2 pels (16–2048 per full turn). `GpiFullArc` draws the closed ellipse around the current position (`DRO_OUTLINE` / `DRO_FILL` / `DRO_OUTLINEFILL`; position unchanged); `GpiPartialArc` a line from the current position to the arc's start and the arc (degrees, counter-clockwise; position at its end); `GpiPointArc` the arc from the current position through two points, found as a circle in the parameters' unit space (collinear points or singular parameters give the straight path — Warpine's choice).
- *Patterns and line widths.* `GpiSetPattern` (`PATSYM_*`; the 8 × 8 bits are Warpine's own, the PM reference only pictures them: `DENSE1`–`8` ordered dither from 7/8 to 1/16, lines 4 pels apart for `VERT` / `HORIZ` / `DIAG*`, `DIAG2`/`4` doubled) and `GpiSetPatternRefPoint` give the `Pattern` of box and arc fills (set pels: foreground colour and mix; clear pels: background colour with the background mix, left alone by default) and the P operand of `GpiBitBlt` raster operations. `GpiSetLineWidth` (FIXED multiple of normal) rounds to whole pels (`cosmetic_pels`) for lines, box outlines and arcs; `GpiSetLineWidthGeom` is stored only (it applies to stroked paths, which Warpine does not have). Plain boxes and 1-pel solid lines keep the cheaper `DrawOp::Box` / `DrawOp::Line`; patterned fills and wide or styled box outlines go through `DrawOp::Shape`.
- *Lines and text.* `GpiSetLineType` styles `GpiLine`, box outlines and arcs with a 16-pel mask (`DrawOp::Line::style`, `Stroke::style`; the dash lengths are Warpine's choice, `LINETYPE_INVISIBLE` draws nothing). A background mix other than leave-alone (`GpiSetBackMix`) fills a character string's box with the background colour before the text.
- *Presentation-space lifetime.* `WinBeginPaint` without a PS creates one that `WinEndPaint` frees, and reports the whole window as the update rectangle; `WinGetPS` / `WinReleasePS` pair (`PsOrigin`). `DevQueryCaps` describes a 24-bit display the size of the desktop at 96 dpi.
- *Untrusted coordinates.* Coordinates come from the guest. Lines are clipped to the surface (Liang–Barsky) before Bresenham runs, box corners are clamped to one pel outside it, text positions to ±2²⁴, and surfaces to `MAX_SURFACE_DIM` (8192) per side; `WinSetWindowPos` and `WinCreateWindow` bound window positions and sizes to ±`MAX_WINDOW_COORD` (2²⁰). Nothing a guest passes can overflow the arithmetic or make one call run for long.
- *Size.* A surface covers the frame below its title bar and menu bar. `WinSetWindowPos` (`SWP_SIZE`), attaching a menu and interactive resizing resize it with `WindowManager::resize_surface`; `Surface::resize` keeps the bottom-left part of the old content (PM's origin), so an interactive resize shows the old picture until the application repaints instead of a blank window. `WinDestroyWindow` drops the surface.
- *Tests.* In test builds a surface can record the operations applied to it (`Surface::record`), so tests check what was drawn as well as the pixels.

**Compositing.** `composite_and_present` snapshots the visible frames under the window-manager lock, brings the chrome cache up to date, lays the frames out (`place_frame`) and works out the damage; if anything changed it hands a `Scene` (frames back to front with their surfaces and chrome, the damage, the frames whose chrome changed, whether to redraw everything) to the `Compositor`:

- *Backends.* `SoftwareCompositor` keeps a desktop-sized buffer between frames, redraws only the damage into it and uploads only the damaged rectangles to one streaming texture on SDL's `software` renderer. `LayeredCompositor` keeps one texture per frame (client surface plus chrome, rendered at the origin) and one per open pull-down, and draws the desktop as textured quads with whichever SDL renderer the platform offers (OpenGL, Vulkan, Metal, Direct3D; `SDL_RENDER_DRIVER=gpu` selects the SDL_GPU-based one), with vsync (`SDL_RENDER_VSYNC`). A frame's texture is rendered in full (client and chrome) when it is new, its size or chrome changed, or after a full redraw. When only the client's pixels changed (`Surface::generation`, bumped by every `draw`, `put_row` and `resize`), just the changed rectangle is copied from the surface into the texture — clipped to the inside of the frame's border ring (`partial_upload`), since the client reaches the frame's bottom edge and the border is drawn over its last rows: each surface accumulates the bounds of what it was drawn (`Surface::take_changes`; a `DrawOp`'s bounds are computed before it is applied, text as full-width rows around its baseline), and the layered compositor takes them. Uploads go through `upload`, which checks that the rows SDL will read lie inside the source buffer (`upload_span`; sdl3-rs's `Texture::update` does not). Moving a window only changes where the quad is drawn. A client area the surface does not reach yet (a surface lagging a resize) is left out of the quads (`painted_parts`), so frames below show through as with the software backend. Frame textures are capped at `MAX_LAYER_DIM` (8192) per side.
- *Choice.* `WARPINE_COMPOSITOR=software|layered`; by default layered on a real display and software on the `dummy` / `offscreen` drivers, whose output the golden-image tests compare. If no renderer can be created for the layered backend it falls back to software. The choice is logged at info level. The `gui_layered_compositor_*` integration tests run the golden (including typing into controls, which takes the partial-upload path) and drag/resize checks with the layered backend (on SDL's software renderer, under the dummy driver); `dump` reads its back buffer with `SDL_RenderReadPixels`. A render-device or render-target reset redraws everything (all textures are recreated).
- *Measured.* On a real display (2026-11-16, release build, 25 scripted title-bar drags of `samples/pm_menu_dlg_test` in 5 s, frame statistics): software 1.56 ms per composite, 1.31 ms of it presenting — the software present pushes the whole desktop image to the window system; layered 0.25 ms on `opengl`, 0.23 ms on `vulkan`, 0.25 ms on `gpu` (SDL_GPU). The accelerated renderers are equivalent, so SDL's default pick is kept (no preferred `SDL_RENDER_DRIVER`). `samples/pm_bench` reports the same guest-side rates under both compositors, since drawing happens on the API threads either way.
- *Decision: GPI stays on the CPU.* The GPU only composites finished window images; everything drawn inside a window (GPI, `WinFillRect`, built-in controls) is rasterised on the CPU into the window's surface. GPI promises pixel-exact results (Bresenham lines, inclusive box corners, line-type dashes), bitwise mixes and 256 ternary raster operations that GPU blending does not express directly, and immediate read-back (`GpiQueryPel`, `GpiQueryBitmapBits`), which on a GPU would stall and copy on every call; its many small immediate-mode calls would also gain little from GPU draw calls. If a GPU path is needed beyond the SDL renderer (for example for OpenGL forwarding), use `SDL_GPU`, already part of SDL3, rather than adding `wgpu` or raw Vulkan via `ash` — the conclusion 5thPlanet reached in its ADR-0019 / ADR-0020.
- *Damage.* The windows marked dirty since the last composite (`WindowManager::dirty`, taken under the lock) are mapped to the frame that draws them (`owning_frame`: the window itself, the frame of a client, or the frame above a control). For each such frame, and for each frame whose extent (frame plus open pull-down) or chrome changed, both the old and the new extent are damaged; so is the old extent of a frame that was hidden or removed. `src/gui/damage.rs` clips the rectangles to the desktop and merges touching ones, collapsing to one bounding box past `MAX_DAMAGE_RECTS` (8). Start-up, a host resize and an `Exposed` event redraw the whole desktop (`force_full`).
- *Redraw (software backend).* Each damaged rectangle is filled with the desktop colour and the frames reaching into it are drawn back-to-front, clipped to it (`draw_placed_frame`): client surface, title bar, border, menu bar, pull-down. Frames are shown once they have a surface. Each surface is locked only while it is copied. A frame is skipped where an *opaque* frame above — one whose surface covers its whole client area — hides it. Pixels are copied a row slice at a time (`blit_clipped`, `fill_clipped`).
- *Chrome cache.* Title bars, menu bars and open pull-downs are rendered into per-frame pixmaps (`ChromeCache`) and re-rendered only when the title, width, flags, focus or menu change; the window state is compared with the cached copy under the lock without allocating.
- *Upload (software backend).* Only damaged rectangles are uploaded, with `Texture::update` straight from the desktop buffer (viewed as bytes with `bytemuck::cast_slice`). Textures of both backends are sdl3-rs's lifetime-checked `Texture<'static>`, made from a `TextureCreator` kept for the life of the process (`Box::leak`) — no `unsafe_textures`.
- *Safe SDL only.* The GUI module is `#![forbid(unsafe_code)]`: wake-ups are a zero-sized `WakeEvent` pushed with `EventSender::push_custom_event` (the sender is `Send`), mouse capture uses `MouseUtil::capture`, and SDL3's per-window text input is started explicitly for each window.
- *Text.* `font.rs` caches each character's resolved face, advance and coverage bitmap in a per-thread cache (`with_glyph`), so measuring and drawing text takes no lock.

**Event loop.** Each pass rearms the channel's wake-up, drains queued `GUIMessage`s, handles input, composites if due, then blocks in `PmRenderer::wait_for_work`:

- *Waking.* `GUISender::send` and `WindowManager::mark_dirty`/`mark_all_dirty` call `GuiWake::wake` (`src/gui/message.rs`), so a draw from the guest thread (every draw marks its frame dirty) or a window moved by `WinSetWindowPos` ends the wait. Wake-ups are coalesced: only the first after each rearm reaches the hook.
- *How it blocks.* With a real video driver, `SdlRenderer` waits in `SDL_WaitEventTimeout` and the wake hook pushes an SDL user event, so input and wake-ups end the same wait. SDL's `dummy` and `offscreen` drivers have no input to block on (SDL would poll inside its wait), so there the loop waits on the `GuiWake` condition variable instead (`driver_can_wait`).
- *How long.* `wait_timeout()` picks the earliest of: the next frame when a composite is owed (`FRAME_INTERVAL`, 16 ms), the next scripted GUI step, and `IDLE_WAIT` (100 ms), which bounds how long state changes that arrive without a wake-up — such as `exit_requested` — go unnoticed.
- *Coalesced composites.* Drawing, `WinEndPaint` and `WinUpdateWindow` only mark the frame dirty; `poll_events` composites at most once per `FRAME_INTERVAL`, so a paint cycle of many primitives costs one composite (or a few, if it spans frame intervals — drawing outside `WinBeginPaint` shows up too).
- *Clipboard.* The host clipboard is read once at start-up and again on SDL `ClipboardUpdate` and `FocusGained` events, not on every pass.

**Frame statistics.** `src/gui/frame_stats.rs` counts loop passes and composites (average and maximum composite time, present time, share of the desktop redrawn) and logs a summary every 5 s at debug level: `RUST_LOG=warpine::gui=debug`. Use it to measure rendering changes.

**Guest-side benchmark.** `samples/pm_bench` measures what an application sees: GPI calls issued per second inside a paint, `WinSendMsg` and `WinPostMsg` round trips, `WinInvalidateRect` → `WM_PAINT` cycles, and the `WM_TIMER` rate of a 10 ms timer, each timed with `DosQuerySysInfo(QSV_MS_COUNT)`. GPI calls rasterise on the calling thread, so its GPI figures include drawing the pixels (compositing is on the GUI thread; read it from the frame statistics). `samples/pm_pel_test` checks read-back and mix modes from the guest side (`GpiSetPel`/`GpiBox`/`GpiLine` → `GpiQueryPel`, XOR / invert / leave-alone); `samples/pm_bitmap_test` checks memory presentation spaces, bitmaps, `GpiBitBlt`, `GpiLoadBitmap` + `WinDrawBitmap`, colour tables and line types; `samples/pm_shapes_test` arcs, area patterns and line widths. To compare two builds without a display:

```bash
printf '4500 quit\n' > /tmp/bench.script
SDL_VIDEODRIVER=dummy WARPINE_GUI_SCRIPT=/tmp/bench.script \
  cargo run --release -- samples/pm_bench/pm_bench.exe
```

### Input Handling

The GUI thread translates SDL input events into OS/2 messages:
- Keyboard events → `WM_CHAR` in the CHRMSG layout (flags in the low word of `mp1`, repeat count in bits 16–23, scan code in bits 24–31; character in the low word of `mp2`, virtual key in the high word). Typed characters come from SDL `TextInput` events encoded in the active codepage (`text_input_wm_chars`), so shifted and non-ASCII characters arrive correctly; `KeyDown` carries only non-character keys and Ctrl/Alt chords.
- Mouse movement → `WM_MOUSEMOVE`
- Mouse buttons → `WM_BUTTON1DOWN` / `WM_BUTTON1UP`
- Window resize → `WM_SIZE`
- Close request → `WM_CLOSE`

OS/2 uses a bottom-left coordinate origin (Y increases upward), so Y coordinates are flipped during translation.

**Focus and built-in controls.** `WindowManager::focus_hwnd` is the PM focus window (`WinSetFocus` / `WinQueryFocus`); `key_target()` sends keys to it, falling back to the active frame's client. `route_button1()` delivers button-1 messages to the built-in control under the pointer (`control_at()`), otherwise to the client. `pm_controls.rs` handles those messages for `WC_ENTRYFIELD`, `WC_MLE` and `WC_LISTBOX` — click-to-focus, caret, typing, Backspace/Delete/arrows/Home/End, Enter and Up/Down in the MLE, list-box selection and scrolling — and notifies the owner with `WM_CONTROL` (`EN_CHANGE`, `MLN_CHANGE`, `LN_SELECT`). The editing itself is pure logic in `pm_edit.rs` (caret positions in characters, CRLF as one unit, click-x → caret index via `font::text_width`), which keeps it unit-testable without a window. Controls inside `WinDlgBox` dialogs still use the older `#Dialog` path and are not interactive yet.

---

### PM Typography (IBM Plex)

All Presentation Manager text is drawn by `src/gui/font.rs` from the IBM Plex
outline fonts vendored in `vendor/ibm-plex` (SIL OFL 1.1) and embedded in the
binary. The VIO text-mode console is separate and keeps its 8×16 bitmap font.

- **Faces.** IBM Plex Sans, Mono and Serif in Regular/Bold/Italic/Bold Italic,
  plus IBM Plex Sans JP, KR, TC and SC (Regular) for CJK. Faces are parsed
  lazily and zero-copy (`ab_glyph::FontRef`), so the large CJK files cost
  nothing until a character needs them.
- **`FontSpec { family, bold, italic, px }`.** `px` is the line height
  (ascent + descent). `FontSpec::SYSTEM` is Plex Sans at 16 px, matching the
  16-px rows the PM layout uses; title bars use `FontSpec::SYSTEM_BOLD`.
- **Face-name mapping.** `FontSpec::from_facename()` maps the FATTRS face name
  by case-insensitive substring: Courier / System Monospaced / System VIO →
  Mono; Tms Rmn / Times New Roman → Serif; anything else (Helv, WarpSans,
  System Proportional, unknown) → Sans.
- **Fallback.** Each character is looked up in the requested Latin face, then
  the CJK faces in an order chosen by the active codepage (CP932 → JP first,
  CP936 → SC, CP949 → KR, CP950 → TC), then the GNU Unifont bitmap tables.
  CJK glyphs are scaled to the Latin face's em size so mixed text lines up.
- **Coordinates.** `DrawOp::Text` positions system-font text by the
  bottom of its 16-px line box (used by built-in controls);
  `DrawOp::TextFont` positions text by its baseline, the GPI
  convention, and carries the `FontSpec`.
- **GPI.** `GpiCreateLogFont` registers a `FontSpec` per LCID in
  `PresentationSpace::log_fonts`; `GpiSetCharSet`, `GpiSetCharBox` and
  `GpiDeleteSetId` update the selection; `GpiQueryTextBox`,
  `GpiQueryFontMetrics` and the current-position advance after
  `GpiCharString` all use the selected font's real proportional metrics.

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

### WM_CREATE Dispatch (WinCreateStdWindow)

In real OS/2 PM, `WinCreateStdWindow` dispatches `WM_CREATE` **synchronously** to the client window procedure before returning the frame handle to the caller.  Warpine replicates this via a dedicated `ApiResult` variant:

1. `WinCreateStdWindow` (ordinal 908) creates the frame and client `OS2Window` entries, then — if the registered class has a non-zero `pfn_wp` — returns `ApiResult::WmCreateCallback { wnd_proc, hwnd: h_client, h_frame }`.
2. The VMEXIT loop saves the original return address (from `WinCreateStdWindow`'s call site) as `FrameKind::WmCreate { h_frame }` on the callback stack, then pushes `WM_CREATE` arguments onto the guest stack and jumps to `wnd_proc`.
3. The WndProc executes (typically creating child controls via `WinCreateWindow`), then returns to `CALLBACK_RET_TRAP`.
4. `FrameKind::WmCreate` causes the trap handler to write `h_frame` into guest EAX — the correct return value of `WinCreateStdWindow`.

Applications that create their child controls inside `WM_CREATE` (the standard OS/2 pattern) require this synchronous dispatch.  Without it, `WinCreateStdWindow` returns before any controls exist.

### Menu System (WinLoadMenu / WinCreateMenu / FCF_MENU)

**Binary MENUTEMPLATE format (RT_MENU = 4):** An 8-byte header (`u16 version`, `u16 codepage`, `u16 reserved`, `u16 cbInfo`) followed by recursive item records. Each item:

| Field | Type | Notes |
|---|---|---|
| `afStyle` | `u16` | Menu item style flags. **Bit 15 (`MIS_END`) marks the last item at this nesting level** — NOT a separate sentinel record. Strip `MIS_END` (`style & !MIS_END`) to get the real style. |
| `afAttribute` | `u16` | Menu item attribute flags (MIA_CHECKED, MIA_DISABLED, …) |
| `id` | `u16` | Command ID posted as WM_COMMAND mp1 |
| `text` | NUL-terminated `[u8]` | Absent for `MIS_SEPARATOR` items |
| children | recursive | Present only for `MIS_SUBMENU` items; terminated by the next item with `MIS_END` |

`parse_menu_template()` in `pm_win.rs` builds a `Vec<MenuItem>` tree. `WinLoadMenu` registers a menu pseudo-window carrying this tree and attaches it to the frame through the internal `attach_menu_to_frame()` helper, which stores the menu hwnd in `OS2Window::menu_hwnd`, re-lays out the frame and returns the previous handle; `WinCreateStdWindow` does the same for `FCF_MENU`. (PMWIN has no `WinSetMenu` export.)

### Dialog System (WinDlgBox / DlgRunLoop)

**Binary DLGTEMPLATE format (RT_DIALOG = 5):**

```
[8-byte header: u16 version, u16 codepage, u16 reserved, u16 cbInfo]
[cbInfo padding bytes]
i16  x, y, cx, cy       -- dialog position/size
u16  reserved
u16  cItems             -- number of child controls
[NUL-str: presparams]   -- presentation parameters (usually empty)
[NUL-str: classname]    -- dialog frame class (usually empty)
[NUL-str: title]        -- dialog title bar text
[cItems × DlgItemInfo]:
  i16  x, y, cx, cy
  u16  id
  u16  reserved
  u32  flStyle
  [NUL-str: presparams]
  [NUL-str: classname]  -- control class (e.g. "#3" = WC_BUTTON)
  [NUL-str: text]
```

`parse_dlg_template()` returns the frame geometry, title, and a `Vec<DlgItemInfo>`. `create_dialog_from_template()` calls `WinCreateStdWindow` + `WinCreateWindow` for each item.

**DlgRunLoop modal loop:** When `WinDlgBox` or `WinProcessDlg` needs a modal dialog, they return `ApiResult::DlgRunLoop { dlg_proc, hwnd_dlg, hmq }`. The VMEXIT loop handles this via `setup_dlg_dispatch()` — a free function that mirrors `setup_exception_dispatch()`:

1. Saves the caller's RIP/RSP.
2. Blocks on the `MessageQueue` condvar until a message arrives or the dialog is dismissed.
3. If dismissed (`OS2Window::dialog_dismissed == true`): restores saved RIP/RSP, writes `dialog_result` to guest EAX, returns `false` (caller falls through).
4. Otherwise: pushes a `FrameKind::DlgRunLoop` frame, sets guest registers to invoke `dlg_proc(hwnd, msg, mp1, mp2)`, returns `true` (caller does `continue`).
5. At `CALLBACK_RET_TRAP`, the `DlgRunLoop` frame handler checks dismissal again and either returns to caller or calls `setup_dlg_dispatch()` again for the next message.

`WinDismissDlg` sets `dialog_dismissed = true` and `dialog_result = result`, then calls `condvar.notify_one()` to wake the blocked loop.

---

## Text-Mode Console Subsystem

**Modules:** `src/loader/console.rs`, `src/loader/viocalls.rs`, `src/loader/kbdcalls.rs`, `src/gui/text_renderer.rs`

OS/2 text-mode applications use two subsystems: **VIOCALLS** (Video I/O) for screen output and **KBDCALLS** (Keyboard) for input.

### Rendering modes

Two rendering backends are available, selected at startup:

| Mode | Condition | Description |
|---|---|---|
| **SDL text window** | default | 640×400 SDL window; CP437 8×16 font; CGA 16-colour palette; blinking block/underline cursor |
| **Terminal (ANSI)** | `WARPINE_HEADLESS=1` | ANSI escape sequences on the host terminal; raw-mode termios keyboard |

Set `WARPINE_HEADLESS=1` for headless CI runs or when an SDL display is unavailable.

### VioManager (console.rs)

`VioManager` is the central state object for text-mode I/O. It maintains:

- A **screen buffer** of `(char, attribute)` cell pairs, row-major, 80×25 in SDL mode or terminal-detected dimensions otherwise.
- **Cursor position** (`cursor_row`, `cursor_col`) and visibility/shape (`cursor_visible`, `cursor_start`, `cursor_end` — VGA scan-line indices 0–15).
- An `sdl_mode` flag. When `true`, all ANSI terminal output is suppressed; the SDL renderer reads the buffer directly. `enable_raw_mode()` becomes a no-op; `enable_sdl2_mode()` locks dimensions to 80×25.
- `stdin_pending_lf` — for CR→CRLF translation across consecutive `DosRead` calls.
- `stdin_cooked_chars` — running count of printable chars echoed since the last CR/LF; used to bound backspace echo in cooked-mode `DosRead` (prevents erasing the shell prompt).

VIO write methods (`write_tty`, `write_char_str_att`, `write_n_cell`, `write_n_attr`, `scroll_up`, `scroll_down`) always update the screen buffer. In terminal mode they also emit ANSI escape sequences. In SDL mode only the buffer is updated; the renderer picks it up on the next frame.

#### VioScrollUp / VioScrollDn — OS/2 special case

`VioScrollUp` / `VioScrollDn` accept a `pCell` pointer to a 2-byte `(char, attr)` fill cell. **If `lines == 0`, OS/2 defines this as "clear the entire region"** (fill all rows from `top` to `bottom` with `fill_cell`). A null `pCell` defaults to `(' ', 0x07)`.

### SDL text renderer (gui/text_renderer.rs)

```
VgaTextBuffer              — per-frame snapshot of VioManager state
TextModeRenderer (trait)   — render_frame(buf, blink_on), poll_events(shared), frame_sleep()
  SdlTextRenderer         — 640×400 window; streaming ARGB8888 texture; XOR-invert cursor
  HeadlessTextRenderer     — no-op CI backend (counts frames)
run_text_loop(renderer, shared)
```

`run_text_loop` drives the main-thread event loop. Cursor blink uses **wall-clock time** (`Instant`): on for 500 ms, off for 500 ms — independent of frame rate.

**Cursor rendering** uses XOR-inversion of all RGB channels (`old ^ 0x00_FF_FF_FF`), guaranteeing the cursor bar is visible regardless of the underlying cell's fg/bg colour (including black-on-black `attr=0x00`). Degenerate cursor shapes (`cursor_start > cursor_end`) fall back to the default underline at scan lines 14–15.

**Glyph rendering**: `get_glyph_for_char(ch: char) -> [u8; 16]` does a binary search over the `UNIFONT_SBCS` table (7282 half-width 8×16 entries generated from GNU Unifont 17 by `build.rs`). Characters absent from Unifont return a blank glyph. For DBCS wide characters, `get_glyph_dbcs(ch: char) -> [u8; 32]` searches the `font_unifont_wide.bin` binary blob (49,804 16×16 entries). The old hand-crafted `src/font8x16.rs` was removed in Unifont Phase A.

**CGA palette**: `CGA_PALETTE: [u32; 16]` — ARGB8888 values indexed by 4-bit colour nibble (bits 3:0 for foreground, 7:4 for background in the attribute byte).

### Keyboard input

#### SDL mode

Keyboard events from `SdlTextRenderer::poll_events()` are mapped to OS/2 key codes and pushed into `SharedState::kbd_queue` (a `Mutex<VecDeque<KbdKeyInfo>>`); `kbd_cond` (a `Condvar`) is signalled so waiting threads wake up immediately.

```rust
pub struct KbdKeyInfo {
    pub ch:    u8,    // ASCII char code (0x00 for pure extended/navigation keys)
    pub scan:  u8,    // IBM PC Set-1 scan code
    pub state: u16,   // shift/ctrl/alt modifier bits
}
```

`KbdCharIn` (ordinal 4) dequeues from `kbd_queue`, blocking on `kbd_cond` with a 50 ms timeout so it can check `exit_requested`. `IO_NOWAIT` (`wait == 1`) returns immediately if the queue is empty.

`use_sdl_text: AtomicBool` in `SharedState` signals all subsystems (DosWrite, DosRead, KbdCharIn) to use the SDL paths.

#### Terminal mode (WARPINE_HEADLESS)

`KbdCharIn` uses `VioManager::enable_raw_mode()` (termios `VMIN=0, VTIME=1`) and polls `read_byte()` in a loop. `KbdStringIn` provides line-buffered input with echo for simple applications.

### VioSetCurType / VioGetCurType

`VioSetCurType` (ordinal 32) reads a `VIOCURSORINFO` struct from guest memory:

| Offset | Field | Notes |
|---|---|---|
| +0 | `yStart` (u16) | First scan line (0 = top of cell) |
| +2 | `cEnd` (u16) | Last scan line (15 = bottom for 8×16 font) |
| +4 | `cx` (u16) | Cursor width (0 = full) |
| +6 | `attr` (u16) | 0 = normal/visible; 0xFFFF = hidden |

`VioGetCurType` (ordinal 33) writes the current `VioManager` state back to the guest struct. Together they support the read-modify-write pattern common in OS/2 shell startup.

### DosRead / DosWrite in SDL mode

- **`DosWrite(fd=1)`** — routed through `VioManager::write_tty()` with attribute `0x07`. This keeps the screen buffer in sync so `VioGetCurPos` always returns the correct cursor column.
- **`DosRead(fd=0)` (cooked mode)** — reads from `kbd_queue`, echoes printable chars and CR+LF, translates Enter to CR+LF (`stdin_pending_lf` for the second byte). Backspace is only echoed and delivered if `stdin_cooked_chars > 0`; otherwise it is silently discarded to prevent erasing content written before the current input line (e.g. the shell prompt).

### Calling Conventions

VIO/KBD subsystem functions use **Pascal calling convention** (`_Far16 _Pascal` / `APIENTRY16`):
- Arguments pushed **left-to-right** (last argument at ESP+4)
- **Callee** cleans the stack (the loader adjusts ESP after return via `viocalls_arg_bytes()` / `kbdcalls_arg_bytes()`)

This is different from DOSCALLS which uses **_System** convention:
- Arguments pushed **right-to-left** (first argument at ESP+4)
- **Caller** cleans the stack

---

## NLS (National Language Support)

NLS functions (DosQueryCp, DosQueryCtryInfo, DosMapCase, DosGetDBCSEv) can be imported from either DOSCALLS (by ordinal) or the NLS DLL. Both paths use **_System** calling convention.

### NLS DLL Dispatch

NLS DLL imports are dispatched at `NLS_BASE` (7168) + ordinal:

| Ordinal | Function | Notes |
|---|---|---|
| 5 | DosQueryCp / COUNTRYINFO | Dual behavior: returns codepages for cb < 44, full COUNTRYINFO for cb >= 44 |
| 6 | DosQueryCtryInfo | Standard DosQueryCtryInfo with _System args |
| 7 | DosMapCase | Full case mapping: SBCS (CP437/850/852/866/1250–1258) and DBCS (CP932/936/949/950) |
| 8 | DosGetDBCSEv | Returns empty DBCS table (Western locales) |

### Watcom CRT NLS Caching

The Watcom C runtime wraps DosQueryCtryInfo with a caching layer:

1. During CRT init, calls **NLS ordinal 6** with `cb=12` (first 3 ULONGs: country, codepage, fsDateFmt)
2. When user code calls DosQueryCtryInfo, the wrapper calls **NLS ordinal 5** with `cb=44` (full COUNTRYINFO size) to retrieve complete locale data
3. The wrapper caches the result and returns it without calling NLS again

This is why NLS ordinal 5 must return full COUNTRYINFO when `cb >= 44` — the CRT wrapper depends on this behavior.

### DosQueryCtryInfo Bounded Writes

`dos_query_ctry_info()` writes only `min(cb, 44)` bytes to respect the caller's buffer size. The CRT init call with `cb=12` only gets the first 12 bytes (country, codepage, fsDateFmt). Writing more would corrupt the CRT's stack frame. Currently returns hardcoded US English defaults (country=1, codepage=437, MDY format).

---

## Structured Exception Handling (SEH)

OS/2 uses a per-thread exception handler chain (analogous to Win32 SEH) rooted at `TIB+0x00` (`tib_pexchain`). Each handler is an EXCEPTIONREGISTRATIONRECORD linked-list node; the chain terminates at `XCPT_CHAIN_END` (0xFFFFFFFF).

### Handler Chain Maintenance

`DosSetExceptionHandler` (ordinal 354) and `DosUnsetExceptionHandler` (355) are implemented in `stubs.rs`:

- `dos_set_exception_handler(preg_rec)` — writes `preg_rec->prev = TIB[TIB_EXCHAIN_OFFSET]`, then `TIB[TIB_EXCHAIN_OFFSET] = preg_rec`
- `dos_unset_exception_handler(preg_rec)` — writes `TIB[TIB_EXCHAIN_OFFSET] = preg_rec->prev`

`TIB_EXCHAIN_OFFSET` is 0x00 (the first dword of the TIB). `setup_guest()` in `vcpu.rs` initialises this to `XCPT_CHAIN_END` before the first vCPU run.

### Hardware Exception Dispatch Path

When the guest triggers an IDT fault (vectors 0–31) that is not the Far16 thunk bypass:

1. `vcpu.rs` builds a `FaultContext` from the current KVM registers. Pre-fault ESP is `frame_base + 20` (5 dwords pushed by the IDT stub: `[vector][error_code][EIP][CS][EFLAGS]`).
2. `try_hw_exception_dispatch()` in `seh.rs` reads `TIB[TIB_EXCHAIN_OFFSET]`. If the chain is `XCPT_CHAIN_END`, returns `None` → falls through to crash dump.
3. Otherwise allocates guest memory for an EXCEPTIONREPORTRECORD (0x38 bytes) and CONTEXTRECORD (0xCC bytes), writes them via `write_exception_report()` / `write_context_record()`, and returns `ApiResult::ExceptionDispatch`.
4. Back in the VMEXIT loop, `setup_exception_dispatch()` pushes a `CallbackFrame { FrameKind::ExceptionHandler { saved, next_handler, exc_report, ctx_record, guest_allocs } }` and sets up the guest stack for the handler call:

```
[CALLBACK_RET_TRAP]   ← guest EIP (handler will RETF here)
[exc_report ptr]
[reg_rec ptr]
[ctx_record ptr]
[0]                   ← pDispCtx (unused)
```

Handler calling convention is cdecl (caller-cleanup, 4 args).

### CALLBACK_RET_TRAP — ExceptionHandler Frame

When the handler returns via `CALLBACK_RET_TRAP`:

- **`XCPT_CONTINUE_EXECUTION` (0xFFFFFFFF):** calls `read_context_record(ctx_record)` to restore all GP registers and segment selectors to the saved state, then frees `guest_allocs` and resumes execution at `saved.eip`.
- **`XCPT_CONTINUE_SEARCH` (0), or any other value:** reads `reg_rec->prev` (next handler in chain). If not `XCPT_CHAIN_END`, calls `setup_exception_dispatch()` again with the next handler address. If chain exhausted, falls through to crash dump.

### DosRaiseException (ordinal 356)

`dos_raise_exception(p_exc_report)` in `seh.rs` reads the caller-supplied EXCEPTIONREPORTRECORD from guest memory, walks the TIB chain, and returns `ApiResult::ExceptionDispatch`. The VMEXIT loop's `ApiResult::ExceptionDispatch` arm adjusts `saved.eip`/`saved.esp` to point back to the `DosRaiseException` API caller, so `XCPT_CONTINUE_EXECUTION` effectively returns `NO_ERROR` to the API caller.

### DosUnwindException (ordinal 357)

`dos_unwind_exception(p_target_rec, _addr, p_exc_report, _data)` in `seh.rs` walks the TIB chain until `p_target_rec` is reached (or `XCPT_CHAIN_END`), then writes `TIB[TIB_EXCHAIN_OFFSET] = p_target_rec`, truncating the chain to the target registration record.

### xcpt_code_for_vector

Maps x86 CPU fault vectors to OS/2 `XCPT_*` codes:

| Vector | XCPT code |
|---|---|
| 0 (#DE) | `XCPT_INTEGER_DIVIDE_BY_ZERO` (0xC0000094) |
| 4 (#OF) | `XCPT_INTEGER_OVERFLOW` (0xC0000095) |
| 5 (#BR) | `XCPT_ARRAY_BOUNDS_EXCEEDED` (0xC0000093) |
| 6 (#UD) | `XCPT_ILLEGAL_INSTRUCTION` (0xC000001C) |
| 11 (#NP) | `XCPT_ACCESS_VIOLATION` (0xC0000005) |
| 12 (#SS) | `XCPT_ACCESS_VIOLATION` |
| 13 (#GP) | `XCPT_ACCESS_VIOLATION` |
| 14 (#PF) | `XCPT_ACCESS_VIOLATION` |
| others | `XCPT_PROCESS_TERMINATE` (0x40010004) |

### Key Constants (constants.rs)

```rust
pub const TIB_EXCHAIN_OFFSET: u32 = 0x00;
pub const XCPT_CHAIN_END: u32 = 0xFFFF_FFFF;
pub const XCPT_CONTINUE_EXECUTION: u32 = 0xFFFF_FFFF;
pub const XCPT_CONTINUE_SEARCH: u32 = 0x0000_0000;
pub const EXCEPTION_REPORT_SIZE: u32 = 0x38;
pub const CONTEXT_RECORD_SIZE: u32 = 0xCC;
pub const EH_NONCONTINUABLE: u32 = 0x0000_0001;
pub const EH_UNWINDING: u32 = 0x0000_0002;
```

---

## 4OS2 Compatibility

[4OS2](https://github.com/StevenLevine/4os2) is a commercial-grade OS/2 command shell (BSD-like license from JP Software). It serves as Warpine's primary text-mode compatibility target because it exercises nearly every DOSCALLS surface plus the full Kbd/Vio console subsystem.

### Setup

```bash
cd samples/4os2
./fetch_source.sh    # Clones source, applies warpine patches automatically
make                 # Cross-compiles with Open Watcom
```

`fetch_source.sh` applies 6 patches from `patches/` (see `patches/README.md`):
1. `bsesub.h.patch` — APIENTRY16 → _System (eliminate 16-bit VIO/KBD thunks)
2. `viodirect.h` — APIENTRY16/_Seg16 macro overrides (new file)
3. `viowrap.c` — 32-bit VIO/KBD import pragmas (new file)
4. `crt0.c` — minimal CRT startup using DosGetInfoBlocks instead of DosGetInfoSeg (new file)
5. `os2init.c.patch` — DosGetInfoSeg → DosGetInfoBlocks
6. `os2calls.c.patch` — direct DosFindFirst/DosFindNext with FILEFINDBUF4

### Running

```bash
cargo run -- samples/4os2/4os2.exe
```

4OS2 boots to an interactive `[c:\]` prompt in an SDL 640×400 text window (CP437 font, CGA colours, blinking cursor). Working commands include `ver`, `set`, `echo`, `dir`, `exit`, and other built-ins. Use `RUST_LOG=debug` to trace API calls. Use `WARPINE_HEADLESS=1` to run in the host terminal instead.

### Key implementation details

Several issues were discovered and fixed during 4OS2 bring-up that are worth noting for future OS/2 application compatibility work:

- **16-bit thunk elimination** — Watcom's `APIENTRY16` (`_Far16 _Pascal`) generates `__vfthunk` 16-bit bridges that warpine cannot execute. The fix is source-level: replace `APIENTRY16` with `_System` in headers, provide a custom CRT startup (`crt0.c`) that avoids `DosGetInfoSeg` thunks, and use `#pragma import` for direct VIO/KBD ordinal imports. This produces a pure 32-bit binary with zero 16-bit code. See `samples/4os2/patches/` for all modifications.
- **Why the patches cannot be fully reverted even after GDT fixes** — The patches serve two distinct purposes that must be kept separate:
  - *Purpose 1 — OS/2 API calling convention* (`bsesub.h`, `viowrap.c`, `crt0.c`): these eliminate `__vfthunk` generation for VIO/KBD/MOU API calls and replace `DosGetInfoSeg` (16-bit-only) with `DosGetInfoBlocks`. Even with a fully working GDT, Warpine's emulation dispatches all these APIs through the 32-bit `MAGIC_API_BASE` INT 3 thunk mechanism. Reverting these patches would require a complete 16-bit API dispatch path (separate ordinal namespace, Pascal/Far16 calling convention, 16:16 pointer translation) — a large undertaking.
  - *Purpose 2 — correctness fixes* (`os2init.c`, `os2calls.c`): these fix 4OS2 bugs unrelated to calling convention (DosGetInfoSeg → DosGetInfoBlocks, correct DosFindFirst buffer layout). These patches should stay regardless.
- **JPOS2DLL still uses `__Far16` thunks** — 4OS2 itself is pure 32-bit after patching, but JPOS2DLL (its extension DLL) was compiled with its own build rules and still calls some of its entry points via `__Far16` far pointers. The thunk stubs in 4OS2's code do `JMP FAR 0x0028:offset` to enter 16-bit execution. This requires GDT[5] (selector 0x0028) to be a valid 16-bit code descriptor — see the [GDT layout](#kvm-virtualization-engine) in §5.
- **VIOCALLS/KBDCALLS use Pascal calling convention** — Arguments pushed left-to-right (last arg at ESP+4), callee cleans stack. This is different from DOSCALLS which uses `_System` (right-to-left, caller cleans). The loader adds callee stack cleanup after VIO/KBD API returns via `viocalls_arg_bytes()` and `kbdcalls_arg_bytes()`.
- **NLS DLL calling convention** — NLS functions (DosQueryCp, DosQueryCtryInfo, DosMapCase) are imported through the NLS DLL, which uses `_System` convention (same as DOSCALLS, NOT Pascal like VIOCALLS). The Watcom CRT wrapper caches NLS data: it calls NLS ordinal 6 (DosQueryCtryInfo) once during init with cb=12, then calls NLS ordinal 5 with cb=44 to fill the full COUNTRYINFO. NLS ordinal 5 has dual behavior — returns codepages for small cb, returns full COUNTRYINFO for cb >= 44.
- **FSQBUFFER2 layout** — Uses fixed 8-byte header (iType+cbName+cbFSDName+cbFSAData) followed by variable-length strings, not the older FSQBUFFER interleaved format. Getting this wrong causes 4OS2's `ifs_type()` to read garbage, preventing `dir` from calling DosFindFirst.
- **DosFindNext level tracking** — DosFindFirst stores the info level (1=FILEFINDBUF3, 2=FILEFINDBUF4 with EA size). DosFindNext must use the same level, otherwise filenames are offset by 4 bytes (the cbList field size difference).
- **PIB field ordering** — `pib_pchcmd` is at offset `+0x0C` and `pib_pchenv` is at `+0x10`. Getting these swapped causes the app to read the command line string as the environment block.
- **Environment block format** — Must be null-terminated `KEY=VALUE` strings followed by a double-null terminator. The command line string is stored separately at `pib_pchcmd`.
- **Guest memory layout** — TIB/PIB must stay below `0x100000` for 16-bit segment arithmetic (`addr >> 4` must fit in u16). Safe zone: `0x90000–0x9FFFF`.
- **CR→CRLF on stdin** — OS/2 console DosRead returns `\r\n` when Enter is pressed. Linux raw mode sends only `\r`. Without translation, 4OS2 never recognizes end-of-line.
- **BDA initialization** — BIOS Data Area at flat address 0x400 must contain VGA 80x25 text mode info. 4OS2's `crt0.c` reads BDA to determine screen dimensions.

---

## Builtin CMD.EXE Shell

`src/loader/cmd.rs` implements a command shell entirely in host Rust, eliminating the Open Watcom / 4OS2 build dependency for basic interactive use. The shell is invoked in two ways:

1. **From the host command line** — `warpine CMD.EXE [args]`: detected in `main()` by basename before `detect_format()` is called. When stdout is a terminal and `WARPINE_HEADLESS` is not set, enables SDL mode on `VioManager`, spawns the shell on a worker thread via `run_builtin_cmd_sdl()`, and runs `run_text_loop()` on the main thread — the same flow as any CLI LX app. With `WARPINE_HEADLESS=1` or piped stdout, calls `run_builtin_cmd_main()` directly on the main thread (terminal mode).

2. **From a running OS/2 guest** — `DosExecPgm("CMD.EXE")` or `DosExecPgm("OS2SHELL.EXE")`: intercepted in `dos_exec_pgm()` before the VFS path lookup; routes to `Loader::run_builtin_cmd()`. Runs inside the active VIO text window (SDL or headless), sharing keyboard queue and screen buffer with the guest.

### Architecture

```
main.rs CMD.EXE intercept (stdout is terminal):
    ├── enable SDL mode on VioManager
    ├── spawn thread → Loader::run_builtin_cmd_sdl()
    └── run_text_loop() on main thread (SDL window)

main.rs CMD.EXE intercept (WARPINE_HEADLESS / piped):
    └── Loader::run_builtin_cmd_main()    (terminal mode, blocks)

dos_exec_pgm() intercept (guest call):
    └── Loader::run_builtin_cmd()

All paths converge on:
    run_builtin_cmd_main_inner()
        └── CmdShell::run()
                ├── parse_shell_flags()   /C /K processing
                ├── interactive_loop()    REPL: prompt → read_line → execute_line
                └── run_script()          .CMD file execution
```

`CmdShell` holds an `Arc<SharedState>` and has no vCPU — all I/O goes through `SharedState` managers directly.

### Keyboard input

`CmdShell::read_key()` uses dual-path input matching the rest of the VIO subsystem:

- **SDL path** (`shared.use_sdl_text = true`): blocks on `shared.kbd_cond` condvar, pops from `shared.kbd_queue`. Used when the shell is invoked from a running OS/2 guest with an SDL text window open.
- **Terminal path** (`use_sdl_text = false`): calls `console_mgr.read_byte()` with a 5 ms sleep loop, using raw termios mode. Used when the shell is invoked directly from the host command line.

Arrow-key sequences (`\x1B[A` / `\x1B[B`) are decoded in the terminal path to `SCAN_UP` / `SCAN_DOWN` for history navigation.

### Line editor

`read_line()` assembles a `String` from keystrokes:

| Key | Action |
|-----|--------|
| Enter | Submit line |
| Backspace / DEL | Erase last character (`\x08 \x08` sequence to VIO) |
| Esc | Clear line |
| ↑ / ↓ | Navigate history — `replace_input()` erases current chars then writes the new string |

History is a `Vec<String>` capped at 20 entries (constant `HISTORY_SIZE`). Duplicate consecutive entries are not stored.

### Built-in commands

| Command | Description |
|---------|-------------|
| `DIR [path]` | List directory; sorted dirs-first, then files. Reads via `DriveManager::find_first` / `find_next`. |
| `CD [path]` | Change directory. Updates `DriveManager::set_current_dir` and `process_mgr::set_current_dir`. |
| `C:` … `Z:` | Switch current drive. |
| `SET [var[=val]]` | List/get/set host environment variables via `std::env`. |
| `ECHO [text]` | Write text to VIO. `ECHO.` prints a blank line. |
| `CLS` | Clears VIO screen buffer and resets cursor to (0, 0). |
| `VER` | Prints Warpine version string. |
| `TYPE <file>` | Reads file via host `std::fs` and writes to VIO in 4 KiB chunks. |
| `MD <path>` | Creates directory via `std::fs::create_dir_all`. |
| `RD <path>` | Removes empty directory via `std::fs::remove_dir`. |
| `DEL <file>` | Deletes file via `std::fs::remove_file`. |
| `HELP` | Prints command list. |
| `EXIT [code]` | Exits shell with optional numeric exit code. |

### External program execution

`exec_external()` tries `<command>.EXE` then `<command>.CMD` via `DriveManager::resolve_to_host_path`. For `.EXE` files, `spawn_os2_program()` launches the Warpine host process recursively (`std::process::Command::new(warpine_exe)`), captures stdout, and forwards it to `VioManager::write_tty`. For `.CMD` files, `run_script()` reads and interprets each line.

### .CMD script interpreter

`run_script()` reads the script file line by line, strips comments (`REM`, `::`) and calls `execute_line()` for each. Supported directives:

| Directive | Behaviour |
|-----------|-----------|
| `ECHO` | Write to VIO (same as interactive) |
| `SET` | Set/unset host env variable |
| `IF [NOT] EXIST <file> <cmd>` | Conditional on file existence |
| `IF [NOT] ERRORLEVEL <n> <cmd>` | Conditional on last exit code |
| `FOR %%V IN (list) DO <cmd>` | Iterate space-separated list |
| `GOTO <label>` | Jump to `:label` line |
| `CALL <script>` | Recursive script execution |
| `PAUSE` | Waits for any key |
| `REM` / `::` | Comment — skipped |

### Lock ordering

The shell never holds two `SharedState` manager locks simultaneously. Acquire order when both must be consulted: `console_mgr` (level 0) → `process_mgr` (level 1) → `drive_mgr` (level 2). In practice each built-in grabs exactly one lock per operation.

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
| OS/2 wildcard matching | `*`, `?` patterns; `*.*` matches all files (HPFS semantics, unlike FAT where it requires a dot) |
| Directory listing cache | 2-second TTL to avoid repeated `readdir()` for case-insensitive lookup |
| Volume geometry | `statvfs()` on the root directory |
| Sandbox enforcement | Canonicalize + verify prefix stays within volume root |

C: drive is auto-mounted at `~/.local/share/warpine/drive_c/` (via `XDG_DATA_HOME` / `HOME` fallback). The directory is created automatically if absent.

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
  main.rs              Entry point: CLI/PM/text-mode detection, SDL init, thread spawning
  api.rs               DosWrite/DosExit FFI bridge stubs
  build.rs             Compile-time Unifont parser: emits font_unifont_sbcs.rs + font_unifont_wide.bin
  lx/
    mod.rs             LX module re-exports
    header.rs          LX binary format structures and parsing
    lx.rs              LX file orchestration (open, parse, fixups)
  ne/
    mod.rs             NE module re-exports
    header.rs          NE binary format structures (NeHeader, NeSegmentEntry, NeRelocationEntry)
    ne.rs              NE file orchestration (16-bit OS/2 apps, 16 unit tests)
  gui/
    mod.rs             GUI module re-exports; pub use for all renderer types
    message.rs         GUIMessage channel (clipboard, mouse capture, message boxes) + GuiWake
    font.rs            PM typeface: embedded IBM Plex Sans/Mono/Serif + Plex Sans JP/KR/TC/SC
                       (vendor/ibm-plex), FontSpec, text_width(), metrics(), draw_text();
                       fallback chain Latin → CJK (by codepage) → GNU Unifont
    gui_script.rs      Scripted GUI sessions (WARPINE_GUI_SCRIPT): step parser, timed inputs,
                       PPM dump — SDL-free; used by the gui_* integration tests
    damage.rs          Rect, damage list, clipped row-wise blit/fill for the compositor
    frame_stats.rs     GUI loop / composite timing statistics (debug log every 5 s)
    renderer.rs        PmRenderer trait + run_pm_loop() (PM/GUI SDL event loop)
    audio.rs           AudioService (thread-safe audio commands) + SdlAudio (main-thread SDL3 streams)
    surface.rs         Shared per-frame drawing surfaces: Surface, DrawOp, Mix (FM_* raster ops), rop3
    headless.rs        HeadlessRenderer: no-op PM backend for CI
    sdl_renderer.rs    SdlRenderer: PM SDL backend (scene, damage, chrome cache, input);
                       SDL scancode/VK tables; push_msg
    sdl_renderer/compositor.rs
                       Compositor trait: SoftwareCompositor (desktop buffer, one texture) and
                       LayeredCompositor (one texture per frame, drawn by the SDL renderer)
    text_renderer.rs   TextModeRenderer trait, VgaTextBuffer, SdlTextRenderer,
                       HeadlessTextRenderer, run_text_loop(), get_cp437_glyph(),
                       CGA_PALETTE (22 unit tests)
  loader/
    mod.rs             Loader struct, SharedState (kbd_queue/kbd_cond/use_sdl_text), KVM/mock init
    vcpu.rs            vCPU thread: VMEXIT loop, GDB integration, crash dump hooks
    vm_backend.rs      VmBackend/VcpuBackend traits (KVM + mock implementations)
    kvm_backend.rs     KVM-based VmBackend implementation
    guest_mem.rs       Guest memory read/write/translate helpers
    lx_loader.rs       LX executable loading into guest memory and fixup application
    ne_exec.rs         NE executable loader: load_ne(), setup_guest_ne(), handle_ne_api_call(), ne_api_arg_bytes()
    descriptors.rs     GDT/IDT setup, resolve_import() (built-ins + DllManager)
    constants.rs       Named constants (addresses, message IDs, ordinal bases)
    api_registry.rs    Static sorted API thunk table (186 entries); ApiEntry
    api_dispatch.rs    Registry lookup + per-subsystem dispatcher routing
    api_trace.rs       Structured tracing helpers (ordinal_to_name auto-generated from os2api.def,
                       module_for_ordinal, arg_names_for_ordinal, format_call)
    api_ring.rs        256-entry bounded API call ring buffer for crash post-mortem
    crash_dump.rs      Structured crash reports (regs, stack, code bytes) on fatal VMEXITs
    gdb_stub.rs        GDB Remote Stub: RSP over TCP, software breakpoints, single-step
    mutex_ext.rs       MutexExt trait (poison-recovering lock)
    managers.rs        MemoryManager, HandleManager, HDirManager, ResourceManager
    ipc.rs             Semaphores (event, mutex, muxwait) and queues
    pm_types.rs        PM data types (windows, classes, presentation spaces, WindowManager)
    locale.rs          Os2Locale: country/codepage information
    doscalls.rs        DOSCALLS API implementations (~40 functions)
    pm_win.rs          PMWIN API implementations (~50 ordinals)
    pm_gpi.rs          PMGPI API implementations (64 ordinals, incl. DevOpenDC/DevCloseDC/DevQueryCaps)
    pm_shapes.rs       Arc geometry (ARCPARAMS, full/partial/point arcs, FIXED) and the PATSYM_* pattern table
    pm_bitmap.rs       Bitmap codec (BITMAPINFOHEADER/2, RT_BITMAP), memory DCs, bitmaps, GpiBitBlt family
    pm_edit.rs         Edit-buffer logic for text controls (insert/delete/caret/scroll)
    pm_controls.rs     Interactive WC_ENTRYFIELD / WC_MLE / WC_LISTBOX: messages + painting
    mmpm.rs            MMPM/2: MmpmManager, beep_tone, mciSendCommand/mciSendString, parse_wav
    console.rs         VioManager: screen buffer, cursor state/shape, sdl_mode,
                       ANSI output, stdin_cooked_chars, CP437→UTF-8 conversion
    viocalls.rs        VIOCALLS API: VioWrtTTY, VioScrollUp/Dn (pCell + lines=0),
                       VioSetCurType, VioGetCurType, VioWrtNCell, VioWrtNAttr, etc.
    kbdcalls.rs        KBDCALLS API: KbdCharIn (kbd_queue in SDL, termios in terminal),
                       KbdStringIn, KbdGetStatus
    stubs.rs           Stub handlers for unimplemented/low-priority APIs
    process.rs         ProcessManager: DosExecPgm, DosWaitChild, directory tracking
    vfs.rs             VfsBackend trait, DriveManager, Os2Error, OS/2 data types, handle types
    vfs_hostdir.rs     HostDirBackend: HPFS-on-host-directory VfsBackend implementation
build.rs               Code generation: GNU Unifont glyph tables
```

---

## Adding a New API

To add a new OS/2 API call:

1. **Find the ordinal** — Look up the API's ordinal number in `doc/os2_ordinals.md` or OS/2 documentation.

2. **Add the handler method** — In the appropriate file (`doscalls.rs`, `pm_win.rs`, `pm_gpi.rs`, `viocalls.rs`, `kbdcalls.rs`, or `stubs.rs`), add a method on `impl Loader`:
   ```rust
   pub fn dos_my_new_api(&self, param1: u32, param2: u32) -> u32 {
       // Read additional args from guest memory if needed
       // Implement the API logic
       // Return the OS/2 error code (0 = NO_ERROR)
       0
   }
   ```

3. **Wire up the dispatch** — The dispatch path depends on the subsystem:

   - **DOSCALLS / QUECALLS**: `api_registry.rs` contains a **static sorted table** of `ApiEntry` records (186 entries). Add a new entry to the table in ascending ordinal order. The table is binary-searched at dispatch time; no match arm needed in `api_dispatch.rs` for most cases. For subsystem handlers that need special argument extraction (VIO, KBD, PMWIN, PMGPI, etc.), add a match arm in the appropriate `handle_*_calls()` function in `api_dispatch.rs`.

   - **VIOCALLS / KBDCALLS**: add a match arm in `handle_viocalls()` or `handle_kbdcalls()`, and add the ordinal's stack-byte count to `viocalls_arg_bytes()` / `kbdcalls_arg_bytes()`.

   - **PMWIN / PMGPI**: add a match arm in `handle_pmwin_call()` or `handle_pmgpi_call()`.

   ```rust
   // Example: VIOCALLS arm
   // VioMyFunc(pArg, hvio) → ESP+4=hvio, +8=pArg
   99 => self.vio_my_func(read_stack(8), read_stack(4)),
   ```

4. **Add a named constant** — If the ordinal is used elsewhere, add it to `constants.rs`.
   Message numbers, flags, error codes and other values that have a toolkit `#define` keep the toolkit's name and value — copy them from the header (`vendor/watcom/h/os2/*.h`), never from memory. The unit test `constants_match_toolkit_headers` compares every integer `const` in `src/` whose name is a toolkit `#define` with the header value (it runs when the Open Watcom headers are installed); a constant that shares a name but means something else goes in its `NOT_TOOLKIT` list.

5. **Test** — Build with `cargo build`, run with an OS/2 binary that uses the API, verify with `RUST_LOG=debug`.

Key conventions:
- **DOSCALLS** arguments follow `_System` calling convention: first arg at `ESP+4`, second at `ESP+8`, etc.
- **VIOCALLS / KBDCALLS** use **Pascal** calling convention: arguments are pushed left-to-right, so the **last** argument is at `ESP+4` and the **first** is at the highest offset.
- Return OS/2 error codes in EAX (0 = success). Common codes: 2 = FILE_NOT_FOUND, 5 = ACCESS_DENIED, 6 = INVALID_HANDLE, 87 = INVALID_PARAMETER.
- For PM APIs that need to invoke guest code, return `ApiResult::Callback` instead of `ApiResult::Normal`.
- Use `guest_read`/`guest_write` for all guest memory access — never dereference raw guest pointers without bounds checking.

---

## Debugging

Warpine uses the `tracing` crate. `RUST_LOG` controls verbosity; `WARPINE_TRACE` selects the output format:

```bash
# Full debug output — shows every API call, arguments, and return values
RUST_LOG=debug cargo run -- samples/pm_demo/pm_demo.exe

# strace-like compact one-line-per-call format
WARPINE_TRACE=strace RUST_LOG=debug cargo run -- samples/hello/hello.exe

# JSON Lines (machine-readable, for tooling/analysis)
WARPINE_TRACE=json RUST_LOG=debug cargo run -- samples/hello/hello.exe

# Info level — shows high-level milestones (parse, load, entry point)
RUST_LOG=info cargo run -- samples/pm_demo/pm_demo.exe

# Filter to a specific module
RUST_LOG=warpine::loader=debug cargo run -- samples/hello/hello.exe
RUST_LOG=warpine::gui=debug cargo run -- samples/pm_demo/pm_demo.exe
```

`WARPINE_TRACE` values:
- (unset) — default `tracing-subscriber` format with spans and timestamps
- `strace` — compact `syscall(args) = retval` lines, one per API call
- `json` — JSON Lines; each line is a self-contained record for log ingestion tools

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

## Developer Tooling

### GDB Remote Stub

**Module:** `src/loader/gdb_stub.rs`

Warpine includes a built-in GDB Remote Serial Protocol (RSP) stub that allows `gdb`, `gef`, or `pwndbg` to attach to a live KVM guest over TCP.

**Usage:**

```bash
warpine --gdb 1234 samples/hello/hello.exe
# In another terminal:
gdb -ex 'target remote :1234'
```

**Architecture:**

- `GdbState` — Shared `Mutex`+`Condvar` synchronisation channel between the vCPU thread and the GDB stub thread. An `AtomicBool` (`stop_requested`) handles Ctrl-C interrupt from the GDB client.
- `WarpineTarget` — Implements `gdbstub::Target` with `X86_SSE` architecture, `SingleThreadBase`, `SingleThreadResume`, `SingleThreadSingleStep`, and `Breakpoints`/`SwBreakpoint`.
- `GdbBlockingEventLoop` — Polls `stop_cond` with 10 ms timeout and checks the TCP socket for incoming bytes between polls.
- `launch_gdb_stub()` — Binds `127.0.0.1:<port>`, accepts one connection, and runs `GdbStub::run_blocking`.

**Features:** Software breakpoints (INT 3 patching with original byte restore), single-step via `KVM_GUESTDBG_SINGLESTEP`, full 256 MB guest memory read/write, Ctrl-C stop with `SIGINT` mapping. The vCPU pauses at the entry point on startup, sending `SIGTRAP` to the GDB client.

### Crash Dump Facility

**Module:** `src/loader/crash_dump.rs`

On any fatal VMEXIT (guest exception, triple fault, unhandled VMEXIT, KVM run error, unexpected breakpoint), Warpine captures a structured crash report and writes it to both `warpine-crash-<pid>.txt` and stderr.

**Data captured:**
- All general-purpose and segment registers
- Segment descriptor details (base, limit, type)
- Top 32 stack dwords (hex + ASCII)
- 32 bytes of code at EIP (hex dump)
- Last 256 API calls from the ring buffer (see below)
- Context info: exception type, executable name, timestamp

**Integration:** All four fatal VMEXIT paths in `vcpu.rs` call `Loader::collect_crash_report()` and `Loader::dump_crash_report()`. The `collect_crash_report()` method handles 16-bit SS correctly when reading the stack.

### API Call Ring Buffer

**Module:** `src/loader/api_ring.rs`

The last 256 OS/2 API calls are stored in a bounded `VecDeque` (`ApiRingBuffer`) in `SharedState`, populated unconditionally (not gated on `DEBUG` level) so crash dumps include call history even in release/info builds.

- `ApiCallRecord` — Struct with ordinal, module, name, formatted call string, return value, and monotonic sequence number.
- `api_dispatch.rs` computes `format_call` once per call (shared between DEBUG tracing and ring buffer), and pushes a record after each API return.
- `crash_dump.rs` snapshots the ring as `api_history` and renders it as `[seq] MODULE.call() → ret` lines in the crash report.

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
| `src/ne/` | NE header parsing, segment table, relocation records, export/import tables, resource table (16 tests) |
| `src/loader/managers.rs` | MemoryManager allocation, 4KB alignment, free-list reuse, top-of-heap coalescing, overflow/limit rejection, ResourceManager find operations, SharedMemManager name lookup |
| `src/loader/console.rs` | VioManager screen buffer operations (scroll up/down, read cell str, defaults), key mapping (enter, printable, backspace → OS/2 charcode/scancode) |
| `src/loader/stubs.rs` | DosEditName wildcard pattern replacement, DosQuerySysInfo QSV_* constant validation |
| `src/loader/process.rs` | ProcessManager child tracking, wait-any semantics |
| `src/loader/api_trace.rs` | ordinal_to_name (DOSCALLS, QUECALLS, KBDCALLS, VIOCALLS, NLS, MSG, MDM, UCONV — auto-generated from `os2api.def`), module_for_ordinal (all ranges), arg_names_for_ordinal |
| `src/loader/mmpm.rs` | MmpmManager waveaudio device open/close/play lifecycle, mciSendCommand dispatch, mciSendString parsing, mciFreeBlock, mciGetLastError |
| `src/loader/vfs.rs` | DriveManager path resolution, device name detection, drive letter parsing |
| `src/loader/vfs_hostdir.rs` | HostDirBackend case-insensitive lookup, sandbox enforcement, EA operations |
| `src/gui/sdl_renderer.rs` | Y-coordinate flipping, rectangle rendering (filled/outlined), line drawing (horizontal/vertical/diagonal), text rendering pixel output and orientation; scancode/VK mapping |
| `src/gui/text_renderer.rs` | CP437 glyph bitmaps (box-drawing, block elements, ASCII delegation), CGA palette (opaque, correct entries), HeadlessTextRenderer frame counting and loop termination, VgaTextBuffer snapshot, KbdKeyInfo queue push/pop (22 tests) |

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

# VFS: comprehensive filesystem test on drive C: (create, read, seek, truncate,
# mkdir, rename, copy, find, metadata, current dir, delete)
cargo run -- samples/vfs_test/vfs_test.exe

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

CLI samples should print output and exit with code 0. PM samples should open a window and respond to close. 4OS2 should boot to an interactive prompt (`[c:\]`) with working commands: `dir`, `set`, `ver`, `md`, `rd`, `copy`, `move`, `del`, `attrib`, `tree`, `exit`. Use `RUST_LOG=debug` to inspect API call traces if a sample misbehaves.

### Automated integration tests

`tests/integration.rs` contains 9 end-to-end tests that run real OS/2 sample binaries (hello, alloc_test, nls_test, thread_test, pipe_test, mutex_test, queue_test, thunk_test, ne_hello) with headless mode, asserting stdout content and exit code. KVM-gated: tests skip silently when `/dev/kvm` is absent.

```bash
cargo test --test integration     # 9 end-to-end tests (requires /dev/kvm)
```

---

### GUI regression tests

PM rendering is tested end to end without a display.

**Scripted sessions.** Setting `WARPINE_GUI_SCRIPT` to a script file makes the
SDL PM renderer replay it (`src/gui/gui_script.rs`). Each line is
`<milliseconds since start> <action> [arguments]`; blank lines and lines
starting with `#` are ignored (a `#` after an action is not a comment):

```text
# File -> About..., capture the dialog, then move the main window
1500 click 52 62
2500 click 62 80
4000 dump  /tmp/about.ppm
4500 drag  300 42 500 242
5000 quit
```

Coordinates are desktop pixels with the origin at the top-left.

| Action | Effect |
|--------|--------|
| `click X Y` | Left button press and release. |
| `rclick X Y` | Right button press and release. |
| `drag X1 Y1 X2 Y2` | Left button down at `(X1, Y1)`, four evenly spaced mouse moves ending at `(X2, Y2)`, button up there; the six events are 20 ms apart (100 ms in all). Starting on a title bar moves the window, starting on a sizing border resizes it. |
| `key [MOD+]NAME` | Press and release one key: `enter`, `tab`, `esc`, `backspace`, `delete`, `insert`, `left`, `right`, `up`, `down`, `home`, `end`, `pageup`, `pagedown`, `space`, `f1`..`f12`, or one printable ASCII character (`key a`). Modifiers `shift+`, `ctrl+`, `alt+` may be combined (`key alt+f4`, `key ctrl+shift+tab`); a character that needs Shift on a US keyboard (`key A`, `key ?`) implies it. |
| `text STRING` | Type the rest of the line, one character every 10 ms. Non-ASCII text is allowed. |
| `dump PATH` | Write the composited desktop as a binary PPM (P6). The path is the rest of the line. |
| `quit` | End the session as if the desktop window had been closed. |

Steps run in time order and never interleave: a step scheduled inside a
preceding `drag` or `text` starts when that one has finished. Input scripted
before a `dump` has been handled by the renderer when the capture is taken,
but the application still needs time to react, so leave a pause before
capturing.

While a script runs, mouse, keyboard and text input from the host is
ignored (`is_host_input`), so a session behaves the same on a real display
as on the `dummy` driver: a stray pointer movement during a scripted drag
would otherwise move the window off the scripted path. Window events —
resize, expose, closing the desktop window — still count. This makes
scripts usable for timing comparisons on a real display, e.g. 50 title-bar
drags under each `WARPINE_COMPOSITOR` value with
`RUST_LOG=warpine::gui=debug`.

The script injects the SDL events a real mouse and keyboard produce, so the
normal event handling runs. `key` sends `KeyDown`/`KeyUp`, with the modifier
keys pressed before and released after, which become `WM_CHAR`. `text` sends,
per ASCII character, the US-layout keystroke plus a `TextInput` event; any
other character is a `TextInput` event alone, as an input method would commit
it. The PM renderer does not act on `TextInput` yet, so today only the
keystrokes of `text` reach the application, and `WM_CHAR` carries the
unshifted character with `KC_SHIFT` set.

Run it with SDL's dummy drivers so no window is opened:

```bash
SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy WARPINE_GUI_SCRIPT=session.script \
  cargo run -- samples/pm_menu_dlg_test/pm_menu_dlg_test.exe
```

**Golden references.** The `gui_*` tests in `tests/integration.rs` run a sample
under a script and compare the dump with `tests/golden/<name>.sig`. The
reference is a *tile signature*: the average colour of each 16×16-pixel tile,
as text. It is small, tolerates anti-aliasing noise (per-channel tolerance 6),
and a failure lists the tiles that changed as `(tile x, tile y)`, so multiply
by 16 to find the area on the desktop.

Tests of an interaction whose outcome is a few known pixels assert on the dump
directly instead (`gui_pixel`): `gui_drag_title_bar_moves_window` and
`gui_drag_sizing_border_resizes_window` capture the desktop before and after a
`drag` and check the title-bar and desktop colours at the old and new
positions.

After an intentional rendering change, regenerate and review the references:

```bash
WARPINE_UPDATE_GOLDEN=1 cargo test --test integration gui_
git diff --stat tests/golden
```

The tests are skipped when `/dev/kvm` or the Open Watcom-built sample binary
is missing. Because the signature is coarse, a change confined to a few
pixels (a 1-px line, a small pictogram) can pass unnoticed; unit tests on the
drawing functions cover those.

## Appendix: Development Phases

This appendix summarises the major development phases and what each delivered. The sections above document the current architecture; this appendix provides the historical development narrative.

### Phase 1 — Foundation (CLI Hello World)

LX/LE executable parser (MZ header, object table, page map, fixup table). Loader maps LX objects into 128 MB KVM guest memory and applies relocations. API thunk infrastructure: imports resolved to INT 3 trap stubs at `MAGIC_API_BASE` (0x01000000); VMEXIT loop dispatches to Rust handlers by ordinal. Initial DOSCALLS thunks: `DosWrite`, `DosExit`, `DosQuerySysInfo`, `DosQueryConfig`, `DosQueryHType`, `DosGetInfoBlocks`.

### Phase 2 — Core OS/2 Subsystem

Memory: `DosAllocMem` / `DosFreeMem`. Filesystem: `DosOpen`/`DosRead`/`DosWrite`/`DosClose`/`DosDelete`/`DosMove`/`DosCreateDir`/`DosDeleteDir`, `DosFindFirst`/`DosFindNext`, OS/2 drive-letter path translation. Threads: `DosCreateThread`, `DosKillThread`, TLS via TIB. IPC: event, mutex, and MuxWait semaphores; pipes (`DosCreatePipe`); queues (`DosCreateQueue`/`DosWriteQueue`/`DosReadQueue`).

### Phase 3 — Presentation Manager (GUI)

Dual-path execution: PM apps run the SDL2 event loop on the main thread; CLI apps run the vCPU directly. `GUIMessage` channel carried draw/window commands from vCPU thread to main thread (since replaced by shared surfaces drawn on the vCPU thread). PMWIN: `WinInitialize`/`WinTerminate`, message queues, `WinRegisterClass`, `WinCreateStdWindow`, `WinGetMsg`/`WinDispatchMsg`, `WinPostMsg`/`WinSendMsg`, `WinDefWindowProc`, `WinBeginPaint`/`WinEndPaint`, `WinMessageBox`, `WinShowWindow`, `WinDestroyWindow`, timers, dialogs (stubs), menus (stubs), clipboard in-process storage, `WinSetWindowPos`, resource loading. PMGPI: `GpiCreatePS`/`GpiDestroyPS`, `GpiSetColor`, `GpiMove`, `GpiBox`, `GpiLine`, `GpiCharStringAt`, `GpiErase`. Callback mechanism: `ApiResult::Callback` for re-entrant guest window-procedure calls via `CALLBACK_RET_TRAP`. Input: `WM_CHAR`, `WM_MOUSEMOVE`, `WM_BUTTON1DOWN`/`UP`, `WM_SIZE`, `WM_CLOSE`. Embedded 8×16 VGA bitmap font for text rendering.

### Phase 3.5 — Text-Mode Application Support (4OS2)

Target: 4OS2 command shell — validates nearly every DOSCALLS/KBD/VIO surface. Expanded thunk stub area (`KBDCALLS_BASE=4096`, `VIOCALLS_BASE=5120`, `SESMGR_BASE=6144`, `NLS_BASE=7168`, `MSG_BASE=8192`). Console: `VioManager` with screen buffer, cursor, raw termios input, ANSI escape output. KBD: `KbdCharIn` (blocking/non-blocking, arrow/function-key escape parsing), `KbdGetStatus`, `KbdStringIn`. VIO: `VioWrtTTY`, `VioGetMode`, `VioGetCurPos`, `VioSetCurPos`, `VioSetCurType`, `VioScrollUp`, `VioScrollDn`, `VioWrtCharStrAtt`, `VioWrtNCell`, `VioWrtNAttr`, `VioReadCellStr`, `VioSetAnsi`, `VioGetAnsi`, `VioGetConfig`. Process: `DosSetCurrentDir`, `DosQueryCurrentDir`/`DosQueryCurrentDisk`, `DosSetDefaultDisk`, `DosExecPgm`, `DosWaitChild`, `DosKillProcess`, `DosQueryAppType`. System info: full `DosQuerySysInfo` QSV table, `DosGetDateTime`. Result: 4OS2 boots to a prompt; `dir`, `set`, `ver`, `md`, `rd`, `copy`, `move`, `del`, `attrib`, `tree` all work.

### Phase 4 — HPFS-Compatible Virtual Filesystem

`VfsBackend` trait (21 methods) as the OS/2 filesystem semantics contract. `DriveManager` maps drive letters A:–Z: to backends; owns file and find-handle tables. `HostDirBackend`: case-insensitive case-preserving lookup, long filenames (254 chars), file sharing modes, sandbox enforcement, OS/2 wildcard matching, directory listing cache (2s TTL), device name mapping. Extended attributes via Linux xattrs with sidecar fallback. File locking via `fcntl(F_SETLK)`. `DosFindFirst`/`DosFindNext` multi-entry packing, attribute filtering. C: drive auto-mounted at `~/.local/share/warpine/drive_c/`. Verified: 4OS2 `dir` with correct date/time formatting; `samples/file_test`, `find_test`, `fs_ops_test`, `vfs_test` all pass.

### Phase 4.5 — 16-bit Thunk Fix

Eliminated 16-bit thunks from 4OS2 by recompiling with modified headers rather than runtime patching. Key patches: `bsesub.h` changed `APIENTRY16` to `_System`; `crt0.c` replaces Watcom's `__OS2Main` with a pure 32-bit version using `DosGetInfoBlocks`; `viowrap.c` provides `#pragma import` for VIO/KBD ordinals; DOSCALLS/VIOCALLS/KBDCALLS ordinal tables audited and corrected. All 6 patches stored in `samples/4os2/patches/`; `fetch_source.sh` applies them automatically.

### Phase 5 Baseline — MMPM/2 Audio

`DosBeep` plays real sine-wave tones via SDL2 audio queue. MDM.DLL (`MDM_BASE=10240`) wired into the ordinal dispatch. `mciSendCommand` handles `MCI_OPEN`/`MCI_CLOSE`/`MCI_PLAY`/`MCI_STOP`/`MCI_STATUS` for `waveaudio` device. `mciSendString` parses command strings. WAV files loaded via VFS using `SDL_LoadWAV_RW`. Audio format conversion via `SDL_BuildAudioCVT`/`SDL_ConvertAudio`. Synchronous play via `MCI_WAIT` flag.

### Phase 6 — Text-Mode VGA Renderer

`TextModeRenderer` trait with `Sdl2TextRenderer` (640×400 SDL2 window, CP437 8×16 font, CGA 16-colour palette, blinking cursor) and `HeadlessTextRenderer` backends. `run_text_loop()` as the main event loop for CLI apps. `KbdKeyInfo` + `SharedState::kbd_queue`/`kbd_cond`/`use_sdl2_text` for SDL2→KbdCharIn key delivery. Bug fixes: cursor rendering via XOR pixel inversion; `VioGetCurType` (ordinal 33); `VioScrollUp`/`VioScrollDn` `lines=0` as "clear entire region"; `dos_read_stdin` backspace gating. CLI apps default to SDL2 text window; headless fallback via `is_terminal()` detection.

### Phase 5 — NE (16-bit OS/2 1.x) Execution

Full NE loader and 16-bit execution path in `ne_exec.rs`:

- `load_ne()` maps NE segments into GDT-tiled guest memory starting at `NE_SEGMENT_BASE` (0x00100000), one tile per segment.
- `apply_ne_fixups()` patches CALL FAR import fixups to the NE thunk tile using `NE_THUNK_CODE_SELECTOR` (0x87B0, a code tile descriptor — required because x86 CALL FAR mandates an execute descriptor).
- `setup_guest_ne()` configures the full tiled GDT, fills the NE thunk tile at 0x00F00000 with INT 3 stubs, tightens NE segment GDT entries to actual allocation sizes, and returns initial CS:IP and SS:SP from the NE header.
- `handle_ne_api_call()` dispatches 16-bit DOSCALLS (DosWrite, DosExit, DosGetInfoSeg, DosSetSigHandler, DosSetVec, DosGetEnv) and VIOCALLS (VioWrtTTY) using Pascal calling convention (left-to-right push; far pointers as seg:off word pairs on stack; `ne_api_arg_bytes()` for callee cleanup).

Key implementation challenges resolved:
- **CALL FAR selector type**: data tile (0x07B0) caused `#GP` — switched to code tile (0x87B0).
- **16-bit mode `rip`**: KVM reports `rip` as CS-relative offset in 16-bit mode; `flat_rip = CS.base + rip` needed for thunk dispatch.
- **DPL mismatch**: tiled data tiles changed from DPL=0 to DPL=2 (`access=0xD3`) so OS/2 RPL=2 selectors pass the `max(CPL,RPL)≤DPL` check.
- **Watcom CRT incompatibility**: the Watcom C runtime computes LDT-based selectors (TI=1) that our GDT-tile model cannot provide. `ne_hello` is written in pure assembly (no CRT) to avoid this.

The `ne_hello` sample (`samples/ne_hello/ne_hello.asm`) is a minimal 3-segment NE program (CODE/DATA/STACK) built with Open Watcom `wasm`+`wlink`, directly calling DosWrite (ord 138) + DosExit (ord 5) in Pascal convention. Integration test `test_ne_hello` verifies end-to-end output and exit code.

### Phase 7 Baseline — DLL Loader Chain

`DosLoadModule`/`DosQueryProcAddr`/`DosQueryModuleHandle` implemented. `load_dll()` allocates guest memory for each object, loads pages (rebased), applies fixups, and registers the DLL's resources in `ResourceManager` under its module handle (the executable's are under `EXE_RESOURCE_MODULE`, 0 — `NULLHANDLE` in the resource calls); the final `DosFreeModule` removes them. Every resource lookup (`GpiLoadBitmap`, `WinLoad*`, `WinDlgBox`, `FCF_MENU`, `DosGetResource`, `DosQueryResourceSize`) passes the caller's `hmod`. Ordinal-based and name-based export maps. `DllManager` in `SharedState`. `jpos2dll.dll` (4OS2 extension DLL) loads and resolves all 7 exports at runtime.

### Architecture Milestones

- **Virtualization backend abstraction** — `VmBackend`/`VcpuBackend` traits; KVM isolated to `kvm_backend.rs`; `MockVcpu`/`MockVmBackend` for testing without `/dev/kvm`.
- **Guest memory type safety** — `GuestMemory` struct with safe `read<T>`/`write<T>` replacing raw `*mut u8` + `usize`.
- **Structured API trace** — `api_trace.rs` with `ordinal_to_name()`, `module_for_ordinal()`, per-argument typed names. `WARPINE_TRACE=strace|json` output modes.
- **API thunk auto-registration** — `api_registry.rs` static sorted table (186 entries); O(log n) binary search replaces ~120-arm match.
- **SDL2 GUI backend** — migrated from `winit + softbuffer` to SDL2; full keyboard, mouse, clipboard support.
- **PM renderer abstraction** — `PmRenderer` trait with `Sdl2Renderer` and `HeadlessRenderer` backends.
- **GDT tiling** — 4096 tiled 16-bit data descriptors (GDT[6..4101], DPL=2) and 2048 tiled 16-bit code descriptors (GDT[4102..6149]) for 16:16 addressing; GDT[4] 16-bit data alias (0x20), GDT[5] 16-bit code alias (0x28) for Far16 thunks.
- **NE (16-bit OS/2 1.x) execution** — full NE loader in `ne_exec.rs`: segments loaded into GDT-tiled memory, CALL FAR fixups patched to code tile `NE_THUNK_CODE_SELECTOR` (0x87B0), API dispatch via Pascal calling convention thunks, `ne_hello` pure assembly sample runs end-to-end.
- **Modifier key suppression** — pure modifier keys (LShift, RShift, Ctrl, Alt, CapsLock) are filtered before `KbdCharIn` enqueueing; fixes 4OS2 printing raw scan codes on Shift press.
- **Developer tooling** — crash dump facility (`crash_dump.rs`), GDB Remote Stub (`gdb_stub.rs`, `--gdb <port>`), API call ring buffer (`api_ring.rs`, 256 entries).
- **Testing** — 770 unit tests (741 warpine + 18 gen_api + 11 lx_link), 33 integration tests, compatibility report (`warpine --compat`).

---

## 21. Rust Guest Toolchain

Warpine provides a complete toolchain for writing OS/2 guest programs in Rust without Open Watcom.

### Components

**`targets/i686-warpine-os2.json`** — Custom Rust target spec: `i686-unknown-none`, `relocation-model: static`, `panic-strategy: abort`, `linker: lx-link`. Only `R_386_32` relocations are emitted (no PLT, no PC-relative imports).

**`src/bin/gen_api.rs`** — Ordinal Table Canonical Build Tool. `targets/os2api.def` is the single authoritative source for the OS/2 API ordinal address space. Subcommands:
- `show` — pretty-print all ordinals with flat addresses and module ranges
- `check` — validate def file (no duplicates, sorted, no overflow) and drift-detect against `api_trace.rs` by text-scanning `ordinal_to_name()` match arms
- `gen-trace` — emit replacement Rust source for `ordinal_to_name()` + `module_for_ordinal()` (pipe to a file then paste into `api_trace.rs`)
- `gen-def` — re-emit a canonically sorted `os2api.def`
- `validate-doc` — cross-check `os2api.def` against `doc/os2_ordinals.md` (Open Watcom `lib386/os2/os2386.lib` snapshot); reports confirmed ordinals, 16-bit alias pairs auto-detected by uppercase+strip-"16" normalisation, entries only in def (e.g. `DosQueryCtryInfo`, `VioCheckCharType`), and name mismatches to investigate

Run `cargo run --bin gen_api -- check` to verify consistency after any ordinal change. Run `cargo run --bin gen_api -- validate-doc` to cross-reference against the OS/2 ordinal reference.

**`src/bin/lx_link.rs`** — ELF-to-LX linker. Reads ELF `.o`/`.rlib`/`.a` objects produced by rustc, merges `.text`/`.data` sections, resolves OS/2 API imports via `targets/os2api.def` (embedded via `include_str!` as compile-time fallback), and emits a valid MZ+LX executable. Object layout: code at `0x00010000` (flags `READABLE|EXECUTABLE|BIG`), data at `0x00060000`, stack at `0x00070000`.

**`crates/warpine-os2-sys`** — `#![no_std]` raw OS/2 API bindings. DOSCALLS use `extern "C"` (caller-cleanup). VIOCALLS and KBDCALLS use `extern "stdcall"` (callee-cleanup) with **reversed argument order** to compensate for the OS/2 Pascal calling convention (which pushes args left-to-right, placing the last arg at ESP+4). The reversed declaration causes Rust's right-to-left stdcall push to produce the same stack layout.

**`crates/warpine-os2-rt`** — Runtime shim providing `_start` (calls `os2_main() -> u32` then `DosExit`), a `#[panic_handler]` (calls `DosExit(1,1)`), and a `#[global_allocator]` backed by `DosAllocMem(PAG_READ|PAG_WRITE|PAG_COMMIT)` / `DosFreeMem`.

**`crates/warpine-os2`** — Ergonomic safe wrappers: `mod file` (write_stdout/write_stderr), `mod memory` (alloc/free/set_mem), `mod process` (exit), `mod thread` (sleep/create/wait/kill), `mod vio` (write_tty/get_cur_pos/set_cur_pos).

### Build

```bash
rustup toolchain install nightly --component rust-src
cargo build --bin lx_link && cp target/debug/lx_link ~/.cargo/bin/lx-link

cd samples/rust_hello
cargo +nightly build \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -Z json-target-spec \
  --target ../../targets/i686-warpine-os2.json
```

### Pascal calling-convention trick for VIO/KBD

OS/2 VIOCALLS and KBDCALLS use Pascal convention: arguments are pushed left-to-right so the **last** argument ends up at ESP+4. Warpine's vcpu.rs does callee-cleanup (pops args after returning via `viocalls_arg_bytes`/`kbdcalls_arg_bytes`).

Rust has no `extern "pascal"`. The solution: declare VIO/KBD functions with `extern "stdcall"` and **reverse the argument order**. Since stdcall pushes right-to-left, reversing the declaration makes the compiled push sequence match the Pascal layout Warpine expects, while stdcall's callee-cleanup prevents the Rust caller from also emitting `add esp, N`.

Example — `VioWrtTTY(pStr, cb, hvio)` in OS/2 Pascal order means ESP+4=hvio, ESP+8=cb, ESP+12=pStr. In the sys crate it is declared as:
```rust
pub fn VioWrtTTY(hvio: HVIO, cb: ULONG, pch: *const u8) -> APIRET;
```
Rust/stdcall pushes pch first (deepest), then cb, then hvio (top) → ESP+4=hvio ✓
