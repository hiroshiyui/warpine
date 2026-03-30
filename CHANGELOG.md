# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **NE (16-bit OS/2 1.x) Execution** — full NE loader and 16-bit execution path: NE segments mapped into GDT-tiled guest memory; CALL FAR import fixups patched to code tile `NE_THUNK_CODE_SELECTOR` (0x87B0); API calls dispatched via `handle_ne_api_call()` with Pascal calling convention; `ne_hello` (pure assembly, no Watcom CRT) runs `DosWrite`+`DosExit` correctly; integration test added (`ne_exec.rs`)
- **Modifier key suppression** — pure modifier keys (LShift, RShift, Ctrl, Alt, CapsLock) no longer enqueue `KbdKeyInfo` events; fixes 4OS2 printing `*`/`6` on Shift press (`text_renderer.rs`)
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
- 281 unit tests, 9 integration tests

### Fixed
- **Far16 thunk bypass** — `#GP` on `66 EA` (Watcom `__Far16` JMP FAR) now fully bypassed in VMEXIT handler: scans stack for self-referential PUSH EBP frame, restores 32-bit caller state and returns EAX=0, avoiding all 16-bit execution (fixes 4OS2/JPOS2DLL crashes 606162, 623645, 630555)
- **SDL2 text-mode CPU idle** — replaced `poll_iter()` busy-loop with `wait_event_timeout(8)` so idle processes yield the CPU instead of pinning a full core
- Added 2048 code-tile GDT entries (GDT[4102..6149], type 0x9B) so `DosFlatToSel`/`DosSelToFlat` round-trip correctly for code-tile selectors
- Detach relay thread in `DosWaitChild` instead of joining (deadlock fix)
- Rate-limit MMIO exit handlers to prevent 100% CPU spin
- Read IDT exception frame from `SS.base+SP` when SS is 16-bit
- Relay child stdout to parent `VioManager` for `EXEC_ASYNC` `DosExecPgm`
- Detect headless mode via `is_terminal()` instead of env var
- `DosQueryAppType` uses `DriveManager` VFS for path resolution
