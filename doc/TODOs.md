# Warpine TODO List

This document tracks the tasks required to reach a functional OS/2 compatibility layer.

## Engineering Policy

**Near-clean-room, blackbox implementation.** Warpine implements the OS/2 API surface from public documentation only — IBM's *Control Program Programming Reference*, the OS/2 Warp 4 Toolkit headers, published IBM Developer Connection materials, and open-source reference implementations (e.g., ReactOS, osFree, WINE analogues). No IBM-proprietary DLL binaries, no ROM dumps, and no disassembly of original OS/2 system libraries are used as implementation inputs. Behaviour is inferred solely from the observable behaviour of OS/2 applications compiled with the Open Watcom toolchain and from the public specifications listed above.

---

## Completed Work

### Phase 1 — Foundation (CLI Hello World)
LX/LE executable parser (MZ header, object table, page map, fixup table). Loader maps LX objects into 128 MB KVM guest memory and applies relocations. API thunk infrastructure: imports resolved to INT 3 trap stubs at `MAGIC_API_BASE` (0x01000000); VMEXIT loop dispatches to Rust handlers by ordinal. Initial DOSCALLS thunks: `DosWrite`, `DosExit`, `DosQuerySysInfo`, `DosQueryConfig`, `DosQueryHType`, `DosGetInfoBlocks`.

### Phase 2 — Core OS/2 Subsystem
Memory: `DosAllocMem` / `DosFreeMem`. Filesystem: `DosOpen/Read/Write/Close/Delete/Move/CreateDir/DeleteDir`, `DosFindFirst/Next`, OS/2 drive-letter path translation. Threads: `DosCreateThread`, `DosKillThread`, TLS via TIB. IPC: event, mutex, and MuxWait semaphores; pipes (`DosCreatePipe`); queues (`DosCreateQueue/WriteQueue/ReadQueue`).

### Phase 3 — Presentation Manager (GUI)
Dual-path execution: PM apps run the SDL2 event loop on the main thread; CLI apps run the vCPU directly. `GUIMessage` channel carries draw/window commands from vCPU thread to main thread. PMWIN: `WinInitialize/Terminate`, message queues, `WinRegisterClass`, `WinCreateStdWindow`, `WinGetMsg/DispatchMsg`, `WinPostMsg/SendMsg`, `WinDefWindowProc`, `WinBeginPaint/EndPaint`, `WinMessageBox`, `WinShowWindow`, `WinDestroyWindow`, timers, dialogs (stubs), menus (stubs), clipboard in-process storage, `WinSetWindowPos`, resource loading (`WinLoadString/Menu/AccelTable/Dlg`). PMGPI: `GpiCreatePS/DestroyPS`, `GpiSetColor`, `GpiMove`, `GpiBox`, `GpiLine`, `GpiCharStringAt`, `GpiErase`. Callback mechanism: `ApiResult::Callback` for re-entrant guest window-procedure calls via `CALLBACK_RET_TRAP`. Input: `WM_CHAR`, `WM_MOUSEMOVE`, `WM_BUTTON1DOWN/UP`, `WM_SIZE`, `WM_CLOSE`. Embedded 8×16 VGA bitmap font for text rendering.

### Phase 3.5 — Text-Mode Application Support (4OS2)
Target: 4OS2 command shell — validates nearly every DOSCALLS/KBD/VIO surface. Expanded thunk stub area (`KBDCALLS_BASE=4096`, `VIOCALLS_BASE=5120`, `SESMGR_BASE=6144`, `NLS_BASE=7168`, `MSG_BASE=8192`). Console: `VioManager` with screen buffer, cursor, raw termios input, ANSI escape output. KBD: `KbdCharIn` (blocking/non-blocking, arrow/function-key escape parsing), `KbdGetStatus`, `KbdStringIn`. VIO: `VioWrtTTY`, `VioGetMode`, `VioGetCurPos`, `VioSetCurPos`, `VioSetCurType`, `VioScrollUp`, `VioScrollDn`, `VioWrtCharStrAtt`, `VioWrtNCell`, `VioWrtNAttr`, `VioReadCellStr`, `VioSetAnsi`, `VioGetAnsi`, `VioGetConfig`. Process: `DosSetCurrentDir`, `DosQueryCurrentDir/Disk`, `DosSetDefaultDisk`, `DosExecPgm`, `DosWaitChild`, `DosKillProcess`, `DosQueryAppType`. System info: full `DosQuerySysInfo` QSV table, `DosGetDateTime`. Stubs: `DosError`, `DosSetMaxFH`, `DosBeep`, exception handlers, shared memory, codepage/country info, module loading stubs, file metadata APIs, device I/O stubs, semaphore extensions, named pipe stubs, session management stubs. Result: 4OS2 boots to a prompt; `dir`, `set`, `ver`, `md`, `rd`, `copy`, `move`, `del`, `attrib`, `tree` all work.

### Phase 4 — HPFS-Compatible Virtual Filesystem
`VfsBackend` trait (21 methods) as OS/2 filesystem semantics contract. `DriveManager` maps drive letters A:–Z: to `Box<dyn VfsBackend>`; owns file and find-handle tables. `HostDirBackend`: case-insensitive case-preserving lookup (optimistic stat + readdir fallback, kernel casefold detection), long filenames (254 chars), file sharing modes, sandbox enforcement, OS/2 wildcard matching, directory listing cache (2s TTL), device name mapping (NUL/CON/CLOCK$/KBD$/SCREEN$). Extended attributes via Linux xattrs (`user.os2.ea.*`) with sidecar `.os2ea/` fallback. `DosEnumAttribute`, `DosQueryPathInfo` levels 1/2/3. File locking via `fcntl(F_SETLK)`. `DosFindFirst/Next` multi-entry packing, attribute filtering, `*.*` HPFS semantics. All `doscalls.rs` filesystem operations route through `DriveManager`. C: drive auto-mounted at `~/.local/share/warpine/drive_c/`. 4OS2 `dir` with correct date/time formatting verified; `samples/file_test`, `find_test`, `fs_ops_test`, `vfs_test` (16/16) all pass.

### Phase 4.5 — 16-bit Thunk Fix
Eliminated 16-bit thunks from 4OS2 by recompiling with modified headers rather than runtime patching — removed ~350 lines of thunk-handling code from the loader. Key patches: `bsesub.h` changed `APIENTRY16` to `_System` (eliminates `__vfthunk` generation); `crt0.c` replaces Watcom's `__OS2Main` (which called `DosGetInfoSeg` via 16-bit thunk) with a pure 32-bit version using `DosGetInfoBlocks`; `viowrap.c` provides `#pragma import` for VIO/KBD ordinals; DOSCALLS/VIOCALLS/KBDCALLS ordinal tables audited and corrected. All 6 patches stored in `samples/4os2/patches/`; `fetch_source.sh` applies them automatically. OS/2 version now correctly reports 4.50 (fixed `_osmajor`/`_osminor` init via `DosQuerySysInfo`).

### Phase 5 Baseline — MMPM/2 Audio
`DosBeep` plays real sine-wave tones via SDL2 audio queue. MDM.DLL (`MDM_BASE=10240`) wired into the ordinal dispatch. `mciSendCommand` handles MCI_OPEN/CLOSE/PLAY/STOP/STATUS for `waveaudio` device. `mciSendString` parses `open`/`play`/`stop`/`close`/`status` command strings. WAV files loaded via VFS using `SDL_LoadWAV_RW`. Audio format conversion via `SDL_BuildAudioCVT`/`SDL_ConvertAudio`. Synchronous play via `MCI_WAIT` flag. 5 tests in `mmpm.rs`.

### Phase 6 — Text-Mode VGA Renderer
`TextModeRenderer` trait (`render_frame`, `poll_events`, `frame_sleep`) with two backends: `Sdl2TextRenderer` (640×400 SDL2 window, CP437 8×16 font, CGA 16-colour palette, blinking cursor) and `HeadlessTextRenderer` (CI/no-op). `run_text_loop()` is the main event loop for CLI apps. `KbdKeyInfo` struct + `SharedState::kbd_queue/kbd_cond/use_sdl2_text` for SDL2→KbdCharIn/DosRead key delivery. VioManager gains `sdl2_mode` (suppresses ANSI output) and `cursor_start/end` (scan-line cursor shape). `get_cp437_glyph()` provides the full 256-glyph CP437 font. CLI apps default to SDL2 text window; `WARPINE_HEADLESS=1` falls back to terminal mode.

Bug fixes included: cursor rendering switched from fg/bg swap to XOR pixel inversion (always visible regardless of cell attribute); `VioGetCurType` (ordinal 33) implemented so 4OS2's read-modify-write cursor setup works; `VioScrollUp`/`VioScrollDn` now correctly read the 7th argument (`pCell` fill-cell pointer) and handle `lines=0` as "clear entire region" per OS/2 semantics; `dos_read_stdin` cooked-mode backspace gated by `stdin_cooked_chars` counter (prevents erasing the shell prompt); backspace-at-start-of-line returns correct blocking behaviour instead of `actual_bytes=0`. 22+ tests (font, palette, headless renderer, queue, scroll, VioGetCurType).

### Architecture — Completed Items

**Virtualization Backend Abstraction** — `VmBackend` / `VcpuBackend` traits in `vm_backend.rs`; KVM implementation isolated to `kvm_backend.rs`; `MockVcpu` / `MockVmBackend` enable guest-memory and VIO handler tests without `/dev/kvm`.

**Guest Memory Type Safety** — `GuestMemory` struct (`guest_mem.rs`) owns the mmap allocation with safe `read<T>`/`write<T>` methods; replaces the former `*mut u8` + `usize` pair in `SharedState`.

**Structured API Trace System** — `api_trace.rs` provides `ordinal_to_name()` and `module_for_ordinal()`; every API call emits a `tracing::debug_span!` with module, ordinal, name, return value, guest eip/esp. `WARPINE_TRACE=strace` for compact span output, `=json` for JSON lines, unset for default pretty logging.

**API Thunk Auto-Registration** — `api_registry.rs` holds a static sorted `&[ApiEntry]` table (122 entries) covering DOSCALLS, QUECALLS, NLS, and MDM. Each `ApiEntry` carries ordinal, module, name, argc, and a type-erased `fn` pointer handler. `find(ordinal)` does O(log n) binary search; `all()` exposes the full table for compatibility reports. `api_dispatch.rs` reduced from ~120-arm match to pre-read + registry lookup + sub-dispatcher routing. Seven registry regression tests.

**SDL2 GUI Backend** — Migrated from `winit + softbuffer` to SDL2. Per-window `Canvas<Window>` + streaming `Texture`. Full keyboard support: `sdl_scancode_to_os2()`, `sdl_keycode_to_vk()`, `build_wm_char()` encode WM_CHAR with KC_* flags, scan codes, and VK_* virtual keys. Mouse buttons 1–3. `SDL_CaptureMouse` wired to `WinSetCapture`/`WinQueryCapture`. Host↔guest clipboard bridging via `SDL_SetClipboardText`/`SDL_GetClipboardText`.

**PM Renderer Abstraction** — `PmRenderer` trait (`handle_message`, `poll_events`, `frame_sleep`) decouples rendering from SDL2. `Sdl2Renderer` and `HeadlessRenderer` backends. `run_pm_loop()` is the main PM event loop. `src/gui/` sub-modules: `message.rs`, `renderer.rs`, `render_utils.rs`, `headless.rs`, `sdl2_renderer.rs`, `text_renderer.rs`.

### Testing Infrastructure
Unit tests, end-to-end integration tests, and a compatibility report — all implemented and passing.

**Unit tests (no KVM)** — Added 28 new tests for `kbdcalls.rs` (KbdGetStatus, KbdCharIn SDL2 path, KbdStringIn error case), `doscalls.rs` (memory, I/O, semaphores, queues, sleep), and `api_dispatch.rs` (routing to KBDCALLS/VIOCALLS sub-dispatchers and DOSCALLS registry). Fixed a latent bug in `new_mock()`: `MemoryManager` limit was set below base, causing all test allocations to fail silently. Total unit tests: 234 (up from 199).

**Integration tests** — `tests/integration.rs`: 8 end-to-end tests run real OS/2 sample binaries (hello, alloc_test, nls_test, thread_test, pipe_test, mutex_test, queue_test, thunk_test) with `WARPINE_HEADLESS=1`, asserting stdout content and exit code. KVM-gated (skip silently when `/dev/kvm` is absent). Run with `cargo test --test integration`.

**Compatibility matrix** — `api_registry::compat_report()` generates a module-grouped report with `[stub]` tags for pure no-op handlers and sub-dispatcher summaries for VIOCALLS/KBDCALLS/PMWIN/PMGPI (215 entry points total). Exposed via `warpine --compat`. Two tests verify report structure and stub count.

---

## Developer Tooling

### A — Enhanced Crash Dump *(complete)*
On any fatal VMEXIT or unhandled guest exception: capture all CPU registers, segment descriptors, top 32 stack dwords, 32 hex bytes at EIP, and context info. Writes to `warpine-crash-<pid>.txt` and prints the full report to stderr. Implemented in `src/loader/crash_dump.rs`.

- [x] `CrashContext` enum — GuestException, TripleFault, UnhandledVmexit, KvmRunError, UnexpectedBreakpoint
- [x] `CrashReport` struct — regs, sregs, stack words, code bytes, exe name, timestamp
- [x] `Loader::collect_crash_report()` — snapshots vCPU state; handles 16-bit SS for correct ESP
- [x] `Loader::dump_crash_report()` — formats with hex+ASCII dump, writes file + stderr
- [x] All four fatal VMEXIT paths in `vcpu.rs` replaced with crash dump calls
- [x] 13 unit tests (format, hex dump, exception names, timestamp, file creation)

### B — GDB Remote Stub *(complete)*
GDB RSP (Remote Serial Protocol) over TCP so `gdb`, `gef`, or `pwndbg` can attach to a live KVM guest.

- [x] Dependencies: `gdbstub = "0.7"`, `gdbstub_arch = "0.3"` in `Cargo.toml`
- [x] `src/loader/gdb_stub.rs` — `WarpineTarget` implements `gdbstub::Target` (`X86_SSE` arch, `SingleThreadBase`, `SingleThreadResume`, `SingleThreadSingleStep`, `Breakpoints`/`SwBreakpoint`)
- [x] `GdbState` — shared Mutex+Condvar synchronisation channel between vCPU thread and GDB stub thread; `stop_requested` AtomicBool for Ctrl-C
- [x] `GdbBlockingEventLoop` — `BlockingEventLoop` impl: polls `stop_cond` with 10 ms timeout, checks TCP for incoming bytes between polls; `on_interrupt` requests vCPU stop
- [x] TCP listener (`launch_gdb_stub`): binds `127.0.0.1:<port>`, accepts one connection, runs `GdbStub::run_blocking::<GdbBlockingEventLoop>`
- [x] Single-step via `KVM_GUESTDBG_SINGLESTEP`: `VcpuBackend::set_single_step` trait method; `KvmVcpu` toggles `KVM_GUESTDBG_ENABLE | KVM_GUESTDBG_USE_SW_BP | KVM_GUESTDBG_SINGLESTEP`
- [x] Software breakpoints: INT 3 (0xCC) patched into guest memory on `add_sw_breakpoint`; original byte restored on hit / `remove_sw_breakpoint`; RIP rewound to breakpoint address; breakpoint re-installed + single-step before resuming
- [x] Memory read/write: full 256 MB guest flat address space via `GuestMemory::read/write`
- [x] Stop on Ctrl-C interrupt with correct `SIGINT` mapping
- [x] `vcpu.rs` integration: initial pause at entry point (sends `SIGTRAP` to GDB client); Ctrl-C polling loop; GDB debug-break path in `VmExit::Debug` before existing API/IDT checks; single-step stop path
- [x] `--gdb <port>` CLI flag: parsed in `main.rs`; `Loader::set_gdb_state()` attaches before first `Arc` clone; `launch_gdb_stub()` spawns the TCP listener thread
- Usage: `warpine --gdb 1234 samples/hello/hello.exe` then `gdb -ex 'target remote :1234'`

### C — API Call Ring Buffer *(complete)*
The last 256 OS/2 API calls are stored in a bounded `VecDeque` in `SharedState`, populated unconditionally (not gated on DEBUG level) so crash dumps include call history even in release/info builds. Implemented in `src/loader/api_ring.rs`.

- [x] `ApiCallRecord` struct: ordinal, module, name, formatted call string, return value, monotonic seq number
- [x] `ApiRingBuffer` — bounded `VecDeque<ApiCallRecord>`, capacity 256, oldest entry evicted when full
- [x] `SharedState::api_ring: Mutex<ApiRingBuffer>` — independent of all other managers
- [x] `api_dispatch.rs` — `format_call` computed once per call (used for both DEBUG tracing and ring); record pushed after result
- [x] `crash_dump.rs` — `CrashReport::api_history` snapshot; rendered as `[seq] MODULE.call() → ret` section
- [x] GDB stub (from B) can expose the ring via a monitor command (future)
- [x] 9 unit tests (push/evict, seq monotonicity, wrap, snapshot order, call_str storage)

---

## Architecture & Refactoring Backlog

### Ordinal Table Canonical Build Tool
Build a tool to manage the authoritative ordinal→name table used by `api_registry.rs`, sourced exclusively from public documentation (IBM CP Programming Reference, OS/2 Warp 4 Toolkit headers, osFree project). **No real OS/2 system DLLs are used as input** (clean-room policy).

Implementation plan:
1. Extend `LxFile` to parse entry table + resident/non-resident name tables (currently only import tables are parsed) — useful for `jpos2dll.dll` and other Open Watcom-built DLLs in `samples/`
2. `src/bin/ordinals.rs` — dump complete `ordinal → name` map from an LX binary built by us; output as text or `--emit-rust` for `const` definitions
3. `--check` mode — cross-reference against warpine's `api_registry` to surface mismatches between documented and implemented ordinals
4. Maintain a hand-curated `doc/ordinals/` directory with one `.txt` per module (DOSCALLS, PMWIN, PMGPI, …) derived from public IBM documentation

### Structured API Trace — Remaining
- [x] Per-argument typed names — `arg_names_for_ordinal()` covers all 122 registry entries + QUECALLS/NLS/MSG/MDM; `format_call()` emits strace-style `DosWrite(hFile=5, pBuf=0x2001000, cbBuf=42, pcbActual=0x2001100)` at DEBUG level; `psz*` args are auto-dereferenced as strings; handle args (`h*`) shown decimal; zero-cost when DEBUG disabled.
- [ ] TUI debug overlay showing live API call stream, memory map, window hierarchy, and PM message queue

---

## Phase 5 — Multimedia and 16-bit Support

### Audio/Video (MMPM/2) — Remaining
- [ ] `mciSendCommand` MCI_SEEK, MCI_RECORD — seek/recording support
- [ ] Audio mixer / volume control (`MCI_SET` with `MCI_SET_AUDIO`)
- [ ] MIDI playback device type (currently only `waveaudio` supported)
- [ ] Non-blocking play completion notification (`MCI_NOTIFY` flag → post `MM_MCINOTIFY` to hwndCallback)

### 16-bit Compatibility (NE format)
NE format parser complete (`src/ne/`): NeHeader, segment/relocation/entry tables, name table, 16 unit tests. NE loader skeleton in place: `load_ne()`, `apply_ne_fixups()`, `setup_guest_ne()`, `setup_and_run_ne_cli()`, `handle_ne_api_call()`, `resolve_import_16()`.

- [x] **GDT tiling** — 4096 tiled 16-bit read/write data descriptors (GDT[6..4102], selectors 0x30..0x8028) populated in `setup_idt`; `DosFlatToSel`/`DosSelToFlat` use tile arithmetic; 16:16 LX fixups write correct tile selectors. Fixes `__Far16Func2` GPF crash and enables Far16 thunks in LX apps.
- [x] **GDT: 16-bit code alias at selector 0x0028** — Added proper 16-bit CODE descriptor at GDT[5]
  (base=0, limit=0xFFFF, exec+read, db=0) and 16-bit DATA alias at GDT[4] (base=0, limit=0xFFFF).
  Tile descriptors shifted to start at GDT[6] (selector 0x0030, `TILED_SEL_START_INDEX=6`).
  `GDT_ENTRY_COUNT` updated to 4102. Fixes `#GP(0x0028)` crash when 4OS2's Far16 thunk stubs
  execute `JMP FAR 0x0028:xxxx` to enter 16-bit execution mode for JPOS2DLL calls.
- [ ] **16-bit API thunking** — NE apps use Pascal calling convention and `_far16` pointers; add 16-bit dispatch alongside existing 32-bit `_System` dispatch, with segment:offset ↔ flat address translation
- [ ] **Mode switching** — handle transitions between 16-bit NE code and 32-bit flat code (e.g., 16-bit app calling a 32-bit DLL)

---

## Phase 7: Application Compatibility Expansion

Goal: raise the fraction of real OS/2 applications that run correctly.

### DLL Loader Chain (highest priority — blocks nearly everything)
**Baseline complete** — `jpos2dll.dll` (4OS2 extension DLL) loads successfully at runtime.

Completed items:
- [x] Parse LX entry table (ordinal → object + offset) and non-resident names table (ordinal → name)
- [x] `load_dll()` — allocate guest memory for each object, load pages (rebased), apply fixups
- [x] Ordinal-based and name-based export maps; `DllManager` in `SharedState`
- [x] `DosLoadModule` — finds DLL by name (exe dir + C:\OS2\DLL\), loads it, returns HMODULE
- [x] `DosQueryProcAddr` — ordinal or name lookup from `DllManager`
- [x] `DosQueryModuleHandle` — name lookup
- [x] `resolve_import` checks `DllManager` for user DLLs (after built-in thunks)
- [x] `jpos2dll.dll` built by `samples/4os2/Makefile` (`make jpos2dll.dll`)

Remaining:
- [ ] Recursive/static import loading — load a DLL's dependent DLLs from its import table before applying fixups (currently only built-in emulated modules work as DLL dependencies)
- [ ] Call DLL initialisation routines (`DLL_INITTERM` / `eip_object`) at load and unload time
- [ ] `DosFreeModule` — proper reference counting and unload
- [ ] Handle load-order dependencies and circular imports

### DOSCALLS Long Tail
- [ ] **Structured Exception Handling** — real per-thread handler chain; `DosRaiseException`; `DosUnwindException`
- [ ] **Environment** — `DosScanEnv`, `DosSetExtLIBPATH`, `DosQueryExtLIBPATH`
- [ ] **NLS / DBCS** — `DosQueryDBCSEnv` (DBCS lead-byte table), full `DosMapCase` for non-Latin codepages
- [ ] **Thread priorities** — `DosSetPriority` (idle / regular / time-critical / server classes); currently ignored
- [ ] **`DosWaitThread`** — reliable join with timeout; `DosKillThread` — correct cleanup

### Unicode-Internal Architecture (long-term goal)
Convert Warpine's internal string representation to UTF-8, with codepage↔UTF-8 conversion at every guest/host API boundary. Modelled on Wine's ANSI→UTF-16 approach.

- [ ] **Conversion helpers** — `cp_decode(bytes, cp) -> String` / `cp_encode(s, cp) -> Vec<u8>` using `encoding_rs` crate (covers CP850, CP932, CP949, CP950, CP936 and all other OS/2 codepages)
- [ ] **Codepage state** — `DosQueryCp`/`DosSetProcessCp` track the active process codepage in `SharedState`; plumb it through all conversion sites
- [ ] **Path strings** — `DosOpen`, `DosFindFirst/Next`, `DosDelete`, `DosMove`, etc.: decode guest path bytes → UTF-8 before VFS lookup; encode result strings back to guest CP on return
- [ ] **VIO output** — `VioWrtTTY`, `VioWrtCharStrAtt`, etc.: decode CP bytes → Unicode codepoints at write time; `VioManager` screen buffer becomes `Vec<(char, u8)>` (codepoint + attribute)
- [ ] **SDL2 text renderer** — replace static CP437 8×16 bitmap glyph table with GNU Unifont (see *GNU Unifont Integration* sections above); Phase A covers SBCS, Phase B covers DBCS 16×16 glyphs
- [ ] **PM strings** — `WinSetWindowText`, window titles, menu items, clipboard text: decode at PM API entry
- [ ] **UCONV.DLL** — implement `UniCreateUconvObject`, `UniUconvToUcs`, `UniUconvFromUcs` etc. using `encoding_rs`; unlocks OS/2 apps that do their own Unicode conversion

Sequencing: codepage state → path strings → VIO output → screen buffer/font → PM strings → UCONV.DLL.

### GNU Unifont Integration — SBCS (Phase A)

Replace the hand-crafted partial CP437 font with full 256-glyph tables generated at build time from GNU Unifont, then extend to additional SBCS code pages. Unifont is GPL-2+ with a font exception (compatible with GPL-3 Warpine for static embedding).

**Source files to vendor:**
- `vendor/unifont/unifont-<ver>.hex` — Unicode BMP (8×16 for SBCS, 16×16 for CJK)
- `vendor/codepage/CP437.TXT`, `CP850.TXT`, `CP852.TXT`, `CP866.TXT` — Unicode Consortium CP→Unicode mapping tables

**A1 — `build.rs` extractor**
- [ ] For each target codepage: parse `CP<n>.TXT` (u8 → char), look up each of the 256 codepoints in Unifont, emit `src/generated/font_cp<n>.rs` with `pub static GLYPHS: [[u8; 16]; 256]`
- [ ] Skip 16×16 Unifont entries (used only for DBCS — Phase B); undefined bytes → blank `[0u8; 16]`
- [ ] Generated files committed; `build.rs` only reruns if vendor sources change

**A2 — Codepage dispatcher in `text_renderer.rs`**
- [ ] `get_glyph_sbcs(ch: u8, cp: u32) -> [u8; 16]` dispatches to the correct generated table
- [ ] CP targets for initial delivery: 437 (drop-in), 850 (Western Europe), 852 (Central Europe), 866 (Cyrillic)

**A3 — Thread `active_codepage` through to renderer**
- [ ] Add `active_codepage: u32` to `VgaTextBuffer`, populated from `SharedState::locale.codepage` at snapshot time
- [ ] Pass it into `render_frame()` and down to `get_glyph_sbcs()`

**A4 — Cleanup**
- [ ] Delete `src/font8x16.rs` and the hand-crafted `match` block in `get_cp437_glyph()`
- [ ] Update `src/gui/mod.rs` exports; remove `get_cp437_glyph` from public API
- [ ] Unlock `Os2Locale::codepage` for non-437 SBCS locales (850/852/866) once Watcom CRT path is confirmed safe

---

### GNU Unifont Integration — DBCS (Phase B)

DBCS (Double-Byte Character Set) support for CP932 (Shift-JIS / Japanese), CP936 (GBK / Simplified Chinese), CP949 (EUC-KR / Korean), CP950 (Big5 / Traditional Chinese). Depends on Phase A being complete.

**OS/2 DBCS cell model** (important context):
In OS/2 VIO text mode a DBCS character occupies two consecutive screen cells: cell N holds the lead byte + attribute, cell N+1 holds the trail byte + same attribute. `VioCheckCharType` distinguishes SBCS=0, DBCS-lead=2, DBCS-trail=3. `VioManager::buffer: Vec<(u8, u8)>` already stores raw lead/trail bytes naturally — no storage format change is needed.

**B1 — Lead-byte range tables**
- [ ] `dbcs_lead_ranges(cp: u32) -> &'static [(u8, u8)]` in `locale.rs`:
  - CP932: `(0x81, 0x9F), (0xE0, 0xFC)`
  - CP936 / 949 / 950: `(0x81, 0xFE)`
  - All others: `&[]` (SBCS)

**B2 — `CellKind` annotation in `VgaTextBuffer`**
- [ ] Add `pub enum CellKind { Sbcs, DbcsLead, DbcsTail }` and `pub cell_kind: Vec<CellKind>` (parallel to `cells[]`)
- [ ] `VgaTextBuffer::snapshot()` runs `annotate_dbcs(cells, codepage)` — a single left-to-right scan using `dbcs_lead_ranges()`; marks DBCS pairs, leaves everything else as `Sbcs`; O(cols×rows) per frame

**B3 — 16×16 DBCS render path in `Sdl2TextRenderer::render_frame()`**
- [ ] Replace column `for` loop with a `while col < cols` loop:
  - `DbcsLead`: decode lead+trail → Unicode codepoint, look up 16×16 Unifont glyph (`[u8; 32]`), render into pixels at `col*8 .. col*8+16` (two SBCS column widths), advance `col += 2`
  - `DbcsTail`: `col += 1` (already drawn by its lead cell)
  - `Sbcs`: existing 8×16 path, `col += 1`
- [ ] Window stays 640 px wide (80 cols × 8 px) — no resize needed

**B4 — DBCS Unicode mapping tables (build.rs extension)**
- [ ] Vendor `SHIFTJIS.TXT`, `GBK.TXT`, `KSX1001.TXT`, `BIG5.TXT` (Unicode Consortium)
- [ ] `build.rs` emits `src/generated/dbcs_cp<n>.rs`: sorted `&[(u16, u32)]` (DBCS codeword → Unicode codepoint); runtime lookup via `binary_search_by_key`
- [ ] `decode_dbcs(lead: u8, trail: u8, cp: u32) -> char` utility function

**B5 — 16×16 glyph extraction from Unifont**
- [ ] `build.rs` extracts Unifont `.hex` entries with 64 hex chars (16×16) as `[u8; 32]`
- [ ] Emit `src/generated/font_dbcs_wide.rs`: sorted `&[(u32, [u8; 32])]` keyed by Unicode codepoint
- [ ] `get_glyph_dbcs(cp: char) -> [u8; 32]` — `binary_search_by_key` lookup; falls back to two half-width glyphs if not found
- [ ] Scope: CJK Unified Ideographs (U+4E00–U+9FFF), Hangul Syllables (U+AC00–U+D7A3), Kana blocks (~20k–30k entries total, ~600 KB–1 MB per generated file)

**B6 — `NlsGetDBCSEv` — return real lead-byte table**
- [ ] Update the current empty-table stub to return the correct `(first, last)` pairs for the active DBCS codepage, terminated by `(0, 0)` per OS/2 spec

**B7 — `VioCheckCharType` (new VIO API)**
- [ ] `VioCheckCharType(pType *u16, row u16, col u16, hvio u16) → u32`
- [ ] Scans `VioManager::buffer` from column 0 of the given row to correctly classify mid-DBCS positions (must be left-to-right, stateful — cannot annotate a single cell in isolation)
- [ ] Returns 0 (SBCS), 2 (DBCS lead), 3 (DBCS trail)
- [ ] Register in `api_registry.rs` under `VIOCALLS_BASE`

**B8 — DBCS keyboard re-encoding**
- [ ] SDL2 `SDL_TEXTINPUT` events deliver UTF-8; re-encode to active DBCS codepage before pushing to `kbd_queue`
- [ ] Requires reverse mapping (Unicode → CP codeword) — derive from the same build.rs mapping tables

**Implementation order:** B1 → B2 → B3 → B4+B5 (parallel) → B6 → B7 → B8

**Key risks:**
| Risk | Mitigation |
|---|---|
| Watcom CRT crash on non-437 locale | Keep codepage=437 for 4OS2; unlock only per-app |
| DBCS trail byte collides with SBCS range | Annotation must always scan left-to-right from column 0 |
| Unifont missing glyphs for some codepoints | Fall back to two half-width 8×16 glyphs |
| Generated file size (CP932 ~1 MB) | Acceptable; or use `include_bytes!` + runtime decode |
| `VioCheckCharType` mid-row query | Scan full row from col 0, not just the queried position |

---

### VGA Text Renderer — Remaining
- [ ] **Window resize** — dynamic resize of the SDL2 text window to match VioManager rows/cols (currently fixed at 80×25)

---

### Code Page and DBCS Support
- [ ] `DosQueryCp` / `DosSetProcessCp` — track current process code page accurately (prerequisite for Phase B above)
- [ ] Full `DosMapCase` for non-Latin codepages (CP852, CP866, CP932, etc.)

### PM Advanced Controls
- [ ] **`WC_CONTAINER`** — Icon / Name / Text / Detail / Tree view modes; record management; sorting and filtering
- [ ] **`WC_NOTEBOOK`** — tabbed property sheet
- [ ] **Dialog template parsing** — load `DLGTEMPLATE` from LX resource; auto-create child windows; enables real `WinDlgBox` / `WinLoadDlg`
- [ ] **`WinSubclassWindow`** — replace window procedure and chain to original
- [ ] **Drag and drop** — `DrgDrag`, `DrgAccessDraginfo`, `DM_DRAGOVER` / `DM_DROP`
- [ ] **Custom cursors** — `WinSetPointer` via `SDL_CreateColorCursor`
- [ ] **Printing** — `DevOpenDC`, `DevCloseDC`, basic spool API stubs

### TCP/IP Socket API
- [ ] `SO32DLL.DLL` / `TCP32DLL.DLL` thunks: `socket`, `bind`, `connect`, `listen`, `accept`, `send`, `recv`, `select`, `gethostbyname`, `getservbyname`, `setsockopt`, `getsockopt`, `closesocket`
- [ ] Map to Linux BSD socket syscalls; handle OS/2 `SOCE*` error codes → errno mapping
- [ ] Enables: WebExplorer, Netscape for OS/2, FTP/IRC clients, network-licensed software

### REXX Interpreter Bridge
- [ ] Bridge `REXXAPI.DLL` exports (`RexxStart`, `RexxRegisterSubcomDll`, `RexxVariablePool`) to [Regina REXX](http://regina-rexx.sourceforge.net/)
- [ ] Unlocks: OS/2 installation programs, system tools, 4OS2 `.cmd` scripts

### Year 2038 Problem
- [ ] Audit `time_t` usage in DOSCALLS and CRT shim functions
- [ ] `DosGetDateTime` / `DosSetDateTime` use `DATETIME` struct (`USHORT` year) — not affected; verify and document
- [ ] Intercept and redirect CRT time functions imported from CLIB.DLL / CRTL.DLL / EMX.DLL to 64-bit-clean host implementations
- [ ] `FILESTATUS3` timestamps use `FDATE`/`FTIME` (7-bit year from 1980, max 2107) — not affected; verify
- [ ] Optional: `WARPINE.DLL` escape hatch — `WrpGetDateTime64` / `WrpTime64` for programs that can be recompiled

---

## Phase 8: SOM / Workplace Shell (Long-term)

The Workplace Shell (WPS) is built entirely on IBM's System Object Model (SOM). This is a multi-year effort.

### SOM Runtime Core (prerequisite for WPS)
- [ ] Object / class model: SOM class objects, method table dispatch, offset-based and name-lookup dispatch
- [ ] `SOMClassMgrObject` — global class manager; `SOMClassMgr_somFindClass()`, class registration, DLL-based class loading
- [ ] IDL metadata: parse or reconstruct method offsets and class hierarchy at runtime
- [ ] Binary ABI compatibility with IBM SOM 2.1 so WPS extensions (XWorkplace, Object Desktop) load without recompilation

### WPS Object Hierarchy (requires SOM runtime)
- [ ] `WPObject` — root: `wpInitData`, `wpSaveState`, `wpRestoreState`, `wpQueryTitle`, `wpOpen`, `wpDragOver`, `wpDrop`
- [ ] `WPFileSystem` — `wpQueryFilename`, `wpQueryAttr`
- [ ] `WPFolder` — Icon / Detail / Tree via `WC_CONTAINER`; `wpPopulate`
- [ ] `WPDesktop` — singleton root desktop; persists object positions in OS2.INI
- [ ] `WPProgram` — launches via `DosExecPgm`; `WPDataFile` — `.TYPE` EA for app association
- [ ] Persistence via `PrfWriteProfileData` / `PrfQueryProfileData` (OS2.INI / OS2SYS.INI)
- [ ] Settings notebook: `WinLoadDlg` + `WC_NOTEBOOK` + per-class property pages
- [ ] Drag and drop protocol: `wpDragOver` / `wpDrop` / `wpCopyObject` / `wpMoveObject`

---

## Phase 9: XE — 64-bit OS/2-lineage Platform (far future / vision)

Goal: define and implement a new 64-bit executable format and API set as a natural evolution of the OS/2 lineage. XE apps run natively on Warpine alongside existing 32-bit LX apps. This transforms Warpine from a pure compatibility layer into a dual-ABI OS personality for x86-64 Linux.

### XE Executable Format

A new format following the MZ → LX precedent: MZ stub with `"XE"` signature at `e_lfanew`, fields widened to 64 bits where addresses appear.

- [ ] Define format spec: XE header (signature, cpu_type, object_count, entry_rip: u64, entry_rsp: u64), 64-bit object table (base_address: u64, size: u64, flags: u32), 64-bit fixup records, import/export tables (ordinal → u64 offset)
- [ ] `src/xe/` parser module mirroring `src/lx/` structure
- [ ] `detect_format()` in `main.rs` recognises `"XE"` signature
- [ ] `Loader::load_xe()` / `run_xe()` path in `lx_loader.rs`

### KVM Long Mode Execution

- [ ] vCPU initialisation in long mode (set `EFER.LME`, enable 4-level paging, 64-bit GDT — segments mostly flat, FS/GS for TIB/PIB)
- [ ] 64-bit `SharedState` TIB/PIB layout at well-known addresses
- [ ] INT 3 thunk mechanism unchanged — works identically in long mode; thunk handler reads args from `rdi/rsi/rdx/rcx/r8/r9` (System V AMD64 ABI) instead of the stack

### Calling Convention

**System V AMD64 ABI** (`rdi, rsi, rdx, rcx, r8, r9`, caller-saves `rax/rcx/rdx/rsi/rdi/r8–r11`, return in `rax`). Rationale: universal toolchain support (Rust, Clang, GCC) with no custom patches needed; Warpine's Rust host code already uses this ABI natively.

- [ ] Document `_XE64` calling convention in `doc/`
- [ ] Update `api_dispatch.rs` to extract arguments from 64-bit registers for XE calls

### 64-bit API Set (`DOSCALLS64`, `PMWIN64`, …)

Clean-break 64-bit API — pointer-sized arguments, 64-bit handles, `size_t` buffer lengths. New ordinal namespace separate from 32-bit DOSCALLS.

- [ ] Core I/O: `DosWrite64`, `DosRead64`, `DosOpen64`, `DosClose64`, `DosExit64`
- [ ] Memory: `DosAllocMem64` (full 64-bit address space), `DosFreeMem64`
- [ ] Threads: `DosCreateThread64`, `DosWaitThread64`
- [ ] Synchronisation: `DosCreateEventSem64`, `DosCreateMutexSem64`
- [ ] PM: `WinInitialize64`, `WinCreateStdWindow64`, `WinGetMsg64`, `WinDispatchMsg64` — same message model, 64-bit pointers
- [ ] `UCONV64.DLL` — Unicode conversion using UTF-8 natively (complements Unicode-internal architecture goal)

### Rust/Clang Toolchain Support

- [ ] `warpine-xe` Rust crate: safe bindings to the 64-bit API set; `#![no_std]` compatible
- [ ] Custom Rust target spec `x86_64-warpine-xe` (bare-metal, System V ABI, XE binary output via custom linker script)
- [ ] Sample XE app written in Rust: `samples/xe_hello/` — `DosWrite64` to stdout, `DosExit64`
- [ ] Sample XE app written in C (Clang `x86_64-unknown-none`): validates the ABI from C

### Dual-ABI Coexistence

- [ ] 32-bit LX apps and 64-bit XE apps run side-by-side under the same Warpine instance
- [ ] `DosExecPgm` detects XE format and spawns a 64-bit vCPU thread
- [ ] Shared `SharedState` managers (memory, handles, semaphores) serve both 32-bit and 64-bit guests
