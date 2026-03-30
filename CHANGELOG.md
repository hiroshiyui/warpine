# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **GDB Remote Stub** — `--gdb <port>` enables GDB RSP over TCP; software breakpoints, single-step, Ctrl-C interrupt, full register/memory access (`gdb_stub.rs`)
- **Crash Dump Facility** — structured crash reports on fatal VMEXITs: registers, segment descriptors, stack, code bytes at EIP, written to `warpine-crash-<pid>.txt` and stderr (`crash_dump.rs`)
- **API Call Ring Buffer** — last 256 API calls captured unconditionally for crash post-mortem (`api_ring.rs`)
- **GDT 16-bit code/data aliases** — GDT[4] (selector 0x20) 16-bit data alias, GDT[5] (selector 0x28) 16-bit code alias for Far16 JMP FAR thunks; tile base shifted to GDT[6]
- **Structured API trace** — per-argument typed names and strace-like formatter (`api_trace.rs`); `WARPINE_TRACE=strace|json` output modes
- **DLL loader chain** — `DosLoadModule`/`DosQueryProcAddr`/`DosQueryModuleHandle` implemented; `jpos2dll.dll` loads at runtime with all 7 exports resolved (`lx_loader.rs`, `managers.rs`)
- **GDT tiling** — 4096 tiled 16-bit data descriptors (GDT[6..4102]) enabling 16:16 addressing; `DosFlatToSel`/`DosSelToFlat` use proper tile arithmetic
- **SDL2 VGA Text Renderer** — 640×400 text-mode window with CP437 8×16 font, CGA 16-colour palette, blinking cursor; CLI apps default to SDL2 text window (`text_renderer.rs`)
- **PM Renderer Abstraction** — `PmRenderer` trait with `Sdl2Renderer` and `HeadlessRenderer` backends
- **MMPM/2 Audio** — `DosBeep` sine-wave tones; `mciSendCommand`/`mciSendString` for waveaudio via SDL2 audio queue (`mmpm.rs`)
- **HPFS-compatible VFS** — `VfsBackend` trait, `HostDirBackend` with case-insensitive lookup, extended attributes, file locking, sandbox isolation (`vfs.rs`, `vfs_hostdir.rs`)
- **NE Format Parser** — parser for OS/2 1.x 16-bit (NE) executables (`src/ne/`)
- **API thunk auto-registration** — `api_registry.rs` static sorted table (124 entries) with O(log n) lookup; `--compat` compatibility report
- **NLS** — `DosQueryCtryInfo`, `DosQueryCp`, `DosMapCase`, `DosGetDateTime`
- **Text-mode console** — `VioManager` with screen buffer, cursor, ANSI output; VIOCALLS and KBDCALLS subsystems
- **4OS2 compatibility** — 4OS2 command shell runs interactively; `dir`, `set`, `ver`, `md`, `rd`, `copy`, `move`, `del`, `attrib`, `tree` all work
- 22 sample OS/2 applications in `samples/`
- 276 unit tests, 8 integration tests

### Fixed
- Detach relay thread in `DosWaitChild` instead of joining (deadlock fix)
- Rate-limit MMIO exit handlers to prevent 100% CPU spin
- Read IDT exception frame from `SS.base+SP` when SS is 16-bit
- Relay child stdout to parent `VioManager` for `EXEC_ASYNC` `DosExecPgm`
- Detect headless mode via `is_terminal()` instead of env var
- `DosQueryAppType` uses `DriveManager` VFS for path resolution
