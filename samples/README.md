# Sample OS/2 Applications

These are small OS/2 programs compiled with Open Watcom v2, used to test and verify Warpine's API emulation.

## Prerequisites

```bash
./vendor/setup_watcom.sh    # Download Open Watcom compiler (once)
```

## Building

```bash
make -C samples/<name>      # Build a single sample
```

For 4OS2 (requires fetching external source):

```bash
cd samples/4os2 && ./fetch_source.sh && make
```

## Sample Applications

| Sample | Description | Key APIs Tested |
|--------|-------------|-----------------|
| hello | "Hello, OS/2!" smoke test | DosWrite, DosExit |
| alloc_test | Memory allocation round-trip | DosAllocMem, DosFreeMem |
| file_test | File I/O: create, write, read, close | DosOpen, DosRead, DosWrite, DosClose |
| dir_test | Directory creation and enumeration | DosCreateDir, DosDeleteDir, DosFindFirst/Next |
| find_test | Wildcard file search | DosFindFirst, DosFindNext |
| findbuf_test | Multi-entry FILEFINDBUF3/4/4L packing | DosFindFirst buffer layout verification |
| fs_ops_test | Filesystem operations: move, delete, attribs | DosCopy, DosMove, DosDelete, DosQueryPathInfo |
| vfs_test | Comprehensive VFS test: seeks, truncate, metadata | DosSetFilePtr, DosSetFileSize, DosQueryFileInfo |
| thread_test | Basic threading | DosCreateThread, DosWaitThread |
| pipe_test | Inter-process pipe data transfer | DosCreatePipe, DosWrite, DosRead |
| mutex_test | Mutex semaphore recursive locking | DosCreateMutexSem, DosRequestMutexSem |
| muxwait_test | Multiplexed wait on event semaphores | DosCreateMuxWaitSem, DosWaitMuxWaitSem |
| queue_test | Inter-thread message queue | DosCreateQueue, DosWriteQueue, DosReadQueue |
| ipc_test | Event semaphore IPC with threads | DosCreateEventSem, DosPostEventSem, DosWaitEventSem |
| thunk_test | TIB/PIB layout, address conversion | DosGetInfoBlocks, DosQuerySysInfo |
| nls_test | National Language Support | DosQueryCtryInfo, DosQueryCp, DosMapCase |
| screen_test | VIO screen buffer and cursor | VioGetMode, VioSetCurPos, VioWrtTTY |
| pm_demo | PM GUI window with graphics and timers | WinCreateStdWindow, GpiBox, GpiLine, WinStartTimer |
| pm_hello | Minimal PM message box | WinInitialize, WinMessageBox |
| shapes | PM graphics: geometric shape drawing | WinCreateStdWindow, GpiSetColor, GpiBox, GpiLine |
| ne_hello | 16-bit NE format hello world (pure assembly, no Watcom CRT) | DosWrite (ord 138), DosExit (ord 5) via 16-bit Pascal thunk dispatch |
| dbcs_test | DBCS VIO cell classification and lead-byte query | DosSetProcessCp, DosQueryDBCSEnv, VioWrtCellStr, VioCheckCharType |
| uconv_test | UCONV.DLL Unicode conversion round-trip | DosLoadModule, DosQueryProcAddr, UniCreateUconvObject, UniUconvToUcs, UniUconvFromUcs, UniMapCpToUcsCp, UniFreeUconvObject |
| audio_test | MMPM/2 audio from a CLI program: beep tones and MCI command strings (silent when headless) | DosBeep, DosLoadModule/DosQueryProcAddr into MDM.DLL, mciSendString open waveaudio (device by name) / capability / close |
| pm_controls_test | PM built-in controls creation and API verification | WinCreateWindow (WC_STATIC/WC_BUTTON/WC_ENTRYFIELD/WC_SCROLLBAR/WC_LISTBOX/WC_MLE), WinSetWindowText, WinQueryWindowText, WinEnableWindow, WinIsWindowEnabled, LM_INSERTITEM/LM_QUERYITEMCOUNT |
| pm_menu_dlg_test | PM menu bar and modal About dialog loaded from MENU / DLGTEMPLATE resources; replaces the menu bar (destroy the FID_MENU window, WinLoadMenu, WM_UPDATEFRAME) with OK/FAILED checks | WinCreateStdWindow (FCF_MENU), WinWindowFromID (FID_MENU), WinLoadMenu, WM_UPDATEFRAME, WinDestroyWindow, WM_COMMAND, WinDlgBox, WinDefDlgProc, WinDismissDlg |
| pm_codepage_test | Message-queue and presentation-space codepages: WinQueryCpList, WinSetCp(1004) vs DosSetProcessCp, window text converted through the queue codepage, GpiSetCp / GpiQueryCp; ok/FAIL lines on stdout | WinQueryCpList, WinSetCp, WinQueryCp, DosSetProcessCp, WinSetWindowText, WinQueryWindowText, GpiSetCp, GpiQueryCp |
| pm_dialog_test | Modal dialog driven from the keyboard: Ctrl+D (accelerator table) opens it, typing fills the entry field, Tab / Down pick a list item, Enter presses the default button, Esc cancels, Right moves within the OK / Cancel group, Alt+N / Alt+C use the `~` mnemonics, Shift+Left / Shift+Home select, Ctrl+Insert / Shift+Insert copy and paste, Alt+F4 closes; also WinPeekMsg filters, the disabled owner, the destroyed dialog, `EN_`/`LN_` focus notifications, `LM_DELETEITEM` / `LM_DELETEALL` and a read-only entry field; ok/FAIL lines on stdout | WinCreateStdWindow (FCF_ACCELTABLE), WinLoadAccelTable, WinDlgBox, WinDefDlgProc, WM_INITDLG, WinPeekMsg, WinIsWindow, WinIsWindowEnabled, WinQueryFocus, WinQueryWindow, WinQuerySysValue, WM_SYSCOMMAND, WM_CONTROL, EM_SETREADONLY |
| pm_list_test | List box selection modes and MLE word wrap: `LM_SELECTITEM` / `LM_QUERYSELECTION` on a multiple selection, `LIT_CURSOR`, `MLM_SETWRAP` / `MLM_QUERYWRAP`; then the integration test clicks, Shift+clicks, Ctrl+clicks, double-clicks and presses Down / Space / Enter, and F3 reports; then the message queue lifecycle (one queue per thread, WinDestroyMsgQueue, a new queue); ok/FAIL line per check | WinCreateWindow (WC_LISTBOX LS_MULTIPLESEL / LS_EXTENDEDSEL, WC_MLE MLS_WORDWRAP), LM_SELECTITEM, LM_QUERYSELECTION, LN_ENTER, WM_BUTTON1DBLCLK, MLM_SETWRAP / MLM_QUERYWRAP |
| pm_msgbox_test | WinMessageBox as a modal PM dialog: four boxes answered by the integration test (Enter on the MB_DEFBUTTON2 default, Esc, a click on Yes, Enter), while the owner's timer keeps running and its frame is disabled; ok/FAIL line per check | WinMessageBox (MB_YESNOCANCEL / MB_OKCANCEL / MB_YESNO / MB_OK, MB_DEFBUTTON2), WinStartTimer, WinIsWindowEnabled |
| pm_typeahead_test | Type-ahead: the client is busy for 600 ms after the character `n`, then gives the focus to an entry field; the integration test types `ab` meanwhile, clicks a second entry field and types `cd`, and the sample checks that each field got its keys and the client only `n`; ok/FAIL line per check | WinCreateWindow (WC_ENTRYFIELD), WM_CHAR, DosSleep, WinSetFocus, WinQueryFocus, WinQueryWindowText, WinStartTimer |
| pm_font_test | GPI font selection and CJK text: Plex Sans/Mono/Serif, bold/italic, sizes, and Japanese/Korean/Chinese lines with measured-width rules | GpiCreateLogFont, GpiSetCharSet, GpiSetCharBox, GpiDeleteSetId, GpiQueryTextBox, GpiCharStringAt, GpiSetCp |
| pm_bench | PM self-benchmark: measures GPI drawing, `WinSendMsg` / `WinPostMsg` round trips, paint cycles and the `WM_TIMER` rate for about 0.3 s each, prints `pm_bench: <name>: <ops> ops in <ms> ms = …` lines plus `pm_bench: done` to stdout and shows them in the window; R runs it again | DosQuerySysInfo (QSV_MS_COUNT), GpiBox, GpiLine, GpiCharStringAt, WinSendMsg, WinPostMsg, WinInvalidateRect, WinStartTimer |
| pm_pel_test | GPI read-back and mix modes; one ok/FAIL line per check on stdout and in the window | GpiSetPel, GpiQueryPel, GpiBox, GpiLine, GpiSetMix (FM_XOR / FM_INVERT / FM_LEAVEALONE), GpiQueryMix |
| pm_bitmap_test | Memory presentation spaces, bitmaps and bit-block transfers; ok/FAIL lines | DevOpenDC (OD_MEMORY), GpiCreatePS (GPIA_ASSOC), GpiCreateBitmap, GpiSetBitmap, GpiBitBlt (1:1, stretched, ROP_DSTINVERT), GpiQueryBitmapBits, GpiLoadBitmap (RT_BITMAP resource) + WinDrawBitmap, the same-id RLE8 bitmap of BMPRES.DLL (DosLoadModule + GpiLoadBitmap with its hmod; built by the Makefile from bmpres.c / bmpres.rc), GpiCreateLogColorTable, GpiSetLineType, DevQueryCaps |
| pm_shapes_test | GPI arcs, area patterns and line widths, checked with GpiQueryPel, plus a picture of them; ok/FAIL lines | GpiFullArc (DRO_OUTLINE / DRO_FILL / DRO_OUTLINEFILL), GpiPartialArc, GpiPointArc, GpiSetArcParams / GpiQueryArcParams, GpiSetPattern / GpiQueryPattern, GpiSetPatternRefPoint, GpiBitBlt (ROP_PATCOPY), GpiSetLineWidth / GpiQueryLineWidth, GpiSetLineWidthGeom / GpiQueryLineWidthGeom |
| pm_audio_test | DosBeep timing and MMPM/2 waveaudio playback of a WAV file it writes; ok/FAIL lines | DosBeep, mciSendString (open … alias, status, play wait / notify, set volume, close), MM_MCINOTIFY, mciSendCommand (MCI_OPEN by device type ID, MCI_PLAY with MCI_WAIT), linked with mmpm2.lib |
| rust_hello | Rust no_std guest binary: "Hello from Rust on Warpine!" | DosWrite, DosExit (via warpine-os2 crate) |
| 4os2 | 4OS2 command shell (full interactive) | Nearly all DOSCALLS/VIO/KBD APIs; DLL loading (jpos2dll.dll) |
| socket_test | TCP/IP smoke test over loopback through SO32DLL loaded with DosLoadModule: socket, options, bind, listen, select, connect, accept, send / recv, name lookup | DosLoadModule, DosQueryProcAddr, sock_init, socket, setsockopt / getsockopt, bind, listen, select, connect, accept, send, recv, soclose, gethostbyname, getservbyname, sock_errno |
| cmd_test | `test.cmd`: a CMD.EXE script for the built-in shell (not a compiled program) exercising built-in commands, redirection (`>`, `>>`, `<`) and pipes; run with `cargo run -- CMD.EXE /C samples/cmd_test/test.cmd` | built-in CMD.EXE |

## Rust Guest Binaries

`rust_hello` is built with the Warpine Rust guest toolchain (nightly Rust + `lx-link` linker):

```bash
# Install once:
rustup toolchain install nightly --component rust-src
cargo build --bin lx_link && cp target/debug/lx_link ~/.cargo/bin/lx-link

# Build:
cd samples/rust_hello
cargo +nightly build \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -Z json-target-spec \
  --target ../../targets/i686-warpine-os2.json
```
