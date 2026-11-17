# ADR-0003: Run OS/2 code natively on KVM

- **Status:** Accepted
- **Date:** 2026-03-12
- **Sources:** commit 843fbb7 ("implement KVM-based hypervisor for hardware-accelerated OS/2 execution"); `CLAUDE.md` (Project, Execution flow); `src/loader/vm_backend.rs`, `kvm_backend.rs`

## Context

OS/2 programs are 32-bit (LX) or 16-bit (NE) x86 code. Interpreting or translating that code is slow and complex, and the host is an x86-64 Linux machine that can run it directly.

## Decision

Warpine runs guest code natively in a KVM virtual machine: 128 MB of guest memory, a GDT and IDT set up by Warpine, flat 32-bit segments for LX programs and tiled segments for NE programs. Warpine does not emulate a CPU.

- `VmBackend` / `VcpuBackend` traits separate the hypervisor from the loader; `kvm_backend.rs` implements them, and a mock backend serves unit tests.
- The OS/2 API is implemented in Rust on the host (ADR-0005).

## Consequences

- Programs run at native speed.
- Warpine needs Linux with `/dev/kvm` and an x86-64 CPU with VT-x or AMD-V; there is no fallback emulator.
- Guest memory and every value coming from the guest are untrusted: preventing hypervisor escape and host over-reads is the top security priority (`CLAUDE.md`, Security Rules). Guest pointers go through checked guest-memory helpers.
- A vCPU cannot be interrupted while it runs guest code without exiting to Warpine, which limits `DosKillThread`, critical sections and exit lists (ADR-0007).
