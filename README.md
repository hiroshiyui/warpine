# Warpine: OS/2 Compatibility Layer

Warpine is a compatibility layer designed to allow IBM OS/2 applications and games to run natively on Linux using a custom KVM-based hypervisor. It follows an architecture inspired by WINE, aiming to reimplement OS/2 APIs and loader logic while using hardware acceleration for CPU execution.

## Key Features

- **LX Executable Parser:** Full support for parsing Linear Executable (LX) headers, object tables, page maps, and fixups (relocations).
- **KVM Hypervisor:** A custom Virtual Machine Monitor (VMM) that executes 32-bit OS/2 code at native speeds.
- **Multi-Threading:** Support for concurrent OS/2 threads, each mapped to a native host thread and a KVM vCPU.
- **API Thunking:** High-performance system call interception using `INT 3` traps to bridge guest OS/2 calls to host Rust implementations.
- **Dynamic Memory Management:** Implementation of `DosAllocMem` and `DosFreeMem` with a dedicated guest physical memory manager.
- **Filesystem Support:** Bridges OS/2 file I/O (`DosOpen`, `DosRead`, `DosWrite`) to native Linux files with path translation.
- **Zero-Dependency CPU:** Bypasses Linux host memory restrictions (like `mmap_min_addr`) by using a hardware-isolated guest memory space.

## Architecture

- `src/lx/`: Executable parser for the OS/2 LX format.
- `src/loader.rs`: KVM-based VMM and multi-threaded execution engine. Uses a shared-state architecture to manage guest memory and handles across multiple vCPUs.
- `src/api.rs`: Implementation of emulated OS/2 DLLs (e.g., `DOSCALLS.DLL`).
- `samples/`: Example OS/2 applications and build scripts.

## Prerequisites

- **CPU:** x86_64 with Virtualization support (VT-x or AMD-V enabled).
- **OS:** Linux with KVM support (`/dev/kvm` must be accessible by the user).
- **Toolchain:** Rust (Edition 2024).
- **Optional (for samples):** Open Watcom v2 (can be vendored using `vendor/setup_watcom.sh`).

## Getting Started

### 1. Build Warpine
```bash
cargo build
```

### 2. Prepare the sample
If you haven't already, download the compiler and build the samples:
```bash
./vendor/setup_watcom.sh
make -C samples/hello
make -C samples/alloc_test
make -C samples/file_test
```

### 3. Run an OS/2 binary
```bash
cargo run -- samples/hello/hello.exe
```

## Status

Phase 1 (Foundation) is complete. Phase 2 (Core Subsystem) is well underway, with Memory Management, Filesystem APIs, and Process/Thread Management fully implemented and verified. Work is ongoing for IPC (Semaphores, Pipes) and advanced system services.

## License

This project is licensed under the GNU General Public License v3 (GPLv3). See the [LICENSE](LICENSE) file for details.
