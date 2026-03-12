# Warpine: OS/2 Compatibility Layer

Warpine is a compatibility layer designed to allow IBM OS/2 applications and games to run natively on Unix-like operating systems. It follows an architectural approach similar to WINE, aiming to reimplement OS/2 APIs and loader logic while leveraging KVM for hardware-accelerated CPU execution.

## Project Overview

- **Language:** Rust (Edition 2024)
- **License:** GPL-3.0-only
- **Goal:** Native execution of 16-bit and 32-bit OS/2 binaries (LX, LE, NE formats) on Linux/Unix using a custom KVM-based hypervisor for hardware-accelerated compatibility.
- **Current Status:** Phase 1 (Foundation) - **COMPLETED**. Warpine can now load 32-bit LX binaries, resolve relocations, and execute code at native speeds via KVM.

## Architecture

The project is structured into several core modules:

- `src/lx/`: Comprehensive parser for OS/2 Linear Executable (LX). Supports headers, object tables, page maps, and variable-length fixup records.
- `src/loader.rs`: KVM-based Virtual Machine Monitor (VMM). Manages guest memory mapping (bypassing `mmap_min_addr` restrictions), GDT/TIB/PIB initialization, and the VMEXIT loop.
- `src/api.rs`: Emulation of OS/2 System DLLs (e.g., `DOSCALLS.DLL`). System calls are intercepted via `INT 3` (breakpoint) traps in the guest, which trigger `VMEXIT_DEBUG` events.
- `src/main.rs`: CLI entry point for loading and executing OS/2 binaries.

## Implemented APIs (Phase 1)

- `DosWrite`: Basic I/O redirection to native stdout/stderr.
- `DosExit`: Process termination with exit code mapping.
- `DosQuerySysInfo` / `DosQueryConfig`: Runtime environment queries.
- `DosQueryHType`: Handle type identification for standard I/O.
- `DosGetInfoBlocks`: Thread (TIB) and Process (PIB) block resolution.

## Building and Running

### Prerequisites

- **CPU:** x86_64 with Virtualization support (VT-x or AMD-V).
- **OS:** Linux with KVM enabled (`/dev/kvm` accessible).
- **Rust:** 2024 edition.

### Commands

- **Build:** `cargo build`
- **Run:** `cargo run -- <path_to_os2_executable>`
- **Test:** `cargo test` (Includes LX parser unit and integration tests)

## Roadmap

1. **Phase 1 (Foundation):** LX parser, KVM loader, and basic CLI environment. [DONE]
2. **Phase 2 (Core Subsystem):** Full filesystem (`DosOpen`, etc.), advanced memory management (`DosAllocMem`), and multi-threading.
3. **Phase 3 (Presentation Manager):** PMWIN/PMGPI mapping to native X11/Wayland graphics.
4. **Phase 4 (16-bit Support):** x86 emulator integration for legacy NE binaries.
