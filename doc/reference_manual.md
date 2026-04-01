# Warpine Reference Manual

Warpine is an OS/2 compatibility layer for Linux that runs 32-bit OS/2 (LX format) and 16-bit OS/2 1.x (NE format) applications natively using KVM hardware virtualization.

---

## Table of Contents

1. [Command-Line Interface](#command-line-interface)
2. [Environment Variables](#environment-variables)
3. [Execution Modes](#execution-modes)
4. [Builtin CMD.EXE Shell](#builtin-cmdexe-shell)
5. [Virtual Filesystem](#virtual-filesystem)
6. [Logging and Tracing](#logging-and-tracing)
7. [GDB Debugging](#gdb-debugging)
8. [Crash Dumps](#crash-dumps)
9. [API Compatibility Report](#api-compatibility-report)
10. [Implemented APIs](#implemented-apis)
11. [OS/2 Error Codes](#os2-error-codes)
12. [Guest Memory Layout](#guest-memory-layout)

---

## Command-Line Interface

### Usage

```
warpine <os2_executable> [--gdb <port>]
warpine --compat
```

### Arguments

| Argument | Type | Description |
|----------|------|-------------|
| `<os2_executable>` | Path | Path to an OS/2 LX or NE executable |
| `--gdb <port>` | u16 | Enable GDB Remote Stub on the given TCP port |
| `--compat` | Flag | Print API compatibility report and exit |

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Guest exited normally via `DosExit()` |
| 1 | Warpine startup error (bad arguments, file not found, load failure) |
| Other | Guest exit code (passed through from `DosExit()`) |

---

## Environment Variables

### Warpine-Specific

| Variable | Values | Default | Effect |
|----------|--------|---------|--------|
| `WARPINE_TRACE` | _(unset)_, `0`, `false` | _(unset)_ | Default pretty log format |
| `WARPINE_TRACE` | `1`, `strace` | — | Compact strace-like API call format |
| `WARPINE_TRACE` | `json` | — | JSON Lines (one object per event) |
| `RUST_LOG` | Filter string | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error`. Module-scoped filters supported (e.g. `warpine::loader=debug`) |

### Filesystem

| Variable | Default | Effect |
|----------|---------|--------|
| `XDG_DATA_HOME` | `$HOME/.local/share` | Base directory for the guest C: drive |
| `HOME` | _(from passwd)_ | Fallback when `XDG_DATA_HOME` is unset |

The default C: drive maps to `$XDG_DATA_HOME/warpine/drive_c/` and is created automatically. If neither variable is set, falls back to `./drive_c/` in the current working directory.

### Locale

Warpine reads the host locale to populate OS/2 `COUNTRYINFO` fields:

| Variable | Priority |
|----------|----------|
| `LC_ALL` | 1 (highest) |
| `LC_CTYPE` | 2 |
| `LANG` | 3 (lowest) |

The language/territory is parsed (e.g. `ja_JP.UTF-8` → country=Japan) and mapped to OS/2 date format, time format, separators, and currency symbol. Date format: MDY for `en_US`, YMD for `ja`/`ko`/`zh`/`hu`/`sv`/`fi`, DMY for all others. Time format: 12-hour for `en_*`, 24-hour for all others.

---

## Execution Modes

Warpine automatically selects an execution mode based on the executable and environment:

### PM (GUI) Mode

**Trigger:** Executable imports `PMWIN` or `PMGPI`.

- Opens an SDL2 window for Presentation Manager rendering.
- Main thread runs the SDL2 event loop; vCPU runs on a worker thread.
- Window title: `Warpine — <filename>`.
- All PM APIs (windows, menus, dialogs, graphics, clipboard) are active.

### CLI Text Mode (SDL2)

**Trigger:** CLI executable (no PM imports) and stdout is a terminal.

- Opens a 640×400 SDL2 window rendering an 80×25 character grid.
- CP437 8×16 VGA font, CGA 16-colour palette, blinking cursor.
- vCPU runs on a worker thread; main thread runs the text renderer.
- Keyboard input delivered via SDL2 events.

### CLI Headless Mode

**Trigger:** CLI executable and stdout is **not** a terminal (piped or redirected).

- No SDL2 window. VIO output rendered as ANSI escape sequences on stdout.
- CP437 high bytes (0x80–0xFF) converted to Unicode for terminal display.
- vCPU runs on the main thread.
- Keyboard input read from raw terminal (termios).
- Child processes spawned via `DosExecPgm` automatically use headless mode.
- Detection is automatic via `is_terminal()` — no environment variable needed.

---

## Builtin CMD.EXE Shell

Warpine includes a native Rust command shell that requires no Open Watcom compiler or 4OS2 installation.

### Invocation

```bash
warpine CMD.EXE                    # Interactive session (SDL2 text window)
WARPINE_HEADLESS=1 warpine CMD.EXE # Terminal/headless mode
warpine CMD.EXE /C "DIR C:\"      # Run one command and exit
warpine CMD.EXE /K "VER"          # Run one command then stay interactive
```

The shell can also be launched from within a running OS/2 application via `DosExecPgm("CMD.EXE")` or `DosExecPgm("OS2SHELL.EXE")`.

**SDL2 vs terminal:** When stdout is a terminal and `WARPINE_HEADLESS` is not set, the shell opens a 640×400 SDL2 VGA text window (same as any CLI app). Set `WARPINE_HEADLESS=1` or pipe stdout to force terminal/headless mode.

### Flags

| Flag | Effect |
|------|--------|
| `/C <command>` | Execute `<command>` and exit |
| `/K <command>` | Execute `<command>` then enter interactive mode |
| (none) | Enter interactive mode immediately |

### Line editor

The prompt shows the current drive and directory: `[C:\path] `.

| Key | Action |
|-----|--------|
| Enter | Submit line |
| Backspace | Erase last character |
| Esc | Clear the current line |
| ↑ | Previous history entry |
| ↓ | Next history entry |

### Built-in commands

| Command | Syntax | Description |
|---------|--------|-------------|
| `DIR` | `DIR [path]` | List directory contents. Directories are shown first, then files, each with date/time and size. |
| `CD` | `CD [path]` | Print or change the current directory. `CD \` goes to root. `CD ..` goes up one level. |
| `C:` – `Z:` | `C:` | Switch the current drive. |
| `SET` | `SET [VAR[=value]]` | Without args: list all environment variables. `SET VAR` shows matching vars. `SET VAR=value` sets a variable. `SET VAR=` unsets it. |
| `ECHO` | `ECHO [text]` | Write text to the console. `ECHO.` prints a blank line. `ECHO ON`/`OFF` not supported. |
| `CLS` | `CLS` | Clear the screen. |
| `VER` | `VER` | Print the Warpine version. |
| `TYPE` | `TYPE <file>` | Print file contents to the console. |
| `MD` | `MD <path>` | Create a directory (and any missing parent directories). |
| `RD` | `RD <path>` | Remove an empty directory. |
| `DEL` | `DEL <file>` | Delete a file. |
| `HELP` | `HELP` | List available commands. |
| `EXIT` | `EXIT [code]` | Exit the shell with an optional numeric exit code (default 0). |

### Running OS/2 programs

Any token that is not a built-in is treated as a program name. The shell searches the current directory, then the `PATH` environment variable, appending `.EXE` if no extension is given. Example:

```
[C:\] hello
Hello, OS/2!
```

If `hello.exe` is not found, the shell tries `hello.cmd`.

### .CMD scripts

Run a `.CMD` file by typing its name (with or without the `.CMD` extension) or using `/C`:

```
[C:\] warpine CMD.EXE /C "MYSCRIPT.CMD"
```

Supported script directives:

| Directive | Description |
|-----------|-------------|
| `REM text` | Comment — ignored |
| `:: text` | Alternative comment |
| `ECHO text` | Write to console |
| `SET VAR=value` | Set environment variable |
| `IF [NOT] EXIST file cmd` | Execute `cmd` if file exists (or not) |
| `IF [NOT] ERRORLEVEL n cmd` | Execute `cmd` if last exit code ≥ n (or <n for NOT) |
| `FOR %%V IN (a b c) DO cmd` | Loop over space-separated list; `%%V` is replaced by each item |
| `GOTO label` | Jump to `:label` |
| `CALL script` | Execute another `.CMD` file |
| `PAUSE` | Print "Press any key to continue…" and wait |

---

## Virtual Filesystem

### Default Drive Mapping

| Drive | Host Path | Label | Notes |
|-------|-----------|-------|-------|
| C: | `$XDG_DATA_HOME/warpine/drive_c/` | OS2 | Auto-created; default boot drive |

Other drives (A:–B:, D:–Z:) are not mounted by default.

### Path Translation

| OS/2 Path | Resolution |
|-----------|------------|
| `C:\dir\file.exe` | Absolute drive path → host directory |
| `\dir\file.exe` | Absolute on current drive |
| `foo\bar.txt` | Relative to current drive + current directory |
| `C:foo.txt` | Relative to C: current directory |

Backslashes are translated to forward slashes. Paths cannot escape the volume root (sandbox enforced).

### Filesystem Features

- **Case-insensitive, case-preserving** lookup (optimistic stat + readdir fallback)
- **Long filenames** up to 254 characters (HPFS semantics)
- **Extended attributes** via Linux xattrs (`user.os2.ea.*`) with `.os2ea/` sidecar fallback
- **File locking** via `fcntl(F_SETLK)` — sharing modes: DENY_NONE, DENY_WRITE, DENY_READ, DENY_READWRITE
- **OS/2 wildcard matching** (`*`, `?`) in `DosFindFirst`/`DosFindNext`; `*.*` matches all files (HPFS)
- **Sandbox isolation** — path traversal beyond the volume root is blocked

### Reserved Device Names

| Name | Behaviour |
|------|-----------|
| `NUL` | Discards writes; reads return EOF |
| `CON` | Console I/O |
| `CLOCK$` | Real-time clock (via `DosGetDateTime`) |
| `KBD$` | Keyboard input |
| `SCREEN$` | Screen output |

---

## Logging and Tracing

### Log Levels

Control with `RUST_LOG`:

```bash
RUST_LOG=debug cargo run -- samples/hello/hello.exe           # All debug output
RUST_LOG=warpine::loader=debug cargo run -- app.exe           # Loader module only
RUST_LOG=warpine::gui=debug cargo run -- pm_demo.exe          # GUI module only
```

### Trace Formats

Control with `WARPINE_TRACE`:

```bash
# Default: pretty format with timestamps
cargo run -- app.exe

# strace-like: compact one-line-per-call
WARPINE_TRACE=strace RUST_LOG=debug cargo run -- app.exe

# JSON Lines: machine-readable
WARPINE_TRACE=json RUST_LOG=debug cargo run -- app.exe
```

At `debug` level, each intercepted API call is logged with its vCPU ID, function name, arguments (hex), and return value.

---

## GDB Debugging

### Starting the GDB Stub

```bash
cargo run -- --gdb 1234 samples/hello/hello.exe
```

The vCPU pauses at the entry point and waits for a GDB connection.

### Connecting

```bash
gdb -ex 'target remote :1234'
```

### Supported Operations

| Command | Description |
|---------|-------------|
| `c` | Continue execution |
| `si` | Single-step one instruction |
| `b *0x<addr>` | Set software breakpoint |
| `info reg` | Show all registers |
| `p $eax` | Print individual register |
| `x/<N>x <addr>` | Examine memory (hex) |
| `x/<N>i <addr>` | Disassemble at address |
| `set {int}<addr> = <val>` | Write guest memory |
| `Ctrl-C` | Interrupt (pause vCPU) |

### How It Works

- **Software breakpoints:** INT 3 (0xCC) patched into guest memory; original byte restored on hit; RIP rewound to breakpoint address.
- **Single-step:** `KVM_GUESTDBG_SINGLESTEP` flag on vCPU.
- **Memory access:** Full 128 MB guest flat address space.
- **Synchronisation:** `GdbState` (Mutex + Condvar) coordinates vCPU and GDB threads; `AtomicBool` for Ctrl-C.

---

## Crash Dumps

### When They Are Generated

- Unhandled guest CPU exception (divide-by-zero, page fault, GPF, etc.)
- Triple fault / KVM shutdown
- Unhandled VMEXIT reason
- KVM hypervisor error
- Unexpected breakpoint (INT 3 at non-API address)

### Output

- **File:** `warpine-crash-<pid>.txt` in the current working directory
- **Stderr:** Same report printed simultaneously

### Contents

| Section | Data |
|---------|------|
| Event | Exception type, vector number, error code |
| Registers | EAX–EDI, ESP, EBP, EIP, EFLAGS, all segment registers |
| Segments | Base, limit, and type for CS, DS, ES, SS, FS, GS |
| Stack | Top 32 dwords from ESP (hex + ASCII) |
| Code | 32 bytes at EIP (hex dump) |
| API History | Last 256 API calls from the ring buffer |

The API ring buffer is populated unconditionally (not gated on log level), so crash dumps always include call history.

---

## API Compatibility Report

Run `warpine --compat` to print a module-grouped report of all implemented APIs:

```bash
$ warpine --compat
Warpine OS/2 API Compatibility Report
======================================

DOSCALLS  (99 implemented, 8 stub)
  [stub]  [  241]  DosConnectNPipe
          [  213]  DosFlatToSel
          [  234]  DosExit
  ...
```

APIs marked `[stub]` accept calls and return `NO_ERROR` but perform no operation.

---

## Implemented APIs

### DOSCALLS (107 APIs: 99 implemented, 8 stub)

**File I/O:**
DosOpen (273), DosClose (257), DosRead (281), DosWrite (282), DosDelete (259), DosCopy (258), DosMove (271), DosSetFilePtr (256), DosSetFileSize (272), DosQueryFileInfo (279), DosSetFileInfo (218), DosQueryPathInfo (223), DosSetPathInfo (219), DosEnumAttribute (372)

**Directory Operations:**
DosCreateDir (270), DosDeleteDir (226), DosQueryCurrentDir (274), DosSetCurrentDir (255), DosQueryCurrentDisk (275), DosSetDefaultDisk (220), DosFindFirst (264), DosFindNext (265), DosFindClose (263)

**Memory:**
DosAllocMem (299), DosFreeMem (304), DosSetMem (305), DosQueryMem (306), DosAllocSharedMem (300), DosGetSharedMem (302), DosGetNamedSharedMem (301), DosSubAllocMem (344), DosSubFreeMem (345), DosSubSetMem (346)

**Threading:**
DosCreateThread (311), DosWaitThread (349), DosGetInfoBlocks (312), DosSleep (229), DosResumeThread (237), DosSuspendThread (238), DosKillThread (111)

**Semaphores:**
DosCreateEventSem (324), DosOpenEventSem (325), DosCloseEventSem (326), DosPostEventSem (328), DosWaitEventSem (329), DosResetEventSem (327), DosCreateMutexSem (331), DosOpenMutexSem (332), DosCloseMutexSem (333), DosRequestMutexSem (334), DosReleaseMutexSem (335), DosCreateMuxWaitSem (337), DosCloseMuxWaitSem (339), DosWaitMuxWaitSem (340), DosAddMuxWaitSem (341)

**Process:**
DosExit (234), DosExecPgm (283), DosWaitChild (280), DosKillProcess (235), DosQueryAppType (323)

**Modules:**
DosLoadModule (318), DosFreeModule (322), DosQueryModuleHandle (319), DosQueryModuleName (320), DosQueryProcAddr (321)

**System:**
DosQuerySysInfo (348), DosGetDateTime (230), DosFlatToSel (213), DosSelToFlat (227), DosSetExceptionHandler (354), DosUnsetExceptionHandler (355)

**Exception Handling:**
DosRaiseException (356), DosUnwindException (357)

**Miscellaneous:**
DosError (212), DosSetMaxFH (209), DosBeep (286), DosQueryHType (224), DosSetFileLocks (268), DosResetBuffer (254), DosQuerySysState (368)

**Stubs (no-op):**
DosConnectNPipe (241), DosCreateNPipe (243), DosSetNPHState (250), DosDebug (317), DosDeleteMuxWaitSem (342), DosShutdown (415), DOS16REQUESTVDD (267), DosFSCtl (285)

### QUECALLS (7 APIs)

DosCreateQueue (1040), DosOpenQueue (1039), DosCloseQueue (1035), DosReadQueue (1033), DosWriteQueue (1038), DosPurgeQueue (1034), DosQueryQueue (1036)

### NLS (4 APIs)

NlsQueryCp (7173), NlsQueryCtryInfo (7174), NlsMapCase (7175), NlsGetDBCSEv (7176)

### MSG (2 APIs)

DosPutMessage (8195), DosGetMessage (8198)

### MDM / MMPM/2 (4 APIs)

mciSendCommand (10241), mciSendString (10242), mciFreeBlock (10243), mciGetLastError (10244)

Supported MCI commands: `MCI_OPEN`, `MCI_CLOSE`, `MCI_PLAY`, `MCI_STOP`, `MCI_STATUS`. Device type: `waveaudio` (WAV playback via SDL2 audio queue).

### UCONV (5 APIs)

UniCreateUconvObject (12289), UniFreeUconvObject (12290), UniUconvToUcs (12291), UniUconvFromUcs (12292), UniMapCpToUcsCp (12294)

UCS-2 name parser accepts `"IBM-NNN"`, `"CP-NNN"`, and `"UTF-8"` (case-insensitive). Conversion delegates to `cp_decode`/`cp_encode` in `codepage.rs`.

### VIOCALLS (21 APIs: 17 implemented, 4 stub)

**Implemented:**
VioGetAnsi (3), VioSetAnsi (5), VioScrollUp (7), VioScrollDn (8), VioGetCurPos (9), VioSetCurPos (15), VioWrtTTY (19), VioGetMode (21), VioSetMode (22), VioReadCellStr (24), VioWrtNAttr (26), VioSetCurType (32), VioGetCurType (33), VioCheckCharType (39), VioGetConfig (46), VioWrtCharStrAtt (48), VioWrtNCell (52)

**Stubs:**
VioGetBuf (31), VioSetCp (42), VioShowBuf (43), VioSetState (51)

### KBDCALLS (3 APIs)

KbdCharIn (4), KbdStringIn (9), KbdGetStatus (10)

### PMWIN (73 APIs)

Window management, message queues, painting, input, dialogs, menus, clipboard, and resource loading. Key APIs:

WinInitialize (763), WinTerminate (888), WinCreateMsgQueue (716), WinDestroyMsgQueue (726), WinGetMsg (915), WinDispatchMsg (728), WinPostMsg (796), WinSendMsg (866), WinCreateWindow (907), WinCreateStdWindow (908), WinDestroyWindow (729), WinShowWindow (883), WinSetWindowPos (903), WinSetWindowText (904), WinQueryWindowText (841), WinDefWindowProc (728), WinRegisterClass (926), WinBeginPaint (703), WinEndPaint (729), WinMessageBox (793), WinStartTimer (898), WinStopTimer (900), WinSetActiveWindow (875), WinSetFocus (879), WinSetCapture (877), WinMapWindowPoints (789), WinQueryWindowRect (854), WinQueryWindowPos (852), WinLoadMenu (781), WinGetClipbrdData (6), WinInvalidateRect (27), WinEnableWindow (735), WinIsWindowEnabled (773), WinQueryWindowPos (852), WinSetParent (859), WinQueryDlgItemText (815)

### PMGPI (30 APIs: 23 implemented, 7 stub)

**Implemented:**
GpiBox (356), GpiCharString (358), GpiCharStringAt (359), GpiCreatePS (369), GpiDestroyPS (379), GpiCreateLogFont (381), GpiDeleteSetId (385), GpiErase (389), GpiFullArc (392), GpiLine (398), GpiMove (404), GpiQueryCurrentPosition (416), GpiQueryFonts (459), GpiQueryFontMetrics (464), GpiQueryTextBox (476), GpiSetCharSet (481), GpiSetCharBox (482), GpiSetBackMix (503), GpiSetMix (509), GpiSetColor (517), GpiSetBackColor (518), GpiQueryColor (520), GpiQueryBackColor (521)

**Stubs:**
GpiSetArcParams (353), GpiLoadFonts (399), GpiLoadPublicFonts (400), GpiUnloadPublicFonts (401), GpiSetLineType (527), GpiSetLineWidth (529), GpiSetLineWidthGeom (530)

### Summary

| Module | Total |
|--------|-------|
| DOSCALLS | 113 |
| QUECALLS | 7 |
| NLS | 4 |
| MSG | 2 |
| MDM | 4 |
| UCONV | 5 |
| VIOCALLS | 21 |
| KBDCALLS | 3 |
| PMWIN | 73 |
| PMGPI | 30 |
| **Total** | **262** |

---

## OS/2 Error Codes

All OS/2 API calls return a `u32` error code in the EAX register. 0 = success.

| Code | Name | Meaning |
|------|------|---------|
| 0 | `NO_ERROR` | Success |
| 1 | `INVALID_FUNCTION` | Function not implemented |
| 2 | `FILE_NOT_FOUND` | File does not exist |
| 3 | `PATH_NOT_FOUND` | Path does not exist |
| 4 | `TOO_MANY_OPEN_FILES` | Handle limit reached |
| 5 | `ACCESS_DENIED` | Permission denied |
| 6 | `INVALID_HANDLE` | Invalid handle |
| 8 | `NOT_ENOUGH_MEMORY` | Allocation failed |
| 15 | `INVALID_DRIVE` | Drive not mounted |
| 18 | `NO_MORE_FILES` | Search completed |
| 32 | `SHARING_VIOLATION` | Conflicting share mode |
| 33 | `LOCK_VIOLATION` | File region locked |
| 80 | `FILE_EXISTS` | File already exists |
| 87 | `INVALID_PARAMETER` | Invalid argument |
| 111 | `BUFFER_OVERFLOW` | Buffer too small |
| 112 | `DISK_FULL` | No space left |
| 124 | `INVALID_LEVEL` | Invalid info level |
| 145 | `DIR_NOT_EMPTY` | Directory not empty |
| 206 | `FILENAME_EXCED_RANGE` | Filename too long |
| 254 | `EA_NOT_FOUND` | Extended attribute not found |

---

## Guest Memory Layout

All addresses are guest physical (GPA = GVA in Warpine's flat model):

| Address | Size | Purpose |
|---------|------|---------|
| `0x00001000` | — | Executable pages (loaded from LX objects) |
| `0x00080000` | 49,200 B | GDT (6150 entries: 6 fixed + 4096 data tiles + 2048 code tiles) |
| `0x0008D000` | 256 B | IDT (32 exception vectors) |
| `0x0008D800` | — | IDT handler stubs |
| `0x00090000` | 4 KB | Thread Information Block (TIB) — thread 1 |
| `0x00091000` | 4 KB | Process Information Block (PIB) |
| `0x00092000` | 4 KB | Environment string block |
| `0x00100000` | — | NE (16-bit) segment base |
| `0x00F00000` | — | NE API thunk stubs |
| `0x01000000` | 12,288 B | LX API thunk stub area (`MAGIC_API_BASE`) |
| `0x010003FE` | 1 B | PM callback return trap |
| `0x010003FF` | 1 B | Exit trap address |
| `0x02000000` | ~30 MB | Dynamic allocation pool (`DosAllocMem`) |

### GDT Layout

| Index | Selector | Description |
|-------|----------|-------------|
| 0 | — | Null descriptor |
| 1 | 0x08 | 32-bit code: base 0, limit 4 GB, execute/read |
| 2 | 0x10 | 32-bit data: base 0, limit 4 GB, read/write |
| 3 | 0x18 | FS segment (TIB pointer) |
| 4 | 0x20 | 16-bit data alias: base 0, limit 64 KB |
| 5 | 0x28 | 16-bit code alias: base 0, limit 64 KB (Far16 thunk entry) |
| 6–4101 | 0x30+ | 4096 tiled 16-bit **data** descriptors (one per 64 KB, DPL=2, for 16:16 addressing and NE segment DS/ES loads) |
| 4102–6149 | 0x8030+ | 2048 tiled 16-bit **code** descriptors (same bases, execute/read; used by CALL FAR fixups and NE thunk code tile) |
