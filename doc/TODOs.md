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
| PM Menu System (MENUTEMPLATE parser, WinLoadMenu, WinSetMenu, WinCreateMenu) | Complete | `src/loader/pm_win.rs` |
| PM Dialog System (DLGTEMPLATE parser, WinDlgBox, WinLoadDlg, WinProcessDlg, WinDismissDlg, WinDefDlgProc, WinSendDlgItemMsg, DlgRunLoop) | Complete | `src/loader/pm_win.rs`, `src/loader/vcpu.rs` |

---

## Architecture Backlog

---

## Virtual Desktop Renderer (VDR) — Architecture Refactor

The current renderer maps each PM frame window to a separate SDL2 OS window.
This makes Z-ordering, modal dialogs, window decorations, and focus management
painful because they require co-ordinating multiple OS windows.

The VDR replaces this with a single host window acting as the PM desktop
surface. All PM frame windows are composited as clipping regions within it —
the same model used by WINE's `--virtual-desktop` option.

### Design Principles

- One SDL2/host window = the entire PM screen (default 1024×768, configurable).
- `WindowManager` owns the Z-order stack, focus state, and dirty-rect list.
- All rendering messages carry desktop-absolute coordinates computed from the
  window's position in the PM hierarchy.
- Input events (mouse, keyboard) are routed by Warpine, not by the OS.
- Window chrome (title bar, border, resize handles) is drawn by the compositor,
  not delegated to SDL2.

---

### VDR-A: Single-Surface Rendering Model

The largest structural change: remove per-PM-window SDL2 windows.

- [x] **VDR-A1 — Single desktop SDL2 window**: create one SDL2 window at startup
  (size from `WARPINE_DESKTOP_W` / `WARPINE_DESKTOP_H` env vars, default 1024×768).
  Replace `HashMap<u32, WindowData>` in `Sdl2Renderer` with a single `DesktopCanvas`.
- [x] **VDR-A2 — `GUIMessage::CreateWindow` → FrameBuffer entry**: stop creating an SDL2
  window per PM frame; instead allocate a `FrameBuffer` (off-screen pixel buffer) keyed
  by PM handle in `Sdl2Renderer::frame_buffers`. `CreateWindow` → `FrameBuffer::new`.
- [x] **VDR-A3 — Coordinate offset handled at composite time**: draw calls write
  frame-local coordinates to the `FrameBuffer`; the compositor in `PresentBuffer` blits
  each frame at its OS/2 screen position (`win.x, win.y`) with the y-flip applied:
  `dst_top_y = desktop_height - win.y - win.cy`.
- [x] **VDR-A4 — Full-desktop PresentBuffer**: `GUIMessage::PresentBuffer` composites all
  visible frames (bottom-to-top z-order) onto the single desktop surface and presents.
  Background filled with `DESKTOP_BG` (OS/2-style teal `0x00408040`).
- [ ] **VDR-A5 — Dialog windows composited in-surface**: `create_dialog_from_template`
  no longer emits `CreateWindow` + `ResizeWindow`; dialogs are Z-stack entries just
  like frames. Remove the `ResizeWindow` dialog hack added in the current fix.

---

### VDR-B: Z-Order and Window Lifecycle

- [x] **VDR-B1 — `WindowManager::z_order: Vec<u32>`**: ordered back-to-front list of
  visible frame HWNDs. `WinCreateStdWindow` calls `z_push_top`; `WinDestroyWindow` calls
  `z_remove`. Helpers: `z_push_top/bottom`, `z_insert_behind`, `z_hit_test`, `z_remove`.
  6 unit tests in `pm_types::tests`.
- [x] **VDR-B2 — `WinSetWindowPos` SWP_ZORDER**: `HWND_TOP`/`HWND_FLOAT` → `z_push_top`;
  `HWND_BOTTOM` → `z_push_bottom`; arbitrary `hwnd_behind` → `z_insert_behind`;
  `SWP_ACTIVATE` → `z_push_top` + `focused_hwnd` update. Constants in `constants.rs`.
- [x] **VDR-B3 — Full-frame compositor loop**: 60Hz periodic `composite_and_present`
  call in `poll_events` (`last_composite: Instant` guards the 16ms interval);
  drag animations and focus-change repaints no longer require an app `PresentBuffer`.
- [ ] **VDR-B4 — Dirty-rect tracking** (optimisation, can be deferred): add
  `WindowManager::dirty: HashSet<u32>` — only repaint frames marked dirty. Mark dirty
  on `WinInvalidateRect`, `WinShowWindow`, move/resize, Z-order change.

---

### VDR-C: Window Decorations

Currently app windows have no title bar or chrome (SDL2 provides it via the OS).
After VDR-A those decorations must be drawn by Warpine.

- [x] **VDR-C1 — Title bar rendering**: for frames with `FCF_TITLEBAR` draw a 20-px
  navy bar (active) / dark-gray bar (inactive) at the top of the frame in the desktop
  buffer; white title text at x+4; close (×) button right-aligned. `flCreateFlags`
  stored as `OS2Window::frame_flags`; FCF_* constants added to `constants.rs`.
- [x] **VDR-C2 — Frame border**: 2-px gray border overlay in the compositor for frames
  with `FCF_BORDER`, `FCF_DLGBORDER`, or `FCF_SIZEBORDER`.
- [ ] **VDR-C3 — System menu icon**: small OS/2 "warp" glyph in the title bar left
  corner for `FCF_SYSMENU`; clicking posts `WM_SYSCOMMAND(SC_CLOSE)`.
- [ ] **VDR-C4 — Minimize / maximize buttons**: right side of title bar; clicking
  posts `WM_SYSCOMMAND(SC_MINIMIZE / SC_MAXIMIZE)` to the frame.
- [ ] **VDR-C5 — Resize handles**: 4-px corner/edge grabs; drag generates
  `WM_WINDOWPOSCHANGED` with new cx/cy via `WinSetWindowPos`.

---

### VDR-D: Input Routing

All input currently relies on SDL2 routing events to the correct OS window.
After VDR-A there is only one OS window, so Warpine must route events itself.

- [x] **VDR-D1 — Mouse hit-testing**: on `SDL_MouseButtonDown` / `SDL_MouseMotion`,
  `z_hit_test(x, os2_y)` walks `z_order` front-to-back, returns the topmost frame
  whose rect contains the event. Local coords computed as `(x - win.x, os2_y - win.y)`;
  `push_msg` called with local coordinates as before.
- [x] **VDR-D2 — Title bar / chrome hit-testing**: before forwarding to the PM window,
  check if the click is in the title bar region (OS/2 y >= win.y + win.cy - 20);
  close button (×) sends WM_CLOSE; other title bar clicks activate-only (no forwarding
  to app). `ChromeHit` enum dispatches in `handle_sdl_event`.
- [x] **VDR-D3 — Window dragging**: `DragState { hwnd, anchor_x, anchor_y_sdl }` in
  `Sdl2Renderer`; `ChromeHit::TitleBar` starts drag; `MouseMotion` updates
  `win.x/win.y` and syncs client window; `MouseButtonUp` ends drag; compositor loop
  (VDR-B3) repaints each frame during drag.
- [ ] **VDR-D4 — Window activation on click**: clicking any non-focused frame brings
  it to the top (`z_order` splice) and posts `WM_ACTIVATE(WA_CLICK, hwnd)` to the
  newly active window and `WM_ACTIVATE(WA_INACTIVE, hwnd)` to the previous.
- [ ] **VDR-D5 — Keyboard routing**: all `SDL_KeyDown` / `SDL_KeyUp` / `SDL_TextInput`
  events go to `focused_hwnd`'s HMQ via `push_msg`. `focused_hwnd` tracks the PM
  window that received the last `WM_ACTIVATE`.
- [ ] **VDR-D6 — `WinSetCapture` / `capture_hwnd`**: if set, all mouse events go to
  the captured window regardless of position. Already tracked in `WindowManager`;
  wire into the event router.

---

### VDR-E: Focus Management

- [x] **VDR-E1 — `WindowManager::focused_hwnd: u32`**: the currently active frame
  HWND (0 = none). Updated by `WinCreateStdWindow`, `WinSetWindowPos(SWP_ACTIVATE)`,
  `WinSetActiveWindow`, and `MouseButtonDown` in `Sdl2Renderer` (VDR-D4).
- [x] **VDR-E2 — `WinSetActiveWindow` (ord 795) / `WinQueryActiveWindow` (ord 797)**:
  set / query `focused_hwnd`; `WinSetActiveWindow` also calls `z_push_top`.
- [x] **VDR-E3 — Title bar highlight**: the focused window's title bar is navy; all
  others are dark gray (standard OS/2 PM visual behaviour).

---

### VDR-F: winit + pixels Migration (optional follow-on)

After VDR-A–E, SDL2 is used only for: one window, pixel buffer upload, event loop,
and clipboard. Replace with the lighter Rust-native stack:

- [ ] **VDR-F1 — Add `winit` + `pixels` dependencies**; gate behind a
  `--features winit-renderer` Cargo feature.
- [ ] **VDR-F2 — `WinitRenderer` struct** implementing `PmRenderer` trait; mirrors
  `Sdl2Renderer` but uses `winit::EventLoop` + `pixels::Pixels`.
- [ ] **VDR-F3 — Event translation**: `winit::event::WindowEvent` → existing
  `push_msg` calls (keyboard, mouse, resize, close).
- [ ] **VDR-F4 — Clipboard via `arboard` crate** (replaces `sdl2::clipboard`).
- [ ] **VDR-F5 — Remove `libsdl2-dev` system dependency** once `winit` renderer
  passes all integration tests.

---

### VDR Implementation Order

Suggested sequence to keep the codebase working at each step:

1. VDR-B1 (z_order), VDR-E1 (focused_hwnd) — data model only, no rendering change.
2. VDR-A2 (stop creating SDL2 windows for dialogs; already partially done).
3. VDR-D1 + VDR-D4 (hit-test + activation) — prerequisite for correct input.
4. VDR-A1 + VDR-A3 + VDR-A4 (single surface, coordinate offset) — big change; do
   behind a `WARPINE_VDR=1` env var until stable.
5. VDR-C1–C3 (basic chrome: title bar, border, sys-menu).
6. VDR-B2–B3 (Z-order, compositor loop).
7. VDR-A5 (dialogs fully in-surface).
8. VDR-D2–D6 (full chrome interaction, drag, keyboard routing).
9. VDR-C4–C5, VDR-E2–E3 (min/max, resize, active-title highlight).
10. VDR-F1–F5 (winit migration, optional).

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

### PM Advanced Controls

- [ ] **`WC_CONTAINER`** — Icon / Name / Text / Detail / Tree views; record management
- [ ] **`WC_NOTEBOOK`** — tabbed property sheet
- [ ] **Drag and drop** — `DrgDrag`, `DrgAccessDraginfo`, `DM_DRAGOVER` / `DM_DROP`
- [ ] **Custom cursors** — `WinSetPointer` via `SDL_CreateColorCursor`
- [ ] **Printing** — `DevOpenDC`, `DevCloseDC`, basic spool API stubs

### TCP/IP Socket API

- [ ] `SO32DLL.DLL` / `TCP32DLL.DLL` thunks: `socket`, `bind`, `connect`, `listen`, `accept`, `send`, `recv`, `select`, `gethostbyname`, `getservbyname`, `setsockopt`, `getsockopt`, `closesocket`
- [ ] Map to Linux BSD socket syscalls; OS/2 `SOCE*` → errno mapping
- [ ] Enables: WebExplorer, Netscape for OS/2, FTP/IRC clients

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
