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
| Presentation Manager | VIO programs in a window on the PM desktop (`WARPINE_VIO=window|fullscreen`, LX application type) | Complete | `src/gui/vio_window.rs`, `src/loader/vio_frame.rs` |
| Presentation Manager | VIO window view, scroll bars, minimize / maximize, system menu, Mark / Copy / Copy All / Paste; full screen and Alt+Home | Complete | `src/gui/vio_window.rs`, `src/gui/vio_view.rs`, `src/gui/popup_menu.rs`, `src/gui/vio_switch.rs` |
| Presentation Manager | PM frames' system menu (icon click, Alt+Space / Shift+Esc, `SC_SYSMENU`), keyboard move / size (Alt+F7 / Alt+F8, `SC_MOVE` / `SC_SIZE`), `WinPopupMenu` | Complete | `src/gui/system_menu.rs`, `src/gui/frame_track.rs`, `src/loader/pm_sysmenu.rs`, `samples/pm_sysmenu_test/` |
| Console | MOUCALLS mouse API (20 calls) | Complete | `src/loader/moucalls.rs`, `samples/mou_test/` |
| Presentation Manager | `WinIsWindowVisible`, `WinQueryWindowTextLength`, `WinSetParent`, `WinSetOwner`, `WinQueryWindowPtr` / `WinSetWindowPtr` | Complete | `src/loader/pm_window.rs`, `samples/pm_window_test/` |
| Presentation Manager | Virtual Desktop Renderer (VDR) — single-surface compositor, full window chrome, input routing, dirty-rect tracking (A1–A5/B1–B4/C1–C5/D1–D6/E1–E3) | Complete | `src/gui/sdl_renderer.rs`, `src/loader/pm_types.rs` |
| Multimedia | MMPM/2 audio (DosBeep, waveaudio MCI) | Complete | Developer Guide §10 |
| Codepages and NLS | DBCS full support (B1–B8: cell annotation, render, encode, keyboard) | Complete | Developer Guide §15 |
| Codepages and NLS | Unicode-internal architecture (codepage↔UTF-8 at all boundaries) | Complete | Developer Guide §16 |
| Codepages and NLS | UCONV.DLL emulation | Complete | Reference Manual §9 |
| Codepages and NLS | DosMapCase — full SBCS + DBCS + CP866 | Complete | Developer Guide §16 |
| Codepages and NLS | NLS ordinals 5–8 as in `<bseord.h>` (`DosQueryCtryInfo`, `DosQueryDBCSEnv`, `DosMapCase`, `DosQueryCollate`), short buffers truncated | Complete | Developer Guide, NLS; `samples/nls_test/` |
| Console | `VioCheckCharType` at VIOCALLS 75 (`VIO16CHECKCHARTYPE`), `VioWrtCellStr`, VIO screen codepage from `DosSetProcessCp`; VIOCALLS by-name table and 4OS2 `viowrap.c` checked against `os2386.lib` | Complete | `src/loader/viocalls.rs`, `samples/dbcs_test/` |
| Codepages and NLS | Starting country and codepage from the host locale, `WARPINE_COUNTRY` / `WARPINE_CODEPAGE` (the Watcom CRT no longer hangs with non-US countries) | Complete | `src/loader/locale.rs`, Developer Guide, NLS |
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

- [ ] **Frame and system-key follow-ups** — Alt+F4/F5/F7/F8/F9/F10, Alt+Space / Shift+Esc, the system menu, the min/max/restore and hide buttons and title-bar / system-menu double clicks work (`pm_minmax.rs`, `pm_sysmenu.rs`); still missing: F10 / Alt for the menu bar (`SC_APPMENU`, `MM_STARTMENUMODE`), F1 → `WM_HELP`, real icons for minimized frames (`FCF_ICON`, `WinSetIcon`), min/max for dialogs, `WinSetActiveWindow` accepting minimized or disabled frames; the keys come from the OS/2 1.2 Quick Reference card (no public source maps them to `SC_*` codes); unverified: the `WM_SYSCOMMAND` mp2 of chrome actions, the `SWP` passed with `WM_MINMAXFRAME` (`SWP_MOVE | SWP_SIZE` added; changes the client makes are ignored), restoring a frame that was maximized then minimized to its normal size, the minimized frame's look (a 160 × 22 title bar in a bottom-left icon area), activation moving to the next frame when the active one is minimized or hidden (PM documents it only for a destroyed window), `SWP_ACTIVATE` / `SWP_DEACTIVATE` reported for frames only, `WM_SIZE` posted rather than sent and not at all for an unchanged size, the hide button taking the minimize button's place when a frame asks for both, `SC_HIDE` hiding frames without a hide button
- [ ] **System menu and pop-up menu follow-ups** — PM frames have a system menu (`gui/system_menu.rs`, drawn as a `PopupMenu`), keyboard move / size (`gui/frame_track.rs`) and `WinPopupMenu` (`pm_sysmenu.rs`); still missing: a real `FID_SYSMENU` window (`WinWindowFromID(frame, FID_SYSMENU)`, `MM_SETITEMATTR` / `MM_QUERYITEMATTR` to grey or remove items, and `SC_CLOSE` being "ignored if Close is disabled in the system menu"), `WM_TRACKFRAME` / `WM_QUERYTRACKINFO` and `WinTrackRect` for applications (the tracking runs on the GUI thread and moves the window itself instead of a tracking rectangle), the pointer taking over a keyboard tracking (a click ends it where it is), Window list / `SC_TASKMANAGER` (greyed: no window list), Alt+F11 for Hide, a minimized frame's system menu on a click, submenus and `PU_MOUSEBUTTONnDOWN` initial states in `WinPopupMenu` (submenu items are greyed), the `PU_KEYBOARD` / `PU_MOUSEBUTTONn` usage flags (keyboard and button 1 always work); unverified: the item order, mnemonics and accelerator texts (from the GG24-3732 screenshot also used for the VIO window), which items are enabled when (Minimize / Maximize / Hide only with their button, Size only with a sizing border, Move unless maximized), no `WM_INITMENU` / `WM_MENUSELECT` / `WM_MENUEND` to the client for the system menu (documented for `FID_MENU` only) nor to a pop-up menu's owner, the first arrow of a keyboard size only picking the edge, the 8-pel arrow step (WinTrackRect's example), `WM_SIZE` once at the end of a keyboard size, `(x, y)` of `WinPopupMenu` as the menu's bottom-left corner and `PU_POSITIONONITEM` centring the item's row on it, the menu always kept on the desktop
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

- [ ] **VIO window follow-ups** — the VIO window resizes with scroll bars, minimizes and maximizes, has a system menu with Mark / Copy / Copy All / Paste, takes the mouse (MOUCALLS) and switches to full screen with Alt+Home; still missing: Font Size (no source lists the cell sizes; Warpine has only the 8 × 16 font), the Scroll item (undocumented), Help, Hide (no window list to bring the window back), the window list (Ctrl+Esc), keyboard marking and autoscroll while marking past the view's edge, several VIO sessions on one desktop (`DosStartSession` from a windowed session), Advanced VIO (`VioCreatePS`, `VioAssociate`) for PM programs; unverified: that `SSF_TYPE_DEFAULT` runs a window-compatible program windowed and an unspecified application type windowed, the largest size (the whole screen plus a scroll bar's room), the maximized placement, the view following the cursor, the minimum size, the scroll-bar and menu look, Alt+Space / Shift+Esc opening the menu and Alt+F4 ending the program instead of reaching it, the mnemonics, which items are enabled when, keys ignored in Mark mode, Copy All's trailing blank lines, Paste's scan codes and dropped control characters, Alt+Home for OS/2 (not DOS) sessions and Alt+Home being consumed, integer scaling with black borders full screen
- [ ] **MOUCALLS follow-ups** — `MouGetHotKey` / `MouSetHotKey`, `MouRegister` / `MouDeRegister`, `MouInitReal`, `MouGetThreshold` / `MouSetThreshold` (`ERROR_INVALID_FUNCTION` now); MOUCALLS for 16-bit NE programs (`ne_exec.rs`); argument names of Pascal-order calls in `api_trace`; unverified: one handle and `MouOpen` again returning it, `MouOpen` failing without a display, the 64-event queue dropping the oldest, a move with a button change split into two events, `MouReadEventQue` with no event and no wait returning an all-zero record (the reference also lists `ERROR_MOUSE_NO_DATA`), the mask filtering, mickey reporting, `MouSetPtrPos` queuing nothing, the `NOPTRRECT` reading, error codes for a short `MouGetPtrShape` buffer and reserved mask bits, text-mode pointer shape limits, 3 buttons and 79 mickeys/cm, the default pointer masks, the PM pointer in a window instead of the text pointer
- [ ] **4OS2 pipes** — `ver | find "x"` hangs: 4OS2 creates a pipe, duplicates it into handle 0/1 and calls `DosExecPgm` with an empty program name (`execFlag` 2), then sleeps waiting on its `\SHAREMEM\4OS2…` block; Warpine fails to start anything. Needs a look at how 4OS2 starts the other side of a pipe and at passing OS/2 pipe handles to child processes (redirected handles to files are passed on since `DosExecPgm` hands the host file to the child)
- [ ] **ANSI follow-ups** — not implemented: the cursor position report `ESC[6n` (OS/2 puts `ESC[row;colR` into the keyboard buffer), keyboard reassignment `ESC[…p` (parsed, quoted strings skipped, ignored), display modes `ESC[=0h`…`ESC[=6h` (ignored). Unverified (Warpine's own choices, sources: OS/2 FAQ 5.12, REXX Tips & Tricks "ANSI ESC Sequences", the VioWrtTTY reference): `ESC[J` / `ESC[1J` ignored (only `ESC[2J` is documented); unknown sequences swallowed; an `ESC` not followed by `[` shown as a character; `ESC[?7h` treated like `ESC[=7h`; `SGR 2` = normal intensity (REXX Tips only); underline (4) ignored on the colour attribute; `ESC[2J` / `ESC[K` fill with the current ANSI attribute, scrolled-in lines with 0x07; wrap off overwrites the last column; TAB writes spaces to the next multiple of 8; `VioSetAnsi` values other than 0 / 1 return `ERROR_VIO_INVALID_PARMS`; one ANSI state per process (OS/2 has one per session); LF also returns to column 0 (kept from before)
- [ ] **Console handle details** — `DosClose` of a standard handle marks it closed until something is duplicated into it (unverified which error a later `DosWrite` gets: Warpine says `ERROR_INVALID_HANDLE`); `DosSetFilePtr` on the console fails with `ERROR_INVALID_HANDLE` (OS/2 may say `ERROR_SEEK_ON_DEVICE`); console output to a host pipe or file is converted from CP437 whatever the process codepage (`api::doscalls::dos_write`)
- [ ] **`KbdCharIn` / `KbdStringIn` at end of piped input** — Warpine's own choice (the KBD reference lists no end-of-input error): a waiting call ends the process with exit code 0 (`EXIT_CODE_KBD_END_OF_INPUT`, like the built-in CMD.EXE); `KbdStringIn` first returns a partly read line; `IO_NOWAIT` still reports "no character". Check against real OS/2 with redirected input if a reference turns up

---

## Codepages and NLS

Codepages of PM text (message queues, presentation spaces) are under "Presentation Manager".

No open items.


---

## OS/2 2.0 parity

Small but fundamental gaps between Warpine and what OS/2 2.0 offered a 32-bit program, found on 2026-11-18 by searching `src/` for each API name. Large subsystems (DOS and Windows sessions, the Workplace Shell, containers, notebooks, drag and drop, printing, the help viewer, REXX) are tracked elsewhere.

### Kernel (DOSCALLS)

- [ ] **Thread control** — `DosSuspendThread` / `DosResumeThread`, `DosSendSignalException`
- [ ] **Shared memory** — `DosGiveSharedMem`
- [ ] **Named pipes** — only create, connect and set-state exist: `DosDisConnectNPipe`, `DosPeekNPipe`, `DosTransactNPipe`, `DosCallNPipe`, `DosWaitNPipe`, `DosQueryNPHState`, `DosQueryNPipeInfo`
- [ ] **Messages** — `DosInsertMessage`, `DosQueryMessageCP`

### PM window manager (PMWIN)

- [ ] **Painting** — `WinInvalidateRegion`, `WinExcludeUpdateRegion`, `WinLockWindowUpdate`, `WM_ERASEBACKGROUND`
- [ ] **Messaging** — `WinPostQueueMsg`, `WinBroadcastMsg`, `WinInSendMsg`, `WinWaitMsg`, `WinWaitEventSem`, `WinRequestMutexSem`
- [ ] **Presentation parameters** — `WinSetPresParam`, `WinQueryPresParam`, `WinRemovePresParam`, `WM_PRESPARAMCHANGED`
- [ ] **Frames and accelerators** — `WinCreateFrameControls`, `WinCalcFrameRect`, `WinFormatFrame`; `WinCreateAccelTable`, `WinDestroyAccelTable`, `WinCopyAccelTable`
- [ ] **Other window calls** — `WinLoadMessage`, `WinQueryVersion`, `WinFlashWindow`, `WinSetWindowBits`, `WinSetMultWindowPos`, `WinSetDlgItemShort` / `WinQueryDlgItemShort`, `WinSetHook` / `WinReleaseHook` (help and message-filter hooks at least)

### Built-in controls

- [ ] **Owner-drawn items** — `WM_DRAWITEM` / `WM_MEASUREITEM` for list boxes, menus and buttons
- [ ] **Combo boxes, spin buttons, sliders, value sets** — `WC_COMBOBOX` and `WC_SPINBUTTON` exist only as class names; `WC_SLIDER` and `WC_VALUESET` not at all
- [ ] **Static and class styles** — `SS_ICON`, `SS_BITMAP`, `SS_FGNDRECT`; `CS_SIZEREDRAW`, `CS_SYNCPAINT`, `CS_CLIPCHILDREN`, `WS_CLIPSIBLINGS`

### GPI

- [ ] **Polylines and polygons** — `GpiPolygons`, `GpiPolyFillet`, `GpiPolySpline`
- [ ] **Areas, paths and clipping** — `GpiBeginArea` / `GpiEndArea`, `GpiBeginPath` / `GpiStrokePath` / `GpiFillPath`, `GpiCreateRegion`, `GpiSetClipRegion`
- [ ] **State and coordinate spaces** — `GpiSavePS` / `GpiRestorePS` / `GpiResetPS`, `GpiSetAttrs`, `GpiSetPageViewport`, `GpiSetDefaultViewMatrix`, `GpiConvert`, model transforms
- [ ] **Text layout** — `GpiCharStringPos`, `GpiQueryCharStringPos`, `GpiQueryWidthTable`, character angle and shear

### System DLLs not present at all

- [ ] **PMSHAPI** — `WinStartApp`, the switch list (`WinAddSwitchEntry`, `WinCreateSwitchEntry`, `WinQuerySwitchList`)
- [ ] **Atoms and DDE** — `WinCreateAtomTable`, `WinAddAtom`, `WinFindAtom`, `WinQueryAtomName`; `WinDdeInitiate` / `WinDdePostMsg`
- [ ] **Standard dialogs** — `WinFileDlg`, `WinFontDlg`
- [ ] **Printing stubs** — the spooler (`SplQmOpen`, …) reporting no printers, `DevEscape`, `DevPostDeviceModes`


### Follow-ups of the first parity batch

- [ ] **Exit-list follow-ups** — the routines run on the thread that ends the process (OS/2 uses thread 1); they do not run yet for a closed window, Ctrl+Esc, the end of piped keyboard input, triple faults / KVM errors, or `DosKillProcess` from a parent (catch SIGTERM, `TC_KILLPROCESS`); `TC_HARDERROR` / `TC_TRAP` are never produced; a thread spinning in guest code is not stopped during the list. Unverified: a routine that returns, calls `DosExit` or faults counts as `EXLST_EXIT` (the reference says the process hangs), `EXLST_EXIT` outside processing → `ERROR_INVALID_FUNCTION`, removing an unlisted routine or adding NULL → `ERROR_INVALID_DATA`, re-adding moves a routine to its new order, at most 256 routines (`ERROR_NOT_ENOUGH_MEMORY`), the exception chain emptied before each routine
- [ ] **Semaphore follow-ups** — `ERROR_SEM_OWNER_DIED` for a mutex whose owner ended, `DCE_POSTONE` / `DCE_AUTORESET`, `ERROR_SEM_BUSY` when closing a semaphore with waiters or an owner, `DosWaitMuxWaitSem` polling every 10 ms instead of waiting on a condition variable, named semaphores shared between processes. Unverified: pid / tid 0 for an unowned mutex, `ERROR_INVALID_PARAMETER` for NULL output pointers, 0 written by `DosResetEventSem` when already reset, names case-insensitive and unique across kinds, `ERROR_SEM_NOT_FOUND` for a name of the wrong kind, `ERROR_INVALID_HANDLE` deleting a non-member, `ERROR_TOO_MANY_SEMAPHORES` reported before a duplicate, `ERROR_INVALID_PARAMETER` for muxwait flags with neither / both `DCMW_*` bits, a waiter whose member was closed getting `ERROR_INVALID_HANDLE`, wait-all returning the last record's user value
- [ ] **Scroll-bar follow-ups** — frame scroll bars (`FCF_VERTSCROLL` / `FCF_HORZSCROLL`; blocked: client GPI drawing ignores the client's position in the frame surface and `control_at` searches only the client's children), the keyboard on a focused scroll bar, `SBS_AUTOTRACK`, the slider's tracking rectangle (slider 0 when released far from the bar), repeat rates from `SV_FIRSTSCROLLRATE` / `SV_SCROLLRATE` (now `DRAG_SCROLL_MS`); unverified: an 8-pel minimum slider, a new bar starting with range 0..0 (disabled), `SBCDATA` with `cTotal` 0 setting no thumb size, a click not taking the focus, the bar's look
- [ ] **Text-cursor follow-ups** — hide the cursor while the user tracks a frame; unverified: one cursor per message queue (the reference's "destroys any existing one" may mean one in the system), `CURSOR_SETPOS` from a window that does not own the cursor returning FALSE, a negative width / height extending left / down, `rclClip` reported as the window rectangle when none was given, `WinQueryCursorInfo` reading the caller's queue, no `WM_TIMER(TID_CURSOR)` posted to the application for `CURSOR_FLASH`
- [ ] **Rectangles and WinDrawText: unverified details** — WinPtInRect leaving the right and top edges out, (0, 0, 0, 0) as the empty result of WinIntersectRect / WinUnionRect / WinSubtractRect, NULL pointers failing without an error code, coordinates not limited to 16 bits; WinDrawText's count with `DT_WORDBREAK` including trailing blanks and the line end, nothing drawn with `DT_QUERYEXTENT`, the inclusive extent it writes, `DT_ERASERECT` using the background colour, `DT_HALFTONE` as the mean colour, `~` as the `DT_MNEMONIC` prefix, no clipping to the rectangle, `DT_EXTERNALLEADING` adding nothing
- [ ] **Profile follow-ups** — `PrfQueryProfile`, `PrfReset`, the program-list calls (`PrfAddProgram`, …), the 16-bit `PRF16*` / `WIN16*` exports; reading binary OS/2 INI files shipped with programs (format not publicly documented; refused as corrupt now). Unverified: truncated `PrfQueryProfileData` returning TRUE with the bytes copied, a truncated string's last byte forced to NUL, list sizes with both NULs but counts without the last, `HINI_PROFILE` enumeration merging user and system names, deleting what does not exist succeeding, deleting the last key removing the application, a NULL application on a write rejected, integer overflow in `PrfQueryProfileInt` wrapping, the 64 KB value limit, `PrfOpenProfile` creating a missing file and returning the same handle on a re-open, `PrfCloseProfile(HINI_USER/SYSTEMPROFILE)` failing, `PMERR_INVALID_HWND` / `PMERR_INVALID_PARM` from the help calls, the owner-chain walk of `WinQueryHelpInstance`
- [ ] **Interrupting a thread that spins in guest code** — critical sections, `DosKillThread` and exit lists stop other threads only at their next call into Warpine; a KVM immediate exit (or a signal to the vCPU thread) would stop busy loops too
- [ ] **Critical-section, timer and module follow-ups** — unverified: a thread ending inside a section ends it, held threads end when the process ends or they are killed, `ERROR_INVALID_THREADID` never returned (calls from exception handlers), the 32 ms timer tick and an interval of 0 as one tick, the 64-timer limit, a one-shot handle invalid after it posted, a closed semaphore stopping its timer, "shared" meaning `DC_SEM_SHARED` or named; `DosSearchPath` skipping elements that do not resolve whatever `SEARCH_IGNORENETERRS`, matching files only (hidden and system included), keeping the caller's spelling; `DosQueryModuleName` reporting module handle 0 as the executable, `C:\NAME.EXE`, built-in modules as `C:\OS2\DLL\NAME.DLL`
- [ ] **Suballocation follow-ups** — a pool whose memory is freed with `DosFreeMem` without `DosSubUnsetMem` stays registered (a new `DOSSUB_INIT` there gets `ERROR_DOSSUB_OVERLAP`); shared pools between processes; unverified: the error codes chosen for unknown flags, small or unset pools, zero / oversized blocks, unknown pools and blocks off the 8-byte grid, first-fit allocation, the 1024-pool and 65536-free-range limits, `DOSSUB_SPARSE_OBJ` only recorded and `DOSSUB_SERIALIZE` changing nothing
- [ ] **Painting follow-ups** — true update regions (needs GPI regions; then `hrgnUpdate` of `WinScrollWindow`), clipping the paint PS to the update rectangle, a synchronous `WinUpdateWindow`, `WinShowWindow` invalidating a window it shows, GPI / `WinFillRect` through a child window's PS (drawn at the client origin, no child offset), `CS_SYNCPAINT`, `DB_RAISED` / `DB_DEPRESSED` if a source documents them; unverified: `WinValidateRect` removing only bands off an edge, (0, 0, 0, 0) from `WinQueryUpdateRect` / `WinBeginPaint` for a valid window, `WinScrollWindow` clipping to the scroll rectangle without a clip rectangle and failing for `HWND_DESKTOP`, `WinDefWindowProc(WM_PAINT)` validating the window only, a border without `DB_AREAATTRS` in solid `clrFore`, `DB_DLGBORDER` details, negative border widths as 0
- [ ] **Window-lookup, input-state and colour follow-ups** — the desktop background, menus and control faces should read `SYSCLR_*` too; a guest-callable window procedure for built-in classes (subclassing through `WinQueryClassInfo` / `WinSubclassWindow` keeping their behaviour); a synchronous key state per queue (one per process now); CapsLock / NumLock toggle state from the host at start; dialogs placed under a client window positioned frame-relative by `WinSetWindowPos`; disabled buttons not drawn greyed. Unverified: `WinWindowFromPoint` skipping hidden but not disabled windows and testing only the Z-order on the desktop, the `PMERR_*` codes chosen for bad counts / pointers / keys / `cchMax`, `WinEnumDlgItem` eligibility (visible and enabled, `WS_TABSTOP` for tab items), the class styles and window procedure `WinQueryClassInfo` reports for built-in classes, case-sensitive class names, `HWND_DESKTOP` from `WinQueryDesktopWindow`, a double click counting as a press, `PMERR_NO_MSG_QUEUE` from `WinQueryMsgPos` / `WinQueryMsgTime`, `WM_SYSCOLORCHANGE` posted rather than sent, `LCOL_PURECOLOR` ignored
- [ ] **Help viewer** — an INF / HLP reader (the IPF format is not published by IBM: check osFree / NewView descriptions for clean-room use first), `RT_HELPTABLE` / `RT_HELPSUBTABLE`, F1 → `WM_HELP` → the help-table panel, the `HM_*` notifications back to the program, the `Ddf*` calls

---

## Application compatibility

- [ ] **Thread follow-ups** — ending a thread ends only that thread (`threads.rs`); still missing: `DosCreateThread` flags (`CREATE_SUSPENDED`, `STACK_COMMITTED`) and `DosSuspendThread` / `DosResumeThread`, `DosKillThread` interrupting a thread that spins in guest code or blocks in a call (it ends at the thread's next exit to Warpine), `ERROR_SEM_OWNER_DIED` for mutex semaphores owned by an ended thread, a PM thread's windows destroyed with it (its message queue is); unverified: thread 1 ending with `DosExit(EXIT_THREAD)` while other threads run (the reference only says "do not end thread 1 without ending the process"; here the process goes on until its last thread ends), the exit code when thread 1 returns from the entry point (unchanged here, 0 unless set), `DosWaitThread(0)` when no thread that could end is left (`ERROR_INVALID_THREADID` here, instead of waiting forever), an ended thread reported once and then `ERROR_INVALID_THREADID`, `DosKillThread` on the calling thread (`ERROR_INVALID_THREADID` here), a thread's message queue destroyed when the thread ends, thread ids not reused
- [ ] **Sockets: `select` never reports a socket** — `samples/socket_test` fails `select(srv readable)`. Warpine numbers sockets from 0xA000 (`SocketManager::next_handle`), but `so_select` reads the guest's `fd_set` as a 64-bit mask of handles 0–63, so no socket can ever be in it (and the guest's own `FD_SET(0xA000)` writes far past an 8-byte set). The layout is also uncertain: Open Watcom's OS/2 `<types.h>` declares `fd_set` as `{ u_short fd_count; int fd_array[FD_SETSIZE]; }` (WinSock style, `FD_SETSIZE` 64, `MAXSOCKETS` 32768), while the sample and `so_select` assume a bitmask. Find in public IBM TCP/IP programming references which layout(s) the 32-bit `select` (SO32DLL / TCP32DLL ordinal 13) and `os2_select` take and how socket numbers are allocated, then renumber sockets and translate the set accordingly

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
