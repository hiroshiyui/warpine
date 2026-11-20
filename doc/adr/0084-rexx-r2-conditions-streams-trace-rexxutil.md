# ADR-0084: REXX R2: conditions, streams, TRACE and REXXUTIL

- **Status:** Accepted; phase R2 of ADR-0081, after ADR-0083
- **Date:** 2026-11-20
- **Sources:**
  - IBM's public *OS/2 Procedures Language 2/REXX Reference* and its REXX Utility Functions;
  - M. F. Cowlishaw, *The REXX Language*;
  - ANSI X3.274-1996 (ADR-0081);
  - the code:
    - `src/loader/rexx/`: `condition.rs`, `stream.rs`, `trace.rs`, `rexxutil.rs`, and changes to `interp.rs` (the run loop, `Host`), `program.rs`, `lexer.rs` (token positions), `number.rs` (`format_form`), `builtins.rs`, `template.rs`;
    - `src/loader/cmd/rexx_host.rs`: the shell's side of the new `Host` calls;
  - the sample: `samples/rexx_scripts/rexxr2.cmd`.

## Context

R1 (ADR-0083) gave the built-in CMD.EXE the REXX language core. Real scripts also:
- trap errors (`SIGNAL ON ERROR`, `CALL ON ERROR`);
- read and write files with the stream functions;
- debug with `TRACE`;
- start with `call RxFuncAdd 'SysLoadFuncs', 'RexxUtil', 'SysLoadFuncs'` to use REXXUTIL.

R2 adds these.

## Decision

1. **Conditions** (`condition.rs`, `interp.rs`):
   - **The traps.** `SIGNAL ON | OFF cond [NAME label]` and `CALL ON | OFF cond [NAME label]` work for ERROR, FAILURE, HALT, NOTREADY, NOVALUE and SYNTAX; CALL takes neither NOVALUE nor SYNTAX (Error 25). The label is the condition's name unless NAME gives another. The traps are part of a routine's settings: a routine starts with its caller's, and they come back when it returns.
   - **SIGNAL ON.** When the condition is raised, the trap is turned off and the program goes to the label as SIGNAL does. The DO and SELECT groups of the routine end, and `SIGL` holds the line. For SYNTAX, `RC` holds the error number.
   - **CALL ON.** The label is called as a subroutine when the clause ends. While it runs, its trap is delayed: the condition is ignored. Afterwards the trap is back on.
   - **Raising:**
     - **ERROR** for a command whose return code is not 0. A negative code raises **FAILURE**, or ERROR when FAILURE is not trapped. An unknown environment is a failure, with `RC` -3.
     - **NOVALUE** when a variable without a value is used in an expression.
     - **NOTREADY** when a stream cannot be opened, read past its end, or written.
     - **SYNTAX** for any REXX error.
     - **HALT** when the host asks the program to stop.
     A condition that is not trapped does nothing, except SYNTAX and HALT, which end the program as errors (HALT as Error 4).
   - **Not the caller's.** A routine's untrapped error is not trapped by its callers; it ends the program.
   - **`CONDITION([C | D | I | S])`** describes the condition last trapped: its name, its description (the command, the variable's name, the stream's name, a SYNTAX error's detail), the instruction (`SIGNAL` or `CALL`), and its trap's state now (`ON`, `OFF`, `DELAY`).
2. **Streams** (`stream.rs`):
   - **The functions.** `CHARIN`, `CHAROUT`, `CHARS`, `LINEIN`, `LINEOUT`, `LINES` and `STREAM` work on files named by OS/2 paths. Files are reached only through new `Host` calls (`full_name`, `file_open`, `file_read`, `file_write`, `file_size`, `file_close`, `file_info`), which the shell implements with the VFS, so streams stay inside the drives.
   - **The standard streams.** `STDIN` (and an omitted name for input) is the shell's input, which PULL also reads. `STDOUT` (and an omitted name for output) is the shell's output, as SAY. `STDERR` is the shell's output in its error colour.
   - **Opening.** A file stream opens on first use: for reading, or for reading and writing once it is written to. A missing file is created when written to.
   - **Positions.** The read position starts at the start, the write position at the end. Each moves with its own operations, or with a `start` / `line` argument.
   - **Line ends.** Lines are written with CR LF and read up to LF, a CR before it dropped.
   - **`LINES(name)`** is 1 or 0; `LINES(name, 'C')` counts.
   - **`STREAM(name, 'S' | 'D')`** gives READY, NOTREADY, ERROR or UNKNOWN, and with `D` the reason (`EOF` or an OS/2 error code).
   - **`STREAM(name, 'C', …)` commands:**
     - `OPEN [READ | WRITE | BOTH] [REPLACE | APPEND]`, `CLOSE`, `FLUSH`;
     - `SEEK` / `POSITION [= < + -]n [READ | WRITE] [CHAR | LINE]`;
     - `QUERY EXISTS | SIZE | DATETIME | TIMESTAMP | HANDLE | STREAMTYPE | SEEK | POSITION`.
   - **The end.** Every stream closes when the program ends. `PARSE LINEIN` reads a line of the default input stream.
3. **TRACE** (`trace.rs`):
   - **Settings.** `TRACE [setting | VALUE expr]` and `TRACE([setting])` take A(ll), C(ommands), E(rrors), F(ailures), I(ntermediates), L(abels), N(ormal, the default), O(ff) and R(esults). A leading `?` toggles interactive tracing, and a whole number is accepted.
   - **Clauses.** A traced clause is written with its line number and `*-*`, its own text from the source (tokens record their byte offsets), `*,*` for its continuation lines and `*~*` for INTERPRET's.
   - **Values.** R writes each instruction's expression result (`>>>`, and `>.>` for a template's placeholder). I also writes each step: `>V>` variable, `>L>` literal, `>F>` function, `>O>` operation, `>P>` prefix operation, `>C>` compound name.
   - **Failed commands.** A failed command is followed by `+++ RC(n) +++`: under N and F for negative codes, under E, C, A, R and I for any.
   - **Where it goes.** Trace lines go to the shell's output through `Host::trace`.
4. **`NUMERIC FORM ENGINEERING`** writes exponential numbers with an exponent that is a multiple of three and one to three digits before the point (`Num::format_form`), for every result the interpreter writes.
5. **REXXUTIL** (`rexxutil.rs`): our own implementations, from IBM's public descriptions, under the familiar names.
   - **The functions:**
     - `SysLoadFuncs`, `SysDropFuncs`;
     - `SysCls`, `SysSleep`, `SysOS2Ver` (`2.45`, Warpine's reported version), `SysBootDrive` (`C:`), `SysDriveMap`;
     - `SysFileTree` (options F, D, B, S, T, O; an attribute mask), `SysFileSearch` (options C, N), `SysFileDelete`, `SysMkDir`, `SysRmDir`, `SysSearchPath`, `SysTempFileName`.
   - **Registration.** `RxFuncAdd`, `RxFuncQuery` and `RxFuncDrop` are built-in functions. Registered functions are found after the built-in ones and before external programs. `RxFuncAdd` registers a REXXUTIL function under any name.
   - **Through the host.** File work goes through `Host` (`file_find`, `file_delete`, `dir_make`, `dir_remove`, `drives`, `clear_screen`), inside the drives.
   - **Not here.** REXXUTIL's screen and keyboard functions, EAs, INI files and Workplace Shell objects are not; calling one is Error 43.
6. **Bounded resources:**
   - file streams open at once: 64;
   - a line or a CHARIN: 16 MiB;
   - conditions waiting for their CALL ON routines: 64;
   - entries from `SysFileTree` or `SysFileSearch`: 65,536;
   - SysFileTree's depth into subdirectories: 32;
   - one SysSleep: one day, woken early when the program must stop;
   - after HALT, the program may run 100,000 more clauses (its HALT routine's cleanup), then ends with Error 4 whatever it traps.

## Warpine's choices (unverified)

- A delayed CALL ON trap ignores its condition rather than holding it.
- External routines start with all traps off.
- A routine's untrapped condition ending the program rather than reaching its caller's traps.
- A negative return code as a failure, and an unknown environment's -3.
- HALT raised once, with 100,000 clauses' grace.
- TRACE's layout: the six-place line number, `*-*` / `*,*` / `*~*`, values as `>>>` and three blanks before the quoted value, `+++ RC(n) +++`; trace lines going to the output, not a separate error stream; THEN and ELSE not traced on their own.
- Interactive tracing (`?`) accepted but never pausing; numbers ignored.
- `LINES` 1 or 0 by default (ANSI's `N`), counting only with `C`.
- Streams on first use opening read-only, and read-write once written; OPEN without a mode reading and writing.
- `SEEK` without READ or WRITE moving both positions; lines sought only from the start.
- The standard streams always READY; the STDIN read-ahead lines ending in CR LF for CHARIN.
- STREAM's descriptions (`READY:`, `NOTREADY:EOF`, `NOTREADY:<OS/2 error>`), `CLOSE` giving `READY:` even when nothing was open, `QUERY EXISTS` only for files, the DATETIME and TIMESTAMP layouts.
- REXXUTIL registered from the start (as after SysLoadFuncs ran once on an OS/2 system), per program rather than for the whole system; `RxFuncAdd` answering 1 for other modules.
- SysFileTree's line layout (`mm/dd/yy  hh:mmx  size  ADHRS  name`, and with T `yy/mm/dd/hh/mm`); changing attributes not supported.
- SysFileSearch's numbered lines as the number, a blank and the line.
- SysSearchPath looking in the current directory first.
- SysSleep accepting fractions of a second.
- SysTempFileName's numbering (from a random start, through every combination).
- SysBootDrive always `C:`; every drive local for SysDriveMap.

## Consequences

- **What scripts can do.** REXX scripts written for OS/2 can trap errors, use files and REXXUTIL's common file functions, and be traced.
- **Left for R3:** the REXX API for OS/2 programs (ADR-0081).
- **Still open:** REXXUTIL's screen, keyboard, EA, INI and WPS functions, interactive tracing's pause, and an error output separate from the shell's output.
- **Tests:**
  - `program.rs`: `traps_and_trace_parse`;
  - `number.rs`: `engineering_notation_has_exponents_in_threes`;
  - `rexxutil.rs`: `attributes_and_masks`, `tree_lines_are_laid_out`;
  - `rexx.rs`, through a test host: `conditions_are_trapped`, `halt_is_raised_once`, `trace_writes_clauses_and_values`, `engineering_notation`, `rexxutil_functions_are_registered`, `default_streams`;
  - `rexx_host.rs`, with a temporary drive C: `streams_read_and_write_files_on_the_drives`, `rexxutil_works_with_files_on_the_drives`;
  - `tests/integration.rs`: `test_rexx_conditions_streams_and_rexxutil`, 14 checks in `rexxr2.cmd`, with TRACE R's output.
