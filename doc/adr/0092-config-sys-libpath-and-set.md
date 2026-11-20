# ADR-0092: LIBPATH and SET from CONFIG.SYS

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - The OS/2 Warp *Command Reference*, CONFIG.SYS statements: `LIBPATH`, `SET`, `THREADS`.
  - IBM's *Control Program Programming Reference*: `DosLoadModule`, `DosSetExtLIBPATH`, `DosQueryExtLIBPATH`, `DosScanEnv`; `bsedos.h` `BEGIN_LIBPATH` / `END_LIBPATH`.
  - The code: `src/loader/config_sys.rs`, `lx_loader.rs` (`find_dll_path`), `inherit.rs` (`startup_block`), `cmd.rs`, `cmd/external.rs`, `stubs.rs`.
  - The sample: `samples/libpath_test`.
  - ADR-0032 (`THREADS=` and the session thread limit).

## Context

Warpine read one CONFIG.SYS statement, `THREADS=` (ADR-0032). Two more decide how programs start:
- **`LIBPATH=`** lists the directories searched for DLLs. Warpine searched only the program's own directory and `C:\OS2\DLL`, so a program whose DLLs live elsewhere could not start.
- **`SET name=value`** builds the environment every program starts with. Warpine had four hard-coded copies of its default environment: the 32-bit start, the 16-bit start (missing `TMP`), CMD.EXE, and CMD's `START`.

`DosSetExtLIBPATH` also stored BEGINLIBPATH and ENDLIBPATH, but the DLL search never used them. `DosQueryExtLIBPATH` returned them as UTF-8 instead of in the program's codepage.

## Decision

1. **One parse.** `config_sys.rs` parses `THREADS=`, `LIBPATH=` and `SET` into `ConfigSys`. Each process reads drive C's CONFIG.SYS once, the first time it needs it (`SharedState::config_sys`), and never writes or creates it.
2. **The DLL search** (`find_dll_path`) tries, in this order:
   1. BEGINLIBPATH;
   2. the program's own directory;
   3. `LIBPATH`, or `C:\OS2\DLL` when CONFIG.SYS has no `LIBPATH=`;
   4. ENDLIBPATH.

   Every entry is an OS/2 directory resolved through the drives, `.` being the current directory, so no entry reaches a host path outside a drive.
3. **The starting environment.** Warpine's base environment (`PATH=C:\`, `COMSPEC=C:\CMD.EXE`, `OS=OS2`, `TMP=C:\TMP`) has the `SET` statements applied in order. This one environment (`SharedState::initial_environment`) serves the 32-bit and 16-bit starts, a CMD.EXE nobody started, and CMD's `START` without `/I`. A program started by another still gets its parent's environment.
4. **Bounds.**
   - CONFIG.SYS is read up to 256 KB.
   - `LIBPATH` keeps at most 64 directories of at most 260 bytes each.
   - A `SET` that would make the environment exceed 64 KB is ignored, with a warning.
5. **`DosQueryExtLIBPATH`** writes the stored path in the active codepage.

## Warpine's choices (unverified)

- The program's own directory is searched although OS/2 searches only LIBPATH. Programs installed under Warpine have no installer to add their directory to LIBPATH.
- CONFIG.SYS is read when each process starts, not at boot, so an edit applies to programs started afterwards. `THREADS=` alone is passed from the session's first process.
- A `SET` name is upper-cased and trimmed, and its value trimmed. A later `SET` of the same name replaces the earlier one, and an empty value removes the variable. `%name%` is not expanded.
- The base environment is Warpine's: OS/2's comes from CONFIG.SYS alone.
- The last `LIBPATH=` statement wins. An empty one searches no directory.
- At most 64 LIBPATH directories are kept.

## Consequences

- **Programs** whose DLLs sit in a directory of their own start once that directory is on `LIBPATH`, BEGINLIBPATH or ENDLIBPATH, as on OS/2.
- **Users** set variables for all programs in CONFIG.SYS.
- **One default environment** replaces four copies. A 16-bit program now gets `TMP` too.
- **Not done yet:**
  - BEGINLIBPATH and ENDLIBPATH are not passed to child processes;
  - `%BEGINLIBPATH%` / `%ENDLIBPATH%` are not expanded in `DosSetExtLIBPATH`;
  - 16-bit (NE) programs load no DLLs from disk, so LIBPATH does not apply to them.
- **Tests:**
  - `config_sys.rs`: LIBPATH parsing and bounds, the SET rules, the environment size limit, a single read per process;
  - `lx_loader.rs`: the search order, `.`, names matched as on the drive, entries leading out of drive C, the default `C:\OS2\DLL`;
  - `stubs.rs`: the codepage round trip;
  - `tests/integration.rs` `test_config_sys_libpath_and_set`: `libpath_test`, 14 checks.
