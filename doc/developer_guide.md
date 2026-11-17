# Warpine Developer Guide

Warpine is an OS/2 compatibility layer for Linux. It loads 32-bit OS/2 executables (LX format) and runs them natively using KVM hardware virtualization — analogous to WINE for Windows, but targeting OS/2 instead.

This guide introduces the internals of Warpine and the OS/2 concepts it emulates. Why the main design choices were made is recorded in the architecture decision records in [`doc/adr/`](adr/README.md).

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

**Module:** `src/ne.rs` (orchestration) and `src/ne/header.rs` (binary structures)

The NE (New Executable) format is used by OS/2 1.x 16-bit applications. Warpine now supports both LX (32-bit) and NE (16-bit) execution. The NE parser in `src/ne/` provides the binary structures; `src/loader/ne_exec.rs` provides the full NE loader and 16-bit execution path.

The parser handles:

- **MZ stub detection** — Same as LX: reads `e_lfanew` to find the NE header.
- **NE header** — Magic (`NE`), linker version, segment/resource/module table offsets and counts, entry table offset, flags, initial CS/IP and SS/SP.
- **Segment table** — Each `NeSegmentEntry` describes a 16-bit segment: file offset, size, flags (code/data/movable/preload), and minimum allocation size.
- **Relocation records** — Per-segment relocation entries with source type (byte/word/far) and targets (internal, imported ordinal, imported name, OS fixup).
- **Export table** — Entry table and exported name table mapping names to entry points.
- **Import table** — Module reference table listing names of imported DLLs.
- **Resource table** — Resource type/name table mapping resource IDs to file offsets.

The parser has 17 unit tests (`src/ne.rs`, `src/ne/header.rs`). NE loading and execution is implemented in `src/loader/ne_exec.rs` — see [§20 Appendix: Development Phases](#appendix-development-phases) for details.

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

During loading, `setup_stubs()` fills an 18432-byte region (`STUB_AREA_SIZE`) at `MAGIC_API_BASE` with INT 3 (0xCC) instructions. When the LX loader encounters an import fixup for a known module, `resolve_import()` maps it to a specific address:

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
| UCONV | `UCONV_BASE` (12288) | `MAGIC_API_BASE + 12288 + ordinal` | 12288–13311 |
| SO32DLL | `SO32DLL_BASE` (13312) | `MAGIC_API_BASE + 13312 + ordinal` | 13312–14335 |
| TCP32DLL | `TCP32DLL_BASE` (14336) | `MAGIC_API_BASE + 14336 + ordinal` | 14336–15359 |
| MOUCALLS | `MOUCALLS_BASE` (15360) | `MAGIC_API_BASE + 15360 + ordinal` | 15360–16383 |
| PMSHAPI | `PMSHAPI_BASE` (16384) | `MAGIC_API_BASE + 16384 + ordinal` | 16384–17407 |
| HELPMGR | `HELPMGR_BASE` (17408) | `MAGIC_API_BASE + 17408 + ordinal` | 17408–18431 |

A new module takes the next 1024-ordinal slot and grows `STUB_AREA_SIZE`; the compile-time asserts in `constants.rs` keep each range 1024 wide, the area below `DYNAMIC_ALLOC_BASE` and addressable by the NE loader's 16-bit offsets. Every place that lists the modules must follow: `resolve_import` / `resolve_import_by_name` (`descriptors.rs`), the range of the previous last module in `api_dispatch.rs`, `vcpu.rs` (Pascal argument pop) and `api_trace.rs`, `lx_loader.rs` `BUILTIN_MODULES`, `stubs.rs` `BUILTINS` (`DosLoadModule`), `lx_link.rs`, `gen_api.rs` (`MODULES`, `base_const` / `upper_const`, `resolve_const`, the `check` / `validate-doc` module lists) and `targets/os2api.def`.

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
| **Memory** | DosAllocMem, DosFreeMem, DosAllocSharedMem, DosGetNamedSharedMem, DosGetSharedMem, DosSetMem, DosQueryMem, DosSubSetMem, DosSubAllocMem, DosSubFreeMem, DosSubUnsetMem |
| **Threading** | DosCreateThread, DosSleep, DosWaitThread, DosGetInfoBlocks, DosEnterCritSec, DosExitCritSec |
| **Timers** | DosAsyncTimer, DosStartTimer, DosStopTimer |
| **Modules** | DosLoadModule, DosFreeModule, DosQueryModuleHandle, DosQueryProcAddr, DosQueryModuleName, DosSearchPath |
| **Process** | DosExecPgm, DosWaitChild, DosKillProcess, DosQueryAppType, DosExit, DosExitList |
| **Semaphores** | DosCreateEventSem, DosCloseEventSem, DosPostEventSem, DosWaitEventSem, DosOpenEventSem, DosResetEventSem, DosQueryEventSem, DosCreateMutexSem, DosCloseMutexSem, DosRequestMutexSem, DosReleaseMutexSem, DosOpenMutexSem, DosQueryMutexSem, DosCreateMuxWaitSem, DosOpenMuxWaitSem, DosCloseMuxWaitSem, DosWaitMuxWaitSem, DosAddMuxWaitSem, DosDeleteMuxWaitSem, DosQueryMuxWaitSem |
| **Queues** | DosCreateQueue, DosOpenQueue, DosWriteQueue, DosReadQueue, DosCloseQueue, DosPurgeQueue, DosQueryQueue |
| **Pipes** | DosCreatePipe |
| **System** | DosQuerySysInfo, DosGetDateTime, DosQueryCp, DosQueryCtryInfo, DosQueryDBCSEnv, DosMapCase, DosQueryCollate, DosDevConfig, DosDevIOCtl |
| **Audio** | DosBeep (real sine-wave tones via MMPM/2 beep_tone) |
| **Stubs** | DosError, DosSetMaxFH, DosSetExceptionHandler, DosLoadModule, DosStartSession, and others |

All blocking operations (DosSleep, DosWaitEventSem, DosRequestMutexSem, DosWaitMuxWaitSem, DosReadQueue, DosWaitThread) check `Loader::shutting_down()` in 100 ms intervals to ensure clean shutdown: true once `exit_requested` is set, and on every thread but the exit-list thread while an exit list runs.

#### Timers (src/loader/dostimer.rs)

`DosAsyncTimer` / `DosStartTimer(msec, hsem, phtimer)` start one host thread per timer (`run_timer`), registered in `SharedState::dos_timers` (`DosTimerTable`, at most `MAX_DOS_TIMERS` = 64 → `ERROR_TS_NOTIMER`; handles from `0x00010001`, never 0). The thread sleeps on its `TimerStop` (the `WinStartTimer` stop signal) until the next deadline `start + k × interval` — no drift, no busy waiting, at most `EXIT_POLL` (100 ms) at a time so it sees `exit_requested` — and then posts the event semaphore like `DosPostEventSem` (post counts add up). The interval is rounded up to whole `DOS_TIMER_TICK_MS` (32 ms) clock ticks, at least one; `DosQuerySysInfo(QSV_TIMER_INTERVAL)` reports that tick as 320 (tenths of a millisecond). The semaphore must be a shared event semaphore (`SemaphoreManager::is_shared_event`: `DC_SEM_SHARED` or named) or the call fails with `ERROR_TS_SEMHANDLE`. A one-shot timer leaves the table once it has posted, a timer whose semaphore was closed ends by itself, `DosStopTimer` wakes and removes the thread (`ERROR_TS_HANDLE` for unknown or finished handles), and `request_process_exit` stops every timer.

#### DosSearchPath and DosQueryModuleName (src/loader/modpath.rs)

`DosSearchPath` splits the path (`split_path`: `;`, `"…"` quotes a `;`, empty elements dropped, at most 256), puts `.` first for `SEARCH_CUR_DIRECTORY`, reads the path from the environment block (`env_value`, bounded to 64 KB) for `SEARCH_ENVIRONMENT`, and tries each element through the `DriveManager` like `DosOpen`: `resolve_path` (elements that do not resolve are skipped), then `query_path_info` (files only) or, for a name with `*` / `?`, `find_first` with hidden / system files. The result is `qualify`d (`X:\DIR\NAME`, `.` / `..` removed, the caller's spelling of the name, wildcards kept) and written only when it fits `cbBuf` with its NUL (`ERROR_BUFFER_OVERFLOW`). `DosQueryModuleName` names module handle 0 — the handle Warpine gives the executable (`EXE_MODULE_HANDLE`) — as its command-line name `os2_exe_path()` (`C:\NAME.EXE`, also used by `setup_guest`), a loaded DLL by the OS/2 path `load_dll_impl` records (`DllManager::set_module_path`, dropped by the last `DosFreeModule`; `dll_os2_path`: beside the executable → `C:\FILE`, on a drive → `DriveManager::host_to_os2_path` (canonicalised), else `C:\OS2\DLL\FILE`), and a built-in module loaded with `DosLoadModule` as `C:\OS2\DLL\NAME.DLL`; a name that does not fit `cbName` is `ERROR_BAD_LENGTH`.

#### Memory suballocation (src/loader/suballoc.rs)

`SharedState::suballoc` (`SubAllocTable`) keeps every pool on the host, keyed by its base address: size, `DOSSUB_SPARSE_OBJ`, attach count and a merged free list (`BTreeMap` start → length). Nothing is read from or written to the pool's memory except the block address `DosSubAllocMem` returns, so a guest cannot corrupt the bookkeeping; the first 64 bytes (`DOSSUB_CONTROL_SIZE`, OS/2's control information) stay unused so that the documented limits hold (pool ≥ 72 bytes, block ≤ pool − 64). Sizes round down (pool) or up (blocks) to 8 bytes; blocks come first fit, lowest address; a free that overlaps free space is `ERROR_DOSSUB_OVERLAP`. Bounds: pools lie inside guest memory and do not overlap, at most `MAX_SUBALLOC_POOLS` (1024) pools and `MAX_FREE_RANGES` (65536) free ranges per pool (`ERROR_NOT_ENOUGH_MEMORY`). `DOSSUB_SPARSE_OBJ` is only recorded (Warpine commits everything) and `DOSSUB_SERIALIZE` changes nothing (every call holds the table lock). The module comment lists the error-code choices.

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

Window management APIs — 127 ordinals (see the reference manual for the list) including WinInitialize, WinCreateMsgQueue, WinRegisterClass, WinCreateStdWindow, WinGetMsg, WinDispatchMsg, WinSendMsg, WinPostMsg, WinDefWindowProc, WinBeginPaint, WinEndPaint, WinMessageBox, WinStartTimer, WinStopTimer, clipboard operations, and dialog support.

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

Graphics Primitive Interface, 67 ordinals (see the reference manual for the list): presentation spaces, colours and mixes, lines, boxes, arcs, area patterns, line widths, text and fonts, read-back, colour tables, and — in `pm_bitmap.rs` — device contexts, bitmaps and bit-block transfers. Arc geometry and the pattern table are in `pm_shapes.rs`. How drawing reaches the screen: "Rendering Pipeline" below.

---

### Sockets (SO32DLL / TCP32DLL)

`tcpip.rs` maps the BSD socket calls onto Linux sockets. OS/2's TCP/IP uses BSD numbering for socket options (toolkit `<sys/socket.h>`: `SOL_SOCKET` 0xFFFF, `SO_REUSEADDR` 4, `SO_TYPE` 0x1008, …), which `linux_sockopt` translates; only options whose value has the same layout on both sides (an `int`, `struct linger`) are mapped, others (`SO_SNDTIMEO` / `SO_RCVTIMEO`, a 32-bit `timeval` on OS/2) fail with `SOCENOPROTOOPT`. Every call passes Linux the length of the host buffer Warpine actually copied, never the guest's claimed length (`setsockopt` values are at most `MAX_SOCKOPT_LEN`). `select` cannot report a socket yet: sockets are numbered from 0xA000 while its `fd_set` translation covers handles 0–63, and the 32-bit `fd_set` layout is unsettled (doc/TODOs.md).

### Profiles (PMSHAPI, src/loader/profile.rs + pmshapi.rs)

`profile.rs` is the pure model: a `Profile` holds applications, each with keys and byte values, in insertion order; names are the guest's bytes (never decoded) and compared case-sensitively. Deleting the last key of an application removes the application. `name_list` builds the double-NUL-terminated lists the enumerating calls return; `parse_profile_int` is `PrfQueryProfileInt`'s conversion (optional `-`, digits up to the first other character).

The real OS/2 INI binary layout is not documented by IBM, so Warpine stores profiles in its own text format, ASCII and binary-safe:

```text
; warpine-profile 1
["PrfTest"]
"Greeting" = "hello\0"
"Blob" = "\0\x01\x02\xff\"\0"
```

Names and values are quoted byte strings (`\\`, `\"`, `\0`, `\t`, `\n`, `\r`, `\xHH`; other printable ASCII as is). A file without the first line — for example a real binary INI a program ships — is `StoreError::Corrupt` rather than misread. Files are read and written through the VFS backend of their drive (`load` / `save` / `exists`: sandboxed like `DosOpen`; at most `MAX_PROFILE_FILE` bytes read), and a save writes `NAME.~wp` next to the file and renames it over it, so a profile is never half-written.

`pmshapi.rs` holds the `Prf*` handlers and `ProfileTable` (`SharedState::profiles`, locked before `drive_mgr`): `HINI_USERPROFILE` is `C:\OS2\OS2.INI`, `HINI_SYSTEMPROFILE` `C:\OS2\OS2SYS.INI` (`C:\OS2` is created on the first write), `HINI_PROFILE` reads both (user first) and writes the user profile; `PrfOpenProfile` handles start at `FIRST_PROFILE_HANDLE` and map to an absolute OS/2 path. Every call loads the file again and every change saves it, so several processes sharing a profile see each other's writes. Guest strings are read as raw bytes with `prf_string` (bounded by `MAX_PROFILE_VALUE`, NULL → `None`, unterminated → `PMERR_INVALID_PARM`); every output goes through `guest_write_bytes`, and only as many bytes as the guest's buffer size and the data allow. Failures record a `PMERR_*` code for `WinGetLastError` (`pm_error`).

Tests: `profile.rs` (model, format, round trip of all 256 byte values, VFS storage on a temporary drive), `pmshapi.rs` (the calls on a mock loader whose C: is a temporary directory), `samples/pm_profile_test` + `gui_pm_profile_test_reads_and_writes_profiles` (runs with `XDG_DATA_HOME` in a temporary directory and checks the files left on C:).

### Help Manager (HELPMGR, src/loader/helpmgr.rs)

There is no help viewer. `WinCreateHelpInstance` creates a hidden object window of class `#HelpInstance` (its procedure, in `dispatch_builtin_control`, returns 0 for every message, so `HM_*` messages are accepted and ignored), records the readable `HELPINIT` fields (title, library, tutorial, accelerator / action-bar ids, the help table — `MAKELONG(id, 0xFFFF)` is a resource id in `hmodHelpTableModule`) in `WindowManager::help` (`HelpRegistry`), clears `ulReturnCode`, and logs once that help is not available. `WinAssociateHelpInstance` maps an application window to an instance (NULLHANDLE removes it), `WinQueryHelpInstance` searches the window's parent chain and then its owner chains (`find_help_instance`), `WinLoadHelpTable` / `WinCreateHelpTable` append to the instance's tables, `WinDestroyHelpInstance` destroys the window; `WindowManager::destroy_window` forgets the instances and associations of destroyed windows. A viewer would need: an INF/HLP reader (the IPF compiled format is not publicly documented by IBM; the osFree / NewView descriptions would have to be checked for clean-room use), parsing of `RT_HELPTABLE` / `RT_HELPSUBTABLE` resources, F1 → `WM_HELP` routing through the help table to a panel, and the `HM_*` notifications (`HM_HELPSUBITEM_NOT_FOUND`, `HM_INFORM`, `HM_ERROR`) back to the application.

## Threading Model

Each OS/2 thread maps to a native Rust thread with its own KVM vCPU. The thread table lives in `loader/threads.rs` (`SharedState::threads`): `ThreadTable` records running threads, ended threads not yet waited for (at most `MAX_ENDED_THREADS`), kill flags, each created thread's guest stack and TIB, and the idle vCPUs of ended threads; the `Threads::ended` condition variable is signalled whenever a thread ends.

- **Ids.** Thread 1 is `MAIN_TID` on vCPU id `MAIN_VCPU_ID` (0). Created threads get ids from 2, which are not reused, and use their id as vCPU id.
- **`DosCreateThread`** allocates the stack (64 KB default, `DEFAULT_THREAD_STACK`) and a TIB with TIB2 (exception chain `XCPT_CHAIN_END`, stack base / limit, thread id), takes an idle vCPU if there is one (KVM vCPUs cannot be destroyed and their ids are limited), starts it with fresh registers at the thread function with `EXIT_TRAP_ADDR` as return address, and returns an error code instead of panicking when memory or vCPUs run out. Host threads are detached. `CREATE_SUSPENDED` gives the thread a suspend count of 1 before its host thread starts; `STACK_COMMITTED` changes nothing (Warpine's memory is always committed); other flag bits are ignored.
- **Ending.** `run_vcpu` = `vcpu_loop` + `end_thread`. `vcpu_loop` returns `ThreadEnd::Thread(rc)` for `DosExit(EXIT_THREAD)` (`ApiResult::ExitThread`), a return to `EXIT_TRAP_ADDR` from a created thread, or a kill flag, and `ThreadEnd::Process` when the process ends (`DosExit(EXIT_PROCESS)` is `ApiResult::ExitProcess`; process-level events go through `end_process` / `request_process_exit`, where the first result code wins). `end_thread` destroys the thread's PM message queue, frees its stack and TIB, parks its vCPU for reuse and, for the last thread, ends the process with that thread's code. Thread 1 returning from the entry point still ends the process. `run_vcpu` records the thread's initial ESP for exit-list routines.
- **Exit lists** (`loader/exitlist.rs`, `Threads::exit_list` + `exit_list_active`; lock order `table` → `exit_list`). `ExitList` is pure bookkeeping (routines by order, last added first; `start` once; `take_next` from the live list, so routines can add / remove others). The process ending — `ApiResult::ExitProcess`, thread 1 at `EXIT_TRAP_ADDR`, an unhandled exception (`end_by_exception`, `TC_EXCEPTION`, code 1) — calls `begin_exit_list` on the ending thread: when thread 1 still runs and the list is not empty, `claim_exit_list` records an `ExitRun` (vCPU, host thread, termination code, exit code, initial ESP), the callback stack is dropped and `next_exit_routine` points the vCPU at the first routine (`[ESP] = EXIT_TRAP_ADDR`, `[ESP+4]` = termination code, ESP = initial ESP − 8, the exception chain emptied). `DosExitList(EXLST_EXIT)` (`ApiResult::ExitListNext`), `DosExit`, a return to `EXIT_TRAP_ADDR` or an unhandled exception in the routine move on to the next; when none is left `request_process_exit` ends the process with the recorded code. Thread 1 ending as the last thread claims the list in `end_thread` under the table lock (so two threads ending at once cannot both miss it) and re-enters `vcpu_loop` (`resumed`). Meanwhile every other thread leaves `vcpu_loop` at its next exit (`stopped_by_exit_list`) without setting `exit_requested`, `shutting_down()` is true for them (`Threads::exit_list_stops_caller`, by host thread id) so their blocking calls return, and `wait_for_process_end` waits for `exit_requested` itself. `begin_exit_list` kicks the other threads, so those spinning in guest code stop too.
- **Critical sections** (`loader/critsec.rs`, `Threads::crit` (`CritSection`: owner + nesting count, at most `MAX_CRITSEC_NESTING` = 0xFFFF → `ERROR_CRITSEC_OVERFLOW`; leaving at 0 → `ERROR_CRITSEC_UNDERFLOW`) + `crit_cond` + `crit_held` fast flag; lock order `crit` → `table` → `exit_list`). A vCPU can only be held when it exits to Warpine, so `vcpu_loop` calls `wait_crit_sec` at the top of every pass (before re-entering the guest) and again before dispatching an API call (for a thread that was already running guest code when the section began; giving up there `continue`s with RIP still on the thunk). The wait ends when the section ends, the process ends (`shutting_down()`, including an exit list on another thread) or the thread is killed; `request_process_exit`, `claim_exit_list` and `DosKillThread` wake the waiters. `end_thread` ends a section its thread was in. Entering a section kicks the other threads, so a thread spinning in guest code stops at once.
- **Waiting.** `DosWaitThread` supports `DCWW_WAIT` / `DCWW_NOWAIT`, `*ptid == 0` for any thread, reports an already-ended thread at once (once), and returns `ERROR_INVALID_THREADID` for thread 1 or the caller. `DosKillThread` sets the kill flag and kicks the thread; it ends at once if it runs guest code, after the call if it is blocked in one, and on resume if it is suspended. The front ends call `wait_for_process_end()` after thread 1's `run_vcpu` returns, because thread 1 may end before the others.
- **Kicking a vCPU** (`VcpuKick`, `VcpuBackend::kicker()`; `ThreadTable::set_kicker` / `kick` / `kick_all_but`, registered by `run_vcpu`, dropped by `ThreadTable::end`). A kick makes a vCPU that runs guest code leave it with `VmExit::Interrupted`; the vCPU loop then `continue`s to the top of the pass, which looks at the flags (critical section, process end, exit list, suspension, kill, signals) and re-enters the guest with its registers untouched. KVM (`kvm_backend.rs`, `KvmKick`) follows the Linux KVM API's `immediate_exit`: the kicker stores 1 in the vCPU's `kvm_run.immediate_exit` byte (an `AtomicU8` over the mapped page, behind a mutex that `KvmVcpu::drop` clears before the page is unmapped) and then sends the real-time kick signal (`host_signal::kick_signal()`, `SIGRTMIN + 2`, no-op handler with `SA_RESTART`) with `tgkill` to the host thread recorded in `host_tid` while it is inside `KVM_RUN`. A vCPU about to enter `KVM_RUN` sees the byte and returns `EINTR` at once; one inside is interrupted by the signal. On `EINTR` the vCPU thread clears the byte before the loop looks at its flags, so a later kick is never lost. Several kicks before an exit count as one. The mock backend's `MockKick` counts kicks and makes the next `run()` return `Interrupted`. Callers: `enter_crit_sec` (all but the owner), `DosKillThread`, `DosSuspendThread`, `begin_exit_list`, `request_process_exit` (all), a signal for thread 1. A thread blocked in a host call is not interrupted (its call returns first).
- **Suspension** (`DosSuspendThread` 238 / `DosResumeThread` 237, `stubs.rs`; `ThreadTable::suspend` / `resume` / `suspend_count`, `Threads::wait_while_suspended` + `resumed` condvar). Suspensions count; the thread runs again at 0. `vcpu_loop` waits at the top of each pass while the count is above 0 (after the critical-section, process-end and exit-list checks, before the kill check: a killed suspended thread ends on resume), then looks at all flags again. The process ending or an exit list on another thread releases the wait. Suspending the calling thread or a thread that does not run is `ERROR_INVALID_THREADID`; resuming a thread that is not suspended `ERROR_NOT_FROZEN`. Sample `thread_control_test`.
- **Unverified** (doc/TODOs.md, "Thread follow-ups"): thread 1 calling `DosExit(EXIT_THREAD)` while others run lets the process go on until its last thread ends.

All threads share the same `SharedState` via `Arc`. Managers are protected by `Mutex` with a documented lock ordering to prevent deadlocks:

```
Level 1: threads                    (thread table)
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

`SemaphoreManager` keeps every semaphore in one handle space (`SemEntry`: the object, upper-cased name, attributes, usage count). Creating with a name checks `\SEM32\` (`ERROR_INVALID_NAME`) and uniqueness across kinds (`ERROR_DUPLICATE_NAME`); `open(kind, name, handle)` adds a use (`ERROR_SEM_NOT_FOUND`, `ERROR_TOO_MANY_OPENS`), `close` removes one and the last deletes the semaphore. The `Loader` handlers in `doscalls.rs` (`open_sem` for the three `DosOpen*Sem`) only move data between guest memory and the manager.

- **Event semaphores** — `EventSemaphore::post_count` (`Arc<(Mutex<_>, Condvar)>`): `post` counts (`ERROR_ALREADY_POSTED` from the second post, `ERROR_TOO_MANY_POSTS` at `MAX_EVENT_POSTS`) and wakes waiters, `reset` returns the count (`ERROR_ALREADY_RESET` at 0), `DosQueryEventSem` reads it. Waits go through `Loader::wait_sem` (condvar, 100 ms process-end polls, `ERROR_TIMEOUT`).
- **Mutex semaphores** — `owner_vcpu` (Warpine vCPU id; `tid_of_vcpu` for `DosQueryMutexSem`) + `request_count`; `available_to` / `acquire`. Created owned, the creating thread owns it.
- **MuxWait semaphores** — `MuxWaitSemaphore { records, kind, wait_all }` behind a `Mutex` so that `DosAddMuxWaitSem` / `DosDeleteMuxWaitSem` change it; `check_records` enforces 64 records, one kind (`ERROR_WRONG_TYPE`; no muxwait in a list) and no duplicates; `check_mux_attr` wants exactly one of `DCMW_WAIT_ANY` / `DCMW_WAIT_ALL`. `try_mux` (under `sem_mgr`, locking the members) decides one poll: events by `posted()`, mutexes by `available_to` and then taken (all of them for wait-all); `DosWaitMuxWaitSem` polls it every 10 ms. `query_mux` feeds `DosQueryMuxWaitSem` (`ERROR_PARAM_TOO_SMALL` + count).

### Queues

OS/2 queues (`DosCreateQueue` / `DosReadQueue`) are named, prioritized message queues. Each `OS2Queue` has a `VecDeque<QueueEntry>` and a condvar for blocking reads. `DosWriteQueue` pushes an entry and signals the condvar; `DosReadQueue` dequeues (FIFO or LIFO depending on queue attributes) or blocks if empty.

---

## Presentation Manager (GUI)

**Modules:** `src/gui/renderer.rs`, `src/gui/sdl_renderer.rs`, `src/gui/headless.rs`, `src/loader/pm_win.rs`, `src/loader/pm_gpi.rs`, `src/loader/pm_types.rs`

The Presentation Manager (PM) is OS/2's GUI subsystem. Warpine implements it using **SDL** (window management, event handling, and hardware-accelerated framebuffer rendering via streaming textures).

### Three Execution Paths

`main.rs` selects an execution mode based on the executable's imports and environment:

- **CLI apps in a PM window** — default for apps that do not import `PMWIN` when stdout is a terminal (`run_vio_session` in `main.rs`, `VioDisplay::Window`). The PM desktop runs on the main thread with a host-owned frame showing the VIO buffer (see "VIO programs in a PM window" below); the vCPU runs on a worker thread.
- **CLI apps full screen (SDL text window)** — `WARPINE_VIO=fullscreen`, or a program whose LX module flags say "incompatible with PM windowing". `SdlTextRenderer` runs `run_text_loop()` on the main thread (required by SDL), rendering a 640×400 VGA text window.
- **CLI apps (terminal / headless)** — `WARPINE_HEADLESS=1` or stdout not a terminal (`headless_requested`). The vCPU runs on the main thread; terminal ANSI output and raw-mode keyboard are used.
- **PM apps** — SDL event loop runs on the main thread. The vCPU is spawned on a worker thread. Drawing goes through shared per-frame surfaces (see Rendering Pipeline); the few requests only the GUI thread can carry out — host clipboard, mouse capture — go through a channel (`GUISender` / `GUIReceiver`).

### Window Lifecycle

1. **WinInitialize** — Returns a handle to the anchor block (HAB).
2. **WinCreateMsgQueue** — Creates a `PM_MsgQueue` and maps the calling thread to it (`tid_to_hmq`). One queue per thread, as the PM reference says: a second call returns `NULLHANDLE`.
3. **WinRegisterClass** — Stores a `WindowClass` with the guest's window procedure address.
4. **WinCreateStdWindow** — Creates frame and client `OS2Window` entries in the `WindowManager` and the frame's drawing surface (from then on the compositor shows the frame), and returns the frame HWND.
5. **WinGetMsg** — Blocks on the message queue condvar until a message is available. Returns FALSE on WM_QUIT (ending the message loop).
6. **WinDispatchMsg** — Invokes the guest's window procedure via the callback mechanism (see below).
7. **WinDestroyMsgQueue / WinTerminate** — Cleanup. `WindowManager::destroy_mq` removes the queue and its pending messages if it is the calling thread's own queue (only the creating thread may destroy it; otherwise FALSE); the thread may then create a new one. Windows of the queue are not destroyed (the reference does not say they are); `WinPostMsg` to them returns FALSE and messages for them are dropped. `WinGetLastError` does not report the `PMERR_*` codes yet.

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
- *Regions and clipping* (`gui/region.rs`, `pm_region.rs`). A `Region` is a set of pels as y-bands (`y0..y1`, exclusive at the top, sorted bottom-up, never empty) of sorted, disjoint, non-touching x spans (exclusive at the right); touching bands with equal spans are merged, so equal sets compare equal. `Region::combine` sweeps the y breakpoints of both operands and combines the spans of each band (`SetOp` Or / Copy / Xor / And / Diff); `from_rects`, `from_rows`, `offset` (saturating at ±2²⁷), `overlap` (`RRGN_*`), `frame` (erosion by the box as erosion of the rows and then, transposed, of the columns) and `rects(RectOrder)` build on it; every result is bounded by `MAX_REGION_RECTS` (16384, else the call fails). Regions live in `WindowManager::regions` (`RegionTable`, at most `MAX_REGIONS`); a presentation space holds its clip region's handle (`clip_region`), its clip path as pels (`clip_path`) and their intersection (`clip`, recomputed by `WindowManager::refresh_clip`), which `ps_target` puts in `PsTarget::clip`. Every drawing call through a PS goes through `Loader::draw_ps` → `draw_clipped` → `Surface::draw_clipped`: with no clip it is `Surface::draw`; otherwise the area the operation can touch (`op_bounds`) is saved, the operation applied and the pels of that area outside the clip spans put back — one generic, pel-exact path for every `DrawOp` (mixes, patterns, text, blits), skipped entirely when the clip holds or misses the whole area.
- *Areas, paths and polygons* (`pm_area.rs`, `DrawOp::Figures`). Inside an area or path bracket `Loader::figure_part` adds a primitive's points to the bracket's `Figures` (current figure from the current position; `GpiMove` ends it; boxes and full arcs are closed figures; at most `MAX_SHAPE_POINTS`). `DrawOp::Figures` rasterises several figures in one pel mask like `DrawOp::Shape` (which is now its one-figure case): the inside by `FillRule::Alternate` or `Winding` (`region::scan_row`: crossings sorted, a running winding count), plus the pels of a `boundary`-wide square brush along the edges (1 = the inclusive model, 0 = `Edges::TopLeft`, leaving out the bottom and right edges, `BA_EXCL` / `FPATH_EXCL` / `POLYGON_EXCL`; wider = `GpiStrokePath`'s geometric line, up to `MAX_BOUNDARY_WIDTH` 64), and the stroke on top. `Region::from_figures` scan-converts the same way into rows for clip paths and `GpiPathToRegion` (within ±16384). Paths are `PathState::Open` / `Defined` / `Stroked(width)` (`GpiModifyPath`) and are taken by the call that uses them. Curves come from `pm_shapes.rs`: `cubic` (`GpiPolySpline`), `conic` (rational quadratic Bézier, weight = the reference's sharpness WD/DB, `GpiPolyFilletSharp`) and `poly_fillet` (parabolic fillets between the midpoints of the inner lines), 4–256 points each.
- *Patterns and line widths.* `GpiSetPattern` (`PATSYM_*`; the 8 × 8 bits are Warpine's own, the PM reference only pictures them: `DENSE1`–`8` ordered dither from 7/8 to 1/16, lines 4 pels apart for `VERT` / `HORIZ` / `DIAG*`, `DIAG2`/`4` doubled) and `GpiSetPatternRefPoint` give the `Pattern` of box and arc fills (set pels: foreground colour and mix; clear pels: background colour with the background mix, left alone by default) and the P operand of `GpiBitBlt` raster operations. `GpiSetLineWidth` (FIXED multiple of normal) rounds to whole pels (`cosmetic_pels`) for lines, box outlines and arcs; `GpiSetLineWidthGeom` sets the width of stroked paths (`GpiStrokePath`, `GpiModifyPath`). Plain boxes and 1-pel solid lines keep the cheaper `DrawOp::Box` / `DrawOp::Line`; patterned fills and wide or styled box outlines go through `DrawOp::Shape`.
- *Lines and text.* `GpiSetLineType` styles `GpiLine`, box outlines and arcs with a 16-pel mask (`DrawOp::Line::style`, `Stroke::style`; the dash lengths are Warpine's choice, `LINETYPE_INVISIBLE` draws nothing). A background mix other than leave-alone (`GpiSetBackMix`) fills a character string's box with the background colour before the text.
- *Presentation-space lifetime.* `WinBeginPaint` without a PS creates one that `WinEndPaint` frees, and reports the update rectangle (`pm_paint.rs`), which it empties; `WinGetPS` / `WinReleasePS` pair (`PsOrigin`). `DevQueryCaps` describes a 24-bit display the size of the desktop at 96 dpi.
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

**Guest-side benchmark.** `samples/pm_bench` measures what an application sees: GPI calls issued per second inside a paint, `WinSendMsg` and `WinPostMsg` round trips, `WinInvalidateRect` → `WM_PAINT` cycles, and the `WM_TIMER` rate of a 10 ms timer, each timed with `DosQuerySysInfo(QSV_MS_COUNT)`. GPI calls rasterise on the calling thread, so its GPI figures include drawing the pixels (compositing is on the GUI thread; read it from the frame statistics). `samples/pm_pel_test` checks read-back and mix modes from the guest side (`GpiSetPel`/`GpiBox`/`GpiLine` → `GpiQueryPel`, XOR / invert / leave-alone); `samples/pm_bitmap_test` checks memory presentation spaces, bitmaps, `GpiBitBlt`, `GpiLoadBitmap` + `WinDrawBitmap`, colour tables and line types; `samples/pm_shapes_test` arcs, area patterns and line widths; `samples/pm_region_test` regions, clipping, areas, paths, polygons and curves. To compare two builds without a display:

```bash
printf '4500 quit\n' > /tmp/bench.script
SDL_VIDEODRIVER=dummy WARPINE_GUI_SCRIPT=/tmp/bench.script \
  cargo run --release -- samples/pm_bench/pm_bench.exe
```

### Input Handling

The GUI thread translates SDL input events into OS/2 messages:
- Keyboard events → `WM_CHAR` in the CHRMSG layout (flags in the low word of `mp1`, repeat count in bits 16–23, scan code in bits 24–31; character in the low word of `mp2`, virtual key in the high word). Typed characters come from SDL `TextInput` events, so shifted and non-ASCII characters arrive correctly; `KeyDown` carries only non-character keys and Ctrl/Alt chords.
- Mouse movement → `WM_MOUSEMOVE`
- Mouse buttons → `WM_BUTTONnDOWN` / `WM_BUTTONnUP` / `WM_BUTTONnDBLCLK`
- Window resize → `WM_SIZE`
- Close request → `WM_CLOSE`

OS/2 uses a bottom-left coordinate origin (Y increases upward), so Y coordinates are flipped during translation.

**The system input queue (type-ahead).** Keyboard and pointer messages are not posted to a window's queue on arrival: they go into one queue in arrival order, `WindowManager::input` (`pm_input.rs`), as `InputEvent::Key` (WM_CHAR parameters), `InputEvent::Char` (a typed character) or `InputEvent::Window` (a pointer message for the window hit-tested on arrival). `WinGetMsg` / `WinPeekMsg` (`next_message`) and the modal dialog loop (`setup_dlg_dispatch`) read a queue through `WindowManager::next_queued`, in PM's order: posted messages in posting order, then input (`next_input(hmq, …)`), then `WM_PAINT`, then `WM_TIMER` (`pm_input::Rank`; the PM Reference ranks `WM_TIMER` "lower than all other messages except WM_SEM4" and the system's `WM_PAINT` / `WM_TIMER` below `WM_SEM2`, which is "lower than any other message"). For input, the oldest event is resolved at that moment — a key to `key_target()`, the focus window *now*, a character encoded in that window's queue codepage (`char_message`) — and delivered only if its window belongs to the reading queue; otherwise it stays, blocks the events behind it and wakes that queue's thread. So keys typed while the program is busy go to the window that has the focus once the program reads them — after a key whose handling called `WinSetFocus`, for example — as in PM, which handles one input event before routing the next. Events for destroyed windows and characters the codepage lacks are dropped; at most `MAX_INPUT_EVENTS` wait. Two things still happen on the GUI thread when a click arrives: activating another frame and taking the focus from a control when the click is beside it. Before either, `pin_pending_input()` turns the waiting keys into `InputEvent::Window` for their current target, because they were typed for the old focus. A click on a control changes nothing there: the control takes the focus when it handles the press, in order. The accelerator check (`translate_queued_accelerator`) runs on the message taken, as before. `samples/pm_typeahead_test` types ahead while its client sleeps and checks where the keys landed.

**Focus and built-in controls.** `WindowManager::focus_hwnd` is the PM focus window (`WinSetFocus` / `WinQueryFocus`); `key_target()` sends keys to it, falling back to the active frame's client. `route_button1()` delivers button-1 messages to the built-in control under the pointer (`control_at()`), otherwise to the client. `pm_controls.rs` handles those messages for `WC_ENTRYFIELD`, `WC_MLE` and `WC_LISTBOX` — click-to-focus, caret, typing, Backspace/Delete/arrows/Home/End, Enter and Up/Down in the MLE, list-box selection and scrolling — and notifies the owner with `WM_CONTROL` (`EN_CHANGE`, `MLN_CHANGE`, `LN_SELECT`). The editing itself is pure logic in `pm_edit.rs` (caret positions in characters, CRLF as one unit, click-x → caret index via `font::text_width`), which keeps it unit-testable without a window. **Selection and clipboard.** An `EditBuffer` holds an anchor besides the caret (`OS2Window::edit_anchor`); the selection runs between them. Movement keys with Shift keep the anchor (extend), without it drop the selection (Left / Right then stop at its start / end); typing, Enter, Backspace and Delete replace or remove it. Button 1 down in a text control makes it `WindowManager::pressed_control`; while held, the renderer sends it the pointer's `WM_MOUSEMOVE` and the release wherever the pointer is (`pressed_control_release()`), so dragging extends the selection (a list box selects the item under the pointer); buttons use the same field to pop up. The selection is drawn highlighted (`draw_edit_line`) while the control has the focus. `EM_SETSEL` / `EM_QUERYSEL` (`MPFROM2SHORT(first, last)`) and `MLM_SETSEL` / `MLM_QUERYSEL` (anchor and cursor insertion points, `MLFQS_*`) set and report it in characters. Cut / copy / paste / clear — the CUA keys Ctrl+Insert, Shift+Delete, Shift+Insert and `EM_` / `MLM_` `CUT` / `COPY` / `PASTE` / `CLEAR` — go through `WindowManager::clipboard_text`, which mirrors the host clipboard (copying sends it there and invalidates a guest `CF_TEXT` handle); pasting into an entry field stops at the first line break, control characters other than line breaks are dropped. Mouse button messages carry the Shift / Ctrl / Alt state in `fsInp` (high word of mp2; the renderer keeps it from the last key event in `SdlRenderer::key_mods`), so Shift+click extends the selection. While dragging, a pointer above or below an MLE or list box maps to the row just outside the view (`row_under_pointer`), so the view scrolls a row per pointer move (there is no timer: a pointer held still does not keep scrolling). PageUp / PageDown move by the visible rows less one (`EditBuffer::set_page_rows`; the MLE view scrolls by as many lines as the caret moved). A read-only control (`ES_READONLY` / `MLS_READONLY` in its style, toggled by `EM_SETREADONLY` / `MLM_SETREADONLY`) applies a key to a copy of its buffer and drops the result if the text would change, so movement and selection work; only Copy of the clipboard commands applies. **Focus notifications.** Every focus change goes through `WindowManager::set_focus`, which posts `EN_` / `MLN_` / `LN_` `KILLFOCUS` to the owner of the control losing the focus and `SETFOCUS` for the one gaining it (`notify_focus`; other windows have none); a dialog's first focus control is set directly at creation and notifies after `WM_INITDLG` (`queue_dialog_start`). `LM_DELETEITEM` / `LM_DELETEALL` keep the selection on its item, or clear it when the item goes. **Word wrap.** With `MLS_WORDWRAP` (`MLM_SETWRAP` sets the style bit) `edit_buffer()` gives the `EditBuffer` a wrap width — the text area less the caret — and `lines()` returns display rows (`pm_edit::wrap_rows`: break after the spaces that follow the last word that fits, spaces may hang past the edge, a word wider than the row breaks between characters). Everything that works on lines — Home / End, Up / Down, page keys, clicks, painting, vertical scrolling — then works on rows. At a soft wrap the caret position after the row's last character is the start of the next row, so End and vertical moves stop before the last character of a soft-wrapped row (`line_caret_end`). **List selection modes.** `OS2Window::listbox_sel` is a `pm_list::ListSelection` (selected set, cursor, anchor), changed through `Loader::listbox_update()` with the `ListMode` of the style (`LS_MULTIPLESEL` wins over `LS_EXTENDEDSEL`); the model keeps the mode rules (click / drag / cursor key / Space / `LM_SELECTITEM`) and the index shifts on insert and delete, and is unit-tested on its own. Space arrives twice from a real keyboard — `VK_SPACE` on the key-down, `' '` from text input — so the list toggles on `VK_SPACE` and swallows the bare character. **Double clicks.** The renderer turns every second press of an SDL multi-click (`clicks` even) into `WM_BUTTONnDBLCLK` (`button_down_msg`); `dispatch_builtin_control` handles `WM_BUTTON1DBLCLK` as a press, except that a list box then sends `LN_ENTER` when an item is under the pointer, and `WinDefWindowProc` forwards it to the menu bar like a press. **Caret blink.** `SdlRenderer::caret_next` schedules a blink every `CARET_BLINK_MS`; `WindowManager::post_caret_blink()` posts `WM_TIMER(TID_CURSOR)` to the focus window if it is an enabled entry field or MLE and none is pending, and the control flips `OS2Window::caret_off` and repaints. The interval is `WindowManager::cursor_rate_ms` (default `CARET_BLINK_MS`), which `WinQuerySysValue(SV_CURSORRATE)` reports and `WinSetSysValue` (873) changes (values ≥ 1; other indices fail with `PMERR_PARAMETER_OUT_OF_RANGE`, a Warpine restriction). While a control is held (`pressed_control`) with the pointer outside it, the renderer's `drag_scroll_step` posts `WM_TIMER(TID_SCROLL)` every `DRAG_SCROLL_MS` (100 ms, unverified) through `WindowManager::post_drag_scroll`, and the control replays the last pointer position (`drag_pointer`) through `control_mouse_move`, so a selection keeps scrolling while the pointer rests past the edge. Text is measured in the face it is drawn in: `pm_controls::system_font(cp)` / `measure_in(face)` and `Loader::control_face` give the CJK face of the control's queue codepage to caret positions, clicks, scroll offsets and word wrap (`pm_edit::Measure` is an `Rc<dyn Fn>` so the wrap can keep it); `WinMessageBox` wraps in the thread queue's face. The focused entry field or MLE records its caret (`WindowManager::text_caret`, a `TextCaret` in frame-surface coordinates) when painted; the renderer turns it into a desktop rectangle (`text_input_area`) and passes it to SDL (`TextInputUtil::set_rect`, i.e. `SDL_SetTextInputArea`) whenever it changes, so the host input-method window appears at the caret. Focus changes and input clear `caret_off`. A scripted session never blinks (`caret_next` is `None`), so golden dumps do not depend on the phase. **Coordinates.** A control's position is relative to its parent (the client, or a dialog), and a client's to its frame — (0, 0), below the title and menu bars; only frames carry desktop positions. `get_abs_rect_in_frame` adds up that chain, so moving a frame (`WindowManager::move_frame`, used by title-bar drags) must change the frame alone; the client and controls move with it. Dialogs use the same paths — see "Dialog System" below.

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

`parse_menu_template()` in `pm_win.rs` builds a `Vec<MenuItem>` tree. `WinLoadMenu` and `WinCreateMenu` create a menu pseudo-window carrying this tree (`create_menu_window()`): as the PM reference says, a child of the given frame with the frame as owner and identity `FID_MENU`, so `WinWindowFromID(frame, FID_MENU)` finds it. They do not show it: the PM Guide has the application send `WM_UPDATEFRAME` to the frame after loading a menu, and `WinDefWindowProc` handles `WM_UPDATEFRAME` with `FCF_MENU` on a standard frame by attaching the frame's topmost `FID_MENU` child, or none (`update_frame_menu()` → `attach_menu_to_frame()`, which stores the menu hwnd in `OS2Window::menu_hwnd`, re-lays out the frame, posts `WM_SIZE` / `WM_PAINT` to the client when its size changed and returns the previous handle). A menu bar replacement is therefore: `WinDestroyWindow` of the old `FID_MENU` window, `WinLoadMenu`, `WM_UPDATEFRAME(FCF_MENU)`. `WinDestroyWindow` of the attached menu (or of a window containing it) detaches it from its frame at once. `WinCreateStdWindow` with `FCF_MENU` creates and attaches its menu directly (also as `FID_MENU`). The reference does not describe the frame's response to `WM_UPDATEFRAME` beyond this, so other `FCF_*` flags are ignored; a dialog's `WM_UPDATEFRAME` goes to `WinDefDlgProc` and is not handled yet. (PMWIN has no `WinSetMenu` export.)

### Dialog System (WinDlgBox / DlgRunLoop)

**Binary DLGTEMPLATE format (RT_DIALOG = 4, `<pmwin.h>`), parsed by `pm_dialog::parse_dlg_template`:**

```
DLGTEMPLATE (14 bytes)                 DLGTITEM (30 bytes each)
  u16 cbTemplate                         u16 fsItemStatus
  u16 type                               u16 cChildren      -- descendants follow it
  u16 codepage   (0 = process cp)        u16 cchClassName   -- 0: offClassName is an atom (3 = WC_BUTTON)
  u16 offadlgti  -- first DLGTITEM       u16 offClassName
  u16 fsTemplateStatus                   u16 cchText, offText
  u16 iItemFocus (0xFFFF = none)         u32 flStyle
  u16 coffPresParams                     i16 x, y, cx, cy   -- dialog units, relative to the parent item
                                         u16 id, offPresParams, offCtlData
```

The first item is the dialog frame itself; the controls follow in tree order (each item followed by its `cChildren` descendants), and all strings live in a pool addressed by offsets from the start of the template. Texts are decoded with the template's codepage. The frame control data of the dialog item is one ULONG of `FCF_*` flags — what the Open Watcom resource compiler writes for `DIALOG ..., FCF_TITLEBAR | FCF_SYSMENU` (checked against the `.res` of `samples/pm_menu_dlg_test`) — and the border comes from the dialog's `FS_DLGBORDER` / `FS_BORDER` / `FS_SIZEBORDER` style. `iItemFocus` counts the dialog as item 0.

**Dialog windows.** `WinDlgBox` and `WinLoadDlg` share `load_dialog()` / `create_dialog()` in `pm_win.rs`: the dialog is a `#Dialog` window (`WindowManager::DIALOG_CLASS`) with the template's chrome, positioned relative to its parent's frame (the desktop for `HWND_DESKTOP`), and its controls are its children, owned by it. A dialog is a frame without a client: `find_frame_for_hwnd()` stops at it, `control_root()` returns it (a standard frame's controls live under its client), `get_abs_rect_in_frame()` maps control positions — relative to the dialog window, border included, as in PM — into its surface, which starts inside the left border, and `builtin_control_descendants()` never descends from an owner window into a dialog. The dialog's `WM_PAINT` (default processing) fills the face and paints each control with the same code as controls in client windows, so entry fields, list boxes and buttons in dialogs are fully interactive. The focus goes first to the template's `iItemFocus` control, else the first `WS_TABSTOP` control (`initial_dialog_focus()`); `WM_INITDLG` names it in mp1. While a modal loop runs (`WinDlgBox`, `WinProcessDlg`) the owner's frame is disabled (`begin_modal_dialog()`; the renderer ignores clicks on a disabled frame, `clicks_blocked()`). When the loop ends, however it ends, `finish_modal_dialog()` takes the dialog out of the Z-order and releases a focus or pressed button inside it (`close_dialog()`), enables the owner again and, for `WinDlgBox`, destroys the dialog (`destroy_window()`; a `WinProcessDlg` dialog stays for the program's `WinDestroyWindow`).

**Default dialog processing** (`Loader::def_dlg_proc`, `pm_dialog.rs`): `WM_PAINT` paints; `WM_CHAR` — Tab / Shift+Tab (`VK_BACKTAB`) move between tab stops (`next_tab_stop()`, wrapping), Enter / keypad Enter click the focused push button or else the `BS_DEFAULT` one (`default_button()`), Esc posts `WM_COMMAND(DID_CANCEL)`, the arrow keys move the focus within its group — from a `WS_GROUP` control up to the next (`next_in_group()`, wrapping) — and a mnemonic (`pm_dialog::mnemonic()`: the character after `~`) typed with Alt, or plainly when no control takes characters (a button has the focus), activates its control (`dialog_mnemonic()`: a button is clicked, a label passes the focus to the next control that can take it, as a field's label does); control labels are drawn by `control_label()` without the `~` and with that letter underlined; any `WM_COMMAND` dismisses the dialog with the command value; `WM_SYSCOMMAND(SC_CLOSE)` posts `WM_CLOSE`, which dismisses with `DID_CANCEL`. Keys a control does not use (Tab, Enter, Esc in an entry field) are posted to its owner, the dialog.

**Buttons** (`pm_controls.rs`): button 1 down holds a button (`WindowManager::pressed_button`, drawn darker) and gives it the focus unless `BS_NOPOINTERFOCUS`; the renderer sends the release to the held button wherever the pointer is (`pressed_button_release()`), and it clicks only when released over itself. Space clicks the focused button. A push button posts `WM_COMMAND` (mp1 = id, mp2 = `MPFROM2SHORT(CMDSRC_PUSHBUTTON, fPointer)`), or `WM_SYSCOMMAND` / `WM_HELP` with `BS_SYSCOMMAND` / `BS_HELP`, to its owner; other button kinds notify `WM_CONTROL(BN_CLICKED)`. The default button has a black outline, the focused push button a focus mark inside.

**DlgRunLoop modal loop:** When `WinDlgBox` or `WinProcessDlg` needs a modal dialog, they return `ApiResult::DlgRunLoop { dlg_proc, hwnd_dlg, hmq }`. The VMEXIT loop handles this via `setup_dlg_dispatch()` — a free function that mirrors `setup_exception_dispatch()`:

1. Saves the caller's RIP/RSP.
2. Blocks on the `MessageQueue` condvar until a message arrives or the dialog is dismissed.
3. If dismissed (`OS2Window::dialog_dismissed == true`): restores saved RIP/RSP, writes `dialog_result` to guest EAX, returns `false` (caller falls through).
4. Otherwise, like `WinGetMsg` + `WinDispatchMsg`: translates an accelerator keystroke, then hands a message for a built-in control to `dispatch_builtin_control` and loops, or pushes a `FrameKind::DlgRunLoop` frame and sets guest registers to call the target window's procedure — `dlg_proc` for the dialog, the owner's window procedure for a repaint behind it — and returns `true` (caller does `continue`).
5. At `CALLBACK_RET_TRAP`, the `DlgRunLoop` frame handler checks dismissal again and either returns to caller or calls `setup_dlg_dispatch()` again for the next message.

`WinDismissDlg` sets `dialog_dismissed = true` and `dialog_result = result`, then calls `condvar.notify_one()` to wake the blocked loop.

### Message boxes (WinMessageBox)

`WinMessageBox` (`pm_msgbox.rs`) builds a modal dialog in memory and runs it on the same modal loop as `WinDlgBox` (`ApiResult::DlgRunLoop`), so the rest of the desktop and the owner's own window procedure (timers, repaints) keep running while it is open; only the owner's frame is disabled. Before, it was SDL's native message box on the GUI thread, which froze the whole desktop until it closed.

- *Layout* (`layout()`, pure and unit-tested; sizes are Warpine's own): the text breaks at its line breaks and wraps at word boundaries past 400 px (`EditBuffer::set_wrap`), one `WC_STATIC` per row; below it the push buttons of the `MB_*` set (`buttons()`), 80 × 26 px, centred, the first in a `WS_GROUP` so the arrow keys move between them; the default button (`MB_DEFBUTTON1/2/3`, `default_button()`) has `BS_DEFAULT` and the focus. The dialog has a title bar (the caption), a system menu and a dialog border, and is centred on the desktop.
- *Behaviour*: the dialog has no dialog procedure (`pfn_wp == 0`), so the modal loop gives its messages to `dispatch_builtin_control`, which hands every message except `WM_PAINT` to `procless_dialog_message`: `WinDefDlgProc`, except that a `WM_COMMAND` ends the dialog only when one of its controls has that id, while `WM_CLOSE` ends it with `DID_CANCEL` (= `MBID_CANCEL`) whatever its buttons — the `WinMessageBox` reference: "If the user selects Close, the message box is removed and the usResponse is set to MBID_CANCEL, whether or not a cancel button existed". A push button's `WM_COMMAND(MBID_*)` therefore ends it with that value; Enter presses the default button; the close button and Alt+F4 cancel any box; Esc (`WM_COMMAND(DID_CANCEL)`) cancels only a box with a Cancel button (unverified).
- *Icons*: `icon(style)` maps the icon bits (`flStyle & 0xF0`: `MB_ICONHAND` / `MB_ERROR` / `MB_CRITICAL`, `MB_ICONASTERISK` / `MB_INFORMATION`, `MB_ICONEXCLAMATION` / `MB_WARNING`, `MB_ICONQUESTION` / `MB_QUERY`) to the `SPTR_ICON*` system pointer it stands for, stored in `OS2Window::msgbox_icon`; `layout()` reserves `MSGBOX_ICON_SIZE` (32) + 12 px left of the text (`icon_origin`: top-left of the content; text shorter than the icon is centred against it), and the `#Dialog` `WM_PAINT` blits `icon_pels(sptr)` — Warpine's own 32 × 32 pictograms after the reference's descriptions (red circle with a line across it, "i", "!", "?"), transparent outside the shape — onto the face before the controls (`paint_message_box_icon`). `MB_NOICON` and `MB_CUSTOMICON` show none.
- *Help*: `MB_HELP` appends a `BS_HELP` push button `MBID_HELP`; selecting it posts `WM_HELP(MBID_HELP, MPFROM2SHORT(CMDSRC_PUSHBUTTON, fPointer))` to the box ("a WM_HELP message is sent to the window procedure of the message box"), which stays open; `procless_dialog_message` would pass it on to a parent other than the desktop (the `WM_HELP` default), but a message box's parent is the desktop. The `HK_HELP` hook is not implemented.
- A thread without a message queue gets `MBID_ERROR`; a program without a PM desktop (a text-mode program, no GUI channel) gets `MBID_OK` at once.
- Not yet: `MB_CUSTOMICON` / `WinMessageBox2`, the `HK_HELP` hook, `MB_NONMODAL`, `MB_SYSTEMMODAL`, `MB_MOVEABLE`.

### Minimize, maximize and restore

`src/loader/pm_minmax.rs`. `WindowManager::begin_minmax(frame, op, pos, size)` works out the target rectangle — maximize: the desktop plus the frame's sizing border on every side, so the border lies just outside the screen (PM Guide: "the size of the frame window increases to the size of its parent window, plus an additional amount on each of its four sides equal to the width of its sizing border"); minimize: `MINIMIZED_CX` × `MINIMIZED_CY` at the remembered `QWS_XMINIMIZE` / `QWS_YMINIMIZE` or the first free slot of an icon area along the bottom of the desktop ("the system moves a minimized window into the first available icon position in the icon area"); restore: the `QWS_*RESTORE` window words, overridden by `SWP_MOVE` / `SWP_SIZE` — and stores the restore words when the frame leaves its normal state. `Loader::send_minmax` returns `ApiResult::MinMaxFrame`: `vcpu.rs` writes a 36-byte `SWP` on the guest stack below the caller and calls the client's procedure with `WM_MINMAXFRAME` (mp1 = the PSWP; the state has not changed yet, the window words have), and `FrameKind::MinMaxFrame` calls `WindowManager::finish_minmax` unless the reply is TRUE. `finish_minmax` sets the style bits, geometry, layout and surface, hides the frames and dialogs owned by a minimized frame or its client and shows them again afterwards (`sync_owned_frames`, `hidden_with_owner`), and posts `WM_SIZE` (old, new) and `WM_PAINT` (`post_size`). `WinDefWindowProc` runs it for `WM_SYSCOMMAND` `SC_MINIMIZE` / `SC_MAXIMIZE` / `SC_RESTORE` only for frames with `FCF_MINBUTTON` / `FCF_MAXBUTTON` (PM: "If a control with the identifier FID_MINMAX is present … otherwise nothing is done"); `WinSetWindowPos` does it for any standard frame. The renderer draws a minimized frame with `display_flags()` (title bar and border, title only, no menu), treats the whole of it as title bar (`frame_chrome_hit`), shows a restore pictogram on a maximized frame's maximize button (`FrameChrome::state`, `ButtonGlyph::Restore`), and does not resize maximized or minimized frames or drag maximized ones. A title-bar double click maximizes, or restores a minimized or maximized frame; a double click on the system-menu icon sends `SC_CLOSE`. Alt+F5 / Alt+F9 / Alt+F10 are in `SYSTEM_ACCELS` beside Alt+F4 (keys from the IBM OS/2 1.2 Quick Reference card).

- *Hide*: `FCF_HIDEBUTTON` (`FCF_HIDEMAX` = hide + maximize) draws a hide button in the minimize button's place (when a frame asks for both, only the hide button: unverified); its click posts `WM_SYSCOMMAND(SC_HIDE, CMDSRC_MENU)`, and `WinDefWindowProc` hides the frame (`hide_frame`, for any frame: "Sets the visibility state of the frame window to off"). Frames it owns are hidden with it and shown again when it is visible and not minimized (PM Guide, "Windows": "When the frame window is minimized or hidden, it hides all the windows it owns"); `WinShowWindow` / `SWP_HIDE` / `SWP_SHOW` go through the same `frame_visibility_changed`.
- *Activation*: when the active frame is minimized or hidden, `activate_next_frame` makes the topmost other visible, enabled, not minimized frame active and raises it, posting `WM_ACTIVATE(FALSE, new)` to the old frame's client and then `WM_ACTIVATE(TRUE, old)` to the new one's (`frame_msg_target`; `WinSetActiveWindow` and clicks post to the client too); with no other frame the minimized one stays active. PM documents the hand-over only for a destroyed window (the next Alt+Esc window), so this is unverified.
- *Size changes*: `WinSetWindowPos(SWP_SIZE)` posts `WM_SIZE` (mp1 old, mp2 new, `MPFROM2SHORT(cx, cy)`) to the window, or for a standard frame to its client with the client's sizes (the frame's own `WM_SIZE` only leads to `WM_FORMATFRAME`, which Warpine does in `layout_frame`), and nothing when the size does not change. A border drag and a menu-bar change use `post_size` too (they reported 0 as the old size).
- *Query*: `WinQueryWindowPos` fills the whole 36-byte `SWP`; `fl` is `SWP_MOVE | SWP_SIZE`, plus `SWP_MINIMIZE` / `SWP_MAXIMIZE` for the state and, for frames, `SWP_ACTIVATE` or `SWP_DEACTIVATE` (`query_swp_flags`; the reference: "set to the current state").

### Check boxes and radio buttons

Buttons with a check state live in `src/loader/pm_button.rs` (`has_check_state(style)`: `BS_CHECKBOX`, `BS_AUTOCHECKBOX`, `BS_RADIOBUTTON`, `BS_AUTORADIOBUTTON`, `BS_3STATE`, `BS_AUTO3STATE`); the state is `OS2Window::check_state` (0 unchecked, 1 checked, 2 indeterminate). `BM_SETCHECK` stores it and returns the old one; for the radio styles it sets `WS_TABSTOP` when checked and clears it otherwise (the `BM_SETCHECK` reference), so Tab enters a radio group on its checked button ("the system automatically maintains a tab stop on any selected item in a radio-button group"); it neither unchecks the others nor notifies. `BM_QUERYCHECK` reads the state, `BM_QUERYCHECKINDEX` gives the index of the checked button among the radio buttons of the group (-1 if none or not a radio button). A group is a run of sibling windows from one `WS_GROUP` sibling to the next (`WindowManager::button_group` / `radio_group`). A selection — mouse release over the button, Space, a mnemonic, or a dialog arrow key onto an auto-radio button — goes through `WindowManager::select_button` (called by `Loader::click_button` before it notifies): an auto check box toggles, an auto three-state button cycles 0 → 1 → 2 → 0, an auto-radio button is checked and the other auto-radio buttons of its group unchecked; plain styles are left to their owner; every check box and radio button then posts `WM_CONTROL(id, BN_CLICKED)` (mp2 = the button). In a dialog the arrow keys move within the group, wrapping, and check the auto-radio button they move to unless it has `BS_NOCURSORSELECT`. `paint_check_button` draws Warpine's own 13-px indicator — a box with a tick or a grey indeterminate square, a ring with a dot — the label with its mnemonic, and a dashed focus mark. Unverified (no source): `BM_SETCHECK` with mp1 > 2 counts as checked, the three-state order, `BN_CLICKED` for an arrow-key selection, a group with nothing checked having no tab stop.

Every button also takes `BM_CLICK`, `BM_SETHILITE` / `BM_QUERYHILITE` and `BM_SETDEFAULT` (`Loader::button_message`). `BM_CLICK(TRUE)` is "the default upclick action": the highlight is cleared and `click_button(hwnd, pointer = FALSE)` runs as for a real click (auto styles change, then `WM_COMMAND` with `CMDSRC_PUSHBUTTON` or `BN_CLICKED`); `BM_CLICK(FALSE)`, "the default downclick action", only highlights the button (unverified reading). A disabled button ignores it (unverified). `BM_SETHILITE` returns the old highlight state (`OS2Window::hilite`, or the button being held with the mouse: `WindowManager::button_highlighted`) and repaints: a highlighted push button is drawn pressed, a check box or radio button with a shaded indicator (Warpine's own look); `BM_QUERYHILITE` answers for `BS_PUSHBUTTON` only, FALSE for every other style, as documented. `BM_SETDEFAULT` sets or clears `BS_DEFAULT` on push and user buttons and returns TRUE (FALSE for other styles); `WinDefDlgProc`'s Enter follows the style bit. `BS_USERBUTTON` stores its highlight but gets no `BN_PAINT` yet. The dashed focus mark (`pm_button::focus_mark`, `FOCUS_MARK_STYLE`) is shared with list boxes, which outline their cursor item with it whenever they have the focus (black on an unselected row, white on a selected one).

### Scroll bars (WC_SCROLLBAR)

`src/loader/pm_scrollbar.rs` holds the control. `OS2Window::scrollbar` is a `ScrollBarState` (position, range, optional `SBM_SETTHUMBSIZE` visible/total, and the `Held` part while button 1 is down). `set_range` follows EDM2 `SBM_SETSCROLLBAR` (first bound below 0 becomes 0, a last bound below the first takes it, the position is clamped); `enabled()` is false without a range or when the visible part is the whole (`SBM_SETTHUMBSIZE`: "the scroll bar is disabled"). `Geometry::new(state, len, breadth, pos)` lays the bar out along its axis from the first end (top / left): an arrow button at each end as long as the bar is broad (at most half the length), the shaft between, a slider of `breadth` pels or proportional (`visible / total` of the shaft, at least `MIN_THUMB_LEN`) placed by `(pos − first) / (last − first)`; `part_at` hit-tests and `pos_for` turns a slider start back into a rounded position. `WinCreateWindow` reads `SBCDATA` control data through `scrollbar_init` (a `cTotal` of 0 sets no thumb size).

Messages arrive through `dispatch_builtin_control`: `SBM_*` (`scrollbar_message`), `WM_BUTTON1DOWN` (arrows and shaft post their `SB_*` command at once with slider 0; the slider starts a drag remembering the grab offset; `pressed_control` holds the bar so the renderer's existing routing sends it every move and the release), `WM_MOUSEMOVE` (the dragged slider follows the pointer — drawn at `Held::track`, the owner told `SB_SLIDERTRACK` on every change; a held arrow is drawn pressed only while the pointer is on it), `WM_BUTTON1UP` (`SB_SLIDERPOSITION` with the tracked position after a drag, else `SB_ENDSCROLL`; EDM2 `WM_VSCROLL`: no end-scroll after absolute positioning) and `WM_TIMER(TID_SCROLL)`: `WindowManager::post_drag_scroll` posts it every `DRAG_SCROLL_MS` while an arrow or shaft is held (not in scripted sessions), and `scrollbar_repeat` repeats the command while the pointer is still on that part — a shaft stops once the slider has reached the pointer, since the owner moves the slider. Notifications are posted (EDM2: the control "posts the message to the owner") to `notify_target` as `WM_VSCROLL` (`SBS_VERT`) / `WM_HSCROLL` with mp1 = id and mp2 = `scroll_mp2(slider, command)`. The slider does not move by itself: the application calls `SBM_SETPOS`. `paint_scrollbar` draws the shaft (`CONTROL_SCROLLBAR_COLOR`), two raised arrow buttons with triangles (grey when disabled) and the slider. Frame scroll bars (`FCF_VERTSCROLL` / `FCF_HORZSCROLL`) are not created yet: a client's GPI drawing starts at its frame surface's origin whatever the client's position, and `control_at` only searches the client's children.

### Rectangles and WinDrawText

`pm_rect.rs` holds the eleven rectangle calls (`WinSetRect` … `WinEqualRect`) as pure `Rectl` operations plus guest `RECTL` read / write (`read_rectl` / `write_rectl`; a NULL pointer fails with FALSE). PM rectangles are exclusive at the right and top: empty means `xLeft >= xRight || yBottom >= yTop` (the WinIsRectEmpty page), two empty rectangles are equal, an empty `WinUnionRect` source yields the other, and `WinSubtractRect` returns the first source when the difference is not a rectangle. Warpine's own choices: WinPtInRect leaves the right and top edges out, and the "empty rectangle" the calls write is (0, 0, 0, 0).

`pm_drawtext.rs` implements `WinDrawText`. `layout_line` is pure: the line ends at the first CR or LF; with `DT_WORDBREAK` only whole words that fit are drawn (words split at spaces only, so a DBCS character is never cut), always at least one. Without `DT_WORDBREAK` the result is the whole string's length, as the reference says; with it, the count also takes in trailing blanks and the line end when the whole line fit, so a caller advancing by the count reaches the next line (unverified). The rectangle includes its boundary for placement and for the `DT_QUERYEXTENT` result, as the reference says of `prcl`. Text is drawn with `DrawOp::TextFont` in the PS's font and codepage; `DT_TEXTATTRS` takes the PS colours, `DT_ERASERECT` fills the rectangle with the background colour, `DT_MNEMONIC` strips the `~` and underlines the next character, `DT_HALFTONE` uses the mean of the two colours. The string is read up to `MAX_DRAW_TEXT_CHARS` (4096) bytes.

`GpiPolyLine` (`pm_gpi.rs`, `gpi_poly_line`) draws the current position and the points as one open `DrawOp::Shape`, so a line style runs on across corners and an XOR mix paints shared corners once; at most `MAX_POLYLINE_POINTS` (8192) points, a negative count fails with `PMERR_INV_LENGTH_OR_COUNT`.

### Update rectangles, WinScrollWindow, WinDrawBorder

`src/loader/pm_paint.rs`. `OS2Window::update` is the window's update rectangle (window coordinates, a `pm_rect::Rectl`; empty = valid). PM keeps an update region; Warpine keeps its bounding rectangle, which can only make a window paint more than it must. `WindowManager::invalidate(hwnd, rect, include_children)` adds a rectangle, recursing into visible children (offset into their coordinates) when `include_children` or the window is not `WS_CLIPCHILDREN`, and `post_paint` queues one `WM_PAINT` per window unless one is waiting (the queue's ranking in `pm_input.rs` still puts it after posted messages and input). `invalidate_window_only` is the system's own invalidation — a window created visible (`WinCreateStdWindow`'s client, `WinCreateWindow`, a dialog after `WM_INITDLG`), a size change (`post_size`), a control losing the focus on a click — and leaves the children out, as the full repaints did before. `validate` subtracts with `Rectl::subtract` (exact for a band off an edge, otherwise the rectangle stays); `begin_paint` (`WinBeginPaint`, and `WinDefWindowProc(WM_PAINT)`) takes the rectangle and empties it; whenever it becomes empty `withdraw_paint` removes the window's waiting `WM_PAINT`, so `WM_PAINT` comes only while there is something to paint. A built-in control painting itself (`dispatch_builtin_control(WM_PAINT)`) marks itself valid (`painted`). A frame stands for its client (`paint_target`, rectangles offset by the client's position). An application's own `WinPostMsg(WM_PAINT)` stays an ordinary message, but validating the window withdraws it too. Not done: clipping the paint presentation space to the update rectangle, `WinUpdateWindow` sending `WM_PAINT` synchronously, `CS_SYNCPAINT`.

`WinScrollWindow`: in window coordinates, `S` = scroll rectangle ∩ window, `C` = clip rectangle ∩ window (else `S`), destination `D` = `S` moved by `(dx, dy)` ∩ `C`; `Surface::scroll` (`gui/surface.rs`) fills `D` (in frame-surface coordinates, `get_abs_rect_in_frame`) with the pels `(dx, dy)` away, read before writing, under the window-manager → surface lock order, records `D` as changed (`take_changes`, for the layered compositor) and bumps the generation; the frame is marked dirty. The uncovered area is `(S ∩ C) − D` as disjoint rectangles (`rect_difference`), its bounds and complexity from `region_of`. Distances are clamped to ±2²⁰.

`WinDrawBorder`: `border_parts` splits the rectangle (exclusive right / top) into the four sides and the interior as inclusive corners; each part is one `DrawOp::Shape` rectangle with an `AreaFill` whose pattern is solid `clrFore` on `clrBack` (interior swapped), or the PS's area pattern with `DB_AREAATTRS`; the mix is `Overpaint` / `Xor` / `Invert` / the PS mix for `DB_PATCOPY` / `DB_PATINVERT` / `DB_DESTINVERT` / `DB_AREAMIXMODE`, applied to the pattern's foreground and background pels alike (the ROP takes the whole pattern). `DB_DLGBORDER` uses `DLG_BORDER_ACTIVE_COLOR` / `DLG_BORDER_INACTIVE_COLOR` (the compositor's title-bar colours).

Unverified (the references are silent): the pattern of a border without `DB_AREAATTRS` (solid `clrFore`), whether `DB_DLGBORDER` honours `cx` / `cy` and draws the interior only with `DB_INTERIOR`, that negative `cx` / `cy` count as 0, the colour of an inactive dialog border, `WinQueryUpdateRect` writing (0,0,0,0) when the window is valid, `WinBeginPaint` with nothing to paint reporting (0,0,0,0), `WinScrollWindow` clipping to the scroll rectangle without a clip rectangle and failing for `HWND_DESKTOP`, `WinDefWindowProc(WM_PAINT)` validating only the window itself. The sample `samples/pm_paint_test` checks the behaviour from the guest side (`gui_pm_paint_test_*`, golden `pm_paint_test.sig`, also with the layered compositor).

### PM text cursor (WinCreateCursor)

`src/loader/pm_cursor.rs`: `WindowManager::cursors` maps a message queue to its `PmCursor` (window, rectangle, `CURSOR_*` flags, optional clip rectangle, hide level). `WinCreateCursor` replaces the queue's cursor with a hidden one (hide level 1; a zero extent is the nominal border, 1 pel; a negative one runs left / down); `CURSOR_SETPOS` moves the queue's cursor if it belongs to `hwnd`. `WinShowCursor` counts the hide level (EDM2: showing a visible cursor fails), `WinDestroyCursor` removes the cursor if it is `hwnd`'s, and `WinQueryCursorInfo` writes the 40-byte `CURSORINFO` (clip = the window's rectangle when none was given). `destroy_window` and `destroy_mq` drop the cursors of what they destroy. Every change marks the window dirty.

The cursor is never drawn into a surface. `WindowManager::cursor_marks()` gives each shown cursor on a visible window as a `gui::cursor::CursorMark` — rectangle and clip in the frame surface's PM coordinates, `CursorKind::Solid` / `Halftone` / `Frame` — and `composite_and_present` passes it with the frame in `SceneFrame::cursor`, redrawing frames whose mark changed. `CursorMark::apply` inverts the covered pels (XOR `0x00FFFFFF`; every other pel for halftone, the outline for a frame, which keeps its edges where clipped). The software compositor applies it after `draw_placed_frame` within the damaged rectangle and `cursor_clip` (the client inside the frame border); the layered compositor applies it to the scratch frame on a full render, and on a partial upload copies any changed rectangle the cursor touches into scratch, applies it and uploads that, re-uploading the old and new cursor rectangles when the mark changes (`Layer::cursor`). So the application's painting never meets cursor pels. `CURSOR_FLASH` cursors blink with the caret timer: `flash_cursors` flips `WindowManager::cursor_blink_off` every `cursor_rate_ms` and marks the flashing cursors' windows dirty (not in scripted sessions, so captures are steady).

### Window destruction (WinDestroyWindow / WM_DESTROY)

`WinDestroyWindow` (`pm_destroy.rs`) follows the `WinDestroyWindow` reference — chosen over the PM Guide's Windows chapter, which describes the opposite order: the window is hidden, then `WM_DESTROY` is sent first to it and then to its children, each before its own children, while all of them still exist; only then are they removed (`WindowManager::destroy_window` is now just that removal, which also frees `WinGetPS` / `WinBeginPaint` presentation spaces, disassociates `GpiCreatePS` ones and closes window DCs). A `DestroyChain` walks lazily: `WindowManager::destroy_step` yields the next window after the previous handler returned, so a window a handler destroyed is skipped; for a frame or dialog, the windows owned by it or its descendants outside its subtree come next (the frame's `WM_DESTROY` processing "destroys any owned windows"; their place before the children is unverified). Windows with a guest procedure are called through `ApiResult::DestroyWindows` / `FrameKind::DestroyWindows` (`continue_destroy` in `vcpu.rs`, every call built at `saved_rsp - 20` so the stack does not drift); the others are passed on the host; a standard frame that was not subclassed gets Warpine's frame default instead of its client class procedure. `WindowManager::destroying` marks windows already sent `WM_DESTROY`: `WinDestroyWindow` on one returns TRUE and does nothing more, nested chains pass over them, so no window gets the message twice. A frame being destroyed hands the focus to its parent (none for the desktop) and the active window to the next frame in the Z-order (Alt+Esc order is not modelled). Destroying a window of another thread's queue fails ("must have been created by the thread that is issuing this call"); an unknown handle fails with `PMERR_INVALID_HWND`. `WinDlgBox` destroys its dialog the same way before returning (`finish_dlg_loop`: `end_modal_dialog` + `destroy_dialog`). Not yet: `WM_ACTIVATE` when the active window is destroyed.

### PM errors (WinGetLastError)

`src/loader/pm_error.rs`: `SharedState::pm_errors` (`PmErrorStore`) keeps the last `ERRORID` per thread (vCPU id) behind its own leaf lock, so an error can be recorded while the window manager is locked. A handler records one with `self.pm_error(tid, PMERR_…)`, or returns `self.pm_fail(tid, PMERR_…)` to fail with 0; `self.pm_check_hwnd(tid, hwnd, allow_null)` checks an HWND argument (`HWND_DESKTOP` / `HWND_OBJECT` are valid). `WinGetLastError` returns `SEVERITY_ERROR << 16 | code` and resets it ("retrieves the last error code and resets it to zero"; errors are per thread). Recorded so far: `PMERR_MSG_QUEUE_ALREADY_EXISTS` (second `WinCreateMsgQueue`), `PMERR_INVALID_HMQ` (`WinDestroyMsgQueue` of another thread's or an unknown queue), `PMERR_NO_MSG_QUEUE` (`WinGetMsg` / `WinPeekMsg` without a queue), `PMERR_INVALID_HWND` from the calls taking a window (`WinDestroyWindow`, `WinSetWindowPos` / `WinQueryWindowPos`, `WinSetWindowText` / `WinQueryWindowText`, `WinWindowFromID`, `WinPostMsg` / `WinSendMsg`, `WinShowWindow`, `WinQueryWindowRect`, `WinEnableWindow` / `WinIsWindowEnabled`, `WinInvalidateRect`, `WinValidateRect`, `WinQueryUpdateRect`, `WinScrollWindow` (`RGN_ERROR`), `WinUpdateWindow`, `WinGetPS`, `WinBeginPaint`, `WinOpenWindowDC`, `WinSetActiveWindow`, `WinSetFocus`, `WinStopTimer`, `WinSetCapture`, `WinSubclassWindow`, the dialog-item calls, `WinDismissDlg`, `WinProcessDlg`, `WinCreateWindow`'s parent and owner, the `WinGetMsg` / `WinPeekMsg` window filter, `WinSetAccelTable`) — each failing with FALSE / NULLHANDLE as its reference page says (several returned TRUE before); `PMERR_PARAMETER_OUT_OF_RANGE` for a bad `WinQueryWindow` code and for window words (`pm_check_window_word` / `window_word_in_range`: an index below `QWL_MIN` / `QWS_MIN`, or past the `cbWindowData` the program registered its class with, `WindowClass::cb_window_data`; frames and built-in windows are not bounded, their size being unknown); `PMERR_INVALID_FLAG` for `WinPeekMsg` flags other than `PM_REMOVE`; `PMERR_INVALID_HACCEL` for `WinSetAccelTable`; `PMERR_INVALID_HPTR` / `PMERR_RESOURCE_NOT_FOUND` from the pointer calls; `PMERR_PARAMETER_OUT_OF_RANGE` / `PMERR_INVALID_HWND` from `WinSetSysValue`. Unverified: the severity of each code (always `SEVERITY_ERROR`), that a successful call leaves the error alone, what `WinGetMsg` returns for a bad filter (FALSE), and the errors of `WinSetWindowUShort` and of `WinSendDlgItemMsg` with a missing item (inferred).

### Window tree and window pointers

`src/loader/pm_window.rs` has the calls that move windows in the tree and reach window pointers. `WinIsWindowVisible` walks from the window up to the desktop and answers TRUE only if every window on the way has `WS_VISIBLE` ("Window and all its parents have the WS_VISIBLE style bit set on"; an `HWND_OBJECT` ancestor counts as not visible). `WinQueryWindowTextLength` gives the bytes `WinQueryWindowText` would write, without the null. `WinSetParent` (`WindowManager::set_parent`) takes the window out of its old parent's children and puts it on top of the new parent's (`HWND_DESKTOP` and `HWND_OBJECT` keep no child list); a frame or dialog that becomes a main window enters the Z-order (raised if visible) and one that stops being one leaves it — a frame inside another window is no longer drawn, since only main frames are composited; a parent that is the window itself or one of its descendants is refused with `PMERR_INVALID_HWND` ("This cannot be a descendant of hwnd"). `fRedraw` is not needed: the compositor redraws the damage anyway. `WinSetOwner` sets `OS2Window::owner` (NULLHANDLE disowns). `WinQueryWindowPtr` / `WinSetWindowPtr` read and write the application's window words — the same storage as `WinQueryWindowULong` — within `cbWindowData - 4`, or the window procedure for `QWP_PFNWP` (setting it replaces the procedure as `WinSubclassWindow` does; unverified); other negative indices fail with `PMERR_PARAMETER_OUT_OF_RANGE`. `WinCreateStdWindow` gives the client its `flClientStyle`, so a client is visible only when created with `WS_VISIBLE` (the PM Guide's samples pass it there).

### Window coordinates and lookups

`src/loader/pm_query.rs`. `WindowManager::window_origin` gives a window's bottom-left corner in desktop coordinates, following how Warpine stores positions: a main window or a dialog (`is_dialog`; dialogs are always placed on the desktop, whatever their parent) has desktop `x` / `y`; a standard frame's client has `x` = `y` = 0 but lies inside the frame's left border (`origin_in_parent` adds `frame_border_width`), at the frame's bottom edge, as the compositor places its surface; every other window is relative to its parent. `WinMapWindowPoints` adds the difference of two origins to each `POINTL` (read and written as one bounds-checked block); `WinWindowFromPoint` converts the point to the desktop and walks down from `hwndParent` (`window_from_point`): the topmost visible child whose rectangle holds the point (children are bottom to top, so the last match wins; a frame's client counts as visible like the compositor draws it), once for `fChildren` FALSE, to the deepest for TRUE; the desktop's children for this walk are the Z-order (the windows shown), for `WinMultWindowFromIDs` all main windows (`child_windows`). `WinQueryDesktopWindow` answers `HWND_DESKTOP`, the parent Warpine reports for main windows. `WinIsChild` uses `is_same_or_descendant`, with the desktop matching every window whose chain ends there and object windows (`root_of` = `HWND_OBJECT`) never children. `WinQueryWindowProcess` finds the thread through the window's queue (`tid_to_hmq`, `threads::tid_of_vcpu`). `WinEnumDlgItem` (`enum_dlg_item`) works on the immediate children of the dialog and maps `hwnd` to the child holding it; tab and group walks share `pm_types::step_from` (the next eligible window after a position, wrapping) and `group_range` (from the nearest `WS_GROUP` item to the next) with `next_tab_stop` / `next_in_group`, Warpine's dialog Tab and arrow keys. `WinQueryClassName` names preregistered classes "#n" after their `WC_*` atom (`PUBLIC_CLASSES`, standard frames and `#Dialog` → "#1", `#Menu` → "#4", unknown atoms `WC_ATOM_…`), `WinQueryClassInfo` accepts an atom pointer (`0xFFFFnnnn`), "#n" or a registered name and reports preregistered classes with `CS_PUBLIC` (`CS_FRAME` for the frame), no window words and the `WinDefWindowProc` thunk (`resolve_import`) as procedure — there is no guest procedure for a host-side control. Unverified choices are listed in the module header.

### Input state and message time / position

`src/loader/pm_keystate.rs`. `WindowManager::key_state` (`InputState`) holds three things. The physical keyboard by scan code: the GUI thread calls `physical_key` for every SDL key press and release (`track_physical_input` in `sdl_renderer.rs`, before anything else handles the event, so keys for the VIO window count too); `WinGetPhysKeyState` reads 0x8000 down and 0x0001 "pressed since the last call", which the read clears. The synchronous state by virtual key: `pm_input::next_input` calls `read_input` when a thread removes an input event from the system input queue, so a `WM_CHAR` with `KC_VIRTUALKEY` (a repeat — `KC_PREVDOWN` while down — is no new press; `VK_BACKTAB` also sets `VK_TAB`) or a `WM_BUTTONn*` message updates what `WinGetKeyState` reports: the state "at the time that the last message obtained from the queue was posted". It is one table for the process (unverified). The pointer position (`pointer`, desktop coordinates, bottom-left origin), updated by the GUI thread on every mouse event and by `WinSetPointerPos`, which also sends `GUIMessage::SetPointerPos` so the renderer warps the host pointer (not in a scripted session). `sdl_keycode_to_vk` maps Shift, Ctrl, Alt / AltGr, Caps Lock, Pause and Print Screen to their `VK_*`, so these keys reach programs as virtual keys.

The system input queue stores each event with its arrival time (`stubs::ms_count`, the `QSV_MS_COUNT` clock) and the pointer position (`QueuedInput`); `WindowManager::next_queued` stamps any other message when it is read (`note_read`) and records the time and position in the queue (`PM_MsgQueue::last_msg_time` / `last_msg_pos`) for `WinQueryMsgTime` / `WinQueryMsgPos`. `WinGetMsg` / `WinPeekMsg` write them into the `QMSG` — `ptl` is a `POINTL` of two LONGs at offsets 20 and 24, then `reserved`. Pointer messages from the renderer carry coordinates of the window that gets them (`pointer_local`): a standard frame's client, inside the frame's border.

### System colours

`src/loader/pm_syscolor.rs`. `SysColorTable` holds the 41 `SYSCLR_*` values as atomics, shared (`Arc`) by `WindowManager::sys_colors` and every `PresentationSpace`, so it is read without a lock. The defaults are Warpine's drawing colours where one corresponds (title bars, desktop, control face, selection, menus, entry fields, scroll bars, borders) and the PM Reference's table otherwise. `WinSetSysColors` checks options (`LCOL_RESET`, `LCOL_PURECOLOR`), format (`LCOLF_INDRGB` pairs, an even count; `LCOLF_CONSECRGB` from `lStart`) and every index before it sets anything (`parse_sys_colors`), then `broadcast_sys_color_change` posts `WM_SYSCOLORCHANGE(flOptions, 0)` to each main window's message target (`frame_msg_target`) and `WM_PAINT` to every frame and dialog. Readers: the compositor builds `TitleColors::of(&wm.sys_colors)` on each composite and `ChromeCache::set_colors` re-renders a title bar whose colours changed; `PresentationSpace::resolve_color` and `Loader::map_color` (`WinFillRect`) resolve a `SYSCLR_*` colour value through the table. The desktop background, control faces and menus still use their constants.

### Mouse pointers

`src/loader/pm_pointer.rs` keeps the pointers in `PointerManager` (`SharedState::pointers`, locked before `window_mgr`). `WinQuerySysPointer(desktop, iptr, fCopy)` returns `SYS_HPOINTER_BASE + iptr` for the `SPTR_*` pointers of `<pmwin.h>` (1–14, 18–39) or, with `fCopy`, a new handle the caller destroys; a bad index fails with `PMERR_PARAMETER_OUT_OF_RANGE`. `WinSetPointer` / `WinQueryPointer` change and report the current pointer (initially the arrow; NULLHANDLE removes it; a bad handle fails with `PMERR_INVALID_HPTR` and keeps the current one), `WinShowPointer` keeps PM's display level (TRUE lowers it to no less than 0, FALSE raises it; visible at 0; `SV_POINTERLEVEL`). `WinLoadPointer` finds the `RT_POINTER` resource by module (`ResourceManager::find`) and `decode_pointer_resource` turns it into RGBA with a top-left hot spot: `PT` / `IC` (one 1-bpp bitmap of double height, AND mask over XOR mask) and `CP` / `CI` (the mask bitmap, then the colour bitmap), both header layouts, any depth and compression `pm_bitmap` decodes; a `BA` array picks the first 32 × 32 entry, colour before black-and-white. Every offset is checked, images are at most `MAX_POINTER_DIM` square, at most 64 array entries are read and their offsets must grow. "Invert screen" pels are drawn black (a colour cursor cannot invert). `WinDestroyPointer` frees pointers the thread created; destroying the current one shows the arrow. Whenever what is shown changes, `GUIMessage::SetPointer(Option<PointerShape>)` goes to the GUI thread (sent under the pointer lock, so updates arrive in order), where `gui/pointer.rs` `HostPointer` makes and keeps the SDL cursor: SDL system cursors for the arrow, I-beam, wait, move and sizing arrows and the illegal pointer, colour cursors for the message-box icons and loaded pointers; on the `dummy` driver nothing is shown and nothing fails. `WinDefWindowProc` sets the arrow on `WM_MOUSEMOVE` (PM Guide). The built-in controls do not set their own pointers yet. `samples/pm_pointer_test/make_ptr.py` writes the sample's `.ptr` files from the documented format (Toolkit "Bitmap File Format").

### Keyboard accelerators

`ACCELTABLE` resources (`u16 cAccel, u16 codepage`, then `ACCEL { u16 fs, key, cmd }`) are parsed by `pm_dialog::parse_accel_table`; `WinLoadAccelTable` registers one and `FCF_ACCELTABLE` gives a frame the table with its `idResources`. `WinSetAccelTable` with a window sets that window's table, with `NULLHANDLE` the queue's. `WinGetMsg` and the modal dialog loop translate a key press (`translate_queued_accelerator()`): the tables of the target window's frame (or dialog), of the queue and of the system (`SYSTEM_ACCELS`: Alt+F4 → `WM_SYSCOMMAND(SC_CLOSE)`) are searched in that order, and a match is delivered to the frame's client (or the dialog) as `WM_COMMAND` — `WM_SYSCOMMAND` for `AF_SYSCOMMAND`, `WM_HELP` for `AF_HELP` — with mp2 = `CMDSRC_ACCELERATOR`. `accel_matches()` requires Ctrl and Alt exactly as the entry says, Shift too for `AF_VIRTUALKEY` / `AF_SCANCODE` (for `AF_CHAR` the character already includes it), and compares characters without case when Ctrl or Alt is held (chords report the unshifted character). `WinTranslateAccel` applies the table it is given to a `QMSG` and readdresses it to the window it names.

**Getting messages.** `WinGetMsg` (waits) and `WinPeekMsg` (never waits; `PM_REMOVE` / `PM_NOREMOVE`) share `next_message()` in `pm_win.rs`: the first queued message that passes the window filter (that window or a descendant, `is_same_or_descendant()`) and the message range (`msg_in_range()`: both 0 = all, `first > last` = all but the gap between them — the exact bounds are unverified) is written to the `QMSG`, translated if it is an accelerator keystroke. `WM_QUIT` passes every filter, so a filtered loop still ends. `WinDefWindowProc` turns `WM_SYSCOMMAND(SC_CLOSE)` into `WM_CLOSE`.

---

### System menu, keyboard move / size and WinPopupMenu

There is no `FID_SYSMENU` window. `WinDefWindowProc` / `WinDefDlgProc` turn `WM_SYSCOMMAND(SC_SYSMENU)` into `GUIMessage::SystemMenu` (`loader/pm_sysmenu.rs`, `Loader::frame_syscommand`), and the renderer opens a `PopupMenu` built by `system_menu::frame_system_menu_items(frame_flags, state)` below the icon; a click on the icon opens it directly, and Alt+Space / Shift+Esc are in `SYSTEM_ACCELS`. `PopupOwner` decides where a choice goes: a frame's menu → `WM_SYSCOMMAND(SC_*, MPFROM2SHORT(CMDSRC_MENU, fPointer))` to the client; a `WinPopupMenu` menu → `WM_COMMAND` / `WM_SYSCOMMAND` / `WM_HELP` to the owner; the VIO menu → `vio_command`. Item order, accelerator texts and enabling follow the GG24-3732 screenshot also used for the VIO menu (unverified against a reference page).

`SC_MOVE` / `SC_SIZE` (Alt+F7 / Alt+F8) become `GUIMessage::TrackFrame` when `system_menu::can_track` allows (Move unless maximized; Size only with `FCF_SIZEBORDER` in the normal state). The renderer keeps a `FrameTrack` around `frame_track::KeyTrack`, a pure state machine: arrows move by `FRAME_TRACK_STEP` (8 pels) or, when sizing, first pick an edge or corner; Enter keeps, Esc restores; one `WM_SIZE` is posted at the end. The VIO window uses the same `KeyTrack` with a cell-sized step. No `WM_TRACKFRAME` / `WM_QUERYTRACKINFO` is sent and the frame moves rather than an outline (doc/TODOs.md).

`WinPopupMenu` maps `(x, y)` from the parent window to the desktop — the menu's bottom-left corner, or the middle of the item's row with `PU_POSITIONONITEM` — and sends `GUIMessage::PopupMenu`; submenu items are shown greyed.

## Text-Mode Console Subsystem

**Modules:** `src/loader/console.rs`, `src/loader/viocalls.rs`, `src/loader/kbdcalls.rs`, `src/gui/text_renderer.rs`

OS/2 text-mode applications use two subsystems: **VIOCALLS** (Video I/O) for screen output and **KBDCALLS** (Keyboard) for input.

### Rendering modes

Two rendering backends are available, selected at startup:

| Mode | Condition | Description |
|---|---|---|
| **PM window** | default | A frame on the PM desktop showing the 80×25 screen (`gui/vio_window.rs`), same font, palette and cursor as the text window |
| **SDL text window** | `WARPINE_VIO=fullscreen`, or not window-compatible | 640×400 SDL window; CP437 8×16 font; CGA 16-colour palette; blinking block/underline cursor |
| **Terminal (ANSI)** | `WARPINE_HEADLESS=1`, or stdout not a terminal | ANSI escape sequences on the host terminal; raw-mode termios keyboard |

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

**Shared pieces**: `render_cells(buf, blink_on, pixels, w, h)` draws a `VgaTextBuffer` into an ARGB buffer, and `kbd_key_down` / `kbd_text_input` turn SDL key and text events into `KbdKeyInfo` entries of `kbd_queue`; the text window and the VIO window on the PM desktop both use them.

### VIO programs in a PM window

On OS/2 the session type is chosen when a program starts, not by the program: a window-compatible VIO program runs either "OS/2 full screen" or in an "OS/2 window" on the PM desktop (`DosStartSession` `SSF_TYPE_FULLSCREEN` / `SSF_TYPE_WINDOWABLEVIO`; the default type is the program's own). The program only says what it allows, in the application-type bits of its LX module flags (`LxHeader::app_type`: 100h "Incompatible with PM windowing", 200h "Compatible with PM windowing", 300h "Uses PM windowing API"). Warpine runs a VIO program in a window unless `WARPINE_VIO=fullscreen` asks for the text window or the program is not window-compatible (`gui::vio_window::choose_display`; which type `SSF_TYPE_DEFAULT` gives a window-compatible program is not stated in the sources found, so the window default is Warpine's choice).

`run_vio_session` (`main.rs`) enables the VIO buffer and the SDL keyboard queue as for the text window, then starts the PM renderer and calls `SdlRenderer::attach_vio_window`, which creates the frame through `WindowManager::create_vio_frame` (`loader/vio_frame.rs`): class `#VioWindow`, `FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER`, no message queue, `cols × 8` by `rows × 16` pels inside its border and title bar (`vio_frame_size`), active and on top (`WindowManager::vio_frame`). On every pass of the GUI loop `VioWindow::update` snapshots the buffer and, when the cells, the size or the cursor's blink phase changed, draws the screen with `render_cells` and writes it into the frame's surface inside the left border (`Surface::put_span`); a mode change resizes the frame keeping its top edge (`fit_vio_frame`). The program writes the buffer without waking the loop, so the loop waits at most `VIO_POLL` (30 ms) while a VIO window exists. While the frame is the active window, key presses and text go to `kbd_queue` (`kbd_key_down` / `kbd_text_input`) instead of the PM input queue; the close button and a double click on the system-menu icon end the program (`VioWindow::close`), and the title bar moves it. **View, scroll bars and size** (`gui/vio_view.rs`, `loader/vio_frame.rs`). The frame (`FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON`) is a view on the screen, "a view on the larger logical video buffer (LVB)" (IBM `MouGetPtrPos`): `VioLayout` divides the client between the text view and scroll bars (`vio_scroll_bars`: a bar appears in a direction the view is short of the screen, and its room may let the other need one), `VioWindow` renders the whole screen, inverts marked cells and composes the visible part from `origin` plus the bars into the surface. `clamp_vio_size` keeps the frame between `VIO_MIN_*` and the whole screen (plus a bar's room where needed — unverified); a border drag goes through the pure `resized_rect` (which fixed the inverted N/S edges of every frame). The view follows the cursor when it leaves (`follow_cursor`, unverified). Minimize / maximize / restore run host-side through `WindowManager::vio_minmax` (`begin_minmax` / `finish_minmax` without a client); maximize goes to `vio_maximized_rect`, the whole screen up to the desktop (user reports: a maximized window may still need its scroll bar). A mode change refits a normal frame (`fit_vio_frame`, `vio_screen`), re-maximizes a maximized one and updates a minimized one's restore words.

**System menu and clipboard** (`gui/popup_menu.rs`). `PopupMenu` is a renderer-level pop-up (items with id, `~` mnemonic, accelerator text, enabled, checked, separators; Up / Down / Home / End / Enter / Esc / mnemonics; hover and release to choose) drawn like the pull-down as a `Scene::overlay` with its own damage in both compositors. For the VIO window it opens below the system-menu icon on a single click, Alt+Space or Shift+Esc (unverified keys) with `system_menu_items` in the order of IBM's screenshot of an OS/2 window (GG24-3732): Restore, Move, Size, Minimize, Maximize, Hide (greyed: no window list), Close, Help / Font Size / Scroll (greyed), Mark, Copy, Copy All, Paste. Move / Size start a keyboard move / size (arrows by a cell, Enter, Esc). In Mark mode the title reads "Marking: …", button 1 marks a rectangle of cells (`CellRect`), drawn inverted; Copy / Copy All put `rect_text` (trailing blanks trimmed, CRLF between lines) into `clipboard_text` and the host clipboard; Paste turns the clipboard text into key presses (`paste_keys`: line breaks become Enter, other control characters are dropped) for `kbd_queue` — "copied from the clipboard character-by-character into the input area" (GG24-3731). Alt+F4 / F5 / F7 / F8 / F9 / F10 run the menu's commands.

**Mouse.** The renderer feeds pointer motion and buttons over the view to MOUCALLS (`VioWindow::pointer_event`, cells through `VioLayout::cell_at` with the view origin), not while the window uses the mouse itself (an open menu, Mark mode, a scroll-bar or frame drag). See "MOUCALLS" below.

**Full screen and Alt+Home** (`gui/vio_switch.rs`). The full-screen display is a desktop-full-screen SDL window whose canvas keeps the text screen's logical size (integer scaling, black letterbox; `Event::convert_coords` turns host pointer positions into logical ones). Alt+Home (`request_switch`; documented by IBM for DOS sessions, unverified for OS/2 VIO) makes the current display's loop return; `run_vio_session` then parks it (`park`: the hidden window, audio output and GUI script go into `Parked`) and opens the other (`open`), reusing a parked window — renderers hold leaked texture creators, so windows are hidden and reused, never destroyed, and switching does not leak. The guest keeps running throughout; `attach_vio_window` reuses the existing VIO frame. Renderers must ignore `Event::Window` for other windows (a hidden one still sends them).

**MOUCALLS** (`loader/moucalls.rs`). `MouManager` (`SharedState::mou_mgr`, `mou_cond`) holds one mouse handle, a queue of 64 `MOUEVENTINFO`s (oldest dropped when full), the event mask, scale factors, device status, pointer position and shape and the collision area. The GUI thread reports motion and buttons in text cells (`mouse_moved` / `mouse_button`; a move with a button change becomes a motion event and then a button event); `MouReadEventQue` with `MOU_WAIT` waits on `mou_cond` and ends with `ERROR_MOUSE_NO_DEVICE` when the program exits. Calls are 32-bit `_Pascal` like KBDCALLS (callee pops). Unverified choices are listed in the TODO list.

**Scripted sessions**: `SdlTextRenderer` loads `WARPINE_GUI_SCRIPT` like the PM renderer (`GuiScript::from_env`). Each pass turns the due `key` / `text` steps into SDL key and text events (`sdl_renderer::script_input_to_event`) that go through the normal keyboard path into `kbd_queue`, drops host keyboard events meanwhile (`is_host_input`), and after them carries out `dump PATH`: the text buffer is rendered afresh with the cursor shown and written as a PPM (`gui_script::write_ppm`). Mouse steps have no effect in the text window. The window opens only when stdout is a terminal, so a headless capture runs under a pseudo-terminal: `SDL_VIDEODRIVER=dummy WARPINE_GUI_SCRIPT=s.script script -qec "target/debug/warpine samples/4os2/4os2.exe" /dev/null` (the integration test `text_mode_window_replays_a_gui_script_and_dumps_the_screen` and the README screenshot do this).

### TTY output and ANSI escape sequences

`VioManager::write_tty` is OS/2's `VioWrtTTY`: CR, LF, BS, BEL and TAB (to the next multiple of 8), wrapping and scrolling of the buffer, text in "the current attribute defined by ANSI or the default value of 7". While `ansi_mode` is on (the default, as on OS/2; `VioSetAnsi`), bytes go through `ansi::AnsiParser`, whose state lives in `VioManager` so a sequence split across writes works; parameters and sequence length are bounded. Supported: cursor position and movement (clamped), save / restore, `ESC[2J`, `ESC[K`, SGR (bold, blink, reverse, concealed, colours 30–37 / 40–47 in CGA order, kept in `Rendition`) and wrap mode `ESC[=7h` / `ESC[=7l`; other sequences are swallowed (doc/TODOs.md, "ANSI follow-ups"). `DosWrite` to the console (`Loader::write_handle`, also the 16-bit `DosWrite`) uses `write_tty` when the SDL window or a host terminal shows the console — the terminal echo goes through `TtyOut` — and otherwise passes the bytes to the host pipe unchanged while `track_tty` keeps the VIO buffer and cursor in step.

### Keyboard input

**End of piped input.** In the terminal path a waiting `KbdCharIn` / `KbdStringIn` that finds the end of a piped (non-terminal) stdin (`HostInput::EndOfInput`) ends the process with `EXIT_CODE_KBD_END_OF_INPUT` (0) through `request_process_exit`; `KbdStringIn` first returns a partly read line, and `IO_NOWAIT` still returns "no character" at once. The KBD reference lists no end-of-input error, so this is Warpine's choice.

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

32-bit programs import the NLS calls from the **NLS** module (`DosQueryCp` and `DosSetProcessCp` are DOSCALLS 291 / 289). They use the **_System** calling convention, as DOSCALLS does.

### NLS ordinals

NLS imports are dispatched at `NLS_BASE` (7168) + ordinal, from the static registry. The ordinals are those of the toolkit's `<bseord.h>` (`ORD_DOS32QUERYCTRYINFO` … `ORD_DOS32QUERYCOLLATE`) and Open Watcom's `os2386.lib` (see `doc/os2_ordinals.md`, NLS):

| Ordinal | Function | Notes |
|---|---|---|
| 1–4 | `DOS16CASEMAP` / `DOS16GETCOLLATE` / `DOS16GETCTRYINFO` / `DOS16GETDBCSEV` | 16-bit; not implemented |
| 5 | `DosQueryCtryInfo(cb, pcc, pci, pcbActual)` | COUNTRYINFO from the host locale, at most `min(cb, 44)` bytes |
| 6 | `DosQueryDBCSEnv(cb, pcc, pBuf)` | Lead-byte ranges of the codepage, `(0,0)`-terminated |
| 7 | `DosMapCase(cb, pcc, pch)` | Full case mapping: SBCS (CP437/850/852/866/1250–1258) and DBCS (CP932/936/949/950) |
| 8 | `DosQueryCollate(cb, pcc, pch, pcch)` | 256 sort weights (`codepage::collate_table`) |

- `Loader::nls_codepage(pcc)` gives the codepage for 6–8: `COUNTRYCODE.codepage` when it is not 0, else the process codepage. The country is not used.
- A short buffer gets as much of the table as fits, in order ("as much information as possible is provided in the available space", CP Reference), and `ERROR_NLS_TABLE_TRUNCATED`. The reference does not say when that code is returned; Warpine's choice.
- IBM's collating tables (COUNTRY.SYS) are not published. Warpine weighs each byte as its uppercase form (`cp_map_case_upper`): letters sort without regard to case, everything else in byte order, DBCS lead bytes as themselves.

Until this table was checked against the toolkit, Warpine had the NLS ordinals wrong: 5 was a mix of `DosQueryCp` and `DosQueryCtryInfo` chosen by `cb`, 6 was `DosQueryCtryInfo` and 8 the DBCS query. `DOSCALLS.373` was served as `DosQueryDBCSEnv` and `DOSCALLS.397` as `DosQueryCtryInfo`, but 373 is `DosQueryDOSProperty` and 397 is no DOSCALLS export. `gen_api validate-doc` now checks NLS (and MOUCALLS) against `doc/os2_ordinals.md`, so this cannot drift again unnoticed.

### Watcom C runtime start-up

During start-up the Open Watcom C runtime calls **NLS 6, `DosQueryDBCSEnv`, with `cb=12`** (EDM2: "A length value of 12 bytes is sufficient") to learn the lead bytes for its multibyte-character functions. A program's own `DosQueryCtryInfo` goes to **NLS 5** with `cb=44`, the size of Watcom's COUNTRYINFO.

With the old table the start-up call got a COUNTRYINFO instead, whose first bytes (country, then codepage) were read as lead-byte ranges. For every non-US country that hung the runtime before `main` (country 1 happened to be harmless), which is why `locale.rs` used to report country 1 / CP437 whatever the host locale. A build with the old table hangs in `nls_test` for countries 7, 33, 49, 81, 82, 86 and 88; the current one runs `hello`, `alloc_test`, `nls_test`, `dbcs_test` and 4OS2 with all of them, in their own codepages and in 437.

### Starting country and codepage

`locale::resolve_country` turns the host locale into the OS/2 country code and the starting process codepage, as an OS/2 installation in that language would have them (`ja_JP` → 81 / CP932, `de_DE` → 49 / CP850, `C` → 1 / CP437). `WARPINE_COUNTRY` and `WARPINE_CODEPAGE` override them; an unsupported codepage is ignored with a warning. `Loader::new` starts `SharedState::active_codepage`, the VIO screen's codepage and the PM CJK face preference from it, as `DosSetProcessCp` does later; `Loader::new_mock` pins 1 / 437 so unit tests do not depend on the host. The integration tests and the screenshot helper pin `WARPINE_COUNTRY=1 WARPINE_CODEPAGE=437` (`PINNED_LOCALE`). The country codes (dialling codes, 88 for Taiwan, 42 for the Czech Republic and Slovakia) are not checked against an IBM source.

### PM codepages

PM text follows the codepage of the **message queue**, not the process codepage, as in PM: `WinSetCp` (`PM_MsgQueue::codepage`, initialised from the process codepage by `WinCreateMsgQueue`) decides how the text of that queue's windows is converted. `Loader::window_codepage(hwnd)` gives the codepage of a window's queue (window text in `WinSetWindowText` / `WinQueryWindowText` / `WinSetDlgItemText` / `WinQueryDlgItemText`, list-box items, characters typed into a control — both when the renderer encodes `TextInput` and when the control decodes `WM_CHAR`), `Loader::thread_codepage(vcpu_id)` that of the calling thread's queue where no window exists yet (titles in `WinCreateStdWindow` / `WinCreateWindow`, `WinMessageBox`, `CF_TEXT` clipboard data, dialog templates with codepage 0); both fall back to the process codepage. A presentation space has its own codepage (`PresentationSpace::codepage`, `GpiSetCp` / `GpiQueryCp`): its window queue's when created, the creating thread's queue's for a memory PS; GPI text, face names and the CJK face choice use it (`Loader::ps_codepage`). Built-in control text is drawn in the CJK face of its window's queue codepage (`Painter::new` sets `Painter::cjk`, carried by `DrawOp::Text { cjk }`), and title-bar and menu text in that of the frame's queue (`ChromeCache::set_cjk`, which re-renders the chrome when it changes); a window whose queue is set to CP949 shows Hangul and hanja in the Korean face whatever the process codepage. Caret positions and word wrap in entry fields and MLEs are still measured with the process codepage's face (`pm_controls::system_width`, a plain `fn` pointer); CJK ideographs are one em wide in every regional face, so this matters only for characters whose width differs between faces. Menu templates with codepage 0 and VIO text still use the process codepage.

`WinQueryCpList` reports the queue codepages (`stubs::pm_codepages()`): the process codepages plus **1004**, IBM's "Windows Extended" codepage, which only a queue can use — `DosSetProcessCp(1004)` fails. CP1004 is Windows-1252 with spacing diacritics in seven C0 positions (0x04 ˉ, 0x05 ˘, 0x06 ˙, 0x08 ˚, 0x0A ˝, 0x0B ˛, 0x0C ˇ; `codepage::CP1004`), from the code-page 1004 table in Wikipedia's "Windows-1252" article; whether 1004 lacks 1252's ƒ (0x83), as another listing claims, is unverified.

### DosQueryCtryInfo Bounded Writes

`dos_query_ctry_info()` writes only `min(cb, 44)` bytes to respect the caller's buffer size; writing more would corrupt a caller's stack frame. It returns the host locale's formats with country 1 and codepage 437 (`locale.rs` `map_country`).

---

## Structured Exception Handling (SEH)

OS/2 uses a per-thread exception handler chain (analogous to Win32 SEH) rooted at `TIB+0x00` (`tib_pexchain`). Each handler is an EXCEPTIONREGISTRATIONRECORD linked-list node; the chain terminates at `XCPT_CHAIN_END` (0xFFFFFFFF).

### Handler Chain Maintenance

`DosSetExceptionHandler` (ordinal 354) and `DosUnsetExceptionHandler` (355) are implemented in `stubs.rs`:

- `dos_set_exception_handler(vcpu_id, preg_rec)` — writes `preg_rec->prev = TIB[TIB_EXCHAIN_OFFSET]`, then `TIB[TIB_EXCHAIN_OFFSET] = preg_rec`
- `dos_unset_exception_handler(vcpu_id, preg_rec)` — writes `TIB[TIB_EXCHAIN_OFFSET] = preg_rec->prev`

The TIB is the calling thread's own: `Loader::exception_chain_addr(vcpu_id)` looks it up in the thread table (`ThreadTable::tib_of`: `TIB_BASE` for thread 1, the TIB `DosCreateThread` allocated for the others — the one the thread's FS selector points at). `DosRaiseException`, `DosUnwindException` and hardware exceptions use the same lookup, so an exception reaches only the handlers of the thread it happened on. A record outside guest memory fails with `ERROR_INVALID_PARAMETER` instead of panicking. `TIB_EXCHAIN_OFFSET` is 0x00 (the first dword of the TIB). `setup_guest()` in `vcpu.rs` initialises thread 1's to `XCPT_CHAIN_END` before the first vCPU run, `DosCreateThread` each new thread's. When a handler continues after `DosRaiseException` (`XCPT_CONTINUE_EXECUTION`), the context restored is the caller's at the call — its EBX / ESI / EDI / EBP and flags — with EAX = `NO_ERROR`. Sample `xcpt_thread_test`.

### Signal exceptions (`signals.rs`, `host_signal.rs`)

`XCPT_SIGNAL` goes only to thread 1, through its exception chain (Control Program Guide, "Signal Exceptions"). `Threads::signals` (`SignalState`: focus count, pending and held signal bits) + `signal_pending` fast flag.

- `DosSetSignalExceptionFocus(flag, pulTimes)` (378) counts `SIG_SETFOCUS` up (`ERROR_NESTING_TOO_DEEP` above `MAX_SIGNAL_FOCUS` 65535) and `SIG_UNSETFOCUS` down (`ERROR_ALREADY_RESET` at 0) and writes the count; a bad `pulTimes` changes nothing.
- `DosSendSignalException(pid, signal)` (379): only `XCPT_SIGNAL_INTR` / `XCPT_SIGNAL_BREAK` (else `ERROR_INVALID_SIGNAL_NUMBER`); the own `PROCESS_ID` posts it here (`post_signal`); a running child (`ProcessManager::children`, `try_wait`) gets a host real-time signal (`host_signal::send_to_process`: INTR = `SIGRTMIN + 3`, BREAK = `SIGRTMIN + 4`); anything else is `ERROR_INVALID_PROCID`. Every LX process listens for these (`listen_for_signals` in `setup_guest`: the handler stores a bit in an atomic and writes a byte to a socket pair, both async-signal-safe; an `os2-signals` thread reads it and calls `post_signal`).
- `post_signal` keeps one pending signal per type and kicks thread 1. At the top of its vCPU loop thread 1 asks `signal_action`: it takes the next signal that is not held (and holds it), unless thread 1 runs 16-bit code, has no room for the 20-byte handler frame below ESP, or runs the exit list. INTR / BREAK with focus 0 end the process (`SignalAction::EndProcess`: exit list with `TC_KILLPROCESS`, exit code `SIGNAL_EXIT_CODE` 1); otherwise `chain_dispatch` (`seh.rs`) calls the first handler with an `EXCEPTIONREPORTRECORD` (`XCPT_SIGNAL`, `ExceptionInfo[0]` = signal) and a `CONTEXTRECORD` of the interrupted state, so `XCPT_CONTINUE_EXECUTION` resumes exactly there. No handler, or every handler continuing the search, ends the process the same way (`termination_code`).
- A held signal is released by `DosAcknowledgeSignalException` (418; `ERROR_INVALID_SIGNAL_NUMBER` for other than 1 / 3 / 4) or by a handler returning `XCPT_CONTINUE_EXECUTION` for it (`signal_continued`); then a pending one of that type is delivered. With thread 1 ended, a signal whose default action applies ends the process at once.
- Not done: Ctrl+C / Ctrl+Break from the keyboard (the host `SIGINT` keeps its default), `XCPT_SIGNAL_KILLPROC` from `DosKillProcess` (which kills the child host process), interrupting a blocking call of thread 1, must-complete sections, `ERROR_INVALID_PROCID` for PM programs. Sample `thread_control_test`.

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
- **NLS DLL calling convention** — the NLS functions (`DosQueryCtryInfo`, `DosQueryDBCSEnv`, `DosMapCase`, `DosQueryCollate`, NLS 5–8) use the `_System` convention (same as DOSCALLS, NOT Pascal like VIOCALLS). The Watcom CRT calls NLS 6 (`DosQueryDBCSEnv`) with cb=12 during start-up; see [NLS](#nls-national-language-support).
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

### Redirected and duplicated handles

`Loader::handle_target(h)` resolves any handle to `Pipe` (`HandleManager`, also a pipe duplicated into 0–2), `File` (`DriveManager`, also a standard handle redirected to a file) or `Console(stream)`; a number is in at most one table. `DosRead`, `DosWrite`, `DosClose`, `DosDupHandle`, `DosSetFilePtr`, `DosSetFileSize`, `DosQueryFileInfo`, `DosResetBuffer` and `DosQueryHType` route through it. `DosDupHandle` shares the backend file between handles (`file_refs`; one file pointer, closed with the last handle) and records duplicates of the console (`console_handles`); `closed_std` marks standard handles closed by `DosClose` until something is duplicated into them. `DosExecPgm` hands redirected standard handles to the child as host descriptors (`VfsBackend::clone_host_file`), so `prog > file` works from 4OS2.

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
    ne.rs              NE file orchestration (16-bit OS/2 apps; 17 unit tests with ne/header.rs)
  gui/
    mod.rs             GUI module re-exports; pub use for all renderer types
    message.rs         GUIMessage channel (clipboard, mouse capture) + GuiWake
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
                       CGA_PALETTE, render_cells(), kbd_key_down() (44 unit tests)
    vio_window.rs      VIO program in a PM window: choose_display(), VioWindow
    system_menu.rs     System-menu items of PM frames and the VIO window, can_track, menu_popup_items
    frame_track.rs     KeyTrack: keyboard move / size state machine (Alt+F7 / Alt+F8)
    cursor.rs          CursorMark: the PM text cursor, inverted over a frame's pels by the compositors
  loader/
    mod.rs             Loader struct, SharedState (kbd_queue/kbd_cond/use_sdl_text), KVM/mock init
    vcpu.rs            vCPU thread: VMEXIT loop, GDB integration, crash dump hooks
    threads.rs         Thread table: running / ended threads, kill flags, per-thread memory, idle vCPUs
    exitlist.rs        DosExitList: the exit-list routines run when the process ends
    critsec.rs         DosEnterCritSec / DosExitCritSec
    dostimer.rs        DosAsyncTimer / DosStartTimer / DosStopTimer (a host thread per timer)
    modpath.rs         DosSearchPath, DosQueryModuleName, OS/2 paths of modules
    suballoc.rs        DosSub* memory suballocation (host-side bookkeeping)
    vm_backend.rs      VmBackend/VcpuBackend traits (KVM + mock implementations)
    kvm_backend.rs     KVM-based VmBackend implementation
    guest_mem.rs       Guest memory read/write/translate helpers
    lx_loader.rs       LX executable loading into guest memory and fixup application
    ne_exec.rs         NE executable loader: load_ne(), setup_guest_ne(), handle_ne_api_call(), ne_api_arg_bytes()
    descriptors.rs     GDT/IDT setup, resolve_import() (built-ins + DllManager)
    constants.rs       Named constants (addresses, message IDs, ordinal bases)
    api_registry.rs    Static sorted API thunk table (216 entries); ApiEntry
    api_dispatch.rs    Registry lookup + per-subsystem dispatcher routing
    api_trace.rs       Structured tracing helpers (ordinal_to_name auto-generated from os2api.def,
                       module_for_ordinal, arg_names_for_ordinal, format_call)
    api_ring.rs        256-entry bounded API call ring buffer for crash post-mortem
    crash_dump.rs      Structured crash reports (regs, stack, code bytes) on fatal VMEXITs
    gdb_stub.rs        GDB Remote Stub: RSP over TCP, software breakpoints, single-step
    mutex_ext.rs       MutexExt trait (poison-recovering lock)
    managers.rs        MemoryManager, HandleManager, HDirManager, ResourceManager
    ipc.rs             Semaphores (event, mutex, muxwait; names, usage and post counts) and queues
    pm_types.rs        PM data types (windows, classes, presentation spaces, WindowManager)
    locale.rs          Os2Locale: country/codepage information
    doscalls.rs        DOSCALLS API implementations (file, memory, process, NLS; others in their own modules)
    pm_win.rs          PMWIN dispatcher and window calls (127 ordinals, logic in the pm_* modules)
    pm_gpi.rs          PMGPI API implementations (67 ordinals, incl. DevOpenDC/DevCloseDC/DevQueryCaps)
    pm_shapes.rs       Arc geometry (ARCPARAMS, full/partial/point arcs, FIXED) and the PATSYM_* pattern table
    pm_bitmap.rs       Bitmap codec (BITMAPINFOHEADER/2, RT_BITMAP), memory DCs, bitmaps, GpiBitBlt family
    pm_edit.rs         Edit-buffer logic for text controls (insert/delete/caret/scroll)
    pm_controls.rs     Interactive WC_ENTRYFIELD / WC_MLE / WC_LISTBOX / WC_BUTTON: messages + painting
    pm_dialog.rs       DLGTEMPLATE / ACCELTABLE parsing, dialog keys (WinDefDlgProc), accelerator translation
    pm_sysmenu.rs      SC_SYSMENU / SC_MOVE / SC_SIZE for frames and dialogs, WinPopupMenu
    pm_rect.rs         Rectangle helpers (WinSetRect ... WinEqualRect): pure Rectl operations
    pm_drawtext.rs     WinDrawText: line layout, word break, DT_* placement and attributes
    pm_paint.rs        Update rectangles (WinInvalidateRect / WinValidateRect / WinQueryUpdateRect / WinBeginPaint), WinScrollWindow, WinDrawBorder
    pm_scrollbar.rs    WC_SCROLLBAR: SBM_* messages, geometry, WM_VSCROLL / WM_HSCROLL
    pm_cursor.rs       PM text cursor: WinCreateCursor / WinShowCursor / WinDestroyCursor / WinQueryCursorInfo
    pmshapi.rs         PMSHAPI Prf* calls, ProfileTable
    profile.rs         Profile model and Warpine's text profile format, VFS storage
    helpmgr.rs         HELPMGR help instances (no viewer)
    pm_paint.rs        Update rectangles, WinScrollWindow, WinDrawBorder
    pm_query.rs        WinMapWindowPoints, WinWindowFromPoint, WinEnumDlgItem, class names / info, ...
    pm_keystate.rs     WinGetKeyState / WinGetPhysKeyState, message time and position, pointer position
    pm_syscolor.rs     System colours: WinQuerySysColor / WinSetSysColors, WM_SYSCOLORCHANGE
    mmpm.rs            MMPM/2: MmpmManager, beep_tone, mciSendCommand/mciSendString, parse_wav
    console.rs         VioManager: screen buffer, cursor state/shape, sdl_mode,
                       write_tty (ANSI via ansi.rs), stdin_cooked_chars, CP437→UTF-8 conversion
    ansi.rs            ANSI escape-sequence parser for TTY output (state kept across writes)
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

   - **DOSCALLS / QUECALLS**: `api_registry.rs` contains a **static sorted table** of `ApiEntry` records (216 entries). Add a new entry to the table in ascending ordinal order. The table is binary-searched at dispatch time; no match arm needed in `api_dispatch.rs` for most cases. For subsystem handlers that need special argument extraction (VIO, KBD, PMWIN, PMGPI, etc.), add a match arm in the appropriate `handle_*_calls()` function in `api_dispatch.rs`.

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
cargo clippy --all-targets -- -D warnings   # lint, test code included
```

The pre-commit hook (`githooks/pre-commit`; `git config core.hooksPath githooks`) runs both and aborts the commit on a failing test or any lint.

Tests never use the user's drive C (`$XDG_DATA_HOME/warpine/drive_c`): `Loader::new_mock` mounts C: at `target/test-drive-c` (`DriveManager::with_drive_c`; `default_drive_c_path()` only computes the user's path), and every Warpine process an integration test starts gets `XDG_DATA_HOME` in a temporary directory (`run_sample_env`, `run_gui_script_env` and the 4OS2 / ANSI session helpers), unless the test names its own.

The unit tests (1322: 1286 warpine + 19 `gen_api` + 12 `lx_link` + 5 `gen_arch`) include, among others:

| Module | What's tested |
|---|---|
| `src/lx/header.rs` | LX header parsing, object table entry parsing, resource entry parsing |
| `src/lx.rs` | MZ validation, LX signature detection, rejection of malformed binaries (excessive object/page counts, invalid EIP object, invalid page offset shift), parsing of a real `hello.exe` |
| `src/ne.rs`, `src/ne/header.rs` | NE header parsing, segment table, relocation records, export/import tables, resource table (17 tests) |
| `src/loader/managers.rs` | MemoryManager allocation, 4KB alignment, free-list reuse, top-of-heap coalescing, overflow/limit rejection, ResourceManager find operations, SharedMemManager name lookup |
| `src/loader/console.rs` | VioManager screen buffer operations (scroll up/down, read cell str, defaults), key mapping (enter, printable, backspace → OS/2 charcode/scancode) |
| `src/loader/stubs.rs` | DosEditName wildcard pattern replacement, DosQuerySysInfo QSV_* constant validation |
| `src/loader/process.rs` | ProcessManager child tracking, wait-any semantics |
| `src/loader/api_trace.rs` | ordinal_to_name (DOSCALLS, QUECALLS, KBDCALLS, VIOCALLS, NLS, MSG, MDM, UCONV — auto-generated from `os2api.def`), module_for_ordinal (all ranges), arg_names_for_ordinal |
| `src/loader/mmpm.rs` | MmpmManager waveaudio device open/close/play lifecycle, mciSendCommand dispatch, mciSendString parsing, mciFreeBlock, mciGetLastError |
| `src/loader/vfs.rs` | DriveManager path resolution, device name detection, drive letter parsing |
| `src/loader/vfs_hostdir.rs` | HostDirBackend case-insensitive lookup, sandbox enforcement, EA operations |
| `src/gui/sdl_renderer.rs` | Scancode / VK / WM_CHAR mapping, mouse modifiers (`fsInp`) and double clicks, window chrome and menu drawing, chrome hit-testing, the event-loop wait, scripted input → SDL events |
| `src/gui/surface.rs`, `damage.rs`, `sdl_renderer/compositor.rs` | Drawing ops and clipping, mix modes, read-back, shapes and patterns, damage rectangles, both compositors |
| `src/gui/gui_script.rs` | Script parsing and expansion (clicks, modifier clicks, double clicks, drags, keys, text) |
| `src/loader/pm_edit.rs` | Edit buffer: caret and selection, CRLF units, page keys, word-wrapped rows, scrolling arithmetic, WM_CHAR decoding |
| `src/loader/pm_input.rs` | System input queue: keys routed when read, queue order (posted, input, `WM_PAINT`, `WM_TIMER`), other queues wait, pinning before a click moves the focus, character encoding per queue codepage, bound |
| `src/loader/pm_minmax.rs` | Minimize / maximize / restore rectangles, icon slots, restore words, owned frames hidden while minimized or hidden, `WM_MINMAXFRAME` `SWP` layout, `SC_HIDE`, activation hand-over, `WM_SIZE` on resize, `WinQueryWindowPos` flags |
| `src/loader/pm_destroy.rs` | `WM_DESTROY` order (window, then children, while they exist), owned windows, owner cycles, nested and repeated destroys, focus and presentation spaces, other threads' windows |
| `src/loader/pm_button.rs` | Check state, `BM_SETCHECK` / `BM_QUERYCHECK` / `BM_QUERYCHECKINDEX`, groups, auto styles, clicks and Space, `BM_CLICK` / `BM_SETHILITE` / `BM_QUERYHILITE` / `BM_SETDEFAULT`, painting |
| `src/loader/pm_scrollbar.rs` | Scroll-bar range / position clamping, enabled state, geometry (arrows, proportional slider, hit-testing, position rounding), `SBM_*`, line / page / slider notifications and `SB_ENDSCROLL`, repeat while held, painting |
| `src/loader/pm_cursor.rs`, `src/gui/cursor.rs` | Cursor per queue, hidden on creation, show level, `CURSOR_SETPOS`, `CURSORINFO`, frame-surface marks and clipping, flash phase, errors and cleanup; inverted solid / halftone / frame pels |
| `src/loader/pm_window.rs` | `WinIsWindowVisible` through the parents, `WinQueryWindowTextLength`, `WinSetParent` (tree, Z-order, descendants refused), `WinSetOwner`, window pointers and `QWP_PFNWP` |
| `src/loader/pm_query.rs` | Window origins (client inside the border, dialogs on the desktop), `WinMapWindowPoints`, `WinWindowFromPoint` depth and visibility, `WinIsChild` and object windows, `WinQueryWindowProcess`, `WinMultWindowFromIDs`, `WinEnumDlgItem` tab / group walks, class names and `CLASSINFO`, errors |
| `src/loader/pm_keystate.rs` | Synchronous key state as input is read, buttons, `VK_BACKTAB`, physical state and its press bit, message time / position stamps, `QMSG.ptl`, pointer position set / query / clamping |
| `src/loader/pm_syscolor.rs` | `SYSCLR_*` slots and defaults, `LCOLF_INDRGB` / `LCOLF_CONSECRGB`, `LCOL_RESET`, validation before change, `WM_SYSCOLORCHANGE` and repaint, `SYSCLR_*` as GPI / `WinFillRect` colours; the title-bar colours in `sdl_renderer.rs` |
| `src/loader/pm_pointer.rs` | System pointers, set / query / show level, `RT_POINTER` decoding (formats, arrays, hot spot, truncated and hostile data), destroy rules |
| `src/loader/pm_error.rs` | `ERRORID` layout, per-thread last error reset on read, `PMERR_*` from queue, window, window-word, message and dialog-item calls, window-word bounds, `SV_CURSORRATE` / `WinSetSysValue` |
| `src/loader/pm_list.rs` | List-box selection modes (single, `LS_MULTIPLESEL`, `LS_EXTENDEDSEL`), cursor and anchor, index shifts |
| `src/loader/pm_controls.rs` | Entry field / MLE / list box input and painting, focus notifications, selection and clipboard, read-only fields, word wrap, multi-select, `LN_ENTER`, caret blink, measuring in the queue's CJK face, drag-scroll timer, input-method caret area, list focus mark |
| `src/loader/pm_dialog.rs`, `pm_msgbox.rs` | DLGTEMPLATE / ACCELTABLE parsing, dialog keys, mnemonics, arrow keys over radio buttons, accelerators (system keys); message-box layout, icons, Help and Close |
| `src/loader/pm_win.rs`, `pm_types.rs` | Window tree, Z-order, focus, frame layout and moves, menus and `WM_UPDATEFRAME`, message queues, `WinPeekMsg` filters, codepages, system values |
| `src/loader/pm_gpi.rs`, `pm_bitmap.rs`, `pm_shapes.rs` | GPI calls, bitmaps and the codec, `GpiBitBlt`, colour tables, arcs and patterns |
| `src/loader/constants.rs` | Every constant against the OS/2 toolkit headers (`constants_match_toolkit_headers`) |
| `src/gui/vio_view.rs`, `src/gui/popup_menu.rs` | View layout and scroll bars, cursor following, cells under the pointer, thumb geometry and hits, rectangle to text, paste to keys; pop-up menu keys, mnemonics, hover and choosing |
| `src/loader/moucalls.rs`, `src/gui/vio_switch.rs` | Mouse queue, mask filtering, mickeys, validation and return codes of each MOUCALLS API, waiting and wake-up; Alt+Home targets and switch requests |
| `src/gui/vio_window.rs`, `src/loader/vio_frame.rs` | Display choice (`WARPINE_VIO`, LX application type), the VIO frame's size and chrome, drawing the screen into its surface only on a change, following a mode change, closing ends the program |
| `src/gui/text_renderer.rs` | CP437 glyph bitmaps (box-drawing, block elements, ASCII delegation), CGA palette (opaque, correct entries), HeadlessTextRenderer frame counting and loop termination, VgaTextBuffer snapshot, KbdKeyInfo queue push/pop (44 tests) |

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

# Text-mode: 4OS2 command shell (interactive)
cd samples/4os2 && ./fetch_source.sh && make && cd ../..
cargo run -- samples/4os2/4os2.exe
# Should show banner, [c:\] prompt, accept commands (ver, set, exit)
```

CLI samples should print output and exit with code 0. PM samples should open a window and respond to close. 4OS2 should boot to an interactive prompt (`[c:\]`) with working commands: `dir`, `set`, `ver`, `md`, `rd`, `copy`, `move`, `del`, `attrib`, `tree`, `exit`. Use `RUST_LOG=debug` to inspect API call traces if a sample misbehaves.

### Automated integration tests

`tests/integration.rs` contains 73 end-to-end tests that run real OS/2 sample binaries under KVM: CLI samples (hello, alloc_test, nls_test, thread_test, pipe_test, mutex_test, queue_test, thunk_test, ne_hello, rust_hello, …) with headless output checks, and PM samples driven by GUI scripts on SDL's `dummy` driver — the self-checking samples (`pm_dialog_test`, `pm_list_test`, `pm_msgbox_test`, `pm_typeahead_test`, `pm_codepage_test`, `pm_pointer_test`, `pm_audio_test`, `pm_menu_dlg_test`, …) must report every check as passed, and desktop dumps are compared with golden tile signatures (see "GUI regression tests" below). Each test skips itself when `/dev/kvm` or its sample binary is missing.

```bash
cargo test --test integration     # 73 end-to-end tests (requires /dev/kvm)
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
| `MOD+click X Y` | The same with modifier keys held (`shift+click`, `ctrl+shift+click`): they go down before the press and up after the release. |
| `rclick X Y` | Right button press and release. |
| `dblclick X Y` | Two left clicks; the second press is reported as the second click of a double click (`WM_BUTTON1DBLCLK`). |
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
- **API thunk auto-registration** — `api_registry.rs` static sorted table (216 entries); O(log n) binary search replaces ~120-arm match.
- **SDL2 GUI backend** — migrated from `winit + softbuffer` to SDL2; full keyboard, mouse, clipboard support.
- **PM renderer abstraction** — `PmRenderer` trait with `Sdl2Renderer` and `HeadlessRenderer` backends.
- **GDT tiling** — 4096 tiled 16-bit data descriptors (GDT[6..4101], DPL=2) and 2048 tiled 16-bit code descriptors (GDT[4102..6149]) for 16:16 addressing; GDT[4] 16-bit data alias (0x20), GDT[5] 16-bit code alias (0x28) for Far16 thunks.
- **NE (16-bit OS/2 1.x) execution** — full NE loader in `ne_exec.rs`: segments loaded into GDT-tiled memory, CALL FAR fixups patched to code tile `NE_THUNK_CODE_SELECTOR` (0x87B0), API dispatch via Pascal calling convention thunks, `ne_hello` pure assembly sample runs end-to-end.
- **Modifier key suppression** — pure modifier keys (LShift, RShift, Ctrl, Alt, CapsLock) are filtered before `KbdCharIn` enqueueing; fixes 4OS2 printing raw scan codes on Shift press.
- **Developer tooling** — crash dump facility (`crash_dump.rs`), GDB Remote Stub (`gdb_stub.rs`, `--gdb <port>`), API call ring buffer (`api_ring.rs`, 256 entries).
- **Testing** — 1322 unit tests (1286 warpine + 19 gen_api + 12 lx_link + 5 gen_arch), 89 integration tests, compatibility report (`warpine --compat`).

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
- `validate-doc` — cross-check `os2api.def` against `doc/os2_ordinals.md` (Open Watcom `lib386/os2/os2386.lib` snapshot); reports confirmed ordinals, 16-bit alias pairs auto-detected by uppercase+strip-"16" normalisation, entries only in def, name differences acknowledged by a `# NOTE:` on the def line (naming conventions the normaliser cannot detect, e.g. DOSCALLS 75 `DosQueryFileMode16` / `DOS16QFILEMODE`; listed with the note, not counted as mismatches), and the remaining name mismatches to investigate; it covers DOSCALLS, QUECALLS, KBDCALLS, VIOCALLS, PMGPI, PMWIN, SESMGR, NLS, MOUCALLS, PMSHAPI and HELPMGR

Run `cargo run --bin gen_api -- check` to verify consistency after any ordinal change. Run `cargo run --bin gen_api -- validate-doc` to cross-reference against the OS/2 ordinal reference.

**`src/bin/gen_arch.rs`** — Architecture diagram generator ([ADR-0021](adr/0021-generated-architecture-diagram.md)). The diagram at the head of the reference manual comes from two sources: `doc/architecture/layers.toml`, a hand-written layer map (a subset of TOML — `[[zone]]`, `[[layer]]` with `modules`, `[[external]]`, `[[flow]]`; a zone's `rank = "min"` / `"max"` pins it to the top / bottom row), and the list of `.rs` files under `src/`, which `gen_arch` reads itself.
- `check` — validate (every module in exactly one layer, no layer naming a missing module, every flow between known nodes, known zones) and compare the committed `warpine.dot` with what the sources generate
- `dot` — print the generated DOT
- `render` — write `doc/architecture/warpine.dot` and run Graphviz `dot` for `warpine.svg` and `warpine.png` (96 dpi)

The test `layer_map_matches_src_and_the_committed_diagram` runs the validation and the comparison, so adding, renaming or removing a module fails `cargo test` until `layers.toml` places it and `render` has been run. Flows describe the architecture and are not checked against the call graph.

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
