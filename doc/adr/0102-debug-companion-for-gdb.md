# ADR-0102: A debug companion for GDB: lx-link keeps the DWARF beside the LX program

- **Status:** Accepted; builds on ADR-0095
- **Date:** 2026-11-21
- **Sources:**
  - The ELF and System V i386 ABI specifications: sections, symbols and program headers.
  - The DWARF 4 and 5 standards: which sections a debugger reads, and that their cross-references are section offsets.
  - GDB's manual: `symbol-file`, `add-symbol-file -o`, the remote protocol's initial stop, `tcp auto-retry`.

## Context

M4's first part: source-level debugging of programs from GCC and clang. Warpine has a GDB stub (`--gdb <port>`) with registers, memory, breakpoints and stepping, but GDB knew nothing of the program: no symbols, no lines, no types.

The LX format has no place for DWARF, and `lx-link` dropped every section that is not loaded.

## Decision

1. **The debug companion.**
   - When an input object has `.debug_*` sections, `lx-link` writes `<output>.dbg` beside the program. `--debug-file PATH` names it; `--no-debug-file` turns it off.
   - It is an ELF32 i386 executable holding:
     - `.text` and `.data` at the addresses they have in the program, with their linked bytes (relocations applied), and `.bss`, plus two `PT_LOAD` program headers;
     - the inputs' debug sections, joined by name in input order;
     - the global symbols, as functions or objects by the section they are in.
2. **Debug relocations** are applied against the final layout:
   - a place in another debug section is its offset in the joined section;
   - a place in the program is its address;
   - a section the link dropped (`--gc-sections`), or an undefined symbol, is 0.

   Relocation types a debug section has but the program could not (thread-local variables) are left as they are.
3. **Using it:**

   ```bash
   warpine --gdb 1234 prog.exe
   gdb -ex 'symbol-file prog.dbg' -ex 'target remote :1234'
   ```

   A DLL, which the loader moves, takes `add-symbol-file mod.dbg -o <load address − link address>`.
4. **The stub's first stop.**
   - The vCPU reports its stop at the entry point before a client attaches. The stub now takes that stop when the client connects, so the first registers GDB reads are the entry point's (they were zeros), and the first `continue` waits for a real stop.

## Warpine's choices (unverified)

- **The file:** a separate ELF file rather than DWARF inside the LX file. Nothing defines where the latter would live, and GDB reads the former as it is.
- **Local symbols** are not in the companion's symbol table; the DWARF describes static functions and variables.

## Consequences

- **GDB debugs C programs at the source level:** breakpoints by function and line, arguments and locals, structures through pointers, `finish` with the return value, backtraces, globals. This holds for GCC's and clang's DWARF 5 alike.
- **`samples/c_gdb_test`** (built with `-g -O0`) is debugged by GDB in the integration test `gdb_debugs_a_c_program_at_the_source_level`, for both compilers.
- **Tests:**
  - `lx_link.rs`: joining and relocating debug sections, the companion as an ELF file (`object` reads it back), the flags;
  - `tests/integration.rs`: the GDB session.
