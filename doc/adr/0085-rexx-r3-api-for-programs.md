# ADR-0085: REXX R3: the REXX API for OS/2 programs

- **Status:** Accepted; phase R3 of ADR-0081, after ADR-0083 and ADR-0084
- **Date:** 2026-11-20
- **Sources:**
  - IBM's public REXX Application Programming Interface description (OS/2 *REXX Program Reference*);
  - Open Watcom's `h/os2/rexxsaa.h` and its import library `lib386/os2/rexx.lib` (the ordinals, as `os2386.lib` gives the others: ADR-0006);
  - the OS/2 Warp 4.5 Toolkit's `rexxsaa.h` at the commit ADR-0072 pins (a cross-check; it has no queue functions either);
  - the code:
    - `src/loader/rexxapi.rs` (the API, the sessions, the chain glue), `src/loader/rexxapi/host.rs` (the interpreter's host), `src/loader/rexx/embed.rs` (the interpreter's side), `src/loader/rexx_vfs.rs` (files, shared with the shell);
    - `ChainThen::Rexx` in `pm_hook.rs`; `run_shell_command` in `cmd.rs`;
    - `REXX_BASE` / `REXXAPI_BASE` in `constants.rs`, `targets/os2api.def`, `doc/os2_ordinals.md`;
  - the sample: `samples/rexx_api_test`.

## Context

OS/2 programs embed REXX through `REXX.DLL` and `REXXAPI.DLL`:
- install programs and editors run macros with `RexxStart`;
- they give scripts commands of their own (subcommand handlers) and functions (external function handlers);
- they capture SAY or set variables before a run (system exits, the variable pool);
- they pass lines through queues.

ADR-0081 planned this as R3, after the interpreter (R1, ADR-0083) and its conditions, streams and REXXUTIL (R2, ADR-0084). Its open question was where the ordinals come from.

## Decision

1. **Two built-in modules,** `REXX` (base 26624) and `REXXAPI` (base 27648), after SOM. The stub area grows to 28672.
2. **The ordinals** are those of Open Watcom's `rexx.lib` IMPDEF records: REXX 1–11, REXXAPI 1–48. They are added to `doc/os2_ordinals.md` (REXX and REXXAPI sections) and checked by `gen_api validate-doc`, as `os2386.lib`'s are (ADR-0006).
3. **Implemented:**
   - REXX: RexxStart, RexxVariablePool, RexxSetHalt, RexxSetTrace, RexxResetTrace;
   - REXXAPI: the five queue functions; register / query / deregister of subcommand handlers, system exits and external functions (Exe and Dll forms).

   The macro space (REXXAPI.22–28) is a stub that stores nothing and finds nothing. The 16-bit entry points (REXX.6–11, REXXAPI.30–48), RexxLoadSubcom, RexxCallExit, RexxCallSubcom, RexxCallFunction and RexxExecuteMacroFunction are not registered.
4. **The threading model.**
   - **An interpreter thread.** `RexxStart` runs the program on a new host thread with a 64 MiB stack. The thread has a `Loader` of its own over the process's shared state, so it uses the program's handles and memory directly.
   - **Handler calls.** A handler is guest code. The interpreter thread lays the call's arguments out in new guest memory (RXSTRINGs and their bytes) and sends the call to the thread that called RexxStart.
   - **The calling thread** waits in `rexx_wait` and makes the call as a call chain (ADR-0026, `ChainThen::Rexx`). Each result goes back to the interpreter, and the thread waits again until the program ends. Calls nest: a handler may call RexxStart again.
   - **The variable pool.** A handler's `RexxVariablePool` runs on the calling thread. It reads the SHVBLOCKs, sends the requests to the interpreter (which waits for the handler) and writes the answers back.
5. **The interpreter's side** (`embed.rs`):
   - **Callbacks.** `Interp::callback` asks the host for a handler call and answers pool requests until it returns. While it runs, the program's queue is lent to the host as the `SESSION` queue.
   - **Exits.** SAY, TRACE and terminal input go through the `RXSIO` exit first, then the host. `RXINI` and `RXTER` surround the program, and `RXHLT`'s test runs after each clause.
   - **Commands** go to a subcommand handler before the host's own environments.
   - **Function search order:** internal, built-in, REXXUTIL, a registered function, then an external program.
6. **The host for RexxStart:**
   - SAY writes to standard output, TRACE and error reports to standard error, and PULL reads standard input: the program's handles 1, 2 and 0, wherever they lead.
   - `ADDRESS CMD` runs in the built-in shell, in the process, as `DosExecPgm("CMD.EXE")` already does.
   - Files go through the VFS (`rexx_vfs.rs`, shared with the shell).
   - The environment is a copy of the program's environment block.
7. **RexxStart's results:**
   - **Return value.** 0 when the program ran. Minus the REXX error number when an error ended it, with the error written as the shell writes it. -3 when the program cannot be read; `ERROR_INVALID_PARAMETER` for bad arguments; `RXEXIT_NOTREG` / `RXEXIT_BADTYPE` for a bad exit list.
   - **`retc`** is the result as a 16-bit whole number, else 0.
   - **`result`** goes into the caller's buffer when it is large enough, else into new memory the caller frees with DosFreeMem.
8. **Registrations, queues and running programs** belong to the process (`SharedState::rexx_api`, taken alone). A `…Dll` registration is resolved by name among the modules the program has loaded, when it is called.
9. **Bounded resources:**
   - RexxStart's arguments: 1024, 64 MiB together;
   - a program: 4 MiB;
   - one RexxVariablePool call: 4096 SHVBLOCKs, 64 MiB of names and values;
   - exits in a list: 64;
   - handlers of each kind, and queues: 1024;
   - lines in all queues: 65,536;
   - a handler call's block: 16 MiB plus its strings' room.

   A thread that must stop (it is killed or signalled, or the process ends) halts its program, and gives it up after 5 seconds.

## Warpine's choices (unverified)

- The queue functions' signatures and return codes (`RXQUEUE_*`): no public header declares them; they follow IBM's description.
- Queue names: upper case, letters, digits and `.!?_`; generated names `Qn`; `RexxCreateQueue` answering `RXQUEUE_DUP` with the new name.
- `RexxDeleteQueue("SESSION")` failing with `RXQUEUE_BADQNAME`.
- `RexxPullQueue`'s DATETIME being the time of the pull.
- Queues and registrations per process rather than for the system.
- Registering a name twice failing (`RXSUBCOM_NOTREG`, `RXFUNC_DEFINED`, `RXEXIT_NOTREG`), not replacing.
- RexxStart's default environment `CMD`, its -3 for a missing program or an image, `ERROR_INVALID_PARAMETER` for bad arguments, and a program file searched with `.CMD` in the current directory and the PATH.
- `retc` 0 for a result that is not a 16-bit whole number; a missing result as length 0.
- A subcommand handler's ULONG result ignored, RC taken from its return string ("0" when empty).
- `RXINI` / `RXTER` called with a parameter block of four zero bytes.
- New memory for a variable-pool value whose RXSTRING has no buffer.
- The `RXFNC`, `RXCMD`, `RXMSQ` and `RXTRC` exits accepted and never called.
- `RexxSetTrace` meaning `TRACE ?R` and `RexxResetTrace` `TRACE O`; `RexxSetHalt` for any thread when `tid` is 0.
- TRACE output on standard error for RexxStart (the shell's own output for the shell, ADR-0084).
- The environment as a copy: `VALUE(…, 'OS2ENVIRONMENT')` changes the copy, not the process's environment block.
- The abandon time of 5 seconds.

## Consequences

- **What programs can do.** OS/2 programs that embed REXX run their macros on Warpine's interpreter, with their own commands, functions, exits and queues.
- **Still open:** the macro space, tokenized images, the remaining exits, the 16-bit entry points, system-wide queues and registrations, and `RxFuncAdd` of DLL function packages from a script.
- **Tests:**
  - `rexx.rs`, through a test host: `handlers_are_called_and_use_the_variable_pool`, `the_pool_walks_the_variables_and_handlers_can_fail`, `the_session_queue_is_the_hosts`;
  - `rexxapi.rs` on a mock loader: `handlers_register_once_and_are_queried`, `queues_hold_lines`, `the_pool_and_signals_need_a_program`, `queue_names_are_checked`;
  - `tests/integration.rs`: `test_rexx_api_from_a_program`, 17 checks in `samples/rexx_api_test`, with guest handlers called back from the interpreter.
