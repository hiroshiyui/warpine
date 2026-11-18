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
- [Application compatibility](#application-compatibility) — 16-bit (NE) programs, REXX, date and time follow-ups
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
| Presentation Manager | Pointer input routed when read — clicks hit-tested, activating their frame and moving the focus at the head of the system input queue, strictly after the input before them (ADR-0051, `pm_inputorder_test`) | Complete | [ADR-0051](adr/0051-pointer-input-routed-when-read.md) |
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
| Core | Kicking vCPUs out of guest code; `DosSuspendThread` / `DosResumeThread`, signal exceptions (ADR-0023) | Complete | `src/loader/host_signal.rs`, `src/loader/signals.rs`, `samples/thread_control_test/` |
| Core | Named pipes, pipes / arguments / environment for child processes, one handle space for files and pipes, `DosGiveSharedMem` (ADR-0022) | Complete | `src/loader/npipe.rs`, `src/loader/inherit.rs`, `samples/npipe_test/` |
| Core | Shared event and mutex semaphores between the processes of a session: named and `DC_SEM_SHARED` semaphores in a host-only memfd of slots, session-wide handles, an OFD lock, futex wake-ups, owner-died on process end (ADR-0050) | Complete | `src/loader/shared_sem.rs`, `samples/shsem_test/` |
| Core | Shared memory between the processes of a session: named, gettable and giveable blocks in a sealed memfd arena at the same address in every process, holders tracked in the session directory (ADR-0049) | Complete | `src/loader/shared_arena.rs`, `samples/shmem_test/` |
| Presentation Manager | GPI regions, clipping, areas, paths, polygons and curves (ADR-0024) | Complete | `src/gui/region.rs`, `src/loader/pm_region.rs`, `src/loader/pm_area.rs`, `samples/pm_region_test/` |
| Presentation Manager | Combination boxes, spin buttons, owner drawing (ADR-0025) | Complete | `src/loader/pm_combo.rs`, `src/loader/pm_spin.rs`, `src/loader/pm_owner.rs`, `samples/pm_combo_spin_test/` |
| Presentation Manager | PM hooks and messaging calls (`WinSetHook`, `WinBroadcastMsg` …; ADR-0026), frame controls and accelerator tables, presentation parameters, static and class styles, `WinLockWindowUpdate`, sliders and value sets | Complete | `src/loader/pm_hook.rs`, `pm_message.rs`, `pm_frame.rs`, `pm_accel.rs`, `pm_presparam.rs`, `pm_static.rs`, `pm_paint_style.rs`, `pm_slider.rs`, `pm_valueset.rs`; samples `pm_msg_hook_test`, `pm_frame_accel_test`, `pm_presparam_test`, `pm_slider_valueset_test` |
| Presentation Manager | GPI presentation-space state, coordinate spaces and transforms (ADR-0027), text layout and character attributes | Complete | `src/loader/pm_psstate.rs`, `pm_xform.rs`, `pm_coords.rs`, `pm_text.rs`, `src/gui/glyph_transform.rs`; samples `pm_xform_test`, `pm_textpos_test` |
| Presentation Manager | `WinStartApp`, switch list, atom tables, DDE (per process, ADR-0030); standard file and font dialogs, PMSPL / PMCTLS module slots, printing stubs (ADR-0029) | Complete | `src/loader/pm_startapp.rs`, `pm_switch.rs`, `pm_atom.rs`, `pm_dde.rs`, `pm_filedlg.rs`, `pm_fontdlg.rs`, `pmspl.rs`; samples `pm_switch_dde_test`, `pm_stddlg_test` |
| Core | MSG `DosInsertMessage` / `DosQueryMessageCP`, MSG ordinals from `os2386.lib`; process nesting limit (ADR-0028) | Complete | `src/loader/msgcalls.rs`, `src/loader/inherit.rs`; sample `nest_test` |
| Core | Parity follow-ups: `DosKillProcess` with KILLPROC and exit lists (ADR-0034), interruptible blocking calls (ADR-0035), session-unique process ids (ADR-0036), semaphore owner death / busy / auto-reset, pipe semaphores and stale-directory sweeps (ADR-0037), frame scroll-bar input and client origins, modal message-filter hooks, standard-dialog notifications, `GpiSetAttrs` character bundle (ADR-0038) | Complete | `src/loader/signals.rs`, `interrupt.rs`, `inherit.rs`, `ipc.rs`, `npipe_sem.rs`, `npipe_dir.rs`, `pm_stddlg.rs`; samples `kill_test`, `argv_test`, `semfollow_test`, `pm_frame_scroll_test`, `pm_charbundle_test`, `pm_modal_filter_test` |
| Presentation Manager | Embedded dialogs (a dialog with a window parent is a child of it, ADR-0042) and `WC_NOTEBOOK` (pages, major / minor tabs, status line, page buttons, `BKN_PAGESELECTED` / `BKN_PAGEDELETED` sent with their structures) | Complete | `src/loader/pm_types.rs`, `pm_notebook.rs`; samples `pm_child_dialog_test`, `pm_notebook_test` |
| Presentation Manager | `WC_CONTAINER`, first stage — records and field infos in guest memory carved from per-container arenas (ADR-0043), insertion orders and hierarchy, `CNRINFO`, text / name / detail views with titles and separators, single / multiple / extended selection, `CN_EMPHASIS` / `CN_ENTER` sent with their structures, `CM_SEARCHSTRING`, `CM_QUERYRECORDRECT` | Complete | `src/loader/pm_container.rs`; sample `pm_container_test` |
| Presentation Manager | `WC_CONTAINER`, stage 2a — sorting with the application's comparison function as a resumable merge sort, one comparison per call chain (`CM_SORTRECORD`, `CNRINFO.pSortRecord` on insertion; ADR-0044), the tree view (expander boxes, `CM_EXPANDTREE` / `CM_COLLAPSETREE`, `CN_EXPANDTREE` / `CN_COLLAPSETREE`, + / - / Left / Right), the vertical scroll bar (`CID_VSCROLL`, `WM_VSCROLL`, `CN_SCROLL`) | Complete | `src/loader/pm_container.rs`; sample `pm_cnrtree_test` |
| Presentation Manager | `WC_CONTAINER`, stage 2b — the icon view at `ptlIcon` with pel scrolling, `CM_ARRANGE` / `CCS_AUTOPOSITION`, icons and bitmaps in the icon, name, tree and detail views (`CV_MINI`, `CA_DRAWBITMAP`, `CFA_BITMAPORICON`; system-icon pictograms), `CM_QUERYRECORDFROMRECT`, `CM_FILTER` as one call chain with `CMA_FILTER`, direct editing in an MLE (`CM_OPENEDIT` / `CM_CLOSEEDIT`, `CN_BEGINEDIT` / `CN_REALLOCPSZ` / `CN_ENDEDIT`, read-only rules; ADR-0046) | Complete | `src/loader/pm_container.rs`; sample `pm_cnricon_test` |
| Presentation Manager | Tier-1 PM fundamentals — `WrpBuiltinWindowProc` (`WARPINE.5`) as the built-in classes' procedure for subclassing, standard frames running the default procedure, `WM_QUERYFRAMECTLCOUNT` / `WM_FORMATFRAME` / `WM_CALCFRAMERECT` for subclassed frames, `WC_FRAME` windows composited at creation, child-window presentation spaces, the `WinBeginPaint` clip, a synchronous `WinUpdateWindow`, `WinShowWindow` invalidating, `WM_CHAR` to the owner, `WM_SETFOCUS`, application controls in dialog Tab and mnemonics (`WM_QUERYDLGCODE` / `WM_MATCHMNEMONIC`), a thread's windows going with it, interruptible pipe writes, sockets and modal loops (ADR-0048) | Complete | `pm_window.rs`, `pm_frame.rs`, `pm_paint.rs`, `pm_dialog.rs`; sample `pm_subclass_test` |
| Presentation Manager | Drag and drop within a process — the PMDRAG module (`DrgAllocDraginfo`, items, string handles, `DrgVerify*` / `DrgQuery*`, transfers), `WM_BEGINDRAG` from a button-2 drag, `DrgDrag` sending `DM_DRAGOVER` / `DM_DRAGLEAVE` / `DM_DROP` through call chains fed by the GUI thread, containers as targets (`CN_DRAGOVER` / `CN_DRAGLEAVE` / `CN_DROP`) and sources (`CN_INITDRAG`) (ADR-0045) | Complete | `src/loader/pm_drag.rs`; sample `pm_drag_test` |
| Core | Quick fixes after 0.0.3 — socket `select` on the OS/2 TCP/IP `fd_set` with `poll(2)` (ADR-0041), real `GINFOSEG` / `LINFOSEG` with a running clock (ADR-0040), `FILE_READONLY` set by path, handle and `DosSetFileMode`, read-only files not deletable | Complete | `src/loader/tcpip.rs`, `infoseg.rs`, `vfs_hostdir.rs`; samples `socket_test`, `infoseg_test`, `fileattr_test` |
| Core | Dates after 2038 — session clock for `DosSetDateTime` (`WARPINE_CLOCK`, inherited offset), `DATETIME` time zone west of UTC, file times in local time clamped to 1980–2107 and set by `DosSetFileInfo` / `DosSetPathInfo`, `WARPINE.DLL` 64-bit time calls, `DosQueryProcAddr` by name for built-in modules (ADR-0039); redirecting third-party C-runtime time imports dropped (a signed 32-bit `time_t` cannot be fixed from outside) | Complete | `src/loader/clock.rs`, `warpine_api.rs`; sample `datetime_test` |
| Project | Development history (the baseline phases 1–7) | Complete | [Developer Guide §20](developer_guide.md#appendix-development-phases) |

---

## Presentation Manager

Window management, messages, GPI drawing, built-in controls, menus, dialogs and message boxes work (see the "Completed" table and the developer guide). Open work:

### Controls and text editing

- [ ] **Text controls: unverified details** — the drag-scroll rate past the edge (`DRAG_SCROLL_MS`, 100 ms; no source gives one for text controls), whether `SV_CURSORRATE` is the time between toggles (as here) or a whole blink cycle, the input-method area spanning the caret's line across the control; `WinSetSysValue` keeps only `SV_CURSORRATE` (other indices fail here, while PM accepts its settable values); whether a list box keeps Enter after sending `LN_ENTER` (it passes it on to the owner here, so a dialog's default button still works), what `MLM_SETWRAP` returns (TRUE here), the Space / Ctrl+Space rules of `LS_EXTENDEDSEL` (Space toggles the cursor item here), the mp2 of the `MLN_*` notifications (the control's handle for `MLN_SETFOCUS` / `MLN_KILLFOCUS`, 0 for `MLN_CHANGE` here), what `LM_DELETEITEM` returns for an index out of range (the unchanged count here), and whether PM entry fields also take Ctrl+C / Ctrl+X / Ctrl+V (only the CUA keys Ctrl+Insert / Shift+Delete / Shift+Insert are handled)
- [ ] **`WC_CONTAINER`, next stages** — records (in guest memory, ADR-0043), field infos, `CNRINFO`, the text, name, detail, tree and icon views with icons and bitmaps, selection, sorting (`CM_SORTRECORD`, `pSortRecord`; ADR-0044), filtering and direct editing of record texts (ADR-0046), `CM_ARRANGE` / `CCS_AUTOPOSITION`, the vertical scroll bar and `CN_EMPHASIS` / `CN_ENTER` / `CN_SCROLL` / `CN_EXPANDTREE` / `CN_COLLAPSETREE` / `CN_BEGINEDIT` / `CN_REALLOCPSZ` / `CN_ENDEDIT` / `CN_SETFOCUS` / `CN_KILLFOCUS` work (`pm_container.rs`); still missing: the tree view's `hbmExpanded` / `hptrExpanded` and `pTreeItemDesc`, tree lines (`CA_TREELINE`), `CM_MOVETREE`, the grid arrangements (`CMA_ARRANGEGRID`, `CMA_ARRANGESELECTED`, `CM_SETGRIDINFO`), `RECORDINSERT.zOrder` / `CMA_ZORDER` as an order of its own, `CM_ERASERECORD`, editing the container title and column headings (`CID_CNRTITLEWND`, `CID_LEFTCOLTITLEWND`), keeping an edit when the focus leaves the MLE without a click in the container, owner drawing (`CA_OWNERDRAW`, `CFA_OWNER`, `CNRDRAWITEMINFO`), the horizontal scroll bar and horizontal scrolling of the detail and icon views, the split bar (`pFieldInfoLast`, `xVertSplitbar`), the mouse wheel, `CM_QUERYVIEWPORTRECT(CMA_WORKSPACE)`, `CN_CONTEXTMENU` / `CN_HELP`, records shared between containers; the selection and `CN_*` notifications from the mouse and keys are Warpine's reading of the PM Guide; unverified: whether `CM_SETRECORDEMPHASIS` sends `CN_EMPHASIS` (it does here), all or any of `CM_QUERYRECORDEMPHASIS`'s bits (all here), `RECORDCORE.cb` counting the application's bytes (core size here), the container freeing inserted records when destroyed (it does here), the default `CNRINFO` values (icon view, no line spacing, tree indentation 16), `CN_SCROLL`'s `lScrollInc` in pels, `CM_EXPANDTREE` / `CM_COLLAPSETREE` with a NULL record acting on every record, `pStorage` NULL for `pSortRecord` sorts, whether PM's sort is stable (it is here), `ptlIcon` naming the icon's lower-left corner, the arrangement spacing (8 pels), the tree view's icon size (16), mini handles falling back to the normal ones, the filter asking about child records, `CMA_FILTER` passing over filtered records, `cbText` counting the NUL, `CN_REALLOCPSZ` for unchanged text
- [ ] **Notebook follow-ups** — `WC_NOTEBOOK` works (`pm_notebook.rs`); still missing: `BKN_PAGESELECTEDPENDING` (Warp 4: the owner may veto a turn), drawing tab bitmaps (`BKM_SETTABBITMAP`), tab colours (`BKM_SETTABCOLOR`) and notebook buttons (`BKM_SETNOTEBOOKBUTTONS`, `BKS_BUTTONAREA`), page dialogs loaded by `BKM_SETPAGEINFO` (`BFA_PAGEFROMDLGRES` / `BFA_PAGEFROMDLGTEMPLATE`), scrolling arrows when tabs do not fit, Tab moving from the notebook into the top page; `BKS_SPIRALBIND`, `BKS_ROUNDEDTABS` / `BKS_POLYGONTABS` and the back-page corners are drawn as the defaults; unverified: whether inserting the first page or deleting the top page notifies, and whether `BKM_QUERYPAGECOUNT` counts its end pages
- [ ] **Embedded dialog follow-ups** — a dialog whose parent is a window is embedded in it (ADR-0042); still missing: chrome (title bar, border) for an embedded dialog that asks for it, Tab between an embedded dialog's controls and those around it, `WinWindowFromPoint` / pointer messages reaching an embedded dialog's guest-drawn child windows
- [ ] **Drag and drop follow-ups** — drags within a process work (`pm_drag.rs`, ADR-0045); still missing: host files dropped on a window (SDL drop events → a `DRAGINFO` of `DRM_OS2FILE` items and `DM_DRAGOVER` / `DM_DROP`), `DrgDragFiles` / `DrgAcceptDroppedFiles`, lazy drag (`DrgLazyDrag`, `DrgLazyDrop`, `DrgCancelLazyDrag`, `CN_PICKUP`, `CN_DROPNOTIFY`), `DrgReallocDraginfo`, `DrgQueryFormat`, `DrgQueryDraginfoPtr` / `FromHwnd`, the drag image (`DrgSetDragImage` accepted, not drawn), `DM_DRAGOVER` on key presses (Ctrl / Shift for the operation, `usOperation`), target emphasis drawn by the container, `CN_DRAGAFTER`, `WM_BEGINDRAG` / `WM_ENDDRAG` passed to the owner by `WinDefWindowProc` and from the keyboard, drags between OS/2 programs (needs the session desktop server); unverified: the drag button and threshold (button 2, 4 pels), string handles shared by equal strings, `xDrop` / `yDrop` updated at each `DM_DRAGOVER`, a container passing its owner's `CN_DRAGOVER` reply on

### Dialogs, menus and message boxes

- [ ] **Dialog and button follow-ups** — asking application controls `WM_QUERYDLGCODE` (they take Tab when `WS_TABSTOP`, ADR-0048), `BS_USERBUTTON` / `BN_PAINT` (a user button only stores its highlight), `WM_ACTIVATE` when the active window is destroyed, `WinProcessDlg` leaving its dialog to the caller (as now) to be checked; unverified for buttons (`pm_button.rs`): what the down-click `BM_CLICK(FALSE)` does (highlight only here), `BM_CLICK` on a disabled button (nothing here), how a highlighted check box or radio button looks (shaded indicator here), whether `BM_SETDEFAULT` changes the `BS_DEFAULT` style bit (it does here), `BM_SETCHECK` with mp1 > 2, the auto three-state order 0 → 1 → 2 → 0, `BN_CLICKED` for an arrow-key selection, a radio group with nothing checked having no tab stop; unverified for `WM_DESTROY` (`pm_destroy.rs`, following the `WinDestroyWindow` reference over the PM Guide's opposite order): owned windows destroyed before the frame's children, the focus leaving a desktop-level frame for no window; unverified against the PM reference: the `CMDSRC_*` code of the `WM_COMMAND(DID_CANCEL)` that Esc posts (`CMDSRC_OTHER` here), and the frame control data of a template built by IBM's resource compiler (Open Watcom `wrc` writes one ULONG of `FCF_*` flags), whether clicking a push button by its mnemonic moves the focus (it does not here), and the bounds of the `msgFirst > msgLast` exclusion of `WinGetMsg` / `WinPeekMsg`
- [ ] **Menus in dialogs and other frame controls** — `WM_UPDATEFRAME(FCF_MENU)` attaches a menu to a standard frame only; a dialog (PM Guide: `WinLoadMenu` then `WM_UPDATEFRAME` to the dialog) goes through `WinDefDlgProc`, which ignores it, and other `FCF_*` flags are ignored; `WinSetParent` / `WinSetOwner` of a menu window are not implemented
- [ ] **Message box follow-ups** — `MB_CUSTOMICON` / `WinMessageBox2`, the `HK_HELP` hook (`idWindow`), `MB_NONMODAL`, `MB_SYSTEMMODAL`, `MB_MOVEABLE`; unverified: whether Esc ends a box without a Cancel button (here it does nothing; Close always cancels, as documented), that the box stays open after Help, and PM's layout and icon pictures (Warpine's own; centred on the desktop here)

### Keyboard, pointer and messages

- [ ] **Frame and system-key follow-ups** — Alt+F4/F5/F7/F8/F9/F10, Alt+Space / Shift+Esc, the system menu, the min/max/restore and hide buttons and title-bar / system-menu double clicks work (`pm_minmax.rs`, `pm_sysmenu.rs`); still missing: F10 / Alt for the menu bar (`SC_APPMENU`, `MM_STARTMENUMODE`), F1 → `WM_HELP`, real icons for minimized frames (`FCF_ICON`, `WinSetIcon`), min/max for dialogs, `WinSetActiveWindow` accepting minimized or disabled frames; the keys come from the OS/2 1.2 Quick Reference card (no public source maps them to `SC_*` codes); unverified: the `WM_SYSCOMMAND` mp2 of chrome actions, the `SWP` passed with `WM_MINMAXFRAME` (`SWP_MOVE | SWP_SIZE` added; changes the client makes are ignored), restoring a frame that was maximized then minimized to its normal size, the minimized frame's look (a 160 × 22 title bar in a bottom-left icon area), activation moving to the next frame when the active one is minimized or hidden (PM documents it only for a destroyed window), `SWP_ACTIVATE` / `SWP_DEACTIVATE` reported for frames only, `WM_SIZE` posted rather than sent and not at all for an unchanged size, the hide button taking the minimize button's place when a frame asks for both, `SC_HIDE` hiding frames without a hide button
- [ ] **System menu and pop-up menu follow-ups** — PM frames have a system menu (`gui/system_menu.rs`, drawn as a `PopupMenu`), keyboard move / size (`gui/frame_track.rs`) and `WinPopupMenu` (`pm_sysmenu.rs`); still missing: a real `FID_SYSMENU` window (`WinWindowFromID(frame, FID_SYSMENU)`, `MM_SETITEMATTR` / `MM_QUERYITEMATTR` to grey or remove items, and `SC_CLOSE` being "ignored if Close is disabled in the system menu"), `WM_TRACKFRAME` / `WM_QUERYTRACKINFO` and `WinTrackRect` for applications (the tracking runs on the GUI thread and moves the window itself instead of a tracking rectangle), the pointer taking over a keyboard tracking (a click ends it where it is), Window list / `SC_TASKMANAGER` (greyed: no window list), Alt+F11 for Hide, a minimized frame's system menu on a click, submenus and `PU_MOUSEBUTTONnDOWN` initial states in `WinPopupMenu` (submenu items are greyed), the `PU_KEYBOARD` / `PU_MOUSEBUTTONn` usage flags (keyboard and button 1 always work); unverified: the item order, mnemonics and accelerator texts (from the GG24-3732 screenshot also used for the VIO window), which items are enabled when (Minimize / Maximize / Hide only with their button, Size only with a sizing border, Move unless maximized), no `WM_INITMENU` / `WM_MENUSELECT` / `WM_MENUEND` to the client for the system menu (documented for `FID_MENU` only) nor to a pop-up menu's owner, the first arrow of a keyboard size only picking the edge, the 8-pel arrow step (WinTrackRect's example), `WM_SIZE` once at the end of a keyboard size, `(x, y)` of `WinPopupMenu` as the menu's bottom-left corner and `PU_POSITIONONITEM` centring the item's row on it, the menu always kept on the desktop
- [ ] **Input follow-ups** — the system input queue (`pm_input.rs`) routes keys and pointer input when they are read (ADR-0051); still missing: `WM_HITTEST` / `CS_HITTEST` (a window cannot refuse or let through a mouse message), `WM_MOUSEMOVE` coalescing when a queue is full (events are dropped beyond `MAX_INPUT_EVENTS`); clicks on frame chrome still act on the GUI thread when they arrive (unverified: whether PM queues them too); `WinQueryQueueStatus` (825, `QS_KEY` / `QS_MOUSE`…) does not exist yet: the PM Reference pages found so far list the `QS_*` flags and an "added since the last call" subset but not which word of the result holds which, nor whether `WinGetMsg` / `WinPeekMsg` reset that subset — find that before implementing it
- [ ] **Pointer follow-ups** — system pointers, `WinSetPointer`, `WinShowPointer` and `WinLoadPointer` work (`pm_pointer.rs`); missing: the I-beam over entry fields and MLEs (the MLE's `WM_MOUSEMOVE` page documents it, but the renderer sends `WM_MOUSEMOVE` to a control only while button 1 holds it), sizing arrows over frame borders, `WM_CONTROLPOINTER`, `WinCreatePointer` / `WinCreatePointerIndirect`, `WinQueryPointerInfo`, `WinDrawPointer`; unverified: NULLHANDLE accepted as the desktop window, the arrow as the initial pointer and after the current pointer is destroyed, "invert screen" pels drawn black, bitmap-array selection ignoring the display size, pen and object pointers shown as the arrow
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

All items of this list are implemented (five batches, 2026-11-18): kernel thread control, signals, named pipes and child inheritance; PM window manager, messaging and hooks, frames, presentation parameters; built-in controls up to sliders and value sets; GPI regions, clipping, areas, paths, state, coordinate spaces and text layout; PMSHAPI applications and switch list, atoms, DDE, MSG, the standard dialogs and printing stubs. What each batch left open is listed below.

### Follow-ups of the first parity batch

- [ ] **Exit-list follow-ups** — the routines run on the thread that ends the process (OS/2 uses thread 1); they run on a kill, a closed window or session, the end of piped input, faults (`TC_TRAP`) and KVM errors on other threads (`TC_HARDERROR`) since ADR-0034, but not after `HLT` or a KVM error on thread 1. Unverified: the 5 s grace period even when a handler continues, a routine that returns, calls `DosExit` or faults counts as `EXLST_EXIT` (the reference says the process hangs), `EXLST_EXIT` outside processing → `ERROR_INVALID_FUNCTION`, removing an unlisted routine or adding NULL → `ERROR_INVALID_DATA`, re-adding moves a routine to its new order, at most 256 routines (`ERROR_NOT_ENOUGH_MEMORY`), the exception chain emptied before each routine
- [ ] **Shared memory follow-ups** — blocks are shared across the processes of a session (ADR-0049), but without paging every process can reach the whole arena, and the access flags of a get or give are not enforced; 16-bit shared segments (`DosAllocShrSeg`, `DosGetShrSeg`) do not exist yet; a killed process's blocks go only when the table is next used. Unverified: getting a block twice counting once, `ERROR_INVALID_ADDRESS` from `DosGetSharedMem` for an address that is not a block
- [ ] **Semaphore follow-ups** — shared semaphores work across the processes of a session (ADR-0050), but named and shared muxwait semaphores stay within their process, a session holds at most 512 shared semaphores with at most 28 processes having one open, and a semaphore only a killed process had open goes only at the next create or open. Unverified: OS/2's own limits on shared semaphores, the owner-died hand-over to the next requester when a single thread ends (the reference describes it for a process end) and the count of 0 in `DosQueryMutexSem`, the reading of `DCE_POSTONE` / `DCE_AUTORESET`, an owned mutex without waiters closing fine, a wait stopped by process end returning `ERROR_TIMEOUT`, pid / tid 0 for an unowned mutex, `ERROR_INVALID_PARAMETER` for NULL output pointers, 0 written by `DosResetEventSem` when already reset, names case-insensitive and unique across kinds, `ERROR_SEM_NOT_FOUND` for a name of the wrong kind, `ERROR_INVALID_HANDLE` deleting a non-member, `ERROR_TOO_MANY_SEMAPHORES` reported before a duplicate, `ERROR_INVALID_PARAMETER` for muxwait flags with neither / both `DCMW_*` bits, a waiter whose member was closed getting `ERROR_INVALID_HANDLE`, wait-all returning the last record's user value
- [ ] **Scroll-bar follow-ups** — frame controls are positioned in frame coordinates but painted from the surface origin inside the left border (the vertical bar's right 2 pels lie under the border, a 2-pel gap left of the horizontal bar), the keyboard on a focused scroll bar, `SBS_AUTOTRACK`, the slider's tracking rectangle (slider 0 when released far from the bar), repeat rates from `SV_FIRSTSCROLLRATE` / `SV_SCROLLRATE` (now `DRAG_SCROLL_MS`); unverified: an 8-pel minimum slider, a new bar starting with range 0..0 (disabled), `SBCDATA` with `cTotal` 0 setting no thumb size, a click not taking the focus, the bar's look
- [ ] **Text-cursor follow-ups** — hide the cursor while the user tracks a frame; unverified: one cursor per message queue (the reference's "destroys any existing one" may mean one in the system), `CURSOR_SETPOS` from a window that does not own the cursor returning FALSE, a negative width / height extending left / down, `rclClip` reported as the window rectangle when none was given, `WinQueryCursorInfo` reading the caller's queue, no `WM_TIMER(TID_CURSOR)` posted to the application for `CURSOR_FLASH`
- [ ] **Rectangles and WinDrawText: unverified details** — WinPtInRect leaving the right and top edges out, (0, 0, 0, 0) as the empty result of WinIntersectRect / WinUnionRect / WinSubtractRect, NULL pointers failing without an error code, coordinates not limited to 16 bits; WinDrawText's count with `DT_WORDBREAK` including trailing blanks and the line end, nothing drawn with `DT_QUERYEXTENT`, the inclusive extent it writes, `DT_ERASERECT` using the background colour, `DT_HALFTONE` as the mean colour, `~` as the `DT_MNEMONIC` prefix, no clipping to the rectangle, `DT_EXTERNALLEADING` adding nothing
- [ ] **Profile follow-ups** — `PrfQueryProfile`, `PrfReset`, the program-list calls (`PrfAddProgram`, …), the 16-bit `PRF16*` / `WIN16*` exports; reading binary OS/2 INI files shipped with programs (format not publicly documented; refused as corrupt now). Unverified: truncated `PrfQueryProfileData` returning TRUE with the bytes copied, a truncated string's last byte forced to NUL, list sizes with both NULs but counts without the last, `HINI_PROFILE` enumeration merging user and system names, deleting what does not exist succeeding, deleting the last key removing the application, a NULL application on a write rejected, integer overflow in `PrfQueryProfileInt` wrapping, the 64 KB value limit, `PrfOpenProfile` creating a missing file and returning the same handle on a re-open, `PrfCloseProfile(HINI_USER/SYSTEMPROFILE)` failing, `PMERR_INVALID_HWND` / `PMERR_INVALID_PARM` from the help calls, the owner-chain walk of `WinQueryHelpInstance`
- [ ] **Critical-section, timer and module follow-ups** — unverified: a thread ending inside a section ends it, held threads end when the process ends or they are killed, `ERROR_INVALID_THREADID` never returned (calls from exception handlers), the 32 ms timer tick and an interval of 0 as one tick, the 64-timer limit, a one-shot handle invalid after it posted, a closed semaphore stopping its timer, "shared" meaning `DC_SEM_SHARED` or named; `DosSearchPath` skipping elements that do not resolve whatever `SEARCH_IGNORENETERRS`, matching files only (hidden and system included), keeping the caller's spelling; `DosQueryModuleName` reporting module handle 0 as the executable, `C:\NAME.EXE`, built-in modules as `C:\OS2\DLL\NAME.DLL`
- [ ] **Suballocation follow-ups** — shared pools between processes; unverified: the error codes chosen for unknown flags, small or unset pools, zero / oversized blocks, unknown pools and blocks off the 8-byte grid, first-fit allocation, the 1024-pool and 65536-free-range limits, `DOSSUB_SPARSE_OBJ` only recorded and `DOSSUB_SERIALIZE` changing nothing
- [ ] **Painting follow-ups** — true update regions (GPI regions exist and the window-region calls convert, but a window's update area is still one rectangle, and the `WinBeginPaint` clip is that rectangle), a child window's PS clipped to its ancestors too (only to the child now, ADR-0048), `CS_SYNCPAINT`, `DB_RAISED` / `DB_DEPRESSED` if a source documents them; unverified: `WinBeginPaint` with nothing to update drawing unclipped (Warpine's choice), `WinValidateRect` removing only bands off an edge, (0, 0, 0, 0) from `WinQueryUpdateRect` / `WinBeginPaint` for a valid window, `WinScrollWindow` clipping to the scroll rectangle without a clip rectangle and failing for `HWND_DESKTOP`, `WinDefWindowProc(WM_PAINT)` validating the window only, a border without `DB_AREAATTRS` in solid `clrFore`, `DB_DLGBORDER` details, negative border widths as 0
- [ ] **Window-lookup, input-state and colour follow-ups** — the desktop background, menus and control faces should read `SYSCLR_*` too; messages Warpine gives a subclassed control directly (repainting after `WinEndPaint`, `WinDefDlgProc` pressing a button) passing through the subclass procedure; `WM_SETFOCUS` sent rather than posted (ADR-0048); a synchronous key state per queue (one per process now); CapsLock / NumLock toggle state from the host at start; dialogs placed under a client window positioned frame-relative by `WinSetWindowPos`; disabled buttons not drawn greyed. Unverified: `WinWindowFromPoint` skipping hidden but not disabled windows and testing only the Z-order on the desktop, the `PMERR_*` codes chosen for bad counts / pointers / keys / `cchMax`, `WinEnumDlgItem` eligibility (visible and enabled, `WS_TABSTOP` for tab items), the class styles and window procedure `WinQueryClassInfo` reports for built-in classes, case-sensitive class names, `HWND_DESKTOP` from `WinQueryDesktopWindow`, a double click counting as a press, `PMERR_NO_MSG_QUEUE` from `WinQueryMsgPos` / `WinQueryMsgTime`, `WM_SYSCOLORCHANGE` posted rather than sent, `LCOL_PURECOLOR` ignored
- [ ] **Signal follow-ups** — calls that are still not interruptible (ADR-0035, ADR-0048): a pipe write blocked in the middle of a frame, a `send` larger than the socket's free room after its wait, `connect`, host file I/O, MMPM/2 waits, CMD.EXE's child waits, all calls of 16-bit programs; signals in the modal dialog loop (a kill ends it); Ctrl+Break at a terminal; signal focus across the processes of a session; must-complete sections (`DosEnterMustComplete`); `ERROR_INVALID_PROCID` handling for PM programs; a GDB stop request kicking the vCPUs
- [ ] **Pipe follow-ups** — `DosRawReadNPipe` / `DosRawWriteNPipe` (no public description found); muxwait semaphores as pipe semaphores and a real `NPSS_WAIT` flag (ADR-0037); stale `warpine-threads-*` session directories of killed sessions (pipe directories are swept); echo of piped standard input
- [ ] **GPI follow-ups** — pair kerning from the fonts' GPOS tables (IBM Plex kerns pairs such as "To", "AV", "r."; `ab_glyph` reads no GPOS, and applying it changes text widths, so `GpiQueryTextBox` / `GpiQueryWidthTable` and control layouts with it), `GpiSetLineEnd` / `GpiSetLineJoin` (strokes use square ends and joins), outline-font text inside paths, elliptic, rounded-rectangle and polygon region constructors (`GpiCreateEllipticRegion` …), GPI window origins for child-window presentation spaces
- [ ] **Control follow-ups** — menus: `WM_MEASUREITEM` for owner-drawn items and redrawing on highlight changes; `WM_MEASUREITEM` on deletion and partial `WM_DRAWITEM` (`fsStateOld`); whether a dropped combination-box list may reach outside its parent window (unverified; ADR-0025); unverified spin-button wrap at the limits and case-insensitive array matching
- [ ] **Hook and messaging follow-ups** — `MSGF_TRACK` (frames are tracked by the GUI thread, not a guest loop) and input hooks (`HK_INPUT`) in the modal loops, the help hook of a message box, journal hooks and the other hook types (stored, never called), hooks across OS/2 processes (ADR-0026), sent messages processed on the receiving thread (so `WinInSendMsg` answers there), `WinQueryQueueStatus` / `WinSetMsgInterest`; frame windows carry the client's window procedure, so a `BMSG_FRAMEONLY` send reaches the client procedure with the frame's handle
- [ ] **Frame follow-ups** — `SWP_ZORDER` and `hwndInsertBehind` in a subclassed frame's `WM_FORMATFRAME` answer (positions, sizes and visibility are applied, ADR-0048); `QWS_FLAGS` (`FF_FLASHWINDOW` …); the alarm of `WinFlashWindow`; `WM_MINMAXFRAME` from `WinSetMultWindowPos`
- [ ] **Presentation-parameter and painting follow-ups** — `SS_TEXT` with `DT_WORDBREAK`; sending `WM_PRESPARAMCHANGED` instead of posting it; `PP_HILITE*` / `PP_DISABLED*` colours; presentation parameters for title bars and menus; row heights that follow a larger `PP_FONTNAMESIZE`; sliders and value sets ignore presentation parameters; `SS_ICON` / `SS_BITMAP` from the dialog's own module; locking one window instead of its whole frame; `WS_CLIPSIBLINGS` / `WS_PARENTCLIP` clipping; `CS_MOVENOTIFY` / `WM_MOVE`; `WinEnableWindowUpdate` (a stub)
- [ ] **Slider and value-set follow-ups** — dragging the arm with button 2, `WC_CIRCULARSLIDER`; value-set drag and drop (`VN_INITDRAG` / `VN_DRAGOVER` / `VN_DROP` / `VN_DROPHELP`), `VN_HELP`, `VDA_ITEMBACKGROUND` / `VDA_SURROUNDING`; a `wrc` dialog with `CTLDATA` in a sample; all sizes and colours are Warpine's own (unverified)
- [ ] **GPI state, coordinate and text follow-ups** — geometric line widths, glyphs, the character box and blitted images are not scaled or rotated by the transforms; rotated viewing limits and clip rectangles become bounding boxes; separate colour and mix per primitive type; segments (`GpiSetSegmentTransformMatrix`, `GpiCallSegmentMatrix`); `GpiSetDefViewingLimits` / `GpiSetDefAttrs`; whether `WinDrawText` / `WinFillRect` follow the transforms; `GpiCharBox` width scaling and `GpiQueryCharBox`; `GpiTabbedCharStringAt`; image fonts ignoring angle and shear under `CM_MODE1`; text inside path and area brackets
- [ ] **Application, switch-list, atom and DDE follow-ups** — the switch list, system atoms and DDE across Warpine processes (a session-wide host channel and probably a shared desktop; ADR-0030); `WinStartApp` `SAF_MINIMIZED` / `SAF_MAXIMIZED` / `SAF_BACKGROUND`, `swpInitial` and the title, ending `SAF_STARTCHILDAPP` programs with their parent; `WinSwitchToProgram` restoring minimized frames; switch-list titles following `WinSetWindowText`; `WinQuerySessionTitle`, `WinQueryTaskTitle` / `WinQueryTaskSizePos`, `WinSwitchProgramRegister`; reading real `.MSG` files
- [ ] **Standard dialog and printing follow-ups** — a default error box when `FDM_ERROR` returns 0, filtering the style list, `FNTM_COLORCHANGED` / `FNTM_UPDATEPREVIEW` (need colour UI and an owner-drawn preview); unverified: the mp2 parameters, the `FDM_ERROR` ids, the `FNTI_*` bits and the `STYLECHANGE` layout (the Open Watcom headers lack the struct); the read-only check box, overwrite confirmation on Save As; emphasis in the font sample; `WinRegisterCircularSlider` (PMCTLS 24); a PMCTLS section for `doc/os2_ordinals.md`; real printing (a PostScript / PDF spooler)
- [ ] **CONFIG.SYS follow-ups** — confirm the `THREADS=` minimum (sources say 32 or 64; 64 here) and the parsing rules (duplicates, comments) against the IBM Command Reference; other statements could use `config_sys.rs` (`LIBPATH`, `MAXWAIT`, `SET`, …)
- [ ] **Compatibility-chart data** — date the 22 `unknown` exports (mostly undocumented GPI ordinals 658–730) and check the `H1` ordinal-chronology lines and Warp 3 DOSCALLS (the 1996 table includes fixpaks) against a Warp 3 Control Program reference; full export lists for the DLLs outside `os2386.lib` (UCONV, MMPM/2, TCP/IP, PMCTLS, MSG) so Warp 4's additions can be counted (`doc/compat/SOURCES.md`, ADR-0033)
- [ ] **Process-id and entry follow-ups** — NE programs: the 16-bit entry registers (AX = environment selector, BX = command-line offset) and real 16-bit `DosGetEnv` / `DosGetPID`; exit codes reach the parent through the host process, so only 8 of OS/2's 16 bits arrive; `DosSendSignalException` to the parent or other processes of the session (ids are session-unique but only one's own children have a host-pid map, ADR-0036); `DosWaitChild` after `DosKillProcess`
- [ ] **Help viewer** — an INF / HLP reader (the IPF format is not published by IBM: check osFree / NewView descriptions for clean-room use first), `RT_HELPTABLE` / `RT_HELPSUBTABLE`, F1 → `WM_HELP` → the help-table panel, the `HM_*` notifications back to the program, the `Ddf*` calls

---

## Application compatibility

- [ ] **Socket `select` follow-ups** — `select` reads the OS/2 TCP/IP count-and-array `fd_set` of Open Watcom's headers (ADR-0041); still open: the real SO32DLL / TCP32DLL ordinals of `select`, `bsdselect` (BSD bit mask, `TCPV40HDRS`) and `os2_select` (an array of socket numbers with read / write / exception counts and a timeout in milliseconds, not-ready entries set to -1), which no public source we found lists, and whether the BSD 4.3 stack's `select` took a bit mask
- [ ] **Thread follow-ups** — ending a thread ends only that thread (`threads.rs`); still missing: `DosKillThread` interrupting the calls that are still not interruptible (see Signal follow-ups); unverified: a thread's windows destroyed without `WM_DESTROY` when it ends (ADR-0048), thread 1 ending with `DosExit(EXIT_THREAD)` while other threads run (the reference only says "do not end thread 1 without ending the process"; here the process goes on until its last thread ends), the exit code when thread 1 returns from the entry point (unchanged here, 0 unless set), `DosWaitThread(0)` when no thread that could end is left (`ERROR_INVALID_THREADID` here, instead of waiting forever), an ended thread reported once and then `ERROR_INVALID_THREADID`, `DosKillThread` on the calling thread (`ERROR_INVALID_THREADID` here), a thread's message queue destroyed when the thread ends, thread ids not reused

### 16-bit (NE) programs

NE execution works (`ne_hello` runs end-to-end). Open work:

- [ ] **Watcom CRT NE apps** — Watcom 16-bit CRT requires LDT-based selectors (TI=1); would need stub LDT or full LDT emulation
- [ ] **Mode switching** — 16-bit NE code calling a 32-bit flat DLL
- [ ] **Broader 16-bit API coverage** — more DOSCALLS / VIOCALLS / KBDCALLS ordinals beyond minimal hello-world

### REXX interpreter bridge

- [ ] Bridge `REXXAPI.DLL` exports (`RexxStart`, `RexxRegisterSubcomDll`, `RexxVariablePool`) to [Regina REXX](http://regina-rexx.sourceforge.net/)
- [ ] Unlocks: OS/2 install programs, system tools, 4OS2 `.cmd` scripts

### Date and time follow-ups

The 2038 audit is done (ADR-0039); `DosGetInfoSeg` has real information segments (ADR-0040). What remains:

- [ ] **4OS2's `DosGetInfoSeg` and other 16-bit calls through Far16 thunks** — `DosGetInfoSeg` fills real segments (ADR-0040), but a 32-bit program calling a 16-bit entry through a Far16 thunk (4OS2's `APIENTRY16` declaration) never reaches it: `vcpu.rs` bypasses the thunk and returns 0. Such programs also turn selectors into pointers with OS/2's tiling formula `(sel >> 3) << 16`, while Warpine's tiled selectors start at GDT index 6 (`TILED_SEL_START_INDEX`), so the pointer would miss by six tiles. Needs: dispatching thunked 16-bit calls to their handlers, and a tiling that matches OS/2's (or LDT selectors)
- [ ] **File attributes: hidden, system, archive** — `DosSetFileInfo` / `DosSetPathInfo` / `DosSetFileMode` apply `FILE_READONLY` (the host write bits; directories are left writable) but have nowhere to keep the other attributes; an extended attribute or the sidecar could hold them, which `DosQueryPathInfo` and `DosFindFirst` would then report
- [ ] **A clock shared by the session** — `DosSetDateTime` moves the clock of the calling process and of the children it starts afterwards, not of processes already running (ADR-0039)

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

### Session desktop server (prerequisite for cross-process PM and the WPS)

Each OS/2 process is its own Warpine host process (ADR-0020) with its own SDL window and PM desktop, so window handles, z-order, focus, the switch list, system atoms and DDE stay inside one process (ADR-0030), and a program started with `WinStartApp` opens a second desktop window.

- [ ] **One desktop per session** — one Warpine process (or a small server) owns the SDL window, the compositor, input routing and a session-wide window table; the other Warpine processes of the session connect over a private socket in the session directory (as clients to an X server or Wayland compositor), send their frame surfaces and window-manager requests, and receive their input and messages
- [ ] **Session-wide window handles** — unique across processes, so cross-process `WinSendMsg` / `WinPostMsg`, `WinQueryWindowProcess`, z-order and focus between programs work
- [ ] **Shared shell services on top** — one switch list and Ctrl+Esc window list, the system atom table, DDE between programs, `WinStartApp` children on the same desktop (supersedes ADR-0030)
- [ ] **Decisions to record** — the protocol and its security (a private, mode-0700 socket; every message from a client treated as hostile), what happens when the server process ends, and whether this reverses ADR-0020 for PM programs; an ADR before implementation

### SOM / Workplace Shell

The Workplace Shell is an ordinary PM application on top of the session desktop server above: its folders and the programs it starts must share one desktop.

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
- [ ] **MZ stub in front of the XE header**, as NE / LE / LX / PE have it (record it in the spec and an ADR). Every XE file starts with a valid MZ executable whose `e_lfanew` (offset 0x3C) points to the XE header, aligned to 8 bytes; the stub prints a compatibility warning, e.g. "This program requires a 64-bit OS/2 (Warpine XE).", and exits (INT 21h AH=09h, then AH=4Ch). Why:
  - MS-DOS runs the stub instead of treating a file without `MZ` as a `.COM` program and executing the header bytes;
  - real OS/2, or a Warpine without XE, finds the unknown signature at `e_lfanew` and refuses cleanly with `ERROR_BAD_EXE_FORMAT` (193);
  - `detect_format()` keeps one path, MZ → `e_lfanew` → signature;
  - tools such as `file` still recognise an executable.

  The XE linker reuses `lx_link`'s stub writer. Optional, for the spec to decide: a "fat" executable carrying both a 32-bit LX and a 64-bit XE program, which needs a second header pointer because MZ has only one `e_lfanew`.
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
