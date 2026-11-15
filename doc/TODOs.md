# Warpine TODO List

This document tracks **open work only**. Completed items are documented in the
[Developer Guide](developer_guide.md) and [Reference Manual](reference_manual.md).

## Engineering Policy

**Near-clean-room, blackbox implementation.** All API behaviour is derived from
public documentation only (IBM *Control Program Programming Reference*, OS/2 Warp 4
Toolkit headers, IBM Developer Connection, ReactOS/osFree reference). No IBM-proprietary
DLL binaries, no ROM dumps, and no disassembly of original OS/2 system libraries.

---

## Completed Phases (summary)

| Area | Status | Reference |
|------|--------|-----------|
| Phases 1–7 baseline | Complete | [Developer Guide §20](developer_guide.md#appendix-development-phases) |
| LX/NE loader, KVM vCPU, GDT/IDT | Complete | Developer Guide §3–6 |
| DOSCALLS core (file I/O, memory, threads, semaphores, IPC) | Complete | Reference Manual §9 |
| VIO/KBD console subsystem | Complete | Developer Guide §15 |
| SDL2 VGA text renderer + GNU Unifont (SBCS + DBCS 16×16) | Complete | Developer Guide §15 |
| DBCS full support (B1–B8: cell annotation, render, encode, keyboard) | Complete | Developer Guide §15 |
| PM window management + built-in controls | Complete | Developer Guide §11 |
| GPI drawing primitives (20+ ordinals) | Complete | Developer Guide §12 |
| MMPM/2 audio (DosBeep, waveaudio MCI) | Complete | Developer Guide §10 |
| NE (16-bit OS/2 1.x) execution | Complete | Developer Guide §7 |
| Unicode-internal architecture (codepage↔UTF-8 at all boundaries) | Complete | Developer Guide §16 |
| UCONV.DLL emulation | Complete | Reference Manual §9 |
| DLL loader (recursive, ref-counted, INITTERM, builtin modules) | Complete | Developer Guide §20 |
| Structured Exception Handling (SEH) — DosSetExceptionHandler, DosRaiseException, DosUnwindException | Complete | Developer Guide (SEH section) |
| DosMapCase / NlsMapCase — full SBCS + DBCS + CP866 | Complete | Developer Guide §16 |
| Developer tooling (crash dump, GDB stub, API ring buffer) | Complete | Developer Guide §19 |
| Builtin CMD.EXE host Rust shell (core built-ins + .CMD scripts) | Complete | `src/loader/cmd.rs` |
| CMD.EXE I/O redirection (`>`, `>>`, `<`) + pipe (`\|`) + sample script | Complete | `src/loader/cmd.rs`, `samples/cmd_test/test.cmd` |
| Rust Guest Toolchain | Complete | Targets, lx-link linker, warpine-os2 crate family, rust_hello sample, test_rust_hello |
| Ordinal Table Canonical Build Tool | Complete | `src/bin/gen_api.rs`; `targets/os2api.def` is single source of truth |
| PM Menu System (MENUTEMPLATE parser, WinLoadMenu, WinCreateMenu, FCF_MENU) | Complete | `src/loader/pm_win.rs` |
| PM Dialog System (DLGTEMPLATE parser, WinDlgBox, WinLoadDlg, WinProcessDlg, WinDismissDlg, WinDefDlgProc, WinSendDlgItemMsg, DlgRunLoop) | Complete | `src/loader/pm_win.rs`, `src/loader/vcpu.rs` |
| PM typography — IBM Plex Sans/Mono/Serif + Plex Sans JP/KR/TC/SC embedded; `GpiCreateLogFont` face/style/size mapping; real proportional font metrics | Complete | `src/gui/font.rs`, `vendor/ibm-plex`, Developer Guide (PM Typography) |
| PM menu bar as frame chrome — `FCF_MENU` auto-load, client area below the bar, compositor-drawn bar and pull-down | Complete | `src/loader/pm_types.rs`, `src/gui/sdl2_renderer.rs` |
| Bevelled window chrome — raised title-bar buttons with pictograms, bevelled sizing border | Complete | `src/gui/sdl2_renderer.rs` |
| Virtual Desktop Renderer (VDR) — single-surface compositor, full window chrome, input routing, dirty-rect tracking (A1–A5/B1–B4/C1–C5/D1–D6/E1–E3) | Complete | `src/gui/sdl2_renderer.rs`, `src/loader/pm_types.rs` |
| TCP/IP Socket API — SO32DLL/TCP32DLL (51 thunks: socket/bind/connect/listen/accept/send/recv/select/setsockopt/getsockopt/gethostbyname/getservbyname/…) | Complete | `src/loader/tcpip.rs`, `samples/socket_test/` |

---

## Phase 5 — Multimedia (remaining)

- [ ] **MIDI playback** — device type `midi`; requires FluidSynth / SDL2_mixer or ALSA sequencer; deferred (external dependency cost)

---

## Phase 7 — Application Compatibility (remaining)

### 16-bit (NE) Compatibility

NE execution baseline complete (`ne_hello` runs end-to-end). Remaining:

- [ ] **Watcom CRT NE apps** — Watcom 16-bit CRT requires LDT-based selectors (TI=1); would need stub LDT or full LDT emulation
- [ ] **Mode switching** — 16-bit NE code calling a 32-bit flat DLL
- [ ] **Broader 16-bit API coverage** — more DOSCALLS / VIOCALLS / KBDCALLS ordinals beyond minimal hello-world

### PM Rendering Quality

- [ ] **Text controls: remaining editing features** — selection (Shift+arrows, mouse drag), clipboard, word wrap in `WC_MLE`, caret blink, PageUp/PageDown, Tab focus traversal, `EN_SETFOCUS` / `EN_KILLFOCUS`, `LM_DELETEITEM` / `LM_DELETEALL`, multi-select list boxes
- [ ] **Controls inside dialogs** — `WinDlgBox` dialogs still paint and hit-test their controls with the older `#Dialog` code, so entry fields and list boxes in dialogs are not interactive
- [ ] **Controls after a window drag** — `get_abs_rect_in_frame` adds the client's x/y and the drag handler copies the frame position into the client; check whether controls paint offset after a window is moved
- [ ] **Keyboard in samples** — no sample handles `WM_CHAR` or has an accelerator table, so Esc/Enter on dialogs and Alt+F4 have no effect; add default dialog key handling and a sample that exercises it
- [ ] **Queue codepage in text paths** — `WinSetCp` stores a per-queue codepage but window text (`WinSetWindowText` / `WinQueryWindowText`), GPI text and the CJK face choice still use the process codepage; needs a thread → hmq → codepage lookup at those sites
- [ ] **Codepage 1004 and 0** — `WinSetCp` / `DosSetProcessCp` reject CP1004 (common in PM programs) and 0 ("default"); needs support in `codepage.rs`
- [ ] **Menu replacement** — `WinCreateMenu` does not attach to its parent frame, and replacing a frame's menu via `WinSetParent` + `WM_UPDATEFRAME(FCF_MENU)` is not implemented (verify the sequence against the PM reference first)
- [ ] **`WinQueryCpList`** (PMWIN 811) and a real `WinDestroyMsgQueue` (currently a stub that keeps the queue alive)

### PM Graphics Performance and Architecture

From a code review of the PM rendering path (`src/gui/sdl2_renderer.rs`, `renderer.rs`, `font.rs`, `message.rs`) on 2026-11-15. Nothing was measured: the costs below come from reading the code, so add timing first where an item says so. The steps are in suggested order; the first two groups do not depend on the GPU decision.

**Step 0 — measure**

- [ ] **Timing instrumentation** — record time per composite, per present and per loop iteration (debug log or `WARPINE_TRACE`) so the items below are backed by numbers

**Step 1 — event loop (small, independent)**

- [ ] **Block instead of poll** — `run_pm_loop` sleeps 8 ms per iteration (`PmRenderer::frame_sleep`), adding up to 8 ms latency to every draw and input event and waking ~125 times a second when idle; wait on SDL events with a timeout and wake the loop when a `GUIMessage` arrives
- [ ] **Stop polling the host clipboard** — `poll_events` calls `clipboard_text()` every iteration (a string allocation and, on X11, possibly a server round trip); read it on focus gain or when the guest asks
- [ ] **Coalesce presents** — every `PresentBuffer` (six send sites; several per paint cycle) runs a full composite; mark dirty and composite at most once per frame
- [ ] **Frame pacing** — no vsync today; presents happen on message arrival plus a 16 ms timer

**Step 2 — compositor cost (small, independent)**

- [ ] **Damage rectangles** — `WindowManager::dirty` only decides whether to skip the composite; any change refills the whole desktop and re-blits every window, including occluded ones. Track rectangles and redraw only those
- [ ] **Row-wise clipped blit** — the frame blit in `composite_and_present` tests bounds and indexes per pixel; clip once and `copy_from_slice` per row
- [ ] **Cache chrome** — title-bar and menu text are re-rasterised on every composite, and titles / menu items are cloned while the window-manager lock is held
- [ ] **Glyph cache locking** — `font::rasterize` locks a global `Mutex<HashMap>` and clones an `Arc` per glyph; use a per-thread cache or a glyph atlas
- [ ] **Keep content on resize** — `FrameBuffer::resize` reallocates and blanks the buffer, so an interactive resize flashes white until the application repaints

**Step 3 — rasterise on the API thread (largest; affects compatibility)**

- [ ] **Shared per-window surface** — GPI calls are sent as one `GUIMessage` per primitive and rasterised later on the GUI thread, so the API thread cannot read pixels back. Draw on the API thread into a shared surface and send only damage to the GUI thread
- [ ] **Unlocks** — `GpiQueryPel`, `GpiBitBlt` from a window, destination-dependent mix modes (XOR…), memory presentation spaces and bitmaps

**Step 4 — GPU compositing**

- [ ] **Compositor trait** — put compositing behind a small trait; keep the software backend for headless tests (`SDL_VIDEODRIVER=dummy`, golden signatures)
- [ ] **Intermediate step** — the PM canvas is created with `.software()`; try `.accelerated().present_vsync()` with one SDL texture per window (window moves without re-blit, partial uploads)
- [ ] **GPU backend** — one texture per window drawn as quads, scaling for HiDPI, vsync. Prefer `wgpu` (Vulkan on Linux, OpenGL fallback) over raw Vulkan via `ash`, which needs far more setup for a handful of textured quads
- [ ] **Keep GPI on the CPU** — GPI is immediate-mode with pixel-exact semantics, raster operations and readback; mapping it to GPU draw calls is a large effort with poor returns

**Step 5 — only if profiling still shows text as significant**

- [ ] **Glyph atlas** shared by chrome, controls and GPI text

### PM Advanced Controls

- [ ] **`WC_CONTAINER`** — Icon / Name / Text / Detail / Tree views; record management
- [ ] **`WC_NOTEBOOK`** — tabbed property sheet
- [ ] **Drag and drop** — `DrgDrag`, `DrgAccessDraginfo`, `DM_DRAGOVER` / `DM_DROP`
- [ ] **Custom cursors** — `WinSetPointer` via `SDL_CreateColorCursor`
- [ ] **Printing** — `DevOpenDC`, `DevCloseDC`, basic spool API stubs

### REXX Interpreter Bridge

- [ ] Bridge `REXXAPI.DLL` exports (`RexxStart`, `RexxRegisterSubcomDll`, `RexxVariablePool`) to [Regina REXX](http://regina-rexx.sourceforge.net/)
- [ ] Unlocks: OS/2 install programs, system tools, 4OS2 `.cmd` scripts

### Year 2038

- [ ] Audit `time_t` usage in DOSCALLS and CRT shim functions
- [ ] `DosGetDateTime` / `DosSetDateTime` use `DATETIME` (`USHORT` year) — verify not affected
- [ ] `FILESTATUS3` timestamps use `FDATE`/`FTIME` (7-bit year from 1980, max 2107) — verify not affected
- [ ] Redirect CRT time imports (`CLIB.DLL` / `CRTL.DLL` / `EMX.DLL`) to 64-bit-clean host implementations
- [ ] Optional: `WARPINE.DLL` escape — `WrpGetDateTime64` / `WrpTime64` for recompilable apps

---

## Phase 8 — SOM / Workplace Shell (long-term)

The Workplace Shell (WPS) is built entirely on IBM's System Object Model (SOM).
Multi-year effort; depends on Phase 7 PM completion.

### SOM Runtime Core (prerequisite for WPS)

- [ ] Object / class model: SOM class objects, method table dispatch, offset-based and name-lookup dispatch
- [ ] `SOMClassMgrObject` — global class manager; `SOMClassMgr_somFindClass()`, class registration, DLL-based class loading
- [ ] IDL metadata: parse or reconstruct method offsets and class hierarchy at runtime
- [ ] Binary ABI compatibility with IBM SOM 2.1 (for XWorkplace, Object Desktop)

### WPS Object Hierarchy (requires SOM runtime)

- [ ] `WPObject` — root: `wpInitData`, `wpSaveState`, `wpRestoreState`, `wpQueryTitle`, `wpOpen`, `wpDragOver`, `wpDrop`
- [ ] `WPFileSystem` — `wpQueryFilename`, `wpQueryAttr`
- [ ] `WPFolder` — Icon / Detail / Tree via `WC_CONTAINER`; `wpPopulate`
- [ ] `WPDesktop` — singleton root desktop; persists object positions in OS2.INI
- [ ] `WPProgram` — launches via `DosExecPgm`; `WPDataFile` — `.TYPE` EA for app association
- [ ] Persistence via `PrfWriteProfileData` / `PrfQueryProfileData` (OS2.INI / OS2SYS.INI)
- [ ] Settings notebook: `WinLoadDlg` + `WC_NOTEBOOK` + per-class property pages
- [ ] Drag and drop: `wpDragOver` / `wpDrop` / `wpCopyObject` / `wpMoveObject`

---

## Phase 9 — XE: 64-bit OS/2-lineage Platform (far future / vision)

Define and implement a new 64-bit executable format and API set as a natural evolution
of the OS/2 lineage. XE apps run natively on Warpine alongside existing 32-bit LX apps.

### XE Executable Format

- [ ] Define spec: XE header (`"XE"` signature, `cpu_type`, `object_count`, `entry_rip: u64`), 64-bit object table, fixup records, import/export tables
- [ ] `src/xe/` parser module mirroring `src/lx/`
- [ ] `detect_format()` in `main.rs` recognises `"XE"` signature
- [ ] `Loader::load_xe()` / `run_xe()` path

### KVM Long Mode Execution

- [ ] vCPU initialisation in long mode (`EFER.LME`, 4-level paging, 64-bit GDT with FS/GS for TIB/PIB)
- [ ] 64-bit `SharedState` TIB/PIB layout
- [ ] INT 3 thunk mechanism in long mode; args from `rdi/rsi/rdx/rcx/r8/r9` (System V AMD64 ABI)

### 64-bit API Set (`DOSCALLS64`, `PMWIN64`, …)

- [ ] Core I/O: `DosWrite64`, `DosRead64`, `DosOpen64`, `DosClose64`, `DosExit64`
- [ ] Memory: `DosAllocMem64` (full 64-bit address space), `DosFreeMem64`
- [ ] Threads: `DosCreateThread64`, `DosWaitThread64`
- [ ] Synchronisation: `DosCreateEventSem64`, `DosCreateMutexSem64`
- [ ] PM: `WinInitialize64`, `WinCreateStdWindow64`, `WinGetMsg64`, `WinDispatchMsg64`
- [ ] `UCONV64.DLL` — Unicode conversion using UTF-8 natively

### Toolchain Support

- [ ] `warpine-xe` Rust crate: safe bindings to the 64-bit API; `#![no_std]` compatible
- [ ] Custom Rust target `x86_64-warpine-xe` (bare-metal, System V ABI, XE output via linker script)
- [ ] Sample XE app in Rust: `samples/xe_hello/`
- [ ] Sample XE app in C (Clang `x86_64-unknown-none`)

### Dual-ABI Coexistence

- [ ] 32-bit LX and 64-bit XE run side-by-side under the same Warpine instance
- [ ] `DosExecPgm` detects XE format and spawns a 64-bit vCPU thread
- [ ] Shared `SharedState` managers serve both 32-bit and 64-bit guests
