# ADR-0095: C programs from GCC and clang through lx-link

- **Status:** Accepted; amends ADR-0015 (`lx-link`'s layout and what it reads)
- **Date:** 2026-11-21
- **Sources:**
  - The System V ABI Intel386 supplement: relocation types and the REL implicit addend; `SHT_INIT_ARRAY` / `SHT_FINI_ARRAY` / `SHT_PREINIT_ARRAY`; `SHN_COMMON`; weak symbols.
  - The ELF specification (section flags `SHF_ALLOC` / `SHF_EXECINSTR` / `SHF_TLS`).
  - IBM's *Control Program Programming Reference*: `DosWrite`, `DosExit`, the `_System` convention.
  - The code: `src/bin/lx_link.rs`, `samples/c_hello`, `tests/integration.rs`.
  - `doc/TODOs.md` "Development toolchain" (the route: unmodified compilers, i386 ELF in, LX out, `lx-link` the one bridge).

## Context

Open Watcom stays the reference toolchain. A modern compiler should build OS/2 programs too, without a compiler fork: GCC and clang emit freestanding i386 ELF objects (`-m32 -ffreestanding -fno-pic`), and `lx-link` turns ELF into LX. `lx-link` was written for rustc's objects and did not hold for theirs:
- **Wrong section:** a relocation against a section or a local symbol went to the first section *of that name in any object*, so the second object's `.data` was the first's.
- **Lost addends:** i386 uses REL relocations, whose addend is in the section bytes; `lx-link` overwrote them, so `&msg[8]` became `&msg[0]`. Local symbols also lost their offsets.
- **Unknown kinds:** relocation types other than the absolute and PC-relative ones were only warned about.
- **Missing pieces:** no common symbols, init / fini arrays, weak symbols or section garbage collection.
- **Archives:** every member was linked, and the members of Rust's archives failed to parse when not aligned in the file.
- **Fixed addresses:** code at 0x10000, data at 0x60000, stack at 0x70000. Code past 320 KB overlapped the data, and ADR-0094's loader check now refuses that.

The Rust guest was in fact broken: its integration test looked for `target/debug/lx-link` (Cargo builds `lx_link`) and skipped silently.

## Decision

1. **Sections by flags.** Allocated sections are linked:
   - executable ones into code;
   - the other initialised ones, read-only or writable, into data;
   - `SHT_NOBITS` ones into zeros.

   Unallocated sections, `.eh_frame` and notes are dropped. Thread-local sections are an error.
2. **Relocations per object.** A target is a global symbol, a place in a section of the same object (a local symbol with its offset, or the section), or an absolute value.
   - **Addends:** the REL addend is read from the section bytes; a RELA addend is taken as given.
   - **Values:** `R_386_32` gives S + A with an LX fixup; `R_386_PC32` gives S + A − P. `R_386_PLT32` is a direct call in a static link, so it is treated as `R_386_PC32`.
   - **Everything else is an error.** GOT-based relocations get "compile with -fno-pic"; 8- and 16-bit and thread-local ones are refused too.
3. **Symbols.**
   - A strong definition wins over a weak one; two strong ones are an error.
   - An undefined weak symbol is 0; any other undefined symbol is an error listing them all.
   - OS/2 APIs resolve through `targets/os2api.def`, as before.
   - Common symbols merge, at their largest size and alignment, into the zeros unless something defines them.
4. **Arrays.** The preinit, init and fini arrays are laid out together in data, between `__preinit_array_start` / `_end`, `__init_array_start` / `_end` and `__fini_array_start` / `_end`; a start-up routine runs them.
5. **Archives** are searched like a static linker's: a member is linked only when it defines a symbol still undefined, until none does. Each member is parsed from an aligned copy.
6. **`--gc-sections`** (rustc passes it) links only the sections reached from `_start` (or `main`) and the arrays through relocations. Undefined symbols then count only in what is linked.
7. **Layout:**
   - code at 64 KB;
   - data on the next 64 KB boundary after the code;
   - the stack (64 KB) after the data, all inside the program area of ADR-0094;
   - pages past the initialised bytes (zeros, the stack) take no room in the file.

   A program that does not fit is an error.
8. **The C side, M1.** `samples/c_hello` carries:
   - its own `crt0.c` (`_start` with the OS/2 entry arguments, the stack realigned for the compiler, the arrays, `main`, `DosExit`);
   - `os2min.h`: the two calls it uses, `_System` being cdecl on i386.

   A compiler-neutral `os2.h` and C runtime are M2.

## Warpine's choices (unverified)

- **Merged strings** (`SHF_MERGE`) are concatenated, not merged.
- **Read-only data** shares the writable data object.
- **The entry point** is `_start`, else `main`.
- **Garbage collection** happens only with `--gc-sections`.
- **The stack** is 64 KB.
- **The module name** stays `WARPEXE`.

## Consequences

- **GCC and clang** build OS/2 programs for Warpine with no change to either: `samples/c_hello` passes its checks built by GCC 16 and by clang 21, with and without `--gc-sections`.
- **Rust guests** link again, much smaller (`rust_hello`: 1.2 KB), and their test now fails when the build fails.
- **A link error is an error:** `lx-link` exits non-zero for undefined symbols, unsupported relocations and programs too large, instead of writing a broken program.
- **Next:** M2 (headers and a C runtime for both compilers), M3 (both compilers on every C sample), M4 (DWARF, C++).
- **Tests:**
  - `lx_link.rs`, 21 unit tests: per-object targets, addends, PC-relative values, layout, undefined / weak / imported symbols, precedence and commons, array bounds, section GC, archive selection;
  - `tests/integration.rs`:
    - `test_c_hello_from_gcc_and_clang`;
    - `test_lx_link_refuses_position_independent_code`;
    - `test_rust_hello`, now with this build's `lx-link` and no silent skip.
