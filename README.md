# Warpine: OS/2 Compatibility Layer

Warpine is a compatibility layer designed to allow IBM OS/2 applications and games to run natively on Linux using a custom KVM-based hypervisor. It follows an architecture inspired by WINE, aiming to reimplement OS/2 APIs and loader logic while using hardware acceleration for CPU execution.

## Key Features

- **LX Executable Parser:** Full support for parsing Linear Executable (LX) headers, object tables, page maps, and fixups (relocations).
- **KVM Hypervisor:** A custom Virtual Machine Monitor (VMM) that executes 32-bit OS/2 code at native speeds.
- **API Thunking:** High-performance system call interception using `INT 3` traps to bridge guest OS/2 calls to host Rust implementations.
- **Zero-Dependency CPU:** Bypasses Linux host memory restrictions (like `mmap_min_addr`) by using a hardware-isolated guest memory space.

## Architecture

- `src/lx/`: Executable parser for the OS/2 LX format.
- `src/loader.rs`: KVM-based loader and hypervisor loop.
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
If you haven't already, download the compiler and build the "Hello World" sample:
```bash
./vendor/setup_watcom.sh
make -C samples/hello
```

### 3. Run the OS/2 binary
```bash
cargo run -- samples/hello/hello.exe
```

## Status

Phase 1 (Foundation) is complete. Warpine can load and execute a 32-bit "Hello World" OS/2 console application. Work is ongoing for Phase 2 (Core Subsystem), which includes advanced memory management and filesystem APIs.

## License

This project is licensed under the GNU General Public License v3 (GPLv3). See the [LICENSE](LICENSE) file for details.
