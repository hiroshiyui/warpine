# Warpine: OS/2 Compatibility Layer

Warpine is a compatibility layer that runs 32-bit OS/2 (LX format) applications natively on Linux using KVM hardware virtualization. Analogous to WINE for Windows, but targeting OS/2 instead.

## Key Features

- **LX Executable Parser:** Full support for parsing Linear Executable (LX) headers, object tables, page maps, fixups (relocations), resource tables, and import/export tables.
- **KVM Hypervisor:** A custom Virtual Machine Monitor (VMM) that executes 32-bit OS/2 code at native speeds using hardware-accelerated virtualization.
- **API Thunking:** System call interception using `INT 3` traps to bridge guest OS/2 calls to host Rust implementations. Supports DOSCALLS, PMWIN, PMGPI, KBDCALLS, VIOCALLS, and other OS/2 modules.
- **Multi-Threading:** Concurrent OS/2 threads, each mapped to a native host thread with its own KVM vCPU.
- **Presentation Manager (GUI):** Window management, message loop, graphics primitives, timer support, clipboard, dialog boxes, menus, and resource loading — implemented via winit + softbuffer.
- **Text-Mode Console:** Full VIO (Video I/O) and KBD (Keyboard) subsystem emulation via ANSI terminal escape sequences and termios raw mode.
- **Filesystem Support:** OS/2 file I/O with drive letter path translation, directory enumeration, and file metadata APIs. Phase 4 introduces a VFS layer with HPFS-compatible semantics (case-insensitive lookup, extended attributes, file locking, sandbox isolation).
- **Memory Management:** `DosAllocMem`/`DosFreeMem`, shared memory, and a dedicated guest physical memory manager.
- **IPC:** Event/mutex/muxwait semaphores, pipes, and named message queues.
- **Process Management:** `DosExecPgm`, `DosWaitChild`, directory tracking, and system information queries.

## Architecture

```
src/
  main.rs              Entry point, CLI/PM detection, event loop setup
  gui.rs               winit/softbuffer GUI, event handling, drawing
  lx/                  LX executable format parser
  loader/
    mod.rs             KVM VMM, VMEXIT loop, API dispatch
    doscalls.rs        DOSCALLS API implementations
    viocalls.rs        VIOCALLS (Video I/O) implementations
    kbdcalls.rs        KBDCALLS (Keyboard) implementations
    pm_win.rs          PMWIN (Window Manager) implementations
    pm_gpi.rs          PMGPI (Graphics) implementations
    console.rs         VioManager: screen buffer, cursor, raw mode, ANSI output
    process.rs         Process execution and directory tracking
    managers.rs        Memory, handle, resource managers
    stubs.rs           Stub handlers for unimplemented APIs
    ipc.rs             Semaphores and queues
samples/               Example OS/2 applications and build scripts
```

See [doc/developer_guide.md](doc/developer_guide.md) for detailed internals documentation.

## Prerequisites

- **CPU:** x86_64 with virtualization support (VT-x or AMD-V enabled).
- **OS:** Linux with KVM support (`/dev/kvm` must be accessible by the user).
- **Toolchain:** Rust (Edition 2024).
- **Optional (for samples):** Open Watcom v2 (can be vendored using `vendor/setup_watcom.sh`).

## Getting Started

### 1. Build Warpine
```bash
cargo build
```

### 2. Build sample OS/2 applications
```bash
./vendor/setup_watcom.sh          # Download Open Watcom compiler
make -C samples/hello             # Simple "Hello, OS/2!" CLI app
make -C samples/alloc_test        # Memory allocation test
make -C samples/file_test         # File I/O test
make -C samples/pm_demo           # Presentation Manager GUI demo
```

### 3. Run an OS/2 binary
```bash
cargo run -- samples/hello/hello.exe        # CLI app
cargo run -- samples/pm_demo/pm_demo.exe    # GUI app
```

### 4. Run 4OS2 (interactive OS/2 command shell)
```bash
cd samples/4os2 && ./fetch_source.sh && make && cd ../..
cargo run -- samples/4os2/4os2.exe
```

Use `RUST_LOG=debug` for detailed API call tracing:
```bash
RUST_LOG=debug cargo run -- samples/hello/hello.exe
```

### 5. Run tests
```bash
cargo test
```

## Status

- **Phase 1** (Foundation) — Complete. LX parser, KVM loader, basic API thunks.
- **Phase 2** (Core Subsystem) — Complete. Memory, filesystem, threading, IPC, process management.
- **Phase 3** (Presentation Manager GUI) — Complete. Window management, graphics, input, timers, dialogs, menus, clipboard, resource loading.
- **Phase 3.5** (Text-Mode Application Support) — Complete. VIO/KBD console subsystem, DosRead stdin with CR-CRLF translation and echo. 4OS2 command shell runs interactively.
- **Phase 4** (Filesystem I/O) — In progress. HPFS-compatible virtual filesystem with VfsBackend trait, pluggable backends (host-directory first), case-insensitive lookup, extended attributes, file locking, and sandbox isolation. Steps 1–5 (VFS trait, DriveManager, HostDirBackend, EAs, FS info, locking, HPFS wildcards) complete.

See [doc/TODOs.md](doc/TODOs.md) for the full roadmap.

## License

This project is licensed under the GNU General Public License v3.0 only (GPL-3.0-only). See the [LICENSE](LICENSE) file for details.
