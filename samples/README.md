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
| audio_test | MMPM/2 audio: beep tones and MCI command string | DosBeep, DosLoadModule/DosQueryProcAddr into MDM.DLL, mciSendString open/capability/close |
| pm_controls_test | PM built-in controls creation and API verification | WinCreateWindow (WC_STATIC/WC_BUTTON/WC_ENTRYFIELD/WC_SCROLLBAR/WC_LISTBOX/WC_MLE), WinSetWindowText, WinQueryWindowText, WinEnableWindow, WinIsWindowEnabled, LM_INSERTITEM/LM_QUERYITEMCOUNT |
| rust_hello | Rust no_std guest binary: "Hello from Rust on Warpine!" | DosWrite, DosExit (via warpine-os2 crate) |
| 4os2 | 4OS2 command shell (full interactive) | Nearly all DOSCALLS/VIO/KBD APIs; DLL loading (jpos2dll.dll) |

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
