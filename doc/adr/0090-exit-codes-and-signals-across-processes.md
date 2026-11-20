# ADR-0090: Whole exit codes and signals across a session's processes

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - IBM's *Control Program Programming Reference*: `DosExit` (`ulResult`, a ULONG), `DosExecPgm` / `DosWaitChild` (`RESULTCODES.codeResult`), `DosSendSignalException`.
  - *PM Programming Reference*: `WM_APPTERMINATENOTIFY`.
  - The Linux man pages: `pipe2`, `pidfd_open`, `pidfd_send_signal`, `proc(5)` (field 22 of `/proc/<pid>/stat`).
  - The code: `src/loader/exit_report.rs` (new), `inherit.rs`, `process.rs`, `managers.rs`, `pm_startapp.rs`, `cmd/external.rs`, `signals.rs`, `host_signal.rs`, `session_threads.rs`, the exit paths in `vcpu.rs`, `ne_exec.rs`, `cmd.rs` and `main.rs`.
  - The sample: `samples/proc_test`.
  - ADR-0020 (one host process per OS/2 process), ADR-0036 (session-unique process ids), ADR-0034 (ending a process from outside).

## Context

Every OS/2 process is a host process of its own (ADR-0020), and that causes two gaps:

1. **Exit codes.** A parent learnt a child's exit code from the host exit status, which carries 8 bits. `DosExit` takes a ULONG, and `RESULTCODES.codeResult`, `WM_APPTERMINATENOTIFY` and CMD's ERRORLEVEL returned only its low byte.
2. **Signals.** `DosSendSignalException` reached only the process itself and its own children, the only processes whose host pids it knew. A child could not signal its parent, nor one program another. The session's process ids are unique (ADR-0036), and each live process already has a record in the session directory (`p-<host pid>`: host pid, start time, thread count, OS/2 id; `session_threads.rs`).

## Decision

1. **An exit-report pipe** (`exit_report.rs`):
   - **The parent.** `admit_child` makes a pipe for every child (both ends close-on-exec). The write end goes to the child with close-on-exec cleared, named in `WARPINE_EXIT_FD=<fd>:<the parent's host pid>`. After the spawn the parent keeps only the read end, in `SharedState::exit_reports`, by the child's host pid.
   - **The child.** It adopts the write end at its start, only when:
     - its host parent is the pid named;
     - the descriptor is a pipe open for writing.

     It makes the descriptor close-on-exec at once, so nothing it starts later inherits it. Just before it ends (every exit path), it writes its exit code, 4 bytes little-endian.
   - **Reaping.** Every path that reaps a child calls `SharedState::child_exit_code`, which also drops the read end:
     - `EXEC_SYNC`;
     - `DosWaitChild` for one child or any (`ProcessManager::wait_any` now returns the host pid and status);
     - `WinStartApp`'s watcher;
     - CMD's `wait_child`, and the `DETACH` reaper.

     The report counts only when it is 4 bytes and its low 8 bits equal the host status; otherwise, as for a child killed before it reported, the host status stands.
2. **Signals by session record.** For a pid that is neither the process itself nor one of its children, `DosSendSignalException` looks up the session's live `p-` record with that OS/2 id (`SessionTables::host_of`; reservations are no process yet). It then sends through a pidfd, after checking that the process is still the recorded one by its start time (`host_signal::send_to_identity`), so a reused host pid is never signalled. Without a session directory, only the process itself and its children can be reached, as before.

## Warpine's choices (unverified)

- The whole ULONG is returned, while 16-bit `DosCwait` callers see USHORTs.
- Any process of the session may signal any other: the parent, siblings, unrelated programs. The same focus rules apply at the receiver.
- A report that disagrees with the host status is ignored.

## Consequences

- **Programs and scripts** see whole exit codes: ERRORLEVEL above 255, codes of `DosExit(EXIT_PROCESS, 0x12345)`.
- **Signals between programs** work: a child interrupting its parent, one program signalling another by the id it was given.
- **Cost:** one pipe per child while it runs. Every reaping path closes it, so no descriptor outlives the child's reaping.
- **Safety:**
  - the environment value is parsed strictly and believed only by a direct child, for a writable pipe;
  - the report is a bounded 4-byte read;
  - signals go by pidfd after the start-time check.
- **Tests:**
  - unit tests in `exit_report.rs` (parsing, the report's use and fallbacks, only a writable pipe believed) and `session_threads.rs` (`a_session_process_is_found_by_its_os2_id`);
  - `tests/integration.rs` `test_whole_exit_codes_and_signals_between_processes`: `proc_test`, 19 checks.
- **The TODO "`DosWaitChild` after `DosKillProcess`"** was already covered by `kill_test`, which checks `TC_KILLPROCESS` after a kill; it is removed.
