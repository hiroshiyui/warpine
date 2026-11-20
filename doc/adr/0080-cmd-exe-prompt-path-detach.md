# ADR-0080: The built-in CMD.EXE's PROMPT, PATH and DETACH

- **Status:** Accepted; completes Stage 2 of the built-in CMD.EXE (ADR-0077–0079), except START
- **Date:** 2026-11-20
- **Sources:**
  - OS/2's PROMPT (its `$` codes), PATH and DETACH as IBM's public *OS/2 Command Reference* describes them, followed for what is implemented;
  - the code: `src/loader/cmd/prompt.rs`, `src/loader/cmd/builtins.rs` (`cmd_prompt`, `cmd_path`), `src/loader/cmd/external.rs` (`child_command`, `cmd_detach`), `src/loader/cmd.rs` (`write_prompt`), `src/loader/lx_loader.rs` (`Loader::load`);
  - the samples: `samples/cmd_scripts/session.cmd`, `samples/cmd_tool` (`touch`).

## Context

Stage 2's last slice:
- **PROMPT.** The prompt was fixed at `[C:\DIR] `.
- **PATH.** `PATH` was only a variable, set with `SET`.
- **DETACH.** There was no way to start a program in the background.

START needs several VIO sessions on one desktop, which Warpine does not have yet (`doc/TODOs.md`), so it waits.

While I checked the code for unchecked `length - 1` arithmetic (the `DosEditName` fault, ADR-0079), I found one more case: `Loader::load` did not check the executable's entry and stack object numbers. The LX parser lets 0 through, which is valid for a DLL, so an executable claiming object 0 made `setup_guest` index the object table at `0 - 1` and panic.

## Decision

1. **PROMPT text** sets the variable `PROMPT`; **PROMPT** alone removes it. The prompt is the template with its `$` codes replaced, letters without case (`prompt.rs`):

   | Code | Shows |
   |------|-------|
   | `$P` | the current drive and directory |
   | `$N` | the current drive's letter |
   | `$G` / `$L` | `>` / `<` |
   | `$Q` | `=` |
   | `$A` / `$B` | `&` / `\|` |
   | `$C` / `$F` | `(` / `)` |
   | `$S` | a blank |
   | `$_` | CR LF |
   | `$$` | `$` |
   | `$H` | a backspace |
   | `$E` | ESC, for ANSI sequences |
   | `$D` | the date, `Thu 11-20-2026` |
   | `$T` | the time, `13:05:09.42` |
   | `$V` | VER's line |
   | `$R` | the last return code |

   `$I` (OS/2's help line) and unknown codes show nothing.
   - **The default** without `PROMPT` is `[$P] `, in the header colour as before.
   - **A template of one's own** is written in the normal colour, so `$E[…m` sequences colour it. The colour of the text typed after it is restored.
   - **The date and time** come from the session clock, which `DosSetDateTime` moves, not from the host's.
2. **PATH** shows `PATH=…` (or `No Path.`); **PATH ;** clears it; **PATH dirs** sets it. Programs are looked for along it (ADR-0077).
3. **DETACH program args** starts a program in the background, without a console: its standard input, output and error are empty. The shell goes on at once and writes `The Process Identification Number is n.`
   - **Limits.** The program is admitted under the session's limits (nesting, children, THREADS=) and counts as one of the shell's children until it ends, so a script cannot start detached programs without bound.
   - **Reaping.** A thread of the shell takes the ended program off the list.
   - **Refusals:** a script, a missing program and an empty command line are errors.
4. **The shared start-up.** Starting a program, in the foreground or detached, shares one setup (`child_command`): the shell's environment and current directories, and the admission.
5. **Entry and stack objects.** `Loader::load` refuses an executable whose entry or stack object is not one of its objects (`InvalidData`), before anything indexes the object table.

## Warpine's choices (unverified)

- `$D`'s and `$T`'s formats; `$I` showing nothing.
- The default prompt `[$P] ` with its trailing blank.
- A prompt template keeps the blanks after `PROMPT `, after one separator.
- `No Path.`
- DETACH's message.
- DETACH refusing scripts.
- A detached program's output is discarded, not shown.

## Consequences

- **Stage 2 is complete except START,** which waits for several VIO sessions. What remains for the shell is listed in `doc/TODOs.md` and taken case by case: grouping with parentheses, pipeline stages running at the same time, more commands.
- **A malformed executable** gets a load error instead of crashing Warpine.
- **Tests:**
  - `prompt.rs`: `dollar_codes_render`;
  - `cmd.rs`: `prompt_sets_the_prompt`, `path_shows_sets_and_clears`, `detach_refuses_what_it_cannot_run`;
  - `lx_loader.rs`: `a_program_needs_entry_and_stack_objects`;
  - `tests/integration.rs`: `test_cmd_script_path_detach_and_prompt`, where the detached `cmd_tool touch` makes a file after the shell went on.
