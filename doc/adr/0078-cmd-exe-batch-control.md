# ADR-0078: The built-in CMD.EXE's batch control

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - OS/2's CMD.EXE batch commands as IBM's public *OS/2 Command Reference* describes them (ECHO, `@`, `%0`–`%9`, SHIFT, IF, FOR, GOTO, CALL, SETLOCAL / ENDLOCAL, PAUSE), followed for what is implemented;
  - the code: `src/loader/cmd/batch.rs`, `src/loader/cmd/parse.rs` (`parse_if`, `parse_for`, `first_word`), `src/loader/cmd/env.rs` (`expand_vars` with parameters), `src/loader/cmd.rs` (dispatch), `src/loader/cmd/external.rs` (`exec_external`);
  - the samples: `samples/cmd_scripts/batch.cmd`, `batchsub.cmd`.

## Context

Slice 2a gave the shell return codes, expansion and chaining (ADR-0077). A `.CMD` script, though, still ran its lines once, top to bottom: it could not branch, loop, take arguments or call another script and come back. Slice 2b adds OS/2's batch control, so that scripts written for OS/2's CMD.EXE run. The syntax stays OS/2's, as decided for Stage 2.

## Decision

1. **The script runner** (`batch.rs`). A script is read whole, decoded in the process codepage, and run from a position GOTO can move. Its state is a `Batch`:
   - its lines and the next one to run;
   - `%0`–`%9`;
   - the open SETLOCALs;
   - whether a GOTO ended the current line.

   It runs in a nested `CmdShell` that shares the caller's environment, output and input (as in 2a) and its ECHO state.
2. **ECHO.** It is on when a script starts.
   - **What is echoed.** While it is on, each line is shown after the prompt, expanded, before it runs. Lines starting with `@`, labels and empty lines are not shown.
   - **The state** is shared by a script and the scripts it runs. The prompt has its own, which a script does not change.
   - **The forms:** `ECHO ON`, `ECHO OFF`, `ECHO` (shows the state), `ECHO text` (the text after one blank, kept as typed) and `ECHO.` / `ECHO.text`.
   - **At the prompt,** `ECHO OFF` hides the prompt.
3. **Parameters.** `%0` is the script's name as typed, and `%1`–`%9` are its arguments. A missing one is nothing.
   - They are expanded in the same pass as `%name%` and `%%`.
   - At the prompt they stay as typed.
   - `SHIFT` drops `%0`, so each argument moves down by one.
4. **GOTO label** goes on after the line `:label`. The label's first word is matched without case, and the search starts at the script's top.
   - **The rest of GOTO's line** is not run.
   - **A missing label** ends the script with an error and return code 1.
   - **At the prompt** GOTO does nothing.
5. **IF [NOT] condition command:**
   - `ERRORLEVEL n` holds when the return code is `n` or more;
   - `EXIST name` holds when a file or directory matches `name` (wildcards allowed, through the VFS);
   - `a==b` holds when the strings are equal, case and quotes included; blanks around `==` are allowed.

   The command runs when the condition holds (or does not, with NOT) and leaves its own return code. A condition that does not hold leaves the return code as it was. A malformed IF is a syntax error with return code 1.
6. **FOR %v IN (set) DO command** runs the command once for each item of the set, with `%v` replaced by the item. A script writes `%%v`, which expansion makes `%v`.
   - **The set** is split at blanks, commas and semicolons.
   - **Wildcards.** An item with `*` or `?` stands for the files it matches (not directories), with the directory part as written.
   - **Limit:** at most `MAX_FOR_ITEMS` (65,536) items.
7. **CALL script args** runs a script and comes back; CALL of a program runs it as usual.
   - **Without CALL,** a script started from a script takes its caller's place: the caller does not go on afterwards.
   - **Nesting** is limited to `MAX_SCRIPT_DEPTH` (32), CALL and plain starts alike, because each level is a nested call on the host's stack.
8. **EXIT** in a script ends the whole shell with its code, as OS/2's EXIT ends CMD.EXE.
9. **SETLOCAL** saves the environment and the current drive and directories (`inherit::cwd_strings`). **ENDLOCAL** puts back the innermost saved state.
   - **The script's end** closes the SETLOCALs it left open.
   - **Limit:** at most `MAX_LOCAL_DEPTH` (32) open at once.
   - **At the prompt** both do nothing.
10. **PAUSE** writes a message and waits for a key: from the keyboard, or one byte of a pipe or file read as input.
11. **Limits:** a script is at most 4 MiB.

## Warpine's choices (unverified)

- PAUSE's wording: "Press any key when ready . . .".
- The echoed form of a line: the prompt, then the expanded line, with no blank line before it.
- ECHO's state is separate for the prompt and for scripts.
- At the prompt, ECHO OFF hides the prompt.
- The message for a missing label, and the return code 1 that comes with it.
- `IF EXIST` matches directories as well as files.
- FOR's wildcard items keep the directory part as written.
- FOR's variable replaces only `%v` with the same letter case.
- The limits on script nesting (32), SETLOCAL depth (32), FOR items (65,536) and script size (4 MiB).
- Not implemented: IF's `/I`, ELSE and `DEFINED`; FOR's switches; `%*`; GOTO `:EOF`; `EXIT /B`. These come from later shells than OS/2's, or OS/2's does not have them.

## Consequences

- **`.CMD` scripts** written for OS/2's CMD.EXE run: branching, loops, arguments and subroutines in other scripts.
- **ECHO's default is a visible change.** Scripts without `@ECHO OFF` now show their lines, as on OS/2; `samples/cmd_scripts/basics.cmd` starts with `@ECHO OFF` for that reason.
- **Tests:**
  - `batch.rs`:
    - `script_lines_are_echoed_until_echo_off`;
    - `goto_jumps_to_labels`;
    - `if_tests_errorlevel_exist_and_strings`;
    - `for_runs_a_command_per_item`;
    - `parameters_shift`;
    - `setlocal_restores_environment_and_directory`;
    - `call_returns_and_a_plain_start_does_not`;
    - `scripts_nest_only_so_deep`;
    - `pause_reads_a_key`;
  - `parse.rs`: `if_commands_parse`, `for_commands_parse`;
  - `env.rs`: `script_parameters_expand`;
  - `tests/integration.rs`: `test_cmd_script_batch_control` (with `samples/cmd_tool` for ERRORLEVEL) and `test_cmd_script_lines_are_echoed`.
