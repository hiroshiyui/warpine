# AGENTS.md

Welcome, AI agent! This project is **Warpine**, an OS/2 compatibility layer for Unix-like systems. Your goal is to help maintain and extend its ability to run legacy OS/2 binaries natively using KVM.

## Project Overview

Warpine reimplements OS/2 APIs and loader logic, leveraging KVM for hardware-accelerated CPU execution. It targets 32-bit OS/2 binaries (LX format), with future goals for 16-bit NE/LE format support.

- **License:** GPL-3.0-only
- **Status:** Phase 1 (Foundation) and Phase 2 (Core Subsystem) are complete. Phase 3 (Presentation Manager GUI) is in progress. See `doc/TODOs.md` for the full roadmap.

### Architecture
- `src/lx/`: Comprehensive parser for OS/2 Linear Executable (LX).
- `src/loader.rs`: KVM-based Virtual Machine Monitor (VMM). Manages guest memory and GDT/TIB/PIB.
- `src/api.rs`: Emulation of OS/2 System DLLs (e.g., `DOSCALLS.DLL`).
- `src/gui.rs`: Presentation Manager mapping (Phase 3).
- `src/main.rs`: CLI entry point.

## Setup & Build

- **Prerequisites:** Linux with KVM enabled (`/dev/kvm`), x86_64 CPU with VT-x/AMD-V.
- **Build:** `cargo build`
- **Run:** `cargo run -- <path_to_os2_executable>`

## Testing

- **Command:** `cargo test`
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
