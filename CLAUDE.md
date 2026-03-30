# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Warpine is an OS/2 compatibility layer for Linux that runs 32-bit OS/2 (LX format) binaries natively using KVM for hardware-accelerated CPU emulation. Analogous to WINE but for OS/2 instead of Windows. Future goals include 16-bit NE/LE format support.

**License:** GPL-3.0-only

**Engineering policy — near-clean-room, blackbox implementation:** All API behaviour is derived from public documentation only (IBM *Control Program Programming Reference*, OS/2 Warp 4 Toolkit headers, IBM Developer Connection materials, osFree/ReactOS reference). No IBM-proprietary DLL binaries, no ROM dumps, and no disassembly of original OS/2 system libraries are used. Observable behaviour of Open Watcom-compiled OS/2 applications is the primary test oracle.

**Status:** Phases 1–6 complete. Phase 3.5 (Text-Mode Application Support) complete — 4OS2 command shell runs interactively with working `dir` command (date/time formatting verified). Phase 4.5 (16-bit Thunk Fix) complete — eliminated 16-bit thunks via source-level recompilation of 4OS2 (patches in `samples/4os2/patches/`). NLS (National Language Support) working — DosQueryCtryInfo, DosQueryCp, DosMapCase all verified. NE format parser complete. GUI backend migrated from winit+softbuffer to SDL2. MMPM/2 audio baseline: `DosBeep` plays real tones; MDM.DLL `mciSendCommand`/`mciSendString` for `waveaudio` devices implemented via SDL2 audio queue. API thunk auto-registration complete — `api_registry.rs` static sorted table replaces the ~120-arm match in `api_dispatch.rs` (now 124 entries). PM Renderer Abstraction complete — `PmRenderer` trait with `Sdl2Renderer` and `HeadlessRenderer` backends; `run_pm_loop()` replaces `run_gui_loop()`. Phase 6 (Text-Mode VGA Renderer) complete — `TextModeRenderer` trait with `Sdl2TextRenderer` (640×400 SDL2 window, CP437 8×16 font, CGA 16-colour palette, blinking cursor) and `HeadlessTextRenderer`; CLI apps default to SDL2 text window (`WARPINE_HEADLESS=1` for terminal fallback); `KbdKeyInfo`/`kbd_queue`/`kbd_cond`/`use_sdl2_text` in `SharedState` replaces termios stdin in SDL2 mode. Phase 7 baseline: DLL loader chain operational — `DosLoadModule`/`DosQueryProcAddr`/`DosQueryModuleHandle` implemented; `jpos2dll.dll` (4OS2 extension DLL) loads and exports resolved at runtime. GDT tiling complete — 4096 tiled 16-bit data descriptors (GDT[6..4102]) enable Far16 thunks; GDT[4]=16-bit data alias (selector 0x20), GDT[5]=16-bit code alias (selector 0x28) for Far16 JMP FAR; `DosFlatToSel`/`DosSelToFlat` use proper tile arithmetic; 16:16 LX fixups write correct tile selectors. GDB Remote Stub complete — `--gdb <port>` enables GDB RSP over TCP (`gdbstub 0.7`); software breakpoints, single-step (`KVM_GUESTDBG_SINGLESTEP`), Ctrl-C interrupt, full register/memory access; `src/loader/gdb_stub.rs`. See `doc/TODOs.md` for the full roadmap.

## Build & Run

```bash
cargo build                              # Debug build
cargo run -- <path_to_os2_executable>    # Run an OS/2 binary
cargo run -- samples/hello/hello.exe     # Example: run hello world
cargo test                               # Unit tests (276 tests: LX/NE parsers, VFS, managers, MMPM, API registry, HeadlessRenderer, scan codes, text renderer, DLL loader, crash dump, API ring buffer)
```

**Prerequisites:** Linux with KVM enabled (`/dev/kvm`), x86_64 CPU with VT-x/AMD-V, Rust 2024 edition, `libsdl2-dev` (for PM/GUI window support).

**Sample OS/2 apps** are in `samples/` (hello, alloc_test, file_test, pipe_test, 4os2, etc.). Build them with Open Watcom: `./vendor/setup_watcom.sh` then `make -C samples/<name>`. For 4OS2: `cd samples/4os2 && ./fetch_source.sh && make`.

## Architecture

### Execution flow

1. **Parse** — `main.rs` calls `LxFile::open()` to parse the MZ+LX executable (headers, object table, page map, fixup records, imports)
2. **Load** — `Loader::load()` maps executable pages into KVM guest memory (128MB) and applies relocations via `apply_fixups()`
3. **Execute** — `Loader::run()` sets up TIB/PIB, creates a vCPU, and enters the VMEXIT loop (`run_vcpu()`)
4. **API thunking** — Guest API calls hit INT 3 breakpoints at magic addresses (MAGIC_API_BASE = 0x01000000), causing VMEXIT_DEBUG → `handle_api_call()` dispatches to host-side Rust implementations by ordinal number

### Key modules

- **`src/lx/`** — LX executable format parser (`header.rs` for binary structures, `lx.rs` for orchestration). Unit tests live here.
- **`src/ne/`** — NE (New Executable) format parser for OS/2 1.x 16-bit apps (`header.rs` for structures, `ne.rs` for orchestration). 16 unit tests. Phase 5 will add NE loading/execution.
- **`src/loader/`** — The core: KVM VMM, memory manager, handle manager, semaphore manager, queue manager, VMEXIT loop, and OS/2 API handler functions. Split into `mod.rs` (loader core + `SharedState` with `DllManager`), `api_registry.rs` (static sorted API thunk table, 124 entries), `api_dispatch.rs` (registry lookup + sub-dispatcher routing), `api_trace.rs` (ordinal-to-name/module/arg-names helpers + `format_call()` strace formatter), `doscalls.rs`, `viocalls.rs`, `kbdcalls.rs`, `console.rs`, `pm_win.rs`, `pm_gpi.rs`, `stubs.rs` (includes `DosLoadModule`/`DosQueryProcAddr`), `process.rs`, `managers.rs` (`DllManager`/`LoadedDll` for runtime-loaded user DLLs), `constants.rs`, `mmpm.rs` (MMPM/2 audio: MmpmManager, beep_tone, mciSendCommand/String), `lx_loader.rs` (`load_dll()`, `find_dll_path()`), `gdb_stub.rs` (`GdbState`, `WarpineTarget`, `GdbBlockingEventLoop`, `launch_gdb_stub` — GDB RSP over TCP). Phase 4 adds `vfs.rs` (VfsBackend trait, DriveManager) and `vfs_hostdir.rs` (HostDirBackend) — see developer guide for VFS architecture.
- **`src/api.rs`** — Small module with `DosWrite`/`DosExit` implementations and FFI bridge stubs.
- **`src/gui/`** — GUI and text-mode backends. `message.rs`: `GUIMessage` channel. `renderer.rs`: `PmRenderer` trait + `run_pm_loop()`. `render_utils.rs`: geometry helpers. `headless.rs`: `HeadlessRenderer`. `sdl2_renderer.rs`: `Sdl2Renderer` (PM SDL2 backend), `push_msg`, scancode/VK tables. `text_renderer.rs`: `TextModeRenderer` trait, `Sdl2TextRenderer` (640×400 VGA text window), `HeadlessTextRenderer`, `run_text_loop()`, `get_cp437_glyph()`, `CGA_PALETTE`.

### Concurrency model

Each OS/2 thread maps to a native Rust thread with its own KVM vCPU. `SharedState` wraps all managers in `Arc<Mutex<...>>` for cross-thread access. Semaphores use `Arc<(Mutex<State>, Condvar)>`.

### Important constants (in constants.rs)

- `MAGIC_API_BASE` (0x01000000) — API thunk stub area
- `EXIT_TRAP_ADDR` (0x010003FF) — Special exit breakpoint
- `DYNAMIC_ALLOC_BASE` (0x02000000) — Guest memory allocation pool
- `TIB_BASE` (0x00090000), `PIB_BASE` (0x00091000) — Thread/Process info blocks (must stay below 0x100000 for 16-bit segment arithmetic)
- `KBDCALLS_BASE` (4096), `VIOCALLS_BASE` (5120), `SESMGR_BASE` (6144), `NLS_BASE` (7168), `MSG_BASE` (8192), `MDM_BASE` (10240) — Ordinal offset bases for subsystem dispatch; `STUB_AREA_SIZE` (12288)

### OS/2 PIB layout (key offsets)

- `+0x00` pib_ulpid, `+0x04` pib_ulppid, `+0x08` pib_hmte, `+0x0C` pib_pchcmd, `+0x10` pib_pchenv

## Conventions

- **Modular separation:** Keep API emulations, loader logic, and format parsing in their respective modules.
- **Every new feature or bug fix must include tests.** High coverage required for the LX parser and API thunks.
- **Unsafe code** is expected for KVM/guest memory operations but must have clear safety justification and enforce guest memory isolation. All code involving pointer arithmetic or guest-to-host transitions must be reviewed for buffer overflows.
- **Use named constants** — no magic numbers for GDT entries, segment selectors, or API ordinals.
- **Stubbing pattern** — unimplemented APIs should use clear stubs to allow incremental progress.
- OS/2 paths (`C:\path`) are translated to Unix paths by replacing backslashes. OS/2 error codes (u32) are returned in RAX.
- Ensure this document is updated to reflect any changes in the workflow and maintain consistency.

### While Planning, Refactoring & Doing Code Review

- When a feature requirement is unclear or ambiguous, seek clarification on definition and scope rather than guessing.

### While Coding

- **Always** try to write test sample OS/2 applications (in `samples/`) to verify assumptions, evaluations.

### After Every Change

1. **Always** update all relevant documents (`README.md`, `doc/*.md` and this file `CLAUDE.md`...)
2. **Always** add essential but missing tests to improve test coverage and ensure code quality
3. **Always** check if there is any missing or incomplete test
4. Remove the finishied tasks from TODOs
5. When a bug is discovered, **always** check for similar issues across the project after applying the fix

### Release Engineering

When creating a new release:

1. Update `CHANGELOG.md` with the new version entry (follow [Keep a Changelog](https://keepachangelog.com/) format)
2. Update `version` in `Cargo.toml` to match the new tag version
3. Commit, push, and create the git tag (e.g. `v1.1.21`)
4. Push the tag (`git push --tags`)
5. Create the GitHub release via `gh release create`

### Code Organization

- Commit by topic — group related files per commit

### Security Rules

- **Always** take care about hypervisor escape prevention
- Memory safety is top priority
