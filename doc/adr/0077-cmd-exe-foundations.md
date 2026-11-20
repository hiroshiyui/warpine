# ADR-0077: The built-in CMD.EXE's foundations: OS/2 syntax, return codes and what programs inherit

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - OS/2's CMD.EXE syntax as IBM's public *OS/2 Command Reference* describes it (`&`, `&&`, `||`, `^`, `%name%`, `%%`), followed for the commands Warpine implements;
  - the code:
    - `src/loader/cmd.rs` and `src/loader/cmd/` (`builtins.rs`, `env.rs`, `external.rs`, `input.rs`, `parse.rs`);
    - `src/loader/inherit.rs` (`CWD_ENV`, `cwd_strings`, `apply_cwd`);
    - `src/loader/vfs.rs` (`set_current_dir`, `normalize_rel_path`, `current_dir_os2`, `resolve_path`);
    - `src/loader/doscalls.rs` (`DosQueryCurrentDir`, redirected `DosRead`);
    - `src/loader/process.rs`, `src/loader/pm_startapp.rs`;
  - the samples: `samples/cmd_tool`, `samples/cmd_scripts/basics.cmd`.

## Context

With 4OS2 removed (ADR-0075), the built-in `CMD.EXE` is Warpine's shell, and Stage 2 grows it in four slices (`doc/TODOs.md`). The first slice, the foundations, is what the batch commands, file commands and prompt of the later slices build on. Before it:
- **No return codes.** A program's exit status was thrown away, so there was nothing for `&&`, `||` or `IF ERRORLEVEL` to test, and `CMD.EXE /C` always ended with 0.
- **No `%name%` expansion**, and no `&`, `&&` or `||`.
- **Programs did not get the shell's pipes or files.** A program in a pipeline read the host terminal and took the shell's remaining input.
- **Redirected standard input was echoed.** `DosRead` on a pipe or file echoed every byte to standard output and turned CR into CR LF, as for typed keys.
- **Children lost the current directory.** No child process learned its parent's OS/2 current drive and directories: not one started by the shell, nor by `DosExecPgm` or `WinStartApp`. Each of the three instead set the child's *host* working directory to the host's own directory joined with the OS/2 path, a directory that usually did not exist.
- **Two copies of the current directory.** `ProcessManager` kept one beside the VFS's and synchronised it by hand.
- **`set_current_dir` did not check its path.** It took any path, missing directories included, and kept `.` and `..` as they were; `\` and `C:\` resolved to the current directory instead of the root.
- **`DosQueryCurrentDir` returned `/` separators** and wrapped a drive number above 255 round to some drive.
- **Several host arguments lost the command.** `warpine CMD.EXE /C "DIR C:\"` arrives as several host arguments, and the shell read only the first, so `/C` had no command.
- **Redirections did not restore what was there before.** Every command reset output to the screen and input to the keyboard instead. A script, run in a fresh shell, could not have its output redirected.

The project's goal is an OS/2-alike API, not a clone (ADR-0075). The shell's syntax still follows OS/2's CMD.EXE, so that people who know OS/2, and `.CMD` files written for it, find a familiar shell.

## Decision

**Syntax: OS/2's CMD.EXE**, for what is implemented. Where IBM's reference leaves a detail open, Warpine chooses and lists the choice below.

1. **A command line is processed in this order:**
   1. **A `REM` line** is skipped whole, `&` and `|` included, and so is a label line (`:name`, which `::` comments also are).
   2. **Variables are expanded**, in one pass (`env.rs`):
      - `%name%` becomes the variable's value, its name matched without case; an undefined one becomes nothing;
      - `%%` becomes `%`;
      - a `%` that starts no `%name%` stays;
      - a value is not expanded again.
   3. **The line is split into commands** at `&` (always run), `&&` (run when the return code so far is 0) and `||` (run when it is not 0), outside quotes and not escaped (`parse.rs`). They run left to right.
   4. **Each command is split into a pipeline** with its redirections. `^` outside quotes takes the next character as it is: `^&`, `^|`, `^>`.
2. **Return codes.** Every command leaves one:
   - a program, its exit code;
   - a `.CMD` script, `EXIT`'s code or its last command's;
   - a built-in, 0, or 1 (`RC_BUILTIN_FAILED`) when it reported an error;
   - a program that cannot be found or started, 1.

   `CMD.EXE /C` ends with the return code of its command line.
3. **Programs read the pipe or file before them.** The bytes of a pipe, or of `< file`, become the program's standard input, which then ends. A program with neither shares the shell's standard input.
   - **Bounded memory.** The commands of a pipeline run one after another, so the shell holds each command's output in memory. It keeps at most `MAX_CAPTURED_OUTPUT` (16 MiB) and reads and drops the rest, so a guest cannot grow the host's memory without bound.
4. **Redirected standard input is data.** `DosRead` on standard input that is a host pipe or file reads up to the requested length, with no echo and no CR → CR LF. Echo and line editing stay for the console only.
5. **Program search.** A bare name is looked for in the current directory, then in each directory of `PATH`. A name with a drive or a directory is looked for only there. Without an extension, `.EXE` is tried before `.CMD`.
6. **Children inherit the current drive and directories.** Each Warpine process has its own VFS, so a parent passes them in `WARPINE_OS2CWD` (`inherit.rs`):
   - **The format:** `X:\dir` strings separated by NULs, with the current drive first.
   - **Who sends them:** the shell, `DosExecPgm` and `WinStartApp`. For `WinStartApp`, the program's start directory becomes the current drive and directory.
   - **What the child does:** it sets each through its own VFS, which checks that the directory exists inside its drive, and drops what fails.
   - **The host's working directory** is no longer set from a guest path.
   - **One copy.** `ProcessManager::current_dir` and the unused `translate_path` are removed; the VFS holds the only current directories.
7. **Current directories exist.** `DriveManager::set_current_dir` collapses `.` and `..` and takes only an existing directory. `\` and `C:\` are the root; `C:` is the drive's current directory. `DosQueryCurrentDir` writes `\` separators and refuses a drive that is not mounted.
8. **The shell's arguments.** The host arguments of `warpine CMD.EXE …` are joined by blanks into OS/2's single argument string.
9. **Redirections hold for one command only.** Afterwards, the output and input that were there before come back. A `.CMD` script shares its caller's output and input, so `SCRIPT.CMD > LOG.TXT` and `X | SCRIPT` work.
10. **The module is split** into `cmd.rs` (the shell and dispatch) and `cmd/`:
    - `builtins.rs`: the built-in commands;
    - `env.rs`: the environment and its expansion;
    - `external.rs`: programs and scripts;
    - `input.rs`: the line editor;
    - `parse.rs`: the command line.

## Warpine's choices (unverified)

- `%name%` expands at the prompt as in scripts, and an undefined variable becomes nothing in both.
- `%%` becomes `%` at the prompt too.
- A built-in returns 0, or 1 after an error.
- A program the shell cannot find returns 1.
- A program that ends without an exit code (killed by a signal) returns 255.
- Host exit codes have 8 bits, so a program's exit code reaches the shell modulo 256.
- `&&` and `||` look at the return code the line has so far, left to right; there is no grouping with parentheses.
- The commands of a pipeline run one after another, not at the same time, so a program that never ends blocks the pipeline.
- The output a pipe holds is capped at 16 MiB.
- `CD` to a file, or `..` above the root, fails with `ERROR_PATH_NOT_FOUND`.
- The current directory keeps the case it was typed in.
- Program search tries `.EXE` before `.CMD`; `.COM` and `.BAT` are not tried.

## Consequences

- **The batch slice (2b) builds on these foundations.** `IF ERRORLEVEL` has its return code, and `%0`–`%9` extend the same expansion.
- **Programs started by any means** begin in their parent's directory, as on OS/2.
- **Samples:** `samples/cmd_tool` is a probe for these tests: `exit N`, `cat`, `count`, `cwd`, and `exec DIR`, which tests `DosExecPgm`'s directory handoff. `samples/cmd_scripts/basics.cmd` is a script that uses the foundations.
- **Tests:**
  - `cmd.rs`:
    - `builtins_leave_return_codes_for_and_and_or`;
    - `slash_c_returns_the_commands_return_code`;
    - `programs_are_found_in_the_current_directory_then_along_path`;
    - `a_scripts_output_follows_its_redirection`;
    - `pipe_data_reads_as_lines`;
    - `captured_output_is_bounded`;
  - `parse.rs`: `commands_split_at_and_or_and_then`, `caret_escapes_the_next_character`;
  - `env.rs`: `variables_expand_once`;
  - `vfs.rs`: `current_directories_exist_and_stay_on_their_drive`;
  - `inherit.rs`: `current_directories_pass_to_a_child`;
  - `doscalls.rs`: `query_current_dir_uses_backslashes_and_checks_the_drive`;
  - `tests/integration.rs`:
    - `test_cmd_pipe_to_an_external_program`, enabled again;
    - `test_cmd_return_codes_drive_and_or_and_the_shell`;
    - `test_cmd_programs_get_pipes_files_and_the_current_directory`;
    - `test_cmd_script_basics`.
