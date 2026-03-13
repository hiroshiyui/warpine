# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Warpine is an OS/2 compatibility layer for Linux that runs 32-bit OS/2 (LX format) binaries natively using KVM for hardware-accelerated CPU emulation. Analogous to WINE but for OS/2 instead of Windows. Future goals include 16-bit NE/LE format support.

**License:** GPL-3.0-only

**Status:** Phase 1 (Foundation) and Phase 2 (Core Subsystem) are complete. Phase 3 (Presentation Manager GUI) is in progress. See `doc/TODOs.md` for the full roadmap.

## Build & Run

```bash
cargo build                              # Debug build
cargo run -- <path_to_os2_executable>    # Run an OS/2 binary
cargo run -- samples/hello/hello.exe     # Example: run hello world
cargo test                               # Unit tests (LX parser)
```

**Prerequisites:** Linux with KVM enabled (`/dev/kvm`), x86_64 CPU with VT-x/AMD-V, Rust 2024 edition.

**Sample OS/2 apps** are in `samples/` (hello, alloc_test, file_test, pipe_test, etc.). Build them with Open Watcom: `./vendor/setup_watcom.sh` then `make -C samples/<name>`.

## Architecture

### Execution flow

1. **Parse** — `main.rs` calls `LxFile::open()` to parse the MZ+LX executable (headers, object table, page map, fixup records, imports)
2. **Load** — `Loader::load()` maps executable pages into KVM guest memory (128MB) and applies relocations via `apply_fixups()`
3. **Execute** — `Loader::run()` sets up TIB/PIB, creates a vCPU, and enters the VMEXIT loop (`run_vcpu()`)
4. **API thunking** — Guest API calls hit INT 3 breakpoints at magic addresses (MAGIC_API_BASE = 0x01000000), causing VMEXIT_DEBUG → `handle_api_call()` dispatches to host-side Rust implementations by ordinal number

### Key modules

- **`src/lx/`** — LX executable format parser (`header.rs` for binary structures, `lx.rs` for orchestration). Unit tests live here.
- **`src/loader.rs`** — The core: KVM VMM, memory manager, handle manager, semaphore manager, queue manager, VMEXIT loop, and 40+ OS/2 API handler functions. This is ~57% of the codebase.
- **`src/api.rs`** — Small module with `DosWrite`/`DosExit` implementations and FFI bridge stubs.
- **`src/gui.rs`** — Phase 3 Presentation Manager GUI (winit + softbuffer). Work in progress.

### Concurrency model

Each OS/2 thread maps to a native Rust thread with its own KVM vCPU. `SharedState` wraps all managers in `Arc<Mutex<...>>` for cross-thread access. Semaphores use `Arc<(Mutex<State>, Condvar)>`.

### Important constants (in loader.rs)

- `MAGIC_API_BASE` (0x01000000) — API thunk stub area
- `EXIT_TRAP_ADDR` (0x010003FF) — Special exit breakpoint
- `DYNAMIC_ALLOC_BASE` (0x02000000) — Guest memory allocation pool

## Conventions

- **Modular separation:** Keep API emulations, loader logic, and format parsing in their respective modules.
- **Every new feature or bug fix must include tests.** High coverage required for the LX parser and API thunks.
- **Unsafe code** is expected for KVM/guest memory operations but must have clear safety justification and enforce guest memory isolation. All code involving pointer arithmetic or guest-to-host transitions must be reviewed for buffer overflows.
- **Use named constants** — no magic numbers for GDT entries, segment selectors, or API ordinals.
- **Stubbing pattern** — unimplemented APIs should use clear stubs to allow incremental progress.
- OS/2 paths (`C:\path`) are translated to Unix paths by replacing backslashes. OS/2 error codes (u32) are returned in RAX.
