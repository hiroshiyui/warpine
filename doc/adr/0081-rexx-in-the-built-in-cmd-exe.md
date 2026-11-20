# ADR-0081: REXX in the built-in CMD.EXE: our own interpreter, in three phases

- **Status:** Accepted; implemented in phases: R1 ADR-0083, R2 ADR-0084, R3 ADR-0085
- **Date:** 2026-11-20
- **Sources:**
  - IBM's public OS/2 REXX documentation: *OS/2 Procedures Language 2/REXX Reference* and the REXX application programming interface reference;
  - M. F. Cowlishaw, *The REXX Language* (the language's definition);
  - ANSI X3.274-1996, *Programming Language REXX*.

## Context

On OS/2, REXX is the system's scripting language:
- **Which files are REXX.** CMD.EXE runs a `.CMD` file as REXX when its first line starts with a `/* … */` comment, and as a batch file otherwise.
- **Commands.** A REXX clause that is not REXX goes to the default command environment, `CMD`, and the command's return code comes back in `RC`.
- **Programs.** They embed REXX through `REXX.DLL`: `RexxStart`, subcommand handlers, the variable pool and queues.
- **Utilities.** Scripts lean on the functions of `REXXUTIL` (`SysFileTree`, `SysCls` …).

The built-in CMD.EXE runs batch files (ADR-0077–0080). The project's goal is an OS/2-alike API and runtime (ADR-0075), and REXX belongs to it, as the shell's language does on Unix.

IBM's REXX implementation is proprietary and is not used. The language itself is publicly defined, by Cowlishaw's book and the ANSI standard, and IBM's REXX documentation is public, like the other references Warpine follows. Free implementations exist (Regina, LGPL-2.1; ooRexx, CPL). Three ways were weighed:
1. **Our own interpreter in Rust, in the host,** beside the shell.
2. **Regina built with Open Watcom as a guest `REXX.DLL`.**
3. **Regina linked into the host through FFI.**

## Decision

1. **Our own interpreter, in Rust, in the host** (`src/loader/rexx/`, beside `cmd/`):
   - no third-party C code and no `unsafe`;
   - files only through the VFS, so a script stays inside its drives as the shell does (ADR-0066).

   This rules out a Regina port, whose large C codebase would have to be pinned, patched and maintained, and FFI, which would put C code into the host process against the security priority.
2. **Sources:** IBM's public REXX documentation, Cowlishaw and ANSI X3.274-1996.
   - **No IBM binaries,** and no other implementation's source.
   - **No other oracle.** Regina is not used as a test oracle either. What the sources leave open is Warpine's choice, listed as unverified in each phase's ADR.
3. **OS/2's convention.** A `.CMD` whose first line starts with `/*` runs as REXX.
   - **Commands.** Command clauses go to `ADDRESS CMD`, which is our shell, and `RC` takes their return code.
   - **Arguments.** The script's arguments arrive through `PARSE ARG`.
4. **Three phases,** each tested, documented in its own ADR, reported and committed on its own:
   1. **R1, the language core:**
      - **Basics:** the tokenizer and clauses; expressions with REXX's arbitrary-precision decimal arithmetic (`NUMERIC DIGITS 9` by default), built in from the start because REXX numbers are decimal strings; simple and compound variables (stems).
      - **Control:** `IF`, `DO` (all forms), `SELECT`, `ITERATE`, `LEAVE`, `CALL`, `RETURN`, `EXIT`, internal routines with `PROCEDURE EXPOSE`, `SIGNAL` to labels, `INTERPRET`, `NOP`, `DROP`.
      - **Data:** `PARSE` with full templates (`ARG`, `PULL`, `VAR`, `VALUE`, `SOURCE`, `VERSION`); the queue (`PUSH`, `QUEUE`, `PULL`, `QUEUED`); the built-in string functions.
      - **Commands:** `ADDRESS CMD`.
   2. **R2, scripting reach:**
      - stream I/O (`LINEIN`, `LINEOUT`, `CHARIN`, `CHAROUT`, `LINES`, `CHARS`, `STREAM`) through the VFS;
      - conditions (`ERROR`, `FAILURE`, `NOVALUE`, `SYNTAX`, `HALT`, `NOTREADY`) with `SIGNAL ON` / `CALL ON`;
      - `NUMERIC FORM` / `FUZZ`;
      - `TRACE`;
      - a subset of `REXXUTIL`-compatible functions.
   3. **R3, REXX for OS/2 programs:** a built-in `REXX` module with the REXX API (`RexxStart`, subcommand and external function handlers, the variable pool, queues). Where its ordinals come from is an open question, as SOM's was (ADR-0070), and is decided then.
5. **`REXXUTIL` names.** The `REXXUTIL`-style functions keep their familiar names (`SysFileTree`, `SysSleep` …), as part of the familiar API. They are our own implementations, written from the public documentation and added case by case as scripts need them.
6. **Bounded resources.** Every phase bounds what a script can make the host spend: recursion and `INTERPRET` depth (each a nested call on the host's stack), `NUMERIC DIGITS`, string and stem sizes, and the queue's size.

## Consequences

- **REXX scripts written for OS/2** will run on the built-in CMD.EXE once R1 and R2 are done; programs that embed REXX, after R3.
- **The largest piece of the shell's work.** Classic REXX is a sizeable language, so R1 is split into internal steps (tokenizer and expressions, control flow, PARSE and functions, the shell hook) but reported as one phase.
- **Samples:** REXX sample scripts go in `samples/rexx_scripts`, run by integration tests. Open Watcom programs (`samples/cmd_tool`) check `RC` and, in R3, the API.
- **Documented gaps.** Where IBM's REXX and the language definition differ, or the sources leave a behaviour open, the phase's ADR records Warpine's choice.
