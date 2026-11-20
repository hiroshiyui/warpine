# Warpine TODO List

Open work only. Finished work is in the [CHANGELOG](../CHANGELOG.md), the
[Developer Guide](developer_guide.md), the [Reference Manual](reference_manual.md)
and the [ADRs](adr/README.md). Behaviour Warpine chose without a source to
confirm it is listed in [Unverified behaviour](unverified.md).

All work follows the near-clean-room policy: public documentation, toolkit
headers and osFree / ReactOS references only; no IBM binaries, ROM dumps or
disassembly.

**Contents:**
- [Presentation Manager](#presentation-manager)
- [Kernel and processes](#kernel-and-processes)
- [Console and multimedia](#console-and-multimedia)
- [Application compatibility](#application-compatibility)
- [Development toolchain](#development-toolchain)
- [Long-term](#long-term)

---

## Presentation Manager

### Controls

- [ ] **Containers** (ADR-0043–0046, 0065, 0067):
  - keep an edit when the focus leaves the MLE without a click in the container;
  - `CA_OWNERPAINTBACKGROUND`;
  - the grid squares list (`GRIDINFO.pGrid`, `CN_GRIDRESIZED`);
  - records shared between containers.
- [ ] **Notebooks** (ADR-0064):
  - images on notebook buttons (`hImage`);
  - `BKS_SPIRALBIND`, `BKS_ROUNDEDTABS` / `BKS_POLYGONTABS` and the back-page corners, which are drawn as the defaults now.
- [ ] **Embedded dialogs** (ADR-0042, 0069): a working title bar (moving the dialog, its buttons).
- [ ] **Drag and drop** (ADR-0045, 0057, 0068):
  - `DrgAcceptDroppedFiles`, `DrgQueryFormat`;
  - lazy drag: `DrgLazyDrag`, `DrgLazyDrop`, `DrgCancelLazyDrag`, `CN_PICKUP`, `CN_DROPNOTIFY`;
  - drawing the drag image (`DrgSetDragImage` is accepted, but nothing is drawn);
  - target emphasis drawn by the container, `CN_DRAGAFTER`;
  - `WM_BEGINDRAG` from the keyboard;
  - a host drop's position on real displays (the last pointer position is used).
- [ ] **Menus**:
  - `WM_MEASUREITEM` for owner-drawn items, and a redraw when the highlight changes;
  - `WM_MEASUREITEM` on deletion, partial `WM_DRAWITEM` (`fsStateOld`).
- [ ] **Sliders and value sets**:
  - dragging the arm with button 2, `WC_CIRCULARSLIDER`;
  - value-set drag and drop (`VN_INITDRAG` / `VN_DRAGOVER` / `VN_DROP` / `VN_DROPHELP`), `VN_HELP`, `VDA_ITEMBACKGROUND` / `VDA_SURROUNDING`;
  - a `wrc` dialog with `CTLDATA` in a sample.
- [ ] **Scroll bars**:
  - frame scroll bars are painted 2 pels off (under the left border);
  - keyboard input on a focused bar, `SBS_AUTOTRACK`;
  - the slider's tracking rectangle;
  - repeat rates from `SV_FIRSTSCROLLRATE` / `SV_SCROLLRATE`.
- [ ] **Text controls**:
  - `WinSetSysValue` for indices other than `SV_CURSORRATE`;
  - hide the text cursor while a frame is tracked.

### Dialogs, menus and message boxes

- [ ] **Dialogs and buttons**:
  - `BS_USERBUTTON` / `BN_PAINT` (a user button only stores its highlight now);
  - `WM_ACTIVATE` when the active window is destroyed;
  - check that `WinProcessDlg` really leaves its dialog to the caller.
- [ ] **Menus in dialogs**:
  - `WM_UPDATEFRAME(FCF_MENU)` to a dialog (`WinDefDlgProc` ignores it), and the other `FCF_*` flags of `WM_UPDATEFRAME`;
  - `WinSetParent` / `WinSetOwner` of a menu.
- [ ] **Message boxes**: `MB_CUSTOMICON` / `WinMessageBox2`, the `HK_HELP` hook (`idWindow`), `MB_NONMODAL`, `MB_SYSTEMMODAL`, `MB_MOVEABLE`.
- [ ] **Standard dialogs**:
  - a default error box when `FDM_ERROR` returns 0;
  - filtering the style list, `FNTM_COLORCHANGED` / `FNTM_UPDATEPREVIEW`;
  - the read-only check box, overwrite confirmation on Save As, emphasis in the font sample;
  - `WinRegisterCircularSlider` (PMCTLS 24), and a PMCTLS section in `doc/os2_ordinals.md`.
- [ ] **Help viewer**:
  - an INF / HLP reader (IPF is unpublished: check the osFree / NewView descriptions first);
  - `RT_HELPTABLE` / `RT_HELPSUBTABLE`, and F1 → `WM_HELP` → the help-table panel;
  - `HM_*` notifications, the `Ddf*` calls.

### Frames, input and messages

- [ ] **Frames and system keys**:
  - F10 / Alt for the menu bar (`SC_APPMENU`, `MM_STARTMENUMODE`), F1 → `WM_HELP`;
  - icons for minimized frames (`FCF_ICON`, `WinSetIcon`), min/max for dialogs;
  - `WinSetActiveWindow` on minimized or disabled frames;
  - `SWP_ZORDER` / `hwndInsertBehind` in a subclassed frame's `WM_FORMATFRAME`;
  - `QWS_FLAGS`, the alarm of `WinFlashWindow`, `WM_MINMAXFRAME` from `WinSetMultWindowPos`.
- [ ] **System and pop-up menus**:
  - a real `FID_SYSMENU` window (`MM_SETITEMATTR` / `MM_QUERYITEMATTR`; `SC_CLOSE` ignored when Close is disabled);
  - the system menu's Window list item, which is greyed although Ctrl+Esc works;
  - Alt+F11 for Hide, and a minimized frame's system menu on a click;
  - `WM_TRACKFRAME` / `WM_QUERYTRACKINFO` / `WinTrackRect` for applications, and the pointer taking over a keyboard tracking;
  - submenus, `PU_MOUSEBUTTONnDOWN`, `PU_KEYBOARD` / `PU_MOUSEBUTTONn` in `WinPopupMenu`.
- [ ] **Input**:
  - the horizontal wheel;
  - `WM_HITTEST` for clicks on frame chrome, which still act on the GUI thread when they arrive.
- [ ] **Pointers**:
  - the I-beam over entry fields and MLEs, sizing arrows over frame borders;
  - `WM_CONTROLPOINTER`;
  - `WinCreatePointer` / `WinCreatePointerIndirect`, `WinQueryPointerInfo`, `WinDrawPointer`.
- [ ] **PM errors**:
  - `PMERR_INVALID_FLAG` (`WinInvalidateRect`, `WinQueryWindowPos`), `PMERR_CANNOT_SET_FOCUS`, `PMERR_INV_HPS` (`WinBeginPaint`);
  - `PMERR_NO_MSG_QUEUE` / `PMERR_INVALID_PARAMETERS` from `WinCreateWindow`;
  - errors from `WinStartTimer` and from the dialog-item text calls;
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
  - disabled buttons drawn greyed.
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
  - pointers larger than 128 × 128 show as the arrow.
- [ ] **Codepages**: decide whether `WinSetCp(0)` means "the process codepage" (it is rejected now).

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

## Kernel and processes

- [ ] **Shared memory** (ADR-0049):
  - enforce access flags (every process can reach the whole arena now);
  - `DosAllocShrSeg` / `DosGetShrSeg`;
  - free a killed process's blocks at once (now at the next table use).
- [ ] **Semaphores** (ADR-0050):
  - named and shared muxwait semaphores across processes;
  - limits of 512 per session and 28 processes per semaphore;
  - free a semaphore only a killed process held at once.
- [ ] **Signals**:
  - interrupting a pipe write blocked mid-frame and host file I/O;
  - signals in the modal dialog loop, Ctrl+Break at a terminal;
  - signal focus across the session, `ERROR_INVALID_PROCID` for PM programs;
  - a GDB stop request kicking the vCPUs;
  - `DosKillThread` interrupting the same calls.
- [ ] **Pipes**:
  - muxwait semaphores as pipe semaphores, and a real `NPSS_WAIT`;
  - sweeping stale `warpine-threads-*` directories;
  - echo of piped standard input.
- [ ] **Memory protection on the page tables** (ADR-0088): 32-bit threads now run paged, which makes these possible; each needs its own decision: `PAG_GUARD` guard pages and stack growth, decommitted pages (`DosSetMem`), read-only memory (`PAG_READ` without `PAG_WRITE`), unmapped page 0; paging for 16-bit (NE) programs.
- [ ] **Suballocation**: shared pools between processes.
- [ ] **Processes**:
  - `DosSendSignalException` to the parent or other session processes;
  - `DosWaitChild` after `DosKillProcess`;
  - only 8 of the 16 exit-code bits reach the parent.
- [ ] **Profiles**:
  - `PrfQueryProfile`, `PrfReset`, the program-list calls, the `PRF16*` / `WIN16*` exports;
  - binary OS/2 INI files (format unpublished).
- [ ] **CONFIG.SYS**:
  - check the `THREADS=` minimum (32 or 64?) and the parsing rules against the IBM Command Reference;
  - `LIBPATH`, `MAXWAIT`, `SET`, ….
- [ ] **Compatibility chart** (ADR-0033):
  - date the 22 `unknown` exports;
  - check the Warp 3 data;
  - get export lists for UCONV, MMPM/2, TCP/IP, PMCTLS and MSG.
- [ ] **SOM.DLL ordinals 48 and above**: see [SOM](#som--workplace-shell).

---

## Console and multimedia

- [ ] **The built-in CMD.EXE** (`cmd.rs`, `cmd/`; it replaces 4OS2, ADR-0075). Stage 2 is done (ADR-0077–0080, 0082). Still open:
  - START's sessions on the shared desktop, with the VIO-window item below (each opens a window of its own now);
  - which of OS/2's other `CMD.EXE` commands to add is decided case by case.
- [ ] **VIO window**:
  - Font Size, Scroll, Help, Hide;
  - keyboard marking, and autoscroll while marking;
  - several VIO sessions on one desktop (`DosStartSession`, the shell's START; each opens its own window now);
  - Advanced VIO (`VioCreatePS`, `VioAssociate`).
- [ ] **MOUCALLS**:
  - `MouGetHotKey` / `MouSetHotKey`, `MouRegister` / `MouDeRegister`, `MouInitReal`, `MouGetThreshold` / `MouSetThreshold`;
  - MOUCALLS for NE programs;
  - argument names of Pascal-order calls in `api_trace`.
- [ ] **ANSI**: `ESC[6n` (cursor position report), keyboard reassignment `ESC[…p`, display modes `ESC[=nh`.
- [ ] **Console handles**: console output to a host pipe or file is converted from CP437, whatever the process codepage.
- [ ] **Audio**:
  - sound in headless runs (no main-thread loop now);
  - `MCI_RECORD` from an SDL3 recording device;
  - the high words of `MM_MCINOTIFY`;
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
- [ ] **REXX** (ADR-0081): our own interpreter in Rust, in the built-in CMD.EXE; a `.CMD` file starting with `/*` runs as REXX and its commands go to `ADDRESS CMD`. No Regina, no IBM code. Phases:
  - R1, the language core, is done (ADR-0083); R2, conditions, streams, TRACE and a REXXUTIL subset, is done (ADR-0084);
  - R3, the REXX API for programs (REXX.DLL / REXXAPI.DLL: RexxStart, handlers, exits, the variable pool, queues), is done (ADR-0085);
  - still open from R3's area: the macro space (`RexxAddMacro` … are stubs), tokenized images (`instore[1]`), the `RXFNC`, `RXCMD`, `RXMSQ` and `RXTRC` exits, the 16-bit entry points (`REXXSAA`, `RXVAR` …), system-wide queues and registrations (they are per process), `RxFuncAdd` of DLL function packages from a script;
  - still open from R2's area: REXXUTIL's screen and keyboard functions (`SysCurPos`, `SysGetKey`, `SysTextScreenRead` …), EAs, INI files and Workplace Shell objects, case by case; interactive tracing's pause (`TRACE ?`); an error output (STDERR, trace) separate from the shell's output;
- [ ] **File attributes**: keep hidden / system / archive (an EA or the sidecar) and report them.
- [ ] **Session clock**: `DosSetDateTime` reaching processes that are already running (ADR-0039).
- [ ] **Sockets** (ADR-0041): the real ordinals and layouts of `select`, `bsdselect` and `os2_select`.

---

## Development toolchain

Open Watcom stays the frozen ABI baseline. A second, modern toolchain goes only through stable formats: i386 ELF objects in, LX out, with `lx-link` as the one bridge. That rules out compiler plugins, LLVM passes and lld / BFD back ends.

- [ ] **M1, C hello world from GCC**:
  - `samples/c_hello` with `gcc -m32 -ffreestanding -fno-pic -c` + `lx-link`, and an integration test;
  - `lx-link` support for `.text.*` / `.rodata.*` / `.data.*` / `.bss` variants, `COMMON`, string merging and `.init_array`, with a clear error for PIC relocations;
  - a minimal `_start`.
- [ ] **M2, compiler-neutral SDK**:
  - `os2.h` headers for GCC and clang (conventions behind macros);
  - imports from `targets/os2api.def`;
  - a small freestanding C runtime.
- [ ] **M3, two compilers in CI**:
  - GCC and clang (`i386-unknown-none`) for every C sample;
  - pinned compilers (Guix or Nix);
  - Open Watcom regression binaries kept.
- [ ] **M4, debugging and C++**:
  - DWARF through `lx-link` for the GDB stub;
  - Itanium C++ ABI runtime and unwinder, coexisting with OS/2 SEH.
- [ ] **Ported-software tier**:
  - bitwiseworks GCC / LIBCn / LIBCx binaries as regression inputs (check licences);
  - list the DOSCALLS ordinals LIBCn / LIBCx import;
  - check current GCC / LIBCn versions and debug formats.

---

## Long-term

### SOM / Workplace Shell

The WPS is a PM application on the session desktop, built on SOM. The SOM kernel's stages 1–4 are done (ADR-0059–0063).

- [ ] **IBM's SOM.DLL ordinals and layouts** (1–47, ADR-0070; the root classes' layouts, ADR-0072):
  - ordinals 48 and above: unassigned until a permissive source lists them (Free Pascal's LGPL `som.pas` is not used);
  - Stage B2/B3 is on hold (decided 2026-11-20) until IBM-built binaries can load (ordinals 48+) or a source describes the control-block protocol: the SOM Programming Guide ("Initializing and Uninitializing Objects") gives the semantics — each class's initializer exactly once, in a left-to-right visit of `directinitclasses` (default: the parents); `somDefaultInit` calls a legacy `somInit`; `somDestruct` runs the class's own clean-up, then its ancestors' — and the Toolkit headers give `somInitCtrl` / `somInitInfo` / `somDestructInfo` / `somAssignInfo`, but neither shows how generated `BeginInitializer` / `Init_<Ancestor>_…` / `EndDestructor` code indexes the mask and info arrays or turns a null `ctrl` into a block;
  - class building from IBM's static class data, Stage B2/B3 (`somBuildClass` itself is done, ADR-0073): initializers and direct-init classes, the initializer / destructor / assignment control blocks of `somParentMtabStruct`, DirectToSOM classes, migrated methods, `somDynamicSCI`, `inheritVars`, `somConstructClass`; `somBuildClass` and the other SOM 2.x entries reach IBM-built binaries only once IBM's ordinals above 47 are known;
  - the root methods that resolve to the not-implemented procedure: the copy, assign, dispatch, cast, reset and renew families, `somGetParents`, the class manager's repository and class-library methods;
  - class loading calling a program's own `SOMClassInitFuncName`.
- [ ] **C++**: SOM objects on the stack and in static storage; `somInitCtrl` / `somDestructCtrl` and multi-parent initialization order.
- [ ] **Class files**: `dllname` for `somLocateClassFile`.
- [ ] **Utilities and metadata**: `SOMU` classes, the interface repository (`SOMIR`), IDL metadata.
- [ ] **WPS classes**:
  - `WPObject`, `WPFileSystem`, `WPFolder` (on `WC_CONTAINER`), `WPDesktop`, `WPProgram`, `WPDataFile`;
  - persistence in OS2.INI;
  - settings notebooks;
  - `wpDragOver` / `wpDrop`.

### XE — a 64-bit OS/2-lineage platform (vision)

- [ ] **Format**:
  - an XE spec (header with `entry_rip: u64`, 64-bit objects, fixups, imports and exports) behind an MZ stub;
  - the stub prints "requires a 64-bit OS/2" and exits, so DOS, OS/2 and `file` all behave; record this in an ADR;
  - `src/xe/`, and `detect_format()` → `load_xe()` / `run_xe()`.
- [ ] **Long mode**: `EFER.LME`, 4-level paging, a 64-bit GDT with FS/GS for the TIB / PIB, thunks with System V AMD64 arguments.
- [ ] **APIs**:
  - `DOSCALLS64` (I/O, memory, threads, semaphores);
  - `PMWIN64`;
  - `UCONV64` on UTF-8.
- [ ] **Toolchain**: a `warpine-xe` crate, the `x86_64-warpine-xe` target, and `xe_hello` samples in Rust and C.
- [ ] **Dual ABI**: LX and XE side by side; `DosExecPgm` starting XE programs.
