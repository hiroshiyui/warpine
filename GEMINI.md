# Warpine: OS/2 Compatibility Layer

Warpine is a compatibility layer designed to allow IBM OS/2 applications and games to run natively on Unix-like operating systems. It follows an architectural approach similar to WINE, aiming to reimplement OS/2 APIs and loader logic rather than relying on full system emulation.

## Project Overview

- **Language:** Rust (Edition 2024)
- **Goal:** Native execution of 16-bit and 32-bit OS/2 binaries (LX, LE, NE formats) on Linux/Unix.
- **Current Status:** Phase 1 (Foundation). Basic LX parser stub, loader structure, and minimal `DOSCALLS.DLL` API thunks (`DosWrite`, `DosExit`) are implemented.

## Architecture

The project is structured into several core modules:

- `src/lx/`: Executable parser for OS/2 Linear Executable (LX) and related formats.
- `src/loader.rs`: Responsible for mapping executable sections into memory and preparing the execution environment.
- `src/api.rs`: Emulation of OS/2 System DLLs (e.g., `DOSCALLS.DLL`).
- `src/main.rs`: CLI entry point for loading and executing OS/2 binaries.

## Building and Running

### Prerequisites

- Rust toolchain (2024 edition support required).

### Commands

- **Build:** `cargo build`
- **Run:** `cargo run -- <path_to_os2_executable>`
- **Check:** `cargo check`
- **Test:** `cargo test` (Note: No tests implemented yet - TODO)

## Development Conventions

- **Modular Design:** Keep API emulations, loader logic, and file format parsing strictly separated.
- **Safety:** Leverage Rust's safety features for memory mapping and buffer handling, which is critical when dealing with legacy binary formats.
- **Stubs:** Use clear stubbing patterns for unimplemented APIs to allow incremental progress.
- **Documentation:** Maintain this `GEMINI.md` as the primary context for AI-assisted development.

## Roadmap

1. **Phase 1 (Foundation):** Complete LX parser (MZ header -> LX header) and basic CLI loader.
2. **Phase 2 (Core Subsystem):** Expand `DOSCALLS.DLL` with filesystem, thread, and memory management APIs.
3. **Phase 3 (Presentation Manager):** Implement GUI support mapping OS/2 PM APIs to X11/Wayland.
4. **Phase 4 (16-bit Support):** Integrate a 16-bit execution environment or emulator.
