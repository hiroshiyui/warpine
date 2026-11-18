# ADR-0028: Limit how deeply Warpine processes start one another and how many children each keeps

- **Status:** Accepted; ADR-0032 adds the session-wide CONFIG.SYS THREADS= limit
- **Date:** 2026-11-18
- **Sources:** batch 5; `src/loader/inherit.rs` (`admit_child`, `WARPINE_DEPTH`, `MAX_PROCESS_DEPTH`, `MAX_LIVE_CHILDREN`), `src/loader/process.rs`, `src/loader/pm_startapp.rs`, `src/loader/cmd.rs`, `src/main.rs`; `samples/nest_test`; the out-of-memory incident of 2026-11-18; ADR-0020, ADR-0022

## Context

Each OS/2 process is a separate Warpine host process with its own VM (ADR-0020), roughly 290 MB of host memory. `DosExecPgm`, `WinStartApp` and CMD.EXE start such processes on a guest program's request.

On 2026-11-18 a test sample started itself through `WinStartApp` and relied on `argv` to tell the copy it was the child. Open Watcom programs under Warpine do not get `argv` yet, so every copy ran as a parent and started another one. About 230 processes later the host ran out of memory and the kernel's OOM killer ended unrelated work.

A guest program must not be able to exhaust the host this way, by accident or on purpose.

## Decision

- **A depth counter.** Every child Warpine process gets `WARPINE_DEPTH` = parent depth + 1; the first process is depth 0.
  - At start-up, `main.rs` checks it before running guest code: a process deeper than `MAX_PROCESS_DEPTH` (8), or with a malformed value, exits with code 89 (`ERROR_NO_PROC_SLOTS`).
  - `clear_parent_vars` keeps the variable.
- **An admission check before every spawn.** `admit_child` runs before `DosExecPgm`, `WinStartApp` and CMD.EXE external commands start a child. It refuses when the process is already at the depth limit, or already keeps `MAX_LIVE_CHILDREN` (16) children: asynchronous children not yet waited for, plus started applications.
- **Refusals use documented error paths:**
  - `DosExecPgm` returns `ERROR_NO_PROC_SLOTS` (89).
  - `WinStartApp` fails with `PMERR_CANNOT_START`.
  - CMD.EXE prints a message.
- **Samples with a child mode** read their command line through `DosGetInfoBlocks` (`pib_pchcmd`), not `argv`. Tests that start children run under a timeout and check that no Warpine process is left.

## Consequences

- A runaway program now stops at 8 levels and 16 children per process: at most about 8 × 16 processes in the worst tree instead of unbounded. `samples/nest_test` and an integration test check the limit.
- The limits and the choice of error codes are Warpine's own. OS/2 limits processes by its process table, not by nesting depth. A real program needing deeper nesting would need the limits raised (they are named constants).
- Processes started outside Warpine's control (another host program running Warpine) start again at depth 0. The guard protects against guest programs, not against the host user.
