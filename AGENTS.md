# AGENTS.md

Welcome, AI agent! This project is **Warpine**, an OS/2 compatibility layer for Linux. Your goal is to help maintain and extend its ability to run legacy OS/2 binaries natively using KVM.

## Project Overview

Warpine reimplements OS/2 APIs and loader logic, leveraging KVM for hardware-accelerated CPU execution. It targets 32-bit OS/2 binaries (LX format), with future goals for 16-bit NE/LE format support.

- **License:** GPL-3.0-only
- **Status:** Phases 1–4 complete (Foundation, Core Subsystem, Presentation Manager GUI, HPFS-compatible VFS). Phase 4.5 complete (16-bit thunk elimination via source-level 4OS2 recompilation). Phase 5 in progress (MMPM/2 audio baseline done; NE parser complete; NE loader/execution planned). See `doc/TODOs.md` for the full roadmap.

### Architecture

- `src/lx/` — Comprehensive parser for OS/2 Linear Executable (LX) format.
- `src/ne/` — Parser for OS/2 1.x 16-bit NE (New Executable) format. Foundation for Phase 5.
- `src/loader/` — KVM-based Virtual Machine Monitor (VMM). Manages guest memory, GDT/TIB/PIB, VMEXIT loop, API dispatch, and all OS/2 subsystem implementations.
  - `mod.rs` — Loader struct, SharedState, KVM setup
  - `api_dispatch.rs` — OS/2 API dispatch (ordinal → handler)
  - `api_trace.rs` — Structured tracing helpers (`ordinal_to_name`, `module_for_ordinal`)
  - `doscalls.rs` — DOSCALLS API implementations
  - `pm_win.rs` — PMWIN (Window Manager) implementations
  - `pm_gpi.rs` — PMGPI (Graphics) implementations
  - `viocalls.rs` — VIOCALLS (Video I/O) implementations
  - `kbdcalls.rs` — KBDCALLS (Keyboard) implementations
  - `console.rs` — VioManager: screen buffer, cursor, raw mode, ANSI output
  - `mmpm.rs` — MMPM/2 audio: MmpmManager, beep_tone, mciSendCommand/mciSendString
  - `vfs.rs` — VfsBackend trait, DriveManager, OS/2 filesystem types
  - `vfs_hostdir.rs` — HostDirBackend: HPFS-on-host-directory implementation
  - `locale.rs` — Os2Locale: country/codepage information
  - `vm_backend.rs` — VmBackend/VcpuBackend traits (KVM + mock implementations)
  - `guest_mem.rs` — Guest memory read/write helpers
  - `managers.rs` — Memory, handle, resource managers
  - `ipc.rs` — Semaphores and queues
  - `process.rs` — Process execution and directory tracking
  - `stubs.rs` — Stub handlers for unimplemented APIs
  - `constants.rs` — Named constants (addresses, message IDs, ordinal bases)
- `src/api.rs` — Small DosWrite/DosExit FFI bridge stubs.
- `src/gui.rs` — Presentation Manager GUI backend (SDL2). Event loop, Canvas/Texture rendering, input dispatch.
- `src/main.rs` — Entry point, CLI/PM detection, SDL2 init.
- `build.rs` — Linker search path for libSDL2 (via pkg-config).

## Setup & Build

- **Prerequisites:** Linux with KVM enabled (`/dev/kvm`), x86_64 CPU with VT-x/AMD-V, `libsdl2-dev`.
- **Build:** `cargo build`
- **Run:** `cargo run -- <path_to_os2_executable>`

## Testing

- **Command:** `cargo test`
- **Count:** ~155 tests covering LX/NE parsers, VFS, managers, semaphores, MMPM/2 audio, and API tracing.
- **Guidelines:** Every new feature or bug fix **MUST** include corresponding unit or integration tests. High coverage is required for the `lx` parser and API thunks.

## Code Style & Conventions

- **Language:** Rust (Edition 2024).
- **Safety:** Use Rust's safety features. When using `unsafe` for KVM or memory manipulation, provide clear safety comments. All code involving pointer arithmetic or guest-to-host transitions must be reviewed for buffer overflows.
- **Modular Design:** Keep API emulations, loader logic, and file format parsing strictly separated.
- **Stubs:** Use clear stubbing patterns for unimplemented APIs.
- **Abstractions:** Avoid "magic numbers"; use named constants for GDT entries, segment selectors, and API ordinals.

## Development Priorities

1. **Safety & Security:** Given the use of KVM and `unsafe` blocks, prevent buffer overflows and ensure guest memory isolation.
2. **Technical Integrity:** Prioritize readability and long-term maintainability. Refactor complex logic (like the VMEXIT loop) into clean abstractions.
3. **Validation:** Validation is not just running tests; it's ensuring behavioral, structural, and stylistic correctness.

## Contextual Precedence

`CLAUDE.md` contains the canonical project guidance. Check it for the latest project status and specific engineering requirements.
