# ADR-0104: GDB finds the DLLs' debug companions through the stub's library list

- **Status:** Accepted; builds on ADR-0102, ADR-0101
- **Date:** 2026-11-21
- **Sources:**
  - GDB's manual: the remote protocol's `qXfer:libraries:read` and its library list format (segment addresses), the `library` stop reason, target descriptions and their `<osabi>` element, and `sysroot` with a target that has no file transfer.

## Context

ADR-0102 gave programs a debug companion. A DLL's companion has the addresses `lx-link` linked it for, but the loader places a DLL's objects where it finds room. GDB needed `add-symbol-file mod.dbg -o <offset>`, with the offset worked out by hand. A DLL loaded by `DosLoadModule` after the program started had to be caught first.

## Decision

1. **The stub lists the loaded DLLs** (`qXfer:libraries:read`, the generic format GDB uses for Windows targets):
   - one `<library>` per DLL loaded from a file that has a companion beside it (`X.DLL` → `X.dbg`, or `X.DBG`);
   - its name is the companion's host path;
   - it has one `<segment address>` per LX object, in object order. The companion has one loadable segment per object (code, then data), so GDB moves each by its own offset.
   - The DLL manager records each module's host file for this (`DllManager::set_host_file`, `loaded_files`).
2. **Module changes stop the program for the debugger.**
   - Once the program has made its first stop, loading or freeing a DLL (`load_dll`, `DosFreeModule`) sets `GdbState::library_event` and asks for a stop.
   - The vCPU stops at its next pass through its loop, before the new module's `_DLL_InitTerm` runs, with the `library` stop reason.
   - GDB reads the list again, resolves pending breakpoints, and goes on by itself.
   - Modules loaded before the first stop (the program's imports) are in the list GDB reads when it connects.
3. **The target description names no OS ABI** (`<osabi>none</osabi>`).
   - A companion is an ELF file that names no OS. A GDB built for Linux takes it for GNU/Linux and asks for SVR4 library lists, which Warpine does not have.
   - With no OS ABI, GDB reads the generic list.
4. **The program's own companion is not listed.** It is loaded with `symbol-file`, as before, at the addresses it was linked for.

## Warpine's choices (unverified)

- **Companions are found beside the DLL's host file only.** They are not searched along LIBPATH or GDB's `solib-search-path`.
- **Host paths are sent:** the stub runs for the host user who started it and listens on `127.0.0.1` only, so the paths are not a guest's to see.
- **No file transfer:** GDB warns that the target does not support it and reads the companions from the local file system.
- **The first stop** is where the program's first instruction is. When its DLLs have `_DLL_InitTerm` routines, that is Warpine's start thunk (`WARPINE.6`), not `_start`: the routines run before the program.

## Consequences

- **GDB versions differ:** GDB 16 (Debian trixie's) lists the DLLs of the first list but reads their symbols only after `sharedlibrary`; later GDBs read them on connecting. A library stop right after the first resume did not help GDB 16 (it re-reported a breakpoint), so the documented step is `sharedlibrary` after `target remote`, which the integration test runs.
- `break dll_function` works with no `add-symbol-file`, including for a DLL that `DosLoadModule` loads later (`set breakpoint pending on`). `info sharedlibrary` lists the companions.
- When `DosFreeModule` frees a DLL, GDB disables its breakpoints ("unloaded shared library").
- **`samples/c_gdb_test`** now has two DLLs:
  - `gdbdll.dll`, which the program imports;
  - `gdbdyn.dll`, which it loads with `DosLoadModule` and frees.

  The integration test stops in both at their source lines, with GCC and with clang.
- **Tests:**
  - `gdb_stub.rs`: the library list XML and its escaping, the reply window, the target description, the companion lookup, the stop gating, the stop reason;
  - `managers.rs`: `loaded_files`.

## Addendum (2026-11-26): a stop request kicks the vCPUs

A GDB stop request (the interrupt byte, Ctrl-C) used to set a flag the vCPU looks at before each `KVM_RUN`. A program spinning in guest code without calling Warpine never left `KVM_RUN`, so the stub gave up after two seconds and the program was never stopped.

`request_stop` (`gdb_stub.rs`) now sets the flag and then kicks every vCPU (`ThreadTable::kick_all_but(0)`, the kick `flush_tlbs` uses: `immediate_exit` and a signal to the thread). A kicked vCPU leaves the guest with `VmExit::Interrupted`, finds the flag at the top of its loop and reports the stop.
- A thread blocked in a host call (`DosSleep`, a semaphore wait) still stops when that call returns.
- `gdb_interrupts_a_program_spinning_in_guest_code` checks the behaviour end to end. It runs `c_gdb_test spin`, which loops on a `volatile` flag, and speaks raw RSP, since a batch-mode GDB cannot send Ctrl-C: continue, interrupt, expect the stop reply. Without the kick, no reply comes.
