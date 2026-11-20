# ADR-0083: REXX R1: the language core in the built-in CMD.EXE

- **Status:** Accepted; phase R1 of ADR-0081
- **Date:** 2026-11-20
- **Sources:**
  - IBM's public *OS/2 Procedures Language 2/REXX Reference*;
  - M. F. Cowlishaw, *The REXX Language*;
  - ANSI X3.274-1996 (ADR-0081);
  - the code:
    - `src/loader/rexx.rs` and `src/loader/rexx/`: `number.rs`, `lexer.rs`, `expr.rs`, `program.rs`, `interp.rs`, `template.rs`, `builtins.rs`;
    - `src/loader/cmd/rexx_host.rs`, `src/loader/cmd/batch.rs` (`run_script`), `src/loader/cmd/external.rs` (`find_program_with_name`);
  - the samples: `samples/rexx_scripts/` (`rexxtest.cmd`, `rexxhelp.cmd`, `rexxask.cmd`), `samples/cmd_tool`.

## Context

ADR-0081 decided that REXX comes to the built-in CMD.EXE as our own interpreter in Rust, in three phases. R1 is the language core: enough for real scripts to compute, branch, loop, call routines, parse strings and drive commands.

## Decision

1. **Which scripts.** A `.CMD` whose first line starts with `/*` (blanks before it allowed) runs as REXX, not as a batch file (`rexx_host::is_rexx`). Its argument string is its arguments as typed, read with `PARSE ARG`.
2. **Where it runs.** The interpreter runs in the host, on a thread of its own with a 64 MiB stack, so deep recursion stays within the interpreter's own limits. The shell is its `Host`:
   - **SAY** writes to the shell's output;
   - **commands** run in the shell (`ADDRESS CMD`, the default environment), and `RC` takes their return code. While ECHO is on, a command is echoed with the prompt unless it starts with `@`, as on OS/2, hence the usual `'@ECHO OFF'`;
   - **PULL** reads the queue, or the shell's input when the queue is empty;
   - **the session clock** gives `DATE` and `TIME`;
   - **external routines** are other REXX `.CMD` files, found like programs (current directory, then `PATH`);
   - **`VALUE(name, new, 'OS2ENVIRONMENT')`** reads and sets the shell's environment.
3. **Values are byte strings,** as in classic REXX. Scripts are read in the process codepage; the shell decodes only at its edges.
4. **Arithmetic** (`number.rs`): decimal numbers of any size: a sign, a coefficient and an exponent.
   - **Rounding.** Each result is rounded to `NUMERIC DIGITS` significant digits (9 by default, at most 1000), half up.
   - **Trailing zeros** are kept except after division, so `1.50 + 1` is `2.50`, `1.5 * 2` is `3.0` and `6 / 2` is `3`.
   - **The operators** are `+ - * / % // **`, with `**` taking whole exponents.
   - **Notation.** A result is written in exponential form when it would need more than DIGITS places before the point, or more than twice DIGITS after it.
   - **Comparison.** Normal comparisons are numeric when both sides are numbers (`NUMERIC FUZZ` digits ignored). Otherwise they compare strings without leading and trailing blanks, the shorter padded with blanks. The strict comparisons compare bytes.
5. **Syntax** (`lexer.rs`, `expr.rs`, `program.rs`):
   - **Tokens and clauses.** Clauses end at line ends (a trailing comma continues) and at `;`. Comments nest. `X` and `B` strings are supported. `¬` (0xAA) is NOT.
   - **Expressions.** Precedence is: prefix > `**` > `* / % //` > `+ -` > concatenation (blank, abuttal, `||`) > comparison > `&` > `| &&`, left to right within a level.
   - **The instructions:**
     - `SAY`, assignment, commands, `NOP`;
     - labels, `IF` / `THEN` / `ELSE`;
     - `DO` (`var = start TO BY FOR`, a count, `FOREVER`, `WHILE`, `UNTIL`), `END [var]`, `ITERATE` / `LEAVE [var]`, `SELECT` / `WHEN` / `OTHERWISE`;
     - `CALL`, `RETURN`, `EXIT`, `PROCEDURE [EXPOSE …]` (with `(var)` lists), `DROP`, `SIGNAL label` / `SIGNAL VALUE`, `INTERPRET`;
     - `PARSE [UPPER] ARG | PULL | VAR | VALUE … WITH | SOURCE | VERSION | EXTERNAL | LINEIN`, `ARG`, `PULL`, `PUSH`, `QUEUE`;
     - `ADDRESS [env [command] | VALUE expr]`, `NUMERIC DIGITS | FUZZ | FORM`;
     - `TRACE`, accepted and not acted on.
   - **The run loop.** The program is a flat list of instructions with jump tables for IF, DO, SELECT and WHEN, so SIGNAL and CALL reach any label.
6. **PARSE templates** (`template.rs`):
   - **Patterns.** Literal patterns (strings or `(var)`) are searched for from the current position. Absolute positions (`n`, `=n`, `=(v)`) and relative ones (`+n`, `-n`, counted from the last match's start) move to their position; a position left of or at the current one takes the piece to the end.
   - **Words.** The targets between two patterns get one word each, and the last gets the rest after the blank that ended the previous word; a lone target gets the whole piece. `.` drops a part.
7. **Routines** (`interp.rs`):
   - **The search order:** an internal label (not for a quoted name), then a built-in function, then an external REXX program, which gets its own variables.
   - **Settings.** NUMERIC and ADDRESS settings come back when a routine returns.
   - **PROCEDURE** makes a new scope; `EXPOSE` links names (simple, stems, compound) to the caller's scope.
   - **EXIT** inside a routine ends the program.
   - **Stems** have a default value (`S. = 0`), and a dropped entry has no value even with one.
8. **Built-in functions** (`builtins.rs`):

   | Kind | Functions |
   |------|-----------|
   | Strings | ABBREV, CENTER / CENTRE, COMPARE, COPIES, DELSTR, INSERT, LASTPOS, LEFT, LENGTH, OVERLAY, POS, REVERSE, RIGHT, SPACE, STRIP, SUBSTR, TRANSLATE, VERIFY, XRANGE, BITAND / BITOR / BITXOR |
   | Words | DELWORD, SUBWORD, WORD, WORDINDEX, WORDLENGTH, WORDPOS, WORDS |
   | Conversions | B2X, C2D, C2X, D2C, D2X, X2B, X2C, X2D |
   | Numbers | ABS, FORMAT (up to three arguments), MAX, MIN, RANDOM, SIGN, TRUNC |
   | Information | ADDRESS, ARG, DATATYPE, DATE, DIGITS, ERRORTEXT, FORM, FUZZ, QUEUED, SOURCELINE, SYMBOL, TIME, TRACE, VALUE |

   Each checks its arguments; a wrong one is Error 40.
9. **Errors.** A syntax or run-time error ends the script. The shell writes the failing line (`    10 +++ …`) and `REXnnnn: Error n running C:\NAME.CMD, line l: text` with the language's message text. The error number is the script's return code. An `EXIT` (or `RETURN`) value that is a whole number is the return code; any other value gives 0.
10. **Bounded resources:**
    - routines active inside one another: 100;
    - INTERPRETs active inside one another: 20;
    - expression nesting: 200;
    - `NUMERIC DIGITS`: 1000;
    - a string: 16 MiB;
    - stem entries in all: 2^20;
    - queue lines: 65,536;
    - an external routine's source: 4 MiB.

    Going past one is Error 5 or 11, not a crash.

## Warpine's choices (unverified)

- Blanks may come before the `/*` that makes a script REXX.
- The last target of a word list keeps the rest's blanks except the one ending the previous word.
- The error number as the script's return code; a non-numeric EXIT value giving 0.
- A command to an unknown environment sets `RC` to -3 (the FAILURE condition comes in R2).
- Commands are echoed while ECHO is on.
- An `EXIT` sent as a command ends only that command.
- External routines are REXX `.CMD` files only, not batch files.
- PARSE VERSION's `REXX-Warpine_<version> 4.00 20 Nov 2026`.
- DATE('L') as `20 November 2026`.
- TIME('L')'s microseconds come from the clock's hundredths.
- RANDOM's sequence (splitmix64), not IBM's.
- The resource limits above.

## Consequences

- **What scripts can do.** REXX scripts can compute, parse and drive OS/2 programs through the shell, with RC as their result.
- **Left for R2:**
  - stream I/O (CHARIN, LINEIN and the others are Error 43 until then);
  - conditions (`SIGNAL ON` / `CALL ON` are Error 25 until then);
  - `TRACE`'s output, and `NUMERIC FORM ENGINEERING`'s notation, which is recorded but not yet used when writing numbers;
  - REXXUTIL.

  Left for R3: the REXX API for programs.
- **Tests:**
  - `number.rs` (4), `lexer.rs` (3), `expr.rs` (3), `program.rs` (4);
  - `rexx.rs` (7, through a test host): expressions, control structures, routines and variables, templates, commands, built-in functions, errors;
  - `rexx_host.rs`: `rexx_scripts_start_with_a_comment`;
  - `tests/integration.rs`: `test_rexx_script_language_core` (16 checks, with `cmd_tool` for RC and an external routine), `test_rexx_pull_and_error_report`.
