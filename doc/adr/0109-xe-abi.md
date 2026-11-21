# ADR-0109: XE's ABI: the System V AMD64 psABI with LP64, and the Itanium C++ ABI

- **Status:** Proposed (XE is a long-term vision; see `doc/TODOs.md`, "XE — a 64-bit OS/2-lineage platform")
- **Date:** 2026-11-21
- **Sources:**
  - *System V Application Binary Interface, AMD64 Architecture Processor Supplement* (the x86-64 psABI), maintained publicly by the x86-64 ABI group: calling convention, data layout, the microarchitecture levels `x86-64-v1`–`v4`, TLS, `.eh_frame` unwind tables.
  - *Itanium C++ ABI* (itanium-cxx-abi.github.io): name mangling, class and vtable layout, RTTI, exceptions (`__cxa_*`, the personality routine, two-phase unwinding), guard variables, `__cxa_atexit`.
  - IEEE 1003.1 / ISO/IEC 9945 (POSIX, the Single UNIX Specification): the LP64 programming environment (`_POSIX_V7_LP64_OFF64`).
  - The Open Group, *64-Bit Programming Models: Why LP64?*: LP64 against LLP64 and ILP64.
  - IEEE 754 / ISO/IEC 60559 (floating point); DWARF 5 (debug information and call-frame information); ISO C23 `<stdint.h>` (fixed-width types).
  - For later ports: Arm's AAPCS64, the RISC-V psABI.

## Context

XE is the planned 64-bit OS/2-lineage format. Its programs need a binary interface that free compilers already produce: a C-level ABI (calls, data model, structure layout), a C++ ABI on top, and rules for OS/2's own types and exceptions.

No international standard defines a C ABI. ISO C (ISO/IEC 9899) leaves the sizes, calls and layouts to each implementation; the ABIs in use are de facto standards:
- **The System V AMD64 psABI** is used by Linux, the BSDs and Solaris (macOS a close variant), and by GCC, clang, Rust and Go.
- **Microsoft's x64 convention** (LLP64) is documented, but GCC and clang support it only as a secondary target.

The Itanium C++ ABI, despite its name, is the C++ ABI of every non-Windows x86-64 system, defined on top of the psABI. The SDK's 32-bit C++ runtime already follows it (ADR-0103, ADR-0107), and the vendored LLVM libc++ chosen for the SDK assumes it.

## Decision (proposed)

1. **C-level ABI: the System V AMD64 psABI, unchanged.**
   - Arguments in RDI, RSI, RDX, RCX, R8, R9 and XMM0–7, its struct classification, 16-byte stack alignment at calls.
   - Every free toolchain targets it with no patches.
2. **C++ ABI: the Itanium C++ ABI**, with the SDK's runtime and libc++ / libc++abi built for it.
3. **Data model: LP64** (POSIX's `_POSIX_V7_LP64_OFF64`). OS/2's types are defined from `<stdint.h>`, never from `long`:
   - `ULONG` / `LONG` stay 32-bit;
   - pointer-carrying types widen to 64-bit: `PVOID` fields, handles that hold pointers, `MPARAM` / `MRESULT`, `ULONG_PTR`-style types.

   Every structure and PM message is then laid out from these types, recorded in tables like `tests/sdk_layout`.
4. **`APIENTRY` is the psABI convention:** Warpine's thunks take System V arguments, with no special calling convention.
5. **A CPU baseline** from the psABI's microarchitecture levels (`x86-64-v2` or `x86-64-v3`), named in the XE header so the loader can refuse a program the CPU cannot run.
6. **Unwinding:** psABI `.eh_frame` and DWARF call-frame information for C and C++ alike.
7. **Floating point:** IEEE 754 (SSE2 at least), as the psABI requires.

## Still open

- **OS/2 exceptions:**
  - how OS/2-style exception handlers and `DosUnwindException` map onto `.eh_frame` unwinding;
  - whether XE keeps per-thread registration records at all;
  - traps running C++ destructors, and `longjmp`.

  The 32-bit work on `DosUnwindException` informs this.
- **The red zone:**
  - the psABI gives a function 128 bytes below RSP that interrupts must not touch;
  - Warpine runs guests at ring 0, where a hardware exception does not switch stacks and would overwrite it;
  - so: run XE guests at ring 3 (keeps the psABI unchanged), give the interrupt descriptors IST stacks, or require `-mno-red-zone` as kernels do (a deviation from the psABI).
- **The container:** XE's own LX-like format (`doc/TODOs.md`) or ELF.
  - The psABI's calling and data rules apply either way;
  - its ELF-specific parts (relocation types, TLS models, symbol versioning) need XE equivalents if the container is not ELF.
- **SOM:** its 64-bit binary interface is independent of the C++ ABI and needs its own definition.
- **Other architectures:** AAPCS64 and the RISC-V psABI are LP64 in the same spirit, so the type rules above carry over.

## Consequences

- GCC, clang and Rust build XE programs with stock 64-bit targets. Only the OS/2 type definitions, the linker and the XE format are Warpine's.
- The 32-bit SDK's C++ runtime design carries over to XE.
- Choosing LP64 and explicit OS/2 type widths up front fixes every XE structure layout before any API is written.
