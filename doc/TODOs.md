# Warpine TODO List

Open work only. Finished work is in the [CHANGELOG](../CHANGELOG.md), the
[Developer Guide](developer_guide.md), the [Reference Manual](reference_manual.md)
and the [ADRs](adr/README.md). Behaviour Warpine chose without a source to
confirm it is listed in [Unverified behaviour](unverified.md).

All work follows the near-clean-room policy: public documentation, toolkit
headers and osFree / ReactOS references only; no IBM binaries, ROM dumps or
disassembly.

**Contents** (highest priority first):
- [Security Facilities](#security-facilities)
- [SOM / Workplace Shell](#som--workplace-shell)
- [Kernel and processes](#kernel-and-processes)
- [Presentation Manager](#presentation-manager)
- [Console and multimedia](#console-and-multimedia)
- [Application compatibility](#application-compatibility)
- [Development toolchain](#development-toolchain)
- [Project Blockers](#project-blockers) (waiting on sources)
- [Long-term](#long-term)

---

## Security Facilities

Warpine's first priority is that a guest cannot escape to the host (CLAUDE.md, ADR-0066). These would make that boundary harder to cross, and find mistakes before guests do.

- [ ] **Confining the host process, further** (Landlock is done, ADR-0111; a seccomp deny list per role, ADR-0124):
  - check the sandbox's display and sound paths on real desktops (Wayland, X11, PulseAudio, PipeWire, GPU drivers); only SDL's dummy drivers are tested, and so is the seccomp deny list;
  - namespaces (mount, user, network) only if those leave gaps: the session's AF_UNIX sockets, memfds, `/dev/kvm` and the display make them delicate.
- [ ] **Fuzzing, further** (ADR-0112 fuzzes the desktop protocol, LX, NE, message files, `os2rc`'s scripts, `lx-link`'s inputs, bitmaps, pointers, WAV files and the dialog / menu / accelerator / string / `PRESPARAMS` resources, CONFIG.SYS and profiles, a minute per target on each push and ten minutes weekly): the font and help-file readers once Warpine reads guest-supplied ones (it reads none today: built-in fonts, `GpiLoadFonts` a stub, help libraries named only).

---

## SOM / Workplace Shell


The WPS is a PM application on the session desktop, built on SOM. The SOM kernel's stages 1–4 are done (ADR-0059–0063).

**Sources at hand** (found 2026-11-23; kept locally under `doc/references/`, never committed; public URLs in the README's sources):
- the OS/2 Warp 4 Toolkit's *System Object Model Programming Guide* and *Reference* and *Workplace Shell Programming Guide* and *Reference* (Internet Archive, reprinted with IBM's permission) — the OS/2 description of SOM and the WPS classes;
- IBM's OS/390 *SOMobjects Programmer's Reference, Volume 1* (SC28-1997) and *Messages, Codes and Diagnosis* (SC28-1996) — the kernel as documented for OS/390, and the error codes;
- IBM's *SOMobjects Developer's Toolkit* Version 3.0 books (1996, Internet Archive): *Programmer's Guide* Vol. I–II and *Programmer's Reference* Vol. I–IV — SOM 3.0's concepts, initializers and kernel.

**Object model options** (for Warpine's own objects, and for the WPS should SOM compatibility stay blocked; to be decided in an ADR):
1. SOM, compatible with IBM's as far as the sources allow (the current course, ADR-0059–0073);
2. the Cross Platform Component Object Model (XPCOM), Mozilla's portable COM-style component model (interfaces described in XPIDL, reference counting, `QueryInterface`, a component manager; MPL-2.0; also used by VirtualBox on its non-Windows hosts) — not binary-compatible with SOM, so IBM-built SOM and WPS binaries would not run on it.

IBM's own SOM 3.0 FTP directory (`ftp.software.ibm.com/publications/clubod/som30`) is gone and was never archived; the 3.0 books above are a separate copy.

- [x] **Checked the kernel against the OS/2 SOM Programming Reference** (2026-11-23): of its 130 kernel entries (functions, runtime variables, macros, and the `SOMObject`, `SOMClass` and `SOMClassMgr` methods), 55 match, 26 differ and 47 are missing. The findings, in the suggested order of work:
- [ ] **Missing, documented by the Reference** (the rest came 2026-11-24):
  - the class manager's `somSubstituteClass` and `somMergeInto`;
  - dispatch beyond ADR-0126 (somApply, somDispatch, somClassDispatch, somDispatchV / L / A and the somVaBuf helpers are done): `somDispatchD` (a `double` in the FPU), `somAddDynamicMethod` and dynamic methods, and applying classes built with IBM's tools (their apply stubs' protocol is not public);
  - TypeCodes (`any` carries an opaque one).
- [ ] **IBM's SOM.DLL ordinals and layouts** (1–47, ADR-0070; the root classes' layouts, ADR-0072):
  - ordinals 48 and above: no public source lists them, so SOM.DLL's other exports sit at Warpine's own ordinals (3072 and up), which programs built with `somidl` and the SDK link by name — source-level compatibility (ADR-0123); IBM-built SOM binaries importing by IBM's numbers stay out of reach unless a public list turns up;
  - IBM-built class libraries also need IBM's initializer / destructor / assignment control blocks, whose layout no public source gives; code rebuilt with `somidl` uses Warpine's own protocol instead (ADR-0122). Still unused by `somBuildClass` (ADR-0073): DirectToSOM classes, migrated methods, `somDynamicSCI`, `inheritVars`, `somConstructClass`;
  - the root methods that resolve to the not-implemented procedure: see "Missing, documented by the Reference" above; the class manager's `somInterfaceRepository` / `somRegisteredClasses` attributes are not in the Reference.
- [ ] **`somidl` beyond ADR-0122's four stages** (all done: C and C++ bindings, multiple inheritance, initializers, exceptions; Open Watcom's hello, animals and hellocpp build from them), if needed: OIDL `.csc` input (Open Watcom's classes sample stays hand-bound), an `ir` emitter and an interface repository, TypeCodes behind `any`, DSOM.
- [ ] **C++**: SOM objects on the stack and in static storage.
- [ ] **Class files**: `dllname` for `somLocateClassFile`.
- [ ] **Utilities and metadata**: `SOMU` classes, the interface repository (`SOMIR`), IDL metadata.
- [ ] **WPS classes** (after the *Workplace Shell Programming Guide* and *Reference*):
  - `WPObject`, `WPFileSystem`, `WPFolder` (on `WC_CONTAINER`), `WPDesktop`, `WPProgram`, `WPDataFile`;
  - persistence in OS2.INI;
  - settings notebooks;
  - `wpDragOver` / `wpDrop`.

---

## Kernel and processes

- [ ] **Shared memory** (ADR-0049):
  - `DosAllocShrSeg` / `DosGetShrSeg`;
  - the blocks of a killed process whose parent never waits for it, swept at the next table use (a reaped child's go at once).
- [ ] **Semaphores** (ADR-0050):
  - named and shared muxwait semaphores across processes;
  - limits of 512 per session and 28 processes per semaphore.
- [ ] **Signals** (ADR-0035, 0093):
  - interrupting writes to anonymous pipes, and host file I/O on FIFOs and devices;
  - signal focus across the session, `ERROR_INVALID_PROCID` for PM programs;
  - a GDB stop request kicking the vCPUs.
- [ ] **Pipes**:
  - muxwait semaphores as pipe semaphores, and a real `NPSS_WAIT`;
  - echo of piped standard input.
- [ ] **Suballocation**: shared pools between processes.
- [ ] **Threads per process**: the paging arena caps a process at 736 threads (ADR-0115), below `THREADS=`'s 4095; blocks from the allocator would lift the cap (weighed in ADR-0115).
- [ ] **Profiles**:
  - the program-list calls, the `PRF16*` / `WIN16*` exports;
  - binary OS/2 INI files (format unpublished).
- [ ] **CONFIG.SYS** (ADR-0032, 0092):
  - check the `THREADS=` range and the parsing rules against IBM's Command Reference itself (`CMDREF.INF`) once a text copy outside install media turns up: two searches (the second 2026-11-25) found only secondary sources, which agree on 64 to 4095 with 64 as the default for Warp 3 / 4 and ArcaOS (Warpine's values) but say nothing of clamping, keyword case, spaces or `REM` (`config_sys.rs`);
  - `MAXWAIT`, `DPATH` use, ….
- [ ] **Compatibility chart** (ADR-0033):
  - date the `unknown` exports (22 of os2386.lib, 7 of the other modules);
  - check the Warp 3 data, and the Warp 3 dates of SOM 2.0 (from an AIX edition) and of the VIO / MOU threshold and `VioGlobalReg` calls;
  - a public export list of SO32DLL / TCP32DLL (the split by the reference's chapters is a guess, ADR-0033 addendum);
  - the modules not charted: Open32 (PMWINX), OpenGL, DIVE, MMIO, MCIAPI, LIBUNI, TCPIP32.
- [ ] **SOM.DLL ordinals 48 and above**: see [SOM](#som--workplace-shell).

---

## Presentation Manager


### Controls

- [ ] **Containers** (ADR-0043–0046, 0065, 0067):
  - keep an edit when the focus leaves the MLE without a click in the container;
  - `CA_OWNERPAINTBACKGROUND`;
  - the grid squares list (`GRIDINFO.pGrid`, `CN_GRIDRESIZED`);
  - records shared between containers.
- [ ] **Embedded dialogs** (ADR-0042, 0069): a working title bar (moving the dialog, its buttons).
- [ ] **Drag and drop** (ADR-0045, 0057, 0068):
  - `DrgAcceptDroppedFiles`, `DrgQueryFormat`;
  - lazy drag: `DrgLazyDrag`, `DrgLazyDrop`, `DrgCancelLazyDrag`, `CN_PICKUP`, `CN_DROPNOTIFY`;
  - drag images drawn as polygons (`DRG_POLYGON`) or `DRG_TRANSPARENT`; `DM_DROPHELP` / `CN_DROPHELP` / `VN_DROPHELP`;
  - `WM_BEGINDRAG` from the keyboard;
  - a host drop's position on real displays (the last pointer position is used).
- [ ] **Menus**:
  - a redraw of an owner-drawn item when the mouse highlights it (the keyboard highlight does), `MIA_NODISMISS`.
- [ ] **Sliders and value sets**:
  - the circular slider's `CSN_QUERYBACKGROUNDCOLOR`;
  - `VN_HELP`, `VDA_ITEMBACKGROUND` / `VDA_SURROUNDING`;
  - a `wrc` dialog with `CTLDATA` in a sample.
- [ ] **Text controls**:
  - `WinSetSysValue` for indices other than `SV_CURSORRATE`, `SV_FIRSTSCROLLRATE` and `SV_SCROLLRATE`;
  - hide the text cursor while a frame is tracked.

### Dialogs, menus and message boxes

- [ ] **Dialogs and buttons**:
  - `WM_ACTIVATE` when the active window is destroyed;
  - check that `WinProcessDlg` really leaves its dialog to the caller.
- [ ] **Menus in dialogs**:
  - `WM_UPDATEFRAME(FCF_MENU)` to a dialog (`WinDefDlgProc` ignores it), and the other `FCF_*` flags of `WM_UPDATEFRAME`;
  - `WinSetParent` / `WinSetOwner` of a menu.
- [ ] **Message boxes**: the `HK_HELP` hook (`idWindow`), `MB_SYSTEMMODAL` as system modal (it is application modal now).
- [ ] **Standard dialogs**:
  - a default error box when `FDM_ERROR` returns 0;
  - filtering the style list, `FNTM_COLORCHANGED` / `FNTM_UPDATEPREVIEW`;
  - the read-only check box, overwrite confirmation on Save As, emphasis in the font sample;
  - a PMCTLS section in `doc/os2_ordinals.md`.
- [ ] **Help viewer**:
  - an INF / HLP reader (IPF is unpublished: check the osFree / NewView descriptions first);
  - `RT_HELPTABLE` / `RT_HELPSUBTABLE`, and the help-table panel for the `WM_HELP` F1 posts;
  - `HM_*` notifications, the `Ddf*` calls.

### Frames, input and messages

- [ ] **Frames and system keys**:
  - cascades deeper than one level, and opening a cascade when the pointer rests on its row;
  - `MM_SETITEM`, `MM_SETITEMHANDLE`, `MM_SELECTITEM`, `MM_QUERYITEMRECT`, `MM_QUERYSELITEMID` and the default-item messages;
  - icons for minimized frames (`FCF_ICON`, `WinSetIcon`), min/max for dialogs;
  - `WinSetActiveWindow` on minimized or disabled frames;
  - `SWP_ZORDER` / `hwndInsertBehind` in a subclassed frame's `WM_FORMATFRAME`;
  - `QWS_FLAGS`, the alarm of `WinFlashWindow`, `WM_MINMAXFRAME` from `WinSetMultWindowPos`.
- [ ] **System and pop-up menus**:
  - the system menu's Window list item, which is greyed although Ctrl+Esc works;
  - Alt+F11 for Hide, and a minimized frame's system menu on a click;
  - `WM_TRACKFRAME` / `WM_QUERYTRACKINFO` for subclassed frames (ADR-0089 phase 2), the pointer taking over a keyboard tracking, and a `WinTrackRect` outline on the desktop (no PS);
  - submenus, `PU_MOUSEBUTTONnDOWN`, `PU_KEYBOARD` / `PU_MOUSEBUTTONn` in `WinPopupMenu`.
- [ ] **Input**:
  - the horizontal wheel;
  - `WM_HITTEST` for clicks on frame chrome, which still act on the GUI thread when they arrive.
- [ ] **PM errors**:
  - `PMERR_INVALID_FLAG` from `WinQueryWindowPos`, which has no flag parameter (the reference lists it); `PMERR_CANNOT_SET_FOCUS` once Warpine has system-modal windows or a focus change in progress;
  - `PMERR_INVALID_FLAG` / `PMERR_INVALID_PARM` / `PMERR_INVALID_PARAMETERS` from `WinCreateWindow` (which values the reference means is not said);
  - `WinGetErrorInfo` / `WinFreeErrorInfo` (the `ERRINFO` layout is unknown).
- [ ] **Hooks and messaging**:
  - `MSGF_TRACK` and `HK_INPUT` in the modal loops, and the help hook of a message box;
  - journal and the other hook types (stored, never called), and hooks across processes;
  - `WinSetMsgInterest`;
  - a `BMSG_FRAMEONLY` send reaches the client procedure with the frame's handle.
- [ ] **Windows, key state and colours**:
  - the desktop background, menus and control faces reading `SYSCLR_*`;
  - messages that Warpine gives a subclassed control directly passing through its subclass procedure;
  - `WM_SETFOCUS` sent, not posted;
  - key state per queue, and CapsLock / NumLock from the host at start;
  - dialogs under a client positioned frame-relative by `WinSetWindowPos`;
  - other disabled controls drawn greyed (static text, entry fields, list boxes), and `PP_DISABLEDFOREGROUNDCOLOR`.
- [ ] **Presentation parameters and painting**:
  - `SS_TEXT` with `DT_WORDBREAK`, and `WM_PRESPARAMCHANGED` sent rather than posted;
  - `PP_HILITE*` / `PP_DISABLED*` colours, and presentation parameters for title bars, menus, sliders and value sets;
  - row heights that follow `PP_FONTNAMESIZE`, and `SS_ICON` / `SS_BITMAP` from the dialog's module;
  - locking one window instead of its frame, and `WinEnableWindowUpdate` (a stub);
  - `WS_CLIPSIBLINGS` / `WS_PARENTCLIP`, `CS_MOVENOTIFY` / `WM_MOVE`;
  - `DB_RAISED` / `DB_DEPRESSED`, if a source documents them.
- [ ] **Applications, switch list, DDE**:
  - `WinStartApp` with `SAF_MINIMIZED` / `SAF_MAXIMIZED` / `SAF_BACKGROUND`, `swpInitial` and the title;
  - `SAF_STARTCHILDAPP` programs ending with their parent;
  - `WinSwitchToProgram` restoring minimized frames, and switch-list titles following `WinSetWindowText`;
  - `WinQuerySessionTitle`, `WinQueryTaskTitle` / `WinQueryTaskSizePos`, `WinSwitchProgramRegister`;
  - reading real `.MSG` files;
  - PM's Window List window (it is a pop-up menu now).
- [ ] **Desktop server** (ADR-0052–0057):
  - text-mode programs keep their own window;
  - the server composites in software only;
  - a crashed server ends the session's PM programs instead of their reconnecting;
  - a program that stops reading input delays everyone for `INPUT_STALL` (5 s);
  - pointers larger than 128 × 128 show as the arrow;
  - the clipboard between programs is asynchronous: a program's text reaches the others through its GUI thread and the server's broadcast, so another program told by other means that it changed can still read the old text (the race ADR-0108 fixed for activation).

### Graphics

- [ ] **GPI**:
  - custom bitmap patterns (`GpiSetPatternSet`), rounded box corners (`lHRound` / `lVRound`);
  - `GpiSetDefArcParams`, `GpiSetDefAttrs`, `GpiSetDefViewingLimits`;
  - palettes (`GpiCreatePalette`, `LCOLF_PALETTE`);
  - 16-bit and `BCA_HUFFMAN1D` bitmaps, and compressed `GpiQueryBitmapBits` output;
  - `GpiSetLineEnd` / `GpiSetLineJoin`;
  - elliptic, rounded and polygon region constructors;
  - segments;
  - `GpiTabbedCharStringAt`, `GpiQueryCharBox` and character-box width scaling;
  - outline text inside paths and areas;
  - transforms not yet applied to geometric line widths, glyphs, the character box and blitted images (rotated viewing limits and clip rectangles become bounding boxes);
  - a separate colour and mix per primitive type;
  - GPI window origins for child-window PSs;
  - pair kerning from GPOS (changes text widths everywhere).
- [ ] **Printing**: printer and metafile DCs (`DevOpenDC` takes only `OD_MEMORY` / `OD_INFO`), and a real spooler (PostScript / PDF).
- [ ] **DIVE**: direct framebuffer blitting for games and video. It maps onto the shared per-window surfaces; find the export list in public sources first, and pair it with DART audio.
- [ ] **OpenGL (`OPENGL.DLL`, PGL)**: forward GL 1.x to the host's compatibility profile. The real work is PGL contexts and buffer swaps, guest pointers in array and pixel calls, and an offscreen framebuffer composited as the window's texture. Find the export list first.

### Rendering and compositing

Measure with the frame statistics (`RUST_LOG=warpine::gui=debug`) and `samples/pm_bench` before and after.

- [ ] Measure idle CPU and input latency on real X11 / Wayland displays (only measured on `dummy` so far).
- [ ] Damage smaller than a window: record invalidated rectangles instead of whole frames.
- [ ] Damage a moved window's old and new rectangles separately.
- [ ] Flicker during long paints: present a `WinBeginPaint` PS only at `WinEndPaint`, if visible.
- [ ] HiDPI (deferred, no display to verify on): agree the approach first.
- [ ] A shared glyph atlas, only if profiling shows text as significant.

---

## Console and multimedia

- [ ] **The built-in CMD.EXE** (`cmd.rs`, `cmd/`; it replaces 4OS2, ADR-0075). Stage 2 is done (ADR-0077–0080, 0082). Still open:
  - START's sessions on the shared desktop, with the VIO-window item below (each opens a window of its own now);
  - which of OS/2's other `CMD.EXE` commands to add is decided case by case.
- [ ] **TCP/IP calls still missing** (the chart's TCP/IP row; Warpine's own ordinals per ADR-0123):
  - the resolver's `_res` state and options, TCP for answers over 512 bytes, `res_mkquery`'s other ops, `dn_find`, compression in `dn_comp`; `%ETC%` files as an alternative to the host's databases (the hosts, networks and services databases are the host's);
  - `sendmsg` / `recvmsg`, `so_cancel`, `addsockettolist` / `removesocketfromlist`, `rexec`, the SOCKS calls (`Raccept`, `Rbind`, `Rconnect`, `Rlisten`, `Rgetsockname`, `Rgethostbyname`).
- [ ] **VIO and KBD calls still missing** (the chart's VIO / KBD row):
  - `KbdStringIn`'s line-editing keys beyond Backspace (the template recall of `cchIn`), and the TERM mode;
  - `KbdSetCustXt`, `KbdRegister` / `KbdDeRegister`, `KbdSetFgnd`, `KbdSetHWID`, keyboard layouts other than the US one (`KbdXlate`);
  - `VioGetFont` / `VioSetFont` (a bitmap font of Warpine's own is needed: IBM's VGA font is not free), `VioGetBuf` / `VioShowBuf` (stubs), `VioSetState` / `VioSetCp` acted on by the renderer, `VioRegister` / `VioDeRegister`, `VioGlobalReg`, `VioPrtSc` / `VioPrtScToggle`, `VioModeWait` / `VioModeUndo`, `VioSavRedrawWait` / `VioSavRedrawUndo`, `VioGetPhysBuf`.
- [ ] **VIO window**:
  - several VIO sessions on one desktop (`DosStartSession`, the shell's START; each opens its own window now);
  - Advanced VIO (`VioCreatePS`, `VioAssociate`).
- [ ] **MOUCALLS**: MOUCALLS for NE programs; calls passed to a registered subsystem (`MouRegister` records it only).
- [ ] **ANSI**: keyboard reassignment `ESC[…p`.
- [ ] **Audio**:
  - sound in headless runs (no main-thread loop now);
  - `MCI_RECORD` from an SDL3 recording device;
  - MIDI (FluidSynth / SDL3_mixer / ALSA; deferred).

---

## Application compatibility

- [ ] **16-bit (NE) programs**:
  - the Watcom 16-bit CRT (needs LDT selectors);
  - 16-bit code calling 32-bit DLLs;
  - more DOSCALLS / VIOCALLS / KBDCALLS ordinals;
  - entry registers (AX = environment, BX = command line);
  - real 16-bit `DosGetEnv` / `DosGetPID`.
- [ ] **Far16 thunks**:
  - dispatch 16-bit calls made through thunks (`vcpu.rs` bypasses them and returns 0; for example `DosGetInfoSeg`);
  - OS/2's tiling formula `(sel >> 3) << 16` (Warpine's tiles start at GDT index 6).
- [ ] **REXX** (ADR-0081): our own interpreter in Rust, in the built-in CMD.EXE; a `.CMD` file starting with `/*` runs as REXX and its commands go to `ADDRESS CMD`. No Regina, no IBM code. Phases R1–R3 are done (ADR-0083–0085). Still open:
  - from R3's area: the macro space (`RexxAddMacro` … are stubs), tokenized images (`instore[1]`), the `RXFNC`, `RXCMD`, `RXMSQ` and `RXTRC` exits, the 16-bit entry points (`REXXSAA`, `RXVAR` …), system-wide queues and registrations (they are per process), `RxFuncAdd` of DLL function packages from a script;
  - from R2's area: REXXUTIL's screen and keyboard functions (`SysCurPos`, `SysGetKey`, `SysTextScreenRead` …), EAs, INI files and Workplace Shell objects, case by case; interactive tracing's pause (`TRACE ?`); an error output (STDERR, trace) separate from the shell's output;
- [ ] **Session clock**: `DosSetDateTime` reaching processes that are already running (ADR-0039).
- [ ] **Sockets** (ADR-0041): the real ordinals and layouts of `select`, `bsdselect` and `os2_select`.

---

## Development toolchain

Open Watcom stays the frozen ABI baseline. A second, modern toolchain goes only through stable formats: i386 ELF objects in, LX out, with `lx-link` as the one bridge. That rules out compiler plugins, LLVM passes and lld / BFD back ends.

- [ ] **The rest of the C SDK** (M1, M2, the PM headers, the runtime and its errno / floating point / strtod / math.h, the resource compiler and the other PM modules done: ADR-0095–0100, `sdk/`):
  - `os2rc`: HELPTABLE / HELPSUBTABLE, ASSOCTABLE, FONTDIR, resource names given as strings;
  - the entries left undeclared (`cargo run --bin gen_api -- check-headers` counts them), the float and long double math functions.
- [ ] **After M3 and M4** (every C sample from GCC and clang, pinned compilers, GDB with DWARF, C++: ADR-0101–0103):
  - CI (ADR-0106): consider caching the image and the crates;
  - C++ exceptions through Warpine's callbacks (a window procedure that `WinDispatchMsg` called; across DLLs is done, ADR-0107);
  - clang does not honour `-fnon-call-exceptions`: a trapping instruction's own frame gets no cleanups, and a caller clang proves cannot throw ends in `std::terminate` (ADR-0110);
  - a C++ standard library (strings, containers, streams);
- [ ] **Ported-software tier**:
  - bitwiseworks GCC / LIBCn / LIBCx binaries as regression inputs (check licences);
  - list the DOSCALLS ordinals LIBCn / LIBCx import;
  - check current GCC / LIBCn versions and debug formats.

---

## Project Blockers

Items below wait on data, information or binaries Warpine does not have, and that the clean-room policy only lets it take from a permissive public source. Each is also listed in its own section. Unblocking one starts with finding that source.

Where no such source turns up and the original cannot be rebuilt (SOM, the WPS, IPF help …), a practical design of Warpine's own may close the item instead, even at the cost of compatibility (its own ordinals, formats or protocols): Warpine is an OS/2-API-compatible platform, not a clone. Each such choice is agreed first and recorded in an ADR that names what compatibility it gives up.

**Unpublished formats or layouts**
- **INF / HLP help files** ([Help viewer](#dialogs-menus-and-message-boxes)): the IPF format is unpublished; check the osFree / NewView descriptions first.
- **Binary OS/2 INI files** ([Profiles](#kernel-and-processes)): the format is unpublished; profiles use Warpine's own format.
- **`WinGetErrorInfo` / `WinFreeErrorInfo`** ([PM errors](#frames-input-and-messages)): the `ERRINFO` layout is unknown.
- **`select`, `bsdselect`, `os2_select`** ([Sockets](#application-compatibility)): their real ordinals and layouts.

**Missing export lists or ordinals**
- **DIVE** and **OpenGL / PGL** ([Graphics](#graphics)): their export lists.
- **The compatibility chart** ([Kernel and processes](#kernel-and-processes)): the SO32DLL / TCP32DLL export split; dates for the `unknown` exports.
- **SOM.DLL ordinals 48 and above** ([SOM](#som--workplace-shell)): no public source lists them; Warpine's own (3072 and up, ADR-0123) serve programs built from source, IBM-built binaries need IBM's.
- **SOM Stage B2/B3**, classes built from IBM's static class data ([SOM](#som--workplace-shell)): waits for those ordinals, or a source for the initializer / destructor control-block protocol. It holds back IBM-built SOM programs, and with them the Workplace Shell's compatibility.

**Behaviour the public documentation does not settle**
- **`DB_RAISED` / `DB_DEPRESSED`** ([Presentation parameters and painting](#frames-input-and-messages)): no source documents them.
- **The `THREADS=` range and parsing** ([CONFIG.SYS](#kernel-and-processes)): only IBM's Command Reference (`CMDREF.INF`) settles them; secondary sources disagree, and reading it needs an INF reader (see above).
- **PM errors** ([PM errors](#frames-input-and-messages)): when `WinQueryWindowPos` reports `PMERR_INVALID_FLAG` (it has no flag parameter), and which values make `WinCreateWindow` report its parameter errors.

**Third-party binaries**
- **bitwiseworks GCC / LIBCn / LIBCx** as regression inputs ([Ported-software tier](#development-toolchain)): their licences must be checked first.

Most valuable to unblock: the SOM ordinals (Stage B2/B3, much of the Workplace Shell), then the IPF format (the help viewer, and `CMDREF.INF`), then the DIVE and PGL export lists.

---

## Long-term

### XE — a 64-bit OS/2-lineage platform (vision)

- [ ] **Format**:
  - an XE spec (header with `entry_rip: u64`, 64-bit objects, fixups, imports and exports) behind an MZ stub;
  - the stub prints "requires a 64-bit OS/2" and exits, so DOS, OS/2 and `file` all behave; record this in an ADR;
  - `src/xe/`, and `detect_format()` → `load_xe()` / `run_xe()`.
- [ ] **Long mode**: `EFER.LME`, 4-level paging, a 64-bit GDT with FS/GS for the TIB / PIB, thunks with System V AMD64 arguments.
- [ ] **ABI** (ADR-0109, proposed):
  - the System V AMD64 psABI unchanged for C, the Itanium C++ ABI for C++ (the SDK's runtime and libc++ carry over), IEEE 754, DWARF `.eh_frame` unwinding;
  - LP64 (POSIX `_POSIX_V7_LP64_OFF64`): OS/2 types from `<stdint.h>`, `ULONG` / `LONG` 32-bit, pointer-carrying types (handles, `MPARAM` / `MRESULT`, `PVOID` fields) 64-bit; a layout table for every structure and PM message;
  - a CPU baseline from the psABI's levels (`x86-64-v2` or `v3`) in the XE header;
  - to decide: OS/2 exception handlers against `.eh_frame` unwinding; the red zone with guests at ring 0 (ring 3, IST stacks, or `-mno-red-zone`); the psABI's ELF-specific parts if XE is not ELF; SOM's 64-bit interface.
- [ ] **APIs**:
  - `DOSCALLS64` (I/O, memory, threads, semaphores);
  - `PMWIN64`;
  - `UCONV64` on UTF-8.
- [ ] **Toolchain**: a `warpine-xe` crate, the `x86_64-warpine-xe` target, and `xe_hello` samples in Rust and C.
- [ ] **Dual ABI**: LX and XE side by side; `DosExecPgm` starting XE programs.
