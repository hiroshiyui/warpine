# Warpine TODO List

This document tracks **open work only**. Completed items are documented in the
[Developer Guide](developer_guide.md) and [Reference Manual](reference_manual.md).

## Engineering Policy

**Near-clean-room, blackbox implementation.** All API behaviour is derived from
public documentation only (IBM *Control Program Programming Reference*, OS/2 Warp 4
Toolkit headers, IBM Developer Connection, ReactOS/osFree reference). No IBM-proprietary
DLL binaries, no ROM dumps, and no disassembly of original OS/2 system libraries.

Open work is grouped by topic:

- [Presentation Manager](#presentation-manager) — controls, dialogs and menus, input, codepages in PM, graphics, rendering
- [Multimedia](#multimedia) — audio
- [Console and text mode](#console-and-text-mode)
- [Codepages and NLS](#codepages-and-nls)
- [Application compatibility](#application-compatibility) — 16-bit (NE) programs, REXX, dates after 2038
- [Development toolchain](#development-toolchain) — modern guest compilers next to Open Watcom
- [Long-term](#long-term) — SOM / Workplace Shell, XE (64-bit)

---

## Completed (summary)

| Category | Area | Status | Reference |
|----------|------|--------|-----------|
| Core | LX/NE loader, KVM vCPU, GDT/IDT | Complete | Developer Guide §3–6 |
| Core | DOSCALLS core (file I/O, memory, threads, semaphores, IPC) | Complete | Reference Manual §9 |
| Core | NE (16-bit OS/2 1.x) execution | Complete | Developer Guide §7 |
| Core | DLL loader (recursive, ref-counted, INITTERM, builtin modules) | Complete | Developer Guide §20 |
| Core | Structured Exception Handling (SEH) — DosSetExceptionHandler, DosRaiseException, DosUnwindException | Complete | Developer Guide (SEH section) |
| Console | VIO/KBD console subsystem | Complete | Developer Guide §15 |
| Console | VGA text renderer (SDL) + GNU Unifont (SBCS + DBCS 16×16) | Complete | Developer Guide §15 |
| Console | Builtin CMD.EXE host Rust shell (core built-ins + .CMD scripts) | Complete | `src/loader/cmd.rs` |
| Console | CMD.EXE I/O redirection (`>`, `>>`, `<`) + pipe (`\|`) + sample script | Complete | `src/loader/cmd.rs`, `samples/cmd_test/test.cmd` |
| Presentation Manager | SDL2 → SDL3 migration — safe sdl3-rs API only (`src/gui`, `mmpm.rs` forbid `unsafe`), main-thread audio service | Complete | Developer Guide, "Rendering Pipeline" and MMPM/2 |
| Presentation Manager | PM window management + built-in controls | Complete | Developer Guide §11 |
| Presentation Manager | GPI drawing primitives (20+ ordinals) | Complete | Developer Guide §12 |
| Presentation Manager | PM Menu System (MENUTEMPLATE parser, WinLoadMenu, WinCreateMenu, FCF_MENU) | Complete | `src/loader/pm_win.rs` |
| Presentation Manager | PM Dialog System (DLGTEMPLATE parser, WinDlgBox, WinLoadDlg, WinProcessDlg, WinDismissDlg, WinDefDlgProc, WinSendDlgItemMsg, DlgRunLoop) | Complete | `src/loader/pm_win.rs`, `src/loader/vcpu.rs` |
| Presentation Manager | PM typography — IBM Plex Sans/Mono/Serif + Plex Sans JP/KR/TC/SC embedded; `GpiCreateLogFont` face/style/size mapping; real proportional font metrics | Complete | `src/gui/font.rs`, `vendor/ibm-plex`, Developer Guide (PM Typography) |
| Presentation Manager | PM menu bar as frame chrome — `FCF_MENU` auto-load, client area below the bar, compositor-drawn bar and pull-down | Complete | `src/loader/pm_types.rs`, `src/gui/sdl_renderer.rs` |
| Presentation Manager | Bevelled window chrome — raised title-bar buttons with pictograms, bevelled sizing border | Complete | `src/gui/sdl_renderer.rs` |
| Presentation Manager | Type-ahead — one system input queue; keys routed to the focus window when `WinGetMsg` / `WinPeekMsg` / the modal loop reads them, after posted messages | Complete | `src/loader/pm_input.rs`, `samples/pm_typeahead_test/` |
| Presentation Manager | Frame minimize / maximize / restore, `WM_MINMAXFRAME`, Alt+F5/F9/F10, title-bar and system-menu double clicks | Complete | `src/loader/pm_minmax.rs`, `samples/pm_minmax_test/` |
| Presentation Manager | `WM_DESTROY` on window destruction (window before its children) | Complete | `src/loader/pm_destroy.rs`, `samples/pm_destroy_test/` |
| Presentation Manager | Check boxes and radio buttons (`BM_SETCHECK` / `BM_QUERYCHECK` / `BM_QUERYCHECKINDEX`, auto styles, arrow keys) | Complete | `src/loader/pm_button.rs`, `samples/pm_button_test/` |
| Presentation Manager | `WinGetLastError` with per-thread `PMERR_*` codes | Complete | `src/loader/pm_error.rs`, `samples/pm_error_test/` |
| Presentation Manager | Message-box icons, Help button, Close → `MBID_CANCEL` | Complete | `src/loader/pm_msgbox.rs`, `samples/pm_msgbox_test/` |
| Presentation Manager | Mouse pointers — `WinQuerySysPointer`, `WinSetPointer` / `WinQueryPointer`, `WinShowPointer`, `WinLoadPointer` (`RT_POINTER`), `WinDestroyPointer` | Complete | `src/loader/pm_pointer.rs`, `src/gui/pointer.rs`, `samples/pm_pointer_test/` |
| Presentation Manager | `BM_CLICK`, `BM_SETHILITE` / `BM_QUERYHILITE`, `BM_SETDEFAULT`, list-box focus mark | Complete | `src/loader/pm_button.rs`, `samples/pm_button_test/` |
| Presentation Manager | Hide button, activation on minimize / hide, `WM_SIZE` on `SWP_SIZE`, `WinQueryWindowPos` state flags | Complete | `src/loader/pm_minmax.rs`, `samples/pm_minmax_test/` |
| Presentation Manager | `PMERR_*` from the window, window-word, message and dialog-item calls | Complete | `src/loader/pm_error.rs`, `samples/pm_error_test/` |
| Presentation Manager | `SV_CURSORRATE` / `WinSetSysValue`, IME area at the caret, drag scrolling past the edge, caret measured in the queue's CJK face | Complete | `src/loader/pm_controls.rs`, `src/loader/pm_edit.rs` |
| Presentation Manager | `WinIsWindowVisible`, `WinQueryWindowTextLength`, `WinSetParent`, `WinSetOwner`, `WinQueryWindowPtr` / `WinSetWindowPtr` | Complete | `src/loader/pm_window.rs`, `samples/pm_window_test/` |
| Presentation Manager | Virtual Desktop Renderer (VDR) — single-surface compositor, full window chrome, input routing, dirty-rect tracking (A1–A5/B1–B4/C1–C5/D1–D6/E1–E3) | Complete | `src/gui/sdl_renderer.rs`, `src/loader/pm_types.rs` |
| Multimedia | MMPM/2 audio (DosBeep, waveaudio MCI) | Complete | Developer Guide §10 |
| Codepages and NLS | DBCS full support (B1–B8: cell annotation, render, encode, keyboard) | Complete | Developer Guide §15 |
| Codepages and NLS | Unicode-internal architecture (codepage↔UTF-8 at all boundaries) | Complete | Developer Guide §16 |
| Codepages and NLS | UCONV.DLL emulation | Complete | Reference Manual §9 |
| Codepages and NLS | DosMapCase / NlsMapCase — full SBCS + DBCS + CP866 | Complete | Developer Guide §16 |
| Networking | TCP/IP Socket API — SO32DLL/TCP32DLL (51 thunks: socket/bind/connect/listen/accept/send/recv/select/setsockopt/getsockopt/gethostbyname/getservbyname/…) | Complete | `src/loader/tcpip.rs`, `samples/socket_test/` |
| Developer tools | Developer tooling (crash dump, GDB stub, API ring buffer) | Complete | Developer Guide §19 |
| Developer tools | Rust Guest Toolchain | Complete | Targets, lx-link linker, warpine-os2 crate family, rust_hello sample, test_rust_hello |
| Developer tools | Ordinal Table Canonical Build Tool | Complete | `src/bin/gen_api.rs`; `targets/os2api.def` is single source of truth |
| Project | Development history (the baseline phases 1–7) | Complete | [Developer Guide §20](developer_guide.md#appendix-development-phases) |

---

## Presentation Manager

Window management, messages, GPI drawing, built-in controls, menus, dialogs and message boxes work (see the "Completed" table and the developer guide). Open work:

### Controls and text editing

- [ ] **Text controls: unverified details** — the drag-scroll rate past the edge (`DRAG_SCROLL_MS`, 100 ms; no source gives one for text controls), whether `SV_CURSORRATE` is the time between toggles (as here) or a whole blink cycle, the input-method area spanning the caret's line across the control; `WinSetSysValue` keeps only `SV_CURSORRATE` (other indices fail here, while PM accepts its settable values); whether a list box keeps Enter after sending `LN_ENTER` (it passes it on to the owner here, so a dialog's default button still works), what `MLM_SETWRAP` returns (TRUE here), the Space / Ctrl+Space rules of `LS_EXTENDEDSEL` (Space toggles the cursor item here), the mp2 of the `MLN_*` notifications (the control's handle for `MLN_SETFOCUS` / `MLN_KILLFOCUS`, 0 for `MLN_CHANGE` here), what `LM_DELETEITEM` returns for an index out of range (the unchanged count here), and whether PM entry fields also take Ctrl+C / Ctrl+X / Ctrl+V (only the CUA keys Ctrl+Insert / Shift+Delete / Shift+Insert are handled)
- [ ] **`WC_CONTAINER`** — Icon / Name / Text / Detail / Tree views; record management
- [ ] **`WC_NOTEBOOK`** — tabbed property sheet
- [ ] **Drag and drop** — `DrgDrag`, `DrgAccessDraginfo`, `DM_DRAGOVER` / `DM_DROP`

### Dialogs, menus and message boxes

- [ ] **Dialog and button follow-ups** — `WM_MATCHMNEMONIC` / `WM_QUERYDLGCODE` so guest controls can take part in mnemonics and Tab, `BS_USERBUTTON` / `BN_PAINT` (a user button only stores its highlight), `WM_ACTIVATE` when the active window is destroyed, `WinProcessDlg` leaving its dialog to the caller (as now) to be checked; unverified for buttons (`pm_button.rs`): what the down-click `BM_CLICK(FALSE)` does (highlight only here), `BM_CLICK` on a disabled button (nothing here), how a highlighted check box or radio button looks (shaded indicator here), whether `BM_SETDEFAULT` changes the `BS_DEFAULT` style bit (it does here), `BM_SETCHECK` with mp1 > 2, the auto three-state order 0 → 1 → 2 → 0, `BN_CLICKED` for an arrow-key selection, a radio group with nothing checked having no tab stop; unverified for `WM_DESTROY` (`pm_destroy.rs`, following the `WinDestroyWindow` reference over the PM Guide's opposite order): owned windows destroyed before the frame's children, the focus leaving a desktop-level frame for no window; unverified against the PM reference: the `CMDSRC_*` code of the `WM_COMMAND(DID_CANCEL)` that Esc posts (`CMDSRC_OTHER` here), and the frame control data of a template built by IBM's resource compiler (Open Watcom `wrc` writes one ULONG of `FCF_*` flags), whether clicking a push button by its mnemonic moves the focus (it does not here), and the bounds of the `msgFirst > msgLast` exclusion of `WinGetMsg` / `WinPeekMsg`
- [ ] **Menus in dialogs and other frame controls** — `WM_UPDATEFRAME(FCF_MENU)` attaches a menu to a standard frame only; a dialog (PM Guide: `WinLoadMenu` then `WM_UPDATEFRAME` to the dialog) goes through `WinDefDlgProc`, which ignores it, and other `FCF_*` flags are ignored; `WinSetParent` / `WinSetOwner` of a menu window are not implemented
- [ ] **Message box follow-ups** — `MB_CUSTOMICON` / `WinMessageBox2`, the `HK_HELP` hook (`idWindow`), `MB_NONMODAL`, `MB_SYSTEMMODAL`, `MB_MOVEABLE`; unverified: whether Esc ends a box without a Cancel button (here it does nothing; Close always cancels, as documented), that the box stays open after Help, and PM's layout and icon pictures (Warpine's own; centred on the desktop here)

### Keyboard, pointer and messages

- [ ] **Frame and system-key follow-ups** — Alt+F4/F5/F9/F10, the min/max/restore and hide buttons and title-bar / system-menu double clicks work (`pm_minmax.rs`); still missing: Alt+F7 / Alt+F8 (keyboard move and size, `WM_TRACKFRAME`), F10 / Alt for the menu bar (`SC_APPMENU`, `MM_STARTMENUMODE`), Shift+Esc / Alt+Space and the system menu itself (`SC_SYSMENU`, `FID_SYSMENU`; a click on the icon opens nothing), F1 → `WM_HELP`, real icons for minimized frames (`FCF_ICON`, `WinSetIcon`), min/max for dialogs, `WinSetActiveWindow` accepting minimized or disabled frames; the keys come from the OS/2 1.2 Quick Reference card (no public source maps them to `SC_*` codes); unverified: the `WM_SYSCOMMAND` mp2 of chrome actions, the `SWP` passed with `WM_MINMAXFRAME` (`SWP_MOVE | SWP_SIZE` added; changes the client makes are ignored), restoring a frame that was maximized then minimized to its normal size, the minimized frame's look (a 160 × 22 title bar in a bottom-left icon area), activation moving to the next frame when the active one is minimized or hidden (PM documents it only for a destroyed window), `SWP_ACTIVATE` / `SWP_DEACTIVATE` reported for frames only, `WM_SIZE` posted rather than sent and not at all for an unchanged size, the hide button taking the minimize button's place when a frame asks for both, `SC_HIDE` hiding frames without a hide button
- [ ] **Input follow-ups** — the system input queue (`pm_input.rs`) routes keys when they are read, but: activation by a click and a click beside the focused control still change the focus on the GUI thread when the click arrives (keys typed before it are pinned to their old target first, so they are not misrouted, but a click is not yet processed strictly after the input before it); pointer messages are hit-tested on arrival (unverified: whether PM picks their window at read time instead, e.g. after a window has moved); `WinQueryQueueStatus` (825, `QS_KEY` / `QS_MOUSE`…) does not exist yet: the PM Reference pages found so far list the `QS_*` flags and an "added since the last call" subset but not which word of the result holds which, nor whether `WinGetMsg` / `WinPeekMsg` reset that subset — find that before implementing it
- [ ] **Pointer follow-ups** — system pointers, `WinSetPointer`, `WinShowPointer` and `WinLoadPointer` work (`pm_pointer.rs`); missing: the I-beam over entry fields and MLEs (the MLE's `WM_MOUSEMOVE` page documents it, but the renderer sends `WM_MOUSEMOVE` to a control only while button 1 holds it), sizing arrows over frame borders, `WM_CONTROLPOINTER`, `WinCreatePointer` / `WinCreatePointerIndirect`, `WinQueryPointerInfo`, `WinDrawPointer`, `WinSetPointerPos` / `WinQueryPointerPos`; unverified: NULLHANDLE accepted as the desktop window, the arrow as the initial pointer and after the current pointer is destroyed, "invert screen" pels drawn black, bitmap-array selection ignoring the display size, pen and object pointers shown as the arrow
- [ ] **PM error follow-ups** — `WinGetLastError` reports per-thread `PMERR_*` codes from the queue, window, window-word, message and dialog-item calls (`pm_error.rs`); still missing: `PMERR_INVALID_FLAG` for `WinInvalidateRect` / `WinQueryWindowPos` (which bit is not documented), `PMERR_CANNOT_SET_FOCUS`, `PMERR_INV_HPS` (`WinBeginPaint`), `WinCreateWindow`'s `PMERR_NO_MSG_QUEUE` / `PMERR_INVALID_PARAMETERS`, errors from `WinStartTimer` and from the dialog-item text calls with a missing item; `WinGetErrorInfo` / `WinFreeErrorInfo` (the `ERRINFO` message layout is not in the sources found); unverified: every code has `SEVERITY_ERROR`, a later successful call leaves the error alone, what `WinGetMsg` returns for a bad filter (FALSE here) and `WinQueryWindow` for a bad code (0), `WinSendDlgItemMsg` with a missing item and `WinSetWindowUShort` recording `PMERR_INVALID_HWND` / `PMERR_PARAMETER_OUT_OF_RANGE` (inferred), frames' window words not bounded (their size is unknown); for the window-tree calls (`pm_window.rs`): an `HWND_OBJECT` ancestor making a window not visible, `PMERR_INVALID_HWND` for a descendant as the new parent, `WinSetWindowPtr(QWP_PFNWP)` replacing the procedure like `WinSubclassWindow`, `WinSetParent` keeping the position relative to the new parent, a frame moved under another window no longer being drawn (frames are composited only on the desktop), `WinQueryWindowTextLength` not sending `WM_QUERYWINDOWPARAMS` to guest windows

### Codepages in PM

The process codepage itself is under "Codepages and NLS" below.

- [ ] **Codepage follow-ups** — menu templates with codepage 0 use the process codepage; unverified: whether CP1004 lacks Windows-1252's ƒ (0x83)
- [ ] **Codepage 0 in `WinSetCp`** — rejected; find in the PM reference whether 0 means "the process codepage" before accepting it

### Graphics (GPI and beyond)

- [ ] **Remaining GPI gaps** — custom bitmap patterns (`GpiSetPatternSet` with a bitmap or font LCID), rounded box corners (`GpiBox` `lHRound` / `lVRound`), areas and paths (`GpiBeginArea` / `GpiEndArea`, `GpiBeginPath` / `GpiStrokePath`, which `GpiSetLineWidthGeom` is for), default-attribute calls (`GpiSetDefArcParams` …), palettes (`GpiCreatePalette`, `LCOLF_PALETTE`), 16-bit bitmaps (no public layout at hand), `BCA_HUFFMAN1D` and compressed output from `GpiQueryBitmapBits` (always uncompressed), coordinate transforms (presentation spaces are always `PU_PELS`), printer and metafile DCs (`DevOpenDC` refuses all but `OD_MEMORY` / `OD_INFO`)
- [ ] **Unverified details** — the dash patterns of `LINETYPE_*` and the 8 × 8 bits of the `PATSYM_*` patterns are Warpine's own; `GpiPointArc` with collinear points draws the straight path through them; whether thick cosmetic lines should be exactly `round(width)` pels wide and how the boundary of a filled and outlined area mixes (Warpine paints each pel once) are to be checked against the PM reference; whether the target of `GpiWCBitBlt` / `GpiDrawBits` is inclusive (as implemented) should be checked against the PM reference
- [ ] **Printing** — `DevOpenDC`, `DevCloseDC`, basic spool API stubs
- [ ] **DIVE (direct framebuffer blitting)** — the API most OS/2 games and video players use instead of GPI: the application fills a buffer and asks for it to be blitted (with scaling and colour conversion) into a window. Maps directly onto the shared per-window surfaces (see "Rendering and compositing" below). The ordinal table (`doc/os2_ordinals.md`) has no DIVE section, so first find the export list and semantics in public headers / import libraries (clean-room policy); pairs with DART audio in MMPM/2
- [ ] **OpenGL forwarding (`OPENGL.DLL`)** — OS/2 Warp 4 shipped OpenGL 1.0 (later a 1.1 update) with GLU and a small PM binding, PGL (`pglChooseConfig`, `pglCreateContext`, `pglMakeCurrent`, `pglSwapBuffers`…); version details are from memory and need checking against public documentation. GL 1.x calls map one-to-one onto the host's OpenGL compatibility profile, so most thunks only pass arguments and can be generated from a table; the real work is PGL context creation / buffer swap, translating guest pointers for array and pixel-data calls, and the few hundred entry points. Builds on the layered compositor: the guest context renders into an offscreen framebuffer that is composited as the window's texture (with the software compositor every frame would need a GPU readback); the SDL renderer may need to give way to SDL_GPU or a shared GL context for this. No `OPENGL` section in `doc/os2_ordinals.md` yet — find the export list first

### Rendering and compositing

From a code review of the PM rendering path (`src/gui/sdl_renderer.rs`, `renderer.rs`, `font.rs`, `message.rs`) on 2026-11-15. Steps 0–2 (frame statistics, a blocking event loop, coalesced presents, clipboard via events; damage-tracked compositing, row-wise blits, cached chrome, a per-thread glyph cache, content kept on resize), step 3 (shared per-frame surfaces drawn on the API thread, `GpiQueryPel`/`GpiSetPel`, `FM_*` mix modes, memory presentation spaces and bitmaps, `GpiBitBlt`, logical colour tables, line types, text background mix) and step 4's compositor trait with the per-window-texture layered compositor are done — see the developer guide, "Rendering Pipeline"; measure the items below with the frame statistics (`RUST_LOG=warpine::gui=debug`) before and after.

- [ ] **Measure on a real display** — the event-loop change was measured only on SDL's `dummy` driver (idle CPU roughly halved); compare idle CPU and input latency on X11 and Wayland, where the old loop also read the clipboard ~125 times a second; `samples/pm_bench` gives repeatable guest-side numbers for the comparison
- [ ] **Damage smaller than a window** — `WindowManager::dirty` holds window handles, so invalidating a small area or drawing one primitive still redraws the whole frame; record the invalidated rectangles (`WinInvalidateRect` already receives them) and damage only those plus what was drawn — surfaces already track the bounds of their drawing (`Surface::take_changes`, used by the layered compositor), but they have one consumer, so the software compositor would need its own copy
- [ ] **Tighter damage for moves** — a window dragged a short distance damages the bounding box of its old and new positions; damage the two rectangles separately (or the old one minus the new one)
- [ ] **Flicker during long paints** — every draw marks its frame dirty, so a composite can show a paint half done (e.g. just after the background fill); if visible, defer presentation of a `WinBeginPaint` presentation space to `WinEndPaint`
- [ ] **HiDPI** (deferred 2026-11-16: no HiDPI display available to verify it) — the desktop window is created without high pixel density, so on a scaled display the OS scales it; use SDL3's pixel density / logical presentation to draw frames at the display's density (fonts would need rasterising at the scale). Agree the approach (render at the display density vs. keep OS scaling) before starting
- [ ] **Glyph atlas** shared by chrome, controls and GPI text (only if profiling still shows text as significant)

---

## Multimedia

- [ ] **Audio output follow-ups** — headless runs (`WARPINE_HEADLESS=1`, or a CLI app whose stdout is not a terminal) have no main-thread loop and are silent; `MCI_RECORD` could use an SDL3 recording device; the high words of `MM_MCINOTIFY` (user parameter, message) are not filled in — check the MMPM/2 reference
- [ ] **MIDI playback** — device type `midi`; requires FluidSynth / SDL3_mixer or ALSA sequencer; deferred (external dependency cost)

---

## Console and text mode

- [ ] **`KbdCharIn` / `KbdStringIn` at end of piped input** — in headless mode with stdin not a terminal, a waiting `KbdCharIn` polls forever once the input is exhausted (`DosRead` and the built-in CMD.EXE now stop at end of input). Waiting is the API's contract, so decide what a headless run should do — e.g. end the process, or report an error code — after checking the KBD reference

---

## Codepages and NLS

Codepages of PM text (message queues, presentation spaces) are under "Presentation Manager".

- [ ] **A configurable default codepage** — the process codepage always starts as 437 (`locale.rs` `map_country` returns country 1 / CP437 because the Watcom CRT crashes on non-US country codes without NLS DLLs), so an app that relies on the machine's `CODEPAGE=` (e.g. 932) without calling `DosSetProcessCp` shows garbled text — the same failure as a non-Unicode Windows program under a different "Language for non-Unicode programs". Add a setting such as `WARPINE_CODEPAGE=932` (or take it from the host locale) for the process and new message queues, keeping the country code at 1 unless the CRT crash is understood

---

## Application compatibility

- [ ] **Ending a thread ends the process** — `DosExit(EXIT_THREAD, …)` and a thread function returning both set `exit_requested` (`api_registry.rs` ordinal 234, `EXIT_TRAP_ADDR` in `vcpu.rs`), so the whole program stops; found while writing `pm_error_test`, whose second thread idles instead of exiting
- [ ] **`thread_test` has no Makefile** — `samples/thread_test` cannot be rebuilt with `make`, so `test_thread` fails in a fresh checkout until `thread_test.exe` is built by hand with `wcl386`; add a Makefile like the other samples'

### 16-bit (NE) programs

NE execution works (`ne_hello` runs end-to-end). Open work:

- [ ] **Watcom CRT NE apps** — Watcom 16-bit CRT requires LDT-based selectors (TI=1); would need stub LDT or full LDT emulation
- [ ] **Mode switching** — 16-bit NE code calling a 32-bit flat DLL
- [ ] **Broader 16-bit API coverage** — more DOSCALLS / VIOCALLS / KBDCALLS ordinals beyond minimal hello-world

### REXX interpreter bridge

- [ ] Bridge `REXXAPI.DLL` exports (`RexxStart`, `RexxRegisterSubcomDll`, `RexxVariablePool`) to [Regina REXX](http://regina-rexx.sourceforge.net/)
- [ ] Unlocks: OS/2 install programs, system tools, 4OS2 `.cmd` scripts

### Dates after 2038

- [ ] Audit `time_t` usage in DOSCALLS and CRT shim functions
- [ ] `DosGetDateTime` / `DosSetDateTime` use `DATETIME` (`USHORT` year) — verify not affected
- [ ] `FILESTATUS3` timestamps use `FDATE`/`FTIME` (7-bit year from 1980, max 2107) — verify not affected
- [ ] Redirect CRT time imports (`CLIB.DLL` / `CRTL.DLL` / `EMX.DLL`) to 64-bit-clean host implementations
- [ ] Optional: `WARPINE.DLL` escape — `WrpGetDateTime64` / `WrpTime64` for recompilable apps

---

## Development toolchain

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

## Long-term

### SOM / Workplace Shell

The Workplace Shell (WPS) is built entirely on IBM's System Object Model (SOM).
Multi-year effort; depends on the Presentation Manager work above.

#### SOM Runtime Core (prerequisite for WPS)

- [ ] Object / class model: SOM class objects, method table dispatch, offset-based and name-lookup dispatch
- [ ] `SOMClassMgrObject` — global class manager; `SOMClassMgr_somFindClass()`, class registration, DLL-based class loading
- [ ] IDL metadata: parse or reconstruct method offsets and class hierarchy at runtime
- [ ] Binary ABI compatibility with IBM SOM 2.1 (for XWorkplace, Object Desktop)

#### WPS Object Hierarchy (requires SOM runtime)

- [ ] `WPObject` — root: `wpInitData`, `wpSaveState`, `wpRestoreState`, `wpQueryTitle`, `wpOpen`, `wpDragOver`, `wpDrop`
- [ ] `WPFileSystem` — `wpQueryFilename`, `wpQueryAttr`
- [ ] `WPFolder` — Icon / Detail / Tree via `WC_CONTAINER`; `wpPopulate`
- [ ] `WPDesktop` — singleton root desktop; persists object positions in OS2.INI
- [ ] `WPProgram` — launches via `DosExecPgm`; `WPDataFile` — `.TYPE` EA for app association
- [ ] Persistence via `PrfWriteProfileData` / `PrfQueryProfileData` (OS2.INI / OS2SYS.INI)
- [ ] Settings notebook: `WinLoadDlg` + `WC_NOTEBOOK` + per-class property pages
- [ ] Drag and drop: `wpDragOver` / `wpDrop` / `wpCopyObject` / `wpMoveObject`

### XE — a 64-bit OS/2-lineage platform (far future / vision)

Define and implement a new 64-bit executable format and API set as a natural evolution
of the OS/2 lineage. XE apps run natively on Warpine alongside existing 32-bit LX apps.

#### XE Executable Format

- [ ] Define spec: XE header (`"XE"` signature, `cpu_type`, `object_count`, `entry_rip: u64`), 64-bit object table, fixup records, import/export tables
- [ ] `src/xe/` parser module mirroring `src/lx/`
- [ ] `detect_format()` in `main.rs` recognises `"XE"` signature
- [ ] `Loader::load_xe()` / `run_xe()` path

#### KVM Long Mode Execution

- [ ] vCPU initialisation in long mode (`EFER.LME`, 4-level paging, 64-bit GDT with FS/GS for TIB/PIB)
- [ ] 64-bit `SharedState` TIB/PIB layout
- [ ] INT 3 thunk mechanism in long mode; args from `rdi/rsi/rdx/rcx/r8/r9` (System V AMD64 ABI)

#### 64-bit API Set (`DOSCALLS64`, `PMWIN64`, …)

- [ ] Core I/O: `DosWrite64`, `DosRead64`, `DosOpen64`, `DosClose64`, `DosExit64`
- [ ] Memory: `DosAllocMem64` (full 64-bit address space), `DosFreeMem64`
- [ ] Threads: `DosCreateThread64`, `DosWaitThread64`
- [ ] Synchronisation: `DosCreateEventSem64`, `DosCreateMutexSem64`
- [ ] PM: `WinInitialize64`, `WinCreateStdWindow64`, `WinGetMsg64`, `WinDispatchMsg64`
- [ ] `UCONV64.DLL` — Unicode conversion using UTF-8 natively

#### Toolchain Support

- [ ] `warpine-xe` Rust crate: safe bindings to the 64-bit API; `#![no_std]` compatible
- [ ] Custom Rust target `x86_64-warpine-xe` (bare-metal, System V ABI, XE output via linker script)
- [ ] Sample XE app in Rust: `samples/xe_hello/`
- [ ] Sample XE app in C (Clang `x86_64-unknown-none`)

#### Dual-ABI Coexistence

- [ ] 32-bit LX and 64-bit XE run side-by-side under the same Warpine instance
- [ ] `DosExecPgm` detects XE format and spawns a 64-bit vCPU thread
- [ ] Shared `SharedState` managers serve both 32-bit and 64-bit guests
