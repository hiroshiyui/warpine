# Warpine TODO List

This document tracks the tasks required to reach a functional OS/2 compatibility layer.

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

### Architecture — Completed Items

**Virtualization Backend Abstraction** — `VmBackend` / `VcpuBackend` traits in `vm_backend.rs`; KVM implementation isolated to `kvm_backend.rs`; `MockVcpu` / `MockVmBackend` enable guest-memory and VIO handler tests without `/dev/kvm`.

**Guest Memory Type Safety** — `GuestMemory` struct (`guest_mem.rs`) owns the mmap allocation with safe `read<T>`/`write<T>` methods; replaces the former `*mut u8` + `usize` pair in `SharedState`.

**Structured API Trace System** — `api_trace.rs` provides `ordinal_to_name()` and `module_for_ordinal()`; every API call emits a `tracing::debug_span!` with module, ordinal, name, return value, guest eip/esp. `WARPINE_TRACE=strace` for compact span output, `=json` for JSON lines, unset for default pretty logging.

**SDL2 GUI Backend** — Migrated from `winit + softbuffer` to `sdl2 = { version = "0.37", features = ["unsafe_textures"] }`. `src/gui.rs` rewritten: polling event loop (`run_gui_loop`), per-window `Canvas<Window>` + streaming `Texture` with `BlendMode::None` for opaque pixel rendering. `build.rs` emits `cargo:rustc-link-search` from `pkg-config` so `rust-lld` finds `libSDL2.so` on Debian multi-arch.

---

## Architecture & Refactoring Backlog

### API Thunk Auto-Registration
- [ ] Replace the large `match ordinal` dispatch table in `api_dispatch.rs` with a registry-based mechanism
- [ ] Option A: procedural macro (`#[os2_api(module = "DOSCALLS", ordinal = 299)]`) auto-registers handlers at compile time
- [ ] Option B: `inventory` crate for distributed static registration without a proc macro
- [ ] Benefits: auto-generate stub lists, compatibility matrices, and missing-API reports; reduces `api_dispatch.rs` maintenance burden as ordinal count grows

### Ordinal Table Canonical Build Tool
- [ ] Write a standalone tool that reads real OS/2 system DLLs (DOSCALLS.DLL, PMWIN.DLL, PMGPI.DLL, etc.) using the LX parser and dumps the complete `ordinal → export name` mapping
- [ ] Use this as ground truth instead of documentation (different fixpak levels can differ; documentation has errors)
- [ ] Auto-generate a Rust source file with `const` ordinal definitions and a verification table
- [ ] Cross-reference against the import tables of target binaries to catch mapping mismatches early
- [ ] Note: the same ordinal can map to different APIs across OS/2 versions (1.x 16-bit vs 2.x 32-bit); the tool should handle multi-version comparison

### Structured API Trace — Remaining
- [ ] Per-argument typed names (e.g. `DosWrite(hfile=1, pBuf=0x500, cbBuf=42)`) — raw eip/esp captured now; argument decoding is future work
- [ ] TUI debug overlay showing live API call stream, memory map, window hierarchy, and PM message queue

### PM Renderer Abstraction
- [ ] Define a `PmRenderer` trait: `render_frame(buf: &VgaTextBuffer)`, `render_pm_window(...)`, `poll_key_event() -> Option<Os2ScanCode>`, `poll_mouse_event() -> Option<Os2MouseEvent>`
- [ ] SDL2 is the concrete backend; the trait boundary would enable a headless renderer for CI/automated PM application testing

### SDL2 Backend — Remaining
- [ ] OS/2 scan code table: map SDL2 `Scancode` to IBM PC Set-1 scan codes for accurate WM_CHAR SC field
- [ ] `SDL_CaptureMouse` for `WinSetCapture` semantics
- [ ] `SDL_SetClipboardText` / `SDL_GetClipboardText` for host clipboard bridging
- ~~SDL2 audio subsystem for `DosBeep` and future MMPM/2~~ — done: `DosBeep` plays real tones; `mciSendCommand`/`mciSendString` waveaudio implemented via SDL2 audio queue in `src/loader/mmpm.rs`

### Testing Strategy
- [ ] **Unit tests (no KVM):** `VmBackend` mock exists; extend coverage so individual API thunk functions can be tested with arbitrary guest memory and register state
- [ ] **Integration tests:** run OS/2 binaries from `samples/` end-to-end in CI; capture stdout + exit code for regression detection; extend to cover `pm_hello`, `screen_test`, `nls_test`, `thunk_test`
- [ ] **Compatibility matrix:** track which OS/2 API ordinals are implemented, stubbed, or missing; generate a report from the ordinal registry; use real OS/2 test programs (e.g., Open Watcom-compiled conformance suites) as targets

---

## Phase 5: Multimedia and 16-bit Support

### Audio/Video (MMPM/2)
MMPM/2 baseline done. MDM.DLL (`MDM_BASE=10240`) wired into the ordinal dispatch. `DosBeep` plays a real sine-wave tone via SDL2 audio queue. `mciSendCommand` handles MCI_OPEN/CLOSE/PLAY/STOP/STATUS for `waveaudio` device. `mciSendString` parses `open`/`play`/`stop`/`close`/`status` command strings. WAV files loaded via the VFS using `SDL_LoadWAV_RW`. Audio format conversion via `SDL_BuildAudioCVT`/`SDL_ConvertAudio` when device output format differs. Synchronous play via `MCI_WAIT` flag. 5 tests in `mmpm.rs`.

- [ ] `mciSendCommand` MCI_SEEK, MCI_RECORD — seek/recording support
- [ ] Audio mixer / volume control (`MCI_SET` with `MCI_SET_AUDIO`)
- [ ] MIDI playback device type (currently only `waveaudio` supported)
- [ ] Non-blocking play completion notification (`MCI_NOTIFY` flag → post `MM_MCINOTIFY` to hwndCallback)

### 16-bit Compatibility (NE format)
NE format parser complete (`src/ne/`): NeHeader, segment/relocation/entry tables, name table, 16 unit tests. NE loader skeleton in place: `load_ne()`, `apply_ne_fixups()`, `setup_guest_ne()`, `setup_and_run_ne_cli()`, `handle_ne_api_call()`, `resolve_import_16()`.

- [ ] **GDT tiling** — create 16-bit segment descriptors in the GDT for each NE segment (one per 64KB region). KVM executes 16-bit code natively; the CPU switches between 16-bit and 32-bit segments when descriptors are set up correctly. Also fixes 16-bit thunks in LX apps as a side effect.
- [ ] **16-bit API thunking** — NE apps use Pascal calling convention and `_far16` pointers; add 16-bit dispatch alongside existing 32-bit `_System` dispatch, with segment:offset ↔ flat address translation
- [ ] **Mode switching** — handle transitions between 16-bit NE code and 32-bit flat code (e.g., 16-bit app calling a 32-bit DLL)

---

## Phase 6: Text-Mode VGA Renderer

Goal: replace the current ANSI-escape terminal approach with a proper VGA text-mode framebuffer rendered into a window.

### Architecture
```
  VIO API layer (viocalls.rs)
        │
        ▼
  VgaTextBuffer (cells: [char+attr; 80×25], cursor state, dirty tracking)
        │
        ▼
  TextModeRenderer trait
        │
        ▼
      SDL2 backend
```

- [ ] **`VgaTextBuffer` struct** — 80×25 (configurable) grid of `VgaCell { character: u8, attribute: u8 }`, cursor position/visibility, dirty-cell tracking (`BitVec`)
- [ ] **VIO API handlers updated** — `VioWrtTTY`, `VioWrtCellStr`, `VioWrtCharStrAtt`, `VioWrtNCell`, `VioScrollUp`, `VioScrollDn`, etc. write into `VgaTextBuffer` instead of emitting ANSI escape sequences
- [ ] **CP437 bitmap font** — embed 8×16 IBM VGA CP437 font via `include_bytes!`
- [ ] **CGA/EGA 16-colour palette** — standard `#000000`…`#FFFFFF` mapping; attribute byte bits 0–3 = foreground, 4–6 = background, 7 = blink/bright-bg
- [ ] **Renderer loop** — scan dirty cells each frame; blit 8×16 glyph pixels to SDL2 pixel buffer; target ≤16ms frame time
- [ ] **`TextModeRenderer` trait** — `render_frame(&VgaTextBuffer)`, `poll_key_event() -> Option<Os2KeyEvent>`
- [ ] **Cursor rendering** — honour `VioSetCurType` start/end scan line for block, underline, or hidden cursor; blink via timer
- [ ] **Keyboard scan codes** — map SDL2 `SDL_Scancode` to OS/2 scan code + character code pairs; handle extended keys (arrows, Home/End/PgUp/PgDn/Ins/Del, F1–F12)
- [ ] **DBCS font support (future)** — CP932/CP950: 16×16 double-width glyph set; `VgaCell` extended to flag lead/trail bytes

---

## Phase 7: Application Compatibility Expansion

Goal: raise the fraction of real OS/2 applications that run correctly.

### DLL Loader Chain (highest priority — blocks nearly everything)
- [ ] Parse LX import table and recursively load dependent DLLs
- [ ] Support both ordinal-based and name-based imports
- [ ] Resolve export tables from loaded DLL LX objects
- [ ] Call DLL initialisation routines (`DLL_INITTERM` entry point) at load and unload time
- [ ] `DosLoadModule` / `DosFreeModule` — full runtime dynamic loading (currently stubs)
- [ ] `DosQueryModuleHandle` / `DosQueryProcAddr` — runtime symbol resolution
- [ ] Handle load-order dependencies and circular imports
- [ ] Option: load real OS/2 system DLL binaries alongside emulated ones (selective real-DLL execution)

### DOSCALLS Long Tail
- [ ] **Structured Exception Handling** — real per-thread handler chain; `DosRaiseException`; `DosUnwindException`
- [ ] **Environment** — `DosScanEnv`, `DosSetExtLIBPATH`, `DosQueryExtLIBPATH`
- [ ] **NLS / DBCS** — `DosQueryDBCSEnv` (DBCS lead-byte table), full `DosMapCase` for non-Latin codepages
- [ ] **Thread priorities** — `DosSetPriority` (idle / regular / time-critical / server classes); currently ignored
- [ ] **`DosWaitThread`** — reliable join with timeout; `DosKillThread` — correct cleanup

### Code Page and DBCS Support
- [ ] `DosQueryCp` / `DosSetProcessCp` — track current process code page accurately
- [ ] DBCS lead-byte table for CP932, CP949, CP950, CP936 — needed for `DosQueryDBCSEnv` and multi-byte VIO string handling
- [ ] VGA font loader for DBCS (16×16 full-width glyphs) — needed for Phase 6 text renderer

### PM Advanced Controls
- [ ] **`WC_CONTAINER`** — Icon / Name / Text / Detail / Tree view modes; record management; sorting and filtering
- [ ] **`WC_NOTEBOOK`** — tabbed property sheet
- [ ] **Dialog template parsing** — load `DLGTEMPLATE` from LX resource; auto-create child windows; enables real `WinDlgBox` / `WinLoadDlg`
- [ ] **`WinSubclassWindow`** — replace window procedure and chain to original
- [ ] **Drag and drop** — `DrgDrag`, `DrgAccessDraginfo`, `DM_DRAGOVER` / `DM_DROP`
- [ ] **Mouse capture** — `WinSetCapture` / `WinQueryCapture` via `SDL_CaptureMouse`
- [ ] **Custom cursors** — `WinSetPointer` via `SDL_CreateColorCursor`
- [ ] **Clipboard bridging** — `WinSetClipbrdData` / `WinQueryClipbrdData` via `SDL_SetClipboardText` / `SDL_GetClipboardText`
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
