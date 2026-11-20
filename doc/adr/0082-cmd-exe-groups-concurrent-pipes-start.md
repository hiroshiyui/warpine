# ADR-0082: The built-in CMD.EXE's groups, pipelines that run at the same time, and START

- **Status:** Accepted; completes Stage 2 of the built-in CMD.EXE (ADR-0077–0080)
- **Date:** 2026-11-20
- **Sources:**
  - OS/2's CMD.EXE as IBM's public *OS/2 Command Reference* describes it: the `( )` grouping symbols, the pipe, and START and its switches;
  - the code: `src/loader/cmd/parse.rs` (`split_chain`, `parse_command`), `src/loader/cmd.rs` (`execute_commands`, `group_inside`, `execute_pipeline`, `drain_pipe_input`), `src/loader/cmd/external.rs` (`spawn_external`, `start_in_background`, `cmd_start`), `src/api.rs` (`dos_write`), `src/loader/doscalls.rs` (`write_handle`);
  - the sample: `samples/cmd_tool` (`yes`).

## Context

Three items were left after slices 2a–2d:
- **Groups:** commands could not be grouped with parentheses.
- **Pipelines:** their stages ran one after another, so a program feeding the next had to finish first, and its whole output was held in memory (16 MiB at most, ADR-0077).
- **START** was missing.

Testing pipelines that run at the same time showed a bug under them. When a guest program's standard output was a host pipe or file, `DosWrite` ignored write errors and always reported every byte written. A program writing into a pipe whose reader had gone therefore never learned it, and kept writing forever. That affects any OS/2 program piped into a host command that stops reading early, not only the shell.

A windowed VIO session, CMD.EXE included, opens its own SDL renderer: its own desktop. Showing several VIO sessions on the session's shared desktop needs the desktop server to host VIO windows of other processes, which is not built (`doc/TODOs.md`).

## Decision

1. **Groups.** `( … )` groups commands, OS/2's grouping symbols.
   - **Splitting.** `split_chain` and `parse_command` treat `&`, `&&`, `||`, `|`, `<` and `>` inside parentheses as part of the group.
   - **Running.** A command that is all one group (`group_inside`) runs its inside through `execute_commands`, which is `execute_line` without the remark check and without expansion, so the line is expanded once.
   - **One command.** The group's redirection holds for all of it, `&&` / `||` after it look at its last command's return code, and IF and FOR can run a group.
   - **Limits.** Groups nest at most `MAX_GROUP_DEPTH` (64) deep. A `)` with no `(` is text.
2. **Pipeline stages run at the same time.** A program that feeds the next stage writes into a host pipe, and the next program reads it as its standard input while both run (`InputSource::Pipe`).
   - **Waiting.** The pipeline waits for such programs only at its end, after it has closed its own end of the pipe. A program still writing into a pipe nobody read to its end then gets a broken pipe, not a wait.
   - **Built-ins and scripts** in a pipeline keep their output in memory as before (16 MiB at most). A built-in that reads its input drains a pipe into bytes first (`drain_pipe_input`).
   - **Nested pipelines.** A pipeline inside a stage (a group's), and FOR's commands, do not pipe their programs to the outer next stage: their output is held for it.
3. **Console write errors reach the program.** `api::doscalls::dos_write` returns the host's write error. `DosWrite` on a console handle maps a broken pipe to `ERROR_BROKEN_PIPE` and other failures to `ERROR_ACCESS_DENIED`.
4. **START** starts a session of its own and goes on:
   - **What runs:**
     - no command, or `/K command`: a new CMD.EXE that stays;
     - `/C command`, or a command that is not a program: CMD.EXE runs it and ends;
     - a program, or `/N` / `/PGM`: the program itself.
   - **Display:** `/FS` full screen, `/WIN` (the default) a window. The session keeps the console, so on a terminal it opens its own window, a desktop of its own until several VIO sessions share one.
   - **Environment:** `/I` gives the session the shell's environment; without it, Warpine's default.
   - **Accepted and not acted on:** `/F`, `/B`, `/MAX`, `/MIN`, `/PM` and a `"title"`. `/DOS` and unknown switches are errors.
   - **Limits.** The session counts as one of the shell's children under the session's limits until it ends, as DETACH's programs do. Both share `start_in_background`.

## Warpine's choices (unverified)

- `&` inside parentheses after a command word (`echo (a & b)`) stays part of the command.
- Groups nest at most 64 deep.
- A built-in in a pipeline still holds its output in memory.
- Without `/I`, START gives Warpine's default environment.
- `/F`, `/B`, `/MAX`, `/MIN` and the title have no effect.
- A START session shows as its own window until several VIO sessions share one desktop.
- START writes nothing when it succeeds.

## Consequences

- **Stage 2 of the built-in CMD.EXE is complete.** What remains for the shell is taken case by case: more commands. Placing START's sessions on the shared desktop follows the VIO-window work. REXX is Stage 3 (ADR-0081).
- **A program that writes without end** into a pipe stops when its reader is done, in the shell and wherever its output is a host pipe.
- **Tests:**
  - `parse.rs`: `parentheses_group_commands`;
  - `cmd.rs`: `groups_run_as_one_command`, `start_refuses_what_it_cannot_start`;
  - `tests/integration.rs`: `test_cmd_pipeline_stages_run_together` (`cmd_tool yes` into a built-in that does not read, three programs in a row, a group into a program), `test_cmd_start_begins_sessions`.
