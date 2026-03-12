# Warpine: OS/2 Compatibility Layer

Warpine is a compatibility layer designed to allow IBM OS/2 applications and games to run natively on Unix-like operating systems. It follows an architectural approach similar to WINE, aiming to reimplement OS/2 APIs and loader logic rather than relying on full system emulation.

## Project Overview

- **Language:** Rust (Edition 2024)
- **Goal:** Native execution of 16-bit and 32-bit OS/2 binaries (LX, LE, NE formats) on Linux/Unix using a custom KVM-based hypervisor for hardware-accelerated compatibility.
- **Current Status:** Phase 1 (Foundation) - **COMPLETED**. Warpine can now load 32-bit LX binaries and execute them at native speeds using the KVM API.

## Architecture

The project is structured into several core modules:

- `src/lx/`: Executable parser for OS/2 Linear Executable (LX). Fully implements header, object table, page map, and fixup parsing.
- `src/loader.rs`: KVM-based VMM (Virtual Machine Monitor) that maps OS/2 objects into a 32-bit guest VM and handles API traps.
- `src/api.rs`: Emulation of OS/2 System DLLs (e.g., `DOSCALLS.DLL`) via native Rust thunks triggered by VMEXITs.
- `src/main.rs`: CLI entry point for loading and executing OS/2 binaries.

## Building and Running

### Prerequisites

- Rust toolchain (2024 edition).
- Linux kernel with KVM support (`/dev/kvm` must be accessible).
- `kvm-ioctls` and `kvm-bindings` crates (handled by Cargo).

### Commands

- **Build:** `cargo build`
- **Run:** `cargo run -- <path_to_os2_executable>`

## Roadmap

1. **Phase 1 (Foundation):** Complete LX parser and CLI loader with basic API thunks. [DONE]
2. **Phase 2 (Core Subsystem):** Expand `DOSCALLS.DLL` with filesystem, thread, and memory management APIs.
3. **Phase 3 (Presentation Manager):** Implement GUI support mapping OS/2 PM APIs to X11/Wayland.
4. **Phase 4 (16-bit Support):** Integrate a 16-bit execution environment or emulator.
