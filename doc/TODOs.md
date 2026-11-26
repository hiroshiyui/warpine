# Warpine TODO List

Open work only, highest priority first. Finished work is in the [CHANGELOG](../CHANGELOG.md), the [Developer Guide](developer_guide.md), the [Reference Manual](reference_manual.md) and the [ADRs](adr/README.md); unconfirmed behaviour in [Unverified behaviour](unverified.md). Everything follows the near-clean-room policy (CLAUDE.md). Items marked 🚧 wait on a source: see [Blockers](#blockers).

## Security

- [ ] **No-execute guest memory**: guests use 32-bit paging without PAE, so every readable page is executable. PAE paging with `EFER.NXE` would mark stacks, heaps and LX data objects non-executable (`PAG_EXECUTE` stays runnable). Needs an ADR: PAE tables are twice the size, which changes the paging arena and its 736-thread budget (ADR-0088, 0114, 0115).
- [ ] **Per-application permissions**: a manifest per program naming the drives, network and devices it gets, narrowing Landlock / seccomp per process (ADR-0066, 0111, 0124); the desktop server asking the user when a program reaches for more.
- [ ] **Host confinement** (Landlock ADR-0111, seccomp ADR-0124): `ci/check_desktop.sh` passed on Wayland with PipeWire and NVIDIA (2026-11-25: PM and VIO windows and sound, the desktop server; the shader cache and `/proc` thread naming stay refused, ADR-0111 addendum); still to run on X11, native PulseAudio and other GPU drivers, widening the policy where it reports a difference; namespaces only if those leave gaps.
- [ ] **Audit 2026-11-25, left**: the shared arena's residual windows (check-then-copy against a free on another thread; TLB flushes do not wait for every vCPU); single-value `guest_read`s of guest-named addresses still reach Warpine's protected memory (Warpine's own reads need them), so a call that echoes one back can leak a word of it.
- [ ] **Fuzzing** (ADR-0112): font and help-file readers once Warpine reads guest-supplied ones (none today).
- [ ] **Guest ASLR**, opt-in (`WARPINE_ASLR`): a random start of each process's allocator region, when Warpine hosts network servers (ADR-0125's revisit condition).

## SOM / Workplace Shell

The SOM kernel (ADR-0059–0063, 0070–0073, 0122, 0126) is done; the WPS is to be a PM application on it. Sources: the Warp 4 Toolkit's SOM and WPS guides and references, IBM's OS/390 SOMobjects books and the SOMobjects 3.0 Toolkit books (`doc/references/`, never committed; URLs in the README). The object model is SOM (ADR-0127).

- [ ] **Kernel calls missing**: `somDispatchD` (a `double` in the FPU), `somAddDynamicMethod` and dynamic methods; applying IBM-built classes (their apply-stub protocol is not public).
- [ ] 🚧 **IBM's SOM.DLL ordinals 48+ and control blocks**: Warpine's own ordinals (3072+, ADR-0123) serve programs built from source; IBM-built binaries need IBM's ordinals and initializer / destructor / assignment control blocks. Unused by `somBuildClass` (ADR-0073): DirectToSOM classes, migrated methods, `somDynamicSCI`, `inheritVars`, `somConstructClass`. An interface repository (`somInterfaceRepository` is NULL).
- [ ] **`somidl`**, if needed: OIDL `.csc` input, an `ir` emitter and interface repository, `TC_<Name>` TypeCode constants for declared types (the kernel has TypeCodes, ADR-0128), DSOM.
- [ ] **Smaller**: C++ SOM objects on the stack and in static storage; `dllname` for `somLocateClassFile`; `SOMU` classes, `SOMIR`, IDL metadata.
- [ ] **WPS classes** (ADR-0129: guest code in the SDK's `libwps.a`): the rest of `WPObject` (views — `wpOpen`, `wpClose`, `wpViewObject`; icons; pop-up menus; `wpCopyObject` / `wpMoveObject`; file objects' handles (`wpQueryHandle`, `WinQueryObject` of a path), `WPDataFile`'s own methods; `WPFolder`'s views (icon, tree, details on `WC_CONTAINER`), icon positions, sorting, `wpclsQueryFolder`'s system folder IDs (`<WP_DESKTOP>` …), unique names for new files; `WinCreateObject`'s locations — folders' contents in the user profile; `WinRegisterObjectClass` / `WinEnumObjectClasses`; several threads at once, and a cross-process lock on the profile's handle counter); `WPDesktop`, `WPProgram`, `WPDataFile`; settings notebooks; `wpDragOver` / `wpDrop`; a Workplace Shell process that makes every recorded object at its start and takes the `Win*Object` calls (ADR-0130).
- [ ] **A system message bus on SOM**: notifications, settings changes and "open this file with…" requests as SOM objects between processes, like D-Bus (ADR-0127); needs an ADR.
- [ ] **Self-contained application bundles**: installed by dropping in a folder, no `CONFIG.SYS` `LIBPATH` editing, shown by the WPS as objects; needs an ADR.

## Kernel and processes

- [ ] **Shared memory** (ADR-0049): the 16-bit `DosAllocShrSeg` / `DosGetShrSeg` (deferred: they need a 16-bit segment allocator first, LDT selectors over the shared arena — see 16-bit (NE) below); blocks of a killed process whose parent never waits, swept at the next table use.
- [ ] **Semaphores** (ADR-0050): limits of 512 per session, 28 processes per semaphore.
- [ ] **Signals** (ADR-0035, 0093): interrupting anonymous-pipe writes and host I/O on FIFOs and devices; signal focus across the session; `ERROR_INVALID_PROCID` for PM programs.
- [ ] **Pipes**: muxwait semaphores as pipe semaphores, a real `NPSS_WAIT`; echo of piped standard input.
- [ ] **Suballocation**: shared pools between processes.
- [ ] **Extended attributes by handle**: `DosSetFileInfo` level 2 and `DosQueryFileInfo` level 3 (they need the path behind a handle); `DosQueryPathInfo` level 3 checking its `FEA2LIST`'s `cbList` before it writes (it writes past a too-small buffer, in the program's own memory); `DosOpen`'s `EAOP2`.
- [ ] **Threads**: the paging arena caps a process at 736 threads (ADR-0115), below `THREADS=`'s 4095; allocator blocks would lift it.
- [ ] **Large files**: `DosOpenL`, `DosSetFilePtrL`, `DosSetFileSizeL`, `DosProtect*L`, `DosSetFileLocksL` and 64-bit sizes in `DosQueryFileInfo` level 11 (`DosQueryProcAddr` answers `ERROR_PROC_NOT_FOUND` for them today).
- [ ] **Asynchronous I/O**: a completion-based interface (like io_uring or completion ports) as `Wrp*` calls in `WARPINE.DLL`, over threads and event semaphores.
- [ ] **UTF-8 for new programs**: an opt-in per program making codepage 1208 its process codepage, so text passes through as UTF-8 (ADR-0008); old programs keep theirs.
- [ ] **Services**: a service manager (start on demand, restart on failure, ordered dependencies) in place of `CONFIG.SYS`'s `RUN=` lines; needs an ADR.
- [ ] **Profiles**: program-list calls, `PRF16*` / `WIN16*` exports; 🚧 binary OS/2 INI files.
- [ ] **CONFIG.SYS** (ADR-0032, 0092): 🚧 `THREADS=` range and parsing against `CMDREF.INF` (secondary sources agree on 64–4095, default 64, but not on clamping, case, spaces, `REM`); `MAXWAIT`, `DPATH`.
- [ ] **Compatibility chart** (ADR-0033): date the `unknown` exports (22 of os2386.lib, 7 elsewhere); check the Warp 3 data and dates (SOM 2.0, the VIO / MOU threshold, `VioGlobalReg`); 🚧 the SO32DLL / TCP32DLL split; chart Open32 (PMWINX), OpenGL, DIVE, MMIO, MCIAPI, LIBUNI, TCPIP32.

## Presentation Manager

**Controls**
- [ ] **Containers** (ADR-0043–0046, 0065, 0067): keep an edit when focus leaves the MLE without a click; `CA_OWNERPAINTBACKGROUND`; grid squares (`GRIDINFO.pGrid`, `CN_GRIDRESIZED`); records shared between containers.
- [ ] **Embedded dialogs** (ADR-0042, 0069): a working title bar.
- [ ] **Drag and drop** (ADR-0045, 0057, 0068): `DrgAcceptDroppedFiles`, `DrgQueryFormat`; lazy drag (`DrgLazyDrag`, `DrgLazyDrop`, `DrgCancelLazyDrag`, `CN_PICKUP`, `CN_DROPNOTIFY`); `DRG_POLYGON` / `DRG_TRANSPARENT` images; `DM_DROPHELP` / `CN_DROPHELP` / `VN_DROPHELP`; `WM_BEGINDRAG` from the keyboard; host drop positions on real displays.
- [ ] **Menus**: redraw an owner-drawn item on mouse highlight; `MIA_NODISMISS`.
- [ ] **Sliders, value sets**: `CSN_QUERYBACKGROUNDCOLOR`; `VN_HELP`, `VDA_ITEMBACKGROUND` / `VDA_SURROUNDING`; a `wrc` dialog with `CTLDATA` in a sample.
- [ ] **Text controls**: `WinSetSysValue` beyond the cursor and scroll rates; hide the cursor while a frame is tracked.

**Dialogs, menus, message boxes**
- [ ] **Dialogs**: `WM_ACTIVATE` when the active window is destroyed; confirm `WinProcessDlg` leaves its dialog to the caller.
- [ ] **Menus in dialogs**: `WM_UPDATEFRAME(FCF_MENU)` and its other flags; `WinSetParent` / `WinSetOwner` of a menu.
- [ ] **Message boxes**: the `HK_HELP` hook; `MB_SYSTEMMODAL` as system modal.
- [ ] **Standard dialogs**: a default error box when `FDM_ERROR` returns 0; style-list filtering, `FNTM_COLORCHANGED` / `FNTM_UPDATEPREVIEW`; the read-only check box, overwrite confirmation, sample emphasis; a PMCTLS section in `os2_ordinals.md`.
- [ ] **Help viewer**: 🚧 an INF / HLP reader (check osFree / NewView first); `RT_HELPTABLE` / `RT_HELPSUBTABLE` and F1 panels; `HM_*` notifications, `Ddf*`.

**Frames, input, messages**
- [ ] **Frames**: cascades deeper than one level, opening on hover; `MM_SETITEM`, `MM_SETITEMHANDLE`, `MM_SELECTITEM`, `MM_QUERYITEMRECT`, `MM_QUERYSELITEMID`, default-item messages; minimized icons (`FCF_ICON`, `WinSetIcon`), min/max for dialogs; `WinSetActiveWindow` on minimized or disabled frames; `SWP_ZORDER` in a subclassed `WM_FORMATFRAME`; `QWS_FLAGS`, `WinFlashWindow`'s alarm, `WM_MINMAXFRAME` from `WinSetMultWindowPos`.
- [ ] **System and pop-up menus**: the greyed Window list item; Alt+F11; a minimized frame's menu on click; `WM_TRACKFRAME` / `WM_QUERYTRACKINFO` for subclassed frames (ADR-0089 phase 2), pointer taking over keyboard tracking, `WinTrackRect` on the desktop; `WinPopupMenu` submenus and `PU_*` flags.
- [ ] **Input**: the horizontal wheel; `WM_HITTEST` for frame-chrome clicks.
- [ ] **PM errors**: 🚧 `PMERR_INVALID_FLAG` from `WinQueryWindowPos` and `WinCreateWindow`'s parameter errors; `PMERR_CANNOT_SET_FOCUS`; 🚧 `WinGetErrorInfo` / `WinFreeErrorInfo`.
- [ ] **Hooks**: `MSGF_TRACK` and `HK_INPUT` in modal loops; journal and other hook types (stored, never called), hooks across processes; `WinSetMsgInterest`; `BMSG_FRAMEONLY` reaching the client with the frame's handle.
- [ ] **Windows and colours**: desktop, menus and control faces reading `SYSCLR_*`; direct messages to subclassed controls passing their subclass; `WM_SETFOCUS` sent, not posted; key state per queue, CapsLock / NumLock from the host; dialogs under a client placed frame-relative; other disabled controls greyed, `PP_DISABLEDFOREGROUNDCOLOR`.
- [ ] **Presentation parameters**: `SS_TEXT` with `DT_WORDBREAK`, `WM_PRESPARAMCHANGED` sent; `PP_HILITE*` / `PP_DISABLED*`, parameters for title bars, menus, sliders, value sets; row heights following `PP_FONTNAMESIZE`; `SS_ICON` / `SS_BITMAP` from the dialog's module; locking one window, `WinEnableWindowUpdate` (stub); `WS_CLIPSIBLINGS` / `WS_PARENTCLIP`, `CS_MOVENOTIFY` / `WM_MOVE`; 🚧 `DB_RAISED` / `DB_DEPRESSED`.
- [ ] **Applications, switch list**: `WinStartApp`'s `SAF_MINIMIZED` / `SAF_MAXIMIZED` / `SAF_BACKGROUND`, `swpInitial`, title; `SAF_STARTCHILDAPP` children ending with the parent; `WinSwitchToProgram` restoring minimized frames, titles following `WinSetWindowText`; `WinQuerySessionTitle`, `WinQueryTaskTitle` / `WinQueryTaskSizePos`, `WinSwitchProgramRegister`; real `.MSG` files; a real Window List window.
- [ ] **Desktop server** (ADR-0052–0057): text-mode programs keep their own window; software compositing only; a crashed server should end the session's PM programs; a program not reading input stalls everyone for 5 s; pointers over 128 × 128 show as the arrow; the clipboard between programs is asynchronous (the race ADR-0108 fixed for activation).

**Graphics**
- [ ] **GPI**: `GpiSetPatternSet`, rounded box corners; `GpiSetDefArcParams` / `DefAttrs` / `DefViewingLimits`; palettes (`GpiCreatePalette`, `LCOLF_PALETTE`); 16-bit and `BCA_HUFFMAN1D` bitmaps, compressed `GpiQueryBitmapBits`; `GpiSetLineEnd` / `LineJoin`; elliptic, rounded and polygon regions; segments; `GpiTabbedCharStringAt`, `GpiQueryCharBox`, char-box width scaling; outline text in paths and areas; transforms for geometric line widths, glyphs, the char box and blits; per-primitive colour and mix; window origins for child-window PSs; GPOS pair kerning.
- [ ] **Printing**: printer and metafile DCs (only `OD_MEMORY` / `OD_INFO` now), a real spooler (PostScript / PDF).
- [ ] 🚧 **DIVE**: framebuffer blitting onto the per-window surfaces, with DART audio.
- [ ] 🚧 **OpenGL / PGL**: GL 1.x forwarded to the host's compatibility profile; PGL contexts and swaps, guest pointers in array and pixel calls, an offscreen framebuffer as the window's texture.

**Rendering** (measure with `RUST_LOG=warpine::gui=debug` and `samples/pm_bench`)
- [ ] Idle CPU and input latency on real X11 / Wayland; damage smaller than a window; a moved window's old and new rectangles damaged apart; present a `WinBeginPaint` PS only at `WinEndPaint`; HiDPI (agree the approach first); a shared glyph atlas if profiling asks for it.
- [ ] **Modern rendering**: alpha (translucent windows and bitmaps) and anti-aliased text in every control; GPU composition through SDL3's renderer; then bring back the modern theme from `feature/modern-theme` (ADR-0118, 0121) once the PM widgets are done.

## Desktop integration

Features for people using Warpine day to day. Each PM program keeps drawing on Warpine's own desktop (ADR-0011); one host window per PM window is not planned.

**Input methods** (the host's fcitx5 / IBus through SDL3: committed text already reaches `WM_CHAR` in each program's codepage, DBCS ones included, and the caret's area goes to `SDL_SetTextInputArea`)
- [ ] **Inline preedit**: draw the text being composed (SDL's `TextEditing`, ignored today) at the caret, underlined with the input method's cursor, in entry fields, MLEs, combo boxes and VIO windows, through the desktop server too; a GUI-script test simulating composition.
- [ ] **Per-control switching**: the input method on in text controls, off in password fields, other controls and programs reading raw keys (start and stop on focus changes).
- [ ] **Candidate placement everywhere**: VIO windows, the out-of-process desktop server, scrolled controls, HiDPI, several monitors.
- [ ] 🚧 **An input method API for programs**: query and switch the input mode, as OS/2's DBCS editions offered; public documentation first, otherwise Warpine's own calls in `WARPINE.DLL` (an ADR).
- [ ] **Input methods as OS/2 programs** (optional, after the above): a framework for guest-side input method engines.

**Host and session**
- [ ] **Launching from the host**: a MIME type for LX executables, so the file manager starts them; optionally a `binfmt_misc` registration; `.desktop` menu entries with the program's icon taken from its LX resources.
- [ ] **A configuration program**: a PM program for drive mappings, codepage and country, theme, sound, the desktop window size and per-application permissions, in place of environment variables and text files; needs an ADR for where the settings live.
- [ ] **A crash dialog**: when a program traps, a dialog naming the program and the exception, with *Close*, *Show details* and the crash dump's path (`$XDG_STATE_HOME/warpine`).
- [ ] **Host drag and drop and portals**: host files dropped onto PM programs and the desktop; host documents and URLs opened, and notifications shown, through XDG desktop portals, each with the user's consent (ADR-0066).
- [ ] **Host fonts**: fontconfig's fonts offered to PM programs, read-only through the sandbox policy (ADR-0012, 0111, 0119).
- [ ] **Translations**: Warpine's own text (system menus, CMD messages, standard dialogs, error boxes) in other languages, following the country setting.
- [ ] **Accessibility**: an AT-SPI bridge so screen readers read PM controls; a high-contrast theme; checked keyboard navigation in every control.
- [ ] **Installing software**: WarpIN archives (`.wpi`; licence first) and a simple install from a ZIP archive.
- [ ] **Session restore**: reopening the last session's programs where their windows were (extends PMWP's window positions).
- [ ] **A Warpine login session, level 1**: the PM desktop full screen (only VIO has `WARPINE_VIO=fullscreen` today) under the kiosk compositor cage, started from a `wayland-sessions/warpine.desktop` entry and a start script, so the login screen offers "Warpine"; cage keeps the display, input and seat, so the sandbox stays as it is.
- [ ] **A Warpine login session, level 2**: the Workplace Shell as the session's shell; log out, restart and shut down through logind; a screen lock; display, audio and network settings in the configuration program; a desktop server that restarts and recovers instead of ending the session.

## Console and multimedia

- [ ] **CMD.EXE** (ADR-0075–0080, 0082): START's sessions on the shared desktop; further OS/2 commands case by case.
- [ ] **TCP/IP** (Warpine's ordinals, ADR-0123): `_res` state and options, TCP for answers over 512 bytes, `res_mkquery`'s other ops, `dn_find`, `dn_comp` compression, `%ETC%` files; `sendmsg` / `recvmsg`, `so_cancel`, `addsockettolist` / `removesocketfromlist`, `rexec`, SOCKS (`Raccept`, `Rbind`, `Rconnect`, `Rlisten`, `Rgetsockname`, `Rgethostbyname`); 🚧 `select`, `bsdselect`, `os2_select` ordinals and layouts (ADR-0041).
- [ ] **KBD**: `KbdStringIn` template recall and TERM mode; `KbdSetCustXt`, `KbdRegister` / `KbdDeRegister`, `KbdSetFgnd`, `KbdSetHWID`, non-US layouts (`KbdXlate`).
- [ ] **VIO**: `VioGetFont` / `VioSetFont` (needs a free bitmap font of Warpine's own); `VioGetBuf` / `VioShowBuf` (stubs); `VioSetState` / `VioSetCp` in the renderer; `VioRegister` / `VioDeRegister`, `VioGlobalReg`, `VioPrtSc` / `VioPrtScToggle`, `VioModeWait` / `VioModeUndo`, `VioSavRedrawWait` / `VioSavRedrawUndo`, `VioGetPhysBuf`; several VIO sessions on one desktop; Advanced VIO (`VioCreatePS`, `VioAssociate`).
- [ ] **MOU**: MOUCALLS for NE programs; calls passed to a registered subsystem.
- [ ] **ANSI**: keyboard reassignment `ESC[…p`.
- [ ] **Audio**: sound in headless runs; `MCI_RECORD` from SDL3; the `capability` command (`mciSendString` answers "unknown verb", as `audio_test` shows); MIDI (deferred).

## Application compatibility

- [ ] **16-bit (NE)**: the Watcom 16-bit CRT (LDT selectors); 16-bit code calling 32-bit DLLs; more DOSCALLS / VIOCALLS / KBDCALLS ordinals; entry registers (AX environment, BX command line); real 16-bit `DosGetEnv` / `DosGetPID`.
- [ ] **Far16 thunks**: dispatch 16-bit calls through thunks (now bypassed, returning 0, e.g. `DosGetInfoSeg`); OS/2's tiling `(sel >> 3) << 16`.
- [ ] **REXX** (ADR-0081, 0083–0085): the macro space (stubs), tokenized images, `RXFNC` / `RXCMD` / `RXMSQ` / `RXTRC` exits, 16-bit entry points, system-wide queues and registrations, `RxFuncAdd` of DLL packages; REXXUTIL screen and keyboard functions, EAs, INI files, WPS objects; `TRACE ?`; a separate error output.
- [ ] **Session clock**: `DosSetDateTime` reaching running processes (ADR-0039).

## Development toolchain

Open Watcom stays the ABI baseline; a modern toolchain enters only through stable formats (i386 ELF in, LX out, `lx-link` the bridge).

- [ ] **C SDK** (ADR-0095–0100): `os2rc` HELPTABLE / HELPSUBTABLE, ASSOCTABLE, FONTDIR, string resource names; undeclared entries (`gen_api -- check-headers`); float and long double math.
- [ ] **CI and C++** (ADR-0101–0110): cache the crates in CI; C++ exceptions through Warpine's callbacks; clang ignores `-fnon-call-exceptions` (ADR-0110); a C++ standard library.
- [ ] **Rust as a first-class SDK language**: complete the guest crates (`warpine-os2-sys` / `-rt` / `warpine-os2`) to the C SDK's coverage, PM included, for memory safety inside the guest.
- [ ] **A declarative UI layer**: guest-side library code over PM (like `libwps`, ADR-0129): a view tree diffed into real PM windows (`WinCreateWindow`, `WinSetWindowText`, `LM_*`, `WinDestroyWindow`), notifications turned into callbacks that update the state, and a layout engine (stacks, grids, fill) applied with `WinSetMultWindowPos`. Rust first (macro syntax over `warpine-os2`), then a markup language compiled to C by a Warpine tool, with state from SOM attributes. Warpine's own names and API; needs an ADR.
- [ ] **Ported software**: 🚧 bitwiseworks GCC / LIBCn / LIBCx as regression inputs (licences first); the DOSCALLS ordinals they import; their current versions and debug formats.

## Blockers

Each waits on a permissive public source; where none turns up and the original cannot be rebuilt, a design of Warpine's own may close it, agreed first and recorded in an ADR naming the compatibility given up. Most valuable: the SOM ordinals, then the IPF format, then the DIVE / PGL export lists.

- **Unpublished formats**: IPF (INF / HLP), binary OS/2 INI, `ERRINFO`, the `select` family's ordinals and layouts.
- **Missing export lists**: DIVE, OpenGL / PGL, the SO32DLL / TCP32DLL split, dates of `unknown` exports, SOM.DLL 48+ (and with them IBM-built SOM classes, Stage B2/B3, and WPS compatibility).
- **Undocumented behaviour**: `DB_RAISED` / `DB_DEPRESSED`; `THREADS=` (`CMDREF.INF` needs an INF reader); `WinQueryWindowPos` / `WinCreateWindow` errors.
- **Third-party binaries**: bitwiseworks GCC / LIBCn / LIBCx (licences).

## Long-term: Warpine as a Wayland compositor

- [ ] **Linux applications in a Warpine session** (after level 2 of the login session): the desktop server as a Wayland compositor (Smithay), Linux windows as surfaces in Warpine's own compositor (ADR-0011) beside OS/2 programs; its own seccomp role and Landlock policy for the larger surface; needs an ADR.

## Long-term: XE, a 64-bit OS/2-lineage platform

- [ ] **Format**: an XE spec (`entry_rip: u64`, 64-bit objects, fixups, imports, exports) behind an MZ stub that says "requires a 64-bit OS/2" (an ADR); `src/xe/`, `detect_format()` → `load_xe()` / `run_xe()`.
- [ ] **Long mode**: `EFER.LME`, 4-level paging, a 64-bit GDT with FS / GS for TIB / PIB, System V AMD64 thunks.
- [ ] **ABI** (ADR-0109, proposed): System V AMD64 psABI and Itanium C++ ABI, LP64 (`ULONG` 32-bit, handles and `MPARAM` 64-bit, a layout table per structure and message), a psABI CPU level in the header; to decide: OS/2 handlers against `.eh_frame`, the red zone, ELF-specific parts, SOM's 64-bit interface.
- [ ] **APIs and tools**: `DOSCALLS64`, `PMWIN64`, `UCONV64` on UTF-8; a `warpine-xe` crate, `x86_64-warpine-xe`, `xe_hello` in Rust and C; LX and XE side by side, `DosExecPgm` starting XE.
