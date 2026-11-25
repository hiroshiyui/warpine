# Warpine TODO List

Open work only, highest priority first. Finished work is in the [CHANGELOG](../CHANGELOG.md), the [Developer Guide](developer_guide.md), the [Reference Manual](reference_manual.md) and the [ADRs](adr/README.md); unconfirmed behaviour in [Unverified behaviour](unverified.md). Everything follows the near-clean-room policy (CLAUDE.md). Items marked 🚧 wait on a source: see [Blockers](#blockers).

## Security

- [ ] **Host confinement** (Landlock ADR-0111, seccomp ADR-0124): `ci/check_desktop.sh` passed on Wayland with PipeWire and NVIDIA (2026-11-25: PM and VIO windows and sound, the desktop server; the shader cache and `/proc` thread naming stay refused, ADR-0111 addendum); still to run on X11, native PulseAudio and other GPU drivers, widening the policy where it reports a difference; namespaces only if those leave gaps.
- [ ] **Audit 2026-11-25, Medium, left**: the other host frames built on the guest's stack (window-procedure calls, `WM_CREATE`, `WM_DESTROY`, owner drawing, modal loops, exception dispatch, `INITTERM`, a chain's `DosExit`) still `expect` room there, as the call chain did; the arena's residual windows (check-then-copy against a free on another thread; TLB flushes do not wait for every vCPU).
- [ ] **Audit 2026-11-25, Low, left**: other calls that copy a guest-named buffer out with `guest_read` / `guest_read_bytes` rather than `guest_read_buffer` (they reach Warpine's protected memory, the process's own); a sweep of the API handlers for them.
- [ ] **Fuzzing** (ADR-0112): font and help-file readers once Warpine reads guest-supplied ones (none today).

## SOM / Workplace Shell

The SOM kernel (ADR-0059–0063, 0070–0073, 0122, 0126) is done; the WPS is to be a PM application on it. Sources: the Warp 4 Toolkit's SOM and WPS guides and references, IBM's OS/390 SOMobjects books and the SOMobjects 3.0 Toolkit books (`doc/references/`, never committed; URLs in the README). Object model for the WPS, to decide in an ADR: SOM as far as the sources allow (current course), or XPCOM (MPL-2.0; not binary-compatible with SOM).

- [ ] **Kernel calls missing**: `somDispatchD` (a `double` in the FPU), `somAddDynamicMethod` and dynamic methods; TypeCodes (`any` carries an opaque one); applying IBM-built classes (their apply-stub protocol is not public).
- [ ] 🚧 **IBM's SOM.DLL ordinals 48+ and control blocks**: Warpine's own ordinals (3072+, ADR-0123) serve programs built from source; IBM-built binaries need IBM's ordinals and initializer / destructor / assignment control blocks. Unused by `somBuildClass` (ADR-0073): DirectToSOM classes, migrated methods, `somDynamicSCI`, `inheritVars`, `somConstructClass`. An interface repository (`somInterfaceRepository` is NULL).
- [ ] **`somidl`**, if needed: OIDL `.csc` input, an `ir` emitter and interface repository, TypeCodes behind `any`, DSOM.
- [ ] **Smaller**: C++ SOM objects on the stack and in static storage; `dllname` for `somLocateClassFile`; `SOMU` classes, `SOMIR`, IDL metadata.
- [ ] **WPS classes**: `WPObject`, `WPFileSystem`, `WPFolder` (on `WC_CONTAINER`), `WPDesktop`, `WPProgram`, `WPDataFile`; persistence in OS2.INI; settings notebooks; `wpDragOver` / `wpDrop`.

## Kernel and processes

- [ ] **Shared memory** (ADR-0049): `DosAllocShrSeg` / `DosGetShrSeg`; blocks of a killed process whose parent never waits, swept at the next table use.
- [ ] **Semaphores** (ADR-0050): named and shared muxwait semaphores across processes; limits of 512 per session, 28 processes per semaphore.
- [ ] **Signals** (ADR-0035, 0093): interrupting anonymous-pipe writes and host I/O on FIFOs and devices; signal focus across the session; `ERROR_INVALID_PROCID` for PM programs; a GDB stop request kicking the vCPUs.
- [ ] **Pipes**: muxwait semaphores as pipe semaphores, a real `NPSS_WAIT`; echo of piped standard input.
- [ ] **Suballocation**: shared pools between processes.
- [ ] **Threads**: the paging arena caps a process at 736 threads (ADR-0115), below `THREADS=`'s 4095; allocator blocks would lift it.
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
- [ ] **Ported software**: 🚧 bitwiseworks GCC / LIBCn / LIBCx as regression inputs (licences first); the DOSCALLS ordinals they import; their current versions and debug formats.

## Blockers

Each waits on a permissive public source; where none turns up and the original cannot be rebuilt, a design of Warpine's own may close it, agreed first and recorded in an ADR naming the compatibility given up. Most valuable: the SOM ordinals, then the IPF format, then the DIVE / PGL export lists.

- **Unpublished formats**: IPF (INF / HLP), binary OS/2 INI, `ERRINFO`, the `select` family's ordinals and layouts.
- **Missing export lists**: DIVE, OpenGL / PGL, the SO32DLL / TCP32DLL split, dates of `unknown` exports, SOM.DLL 48+ (and with them IBM-built SOM classes, Stage B2/B3, and WPS compatibility).
- **Undocumented behaviour**: `DB_RAISED` / `DB_DEPRESSED`; `THREADS=` (`CMDREF.INF` needs an INF reader); `WinQueryWindowPos` / `WinCreateWindow` errors.
- **Third-party binaries**: bitwiseworks GCC / LIBCn / LIBCx (licences).

## Long-term: XE, a 64-bit OS/2-lineage platform

- [ ] **Format**: an XE spec (`entry_rip: u64`, 64-bit objects, fixups, imports, exports) behind an MZ stub that says "requires a 64-bit OS/2" (an ADR); `src/xe/`, `detect_format()` → `load_xe()` / `run_xe()`.
- [ ] **Long mode**: `EFER.LME`, 4-level paging, a 64-bit GDT with FS / GS for TIB / PIB, System V AMD64 thunks.
- [ ] **ABI** (ADR-0109, proposed): System V AMD64 psABI and Itanium C++ ABI, LP64 (`ULONG` 32-bit, handles and `MPARAM` 64-bit, a layout table per structure and message), a psABI CPU level in the header; to decide: OS/2 handlers against `.eh_frame`, the red zone, ELF-specific parts, SOM's 64-bit interface.
- [ ] **APIs and tools**: `DOSCALLS64`, `PMWIN64`, `UCONV64` on UTF-8; a `warpine-xe` crate, `x86_64-warpine-xe`, `xe_hello` in Rust and C; LX and XE side by side, `DosExecPgm` starting XE.
