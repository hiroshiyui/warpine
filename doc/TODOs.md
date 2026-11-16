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
| VGA text renderer (SDL) + GNU Unifont (SBCS + DBCS 16×16) | Complete | Developer Guide §15 |
| SDL2 → SDL3 migration — safe sdl3-rs API only (`src/gui`, `mmpm.rs` forbid `unsafe`), main-thread audio service | Complete | Developer Guide, "Rendering Pipeline" and MMPM/2 |
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
| PM menu bar as frame chrome — `FCF_MENU` auto-load, client area below the bar, compositor-drawn bar and pull-down | Complete | `src/loader/pm_types.rs`, `src/gui/sdl_renderer.rs` |
| Bevelled window chrome — raised title-bar buttons with pictograms, bevelled sizing border | Complete | `src/gui/sdl_renderer.rs` |
| Virtual Desktop Renderer (VDR) — single-surface compositor, full window chrome, input routing, dirty-rect tracking (A1–A5/B1–B4/C1–C5/D1–D6/E1–E3) | Complete | `src/gui/sdl_renderer.rs`, `src/loader/pm_types.rs` |
| TCP/IP Socket API — SO32DLL/TCP32DLL (51 thunks: socket/bind/connect/listen/accept/send/recv/select/setsockopt/getsockopt/gethostbyname/getservbyname/…) | Complete | `src/loader/tcpip.rs`, `samples/socket_test/` |

---

## Phase 5 — Multimedia (remaining)

- [ ] **Audio output follow-ups** — headless runs (`WARPINE_HEADLESS=1`, or a CLI app whose stdout is not a terminal) have no main-thread loop and are silent; `MCI_RECORD` could use an SDL3 recording device; the high words of `MM_MCINOTIFY` (user parameter, message) are not filled in — check the MMPM/2 reference; `SDL_SetTextInputArea` (`TextInputUtil::set_rect`) could place the input-method window at the entry-field / MLE caret
- [ ] **MIDI playback** — device type `midi`; requires FluidSynth / SDL3_mixer or ALSA sequencer; deferred (external dependency cost)

---

## Phase 7 — Application Compatibility (remaining)

### Console Input

- [ ] **`KbdCharIn` / `KbdStringIn` at end of piped input** — in headless mode with stdin not a terminal, a waiting `KbdCharIn` polls forever once the input is exhausted (`DosRead` and the built-in CMD.EXE now stop at end of input). Waiting is the API's contract, so decide what a headless run should do — e.g. end the process, or report an error code — after checking the KBD reference

### 16-bit (NE) Compatibility

NE execution baseline complete (`ne_hello` runs end-to-end). Remaining:

- [ ] **Watcom CRT NE apps** — Watcom 16-bit CRT requires LDT-based selectors (TI=1); would need stub LDT or full LDT emulation
- [ ] **Mode switching** — 16-bit NE code calling a 32-bit flat DLL
- [ ] **Broader 16-bit API coverage** — more DOSCALLS / VIOCALLS / KBDCALLS ordinals beyond minimal hello-world

### PM Rendering Quality

- [ ] **Text controls: remaining editing features** — selection (Shift+arrows, mouse drag), clipboard, word wrap in `WC_MLE`, caret blink, PageUp/PageDown, Tab focus traversal, `EN_SETFOCUS` / `EN_KILLFOCUS`, `LM_DELETEITEM` / `LM_DELETEALL`, multi-select list boxes
- [ ] **Controls after a window drag** — `get_abs_rect_in_frame` adds the client's x/y and the drag handler copies the frame position into the client; check whether controls paint offset after a window is moved
- [ ] **Dialog follow-ups** — a focus mark on list boxes, radio buttons checked by the arrow keys, `WM_MATCHMNEMONIC` / `WM_QUERYDLGCODE` so guest controls can take part in mnemonics and Tab, `WM_DESTROY` sent when a window is destroyed; unverified against the PM reference: the `CMDSRC_*` code of the `WM_COMMAND(DID_CANCEL)` that Esc posts (`CMDSRC_OTHER` here), and the frame control data of a template built by IBM's resource compiler (Open Watcom `wrc` writes one ULONG of `FCF_*` flags), whether clicking a push button by its mnemonic moves the focus (it does not here), and the bounds of the `msgFirst > msgLast` exclusion of `WinGetMsg` / `WinPeekMsg`
- [ ] **System accelerators beyond Alt+F4** — Alt+F5/F7/F8/F9/F10 (restore, move, size, minimise, maximise), F10 for the menu bar, F1 for help
- [ ] **Codepage follow-ups** — chrome and control text pick their CJK face from the process codepage (`CjkRegion::Auto`), not from the window's queue; menu templates with codepage 0 use the process codepage; unverified: whether CP1004 lacks Windows-1252's ƒ (0x83)
- [ ] **Codepage 0 in `WinSetCp`** — rejected; find in the PM reference whether 0 means "the process codepage" before accepting it
- [ ] **Menu replacement** — `WinCreateMenu` does not attach to its parent frame, and replacing a frame's menu via `WinSetParent` + `WM_UPDATEFRAME(FCF_MENU)` is not implemented (verify the sequence against the PM reference first)
- [ ] **A real `WinDestroyMsgQueue`** (currently a stub that keeps the queue alive)

### PM Graphics Performance and Architecture

From a code review of the PM rendering path (`src/gui/sdl_renderer.rs`, `renderer.rs`, `font.rs`, `message.rs`) on 2026-11-15. The steps are in suggested order. Steps 0–2 (frame statistics, a blocking event loop, coalesced presents, clipboard via events; damage-tracked compositing, row-wise blits, cached chrome, a per-thread glyph cache, content kept on resize), step 3 (shared per-frame surfaces drawn on the API thread, `GpiQueryPel`/`GpiSetPel`, `FM_*` mix modes, memory presentation spaces and bitmaps, `GpiBitBlt`, logical colour tables, line types, text background mix) and step 4's compositor trait with the per-window-texture layered compositor are done — see the developer guide, "Rendering Pipeline"; measure the items below with the frame statistics (`RUST_LOG=warpine::gui=debug`) before and after.

- [ ] **Measure on a real display** — the event-loop change was measured only on SDL's `dummy` driver (idle CPU roughly halved); compare idle CPU and input latency on X11 and Wayland, where the old loop also read the clipboard ~125 times a second; `samples/pm_bench` gives repeatable guest-side numbers for the comparison

**Step 2 follow-ups — compositor**

- [ ] **Damage smaller than a window** — `WindowManager::dirty` holds window handles, so invalidating a small area or drawing one primitive still redraws the whole frame; record the invalidated rectangles (`WinInvalidateRect` already receives them) and damage only those plus what was drawn — surfaces already track the bounds of their drawing (`Surface::take_changes`, used by the layered compositor), but they have one consumer, so the software compositor would need its own copy
- [ ] **Tighter damage for moves** — a window dragged a short distance damages the bounding box of its old and new positions; damage the two rectangles separately (or the old one minus the new one)

**Step 3 follow-ups — what the shared surfaces unlock**

- [ ] **Remaining GPI gaps** — custom bitmap patterns (`GpiSetPatternSet` with a bitmap or font LCID), rounded box corners (`GpiBox` `lHRound` / `lVRound`), areas and paths (`GpiBeginArea` / `GpiEndArea`, `GpiBeginPath` / `GpiStrokePath`, which `GpiSetLineWidthGeom` is for), default-attribute calls (`GpiSetDefArcParams` …), palettes (`GpiCreatePalette`, `LCOLF_PALETTE`), 16-bit bitmaps (no public layout at hand), `BCA_HUFFMAN1D` and compressed output from `GpiQueryBitmapBits` (always uncompressed), coordinate transforms (presentation spaces are always `PU_PELS`), printer and metafile DCs (`DevOpenDC` refuses all but `OD_MEMORY` / `OD_INFO`)
- [ ] **Unverified details** — the dash patterns of `LINETYPE_*` and the 8 × 8 bits of the `PATSYM_*` patterns are Warpine's own; `GpiPointArc` with collinear points draws the straight path through them; whether thick cosmetic lines should be exactly `round(width)` pels wide and how the boundary of a filled and outlined area mixes (Warpine paints each pel once) are to be checked against the PM reference; whether the target of `GpiWCBitBlt` / `GpiDrawBits` is inclusive (as implemented) should be checked against the PM reference
- [ ] **Flicker during long paints** — every draw marks its frame dirty, so a composite can show a paint half done (e.g. just after the background fill); if visible, defer presentation of a `WinBeginPaint` presentation space to `WinEndPaint`

- [ ] **DIVE (direct framebuffer blitting)** — the API most OS/2 games and video players use instead of GPI: the application fills a buffer and asks for it to be blitted (with scaling and colour conversion) into a window. Maps directly onto the shared per-window surface above. The ordinal table (`doc/os2_ordinals.md`) has no DIVE section, so first find the export list and semantics in public headers / import libraries (clean-room policy); pairs with DART audio in MMPM/2

**Step 4 follow-ups — GPU compositing** (the `Compositor` trait, `SoftwareCompositor` and the per-window-texture `LayeredCompositor` with vsync are done — see the developer guide, "Compositing")

- [ ] **HiDPI** (deferred 2026-11-16: no HiDPI display available to verify it) — the desktop window is created without high pixel density, so on a scaled display the OS scales it; use SDL3's pixel density / logical presentation to draw frames at the display's density (fonts would need rasterising at the scale). Agree the approach (render at the display density vs. keep OS scaling) before starting

- [ ] **OpenGL forwarding (`OPENGL.DLL`)** — OS/2 Warp 4 shipped OpenGL 1.0 (later a 1.1 update) with GLU and a small PM binding, PGL (`pglChooseConfig`, `pglCreateContext`, `pglMakeCurrent`, `pglSwapBuffers`…); version details are from memory and need checking against public documentation. GL 1.x calls map one-to-one onto the host's OpenGL compatibility profile, so most thunks only pass arguments and can be generated from a table; the real work is PGL context creation / buffer swap, translating guest pointers for array and pixel-data calls, and the few hundred entry points. Builds on the layered compositor: the guest context renders into an offscreen framebuffer that is composited as the window's texture (with the software compositor every frame would need a GPU readback); the SDL renderer may need to give way to SDL_GPU or a shared GL context for this. No `OPENGL` section in `doc/os2_ordinals.md` yet — find the export list first

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

## Modern and High-Soundness Development Toolchain

Open Watcom stays the reference compiler, but it supports C89 plus part of
C99 and almost no C++11. Teaching it modern C/C++ would mean new front-end
work and Watcom-specific ABI extensions (name mangling, EH tables). Instead,
keep Open Watcom frozen as the ABI baseline and add a second, modern guest
toolchain built only on stable, open formats, so that no single compiler
project can lock Warpine in.

**Principle — the ELF/LX firewall.** Warpine's toolchain consumes only i386
ELF relocatable objects (System V i386 ABI) and produces LX. Both formats
are frozen, so any compiler that emits i386 ELF (GCC, clang, TinyCC, rustc)
is a replaceable part. `lx-link` (`src/bin/lx_link.rs`) is already this
ELF → LX bridge for Rust (`R_386_32` / `R_386_PC32`); extend it, don't
start another tool. No compiler plugins, LLVM passes, lld back ends or BFD
targets: their internal APIs carry no stability promise.

**M1 — C hello world from GCC (feasibility)**

- [ ] Build `samples/c_hello` with `gcc -m32 -ffreestanding -fno-pic -fno-stack-protector -c` and link with `lx-link`; run it in Warpine and add an integration test (same shape as `test_rust_hello`)
- [ ] Check what `lx-link` still needs for C objects: `.text.*` / `.rodata.*` / `.data.*` / `.bss` section variants, `COMMON` symbols, string-merge sections, `.init_array` (C constructors), and a clear error for PIC relocations (`R_386_GOTPC`, `R_386_GOTOFF`, `R_386_PLT32`)
- [ ] Minimal C start-up: `_start` → `main` → `DosExit`, modelled on `crates/warpine-os2-rt`

**M2 — compiler-neutral guest SDK**

- [ ] Guest headers (`os2.h` subset) that compile unchanged with GCC and clang: calling-convention and packing syntax only behind macros (`APIENTRY`, `EXPENTRY`, `_System`), using only the GCC/clang common subset; written from public documentation per the engineering policy
- [ ] Import resolution from `targets/os2api.def`, shared with the Rust toolchain
- [ ] A small freestanding C runtime (string, memory, formatted output over `DosWrite`) or a documented choice of an existing freestanding libc

**M3 — two compilers in CI, pinned and reproducible**

- [ ] Build every C guest sample with both GCC and clang (`--target=i386-unknown-none`) in CI; a test that passes with one and fails with the other flags compiler drift early
- [ ] Pin the guest compilers reproducibly (GNU Guix or Nix) next to the host pin in `rust-toolchain.toml`, so upstream breaking changes are taken on Warpine's schedule
- [ ] Keep regression binaries built by the frozen Open Watcom as the "plain OS/2 API" baseline

**M4 — debugging and C++**

- [ ] Debug information as DWARF (an open standard): `lx-link` carries it into an LX debug section or a sidecar file, readable by the GDB stub (`gdb_stub.rs`)
- [ ] Guest C++ on the Itanium C++ ABI (followed by GCC and clang): a runtime (`libsupc++` / `libc++abi` + unwinder) and how C++ exceptions coexist with OS/2 SEH (`seh.rs`)

**Ported-software test tier (bitwiseworks GCC / LIBCn)**

- [ ] Use binaries built by the OS/2-native bitwiseworks GCC toolchain (GCC + LIBCn/kLIBC + LIBCx, linked with WLINK; built inside an ArcaOS VM) as a "real ported software" regression tier. They load several runtime DLLs and lean on exception handling, signals and memory APIs much harder than Open Watcom samples. These are not IBM binaries, so the clean-room policy allows them as test inputs; check each one's licence before adding binaries to the repository
- [ ] First list which DOSCALLS ordinals LIBCn and LIBCx import (from their import tables) to prioritise API work
- [ ] Verify the current state before relying on it: GCC versions available for OS/2 (official RPMs vs. unofficial newer builds), LIBCn release status, stabs vs. DWARF debug info

**Notes**

- Rust guests depend on LLVM through `rustc`, but only through Rust's stability guarantees; `rustc_codegen_gcc` / `gccrs` are possible GCC-based alternatives to watch, not to depend on yet.
- Changing Open Watcom itself is out of scope. Any improvement belongs upstream and must keep default-flag code generation and object output byte-identical; deploying modified OW binaries carries Sybase OWPL source-release obligations.

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
